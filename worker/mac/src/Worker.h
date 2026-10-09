// One worker the app runs: its folder, settings, status and what the app is
// doing with it (starting, stopping, a direct-mode child). The app keeps one
// per folder (Paths allHomes) and shows one at a time.
#import <Foundation/Foundation.h>
#import "Status.h"
#import "Browser.h"

typedef NS_ENUM(NSInteger, Screen) { ScreenSetup, ScreenNoRuntime, ScreenStarting, ScreenPair, ScreenPaired, ScreenRunning, ScreenStopping, ScreenStopped };

@interface Worker : NSObject
@property (nonatomic, strong, readonly) Paths *paths;
@property (nonatomic, strong) AppConfig *config;
@property (nonatomic, strong) StatusFile *status;
@property (nonatomic, strong) NSArray<NSDictionary *> *requests;
@property (nonatomic, strong) NSDate *requestsDirDate;
@property (nonatomic, strong) NSArray<NSDictionary *> *approvals;
@property (nonatomic) Screen screen;
@property (nonatomic, copy) NSString *launchStage;    // "pulling" or "creating" while compose up runs
@property (nonatomic) double launchedAt;              // when Start was pressed (0: not by us)
@property (nonatomic) BOOL stopping;                  // Stop pressed: a stale status is expected
@property (nonatomic) double stoppedAt;               // when Stop was pressed (0: not stopping)
@property (nonatomic) double stopDoneAt;              // when the stop command finished; a status older than this is dead
@property (nonatomic) BOOL wasRunning;
@property (nonatomic) BOOL stoppedByUs;               // the last stop was the owner's: no "stopped on its own"
@property (nonatomic, strong) NSTask *direct;         // the `myous worker` child in direct mode
@property (nonatomic, strong) MacBrowser *browser;    // the browser on this Mac, when that is the choice
@property (nonatomic) BOOL restartAfterStop;          // the browser choice changed while running
@property (nonatomic, copy) NSString *attention, *attentionButton;   // the one thing that needs the owner
@property (nonatomic) BOOL alive;
- (instancetype)initWithHome:(NSString *)home;
- (NSString *)name;      // the configured name, the worker's alias, or "Worker"
- (BOOL)isDefault;       // the first worker (its app.json holds the app's settings)
@end
