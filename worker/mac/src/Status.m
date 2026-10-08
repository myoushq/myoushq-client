#import "Status.h"

const NSTimeInterval kStaleAfter = 15;

@implementation Paths
+ (NSString *)home { return [NSHomeDirectory() stringByAppendingPathComponent:@".myous-worker"]; }
+ (NSString *)status { return [[self home] stringByAppendingPathComponent:@"worker.json"]; }
+ (NSString *)config { return [[self home] stringByAppendingPathComponent:@"app.json"]; }
+ (NSString *)paused { return [[self home] stringByAppendingPathComponent:@"worker.paused"]; }
+ (NSString *)log { return [[self home] stringByAppendingPathComponent:@"worker.log"]; }
+ (NSString *)work { return [[self home] stringByAppendingPathComponent:@"work"]; }
+ (NSString *)bundledCompose { return [[NSBundle mainBundle] pathForResource:@"compose" ofType:@"yml"]; }
@end

@implementation StatusFile
+ (instancetype)read {
    StatusFile *f = [StatusFile new];
    NSData *data = [NSData dataWithContentsOfFile:[Paths status]];
    if (!data) return f;
    NSDictionary *attrs = [[NSFileManager defaultManager] attributesOfItemAtPath:[Paths status] error:nil];
    f.modified = attrs[NSFileModificationDate];
    id parsed = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
    if ([parsed isKindOfClass:[NSDictionary class]]) f.status = parsed;
    return f;
}
- (BOOL)fresh {
    return self.modified && [[NSDate date] timeIntervalSinceDate:self.modified] < kStaleAfter;
}
@end

@implementation AppConfig
+ (instancetype)read {
    AppConfig *c = [AppConfig new];
    NSData *data = [NSData dataWithContentsOfFile:[Paths config]];
    if (!data) return c;
    id parsed = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
    if ([parsed isKindOfClass:[NSDictionary class]]) {
        if ([parsed[@"repo"] isKindOfClass:[NSString class]]) c.repo = parsed[@"repo"];
        if ([parsed[@"mode"] isKindOfClass:[NSString class]]) c.mode = parsed[@"mode"];
    }
    return c;
}
- (void)write {
    [[NSFileManager defaultManager] createDirectoryAtPath:[Paths home] withIntermediateDirectories:YES
                                               attributes:@{NSFilePosixPermissions: @0700} error:nil];
    NSMutableDictionary *d = [NSMutableDictionary new];
    if (self.repo) d[@"repo"] = self.repo;
    if (self.mode) d[@"mode"] = self.mode;
    NSData *data = [NSJSONSerialization dataWithJSONObject:d options:NSJSONWritingPrettyPrinted | NSJSONWritingSortedKeys error:nil];
    [data writeToFile:[Paths config] atomically:YES];
}
- (BOOL)isDirect { return [self.mode isEqualToString:@"direct"]; }
- (BOOL)isImage {
    if ([self.mode isEqualToString:@"image"]) return YES;
    // Nothing configured, or a checkout mode without a checkout: the image.
    return !self.isDirect && self.repo == nil;
}
- (BOOL)usesDocker { return !self.isDirect; }
@end

NSString *appVersion(void) {
    NSString *v = [[NSBundle mainBundle] objectForInfoDictionaryKey:@"CFBundleShortVersionString"];
    return [v isKindOfClass:[NSString class]] && v.length ? v : @"dev";
}

NSString *timeAgo(double unixSeconds) {
    double d = [[NSDate date] timeIntervalSince1970] - unixSeconds;
    if (d < 0) d = 0;
    if (d < 60) return [NSString stringWithFormat:@"%d s ago", (int)d];
    if (d < 3600) return [NSString stringWithFormat:@"%d min ago", (int)(d / 60)];
    if (d < 86400) return [NSString stringWithFormat:@"%d h ago", (int)(d / 3600)];
    return [NSString stringWithFormat:@"%d d ago", (int)(d / 86400)];
}
