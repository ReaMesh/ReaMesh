#import "settings_window.h"
#import <Cocoa/Cocoa.h>

// Native macOS Settings window with General, Overlay, Updates and Advanced panes.
// REAPER access goes through the Rust SettingsCallbacks interface.

static NSString *const kTabGeneral = @"general";
static NSString *const kTabOverlay = @"overlay";
static NSString *const kTabUpdates = @"updates";
static NSString *const kTabAdvanced = @"advanced";
static NSString *const kReaperDownloadURL = @"https://www.reaper.fm/download.php";
static NSString *const kDocsURL = @"https://reamesh.com/docs";

static const CGFloat kWindowWidth = 480;
static const CGFloat kWindowHeight = 448;
static const CGFloat kMargin = 24;

// The window is bilingual (English/German), driven by the same stored
// preference the webview uses. Resolved here so a popup change retitles the
// pane immediately, without a round trip through the webview.
static BOOL gUseGerman = NO;

static NSString *L(NSString *en, NSString *de) {
    return gUseGerman ? de : en;
}

// Resolve the stored preference (kReaMeshLang*) to the UI language; "system"
// follows the device's preferred language, non-German falls back to English.
static void resolveUILanguage(int32_t pref) {
    if (pref == kReaMeshLangEnglish) {
        gUseGerman = NO;
    } else if (pref == kReaMeshLangGerman) {
        gUseGerman = YES;
    } else {
        NSString *device = [[NSLocale preferredLanguages] firstObject] ?: @"en";
        gUseGerman = [device.lowercaseString hasPrefix:@"de"];
    }
}

// Use a flipped container for top-down layout coordinates.
@interface ReaMeshFlippedView : NSView
@end
@implementation ReaMeshFlippedView
- (BOOL)isFlipped { return YES; }
@end

@interface ReaMeshSettingsController : NSObject <NSToolbarDelegate, NSWindowDelegate>
@property (strong) NSWindow *window;
@property (strong) ReaMeshFlippedView *container;
@property (assign) SettingsCallbacks cb;
@property (strong) NSString *selectedTab;
@end

static ReaMeshSettingsController *gController = nil;

@implementation ReaMeshSettingsController

- (instancetype)initWithCallbacks:(SettingsCallbacks)cb {
    self = [super init];
    if (!self) return self;
    _cb = cb;
    _selectedTab = kTabGeneral;
    resolveUILanguage(cb.get_language(cb.ctx));

    NSRect frame = NSMakeRect(0, 0, kWindowWidth, kWindowHeight);
    NSWindow *win = [[NSWindow alloc]
        initWithContentRect:frame
                  styleMask:(NSWindowStyleMaskTitled | NSWindowStyleMaskClosable |
                             NSWindowStyleMaskMiniaturizable)
                    backing:NSBackingStoreBuffered
                      defer:NO];
    win.title = L(@"ReaMesh Settings", @"ReaMesh-Einstellungen");
    win.releasedWhenClosed = NO; // keep the controller reusable across opens
    win.delegate = self;
    [win center];

    NSToolbar *toolbar = [[NSToolbar alloc] initWithIdentifier:@"ReaMeshSettingsToolbar"];
    toolbar.delegate = self;
    toolbar.allowsUserCustomization = NO;
    toolbar.selectedItemIdentifier = kTabGeneral;
    win.toolbar = toolbar;
    if (@available(macOS 11.0, *)) {
        win.toolbarStyle = NSWindowToolbarStylePreference;
    }

    ReaMeshFlippedView *container = [[ReaMeshFlippedView alloc]
        initWithFrame:((NSView *)win.contentView).bounds];
    container.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
    [win.contentView addSubview:container];

    _window = win;
    _container = container;

    [self showTab:kTabGeneral];
    [win makeKeyAndOrderFront:nil];
    return self;
}

#pragma mark - NSToolbarDelegate

- (NSArray<NSToolbarItemIdentifier> *)toolbarDefaultItemIdentifiers:(NSToolbar *)t {
    return @[ kTabGeneral, kTabOverlay, kTabUpdates, kTabAdvanced ];
}
- (NSArray<NSToolbarItemIdentifier> *)toolbarAllowedItemIdentifiers:(NSToolbar *)t {
    return @[ kTabGeneral, kTabOverlay, kTabUpdates, kTabAdvanced ];
}
- (NSArray<NSToolbarItemIdentifier> *)toolbarSelectableItemIdentifiers:(NSToolbar *)t {
    return @[ kTabGeneral, kTabOverlay, kTabUpdates, kTabAdvanced ];
}

- (NSToolbarItem *)toolbar:(NSToolbar *)toolbar
        itemForItemIdentifier:(NSToolbarItemIdentifier)identifier
    willBeInsertedIntoToolbar:(BOOL)flag {
    NSToolbarItem *item = [[NSToolbarItem alloc] initWithItemIdentifier:identifier];
    item.target = self;
    item.action = @selector(tabSelected:);
    if ([identifier isEqualToString:kTabAdvanced]) {
        item.label = L(@"Advanced", @"Erweitert");
        if (@available(macOS 11.0, *)) {
            item.image = [NSImage imageWithSystemSymbolName:@"gearshape.2"
                                   accessibilityDescription:item.label];
        }
    } else if ([identifier isEqualToString:kTabUpdates]) {
        item.label = L(@"Updates", @"Updates");
        if (@available(macOS 11.0, *)) {
            item.image = [NSImage imageWithSystemSymbolName:@"arrow.down.circle"
                                   accessibilityDescription:item.label];
        }
    } else if ([identifier isEqualToString:kTabOverlay]) {
        item.label = L(@"Overlay", @"Overlay");
        if (@available(macOS 11.0, *)) {
            item.image = [NSImage imageWithSystemSymbolName:@"rectangle.on.rectangle"
                                   accessibilityDescription:item.label];
        }
    } else {
        item.label = L(@"General", @"Allgemein");
        if (@available(macOS 11.0, *)) {
            item.image = [NSImage imageWithSystemSymbolName:@"folder"
                                   accessibilityDescription:item.label];
        }
    }
    return item;
}

- (void)tabSelected:(NSToolbarItem *)sender {
    [self showTab:sender.itemIdentifier];
}

#pragma mark - Layout helpers

- (NSTextField *)labelWithString:(NSString *)s frame:(NSRect)f {
    NSTextField *t = [NSTextField labelWithString:(s ?: @"")];
    t.frame = f;
    return t;
}

- (NSTextField *)headerWithString:(NSString *)s y:(CGFloat)y {
    NSTextField *t = [self labelWithString:s
                                     frame:NSMakeRect(kMargin, y, kWindowWidth - 2 * kMargin, 18)];
    t.font = [NSFont boldSystemFontOfSize:13];
    return t;
}

#pragma mark - Panes

- (void)showTab:(NSString *)identifier {
    self.selectedTab = identifier;
    self.window.toolbar.selectedItemIdentifier = identifier;
    for (NSView *v in [self.container.subviews copy]) {
        [v removeFromSuperview];
    }
    // The webview may have changed the language while this window was open.
    resolveUILanguage(self.cb.get_language(self.cb.ctx));
    self.window.title = L(@"ReaMesh Settings", @"ReaMesh-Einstellungen");
    ReaperStatusC st = self.cb.get_status(self.cb.ctx);
    if ([identifier isEqualToString:kTabAdvanced]) {
        [self buildAdvancedPane];
    } else if ([identifier isEqualToString:kTabOverlay]) {
        [self buildOverlayPane];
    } else if ([identifier isEqualToString:kTabUpdates]) {
        [self buildUpdatesPane];
    } else {
        [self buildGeneralPane:st];
    }
}

// The floating presence overlay's visibility toggles. The source of truth is
// backend-side (a bitmask); each checkbox flips its bit and the change is
// pushed to the live overlay webview via the set_overlay callback.
- (void)buildOverlayPane {
    CGFloat w = kWindowWidth - 2 * kMargin;
    CGFloat y = kMargin;

    [self.container addSubview:[self headerWithString:L(@"Overlay",
                                                        @"Overlay")
                                                    y:y]];
    y += 26;

    NSTextField *intro = [self
        labelWithString:L(@"Choose what appears in the overlay while you're in a session.",
                          @"Wähle aus, was während einer Session im Overlay zu sehen ist.")
                  frame:NSMakeRect(kMargin, y, w, 32)];
    intro.textColor = [NSColor secondaryLabelColor];
    intro.font = [NSFont systemFontOfSize:11];
    intro.lineBreakMode = NSLineBreakByWordWrapping;
    intro.maximumNumberOfLines = 0;
    [self.container addSubview:intro];
    y += 36;

    int32_t mask = self.cb.get_overlay(self.cb.ctx);
    BOOL shown = !(mask & kReaMeshOverlayHidden);
    // The first row is the HUD itself; the HUD-only rows grey out while it is
    // off. The tint is drawn by REAPER, not the HUD, so it stays independent.
    NSArray *rows = @[
        @[ @(kReaMeshOverlayHidden), L(@"Show overlay", @"Overlay anzeigen"), @NO ],
        @[ @(kReaMeshOverlayRoster), L(@"Participants", @"Teilnehmende"), @YES ],
        @[ @(kReaMeshOverlayRememberPos), L(@"Remember HUD position", @"HUD-Position merken"),
           @YES ],
        @[ @(kReaMeshOverlayTint), L(@"Tint locked items", @"Gesperrte Items einfärben"), @NO ],
    ];
    for (NSArray *row in rows) {
        int32_t bit = (int32_t)[row[0] integerValue];
        NSButton *cb = [NSButton checkboxWithTitle:row[1]
                                            target:self
                                            action:@selector(overlayToggled:)];
        cb.tag = bit;
        BOOL on = bit == kReaMeshOverlayHidden ? shown : (mask & bit) != 0;
        cb.state = on ? NSControlStateValueOn : NSControlStateValueOff;
        cb.enabled = ![row[2] boolValue] || shown;
        cb.frame = NSMakeRect(kMargin, y, w, 22);
        [self.container addSubview:cb];
        y += 28;
    }
}

// The in-app updater. Rust owns the state (and pushes a refresh on every
// phase change); a found update downloads in the background and installs only
// when the user clicks Restart.
- (void)buildUpdatesPane {
    CGFloat w = kWindowWidth - 2 * kMargin;
    CGFloat y = kMargin;

    [self.container addSubview:[self headerWithString:L(@"Updates", @"Updates") y:y]];
    y += 28;

    UpdateStatusC u = self.cb.get_update(self.cb.ctx);
    NSString *offered = u.version ? @(u.version) : @"";

    CGFloat btnW = 160;
    NSString *btnTitle;
    SEL action = @selector(checkUpdate:);
    BOOL enabled = YES;
    if (u.phase == kReaMeshUpdateReady) {
        btnTitle = L(@"Restart", @"Neu starten");
        action = @selector(restartUpdate:);
        enabled = u.can_restart != 0;
    } else {
        btnTitle = L(@"Check", @"Suchen");
        enabled = u.phase != kReaMeshUpdateChecking && u.phase != kReaMeshUpdateDownloading;
    }
    NSButton *btn = [NSButton buttonWithTitle:btnTitle target:self action:action];
    btn.frame = NSMakeRect(kMargin + w - btnW, y, btnW, 28);
    btn.bezelStyle = NSBezelStyleRounded;
    btn.enabled = enabled;
    if (u.phase == kReaMeshUpdateReady) btn.keyEquivalent = @"\r";
    [self.container addSubview:btn];

    const char *ver = self.cb.get_version(self.cb.ctx);
    const int32_t channel = self.cb.get_update_channel(self.cb.ctx);
    NSTextField *name =
        [self labelWithString:[NSString stringWithFormat:@"ReaMesh %s %@", ver ? ver : "",
                                  channel == kReaMeshChannelStable ? @"Stable" : @"Beta"]
                        frame:NSMakeRect(kMargin, y + 1, w - btnW - 12, 18)];
    name.font = [NSFont systemFontOfSize:13];
    [self.container addSubview:name];

    NSString *status = @"";
    NSColor *statusColor = [NSColor secondaryLabelColor];
    switch (u.phase) {
        case kReaMeshUpdateChecking:
            status = L(@"Checking…", @"Suche nach Updates…");
            break;
        case kReaMeshUpdateCurrent:
            status = L(@"Up to date", @"Auf dem neuesten Stand");
            break;
        case kReaMeshUpdateDownloading:
            status = [NSString stringWithFormat:L(@"Downloading %@…", @"%@ wird geladen…"), offered];
            break;
        case kReaMeshUpdateReady:
            status = u.can_restart
                ? [NSString stringWithFormat:L(@"%@ is ready", @"%@ ist bereit"), offered]
                : [NSString stringWithFormat:L(@"%@ is ready. You can restart after the session.",
                                               @"%@ ist bereit. Neustart nach der Session möglich."),
                                             offered];
            statusColor = [NSColor labelColor];
            break;
        case kReaMeshUpdateFailed:
            status = L(@"Couldn't check for updates", @"Update-Suche fehlgeschlagen");
            statusColor = [NSColor systemOrangeColor];
            break;
        default:
            break;
    }
    NSTextField *statusLabel = [self labelWithString:status
                                               frame:NSMakeRect(kMargin, y + 19, w - btnW - 12, 16)];
    statusLabel.textColor = statusColor;
    statusLabel.font = [NSFont systemFontOfSize:11];
    statusLabel.lineBreakMode = NSLineBreakByTruncatingTail;
    [self.container addSubview:statusLabel];
    y += 48;

    NSButton *autoBox =
        [NSButton checkboxWithTitle:L(@"Check for updates automatically",
                                      @"Automatisch nach Updates suchen")
                             target:self
                             action:@selector(updateAutoToggled:)];
    autoBox.state =
        self.cb.get_update_auto(self.cb.ctx) ? NSControlStateValueOn : NSControlStateValueOff;
    autoBox.frame = NSMakeRect(kMargin, y, w, 22);
    [self.container addSubview:autoBox];
    y += 40;

    // Beta gets every release, Stable only the ones marked stable. Until the
    // first stable release exists, Stable is shown but can't be picked.
    [self.container addSubview:[self headerWithString:L(@"Channel", @"Kanal") y:y]];
    y += 26;
    NSSegmentedControl *seg =
        [NSSegmentedControl segmentedControlWithLabels:@[ @"Beta", @"Stable" ]
                                          trackingMode:NSSegmentSwitchTrackingSelectOne
                                                target:self
                                                action:@selector(channelChanged:)];
    seg.frame = NSMakeRect(kMargin, y, 180, 24);
    seg.selectedSegment = channel == kReaMeshChannelStable ? 1 : 0;
    const BOOL stableOK = self.cb.stable_available(self.cb.ctx) != 0;
    [seg setEnabled:stableOK forSegment:1];
    [self.container addSubview:seg];
}

- (void)channelChanged:(NSSegmentedControl *)sender {
    self.cb.set_update_channel(self.cb.ctx,
                               sender.selectedSegment == 1 ? kReaMeshChannelStable
                                                           : kReaMeshChannelBeta);
}

- (void)checkUpdate:(id)sender {
    self.cb.check_update(self.cb.ctx);
}

- (void)restartUpdate:(id)sender {
    self.cb.restart_update(self.cb.ctx);
}

- (void)updateAutoToggled:(NSButton *)sender {
    self.cb.set_update_auto(self.cb.ctx, sender.state == NSControlStateValueOn ? 1 : 0);
}

- (void)showCredits:(id)sender {
    self.cb.show_credits(self.cb.ctx);
}

- (void)showJoined:(id)sender {
    self.cb.show_joined(self.cb.ctx);
}

- (void)overlayToggled:(NSButton *)sender {
    int32_t mask = self.cb.get_overlay(self.cb.ctx);
    int32_t bit = (int32_t)sender.tag;
    // The HUD bit is stored inverted ("hidden").
    BOOL set = (sender.state == NSControlStateValueOn) != (bit == kReaMeshOverlayHidden);
    if (set) {
        mask |= bit;
    } else {
        mask &= ~bit;
    }
    self.cb.set_overlay(self.cb.ctx, mask);
    // Grey the HUD-only rows in or out.
    if (bit == kReaMeshOverlayHidden) [self showTab:self.selectedTab];
}

- (void)buildGeneralPane:(ReaperStatusC)st {
    CGFloat w = kWindowWidth - 2 * kMargin;
    CGFloat y = kMargin;

    [self.container addSubview:[self headerWithString:L(@"Language", @"Sprache") y:y]];
    y += 26;

    NSPopUpButton *langPopup =
        [[NSPopUpButton alloc] initWithFrame:NSMakeRect(kMargin, y, 220, 25) pullsDown:NO];
    [langPopup addItemWithTitle:L(@"System", @"System")];
    [langPopup addItemWithTitle:@"English"];
    [langPopup addItemWithTitle:@"Deutsch"];
    // Item order matches the kReaMeshLang* values.
    [langPopup selectItemAtIndex:self.cb.get_language(self.cb.ctx)];
    langPopup.target = self;
    langPopup.action = @selector(languageChanged:);
    [self.container addSubview:langPopup];
    y += 38;

    [self.container addSubview:[self headerWithString:L(@"REAPER folder", @"REAPER-Ordner")
                                                    y:y]];
    y += 28;

    NSString *path = st.resource_path ? @(st.resource_path) : @"";
    NSTextField *pathField =
        [[NSTextField alloc] initWithFrame:NSMakeRect(kMargin, y, w, 24)];
    pathField.stringValue = path;
    pathField.editable = NO;
    pathField.selectable = YES;
    pathField.bezeled = YES;
    pathField.bezelStyle = NSTextFieldRoundedBezel;
    pathField.font = [NSFont systemFontOfSize:12];
    pathField.lineBreakMode = NSLineBreakByTruncatingMiddle;
    [self.container addSubview:pathField];
    y += 34;

    if (!st.reaper_found) {
        NSTextField *warn =
            [self labelWithString:L(@"No REAPER folder found at this location.",
                                    @"An diesem Ort wurde kein REAPER-Ordner gefunden.")
                            frame:NSMakeRect(kMargin, y, w, 16)];
        warn.textColor = [NSColor systemOrangeColor];
        warn.font = [NSFont systemFontOfSize:11];
        [self.container addSubview:warn];
        y += 24;
    }

    NSButton *change = [NSButton buttonWithTitle:L(@"Change…", @"Ändern…")
                                          target:self
                                          action:@selector(changeFolder:)];
    change.frame = NSMakeRect(kMargin, y, 100, 24);
    [self.container addSubview:change];

    if (st.has_custom_path) {
        NSButton *reset = [NSButton buttonWithTitle:L(@"Use default", @"Standard verwenden")
                                             target:self
                                             action:@selector(useDefault:)];
        reset.frame = NSMakeRect(kMargin + 108, y, 160, 24);
        [self.container addSubview:reset];
    }
    y += 34;

    if (!st.reaper_found) {
        NSButton *dl = [NSButton buttonWithTitle:L(@"Don't have REAPER? Download it",
                                                   @"REAPER noch nicht installiert? Jetzt herunterladen")
                                          target:self
                                          action:@selector(openDownload:)];
        dl.bezelStyle = NSBezelStyleInline;
        dl.bordered = NO;
        if (@available(macOS 10.14, *)) {
            dl.contentTintColor = [NSColor linkColor];
        }
        dl.frame = NSMakeRect(kMargin - 2, y, w, 20);
        dl.alignment = NSTextAlignmentLeft;
        [self.container addSubview:dl];
        y += 28;
    }

    // Extension status and install controls, followed by the restart reminder.
    y += 12;
    [self.container addSubview:[self headerWithString:L(@"REAPER extension",
                                                        @"REAPER-Erweiterung")
                                                    y:y]];
    y += 28;

    NSString *statusText;
    NSColor *statusColor;
    if (!st.installed) {
        statusText = L(@"● Not installed", @"● Nicht installiert");
        statusColor = [NSColor secondaryLabelColor];
    } else if (st.up_to_date) {
        statusText = L(@"● Installed", @"● Installiert");
        statusColor = [NSColor systemGreenColor];
    } else {
        statusText = L(@"● Update available", @"● Update verfügbar");
        statusColor = [NSColor systemOrangeColor];
    }

    // Installed and current: nothing to install, only removal is offered.
    CGFloat usedW = 0;
    if (!st.installed || !st.up_to_date) {
        NSString *btnTitle = !st.installed
            ? L(@"Install extension", @"Erweiterung installieren")
            : L(@"Update extension", @"Erweiterung aktualisieren");
        CGFloat btnW = 200;
        NSButton *installBtn = [NSButton buttonWithTitle:btnTitle
                                                  target:self
                                                  action:@selector(install:)];
        installBtn.frame = NSMakeRect(kMargin + w - btnW, y, btnW, 28);
        installBtn.bezelStyle = NSBezelStyleRounded;
        installBtn.keyEquivalent = @"\r";
        installBtn.enabled = st.reaper_found ? YES : NO;
        [self.container addSubview:installBtn];
        usedW = btnW;
    }

    // Explicit removal: uninstalling the app leaves the extension in REAPER's
    // UserPlugins folder.
    if (st.installed) {
        CGFloat rmW = 96;
        CGFloat gap = usedW > 0 ? 4 : 0;
        NSButton *removeBtn = [NSButton buttonWithTitle:L(@"Remove", @"Entfernen")
                                                 target:self
                                                 action:@selector(removeExtension:)];
        removeBtn.frame = NSMakeRect(kMargin + w - usedW - gap - rmW, y, rmW, 28);
        removeBtn.bezelStyle = NSBezelStyleRounded;
        [self.container addSubview:removeBtn];
        usedW += rmW + gap;
    }

    NSTextField *statusLabel =
        [self labelWithString:statusText
                        frame:NSMakeRect(kMargin, y + 4, w - usedW - 12, 18)];
    statusLabel.textColor = statusColor;
    statusLabel.font = [NSFont systemFontOfSize:13];
    statusLabel.lineBreakMode = NSLineBreakByTruncatingTail;
    [self.container addSubview:statusLabel];
    y += 36;

    NSTextField *help = [self
        labelWithString:L(@"REAPER only loads the extension when it starts. Restart REAPER "
                          @"after installing or updating it.",
                          @"REAPER lädt die Erweiterung nur beim Start. Starte REAPER nach "
                          @"einer Installation oder einem Update neu.")
                  frame:NSMakeRect(kMargin, y, w, 40)];
    help.textColor = [NSColor secondaryLabelColor];
    help.font = [NSFont systemFontOfSize:11];
    help.lineBreakMode = NSLineBreakByWordWrapping;
    help.maximumNumberOfLines = 0;
    [self.container addSubview:help];
    y += 48;

    // Downloaded copies of joined sessions live in the REAPER folder too.
    [self.container addSubview:[self headerWithString:L(@"Storage", @"Speicher") y:y]];
    y += 28;

    CGFloat showW = 120;
    NSTextField *joinedLabel =
        [self labelWithString:L(@"Joined projects", @"Beigetretene Projekte")
                        frame:NSMakeRect(kMargin, y + 4, w - showW - 12, 18)];
    joinedLabel.font = [NSFont systemFontOfSize:13];
    [self.container addSubview:joinedLabel];
    NSButton *joined = [NSButton buttonWithTitle:L(@"Show…", @"Anzeigen…")
                                          target:self
                                          action:@selector(showJoined:)];
    joined.frame = NSMakeRect(kMargin + w - showW, y, showW, 28);
    joined.bezelStyle = NSBezelStyleRounded;
    [self.container addSubview:joined];
}

// Privacy (network identity) and help (docs, diagnostics, version): the
// settings that came after the first native window.
- (void)buildAdvancedPane {
    CGFloat w = kWindowWidth - 2 * kMargin;
    CGFloat y = kMargin;

    [self.container addSubview:[self headerWithString:L(@"Privacy", @"Datenschutz") y:y]];
    y += 26;

    CGFloat resetW = 120;
    NSTextField *idLabel = [self labelWithString:L(@"Network identity", @"Netzwerkidentität")
                                           frame:NSMakeRect(kMargin, y + 4, w - resetW - 12, 18)];
    idLabel.font = [NSFont systemFontOfSize:13];
    [self.container addSubview:idLabel];
    NSButton *reset = [NSButton buttonWithTitle:L(@"Reset…", @"Zurücksetzen…")
                                         target:self
                                         action:@selector(resetIdentity:)];
    reset.frame = NSMakeRect(kMargin + w - resetW, y, resetW, 28);
    reset.bezelStyle = NSBezelStyleRounded;
    [self.container addSubview:reset];
    y += 50;

    [self.container addSubview:[self headerWithString:L(@"Help", @"Hilfe") y:y]];
    y += 28;

    NSButton *docs = [NSButton buttonWithTitle:L(@"Documentation", @"Dokumentation")
                                        target:self
                                        action:@selector(openDocs:)];
    docs.frame = NSMakeRect(kMargin, y, 180, 28);
    docs.bezelStyle = NSBezelStyleRounded;
    [self.container addSubview:docs];

    NSButton *diag = [NSButton buttonWithTitle:L(@"Copy diagnostics", @"Diagnose kopieren")
                                        target:self
                                        action:@selector(copyDiagnostics:)];
    diag.frame = NSMakeRect(kMargin + 188, y, 180, 28);
    diag.bezelStyle = NSBezelStyleRounded;
    [self.container addSubview:diag];
    y += 36;

    NSButton *credits = [NSButton buttonWithTitle:L(@"Acknowledgements", @"Danksagungen")
                                           target:self
                                           action:@selector(showCredits:)];
    credits.frame = NSMakeRect(kMargin, y, 180, 28);
    credits.bezelStyle = NSBezelStyleRounded;
    [self.container addSubview:credits];
}

#pragma mark - Actions

- (void)resetIdentity:(id)sender {
    NSAlert *confirm = [[NSAlert alloc] init];
    confirm.messageText = L(@"Reset network identity?", @"Netzwerkidentität zurücksetzen?");
    confirm.informativeText =
        L(@"This installation has a stable, anonymous peer ID so others can recognize you "
          @"across sessions. If you reset it, ReaMesh creates a new ID the next time you "
          @"join or host.",
          @"Diese Installation hat eine feste, anonyme Peer-ID, an der dich andere über "
          @"mehrere Sessions hinweg erkennen. Nach dem Zurücksetzen erstellt ReaMesh beim "
          @"nächsten Beitritt oder Hosting eine neue ID.");
    [confirm addButtonWithTitle:L(@"Reset", @"Zurücksetzen")];
    [confirm addButtonWithTitle:L(@"Cancel", @"Abbrechen")];
    [confirm beginSheetModalForWindow:self.window
                    completionHandler:^(NSModalResponse response) {
        if (response != NSAlertFirstButtonReturn) return;
        int32_t r = self.cb.reset_identity(self.cb.ctx);
        // Show the outcome once the confirm sheet is gone.
        dispatch_async(dispatch_get_main_queue(), ^{
            NSAlert *done = [[NSAlert alloc] init];
            if (r < 0) {
                done.alertStyle = NSAlertStyleWarning;
                done.messageText = L(@"Couldn't reset the identity",
                                     @"Identität konnte nicht zurückgesetzt werden");
            } else if (r > 0) {
                done.messageText = L(@"Identity reset. ReaMesh will create a new one for your "
                                     @"next session.",
                                     @"Identität zurückgesetzt. Für die nächste Session "
                                     @"erstellt ReaMesh eine neue.");
            } else {
                done.messageText = L(@"No stored identity yet.",
                                     @"Noch keine gespeicherte Identität.");
            }
            [done beginSheetModalForWindow:self.window completionHandler:nil];
        });
    }];
}

- (void)openDocs:(id)sender {
    [[NSWorkspace sharedWorkspace] openURL:[NSURL URLWithString:kDocsURL]];
}

- (void)copyDiagnostics:(NSButton *)sender {
    self.cb.copy_diagnostics(self.cb.ctx);
    NSString *title = sender.title;
    sender.title = L(@"Copied", @"Kopiert");
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 2 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
        sender.title = title;
    });
}

- (void)removeExtension:(id)sender {
    int32_t ok = self.cb.remove(self.cb.ctx);
    [self showTab:self.selectedTab];
    NSAlert *alert = [[NSAlert alloc] init];
    if (ok) {
        alert.messageText = L(@"Extension removed", @"Erweiterung entfernt");
        alert.informativeText = L(@"Restart REAPER to unload it.",
                                  @"Starte REAPER neu, damit die Änderung wirksam wird.");
    } else {
        alert.alertStyle = NSAlertStyleWarning;
        alert.messageText = L(@"Couldn't remove the extension",
                              @"Erweiterung konnte nicht entfernt werden");
    }
    [alert beginSheetModalForWindow:self.window completionHandler:nil];
}

- (void)languageChanged:(NSPopUpButton *)sender {
    // Item index == kReaMeshLang* value. Rust persists it, rebuilds the menu
    // bar, and notifies the webview; this window retitles itself via showTab.
    self.cb.set_language(self.cb.ctx, (int32_t)sender.indexOfSelectedItem);
    [self rebuildToolbar];
    [self showTab:self.selectedTab];
}

// The toolbar caches its item labels: rebuild it so the tab names follow the
// language.
- (void)rebuildToolbar {
    resolveUILanguage(self.cb.get_language(self.cb.ctx));
    NSToolbar *toolbar = [[NSToolbar alloc] initWithIdentifier:@"ReaMeshSettingsToolbar"];
    toolbar.delegate = self;
    toolbar.allowsUserCustomization = NO;
    toolbar.selectedItemIdentifier = self.selectedTab;
    self.window.toolbar = toolbar;
}

- (void)changeFolder:(id)sender {
    NSOpenPanel *panel = [NSOpenPanel openPanel];
    panel.canChooseDirectories = YES;
    panel.canChooseFiles = NO;
    panel.allowsMultipleSelection = NO;
    panel.title = L(@"Choose your REAPER folder", @"REAPER-Ordner auswählen");
    [panel beginSheetModalForWindow:self.window
                  completionHandler:^(NSModalResponse result) {
        if (result == NSModalResponseOK && panel.URLs.count > 0) {
            NSString *p = panel.URLs.firstObject.path;
            self.cb.set_path(self.cb.ctx, p.UTF8String);
            [self showTab:self.selectedTab];
        }
    }];
}

- (void)useDefault:(id)sender {
    self.cb.set_path(self.cb.ctx, NULL);
    [self showTab:self.selectedTab];
}

- (void)openDownload:(id)sender {
    [[NSWorkspace sharedWorkspace] openURL:[NSURL URLWithString:kReaperDownloadURL]];
}

- (void)install:(id)sender {
    int32_t ok = self.cb.install(self.cb.ctx);
    [self showTab:self.selectedTab];
    NSAlert *alert = [[NSAlert alloc] init];
    if (ok) {
        alert.messageText = L(@"Extension installed", @"Erweiterung installiert");
        alert.informativeText =
            L(@"Start or restart REAPER to load the latest ReaMesh extension.",
              @"Starte REAPER (neu), um die aktuelle ReaMesh-Erweiterung zu laden.");
    } else {
        alert.alertStyle = NSAlertStyleWarning;
        alert.messageText = L(@"Installation failed", @"Installation fehlgeschlagen");
        alert.informativeText =
            L(@"Couldn't copy the extension into REAPER's UserPlugins folder. "
              @"Check that the REAPER folder is correct in the General tab.",
              @"Die Erweiterung konnte nicht in REAPERs UserPlugins-Ordner kopiert "
              @"werden. Prüfe im Tab „Allgemein“, ob der REAPER-Ordner stimmt.");
    }
    [alert beginSheetModalForWindow:self.window completionHandler:nil];
}

#pragma mark - NSWindowDelegate

- (void)windowWillClose:(NSNotification *)notification {
    // Keep gController alive so re-opening reuses the same window.
}

@end

void reamesh_settings_refresh(void) {
    if (gController && gController.window.isVisible) {
        [gController rebuildToolbar];
        [gController showTab:gController.selectedTab];
    }
}

void reamesh_pasteboard_set(const char *text) {
    NSPasteboard *pb = [NSPasteboard generalPasteboard];
    [pb clearContents];
    [pb setString:(text ? @(text) : @"") forType:NSPasteboardTypeString];
}

void reamesh_settings_open(SettingsCallbacks cb) {
    if (gController) {
        gController.cb = cb;
        [gController rebuildToolbar];
        [gController showTab:gController.selectedTab];
        [gController.window makeKeyAndOrderFront:nil];
    } else {
        gController = [[ReaMeshSettingsController alloc] initWithCallbacks:cb];
    }
    [NSApp activateIgnoringOtherApps:YES];
}
