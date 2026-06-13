#import "overlay_panel.h"
#import <Cocoa/Cocoa.h>
#import <CoreGraphics/CoreGraphics.h>
#import <QuartzCore/QuartzCore.h>
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
// All of this runs on the main thread (display link / timer + AppKit
// notifications), so the state below needs no locking.

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
@property (strong) NSTimer *followTimer; // pre-macOS-14 fallback for followLink
@property (strong) id followLink; // CADisplayLink (macOS 14+), vsync-aligned
@property (weak) id<NSWindowDelegate> prevDelegate;
@property (assign) Class originalClass; // tao's NSWindow subclass, restored on detach
@property (assign) NSPoint userOffset; // overlayTopRight - reaperTopRight, CG space
@property (assign) BOOL isProgrammaticMove;
@property (assign) BOOL visible;
@property (assign) NSTimeInterval followPausedUntil;
@property (assign) pid_t cachedReaperPID;
@property (assign) CGWindowID cachedWindowID; // REAPER's main window, re-resolved when stale
@property (assign) NSTimeInterval nextWindowScan; // throttle for failed full scans
@property (strong) NSTimer *visibilityTimer; // always-running show/hide supervisor (followLink suspends while hidden)
@property (strong) NSWindow *arrangeWindow;  // full-REAPER-window, click-through overlay for peer playheads/ghosts
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

// (Re)resolve which REAPER window to follow: the largest layer-0 on-screen
// window: its arrange window dominates the floating FX/docked windows. This
// scans every window on the system, far too expensive per display frame, so it
// runs only when there is no cached window id (throttled when it keeps
// failing, e.g. REAPER minimized). Geometry-only query, so it needs no
// Screen-Recording permission (we never ask for kCGWindowName).
- (BOOL)resolveReaperWindow:(CGRect *)out {
    pid_t pid = [self reaperPID];
    if (pid <= 0) return NO;
    NSTimeInterval now = [NSDate timeIntervalSinceReferenceDate];
    if (now < self.nextWindowScan) return NO;
    CFArrayRef list = CGWindowListCopyWindowInfo(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        kCGNullWindowID);
    if (!list) return NO;
    CGRect best = CGRectZero;
    CGFloat bestArea = 0;
    CGWindowID bestID = kCGNullWindowID;
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
            bestID = (CGWindowID)[info[(__bridge NSString *)kCGWindowNumber] unsignedIntValue];
        }
    }
    CFRelease(list);
    if (bestID == kCGNullWindowID) {
        self.nextWindowScan = now + 0.25;
        return NO;
    }
    self.cachedWindowID = bestID;
    *out = best;
    return YES;
}

// REAPER's main window rect in CoreGraphics global space (top-left origin).
// The per-frame path queries only the cached window id: a single-window
// lookup, cheap enough for a 120 Hz display link.
- (BOOL)reaperWindowBounds:(CGRect *)out {
    if (self.cachedWindowID == kCGNullWindowID) return [self resolveReaperWindow:out];
    BOOL stale = YES, found = NO;
    CFArrayRef list = CGWindowListCopyWindowInfo(kCGWindowListOptionIncludingWindow,
                                                 self.cachedWindowID);
    if (list) {
        if (CFArrayGetCount(list) == 1) {
            NSDictionary *info = (__bridge NSDictionary *)CFArrayGetValueAtIndex(list, 0);
            NSNumber *owner = info[(__bridge NSString *)kCGWindowOwnerPID];
            if (owner.intValue == [self reaperPID]) {
                stale = NO;
                // Off-screen (minimized/hidden): report nothing so the overlay
                // freezes in place, but keep the cache: the window is alive.
                NSNumber *onscreen = info[(__bridge NSString *)kCGWindowIsOnscreen];
                CGRect b;
                CFDictionaryRef boundsDict =
                    (__bridge CFDictionaryRef)info[(__bridge NSString *)kCGWindowBounds];
                if (onscreen.boolValue && boundsDict &&
                    CGRectMakeWithDictionaryRepresentation(boundsDict, &b)) {
                    *out = b;
                    found = YES;
                }
            }
        }
        CFRelease(list);
    }
    if (stale) { // window closed or the id got recycled by another process
        self.cachedWindowID = kCGNullWindowID;
        return [self resolveReaperWindow:out];
    }
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

    // Arrange overlay (if present): cover REAPER's whole window so its webview
    // can paint peer playheads/ghosts in window-relative coordinates. CG
    // top-left rect -> AppKit bottom-left frame.
    if (self.arrangeWindow) {
        NSRect af = NSMakeRect(r.origin.x, H - r.origin.y - r.size.height,
                               r.size.width, r.size.height);
        if (!NSEqualRects(af, self.arrangeWindow.frame))
            [self.arrangeWindow setFrame:af display:YES];
    }

    // Anchor the overlay's top-right corner to REAPER's top-right + offset.
    CGFloat overlayRightX_cg = (r.origin.x + r.size.width) + self.userOffset.x;
    CGFloat overlayTopY_cg = r.origin.y + self.userOffset.y;
    NSPoint origin = NSMakePoint(overlayRightX_cg - w, H - overlayTopY_cg - h);
    if (NSEqualPoints(origin, frame.origin)) return;

    self.isProgrammaticMove = YES;
    [self.panel setFrameOrigin:origin];
    self.isProgrammaticMove = NO;
}

// CADisplayLink requires a one-argument selector; typed id so the method
// compiles below the macOS 14 deployment guard.
- (void)followTick:(id)link {
    [self reposition];
}

- (void)visibilityTick:(NSTimer *)t {
    [self updateVisibility];
}

#pragma mark - Visibility / fade

- (void)updateVisibility {
    // Show only while REAPER is frontmost AND its window is actually on screen.
    // The on-screen half is what handles minimize: a minimized REAPER stays
    // "frontmost" (it still owns the menu bar) but its window reports
    // !kCGWindowIsOnscreen, so we must hide, and re-show on restore. The
    // reaperWindowBounds query is the cheap cached-window path; skip it entirely
    // when REAPER isn't frontmost so an idle background poll never triggers a
    // full window scan.
    BOOL want = NO;
    if ([self reaperFrontmost]) {
        CGRect r;
        want = [self reaperWindowBounds:&r];
    }
    if (want == self.visible) return;
    self.visible = want;
    if (want) {
        [self reposition]; // place before it's seen (positions both windows)
        [self.panel setAlphaValue:0.0];
        [self.panel orderFront:nil];
        [NSAnimationContext runAnimationGroup:^(NSAnimationContext *ctx) {
            ctx.duration = 0.18;
            [self.panel.animator setAlphaValue:1.0];
        }];
        if (self.arrangeWindow) {
            [self.arrangeWindow setAlphaValue:0.0];
            [self.arrangeWindow orderFront:nil];
            [NSAnimationContext runAnimationGroup:^(NSAnimationContext *ctx) {
                ctx.duration = 0.18;
                [self.arrangeWindow.animator setAlphaValue:1.0];
            }];
        }
    } else {
        [NSAnimationContext runAnimationGroup:^(NSAnimationContext *ctx) {
            ctx.duration = 0.15;
            ctx.completionHandler = ^{
                if (!self.visible) [self.panel orderOut:nil];
            };
            [self.panel.animator setAlphaValue:0.0];
        }];
        if (self.arrangeWindow) {
            [NSAnimationContext runAnimationGroup:^(NSAnimationContext *ctx) {
                ctx.duration = 0.15;
                ctx.completionHandler = ^{
                    if (!self.visible) [self.arrangeWindow orderOut:nil];
                };
                [self.arrangeWindow.animator setAlphaValue:0.0];
            }];
        }
    }
}

- (void)activeAppChanged:(NSNotification *)note {
    self.cachedReaperPID = 0; // REAPER may have relaunched with a new pid
    self.cachedWindowID = kCGNullWindowID; // …and its windows with it
    self.nextWindowScan = 0;
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
    self.cachedWindowID = kCGNullWindowID;
    self.nextWindowScan = 0;
    // Always start from the default anchor: the dragged offset lives only for
    // the life of this attach (one session), never across relaunches.
    self.userOffset = NSMakePoint(kDefaultOffsetX, kDefaultOffsetY);

    [[[NSWorkspace sharedWorkspace] notificationCenter]
        addObserver:self
           selector:@selector(activeAppChanged:)
               name:NSWorkspaceDidActivateApplicationNotification
             object:nil];

    // Follow on the display's refresh where available: the callback fires
    // vsync-aligned (no free-running-timer phase lag) at the panel's display
    // rate: 120 Hz on ProMotion. The link tracks the panel across displays
    // and auto-suspends while it's ordered out.
    if (@available(macOS 14.0, *)) {
        CADisplayLink *link = [panel displayLinkWithTarget:self
                                                  selector:@selector(followTick:)];
        [link addToRunLoop:[NSRunLoop mainRunLoop] forMode:NSRunLoopCommonModes];
        self.followLink = link;
    } else {
        self.followTimer = [NSTimer timerWithTimeInterval:(1.0 / 60.0)
                                                   target:self
                                                 selector:@selector(reposition)
                                                 userInfo:nil
                                                  repeats:YES];
        [[NSRunLoop mainRunLoop] addTimer:self.followTimer forMode:NSRunLoopCommonModes];
    }

    // Decide show/hide on a steady cadence that runs even while the overlay is
    // hidden. The follow link above auto-suspends once the panel is ordered out,
    // and macOS posts no app-activation notification when REAPER's own window
    // minimizes/restores while REAPER stays frontmost, so without this poll the
    // overlay would stay gone after an un-minimize until REAPER lost and regained
    // focus. 10 Hz: imperceptible against the deminiaturize animation, and each
    // tick is just a frontmost check plus (only when frontmost) one cached-window
    // query.
    self.visibilityTimer = [NSTimer timerWithTimeInterval:0.1
                                                   target:self
                                                 selector:@selector(visibilityTick:)
                                                 userInfo:nil
                                                  repeats:YES];
    [[NSRunLoop mainRunLoop] addTimer:self.visibilityTimer forMode:NSRunLoopCommonModes];

    self.visible = NO;
    [panel setAlphaValue:0.0];
    [panel orderOut:nil];
    [self updateVisibility];
}

- (void)detach {
    [self.followLink invalidate]; // also releases its strong ref to self
    self.followLink = nil;
    [self.followTimer invalidate];
    self.followTimer = nil;
    [self.visibilityTimer invalidate];
    self.visibilityTimer = nil;
    // The arrange window is a plain (un-subclassed) Tauri window; just forget
    // it here; the Rust side closes it. Order it out so it can't linger.
    if (self.arrangeWindow) {
        [self.arrangeWindow orderOut:nil];
        self.arrangeWindow = nil;
    }
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

#pragma mark - Arrange overlay (full-window, click-through)

// A second Tauri window that covers REAPER's whole main window and lets clicks
// pass straight through (ignoresMouseEvents). It is NOT panel-ized (it never
// takes input), so there's no class swap to undo. Its frame is driven by
// `reposition` (fill REAPER's window) and its visibility by the same
// REAPER-frontmost logic as the HUD; the webview only paints into it.
- (void)attachArrange:(NSWindow *)window {
    window.ignoresMouseEvents = YES;
    window.level = NSFloatingWindowLevel;
    window.collectionBehavior |= (NSWindowCollectionBehaviorCanJoinAllSpaces |
                                  NSWindowCollectionBehaviorFullScreenAuxiliary |
                                  NSWindowCollectionBehaviorStationary);
    window.hidesOnDeactivate = NO;
    [window setAlphaValue:0.0];
    [window orderOut:nil];
    self.arrangeWindow = window;
    // If the HUD is already showing (REAPER frontmost), place and reveal it now;
    // otherwise the next visibility tick brings it in.
    if (self.visible) {
        [self reposition];
        [window setAlphaValue:1.0];
        [window orderFront:nil];
    }
}

- (void)detachArrange {
    if (self.arrangeWindow) {
        [self.arrangeWindow orderOut:nil];
        self.arrangeWindow = nil;
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

void reamesh_overlay_attach_arrange(void *ns_window) {
    if (!ns_window) return;
    NSWindow *window = (__bridge NSWindow *)ns_window;
    if (!gOverlay) gOverlay = [[ReaMeshOverlayController alloc] init];
    [gOverlay attachArrange:window];
}

void reamesh_overlay_detach_arrange(void) {
    if (gOverlay) [gOverlay detachArrange];
}
