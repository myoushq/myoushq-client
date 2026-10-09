#import "App.h"
#import "Status.h"
#import "Icon.h"
#import "Runtime.h"
#import <CoreImage/CoreImage.h>
#import <UserNotifications/UserNotifications.h>
#import <ServiceManagement/ServiceManagement.h>

typedef NS_ENUM(NSInteger, Screen) { ScreenSetup, ScreenNoRuntime, ScreenStarting, ScreenPair, ScreenPaired, ScreenRunning, ScreenStopped };

static const CGFloat kWidth = 560;
static const CGFloat kInner = kWidth - 40;
static const NSUInteger kRequestRows = 200;
static const double kInviteSeconds = 900;

@interface AppDelegate () <NSMenuDelegate, NSTableViewDataSource, NSTableViewDelegate, NSWindowDelegate, UNUserNotificationCenterDelegate>
// model
@property (nonatomic, strong) AppConfig *config;
@property (nonatomic, strong) StatusFile *status;
@property (nonatomic, copy) NSString *runtimeState;   // nil (unknown), "ok", "missing", "stopped"
@property (nonatomic, copy) NSString *runtimeName;
@property (nonatomic, strong) NSArray<NSDictionary *> *requests;
@property (nonatomic, strong) NSDate *requestsDirDate;
@property (nonatomic) Screen screen;
@property (nonatomic, copy) NSString *launchStage;    // "pulling" or "creating" while compose up runs
@property (nonatomic) double launchedAt;              // when Start was pressed (0: not by us)
@property (nonatomic) BOOL stopping;                  // Stop pressed: a stale status is expected
@property (nonatomic) BOOL wasRunning;
@property (nonatomic, copy) NSString *latestRelease;  // "vX.Y.Z" from the hub, when newer
@property (nonatomic) NSUInteger ticks;
@property (nonatomic, strong) NSTask *direct;         // the `myous worker` child in direct mode
@property (nonatomic, strong) NSMutableSet *notified;
@property (nonatomic, copy) NSString *fake;
// ui
@property (nonatomic, strong) NSStatusItem *statusItem;
@property (nonatomic, strong) NSWindow *window;
@property (nonatomic, strong) NSStackView *root;
@property (nonatomic, strong) NSTextField *headTitle, *headRight, *headFacts, *banner;
@property (nonatomic, strong) NSButton *bannerButton;
@property (nonatomic, strong) NSBox *setupCard, *runtimeCard, *startingCard, *pairCard, *pairedCard, *requestsCard, *browserCard, *stoppedCard;
@property (nonatomic, strong) NSTextField *setupRuntime, *nameField, *runtimeText, *directWarning;
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
@property (nonatomic, strong) NSButton *pauseButton, *allowButton, *refuseButton;
@property (nonatomic, strong) NSStackView *approvalRow;
@property (nonatomic, strong) NSArray<NSDictionary *> *approvals;
@property (nonatomic, copy) NSString *fakeApprovalId;
@property (nonatomic, strong) NSTableView *table;
@property (nonatomic, strong) NSTextField *stoppedText;
@property (nonatomic, strong) NSWindow *logWindow, *detailWindow, *settingsWindow;
@property (nonatomic, strong) NSTextView *logView, *detailView;
@property (nonatomic, strong) NSTextField *settingsName;
@property (nonatomic, strong) NSButton *settingsDock, *settingsLogin, *settingsNotify, *settingsUpdate;
@property (nonatomic, strong) NSPopUpButton *settingsReview;
@property (nonatomic, strong) NSTimer *timer;
@property (nonatomic) CGFloat lastHeight;
@end

@implementation AppDelegate

- (void)applicationDidFinishLaunching:(NSNotification *)note {
    self.notified = [NSMutableSet new];
    self.fake = [[NSProcessInfo processInfo] environment][@"MYOUS_FAKE_STATE"];
    self.config = [AppConfig read];
    [NSApp setActivationPolicy:self.config.dock ? NSApplicationActivationPolicyRegular : NSApplicationActivationPolicyAccessory];
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
    if (self.config.autoUpdate && [[NSDate date] timeIntervalSince1970] - self.config.lastUpdateCheck > 86400 && !self.fake)
        [self checkForUpdates:NO];
    if (self.snapshotPath) {
        // Render the window's content view once it has laid out: a PNG from
        // the view cache and, next to it, a PDF (text renders there on Macs
        // where the bitmap cache drops it).
        __weak typeof(self) weak = self;
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(0.5 * NSEC_PER_SEC)), dispatch_get_main_queue(), ^{
            NSView *v = weak.window.contentView;
            NSBitmapImageRep *rep = [v bitmapImageRepForCachingDisplayInRect:v.bounds];
            [v cacheDisplayInRect:v.bounds toBitmapImageRep:rep];
            [[rep representationUsingType:NSBitmapImageFileTypePNG properties:@{}] writeToFile:weak.snapshotPath atomically:YES];
            [[v dataWithPDFInsideRect:v.bounds] writeToFile:[[weak.snapshotPath stringByDeletingPathExtension] stringByAppendingPathExtension:@"pdf"] atomically:YES];
            [NSApp terminate:nil];
        });
    }
}

- (BOOL)applicationShouldTerminateAfterLastWindowClosed:(NSApplication *)sender { return NO; }
- (BOOL)applicationSupportsSecureRestorableState:(NSApplication *)app { return YES; }
- (BOOL)applicationShouldHandleReopen:(NSApplication *)sender hasVisibleWindows:(BOOL)flag {
    [self showWindow];
    return NO;
}
- (void)applicationWillTerminate:(NSNotification *)note {
    [self.direct terminate];   // a direct-mode worker is our child; don't leave it orphaned
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
    NSDictionary *s = self.status.status;
    NSString *name = self.config.name ?: str(s[@"alias"]) ?: @"myous worker";
    NSMenuItem *state = [menu addItemWithTitle:[NSString stringWithFormat:@"%@ · %@", name, [self stateWord]] action:@selector(showWindow) keyEquivalent:@""];
    state.image = statusIcon([self stateColor], NO);
    NSDictionary *paired = dict(s[@"paired"]);
    NSString *pairedWith = str(paired[@"alias"]);
    if (pairedWith) {
        NSMenuItem *p = [menu addItemWithTitle:[NSString stringWithFormat:@"   paired with %@", pairedWith] action:nil keyEquivalent:@""];
        p.enabled = NO;
    }
    [menu addItem:[NSMenuItem separatorItem]];
    BOOL running = self.screen == ScreenRunning || self.screen == ScreenPair || self.screen == ScreenPaired;
    if (running) {
        if (![self.config isDirect]) [menu addItemWithTitle:@"Open browser" action:@selector(openBrowserView) keyEquivalent:@""];
        [menu addItemWithTitle:[self isPaused] ? @"Resume" : @"Pause" action:@selector(togglePaused) keyEquivalent:@""];
        [menu addItemWithTitle:@"Show requests…" action:@selector(showWindow) keyEquivalent:@""];
        [menu addItem:[NSMenuItem separatorItem]];
        [menu addItemWithTitle:@"Stop" action:@selector(stop) keyEquivalent:@""];
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
        [sub addItem:[NSMenuItem separatorItem]];
        [sub addItemWithTitle:@"Show app log" action:@selector(openLog) keyEquivalent:@""];
        [sub addItemWithTitle:@"Show container log" action:@selector(containerLog) keyEquivalent:@""];
        [sub addItemWithTitle:@"Open worker folder" action:@selector(openHome) keyEquivalent:@""];
        [sub addItemWithTitle:@"Remove stale containers" action:@selector(removeContainers) keyEquivalent:@""];
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

    self.root = [self column:@[headRow, self.headFacts, bannerRow, self.setupCard, self.runtimeCard, self.startingCard, self.pairCard,
                               self.pairedCard, self.stoppedCard, self.requestsCard, self.browserCard]];
    self.root.spacing = 12;
    self.root.edgeInsets = NSEdgeInsetsMake(16, 20, 20, 20);
    self.root.translatesAutoresizingMaskIntoConstraints = NO;
    NSView *content = self.window.contentView;
    [content addSubview:self.root];
    [NSLayoutConstraint activateConstraints:@[
        [self.root.leadingAnchor constraintEqualToAnchor:content.leadingAnchor],
        [self.root.trailingAnchor constraintEqualToAnchor:content.trailingAnchor],
        [self.root.topAnchor constraintEqualToAnchor:content.topAnchor],
    ]];
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
    self.setupStart = [self button:@"Start the worker" action:@selector(setupStart:)];
    self.setupStart.keyEquivalent = @"\r";
    NSStackView *col = [self column:@[self.setupRuntime, q, self.nameField, hint, [self buttons:@[self.setupStart]]]];
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
    NSStackView *col = [self column:@[t, msgBox, [self buttons:@[copy, qr]], [self row:@[self.pairCode, self.pairBar, newCode]], self.pairWait]];
    self.pairCard = [self card:@"Pair with your agent" content:col];
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
    NSStackView *top = [self row:@[self.requestsTitle, spacer, self.pauseButton]];
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
    NSTextField *t = [self wrap:@"Your agent can use sites you're logged into here. Open it to log in or to watch."];
    NSButton *open = [self button:@"Open browser" action:@selector(openBrowserView)];
    NSStackView *col = [self column:@[t, [self buttons:@[open]]]];
    self.browserCard = [self card:@"Browser" content:col];
}

- (void)buildStoppedCard {
    self.stoppedText = [self wrap:@"The worker is stopped. Your agent can't reach this Mac until you start it."];
    NSButton *start = [self button:@"Start" action:@selector(start)];
    start.keyEquivalent = @"\r";
    NSStackView *col = [self column:@[self.stoppedText, [self buttons:@[start]]]];
    self.stoppedCard = [self card:@"Stopped" content:col];
}

- (void)fitWindow {
    [self.root layoutSubtreeIfNeeded];
    CGFloat h = self.root.fittingSize.height;
    if (fabs(h - self.lastHeight) < 1) return;
    self.lastHeight = h;
    NSRect frame = self.window.frame;
    NSRect content = [self.window frameRectForContentRect:NSMakeRect(0, 0, kWidth, h)];
    frame.origin.y += frame.size.height - content.size.height;
    frame.size = content.size;
    [self.window setFrame:frame display:YES animate:NO];
}

- (void)showWindow {
    [self.window makeKeyAndOrderFront:nil];
    [NSApp activateIgnoringOtherApps:YES];
}

- (void)windowDidBecomeKey:(NSNotification *)n {
    if (n.object != self.window) return;
    // New requests are the ones since the window was last in front.
    self.config.seenRequestsAt = [[NSDate date] timeIntervalSince1970];
    [self.config write];
    [self updateBadge];
}

#pragma mark - state

- (BOOL)isPaused { return [[NSFileManager defaultManager] fileExistsAtPath:[Paths paused]]; }

- (NSString *)stateWord {
    switch (self.screen) {
        case ScreenSetup: case ScreenNoRuntime: return @"Not set up";
        case ScreenStarting: return @"Starting…";
        case ScreenPair: return @"Running · not paired";
        case ScreenPaired: case ScreenRunning: return [self isPaused] ? @"Paused" : @"Running";
        case ScreenStopped: return @"Stopped";
    }
    return @"";
}

- (NSColor *)stateColor {
    if (self.banner.stringValue.length && !self.banner.hidden) return [NSColor systemRedColor];
    switch (self.screen) {
        case ScreenStarting: return [NSColor systemBlueColor];
        case ScreenPair: case ScreenPaired: case ScreenRunning: return [self isPaused] ? [NSColor systemOrangeColor] : [NSColor systemGreenColor];
        default: return [NSColor systemGrayColor];
    }
}

- (void)refresh {
    self.config = [AppConfig read];
    if (self.fake) [self applyFake]; else {
        self.status = [StatusFile read];
        if (++self.ticks % 5 == 0 && [self.config usesDocker]) [self probeRuntimeAsync];
    }
    [self loadRequestsIfChanged];
    if (!self.fake) self.approvals = loadApprovals();
    NSDictionary *s = self.status.status;
    NSString *phase = [self.status phase] ?: @"";
    BOOL fresh = [self.status fresh];
    // No phase: a worker from before v0.6.0, which is running if it writes.
    BOOL alive = fresh && ([phase isEqualToString:@"running"] || [phase isEqualToString:@"paused"] || !phase.length);
    BOOL booting = fresh && ([phase isEqualToString:@"starting"] || [phase isEqualToString:@"browser"] || [phase isEqualToString:@"registering"]);
    double now = [[NSDate date] timeIntervalSince1970];
    BOOL launching = self.launchStage != nil || (self.launchedAt && now - self.launchedAt < 90 && !alive);
    BOOL runtimeProblem = [self.config usesDocker] && self.runtimeState && ![self.runtimeState isEqualToString:@"ok"];
    if (alive) { self.launchedAt = 0; self.stopping = NO; }

    // An existing install from before names were a setting: take the worker's.
    if (!self.config.name && str(s[@"alias"]) && !self.fake) { self.config.name = str(s[@"alias"]); [self.config write]; }

    Screen screen;
    NSString *attention = nil, *attentionButton = nil;
    if (!self.config.name) {
        screen = runtimeProblem && ![self.runtimeState isEqualToString:@"stopped"] ? ScreenNoRuntime : ScreenSetup;
        if ([self.runtimeState isEqualToString:@"stopped"]) screen = ScreenNoRuntime;
    } else if (runtimeProblem) {
        screen = ScreenNoRuntime;
    } else if (launching || booting) {
        screen = ScreenStarting;
    } else if (alive) {
        NSDictionary *invite = dict(s[@"invite"]);
        NSDictionary *paired = dict(s[@"paired"]);
        if (num(s[@"contacts"]).integerValue == 0 && str(invite[@"code"])) screen = ScreenPair;
        else if (paired && num(paired[@"at"]).doubleValue > self.config.seenPairedAt) screen = ScreenPaired;
        else screen = ScreenRunning;
    } else {
        screen = ScreenStopped;
        if ([phase hasPrefix:@"error"] && fresh) { attention = [phase substringFromIndex:MIN(phase.length, 7)]; attentionButton = @"Show log"; }
        else if (self.wasRunning && !self.stopping) { attention = @"The worker stopped on its own."; attentionButton = @"Start"; [self notifyOnce:@"stopped" title:@"myous worker stopped" body:@"The worker stopped on its own. Open myous to start it again."]; }
        else if (self.launchedAt && !launching) { attention = @"The worker didn't start. The log says why."; attentionButton = @"Show log"; }
    }
    if (self.latestRelease && ![self.latestRelease isEqualToString:self.config.skippedVersion] && !attention) {
        attention = [NSString stringWithFormat:@"myous %@ is available (you have %@).", self.latestRelease, appVersion()];
        attentionButton = @"Download";
    }
    if (alive) self.wasRunning = YES;
    if (screen != ScreenStopped) self.wasRunning = alive;
    self.screen = screen;

    // Header
    NSString *name = self.config.name ?: @"myous";
    NSMutableAttributedString *title = [[NSMutableAttributedString alloc] initWithString:[NSString stringWithFormat:@"● %@ · %@", [self stateWord], name]];
    [title addAttribute:NSForegroundColorAttributeName value:[self stateColor] range:NSMakeRange(0, 1)];
    [title addAttribute:NSFontAttributeName value:self.headTitle.font range:NSMakeRange(0, title.length)];
    self.headTitle.attributedStringValue = title;
    NSDictionary *paired = dict(s[@"paired"]);
    NSString *pairedWith = str(paired[@"alias"]);
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
    self.requestsCard.hidden = !(screen == ScreenRunning || screen == ScreenPaired);
    self.browserCard.hidden = self.requestsCard.hidden || [self.config isDirect];
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
    if (self.approvals.count && self.requestsCard.hidden && (screen == ScreenPair)) { /* a question while unpaired can't happen */ }
    if (screen == ScreenPaired || screen == ScreenRunning) [self notifyPaired:paired];
    [self notifyRefusals];

    self.statusItem.button.image = statusIcon([self stateColor], [self requestInProgress]);
    [self updateBadge];
    [self fitWindow];
}

- (void)fillSetup {
    if (self.nameField.stringValue.length == 0) {
        NSString *host = [[NSHost currentHost] localizedName] ?: @"My Mac";
        self.nameField.stringValue = host;
    }
    self.setupRuntime.stringValue = [self.runtimeState isEqualToString:@"ok"]
        ? [NSString stringWithFormat:@"✓ %@ found. The worker runs in a container there, so your agent's commands stay inside it.", self.runtimeName]
        : @"✓ Ready.";
}

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
        [NSString stringWithFormat:@"%@ Starting the browser%@", browserDone ? @"✓" : (pulled && inContainer) ? @"⟳" : @"○", (!browserDone && pulled && inContainer) ? clock : @""],
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

- (void)fillRequests {
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
    NSString *key = [@"ask-" stringByAppendingString:str(q[@"id"]) ?: @""];
    [self notifyOnce:key title:[NSString stringWithFormat:@"%@ wants to %@ on %@", str(q[@"alias"]) ?: @"Your agent", verb, self.config.name ?: @"the worker"]
                body:what category:@"approval"];
}

- (void)allowRequest { [self answer:@"allow"]; }
- (void)refuseRequest { [self answer:@"refuse"]; }
- (void)answer:(NSString *)verdict {
    NSDictionary *q = self.approvals.firstObject;
    if (!q) return;
    answerApproval(str(q[@"id"]) ?: @"", verdict);
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
    NSDictionary *attrs = [[NSFileManager defaultManager] attributesOfItemAtPath:[Paths requests] error:nil];
    NSDate *d = attrs[NSFileModificationDate];
    if (self.requests && ((!d && !self.requestsDirDate) || [d isEqualToDate:self.requestsDirDate])) return;
    self.requestsDirDate = d;
    self.requests = loadRequests(kRequestRows);
    [self.table reloadData];
}

- (void)updateBadge {
    NSUInteger fresh = 0;
    if (!self.window.keyWindow || !self.window.visible) {
        for (NSDictionary *r in self.requests) if (num(r[@"at"]).doubleValue > self.config.seenRequestsAt) fresh++;
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
    NSMutableDictionary *s = [@{@"alias": @"Max's Mac", @"contacts": @1, @"requests": @12, @"phase": @"running",
                                @"paired": @{@"alias": @"Max's Muse", @"verify": @"358806", @"at": @(now - 3600)},
                                @"last": @{@"op": @"exec", @"at": @(now - 120), @"alias": @"Max's Muse", @"ok": @YES}} mutableCopy];
    self.runtimeState = @"ok";
    self.runtimeName = @"Docker Desktop";
    self.config.name = @"Max's Mac";
    self.config.seenPairedAt = now;
    StatusFile *st = [StatusFile new];
    st.modified = [NSDate date];
    if ([f isEqualToString:@"setup"]) { self.config.name = nil; }
    else if ([f isEqualToString:@"noruntime"]) { self.config.name = nil; self.runtimeState = @"missing"; }
    else if ([f isEqualToString:@"stoppedruntime"]) { self.runtimeState = @"stopped"; }
    else if ([f isEqualToString:@"starting"]) { s[@"phase"] = @"browser"; s[@"contacts"] = @0; self.launchedAt = now - 75; }
    else if ([f isEqualToString:@"pair"]) { s[@"contacts"] = @0; [s removeObjectForKey:@"paired"];
        s[@"invite"] = @{@"code": @"4821-K7F3QX", @"link": @"https://myoushq.com/p/4821#K7F3QX", @"expires_at": @(now + 702)}; }
    else if ([f isEqualToString:@"paired"]) { self.config.seenPairedAt = 0; }
    else if ([f isEqualToString:@"paused"]) { s[@"phase"] = @"paused"; }
    else if ([f isEqualToString:@"approval"]) { self.approvals = @[@{@"id": @"q1", @"op": @"exec", @"alias": @"Max's Muse", @"cmd": @"rm -rf /work/old", @"asked_at": @(now - 20), @"wait": @120}]; }
    else if ([f isEqualToString:@"stopped"]) { st.modified = [NSDate dateWithTimeIntervalSinceNow:-3600]; }
    st.status = s;
    self.status = st;
    if (!self.requests) {
        self.requests = @[
            @{@"id": @"1", @"op": @"exec", @"alias": @"Max's Muse", @"cmd": @"python3 /work/open_page.py", @"decision": @"allow", @"at": @(now - 120), @"done_at": @(now - 118), @"exit": @0, @"duration": @1.4, @"stdout": @"ok\n", @"stderr": @""},
            @{@"id": @"2", @"op": @"put", @"alias": @"Max's Muse", @"path": @"open_page.py", @"size": @537, @"decision": @"allow", @"at": @(now - 130), @"done_at": @(now - 129)},
            @{@"id": @"3", @"op": @"get", @"alias": @"Max's Muse", @"path": @"cp-test.txt", @"decision": @"allow", @"at": @(now - 400), @"done_at": @(now - 399), @"size": @12},
            @{@"id": @"4", @"op": @"exec", @"alias": @"Max's Muse", @"cmd": @"cat /etc/passwd", @"decision": @"refuse", @"reason": @"path outside the work directory", @"at": @(now - 3000), @"done_at": @(now - 3000)},
            @{@"id": @"5", @"op": @"exec", @"alias": @"Max's Muse", @"cmd": @"ls -la", @"decision": @"allow", @"at": @(now - 90000), @"done_at": @(now - 89999), @"exit": @0, @"duration": @0.1},
        ];
        [self.table reloadData];
    }
}

#pragma mark - requests table

- (NSInteger)numberOfRowsInTableView:(NSTableView *)tableView { return self.requests.count; }

- (NSView *)tableView:(NSTableView *)tv viewForTableColumn:(NSTableColumn *)col row:(NSInteger)row {
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
        text = refused ? @"refused" : pending ? @"waiting for you" : !done ? @"running…" : (exit && exit.intValue != 0) ? [NSString stringWithFormat:@"exit %d", exit.intValue] : @"ok";
        color = refused ? [NSColor systemRedColor] : (pending || (exit && exit.intValue != 0)) ? [NSColor systemOrangeColor] : [NSColor secondaryLabelColor];
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
    return [NSString stringWithFormat:@"%@ compose -f '%@' -p myous-worker", [Runtime dockerBin], file];
}

- (NSDictionary *)composeEnv {
    NSMutableDictionary *env = [NSMutableDictionary new];
    if (self.config.name) env[@"MYOUS_ALIAS"] = self.config.name;
    env[@"MYOUS_WORKER_HOME"] = [Paths home];
    return env;
}

- (void)start {
    [[NSFileManager defaultManager] createDirectoryAtPath:[Paths home] withIntermediateDirectories:YES attributes:@{NSFilePosixPermissions: @0700} error:nil];
    self.launchedAt = [[NSDate date] timeIntervalSince1970];
    self.stopping = NO;
    self.wasRunning = NO;
    if ([self.config isDirect]) { [self startDirect]; [self refresh]; return; }
    NSString *file = [self composeFile];
    if (!file || ![[NSFileManager defaultManager] fileExistsAtPath:file]) {
        [self append:[NSString stringWithFormat:@"no compose file at %@", file ?: @"(none)"]];
        self.launchedAt = 0;
        return;
    }
    self.launchStage = @"pulling";
    NSString *cmd = [[self composePrefix] stringByAppendingString:[self.config isImage] ? @" up -d" : @" up -d --build"];
    __weak typeof(self) weak = self;
    [self runLogged:cmd in:[Paths home] line:^(NSString *line) {
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
    self.launchedAt = 0;
    if ([self.config isDirect]) { [self stopDirect]; return; }
    // `down` for our project, then anything left from another project name.
    NSString *cmd = [NSString stringWithFormat:@"%@ down; %@", [self composePrefix], [Runtime removeAllCommand]];
    __weak typeof(self) weak = self;
    [self runLogged:cmd in:[Paths home] line:nil done:^(int status) { [weak refresh]; }];
}

- (void)rebuild {
    if (!self.config.repo) return;
    [self stop];
    __weak typeof(self) weak = self;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 3 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{ [weak start]; });
}

- (void)removeContainers {
    __weak typeof(self) weak = self;
    [self runLogged:[Runtime removeAllCommand] in:[Paths home] line:nil done:^(int status) { [weak refresh]; }];
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
    [[NSFileManager defaultManager] createDirectoryAtPath:[Paths work] withIntermediateDirectories:YES attributes:nil error:nil];
    NSTask *t = [NSTask new];
    t.launchPath = @"/bin/sh";
    // A login shell, so the user's PATH (where `myous` lives) applies. The
    // name is passed only when it changed (passing it re-registers).
    NSData *settings = [NSData dataWithContentsOfFile:[[Paths home] stringByAppendingPathComponent:@"settings.json"]];
    NSString *current = settings ? str(dict([NSJSONSerialization JSONObjectWithData:settings options:0 error:nil])[@"alias"]) : nil;
    NSString *name = self.config.name ?: @"Desktop worker";
    NSString *aliasOpt = [name isEqualToString:current] ? @"" :
        [NSString stringWithFormat:@"--alias '%@' ", [name stringByReplacingOccurrencesOfString:@"'" withString:@"'\\''"]];
    NSString *home = [[Paths home] stringByReplacingOccurrencesOfString:@"'" withString:@"'\\''"];
    NSString *hook = [[Paths bundledReview] stringByReplacingOccurrencesOfString:@"'" withString:@"'\\''"];
    NSString *reviewOpt = hook ? [NSString stringWithFormat:@"--review '%@' ", hook] : @"";
    t.arguments = @[@"-lc", [NSString stringWithFormat:@"exec myous worker %@%@--work '%@/work' >> '%@/worker.log' 2>&1", aliasOpt, reviewOpt, home, home]];
    NSMutableDictionary *env = [[[NSProcessInfo processInfo] environment] mutableCopy];
    env[@"MYOUS_HOME"] = [Paths home];
    t.environment = env;
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
        NSNumber *pid = num([StatusFile read].status[@"pid"]);   // started outside this app
        if (pid) { kill((pid_t)pid.intValue, SIGTERM); [self append:[NSString stringWithFormat:@"sent SIGTERM to worker pid %@", pid]]; }
    }
}

- (void)bannerAction {
    NSString *t = self.bannerButton.title;
    if ([t isEqualToString:@"Start"]) [self start];
    else if ([t isEqualToString:@"Download"]) [[NSWorkspace sharedWorkspace] openURL:[NSURL URLWithString:@"https://myoushq.com/download/mac"]];
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
    if (![[NSFileManager defaultManager] fileExistsAtPath:[Paths review]]) setReviewLevel(@"changes");
    [self start];
}

- (void)useDirect {
    self.config.mode = @"direct";
    [self.config write];
    // No container around the commands: ask before anything that changes things, unless the owner chose otherwise.
    if (![[NSFileManager defaultManager] fileExistsAtPath:[Paths review]]) setReviewLevel(@"changes");
    [self append:@"mode: direct (no container)"];
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

- (void)newCode { sendWorkerCommand(@"new-code"); self.pairWait.stringValue = @"Asking the worker for a new code…"; }

- (void)unpair {
    NSAlert *a = [NSAlert new];
    a.messageText = @"Unpair this worker?";
    a.informativeText = @"The agent loses access, and a new pairing code appears. Use this if the verification numbers differ.";
    [a addButtonWithTitle:@"Unpair"];
    [a addButtonWithTitle:@"Cancel"];
    if ([a runModal] == NSAlertFirstButtonReturn) {
        sendWorkerCommand(@"unpair");
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
    if ([fm fileExistsAtPath:[Paths paused]]) {
        [fm removeItemAtPath:[Paths paused] error:nil];
        [self append:@"resumed"];
    } else {
        [fm createDirectoryAtPath:[Paths home] withIntermediateDirectories:YES attributes:nil error:nil];
        [fm createFileAtPath:[Paths paused] contents:[NSData data] attributes:nil];
        [self append:@"paused: requests are refused while worker.paused exists"];
    }
    [self refresh];
}

- (void)openBrowserView {
    NSString *port = [Runtime browserPort];
    if (!port) { [self append:@"the worker isn't running, so there's no browser to open"]; return; }
    NSString *url = [NSString stringWithFormat:@"http://localhost:%@/?autoconnect=1&reconnect=1&resize=remote", port];
    [self append:[NSString stringWithFormat:@"opening %@", url]];
    [[NSWorkspace sharedWorkspace] openURL:[NSURL URLWithString:url]];
}

- (void)openHome { [[NSWorkspace sharedWorkspace] openURL:[NSURL fileURLWithPath:[Paths home]]]; }

- (void)containerLog {
    [self openLog];
    __weak typeof(self) weak = self;
    [self runLogged:[[self composePrefix] stringByAppendingString:@" logs --tail 100"] in:[Paths home] line:nil done:^(int status) { (void)weak; }];
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
        NSTextField *nameHint = [self wrap:@"The name your agent sees. A new name applies at the next start."];
        nameHint.font = [NSFont systemFontOfSize:11];
        nameHint.textColor = [NSColor secondaryLabelColor];
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
        self.settingsNotify = [NSButton checkboxWithTitle:@"Notify me when the worker pairs, refuses a request or stops" target:nil action:nil];
        self.settingsUpdate = [NSButton checkboxWithTitle:@"Check for a new version daily" target:nil action:nil];
        NSButton *save = [self button:@"Save" action:@selector(saveSettings)];
        save.keyEquivalent = @"\r";
        NSButton *cancel = [self button:@"Cancel" action:@selector(closeSettings)];
        NSStackView *col = [self column:@[[self row:@[[self label:@"Worker name" size:13 weight:NSFontWeightRegular], self.settingsName]], nameHint,
                                          [self label:@"Review" size:13 weight:NSFontWeightRegular], self.settingsReview, reviewHint,
                                          self.settingsDock, self.settingsLogin, self.settingsNotify, self.settingsUpdate, [self row:@[cancel, save]]]];
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
    [self.settingsReview selectItemAtIndex:[@[@"trust", @"changes", @"all"] indexOfObject:reviewLevel()]];
    self.settingsDock.state = self.config.dock ? NSControlStateValueOn : NSControlStateValueOff;
    self.settingsNotify.state = self.config.notifications ? NSControlStateValueOn : NSControlStateValueOff;
    self.settingsUpdate.state = self.config.autoUpdate ? NSControlStateValueOn : NSControlStateValueOff;
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
    self.config.dock = self.settingsDock.state == NSControlStateValueOn;
    self.config.notifications = self.settingsNotify.state == NSControlStateValueOn;
    self.config.autoUpdate = self.settingsUpdate.state == NSControlStateValueOn;
    [self.config write];
    NSString *level = @[@"trust", @"changes", @"all"][MAX(0, self.settingsReview.indexOfSelectedItem)];
    if (![level isEqualToString:reviewLevel()]) { setReviewLevel(level); [self append:[@"review level: " stringByAppendingString:level]]; }
    [NSApp setActivationPolicy:self.config.dock ? NSApplicationActivationPolicyRegular : NSApplicationActivationPolicyAccessory];
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
        self.approvals = loadApprovals();
        for (NSDictionary *q in self.approvals) {
            if ([str(q[@"id"]) isEqualToString:rid]) { answerApproval(rid, action); [self append:[NSString stringWithFormat:@"%@ (from the notification): %@", action, str(q[@"cmd"]) ?: str(q[@"path"]) ?: @""]]; }
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
    if ([self.notified containsObject:key]) return;
    [self.notified addObject:key];
    if (!self.config.notifications || ![NSBundle mainBundle].bundleIdentifier || self.fake) return;
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
        if (self.ticks < 2) { [self.notified addObject:[@"refused-" stringByAppendingString:str(r[@"id"]) ?: @""]]; continue; }   // from before launch
        [self notifyOnce:[@"refused-" stringByAppendingString:str(r[@"id"]) ?: @""] title:[NSString stringWithFormat:@"%@ refused a request", self.config.name ?: @"The worker"]
                    body:[NSString stringWithFormat:@"%@: %@", str(r[@"cmd"]) ?: str(r[@"path"]) ?: str(r[@"op"]) ?: @"", str(r[@"reason"]) ?: @"refused by the review hook"]];
    }
}

#pragma mark - updates

- (void)checkForUpdatesNow { [self checkForUpdates:YES]; }

- (void)checkForUpdates:(BOOL)manual {
    NSString *hub = [[NSProcessInfo processInfo] environment][@"MYOUS_HUB"] ?: @"https://myoushq.com";
    NSURL *url = [NSURL URLWithString:[hub stringByAppendingString:@"/config.json"]];
    self.config.lastUpdateCheck = [[NSDate date] timeIntervalSince1970];
    [self.config write];
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
                else if (newer) { weak.config.skippedVersion = latest; [weak.config write]; }
            }
            [weak refresh];
        });
    }] resume];
}

@end
