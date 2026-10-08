// Entry point. `--render-icon DIR` writes the icon PNGs and exits (used by
// make-app.sh); otherwise run the Dock app.
#import <AppKit/AppKit.h>
#import "App.h"
#import "Icon.h"

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        if (argc >= 3 && strcmp(argv[1], "--render-icon") == 0) {
            NSError *err;
            if (!writeIconSet([NSString stringWithUTF8String:argv[2]], &err)) {
                fprintf(stderr, "render-icon: %s\n", err.localizedDescription.UTF8String);
                return 1;
            }
            return 0;
        }
        NSApplication *app = [NSApplication sharedApplication];
        AppDelegate *delegate = [AppDelegate new];
        if (argc >= 3 && strcmp(argv[1], "--snapshot") == 0) delegate.snapshotPath = [NSString stringWithUTF8String:argv[2]];
        app.delegate = delegate;
        [app setActivationPolicy:NSApplicationActivationPolicyRegular];   // a Dock icon and a window
        [app run];
    }
    return 0;
}
