// What the worker writes to ~/.myous-worker (worker.json, requests/*.json,
// worker.log) and the app's own settings in app.json. The app reads files,
// drops command files for the worker, and runs local commands; it only
// touches the network to check for a new release.
#import <Foundation/Foundation.h>

/// The worker rewrites worker.json every few seconds while it runs, so a
/// file older than this is a stopped (or stuck) worker.
extern const NSTimeInterval kStaleAfter;

@interface Paths : NSObject
+ (NSString *)home;      // ~/.myous-worker
+ (NSString *)status;    // worker.json
+ (NSString *)config;    // app.json
+ (NSString *)paused;    // worker.paused
+ (NSString *)log;       // worker.log
+ (NSString *)work;      // work/
+ (NSString *)commands;  // commands/: files the worker picks up (new-code, unpair)
+ (NSString *)requests;  // requests/: one JSON record per request
+ (NSString *)bundledCompose;   // Contents/Resources/compose.yml: the published image
@end

/// worker.json, parsed leniently: every field is optional.
@interface StatusFile : NSObject
@property (nonatomic, strong) NSDictionary *status;   // nil if missing or unreadable
@property (nonatomic, strong) NSDate *modified;
+ (instancetype)read;
- (BOOL)fresh;
- (NSString *)phase;     // "starting", "browser", "registering", "running", "paused", "error: ..." or nil
@end

/// Modes: "image" runs the published container image from the bundled
/// compose file (the default when nothing is configured: a downloaded app
/// needs no checkout); "docker" runs `docker compose` in a checkout's
/// worker/; "direct" runs `myous worker` on this Mac.
@interface AppConfig : NSObject
@property (nonatomic, copy) NSString *repo;   // path to the myoushq-client checkout
@property (nonatomic, copy) NSString *mode;   // "image", "docker" or "direct"
@property (nonatomic, copy) NSString *name;   // the worker's name (its alias); nil until set up
@property (nonatomic) BOOL dock;              // show a Dock icon too (default: menu bar only)
@property (nonatomic) BOOL notifications;     // default YES
@property (nonatomic) BOOL autoUpdate;        // check for a new release daily (default YES)
@property (nonatomic) double seenPairedAt;    // the pairing the owner has acknowledged
@property (nonatomic) double seenRequestsAt;  // requests before this are not "new"
@property (nonatomic) double lastUpdateCheck;
@property (nonatomic, copy) NSString *skippedVersion;   // "Later" on an update banner
+ (instancetype)read;
- (void)write;
- (BOOL)isDirect;
- (BOOL)isImage;
- (BOOL)usesDocker;
@end

/// requests/*.json, newest first (by `at`), at most `limit`.
NSArray<NSDictionary *> *loadRequests(NSUInteger limit);

/// Drop a command file for the worker (it removes the file once done).
void sendWorkerCommand(NSString *name);

/// The app's own version (CFBundleShortVersionString), which names the image tag.
NSString *appVersion(void);

NSString *timeAgo(double unixSeconds);
NSString *clockTime(double unixSeconds);   // "21:12"
NSString *dayLabel(double unixSeconds);    // "Today", "Yesterday", "3 Oct"

NSString *str(id v);
NSNumber *num(id v);
NSDictionary *dict(id v);
