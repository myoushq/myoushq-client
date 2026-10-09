#import "Status.h"

const NSTimeInterval kStaleAfter = 15;

NSString *str(id v) { return [v isKindOfClass:[NSString class]] ? v : nil; }
NSNumber *num(id v) { return [v isKindOfClass:[NSNumber class]] ? v : nil; }
NSDictionary *dict(id v) { return [v isKindOfClass:[NSDictionary class]] ? v : nil; }

@implementation Paths
+ (NSString *)home {
    NSString *env = [[NSProcessInfo processInfo] environment][@"MYOUS_WORKER_HOME"];
    return env.length ? [env stringByExpandingTildeInPath] : [NSHomeDirectory() stringByAppendingPathComponent:@".myous-worker"];
}
+ (NSString *)status { return [[self home] stringByAppendingPathComponent:@"worker.json"]; }
+ (NSString *)config { return [[self home] stringByAppendingPathComponent:@"app.json"]; }
+ (NSString *)paused { return [[self home] stringByAppendingPathComponent:@"worker.paused"]; }
+ (NSString *)log { return [[self home] stringByAppendingPathComponent:@"worker.log"]; }
+ (NSString *)work { return [[self home] stringByAppendingPathComponent:@"work"]; }
+ (NSString *)commands { return [[self home] stringByAppendingPathComponent:@"commands"]; }
+ (NSString *)requests { return [[self home] stringByAppendingPathComponent:@"requests"]; }
+ (NSString *)approvals { return [[self home] stringByAppendingPathComponent:@"approvals"]; }
+ (NSString *)review { return [[self home] stringByAppendingPathComponent:@"review.json"]; }
+ (NSString *)bundledReview { return [[NSBundle mainBundle] pathForResource:@"review" ofType:@"py"]; }
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
- (NSString *)phase { return str(self.status[@"phase"]); }
@end

@implementation AppConfig
+ (instancetype)read {
    AppConfig *c = [AppConfig new];
    c.notifications = YES;
    c.autoUpdate = YES;
    c.showAgents = YES;
    NSData *data = [NSData dataWithContentsOfFile:[Paths config]];
    if (!data) return c;
    NSDictionary *d = dict([NSJSONSerialization JSONObjectWithData:data options:0 error:nil]);
    if (!d) return c;
    c.repo = str(d[@"repo"]);
    c.mode = str(d[@"mode"]);
    c.name = str(d[@"name"]);
    if (num(d[@"dock"])) c.dock = num(d[@"dock"]).boolValue;
    if (num(d[@"notifications"])) c.notifications = num(d[@"notifications"]).boolValue;
    if (num(d[@"auto_update"])) c.autoUpdate = num(d[@"auto_update"]).boolValue;
    c.seenPairedAt = num(d[@"seen_paired_at"]).doubleValue;
    c.seenRequestsAt = num(d[@"seen_requests_at"]).doubleValue;
    c.lastUpdateCheck = num(d[@"last_update_check"]).doubleValue;
    c.skippedVersion = str(d[@"skipped_version"]);
    if (num(d[@"show_agents"])) c.showAgents = num(d[@"show_agents"]).boolValue;
    NSMutableArray *homes = [NSMutableArray new];
    for (id h in [d[@"agent_homes"] isKindOfClass:[NSArray class]] ? d[@"agent_homes"] : @[]) if (str(h)) [homes addObject:h];
    c.agentHomes = homes;
    return c;
}
- (void)write {
    [[NSFileManager defaultManager] createDirectoryAtPath:[Paths home] withIntermediateDirectories:YES
                                               attributes:@{NSFilePosixPermissions: @0700} error:nil];
    NSMutableDictionary *d = [NSMutableDictionary new];
    if (self.repo) d[@"repo"] = self.repo;
    if (self.mode) d[@"mode"] = self.mode;
    if (self.name) d[@"name"] = self.name;
    d[@"dock"] = @(self.dock);
    d[@"notifications"] = @(self.notifications);
    d[@"auto_update"] = @(self.autoUpdate);
    if (self.seenPairedAt) d[@"seen_paired_at"] = @(self.seenPairedAt);
    if (self.seenRequestsAt) d[@"seen_requests_at"] = @(self.seenRequestsAt);
    if (self.lastUpdateCheck) d[@"last_update_check"] = @(self.lastUpdateCheck);
    if (self.skippedVersion) d[@"skipped_version"] = self.skippedVersion;
    d[@"show_agents"] = @(self.showAgents);
    if (self.agentHomes.count) d[@"agent_homes"] = self.agentHomes;
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

NSArray<NSDictionary *> *loadRequests(NSUInteger limit) {
    NSFileManager *fm = [NSFileManager defaultManager];
    NSArray *names = [fm contentsOfDirectoryAtPath:[Paths requests] error:nil];
    NSMutableArray *out = [NSMutableArray new];
    for (NSString *name in names) {
        if (![name hasSuffix:@".json"]) continue;
        NSData *data = [NSData dataWithContentsOfFile:[[Paths requests] stringByAppendingPathComponent:name]];
        NSDictionary *d = data ? dict([NSJSONSerialization JSONObjectWithData:data options:0 error:nil]) : nil;
        if (d) [out addObject:d];
    }
    [out sortUsingComparator:^NSComparisonResult(NSDictionary *a, NSDictionary *b) {
        double ta = num(a[@"at"]).doubleValue, tb = num(b[@"at"]).doubleValue;
        return ta > tb ? NSOrderedAscending : ta < tb ? NSOrderedDescending : NSOrderedSame;
    }];
    if (out.count > limit) [out removeObjectsInRange:NSMakeRange(limit, out.count - limit)];
    return out;
}

void sendWorkerCommand(NSString *name) {
    NSFileManager *fm = [NSFileManager defaultManager];
    [fm createDirectoryAtPath:[Paths commands] withIntermediateDirectories:YES attributes:nil error:nil];
    [fm createFileAtPath:[[Paths commands] stringByAppendingPathComponent:name] contents:[NSData data] attributes:nil];
}

NSArray<NSDictionary *> *loadApprovals(void) {
    NSFileManager *fm = [NSFileManager defaultManager];
    NSMutableArray *out = [NSMutableArray new];
    for (NSString *name in [fm contentsOfDirectoryAtPath:[Paths approvals] error:nil]) {
        if (![name hasSuffix:@".json"]) continue;
        NSData *data = [NSData dataWithContentsOfFile:[[Paths approvals] stringByAppendingPathComponent:name]];
        NSDictionary *d = data ? dict([NSJSONSerialization JSONObjectWithData:data options:0 error:nil]) : nil;
        if (d) [out addObject:d];
    }
    [out sortUsingComparator:^NSComparisonResult(NSDictionary *a, NSDictionary *b) {
        return [num(a[@"asked_at"]) ?: @0 compare:num(b[@"asked_at"]) ?: @0];
    }];
    return out;
}

void answerApproval(NSString *rid, NSString *verdict) {
    NSString *safe = [[rid componentsSeparatedByCharactersInSet:[[NSCharacterSet alphanumericCharacterSet] invertedSet]] componentsJoinedByString:@""];
    if (!safe.length) return;
    NSString *path = [[Paths approvals] stringByAppendingPathComponent:[safe stringByAppendingString:@".answer"]];
    [[verdict dataUsingEncoding:NSUTF8StringEncoding] writeToFile:path atomically:YES];
}

NSString *reviewLevel(void) {
    NSData *data = [NSData dataWithContentsOfFile:[Paths review]];
    NSString *lv = data ? str(dict([NSJSONSerialization JSONObjectWithData:data options:0 error:nil])[@"level"]) : nil;
    return [@[@"trust", @"changes", @"all"] containsObject:lv] ? lv : @"trust";
}

void setReviewLevel(NSString *level) {
    [[NSFileManager defaultManager] createDirectoryAtPath:[Paths home] withIntermediateDirectories:YES
                                               attributes:@{NSFilePosixPermissions: @0700} error:nil];
    NSData *data = [NSJSONSerialization dataWithJSONObject:@{@"level": level} options:0 error:nil];
    [data writeToFile:[Paths review] atomically:YES];
}

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

NSString *clockTime(double unixSeconds) {
    static NSDateFormatter *f;
    if (!f) { f = [NSDateFormatter new]; f.dateFormat = @"HH:mm"; }
    return [f stringFromDate:[NSDate dateWithTimeIntervalSince1970:unixSeconds]];
}

NSString *dayLabel(double unixSeconds) {
    NSCalendar *cal = [NSCalendar currentCalendar];
    NSDate *d = [NSDate dateWithTimeIntervalSince1970:unixSeconds];
    if ([cal isDateInToday:d]) return @"Today";
    if ([cal isDateInYesterday:d]) return @"Yesterday";
    static NSDateFormatter *f;
    if (!f) { f = [NSDateFormatter new]; f.dateFormat = @"d MMM"; }
    return [f stringFromDate:d];
}
