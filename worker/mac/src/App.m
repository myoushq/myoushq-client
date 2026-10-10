#import "App.h"
#import "Status.h"
#import "Icon.h"
#import "Runtime.h"
#import "Agents.h"
#import "Worker.h"
#import <CoreImage/CoreImage.h>
#import <UserNotifications/UserNotifications.h>
#import <ServiceManagement/ServiceManagement.h>


static const CGFloat kWidth = 560;
static const CGFloat kInner = kWidth - 40;
static const NSUInteger kRequestRows = 200;
static const double kInviteSeconds = 900;
static const CGFloat kSidebar = 150;
static const NSUInteger kAgentsEvery = 15;   // ticks (2 s each) between reads of the local agents

@interface AppDelegate () <NSMenuDelegate, NSTableViewDataSource, NSTableViewDelegate, NSWindowDelegate, UNUserNotificationCenterDelegate>
// model: the workers (Worker.h), one per folder; `current` is the one the window shows.
// The per-worker properties below forward to `current`, so the rest of the
// app reads as it did with one worker.
@property (nonatomic, strong) NSMutableArray<Worker *> *workers;
@property (nonatomic, strong) Worker *current;
@property (nonatomic, strong) AppConfig *config;
@property (nonatomic, strong) StatusFile *status;
@property (nonatomic, strong) NSArray<NSDictionary *> *requests;
@property (nonatomic, strong) NSDate *requestsDirDate;
@property (nonatomic) Screen screen;
@property (nonatomic, copy) NSString *launchStage;
@property (nonatomic) double launchedAt;
@property (nonatomic) BOOL stopping;
@property (nonatomic) double stoppedAt;
@property (nonatomic) BOOL wasRunning;
@property (nonatomic, strong) NSTask *direct;
@property (nonatomic, strong) NSArray<NSDictionary *> *approvals;
@property (nonatomic, readonly) Paths *paths;
@property (nonatomic, readonly) AppConfig *appConfig;   // the first worker's app.json holds the app's settings
@property (nonatomic, copy) NSString *runtimeState;   // nil (unknown), "ok", "missing", "stopped"
@property (nonatomic, copy) NSString *runtimeName;
@property (nonatomic, copy) NSString *latestRelease;  // "vX.Y.Z" from the hub, when newer
@property (nonatomic) NSUInteger ticks;
@property (nonatomic, strong) NSArray<NSDictionary *> *tableRequests;   // what the table last showed
@property (nonatomic, strong) NSMutableSet *notified;
@property (nonatomic, copy) NSString *fake;
// agents on this Mac (Agents.h): read every kAgentsEvery ticks, shown next to the worker
@property (nonatomic, strong) NSArray<LocalAgent *> *agents;
@property (nonatomic, strong) LocalAgent *selectedAgent;   // nil: the worker is selected
@property (nonatomic) BOOL agentsBusy;
@property (nonatomic, strong) NSArray<NSDictionary *> *sidebarRows;
// ui
@property (nonatomic, strong) NSStatusItem *statusItem;
@property (nonatomic, strong) NSWindow *window;
@property (nonatomic, strong) NSStackView *root;
@property (nonatomic, strong) NSTextField *headTitle, *headRight, *headFacts, *banner;
@property (nonatomic, strong) NSButton *bannerButton;
@property (nonatomic, strong) NSBox *setupCard, *runtimeCard, *startingCard, *pairCard, *pairedCard, *requestsCard, *browserCard, *stoppedCard;
@property (nonatomic, strong) NSTextField *setupRuntime, *nameField, *runtimeText, *directWarning;
@property (nonatomic, strong) NSButton *setupRemove, *stoppedRemove;
@property (nonatomic, strong) NSPopUpButton *setupBrowser;
@property (nonatomic, strong) NSTextField *setupBrowserHint, *browserText;
@property (nonatomic, strong) NSButton *browserOpen, *settingsBrowserHidden;
@property (nonatomic, strong) NSPopUpButton *setupGetBrowser;      // "Get a browser…": where each known one is downloaded
@property (nonatomic, strong) NSArray<NSString *> *setupBrowserIds; // one per setupBrowser item: a bundle id, or "" for the container
@property (nonatomic, strong) NSTextField *settingsBrowserHiddenHint;
@property (nonatomic, strong) NSButton *setupStart, *getDockerButton, *getOrbButton, *openRuntimeButton, *directToggle, *directStart;
@property (nonatomic, strong) NSArray<NSTextField *> *startRows;
@property (nonatomic, strong) NSTextField *startNote;
@property (nonatomic, strong) NSButton *startRetry;
@property (nonatomic, strong) NSTextField *pairMessage, *pairCode, *pairWait;
@property (nonatomic, strong) NSProgressIndicator *pairBar;
@property (nonatomic, strong) NSPopover *qrPopover;
@property (nonatomic, copy) NSString *lastQRLink;
@property (nonatomic, strong) NSTextField *pairedText, *pairedCode;
@property (nonatomic, strong) NSTextField *requestsTitle, *pausedNote, *approvalText;
@property (nonatomic, strong) NSButton *pauseButton, *allowButton, *refuseButton, *stopCommandButton;
@property (nonatomic, strong) NSArray<NSButton *> *settingsKinds;
@property (nonatomic, strong) NSStackView *approvalRow;
@property (nonatomic, copy) NSString *fakeApprovalId;
@property (nonatomic, strong) NSTableView *table;
@property (nonatomic, strong) NSTextField *stoppedText, *stoppingText;
@property (nonatomic, strong) NSBox *stoppingCard;
@property (nonatomic, strong) NSWindow *logWindow, *detailWindow, *settingsWindow;
@property (nonatomic, strong) NSTextView *logView, *detailView;
@property (nonatomic, strong) NSTextField *settingsName;
@property (nonatomic, strong) NSButton *settingsDock, *settingsLogin, *settingsNotify, *settingsUpdate;
@property (nonatomic, strong) NSPopUpButton *settingsReview;
@property (nonatomic, strong) NSTimer *timer;
@property (nonatomic) CGFloat lastHeight, lastWidth;
@property (nonatomic, strong) NSTableView *sidebar;
@property (nonatomic, strong) NSScrollView *sidebarScroll;
@property (nonatomic, strong) NSBox *agentCard;
@property (nonatomic, strong) NSTextField *agentFacts, *agentPending, *agentNote;
@property (nonatomic, strong) NSTableView *agentContacts;
@property (nonatomic, strong) NSPopUpButton *pairWhich;
@property (nonatomic, strong) NSStackView *pairWhichRow, *pairLocalRow;
@property (nonatomic, strong) NSArray<NSView *> *pairMessageViews;
@property (nonatomic, strong) NSButton *pairLocalButton, *settingsAgents;
@property (nonatomic, strong) NSTextField *pairLocalNote;
@property (nonatomic, strong) NSTextField *descriptionField, *settingsDescription;   // the worker's card: what the agent is told this computer is
@end

/// What the agent is told this computer is when the owner writes nothing:
/// enough to bind the owner's words ("the myous browser") to this worker
/// rather than to the agent's own environment.
static NSString *defaultDescription(NSString *name) {
    return [NSString stringWithFormat:@"My own computer, %@. Its browser is where I log into sites for you: when I say "
            "\"the myous browser\" or \"the worker browser\", I mean that one, never your own. Commands you send run in a container on it.",
            name ?: @"this Mac"];
}

@implementation AppDelegate
@dynamic config, status, requests, requestsDirDate, screen, launchStage, launchedAt, stopping, stoppedAt, wasRunning, direct, approvals, paths, appConfig;

- (AppConfig *)config { return self.current.config; }
- (void)setConfig:(AppConfig *)c { self.current.config = c; }
- (StatusFile *)status { return self.current.status; }
- (void)setStatus:(StatusFile *)v { self.current.status = v; }
- (NSArray<NSDictionary *> *)requests { return self.current.requests; }
- (void)setRequests:(NSArray<NSDictionary *> *)v { self.current.requests = v; }
- (NSDate *)requestsDirDate { return self.current.requestsDirDate; }
- (void)setRequestsDirDate:(NSDate *)v { self.current.requestsDirDate = v; }
- (Screen)screen { return self.current.screen; }
- (void)setScreen:(Screen)v { self.current.screen = v; }
- (NSString *)launchStage { return self.current.launchStage; }
- (void)setLaunchStage:(NSString *)v { self.current.launchStage = v; }
- (double)launchedAt { return self.current.launchedAt; }
- (void)setLaunchedAt:(double)v { self.current.launchedAt = v; }
- (BOOL)stopping { return self.current.stopping; }
- (double)stoppedAt { return self.current.stoppedAt; }
- (void)setStoppedAt:(double)v { self.current.stoppedAt = v; }
- (void)setStopping:(BOOL)v { self.current.stopping = v; }
- (BOOL)wasRunning { return self.current.wasRunning; }
- (void)setWasRunning:(BOOL)v { self.current.wasRunning = v; }
- (NSTask *)direct { return self.current.direct; }
- (void)setDirect:(NSTask *)v { self.current.direct = v; }
- (NSArray<NSDictionary *> *)approvals { return self.current.approvals; }
- (void)setApprovals:(NSArray<NSDictionary *> *)v { self.current.approvals = v; }
- (Paths *)paths { return self.current.paths; }
- (AppConfig *)appConfig { return self.workers.firstObject.config; }

/// One Worker per folder (the default and every ~/.myous-worker-<n>), keeping
/// the ones already loaded.
- (void)loadWorkers {
    if (!self.workers) self.workers = [NSMutableArray new];
    for (NSString *home in [Paths allHomes]) {
        BOOL have = NO;
        for (Worker *w in self.workers) if ([w.paths.home isEqualToString:home]) have = YES;
        if (!have) [self.workers addObject:[[Worker alloc] initWithHome:home]];
    }
    if (!self.current || ![self.workers containsObject:self.current]) self.current = self.workers.firstObject;
}

- (void)applicationDidFinishLaunching:(NSNotification *)note {
    self.notified = [NSMutableSet new];
    self.fake = [[NSProcessInfo processInfo] environment][@"MYOUS_FAKE_STATE"];
    [self loadWorkers];
    [NSApp setActivationPolicy:self.appConfig.dock ? NSApplicationActivationPolicyRegular : NSApplicationActivationPolicyAccessory];
    [self buildMainMenu];
    [self buildStatusItem];
    [self buildWindow];
    self.runtimeState = [Runtime probe];   // once synchronously: the first screen depends on it
    self.runtimeName = [Runtime name];
    [self refresh];
    [self setupNotifications];
    self.timer = [NSTimer scheduledTimerWithTimeInterval:2 target:self selector:@selector(refresh) userInfo:nil repeats:YES];
    [self.window makeKeyAndOrderFront:nil];
    [NSApp activateIgnoringOtherApps:YES];
    if (self.appConfig.autoUpdate && [[NSDate date] timeIntervalSince1970] - self.appConfig.lastUpdateCheck > 86400 && !self.fake)
        [self checkForUpdates:NO];
    if (self.snapshotPath) {
        // Render the window's content view once it has laid out: a PNG from
        // the view cache and, next to it, a PDF (text renders there on Macs
        // where the bitmap cache drops it).
        __weak typeof(self) weak = self;
        __block int waited = 0;
        __block __weak void (^weakShoot)(void);
        void (^shoot)(void);
        weakShoot = shoot = ^{
            if (weak.agentsBusy && waited++ < 10) {   // the local agents answer in the background
                dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(0.5 * NSEC_PER_SEC)), dispatch_get_main_queue(), weakShoot);
                return;
            }
            // MYOUS_SNAPSHOT_AGENT=1: show the first local agent's card instead of the worker.
            if ([[NSProcessInfo processInfo] environment][@"MYOUS_SNAPSHOT_AGENT"] && weak.agents.count) weak.selectedAgent = weak.agents.firstObject;
            // MYOUS_SNAPSHOT_WORKER=<n>: show the n-th worker (1-based) instead of the first.
            NSInteger n = [[NSProcessInfo processInfo] environment][@"MYOUS_SNAPSHOT_WORKER"].integerValue;
            if (n > 1 && (NSUInteger)n <= weak.workers.count) { weak.current = weak.workers[n - 1]; weak.nameField.stringValue = @""; }
            [weak refresh];
            NSView *v = weak.window.contentView;
            NSBitmapImageRep *rep = [v bitmapImageRepForCachingDisplayInRect:v.bounds];
            [v cacheDisplayInRect:v.bounds toBitmapImageRep:rep];
            [[rep representationUsingType:NSBitmapImageFileTypePNG properties:@{}] writeToFile:weak.snapshotPath atomically:YES];
            [[v dataWithPDFInsideRect:v.bounds] writeToFile:[[weak.snapshotPath stringByDeletingPathExtension] stringByAppendingPathExtension:@"pdf"] atomically:YES];
            [NSApp terminate:nil];
        };
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(0.5 * NSEC_PER_SEC)), dispatch_get_main_queue(), shoot);
    }
}

- (BOOL)applicationShouldTerminateAfterLastWindowClosed:(NSApplication *)sender { return NO; }
- (BOOL)applicationSupportsSecureRestorableState:(NSApplication *)app { return YES; }
- (BOOL)applicationShouldHandleReopen:(NSApplication *)sender hasVisibleWindows:(BOOL)flag {
    [self showWindow];
    return NO;
}
- (void)applicationWillTerminate:(NSNotification *)note {
    for (Worker *w in self.workers) { [w.direct terminate]; [w.browser stop]; }   // our children; don't leave them orphaned
}

#pragma mark - widgets

- (NSTextField *)label:(NSString *)text size:(CGFloat)size weight:(NSFontWeight)weight {
    NSTextField *l = [NSTextField labelWithString:text];
    l.font = [NSFont systemFontOfSize:size weight:weight];
    return l;
}

- (NSTextField *)wrap:(NSString *)text {
    NSTextField *l = [NSTextField wrappingLabelWithString:text];
    l.font = [NSFont systemFontOfSize:13];
    l.preferredMaxLayoutWidth = kInner - 40;
    l.selectable = YES;
    return l;
}

/// An editable, wrapping text field of a few lines (for the description).
- (NSTextField *)textArea:(CGFloat)width {
    NSTextField *f = [NSTextField wrappingLabelWithString:@""];
    f.editable = YES;
    f.selectable = YES;
    f.bezeled = YES;
    f.bezelStyle = NSTextFieldSquareBezel;
    f.drawsBackground = YES;
    f.font = [NSFont systemFontOfSize:12];
    f.preferredMaxLayoutWidth = width - 8;
    [f.widthAnchor constraintEqualToConstant:width].active = YES;
    // Room for five lines of 12 pt: the default description is four.
    [f.heightAnchor constraintGreaterThanOrEqualToConstant:84].active = YES;
    [f setContentCompressionResistancePriority:NSLayoutPriorityRequired forOrientation:NSLayoutConstraintOrientationVertical];
    return f;
}

- (NSButton *)button:(NSString *)title action:(SEL)action {
    NSButton *b = [NSButton buttonWithTitle:title target:self action:action];
    b.bezelStyle = NSBezelStyleRounded;
    return b;
}

- (NSStackView *)column:(NSArray<NSView *> *)views {
    NSStackView *s = [NSStackView stackViewWithViews:views];
    s.orientation = NSUserInterfaceLayoutOrientationVertical;
    s.alignment = NSLayoutAttributeLeading;
    s.spacing = 8;
    return s;
}

- (NSStackView *)row:(NSArray<NSView *> *)views {
    NSStackView *s = [NSStackView stackViewWithViews:views];
    s.orientation = NSUserInterfaceLayoutOrientationHorizontal;
    s.alignment = NSLayoutAttributeCenterY;
    s.spacing = 8;
    return s;
}

/// A right-aligned row of buttons.
- (NSStackView *)buttons:(NSArray<NSView *> *)views {
    NSView *spacer = [NSView new];
    [spacer setContentHuggingPriority:1 forOrientation:NSLayoutConstraintOrientationHorizontal];
    NSStackView *s = [self row:[@[spacer] arrayByAddingObjectsFromArray:views]];
    [s.widthAnchor constraintEqualToConstant:kInner - 28].active = YES;
    return s;
}

- (NSBox *)card:(NSString *)title content:(NSView *)content {
    NSBox *box = [NSBox new];
    box.title = title;
    box.titleFont = [NSFont systemFontOfSize:13 weight:NSFontWeightSemibold];
    box.translatesAutoresizingMaskIntoConstraints = NO;
    content.translatesAutoresizingMaskIntoConstraints = NO;
    [box.contentView addSubview:content];
    [NSLayoutConstraint activateConstraints:@[
        [content.leadingAnchor constraintEqualToAnchor:box.contentView.leadingAnchor constant:10],
        [content.trailingAnchor constraintEqualToAnchor:box.contentView.trailingAnchor constant:-10],
        [content.topAnchor constraintEqualToAnchor:box.contentView.topAnchor constant:8],
        [content.bottomAnchor constraintEqualToAnchor:box.contentView.bottomAnchor constant:-8],
        [box.widthAnchor constraintEqualToConstant:kInner],
    ]];
    return box;
}

#pragma mark - menus

- (void)buildMainMenu {
    // Not shown (the app lives in the menu bar) but it carries the key
    // equivalents: Cmd-C in the pairing message, Cmd-W, Cmd-Q.
    NSMenu *main = [NSMenu new];
    NSMenuItem *appItem = [NSMenuItem new];
    NSMenu *appMenu = [NSMenu new];
    [appMenu addItemWithTitle:@"Settings…" action:@selector(openSettings) keyEquivalent:@","];
    [appMenu addItemWithTitle:@"Quit myous" action:@selector(terminate:) keyEquivalent:@"q"];
    appItem.submenu = appMenu;
    [main addItem:appItem];
    NSMenuItem *editItem = [NSMenuItem new];
    NSMenu *edit = [[NSMenu alloc] initWithTitle:@"Edit"];
    [edit addItemWithTitle:@"Copy" action:@selector(copy:) keyEquivalent:@"c"];
    [edit addItemWithTitle:@"Paste" action:@selector(paste:) keyEquivalent:@"v"];
    [edit addItemWithTitle:@"Select All" action:@selector(selectAll:) keyEquivalent:@"a"];
    editItem.submenu = edit;
    [main addItem:editItem];
    NSMenuItem *windowItem = [NSMenuItem new];
    NSMenu *window = [[NSMenu alloc] initWithTitle:@"Window"];
    [window addItemWithTitle:@"Close" action:@selector(performClose:) keyEquivalent:@"w"];
    windowItem.submenu = window;
    [main addItem:windowItem];
    NSApp.mainMenu = main;
}

- (void)buildStatusItem {
    self.statusItem = [[NSStatusBar systemStatusBar] statusItemWithLength:NSVariableStatusItemLength];
    self.statusItem.button.image = statusIcon([NSColor systemGrayColor], NO);
    self.statusItem.button.imagePosition = NSImageLeft;
    NSMenu *menu = [NSMenu new];
    menu.delegate = self;
    self.statusItem.menu = menu;
}

- (void)menuNeedsUpdate:(NSMenu *)menu {
    [menu removeAllItems];
    BOOL several = self.workers.count > 1;
    for (Worker *w in self.workers) {
        NSDictionary *s = w.status.status;
        NSMenuItem *state = [menu addItemWithTitle:[NSString stringWithFormat:@"%@ · %@", w.name, [self stateWordFor:w]] action:@selector(selectWorkerFromMenu:) keyEquivalent:@""];
        state.image = statusIcon([self stateColorFor:w], NO);
        state.representedObject = w.paths.home;
        NSString *pairedWith = str(dict(s[@"paired"])[@"alias"]);
        if (pairedWith && !several) {
            NSMenuItem *p = [menu addItemWithTitle:[NSString stringWithFormat:@"   paired with %@", pairedWith] action:nil keyEquivalent:@""];
            p.enabled = NO;
        }
        if (several) {
            // Each worker's actions in its own submenu; the flat list below is for one worker.
            NSMenu *sub = [NSMenu new];
            if (pairedWith) [sub addItemWithTitle:[NSString stringWithFormat:@"Paired with %@", pairedWith] action:nil keyEquivalent:@""].enabled = NO;
            BOOL up = w.screen == ScreenRunning || w.screen == ScreenPair || w.screen == ScreenPaired;
            if (up) {
                if (![w.config isDirect]) [self menu:sub add:[w.config macBrowser] ? @"Show browser" : @"Open browser" worker:w sel:@selector(openBrowserView)];
                [self menu:sub add:[self isPausedFor:w] ? @"Resume" : @"Pause" worker:w sel:@selector(togglePaused)];
                [self menu:sub add:@"Show requests…" worker:w sel:@selector(showWindow)];
                [self menu:sub add:@"Stop" worker:w sel:@selector(stop)];
            } else if (w.screen == ScreenStopping) {
                [sub addItemWithTitle:@"Stopping…" action:nil keyEquivalent:@""].enabled = NO;
            } else if (w.screen == ScreenStarting) {
                [self menu:sub add:@"Stop" worker:w sel:@selector(stop)];
            } else if (w.screen == ScreenStopped) {
                [self menu:sub add:@"Start" worker:w sel:@selector(start)];
            } else {
                [self menu:sub add:@"Set up…" worker:w sel:@selector(showWindow)];
            }
            if (!w.isDefault) {
                [sub addItem:[NSMenuItem separatorItem]];
                [self menu:sub add:[self isSetUp:w] ? @"Remove…" : @"Remove" worker:w sel:@selector(removeWorker)];
            }
            state.submenu = sub;
        }
    }
    if (several) [menu addItemWithTitle:@"Add a worker…" action:@selector(addWorker) keyEquivalent:@""];
    for (LocalAgent *a in self.agents) {
        NSString *what = a.error ? @"not readable" : [NSString stringWithFormat:@"%lu contact%@", (unsigned long)a.contacts.count, a.contacts.count == 1 ? @"" : @"s"];
        NSMenuItem *m = [menu addItemWithTitle:[NSString stringWithFormat:@"%@ · %@", a.displayName, what] action:@selector(selectAgentFromMenu:) keyEquivalent:@""];
        m.representedObject = a.home;
        m.image = agentIcon();
    }
    [menu addItem:[NSMenuItem separatorItem]];
    BOOL running = self.screen == ScreenRunning || self.screen == ScreenPair || self.screen == ScreenPaired;
    if (several) {
        // the per-worker items are in the submenus above
    } else if (running) {
        if (![self.config isDirect]) [menu addItemWithTitle:[self.config macBrowser] ? @"Show browser" : @"Open browser" action:@selector(openBrowserView) keyEquivalent:@""];
        [menu addItemWithTitle:[self isPaused] ? @"Resume" : @"Pause" action:@selector(togglePaused) keyEquivalent:@""];
        [menu addItemWithTitle:@"Show requests…" action:@selector(showWindow) keyEquivalent:@""];
        [menu addItem:[NSMenuItem separatorItem]];
        [menu addItemWithTitle:@"Stop" action:@selector(stop) keyEquivalent:@""];
    } else if (self.screen == ScreenStopping) {
        [menu addItemWithTitle:@"Stopping…" action:nil keyEquivalent:@""].enabled = NO;
    } else if (self.screen == ScreenStarting) {
        [menu addItemWithTitle:@"Starting…" action:nil keyEquivalent:@""].enabled = NO;
        [menu addItemWithTitle:@"Stop" action:@selector(stop) keyEquivalent:@""];
    } else if (self.screen == ScreenStopped) {
        [menu addItemWithTitle:@"Start" action:@selector(start) keyEquivalent:@""];
    } else {
        [menu addItemWithTitle:@"Set up…" action:@selector(showWindow) keyEquivalent:@""];
    }
    [menu addItem:[NSMenuItem separatorItem]];
    [menu addItemWithTitle:@"Settings…" action:@selector(openSettings) keyEquivalent:@""];
    [menu addItemWithTitle:@"Check for updates…" action:@selector(checkForUpdatesNow) keyEquivalent:@""];
    BOOL option = ([NSEvent modifierFlags] & NSEventModifierFlagOption) != 0;
    if (option || self.config.repo || [self.config isDirect]) {
        NSMenuItem *adv = [menu addItemWithTitle:@"Advanced" action:nil keyEquivalent:@""];
        NSMenu *sub = [NSMenu new];
        NSString *mode = [self.config isDirect] ? @"direct (no container)" : [self.config isImage] ? @"published image" : @"checkout";
        [sub addItemWithTitle:[NSString stringWithFormat:@"Mode: %@", mode] action:nil keyEquivalent:@""].enabled = NO;
        [sub addItemWithTitle:@"Use the published image" action:@selector(useImage) keyEquivalent:@""];
        [sub addItemWithTitle:@"Use a checkout…" action:@selector(chooseRepo) keyEquivalent:@""];
        [sub addItemWithTitle:@"Run without a container" action:@selector(useDirect) keyEquivalent:@""];
        if (self.config.repo && ![self.config isImage]) [sub addItemWithTitle:@"Rebuild the image" action:@selector(rebuild) keyEquivalent:@""];
        if ([self.config usesDocker]) {
            NSMenuItem *bi = [sub addItemWithTitle:@"Browser" action:nil keyEquivalent:@""];
            NSMenu *bm = [NSMenu new];
            // One entry per installed browser, the chosen one ticked (or the
            // first, which is what runs when none was chosen), then the container.
            NSArray *installed = [MacBrowser installed];
            NSString *why = [MacBrowser unavailableReason];
            NSString *chosen = [MacBrowser find:self.config.browserApp][@"id"];
            if (why) [bm addItemWithTitle:[NSString stringWithFormat:@"On this Mac (%@)", why] action:nil keyEquivalent:@""].enabled = NO;
            for (NSDictionary *b in why ? @[] : installed) {
                NSMenuItem *it = [bm addItemWithTitle:[NSString stringWithFormat:@"On this Mac, in %@", b[@"name"]] action:@selector(chooseBrowserItem:) keyEquivalent:@""];
                it.representedObject = b[@"id"];
                it.state = [self.config macBrowser] && [b[@"id"] isEqualToString:chosen] ? NSControlStateValueOn : NSControlStateValueOff;
            }
            NSMenuItem *cont = [bm addItemWithTitle:@"In the container" action:@selector(chooseBrowserItem:) keyEquivalent:@""];
            cont.representedObject = @"";
            cont.state = [self.config macBrowser] ? NSControlStateValueOff : NSControlStateValueOn;
            bi.submenu = bm;
        }
        [sub addItem:[NSMenuItem separatorItem]];
        [sub addItemWithTitle:@"Show app log" action:@selector(openLog) keyEquivalent:@""];
        [sub addItemWithTitle:@"Show container log" action:@selector(containerLog) keyEquivalent:@""];
        [sub addItemWithTitle:@"Open worker folder" action:@selector(openHome) keyEquivalent:@""];
        [sub addItemWithTitle:@"Remove stale containers" action:@selector(removeContainers) keyEquivalent:@""];
        [sub addItem:[NSMenuItem separatorItem]];
        [sub addItemWithTitle:@"Add a worker…" action:@selector(addWorker) keyEquivalent:@""];
        if (!self.current.isDefault) [sub addItemWithTitle:[NSString stringWithFormat:@"Remove %@%@", self.current.name, [self isSetUp:self.current] ? @"…" : @""] action:@selector(removeWorker) keyEquivalent:@""];
        [sub addItemWithTitle:@"Add an agent's folder…" action:@selector(addAgentHome) keyEquivalent:@""];
        adv.submenu = sub;
    }
    [menu addItem:[NSMenuItem separatorItem]];
    [menu addItemWithTitle:@"Open myous…" action:@selector(showWindow) keyEquivalent:@""];
    [menu addItemWithTitle:@"Quit myous" action:@selector(terminate:) keyEquivalent:@""];
}

#pragma mark - window

- (void)buildWindow {
    self.window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, kWidth, 400)
        styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable | NSWindowStyleMaskMiniaturizable
        backing:NSBackingStoreBuffered defer:NO];
    self.window.title = @"myous";
    self.window.delegate = self;
    self.window.releasedWhenClosed = NO;
    [self.window center];

    // Header
    self.headTitle = [self label:@"" size:17 weight:NSFontWeightSemibold];
    self.headRight = [self label:@"" size:13 weight:NSFontWeightRegular];
    self.headRight.textColor = [NSColor secondaryLabelColor];
    NSView *spacer = [NSView new];
    [spacer setContentHuggingPriority:1 forOrientation:NSLayoutConstraintOrientationHorizontal];
    NSStackView *headRow = [self row:@[self.headTitle, spacer, self.headRight]];
    [headRow.widthAnchor constraintEqualToConstant:kInner].active = YES;
    self.headFacts = [self label:@"" size:12 weight:NSFontWeightRegular];
    self.headFacts.textColor = [NSColor secondaryLabelColor];

    // Attention banner
    self.banner = [self wrap:@""];
    self.banner.textColor = [NSColor systemRedColor];
    self.banner.preferredMaxLayoutWidth = kInner - 120;
    self.bannerButton = [self button:@"" action:@selector(bannerAction)];
    NSStackView *bannerRow = [self row:@[self.banner, self.bannerButton]];

    [self buildSetupCard];
    [self buildRuntimeCard];
    [self buildStartingCard];
    [self buildPairCard];
    [self buildPairedCard];
    [self buildRequestsCard];
    [self buildBrowserCard];
    [self buildStoppedCard];
    [self buildStoppingCard];

    [self buildAgentCard];
    [self buildSidebar];

    self.root = [self column:@[headRow, self.headFacts, bannerRow, self.setupCard, self.runtimeCard, self.startingCard, self.pairCard,
                               self.pairedCard, self.stoppingCard, self.stoppedCard, self.requestsCard, self.browserCard, self.agentCard]];
    self.root.spacing = 12;
    self.root.edgeInsets = NSEdgeInsetsMake(16, 20, 20, 20);
    // The list of identities on the left appears once there is more than one (On this Mac).
    NSStackView *shell = [self row:@[self.sidebarScroll, self.root]];
    shell.alignment = NSLayoutAttributeTop;
    shell.spacing = 0;
    shell.translatesAutoresizingMaskIntoConstraints = NO;
    NSView *content = self.window.contentView;
    [content addSubview:shell];
    [NSLayoutConstraint activateConstraints:@[
        [shell.leadingAnchor constraintEqualToAnchor:content.leadingAnchor],
        [shell.topAnchor constraintEqualToAnchor:content.topAnchor],
        [self.sidebarScroll.heightAnchor constraintEqualToAnchor:self.root.heightAnchor],
    ]];
}

- (void)buildSidebar {
    self.sidebar = [NSTableView new];
    NSTableColumn *col = [[NSTableColumn alloc] initWithIdentifier:@"row"];
    col.width = kSidebar - 20;
    [self.sidebar addTableColumn:col];
    self.sidebar.dataSource = self;
    self.sidebar.delegate = self;
    self.sidebar.headerView = nil;
    self.sidebar.rowHeight = 24;
    self.sidebar.style = NSTableViewStyleSourceList;
    self.sidebar.backgroundColor = [NSColor clearColor];
    self.sidebarScroll = [NSScrollView new];
    self.sidebarScroll.documentView = self.sidebar;
    self.sidebarScroll.hasVerticalScroller = YES;
    self.sidebarScroll.drawsBackground = YES;
    self.sidebarScroll.backgroundColor = [NSColor windowBackgroundColor];
    self.sidebarScroll.borderType = NSNoBorder;
    self.sidebarScroll.translatesAutoresizingMaskIntoConstraints = NO;
    [self.sidebarScroll.widthAnchor constraintEqualToConstant:kSidebar].active = YES;
    self.sidebarScroll.hidden = YES;
}

/// A local agent's card: read-only facts and contacts from its client's
/// status, and the one thing the owner can do for it: accept a code.
- (void)buildAgentCard {
    self.agentFacts = [self wrap:@""];
    self.agentFacts.textColor = [NSColor secondaryLabelColor];
    self.agentContacts = [NSTableView new];
    NSArray *cols = @[@[@"name", @140], @[@"relationship", @90], @[@"paired", @90], @[@"via", @110]];
    for (NSArray *c in cols) {
        NSTableColumn *col = [[NSTableColumn alloc] initWithIdentifier:c[0]];
        col.width = [c[1] doubleValue];
        [self.agentContacts addTableColumn:col];
    }
    self.agentContacts.dataSource = self;
    self.agentContacts.delegate = self;
    self.agentContacts.rowHeight = 20;
    self.agentContacts.headerView = nil;
    self.agentContacts.usesAlternatingRowBackgroundColors = YES;
    NSScrollView *scroll = [NSScrollView new];
    scroll.documentView = self.agentContacts;
    scroll.hasVerticalScroller = YES;
    scroll.borderType = NSBezelBorder;
    scroll.translatesAutoresizingMaskIntoConstraints = NO;
    [scroll.heightAnchor constraintEqualToConstant:120].active = YES;
    [scroll.widthAnchor constraintEqualToConstant:kInner - 28].active = YES;
    self.agentPending = [self wrap:@""];
    self.agentNote = [self wrap:@""];
    self.agentNote.textColor = [NSColor secondaryLabelColor];
    NSTextField *hint = [self wrap:@"The agent manages itself; this is what its client reports. A pairing you make here is marked as added by you, so the agent knows it came from you."];
    hint.font = [NSFont systemFontOfSize:11];
    hint.textColor = [NSColor tertiaryLabelColor];
    NSButton *pair = [self button:@"Pair with code…" action:@selector(pairAgentWithCode)];
    NSButton *reload = [self button:@"Refresh" action:@selector(readAgentsNow)];
    NSStackView *col = [self column:@[self.agentFacts, [self label:@"Contacts" size:12 weight:NSFontWeightSemibold], scroll,
                                      self.agentPending, self.agentNote, hint, [self buttons:@[reload, pair]]]];
    self.agentCard = [self card:@"" content:col];
    self.agentCard.hidden = YES;
}

- (void)buildSetupCard {
    self.setupRuntime = [self wrap:@""];
    NSTextField *q = [self label:@"What should your agent call this computer?" size:13 weight:NSFontWeightRegular];
    self.nameField = [NSTextField textFieldWithString:@""];
    self.nameField.placeholderString = @"Max's Mac";
    [self.nameField.widthAnchor constraintEqualToConstant:300].active = YES;
    NSTextField *hint = [self wrap:@"Already have an agent on this Mac? It installs the myous client itself (see the skill). This app runs workers: computers an agent can use."];
    hint.textColor = [NSColor secondaryLabelColor];
    hint.font = [NSFont systemFontOfSize:12];
    NSTextField *dq = [self label:@"How should it describe this computer to your agent?" size:13 weight:NSFontWeightRegular];
    self.descriptionField = [self textArea:420];
    NSTextField *dhint = [self wrap:@"Sent to the agent when they pair, and again when you change it: it is how the agent tells this computer, and its browser, from its own."];
    dhint.textColor = [NSColor secondaryLabelColor];
    dhint.font = [NSFont systemFontOfSize:12];
    NSTextField *bq = [self label:@"Where should its browser run?" size:13 weight:NSFontWeightRegular];
    self.setupBrowser = [NSPopUpButton new];
    self.setupBrowserHint = [self wrap:@""];
    self.setupBrowserHint.textColor = [NSColor secondaryLabelColor];
    self.setupBrowserHint.font = [NSFont systemFontOfSize:12];
    // A pull-down of the browsers the app can run, each to its download page.
    self.setupGetBrowser = [[NSPopUpButton alloc] initWithFrame:NSZeroRect pullsDown:YES];
    [self.setupGetBrowser addItemWithTitle:@"Get a browser…"];
    for (NSMenuItem *it in [self getBrowserMenu].itemArray) { [[self.setupGetBrowser menu] addItem:[it copy]]; }
    [self.setupGetBrowser setContentCompressionResistancePriority:NSLayoutPriorityRequired forOrientation:NSLayoutConstraintOrientationHorizontal];
    self.setupStart = [self button:@"Start the worker" action:@selector(setupStart:)];
    self.setupStart.keyEquivalent = @"\r";
    self.setupRemove = [self button:@"Remove this worker" action:@selector(removeWorker)];
    NSStackView *col = [self column:@[self.setupRuntime, q, self.nameField, dq, self.descriptionField, dhint, bq, [self row:@[self.setupBrowser, self.setupGetBrowser]], self.setupBrowserHint, hint, [self buttons:@[self.setupRemove, self.setupStart]]]];
    self.setupCard = [self card:@"Set up" content:col];
}

- (void)buildRuntimeCard {
    self.runtimeText = [self wrap:@""];
    self.getDockerButton = [self button:@"Get Docker Desktop" action:@selector(getDocker)];
    self.getOrbButton = [self button:@"Get OrbStack" action:@selector(getOrb)];
    self.openRuntimeButton = [self button:@"Open Docker Desktop" action:@selector(openRuntime)];
    self.directToggle = [self button:@"▸ Run without a container (not recommended)" action:@selector(toggleDirectInfo)];
    self.directToggle.bezelStyle = NSBezelStyleInline;
    self.directToggle.bordered = NO;
    self.directWarning = [self wrap:@"Commands your agent sends will run on this Mac as you, with everything you can reach, and only the review hook stands in the way. The myous client must be installed (pip install myous). Use a container unless you know why you don't want one."];
    self.directWarning.textColor = [NSColor systemOrangeColor];
    self.directStart = [self button:@"Run without a container" action:@selector(useDirectAndStart)];
    NSStackView *col = [self column:@[self.runtimeText, [self row:@[self.getDockerButton, self.getOrbButton, self.openRuntimeButton]], self.directToggle,
                                      self.directWarning, self.directStart]];
    self.directWarning.hidden = self.directStart.hidden = YES;
    self.runtimeCard = [self card:@"Set up" content:col];
}

- (void)buildStartingCard {
    NSMutableArray *rows = [NSMutableArray new];
    for (int i = 0; i < 4; i++) {
        NSTextField *l = [self label:@"" size:13 weight:NSFontWeightRegular];
        [rows addObject:l];
    }
    self.startRows = rows;
    self.startNote = [self wrap:@"About two minutes the first time. This Mac must stay awake."];
    self.startNote.textColor = [NSColor secondaryLabelColor];
    self.startRetry = [self button:@"Show log" action:@selector(openLog)];
    NSStackView *col = [self column:[rows arrayByAddingObjectsFromArray:@[self.startNote, [self buttons:@[self.startRetry]]]]];
    self.startingCard = [self card:@"Starting" content:col];
}

- (void)buildPairCard {
    NSTextField *t = [self label:@"Paste this to your agent:" size:13 weight:NSFontWeightRegular];
    self.pairMessage = [self wrap:@""];
    self.pairMessage.font = [NSFont systemFontOfSize:12];
    self.pairMessage.textColor = [NSColor secondaryLabelColor];
    NSBox *msgBox = [NSBox new];
    msgBox.boxType = NSBoxCustom;
    msgBox.cornerRadius = 6;
    msgBox.fillColor = [NSColor controlBackgroundColor];
    msgBox.borderColor = [NSColor separatorColor];
    msgBox.contentViewMargins = NSMakeSize(8, 8);
    msgBox.translatesAutoresizingMaskIntoConstraints = NO;
    self.pairMessage.translatesAutoresizingMaskIntoConstraints = NO;
    [msgBox.contentView addSubview:self.pairMessage];
    [NSLayoutConstraint activateConstraints:@[
        [self.pairMessage.leadingAnchor constraintEqualToAnchor:msgBox.contentView.leadingAnchor],
        [self.pairMessage.trailingAnchor constraintEqualToAnchor:msgBox.contentView.trailingAnchor],
        [self.pairMessage.topAnchor constraintEqualToAnchor:msgBox.contentView.topAnchor],
        [self.pairMessage.bottomAnchor constraintEqualToAnchor:msgBox.contentView.bottomAnchor],
        [msgBox.widthAnchor constraintEqualToConstant:kInner - 28],
    ]];
    NSButton *copy = [self button:@"Copy message" action:@selector(copyMessage)];
    copy.keyEquivalent = @"\r";
    NSButton *qr = [self button:@"Show QR code" action:@selector(showQR:)];
    self.pairCode = [self label:@"" size:13 weight:NSFontWeightRegular];
    self.pairBar = [NSProgressIndicator new];
    self.pairBar.style = NSProgressIndicatorStyleBar;
    self.pairBar.indeterminate = NO;
    self.pairBar.minValue = 0;
    self.pairBar.maxValue = 1;
    [self.pairBar.widthAnchor constraintEqualToConstant:120].active = YES;
    NSButton *newCode = [self button:@"New code" action:@selector(newCode)];
    self.pairWait = [self label:@"Waiting for your agent… (it accepts in about a minute)" size:12 weight:NSFontWeightRegular];
    self.pairWait.textColor = [NSColor secondaryLabelColor];
    // "Which agent?" appears when an agent lives on this Mac: the app can
    // let it accept the code, so the owner types nothing.
    self.pairWhich = [NSPopUpButton new];
    self.pairWhich.target = self;
    self.pairWhich.action = @selector(pairWhichChanged);
    self.pairWhichRow = [self row:@[[self label:@"Which agent?" size:13 weight:NSFontWeightRegular], self.pairWhich]];
    self.pairWhichRow.hidden = YES;
    self.pairLocalButton = [self button:@"" action:@selector(acceptAsLocalAgent)];
    self.pairLocalNote = [self wrap:@""];
    self.pairLocalNote.textColor = [NSColor secondaryLabelColor];
    self.pairLocalRow = [self column:@[self.pairLocalButton, self.pairLocalNote]];
    self.pairLocalRow.hidden = YES;
    NSStackView *buttonsRow = [self buttons:@[copy, qr]];
    self.pairMessageViews = @[t, msgBox, buttonsRow];
    NSStackView *col = [self column:@[self.pairWhichRow, t, msgBox, buttonsRow, self.pairLocalRow, [self row:@[self.pairCode, self.pairBar, newCode]], self.pairWait]];
    self.pairCard = [self card:@"Pair with your agent" content:col];
}

- (void)pairWhichChanged {
    BOOL local = self.pairWhich.indexOfSelectedItem > 0 && !self.pairWhichRow.hidden;
    for (NSView *v in self.pairMessageViews) v.hidden = local;
    self.pairLocalRow.hidden = !local;
    if (local) {
        LocalAgent *a = self.agents[MIN(self.agents.count - 1, (NSUInteger)self.pairWhich.indexOfSelectedItem - 1)];
        self.pairLocalButton.title = [NSString stringWithFormat:@"Let %@ accept the code", a.displayName];
        if (!self.pairLocalNote.stringValue.length)
            self.pairLocalNote.stringValue = [NSString stringWithFormat:@"%@ pairs with this worker as its owner's own worker; the contact is marked as added by you.", a.displayName];
    }
    [self fitWindow];
}

- (void)acceptAsLocalAgent {
    NSInteger i = self.pairWhich.indexOfSelectedItem - 1;
    if (i < 0 || (NSUInteger)i >= self.agents.count) return;
    LocalAgent *a = self.agents[i];
    NSString *code = str(dict(self.status.status[@"invite"])[@"code"]);
    if (!code) return;
    self.pairLocalButton.enabled = NO;
    self.pairLocalNote.stringValue = [NSString stringWithFormat:@"Asking %@ to accept the code…", a.displayName];
    [self append:[NSString stringWithFormat:@"%@ accepts %@ as %@", a.binary, code, a.displayName]];
    __weak typeof(self) weak = self;
    [Agents run:@[@"accept", code, @"--wait", @"90", @"--relationship", @"other", @"--sharing", @"my own worker; run commands there for me", @"--added-by", @"owner"]
             as:a done:^(int status, NSString *output) {
        weak.pairLocalButton.enabled = YES;
        [weak append:output];
        weak.pairLocalNote.stringValue = status == 0
            ? [NSString stringWithFormat:@"%@ accepted. The worker confirms in a few seconds.", a.displayName]
            : [NSString stringWithFormat:@"%@ couldn't accept: %@", a.displayName, [output componentsSeparatedByString:@"\n"].lastObject ?: @"see the log"];
        [weak readAgentsNow];
    }];
}

#pragma mark - agents on this Mac

- (void)readAgentsNow { self.agents = nil; [self refresh]; }

/// Ask every local client for its status, in the background.
- (void)readAgents {
    if (self.agentsBusy) return;
    self.agentsBusy = YES;
    __weak typeof(self) weak = self;
    [Agents read:[Agents homes:self.appConfig.agentHomes] done:^(NSArray<LocalAgent *> *agents) {
        weak.agentsBusy = NO;
        weak.agents = agents;
        if (weak.selectedAgent) {
            LocalAgent *same = nil;
            for (LocalAgent *a in agents) if ([a.home isEqualToString:weak.selectedAgent.home]) same = a;
            weak.selectedAgent = same;
        }
        [weak refresh];
    }];
}

- (void)fillAgent {
    LocalAgent *a = self.selectedAgent;
    self.agentCard.title = a.error ? @"Agent" : [NSString stringWithFormat:@"%@'s contacts and pairings", a.displayName];
    if (a.error) {
        self.agentFacts.stringValue = [NSString stringWithFormat:@"%@\n%@", a.home, a.error];
        self.agentFacts.textColor = [NSColor systemRedColor];
    } else {
        NSMutableArray *facts = [NSMutableArray arrayWithObject:a.shortNpub];
        if (a.version) [facts addObject:[NSString stringWithFormat:@"%@ client %@", a.client ?: @"myous", a.version]];
        if (a.lastUsed) [facts addObject:[NSString stringWithFormat:@"last used %@", timeAgo(a.lastUsed)]];
        if (a.unread) [facts addObject:[NSString stringWithFormat:@"%ld unread", (long)a.unread]];
        self.agentFacts.stringValue = [NSString stringWithFormat:@"%@\n%@", [facts componentsJoinedByString:@"   ·   "], a.home];
        self.agentFacts.textColor = [NSColor secondaryLabelColor];
    }
    self.agentPending.stringValue = a.pending.count
        ? [@"Pairing in progress: " stringByAppendingString:[a.pending componentsJoinedByString:@"; "]]
        : @"No pairing in progress.";
    [self.agentContacts reloadData];
}

- (void)pairAgentWithCode {
    LocalAgent *a = self.selectedAgent;
    if (!a || a.error) return;
    NSAlert *alert = [NSAlert new];
    alert.messageText = [NSString stringWithFormat:@"Pair %@ with a code", a.displayName];
    alert.informativeText = @"Paste the link or code another agent's owner gave you. The agent accepts it on your behalf and the contact is marked as added by you.";
    NSTextField *field = [NSTextField textFieldWithString:@""];
    field.placeholderString = @"4821-K7F3QX or https://myoushq.com/p/…";
    field.frame = NSMakeRect(0, 0, 300, 24);
    alert.accessoryView = field;
    [alert addButtonWithTitle:@"Pair"];
    [alert addButtonWithTitle:@"Cancel"];
    alert.window.initialFirstResponder = field;
    if ([alert runModal] != NSAlertFirstButtonReturn) return;
    NSString *code = [field.stringValue stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceAndNewlineCharacterSet]];
    if (!code.length) return;
    self.agentNote.stringValue = [NSString stringWithFormat:@"Asking %@ to accept…", a.displayName];
    [self append:[NSString stringWithFormat:@"%@ accepts %@ as %@", a.binary, code, a.displayName]];
    __weak typeof(self) weak = self;
    [Agents run:@[@"accept", code, @"--wait", @"90", @"--added-by", @"owner"] as:a done:^(int status, NSString *output) {
        [weak append:output];
        weak.agentNote.stringValue = status == 0
            ? [NSString stringWithFormat:@"%@ accepted. Tell it how you know the new contact (it will ask).", a.displayName]
            : [NSString stringWithFormat:@"%@ couldn't accept: %@", a.displayName, [output componentsSeparatedByString:@"\n"].lastObject ?: @"see the log"];
        [weak readAgentsNow];
    }];
}

- (void)menu:(NSMenu *)menu add:(NSString *)title worker:(Worker *)w sel:(SEL)sel {
    NSMenuItem *item = [menu addItemWithTitle:title action:@selector(workerMenuAction:) keyEquivalent:@""];
    item.representedObject = @{@"home": w.paths.home, @"sel": NSStringFromSelector(sel)};
}

- (void)workerMenuAction:(NSMenuItem *)item {
    for (Worker *w in self.workers) if ([w.paths.home isEqualToString:item.representedObject[@"home"]]) self.current = w;
    self.selectedAgent = nil;
    SEL sel = NSSelectorFromString(item.representedObject[@"sel"]);
    #pragma clang diagnostic push
    #pragma clang diagnostic ignored "-Warc-performSelector-leaks"
    [self performSelector:sel];
    #pragma clang diagnostic pop
}

- (void)selectWorkerFromMenu:(NSMenuItem *)item {
    for (Worker *w in self.workers) if ([w.paths.home isEqualToString:item.representedObject]) self.current = w;
    self.selectedAgent = nil;
    self.nameField.stringValue = @"";
    [self refresh];
    [self showWindow];
}

/// A new worker: its own folder, key, container and pairing; the mode and
/// checkout come from the first worker. Its Set up screen asks for the name.
- (void)addWorker {
    NSString *home = [Paths newHome];
    if (!home) return;
    Worker *w = [[Worker alloc] initWithHome:home];
    w.config.mode = self.appConfig.mode;
    w.config.repo = self.appConfig.repo;
    [w.config write];
    [self.workers addObject:w];
    self.current = w;
    self.selectedAgent = nil;
    self.nameField.stringValue = @"";
    [self append:[NSString stringWithFormat:@"new worker folder %@", home]];
    [self refresh];
    [self showWindow];
}

/// A worker is set up once it has a name or a key: removing it then loses
/// something (its identity, logins, files, the agent's access), so it asks.
- (BOOL)isSetUp:(Worker *)w {
    return w.config.name != nil || [[NSFileManager defaultManager] fileExistsAtPath:[w.paths.home stringByAppendingPathComponent:@"key"]];
}

/// Remove the current worker: the first worker stays (its folder holds the
/// app's settings). A worker never set up goes without a question.
- (void)removeWorker {
    Worker *w = self.current;
    if (w.isDefault) return;
    BOOL setUp = [self isSetUp:w];
    BOOL running = w.screen == ScreenRunning || w.screen == ScreenPair || w.screen == ScreenPaired || w.screen == ScreenStarting || w.screen == ScreenStopping;
    if (setUp) {
        NSAlert *a = [NSAlert new];
        a.messageText = [NSString stringWithFormat:@"Remove %@?", w.name];
        a.informativeText = [NSString stringWithFormat:@"%@Its folder (key, logins, files) goes to the Trash. The agent paired with it loses it; pairing again means a new code.",
                             running ? @"Stops it and removes its container. " : @""];
        [a addButtonWithTitle:@"Remove"];
        [a addButtonWithTitle:@"Cancel"];
        if ([a runModal] != NSAlertFirstButtonReturn) return;
    }
    NSString *home = w.paths.home;
    void (^trash)(void) = ^{
        [[NSFileManager defaultManager] trashItemAtURL:[NSURL fileURLWithPath:home] resultingItemURL:nil error:nil];
    };
    [self append:[NSString stringWithFormat:@"removing worker %@ (%@)", w.name, home]];
    if (!setUp || ![w.config usesDocker]) {
        if (w.direct) [self stopDirect];
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)((w.direct ? 2 : 0) * NSEC_PER_SEC)), dispatch_get_main_queue(), trash);
    } else {
        [self runLogged:[[self composePrefix] stringByAppendingString:@" down --remove-orphans"] in:home line:nil done:^(int status) { trash(); }];
    }
    [self.workers removeObject:w];
    self.current = self.workers.firstObject;
    self.nameField.stringValue = @"";
    [self refresh];
}

- (void)selectAgentFromMenu:(NSMenuItem *)item {
    for (LocalAgent *a in self.agents) if ([a.home isEqualToString:item.representedObject]) self.selectedAgent = a;
    [self refresh];
    [self showWindow];
}

- (void)addAgentHome {
    NSOpenPanel *panel = [NSOpenPanel openPanel];
    panel.canChooseDirectories = YES;
    panel.canChooseFiles = NO;
    panel.showsHiddenFiles = YES;
    panel.message = @"Choose an agent's myous folder (the one with its key file).";
    panel.directoryURL = [NSURL fileURLWithPath:NSHomeDirectory()];
    if ([panel runModal] != NSModalResponseOK) return;
    NSString *home = panel.URL.path;
    if (![[NSFileManager defaultManager] fileExistsAtPath:[home stringByAppendingPathComponent:@"key"]]) {
        NSAlert *alert = [NSAlert new];
        alert.messageText = @"No key file there";
        alert.informativeText = @"An agent's folder holds a file named key. This one doesn't.";
        [alert runModal];
        return;
    }
    self.appConfig.agentHomes = [(self.appConfig.agentHomes ?: @[]) arrayByAddingObject:home];
    self.appConfig.showAgents = YES;
    [self.appConfig write];
    [self readAgentsNow];
}

- (void)buildPairedCard {
    self.pairedText = [self wrap:@""];
    self.pairedCode = [self label:@"" size:26 weight:NSFontWeightBold];
    self.pairedCode.font = [NSFont monospacedSystemFontOfSize:26 weight:NSFontWeightBold];
    self.pairedCode.selectable = YES;
    NSTextField *note = [self wrap:@"Your agent shows the same number. If it doesn't, press Unpair and start again."];
    note.textColor = [NSColor secondaryLabelColor];
    NSButton *unpair = [self button:@"Unpair" action:@selector(unpair)];
    NSButton *cont = [self button:@"Continue" action:@selector(acknowledgePairing)];
    cont.keyEquivalent = @"\r";
    NSStackView *col = [self column:@[self.pairedText, [self row:@[[self label:@"Verification code" size:13 weight:NSFontWeightRegular], self.pairedCode]],
                                      note, [self buttons:@[unpair, cont]]]];
    self.pairedCard = [self card:@"Paired ✓" content:col];
}

- (void)buildRequestsCard {
    self.requestsTitle = [self label:@"" size:12 weight:NSFontWeightRegular];
    self.requestsTitle.textColor = [NSColor secondaryLabelColor];
    NSView *spacer = [NSView new];
    [spacer setContentHuggingPriority:1 forOrientation:NSLayoutConstraintOrientationHorizontal];
    self.pauseButton = [self button:@"Pause" action:@selector(togglePaused)];
    self.stopCommandButton = [self button:@"Stop this command" action:@selector(stopCommand)];
    self.stopCommandButton.hidden = YES;
    NSStackView *top = [self row:@[self.requestsTitle, spacer, self.stopCommandButton, self.pauseButton]];
    [top.widthAnchor constraintEqualToConstant:kInner - 28].active = YES;
    self.pausedNote = [self wrap:@"Paused: requests are refused until you resume."];
    self.pausedNote.textColor = [NSColor systemOrangeColor];
    // A question from the review hook: the agent is waiting for Allow or Refuse.
    self.approvalText = [self wrap:@""];
    self.approvalText.preferredMaxLayoutWidth = kInner - 200;
    self.allowButton = [self button:@"Allow" action:@selector(allowRequest)];
    self.allowButton.keyEquivalent = @"\r";
    self.refuseButton = [self button:@"Refuse" action:@selector(refuseRequest)];
    self.approvalRow = [self row:@[self.approvalText, self.allowButton, self.refuseButton]];

    self.table = [NSTableView new];
    NSArray *cols = @[@[@"time", @"Time", @44], @[@"agent", @"Agent", @90], @[@"op", @"", @36], @[@"what", @"What", @250], @[@"outcome", @"Outcome", @70]];
    for (NSArray *c in cols) {
        NSTableColumn *col = [[NSTableColumn alloc] initWithIdentifier:c[0]];
        col.title = c[1];
        col.width = [c[2] doubleValue];
        [self.table addTableColumn:col];
    }
    self.table.dataSource = self;
    self.table.delegate = self;
    self.table.rowHeight = 20;
    self.table.usesAlternatingRowBackgroundColors = YES;
    self.table.doubleAction = @selector(showRequestDetail);
    self.table.target = self;
    self.table.headerView = nil;
    NSScrollView *scroll = [NSScrollView new];
    scroll.documentView = self.table;
    scroll.hasVerticalScroller = YES;
    scroll.borderType = NSBezelBorder;
    scroll.translatesAutoresizingMaskIntoConstraints = NO;
    [scroll.heightAnchor constraintEqualToConstant:200].active = YES;
    [scroll.widthAnchor constraintEqualToConstant:kInner - 28].active = YES;
    NSTextField *hint = [self label:@"Double-click a row for the command and its output." size:11 weight:NSFontWeightRegular];
    hint.textColor = [NSColor tertiaryLabelColor];
    NSButton *log = [self button:@"Open log" action:@selector(openLog)];
    NSStackView *col = [self column:@[top, self.pausedNote, self.approvalRow, scroll, [self row:@[hint, [self buttons:@[log]]]]]];
    self.requestsCard = [self card:@"Requests" content:col];
}

- (void)buildBrowserCard {
    NSTextField *t = self.browserText = [self wrap:@"Your agent can use sites you're logged into here. Open it to log in or to watch."];
    NSButton *open = self.browserOpen = [self button:@"Open browser" action:@selector(openBrowserView)];
    NSStackView *col = [self column:@[t, [self buttons:@[open]]]];
    self.browserCard = [self card:@"Browser" content:col];
}

- (void)buildStoppingCard {
    NSProgressIndicator *spin = [NSProgressIndicator new];
    spin.style = NSProgressIndicatorStyleSpinning;
    spin.controlSize = NSControlSizeSmall;
    [spin startAnimation:nil];
    self.stoppingText = [self wrap:@"Stopping the worker. Its container shuts down in a few seconds; your agent's requests are refused meanwhile."];
    self.stoppingCard = [self card:@"Stopping" content:[self row:@[spin, self.stoppingText]]];
}

- (void)buildStoppedCard {
    self.stoppedText = [self wrap:@"The worker is stopped. Your agent can't reach this Mac until you start it."];
    NSButton *start = [self button:@"Start" action:@selector(start)];
    start.keyEquivalent = @"\r";
    self.stoppedRemove = [self button:@"Remove this worker…" action:@selector(removeWorker)];
    NSStackView *col = [self column:@[self.stoppedText, [self buttons:@[self.stoppedRemove, start]]]];
    self.stoppedCard = [self card:@"Stopped" content:col];
}

- (void)fitWindow {
    [self.root layoutSubtreeIfNeeded];
    CGFloat h = self.root.fittingSize.height;
    CGFloat w = kWidth + (self.sidebarScroll.hidden ? 0 : kSidebar);
    if (fabs(h - self.lastHeight) < 1 && fabs(w - self.lastWidth) < 1) return;
    self.lastHeight = h;
    self.lastWidth = w;
    NSRect frame = self.window.frame;
    NSRect content = [self.window frameRectForContentRect:NSMakeRect(0, 0, w, h)];
    frame.origin.y += frame.size.height - content.size.height;
    frame.size = content.size;
    [self.window setFrame:frame display:YES animate:NO];
}

- (void)showWindow {
    [self.window makeKeyAndOrderFront:nil];
    [NSApp activateIgnoringOtherApps:YES];
}

- (void)windowDidBecomeKey:(NSNotification *)n {
    if (n.object != self.window || self.fake) return;
    // New requests are the ones since the window was last in front.
    for (Worker *w in self.workers) { w.config.seenRequestsAt = [[NSDate date] timeIntervalSince1970]; [w.config write]; }
    [self updateBadge];
}

#pragma mark - state

- (BOOL)isPaused { return [[NSFileManager defaultManager] fileExistsAtPath:[self.paths paused]]; }

- (BOOL)isPausedFor:(Worker *)w { return [[NSFileManager defaultManager] fileExistsAtPath:[w.paths paused]]; }

- (NSString *)stateWordFor:(Worker *)w {
    switch (w.screen) {
        case ScreenSetup: case ScreenNoRuntime: return @"Not set up";
        case ScreenStarting: return @"Starting…";
        case ScreenPair: return @"Running · not paired";
        case ScreenPaired: case ScreenRunning: return [self isPausedFor:w] ? @"Paused" : @"Running";
        case ScreenStopping: return @"Stopping…";
        case ScreenStopped: return @"Stopped";
    }
    return @"";
}

- (NSColor *)stateColorFor:(Worker *)w {
    if (w.attention || w.approvals.count) return [NSColor systemRedColor];
    switch (w.screen) {
        case ScreenStarting: case ScreenStopping: return [NSColor systemBlueColor];
        case ScreenPair: case ScreenPaired: case ScreenRunning: return [self isPausedFor:w] ? [NSColor systemOrangeColor] : [NSColor systemGreenColor];
        default: return [NSColor systemGrayColor];
    }
}

- (NSString *)stateWord { return [self stateWordFor:self.current]; }
- (NSColor *)stateColor { return [self stateColorFor:self.current]; }

- (void)refresh {
    if (!self.fake) {
        ++self.ticks;
        if (self.ticks % kAgentsEvery == 0) [self loadWorkers];   // a folder added by hand
        if (self.ticks % 5 == 0 && [self.config usesDocker]) [self probeRuntimeAsync];
        if (self.appConfig.showAgents && (self.agents == nil || self.ticks % kAgentsEvery == 0)) [self readAgents];
        if (!self.appConfig.showAgents) { self.agents = @[]; self.selectedAgent = nil; }
    }
    // Every worker's state (the menu bar shows the worst; each one notifies), then the window for the current one.
    Worker *shown = self.current;
    for (Worker *w in self.workers) { self.current = w; [self evaluate]; }
    self.current = shown;
    [self render];
}

/// The current worker's screen and attention, from its files; notifications
/// for what changed. No UI.
- (void)evaluate {
    if (self.fake) [self applyFake]; else {
        self.config = [AppConfig readAt:self.paths];
        self.status = [StatusFile readAt:self.paths];
    }
    [self loadRequestsIfChanged];
    if (!self.fake) self.approvals = loadApprovals(self.paths);
    NSDictionary *s = self.status.status;
    NSString *phase = [self.status phase] ?: @"";
    BOOL fresh = [self.status fresh];
    // Stop finished and the worker has written nothing since: it is gone,
    // no need to wait for its last status to go stale. This holds until
    // the next start (not only while "Stopping" shows), or the still-recent
    // file would count as alive again on the next tick and the window
    // would flip to Running until the file went stale.
    if (fresh && self.current.stopDoneAt && [self.status.modified timeIntervalSince1970] < self.current.stopDoneAt) fresh = NO;
    // No phase: a worker from before v0.6.0, which is running if it writes.
    BOOL alive = fresh && ([phase isEqualToString:@"running"] || [phase isEqualToString:@"paused"] || !phase.length);
    BOOL booting = fresh && ([phase isEqualToString:@"starting"] || [phase isEqualToString:@"browser"] || [phase isEqualToString:@"registering"]);
    double now = [[NSDate date] timeIntervalSince1970];
    BOOL launching = self.launchStage != nil || (self.launchedAt && now - self.launchedAt < 90 && !alive);
    BOOL runtimeProblem = [self.config usesDocker] && self.runtimeState && ![self.runtimeState isEqualToString:@"ok"];
    double stoppingFor = self.stopping && self.stoppedAt ? now - self.stoppedAt : 0;
    if (alive && !self.stopping) self.launchedAt = 0;
    if (alive && stoppingFor > 90) { self.stopping = NO; self.stoppedAt = 0; }   // it didn't stop; say so below

    // An existing install from before names were a setting: take the worker's.
    if (!self.config.name && str(s[@"alias"]) && !self.fake) { self.config.name = str(s[@"alias"]); [self.config write]; }

    Screen screen;
    NSString *attention = nil, *attentionButton = nil;
    if (!self.config.name) {
        screen = runtimeProblem && ![self.runtimeState isEqualToString:@"stopped"] ? ScreenNoRuntime : ScreenSetup;
        if ([self.runtimeState isEqualToString:@"stopped"]) screen = ScreenNoRuntime;
    } else if (runtimeProblem) {
        screen = ScreenNoRuntime;
    } else if (self.stopping && (alive || booting)) {
        screen = ScreenStopping;
    } else if (launching || booting) {
        screen = ScreenStarting;
    } else if (alive) {
        NSDictionary *invite = dict(s[@"invite"]);
        NSDictionary *paired = dict(s[@"paired"]);
        if (num(s[@"contacts"]).integerValue == 0 && str(invite[@"code"])) screen = ScreenPair;
        else if (paired && num(paired[@"at"]).doubleValue > self.config.seenPairedAt) screen = ScreenPaired;
        else screen = ScreenRunning;
        if (self.stoppedAt && now - self.stoppedAt > 90 && now - self.stoppedAt < 120) { attention = @"The worker didn't stop. The log says why."; attentionButton = @"Show log"; }
        NSString *why = [self.config macBrowser] && !self.fake ? [MacBrowser unavailableReason] : nil;
        if (why && !attention) {
            attention = [why containsString:@"sandbox"]
                ? [NSString stringWithFormat:@"The worker's browser can't run on this Mac: %@. Choose the browser in the container (Advanced › Browser).", why]
                : @"The worker's browser can't run on this Mac: none of Google Chrome, Microsoft Edge, Brave or Chromium is installed. Any one will do (all free); or choose the browser in the container (Advanced › Browser).";
            attentionButton = [why containsString:@"sandbox"] ? @"Show log" : @"Get a browser…";
        }
    } else {
        screen = ScreenStopped;
        if (self.stopping) { self.stopping = NO; self.stoppedAt = 0; self.current.stoppedByUs = YES; }
        if ([phase hasPrefix:@"error"] && fresh) { attention = [phase substringFromIndex:MIN(phase.length, 7)]; attentionButton = @"Show log"; }
        else if (self.wasRunning && !self.current.stoppedByUs) { attention = @"The worker stopped on its own."; attentionButton = @"Start"; [self notifyOnce:@"stopped" title:@"myous worker stopped" body:@"The worker stopped on its own. Open myous to start it again."]; }
        else if (self.launchedAt && !launching) { attention = @"The worker didn't start. The log says why."; attentionButton = @"Show log"; }
    }
    if (alive) { self.wasRunning = YES; self.current.stoppedByUs = NO; }
    if (screen != ScreenStopped) self.wasRunning = alive;
    if (!self.fake) {
        // The browser on this Mac lives and dies with the worker: started
        // when the worker runs without it (the app was relaunched), quit
        // when the worker is gone.
        if ([self.config macBrowser] && (alive || booting) && !self.stopping) [self ensureMacBrowser:self.current];
        else if (self.current.browser.running && screen == ScreenStopped) [self.current.browser stop];
        if (screen == ScreenStopped && self.current.restartAfterStop) { self.current.restartAfterStop = NO; [self start]; return; }
    }
    self.screen = screen;
    self.current.alive = alive;
    self.current.attention = attention;
    self.current.attentionButton = attentionButton;
    if (screen == ScreenPair) {
        NSDictionary *invite = dict(s[@"invite"]);
        double left = num(invite[@"expires_at"]).doubleValue - [[NSDate date] timeIntervalSince1970];
        if (left > 0 && left < 120 && !self.window.visible)
            [self notifyOnce:[@"expiring-" stringByAppendingString:str(invite[@"code"]) ?: @""]
                       title:[NSString stringWithFormat:@"Still waiting for an agent to pair with %@", self.config.name ?: @"the worker"]
                        body:[NSString stringWithFormat:@"The code %@ expires in %d min; the worker then makes a new one. Open myous to copy the message.", str(invite[@"code"]) ?: @"", (int)ceil(left / 60)]];
    }
    if (screen == ScreenPaired || screen == ScreenRunning) [self notifyPaired:dict(s[@"paired"])];
    [self notifyRefusals];
    [self notifyApproval];
}

- (void)render {
    NSDictionary *s = self.status.status;
    NSString *phase = [self.status phase] ?: @"";
    BOOL alive = self.current.alive;
    Screen screen = self.screen;
    NSString *attention = self.current.attention, *attentionButton = self.current.attentionButton;
    if (self.latestRelease && ![self.latestRelease isEqualToString:self.appConfig.skippedVersion] && !attention) {
        attention = [NSString stringWithFormat:@"myous %@ is available (you have %@).", self.latestRelease, appVersion()];
        attentionButton = @"Download";
    }
    if (self.requests != self.tableRequests) { self.tableRequests = self.requests; [self.table reloadData]; }

    // Header
    NSString *name = self.config.name ?: @"myous";
    NSMutableAttributedString *title = [[NSMutableAttributedString alloc] initWithString:[NSString stringWithFormat:@"● %@ · %@", [self stateWord], name]];
    [title addAttribute:NSForegroundColorAttributeName value:[self stateColor] range:NSMakeRange(0, 1)];
    [title addAttribute:NSFontAttributeName value:self.headTitle.font range:NSMakeRange(0, title.length)];
    self.headTitle.attributedStringValue = title;
    NSDictionary *paired = dict(s[@"paired"]);
    NSString *pairedWith = str(paired[@"alias"]);
    (void)phase;
    if (!pairedWith && num(s[@"contacts"]).integerValue > 0) pairedWith = @"your agent";
    self.headRight.stringValue = screen == ScreenSetup || screen == ScreenNoRuntime ? @"" : pairedWith ? [NSString stringWithFormat:@"Paired · %@", pairedWith] : @"Not paired yet";
    NSMutableArray *facts = [NSMutableArray new];
    if (alive && self.config.name) {
        NSUInteger today = 0;
        for (NSDictionary *r in self.requests) if ([dayLabel(num(r[@"at"]).doubleValue) isEqualToString:@"Today"]) today++;
        [facts addObject:[NSString stringWithFormat:@"Requests today: %lu", (unsigned long)today]];
        NSDictionary *last = dict(s[@"last"]);
        if (num(last[@"at"])) [facts addObject:[NSString stringWithFormat:@"Last: %@", timeAgo(num(last[@"at"]).doubleValue)]];
        else [facts addObject:@"No requests yet"];
    } else if (self.runtimeName && [self.config usesDocker] && self.config.name) {
        [facts addObject:[NSString stringWithFormat:@"Runs in %@", self.runtimeName]];
    }
    self.headFacts.stringValue = [facts componentsJoinedByString:@"   ·   "];
    self.headFacts.hidden = facts.count == 0;

    // Banner
    self.banner.stringValue = attention ?: @"";
    self.banner.hidden = self.bannerButton.hidden = attention == nil;
    self.bannerButton.title = attentionButton ?: @"";
    self.banner.superview.hidden = attention == nil;

    // Cards
    self.setupCard.hidden = screen != ScreenSetup;
    self.runtimeCard.hidden = screen != ScreenNoRuntime;
    self.startingCard.hidden = screen != ScreenStarting;
    self.pairCard.hidden = screen != ScreenPair;
    self.pairedCard.hidden = screen != ScreenPaired;
    self.stoppedCard.hidden = screen != ScreenStopped;
    self.stoppingCard.hidden = screen != ScreenStopping;
    self.setupRemove.hidden = self.stoppedRemove.hidden = self.current.isDefault;
    if (screen == ScreenStopping) self.stoppingText.stringValue = [self.config isDirect]
        ? @"Stopping the worker. Your agent's requests are refused from now on."
        : @"Stopping the worker. Its container shuts down in a few seconds; your agent's requests are refused meanwhile.";
    self.requestsCard.hidden = !(screen == ScreenRunning || screen == ScreenPaired);
    self.browserCard.hidden = self.requestsCard.hidden || [self.config isDirect];
    if (!self.browserCard.hidden) {
        if ([self.config macBrowser]) {
            NSString *app = self.current.browser.appName ?: [MacBrowser find:self.config.browserApp][@"name"] ?: @"the browser";
            BOOL up = self.current.browser.port > 0 || self.fake;
            self.browserText.stringValue = up
                ? [NSString stringWithFormat:@"Your agent uses %@ on this Mac, with its own profile kept to the worker's folder. Log into sites there; it is you at the keyboard, so sites behave. Say \"%@'s browser\" to your agent when you mean this one.", app, self.config.name ?: @"the worker"]
                : [NSString stringWithFormat:@"Starting %@ on this Mac… (browser.log in the worker folder says why if it doesn't).", app];
            self.browserOpen.title = @"Show browser";
        } else {
            self.browserText.stringValue = [NSString stringWithFormat:@"Your agent can use sites you're logged into here. Open it to log in or to watch. Say \"%@'s browser\" to your agent when you mean this one.", self.config.name ?: @"the worker"];
            self.browserOpen.title = @"Open browser";
        }
    }
    self.stoppedCard.hidden = screen != ScreenStopped || attention != nil || !self.config.name;
    if (screen == ScreenStopped && !self.stoppedCard.hidden) self.stoppedText.stringValue = @"The worker is stopped. Your agent can't reach this Mac until you start it.";

    switch (screen) {
        case ScreenSetup: [self fillSetup]; break;
        case ScreenNoRuntime: [self fillRuntime]; break;
        case ScreenStarting: [self fillStarting:phase]; break;
        case ScreenPair: [self fillPair:dict(s[@"invite"])]; break;
        case ScreenPaired: [self fillPaired:paired]; break;
        default: break;
    }
    if (!self.requestsCard.hidden) [self fillRequests];
    [self fillApproval];
    [self fillSidebar];
    if (self.selectedAgent) {
        // An agent's card replaces the worker's step; the header describes the agent.
        for (NSBox *c in @[self.setupCard, self.runtimeCard, self.startingCard, self.pairCard, self.pairedCard, self.stoppingCard, self.stoppedCard, self.requestsCard, self.browserCard]) c.hidden = YES;
        self.agentCard.hidden = NO;
        [self fillAgent];
        LocalAgent *a = self.selectedAgent;
        NSMutableAttributedString *t = [[NSMutableAttributedString alloc] initWithString:[NSString stringWithFormat:@"○ %@ · agent on this Mac", a.displayName]];
        [t addAttribute:NSFontAttributeName value:self.headTitle.font range:NSMakeRange(0, t.length)];
        self.headTitle.attributedStringValue = t;
        self.headRight.stringValue = a.error ? @"" : [NSString stringWithFormat:@"%lu contact%@", (unsigned long)a.contacts.count, a.contacts.count == 1 ? @"" : @"s"];
        self.headFacts.hidden = YES;
    } else {
        self.agentCard.hidden = YES;
    }
    // Pairing: offer the local agents as the other side.
    if (!self.pairCard.hidden) {
        NSMutableArray *titles = [NSMutableArray arrayWithObject:@"Another agent (paste the message)"];
        for (LocalAgent *a in self.agents) if (!a.error) [titles addObject:[NSString stringWithFormat:@"%@ on this Mac", a.displayName]];
        if (![[self.pairWhich.itemTitles valueForKey:@"description"] isEqualToArray:titles]) {
            NSInteger keep = self.pairWhich.indexOfSelectedItem;
            [self.pairWhich removeAllItems];
            [self.pairWhich addItemsWithTitles:titles];
            if (keep > 0 && keep < (NSInteger)titles.count) [self.pairWhich selectItemAtIndex:keep];
        }
        self.pairWhichRow.hidden = titles.count < 2;
        if ([self.fake isEqualToString:@"pairlocal"] && titles.count > 1) [self.pairWhich selectItemAtIndex:1];
        [self pairWhichChanged];
    }

    self.statusItem.button.image = statusIcon([self worstColor], [self requestInProgress]);
    [self updateBadge];
    [self fitWindow];
}

/// The menu bar colour: the worst state over every worker.
- (NSColor *)worstColor {
    NSArray *order = @[[NSColor systemRedColor], [NSColor systemBlueColor], [NSColor systemOrangeColor], [NSColor systemGreenColor], [NSColor systemGrayColor]];
    NSColor *worst = [NSColor systemGrayColor];
    for (Worker *w in self.workers) {
        NSColor *c = [self stateColorFor:w];
        if ([order indexOfObject:c] < [order indexOfObject:worst]) worst = c;
    }
    return worst;
}

/// On this Mac: the worker and the local agents, shown once there are two.
- (void)fillSidebar {
    NSMutableArray *rows = [NSMutableArray new];
    [rows addObject:@{@"kind": @"group", @"title": @"WORKERS"}];
    for (Worker *w in self.workers) [rows addObject:@{@"kind": @"worker", @"title": w.name, @"home": w.paths.home}];
    if (self.workers.count > 1 || self.agents.count) [rows addObject:@{@"kind": @"add", @"title": @"Add a worker…"}];
    if (self.agents.count) {
        [rows addObject:@{@"kind": @"group", @"title": @"AGENTS"}];
        for (LocalAgent *a in self.agents) [rows addObject:@{@"kind": @"agent", @"title": a.displayName, @"home": a.home}];
    }
    BOOL show = self.agents.count > 0 || self.workers.count > 1;
    if (!show) self.selectedAgent = nil;
    if (![rows isEqualToArray:self.sidebarRows]) {
        self.sidebarRows = rows;
        [self.sidebar reloadData];
    }
    NSInteger want = 1;
    NSString *selectedHome = self.selectedAgent ? self.selectedAgent.home : self.current.paths.home;
    for (NSUInteger i = 0; i < rows.count; i++) if ([rows[i][@"home"] isEqualToString:selectedHome]) want = i;
    if (self.sidebar.selectedRow != want) [self.sidebar selectRowIndexes:[NSIndexSet indexSetWithIndex:want] byExtendingSelection:NO];
    self.sidebarScroll.hidden = !show;
}

- (void)fillSetup {
    if (self.nameField.stringValue.length == 0) {
        NSString *host = [[NSHost currentHost] localizedName] ?: @"My Mac";
        self.nameField.stringValue = host;
        self.descriptionField.stringValue = defaultDescription(host);
    }
    self.setupRuntime.stringValue = [self.runtimeState isEqualToString:@"ok"]
        ? [NSString stringWithFormat:@"✓ %@ found. The worker runs in a container there, so your agent's commands stay inside it.", self.runtimeName]
        : @"✓ Ready.";
    // One choice per installed browser ("On this Mac, in …", the first
    // recommended), then the container. With none installed, one disabled
    // line says so, and "Get a browser…" lists where each one is.
    BOOL fakeSetup = [self.fake hasPrefix:@"setup"];   // "setup": Chrome and Edge there; "setupnobrowser": none
    BOOL fakeNone = [self.fake isEqualToString:@"setupnobrowser"];
    NSArray *installed = fakeSetup ? @[@{@"id": @"com.google.Chrome", @"name": @"Google Chrome"}, @{@"id": @"com.microsoft.edgemac", @"name": @"Microsoft Edge"}] : [MacBrowser installed];
    NSString *why = fakeNone ? @"no browser it can run is installed" : fakeSetup ? nil : [MacBrowser unavailableReason];
    if (why) installed = @[];
    NSMutableArray *titles = [NSMutableArray new], *ids = [NSMutableArray new];
    if (why) { [titles addObject:[NSString stringWithFormat:@"On this Mac (%@)", why]]; [ids addObject:@""]; }
    for (NSDictionary *b in installed) {
        [titles addObject:[NSString stringWithFormat:@"On this Mac, in %@%@", b[@"name"], b == installed.firstObject ? @" (recommended)" : @""]];
        [ids addObject:b[@"id"]];
    }
    [titles addObject:@"In the container"];
    [ids addObject:@""];
    if (![self.setupBrowser.itemTitles isEqualToArray:titles]) {
        [self.setupBrowser removeAllItems];
        [self.setupBrowser addItemsWithTitles:titles];
        self.setupBrowserIds = ids;
        if (why) [self.setupBrowser itemAtIndex:0].enabled = NO;
        BOOL mac = self.config.browser ? [self.config.browser isEqualToString:@"mac"] : installed.count > 0;
        NSString *want = fakeSetup ? nil : [MacBrowser find:self.config.browserApp][@"id"];
        NSUInteger at = want ? [ids indexOfObject:want] : NSNotFound;
        [self.setupBrowser selectItemAtIndex:mac && installed.count ? (at == NSNotFound ? 0 : (NSInteger)at) : (NSInteger)titles.count - 1];
    }
    self.setupGetBrowser.hidden = installed.count > 0 || [why containsString:@"sandbox"];
    BOOL macChosen = [self.setupBrowserIds[MAX(0, self.setupBrowser.indexOfSelectedItem)] length] > 0;
    self.setupBrowserHint.stringValue = macChosen
        ? @"A real browser with its own profile, kept to the worker's folder by a sandbox: sites see an ordinary Mac, and you use the window itself."
        : why && ![why containsString:@"sandbox"]
        ? @"A Chromium inside the container, shown through a window in your browser. Some sites take it for a bot, even when it's you. To run a real browser on this Mac instead, install any of Google Chrome, Microsoft Edge, Brave or Chromium (all free): the choice above turns on by itself."
        : @"A Chromium inside the container, shown through a window in your browser. Some sites take it for a bot, even when it's you.";
    self.setupBrowser.target = self;
    self.setupBrowser.action = @selector(setupBrowserChanged);
}

- (void)setupBrowserChanged { [self render]; }

/// Where to get each browser the app can run (for the Set up pull-down and
/// the banner's button).
- (NSMenu *)getBrowserMenu {
    NSMenu *m = [NSMenu new];
    for (NSDictionary *k in [MacBrowser known]) {
        NSMenuItem *it = [m addItemWithTitle:k[@"name"] action:@selector(getBrowser:) keyEquivalent:@""];
        it.target = self;
        it.representedObject = k[@"url"];
    }
    return m;
}
- (void)getBrowser:(NSMenuItem *)item { [[NSWorkspace sharedWorkspace] openURL:[NSURL URLWithString:item.representedObject]]; }

- (void)fillRuntime {
    BOOL stopped = [self.runtimeState isEqualToString:@"stopped"];
    self.runtimeText.stringValue = stopped
        ? [NSString stringWithFormat:@"✗ %@ is installed but not running. Open it and wait for it to finish starting; this page updates by itself.", self.runtimeName]
        : @"✗ No container runtime found. The worker runs in a container so your agent's commands stay inside it, not on your Mac. Docker Desktop and OrbStack are both free for personal use.";
    self.getDockerButton.hidden = self.getOrbButton.hidden = stopped;
    self.openRuntimeButton.hidden = !stopped;
    self.openRuntimeButton.title = [NSString stringWithFormat:@"Open %@", self.runtimeName];
    self.directToggle.hidden = stopped;
    if (stopped) self.directWarning.hidden = self.directStart.hidden = YES;
}

- (void)fillStarting:(NSString *)phase {
    // Four rows: runtime, download, browser, registration.
    BOOL pulled = self.launchStage == nil || [self.launchStage isEqualToString:@"creating"];
    BOOL inContainer = phase.length > 0 && [self.status fresh];
    BOOL browserDone = inContainer && ([phase isEqualToString:@"registering"] || [phase isEqualToString:@"running"]);
    BOOL error = [phase hasPrefix:@"error"];
    double elapsed = self.launchedAt ? [[NSDate date] timeIntervalSince1970] - self.launchedAt : 0;
    NSString *clock = elapsed > 0 ? [NSString stringWithFormat:@"  %d:%02d", (int)elapsed / 60, (int)elapsed % 60] : @"";
    NSArray *texts = @[
        @"✓ Container runtime ready",
        [NSString stringWithFormat:@"%@ Downloading the worker (first time only)%@", pulled ? @"✓" : @"⟳", pulled ? @"" : clock],
        [self.config macBrowser]
            ? [NSString stringWithFormat:@"%@ Starting %@ on this Mac", self.current.browser.port > 0 ? @"✓" : @"⟳", self.current.browser.appName ?: @"the browser"]
            : [NSString stringWithFormat:@"%@ Starting the browser%@", browserDone ? @"✓" : (pulled && inContainer) ? @"⟳" : @"○", (!browserDone && pulled && inContainer) ? clock : @""],
        [NSString stringWithFormat:@"%@ Registering with myoushq.com%@", error ? @"✗" : browserDone ? @"⟳" : @"○", error ? [@": " stringByAppendingString:[phase substringFromIndex:MIN(phase.length, 7)]] : browserDone ? clock : @""],
    ];
    for (NSUInteger i = 0; i < 4; i++) {
        self.startRows[i].stringValue = texts[i];
        self.startRows[i].textColor = [texts[i] hasPrefix:@"○"] ? [NSColor tertiaryLabelColor] : [texts[i] hasPrefix:@"✗"] ? [NSColor systemRedColor] : [NSColor labelColor];
    }
    BOOL slow = elapsed > 300 || (pulled && elapsed > 180);
    self.startNote.stringValue = slow ? @"This is taking longer than usual. The log shows what's happening." : @"About two minutes the first time. This Mac must stay awake.";
    self.startNote.textColor = slow ? [NSColor systemOrangeColor] : [NSColor secondaryLabelColor];
}

- (void)fillPair:(NSDictionary *)invite {
    NSString *code = str(invite[@"code"]) ?: @"";
    NSString *alias = self.config.name ?: @"worker";
    self.pairMessage.stringValue = str(invite[@"message"]) ?: [NSString stringWithFormat:
        @"Pair with my worker \"%@\" (an environment I set up for you, not something to run yourself): accept the pairing code %@ "
        @"with relationship other and sharing \"my own worker; run commands there for me\". Then send it the message help, "
        @"and read the Workers section of https://myoushq.com/skill.md before using it.", alias, code];
    double left = num(invite[@"expires_at"]).doubleValue - [[NSDate date] timeIntervalSince1970];
    if (left < 0) left = 0;
    self.pairCode.stringValue = [NSString stringWithFormat:@"Code %@ · valid for %d:%02d", code, (int)left / 60, (int)left % 60];
    self.pairBar.doubleValue = MIN(1, left / kInviteSeconds);
    self.lastQRLink = str(invite[@"link"]) ?: code;
}

- (void)fillPaired:(NSDictionary *)paired {
    self.pairedText.stringValue = [NSString stringWithFormat:@"Paired with %@.", str(paired[@"alias"]) ?: @"your agent"];
    self.pairedCode.stringValue = str(paired[@"verify"]) ?: @"";
}

/// A `stop-<id>` command file: the worker kills the command's process group.
- (void)stopCommand {
    NSDictionary *r = [self runningRequest];
    if (!r) return;
    sendWorkerCommand(self.paths, [@"stop-" stringByAppendingString:str(r[@"id"]) ?: @""]);
    [self append:[NSString stringWithFormat:@"stop: %@", str(r[@"cmd"]) ?: @""]];
    self.stopCommandButton.enabled = NO;
}

- (NSDictionary *)runningRequest {
    NSDictionary *r = self.requests.firstObject;
    BOOL running = r && [str(r[@"op"]) isEqualToString:@"exec"] && [str(r[@"decision"]) isEqualToString:@"allow"] && !r[@"done_at"]
        && [[NSDate date] timeIntervalSince1970] - num(r[@"at"]).doubleValue < 700;
    return running ? r : nil;
}

- (void)fillRequests {
    NSDictionary *running = [self runningRequest];
    self.stopCommandButton.hidden = running == nil;
    if (!running) self.stopCommandButton.enabled = YES;
    BOOL paused = [self isPaused];
    self.pausedNote.hidden = !paused;
    self.pauseButton.title = paused ? @"Resume" : @"Pause";
    self.requestsTitle.stringValue = self.requests.count ? [NSString stringWithFormat:@"%lu recent, newest first", (unsigned long)self.requests.count]
                                                        : @"Nothing yet. Ask your agent to run something on this Mac.";
}

- (void)fillApproval {
    NSDictionary *q = self.approvals.firstObject;
    self.approvalRow.hidden = q == nil;
    if (!q) return;
    NSString *what = str(q[@"cmd"]) ?: str(q[@"path"]) ?: @"";
    NSString *verb = [str(q[@"op"]) isEqualToString:@"exec"] ? @"run" : [str(q[@"op"]) isEqualToString:@"put"] ? @"write" : @"read";
    double left = num(q[@"asked_at"]).doubleValue + num(q[@"wait"]).doubleValue - [[NSDate date] timeIntervalSince1970];
    self.approvalText.stringValue = [NSString stringWithFormat:@"%@ wants to %@ %@%@", str(q[@"alias"]) ?: @"Your agent", verb, what,
                                     left > 0 ? [NSString stringWithFormat:@"  (%d s left)", (int)left] : @""];
    self.approvalText.textColor = [NSColor systemOrangeColor];
}

- (void)notifyApproval {
    NSDictionary *q = self.approvals.firstObject;
    if (!q) return;
    NSString *what = str(q[@"cmd"]) ?: str(q[@"path"]) ?: @"";
    NSString *verb = [str(q[@"op"]) isEqualToString:@"exec"] ? @"run" : [str(q[@"op"]) isEqualToString:@"put"] ? @"write" : @"read";
    NSString *key = [@"ask-" stringByAppendingString:str(q[@"id"]) ?: @""];
    [self notifyOnce:key title:[NSString stringWithFormat:@"%@ wants to %@ on %@", str(q[@"alias"]) ?: @"Your agent", verb, self.config.name ?: @"the worker"]
                body:what category:@"approval"];
}

- (void)allowRequest { [self answer:@"allow"]; }
- (void)refuseRequest { [self answer:@"refuse"]; }
- (void)answer:(NSString *)verdict {
    NSDictionary *q = self.approvals.firstObject;
    if (!q) return;
    answerApproval(self.paths, str(q[@"id"]) ?: @"", verdict);
    [self append:[NSString stringWithFormat:@"%@: %@ %@", verdict, str(q[@"op"]) ?: @"", str(q[@"cmd"]) ?: str(q[@"path"]) ?: @""]];
    self.approvals = @[];
    self.approvalRow.hidden = YES;
    [[UNUserNotificationCenter currentNotificationCenter] removeDeliveredNotificationsWithIdentifiers:@[[@"ask-" stringByAppendingString:str(q[@"id"]) ?: @""]]];
    [self fitWindow];
}

- (BOOL)requestInProgress {
    NSDictionary *r = self.requests.firstObject;
    return r && [str(r[@"decision"]) isEqualToString:@"allow"] && !r[@"done_at"] && [[NSDate date] timeIntervalSince1970] - num(r[@"at"]).doubleValue < 700;
}

- (void)loadRequestsIfChanged {
    if (self.fake) return;
    NSDictionary *attrs = [[NSFileManager defaultManager] attributesOfItemAtPath:[self.paths requests] error:nil];
    NSDate *d = attrs[NSFileModificationDate];
    if (self.requests && ((!d && !self.requestsDirDate) || [d isEqualToDate:self.requestsDirDate])) return;
    self.requestsDirDate = d;
    self.requests = loadRequests(self.paths, kRequestRows);
}

- (void)updateBadge {
    NSUInteger fresh = 0;
    if (!self.window.keyWindow || !self.window.visible) {
        for (Worker *w in self.workers) for (NSDictionary *r in w.requests) if (num(r[@"at"]).doubleValue > w.config.seenRequestsAt) fresh++;
    }
    self.statusItem.button.title = fresh ? [NSString stringWithFormat:@" %lu", (unsigned long)fresh] : @"";
    NSApp.dockTile.badgeLabel = fresh ? [NSString stringWithFormat:@"%lu", (unsigned long)fresh] : nil;
}

- (void)probeRuntimeAsync {
    __weak typeof(self) weak = self;
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_UTILITY, 0), ^{
        NSString *state = [Runtime probe];
        NSString *name = [Runtime name];
        dispatch_async(dispatch_get_main_queue(), ^{
            BOOL changed = ![state isEqualToString:weak.runtimeState] || ![name isEqualToString:weak.runtimeName];
            weak.runtimeState = state;
            weak.runtimeName = name;
            if (changed) [weak refresh];
        });
    });
}

#pragma mark - fake states (snapshots)

- (void)applyFake {
    NSString *f = self.fake;
    double now = [[NSDate date] timeIntervalSince1970];
    if ([f isEqualToString:@"workers"] && self.workers.count < 2) {
        Worker *w = [[Worker alloc] initWithHome:[[Paths defaultHome] stringByAppendingString:@"-2"]];
        w.config.name = @"Family Mac";
        w.status = [StatusFile new];
        [self.workers addObject:w];
    }
    if (!self.current.isDefault) {
        // The second fake worker: stopped, paired with a second agent.
        self.config.name = @"Family Mac";
        StatusFile *st = [StatusFile new];
        st.modified = [NSDate dateWithTimeIntervalSinceNow:-3600];
        st.status = @{@"alias": @"Family Mac", @"contacts": @1, @"paired": @{@"alias": @"Sam's Muse", @"at": @(now - 86400)}};
        self.status = st;
        self.runtimeState = @"ok";
        self.config.seenPairedAt = now;
        return;
    }
    NSMutableDictionary *s = [@{@"alias": @"Max's Mac", @"contacts": @1, @"requests": @12, @"phase": @"running",
                                @"paired": @{@"alias": @"Max's Muse", @"verify": @"358806", @"at": @(now - 3600)},
                                @"last": @{@"op": @"exec", @"at": @(now - 120), @"alias": @"Max's Muse", @"ok": @YES}} mutableCopy];
    self.runtimeState = @"ok";
    self.runtimeName = @"Docker Desktop";
    self.config.name = @"Max's Mac";
    self.config.seenPairedAt = now;
    StatusFile *st = [StatusFile new];
    st.modified = [NSDate date];
    if ([f hasPrefix:@"setup"]) { self.config.name = nil; [s removeObjectForKey:@"alias"]; }
    else if ([f isEqualToString:@"noruntime"]) { self.config.name = nil; [s removeObjectForKey:@"alias"]; self.runtimeState = @"missing"; }
    else if ([f isEqualToString:@"stoppedruntime"]) { self.runtimeState = @"stopped"; }
    else if ([f isEqualToString:@"starting"]) { s[@"phase"] = @"browser"; s[@"contacts"] = @0; self.launchedAt = now - 75; }
    else if ([f isEqualToString:@"pair"]) { s[@"contacts"] = @0; [s removeObjectForKey:@"paired"];
        s[@"invite"] = @{@"code": @"4821-K7F3QX", @"link": @"https://myoushq.com/p/4821#K7F3QX", @"expires_at": @(now + 702)}; }
    else if ([f isEqualToString:@"paired"]) { self.config.seenPairedAt = 0; }
    else if ([f isEqualToString:@"paused"]) { s[@"phase"] = @"paused"; }
    else if ([f isEqualToString:@"busy"]) { /* a command in progress: the requests below get one */ }
    else if ([f isEqualToString:@"macbrowser"]) { self.config.browser = @"mac"; }
    else if ([f isEqualToString:@"approval"]) { self.approvals = @[@{@"id": @"q1", @"op": @"exec", @"alias": @"Max's Muse", @"cmd": @"rm -rf /work/old", @"asked_at": @(now - 20), @"wait": @120}]; }
    else if ([f isEqualToString:@"stopped"]) { st.modified = [NSDate dateWithTimeIntervalSinceNow:-3600]; }
    else if ([f isEqualToString:@"stopping"]) { self.current.stopping = YES; self.current.stoppedAt = now - 3; }
    else if ([f isEqualToString:@"agents"] || [f isEqualToString:@"agent"] || [f isEqualToString:@"pairlocal"]) {
        if (!self.agents.count) {
            LocalAgent *a = [LocalAgent new];
            a.home = [NSHomeDirectory() stringByAppendingPathComponent:@".myous"];
            a.binary = [a.home stringByAppendingPathComponent:@"venv/bin/myous"];
            a.alias = @"Claude Code"; a.npub = @"npub1q7w9k2m4x8e6r3t5y7u9i1o3p5a7s9d1f3g5h7j9k1l3z5x7c9v1b3n5m7x4f";
            a.client = @"python"; a.version = appVersion(); a.lastUsed = now - 180; a.unread = 1;
            a.contacts = @[@{@"alias": @"Max's Muse", @"npub": @"npub1muse", @"status": @"approved", @"relationship": @"colleague", @"paired_at": @(now - 86400 * 2)},
                           @{@"alias": @"Max's Mac", @"npub": @"npub1mac", @"status": @"approved", @"relationship": @"other", @"added_by": @"owner", @"paired_at": @(now - 3600)}];
            a.pending = @[];
            LocalAgent *b = [LocalAgent new];
            b.home = [NSHomeDirectory() stringByAppendingPathComponent:@".myous-codex"];
            b.binary = @"/usr/local/bin/myous";
            b.alias = @"Codex"; b.npub = @"npub1codex000000000000000000000000000000000000000000000000000q2p"; b.client = @"go"; b.version = appVersion();
            b.contacts = @[]; b.pending = @[@"4821: waiting for the other agent to join, expires in 11 min"]; b.lastUsed = now - 86400 * 3;
            self.agents = @[a, b];
        }
        if ([f isEqualToString:@"agent"]) self.selectedAgent = self.agents.firstObject;
        if ([f isEqualToString:@"pairlocal"]) {
            s[@"contacts"] = @0; [s removeObjectForKey:@"paired"];
            s[@"invite"] = @{@"code": @"4821-K7F3QX", @"link": @"https://myoushq.com/p/4821#K7F3QX", @"expires_at": @(now + 702)};
            if (self.pairWhich.numberOfItems > 1) [self.pairWhich selectItemAtIndex:1];
        }
    }
    st.status = s;
    self.status = st;
    if (!self.requests) {
        self.requests = @[
            [f isEqualToString:@"busy"]
            ? @{@"id": @"0", @"op": @"exec", @"alias": @"Max's Muse", @"cmd": @"python3 /work/scrape.py --all", @"decision": @"allow", @"at": @(now - 40)}
            : @{@"id": @"1", @"op": @"exec", @"alias": @"Max's Muse", @"cmd": @"python3 /work/open_page.py", @"decision": @"allow", @"at": @(now - 120), @"done_at": @(now - 118), @"exit": @0, @"duration": @1.4, @"stdout": @"ok\n", @"stderr": @""},
            @{@"id": @"2", @"op": @"put", @"alias": @"Max's Muse", @"path": @"open_page.py", @"size": @537, @"decision": @"allow", @"at": @(now - 130), @"done_at": @(now - 129)},
            @{@"id": @"3", @"op": @"get", @"alias": @"Max's Muse", @"path": @"cp-test.txt", @"decision": @"allow", @"at": @(now - 400), @"done_at": @(now - 399), @"size": @12},
            @{@"id": @"4", @"op": @"exec", @"alias": @"Max's Muse", @"cmd": @"cat /etc/passwd", @"decision": @"refuse", @"reason": @"path outside the work directory", @"at": @(now - 3000), @"done_at": @(now - 3000)},
            @{@"id": @"5", @"op": @"exec", @"alias": @"Max's Muse", @"cmd": @"ls -la", @"decision": @"allow", @"at": @(now - 90000), @"done_at": @(now - 89999), @"exit": @0, @"duration": @0.1},
        ];
        [self.table reloadData];
    }
}

#pragma mark - requests table

- (NSInteger)numberOfRowsInTableView:(NSTableView *)tableView {
    if (tableView == self.sidebar) return self.sidebarRows.count;
    if (tableView == self.agentContacts) return self.selectedAgent.contacts.count;
    return self.requests.count;
}

- (BOOL)tableView:(NSTableView *)tv isGroupRow:(NSInteger)row {
    return tv == self.sidebar && [self.sidebarRows[row][@"kind"] isEqualToString:@"group"];
}

- (BOOL)tableView:(NSTableView *)tv shouldSelectRow:(NSInteger)row {
    return tv != self.sidebar || ![self.sidebarRows[row][@"kind"] isEqualToString:@"group"];
}

- (void)tableViewSelectionDidChange:(NSNotification *)n {
    if (n.object != self.sidebar || self.sidebar.selectedRow < 0) return;
    NSDictionary *r = self.sidebarRows[self.sidebar.selectedRow];
    if ([r[@"kind"] isEqualToString:@"add"]) { [self addWorker]; return; }
    LocalAgent *pick = nil;
    for (LocalAgent *a in self.agents) if ([a.home isEqualToString:r[@"home"]]) pick = a;
    Worker *worker = nil;
    for (Worker *w in self.workers) if ([w.paths.home isEqualToString:r[@"home"]]) worker = w;
    if (pick != self.selectedAgent || (worker && worker != self.current)) {
        self.selectedAgent = pick;
        if (worker) { self.current = worker; self.nameField.stringValue = @""; }
        [self refresh];
    }
}

- (NSView *)sidebarCell:(NSInteger)row {
    NSDictionary *r = self.sidebarRows[row];
    BOOL group = [r[@"kind"] isEqualToString:@"group"];
    NSTextField *cell = [self.sidebar makeViewWithIdentifier:group ? @"group" : @"item" owner:self];
    if (!cell) {
        cell = [NSTextField labelWithString:@""];
        cell.identifier = group ? @"group" : @"item";
        cell.font = group ? [NSFont systemFontOfSize:11 weight:NSFontWeightSemibold] : [NSFont systemFontOfSize:13];
        cell.textColor = group ? [NSColor tertiaryLabelColor] : [NSColor labelColor];
        cell.lineBreakMode = NSLineBreakByTruncatingTail;
    }
    NSString *prefix = group ? @"" : [r[@"kind"] isEqualToString:@"worker"] ? @"● " : [r[@"kind"] isEqualToString:@"add"] ? @"+ " : @"○ ";
    NSMutableAttributedString *t = [[NSMutableAttributedString alloc] initWithString:[prefix stringByAppendingString:r[@"title"]]];
    [t addAttribute:NSFontAttributeName value:cell.font range:NSMakeRange(0, t.length)];
    [t addAttribute:NSForegroundColorAttributeName value:[r[@"kind"] isEqualToString:@"add"] ? [NSColor secondaryLabelColor] : cell.textColor range:NSMakeRange(0, t.length)];
    if ([r[@"kind"] isEqualToString:@"worker"]) {
        for (Worker *w in self.workers) if ([w.paths.home isEqualToString:r[@"home"]]) [t addAttribute:NSForegroundColorAttributeName value:[self stateColorFor:w] range:NSMakeRange(0, 1)];
    }
    cell.attributedStringValue = t;
    return cell;
}

- (NSView *)contactCell:(NSTableColumn *)col row:(NSInteger)row {
    NSDictionary *c = self.selectedAgent.contacts[row];
    NSTextField *cell = [self.agentContacts makeViewWithIdentifier:col.identifier owner:self];
    if (!cell) {
        cell = [NSTextField labelWithString:@""];
        cell.identifier = col.identifier;
        cell.font = [NSFont systemFontOfSize:12];
        cell.lineBreakMode = NSLineBreakByTruncatingTail;
    }
    NSString *id_ = col.identifier, *text = @"";
    if ([id_ isEqualToString:@"name"]) text = str(c[@"alias"]) ?: str(c[@"npub"]) ?: @"";
    else if ([id_ isEqualToString:@"relationship"]) text = [str(c[@"status"]) isEqualToString:@"blocked"] ? @"blocked" : str(c[@"relationship"]) ?: @"";
    else if ([id_ isEqualToString:@"paired"]) text = num(c[@"paired_at"]) ? dayLabel(num(c[@"paired_at"]).doubleValue) : @"";
    else text = [str(c[@"added_by"]) isEqualToString:@"owner"] ? @"paired by you" : str(c[@"added_by"]) ?: @"";
    cell.stringValue = text;
    cell.textColor = [id_ isEqualToString:@"name"] ? [NSColor labelColor] : [NSColor secondaryLabelColor];
    cell.toolTip = str(c[@"sharing"]) ?: str(c[@"npub"]);
    return cell;
}

- (NSView *)tableView:(NSTableView *)tv viewForTableColumn:(NSTableColumn *)col row:(NSInteger)row {
    if (tv == self.sidebar) return [self sidebarCell:row];
    if (tv == self.agentContacts) return [self contactCell:col row:row];
    NSDictionary *r = self.requests[row];
    NSString *id_ = col.identifier;
    NSTextField *cell = [tv makeViewWithIdentifier:id_ owner:self];
    if (!cell) {
        cell = [NSTextField labelWithString:@""];
        cell.identifier = id_;
        cell.font = [NSFont systemFontOfSize:12];
        cell.lineBreakMode = NSLineBreakByTruncatingTail;
    }
    NSString *op = str(r[@"op"]) ?: @"";
    NSString *decision = str(r[@"decision"]) ?: @"";
    BOOL refused = [decision isEqualToString:@"refuse"];
    BOOL done = r[@"done_at"] != nil;
    NSString *text = @"";
    NSColor *color = [NSColor labelColor];
    if ([id_ isEqualToString:@"time"]) {
        double at = num(r[@"at"]).doubleValue;
        NSString *day = dayLabel(at);
        text = [day isEqualToString:@"Today"] ? clockTime(at) : day;
        color = [NSColor secondaryLabelColor];
    } else if ([id_ isEqualToString:@"agent"]) {
        text = str(r[@"alias"]) ?: @"?";
    } else if ([id_ isEqualToString:@"op"]) {
        text = [op isEqualToString:@"exec"] ? @"ran" : [op isEqualToString:@"put"] ? @"put" : [op isEqualToString:@"get"] ? @"got" : op;
        color = [NSColor secondaryLabelColor];
    } else if ([id_ isEqualToString:@"what"]) {
        text = str(r[@"cmd"]) ?: str(r[@"path"]) ?: @"";
        if (num(r[@"size"])) text = [NSString stringWithFormat:@"%@ (%@)", text, [NSByteCountFormatter stringFromByteCount:num(r[@"size"]).longLongValue countStyle:NSByteCountFormatterCountStyleFile]];
        if (refused && str(r[@"reason"])) text = [NSString stringWithFormat:@"%@ — %@", text, str(r[@"reason"])];
        cell.font = [NSFont monospacedSystemFontOfSize:11 weight:NSFontWeightRegular];
    } else {
        NSNumber *exit = num(r[@"exit"]);
        BOOL pending = [decision isEqualToString:@"pending"];
        BOOL stopped = num(r[@"stopped"]).boolValue;
        text = refused ? @"refused" : pending ? @"waiting for you" : !done ? @"running…" : stopped ? @"stopped by you" : (exit && exit.intValue != 0) ? [NSString stringWithFormat:@"exit %d", exit.intValue] : @"ok";
        color = refused ? [NSColor systemRedColor] : (pending || stopped || (exit && exit.intValue != 0)) ? [NSColor systemOrangeColor] : [NSColor secondaryLabelColor];
    }
    cell.stringValue = text;
    cell.textColor = color;
    cell.toolTip = str(r[@"reason"]);
    BOOL isNew = num(r[@"at"]).doubleValue > self.config.seenRequestsAt;
    cell.font = isNew ? [NSFont fontWithDescriptor:[cell.font.fontDescriptor fontDescriptorWithSymbolicTraits:NSFontDescriptorTraitBold] size:cell.font.pointSize] : cell.font;
    return cell;
}

- (void)showRequestDetail {
    NSInteger row = self.table.clickedRow;
    if (row < 0 || row >= (NSInteger)self.requests.count) return;
    NSDictionary *r = self.requests[row];
    NSMutableString *t = [NSMutableString new];
    [t appendFormat:@"%@  %@  %@\n", dayLabel(num(r[@"at"]).doubleValue), clockTime(num(r[@"at"]).doubleValue), str(r[@"alias"]) ?: @""];
    NSString *op = str(r[@"op"]) ?: @"";
    if ([op isEqualToString:@"exec"]) [t appendFormat:@"\n$ %@\n", str(r[@"cmd"]) ?: @""];
    else [t appendFormat:@"\n%@ %@\n", op, str(r[@"path"]) ?: @""];
    if ([str(r[@"decision"]) isEqualToString:@"refuse"]) [t appendFormat:@"\nRefused: %@\n", str(r[@"reason"]) ?: @"by the review hook"];
    else {
        if (num(r[@"exit"])) [t appendFormat:@"\nExit %@", num(r[@"exit"])];
        if (num(r[@"duration"])) [t appendFormat:@" in %.1f s", num(r[@"duration"]).doubleValue];
        if (num(r[@"size"])) [t appendFormat:@"\n%@ bytes", num(r[@"size"])];
        [t appendString:@"\n"];
        if (str(r[@"stdout"]).length) [t appendFormat:@"\n--- output ---\n%@", str(r[@"stdout"])];
        if (str(r[@"stderr"]).length) [t appendFormat:@"\n--- errors ---\n%@", str(r[@"stderr"])];
        if (num(r[@"truncated"]).boolValue) [t appendString:@"\n[output was cut to fit]"];
    }
    if (!self.detailWindow) {
        self.detailWindow = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, 640, 420)
            styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable | NSWindowStyleMaskResizable backing:NSBackingStoreBuffered defer:NO];
        self.detailWindow.title = @"Request";
        self.detailWindow.releasedWhenClosed = NO;
        self.detailView = [self textViewIn:self.detailWindow];
    }
    self.detailView.string = t;
    [self.detailWindow center];
    [self.detailWindow makeKeyAndOrderFront:nil];
}

- (NSTextView *)textViewIn:(NSWindow *)w {
    NSScrollView *scroll = [[NSScrollView alloc] initWithFrame:w.contentView.bounds];
    scroll.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
    scroll.hasVerticalScroller = YES;
    NSTextView *tv = [[NSTextView alloc] initWithFrame:scroll.bounds];
    tv.editable = NO;
    tv.font = [NSFont monospacedSystemFontOfSize:11 weight:NSFontWeightRegular];
    tv.textContainerInset = NSMakeSize(6, 6);
    tv.autoresizingMask = NSViewWidthSizable;
    tv.verticallyResizable = YES;
    tv.maxSize = NSMakeSize(FLT_MAX, FLT_MAX);
    tv.textContainer.widthTracksTextView = YES;
    scroll.documentView = tv;
    [w.contentView addSubview:scroll];
    return tv;
}

#pragma mark - setup and start/stop

- (void)setupStart:(id)sender {
    NSString *name = [self.nameField.stringValue stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceAndNewlineCharacterSet]];
    if (!name.length) { [self.window makeFirstResponder:self.nameField]; return; }
    self.config.name = name;
    NSString *desc = [self.descriptionField.stringValue stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceAndNewlineCharacterSet]];
    self.config.descriptionText = desc.length ? desc : nil;   // nil: the default, which follows the name
    if ([self.config usesDocker]) {
        NSInteger at = MAX(0, self.setupBrowser.indexOfSelectedItem);
        NSString *bid = at < (NSInteger)self.setupBrowserIds.count ? self.setupBrowserIds[at] : @"";
        self.config.browser = bid.length && ![MacBrowser unavailableReason] ? @"mac" : @"container";
        if (bid.length) self.config.browserApp = bid;
    }
    [self.config write];
    [self start];
}

- (NSString *)composeFile {
    if ([self.config isImage]) return [Paths bundledCompose];
    return [[self.config.repo stringByAppendingPathComponent:@"worker"] stringByAppendingPathComponent:@"compose.yml"];
}

- (NSString *)composePrefix {
    // One project name whatever the mode, so Stop finds the containers after
    // a mode switch or an app update; the compose file travels in the bundle.
    NSString *file = [[self composeFile] stringByReplacingOccurrencesOfString:@"'" withString:@"'\\''"];
    return [NSString stringWithFormat:@"%@ compose -f '%@' -p %@", [Runtime dockerBin], file, [self.paths project]];
}

- (NSDictionary *)composeEnv {
    NSMutableDictionary *env = [NSMutableDictionary new];
    if (self.config.name) env[@"MYOUS_ALIAS"] = self.config.name;
    env[@"MYOUS_DESCRIPTION"] = self.config.descriptionText ?: defaultDescription(self.config.name);
    env[@"MYOUS_WORKER_HOME"] = [self.paths home];
    env[@"MYOUS_BROWSER"] = [self.config macBrowser] ? @"host" : @"container";
    env[@"MYOUS_TZ"] = [NSTimeZone localTimeZone].name ?: @"UTC";
    NSString *lang = [NSLocale preferredLanguages].firstObject;
    if (lang.length) env[@"MYOUS_LANG"] = lang;
    return env;
}

/// The browser on this Mac for a worker, started if it isn't running.
- (void)ensureMacBrowser:(Worker *)w {
    if (!w.browser) {
        w.browser = [[MacBrowser alloc] initWithPaths:w.paths];
        __weak typeof(self) weak = self;
        w.browser.log = ^(NSString *line) { [weak append:line]; };
    }
    w.browser.workerName = w.config.name;
    w.browser.bundleId = w.config.browserApp;
    w.browser.hidden = w.config.browserHidden;
    if (w.browser.running) return;
    NSError *err;
    if (![w.browser start:&err]) [self append:[NSString stringWithFormat:@"browser: %@", err.localizedDescription]];
}

- (void)start {
    [[NSFileManager defaultManager] createDirectoryAtPath:[self.paths home] withIntermediateDirectories:YES attributes:@{NSFilePosixPermissions: @0700} error:nil];
    self.launchedAt = [[NSDate date] timeIntervalSince1970];
    self.stopping = NO;
    self.current.stopDoneAt = 0;   // a status newer than the last stop counts again
    self.wasRunning = NO;
    if ([self.config isDirect]) { [self startDirect]; [self refresh]; return; }
    NSString *file = [self composeFile];
    if (!file || ![[NSFileManager defaultManager] fileExistsAtPath:file]) {
        [self append:[NSString stringWithFormat:@"no compose file at %@", file ?: @"(none)"]];
        self.launchedAt = 0;
        return;
    }
    if ([self.config macBrowser]) [self ensureMacBrowser:self.current];
    else if (self.current.browser.running) [self.current.browser stop];
    self.launchStage = @"pulling";
    NSString *cmd = [[self composePrefix] stringByAppendingString:[self.config isImage] ? @" up -d" : @" up -d --build"];
    __weak typeof(self) weak = self;
    [self runLogged:cmd in:[self.paths home] line:^(NSString *line) {
        if ([line containsString:@"Pulled"] || [line containsString:@"Created"] || [line containsString:@"Started"] || [line containsString:@"Running"])
            weak.launchStage = @"creating";
    } done:^(int status) {
        weak.launchStage = nil;
        if (status != 0) { [weak append:@"the start command failed; see above"]; }
        [weak refresh];
    }];
    [self refresh];
}

- (void)stop {
    self.stopping = YES;
    self.stoppedAt = [[NSDate date] timeIntervalSince1970];
    self.current.stopDoneAt = 0;
    self.launchedAt = 0;
    self.launchStage = nil;
    Worker *w = self.current;
    [w.browser stop];
    if ([self.config isDirect]) { [self stopDirect]; [self refresh]; return; }
    // `down` for this worker's project only; other workers keep running.
    // "Remove stale containers" in Advanced clears anything older.
    NSString *cmd = [[self composePrefix] stringByAppendingString:@" down --remove-orphans"];
    __weak typeof(self) weak = self;
    [self runLogged:cmd in:[self.paths home] line:nil done:^(int status) {
        if (status == 0) w.stopDoneAt = [[NSDate date] timeIntervalSince1970];
        [weak refresh];
    }];
    [self refresh];
}

- (void)rebuild {
    if (!self.config.repo) return;
    [self stop];
    __weak typeof(self) weak = self;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 3 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{ [weak start]; });
}

- (void)removeContainers {
    __weak typeof(self) weak = self;
    [self runLogged:[Runtime removeAllCommand] in:[self.paths home] line:nil done:^(int status) { [weak refresh]; }];
}

- (void)runLogged:(NSString *)cmd in:(NSString *)dir line:(LineBlock)line done:(DoneBlock)done {
    [self append:[@"$ " stringByAppendingString:cmd]];
    __weak typeof(self) weak = self;
    [Runtime run:cmd in:dir env:[self composeEnv] line:^(NSString *l) {
        [weak append:l];
        if (line) line(l);
    } done:^(int status) {
        [weak append:[NSString stringWithFormat:@"exit %d", status]];
        done(status);
    }];
}

- (void)startDirect {
    [[NSFileManager defaultManager] createDirectoryAtPath:[self.paths work] withIntermediateDirectories:YES attributes:nil error:nil];
    NSTask *t = [NSTask new];
    t.launchPath = @"/bin/sh";
    // A login shell, so the user's PATH (where `myous` lives) applies. The
    // name is passed only when it changed (passing it re-registers).
    NSData *settings = [NSData dataWithContentsOfFile:[[self.paths home] stringByAppendingPathComponent:@"settings.json"]];
    NSString *current = settings ? str(dict([NSJSONSerialization JSONObjectWithData:settings options:0 error:nil])[@"alias"]) : nil;
    NSString *name = self.config.name ?: @"Desktop worker";
    NSString *aliasOpt = [name isEqualToString:current] ? @"" :
        [NSString stringWithFormat:@"--alias '%@' ", [name stringByReplacingOccurrencesOfString:@"'" withString:@"'\\''"]];
    NSString *home = [[self.paths home] stringByReplacingOccurrencesOfString:@"'" withString:@"'\\''"];
    NSString *hook = [[Paths bundledReview] stringByReplacingOccurrencesOfString:@"'" withString:@"'\\''"];
    NSString *reviewOpt = hook ? [NSString stringWithFormat:@"--review '%@' ", hook] : @"";
    t.arguments = @[@"-lc", [NSString stringWithFormat:@"exec myous worker %@%@--work '%@/work' >> '%@/worker.log' 2>&1", aliasOpt, reviewOpt, home, home]];
    NSMutableDictionary *env = [[[NSProcessInfo processInfo] environment] mutableCopy];
    env[@"MYOUS_HOME"] = [self.paths home];
    t.environment = env;
    __weak typeof(self) weak = self;
    Worker *w = self.current;
    t.terminationHandler = ^(NSTask *task) {
        dispatch_async(dispatch_get_main_queue(), ^{
            [weak append:[NSString stringWithFormat:@"myous worker exited with status %d", task.terminationStatus]];
            w.direct = nil;
            if (w.stopping) w.stopDoneAt = [[NSDate date] timeIntervalSince1970];
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
        NSNumber *pid = num([StatusFile readAt:self.paths].status[@"pid"]);   // started outside this app
        if (pid) { kill((pid_t)pid.intValue, SIGTERM); [self append:[NSString stringWithFormat:@"sent SIGTERM to worker pid %@", pid]]; }
    }
}

- (void)bannerAction {
    NSString *t = self.bannerButton.title;
    if ([t isEqualToString:@"Start"]) [self start];
    else if ([t isEqualToString:@"Download"]) [[NSWorkspace sharedWorkspace] openURL:[NSURL URLWithString:@"https://myoushq.com/download/mac"]];
    else if ([t hasPrefix:@"Get a browser"]) [[self getBrowserMenu] popUpMenuPositioningItem:nil atLocation:NSMakePoint(0, self.bannerButton.bounds.size.height) inView:self.bannerButton];
    else [self openLog];
}

#pragma mark - runtime card

- (void)getDocker { [[NSWorkspace sharedWorkspace] openURL:[NSURL URLWithString:@"https://www.docker.com/products/docker-desktop/"]]; }
- (void)getOrb { [[NSWorkspace sharedWorkspace] openURL:[NSURL URLWithString:@"https://orbstack.dev/"]]; }
- (void)openRuntime { [Runtime launch:self.runtimeName]; }

- (void)toggleDirectInfo {
    BOOL show = self.directWarning.hidden;
    self.directWarning.hidden = self.directStart.hidden = !show;
    self.directToggle.title = show ? @"▾ Run without a container (not recommended)" : @"▸ Run without a container (not recommended)";
    [self fitWindow];
}

- (void)useDirectAndStart {
    if (![self.config.name length]) {
        self.config.name = [[NSHost currentHost] localizedName] ?: @"My Mac";
    }
    if ([[Runtime output:@"command -v myous"] length] == 0) {
        NSAlert *a = [NSAlert new];
        a.messageText = @"The myous client isn't installed";
        a.informativeText = @"Direct mode runs `myous worker` on this Mac. Install the client first (pip install myous, or ask your agent to), then try again.";
        [a runModal];
        return;
    }
    self.config.mode = @"direct";
    [self.config write];
    if (![[NSFileManager defaultManager] fileExistsAtPath:[self.paths review]]) setReviewLevel(self.paths, @"changes");
    [self start];
}

- (void)useDirect {
    self.config.mode = @"direct";
    [self.config write];
    // No container around the commands: ask before anything that changes things, unless the owner chose otherwise.
    if (![[NSFileManager defaultManager] fileExistsAtPath:[self.paths review]]) setReviewLevel(self.paths, @"changes");
    [self append:@"mode: direct (no container)"];
    [self refresh];
}
/// Advanced › Browser: the item's representedObject is a bundle id (a
/// browser on this Mac) or "" (the container).
- (void)chooseBrowserItem:(NSMenuItem *)item {
    NSString *bid = item.representedObject;
    NSString *which = bid.length ? @"mac" : @"container";
    NSString *current = [MacBrowser find:self.config.browserApp][@"id"] ?: @"";
    if ([self.config.browser ?: @"container" isEqualToString:which] && (!bid.length || [bid isEqualToString:current])) return;
    self.config.browser = which;
    if (bid.length) self.config.browserApp = bid;
    [self.config write];
    [self append:[NSString stringWithFormat:@"browser: %@", bid.length ? [NSString stringWithFormat:@"on this Mac, in %@", [MacBrowser find:bid][@"name"]] : @"in the container"]];
    BOOL up = self.screen == ScreenRunning || self.screen == ScreenPair || self.screen == ScreenPaired || self.screen == ScreenStarting;
    if (up) {
        NSAlert *a = [NSAlert new];
        a.messageText = @"Restart the worker to switch its browser?";
        a.informativeText = @"The browser choice applies when the worker starts. Each browser keeps its own logins, so sites may ask you to log in again.";
        [a addButtonWithTitle:@"Restart now"];
        [a addButtonWithTitle:@"Later"];
        if ([a runModal] == NSAlertFirstButtonReturn) { self.current.restartAfterStop = YES; [self stop]; return; }
    }
    [self refresh];
}

- (void)useImage { self.config.mode = @"image"; [self.config write]; [self append:[NSString stringWithFormat:@"mode: the published image ghcr.io/myoushq/worker:%@", appVersion()]]; [self refresh]; }

- (void)chooseRepo {
    NSOpenPanel *panel = [NSOpenPanel openPanel];
    panel.message = @"Choose your myoushq-client checkout (the folder containing worker/)";
    panel.canChooseDirectories = YES;
    panel.canChooseFiles = NO;
    panel.allowsMultipleSelection = NO;
    if ([panel runModal] == NSModalResponseOK && panel.URL) {
        if (![[NSFileManager defaultManager] fileExistsAtPath:[panel.URL.path stringByAppendingPathComponent:@"worker/compose.yml"]]) {
            [self append:[NSString stringWithFormat:@"no worker/compose.yml in %@; is this the myoushq-client checkout?", panel.URL.path]];
            return;
        }
        self.config.repo = panel.URL.path;
        self.config.mode = @"docker";
        [self.config write];
        [self append:[NSString stringWithFormat:@"mode: checkout %@", panel.URL.path]];
        [self refresh];
    }
}

#pragma mark - pairing

- (void)copyMessage {
    NSPasteboard *pb = [NSPasteboard generalPasteboard];
    [pb clearContents];
    [pb setString:self.pairMessage.stringValue forType:NSPasteboardTypeString];
    self.pairWait.stringValue = @"Copied. Waiting for your agent… (it accepts in about a minute)";
}

- (void)showQR:(NSButton *)sender {
    if (!self.qrPopover) {
        self.qrPopover = [NSPopover new];
        self.qrPopover.behavior = NSPopoverBehaviorTransient;
        NSViewController *vc = [NSViewController new];
        NSImageView *iv = [[NSImageView alloc] initWithFrame:NSMakeRect(0, 0, 200, 200)];
        iv.imageScaling = NSImageScaleProportionallyUpOrDown;
        vc.view = iv;
        self.qrPopover.contentViewController = vc;
    }
    ((NSImageView *)self.qrPopover.contentViewController.view).image = [self makeQR:self.lastQRLink side:200];
    [self.qrPopover showRelativeToRect:sender.bounds ofView:sender preferredEdge:NSRectEdgeMaxY];
}

- (void)newCode { sendWorkerCommand(self.paths, @"new-code"); self.pairWait.stringValue = @"Asking the worker for a new code…"; }

- (void)unpair {
    NSAlert *a = [NSAlert new];
    a.messageText = @"Unpair this worker?";
    a.informativeText = @"The agent loses access, and a new pairing code appears. Use this if the verification numbers differ.";
    [a addButtonWithTitle:@"Unpair"];
    [a addButtonWithTitle:@"Cancel"];
    if ([a runModal] == NSAlertFirstButtonReturn) {
        sendWorkerCommand(self.paths, @"unpair");
        self.config.seenPairedAt = [[NSDate date] timeIntervalSince1970];
        [self.config write];
        [self refresh];
    }
}

- (void)acknowledgePairing {
    self.config.seenPairedAt = [[NSDate date] timeIntervalSince1970];
    [self.config write];
    [self refresh];
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

#pragma mark - running

- (void)togglePaused {
    NSFileManager *fm = [NSFileManager defaultManager];
    if ([fm fileExistsAtPath:[self.paths paused]]) {
        [fm removeItemAtPath:[self.paths paused] error:nil];
        [self append:@"resumed"];
    } else {
        [fm createDirectoryAtPath:[self.paths home] withIntermediateDirectories:YES attributes:nil error:nil];
        [fm createFileAtPath:[self.paths paused] contents:[NSData data] attributes:nil];
        [self append:@"paused: requests are refused while worker.paused exists"];
    }
    [self refresh];
}

- (void)openBrowserView {
    if ([self.config macBrowser]) {
        if (self.current.browser.running) [self.current.browser activate];
        else [self ensureMacBrowser:self.current];
        return;
    }
    NSString *port = [Runtime browserPort:[self composePrefix] legacy:self.current.isDefault];
    if (!port) { [self append:@"the worker isn't running, so there's no browser to open"]; return; }
    NSString *url = [NSString stringWithFormat:@"http://localhost:%@/?autoconnect=1&reconnect=1&resize=remote", port];
    [self append:[NSString stringWithFormat:@"opening %@", url]];
    [[NSWorkspace sharedWorkspace] openURL:[NSURL URLWithString:url]];
}

- (void)openHome { [[NSWorkspace sharedWorkspace] openURL:[NSURL fileURLWithPath:[self.paths home]]]; }

- (void)containerLog {
    [self openLog];
    __weak typeof(self) weak = self;
    [self runLogged:[[self composePrefix] stringByAppendingString:@" logs --tail 100"] in:[self.paths home] line:nil done:^(int status) { (void)weak; }];
}

#pragma mark - log window

- (void)openLog {
    if (!self.logWindow) {
        self.logWindow = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, 700, 380)
            styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable | NSWindowStyleMaskResizable backing:NSBackingStoreBuffered defer:NO];
        self.logWindow.title = @"myous log";
        self.logWindow.releasedWhenClosed = NO;
        NSTextView *tv = [self textViewIn:self.logWindow];
        [tv.textStorage appendAttributedString:self.logView.textStorage ?: [[NSAttributedString alloc] initWithString:@""]];
        self.logView = tv;
        [self.logWindow center];
    }
    [self.logWindow makeKeyAndOrderFront:nil];
}

- (void)append:(NSString *)line {
    if (!self.logView) {
        // Keep lines before the window exists.
        self.logView = [NSTextView new];
        self.logView.font = [NSFont monospacedSystemFontOfSize:11 weight:NSFontWeightRegular];
    }
    NSDateFormatter *f = [NSDateFormatter new];
    f.dateFormat = @"HH:mm:ss";
    NSString *text = [NSString stringWithFormat:@"%@ %@\n", [f stringFromDate:[NSDate date]], line];
    [self.logView.textStorage appendAttributedString:[[NSAttributedString alloc] initWithString:text
        attributes:@{NSFontAttributeName: self.logView.font, NSForegroundColorAttributeName: [NSColor textColor]}]];
    [self.logView scrollToEndOfDocument:nil];
}

#pragma mark - settings

- (void)openSettings {
    if (!self.settingsWindow) {
        self.settingsWindow = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, 420, 240)
            styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable backing:NSBackingStoreBuffered defer:NO];
        self.settingsWindow.title = @"myous settings";
        self.settingsWindow.releasedWhenClosed = NO;
        self.settingsName = [NSTextField textFieldWithString:@""];
        [self.settingsName.widthAnchor constraintEqualToConstant:260].active = YES;
        NSTextField *nameHint = [self wrap:@"The name your agent sees. A new name applies at the next start, and the agent is told (it keeps its own name for this computer until it renames)."];
        nameHint.font = [NSFont systemFontOfSize:11];
        nameHint.textColor = [NSColor secondaryLabelColor];
        nameHint.preferredMaxLayoutWidth = 380;
        self.settingsDescription = [self textArea:380];
        NSTextField *descHint = [self wrap:@"How it describes this computer to your agent (its card). Sent at the next start."];
        descHint.font = [NSFont systemFontOfSize:11];
        descHint.textColor = [NSColor secondaryLabelColor];
        descHint.preferredMaxLayoutWidth = 380;
        nameHint.preferredMaxLayoutWidth = 380;
        self.settingsReview = [NSPopUpButton new];
        [self.settingsReview addItemsWithTitles:@[@"Trust my agent: everything it asks runs",
                                                   @"Ask me before anything that changes things",
                                                   @"Ask me before every request"]];
        NSTextField *reviewHint = [self wrap:@"Questions arrive as notifications and at the top of the Requests list; an agent waits up to two minutes for your answer."];
        reviewHint.font = [NSFont systemFontOfSize:11];
        reviewHint.textColor = [NSColor secondaryLabelColor];
        reviewHint.preferredMaxLayoutWidth = 380;
        self.settingsDock = [NSButton checkboxWithTitle:@"Show an icon in the Dock as well as the menu bar" target:nil action:nil];
        self.settingsLogin = [NSButton checkboxWithTitle:@"Open myous at login" target:nil action:nil];
        self.settingsNotify = [NSButton checkboxWithTitle:@"Notify me" target:nil action:nil];
        self.settingsNotify.target = self;
        self.settingsNotify.action = @selector(notifyMasterChanged);
        NSMutableArray *kinds = [NSMutableArray new];
        for (NSArray *k in @[@[@"paired", @"when a worker pairs (and when its code is about to expire)"], @[@"approval", @"when a request waits for my answer"],
                             @[@"refused", @"when a request is refused"], @[@"stopped", @"when a worker stops on its own"], @[@"update", @"when a new version is available"]]) {
            NSButton *b = [NSButton checkboxWithTitle:k[1] target:nil action:nil];
            b.identifier = k[0];
            [kinds addObject:b];
        }
        self.settingsKinds = kinds;
        NSStackView *kindsCol = [self column:kinds];
        kindsCol.edgeInsets = NSEdgeInsetsMake(0, 24, 0, 0);
        kindsCol.spacing = 4;
        self.settingsUpdate = [NSButton checkboxWithTitle:@"Check for a new version daily" target:nil action:nil];
        self.settingsAgents = [NSButton checkboxWithTitle:@"Show the agents on this Mac (their myous folders)" target:nil action:nil];
        // A title wider than the window would push the column out of line.
        self.settingsBrowserHidden = [NSButton checkboxWithTitle:@"Start this worker's browser hidden" target:nil action:nil];
        self.settingsBrowserHiddenHint = [self wrap:@"It works while hidden; Show browser brings it up."];
        self.settingsBrowserHiddenHint.font = [NSFont systemFontOfSize:11];
        self.settingsBrowserHiddenHint.textColor = [NSColor secondaryLabelColor];
        self.settingsBrowserHiddenHint.preferredMaxLayoutWidth = 380;
        NSButton *save = [self button:@"Save" action:@selector(saveSettings)];
        save.keyEquivalent = @"\r";
        NSButton *cancel = [self button:@"Cancel" action:@selector(closeSettings)];
        NSStackView *col = [self column:@[[self row:@[[self label:@"Worker name" size:13 weight:NSFontWeightRegular], self.settingsName]], nameHint,
                                          [self label:@"Description" size:13 weight:NSFontWeightRegular], self.settingsDescription, descHint,
                                          [self label:@"Review" size:13 weight:NSFontWeightRegular], self.settingsReview, reviewHint,
                                          self.settingsBrowserHidden, self.settingsBrowserHiddenHint, self.settingsDock, self.settingsLogin, self.settingsNotify, kindsCol, self.settingsUpdate, self.settingsAgents, [self row:@[cancel, save]]]];
        col.edgeInsets = NSEdgeInsetsMake(16, 20, 16, 20);
        col.translatesAutoresizingMaskIntoConstraints = NO;
        [self.settingsWindow.contentView addSubview:col];
        [NSLayoutConstraint activateConstraints:@[
            [col.leadingAnchor constraintEqualToAnchor:self.settingsWindow.contentView.leadingAnchor],
            [col.trailingAnchor constraintEqualToAnchor:self.settingsWindow.contentView.trailingAnchor],
            [col.topAnchor constraintEqualToAnchor:self.settingsWindow.contentView.topAnchor],
            [col.bottomAnchor constraintEqualToAnchor:self.settingsWindow.contentView.bottomAnchor],
        ]];
    }
    self.settingsName.stringValue = self.config.name ?: @"";
    self.settingsDescription.stringValue = self.config.descriptionText ?: defaultDescription(self.config.name);
    [self.settingsReview selectItemAtIndex:[@[@"trust", @"changes", @"all"] indexOfObject:reviewLevel(self.paths)]];
    self.settingsDock.state = self.appConfig.dock ? NSControlStateValueOn : NSControlStateValueOff;
    self.settingsNotify.state = self.appConfig.notifications ? NSControlStateValueOn : NSControlStateValueOff;
    for (NSButton *b in self.settingsKinds) { b.state = [self.appConfig notifies:b.identifier] ? NSControlStateValueOn : NSControlStateValueOff; b.enabled = self.appConfig.notifications; }
    self.settingsUpdate.state = self.appConfig.autoUpdate ? NSControlStateValueOn : NSControlStateValueOff;
    self.settingsAgents.state = self.appConfig.showAgents ? NSControlStateValueOn : NSControlStateValueOff;
    self.settingsBrowserHidden.state = self.config.browserHidden ? NSControlStateValueOn : NSControlStateValueOff;
    self.settingsBrowserHidden.hidden = self.settingsBrowserHiddenHint.hidden = ![self.config macBrowser];
    if (@available(macOS 13.0, *)) {
        self.settingsLogin.state = SMAppService.mainAppService.status == SMAppServiceStatusEnabled ? NSControlStateValueOn : NSControlStateValueOff;
        self.settingsLogin.enabled = YES;
    } else {
        self.settingsLogin.enabled = NO;
        self.settingsLogin.toolTip = @"Needs macOS 13 or newer; add myous to Login Items in System Settings instead.";
    }
    [self.settingsWindow center];
    [self.settingsWindow makeKeyAndOrderFront:nil];
    [NSApp activateIgnoringOtherApps:YES];
}

- (void)saveSettings {
    NSString *name = [self.settingsName.stringValue stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceAndNewlineCharacterSet]];
    if (name.length) self.config.name = name;
    NSString *desc = [self.settingsDescription.stringValue stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceAndNewlineCharacterSet]];
    self.config.descriptionText = desc.length && ![desc isEqualToString:defaultDescription(self.config.name)] ? desc : nil;
    self.appConfig.dock = self.settingsDock.state == NSControlStateValueOn;
    self.appConfig.notifications = self.settingsNotify.state == NSControlStateValueOn;
    NSMutableDictionary *kinds = [NSMutableDictionary new];
    for (NSButton *b in self.settingsKinds) kinds[b.identifier] = @(b.state == NSControlStateValueOn);
    self.appConfig.notifyKinds = kinds;
    self.appConfig.autoUpdate = self.settingsUpdate.state == NSControlStateValueOn;
    BOOL showAgents = self.settingsAgents.state == NSControlStateValueOn;
    if (showAgents != self.appConfig.showAgents) self.agents = nil;
    self.appConfig.showAgents = showAgents;
    BOOL hidden = self.settingsBrowserHidden.state == NSControlStateValueOn;
    if (hidden != self.config.browserHidden) {
        self.config.browserHidden = hidden;
        self.current.browser.hidden = hidden;
        if (hidden) [self.current.browser hide]; else [self.current.browser activate];
    }
    [self.config write];
    if (self.appConfig != self.config) [self.appConfig write];
    NSString *level = @[@"trust", @"changes", @"all"][MAX(0, self.settingsReview.indexOfSelectedItem)];
    if (![level isEqualToString:reviewLevel(self.paths)]) { setReviewLevel(self.paths, level); [self append:[@"review level: " stringByAppendingString:level]]; }
    [NSApp setActivationPolicy:self.appConfig.dock ? NSApplicationActivationPolicyRegular : NSApplicationActivationPolicyAccessory];
    if (@available(macOS 13.0, *)) {
        BOOL want = self.settingsLogin.state == NSControlStateValueOn;
        BOOL have = SMAppService.mainAppService.status == SMAppServiceStatusEnabled;
        NSError *err;
        if (want && !have) [SMAppService.mainAppService registerAndReturnError:&err];
        if (!want && have) [SMAppService.mainAppService unregisterAndReturnError:&err];
        if (err) [self append:[NSString stringWithFormat:@"login item: %@", err.localizedDescription]];
    }
    [self closeSettings];
    [self refresh];
}

- (void)closeSettings { [self.settingsWindow orderOut:nil]; }
- (void)notifyMasterChanged { for (NSButton *b in self.settingsKinds) b.enabled = self.settingsNotify.state == NSControlStateValueOn; }

#pragma mark - notifications

- (void)setupNotifications {
    if (![NSBundle mainBundle].bundleIdentifier || self.snapshotPath) return;
    UNUserNotificationCenter *c = [UNUserNotificationCenter currentNotificationCenter];
    c.delegate = self;
    UNNotificationAction *allow = [UNNotificationAction actionWithIdentifier:@"allow" title:@"Allow" options:0];
    UNNotificationAction *refuse = [UNNotificationAction actionWithIdentifier:@"refuse" title:@"Refuse" options:UNNotificationActionOptionDestructive];
    UNNotificationCategory *cat = [UNNotificationCategory categoryWithIdentifier:@"approval" actions:@[allow, refuse] intentIdentifiers:@[] options:0];
    [c setNotificationCategories:[NSSet setWithObject:cat]];
    [c requestAuthorizationWithOptions:UNAuthorizationOptionAlert | UNAuthorizationOptionSound completionHandler:^(BOOL granted, NSError *error) {}];
}

- (void)userNotificationCenter:(UNUserNotificationCenter *)center willPresentNotification:(UNNotification *)notification
         withCompletionHandler:(void (^)(UNNotificationPresentationOptions))handler {
    handler(UNNotificationPresentationOptionList | UNNotificationPresentationOptionBanner);
}

- (void)userNotificationCenter:(UNUserNotificationCenter *)center didReceiveNotificationResponse:(UNNotificationResponse *)response
         withCompletionHandler:(void (^)(void))handler {
    NSString *action = response.actionIdentifier;
    NSString *ident = response.notification.request.identifier;
    if ([ident hasPrefix:@"ask-"] && ([action isEqualToString:@"allow"] || [action isEqualToString:@"refuse"])) {
        NSString *rid = [ident substringFromIndex:4];
        for (Worker *w in self.workers) {
            for (NSDictionary *q in loadApprovals(w.paths)) {
                if ([str(q[@"id"]) isEqualToString:rid]) { answerApproval(w.paths, rid, action); [self append:[NSString stringWithFormat:@"%@ (from the notification): %@", action, str(q[@"cmd"]) ?: str(q[@"path"]) ?: @""]]; }
            }
        }
        [self refresh];
    } else {
        [self showWindow];
    }
    handler();
}

- (void)notifyOnce:(NSString *)key title:(NSString *)title body:(NSString *)body {
    [self notifyOnce:key title:title body:body category:nil];
}

- (void)notifyOnce:(NSString *)key title:(NSString *)title body:(NSString *)body category:(NSString *)category {
    NSString *kind = [key hasPrefix:@"ask-"] ? @"approval" : [key hasPrefix:@"update-"] ? @"update" : [key hasPrefix:@"refused-"] ? @"refused"
        : [key hasPrefix:@"paired-"] || [key hasPrefix:@"expiring-"] ? @"paired" : @"stopped";
    if (![key hasPrefix:@"ask-"] && ![key hasPrefix:@"update-"]) key = [NSString stringWithFormat:@"%@/%@", self.paths.home.lastPathComponent, key];
    if ([self.notified containsObject:key]) return;
    [self.notified addObject:key];
    if (!self.appConfig.notifications || ![self.appConfig notifies:kind] || ![NSBundle mainBundle].bundleIdentifier || self.fake) return;
    UNMutableNotificationContent *content = [UNMutableNotificationContent new];
    content.title = title;
    content.body = body;
    if (category) content.categoryIdentifier = category;
    UNNotificationRequest *req = [UNNotificationRequest requestWithIdentifier:key content:content trigger:nil];
    [[UNUserNotificationCenter currentNotificationCenter] addNotificationRequest:req withCompletionHandler:nil];
}

- (void)notifyPaired:(NSDictionary *)paired {
    double at = num(paired[@"at"]).doubleValue;
    if (!at || at < [[NSDate date] timeIntervalSince1970] - 600) return;   // old news
    [self notifyOnce:[NSString stringWithFormat:@"paired-%.0f", at] title:[NSString stringWithFormat:@"%@ is paired with %@", str(paired[@"alias"]) ?: @"Your agent", self.config.name ?: @"the worker"]
                body:[NSString stringWithFormat:@"Verification code %@. Your agent shows the same number.", str(paired[@"verify"]) ?: @"?"]];
}

- (void)notifyRefusals {
    double now = [[NSDate date] timeIntervalSince1970];
    for (NSDictionary *r in self.requests) {
        if (![str(r[@"decision"]) isEqualToString:@"refuse"] || now - num(r[@"at"]).doubleValue > 120) continue;
        if (self.ticks < 2) { [self.notified addObject:[NSString stringWithFormat:@"%@/refused-%@", self.paths.home.lastPathComponent, str(r[@"id"]) ?: @""]]; continue; }   // from before launch
        [self notifyOnce:[@"refused-" stringByAppendingString:str(r[@"id"]) ?: @""] title:[NSString stringWithFormat:@"%@ refused a request", self.config.name ?: @"The worker"]
                    body:[NSString stringWithFormat:@"%@: %@", str(r[@"cmd"]) ?: str(r[@"path"]) ?: str(r[@"op"]) ?: @"", str(r[@"reason"]) ?: @"refused by the review hook"]];
    }
}

#pragma mark - updates

- (void)checkForUpdatesNow { [self checkForUpdates:YES]; }

- (void)checkForUpdates:(BOOL)manual {
    NSString *hub = [[NSProcessInfo processInfo] environment][@"MYOUS_HUB"] ?: @"https://myoushq.com";
    NSURL *url = [NSURL URLWithString:[hub stringByAppendingString:@"/config.json"]];
    self.appConfig.lastUpdateCheck = [[NSDate date] timeIntervalSince1970];
    [self.appConfig write];
    __weak typeof(self) weak = self;
    [[[NSURLSession sharedSession] dataTaskWithURL:url completionHandler:^(NSData *data, NSURLResponse *response, NSError *error) {
        NSDictionary *d = data ? dict([NSJSONSerialization JSONObjectWithData:data options:0 error:nil]) : nil;
        NSString *latest = str(d[@"latest_release"]);
        dispatch_async(dispatch_get_main_queue(), ^{
            NSString *mine = appVersion();
            NSString *theirs = [latest hasPrefix:@"v"] ? [latest substringFromIndex:1] : latest;
            BOOL newer = theirs && [theirs compare:mine options:NSNumericSearch] == NSOrderedDescending && ![mine isEqualToString:@"dev"];
            weak.latestRelease = newer ? latest : nil;
            if (newer) [weak notifyOnce:[@"update-" stringByAppendingString:latest] title:[NSString stringWithFormat:@"myous %@ is available", latest] body:@"Open myous to download it."];
            if (manual) {
                NSAlert *a = [NSAlert new];
                a.messageText = error ? @"Couldn't check" : newer ? [NSString stringWithFormat:@"myous %@ is available", latest] : @"You're up to date";
                a.informativeText = error ? error.localizedDescription : [NSString stringWithFormat:@"You have %@.%@", mine, newer ? @" The download replaces the app; the worker keeps its identity and logins." : @""];
                if (newer) { [a addButtonWithTitle:@"Download"]; [a addButtonWithTitle:@"Later"]; }
                if ([a runModal] == NSAlertFirstButtonReturn && newer)
                    [[NSWorkspace sharedWorkspace] openURL:[NSURL URLWithString:@"https://myoushq.com/download/mac"]];
                else if (newer) { weak.appConfig.skippedVersion = latest; [weak.appConfig write]; }
            }
            [weak refresh];
        });
    }] resume];
}

@end
