#import "App.h"
#import "Status.h"
#import <CoreImage/CoreImage.h>

@interface AppDelegate ()
@property (nonatomic, strong) NSWindow *window;
@property (nonatomic, strong) NSTextField *statusLabel, *detailLabel, *inviteTitle, *codeLabel, *messageLabel;
@property (nonatomic, strong) NSImageView *qrView;
@property (nonatomic, strong) NSButton *clipButton, *startStop, *pauseResume, *browserButton, *logButton, *repoButton, *imageButton, *dockerButton;
@property (nonatomic, copy) NSString *dockerState;   // nil (unknown), "ok", "missing", "stopped"
@property (nonatomic) NSUInteger ticks;
@property (nonatomic, strong) NSTextView *output;
@property (nonatomic, strong) AppConfig *config;
@property (nonatomic, strong) NSTimer *timer;
@property (nonatomic, strong) NSTask *direct;        // the `myous worker` child in direct mode
@property (nonatomic, strong) NSNumber *requestsAtLaunch;   // for the Dock badge
@property (nonatomic, copy) NSString *lastQRLink;
@end

@implementation AppDelegate

- (void)applicationDidFinishLaunching:(NSNotification *)note {
    self.config = [AppConfig read];
    [self buildMenu];
    [self buildWindow];
    if (self.snapshotPath) self.dockerState = [self probeDocker];   // synchronous, so the snapshot shows it
    [self refresh];
    [self checkDocker];
    self.timer = [NSTimer scheduledTimerWithTimeInterval:2 target:self selector:@selector(refresh) userInfo:nil repeats:YES];
    [NSApp activateIgnoringOtherApps:YES];
    if (self.snapshotPath) {
        [self append:@"snapshot: this is the log area"];
        [self.window layoutIfNeeded];
        NSView *v = self.window.contentView;
        NSBitmapImageRep *rep = [v bitmapImageRepForCachingDisplayInRect:v.bounds];
        [v cacheDisplayInRect:v.bounds toBitmapImageRep:rep];
        NSData *png = [rep representationUsingType:NSBitmapImageFileTypePNG properties:@{}];
        [png writeToFile:self.snapshotPath atomically:YES];
        [NSApp terminate:nil];
    }
}

- (BOOL)applicationShouldTerminateAfterLastWindowClosed:(NSApplication *)sender { return YES; }
- (BOOL)applicationSupportsSecureRestorableState:(NSApplication *)app { return YES; }

- (void)applicationWillTerminate:(NSNotification *)note {
    // A direct-mode worker is our child; don't leave it orphaned.
    [self.direct terminate];
}

#pragma mark - layout

- (void)buildMenu {
    NSMenu *main = [NSMenu new];
    NSMenuItem *appItem = [NSMenuItem new];
    NSMenu *appMenu = [NSMenu new];
    [appMenu addItemWithTitle:@"Quit Myous Worker" action:@selector(terminate:) keyEquivalent:@"q"];
    appItem.submenu = appMenu;
    [main addItem:appItem];
    NSMenuItem *editItem = [NSMenuItem new];
    NSMenu *edit = [[NSMenu alloc] initWithTitle:@"Edit"];
    [edit addItemWithTitle:@"Copy" action:@selector(copy:) keyEquivalent:@"c"];
    [edit addItemWithTitle:@"Select All" action:@selector(selectAll:) keyEquivalent:@"a"];
    editItem.submenu = edit;
    [main addItem:editItem];
    NSApp.mainMenu = main;
}

- (NSButton *)button:(NSString *)title action:(SEL)action {
    NSButton *b = [NSButton buttonWithTitle:title target:self action:action];
    b.bezelStyle = NSBezelStyleRounded;
    return b;
}

- (void)buildWindow {
    self.window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, 460, 580)
        styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable | NSWindowStyleMaskMiniaturizable
        backing:NSBackingStoreBuffered defer:NO];
    self.window.title = @"Myous Worker";
    [self.window center];

    self.statusLabel = [NSTextField labelWithString:@""];
    self.statusLabel.font = [NSFont systemFontOfSize:18 weight:NSFontWeightSemibold];
    self.detailLabel = [NSTextField wrappingLabelWithString:@""];
    self.detailLabel.font = [NSFont systemFontOfSize:13];
    self.detailLabel.preferredMaxLayoutWidth = 420;
    self.inviteTitle = [NSTextField labelWithString:@"Pairing code: paste the message below to your agent"];
    self.inviteTitle.font = [NSFont systemFontOfSize:13 weight:NSFontWeightSemibold];
    self.codeLabel = [NSTextField labelWithString:@""];
    self.codeLabel.font = [NSFont monospacedSystemFontOfSize:28 weight:NSFontWeightBold];
    self.codeLabel.selectable = YES;
    self.qrView = [NSImageView new];
    self.qrView.imageScaling = NSImageScaleProportionallyUpOrDown;
    self.qrView.translatesAutoresizingMaskIntoConstraints = NO;
    [self.qrView.widthAnchor constraintEqualToConstant:160].active = YES;
    [self.qrView.heightAnchor constraintEqualToConstant:160].active = YES;

    // The sentence the owner pastes to their agent with the code: an agent
    // handed a bare code may guess what it is (one made itself a worker).
    self.messageLabel = [NSTextField wrappingLabelWithString:@""];
    self.messageLabel.font = [NSFont systemFontOfSize:12];
    self.messageLabel.textColor = [NSColor secondaryLabelColor];
    self.messageLabel.selectable = YES;
    self.messageLabel.preferredMaxLayoutWidth = 440;
    self.clipButton = [self button:@"Copy message for your agent" action:@selector(copyCode)];
    self.startStop = [self button:@"Start" action:@selector(toggleRunning)];
    self.pauseResume = [self button:@"Pause" action:@selector(togglePaused)];
    self.browserButton = [self button:@"Open browser" action:@selector(openBrowserView)];
    self.logButton = [self button:@"Show log" action:@selector(openLog)];
    self.repoButton = [self button:@"Choose repo…" action:@selector(chooseRepo)];
    self.imageButton = [self button:@"Use the built-in image" action:@selector(useImage)];
    self.dockerButton = [self button:@"Get Docker Desktop" action:@selector(getDocker)];

    self.output = [NSTextView new];
    self.output.editable = NO;
    self.output.font = [NSFont monospacedSystemFontOfSize:11 weight:NSFontWeightRegular];
    self.output.textContainerInset = NSMakeSize(4, 4);
    self.output.verticallyResizable = YES;
    self.output.autoresizingMask = NSViewWidthSizable;
    self.output.maxSize = NSMakeSize(FLT_MAX, FLT_MAX);
    self.output.textContainer.widthTracksTextView = YES;
    NSScrollView *scroll = [NSScrollView new];
    scroll.documentView = self.output;
    scroll.hasVerticalScroller = YES;
    scroll.borderType = NSBezelBorder;
    scroll.translatesAutoresizingMaskIntoConstraints = NO;
    [scroll.heightAnchor constraintEqualToConstant:120].active = YES;

    NSStackView *inviteRow = [NSStackView stackViewWithViews:@[self.codeLabel, self.qrView]];
    inviteRow.orientation = NSUserInterfaceLayoutOrientationHorizontal;
    inviteRow.alignment = NSLayoutAttributeCenterY;
    inviteRow.spacing = 16;
    NSStackView *inviteBox = [NSStackView stackViewWithViews:@[self.inviteTitle, inviteRow, self.messageLabel, self.clipButton]];
    inviteBox.orientation = NSUserInterfaceLayoutOrientationVertical;
    inviteBox.alignment = NSLayoutAttributeLeading;
    inviteBox.spacing = 6;

    NSStackView *buttons = [NSStackView stackViewWithViews:@[self.startStop, self.pauseResume, self.browserButton, self.logButton]];
    buttons.spacing = 8;
    NSStackView *buttons2 = [NSStackView stackViewWithViews:@[self.dockerButton, self.imageButton, self.repoButton]];
    buttons2.spacing = 8;

    NSStackView *column = [NSStackView stackViewWithViews:@[self.statusLabel, self.detailLabel, inviteBox, buttons, buttons2, scroll]];
    column.orientation = NSUserInterfaceLayoutOrientationVertical;
    column.alignment = NSLayoutAttributeLeading;
    column.spacing = 12;
    column.edgeInsets = NSEdgeInsetsMake(20, 20, 20, 20);
    column.translatesAutoresizingMaskIntoConstraints = NO;
    NSView *content = self.window.contentView;
    [content addSubview:column];
    [NSLayoutConstraint activateConstraints:@[
        [column.leadingAnchor constraintEqualToAnchor:content.leadingAnchor],
        [column.trailingAnchor constraintEqualToAnchor:content.trailingAnchor],
        [column.topAnchor constraintEqualToAnchor:content.topAnchor],
        [scroll.widthAnchor constraintEqualToAnchor:column.widthAnchor constant:-40],
    ]];
    [self.window makeKeyAndOrderFront:nil];
}

#pragma mark - state

static NSString *str(id v) { return [v isKindOfClass:[NSString class]] ? v : nil; }
static NSNumber *num(id v) { return [v isKindOfClass:[NSNumber class]] ? v : nil; }
static NSDictionary *dict(id v) { return [v isKindOfClass:[NSDictionary class]] ? v : nil; }

- (void)refresh {
    self.config = [AppConfig read];
    if (++self.ticks % 5 == 0) [self checkDocker];
    StatusFile *file = [StatusFile read];
    NSDictionary *s = file.status;
    BOOL running = [file fresh];
    BOOL paused = [[NSFileManager defaultManager] fileExistsAtPath:[Paths paused]];
    NSString *alias = str(s[@"alias"]);
    if (!alias.length) alias = @"worker";

    self.statusLabel.stringValue = [NSString stringWithFormat:@"%@ · %@%@", running ? @"Running" : @"Stopped", alias,
                                    paused ? @" · paused" : @""];
    self.statusLabel.textColor = running ? (paused ? [NSColor systemOrangeColor] : [NSColor systemGreenColor])
                                         : [NSColor secondaryLabelColor];

    // Docker missing or stopped matters more than the worker's state.
    BOOL dockerProblem = [self.config usesDocker] && self.dockerState && ![self.dockerState isEqualToString:@"ok"];
    if (dockerProblem) {
        self.statusLabel.stringValue = [self.dockerState isEqualToString:@"missing"] ? @"Docker Desktop is not installed"
                                                                                      : @"Docker Desktop is not running";
        self.statusLabel.textColor = [NSColor systemOrangeColor];
    }

    NSMutableArray *lines = [NSMutableArray new];
    if ([self.config isDirect]) {
        [lines addObject:@"Mode: direct (myous worker on this Mac)"];
    } else if ([self.config isImage]) {
        [lines addObject:[NSString stringWithFormat:@"Mode: built-in image ghcr.io/myoushq/worker:%@", appVersion()]];
    } else {
        [lines addObject:[NSString stringWithFormat:@"Mode: Docker in checkout %@", self.config.repo]];
    }
    if (s) {
        [lines addObject:[NSString stringWithFormat:@"Contacts: %@   Requests: %@", num(s[@"contacts"]) ?: @0, num(s[@"requests"]) ?: @0]];
        NSDictionary *paired = dict(s[@"paired"]);
        if (paired && str(paired[@"verify"])) {
            [lines addObject:[NSString stringWithFormat:@"Paired with %@: verification code %@ (your agent shows the same number)",
                              str(paired[@"alias"]) ?: @"?", str(paired[@"verify"])]];
        }
        NSDictionary *last = dict(s[@"last"]);
        if (last) {
            NSNumber *at = num(last[@"at"]);
            NSNumber *ok = num(last[@"ok"]);
            [lines addObject:[NSString stringWithFormat:@"Last: %@ from %@, %@%@", str(last[@"op"]) ?: @"?", str(last[@"alias"]) ?: @"?",
                              at ? timeAgo(at.doubleValue) : @"?", ok ? (ok.boolValue ? @", ok" : @", refused or failed") : @""]];
        } else {
            [lines addObject:@"Last: nothing yet"];
        }
        if (str(s[@"work"])) [lines addObject:[NSString stringWithFormat:@"Work dir: %@", s[@"work"]]];
    } else if (!running) {
        [lines addObject:@"No status file yet at ~/.myous-worker/worker.json"];
    }
    self.detailLabel.stringValue = [lines componentsJoinedByString:@"\n"];

    // Pairing: only while an invite is open and not expired.
    NSDictionary *invite = dict(s[@"invite"]);
    NSNumber *exp = num(invite[@"expires_at"]);
    if (exp && exp.doubleValue < [[NSDate date] timeIntervalSince1970]) invite = nil;
    BOOL showInvite = running && str(invite[@"code"]) != nil;
    self.inviteTitle.hidden = self.codeLabel.hidden = self.qrView.hidden = self.clipButton.hidden = self.messageLabel.hidden = !showInvite;
    if (showInvite) {
        self.codeLabel.stringValue = str(invite[@"code"]);
        NSString *alias = str(s[@"alias"]) ?: @"worker";
        self.messageLabel.stringValue = str(invite[@"message"]) ?: [NSString stringWithFormat:
            @"Pair with my worker \"%@\" (an environment I set up for you, not something to run yourself): accept the pairing code %@ "
            @"with relationship other and sharing \"my own worker; run commands there for me\". Then send it the message help, "
            @"and read the Workers section of https://myoushq.com/skill.md before using it.", alias, str(invite[@"code"])];
        NSString *link = str(invite[@"link"]) ?: str(invite[@"code"]);
        if (![link isEqualToString:self.lastQRLink]) {
            self.qrView.image = [self makeQR:link side:160];
            self.lastQRLink = link;
        }
    }

    self.startStop.title = running ? @"Stop" : @"Start";
    self.pauseResume.title = paused ? @"Resume" : @"Pause";
    self.browserButton.hidden = [self.config isDirect];
    self.dockerButton.hidden = !dockerProblem;
    self.imageButton.hidden = [self.config isImage];

    // Dock badge: requests handled since the app started.
    NSNumber *n = num(s[@"requests"]);
    if (n) {
        if (!self.requestsAtLaunch) self.requestsAtLaunch = n;
        long since = n.longValue - self.requestsAtLaunch.longValue;
        NSApp.dockTile.badgeLabel = since > 0 ? [NSString stringWithFormat:@"%ld", since] : nil;
    } else {
        NSApp.dockTile.badgeLabel = nil;
    }
}

- (NSImage *)makeQR:(NSString *)text side:(CGFloat)side {
    CIFilter *filter = [CIFilter filterWithName:@"CIQRCodeGenerator"];
    [filter setValue:[text dataUsingEncoding:NSUTF8StringEncoding] forKey:@"inputMessage"];
    [filter setValue:@"M" forKey:@"inputCorrectionLevel"];
    CIImage *ci = filter.outputImage;
    if (!ci) return nil;
    CGFloat scale = side / ci.extent.size.width;
    CIImage *scaled = [ci imageByApplyingTransform:CGAffineTransformMakeScale(scale, scale)];
    NSCIImageRep *rep = [NSCIImageRep imageRepWithCIImage:scaled];
    NSImage *image = [[NSImage alloc] initWithSize:rep.size];
    [image addRepresentation:rep];
    return image;
}

#pragma mark - docker

/// "ok", "stopped" (installed, daemon not running) or "missing". Runs the
/// user's login shell so their PATH applies; MYOUS_DOCKER_BIN overrides the
/// command (tests use it to simulate a Mac without Docker).
- (NSString *)probeDocker {
    NSString *bin = [[NSProcessInfo processInfo] environment][@"MYOUS_DOCKER_BIN"] ?: @"docker";
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

/// Probe in the background, now and every 10 seconds, when Docker is in use.
- (void)checkDocker {
    if (![self.config usesDocker]) { self.dockerState = nil; return; }
    __weak typeof(self) weak = self;
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_UTILITY, 0), ^{
        NSString *state = [weak probeDocker];
        dispatch_async(dispatch_get_main_queue(), ^{
            if (![state isEqualToString:weak.dockerState]) {
                weak.dockerState = state;
                [weak refresh];
            }
        });
    });
}

- (void)getDocker {
    [[NSWorkspace sharedWorkspace] openURL:[NSURL URLWithString:@"https://www.docker.com/products/docker-desktop/"]];
}

#pragma mark - actions

- (void)copyCode {
    NSPasteboard *pb = [NSPasteboard generalPasteboard];
    [pb clearContents];
    [pb setString:self.messageLabel.stringValue forType:NSPasteboardTypeString];
    [self append:[NSString stringWithFormat:@"copied the pairing message (code %@)", self.codeLabel.stringValue]];
}

- (void)togglePaused {
    NSFileManager *fm = [NSFileManager defaultManager];
    if ([fm fileExistsAtPath:[Paths paused]]) {
        [fm removeItemAtPath:[Paths paused] error:nil];
        [self append:@"resumed: worker.paused removed"];
    } else {
        [fm createDirectoryAtPath:[Paths home] withIntermediateDirectories:YES attributes:nil error:nil];
        [fm createFileAtPath:[Paths paused] contents:[NSData data] attributes:nil];
        [self append:@"paused: the review hook refuses requests while worker.paused exists"];
    }
    [self refresh];
}

- (void)openBrowserView {
    // Straight into the noVNC client, connected and scaled to the tab; the
    // bare port shows the served folder's listing.
    [[NSWorkspace sharedWorkspace] openURL:[NSURL URLWithString:@"http://localhost:6080/vnc.html?autoconnect=1&reconnect=1&resize=remote"]];
}

- (void)openLog {
    if (![[NSFileManager defaultManager] fileExistsAtPath:[Paths log]]) {
        [self append:[NSString stringWithFormat:@"no log yet at %@", [Paths log]]];
        return;
    }
    [[NSWorkspace sharedWorkspace] openURL:[NSURL fileURLWithPath:[Paths log]]];
}

- (void)chooseRepo {
    NSOpenPanel *panel = [NSOpenPanel openPanel];
    panel.message = @"Choose your myoushq-client checkout (the folder containing worker/)";
    panel.canChooseDirectories = YES;
    panel.canChooseFiles = NO;
    panel.allowsMultipleSelection = NO;
    if ([panel runModal] == NSModalResponseOK && panel.URL) {
        self.config.repo = panel.URL.path;
        self.config.mode = @"docker";
        [self.config write];
        [self append:[NSString stringWithFormat:@"repo set to %@", panel.URL.path]];
        [self refresh];
    }
}

- (void)useImage {
    self.config.mode = @"image";
    [self.config write];
    [self append:[NSString stringWithFormat:@"using the built-in image ghcr.io/myoushq/worker:%@", appVersion()]];
    [self refresh];
    [self checkDocker];
}

- (void)toggleRunning {
    BOOL running = [[StatusFile read] fresh];
    NSString *docker = [[NSProcessInfo processInfo] environment][@"MYOUS_DOCKER_BIN"] ?: @"docker";
    if ([self.config isDirect]) {
        if (running || self.direct) [self stopDirect]; else [self startDirect];
    } else if ([self.config isImage]) {
        // The compose file travels in the bundle; a fixed project name
        // means Stop still finds the containers after an app update.
        NSString *compose = [Paths bundledCompose];
        if (!compose) { [self append:@"this build has no compose.yml in its Resources"]; return; }
        [[NSFileManager defaultManager] createDirectoryAtPath:[Paths home] withIntermediateDirectories:YES
                                                   attributes:@{NSFilePosixPermissions: @0700} error:nil];
        NSString *cmd = [NSString stringWithFormat:@"%@ compose -f '%@' -p myous-worker %@", docker,
                         [compose stringByReplacingOccurrencesOfString:@"'" withString:@"'\\''"],
                         running ? @"down" : @"up -d"];
        [self run:cmd in:[Paths home]];
    } else {
        NSString *dir = [self workerDir];
        if (!dir) return;
        [self run:running ? [docker stringByAppendingString:@" compose down"] : [docker stringByAppendingString:@" compose up -d"] in:dir];
    }
}

/// The repo's worker/ folder, asking for the repo if it isn't configured.
- (NSString *)workerDir {
    if (!self.config.repo) [self chooseRepo];
    if (!self.config.repo) return nil;
    NSString *dir = [self.config.repo stringByAppendingPathComponent:@"worker"];
    if (![[NSFileManager defaultManager] fileExistsAtPath:[dir stringByAppendingPathComponent:@"compose.yml"]]) {
        [self append:[NSString stringWithFormat:@"no compose.yml in %@; is this the myoushq-client checkout?", dir]];
        return nil;
    }
    return dir;
}

- (void)startDirect {
    [[NSFileManager defaultManager] createDirectoryAtPath:[Paths work] withIntermediateDirectories:YES attributes:nil error:nil];
    NSTask *t = [NSTask new];
    t.launchPath = @"/bin/sh";
    // A login shell, so the user's PATH (where `myous` lives) applies.
    t.arguments = @[@"-lc", @"exec myous worker --work \"$HOME/.myous-worker/work\" >> \"$HOME/.myous-worker/worker.log\" 2>&1"];
    __weak typeof(self) weak = self;
    t.terminationHandler = ^(NSTask *task) {
        dispatch_async(dispatch_get_main_queue(), ^{
            [weak append:[NSString stringWithFormat:@"myous worker exited with status %d", task.terminationStatus]];
            weak.direct = nil;
            [weak refresh];
        });
    };
    NSError *err;
    if ([t launchAndReturnError:&err]) {
        self.direct = t;
        [self append:[NSString stringWithFormat:@"started myous worker (pid %d); output in worker.log", t.processIdentifier]];
    } else {
        [self append:[NSString stringWithFormat:@"could not start myous worker: %@", err.localizedDescription]];
    }
}

- (void)stopDirect {
    if (self.direct) {
        [self append:[NSString stringWithFormat:@"stopping myous worker (pid %d)", self.direct.processIdentifier]];
        [self.direct terminate];
    } else {
        // Started outside this app: ask it to stop by pid.
        NSNumber *pid = num([StatusFile read].status[@"pid"]);
        if (pid) {
            kill((pid_t)pid.intValue, SIGTERM);
            [self append:[NSString stringWithFormat:@"sent SIGTERM to worker pid %@", pid]];
        }
    }
}

/// Run a shell command in a directory, streaming its output to the log area.
- (void)run:(NSString *)command in:(NSString *)dir {
    [self append:[@"$ " stringByAppendingString:command]];
    NSTask *t = [NSTask new];
    t.launchPath = @"/bin/sh";
    t.arguments = @[@"-lc", command];
    t.currentDirectoryPath = dir;
    NSPipe *pipe = [NSPipe pipe];
    t.standardOutput = pipe;
    t.standardError = pipe;
    __weak typeof(self) weak = self;
    pipe.fileHandleForReading.readabilityHandler = ^(NSFileHandle *h) {
        NSData *data = h.availableData;
        if (!data.length) return;
        NSString *text = [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
        if (!text) return;
        dispatch_async(dispatch_get_main_queue(), ^{
            [weak append:[text stringByTrimmingCharactersInSet:[NSCharacterSet newlineCharacterSet]]];
        });
    };
    t.terminationHandler = ^(NSTask *task) {
        pipe.fileHandleForReading.readabilityHandler = nil;
        dispatch_async(dispatch_get_main_queue(), ^{
            [weak append:[NSString stringWithFormat:@"exit %d", task.terminationStatus]];
            [weak refresh];
        });
    };
    self.startStop.enabled = NO;
    NSError *err;
    if (![t launchAndReturnError:&err]) [self append:[NSString stringWithFormat:@"could not run: %@", err.localizedDescription]];
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, NSEC_PER_SEC), dispatch_get_main_queue(), ^{ weak.startStop.enabled = YES; });
}

- (void)append:(NSString *)line {
    NSDateFormatter *f = [NSDateFormatter new];
    f.dateFormat = @"HH:mm:ss";
    NSString *text = [NSString stringWithFormat:@"%@ %@\n", [f stringFromDate:[NSDate date]], line];
    [self.output.textStorage appendAttributedString:[[NSAttributedString alloc] initWithString:text
        attributes:@{NSFontAttributeName: self.output.font, NSForegroundColorAttributeName: [NSColor textColor]}]];
    [self.output scrollToEndOfDocument:nil];
}

@end
