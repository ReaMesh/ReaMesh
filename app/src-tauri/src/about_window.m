#import "about_window.h"
#import <Cocoa/Cocoa.h>

// Native macOS About window. Replaces the standard about panel, which can't
// run an in-app action (the acknowledgements link) or follow the app's own
// language preference.

static NSString *const kSourceURL = @"https://github.com/ReaMesh/ReaMesh";
static const CGFloat kAboutWidth = 360;

static BOOL gAboutGerman = NO;

static NSString *AL(NSString *en, NSString *de) {
    return gAboutGerman ? de : en;
}

// Esc closes, like the standard panel.
@interface ReaMeshAboutWindow : NSWindow
@end
@implementation ReaMeshAboutWindow
- (void)cancelOperation:(id)sender { [self performClose:sender]; }
@end

@interface ReaMeshAboutController : NSObject
@property (strong) ReaMeshAboutWindow *window;
@property (assign) AboutInfo info;
@property (copy) NSString *commit;
@end

static ReaMeshAboutController *gAbout = nil;

static NSTextField *aboutLabel(NSString *text, NSFont *font, NSColor *color) {
    NSTextField *f = [NSTextField wrappingLabelWithString:text];
    f.font = font;
    f.textColor = color;
    f.alignment = NSTextAlignmentCenter;
    f.selectable = NO;
    f.preferredMaxLayoutWidth = kAboutWidth - 48;
    return f;
}

@implementation ReaMeshAboutController

- (instancetype)init {
    self = [super init];
    if (!self) return self;
    ReaMeshAboutWindow *win = [[ReaMeshAboutWindow alloc]
        initWithContentRect:NSMakeRect(0, 0, kAboutWidth, 300)
                  styleMask:(NSWindowStyleMaskTitled | NSWindowStyleMaskClosable |
                             NSWindowStyleMaskFullSizeContentView)
                    backing:NSBackingStoreBuffered
                      defer:NO];
    win.titlebarAppearsTransparent = YES;
    win.titleVisibility = NSWindowTitleHidden;
    win.movableByWindowBackground = YES;
    win.releasedWhenClosed = NO;
    [win standardWindowButton:NSWindowMiniaturizeButton].hidden = YES;
    [win standardWindowButton:NSWindowZoomButton].hidden = YES;
    _window = win;
    return self;
}

// The bundled app's icon as Finder draws it (the system-rendered .icon); a
// bare dev binary has no bundle icon, so it falls back to the shipped PNG.
- (NSImage *)appIcon {
    NSString *bundle = [NSBundle mainBundle].bundlePath;
    if ([bundle hasSuffix:@".app"]) {
        return [[NSWorkspace sharedWorkspace] iconForFile:bundle];
    }
    if (_info.icon_png && _info.icon_png_len) {
        NSData *data = [NSData dataWithBytes:_info.icon_png length:_info.icon_png_len];
        NSImage *img = [[NSImage alloc] initWithData:data];
        if (img) return img;
    }
    return NSApp.applicationIconImage;
}

- (NSString *)buildLine {
    NSMutableArray<NSString *> *parts = [NSMutableArray array];
    [parts addObject:_info.release ? AL(@"Release build", @"Release-Build")
                                   : AL(@"Dev build", @"Dev-Build")];
    if (_info.build_time > 0) {
        NSDateFormatter *fmt = [[NSDateFormatter alloc] init];
        fmt.locale = [NSLocale localeWithLocaleIdentifier:gAboutGerman ? @"de_AT" : @"en_US"];
        fmt.dateStyle = NSDateFormatterMediumStyle;
        fmt.timeStyle = NSDateFormatterShortStyle;
        [parts addObject:[fmt stringFromDate:[NSDate dateWithTimeIntervalSince1970:_info.build_time]]];
    }
    [parts addObject:[NSString stringWithFormat:AL(@"Protocol %u", @"Protokoll %u"), _info.proto]];
    return [parts componentsJoinedByString:@" · "];
}

- (NSButton *)linkButton:(NSString *)title action:(SEL)action {
    NSButton *b = [NSButton buttonWithTitle:title target:self action:action];
    b.bordered = NO;
    b.attributedTitle = [[NSAttributedString alloc]
        initWithString:title
            attributes:@{
                NSFontAttributeName : [NSFont systemFontOfSize:11],
                NSForegroundColorAttributeName : NSColor.linkColor,
            }];
    return b;
}

- (void)openSource:(id)sender {
    [[NSWorkspace sharedWorkspace] openURL:[NSURL URLWithString:kSourceURL]];
}

- (void)openCommit:(id)sender {
    NSString *url = [NSString stringWithFormat:@"%@/commit/%@", kSourceURL, _commit];
    [[NSWorkspace sharedWorkspace] openURL:[NSURL URLWithString:url]];
}

- (void)openCredits:(id)sender {
    if (_info.show_credits) _info.show_credits(_info.ctx);
}

- (void)rebuild {
    NSFont *small = [NSFont systemFontOfSize:11];
    NSFont *tiny = [NSFont systemFontOfSize:10];

    NSImageView *icon = [NSImageView imageViewWithImage:[self appIcon]];
    icon.imageScaling = NSImageScaleProportionallyUpOrDown;
    [icon.widthAnchor constraintEqualToConstant:96].active = YES;
    [icon.heightAnchor constraintEqualToConstant:96].active = YES;

    NSString *version = [NSString stringWithUTF8String:_info.version ?: ""];
    NSString *commit = [NSString stringWithUTF8String:_info.commit ?: ""];
    NSString *versionText = commit.length
        ? [NSString stringWithFormat:@"Version %@ (%@)", version, commit]
        : [NSString stringWithFormat:@"Version %@", version];

    _commit = commit;

    NSTextField *name = aboutLabel(@"ReaMesh", [NSFont boldSystemFontOfSize:14], NSColor.labelColor);
    NSView *ver;
    if (_info.release && commit.length) {
        // A release build's commit is on GitHub: the hash links to it. A dev
        // build's may exist only locally, so it stays plain text there.
        NSString *pre = [NSString stringWithFormat:@"Version %@ (", version];
        NSMutableAttributedString *title = [[NSMutableAttributedString alloc]
            initWithString:[NSString stringWithFormat:@"%@%@)", pre, commit]
                attributes:@{
                    NSFontAttributeName : small,
                    NSForegroundColorAttributeName : NSColor.labelColor,
                }];
        [title addAttribute:NSForegroundColorAttributeName
                      value:NSColor.linkColor
                      range:NSMakeRange(pre.length, commit.length)];
        NSButton *link = [NSButton buttonWithTitle:title.string
                                            target:self
                                            action:@selector(openCommit:)];
        link.bordered = NO;
        link.attributedTitle = title;
        ver = link;
    } else {
        NSTextField *plain = aboutLabel(versionText, small, NSColor.labelColor);
        plain.selectable = YES; // so the commit can be copied for a bug report
        ver = plain;
    }
    NSTextField *subtitle = aboutLabel(
        AL(@"Realtime, peer-to-peer collaboration for REAPER",
           @"Zusammenarbeit in Echtzeit für REAPER, Peer-to-Peer"),
        small, NSColor.labelColor);
    NSTextField *build = aboutLabel([self buildLine], small, NSColor.secondaryLabelColor);

    NSTextField *dot = aboutLabel(@"·", small, NSColor.tertiaryLabelColor);
    NSStackView *links = [NSStackView stackViewWithViews:@[
        [self linkButton:AL(@"Source code", @"Quellcode") action:@selector(openSource:)],
        dot,
        [self linkButton:AL(@"Acknowledgements", @"Danksagungen") action:@selector(openCredits:)],
    ]];
    links.orientation = NSUserInterfaceLayoutOrientationHorizontal;
    links.spacing = 6;

    NSTextField *cockos = aboutLabel(
        AL(@"REAPER is a trademark of Cockos Incorporated.\n"
           @"ReaMesh is not affiliated with or endorsed by Cockos.",
           @"REAPER ist eine Marke von Cockos Incorporated.\n"
           @"ReaMesh ist unabhängig und nicht mit Cockos verbunden."),
        tiny, NSColor.tertiaryLabelColor);
    NSTextField *copyright = aboutLabel(@"© 2026 tiszu · GPL-3.0", tiny,
                                        NSColor.secondaryLabelColor);

    NSStackView *stack = [NSStackView stackViewWithViews:@[
        icon, name, ver, subtitle, build, links, cockos, copyright,
    ]];
    stack.orientation = NSUserInterfaceLayoutOrientationVertical;
    stack.alignment = NSLayoutAttributeCenterX;
    stack.spacing = 4;
    [stack setCustomSpacing:10 afterView:icon];
    [stack setCustomSpacing:14 afterView:ver];
    [stack setCustomSpacing:6 afterView:build];
    [stack setCustomSpacing:18 afterView:links];
    [stack setCustomSpacing:10 afterView:cockos];
    stack.edgeInsets = NSEdgeInsetsMake(36, 24, 22, 24);
    stack.translatesAutoresizingMaskIntoConstraints = NO;

    NSView *content = [[NSView alloc] init];
    [content addSubview:stack];
    [NSLayoutConstraint activateConstraints:@[
        [stack.topAnchor constraintEqualToAnchor:content.topAnchor],
        [stack.bottomAnchor constraintEqualToAnchor:content.bottomAnchor],
        [stack.leadingAnchor constraintEqualToAnchor:content.leadingAnchor],
        [stack.trailingAnchor constraintEqualToAnchor:content.trailingAnchor],
        [content.widthAnchor constraintEqualToConstant:kAboutWidth],
    ]];

    _window.title = AL(@"About ReaMesh", @"Über ReaMesh");
    _window.contentView = content;
    [content layoutSubtreeIfNeeded];
    [_window setContentSize:content.fittingSize];
}

@end

void reamesh_about_open(AboutInfo info) {
    gAboutGerman = info.german != 0;
    BOOL fresh = gAbout == nil;
    if (fresh) gAbout = [[ReaMeshAboutController alloc] init];
    gAbout.info = info;
    // Copy the borrowed strings now; the Rust side frees them after this call.
    // (The icon bytes are static, the strings are only read inside rebuild.)
    [gAbout rebuild];
    AboutInfo kept = gAbout.info;
    kept.version = NULL;
    kept.commit = NULL;
    gAbout.info = kept;
    if (fresh || !gAbout.window.visible) [gAbout.window center];
    [NSApp activateIgnoringOtherApps:YES];
    [gAbout.window makeKeyAndOrderFront:nil];
}
