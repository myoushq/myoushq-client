#import "Runtime.h"

@implementation Runtime

+ (NSString *)dockerBin {
    return [[NSProcessInfo processInfo] environment][@"MYOUS_DOCKER_BIN"] ?: @"docker";
}

+ (NSDictionary *)check {
    NSString *bin = [self dockerBin];
    NSString *out = @"", *composeOut = @"";
    int status = [self run:[NSString stringWithFormat:@"command -v %@ >/dev/null 2>&1 || exit 199; %@ version", bin, bin] timeout:5 output:&out];
    if (status == 199) return @{@"state": @"missing", @"detail": @""};
    NSNumber *compose = nil;
    if (status == 0) compose = @([self run:[bin stringByAppendingString:@" compose version"] timeout:5 output:&composeOut]);
    return [self judgeVersion:status output:out compose:compose output:composeOut];
}

/// The line of a command's output that says what went wrong: the last one
/// naming an error, else the last line.
static NSString *errorLine(NSString *out) {
    NSMutableArray *lines = [NSMutableArray new];
    for (NSString *l in [out componentsSeparatedByString:@"\n"]) {
        NSString *t = [l stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceCharacterSet]];
        if (t.length) [lines addObject:t];
    }
    for (NSString *l in lines.reverseObjectEnumerator) {
        NSString *low = l.lowercaseString;
        for (NSString *w in @[@"error", @"cannot", @"denied", @"unknown", @"not a docker command", @"failed"])
            if ([low containsString:w]) return l;
    }
    return lines.lastObject ?: @"";
}

+ (NSDictionary *)judgeVersion:(int)status output:(NSString *)output compose:(NSNumber *)composeStatus output:(NSString *)composeOutput {
    if (status == 124) return @{@"state": @"unknown", @"detail": @"no answer within 5 s"};
    if (status == 0) {
        if (!composeStatus || composeStatus.intValue == 0) return @{@"state": @"ok", @"detail": @""};
        if (composeStatus.intValue == 124) return @{@"state": @"unknown", @"detail": @"no answer within 5 s"};
        return @{@"state": @"nocompose", @"detail": errorLine(composeOutput ?: @"")};
    }
    NSString *t = (output ?: @"").lowercaseString;
    for (NSString *p in @[@"permission denied", @"access is denied", @"docker-users"])
        if ([t containsString:p]) return @{@"state": @"permission", @"detail": errorLine(output)};
    // Installed, but its engine (the desktop app's VM) isn't up.
    for (NSString *p in @[@"is the docker daemon running", @"daemon is not running", @"if the daemon is running", @"cannot connect to the docker daemon",
                          @"cannot connect to podman", @"unable to connect to podman", @"connection refused", @"no such file or directory"])
        if ([t containsString:p]) return @{@"state": @"stopped", @"detail": errorLine(output)};
    return @{@"state": @"error", @"detail": errorLine(output ?: @"")};
}

+ (int)run:(NSString *)command timeout:(double)seconds output:(NSString **)output {
    NSTask *t = [NSTask new];
    t.launchPath = @"/bin/sh";
    t.arguments = @[@"-lc", command];
    NSPipe *pipe = [NSPipe pipe];
    t.standardOutput = t.standardError = pipe;
    t.standardInput = [NSFileHandle fileHandleWithNullDevice];
    if (output) *output = @"";
    if (![t launchAndReturnError:nil]) return 127;
    __block NSData *data = nil;
    dispatch_semaphore_t read = dispatch_semaphore_create(0);
    NSFileHandle *h = pipe.fileHandleForReading;
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_UTILITY, 0), ^{
        data = [h readDataToEndOfFile];
        dispatch_semaphore_signal(read);
    });
    if (dispatch_semaphore_wait(read, dispatch_time(DISPATCH_TIME_NOW, (int64_t)(seconds * NSEC_PER_SEC))) != 0) {
        [t terminate];
        return 124;   // the reader ends when the pipe closes
    }
    [t waitUntilExit];
    if (output) {
        NSString *s = [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding] ?: @"";
        *output = [s stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceAndNewlineCharacterSet]];
    }
    return t.terminationStatus;
}

+ (NSString *)brew {
    // An app opened from the Finder has a short PATH: look where Homebrew installs itself.
    for (NSString *p in @[@"/opt/homebrew/bin/brew", @"/usr/local/bin/brew"])
        if ([[NSFileManager defaultManager] isExecutableFileAtPath:p]) return p;
    return nil;
}

+ (NSString *)name {
    NSString *ctx = @"";
    [self run:[[self dockerBin] stringByAppendingString:@" context show 2>/dev/null"] timeout:5 output:&ctx];
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

+ (NSString *)browserPort:(NSString *)composePrefix legacy:(BOOL)legacy {
    // The browser view is published on a port Docker picks at each start;
    // ask compose which one this worker's project got. A container from
    // before v0.6.0 (the first worker only) has no project we started: find
    // it by label, then by its old name.
    NSString *bin = [self dockerBin];
    NSString *cmd = [NSString stringWithFormat:@"%@ port worker 6080 2>/dev/null | head -1", composePrefix];
    if (legacy) cmd = [NSString stringWithFormat:
        @"p=$(%@); [ -n \"$p\" ] && echo \"$p\" || { id=$(%@ ps -q --filter label=com.myoushq.worker | head -1); id=${id:-myous-worker-worker-1}; %@ port \"$id\" 6080/tcp 2>/dev/null | head -1; }", cmd, bin, bin];
    NSString *addr = [self output:cmd];
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
