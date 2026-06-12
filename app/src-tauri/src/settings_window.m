#import "settings_window.h"
#import <Cocoa/Cocoa.h>

// Native macOS settings window: a real NSWindow with an NSToolbar in the
// centered "preference" style (the System-Settings / Finder-Settings look),
// switching between a General pane (REAPER folder) and an Extension pane
// (install / update). All REAPER logic lives in Rust and is reached through
// the SettingsCallbacks function pointers.

static NSString *const kTabGeneral = @"general";
static NSString *const kTabExtension = @"extension";
static NSString *const kReaperDownloadURL = @"https://www.reaper.fm/download.php";

static const CGFloat kWindowWidth = 480;
static const CGFloat kWindowHeight = 300;
static const CGFloat kMargin = 24;

// A flipped container so we can lay controls out top-down (y grows downward),
// which is far easier to reason about than AppKit's default bottom-left origin.
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

    NSRect frame = NSMakeRect(0, 0, kWindowWidth, kWindowHeight);
    NSWindow *win = [[NSWindow alloc]
        initWithContentRect:frame
                  styleMask:(NSWindowStyleMaskTitled | NSWindowStyleMaskClosable |
                             NSWindowStyleMaskMiniaturizable)
                    backing:NSBackingStoreBuffered
                      defer:NO];
    win.title = @"ReaMesh Settings";
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
    return @[ kTabGeneral, kTabExtension ];
}
- (NSArray<NSToolbarItemIdentifier> *)toolbarAllowedItemIdentifiers:(NSToolbar *)t {
    return @[ kTabGeneral, kTabExtension ];
}
- (NSArray<NSToolbarItemIdentifier> *)toolbarSelectableItemIdentifiers:(NSToolbar *)t {
    return @[ kTabGeneral, kTabExtension ];
}

- (NSToolbarItem *)toolbar:(NSToolbar *)toolbar
        itemForItemIdentifier:(NSToolbarItemIdentifier)identifier
    willBeInsertedIntoToolbar:(BOOL)flag {
    NSToolbarItem *item = [[NSToolbarItem alloc] initWithItemIdentifier:identifier];
    item.target = self;
    item.action = @selector(tabSelected:);
    if ([identifier isEqualToString:kTabGeneral]) {
        item.label = @"General";
        if (@available(macOS 11.0, *)) {
            item.image = [NSImage imageWithSystemSymbolName:@"folder"
                                   accessibilityDescription:@"General"];
        }
    } else {
        item.label = @"Extension";
        if (@available(macOS 11.0, *)) {
            item.image = [NSImage imageWithSystemSymbolName:@"puzzlepiece.extension"
                                   accessibilityDescription:@"Extension"];
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
    ReaperStatusC st = self.cb.get_status(self.cb.ctx);
    if ([identifier isEqualToString:kTabExtension]) {
        [self buildExtensionPane:st];
    } else {
        [self buildGeneralPane:st];
    }
}

- (void)buildGeneralPane:(ReaperStatusC)st {
    CGFloat w = kWindowWidth - 2 * kMargin;
    CGFloat y = kMargin;

    [self.container addSubview:[self headerWithString:@"REAPER folder" y:y]];
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
            [self labelWithString:@"No REAPER folder found at this location."
                            frame:NSMakeRect(kMargin, y, w, 16)];
        warn.textColor = [NSColor systemOrangeColor];
        warn.font = [NSFont systemFontOfSize:11];
        [self.container addSubview:warn];
        y += 24;
    }

    NSButton *change = [NSButton buttonWithTitle:@"Change…"
                                          target:self
                                          action:@selector(changeFolder:)];
    change.frame = NSMakeRect(kMargin, y, 100, 24);
    [self.container addSubview:change];

    if (st.has_custom_path) {
        NSButton *reset = [NSButton buttonWithTitle:@"Use default"
                                             target:self
                                             action:@selector(useDefault:)];
        reset.frame = NSMakeRect(kMargin + 108, y, 110, 24);
        [self.container addSubview:reset];
    }
    y += 34;

    if (!st.reaper_found) {
        NSButton *dl = [NSButton buttonWithTitle:@"Don't have REAPER? Download it"
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
    }
}

- (void)buildExtensionPane:(ReaperStatusC)st {
    CGFloat w = kWindowWidth - 2 * kMargin;
    CGFloat y = kMargin;

    [self.container addSubview:[self headerWithString:@"REAPER extension" y:y]];
    y += 28;

    NSString *statusText;
    NSColor *statusColor;
    if (!st.installed) {
        statusText = @"reaper_reamesh: Not installed";
        statusColor = [NSColor secondaryLabelColor];
    } else if (st.up_to_date) {
        statusText = @"reaper_reamesh: Installed";
        statusColor = [NSColor systemGreenColor];
    } else {
        statusText = @"reaper_reamesh: Update available";
        statusColor = [NSColor systemOrangeColor];
    }
    NSTextField *statusLabel =
        [self labelWithString:statusText frame:NSMakeRect(kMargin, y, w, 18)];
    statusLabel.textColor = statusColor;
    statusLabel.font = [NSFont systemFontOfSize:13];
    [self.container addSubview:statusLabel];
    y += 34;

    NSString *title = !st.installed ? @"Install extension"
                    : st.up_to_date ? @"Reinstall extension"
                                    : @"Update extension";
    NSButton *installBtn = [NSButton buttonWithTitle:title
                                              target:self
                                              action:@selector(install:)];
    installBtn.frame = NSMakeRect(kMargin, y, 180, 28);
    installBtn.bezelStyle = NSBezelStyleRounded;
    installBtn.keyEquivalent = @"\r";
    installBtn.enabled = st.reaper_found ? YES : NO;
    [self.container addSubview:installBtn];
    y += 40;

    NSTextField *help = [self
        labelWithString:@"REAPER loads the extension at startup. Restart it "
                         "after installing or updating."
                  frame:NSMakeRect(kMargin, y, w, 40)];
    help.textColor = [NSColor secondaryLabelColor];
    help.font = [NSFont systemFontOfSize:11];
    help.lineBreakMode = NSLineBreakByWordWrapping;
    help.maximumNumberOfLines = 0;
    [self.container addSubview:help];
}

#pragma mark - Actions

- (void)changeFolder:(id)sender {
    NSOpenPanel *panel = [NSOpenPanel openPanel];
    panel.canChooseDirectories = YES;
    panel.canChooseFiles = NO;
    panel.allowsMultipleSelection = NO;
    panel.title = @"Locate your REAPER folder";
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
        alert.messageText = @"Extension installed";
        alert.informativeText =
            @"Restart REAPER (or start it) to load the latest ReaMesh extension.";
    } else {
        alert.alertStyle = NSAlertStyleWarning;
        alert.messageText = @"Installation failed";
        alert.informativeText =
            @"Couldn't copy the extension into REAPER's UserPlugins folder. "
             "Check that the REAPER folder is correct in the General tab.";
    }
    [alert beginSheetModalForWindow:self.window completionHandler:nil];
}

#pragma mark - NSWindowDelegate

- (void)windowWillClose:(NSNotification *)notification {
    // Keep gController alive so re-opening reuses the same window.
}

@end

void reamesh_settings_open(SettingsCallbacks cb) {
    if (gController) {
        gController.cb = cb;
        [gController showTab:gController.selectedTab];
        [gController.window makeKeyAndOrderFront:nil];
    } else {
        gController = [[ReaMeshSettingsController alloc] initWithCallbacks:cb];
    }
    [NSApp activateIgnoringOtherApps:YES];
}
