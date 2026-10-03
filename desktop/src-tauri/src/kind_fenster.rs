//! Child windows of Noki-owned apps (open/save panels, dialogs, alerts).
//!
//! Measured 2026-09-25: TextEdit's "Öffnen …" panel appears at its own
//! remembered/centered frame on the USER display (281,145) - even with the
//! app active and its Noki window key; panel placement cannot be steered.
//! Sheets attach to their parent and are already on Noki's display.
//!
//! Therefore: every app that has a live Noki window gets an AXObserver for
//! `AXWindowCreated` (event-driven, no polling). A window created by that
//! app within `FRIST` after an explicit Noki operation on one of its Noki
//! windows (click, drag, typed key) is caused by Noki: it is moved onto
//! Noki's display inside the notification callback - the earliest moment a
//! third-party process can act - and then registered as a Noki child of the
//! operated window. Without such an operation nothing is touched, so a
//! user's own dialogs stay on the user's desktop.

#![cfg(target_os = "macos")]

use std::ffi::c_void;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const FRIST: Duration = Duration::from_secs(10);

#[repr(C)]
struct Punkt { x: f64, y: f64 }
#[repr(C)]
struct Groesse { w: f64, h: f64 }

type Observer = *mut c_void;
type Callback = unsafe extern "C" fn(Observer, *mut c_void, *const c_void, *mut c_void);

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXObserverCreate(pid: i32, cb: Callback, out: *mut Observer) -> i32;
    fn AXObserverAddNotification(o: Observer, el: *mut c_void, n: *const c_void, refcon: *mut c_void) -> i32;
    fn AXObserverGetRunLoopSource(o: Observer) -> *mut c_void;
    fn AXUIElementCreateApplication(pid: i32) -> *mut c_void;
    fn AXUIElementCopyAttributeValue(el: *mut c_void, a: *const c_void, v: *mut *const c_void) -> i32;
    fn AXUIElementSetAttributeValue(el: *mut c_void, a: *const c_void, v: *const c_void) -> i32;
    fn AXUIElementGetPid(el: *mut c_void, pid: *mut i32) -> i32;
    fn AXValueCreate(t: u32, v: *const c_void) -> *const c_void;
    fn AXValueGetValue(v: *const c_void, t: u32, out: *mut c_void) -> bool;
    fn _AXUIElementGetWindow(el: *mut c_void, wid: *mut u32) -> i32;
}
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFRunLoopDefaultMode: *const c_void;
    fn CFRunLoopGetCurrent() -> *mut c_void;
    fn CFRunLoopAddSource(rl: *mut c_void, src: *mut c_void, mode: *const c_void);
    fn CFRunLoopRunInMode(mode: *const c_void, sec: f64, ret: bool) -> i32;
    fn CFStringCreateWithCString(a: *const c_void, s: *const i8, enc: u32) -> *const c_void;
    fn CFRelease(o: *const c_void);
    fn CFStringGetCString(s: *const c_void, buf: *mut i8, n: isize, enc: u32) -> bool;
}

unsafe fn ax_text(el: *mut c_void, name: &str) -> String {
    let a = cfstr(name);
    let mut v: *const c_void = std::ptr::null();
    let ok = AXUIElementCopyAttributeValue(el, a, &mut v) == 0 && !v.is_null();
    CFRelease(a);
    if !ok { return String::new(); }
    let mut buf = [0i8; 256];
    let r = if CFStringGetCString(v, buf.as_mut_ptr(), 256, 0x0800_0100) {
        std::ffi::CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
    } else { String::new() };
    CFRelease(v);
    r
}

unsafe fn cfstr(s: &str) -> *const c_void {
    let c = std::ffi::CString::new(s).unwrap_or_default();
    CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), 0x0800_0100)
}

/// (pid, parent wid, parent frame, deadline) of the running Noki operation.
static TRANSAKTION: Mutex<Vec<(i32, i64, [f64; 4], Instant)>> = Mutex::new(Vec::new());
/// Windows of the app that existed when the operation started: never
/// touched (the user's own windows stay where they are, whatever gets focus).
static VORHER: Mutex<Vec<(i32, Vec<i64>)>> = Mutex::new(Vec::new());
/// Windows already handled (created + focus notifications both arrive).
static ERLEDIGT: Mutex<Vec<i64>> = Mutex::new(Vec::new());

fn vorher_merken(pid: i32) {
    // Only windows VISIBLE at that moment: AppKit pre-creates panels
    // ordered-out (measured TextEdit: the "Öffnen" panel window existed
    // before the click and only appeared afterwards).
    let ids: Vec<i64> = crate::cgs::alle_fenster().into_iter()
        .filter(|f| f.1 == pid && crate::cgs::fenster_eingeordnet(f.0)).map(|f| f.0).collect();
    if let Ok(mut g) = VORHER.lock() { g.retain(|e| e.0 != pid); g.push((pid, ids)); }
}
/// Apps that should be observed (live Noki windows).
static GEWUENSCHT: Mutex<Vec<i32>> = Mutex::new(Vec::new());
/// Apps whose observer is live on the run loop.
static BEOBACHTET: Mutex<Vec<i32>> = Mutex::new(Vec::new());
/// Windows moved in the callback, waiting for registration.
static ERFASST: Mutex<Vec<(i32, i64, i64)>> = Mutex::new(Vec::new());

/// Explicit Noki operation on `parent` (a Noki window of `pid`).
pub fn transaktion(pid: i32, parent: i64, rahmen: Option<[f64; 4]>) {
    if pid <= 0 || parent <= 0 { return; }
    let Some(r) = rahmen else { return };
    if let Ok(mut g) = TRANSAKTION.lock() {
        g.retain(|e| e.0 != pid && e.3 > Instant::now());
        g.push((pid, parent, r, Instant::now() + FRIST));
    }
    vorher_merken(pid);
    beobachten_mit(pid);
}

/// Noki-requested NEW window of a running app (menu "New Window"): the
/// callback moves every window this app creates in the next seconds onto
/// Noki's display at `rahmen` (parent -1: the launch flow registers it).
pub fn erzeugung(pid: i32, rahmen: [f64; 4]) {
    if pid <= 0 { return; }
    if let Ok(mut g) = TRANSAKTION.lock() {
        g.retain(|e| e.0 != pid && e.3 > Instant::now());
        g.push((pid, -1, rahmen, Instant::now() + Duration::from_secs(6)));
    }
    vorher_merken(pid);
    beobachten_mit(pid);
    // The window must not be created before the observer exists.
    let bis = Instant::now() + Duration::from_millis(600);
    while Instant::now() < bis && !BEOBACHTET.lock().map(|g| g.contains(&pid)).unwrap_or(true) {
        std::thread::sleep(Duration::from_millis(5));
    }
}
pub fn erzeugung_ende(pid: i32) {
    if let Ok(mut g) = TRANSAKTION.lock() { g.retain(|e| !(e.0 == pid && e.1 == -1)); }
}

/// Keep observers for exactly the apps that host Noki windows.
pub fn beobachten(pids: &[i32]) {
    if let Ok(mut g) = GEWUENSCHT.lock() {
        for p in pids { if *p > 0 && !g.contains(p) { g.push(*p); } }
    }
    starten();
}

fn beobachten_mit(pid: i32) { beobachten(&[pid]); }

/// Windows captured by the callback since the last call.
pub fn erfasste() -> Vec<(i32, i64, i64)> {
    ERFASST.lock().map(|mut g| std::mem::take(&mut *g)).unwrap_or_default()
}

unsafe extern "C" fn neues_fenster(_o: Observer, el: *mut c_void, _n: *const c_void, _r: *mut c_void) {
    let mut pid = 0;
    if AXUIElementGetPid(el, &mut pid) != 0 { return; }
    let t = TRANSAKTION.lock().ok()
        .and_then(|g| g.iter().find(|e| e.0 == pid && e.3 > Instant::now()).copied());
    let mut wid: u32 = 0;
    let _ = _AXUIElementGetWindow(el, &mut wid);
    let Some((_, parent, pr, _)) = t else { return };
    if wid == 0 { return; }
    let w64 = wid as i64;
    if w64 == parent { return; }
    // Only REAL windows become Noki children: AXWindow with a standard /
    // dialog subrole, or an AXSheet. Popovers (Calendar's event popover,
    // Safari's), tooltips and help tags were claimed before and lived on as
    // tiny ghost "windows" in the Miniatur.
    let (rolle, sub) = (ax_text(el, "AXRole"), ax_text(el, "AXSubrole"));
    let echt = rolle == "AXSheet" || (rolle == "AXWindow"
        && matches!(sub.as_str(), "AXStandardWindow" | "AXDialog" | "AXSystemDialog"));
    if !echt { return; }
    if VORHER.lock().map(|g| g.iter().any(|e| e.0 == pid && e.1.contains(&w64))).unwrap_or(true) { return; }
    if crate::fenster_pid(w64) != 0 { return; } // already a registered Noki window
    {
        let Ok(mut g) = ERLEDIGT.lock() else { return };
        if g.contains(&w64) { return; }
        g.push(w64);
        if g.len() > 200 { g.drain(..100); }
    }
    let Some(d) = crate::virtual_workspace::info() else { return };
    let (pos, size) = (cfstr("AXPosition"), cfstr("AXSize"));
    let mut v: *const c_void = std::ptr::null();
    let mut p = Punkt { x: 0.0, y: 0.0 };
    let mut s = Groesse { w: 0.0, h: 0.0 };
    if AXUIElementCopyAttributeValue(el, pos, &mut v) == 0 && !v.is_null() { AXValueGetValue(v, 1, &mut p as *mut Punkt as *mut c_void); CFRelease(v); }
    v = std::ptr::null();
    if AXUIElementCopyAttributeValue(el, size, &mut v) == 0 && !v.is_null() { AXValueGetValue(v, 2, &mut s as *mut Groesse as *mut c_void); CFRelease(v); }
    let auf_noki = p.x >= d.x as f64 && p.y >= d.y as f64
        && p.x < (d.x + d.width) as f64 && p.y < (d.y + d.height) as f64;
    let mut bewegt = false;
    if !auf_noki {
        // Centered over the operated parent (launch: at the target slot),
        // clamped into Noki's display.
        let dx = d.x as f64; let dy = d.y as f64;
        let (nx, ny) = if parent < 0 { (pr[0], pr[1]) } else {
            ((pr[0] + (pr[2] - s.w) / 2.0).clamp(dx, dx + (d.width as f64 - s.w).max(0.0)),
             (pr[1] + (pr[3] - s.h) / 3.0).clamp(dy, dy + (d.height as f64 - s.h).max(0.0)))
        };
        let ziel = Punkt { x: nx, y: ny };
        let wert = AXValueCreate(1, &ziel as *const Punkt as *const c_void);
        bewegt = !wert.is_null() && AXUIElementSetAttributeValue(el, pos, wert) == 0;
        if !wert.is_null() { CFRelease(wert); }
    }
    CFRelease(pos); CFRelease(size);
    crate::virtual_workspace::trace(&format!(
        "[OWNERSHIP] child_created wid={wid} pid={pid} parent={parent} was_on_user_display={} moved_in_callback={bewegt} cause=noki_operation",
        !auf_noki
    ));
    if parent < 0 { return; } // launch flow registers the window itself
    if s.w < 150.0 || s.h < 100.0 { return; } // helper bubbles, tooltips
    if let Ok(mut g) = ERFASST.lock() { g.push((pid, wid as i64, parent)); }
    crate::kind_fenster_anstossen();
}

fn starten() {
    static FADEN: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    FADEN.get_or_init(|| {
        let _ = std::thread::Builder::new().name("noki-kindfenster".into()).spawn(|| unsafe {
            let mut beobachtet: Vec<i32> = Vec::new();
            let name = cfstr("AXWindowCreated");
            // Modal panels of Electron apps (VS Code "Open") are not in
            // AXWindows and only announce themselves as the new focused
            // window (measured).
            let fokus = cfstr("AXFocusedWindowChanged");
            loop {
                let wunsch = GEWUENSCHT.lock().map(|g| g.clone()).unwrap_or_default();
                for pid in wunsch {
                    if beobachtet.contains(&pid) { continue; }
                    beobachtet.push(pid);
                    let mut o: Observer = std::ptr::null_mut();
                    if AXObserverCreate(pid, neues_fenster, &mut o) != 0 || o.is_null() { continue; }
                    let app = AXUIElementCreateApplication(pid);
                    let ok = AXObserverAddNotification(o, app, name, std::ptr::null_mut());
                    let _ = AXObserverAddNotification(o, app, fokus, std::ptr::null_mut());
                    CFRunLoopAddSource(CFRunLoopGetCurrent(), AXObserverGetRunLoopSource(o), kCFRunLoopDefaultMode);
                    if ok == 0 { if let Ok(mut g) = BEOBACHTET.lock() { g.push(pid); } }
                    crate::virtual_workspace::trace(&format!("[OWNERSHIP] child_observer pid={pid} ok={}", ok == 0));
                }
                // Short slices: a newly requested app is observed within
                // ~50 ms (callbacks still run as soon as they arrive).
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.05, false);
            }
        });
    });
}

// ---------------------------------------------------------------------
//  REAL_SPACE membership events (minimize / restore / create / destroy)
// ---------------------------------------------------------------------
// The Miniatur must follow the real Noki Space without polling harder:
// these notifications wake the membership loop at once; its 1 s tick only
// remains as fallback. Observers exist only for apps that host windows on
// the Noki Space.

static MITGLIED_PIDS: std::sync::Mutex<Vec<i32>> = std::sync::Mutex::new(Vec::new());
static MITGLIED_WECKER: std::sync::OnceLock<fn()> = std::sync::OnceLock::new();

unsafe extern "C" fn mitglied_ereignis(_o: Observer, _el: *mut c_void, n: *const c_void, _r: *mut c_void) {
    let mut buf = [0i8; 64];
    let name = if CFStringGetCString(n, buf.as_mut_ptr(), 64, 0x0800_0100) {
        std::ffi::CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
    } else { String::new() };
    // Debounce: at most one wake-up per 250 ms.
    static ZULETZT: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);
    let frei = ZULETZT.lock().map(|mut z| {
        let ok = z.is_none_or(|t| t.elapsed() > Duration::from_millis(250));
        if ok { *z = Some(Instant::now()); }
        ok
    }).unwrap_or(true);
    if !frei { return; }
    crate::virtual_workspace::trace(&format!("[MEMBERSHIP] event={name} -> immediate refresh"));
    if let Some(w) = MITGLIED_WECKER.get() { w(); }
}

/// Observe exactly these apps (additive; cheap to call repeatedly).
pub fn mitglieder_beobachten(pids: &[i32], wecker: fn()) {
    let _ = MITGLIED_WECKER.set(wecker);
    if let Ok(mut g) = MITGLIED_PIDS.lock() {
        for p in pids { if *p > 0 && !g.contains(p) { g.push(*p); } }
    }
    static FADEN: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    FADEN.get_or_init(|| {
        let _ = std::thread::Builder::new().name("noki-mitglieder".into()).spawn(|| unsafe {
            let mut beobachtet: Vec<i32> = Vec::new();
            // NOT AXUIElementDestroyed: registered on the app element it fires
            // for every destroyed page element of Chromium apps (measured: a
            // flood of wake-ups). Window closes are caught by the 1 s tick.
            let namen: Vec<*const c_void> = ["AXWindowMiniaturized", "AXWindowDeminiaturized",
                "AXWindowCreated"].iter().map(|n| cfstr(n)).collect();
            loop {
                let wunsch = MITGLIED_PIDS.lock().map(|g| g.clone()).unwrap_or_default();
                for pid in wunsch {
                    if beobachtet.contains(&pid) { continue; }
                    beobachtet.push(pid);
                    let mut o: Observer = std::ptr::null_mut();
                    if AXObserverCreate(pid, mitglied_ereignis, &mut o) != 0 || o.is_null() { continue; }
                    let app = AXUIElementCreateApplication(pid);
                    let mut ok = 0;
                    for n in &namen { if AXObserverAddNotification(o, app, *n, std::ptr::null_mut()) == 0 { ok += 1; } }
                    CFRunLoopAddSource(CFRunLoopGetCurrent(), AXObserverGetRunLoopSource(o), kCFRunLoopDefaultMode);
                    crate::virtual_workspace::trace(&format!("[MEMBERSHIP] observer pid={pid} notifications={ok}"));
                }
                // 1 = kCFRunLoopRunFinished: no source registered (every
                // AXObserverCreate failed, or the observed app quit). The call
                // then returns at once - without this pause the thread spun at
                // 100 % of a core (measured after a reboot, ~10 min daily use).
                if CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.25, false) == 1 {
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
            }
        });
    });
}
