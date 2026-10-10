// "Browser: on this Mac". A real browser on the Mac (Google Chrome, Edge,
// Brave or Chromium, whichever is installed) run by the app for one
// worker, with the worker's own profile folder and nothing of the owner's:
// never the owner's everyday profile. It runs under a seatbelt sandbox
// profile (sandbox-exec) that keeps it to its folder: no reading or
// writing elsewhere under the home folder (not even the worker's key,
// next door), no mounted volumes, no programs outside its own bundle, no
// connections to this Mac's localhost. macOS allows no sandbox inside a
// sandbox, so the browser's own helper sandbox is off (--no-sandbox), as
// in the container, where the container is the sandbox.
//
// It asks the browser for a DevTools port (--remote-debugging-port=0,
// read back from DevToolsActivePort) and writes it to browser.json in the
// worker folder, where the container's forward.py relays 127.0.0.1:9222
// to it. If the browser exits while wanted, it is started again.
#import <Foundation/Foundation.h>
#import "Status.h"

@interface MacBrowser : NSObject
@property (nonatomic, strong, readonly) Paths *paths;
@property (nonatomic, strong) NSTask *task;           // nil when not running
@property (nonatomic) int port;                       // 0 until the browser reported it
@property (nonatomic, copy) NSString *appName;        // "Google Chrome"
@property (nonatomic) BOOL wanted;                    // restart it when it exits
@property (nonatomic, copy) void (^log)(NSString *line);
@property (nonatomic, copy) NSString *workerName;     // for the profile name and start page
@property (nonatomic) BOOL hidden;                    // launch hidden (Cmd+H), until "Show browser"
@property (nonatomic) BOOL shown;                     // the owner asked for it since this launch: stop hiding
/// The first installed browser the app can run: {"id", "name", "exe",
/// "bundle"}, or nil when none of Chrome, Edge, Brave, Chromium is there.
+ (NSDictionary *)find;
/// Whether this macOS can confine the browser (sandbox-exec is there and
/// accepts a profile). Without it the choice is disabled: the app never
/// runs the Mac browser unconfined.
+ (BOOL)canSandbox;
/// Why "On this Mac" can't be offered (no browser, no sandbox), or nil when it can.
+ (NSString *)unavailableReason;
- (instancetype)initWithPaths:(Paths *)paths;
/// Launch it (the port follows within a few seconds). NO with an error
/// when no browser is installed or it could not be launched.
- (BOOL)start:(NSError **)error;
/// Quit it and forget the port.
- (void)stop;
/// Bring its windows to the front (unhiding it if hidden).
- (void)activate;
/// Hide it (Cmd+H); it keeps working.
- (void)hide;
- (BOOL)running;
/// The seatbelt profile text for this worker and browser (for the log and tests).
- (NSString *)seatbeltFor:(NSDictionary *)browser;
@end
