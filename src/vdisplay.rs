//! FastRustDesk: macOS virtual displays.
//!
//! Documentation: `FastRustDesk.md`, section 8 (architecture in 8.4).
//! The native half is `src/platform/macos_vdisplay.m`.
//!
//! Model: every entry point (login, toolbar toggle, tray, CLI, settings page)
//! only edits the *demand* for a slot; `reconcile()` is the one place that
//! creates or destroys displays to match it. Three demand sources stack:
//! persistent (from config), session (toolbar, reclaimed after the last session
//! ends) and local (tray/CLI, kept until `--server` exits).

use hbb_common::{config::Config, log, message_proto::ClientDisplayInfo};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    ffi::{CStr, CString},
    os::raw::c_char,
    sync::Mutex,
    time::{Duration, Instant},
};

/// Vendor id stamped on every display we create ("FR"). It must be non-zero
/// (otherwise `initWithDescriptor:` returns nil) and is how our displays are told
/// apart from real panels: `CGDisplayVendorNumber` reports it immediately.
pub const VENDOR: u32 = 0x4652;
/// Product id = PRODUCT_BASE | slot. Together with the persisted serial this is
/// the display's identity, under which macOS remembers arrangement and mode.
const PRODUCT_BASE: u32 = 0x5600;
/// Matches the controlling side's menu (`kMaxVirtualDisplayCount`).
pub const MAX_SLOTS: u8 = 4;
/// Largest pixel size we declare: 6K-class. Beyond it a 2x (HiDPI) mode falls back
/// to 1x, which keeps capture and VideoToolbox encoding within reach.
const PIXEL_CAP: (u32, u32) = (6144, 3840);
/// Smallest looks-like size; a phone controller is scaled up to at least this.
const LOGICAL_MIN: (u32, u32) = (1024, 640);
/// Used when the controller does not report its display (older clients).
const DEFAULT_LOGICAL: (u32, u32) = (1920, 1080);

pub const IDD_IMPL: &str = "mac_cgvd";

pub const OPT_SLOTS: &str = "mac-virtual-displays";
pub const OPT_HEADLESS: &str = "mac-vd-headless";
pub const OPT_ALLOW_REMOTE: &str = "mac-vd-allow-remote";
pub const OPT_KEEP: &str = "mac-vd-keep";
pub const OPT_HIDPI: &str = "mac-vd-hidpi";

// ------------------------------------------------------------------ FFI

/// Mirror of `fr_disp` in macos_vdisplay.m: one online display as CoreGraphics
/// reports it. `w`/`h` are the looks-like size, `px_w`/`px_h` the pixel size.
#[repr(C)]
#[derive(Default, Clone, Copy, Debug)]
struct FrDisp {
    id: u32,
    vendor: u32,
    model: u32,
    serial: u32,
    active: i32,
    builtin: i32,
    main: i32,
    mirrored: i32,
    w: u32,
    h: u32,
    px_w: u32,
    px_h: u32,
}

extern "C" {
    fn fr_vd_probe(report: *mut c_char, cap: u32) -> i32;
    fn fr_vd_create(
        vendor: u32,
        product: u32,
        serial: u32,
        name: *const c_char,
        max_px_w: u32,
        max_px_h: u32,
        modes: *const u32,
        n: u32,
        hidpi: i32,
        err: *mut c_char,
        err_cap: u32,
    ) -> u32;
    fn fr_vd_apply_mode(id: u32, w: u32, h: u32, px_w: u32, px_h: u32, hz: u32) -> i32;
    fn fr_vd_current_mode(
        id: u32,
        w: *mut u32,
        h: *mut u32,
        px_w: *mut u32,
        px_h: *mut u32,
        hz: *mut u32,
    ) -> i32;
    fn fr_vd_destroy(id: u32, wait_offline_ms: u32) -> i32;
    fn fr_vd_find(vendor: u32, product: u32, serial: u32) -> u32;
    fn fr_vd_list_online(out: *mut FrDisp, cap: u32) -> u32;
    fn fr_vd_wait_quiet(quiet_ms: u32, max_ms: u32);
    fn fr_vd_has_battery() -> i32;
    fn fr_run_nsapp_forever();
}

/// Main-thread fallback for `--server` when the tray could not start. Never returns.
pub fn run_nsapp_forever() -> ! {
    unsafe { fr_run_nsapp_forever() };
    // [NSApp run] only returns on terminate:.
    std::process::exit(0)
}

fn list_online() -> Vec<FrDisp> {
    let mut buf = [FrDisp::default(); 32];
    let n = unsafe { fr_vd_list_online(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    buf[..n.min(buf.len())].to_vec()
}

fn current_mode(id: u32) -> Option<(u32, u32, u32, u32, u32)> {
    let (mut w, mut h, mut pw, mut ph, mut hz) = (0, 0, 0, 0, 0);
    if unsafe { fr_vd_current_mode(id, &mut w, &mut h, &mut pw, &mut ph, &mut hz) } == 1 {
        Some((w, h, pw, ph, hz))
    } else {
        None
    }
}

// ------------------------------------------------------------------ config

fn default_true() -> bool {
    true
}
fn default_refresh() -> u32 {
    60
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SlotConfig {
    pub slot: u8,
    #[serde(default)]
    pub name: String,
    pub w: u32,
    pub h: u32,
    #[serde(default = "default_true")]
    pub hidpi: bool,
    #[serde(default = "default_refresh")]
    pub refresh: u32,
    #[serde(default)]
    pub persistent: bool,
    #[serde(default)]
    pub follow: bool,
    #[serde(default)]
    pub serial: u32,
}

impl SlotConfig {
    fn new(slot: u8, w: u32, h: u32, hidpi: bool) -> Self {
        Self {
            slot,
            name: default_name(slot),
            w,
            h,
            hidpi,
            refresh: 60,
            persistent: false,
            follow: false,
            serial: new_serial(),
        }
    }

    /// Pixel size of the target mode.
    fn px(&self) -> (u32, u32) {
        if self.hidpi {
            (self.w * 2, self.h * 2)
        } else {
            (self.w, self.h)
        }
    }

    fn label(&self) -> String {
        format!(
            "{}×{}{}",
            self.w,
            self.h,
            if self.hidpi { " HiDPI" } else { "" }
        )
    }
}

fn default_name(slot: u8) -> String {
    format!("FastRustDesk Virtual Display {}", slot)
}

fn new_serial() -> u32 {
    let v: u32 = hbb_common::rand::random();
    v.max(1)
}

fn load_configs() -> Vec<SlotConfig> {
    let raw = Config::get_option(OPT_SLOTS);
    if raw.is_empty() {
        return vec![];
    }
    match serde_json::from_str::<Vec<SlotConfig>>(&raw) {
        Ok(mut v) => {
            v.retain(|c| c.slot >= 1 && c.slot <= MAX_SLOTS && c.w > 0 && c.h > 0);
            v.sort_by_key(|c| c.slot);
            v.dedup_by_key(|c| c.slot);
            v
        }
        Err(e) => {
            log::error!("vdisplay: bad {} option ({}), ignoring", OPT_SLOTS, e);
            vec![]
        }
    }
}

fn save_configs(v: &[SlotConfig]) {
    let s = serde_json::to_string(v).unwrap_or_default();
    Config::set_option(OPT_SLOTS.to_owned(), s);
}

fn opt_bool(key: &str, default: bool) -> bool {
    match Config::get_option(key).as_str() {
        "Y" => true,
        "N" => false,
        _ => default,
    }
}

fn keep_secs() -> i64 {
    Config::get_option(OPT_KEEP).parse::<i64>().unwrap_or(30)
}

// ------------------------------------------------------------------ pure helpers

/// Target looks-like size for a session from the controller's display (decision 3).
pub fn resolve_target(cdi: Option<&ClientDisplayInfo>, allow_hidpi: bool) -> (u32, u32, bool) {
    let (mut w, mut h) = match cdi {
        Some(c) if c.width > 0 && c.height > 0 => (c.width as f64, c.height as f64),
        _ => (DEFAULT_LOGICAL.0 as f64, DEFAULT_LOGICAL.1 as f64),
    };
    if h > w {
        std::mem::swap(&mut w, &mut h); // a phone held upright: use landscape
    }
    let up = (LOGICAL_MIN.0 as f64 / w).max(LOGICAL_MIN.1 as f64 / h);
    if up > 1.0 {
        w *= up;
        h *= up;
    }
    let mut hidpi = allow_hidpi && w * 2.0 <= PIXEL_CAP.0 as f64 && h * 2.0 <= PIXEL_CAP.1 as f64;
    if !hidpi {
        let down = (PIXEL_CAP.0 as f64 / w).min(PIXEL_CAP.1 as f64 / h);
        if down < 1.0 {
            w *= down;
            h *= down;
        }
    }
    let (w, h) = ((w.round() as u32) & !1, (h.round() as u32) & !1);
    if w == 0 || h == 0 {
        hidpi = allow_hidpi;
        return (DEFAULT_LOGICAL.0, DEFAULT_LOGICAL.1, hidpi);
    }
    (w, h, hidpi)
}

/// Modes declared for a slot: (px_w, px_h, hz) triples and the pixel maximum.
fn modes_for(c: &SlotConfig) -> (Vec<u32>, u32, u32) {
    let mut rates = vec![60u32];
    if c.refresh != 60 && c.refresh > 0 {
        rates.insert(0, c.refresh);
    }
    let mut sizes = vec![c.px()];
    if c.hidpi {
        sizes.push((c.w, c.h)); // offers looks-like w/2 x h/2 and the 1x w x h
    }
    let mut v = vec![];
    for (pw, ph) in &sizes {
        for r in &rates {
            v.extend_from_slice(&[*pw, *ph, *r]);
        }
    }
    let (mw, mh) = c.px();
    (v, mw, mh)
}

/// A display that is a real panel, as opposed to the headless placeholder macOS
/// spawns, a mirror, or one of ours (FastRustDesk.md 8.5).
fn is_real_panel(d: &FrDisp) -> bool {
    if d.vendor == VENDOR || d.vendor == 0 || d.vendor > 0xFFFF {
        return false;
    }
    // Not `active`: a panel whose screen is asleep reads inactive, and an idle
    // MacBook with its lid open is not headless. A closed lid drops the built-in
    // from the online list altogether.
    if d.mirrored != 0 {
        return false;
    }
    d.builtin != 0 || d.px_w > 1280
}

fn headless_of(list: &[FrDisp]) -> bool {
    !list.iter().any(is_real_panel)
}

pub fn is_headless() -> bool {
    headless_of(&list_online())
}

pub fn is_vd(display_id: u32) -> bool {
    list_online()
        .iter()
        .any(|d| d.id == display_id && d.vendor == VENDOR)
}

// ------------------------------------------------------------------ manager

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Source {
    Persistent,
    Local,
    Session(i32),
}

#[derive(Clone, Debug)]
struct Live {
    display_id: u32,
    cfg: SlotConfig,
}

#[derive(Default)]
struct Manager {
    demand: HashMap<u8, BTreeSet<Source>>,
    live: HashMap<u8, Live>,
    sessions: HashSet<i32>,
    /// Per-session target size (from the controller's display), for toolbar adds.
    targets: HashMap<i32, (u32, u32, bool)>,
    reclaim_at: Option<Instant>,
    probe: Option<(bool, String)>,
    inited: bool,
}

lazy_static::lazy_static! {
    static ref MANAGER: Mutex<Manager> = Mutex::new(Manager::default());
    /// Last status JSON, readable without the manager lock (tray runs on the main
    /// thread and must never wait behind a create).
    static ref STATUS_CACHE: Mutex<String> = Mutex::new(String::new());
}

fn probe_result(m: &mut Manager) -> (bool, String) {
    if m.probe.is_none() {
        let mut buf = vec![0u8; 4096];
        let fails = unsafe { fr_vd_probe(buf.as_mut_ptr() as *mut c_char, buf.len() as u32) };
        let report = unsafe { CStr::from_ptr(buf.as_ptr() as *const c_char) }
            .to_string_lossy()
            .to_string();
        log::info!(
            "vdisplay: probe {} failure(s)\n{}",
            fails,
            report.trim_end()
        );
        m.probe = Some((fails == 0, report));
    }
    m.probe.clone().unwrap_or_default()
}

/// Supported = private API present and FastRustDesk installed.
fn supported_reason(m: &mut Manager) -> Result<(), String> {
    let (ok, _) = probe_result(m);
    if !ok {
        return Err("CGVirtualDisplay unavailable on this macOS".to_owned());
    }
    if !crate::platform::is_installed() {
        return Err("not installed".to_owned());
    }
    Ok(())
}

pub fn is_supported() -> bool {
    supported_reason(&mut MANAGER.lock().unwrap()).is_ok()
}

impl Manager {
    fn create(&mut self, c: &SlotConfig) -> Result<u32, String> {
        let product = PRODUCT_BASE | c.slot as u32;
        let stale = unsafe { fr_vd_find(VENDOR, product, c.serial) };
        if stale != 0 && !self.live.values().any(|l| l.display_id == stale) {
            log::warn!(
                "vdisplay: stale display {} for slot {}, waiting",
                stale,
                c.slot
            );
            let t = Instant::now();
            while unsafe { fr_vd_find(VENDOR, product, c.serial) } != 0
                && t.elapsed() < Duration::from_millis(1500)
            {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        let (modes, mw, mh) = modes_for(c);
        let name = CString::new(if c.name.is_empty() {
            default_name(c.slot)
        } else {
            c.name.clone()
        })
        .unwrap_or_default();
        let mut err = vec![0u8; 512];
        let t = Instant::now();
        let id = unsafe {
            fr_vd_create(
                VENDOR,
                product,
                c.serial,
                name.as_ptr(),
                mw,
                mh,
                modes.as_ptr(),
                (modes.len() / 3) as u32,
                c.hidpi as i32,
                err.as_mut_ptr() as *mut c_char,
                err.len() as u32,
            )
        };
        if id == 0 {
            let e = unsafe { CStr::from_ptr(err.as_ptr() as *const c_char) }
                .to_string_lossy()
                .to_string();
            log::error!(
                "vdisplay: create slot {} {} failed: {}",
                c.slot,
                c.label(),
                e
            );
            return Err(e);
        }
        unsafe { fr_vd_wait_quiet(500, 3000) };
        let applied = self.apply_target(id, c);
        log::info!(
            "vdisplay: created slot {} id {} {} px {:?} in {} ms (mode {})",
            c.slot,
            id,
            c.label(),
            c.px(),
            t.elapsed().as_millis(),
            applied
        );
        self.live.insert(
            c.slot,
            Live {
                display_id: id,
                cfg: c.clone(),
            },
        );
        Ok(id)
    }

    /// Make sure the display runs the slot's target mode. Returns a log word.
    fn apply_target(&self, id: u32, c: &SlotConfig) -> &'static str {
        let (pw, ph) = c.px();
        for _ in 0..30 {
            match unsafe { fr_vd_apply_mode(id, c.w, c.h, pw, ph, c.refresh) } {
                1 => {
                    // Crisp #284: WindowServer may replace a mode that already held
                    // once during registration; confirm it survives a quiet period.
                    unsafe { fr_vd_wait_quiet(300, 1500) };
                    if let Some((w, h, ..)) = current_mode(id) {
                        if w == c.w && h == c.h {
                            return "ok";
                        }
                    }
                }
                0 => return "not-declared",
                -2 => {} // modes not visible to this process yet
                _ => return "config-failed",
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        "timeout"
    }

    fn destroy(&mut self, slot: u8) {
        if let Some(l) = self.live.remove(&slot) {
            let r = unsafe { fr_vd_destroy(l.display_id, 1500) };
            unsafe { fr_vd_wait_quiet(500, 3000) };
            log::info!(
                "vdisplay: destroyed slot {} id {} ({})",
                slot,
                l.display_id,
                if r == 1 { "offline" } else { "still online" }
            );
        }
    }

    /// Whether removing `slot` leaves the machine with something to show (FastRustDesk.md 8.5).
    fn can_remove(&self, slot: u8) -> bool {
        let Some(l) = self.live.get(&slot) else {
            return true;
        };
        let others = list_online()
            .iter()
            .filter(|d| d.id != l.display_id && d.mirrored == 0)
            .filter(|d| d.vendor != 0 && d.vendor <= 0xFFFF)
            .count();
        if others > 0 {
            return true;
        }
        if !self.sessions.is_empty() {
            return false;
        }
        // Losing the last display on a portable sends it into clamshell sleep at
        // once, even under a power assertion (Crisp, measured).
        unsafe { fr_vd_has_battery() == 0 }
    }

    fn reconcile(&mut self) {
        let configs = load_configs();
        // Persistent demand follows the config.
        for s in 1..=MAX_SLOTS {
            let persistent = configs.iter().any(|c| c.slot == s && c.persistent);
            let set = self.demand.entry(s).or_default();
            if persistent {
                set.insert(Source::Persistent);
            } else {
                set.remove(&Source::Persistent);
            }
        }
        for s in 1..=MAX_SLOTS {
            let want = self.demand.get(&s).map(|d| !d.is_empty()).unwrap_or(false);
            let cfg = configs.iter().find(|c| c.slot == s).cloned();
            let have = self.live.contains_key(&s);
            match (want, have, cfg) {
                (true, false, Some(c)) => {
                    if self.create(&c).is_err() {
                        // Do not retry a failing session request forever.
                        if let Some(d) = self.demand.get_mut(&s) {
                            d.retain(|x| matches!(x, Source::Persistent));
                        }
                    }
                }
                (true, false, None) => {
                    if let Some(d) = self.demand.get_mut(&s) {
                        d.clear();
                    }
                }
                (false, true, _) | (true, true, None) => {
                    if self.can_remove(s) {
                        self.destroy(s);
                    } else {
                        log::info!("vdisplay: keeping slot {}: it is the last display", s);
                    }
                }
                (true, true, Some(c)) => {
                    let live = self.live.get(&s).cloned();
                    if let Some(l) = live {
                        if l.cfg.w != c.w
                            || l.cfg.h != c.h
                            || l.cfg.hidpi != c.hidpi
                            || l.cfg.refresh != c.refresh
                        {
                            self.reshape(s, &c);
                        } else if l.cfg != c {
                            // name/persistent/follow only: no display change
                            if let Some(x) = self.live.get_mut(&s) {
                                x.cfg = c;
                            }
                        }
                    }
                }
                (false, false, _) => {}
            }
        }
        self.refresh_status_cache();
    }

    /// Change a live slot to `c`: switch mode if declared, else recreate it with
    /// the same identity (FastRustDesk.md 8.5).
    fn reshape(&mut self, slot: u8, c: &SlotConfig) {
        // A display only offers the modes declared at creation, which are derived
        // from the target size, so a new size means recreating it with the same
        // identity (macOS keeps its arrangement per identity).
        let Some(old) = self.live.remove(&slot) else {
            return;
        };
        log::info!(
            "vdisplay: slot {} recreate {} -> {}",
            slot,
            old.cfg.label(),
            c.label()
        );
        unsafe { fr_vd_destroy(old.display_id, 1500) };
        let _ = self.create(c);
    }

    fn refresh_status_cache(&mut self) {
        let s = self.status_value().to_string();
        *STATUS_CACHE.lock().unwrap() = s;
    }

    fn status_value(&mut self) -> serde_json::Value {
        let supported = supported_reason(self);
        let configs = load_configs();
        let slots: Vec<serde_json::Value> = configs
            .iter()
            .map(|c| {
                let live = self.live.get(&c.slot);
                let sources: Vec<&str> = self
                    .demand
                    .get(&c.slot)
                    .map(|d| {
                        d.iter()
                            .map(|s| match s {
                                Source::Persistent => "persistent",
                                Source::Local => "local",
                                Source::Session(_) => "session",
                            })
                            .collect::<BTreeSet<_>>()
                            .into_iter()
                            .collect()
                    })
                    .unwrap_or_default();
                serde_json::json!({
                    "slot": c.slot, "name": c.name, "w": c.w, "h": c.h, "hidpi": c.hidpi,
                    "refresh": c.refresh, "persistent": c.persistent, "follow": c.follow,
                    "label": c.label(),
                    "active": live.is_some(),
                    "display_id": live.map(|l| l.display_id).unwrap_or(0),
                    "sources": sources,
                })
            })
            .collect();
        let reclaim_in = self
            .reclaim_at
            .map(|t| t.saturating_duration_since(Instant::now()).as_secs());
        serde_json::json!({
            "supported": supported.is_ok(),
            "reason": supported.err().unwrap_or_default(),
            "probe": self.probe.as_ref().map(|p| p.1.clone()).unwrap_or_default(),
            "headless": is_headless(),
            "sessions": self.sessions.len(),
            "reclaim_in": reclaim_in,
            "options": {
                "headless": opt_bool(OPT_HEADLESS, true),
                "allow_remote": opt_bool(OPT_ALLOW_REMOTE, true),
                "hidpi": opt_bool(OPT_HIDPI, true),
                "keep": keep_secs(),
            },
            "max_slots": MAX_SLOTS,
            "slots": slots,
        })
    }

    fn first_free_slot(configs: &[SlotConfig]) -> Option<u8> {
        (1..=MAX_SLOTS).find(|s| !configs.iter().any(|c| c.slot == *s))
    }
}

// ------------------------------------------------------------------ public API

/// Start the manager in `--server`: create persistent displays and run the
/// reclaim timer. Safe to call more than once.
pub fn init() {
    {
        let mut m = MANAGER.lock().unwrap();
        if m.inited {
            return;
        }
        m.inited = true;
    }
    std::thread::spawn(|| {
        // Let WindowServer settle after login before creating anything (Crisp).
        std::thread::sleep(Duration::from_millis(800));
        {
            let mut m = MANAGER.lock().unwrap();
            if let Err(e) = supported_reason(&mut m) {
                log::info!("vdisplay: unavailable: {}", e);
                m.refresh_status_cache();
                return;
            }
            m.reconcile();
        }
        loop {
            std::thread::sleep(Duration::from_secs(1));
            tick();
        }
    });
}

fn tick() {
    let mut m = MANAGER.lock().unwrap();
    if let Some(t) = m.reclaim_at {
        if Instant::now() >= t && m.sessions.is_empty() {
            log::info!("vdisplay: reclaiming session displays");
            for d in m.demand.values_mut() {
                d.retain(|s| !matches!(s, Source::Session(_)));
            }
            m.reclaim_at = None;
            m.reconcile();
        }
    }
}

/// Called before the display list is built for a new remote session.
/// Returns the display id the session should start on, if any (decisions 1-3).
pub fn on_login(conn_id: i32, cdi: Option<&ClientDisplayInfo>) -> Option<u32> {
    let mut m = MANAGER.lock().unwrap();
    if let Err(e) = supported_reason(&mut m) {
        log::debug!("vdisplay: unavailable for conn {}: {}", conn_id, e);
        return None;
    }
    m.sessions.insert(conn_id);
    m.reclaim_at = None;
    let (tw, th, thidpi) = resolve_target(cdi, opt_bool(OPT_HIDPI, true));
    m.targets.insert(conn_id, (tw, th, thidpi));
    log::info!(
        "vdisplay: conn {} controller {:?} -> target {}x{} hidpi={}",
        conn_id,
        cdi.map(|c| (c.width, c.height, c.scale)),
        tw,
        th,
        thidpi
    );
    let mut configs = load_configs();
    let sole = m.sessions.len() == 1;
    if let Some(p) = configs.iter_mut().find(|c| c.persistent) {
        // Only follow a controller that told us its size: an old client sends
        // nothing, and resizing to the default would just undo the last choice.
        let told = cdi.map(|c| c.width > 0 && c.height > 0).unwrap_or(false);
        if sole && told && p.follow && (p.w, p.h, p.hidpi) != (tw, th, thidpi) {
            log::info!("vdisplay: slot {} follows controller {}x{}", p.slot, tw, th);
            p.w = tw;
            p.h = th;
            p.hidpi = thidpi;
            let slot = p.slot;
            save_configs(&configs);
            m.reconcile();
            return m.live.get(&slot).map(|l| l.display_id);
        }
        let slot = p.slot;
        m.reconcile();
        return m.live.get(&slot).map(|l| l.display_id);
    }
    if is_headless() && opt_bool(OPT_HEADLESS, true) {
        let slot = Manager::first_free_slot(&configs).unwrap_or(1);
        configs.retain(|c| c.slot != slot);
        let mut c = SlotConfig::new(slot, tw, th, thidpi);
        c.persistent = true;
        c.follow = true;
        log::info!(
            "vdisplay: headless, creating persistent slot {} {}",
            slot,
            c.label()
        );
        configs.push(c);
        configs.sort_by_key(|c| c.slot);
        save_configs(&configs);
        m.reconcile();
        return m.live.get(&slot).map(|l| l.display_id);
    }
    None
}

/// Called when a remote connection ends; `remote_left` is the number still open.
pub fn on_conn_closed(conn_id: i32, remote_left: usize) {
    let mut m = MANAGER.lock().unwrap();
    m.targets.remove(&conn_id);
    if !m.sessions.remove(&conn_id) && remote_left > 0 {
        return;
    }
    if remote_left == 0 {
        m.sessions.clear();
        let keep = keep_secs();
        let has_session_demand = m
            .demand
            .values()
            .any(|d| d.iter().any(|s| matches!(s, Source::Session(_))));
        if has_session_demand && keep >= 0 {
            m.reclaim_at = Some(Instant::now() + Duration::from_secs(keep as u64));
            log::info!("vdisplay: last session closed, reclaim in {} s", keep);
        }
        m.refresh_status_cache();
    }
}

/// Toolbar `ToggleVirtualDisplay` (slot 1..=4, or -1 for all session displays).
pub fn toggle(conn_id: i32, slot: i32, on: bool) -> Result<(), String> {
    let mut m = MANAGER.lock().unwrap();
    supported_reason(&mut m)?;
    if !opt_bool(OPT_ALLOW_REMOTE, true) {
        return Err("mac_vd_remote_disabled_tip".to_owned());
    }
    let mut configs = load_configs();
    if slot == -1 {
        if on {
            return Err("invalid".to_owned());
        }
        for d in m.demand.values_mut() {
            d.retain(|s| !matches!(s, Source::Session(_) | Source::Local));
        }
        m.reconcile();
        return Ok(());
    }
    if slot < 1 || slot > MAX_SLOTS as i32 {
        return Err(format!("bad slot {}", slot));
    }
    let slot = slot as u8;
    if on {
        if !configs.iter().any(|c| c.slot == slot) {
            // New displays take the requesting controller's size (decision 3).
            let (w, h, hidpi) = m
                .targets
                .get(&conn_id)
                .cloned()
                .unwrap_or_else(|| resolve_target(None, opt_bool(OPT_HIDPI, true)));
            configs.push(SlotConfig::new(slot, w, h, hidpi));
            configs.sort_by_key(|c| c.slot);
            save_configs(&configs);
        }
        m.demand
            .entry(slot)
            .or_default()
            .insert(Source::Session(conn_id));
    } else {
        if configs.iter().any(|c| c.slot == slot && c.persistent) {
            return Err("mac_vd_persistent_tip".to_owned());
        }
        if let Some(d) = m.demand.get_mut(&slot) {
            d.retain(|s| !matches!(s, Source::Session(_) | Source::Local));
        }
        if m.live.contains_key(&slot) && !m.can_remove(slot) {
            return Err("mac_vd_last_display_tip".to_owned());
        }
    }
    m.reconcile();
    if on && !m.live.contains_key(&slot) {
        return Err("Failed to create the virtual display".to_owned());
    }
    Ok(())
}

/// `ChangeResolution` for one of our displays. `None` = not ours, use the normal path.
pub fn set_resolution(display_id: u32, w: u32, h: u32) -> Option<Result<(), String>> {
    let mut m = MANAGER.lock().unwrap();
    let slot = m
        .live
        .iter()
        .find(|(_, l)| l.display_id == display_id)
        .map(|(s, _)| *s)?;
    if w < 640 || h < 480 || w > PIXEL_CAP.0 || h > PIXEL_CAP.1 {
        return Some(Err(format!("unsupported resolution {}x{}", w, h)));
    }
    let mut configs = load_configs();
    let Some(c) = configs.iter_mut().find(|c| c.slot == slot) else {
        return Some(Err("slot has no config".to_owned()));
    };
    c.w = w & !1;
    c.h = h & !1;
    c.hidpi = c.hidpi && c.w * 2 <= PIXEL_CAP.0 && c.h * 2 <= PIXEL_CAP.1;
    c.follow = false; // an explicit choice wins over following the controller
    save_configs(&configs);
    m.reconcile();
    let ok = m
        .live
        .get(&slot)
        .map(|l| l.cfg.w == w & !1 && l.cfg.h == h & !1)
        .unwrap_or(false);
    Some(if ok {
        Ok(())
    } else {
        Err("resolution change failed".to_owned())
    })
}

/// Peer-info additions for the controlling side (FastRustDesk.md 8.6).
pub fn platform_additions() -> serde_json::Map<String, serde_json::Value> {
    let mut map = serde_json::Map::new();
    let mut m = MANAGER.lock().unwrap();
    if supported_reason(&mut m).is_err() {
        return map;
    }
    let configs = load_configs();
    let mut live: Vec<u8> = m.live.keys().cloned().collect();
    live.sort();
    let slots: Vec<serde_json::Value> = (1..=MAX_SLOTS)
        .map(|s| {
            let c = configs.iter().find(|c| c.slot == s);
            serde_json::json!({
                "slot": s,
                "label": c.map(|c| c.label()).unwrap_or_default(),
                "active": m.live.contains_key(&s),
                "persistent": c.map(|c| c.persistent).unwrap_or(false),
            })
        })
        .collect();
    map.insert("idd_impl".into(), serde_json::json!(IDD_IMPL));
    map.insert("rustdesk_virtual_displays".into(), serde_json::json!(live));
    map.insert("mac_vd_slots".into(), serde_json::json!(slots));
    map.insert(
        "mac_vd_allow_remote".into(),
        serde_json::json!(opt_bool(OPT_ALLOW_REMOTE, true)),
    );
    map
}

/// Status JSON (FastRustDesk.md 8.2). Never blocks: the tray calls this on the main thread.
pub fn status_json_cached() -> String {
    STATUS_CACHE.lock().unwrap().clone()
}

/// Status JSON, freshly computed.
pub fn status_json() -> String {
    let mut m = MANAGER.lock().unwrap();
    m.refresh_status_cache();
    let v = m.status_value();
    v.to_string()
}

#[derive(Deserialize, Default)]
struct Cmd {
    cmd: String,
    #[serde(default)]
    slot: Option<serde_json::Value>,
    name: Option<String>,
    w: Option<u32>,
    h: Option<u32>,
    hidpi: Option<bool>,
    refresh: Option<u32>,
    persistent: Option<bool>,
    follow: Option<bool>,
    key: Option<String>,
    value: Option<String>,
}

fn validate_size(w: u32, h: u32, hidpi: bool) -> Result<bool, String> {
    if w < 640 || h < 480 || w > PIXEL_CAP.0 || h > PIXEL_CAP.1 {
        return Err(format!("unsupported resolution {}x{}", w, h));
    }
    Ok(hidpi && w * 2 <= PIXEL_CAP.0 && h * 2 <= PIXEL_CAP.1)
}

/// Local management command (FastRustDesk.md 8.2). Returns a short result text.
pub fn command(json: &str) -> Result<String, String> {
    let c: Cmd = serde_json::from_str(json).map_err(|e| format!("bad command: {}", e))?;
    let mut m = MANAGER.lock().unwrap();
    if c.cmd != "opt" {
        supported_reason(&mut m)?;
    }
    let mut configs = load_configs();
    let slot_num = |v: &Option<serde_json::Value>| -> Option<u8> {
        v.as_ref()
            .and_then(|x| {
                x.as_u64()
                    .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
            })
            .filter(|s| *s >= 1 && *s <= MAX_SLOTS as u64)
            .map(|s| s as u8)
    };
    let all = c.slot.as_ref().and_then(|v| v.as_str()) == Some("all");
    let res = match c.cmd.as_str() {
        "on" => {
            let s = slot_num(&c.slot).ok_or("slot required")?;
            if !configs.iter().any(|x| x.slot == s) {
                let (w, h, hidpi) = resolve_target(None, opt_bool(OPT_HIDPI, true));
                configs.push(SlotConfig::new(s, w, h, hidpi));
                configs.sort_by_key(|x| x.slot);
                save_configs(&configs);
            }
            m.demand.entry(s).or_default().insert(Source::Local);
            m.reconcile();
            if m.live.contains_key(&s) {
                Ok(format!("slot {} on", s))
            } else {
                Err(format!("slot {} failed to start", s))
            }
        }
        "off" => {
            let targets: Vec<u8> = if all {
                (1..=MAX_SLOTS).collect()
            } else {
                vec![slot_num(&c.slot).ok_or("slot required")?]
            };
            let mut refused = vec![];
            for s in targets {
                if configs.iter().any(|x| x.slot == s && x.persistent) {
                    if !all {
                        refused.push(format!("slot {} is persistent", s));
                    }
                    continue;
                }
                if let Some(d) = m.demand.get_mut(&s) {
                    d.retain(|x| matches!(x, Source::Persistent));
                }
            }
            m.reconcile();
            if refused.is_empty() {
                Ok("off".to_owned())
            } else {
                Err(refused.join("; "))
            }
        }
        "add" => {
            let s = Manager::first_free_slot(&configs).ok_or("all slots are in use")?;
            let w = c.w.unwrap_or(DEFAULT_LOGICAL.0);
            let h = c.h.unwrap_or(DEFAULT_LOGICAL.1);
            let hidpi = validate_size(w, h, c.hidpi.unwrap_or(true))?;
            let mut n = SlotConfig::new(s, w & !1, h & !1, hidpi);
            if let Some(name) = c.name.filter(|x| !x.trim().is_empty()) {
                n.name = name;
            }
            n.refresh = match c.refresh {
                Some(30) => 30,
                _ => 60,
            };
            n.persistent = c.persistent.unwrap_or(false);
            n.follow = c.follow.unwrap_or(false);
            configs.push(n);
            configs.sort_by_key(|x| x.slot);
            save_configs(&configs);
            m.reconcile();
            Ok(format!("slot {} added", s))
        }
        "set" => {
            let s = slot_num(&c.slot).ok_or("slot required")?;
            let x = configs
                .iter_mut()
                .find(|x| x.slot == s)
                .ok_or(format!("slot {} not configured", s))?;
            if let Some(name) = c.name {
                x.name = if name.trim().is_empty() {
                    default_name(s)
                } else {
                    name
                };
            }
            let w = c.w.unwrap_or(x.w);
            let h = c.h.unwrap_or(x.h);
            let hidpi = validate_size(w, h, c.hidpi.unwrap_or(x.hidpi))?;
            x.w = w & !1;
            x.h = h & !1;
            x.hidpi = hidpi;
            if let Some(r) = c.refresh {
                x.refresh = if r == 30 { 30 } else { 60 };
            }
            if let Some(p) = c.persistent {
                x.persistent = p;
            }
            if let Some(f) = c.follow {
                x.follow = f;
            }
            save_configs(&configs);
            m.reconcile();
            Ok(format!("slot {} updated", s))
        }
        "rm" => {
            let s = slot_num(&c.slot).ok_or("slot required")?;
            if !configs.iter().any(|x| x.slot == s) {
                return Err(format!("slot {} not configured", s));
            }
            if m.live.contains_key(&s) && !m.can_remove(s) {
                return Err("mac_vd_last_display_tip".to_owned());
            }
            if let Some(d) = m.demand.get_mut(&s) {
                d.clear();
            }
            configs.retain(|x| x.slot != s);
            save_configs(&configs);
            m.reconcile();
            Ok(format!("slot {} removed", s))
        }
        "opt" => {
            let key = c.key.ok_or("key required")?;
            let value = c.value.unwrap_or_default();
            let (k, v) = match key.as_str() {
                "headless" => (OPT_HEADLESS, yn(&value)?),
                "allow-remote" | "allow_remote" => (OPT_ALLOW_REMOTE, yn(&value)?),
                "hidpi" => (OPT_HIDPI, yn(&value)?),
                "keep" => {
                    let n: i64 = value.parse().map_err(|_| "keep must be seconds or -1")?;
                    (OPT_KEEP, n.max(-1).to_string())
                }
                _ => return Err(format!("unknown option {}", key)),
            };
            Config::set_option(k.to_owned(), v);
            m.refresh_status_cache();
            Ok("ok".to_owned())
        }
        other => Err(format!("unknown command {}", other)),
    };
    m.refresh_status_cache();
    res
}

fn yn(v: &str) -> Result<String, String> {
    match v.to_ascii_lowercase().as_str() {
        "y" | "yes" | "on" | "true" | "1" => Ok("Y".to_owned()),
        "n" | "no" | "off" | "false" | "0" => Ok("N".to_owned()),
        _ => Err(format!("expected on/off, got {}", v)),
    }
}

/// `FastRustDesk --vdisplay probe`: run the probe in this process.
pub fn probe_report() -> (i32, String) {
    let mut buf = vec![0u8; 4096];
    let fails = unsafe { fr_vd_probe(buf.as_mut_ptr() as *mut c_char, buf.len() as u32) };
    let report = unsafe { CStr::from_ptr(buf.as_ptr() as *const c_char) }
        .to_string_lossy()
        .to_string();
    (fails, report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cdi(w: i32, h: i32) -> ClientDisplayInfo {
        ClientDisplayInfo {
            width: w,
            height: h,
            scale: 1.0,
            ..Default::default()
        }
    }

    #[test]
    fn target_from_controller() {
        assert_eq!(
            resolve_target(Some(&cdi(1920, 1080)), true),
            (1920, 1080, true)
        );
        assert_eq!(
            resolve_target(Some(&cdi(1280, 832)), true),
            (1280, 832, true)
        );
        // 4K at 100%: 2x would exceed the cap, so 1x
        assert_eq!(
            resolve_target(Some(&cdi(3840, 2160)), true),
            (3840, 2160, false)
        );
        // no controller info
        assert_eq!(resolve_target(None, true), (1920, 1080, true));
        assert_eq!(resolve_target(None, false), (1920, 1080, false));
        // phone held upright, logical 412x915: landscape and scaled up to the minimum
        let (w, h, hi) = resolve_target(Some(&cdi(412, 915)), true);
        assert!(w >= 1024 && h >= 640 && w > h && hi, "{w}x{h}");
        assert!(((w as f64 / h as f64) - 915.0 / 412.0).abs() < 0.02);
        // odd sizes become even
        assert_eq!(
            resolve_target(Some(&cdi(1513, 983)), true),
            (1512, 982, true)
        );
        // huge: scaled into the cap keeping the aspect
        let (w, h, hi) = resolve_target(Some(&cdi(7680, 4320)), true);
        assert!(!hi && w <= PIXEL_CAP.0 && h <= PIXEL_CAP.1, "{w}x{h}");
    }

    #[test]
    fn declared_modes() {
        let mut c = SlotConfig::new(1, 1920, 1080, true);
        let (m, mw, mh) = modes_for(&c);
        assert_eq!((mw, mh), (3840, 2160));
        assert_eq!(m, vec![3840, 2160, 60, 1920, 1080, 60]);
        c.refresh = 30;
        let (m, ..) = modes_for(&c);
        assert_eq!(m.len() / 3, 4);
        c.hidpi = false;
        let (m, mw, _) = modes_for(&c);
        assert_eq!(mw, 1920);
        assert_eq!(m.len() / 3, 2);
    }

    fn disp(vendor: u32, builtin: bool, px_w: u32) -> FrDisp {
        FrDisp {
            id: 1,
            vendor,
            active: 1,
            builtin: builtin as i32,
            px_w,
            ..Default::default()
        }
    }

    #[test]
    fn headless_rules() {
        assert!(headless_of(&[]));
        assert!(headless_of(&[disp(0, false, 1920)])); // vendor 0 stub
        assert!(headless_of(&[disp(0x756e6b6e, false, 1920)])); // 'unkn' placeholder
        assert!(headless_of(&[disp(0x4cab, false, 1024)])); // tiny fallback
        assert!(headless_of(&[disp(VENDOR, false, 3840)])); // only ours
        assert!(!headless_of(&[disp(0x4cab, false, 5120)])); // a real monitor
        assert!(!headless_of(&[disp(0x610, true, 1024)])); // a built-in panel
        let mut asleep = disp(0x610, true, 2940);
        asleep.active = 0;
        assert!(!headless_of(&[asleep])); // screen asleep, lid open
    }

    #[test]
    fn config_roundtrip_defaults() {
        let v: Vec<SlotConfig> = serde_json::from_str(r#"[{"slot":2,"w":1600,"h":900}]"#).unwrap();
        assert_eq!(v[0].refresh, 60);
        assert!(v[0].hidpi);
        assert!(!v[0].persistent);
        assert_eq!(v[0].label(), "1600×900 HiDPI");
    }
}
