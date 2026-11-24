// Loaded into Electron with DYLD_INSERT_LIBRARIES: its general pasteboard becomes one of its own,
// so copies a run makes neither reach nor replace what the user has on theirs.
#import <AppKit/AppKit.h>
#import <objc/runtime.h>

static NSPasteboard *own;

static id general(id self, SEL _cmd) {
  return own;
}

__attribute__((constructor)) static void install(void) {
  @autoreleasepool {
    own = [[NSPasteboard pasteboardWithUniqueName] retain];
    method_setImplementation(class_getClassMethod([NSPasteboard class], @selector(generalPasteboard)), (IMP)general);
  }
}
