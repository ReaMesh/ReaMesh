#import "overlay_panel.h"
#import <Cocoa/Cocoa.h>
#import <CoreGraphics/CoreGraphics.h>
#import <objc/runtime.h>

// Native chrome for the presence overlay. The webview owns the HUD's content
// and height; everything spatial lives here:
//   • the window is turned into a non-activating NSPanel so dragging/clicking
//     it never steals frontmost from REAPER (which would hide it: see below);
//   • it shows only while REAPER is the frontmost app, fading in/out;
//   • it follows REAPER's main window, staying at a fixed offset from REAPER's
//     top-right corner as REAPER moves/resizes;
//   • dragging it (via the webview's drag region → AppKit window move) rewrites
//     that offset for the rest of this session; it is never persisted, so every
//     relaunch starts from the default anchor below.
// All of this runs on the main thread (timer + AppKit notifications), so the
// state below needs no locking.

static NSString *const kReaperBundleId = @"com.cockos.reaper";
// Default anchor: tuck the overlay's top-right corner just inside REAPER's
// top-right (16pt in from the right edge, 48pt below the top, clear of the
// toolbar).
static const CGFloat kDefaultOffsetX = -16.0;
static const CGFloat kDefaultOffsetY = 48.0;
static const NSTimeInterval kDragPause = 0.25; // suspend follow during a drag

// NSPanel subclass with ZERO added ivars, so object_setClass over tao's
// NSWindow stays layout-compatible. Non-activating panels only behave
// non-activating as an NSPanel; a key-but-not-main panel takes keyboard/clicks
// without activating the owning app.
@interface ReaMeshOverlayPanel : NSPanel
@end
@implementation ReaMeshOverlayPanel
- (BOOL)canBecomeKeyWindow { return YES; }
- (BOOL)canBecomeMainWindow { return NO; }
@end

@interface ReaMeshOverlayController : NSObject <NSWindowDelegate>
@property (strong) NSPanel *panel;
@property (strong) NSTimer *followTimer;
@property (weak) id<NSWindowDelegate> prevDelegate;
@property (assign) Class originalClass; // tao's NSWindow subclass, restored on detach
@property (assign) NSPoint userOffset; // overlayTopRight - reaperTopRight, CG space
@property (assign) BOOL isProgrammaticMove;
@property (assign) BOOL visible;
@property (assign) NSTimeInterval followPausedUntil;
@property (assign) pid_t cachedReaperPID;
@end

static ReaMeshOverlayController *gOverlay = nil;

@implementation ReaMeshOverlayController

#pragma mark - REAPER lookup

- (pid_t)reaperPID {
    if (self.cachedReaperPID > 0) return self.cachedReaperPID;
    for (NSRunningApplication *a in [[NSWorkspace sharedWorkspace] runningApplications]) {
        if ([a.bundleIdentifier isEqualToString:kReaperBundleId]) {
            self.cachedReaperPID = a.processIdentifier;
            break;
        }
    }
    return self.cachedReaperPID;
}

- (BOOL)reaperFrontmost {
    NSRunningApplication *front = [[NSWorkspace sharedWorkspace] frontmostApplication];
    return [front.bundleIdentifier isEqualToString:kReaperBundleId];
}

// REAPER's main window rect in CoreGraphics global space (top-left origin).
// Picks the largest layer-0 on-screen window owned by REAPER: its arrange
// window dominates the floating FX/docked windows. Geometry-only query, so it
// needs no Screen-Recording permission (we never ask for kCGWindowName).
- (BOOL)reaperWindowBounds:(CGRect *)out {
    pid_t pid = [self reaperPID];
    if (pid <= 0) return NO;
    CFArrayRef list = CGWindowListCopyWindowInfo(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        kCGNullWindowID);
    if (!list) return NO;
    CGRect best = CGRectZero;
    CGFloat bestArea = 0;
    BOOL found = NO;
    CFIndex n = CFArrayGetCount(list);
    for (CFIndex i = 0; i < n; i++) {
        NSDictionary *info = (__bridge NSDictionary *)CFArrayGetValueAtIndex(list, i);
        NSNumber *owner = info[(__bridge NSString *)kCGWindowOwnerPID];
        if (owner.intValue != pid) continue;
        NSNumber *layer = info[(__bridge NSString *)kCGWindowLayer];
        if (layer.intValue != 0) continue;
        CGRect b;
        CFDictionaryRef boundsDict =
            (__bridge CFDictionaryRef)info[(__bridge NSString *)kCGWindowBounds];
        if (!boundsDict || !CGRectMakeWithDictionaryRepresentation(boundsDict, &b)) continue;
        CGFloat area = b.size.width * b.size.height;
        if (area > bestArea) {
            bestArea = area;
            best = b;
            found = YES;
        }
    }
    CFRelease(list);
    if (found) *out = best;
    return found;
}

#pragma mark - Positioning

// Height of the primary screen (the one carrying the menu bar): the constant
// that maps CG top-left global Y to AppKit bottom-left global Y.
- (CGFloat)primaryScreenHeight {
    NSArray<NSScreen *> *screens = [NSScreen screens];
    return screens.count ? NSHeight(screens.firstObject.frame) : 0;
}

- (void)reposition {
    if (!self.panel || !self.visible) return;
    if ([NSDate timeIntervalSinceReferenceDate] < self.followPausedUntil) return;
    CGRect r;
    if (![self reaperWindowBounds:&r]) return; // minimized/occluded: freeze in place

    NSRect frame = self.panel.frame;
    CGFloat w = NSWidth(frame);
    CGFloat h = NSHeight(frame); // Read again because the webview resizes it.
    CGFloat H = [self primaryScreenHeight];

    // Anchor the overlay's top-right corner to REAPER's top-right + offset.
    CGFloat overlayRightX_cg = (r.origin.x + r.size.width) + self.userOffset.x;
    CGFloat overlayTopY_cg = r.origin.y + self.userOffset.y;
    NSPoint origin = NSMakePoint(overlayRightX_cg - w, H - overlayTopY_cg - h);
    if (NSEqualPoints(origin, frame.origin)) return;

    self.isProgrammaticMove = YES;
    [self.panel setFrameOrigin:origin];
    self.isProgrammaticMove = NO;
}

#pragma mark - Visibility / fade

- (void)updateVisibility {
    BOOL want = [self reaperFrontmost];
    if (want == self.visible) return;
    self.visible = want;
    if (want) {
        [self reposition]; // place before it's seen
        [self.panel setAlphaValue:0.0];
        [self.panel orderFront:nil];
        [NSAnimationContext runAnimationGroup:^(NSAnimationContext *ctx) {
            ctx.duration = 0.18;
            [self.panel.animator setAlphaValue:1.0];
        }];
    } else {
        [NSAnimationContext runAnimationGroup:^(NSAnimationContext *ctx) {
            ctx.duration = 0.15;
            ctx.completionHandler = ^{
                if (!self.visible) [self.panel orderOut:nil];
            };
            [self.panel.animator setAlphaValue:0.0];
        }];
    }
}

- (void)activeAppChanged:(NSNotification *)note {
    self.cachedReaperPID = 0; // REAPER may have relaunched with a new pid
    [self updateVisibility];
}

#pragma mark - NSWindowDelegate

- (void)windowDidMove:(NSNotification *)note {
    if (self.isProgrammaticMove || !self.panel) return;
    // User dragged the overlay: rewrite the offset from where they put it, and
    // hold off the follow timer briefly so it doesn't fight the drag.
    CGRect r;
    if (![self reaperWindowBounds:&r]) return;
    NSRect f = self.panel.frame;
    CGFloat H = [self primaryScreenHeight];
    CGFloat overlayRightX_cg = NSMaxX(f);
    CGFloat overlayTopY_cg = H - NSMaxY(f);
    CGFloat reaperRightX = r.origin.x + r.size.width;
    CGFloat reaperTopY = r.origin.y;
    // Session-only: the drag holds until the overlay detaches; it is never saved.
    self.userOffset = NSMakePoint(overlayRightX_cg - reaperRightX, overlayTopY_cg - reaperTopY);
    self.followPausedUntil = [NSDate timeIntervalSinceReferenceDate] + kDragPause;
}

- (void)windowDidResize:(NSNotification *)note {
    // The webview resizes the window to fit the card; re-pin the top corner so
    // it grows/shrinks downward instead of jumping.
    [self reposition];
    if ([self.prevDelegate respondsToSelector:_cmd]) {
        [self.prevDelegate windowDidResize:note];
    }
}

// Forward any other delegate calls to tao's original delegate so Tauri's own
// window bookkeeping keeps working.
- (BOOL)respondsToSelector:(SEL)aSelector {
    if ([super respondsToSelector:aSelector]) return YES;
    return [self.prevDelegate respondsToSelector:aSelector];
}
- (id)forwardingTargetForSelector:(SEL)aSelector {
    if ([self.prevDelegate respondsToSelector:aSelector]) return self.prevDelegate;
    return nil;
}

#pragma mark - Attach / detach

- (void)attachToWindow:(NSWindow *)window {
    // Turn it into a non-activating floating panel. Remember tao's original
    // window class so detach can restore it: closing a window whose isa we've
    // swapped away from tao's objc2 subclass dispatches into the wrong class and
    // throws an ObjC exception (which aborts the process, since it can't unwind
    // through tao's Rust run-loop observer).
    self.originalClass = object_getClass(window);
    object_setClass(window, [ReaMeshOverlayPanel class]);
    NSPanel *panel = (NSPanel *)window;
    panel.styleMask |= NSWindowStyleMaskNonactivatingPanel;
    panel.collectionBehavior |= (NSWindowCollectionBehaviorCanJoinAllSpaces |
                                 NSWindowCollectionBehaviorFullScreenAuxiliary |
                                 NSWindowCollectionBehaviorStationary);
    panel.floatingPanel = YES;
    panel.becomesKeyOnlyIfNeeded = YES;
    panel.hidesOnDeactivate = NO;
    panel.level = NSFloatingWindowLevel;

    self.panel = panel;
    self.prevDelegate = panel.delegate;
    panel.delegate = self;
    self.cachedReaperPID = 0;
    // Always start from the default anchor; the dragged offset lives only for
    // the life of this attach (one session), never across relaunches.
    self.userOffset = NSMakePoint(kDefaultOffsetX, kDefaultOffsetY);

    [[[NSWorkspace sharedWorkspace] notificationCenter]
        addObserver:self
           selector:@selector(activeAppChanged:)
               name:NSWorkspaceDidActivateApplicationNotification
             object:nil];

    self.followTimer = [NSTimer timerWithTimeInterval:(1.0 / 60.0)
                                               target:self
                                             selector:@selector(reposition)
                                             userInfo:nil
                                              repeats:YES];
    [[NSRunLoop mainRunLoop] addTimer:self.followTimer forMode:NSRunLoopCommonModes];

    self.visible = NO;
    [panel setAlphaValue:0.0];
    [panel orderOut:nil];
    [self updateVisibility];
}

- (void)detach {
    [self.followTimer invalidate];
    self.followTimer = nil;
    [[[NSWorkspace sharedWorkspace] notificationCenter] removeObserver:self];
    if (self.panel) {
        NSPanel *panel = self.panel;
        panel.delegate = self.prevDelegate;
        self.prevDelegate = nil;
        // Undo the panel-ization before Tauri/tao closes its own window. The
        // non-activating style bit is only valid on an NSPanel, so clear it
        // while the class is still our panel subclass, then restore tao's
        // original class so close/dealloc dispatch hits tao's implementations.
        panel.styleMask &= ~NSWindowStyleMaskNonactivatingPanel;
        if (self.originalClass) {
            object_setClass(panel, self.originalClass);
            self.originalClass = nil;
        }
        self.panel = nil;
    }
}

@end

#pragma mark - C ABI

void reamesh_overlay_attach(void *ns_window, double width) {
    (void)width; // width is fixed by the webview; kept in the ABI for symmetry
    if (!ns_window) return;
    NSWindow *window = (__bridge NSWindow *)ns_window;
    if (!gOverlay) gOverlay = [[ReaMeshOverlayController alloc] init];
    [gOverlay attachToWindow:window];
}

void reamesh_overlay_detach(void) {
    if (gOverlay) [gOverlay detach];
}
