#import "Browser.h"
#import <AppKit/AppKit.h>
#import <sys/socket.h>
#import <netinet/in.h>
#import <arpa/inet.h>

/// A free loopback port, chosen here rather than by Chrome: with
/// --remote-debugging-port=0 Chrome takes itself for a test harness and
/// marks every page as automated (navigator.webdriver), which is the very
/// thing the browser on this Mac is meant to avoid.
static int freePort(void) {
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) return 0;
    struct sockaddr_in a = {0};
    a.sin_family = AF_INET;
    a.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    a.sin_port = 0;
    socklen_t len = sizeof a;
    int port = 0;
    if (bind(fd, (struct sockaddr *)&a, sizeof a) == 0 && getsockname(fd, (struct sockaddr *)&a, &len) == 0) port = ntohs(a.sin_port);
    close(fd);
    return port;
}

@implementation MacBrowser

- (instancetype)initWithPaths:(Paths *)paths {
    if ((self = [super init])) _paths = paths;
    return self;
}

+ (NSDictionary *)find {
    for (NSString *bid in @[@"com.google.Chrome", @"com.microsoft.edgemac", @"com.brave.Browser", @"org.chromium.Chromium"]) {
        NSURL *u = [[NSWorkspace sharedWorkspace] URLForApplicationWithBundleIdentifier:bid];
        if (!u) continue;
        NSBundle *b = [NSBundle bundleWithURL:u];
        NSString *exe = b.executableURL.path;
        if (!exe) continue;
        NSString *name = [b objectForInfoDictionaryKey:@"CFBundleDisplayName"] ?: [b objectForInfoDictionaryKey:@"CFBundleName"]
            ?: [u.lastPathComponent stringByDeletingPathExtension];
        return @{@"id": bid, @"name": name, @"exe": exe, @"bundle": u.path};
    }
    return nil;
}

+ (BOOL)canSandbox {
    static BOOL ok;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        NSTask *t = [NSTask new];
        t.launchPath = @"/usr/bin/sandbox-exec";
        t.arguments = @[@"-p", @"(version 1)(allow default)(deny network-outbound (remote ip \"localhost:*\"))", @"/usr/bin/true"];
        t.standardOutput = t.standardError = [NSPipe pipe];
        @try { [t launch]; [t waitUntilExit]; ok = t.terminationStatus == 0; } @catch (NSException *e) { ok = NO; }
    });
    return ok;
}

+ (NSString *)unavailableReason {
    if (![self find]) return @"no Chrome, Edge, Brave or Chromium found";
    if (![self canSandbox]) return @"this macOS can't sandbox it";
    return nil;
}

- (NSString *)seatbeltFor:(NSDictionary *)browser {
    NSString *home = NSHomeDirectory();
    return [NSString stringWithFormat:
        @"(version 1)\n"
        ";; myous: this worker's browser may use its own folder and the network, nothing else of this Mac.\n"
        "(allow default)\n"
        ";; The owner's files: nothing, except the browser's own folder and downloads.\n"
        "(deny file-read* file-write* (subpath \"%@\"))\n"
        "(allow file-read-metadata (subpath \"%@\"))\n"
        "(allow file-read* file-write* (subpath \"%@\"))\n"
        "(allow file-read* file-write* (subpath \"%@\"))\n"
        "(allow file-read* (subpath \"%@/Library/Fonts\"))\n"
        "(allow file-read* (literal \"%@/Library/Preferences/.GlobalPreferences.plist\"))\n"
        "(allow file-read* (literal \"%@/Library/Preferences/%@.plist\"))\n"
        ";; Other disks: no.\n"
        "(deny file-read* file-write* (subpath \"/Volumes\"))\n"
        ";; Programs: only the browser's own.\n"
        "(deny process-exec*)\n"
        "(allow process-exec* (subpath \"%@\"))\n"
        ";; Nothing on this Mac's local ports (the owner's services, Docker, dev servers).\n"
        "(deny network-outbound (remote ip \"localhost:*\"))\n",
        home, home, [self.paths browserDir], [self.paths downloads], home, home, home, browser[@"id"], browser[@"bundle"]];
}

- (void)say:(NSString *)line { if (self.log) self.log([@"browser: " stringByAppendingString:line]); }

- (NSString *)startPage {
    // The page the browser opens with and its home page: says whose browser
    // this is, so the window is never mistaken for the owner's own.
    NSString *name = self.workerName ?: @"the worker";
    NSString *esc = [[[name stringByReplacingOccurrencesOfString:@"&" withString:@"&amp;"] stringByReplacingOccurrencesOfString:@"<" withString:@"&lt;"] stringByReplacingOccurrencesOfString:@">" withString:@"&gt;"];
    NSString *html = [NSString stringWithFormat:
        @"<!doctype html><meta charset=utf-8><title>myous · %@</title>"
        "<style>body{font:15px/1.5 -apple-system,system-ui,sans-serif;color:#222;background:#f4f1ea;margin:0;min-height:100vh;display:flex;flex-direction:column}"
        "main{max-width:36em;margin:0 auto;padding:2em 2em 3em}h1{font-size:1.4em;margin:0 0 .5em}h1 b{color:#1f7a6d}p{margin:.5em 0}small{color:#666}"
        // A speech bubble at the top right, in the flow so the text starts
        // below it; its tail points up at the bar Chrome shows above the
        // page (the bar's × sits at its right end).
        ".bubble{position:relative;align-self:flex-end;margin:22px 14px 0 14px;max-width:24em;background:#fff6dc;border:1px solid #d9b25a;border-radius:12px;padding:.7em 1em;box-shadow:0 2px 8px rgba(0,0,0,.08)}"
        ".bubble:before,.bubble:after{content:'';position:absolute;right:22px;border:12px solid transparent;border-top:0}"
        ".bubble:before{top:-13px;border-bottom-color:#d9b25a}.bubble:after{top:-11px;border-bottom-color:#fff6dc}"
        ".bubble b{color:#7a5a00}kbd{font:inherit;white-space:nowrap;background:#eee;border:1px solid #ccc;border-radius:4px;padding:0 .3em}</style>"
        "<div class=bubble><b>That bar is expected.</b> Close it with its &times;. It says <kbd>--no-sandbox</kbd> is unsupported: "
        "myous confines this browser with a macOS sandbox of its own, and macOS allows no sandbox inside another, so the browser's built-in one is off here. The outer one does the work.</div>"
        "<main><h1><b>myous</b> · %@'s browser</h1>"
        "<p>This window belongs to the worker <b>%@</b>: your agent browses here, and so can you. "
        "Sites you log into here stay logged in for it.</p>"
        "<p>It is kept to the worker's folder by that sandbox: it can't read your files, reach other programs on this Mac, or save anywhere but the worker's downloads folder.</p>"
        "<p><small>Your own browser and its logins are untouched. Close this window when you like; myous starts it again while the worker runs.</small></p></main>",
        esc, esc, esc];
    NSString *path = [[self.paths browserDir] stringByAppendingPathComponent:@"start.html"];
    [[html dataUsingEncoding:NSUTF8StringEncoding] writeToFile:path atomically:YES];
    return [[NSURL fileURLWithPath:path] absoluteString];
}

- (void)seedPreferences:(NSString *)startURL {
    // Set once, before the profile exists; afterwards the owner's own
    // choices in the browser's settings stand. Downloads into the worker
    // folder (inside the sandbox, and mounted in the container); a profile
    // name, avatar and toolbar colour that say "myous", so the window is
    // told apart from the owner's own browser; the start page as home.
    NSString *def = [[self.paths browserDir] stringByAppendingPathComponent:@"Default"];
    NSString *prefs = [def stringByAppendingPathComponent:@"Preferences"];
    NSFileManager *fm = [NSFileManager defaultManager];
    if ([fm fileExistsAtPath:prefs]) return;
    [fm createDirectoryAtPath:def withIntermediateDirectories:YES attributes:@{NSFilePosixPermissions: @0700} error:nil];
    NSDictionary *p = @{@"download": @{@"default_directory": [self.paths downloads], @"prompt_for_download": @NO},
                        @"savefile": @{@"default_directory": [self.paths downloads]},
                        @"profile": @{@"name": [NSString stringWithFormat:@"%@ · myous", self.workerName ?: @"Worker"], @"using_default_name": @NO,
                                      @"avatar_index": @26, @"using_default_avatar": @NO},
                        @"browser": @{@"theme": @{@"user_color": @((int32_t)0xFF1F7A6DU), @"color_variant": @1}, @"show_home_button": @YES},
                        @"homepage": startURL, @"homepage_is_newtabpage": @NO};
    [[NSJSONSerialization dataWithJSONObject:p options:0 error:nil] writeToFile:prefs atomically:YES];
}

- (BOOL)start:(NSError **)error {
    NSDictionary *b = [MacBrowser find];
    NSString *why = [MacBrowser unavailableReason];
    if (why) {
        if (error) *error = [NSError errorWithDomain:@"myous" code:1 userInfo:@{NSLocalizedDescriptionKey:
            [NSString stringWithFormat:@"Can't run the browser on this Mac: %@. Install Google Chrome (or Edge, Brave, Chromium), or choose the browser in the container.", why]}];
        return NO;
    }
    self.appName = b[@"name"];
    self.wanted = YES;
    self.shown = NO;
    NSFileManager *fm = [NSFileManager defaultManager];
    for (NSString *dir in @[[self.paths browserDir], [self.paths downloads]])
        [fm createDirectoryAtPath:dir withIntermediateDirectories:YES attributes:@{NSFilePosixPermissions: @0700} error:nil];
    NSString *startURL = [self startPage];
    [self seedPreferences:startURL];
    [[[self seatbeltFor:b] dataUsingEncoding:NSUTF8StringEncoding] writeToFile:[self.paths browserSeatbelt] atomically:YES];
    [fm removeItemAtPath:[self.paths browserJSON] error:nil];
    self.port = 0;
    int port = freePort();
    if (!port) {
        if (error) *error = [NSError errorWithDomain:@"myous" code:2 userInfo:@{NSLocalizedDescriptionKey: @"Couldn't find a free port for the browser."}];
        return NO;
    }

    NSTask *t = [NSTask new];
    t.launchPath = @"/usr/bin/sandbox-exec";
    t.arguments = @[@"-f", [self.paths browserSeatbelt], b[@"exe"],
                    @"--no-sandbox",             // no sandbox inside a sandbox on macOS; the seatbelt is the sandbox (Chrome shows a bar about it; --test-type would hide it but marks every page as automated)
                    @"--use-mock-keychain",      // never the login keychain (which the seatbelt hides)
                    [@"--user-data-dir=" stringByAppendingString:[self.paths browserDir]],
                    [@"--disk-cache-dir=" stringByAppendingString:[[self.paths browserDir] stringByAppendingPathComponent:@"cache"]],
                    [NSString stringWithFormat:@"--remote-debugging-port=%d", port],
                    @"--no-first-run", @"--no-default-browser-check", @"--disable-search-engine-choice-screen",
                    // Keep working while hidden or behind other windows (the agent's pages still render and run).
                    @"--disable-backgrounding-occluded-windows", @"--disable-renderer-backgrounding", @"--disable-background-timer-throttling",
                    @"--window-size=1200,800", startURL];
    [fm createFileAtPath:[self.paths browserLog] contents:[NSData data] attributes:nil];
    NSFileHandle *logFile = [NSFileHandle fileHandleForWritingAtPath:[self.paths browserLog]];
    t.standardOutput = logFile;
    t.standardError = logFile;
    __weak typeof(self) weak = self;
    t.terminationHandler = ^(NSTask *task) {
        dispatch_async(dispatch_get_main_queue(), ^{
            [logFile closeFile];
            if (weak.task != task) return;
            weak.task = nil;
            weak.port = 0;
            [[NSFileManager defaultManager] removeItemAtPath:[weak.paths browserJSON] error:nil];
            [weak say:[NSString stringWithFormat:@"%@ exited with status %d%@", weak.appName, task.terminationStatus, weak.wanted ? @"; starting it again in 3 s" : @""]];
            if (!weak.wanted) return;
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 3 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
                NSError *err;
                if (weak.wanted && !weak.task && ![weak start:&err]) [weak say:err.localizedDescription];
            });
        });
    };
    NSError *err;
    if (![t launchAndReturnError:&err]) {
        if (error) *error = err;
        [self say:[NSString stringWithFormat:@"could not run %@: %@", self.appName, err.localizedDescription]];
        return NO;
    }
    self.task = t;
    [self say:[NSString stringWithFormat:@"%@ started (pid %d), profile %@, sandboxed to it; output in browser.log", self.appName, t.processIdentifier, [self.paths browserDir]]];
    [self pollPort:0 expecting:port];
    return YES;
}

- (void)pollPort:(int)tries expecting:(int)port {
    if (!self.task) return;
    // Ready once the DevTools server answers on the port we gave it.
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    struct sockaddr_in a = {0};
    a.sin_family = AF_INET;
    a.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    a.sin_port = htons(port);
    BOOL up = fd >= 0 && connect(fd, (struct sockaddr *)&a, sizeof a) == 0;
    if (fd >= 0) close(fd);
    if (up) {
        self.port = port;
        if (self.hidden) [self hideWhenReady:0];
        NSDictionary *d = @{@"host": @"host.docker.internal", @"port": @(port), @"pid": @(self.task.processIdentifier),
                            @"app": self.appName ?: @"", @"at": @([[NSDate date] timeIntervalSince1970])};
        [[NSJSONSerialization dataWithJSONObject:d options:NSJSONWritingPrettyPrinted | NSJSONWritingSortedKeys error:nil] writeToFile:[self.paths browserJSON] atomically:YES];
        [self say:[NSString stringWithFormat:@"%@ listens on 127.0.0.1:%d (browser.json); the container's localhost:9222 goes there", self.appName, port]];
        return;
    }
    if (tries > 60) { [self say:@"the browser didn't open its DevTools port in 30 s; browser.log says why"]; return; }
    __weak typeof(self) weak = self;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(0.5 * NSEC_PER_SEC)), dispatch_get_main_queue(), ^{ [weak pollPort:tries + 1 expecting:port]; });
}

- (NSRunningApplication *)app {
    return self.task ? [NSRunningApplication runningApplicationWithProcessIdentifier:self.task.processIdentifier] : nil;
}

- (void)hide {
    // Out of the way: hidden like Cmd+H, still running (the launch flags keep
    // its pages rendering). Since macOS 14 an app can't take the front by
    // itself, so a script's "bring to front" leaves it hidden too; on older
    // systems it may surface, and "Show browser" always does.
    [[self app] hide];
}

- (void)hideWhenReady:(int)tries {
    // The DevTools port opens before the window exists, and the window may
    // come to the front after it is shown: hide now and again over the
    // first seconds, unless the owner has asked to see it meanwhile.
    if (!self.task || !self.hidden || self.shown) return;
    [self hide];
    if (tries >= 12) return;
    __weak typeof(self) weak = self;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(0.5 * NSEC_PER_SEC)), dispatch_get_main_queue(), ^{ [weak hideWhenReady:tries + 1]; });
}

- (void)stop {
    self.wanted = NO;
    [[NSFileManager defaultManager] removeItemAtPath:[self.paths browserJSON] error:nil];
    if (self.task) {
        [self say:[NSString stringWithFormat:@"quitting %@ (pid %d)", self.appName, self.task.processIdentifier]];
        [self.task terminate];
    }
    self.port = 0;
}

- (void)activate {
    self.shown = YES;
    NSRunningApplication *app = [self app];
    [app unhide];
    [app activateWithOptions:NSApplicationActivateAllWindows];
}

- (BOOL)running { return self.task != nil; }

@end
