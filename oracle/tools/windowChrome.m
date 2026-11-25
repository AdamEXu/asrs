// Loaded into Electron with DYLD_INSERT_LIBRARIES: a window given the represented file
// "asrs-chrome <file>" writes its native properties to the file as JSON.
#import <AppKit/AppKit.h>
#import <objc/runtime.h>

static void (*set_file)(id, SEL, NSString *);

static id color(NSColor *value) {
  if (value.type == NSColorTypeCatalog) return value.colorNameComponent;
  NSColor *c = [value colorUsingColorSpace:[NSColorSpace sRGBColorSpace]];
  if (c == nil) return [NSNull null];
  return @[ @(c.redComponent), @(c.greenComponent), @(c.blueComponent), @(c.alphaComponent) ];
}

static id name(NSAppearance *value) { return value == nil ? [NSNull null] : value.name; }

static id size(NSSize value) { return @[ @(value.width), @(value.height) ]; }

// FLT_MAX or DBL_MAX where there is no limit.
static id limit(NSSize value) {
  id width = value.width < FLT_MAX ? @(value.width) : [NSNull null];
  id height = value.height < FLT_MAX ? @(value.height) : [NSNull null];
  return @[ width, height ];
}

static id button(NSWindow *window, NSWindowButton kind) {
  NSButton *b = [window standardWindowButton:kind];
  if (b == nil) return [NSNull null];
  NSRect r = [b convertRect:b.bounds toView:nil];
  return @{
    @"enabled" : @(b.isEnabled),
    @"hidden" : @(b.isHiddenOrHasHiddenAncestor),
    @"left" : @(r.origin.x),
    @"top" : @(window.frame.size.height - r.origin.y - r.size.height)
  };
}

static void effects(NSView *view, NSMutableArray *out) {
  for (NSView *child in view.subviews) {
    if ([child isKindOfClass:[NSVisualEffectView class]]) {
      NSVisualEffectView *e = (NSVisualEffectView *)child;
      [out addObject:@{
        @"material" : @(e.material),
        @"blendingMode" : @(e.blendingMode),
        @"state" : @(e.state),
        @"fills" : @(NSEqualRects(e.frame, e.window.contentView.bounds))
      }];
    } else {
      effects(child, out);
    }
  }
}

static NSDictionary *chrome(NSWindow *w) {
  NSMutableArray *vibrancy = [NSMutableArray array];
  effects(w.contentView, vibrancy);
  NSRect content = [w contentRectForFrameRect:w.frame];
  return @{
    @"title" : w.title,
    @"styleMask" : @(w.styleMask & ~NSWindowStyleMaskFullSizeContentView),
    @"opaque" : @(w.isOpaque),
    @"backgroundColor" : color(w.backgroundColor),
    @"titlebarAppearsTransparent" : @(w.titlebarAppearsTransparent),
    @"titleVisibility" : @(w.titleVisibility),
    @"titlebarSeparatorStyle" : @(w.titlebarSeparatorStyle),
    @"toolbar" : w.toolbar == nil ? @"none" : @(w.toolbarStyle),
    @"hasShadow" : @(w.hasShadow),
    @"movable" : @(w.isMovable),
    @"movableByWindowBackground" : @(w.isMovableByWindowBackground),
    @"level" : @(w.level),
    @"collectionBehavior" : @(w.collectionBehavior),
    @"hidesOnDeactivate" : @(w.hidesOnDeactivate),
    @"canHide" : @(w.canHide),
    @"panel" : @([w isKindOfClass:[NSPanel class]]),
    @"canBecomeKey" : @(w.canBecomeKeyWindow),
    @"canBecomeMain" : @(w.canBecomeMainWindow),
    @"excludedFromWindowsMenu" : @(w.isExcludedFromWindowsMenu),
    @"animationBehavior" : @(w.animationBehavior),
    @"tabbingMode" : @(w.tabbingMode),
    @"alphaValue" : @(w.alphaValue),
    @"restorable" : @(w.isRestorable),
    @"minSize" : size(w.minSize),
    @"maxSize" : limit(w.maxSize),
    @"contentMinSize" : size(w.contentMinSize),
    @"contentMaxSize" : limit(w.contentMaxSize),
    @"content" : size(content.size),
    @"effectiveAppearance" : w.effectiveAppearance.name,
    @"appAppearance" : name(NSApp.appearance),
    @"close" : button(w, NSWindowCloseButton),
    @"minimize" : button(w, NSWindowMiniaturizeButton),
    @"zoom" : button(w, NSWindowZoomButton),
    @"vibrancy" : vibrancy
  };
}

static NSWindow *looks_active;

static void answer_yes(Class class, SEL selector) {
  Method method = class_getInstanceMethod(class, selector);
  if (method == NULL) return;
  BOOL (*theirs)(id, SEL) = (BOOL (*)(id, SEL))method_getImplementation(method);
  BOOL (^ours)(id) = ^BOOL(id self) {
    BOOL forced = looks_active != nil && (self == looks_active || self == NSApp);
    return forced || theirs(self, selector);
  };
  method_setImplementation(method, imp_implementationWithBlock(ours));
}

static void look_active(NSWindow *window) {
  static dispatch_once_t once;
  dispatch_once(&once, ^{
    for (NSString *name in @[ @"isKeyWindow", @"isMainWindow", @"hasKeyAppearance", @"hasMainAppearance",
                              @"_hasActiveAppearance", @"_hasKeyAppearance", @"_hasMainAppearance", @"_hasActiveControls" ]) {
      answer_yes([NSWindow class], NSSelectorFromString(name));
    }
    answer_yes([NSApplication class], @selector(isActive));
  });
  looks_active = window;
  NSNotificationCenter *center = [NSNotificationCenter defaultCenter];
  for (NSString *name in @[ NSWindowDidBecomeKeyNotification, NSWindowDidBecomeMainNotification ]) {
    [center postNotificationName:name object:window];
  }
}

static void capture(NSWindow *w, NSString *file) {
  if (!w.isSheet) {
    [w setFrameOrigin:NSMakePoint(3000, 100)];
    [w orderWindow:NSWindowBelow relativeTo:0];
    [w setFrameOrigin:NSMakePoint(3000, 100)];
  }
  looks_active = nil;
  if ([file hasSuffix:@"-active.png"]) look_active(w);
  dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 1500 * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{
    [NSTask launchedTaskWithLaunchPath:@"/usr/sbin/screencapture"
                             arguments:@[ @"-x", @"-o", [NSString stringWithFormat:@"-l%ld", (long)w.windowNumber], file ]];
  });
}

static void hooked(NSWindow *self, SEL _cmd, NSString *file) {
  NSString *dump = @"asrs-chrome ", *grab = @"asrs-capture ";
  if ([file hasPrefix:dump]) {
    NSData *json = [NSJSONSerialization dataWithJSONObject:chrome(self) options:NSJSONWritingSortedKeys error:nil];
    [json writeToFile:[file substringFromIndex:dump.length] atomically:YES];
    return;
  }
  if ([file hasPrefix:grab]) {
    capture(self, [file substringFromIndex:grab.length]);
    return;
  }
  set_file(self, _cmd, file);
}

static void hook(void) {
  Method method = class_getInstanceMethod([NSWindow class], @selector(setRepresentedFilename:));
  set_file = (void (*)(id, SEL, NSString *))method_setImplementation(method, (IMP)hooked);
}

__attribute__((constructor)) static void install(void) {
  if (getenv("ASRS_WINDOW_CHROME") == NULL) return;
  [[NSNotificationCenter defaultCenter] addObserverForName:NSApplicationWillFinishLaunchingNotification
                                                    object:nil
                                                     queue:nil
                                                usingBlock:^(NSNotification *note) { hook(); }];
}
