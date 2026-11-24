// Loaded into Electron with DYLD_INSERT_LIBRARIES: the tooltips Chromium gives AppKit are written
// to $ASRS_TOOLTIP_LOG, a JSON string a line, instead of shown. Chromium sets them only in a key
// window, which every window says it is, because the runs take no focus.
#import <AppKit/AppKit.h>
#import <objc/runtime.h>

static FILE *log_file;

static void set_tooltip(id self, SEL _cmd, NSString *text) {
  NSData *json = [NSJSONSerialization dataWithJSONObject:@[ text ?: @"" ] options:0 error:nil];
  fwrite(json.bytes, 1, json.length, log_file);
  fputc('\n', log_file);
  fflush(log_file);
}

static BOOL is_key(id self, SEL _cmd) {
  return YES;
}

static void hook(void) {
  Class view = objc_getClass("ToolTipBaseView");
  method_setImplementation(class_getInstanceMethod(view, sel_registerName("setToolTipAtMousePoint:")), (IMP)set_tooltip);
  method_setImplementation(class_getInstanceMethod([NSWindow class], @selector(isKeyWindow)), (IMP)is_key);
}

__attribute__((constructor)) static void install(void) {
  const char *path = getenv("ASRS_TOOLTIP_LOG");
  if (path == NULL) return;
  log_file = fopen(path, "a");
  // Electron's framework, which has the class, is loaded after this.
  [[NSNotificationCenter defaultCenter] addObserverForName:NSApplicationWillFinishLaunchingNotification
                                                    object:nil
                                                     queue:nil
                                                usingBlock:^(NSNotification *note) { hook(); }];
}
