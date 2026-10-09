#import "Runtime.h"

@implementation Runtime

+ (NSString *)dockerBin {
    return [[NSProcessInfo processInfo] environment][@"MYOUS_DOCKER_BIN"] ?: @"docker";
}

+ (NSString *)probe {
    NSString *bin = [self dockerBin];
    NSTask *t = [NSTask new];
    t.launchPath = @"/bin/sh";
    t.arguments = @[@"-lc", [NSString stringWithFormat:
        @"command -v %@ >/dev/null 2>&1 || exit 3; %@ version >/dev/null 2>&1 && exit 0; exit 2", bin, bin]];
    t.standardOutput = t.standardError = [NSFileHandle fileHandleWithNullDevice];
    if (![t launchAndReturnError:nil]) return @"missing";
    [t waitUntilExit];
    switch (t.terminationStatus) {
        case 0: return @"ok";
        case 3: return @"missing";
        default: return @"stopped";
    }
}

+ (NSString *)name {
    NSString *ctx = [self output:[[self dockerBin] stringByAppendingString:@" context show 2>/dev/null"]];
    if ([ctx containsString:@"orbstack"]) return @"OrbStack";
    if ([ctx containsString:@"colima"]) return @"Colima";
    if ([ctx containsString:@"rancher"]) return @"Rancher Desktop";
    NSFileManager *fm = [NSFileManager defaultManager];
    if ([fm fileExistsAtPath:@"/Applications/Docker.app"]) return @"Docker Desktop";
    if ([fm fileExistsAtPath:@"/Applications/OrbStack.app"]) return @"OrbStack";
    if ([fm fileExistsAtPath:@"/Applications/Rancher Desktop.app"]) return @"Rancher Desktop";
    return @"Docker";
}

+ (void)launch:(NSString *)name {
    NSString *app = [name isEqualToString:@"Docker Desktop"] ? @"Docker" : name;
    [self output:[NSString stringWithFormat:@"open -a '%@'", app]];
}

+ (NSString *)output:(NSString *)command {
    NSTask *t = [NSTask new];
    t.launchPath = @"/bin/sh";
    t.arguments = @[@"-lc", command];
    NSPipe *out = [NSPipe pipe];
    t.standardOutput = out;
    t.standardError = [NSFileHandle fileHandleWithNullDevice];
    if (![t launchAndReturnError:nil]) return @"";
    NSData *data = [out.fileHandleForReading readDataToEndOfFile];
    [t waitUntilExit];
    NSString *s = [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding] ?: @"";
    return [s stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceAndNewlineCharacterSet]];
}

+ (NSString *)browserPort {
    // The browser view is published on a port Docker picks at each start;
    // ask Docker which one, by the label the compose files set.
    NSString *bin = [self dockerBin];
    // A container from before v0.6.0 has no label; its compose name does.
    NSString *addr = [self output:[NSString stringWithFormat:
        @"id=$(%@ ps -q --filter label=com.myoushq.worker | head -1); id=${id:-myous-worker-worker-1}; %@ port \"$id\" 6080/tcp 2>/dev/null | head -1", bin, bin]];
    NSString *port = [addr componentsSeparatedByString:@":"].lastObject;
    return port.integerValue > 0 ? port : nil;
}

+ (NSString *)removeAllCommand {
    NSString *bin = [self dockerBin];
    return [NSString stringWithFormat:@"ids=$(%@ ps -aq --filter label=com.myoushq.worker); [ -z \"$ids\" ] || %@ rm -f $ids", bin, bin];
}

+ (void)run:(NSString *)command in:(NSString *)dir env:(NSDictionary *)env line:(LineBlock)line done:(DoneBlock)done {
    NSTask *t = [NSTask new];
    t.launchPath = @"/bin/sh";
    t.arguments = @[@"-lc", command];
    if (dir) t.currentDirectoryPath = dir;
    if (env.count) {
        NSMutableDictionary *e = [[[NSProcessInfo processInfo] environment] mutableCopy];
        [e addEntriesFromDictionary:env];
        t.environment = e;
    }
    NSPipe *pipe = [NSPipe pipe];
    t.standardOutput = pipe;
    t.standardError = pipe;
    NSMutableData *rest = [NSMutableData new];
    pipe.fileHandleForReading.readabilityHandler = ^(NSFileHandle *h) {
        NSData *data = h.availableData;
        if (!data.length) return;
        [rest appendData:data];
        // Whole lines only; compose redraws progress with \r, treat it as a newline.
        NSString *text = [[NSString alloc] initWithData:rest encoding:NSUTF8StringEncoding];
        if (!text) return;
        [rest setLength:0];
        NSArray *parts = [[text stringByReplacingOccurrencesOfString:@"\r" withString:@"\n"] componentsSeparatedByString:@"\n"];
        for (NSUInteger i = 0; i + 1 < parts.count; i++) {
            NSString *l = parts[i];
            if (!l.length) continue;
            dispatch_async(dispatch_get_main_queue(), ^{ line(l); });
        }
        NSString *tail = parts.lastObject;
        if (tail.length) [rest appendData:[tail dataUsingEncoding:NSUTF8StringEncoding]];
    };
    t.terminationHandler = ^(NSTask *task) {
        pipe.fileHandleForReading.readabilityHandler = nil;
        NSString *tail = [[NSString alloc] initWithData:rest encoding:NSUTF8StringEncoding];
        dispatch_async(dispatch_get_main_queue(), ^{
            if (tail.length) line(tail);
            done(task.terminationStatus);
        });
    };
    NSError *err;
    if (![t launchAndReturnError:&err]) {
        line([NSString stringWithFormat:@"could not run: %@", err.localizedDescription]);
        done(-1);
    }
}

@end
