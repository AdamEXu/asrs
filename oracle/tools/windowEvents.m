// Loaded into Electron with DYLD_INSERT_LIBRARIES: a window given the represented file
// "asrs-post <name>" posts that notification about itself, and "asrs-main" is answered with whether it
// can become the main window, so a run can have both of windows that are never shown.
#import <AppKit/AppKit.h>
#import <objc/runtime.h>

static void (*set_file)(id, SEL, NSString *);

static void hooked(NSWindow *self, SEL _cmd, NSString *file) {
  NSString *post = @"asrs-post ";
  if ([file hasPrefix:post]) {
    [[NSNotificationCenter defaultCenter] postNotificationName:[file substringFromIndex:post.length] object:self];
    return;
  }
  if ([file isEqual:@"asrs-main"]) file = [self canBecomeMainWindow] ? @"yes" : @"no";
  set_file(self, _cmd, file);
}

static void hook(void) {
  Method method = class_getInstanceMethod([NSWindow class], @selector(setRepresentedFilename:));
  set_file = (void (*)(id, SEL, NSString *))method_setImplementation(method, (IMP)hooked);
}

__attribute__((constructor)) static void install(void) {
  if (getenv("ASRS_WINDOW_EVENTS") == NULL) return;
  [[NSNotificationCenter defaultCenter] addObserverForName:NSApplicationWillFinishLaunchingNotification
                                                    object:nil
                                                     queue:nil
                                                usingBlock:^(NSNotification *note) { hook(); }];
}
