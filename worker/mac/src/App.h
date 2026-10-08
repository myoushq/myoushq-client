// One window that mirrors ~/.myous-worker/worker.json and starts, stops and
// pauses the worker. Everything is local: files in ~/.myous-worker and
// `docker compose` (or `myous worker` in direct mode) in a child process.
#import <AppKit/AppKit.h>
@interface AppDelegate : NSObject <NSApplicationDelegate>
/// `--snapshot PATH`: render the window to a PNG and exit (for checking the
/// layout without Screen Recording permission).
@property (nonatomic, copy) NSString *snapshotPath;
@end
