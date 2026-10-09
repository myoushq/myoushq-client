// myous for Mac: a menu bar item and a window that show every myous worker
// on this Mac, start, stop and pause it, hand its pairing code to an
// agent, and list what the agent runs. Everything is local: files in
// ~/.myous-worker and `docker compose` (or `myous worker` in direct mode)
// in a child process; the only network use is the daily release check.
#import <AppKit/AppKit.h>
@interface AppDelegate : NSObject <NSApplicationDelegate>
/// `--snapshot PATH`: render the window to a PNG and exit (for checking the
/// layout without Screen Recording permission). MYOUS_FAKE_STATE=setup |
/// noruntime | starting | pair | paired | running | paused | stopped shows
/// that screen with made-up data.
@property (nonatomic, copy) NSString *snapshotPath;
@end
