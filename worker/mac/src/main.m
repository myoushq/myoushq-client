// Entry point. `--render-icon DIR` writes the icon PNGs and exits (used by
// make-app.sh); otherwise run the app.
#import <AppKit/AppKit.h>
#import "App.h"
#import "Icon.h"
#import "Browser.h"

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
        if (argc >= 3 && strcmp(argv[1], "--browser-test") == 0) {
            // Run the "browser on this Mac" launcher for a worker folder, print
            // browser.json once the port is known, keep it 20 s, quit it.
            MacBrowser *b = [[MacBrowser alloc] initWithPaths:[[Paths alloc] initWithHome:[[NSString stringWithUTF8String:argv[2]] stringByExpandingTildeInPath]]];
            b.log = ^(NSString *line) { fprintf(stderr, "%s\n", line.UTF8String); };
            b.hidden = getenv("MYOUS_BROWSER_HIDDEN") != NULL;   // MYOUS_BROWSER_HIDDEN=1: launch hidden, as the app does by default
            if (getenv("MYOUS_BROWSER_APP")) b.bundleId = [NSString stringWithUTF8String:getenv("MYOUS_BROWSER_APP")];   // a bundle id, e.g. com.microsoft.edgemac
            NSError *err;
            if (![b start:&err]) { fprintf(stderr, "browser-test: %s\n", err.localizedDescription.UTF8String); return 1; }
            NSDate *until = [NSDate dateWithTimeIntervalSinceNow:20];
            while (b.port == 0 && b.task && [until timeIntervalSinceNow] > 0) [[NSRunLoop mainRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.2]];
            NSString *json = [NSString stringWithContentsOfFile:[b.paths browserJSON] encoding:NSUTF8StringEncoding error:nil] ?: @"(no browser.json)";
            printf("%s\n", json.UTF8String);
            fflush(stdout);
            if (b.hidden) {
                [[NSRunLoop mainRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:4]];
                NSRunningApplication *app = [NSRunningApplication runningApplicationWithProcessIdentifier:b.task.processIdentifier];
                printf("hidden after 4 s: %s\n", app.isHidden ? "yes" : "no");
                fflush(stdout);
            }
            while ([until timeIntervalSinceNow] > 0) [[NSRunLoop mainRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.5]];
            [b stop];
            [[NSRunLoop mainRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:1]];
            return 0;
        }
        NSApplication *app = [NSApplication sharedApplication];
        AppDelegate *delegate = [AppDelegate new];
        if (argc >= 3 && strcmp(argv[1], "--snapshot") == 0) delegate.snapshotPath = [NSString stringWithUTF8String:argv[2]];
        app.delegate = delegate;
        [app run];
    }
    return 0;
}
