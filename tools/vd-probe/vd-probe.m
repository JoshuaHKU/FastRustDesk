// vd-probe: P0 feasibility probe for FastRustDesk macOS virtual displays.
//
// Exercises the private CGVirtualDisplay API exactly the way the product code
// will (runtime lookup only, no @interface declarations), and reports what the
// running macOS actually does. See FastRustDesk-DESIGN-mac-virtual-display-DETAIL.md
// section 11.1.
//
// Build: clang -fobjc-arc -framework Foundation -framework CoreGraphics \
//          -framework AppKit tools/vd-probe/vd-probe.m -o vd-probe
// Usage: vd-probe probe
//        vd-probe create <w> <h> <hidpi|lodpi> <seconds>
//        vd-probe offmain <w> <h>
//        vd-probe crash <w> <h>
//        vd-probe list
//
// Call sequence and the "no queue / no colour primaries" rule follow the
// MIT-licensed BetterDummy (c) 2021 Istvan T. and Crisp (c) 2026 Didrik
// Galteland, re-implemented here.

#import <AppKit/AppKit.h>
#import <CoreGraphics/CoreGraphics.h>
#import <Foundation/Foundation.h>
#import <objc/message.h>
#import <objc/runtime.h>
#include <dlfcn.h>

static const uint32_t kVendor = 0x4652;

static double now_ms(void) { return [[NSDate date] timeIntervalSince1970] * 1000.0; }

static int g_fail = 0;
static void check(const char *name, BOOL ok) {
    printf("[%s] %s\n", ok ? "PASS" : "FAIL", name);
    if (!ok) g_fail++;
}

static void probe(void) {
    const char *classes[] = {"CGVirtualDisplayDescriptor", "CGVirtualDisplay",
                             "CGVirtualDisplayMode", "CGVirtualDisplaySettings"};
    for (int i = 0; i < 4; i++) {
        char buf[128];
        snprintf(buf, sizeof buf, "class %s", classes[i]);
        check(buf, NSClassFromString(@(classes[i])) != nil);
    }
    struct { const char *cls; const char *sel; BOOL inst; } sels[] = {
        {"CGVirtualDisplay", "initWithDescriptor:", YES},
        {"CGVirtualDisplay", "applySettings:", YES},
        {"CGVirtualDisplay", "displayID", YES},
        {"CGVirtualDisplayDescriptor", "setName:", YES},
        {"CGVirtualDisplayDescriptor", "setVendorID:", YES},
        {"CGVirtualDisplayDescriptor", "setProductID:", YES},
        {"CGVirtualDisplayDescriptor", "setSerialNum:", YES},
        {"CGVirtualDisplayDescriptor", "setMaxPixelsWide:", YES},
        {"CGVirtualDisplayDescriptor", "setMaxPixelsHigh:", YES},
        {"CGVirtualDisplayDescriptor", "setSizeInMillimeters:", YES},
        {"CGVirtualDisplayMode", "initWithWidth:height:refreshRate:", YES},
        {"CGVirtualDisplaySettings", "setHiDPI:", YES},
        {"CGVirtualDisplaySettings", "setModes:", YES},
    };
    for (size_t i = 0; i < sizeof sels / sizeof sels[0]; i++) {
        Class c = NSClassFromString(@(sels[i].cls));
        BOOL ok = c && [c instancesRespondToSelector:sel_registerName(sels[i].sel)];
        char buf[160];
        snprintf(buf, sizeof buf, "-[%s %s]", sels[i].cls, sels[i].sel);
        check(buf, ok);
    }
}

static id make_display(uint32_t product, uint32_t serial, uint32_t pxw, uint32_t pxh,
                       BOOL hidpi, NSString *name) {
    Class D = NSClassFromString(@"CGVirtualDisplayDescriptor");
    Class V = NSClassFromString(@"CGVirtualDisplay");
    Class M = NSClassFromString(@"CGVirtualDisplayMode");
    Class S = NSClassFromString(@"CGVirtualDisplaySettings");
    if (!D || !V || !M || !S) return nil;

    id desc = [[D alloc] init];
    ((void (*)(id, SEL, id))objc_msgSend)(desc, @selector(setName:), name);
    ((void (*)(id, SEL, uint32_t))objc_msgSend)(desc, @selector(setVendorID:), kVendor);
    ((void (*)(id, SEL, uint32_t))objc_msgSend)(desc, @selector(setProductID:), product);
    ((void (*)(id, SEL, uint32_t))objc_msgSend)(desc, @selector(setSerialNum:), serial);
    ((void (*)(id, SEL, uint32_t))objc_msgSend)(desc, @selector(setMaxPixelsWide:), pxw);
    ((void (*)(id, SEL, uint32_t))objc_msgSend)(desc, @selector(setMaxPixelsHigh:), pxh);
    CGSize mm = CGSizeMake(pxw / 110.0 * 25.4, pxh / 110.0 * 25.4);
    if (getenv("VD_DIAG")) {  // report a fixed diagonal in inches, keeping the aspect
        double d = atof(getenv("VD_DIAG")) * 25.4, r = sqrt((double)pxw * pxw + (double)pxh * pxh);
        mm = CGSizeMake(pxw * d / r, pxh * d / r);
    }
    printf("sizeInMillimeters=%.0fx%.0f\n", mm.width, mm.height);
    ((void (*)(id, SEL, CGSize))objc_msgSend)(desc, @selector(setSizeInMillimeters:), mm);

    if (getenv("VD_QUEUE")) {
        dispatch_queue_t q = strcmp(getenv("VD_QUEUE"), "main") == 0
                                 ? dispatch_get_main_queue()
                                 : dispatch_get_global_queue(QOS_CLASS_USER_INTERACTIVE, 0);
        ((void (*)(id, SEL, id))objc_msgSend)(desc, @selector(setQueue:), q);
        ((void (*)(id, SEL, id))objc_msgSend)(desc, @selector(setTerminationHandler:), ^(id a, id b) {
          printf("terminationHandler fired: %s %s\n", [[a description] UTF8String],
                 [[b description] UTF8String]);
        });
        printf("(queue=%s + terminationHandler set)\n", getenv("VD_QUEUE"));
    }
    id vd = ((id (*)(id, SEL, id))objc_msgSend)([V alloc], @selector(initWithDescriptor:), desc);
    if (!vd) return nil;

    NSMutableArray *modes = [NSMutableArray array];
    id (*mk)(id, SEL, NSUInteger, NSUInteger, double) =
        (id (*)(id, SEL, NSUInteger, NSUInteger, double))objc_msgSend;
    [modes addObject:mk([M alloc], @selector(initWithWidth:height:refreshRate:), pxw, pxh, 60.0)];
    if (hidpi) {
        [modes addObject:mk([M alloc], @selector(initWithWidth:height:refreshRate:), pxw / 2,
                            pxh / 2, 60.0)];
    }
    id settings = [[S alloc] init];
    ((void (*)(id, SEL, uint32_t))objc_msgSend)(settings, @selector(setHiDPI:), hidpi ? 1u : 0u);
    ((void (*)(id, SEL, id))objc_msgSend)(settings, @selector(setModes:), modes);
    BOOL ok = ((BOOL (*)(id, SEL, id))objc_msgSend)(vd, @selector(applySettings:), settings);
    if (!ok) {
        printf("applySettings: returned NO\n");
        return nil;
    }
    return vd;
}

static uint32_t display_id(id vd) {
    return ((uint32_t (*)(id, SEL))objc_msgSend)(vd, @selector(displayID));
}

static void list_displays(const char *tag) {
    uint32_t n = 0;
    CGDirectDisplayID ids[16];
    CGGetOnlineDisplayList(16, ids, &n);
    printf("-- online displays (%s): %u\n", tag, n);
    for (uint32_t i = 0; i < n; i++) {
        CGDisplayModeRef m = CGDisplayCopyDisplayMode(ids[i]);
        printf("   id=%u vendor=0x%x model=0x%x serial=0x%x active=%d builtin=%d main=%d "
               "mode=%zux%zu px=%zux%zu %.0fHz\n",
               ids[i], CGDisplayVendorNumber(ids[i]), CGDisplayModelNumber(ids[i]),
               CGDisplaySerialNumber(ids[i]), CGDisplayIsActive(ids[i]),
               CGDisplayIsBuiltin(ids[i]), CGDisplayIsMain(ids[i]),
               m ? CGDisplayModeGetWidth(m) : 0, m ? CGDisplayModeGetHeight(m) : 0,
               m ? CGDisplayModeGetPixelWidth(m) : 0, m ? CGDisplayModeGetPixelHeight(m) : 0,
               m ? CGDisplayModeGetRefreshRate(m) : 0);
        if (m) CGDisplayModeRelease(m);
    }
}

static BOOL is_online(uint32_t did) {
    uint32_t n = 0;
    CGDirectDisplayID ids[16];
    CGGetOnlineDisplayList(16, ids, &n);
    for (uint32_t i = 0; i < n; i++)
        if (ids[i] == did) return YES;
    return NO;
}

static void spin(double seconds) {
    if (getenv("VD_PUMP")) {  // drain NSEvents like a real app's event loop does
        NSDate *until = [NSDate dateWithTimeIntervalSinceNow:seconds];
        while ([until timeIntervalSinceNow] > 0) {
            NSEvent *e = [NSApp nextEventMatchingMask:NSEventMaskAny untilDate:until
                                               inMode:NSDefaultRunLoopMode dequeue:YES];
            if (e) [NSApp sendEvent:e];
        }
        return;
    }
    [[NSRunLoop mainRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:seconds]];
}

static BOOL apply_mode(uint32_t did, size_t w, size_t h, size_t pxw, size_t pxh) {
    NSDictionary *opt = @{(__bridge NSString *)kCGDisplayShowDuplicateLowResolutionModes : @YES};
    CFArrayRef modes = CGDisplayCopyAllDisplayModes(did, (__bridge CFDictionaryRef)opt);
    if (!modes) return NO;
    BOOL ok = NO;
    printf("   available modes:\n");
    for (CFIndex i = 0; i < CFArrayGetCount(modes); i++) {
        CGDisplayModeRef m = (CGDisplayModeRef)CFArrayGetValueAtIndex(modes, i);
        printf("     %zux%zu px=%zux%zu %.0fHz\n", CGDisplayModeGetWidth(m),
               CGDisplayModeGetHeight(m), CGDisplayModeGetPixelWidth(m),
               CGDisplayModeGetPixelHeight(m), CGDisplayModeGetRefreshRate(m));
    }
    for (CFIndex i = 0; i < CFArrayGetCount(modes); i++) {
        CGDisplayModeRef m = (CGDisplayModeRef)CFArrayGetValueAtIndex(modes, i);
        if (CGDisplayModeGetWidth(m) == w && CGDisplayModeGetHeight(m) == h &&
            CGDisplayModeGetPixelWidth(m) == pxw && CGDisplayModeGetPixelHeight(m) == pxh) {
            CGDisplayConfigRef cfg;
            CGBeginDisplayConfiguration(&cfg);
            CGConfigureDisplayWithDisplayMode(cfg, did, m, NULL);
            ok = CGCompleteDisplayConfiguration(cfg, kCGConfigureForSession) == kCGErrorSuccess;
            break;
        }
    }
    CFRelease(modes);
    return ok;
}

static int cmd_create(int w, int h, BOOL hidpi, double secs, BOOL crash) {
    list_displays("before");
    uint32_t pxw = hidpi ? w * 2 : w, pxh = hidpi ? h * 2 : h;
    double t0 = now_ms();
    uint32_t serial = getenv("VD_SERIAL") ? (uint32_t)strtoul(getenv("VD_SERIAL"), NULL, 0) : arc4random();
    printf("serial=0x%x\n", serial);
    id vd = make_display(0x5601, serial, pxw, pxh, hidpi, @"FastRustDesk VD Probe");
    double t1 = now_ms();
    if (!vd) {
        printf("CREATE FAILED after %.0f ms\n", t1 - t0);
        return 2;
    }
    uint32_t did = display_id(vd);
    printf("CREATED id=%u in %.0f ms (px %ux%u, hidpi=%d)\n", did, t1 - t0, pxw, pxh, hidpi);
    int polls = getenv("VD_POLL") ? atoi(getenv("VD_POLL")) : 50;
    for (int i = 0; i < polls; i++) {
        uint32_t na = 0;
        CGDirectDisplayID act[16];
        CGGetActiveDisplayList(16, act, &na);
        BOOL in_active = NO;
        for (uint32_t k = 0; k < na; k++) if (act[k] == did) in_active = YES;
        if (i % 5 == 0 || is_online(did))
            printf("  t+%4.0fms online=%d active_list=%d CGDisplayIsOnline=%d isActive=%d "
                   "mirrorsDisplay=%u inMirrorSet=%d asleep=%d\n",
                   now_ms() - t1, is_online(did), in_active, CGDisplayIsOnline(did),
                   CGDisplayIsActive(did), CGDisplayMirrorsDisplay(did),
                   CGDisplayIsInMirrorSet(did), CGDisplayIsAsleep(did));
        if (is_online(did)) break;
        spin(0.1);
    }
    list_displays("after create");
    for (int i = 0; i < 60; i++) {
        CGDisplayModeRef m = CGDisplayCopyDisplayMode(did);
        if (m) {
            printf("  in-process mode visible after %.0f ms: %zux%zu px %zux%zu\n", now_ms() - t1,
                   CGDisplayModeGetWidth(m), CGDisplayModeGetHeight(m),
                   CGDisplayModeGetPixelWidth(m), CGDisplayModeGetPixelHeight(m));
            CGDisplayModeRelease(m);
            break;
        }
        spin(0.1);
    }
    BOOL ok = apply_mode(did, w, h, pxw, pxh);
    printf("apply target mode %dx%d px %ux%u: %s\n", w, h, pxw, pxh, ok ? "OK" : "FAILED");
    spin(0.5);
    list_displays("after apply");
    // Obsoleted in the macOS 15 SDK but still exported; resolve at runtime.
    CGImageRef (*create_image)(CGDirectDisplayID) = dlsym(RTLD_DEFAULT, "CGDisplayCreateImage");
    CGImageRef img = create_image ? create_image(did) : NULL;
    printf("CGDisplayCreateImage: %s (%zux%zu)\n", img ? "OK" : "NULL (no Screen Recording grant?)",
           img ? CGImageGetWidth(img) : 0, img ? CGImageGetHeight(img) : 0);
    if (img) CGImageRelease(img);
    if (crash) {
        printf("aborting with the display alive...\n");
        fflush(stdout);
        abort();
    }
    printf("holding %.0f s...\n", secs);
    fflush(stdout);
    spin(secs);
    double t2 = now_ms();
    vd = nil;
    while (is_online(did) && now_ms() - t2 < 3000) spin(0.05);
    printf("DESTROYED: offline after %.0f ms (still online: %d)\n", now_ms() - t2, is_online(did));
    list_displays("after destroy");
    return 0;
}

static void reconfig_cb(CGDirectDisplayID d, CGDisplayChangeSummaryFlags f, void *u) {
    printf("  [reconfig] id=%u flags=0x%x t=%.0f\n", d, f, now_ms());
}

int main(int argc, const char **argv) {
    @autoreleasepool {
        [NSApplication sharedApplication];
        if (getenv("VD_APP")) {
            [NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];
            [NSApp finishLaunching];
            printf("(finishLaunching done)\n");
        }
        if (argc < 2) {
            fprintf(stderr, "usage: see header\n");
            return 64;
        }
        if (getenv("VD_CB")) {
            CGDisplayRegisterReconfigurationCallback(reconfig_cb, NULL);
            printf("(reconfiguration callback registered)\n");
        }
        NSString *cmd = @(argv[1]);
        if ([cmd isEqual:@"probe"]) {
            probe();
            printf("failures: %d\n", g_fail);
            return g_fail;
        }
        if ([cmd isEqual:@"list"]) {
            list_displays("now");
            return 0;
        }
        if ([cmd isEqual:@"create"] && argc >= 6) {
            return cmd_create(atoi(argv[2]), atoi(argv[3]), strcmp(argv[4], "hidpi") == 0,
                              atof(argv[5]), NO);
        }
        if ([cmd isEqual:@"crash"] && argc >= 4) {
            return cmd_create(atoi(argv[2]), atoi(argv[3]), NO, 0, YES);
        }
        if ([cmd isEqual:@"offmain"] && argc >= 4) {
            int w = atoi(argv[2]), h = atoi(argv[3]);
            __block id vd = nil;
            dispatch_semaphore_t sem = dispatch_semaphore_create(0);
            dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
              vd = make_display(0x5602, 0x22, w, h, NO, @"FastRustDesk VD OffMain");
              dispatch_semaphore_signal(sem);
            });
            dispatch_semaphore_wait(sem, DISPATCH_TIME_FOREVER);
            printf("off-main create: %s\n", vd ? "SUCCEEDED" : "returned nil");
            if (vd) printf("  id=%u\n", display_id(vd));
            spin(1.0);
            if (vd) printf("  online after 1s: %d\n", is_online(display_id(vd)));
            vd = nil;
            spin(0.5);
            return 0;
        }
        fprintf(stderr, "bad args\n");
        return 64;
    }
}
