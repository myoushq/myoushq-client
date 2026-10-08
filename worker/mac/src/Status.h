// What the worker writes to ~/.myous-worker/worker.json, and the app's own
// settings in ~/.myous-worker/app.json. The app only reads files and runs
// local commands; it never talks to the network itself.
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
+ (NSString *)bundledCompose;   // Contents/Resources/compose.yml: the published image
@end

/// worker.json, parsed leniently: every field is optional.
@interface StatusFile : NSObject
@property (nonatomic, strong) NSDictionary *status;   // nil if missing or unreadable
@property (nonatomic, strong) NSDate *modified;
+ (instancetype)read;
- (BOOL)fresh;
@end

/// Modes: "image" runs the published container image from the bundled
/// compose file (the default when nothing is configured: a downloaded app
/// needs no checkout); "docker" runs `docker compose` in a checkout's
/// worker/; "direct" runs `myous worker` on this Mac.
@interface AppConfig : NSObject
@property (nonatomic, copy) NSString *repo;   // path to the myoushq-client checkout
@property (nonatomic, copy) NSString *mode;   // "image", "docker" or "direct"
+ (instancetype)read;
- (void)write;
- (BOOL)isDirect;
- (BOOL)isImage;
- (BOOL)usesDocker;
@end

/// The app's own version (CFBundleShortVersionString), which names the image tag.
NSString *appVersion(void);

NSString *timeAgo(double unixSeconds);
