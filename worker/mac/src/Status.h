// What the worker writes to ~/.myous-worker (worker.json, requests/*.json,
// worker.log) and the app's own settings in app.json. The app reads files,
// drops command files for the worker, and runs local commands; it only
// touches the network to check for a new release.
#import <Foundation/Foundation.h>

/// The worker rewrites worker.json every few seconds while it runs, so a
/// file older than this is a stopped (or stuck) worker.
extern const NSTimeInterval kStaleAfter;

/// One worker's folder: ~/.myous-worker for the first worker (or
/// MYOUS_WORKER_HOME), ~/.myous-worker-<n> for the others.
@interface Paths : NSObject
@property (nonatomic, copy, readonly) NSString *home;
- (instancetype)initWithHome:(NSString *)home;
+ (NSString *)defaultHome;              // the first worker's folder
+ (NSArray<NSString *> *)allHomes;      // the default plus every ~/.myous-worker-* that exists
+ (NSString *)newHome;                  // the next free ~/.myous-worker-<n>
- (NSString *)project;   // the compose project name: myous-worker, or myous-worker-<n>
- (NSString *)status;    // worker.json
- (NSString *)config;    // app.json
- (NSString *)paused;    // worker.paused
- (NSString *)log;       // worker.log
- (NSString *)work;      // work/
- (NSString *)commands;  // commands/: files the worker picks up (new-code, unpair)
- (NSString *)requests;  // requests/: one JSON record per request
- (NSString *)approvals; // approvals/: questions from the review hook, answers from the app
- (NSString *)review;    // review.json: {"level": "trust" | "changes" | "all"}
- (NSString *)browserDir;       // browser/: the profile of the browser on this Mac
- (NSString *)downloads;        // downloads/: where that browser saves files
- (NSString *)browserJSON;      // browser.json: {"host", "port", ...} for the container's forwarder
- (NSString *)browserSeatbelt;  // browser.sb: the sandbox profile it runs under
- (NSString *)browserLog;       // browser.log: its output
+ (NSString *)bundledReview;    // Contents/Resources/review.py, for direct mode
+ (NSString *)bundledCompose;   // Contents/Resources/compose.yml: the published image
@end

/// worker.json, parsed leniently: every field is optional.
@interface StatusFile : NSObject
@property (nonatomic, strong) NSDictionary *status;   // nil if missing or unreadable
@property (nonatomic, strong) NSDate *modified;
+ (instancetype)readAt:(Paths *)paths;
- (BOOL)fresh;
- (NSString *)phase;     // "starting", "browser", "registering", "running", "paused", "error: ..." or nil
@end

/// app.json in a worker's folder: the worker's own settings (repo, mode,
/// name, what the owner has seen) and, in the first worker's folder, the
/// app's settings (dock, notifications, updates, agents).
/// Modes: "image" runs the published container image from the bundled
/// compose file (the default when nothing is configured: a downloaded app
/// needs no checkout); "docker" runs `docker compose` in a checkout's
/// worker/; "direct" runs `myous worker` on this Mac.
@interface AppConfig : NSObject
@property (nonatomic, strong) Paths *paths;
@property (nonatomic, copy) NSString *repo;   // path to the myoushq-client checkout
@property (nonatomic, copy) NSString *mode;   // "image", "docker" or "direct"
@property (nonatomic, copy) NSString *name;   // the worker's name (its alias); nil until set up
@property (nonatomic, copy) NSString *browser; // "container" (default) or "mac": where the worker's browser runs
@property (nonatomic, copy) NSString *browserApp;   // bundle id of the Mac browser to run (nil: the first installed)
@property (nonatomic) BOOL browserHidden;      // the Mac browser starts hidden, until "Show browser" (default YES)
@property (nonatomic) BOOL dock;              // show a Dock icon too (default: menu bar only)
@property (nonatomic) BOOL notifications;     // default YES
@property (nonatomic, strong) NSDictionary *notifyKinds;   // "paired", "approval", "refused", "stopped", "update" -> BOOL (missing: YES)
- (BOOL)notifies:(NSString *)kind;
@property (nonatomic) BOOL autoUpdate;        // check for a new release daily (default YES)
@property (nonatomic) double seenPairedAt;    // the pairing the owner has acknowledged
@property (nonatomic) double seenRequestsAt;  // requests before this are not "new"
@property (nonatomic) double lastUpdateCheck;
@property (nonatomic, copy) NSString *skippedVersion;   // "Later" on an update banner
@property (nonatomic) BOOL showAgents;        // list the agents on this Mac (default YES)
@property (nonatomic, strong) NSArray<NSString *> *agentHomes;   // folders the owner added beyond ~/.myous*
+ (instancetype)readAt:(Paths *)paths;
- (void)write;
- (BOOL)isDirect;
- (BOOL)isImage;
- (BOOL)usesDocker;
- (BOOL)macBrowser;   // a container worker whose browser runs on this Mac
@end

/// requests/*.json, newest first (by `at`), at most `limit`.
NSArray<NSDictionary *> *loadRequests(Paths *paths, NSUInteger limit);

/// Drop a command file for the worker (it removes the file once done).
void sendWorkerCommand(Paths *paths, NSString *name);

/// approvals/*.json: requests the review hook is waiting on, oldest first.
NSArray<NSDictionary *> *loadApprovals(Paths *paths);
/// Answer a question: "allow" or "refuse" into approvals/<id>.answer.
void answerApproval(Paths *paths, NSString *rid, NSString *verdict);
/// The review level ("trust", "changes", "all"); missing file means "trust".
NSString *reviewLevel(Paths *paths);
void setReviewLevel(Paths *paths, NSString *level);

/// The app's own version (CFBundleShortVersionString), which names the image tag.
NSString *appVersion(void);

NSString *timeAgo(double unixSeconds);
NSString *clockTime(double unixSeconds);   // "21:12"
NSString *dayLabel(double unixSeconds);    // "Today", "Yesterday", "3 Oct"

NSString *str(id v);
NSNumber *num(id v);
NSDictionary *dict(id v);
