// FastRustDesk: macOS virtual displays (CGVirtualDisplay, private CoreGraphics API).
//
// Design: FastRustDesk-DESIGN-mac-virtual-display-DETAIL.md, section 5.
// Everything private is looked up at run time (NSClassFromString/respondsToSelector),
// so a macOS that drops these classes makes fr_vd_probe() fail instead of
// crashing at load or at first call.
//
// The call sequence follows the MIT-licensed BetterDummy (c) 2021 Istvan T. and
// Crisp (c) 2026 Didrik Galteland, re-implemented here. Two behaviours differ
// from what Crisp documents, both measured on macOS 26.5 / Apple M4 (P0):
//   * the reported physical size decides whether macOS treats the display as a
//     TV: 110 PPI made 3840-wide displays pop the Screen Mirroring picker and
//     come up mirrored ~6 s later, a fixed 27" diagonal never did;
//   * creating off the main thread works. What does need the main thread is
//     NSEvent pumping: without it this process never sees the new display's
//     modes nor any reconfiguration callback.

#import <AppKit/AppKit.h>
#import <CoreGraphics/CoreGraphics.h>
#import <Foundation/Foundation.h>
#import <IOKit/ps/IOPSKeys.h>
#import <IOKit/ps/IOPowerSources.h>
#import <objc/message.h>
#import <objc/runtime.h>
#include <stdatomic.h>

#define FR_DIAG_INCHES 27.0

static NSMutableDictionary<NSNumber *, id> *g_live;  // displayID -> CGVirtualDisplay
static _Atomic double g_last_reconfig_ms = 0;
static _Atomic int g_cb_registered = 0;

static double fr_now_ms(void) { return [[NSDate date] timeIntervalSince1970] * 1000.0; }

static void fr_copy(char *dst, uint32_t cap, NSString *s) {
    if (!dst || cap == 0) return;
    const char *c = s ? [s UTF8String] : "";
    strlcpy(dst, c ? c : "", cap);
}

static void fr_reconfig_cb(CGDirectDisplayID d, CGDisplayChangeSummaryFlags f, void *u) {
    (void)d; (void)f; (void)u;
    atomic_store(&g_last_reconfig_ms, fr_now_ms());
}

static void fr_init_once(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
      g_live = [NSMutableDictionary dictionary];
      if (CGDisplayRegisterReconfigurationCallback(fr_reconfig_cb, NULL) == kCGErrorSuccess) {
          atomic_store(&g_cb_registered, 1);
      }
    });
}

// ---------------------------------------------------------------- probe

static BOOL fr_has_class(const char *name) { return NSClassFromString(@(name)) != nil; }

static BOOL fr_responds(const char *cls, const char *sel) {
    Class c = NSClassFromString(@(cls));
    return c && [c instancesRespondToSelector:sel_registerName(sel)];
}

int32_t fr_vd_probe(char *report, uint32_t cap) {
    fr_init_once();
    NSMutableString *out = [NSMutableString string];
    int32_t fails = 0;
    NSOperatingSystemVersion v = [[NSProcessInfo processInfo] operatingSystemVersion];
    BOOL os_ok = v.majorVersion >= 14;
    [out appendFormat:@"[%@] macOS >= 14 (%ld.%ld.%ld)\n", os_ok ? @"PASS" : @"FAIL",
                      (long)v.majorVersion, (long)v.minorVersion, (long)v.patchVersion];
    if (!os_ok) fails++;
    const char *classes[] = {"CGVirtualDisplayDescriptor", "CGVirtualDisplay",
                             "CGVirtualDisplayMode", "CGVirtualDisplaySettings"};
    for (int i = 0; i < 4; i++) {
        BOOL ok = fr_has_class(classes[i]);
        [out appendFormat:@"[%@] class %s\n", ok ? @"PASS" : @"FAIL", classes[i]];
        if (!ok) fails++;
    }
    const char *sels[][2] = {
        {"CGVirtualDisplay", "initWithDescriptor:"},
        {"CGVirtualDisplay", "applySettings:"},
        {"CGVirtualDisplay", "displayID"},
        {"CGVirtualDisplayDescriptor", "setName:"},
        {"CGVirtualDisplayDescriptor", "setVendorID:"},
        {"CGVirtualDisplayDescriptor", "setProductID:"},
        {"CGVirtualDisplayDescriptor", "setSerialNum:"},
        {"CGVirtualDisplayDescriptor", "setMaxPixelsWide:"},
        {"CGVirtualDisplayDescriptor", "setMaxPixelsHigh:"},
        {"CGVirtualDisplayDescriptor", "setSizeInMillimeters:"},
        {"CGVirtualDisplayMode", "initWithWidth:height:refreshRate:"},
        {"CGVirtualDisplaySettings", "setHiDPI:"},
        {"CGVirtualDisplaySettings", "setModes:"},
    };
    for (size_t i = 0; i < sizeof sels / sizeof sels[0]; i++) {
        BOOL ok = fr_responds(sels[i][0], sels[i][1]);
        [out appendFormat:@"[%@] -[%s %s]\n", ok ? @"PASS" : @"FAIL", sels[i][0], sels[i][1]];
        if (!ok) fails++;
    }
    fr_copy(report, cap, out);
    return fails;
}

// ---------------------------------------------------------------- create

static id fr_make(uint32_t vendor, uint32_t product, uint32_t serial, NSString *name,
                  uint32_t max_w, uint32_t max_h, const uint32_t *modes, uint32_t n,
                  int32_t hidpi, NSString **err) {
    Class D = NSClassFromString(@"CGVirtualDisplayDescriptor");
    Class V = NSClassFromString(@"CGVirtualDisplay");
    Class M = NSClassFromString(@"CGVirtualDisplayMode");
    Class S = NSClassFromString(@"CGVirtualDisplaySettings");
    if (!D || !V || !M || !S) {
        *err = @"CGVirtualDisplay classes missing";
        return nil;
    }
    id desc = [[D alloc] init];
    ((void (*)(id, SEL, id))objc_msgSend)(desc, @selector(setName:), name);
    ((void (*)(id, SEL, uint32_t))objc_msgSend)(desc, @selector(setVendorID:), vendor);
    ((void (*)(id, SEL, uint32_t))objc_msgSend)(desc, @selector(setProductID:), product);
    ((void (*)(id, SEL, uint32_t))objc_msgSend)(desc, @selector(setSerialNum:), serial);
    ((void (*)(id, SEL, uint32_t))objc_msgSend)(desc, @selector(setMaxPixelsWide:), max_w);
    ((void (*)(id, SEL, uint32_t))objc_msgSend)(desc, @selector(setMaxPixelsHigh:), max_h);
    double diag_mm = FR_DIAG_INCHES * 25.4;
    double r = sqrt((double)max_w * max_w + (double)max_h * max_h);
    CGSize mm = CGSizeMake(max_w * diag_mm / r, max_h * diag_mm / r);
    ((void (*)(id, SEL, CGSize))objc_msgSend)(desc, @selector(setSizeInMillimeters:), mm);
    // No queue, no colour primaries (Crisp: not needed, may interfere).

    id vd = ((id (*)(id, SEL, id))objc_msgSend)([V alloc], @selector(initWithDescriptor:), desc);
    if (!vd) {
        *err = @"initWithDescriptor returned nil";
        return nil;
    }
    NSMutableArray *arr = [NSMutableArray arrayWithCapacity:n];
    id (*mk)(id, SEL, NSUInteger, NSUInteger, double) =
        (id (*)(id, SEL, NSUInteger, NSUInteger, double))objc_msgSend;
    for (uint32_t i = 0; i < n; i++) {
        id m = mk([M alloc], @selector(initWithWidth:height:refreshRate:), modes[i * 3],
                  modes[i * 3 + 1], (double)modes[i * 3 + 2]);
        if (m) [arr addObject:m];
    }
    id settings = [[S alloc] init];
    // The property is an unsigned int in the class dump: pass 32 bits, not a BOOL.
    ((void (*)(id, SEL, uint32_t))objc_msgSend)(settings, @selector(setHiDPI:), hidpi ? 1u : 0u);
    ((void (*)(id, SEL, id))objc_msgSend)(settings, @selector(setModes:), arr);
    BOOL ok = ((BOOL (*)(id, SEL, id))objc_msgSend)(vd, @selector(applySettings:), settings);
    if (!ok) {
        *err = @"applySettings returned NO";
        return nil;
    }
    return vd;
}

static uint32_t fr_display_id(id vd) {
    return ((uint32_t (*)(id, SEL))objc_msgSend)(vd, @selector(displayID));
}

static BOOL fr_is_online(uint32_t did) {
    uint32_t n = 0;
    CGDirectDisplayID ids[32];
    if (CGGetOnlineDisplayList(32, ids, &n) != kCGErrorSuccess) return NO;
    for (uint32_t i = 0; i < n; i++)
        if (ids[i] == did) return YES;
    return NO;
}

uint32_t fr_vd_create(uint32_t vendor, uint32_t product, uint32_t serial, const char *name_utf8,
                      uint32_t max_px_w, uint32_t max_px_h, const uint32_t *modes, uint32_t n,
                      int32_t hidpi, char *err_buf, uint32_t err_cap) {
    fr_init_once();
    @autoreleasepool {
        NSString *name = name_utf8 ? @(name_utf8) : @"FastRustDesk Virtual Display";
        NSString *err = nil;
        id vd = fr_make(vendor, product, serial, name, max_px_w, max_px_h, modes, n, hidpi, &err);
        if (!vd && ![NSThread isMainThread]) {
            // Crisp reports init must run on the main thread; P0 found it does not on
            // macOS 26.5. Retry there once in case another version disagrees.
            __block id vd2 = nil;
            __block NSString *err2 = nil;
            dispatch_semaphore_t sem = dispatch_semaphore_create(0);
            dispatch_async(dispatch_get_main_queue(), ^{
              NSString *e = nil;
              vd2 = fr_make(vendor, product, serial, name, max_px_w, max_px_h, modes, n, hidpi, &e);
              err2 = e;
              dispatch_semaphore_signal(sem);
            });
            if (dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC)) != 0) {
                err = [NSString stringWithFormat:@"%@; main-thread retry timed out", err];
            } else {
                vd = vd2;
                if (!vd) err = [NSString stringWithFormat:@"%@; main-thread retry: %@", err, err2];
            }
        }
        if (!vd) {
            fr_copy(err_buf, err_cap, err);
            return 0;
        }
        uint32_t did = fr_display_id(vd);
        if (did == 0) {
            fr_copy(err_buf, err_cap, @"displayID is 0");
            return 0;
        }
        @synchronized(g_live) {
            g_live[@(did)] = vd;
        }
        // Wait (bounded) for it to come online; the picker case took ~6 s.
        double t0 = fr_now_ms();
        while (!fr_is_online(did) && fr_now_ms() - t0 < 8000) usleep(20 * 1000);
        if (!fr_is_online(did)) {
            @synchronized(g_live) {
                [g_live removeObjectForKey:@(did)];
            }
            fr_copy(err_buf, err_cap, @"display never came online");
            return 0;
        }
        return did;
    }
}

// ---------------------------------------------------------------- modes

// 1 = applied (or already current), 0 = no such mode, -1 = configuration failed,
// -2 = the display's modes are not visible to this process yet.
int32_t fr_vd_apply_mode(uint32_t did, uint32_t w, uint32_t h, uint32_t px_w, uint32_t px_h,
                         uint32_t hz) {
    @autoreleasepool {
        CGDisplayModeRef cur = CGDisplayCopyDisplayMode(did);
        if (!cur) return -2;
        BOOL same = CGDisplayModeGetWidth(cur) == w && CGDisplayModeGetHeight(cur) == h &&
                    CGDisplayModeGetPixelWidth(cur) == px_w &&
                    CGDisplayModeGetPixelHeight(cur) == px_h &&
                    fabs(CGDisplayModeGetRefreshRate(cur) - hz) < 0.5;
        CGDisplayModeRelease(cur);
        if (same) return 1;
        NSDictionary *opt = @{(__bridge NSString *)kCGDisplayShowDuplicateLowResolutionModes : @YES};
        CFArrayRef all = CGDisplayCopyAllDisplayModes(did, (__bridge CFDictionaryRef)opt);
        if (!all) return -2;
        int32_t rc = 0;
        for (CFIndex i = 0; i < CFArrayGetCount(all); i++) {
            CGDisplayModeRef m = (CGDisplayModeRef)CFArrayGetValueAtIndex(all, i);
            if (CGDisplayModeGetWidth(m) == w && CGDisplayModeGetHeight(m) == h &&
                CGDisplayModeGetPixelWidth(m) == px_w && CGDisplayModeGetPixelHeight(m) == px_h &&
                fabs(CGDisplayModeGetRefreshRate(m) - hz) < 0.5) {
                CGDisplayConfigRef cfg;
                if (CGBeginDisplayConfiguration(&cfg) != kCGErrorSuccess) {
                    rc = -1;
                    break;
                }
                CGConfigureDisplayWithDisplayMode(cfg, did, m, NULL);
                rc = CGCompleteDisplayConfiguration(cfg, kCGConfigureForSession) == kCGErrorSuccess
                         ? 1
                         : -1;
                break;
            }
        }
        CFRelease(all);
        return rc;
    }
}

// Current mode of any display: logical w/h, pixel w/h, refresh. 0 if unknown.
int32_t fr_vd_current_mode(uint32_t did, uint32_t *w, uint32_t *h, uint32_t *px_w, uint32_t *px_h,
                           uint32_t *hz) {
    CGDisplayModeRef m = CGDisplayCopyDisplayMode(did);
    if (!m) return 0;
    *w = (uint32_t)CGDisplayModeGetWidth(m);
    *h = (uint32_t)CGDisplayModeGetHeight(m);
    *px_w = (uint32_t)CGDisplayModeGetPixelWidth(m);
    *px_h = (uint32_t)CGDisplayModeGetPixelHeight(m);
    *hz = (uint32_t)lround(CGDisplayModeGetRefreshRate(m));
    CGDisplayModeRelease(m);
    return 1;
}

// ---------------------------------------------------------------- destroy / lookup

int32_t fr_vd_destroy(uint32_t did, uint32_t wait_offline_ms) {
    fr_init_once();
    BOOL had = NO;
    @synchronized(g_live) {
        had = g_live[@(did)] != nil;
        [g_live removeObjectForKey:@(did)];  // ARC releases it: WindowServer tears it down
    }
    if (!had) return 0;
    double t0 = fr_now_ms();
    while (fr_is_online(did) && fr_now_ms() - t0 < wait_offline_ms) usleep(20 * 1000);
    return fr_is_online(did) ? -1 : 1;
}

int32_t fr_vd_is_live(uint32_t did) {
    fr_init_once();
    @synchronized(g_live) {
        return g_live[@(did)] != nil;
    }
}

uint32_t fr_vd_find(uint32_t vendor, uint32_t product, uint32_t serial) {
    uint32_t n = 0;
    CGDirectDisplayID ids[32];
    if (CGGetOnlineDisplayList(32, ids, &n) != kCGErrorSuccess) return 0;
    for (uint32_t i = 0; i < n; i++) {
        if (CGDisplayVendorNumber(ids[i]) == vendor && CGDisplayModelNumber(ids[i]) == product &&
            CGDisplaySerialNumber(ids[i]) == serial)
            return ids[i];
    }
    return 0;
}

typedef struct {
    uint32_t id, vendor, model, serial;
    int32_t active, builtin, main, mirrored;
    uint32_t w, h, px_w, px_h;
} fr_disp;

// Online displays (up to cap). Returns the count written.
uint32_t fr_vd_list_online(fr_disp *out, uint32_t cap) {
    uint32_t n = 0;
    CGDirectDisplayID ids[32];
    if (CGGetOnlineDisplayList(32, ids, &n) != kCGErrorSuccess) return 0;
    uint32_t k = 0;
    for (uint32_t i = 0; i < n && k < cap; i++, k++) {
        CGDirectDisplayID d = ids[i];
        fr_disp *o = &out[k];
        memset(o, 0, sizeof *o);
        o->id = d;
        o->vendor = CGDisplayVendorNumber(d);
        o->model = CGDisplayModelNumber(d);
        o->serial = CGDisplaySerialNumber(d);
        o->active = CGDisplayIsActive(d);
        o->builtin = CGDisplayIsBuiltin(d);
        o->main = CGDisplayIsMain(d);
        o->mirrored = CGDisplayMirrorsDisplay(d) != kCGNullDirectDisplay;
        uint32_t hz = 0;
        if (!fr_vd_current_mode(d, &o->w, &o->h, &o->px_w, &o->px_h, &hz)) {
            CGRect b = CGDisplayBounds(d);
            o->w = o->px_w = (uint32_t)b.size.width;
            o->h = o->px_h = (uint32_t)b.size.height;
        }
    }
    return k;
}

// ---------------------------------------------------------------- misc

// Block until no display reconfiguration event for quiet_ms (max max_ms).
// Only meaningful while the main thread pumps NSEvents (see header).
void fr_vd_wait_quiet(uint32_t quiet_ms, uint32_t max_ms) {
    fr_init_once();
    double t0 = fr_now_ms();
    if (atomic_load(&g_last_reconfig_ms) < t0 - quiet_ms) {
        // Give a just-issued change a chance to produce its first event.
        usleep(100 * 1000);
    }
    while (fr_now_ms() - t0 < max_ms) {
        if (fr_now_ms() - atomic_load(&g_last_reconfig_ms) >= quiet_ms) return;
        usleep(25 * 1000);
    }
}

double fr_vd_last_reconfig_ms(void) { return atomic_load(&g_last_reconfig_ms); }

int32_t fr_vd_has_battery(void) {
    int32_t found = 0;
    CFTypeRef info = IOPSCopyPowerSourcesInfo();
    if (!info) return 0;
    CFArrayRef list = IOPSCopyPowerSourcesList(info);
    if (list) {
        for (CFIndex i = 0; i < CFArrayGetCount(list); i++) {
            CFDictionaryRef d = IOPSGetPowerSourceDescription(info, CFArrayGetValueAtIndex(list, i));
            if (!d) continue;
            CFStringRef type = CFDictionaryGetValue(d, CFSTR(kIOPSTypeKey));
            if (type && CFEqual(type, CFSTR(kIOPSInternalBatteryType))) found = 1;
        }
        CFRelease(list);
    }
    CFRelease(info);
    return found;
}

// Fallback main-thread loop for --server when the tray could not start: display
// notifications are delivered through the NSEvent stream, so a bare CFRunLoop is
// not enough (P0). Never returns.
void fr_run_nsapp_forever(void) {
    @autoreleasepool {
        [NSApplication sharedApplication];
        [NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];
        [NSApp run];
    }
}
