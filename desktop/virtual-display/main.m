// NokiVirtualDisplay - owns exactly one app-scoped virtual display.
//
// The private CoreGraphics classes are resolved at runtime.  If Apple
// removes one of them the helper exits cleanly and Noki keeps the selected
// virtual backend in FAILED state; it never falls through to a hidden Space
// trip for an individual input event.
#import <Foundation/Foundation.h>
#import <CoreGraphics/CoreGraphics.h>
#import <AppKit/AppKit.h>
#import <objc/message.h>
#import <IOKit/IOKitLib.h>
#include <signal.h>
#include <unistd.h>

// Runtime-only declarations: the classes themselves are still resolved by
// name, while Clang can now apply Objective-C initializer ownership rules.
@interface NSObject (NokiVirtualDisplayPrivate)
- (instancetype)initWithDescriptor:(id)descriptor;
- (instancetype)initWithWidth:(unsigned)width height:(unsigned)height refreshRate:(double)rate;
- (BOOL)applySettings:(id)settings;
@end

static volatile sig_atomic_t stopped = 0;
static void stop_handler(int signal_number) { (void)signal_number; stopped = 1; }

static void line(NSString *text) {
  fprintf(stdout, "%s\n", text.UTF8String);
  fflush(stdout);
}

// ---------------------------------------------------------------------
// Keep the diagonal placement.  Measured on macOS 26.6: WindowServer can
// re-arrange the virtual display later (READY at 1470,956, later 1470,0 -
// edge-adjacent to the built-in display).  The pointer could then enter it
// and Noki's cached bounds went stale.  After every completed
// reconfiguration the display is put back diagonally (rate-limited) and
// the new bounds are reported as `MOVED`.
// ---------------------------------------------------------------------
static CGDirectDisplayID own_display = 0;
static CGRect last_reported = {{0, 0}, {0, 0}};
static NSMutableArray<NSDate *> *recent_moves = nil;

static CGRect physical_union_without(CGDirectDisplayID display_id) {
  CGRect physical_union = CGRectNull;
  CGDirectDisplayID active[32]; uint32_t active_count = 0;
  if (CGGetActiveDisplayList(32, active, &active_count) == kCGErrorSuccess) {
    for (uint32_t i = 0; i < active_count; i++) {
      if (active[i] != display_id) {
        CGRect b = CGDisplayBounds(active[i]);
        physical_union = CGRectIsNull(physical_union) ? b : CGRectUnion(physical_union, b);
      }
    }
  }
  if (CGRectIsNull(physical_union)) physical_union = CGDisplayBounds(CGMainDisplayID());
  return physical_union;
}

static void keep_diagonal(void) {
  if (own_display == 0) return;
  CGRect physical = physical_union_without(own_display);
  CGRect bounds = CGDisplayBounds(own_display);
  int32_t want_x = (int32_t)CGRectGetMaxX(physical), want_y = (int32_t)CGRectGetMaxY(physical);
  if ((int32_t)bounds.origin.x != want_x || (int32_t)bounds.origin.y != want_y) {
    NSDate *now = [NSDate date];
    [recent_moves filterUsingPredicate:[NSPredicate predicateWithBlock:^BOOL(NSDate *d, id _) {
      return [now timeIntervalSinceDate:d] < 30;
    }]];
    if (recent_moves.count < 3) {
      [recent_moves addObject:now];
      CGDisplayConfigRef config = NULL;
      if (CGBeginDisplayConfiguration(&config) == kCGErrorSuccess) {
        if (CGConfigureDisplayOrigin(config, own_display, want_x, want_y) == kCGErrorSuccess) {
          CGCompleteDisplayConfiguration(config, kCGConfigureForAppOnly);
        } else {
          CGCancelDisplayConfiguration(config);
        }
      }
      [NSThread sleepForTimeInterval:0.25];
      bounds = CGDisplayBounds(own_display);
    }
  }
  if (!CGRectEqualToRect(bounds, last_reported) && bounds.size.width >= 1) {
    last_reported = bounds;
    line([NSString stringWithFormat:@"MOVED display=%u x=%.0f y=%.0f w=%.0f h=%.0f",
        own_display, bounds.origin.x, bounds.origin.y, bounds.size.width, bounds.size.height]);
  }
}

static void reconfigured(CGDirectDisplayID display, CGDisplayChangeSummaryFlags flags, void *user) {
  (void)display; (void)user;
  if (flags & kCGDisplayBeginConfigurationFlag) return;
  static BOOL scheduled = NO;
  if (scheduled) return;
  scheduled = YES;
  dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(0.4 * NSEC_PER_SEC)),
                 dispatch_get_main_queue(), ^{ scheduled = NO; keep_diagonal(); });
}

// Is there a real (non-Noki) display the user can see, and is the lid open
// (or an external monitor present)?  Measured 2026-09-25: with the lid closed
// the built-in panel leaves the active list and the Noki display becomes the
// ONLY display - macOS then treats the Mac as "external display attached",
// never sleeps, and a display released in that state lingers ownerless until
// the lid opens again.  So the helper must let go BEFORE that happens.
static BOOL clamshell_closed(void) {
  io_service_t root = IOServiceGetMatchingService(kIOMainPortDefault, IOServiceMatching("IOPMrootDomain"));
  if (!root) return NO;
  CFTypeRef v = IORegistryEntryCreateCFProperty(root, CFSTR("AppleClamshellState"), kCFAllocatorDefault, 0);
  IOObjectRelease(root);
  BOOL closed = v && CFGetTypeID(v) == CFBooleanGetTypeID() && CFBooleanGetValue((CFBooleanRef)v);
  if (v) CFRelease(v);
  return closed;
}

static BOOL physical_display_available(CGDirectDisplayID own) {
  // Test switch: simulates "lid closed / no physical display" (same path).
  if (access("/tmp/noki-vd-suspend", F_OK) == 0) return NO;
  CGDirectDisplayID active[32]; uint32_t n = 0;
  if (CGGetActiveDisplayList(32, active, &n) != kCGErrorSuccess) return NO;
  BOOL builtin = NO, external = NO;
  for (uint32_t i = 0; i < n; i++) {
    if (active[i] == own || CGDisplayVendorNumber(active[i]) == 0x4e4f) continue;
    if (CGDisplayIsBuiltin(active[i])) builtin = YES; else external = YES;
  }
  // Closed lid: only an external monitor counts (normal clamshell use).
  // An asleep built-in panel is not "available" either.
  BOOL builtin_awake = NO;
  for (uint32_t i = 0; i < n; i++) {
    if (CGDisplayIsBuiltin(active[i]) && !CGDisplayIsAsleep(active[i])) builtin_awake = YES;
  }
  return external || (builtin && builtin_awake && !clamshell_closed());
}

static volatile sig_atomic_t going_to_sleep = 0;

int main(void) {
  @autoreleasepool {
    // The display belongs to the Noki process, not to this helper after it
    // has been orphaned.  A SIGKILL/cold-test can close the parent's side
    // without delivering SIGTERM to us; launchd then reparents the helper
    // to PID 1 and the old CGVirtualDisplay used to survive indefinitely.
    // The next Noki launch consequently received create_failed and showed
    // only its obsolete black DOM placeholder.  Remember the real parent
    // and release the display as soon as that ownership disappears.
    const pid_t owner_pid = getppid();
    // A WindowServer-connected application (never visible: prohibited
    // activation policy).  Without it CoreGraphics never delivers display
    // reconfigurations to this process and CGDisplayBounds keeps returning
    // the bounds cached at start - measured: the display had been moved to
    // 1470,0 while this helper still read 1470,956.
    [NSApplication sharedApplication];
    [NSApp setActivationPolicy:NSApplicationActivationPolicyProhibited];
    signal(SIGTERM, stop_handler);
    signal(SIGINT, stop_handler);

    // Stable for 2 s, not just one sample: measured 2026-09-25 while the lid
    // was closing, one sample said "available" 4 s after a suspension; the
    // new display then became the ONLY (main) display, applySettings failed
    // and the released display lingered ownerless with the lid closed.
    for (int i = 0; i < 20; i++) {
      if (!physical_display_available(0)) {
        line(@"SUSPENDED reason=no_physical_display");
        return 6;
      }
      [NSThread sleepForTimeInterval:0.1];
    }
    [[[NSWorkspace sharedWorkspace] notificationCenter]
        addObserverForName:NSWorkspaceWillSleepNotification object:nil queue:nil
                usingBlock:^(NSNotification *note) { (void)note; going_to_sleep = 1; }];

    Class Descriptor = NSClassFromString(@"CGVirtualDisplayDescriptor");
    Class Display = NSClassFromString(@"CGVirtualDisplay");
    Class Settings = NSClassFromString(@"CGVirtualDisplaySettings");
    Class Mode = NSClassFromString(@"CGVirtualDisplayMode");
    if (!Descriptor || !Display || !Settings || !Mode) {
      line(@"FAILED reason=private_api_unavailable");
      return 2;
    }

    const unsigned width = 1470, height = 956;
    id descriptor = [[Descriptor alloc] init];
    @try {
      [descriptor setValue:@"Noki Workspace" forKey:@"name"];
      [descriptor setValue:@(width * 2) forKey:@"maxPixelsWide"];
      [descriptor setValue:@(height * 2) forKey:@"maxPixelsHigh"];
      [descriptor setValue:[NSValue valueWithSize:NSMakeSize(300, 195)]
                     forKey:@"sizeInMillimeters"];
      [descriptor setValue:@(0x4e4f) forKey:@"vendorID"];
      [descriptor setValue:@(0x4b49) forKey:@"productID"];
      [descriptor setValue:@(2) forKey:@"serialNum"];
      ((void (*)(id, SEL, dispatch_queue_t))objc_msgSend)(
          descriptor, NSSelectorFromString(@"setQueue:"), dispatch_get_main_queue());
    } @catch (NSException *exception) {
      line([NSString stringWithFormat:@"FAILED reason=descriptor_%@", exception.name]);
      return 2;
    }

#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wobjc-method-access"
    // Keep ARC's initializer-family ownership semantics. Calling these
    // initializers through a cast objc_msgSend made -O release the original
    // `alloc` receiver after the private initializer had replaced it
    // (objc_release at main+532 on macOS 26.6.2).
    id display = [[Display alloc] initWithDescriptor:descriptor];
    if (!display) {
      line(@"FAILED reason=create_failed");
      return 3;
    }
    id mode = [[Mode alloc] initWithWidth:width height:height refreshRate:60.0];
    id settings = [[Settings alloc] init];
    [settings setValue:@[mode] forKey:@"modes"];
    [settings setValue:@YES forKey:@"hiDPI"];
    BOOL applied = [display applySettings:settings];
#pragma clang diagnostic pop
    CGDirectDisplayID display_id =
        (CGDirectDisplayID)[[display valueForKey:@"displayID"] unsignedIntValue];
    if (!applied || display_id == 0 || display_id == CGMainDisplayID()) {
      line(@"FAILED reason=settings_failed");
      display = nil;
      return 4;
    }

    [NSThread sleepForTimeInterval:0.8];
    // Place against the lower-right corner of the complete *physical*
    // topology.  Using only CGMainDisplayID could overlap a second monitor.
    CGRect physical_union = CGRectNull;
    CGDirectDisplayID active[32]; uint32_t active_count = 0;
    if (CGGetActiveDisplayList(32, active, &active_count) == kCGErrorSuccess) {
      for (uint32_t i = 0; i < active_count; i++) {
        if (active[i] != display_id) {
          CGRect b = CGDisplayBounds(active[i]);
          physical_union = CGRectIsNull(physical_union) ? b : CGRectUnion(physical_union, b);
        }
      }
    }
    if (CGRectIsNull(physical_union)) physical_union = CGDisplayBounds(CGMainDisplayID());
    CGDisplayConfigRef config = NULL;
    CGError configuration_error = CGBeginDisplayConfiguration(&config);
    if (configuration_error == kCGErrorSuccess) {
      // WindowServer snaps detached displays into contact.  Diagonal
      // placement reduces the shared boundary to one point and leaves all
      // ordinary physical edges unchanged.
      configuration_error = CGConfigureDisplayOrigin(
          config, display_id, (int32_t)CGRectGetMaxX(physical_union),
          (int32_t)CGRectGetMaxY(physical_union));
      if (configuration_error == kCGErrorSuccess) {
        configuration_error = CGCompleteDisplayConfiguration(config, kCGConfigureForAppOnly);
      } else {
        CGCancelDisplayConfiguration(config);
      }
    }
    [NSThread sleepForTimeInterval:0.35];
    CGRect bounds = CGDisplayBounds(display_id);
    if (configuration_error != kCGErrorSuccess || bounds.size.width < 1 || bounds.size.height < 1) {
      line([NSString stringWithFormat:@"FAILED reason=placement_%d", configuration_error]);
      display = nil;
      return 5;
    }

    line([NSString stringWithFormat:
        @"READY display=%u main=%u x=%.0f y=%.0f w=%.0f h=%.0f",
        display_id, CGMainDisplayID(), bounds.origin.x, bounds.origin.y,
        bounds.size.width, bounds.size.height]);
    own_display = display_id;
    last_reported = bounds;
    recent_moves = [NSMutableArray array];
    CGDisplayRegisterReconfigurationCallback(reconfigured, NULL);

    NSString *suspend_reason = nil;
    while (!stopped && getppid() == owner_pid && owner_pid > 1) {
      @autoreleasepool {
        if (going_to_sleep) { suspend_reason = @"system_sleep"; break; }
        if (!physical_display_available(display_id)) { suspend_reason = @"no_physical_display"; break; }
        NSEvent *event = [NSApp nextEventMatchingMask:NSEventMaskAny
                                            untilDate:[NSDate dateWithTimeIntervalSinceNow:0.1]
                                               inMode:NSDefaultRunLoopMode
                                              dequeue:YES];
        if (event) [NSApp sendEvent:event];
        // Also checked on every wake-up, independent of callback delivery;
        // one CGDisplayBounds call, acting only on drift.
        keep_diagonal();
      }
    }
    if (suspend_reason) {
      display = nil;
      [NSThread sleepForTimeInterval:0.3];
      line([NSString stringWithFormat:@"SUSPENDED reason=%@ display=%u", suspend_reason, display_id]);
      return 6;
    }
    line([NSString stringWithFormat:@"TEARDOWN reason=%@ owner=%d parent=%d",
        stopped ? @"signal" : @"owner_exit", owner_pid, getppid()]);
    display = nil; // Process death also releases and removes the display.
  }
  return 0;
}
