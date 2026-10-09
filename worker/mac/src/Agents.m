#import "Agents.h"
#import "Status.h"

@implementation LocalAgent
- (NSString *)shortNpub {
    if (self.npub.length < 12) return self.npub ?: @"";
    return [NSString stringWithFormat:@"%@…%@", [self.npub substringToIndex:8], [self.npub substringFromIndex:self.npub.length - 3]];
}
- (NSString *)displayName { return self.alias ?: self.home.lastPathComponent; }
@end

@implementation Agents

+ (NSArray<NSString *> *)homes:(NSArray<NSString *> *)extra {
    NSString *env = [[NSProcessInfo processInfo] environment][@"MYOUS_AGENT_HOMES"];
    NSMutableArray *out = [NSMutableArray new];
    NSFileManager *fm = [NSFileManager defaultManager];
    if (env) {
        for (NSString *h in [env componentsSeparatedByString:@":"]) if (h.length) [out addObject:[h stringByExpandingTildeInPath]];
        return out;
    }
    NSString *user = NSHomeDirectory();
    for (NSString *entry in [[fm contentsOfDirectoryAtPath:user error:nil] sortedArrayUsingSelector:@selector(compare:)]) {
        if (![entry isEqualToString:@".myous"] && ![entry hasPrefix:@".myous-"]) continue;
        if ([entry isEqualToString:@".myous-worker"] || [entry hasPrefix:@".myous-worker-"]) continue;   // this app's own
        NSString *home = [user stringByAppendingPathComponent:entry];
        if ([fm fileExistsAtPath:[home stringByAppendingPathComponent:@"key"]]) [out addObject:home];
    }
    for (NSString *h in extra) {
        NSString *home = [h stringByExpandingTildeInPath];
        if (![out containsObject:home] && [fm fileExistsAtPath:home]) [out addObject:home];
    }
    return out;
}

+ (NSString *)binaryFor:(NSString *)home {
    NSFileManager *fm = [NSFileManager defaultManager];
    for (NSString *rel in @[@"venv/bin/myous", @"bin/myous"]) {
        NSString *p = [home stringByAppendingPathComponent:rel];
        if ([fm isExecutableFileAtPath:p]) return p;
    }
    static NSString *onPath;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        NSTask *t = [NSTask new];
        t.launchPath = @"/bin/sh";
        t.arguments = @[@"-lc", @"command -v myous"];
        NSPipe *pipe = [NSPipe pipe];
        t.standardOutput = pipe;
        t.standardError = [NSPipe pipe];
        @try { [t launch]; [t waitUntilExit]; } @catch (NSException *e) { return; }
        NSString *s = [[[NSString alloc] initWithData:[pipe.fileHandleForReading readDataToEndOfFile] encoding:NSUTF8StringEncoding]
                       stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceAndNewlineCharacterSet]];
        if (t.terminationStatus == 0 && s.length) onPath = s;
    });
    return onPath;
}

+ (void)read:(NSArray<NSString *> *)homes done:(AgentsBlock)done {
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_UTILITY, 0), ^{
        NSMutableArray *agents = [NSMutableArray new];
        for (NSString *home in homes) {
            LocalAgent *a = [LocalAgent new];
            a.home = home;
            a.binary = [self binaryFor:home];
            if (!a.binary) { a.error = @"no myous client found for this folder"; [agents addObject:a]; continue; }
            int status = 0;
            NSString *out = [self runSync:@[@"status", @"--json"] home:home binary:a.binary status:&status];
            NSDictionary *d = dict([NSJSONSerialization JSONObjectWithData:[out dataUsingEncoding:NSUTF8StringEncoding] ?: [NSData data] options:0 error:nil]);
            if (status != 0 || !d) {
                a.error = [NSString stringWithFormat:@"the client didn't answer (%@)", out.length ? [out componentsSeparatedByString:@"\n"].lastObject : @"no output"];
                [agents addObject:a];
                continue;
            }
            a.alias = str(d[@"alias"]);
            a.npub = str(d[@"identity"]);
            a.client = str(d[@"client"]);
            a.version = str(d[@"version"]);
            NSMutableArray *contacts = [NSMutableArray new];
            for (id c in [d[@"contact_list"] isKindOfClass:[NSArray class]] ? d[@"contact_list"] : @[]) if (dict(c)) [contacts addObject:c];
            a.contacts = contacts;
            NSMutableArray *pending = [NSMutableArray new];
            for (id p in [d[@"pending_pairings"] isKindOfClass:[NSArray class]] ? d[@"pending_pairings"] : @[]) if (str(p)) [pending addObject:p];
            a.pending = pending;
            a.unread = num(d[@"unread"]).integerValue;
            a.lastUsed = num(d[@"last_used"]).doubleValue;
            [agents addObject:a];
        }
        dispatch_async(dispatch_get_main_queue(), ^{ done(agents); });
    });
}

+ (void)run:(NSArray<NSString *> *)args as:(LocalAgent *)agent done:(CommandBlock)done {
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        int status = 0;
        NSString *out = [self runSync:args home:agent.home binary:agent.binary status:&status];
        dispatch_async(dispatch_get_main_queue(), ^{ done(status, out); });
    });
}

+ (NSString *)runSync:(NSArray<NSString *> *)args home:(NSString *)home binary:(NSString *)binary status:(int *)status {
    NSTask *t = [NSTask new];
    t.launchPath = binary;
    t.arguments = args;
    t.currentDirectoryPath = home;
    NSMutableDictionary *env = [[[NSProcessInfo processInfo] environment] mutableCopy];
    env[@"MYOUS_HOME"] = home;
    t.environment = env;
    NSPipe *pipe = [NSPipe pipe];
    t.standardOutput = pipe;
    t.standardError = pipe;
    @try { [t launch]; } @catch (NSException *e) { *status = 127; return e.reason ?: @"could not run the client"; }
    NSData *data = [pipe.fileHandleForReading readDataToEndOfFile];
    [t waitUntilExit];
    *status = t.terminationStatus;
    return [[[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding] ?: @"" stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceAndNewlineCharacterSet]];
}

@end
