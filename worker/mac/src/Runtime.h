// The container runtime (Docker Desktop, OrbStack, Colima: anything with a
// `docker` command and compose) and the commands the app runs through it.
// Every command runs in the user's login shell so their PATH applies;
// MYOUS_DOCKER_BIN overrides the command (tests simulate a Mac without one).
#import <Foundation/Foundation.h>

typedef void (^LineBlock)(NSString *line);
typedef void (^DoneBlock)(int status);

@interface Runtime : NSObject
/// "ok", "stopped" (installed, daemon not running) or "missing". Synchronous.
+ (NSString *)probe;
/// "Docker Desktop", "OrbStack", "Colima", "Rancher Desktop" or "Docker", by
/// what is installed and the current docker context. Synchronous.
+ (NSString *)name;
/// Open the runtime's app so its daemon starts (`open -a`).
+ (void)launch:(NSString *)name;
/// Run a shell command in a directory with extra environment, streaming
/// lines (main thread) and the exit status (main thread).
+ (void)run:(NSString *)command in:(NSString *)dir env:(NSDictionary *)env line:(LineBlock)line done:(DoneBlock)done;
/// Run a command and return its trimmed stdout (synchronous, for short ones).
+ (NSString *)output:(NSString *)command;
/// The host port of a worker's browser view (`<compose prefix> port worker
/// 6080`), or nil. `legacy` also looks for a container from before v0.6.0.
+ (NSString *)browserPort:(NSString *)composePrefix legacy:(BOOL)legacy;
/// Remove any worker container, whatever compose project made it.
+ (NSString *)removeAllCommand;
+ (NSString *)dockerBin;
@end
