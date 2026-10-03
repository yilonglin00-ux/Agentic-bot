//! Arbeitsplatz-Fokus as a WINDOW SESSION.
//!
//! Start (one pass, no polling loop):
//!   * windows belonging only to the captured desktop (standard, not full screen)
//!     that do not belong to a selected app go to the Dock (AXMinimized);
//!   * a selected app that already has such a window keeps using it;
//!   * a selected app that only lives elsewhere (another Space, a full-screen
//!     Space) gets a NEW window here via its own "New Window" menu command
//!     (Accessibility AXPress - no clicks, no keystrokes, no Space switch);
//!   * a selected app that is not running is launched here.
//! Every window Noki minimized or created is kept as a retained AX element
//! (identity independent of titles).
//!
//! End (one pass): close exactly the windows this session created (native
//! close button - an app may ask to save; nothing is forced), then restore
//! exactly the windows this session minimized, if they still exist and are
//! still minimized. Windows on other Spaces/full screen are never touched.
#![allow(clashing_extern_declarations)]

use std::ffi::{c_void, CString};
use std::sync::Mutex;
use std::time::{Duration, Instant};

type Id = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> Id;
    fn AXUIElementCopyAttributeValue(e: Id, a: Id, v: *mut Id) -> i32;
    fn AXUIElementSetAttributeValue(e: Id, a: Id, v: Id) -> i32;
    fn AXUIElementPerformAction(e: Id, a: Id) -> i32;
    fn AXValueGetValue(v: Id, t: u32, p: *mut c_void) -> bool;
    fn AXValueCreate(t: u32, p: *const c_void) -> Id;
    fn _AXUIElementGetWindow(e: Id, wid: *mut u32) -> i32;
}
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFArrayGetCount(a: Id) -> isize;
    fn CFArrayGetValueAtIndex(a: Id, i: isize) -> Id;
    fn CFRelease(x: Id);
    fn CFRetain(x: Id) -> Id;
    fn CFEqual(a: Id, b: Id) -> bool;
    fn CFStringCreateWithCString(alloc: Id, s: *const i8, enc: u32) -> Id;
    fn CFStringGetCString(s: Id, buf: *mut i8, len: isize, enc: u32) -> bool;
    fn CFGetTypeID(x: Id) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFBooleanGetTypeID() -> usize;
    fn CFBooleanGetValue(b: Id) -> bool;
    fn CFNumberGetValue(n: Id, t: isize, out: *mut c_void) -> bool;
    static kCFBooleanTrue: Id;
    static kCFBooleanFalse: Id;
}
#[link(name = "objc")]
extern "C" {
    fn objc_getClass(n: *const i8) -> Id;
    fn sel_registerName(n: *const i8) -> Id;
    fn objc_msgSend();
    fn objc_autoreleasePoolPush() -> Id;
    fn objc_autoreleasePoolPop(p: Id);
}

const UTF8: u32 = 0x0800_0100;

struct Cf(Id);
impl Drop for Cf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) }
        }
    }
}
unsafe fn cfs(s: &str) -> Cf {
    let c = CString::new(s).unwrap_or_default();
    Cf(CFStringCreateWithCString(std::ptr::null_mut(), c.as_ptr(), UTF8))
}
unsafe fn attr(e: Id, name: &str) -> Cf {
    let k = cfs(name);
    let mut v: Id = std::ptr::null_mut();
    if AXUIElementCopyAttributeValue(e, k.0, &mut v) != 0 {
        v = std::ptr::null_mut();
    }
    Cf(v)
}
unsafe fn text(v: Id) -> Option<String> {
    if v.is_null() || CFGetTypeID(v) != CFStringGetTypeID() {
        return None;
    }
    let mut buf = vec![0i8; 512];
    if CFStringGetCString(v, buf.as_mut_ptr(), buf.len() as isize, UTF8) {
        Some(std::ffi::CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
    } else {
        None
    }
}
unsafe fn bool_attr(e: Id, name: &str) -> bool {
    let v = attr(e, name);
    !v.0.is_null() && CFGetTypeID(v.0) == CFBooleanGetTypeID() && CFBooleanGetValue(v.0)
}
unsafe fn zahl_attr(e: Id, name: &str) -> Option<i64> {
    let v = attr(e, name);
    if v.0.is_null() {
        return None;
    }
    let mut n: i64 = 0;
    // kCFNumberSInt64Type = 4
    if CFNumberGetValue(v.0, 4, &mut n as *mut i64 as *mut c_void) { Some(n) } else { None }
}
unsafe fn rahmen(w: Id) -> Option<Rect> {
    let p = attr(w, "AXPosition");
    let s = attr(w, "AXSize");
    if p.0.is_null() || s.0.is_null() {
        return None;
    }
    let mut pt = [0f64; 2];
    let mut sz = [0f64; 2];
    if AXValueGetValue(p.0, 1, pt.as_mut_ptr() as *mut c_void) && AXValueGetValue(s.0, 2, sz.as_mut_ptr() as *mut c_void) {
        Some(Rect { x: pt[0], y: pt[1], w: sz[0], h: sz[1] })
    } else {
        None
    }
}
unsafe fn aktion(e: Id, name: &str) -> bool {
    let k = cfs(name);
    AXUIElementPerformAction(e, k.0) == 0
}
/// Retained AX windows of an app (caller releases).
unsafe fn fenster(pid: i32) -> Vec<Id> {
    let app = Cf(AXUIElementCreateApplication(pid));
    if app.0.is_null() {
        return Vec::new();
    }
    let arr = attr(app.0, "AXWindows");
    if arr.0.is_null() {
        return Vec::new();
    }
    (0..CFArrayGetCount(arr.0)).map(|i| CFRetain(CFArrayGetValueAtIndex(arr.0, i))).collect()
}
unsafe fn freigeben(v: Vec<Id>) {
    for w in v {
        CFRelease(w);
    }
}
fn standard(w: Id) -> bool {
    unsafe { text(attr(w, "AXSubrole").0).as_deref() == Some("AXStandardWindow") }
}

/// Ordinary windows belonging to exactly the captured Space (pid, frame).
/// Shared/all-Spaces windows are intentionally excluded: minimizing one
/// would also change the user's other Desktops.
fn auf_dem_schreibtisch(space: u64) -> Vec<(i32, Rect)> {
    if space == 0 { return Vec::new(); }
    crate::cgs::fenster_auf_space(space).into_iter()
        .filter(|(wid, ..)| crate::cgs::spaces_des_fensters(*wid).is_some_and(|s| s.as_slice() == [space]))
        .map(|(_, pid, _, _, x, y, w, h)| (pid, Rect { x: x as f64, y: y as f64, w: w as f64, h: h as f64 }))
        .collect()
}
fn gleiche_lage(a: &Rect, b: &Rect) -> bool {
    (a.x - b.x).abs() < 4.0 && (a.y - b.y).abs() < 4.0 && (a.w - b.w).abs() < 4.0 && (a.h - b.h).abs() < 4.0
}
/// A window of this desktop the session may use or minimize.
unsafe fn nutzbar_hier(pid: i32, w: Id, alle: &[Id], schirm: &[(i32, Rect)]) -> bool {
    if !standard(w) || bool_attr(w, "AXMinimized") || bool_attr(w, "AXFullScreen") {
        return false;
    }
    let Some(r) = rahmen(w) else { return false };
    // AX doesn't expose the CGWindow ID. If same-app windows share a frame,
    // refuse to guess which AX element belongs to the target Space.
    // Chrome web apps (GitHub.app): AX through the shim pid, but WindowServer
    // lists the window under CHROME's pid (measured: window 31669).
    let partner = shim_partner(pid);
    let cg_treffer = schirm.iter().filter(|(p, s)| (*p == pid || Some(*p) == partner) && gleiche_lage(s, &r)).count();
    let ax_treffer = alle.iter().filter(|x| standard(**x) && rahmen(**x).is_some_and(|x| gleiche_lage(&x, &r))).count();
    cg_treffer == 1 && ax_treffer == 1
}

/// Chrome's pid if `pid` is a Chrome web-app shim (app_mode_loader).
fn shim_partner(pid: i32) -> Option<i32> {
    let exe = unsafe {
        let pool = objc_autoreleasePoolPush();
        let s = |n: &[u8]| sel_registerName(n.as_ptr() as *const i8);
        let m0: unsafe extern "C" fn(Id, Id) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let mp: unsafe extern "C" fn(Id, Id, i32) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let mc: unsafe extern "C" fn(Id, Id) -> *const i8 = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let app = mp(objc_getClass(b"NSRunningApplication\0".as_ptr() as *const i8), s(b"runningApplicationWithProcessIdentifier:\0"), pid);
        let name = if app.is_null() { std::ptr::null_mut() } else { m0(m0(app, s(b"executableURL\0")), s(b"lastPathComponent\0")) };
        let c = if name.is_null() { std::ptr::null() } else { mc(name, s(b"UTF8String\0")) };
        let r = if c.is_null() { String::new() } else { std::ffi::CStr::from_ptr(c).to_string_lossy().into_owned() };
        objc_autoreleasePoolPop(pool);
        r
    };
    (exe == "app_mode_loader").then(|| pid_fuer("com.google.Chrome")).flatten()
}

/// Regular running apps: (pid, bundle path, name).
fn laufende_apps() -> Vec<(i32, String, String)> {
    let mut out = Vec::new();
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let m0: unsafe extern "C" fn(Id, Id) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let mi: unsafe extern "C" fn(Id, Id) -> isize = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let mu: unsafe extern "C" fn(Id, Id, usize) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let mc: unsafe extern "C" fn(Id, Id) -> *const i8 = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let s = |n: &[u8]| sel_registerName(n.as_ptr() as *const i8);
        let ns = |o: Id| -> String {
            if o.is_null() { return String::new(); }
            let c = mc(o, s(b"UTF8String\0"));
            if c.is_null() { String::new() } else { std::ffi::CStr::from_ptr(c).to_string_lossy().into_owned() }
        };
        let ws = m0(objc_getClass(b"NSWorkspace\0".as_ptr() as *const i8), s(b"sharedWorkspace\0"));
        let arr = m0(ws, s(b"runningApplications\0"));
        let n = mi(arr, s(b"count\0"));
        for i in 0..n.max(0) as usize {
            let a = mu(arr, s(b"objectAtIndex:\0"), i);
            if a.is_null() || mi(a, s(b"activationPolicy\0")) != 0 {
                continue;
            }
            let pid = mi(a, s(b"processIdentifier\0")) as i32;
            let pfad = ns(m0(m0(a, s(b"bundleURL\0")), s(b"path\0")));
            let name = ns(m0(a, s(b"localizedName\0")));
            out.push((pid, pfad, name));
        }
        objc_autoreleasePoolPop(pool);
    }
    out
}

/// The app's own "New Window" command from its menu bar (Accessibility).
unsafe fn neues_fenster_menue(pid: i32) -> bool {
    neues_fenster_menue_mit(pid, false)
}
/// `auch_dokument`: also "New"/"New Document" - only for apps that have NO
/// window anywhere (nothing on another Space to be pulled to).
unsafe fn neues_fenster_menue_mit(pid: i32, auch_dokument: bool) -> bool {
    const TITEL: [&str; 6] = ["Neues Fenster", "New Window", "Neues Finder-Fenster", "New Finder Window", "Neues Browserfenster", "Neues Safari-Fenster"];
    const DOKUMENT: [&str; 4] = ["Neu", "New", "Neues Dokument", "New Document"];
    let app = Cf(AXUIElementCreateApplication(pid));
    if app.0.is_null() {
        return false;
    }
    let bar = attr(app.0, "AXMenuBar");
    if bar.0.is_null() {
        return false;
    }
    let kinder = attr(bar.0, "AXChildren");
    if kinder.0.is_null() {
        return false;
    }
    let mut cmd_n: Option<Id> = None;
    let mut treffer: Option<Id> = None;
    'aussen: for i in 0..CFArrayGetCount(kinder.0) {
        let top = CFArrayGetValueAtIndex(kinder.0, i);
        let menues = attr(top, "AXChildren");
        if menues.0.is_null() || CFArrayGetCount(menues.0) == 0 {
            continue;
        }
        let menue = CFArrayGetValueAtIndex(menues.0, 0);
        let items = attr(menue, "AXChildren");
        if items.0.is_null() {
            continue;
        }
        for j in 0..CFArrayGetCount(items.0) {
            let it = CFArrayGetValueAtIndex(items.0, j);
            let titel = text(attr(it, "AXTitle").0).unwrap_or_default();
            // "Neu …" (Grapher) = "Neu": the ellipsis only says a chooser follows.
            let basis = titel.trim().trim_end_matches('…').trim_end_matches("...").trim();
            if TITEL.iter().any(|t| *t == titel) || (auch_dokument && DOKUMENT.iter().any(|t| *t == basis)) {
                treffer = Some(CFRetain(it));
                break 'aussen;
            }
            let ch = text(attr(it, "AXMenuItemCmdChar").0).unwrap_or_default();
            let modi = zahl_attr(it, "AXMenuItemCmdModifiers").unwrap_or(-1);
            // Cmd+N only if its title names a WINDOW (GitHub Desktop's Cmd+N is
            // "New Repository…" - that must never be pressed).
            let fensterhaft = { let t = titel.to_lowercase(); t.contains("fenster") || t.contains("window") };
            if cmd_n.is_none() && fensterhaft && ch.eq_ignore_ascii_case("N") && modi == 0 && bool_attr(it, "AXEnabled") {
                cmd_n = Some(CFRetain(it));
            }
        }
    }
    let wahl = treffer.or(cmd_n);
    let Some(it) = wahl else { return false };
    let ok = aktion(it, "AXPress");
    CFRelease(it);
    if let Some(c) = cmd_n {
        if c != it {
            CFRelease(c);
        }
    }
    ok
}

struct Sitzung {
    id: String,
    space: u64,
    selected_apps: Vec<String>,
    prior_energy: String,
    minimiert: Vec<FensterIdentitaet>,
    erstellt: Vec<FensterIdentitaet>,
    /// Workplace profile (frames are remembered per profile) and the bundle
    /// id of each session window (window id -> bundle).
    profil: String,
    fenster_app: Vec<(u32, String)>,
}
unsafe impl Send for Sitzung {}

/// Stable identity: WindowServer id + pid are authoritative; the retained AX
/// object is only a fast handle (0 after a Noki restart). Titles are
/// deliberately absent.
struct FensterIdentitaet { pid: i32, wid: u32, ax: usize }
unsafe impl Send for FensterIdentitaet {}
impl Drop for FensterIdentitaet {
    fn drop(&mut self) {
        if self.ax != 0 { unsafe { CFRelease(self.ax as Id) } }
    }
}

unsafe fn identitaet(pid: i32, w: Id) -> Option<FensterIdentitaet> {
    let mut wid = 0u32;
    if _AXUIElementGetWindow(w, &mut wid) != 0 || wid == 0 { return None; }
    Some(FensterIdentitaet { pid, wid, ax: CFRetain(w) as usize })
}

/// Re-resolve by pid + CGWindowID if an app replaced its AX object or the
/// handle is gone (Noki restarted). Minimized / off-Space windows are not in
/// AXWindows from every Space - the remote-token route finds them by id.
/// Returned elements are retained exactly once.
unsafe fn identitaet_fenster(i: &FensterIdentitaet) -> Option<Id> {
    let direkt = i.ax as Id;
    let mut wid = 0u32;
    if !direkt.is_null() && _AXUIElementGetWindow(direkt, &mut wid) == 0 && wid == i.wid
        && !attr(direkt, "AXRole").0.is_null()
    {
        return Some(CFRetain(direkt));
    }
    let alle = fenster(i.pid);
    let (hit, rest): (Vec<Id>, Vec<Id>) = alle.into_iter().partition(|w| {
        let mut wid = 0u32;
        _AXUIElementGetWindow(*w, &mut wid) == 0 && wid == i.wid
    });
    freigeben(rest);
    let mut hit = hit.into_iter();
    let first = hit.next();
    freigeben(hit.collect());
    first.or_else(|| crate::vorschau::fernbedienung::ax_fenster_fuer(i.pid, i.wid as i64).map(|p| p as Id))
}

// ---- Session on disk ---------------------------------------------------
// REGRESSION (2026-10-03): the restore list lived only in this process. A
// Noki restart during Focus (rebuild, crash, quit) dropped it, the frontend
// then cleared `fokusAktiv`, and Beenden could never bring the minimized
// windows back. pid + CGWindowID survive a Noki restart, so they are saved.
#[derive(serde::Serialize, serde::Deserialize)]
struct Gespeichert {
    session: String,
    space: u64,
    selected_apps: Vec<String>,
    prior_energy: String,
    minimiert: Vec<(i32, u32)>,
    erstellt: Vec<(i32, u32)>,
    #[serde(default)]
    profil: String,
    #[serde(default)]
    fenster_app: Vec<(u32, String)>,
}
fn sitzungs_datei() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(std::path::Path::new(&home).join("Library/Application Support/com.noki.desktop/fokus-sitzung.json"))
}
fn sitzung_speichern(s: &Sitzung) {
    let Some(pfad) = sitzungs_datei() else { return };
    let g = Gespeichert {
        session: s.id.clone(), space: s.space, selected_apps: s.selected_apps.clone(),
        prior_energy: s.prior_energy.clone(),
        minimiert: s.minimiert.iter().map(|i| (i.pid, i.wid)).collect(),
        erstellt: s.erstellt.iter().map(|i| (i.pid, i.wid)).collect(),
        profil: s.profil.clone(), fenster_app: s.fenster_app.clone(),
    };
    let Ok(text) = serde_json::to_string(&g) else { return };
    if let Some(p) = pfad.parent() { let _ = std::fs::create_dir_all(p); }
    let tmp = pfad.with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_ok() { let _ = std::fs::rename(&tmp, &pfad); }
}
fn sitzung_datei_loeschen() {
    if let Some(p) = sitzungs_datei() { let _ = std::fs::remove_file(p); }
}
fn sitzung_von_datei() -> Option<Sitzung> {
    let text = std::fs::read_to_string(sitzungs_datei()?).ok()?;
    let g: Gespeichert = serde_json::from_str(&text).ok()?;
    let ids = |v: Vec<(i32, u32)>| v.into_iter().map(|(pid, wid)| FensterIdentitaet { pid, wid, ax: 0 }).collect();
    Some(Sitzung { id: g.session, space: g.space, selected_apps: g.selected_apps, prior_energy: g.prior_energy,
        minimiert: ids(g.minimiert), erstellt: ids(g.erstellt), profil: g.profil, fenster_app: g.fenster_app })
}

// ---- Focus window layout (per profile, persistent) ------------------------
// Saved at Beenden from the REAL final frame of each session window, applied
// once on the next Start after a short settle. Priority: the user's layout
// tool (BetterTouchTool) > saved frame > the app's own default.
static PROFIL: Mutex<String> = Mutex::new(String::new());
pub fn profil_merken(p: &str) { if let Ok(mut g) = PROFIL.lock() { *g = p.to_string(); } }
fn layout_datei() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(std::path::Path::new(&home).join("Library/Application Support/com.noki.desktop/fokus-layout.json"))
}
fn layout_laden() -> serde_json::Map<String, serde_json::Value> {
    layout_datei().and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.as_object().cloned()).unwrap_or_default()
}
fn layout_frame(profil: &str, bid: &str) -> Option<Rect> {
    let a = layout_laden();
    let f = a.get(profil)?.get(bid)?;
    let z = |k: &str| f.get(k).and_then(|v| v.as_f64());
    Some(Rect { x: z("x")?, y: z("y")?, w: z("w")?, h: z("h")? })
}
fn layout_speichern(profil: &str, rahmen: &[(String, Rect)]) {
    if rahmen.is_empty() { return; }
    let mut alle = layout_laden();
    let eintrag = alle.entry(profil.to_string()).or_insert_with(|| serde_json::json!({}));
    if !eintrag.is_object() { *eintrag = serde_json::json!({}); }
    let display = unsafe { CGMainDisplayID() };
    for (bid, r) in rahmen {
        eintrag[bid] = serde_json::json!({ "display": display, "x": r.x, "y": r.y, "w": r.w, "h": r.h });
    }
    let Some(p) = layout_datei() else { return };
    if let Ok(t) = serde_json::to_string_pretty(&serde_json::Value::Object(alle)) {
        let tmp = p.with_extension("json.tmp");
        if std::fs::write(&tmp, t).is_ok() { let _ = std::fs::rename(&tmp, &p); }
    }
}
#[repr(C)]
#[derive(Clone, Copy)]
struct CgRect { x: f64, y: f64, w: f64, h: f64 }
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGMainDisplayID() -> u32;
    fn CGDisplayBounds(d: u32) -> CgRect;
}
fn btt_laeuft() -> bool { pid_fuer("com.hegenberg.BetterTouchTool").is_some() }
unsafe fn rahmen_setzen(w: Id, r: Rect) -> bool {
    let pt = [r.x, r.y];
    let sz = [r.w, r.h];
    let pv = Cf(AXValueCreate(1, pt.as_ptr() as *const c_void));
    let sv = Cf(AXValueCreate(2, sz.as_ptr() as *const c_void));
    let a = AXUIElementSetAttributeValue(w, cfs("AXSize").0, sv.0);
    let b = AXUIElementSetAttributeValue(w, cfs("AXPosition").0, pv.0);
    a == 0 || b == 0
}
/// Keep a saved frame on the screen (display may have changed).
fn einklemmen(r: Rect) -> Rect {
    let d = unsafe { CGDisplayBounds(CGMainDisplayID()) };
    let menue = 25.0;
    let w = r.w.min(d.w).max(120.0);
    let h = r.h.min(d.h - menue).max(80.0);
    let x = r.x.max(d.x).min(d.x + d.w - w);
    let y = r.y.max(d.y + menue).min(d.y + d.h - h);
    Rect { x, y, w, h }
}
/// One session window: short settle (frame stable 250 ms, max ~1 s - no
/// permanent watch), then decide ONCE. BetterTouchTool moved it during the
/// settle -> its frame wins and is never written against. Otherwise the
/// saved frame of this profile/app is applied (clamped).
unsafe fn layout_anwenden(i: &FensterIdentitaet, bid: &str, name: &str, profil: &str, space: u64) {
    let Some(w) = identitaet_fenster(i) else { return };
    let start = rahmen(w);
    let t0 = Instant::now();
    let (mut letzt, mut stabil_seit, mut bewegt) = (start, Instant::now(), false);
    while t0.elapsed() < Duration::from_millis(1000) {
        std::thread::sleep(Duration::from_millis(50));
        let jetzt = rahmen(w);
        let gleich = match (jetzt, letzt) { (Some(a), Some(b)) => gleiche_lage(&a, &b), (None, None) => true, _ => false };
        if !gleich { bewegt = true; stabil_seit = Instant::now(); letzt = jetzt; }
        else if stabil_seit.elapsed() >= Duration::from_millis(250) { break; }
    }
    let ist = letzt.unwrap_or_default();
    let log = |grund: &str, f: Rect| crate::virtual_workspace::trace(&format!(
        "FOCUS_LAYOUT_FRAME app={name} bundle={bid} window={} space={space} frame={},{},{}x{} reason={grund}",
        i.wid, f.x as i64, f.y as i64, f.w as i64, f.h as i64));
    if bewegt {
        // Someone else placed it right after it appeared (BetterTouchTool or
        // another layout tool, or the app restoring its own frame): that
        // position wins - Noki never writes against it.
        log(if btt_laeuft() { "user_layout_tool_kept btt=1" } else { "external_layout_kept btt=0" }, ist);
    } else if let Some(ziel) = layout_frame(profil, bid).map(einklemmen) {
        if gleiche_lage(&ziel, &ist) { log("saved_already", ist); }
        else if rahmen_setzen(w, ziel) { log("restored_saved", rahmen(w).unwrap_or(ziel)); }
        else { log("restore_refused_by_app", ist); }
    } else {
        log("app_default_no_saved", ist);
    }
    CFRelease(w);
}

/// Is a focus session open (this process or saved by a previous Noki)?
/// The frontend asks at load so Beenden stays available after a restart.
pub fn status() -> serde_json::Value {
    let _aktion = SITZUNGS_AKTION.lock().unwrap_or_else(|e| e.into_inner());
    let antwort = |s: &Sitzung, quelle: &str| serde_json::json!({
        "aktiv": true, "session": s.id, "focusSpace": s.space, "selectedApps": s.selected_apps,
        "priorEnergyMode": s.prior_energy, "minimiert": s.minimiert.len(), "erstellt": s.erstellt.len(), "quelle": quelle });
    if let Some(s) = SITZUNG.lock().ok().as_ref().and_then(|g| g.as_ref().map(|s| antwort(s, "prozess"))) {
        return s;
    }
    match sitzung_von_datei() {
        Some(s) => {
            let a = antwort(&s, "datei");
            crate::virtual_workspace::trace(&format!("FOCUS_SESSION_RESUMED {a}"));
            if let Ok(mut g) = SITZUNG.lock() { *g = Some(s); }
            a
        }
        None => serde_json::json!({ "aktiv": false }),
    }
}

fn space_action(reason: &str, app: &str, window: &str, before: u64, after: u64) {
    crate::space_action_log(reason, app, window, before, after);
}
static SITZUNG: Mutex<Option<Sitzung>> = Mutex::new(None);
static SITZUNGS_AKTION: Mutex<()> = Mutex::new(());

/// Start of a focus session. `apps` = bundle paths of the selected apps.
/// Dry run: what Start WOULD do on this desktop (names only, no window is
/// touched). For checks and diagnostics.
pub fn pruefen(apps: &[String]) -> serde_json::Value {
    let eigen = std::process::id() as i32;
    let space = crate::cgs::aktiver_space().map(|(s, _)| s).unwrap_or(0);
    let schirm = auf_dem_schreibtisch(space);
    let (mut ins_dock, mut behalten, mut neues_fenster, mut starten_) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let laufend = laufende_apps();
    unsafe {
        let pool = objc_autoreleasePoolPush();
        for (pid, pfad, name) in &laufend {
            if *pid == eigen { continue; }
            let ws = fenster(*pid);
            let n = ws.iter().filter(|w| nutzbar_hier(*pid, **w, &ws, &schirm)).count();
            freigeben(ws);
            if apps.iter().any(|a| a == pfad) {
                if n > 0 { behalten.push(name.clone()); } else { neues_fenster.push(name.clone()); }
            } else if n > 0 {
                ins_dock.push(format!("{name} ({n})"));
            }
        }
        objc_autoreleasePoolPop(pool);
    }
    for a in apps {
        if !laufend.iter().any(|(_, p, _)| p == a) { starten_.push(a.rsplit('/').next().unwrap_or(a).to_string()); }
    }
    serde_json::json!({ "ins_dock": ins_dock, "behalten": behalten, "neues_fenster": neues_fenster, "starten": starten_ })
}

/// Bundle id and executable of an app bundle (from its Info.plist).
fn bundle_info(pfad: &str) -> (String, String) {
    let lies = |k: &str| std::process::Command::new("/usr/bin/plutil")
        .args(["-extract", k, "raw", "-o", "-", &format!("{pfad}/Contents/Info.plist")])
        .output().ok().filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    (lies("CFBundleIdentifier"), lies("CFBundleExecutable"))
}
/// pid of a running app by bundle id (NSRunningApplication).
fn pid_fuer(bid: &str) -> Option<i32> {
    if bid.is_empty() { return None; }
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let s = |n: &[u8]| sel_registerName(n.as_ptr() as *const i8);
        let m1: unsafe extern "C" fn(Id, Id, Id) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let mi: unsafe extern "C" fn(Id, Id) -> isize = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let mu: unsafe extern "C" fn(Id, Id, usize) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let ms: unsafe extern "C" fn(Id, Id, *const i8) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let c = CString::new(bid).unwrap_or_default();
        let ns = ms(objc_getClass(b"NSString\0".as_ptr() as *const i8), s(b"stringWithUTF8String:\0"), c.as_ptr());
        let arr = m1(objc_getClass(b"NSRunningApplication\0".as_ptr() as *const i8), s(b"runningApplicationsWithBundleIdentifier:\0"), ns);
        let r = if !arr.is_null() && mi(arr, s(b"count\0")) > 0 { Some(mi(mu(arr, s(b"objectAtIndex:\0"), 0), s(b"processIdentifier\0")) as i32) } else { None };
        objc_autoreleasePoolPop(pool);
        r
    }
}
/// Short, bounded condition wait on ONE app: a standard window of `pid` that
/// is on THIS desktop and not in `vorher`. 50 ms steps - no blind sleeps.
unsafe fn warte_hier(pid: i32, vorher: &[Id], frist: Duration, space: u64) -> Vec<Id> {
    let t0 = Instant::now();
    loop {
        let schirm = auf_dem_schreibtisch(space);
        let alle = fenster(pid);
        let gueltig: Vec<usize> = alle.iter().copied()
            .filter(|w| nutzbar_hier(pid, *w, &alle, &schirm))
            .map(|w| w as usize)
            .collect();
        let (neu, alt): (Vec<Id>, Vec<Id>) = alle.into_iter().partition(|w| {
            !vorher.iter().any(|v| CFEqual(*v, *w)) && gueltig.contains(&(*w as usize))
        });
        freigeben(alt);
        if !neu.is_empty() || t0.elapsed() > frist { return neu; }
        freigeben(neu);
        std::thread::sleep(Duration::from_millis(50));
    }
}
fn oeffnen(args: &[&str]) -> bool {
    std::process::Command::new("/usr/bin/open").args(args).status().map(|x| x.success()).unwrap_or(false)
}

/// Result of preparing ONE selected app on this desktop.
struct AppErgebnis { name: String, art: &'static str, erstellt: Vec<FensterIdentitaet>, ms: u64, grund: String }

/// Prepares one selected app HERE. Never activates an app that has windows
/// on another Space (that is what teleported the user).
/// New window WITHOUT activating the app (AppleScript, no `activate`).
/// Measured 2026-10-03: Chrome with a full-screen window on another Space
/// activates itself for menu "New Window" and for `--new-window`; macOS then
/// switches to that Space BEFORE the window exists. A scripted
/// `make new window` creates it on the ACTIVE Space with no activation.
/// Needs the one-time Automation permission (NSAppleEventsUsageDescription).
fn skript_neues_fenster(bid: &str) -> Option<&'static str> {
    match bid {
        "com.google.Chrome" | "com.google.Chrome.beta" | "com.brave.Browser" | "com.microsoft.edgemac"
        | "org.chromium.Chromium" | "com.vivaldi.Vivaldi" => Some("make new window"),
        "com.apple.Safari" => Some("make new document"),
        "com.apple.finder" => Some("make new Finder window"),
        _ => None,
    }
}
fn skript_fenster(bid: &str, space_jetzt: fn() -> u64) -> (bool, String) {
    skript_fenster_url(bid, None, space_jetzt)
}
/// `url`: open it in the new window's tab (Chromium only) - used for Chrome
/// web apps (GitHub.app), whose own window may live on another Space.
fn skript_fenster_url(bid: &str, url: Option<&str>, space_jetzt: fn() -> u64) -> (bool, String) {
    let Some(befehl) = skript_neues_fenster(bid) else { return (false, String::new()) };
    let skript = match url.filter(|u| chromium(bid) && (u.starts_with("https://") || u.starts_with("http://"))) {
        Some(u) => format!("tell application id \"{bid}\"\nset w to {befehl}\nset URL of active tab of w to \"{}\"\nend tell", u.replace('"', "%22")),
        None => format!("tell application id \"{bid}\" to {befehl}"),
    };
    let before = space_jetzt();
    let r = match std::process::Command::new("/usr/bin/osascript").args(["-e", &skript]).output() {
        Ok(o) if o.status.success() => (true, String::new()),
        Ok(o) => (false, String::from_utf8_lossy(&o.stderr).trim().to_string()),
        Err(e) => (false, e.to_string()),
    };
    space_action("focus_new_window_script", bid, "new", before, space_jetzt());
    r
}
fn plist_wert(pfad: &str, schluessel: &str) -> String {
    std::process::Command::new("/usr/bin/plutil")
        .args(["-extract", schluessel, "raw", "-o", "-", &format!("{pfad}/Contents/Info.plist")])
        .output().ok().filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}
/// Chrome web app (GitHub.app). A running shim's window HERE is reused;
/// otherwise a NEW normal Chrome window with the app's URL is made by script
/// (no activation, born on the active Space).
/// NEVER `open` a running shim: the reopen re-shows the app's old window on
/// ITS Space and Chrome activates - same mechanism as the GitHub Desktop
/// reopen that moved the user to Desktop 7 (log 2026-10-03:
/// SPACE_GUARD_VIOLATION expected=9 actual=7, frontmost=GitHub Desktop).
/// None = Chrome itself not running -> background cold launch below.
unsafe fn shim_vorbereiten(pfad: &str, bid: &str, laufend_pid: Option<i32>, space: u64, space_jetzt: fn() -> u64)
    -> Option<(&'static str, Vec<FensterIdentitaet>, String)>
{
    if let Some(shim_pid) = laufend_pid.or_else(|| pid_fuer(bid)) {
        let schirm = auf_dem_schreibtisch(space);
        let alle = fenster(shim_pid);
        let hier = alle.iter().any(|w| nutzbar_hier(shim_pid, *w, &alle, &schirm));
        freigeben(alle);
        if hier { return Some(("behalten", Vec::new(), String::new())); }
    } else if pid_fuer("com.google.Chrome").is_none() {
        return None; // Chrome not running: background cold launch below.
    }
    // Measured 2026-10-03: a cold-launched shim showed its window here
    // (32711) and quit ~1-5 s later, taking the window with it - Beenden then
    // had nothing to close. The scripted Chrome window is deterministic.
    let url = plist_wert(pfad, "CrAppModeShortcutURL");
    let Some(chrome) = pid_fuer("com.google.Chrome") else {
        return Some(("ohne_fenster", Vec::new(), "Web-App läuft, Chrome nicht erreichbar".into()));
    };
    let vorher = fenster(chrome);
    let (ok, fehler) = skript_fenster_url("com.google.Chrome", (!url.is_empty()).then_some(url.as_str()), space_jetzt);
    let neu = if ok { warte_hier(chrome, &vorher, Duration::from_millis(2500), space) } else { Vec::new() };
    freigeben(vorher);
    if neu.is_empty() {
        let grund = if fehler.contains("-1743") { "Automations-Freigabe für Chrome fehlt".to_string() } else if ok { "neues Fenster erschien nicht hier".to_string() } else { format!("Skript: {fehler}") };
        return Some(("ohne_fenster", Vec::new(), grund));
    }
    let v = neu.into_iter().filter_map(|w| { let i = identitaet(chrome, w); CFRelease(w); i }).collect();
    Some(("neu", v, String::new()))
}
/// Does `pid` own a real window on a Desktop other than `space`?
fn fenster_anderswo(pid: i32, space: u64) -> bool {
    crate::cgs::alle_fenster().iter().any(|f| f.1 == pid && f.6 >= 200 && f.7 >= 150
        && crate::cgs::spaces_des_fensters(f.0).is_some_and(|s| !s.is_empty() && !s.contains(&space)))
}
extern "C" { fn kill(pid: i32, sig: i32) -> i32; }
/// Clean quit (NSRunningApplication terminate: the app may ask to save or
/// refuse - then nothing is forced) and a background launch on THIS Desktop.
unsafe fn neu_starten_hier(pfad: &str, bid: &str, pid: i32, space: u64, space_jetzt: fn() -> u64) -> Result<Vec<FensterIdentitaet>, String> {
    let before = space_jetzt();
    // Measured 2026-10-03 (cycle 1, Desktop 1 -> 7): quitting the FRONTMOST
    // app makes macOS activate the next app in line (Terminal) and follow it
    // to its Desktop. Hand the front to Noki's character window first - it is
    // always a member of the current Desktop, so this never switches. If the
    // handoff cannot be verified, nothing is quit.
    if crate::blende::vorn_pid() == pid {
        let eigen = std::process::id() as i32;
        let noki = crate::cgs::alle_fenster().into_iter()
            .find(|f| f.1 == eigen && f.3 == "Noki Character")
            .map(|f| f.0);
        let ok = noki.is_some_and(|w| crate::window_overview::fenster_front_exakt(eigen, w));
        let frist = Instant::now() + Duration::from_millis(400);
        while ok && crate::blende::vorn_pid() == pid && Instant::now() < frist { std::thread::sleep(Duration::from_millis(20)); }
        space_action("focus_relaunch_front_handoff", bid, &format!("noki_wid={}", noki.unwrap_or(0)), before, space_jetzt());
        if crate::blende::vorn_pid() == pid {
            return Err("App ist im Vordergrund – Neustart hier würde den Schreibtisch wechseln".into());
        }
    }
    {
        let pool = objc_autoreleasePoolPush();
        let s = |n: &[u8]| sel_registerName(n.as_ptr() as *const i8);
        let mp: unsafe extern "C" fn(Id, Id, i32) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let mb: unsafe extern "C" fn(Id, Id) -> bool = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let app = mp(objc_getClass(b"NSRunningApplication\0".as_ptr() as *const i8), s(b"runningApplicationWithProcessIdentifier:\0"), pid);
        if !app.is_null() { let _ = mb(app, s(b"terminate\0")); }
        objc_autoreleasePoolPop(pool);
    }
    let t = Instant::now();
    while kill(pid, 0) == 0 && t.elapsed() < Duration::from_secs(5) { std::thread::sleep(Duration::from_millis(50)); }
    if kill(pid, 0) == 0 {
        return Err("App hat das Beenden für den Neustart hier abgelehnt (ungesicherte Änderungen?)".into());
    }
    let ok = oeffnen(&["-g", "-a", pfad, "--args", "-ApplePersistenceIgnoreState", "YES"]);
    space_action("focus_single_window_relaunch", bid, "new", before, space_jetzt());
    if !ok { return Err("Neustart fehlgeschlagen".into()); }
    let t = Instant::now();
    let mut neu_pid = None;
    while neu_pid.is_none() && t.elapsed() < Duration::from_secs(8) {
        neu_pid = pid_fuer(bid);
        if neu_pid.is_none() { std::thread::sleep(Duration::from_millis(50)); }
    }
    let Some(np) = neu_pid else { return Err("App startete nicht neu".into()) };
    let rest = Duration::from_secs(10).saturating_sub(t.elapsed());
    let neu = warte_hier(np, &[], rest, space);
    if neu.is_empty() { return Err("nach Neustart kein Fenster auf diesem Schreibtisch".into()); }
    Ok(neu.into_iter().filter_map(|w| { let i = identitaet(np, w); CFRelease(w); i }).collect())
}
/// Press "Abbrechen"/"Cancel" of a chooser: a sheet attached to `w`, or
/// `w` itself when it IS the chooser window (Grapher "Neuer Graph":
/// standard window with Auswählen/Abbrechen and no close button).
unsafe fn blatt_abbrechen(w: Id) -> bool {
    let kinder = attr(w, "AXChildren");
    if kinder.0.is_null() { return false; }
    for i in 0..CFArrayGetCount(kinder.0) {
        let k = CFArrayGetValueAtIndex(kinder.0, i);
        let t = text(attr(k, "AXTitle").0).unwrap_or_default();
        if text(attr(k, "AXRole").0).as_deref() == Some("AXButton") && matches!(t.as_str(), "Abbrechen" | "Cancel") {
            return aktion(k, "AXPress");
        }
    }
    for i in 0..CFArrayGetCount(kinder.0) {
        let k = CFArrayGetValueAtIndex(kinder.0, i);
        if text(attr(k, "AXRole").0).as_deref() != Some("AXSheet") { continue; }
        let knoepfe = attr(k, "AXChildren");
        if knoepfe.0.is_null() { continue; }
        for j in 0..CFArrayGetCount(knoepfe.0) {
            let b = CFArrayGetValueAtIndex(knoepfe.0, j);
            let t = text(attr(b, "AXTitle").0).unwrap_or_default();
            if text(attr(b, "AXRole").0).as_deref() == Some("AXButton") && matches!(t.as_str(), "Abbrechen" | "Cancel") {
                return aktion(b, "AXPress");
            }
        }
    }
    false
}
fn erfolg(art: &str) -> bool {
    !matches!(art, "fehlt" | "ohne_fenster" | "space_wechsel")
}
/// A real, visible window of this selected app on exactly `space`:
/// (window id, (x, y, w, h)). Session windows first, else any window of the
/// app's current process (relaunch changes the pid; a web app's window is
/// Chrome's).
fn sichtbar_hier(pfad: &str, r: &AppErgebnis, space: u64) -> Option<(i64, (i64, i64, i64, i64))> {
    let mut pids: Vec<i32> = laufende_apps().into_iter().filter(|(_, p, _)| p == pfad).map(|x| x.0).collect();
    if let Some(c) = pids.iter().find_map(|p| shim_partner(*p)) { pids.push(c); }
    let alle = crate::cgs::alle_fenster();
    let ok = |f: &(i64, i32, String, String, i64, i64, i64, i64, i64)| f.6 >= 120 && f.7 >= 80
        && crate::cgs::spaces_des_fensters(f.0).is_some_and(|s| s.as_slice() == [space])
        && crate::cgs::fenster_eingeordnet(f.0);
    let treffer = |f: &(i64, i32, String, String, i64, i64, i64, i64, i64)| (f.0, (f.4, f.5, f.6, f.7));
    if let Some(f) = alle.iter().find(|f| r.erstellt.iter().any(|i| i.wid as i64 == f.0) && ok(f)) {
        return Some(treffer(f));
    }
    alle.iter().find(|f| pids.contains(&f.1) && ok(f)).map(treffer)
}
fn chromium(bid: &str) -> bool {
    matches!(bid, "com.google.Chrome" | "com.google.Chrome.beta" | "com.brave.Browser" | "com.microsoft.edgemac" | "org.chromium.Chromium" | "com.vivaldi.Vivaldi")
}
fn app_vorbereiten(pfad: String, laufend_pid: Option<i32>, t0: Instant, space: u64, space_jetzt: fn() -> u64) -> AppErgebnis {
    let space0 = space_jetzt();
    if space == 0 || space0 != space {
        let name = pfad.rsplit('/').next().unwrap_or(&pfad).trim_end_matches(".app").to_string();
        return AppErgebnis { name, art: "space_wechsel", erstellt: Vec::new(), ms: t0.elapsed().as_millis() as u64, grund: format!("Fokus-Schreibtisch {space} ist nicht aktiv (aktuell {space0})") };
    }
    crate::virtual_workspace::trace(&format!("FOCUS_APP_REQUEST app={} bundle={} window=- space={space} frame=- reason={}",
        pfad.rsplit('/').next().unwrap_or(&pfad), bundle_info(&pfad).0, if laufend_pid.is_some() { "running" } else { "not_running" }));
    let mut r = app_vorbereiten_innen(pfad, laufend_pid, t0, space, space_jetzt);
    // SPACE GUARD per app: a path that moved the user is never a success.
    let space1 = space_jetzt();
    if space0 != 0 && space1 != 0 && space0 != space1 {
        crate::virtual_workspace::trace(&format!("SPACE_GUARD_VIOLATION app={} expected={space0} actual={space1}", r.name));
        r.art = "space_wechsel";
        r.grund = format!("Space-Wechsel {space0} -> {space1}");
    }
    r
}
fn app_vorbereiten_innen(pfad: String, laufend_pid: Option<i32>, t0: Instant, space: u64, space_jetzt: fn() -> u64) -> AppErgebnis {
    let name = pfad.rsplit('/').next().unwrap_or(&pfad).trim_end_matches(".app").to_string();
    let fertig = |art, erstellt, grund: &str| AppErgebnis { name: name.clone(), art, erstellt, ms: t0.elapsed().as_millis() as u64, grund: grund.to_string() };
    let (bid, exe) = bundle_info(&pfad);
    // Chrome web-app shim (e.g. GitHub.app): the window belongs to Chrome.
    let shim = exe == "app_mode_loader" || bid.starts_with("com.google.Chrome.app.");
    unsafe {
        if shim {
            if let Some(r) = shim_vorbereiten(&pfad, &bid, laufend_pid, space, space_jetzt) {
                return fertig(r.0, r.1, &r.2);
            }
        }
        let ziel_pid = if shim { pid_fuer("com.google.Chrome") } else { laufend_pid };
        if let Some(pid) = ziel_pid.filter(|_| !shim) {
            let schirm = auf_dem_schreibtisch(space);
            let alle = fenster(pid);
            let hier = alle.iter().any(|w| nutzbar_hier(pid, *w, &alle, &schirm));
            let irgendwo = alle.iter().any(|w| standard(*w));
            if hier { freigeben(alle); return fertig("behalten", Vec::new(), ""); }
            // Apps with a scripting adapter: ALWAYS the script, also when AX
            // shows no window at all (measured: Chrome's full-screen window on
            // another Space is not in AXWindows from here - "no window" then
            // led to a reopen that activates Chrome and switches Space).
            if skript_neues_fenster(&bid).is_some() {
                let vorher = alle;
                let (ok, fehler) = skript_fenster(&bid, space_jetzt);
                let neu = if ok { warte_hier(pid, &vorher, Duration::from_millis(2500), space) } else { Vec::new() };
                freigeben(vorher);
                if neu.is_empty() {
                    let grund = if fehler.contains("-1743") { "Automations-Freigabe fehlt (Systemeinstellungen › Datenschutz › Automation)".to_string() } else if ok { "neues Fenster erschien nicht hier".to_string() } else { format!("Skript: {fehler}") };
                    return fertig("ohne_fenster", Vec::new(), &grund);
                }
                // It is already visible on the captured Space. AXRaise here
                // can activate an old/full-screen window of the same app.
                let v = neu.into_iter().filter_map(|w| { let i = identitaet(pid, w); CFRelease(w); i }).collect();
                return fertig("neu", v, "");
            }
            if !irgendwo {
                // AXWindows is NOT proof that an app has no window: several
                // apps omit windows on other Spaces/full screen. The old
                // `open -g -a` path therefore reopened such an old window and
                // asynchronously switched to its Space. Ask the app for a new
                // document/window via AX only; otherwise fail explicitly.
                let vorher = alle;
                // REGRESSION 2026-10-03 (GitHub Desktop, Activity Monitor):
                // after Beenden the app keeps running with its window CLOSED
                // (hidden, still a member of this Desktop, not in AXWindows).
                // No "New Window" command, nothing "elsewhere" -> every later
                // Start failed. When the app has NO real window on another
                // Desktop, the app's own reopen (Dock click semantics, `open
                // -g`: no activation) shows its window on THIS Desktop -
                // measured: Activity Monitor 35791 back on Space 7, Terminal
                // stayed frontmost. With a window on another Desktop reopen
                // would pull the user there (the original teleport) - never.
                if !fenster_anderswo(pid, space) {
                    let before = space_jetzt();
                    let ok = oeffnen(&["-g", "-a", &pfad]);
                    space_action("focus_reopen_hidden_here", &bid, "reopen", before, space_jetzt());
                    let neu = if ok { warte_hier(pid, &vorher, Duration::from_millis(2500), space) } else { Vec::new() };
                    if !neu.is_empty() {
                        freigeben(vorher);
                        let v = neu.into_iter().filter_map(|w| { let i = identitaet(pid, w); CFRelease(w); i }).collect();
                        return fertig("neu", v, "");
                    }
                }
                let before = space_jetzt();
                let pressed = neues_fenster_menue_mit(pid, true);
                space_action("focus_new_window_menu_ax_empty", &bid, "new", before, space_jetzt());
                let neu = if pressed { warte_hier(pid, &vorher, Duration::from_millis(2500), space) } else { Vec::new() };
                freigeben(vorher);
                // Also when "New" was pressed but no STANDARD window came (Grapher's
                // "Neu …" opens a template chooser panel - measured 2026-10-03).
                if neu.is_empty() && fenster_anderswo(pid, space) {
                    // Single-window app (GitHub Desktop: Electron, no "New
                    // Window", its one window on another Desktop). Measured
                    // 2026-10-03: a foreign window cannot be moved between
                    // Spaces (CGS no-op; un-minimize returns it to ITS Space),
                    // reopen/activate pulls the USER to it. The only route
                    // without a Space switch is a clean relaunch here.
                    return match neu_starten_hier(&pfad, &bid, pid, space, space_jetzt) {
                        // Its window here belongs to the session: Beenden
                        // closes it (the app keeps running).
                        Ok(v) => fertig("neu_gestartet", v, ""),
                        Err(g) => fertig("ohne_fenster", Vec::new(), &g),
                    };
                }
                if neu.is_empty() { return fertig("ohne_fenster", Vec::new(), "kein sicherer Befehl für ein neues Fenster; vorhandenes Fenster bleibt im anderen Space"); }
                let v = neu.into_iter().filter_map(|w| { let i = identitaet(pid, w); CFRelease(w); i }).collect();
                return fertig("neu", v, "");
            }
            // Windows only on other Spaces / full screen: a NEW window here -
            // never its old window. Chromium browsers / VS Code: their own
            // binary with --new-window; others: their "New Window" command.
            let vorher = alle;
            let before = space_jetzt();
            let ok = neues_fenster_menue(pid);
            space_action("focus_new_window_menu_other_space", &bid, "new", before, space_jetzt());
            let neu = if ok { warte_hier(pid, &vorher, Duration::from_millis(2500), space) } else { Vec::new() };
            freigeben(vorher);
            if neu.is_empty() {
                return fertig("ohne_fenster", Vec::new(), if ok { "neues Fenster erschien nicht hier" } else { "kein Befehl „Neues Fenster“ - nur in anderem Space" });
            }
            let v = neu.into_iter().filter_map(|w| { let i = identitaet(pid, w); CFRelease(w); i }).collect();
            return fertig("neu", v, "");
        }
        // Not running (or a web-app shim): launch in the BACKGROUND without
        // restoring old window state (restored windows return to their old
        // Space). New windows are created on the active desktop.
        let chrome_vorher: Vec<Id> = match ziel_pid { Some(p) if shim => fenster(p), _ => Vec::new() };
        let before = space_jetzt();
        let open_ok = oeffnen(&["-g", "-a", &pfad, "--args", "-ApplePersistenceIgnoreState", "YES"]);
        space_action("focus_background_launch", &bid, "new", before, space_jetzt());
        if !open_ok {
            freigeben(chrome_vorher);
            let grund = if std::path::Path::new(&pfad).exists() { "open fehlgeschlagen" } else { "App nicht installiert" };
            return fertig("fehlt", Vec::new(), grund);
        }
        let start = Instant::now();
        let mut pid = if shim { ziel_pid } else { None };
        while pid.is_none() && start.elapsed() < Duration::from_secs(8) {
            pid = pid_fuer(if shim { "com.google.Chrome" } else { &bid });
            if pid.is_none() { std::thread::sleep(Duration::from_millis(50)); }
        }
        let Some(pid) = pid else { freigeben(chrome_vorher); return fertig("fehlt", Vec::new(), "Prozess startete nicht"); };
        // Some apps show their first window from a helper app INSIDE their
        // bundle (Minecraft: "Minecraft Updater"). Watch those pids too.
        let mut neu: Vec<Id> = Vec::new();
        let mut neu_pid = pid;
        let t_w = Instant::now();
        while neu.is_empty() && t_w.elapsed() < Duration::from_secs(8) {
            let mut pids = vec![pid];
            // A web-app window is owned by the shim process, not by Chrome
            // (measured: GitHub.app cold launch waited on Chrome -> "kein Fenster").
            if shim { pids.extend(pid_fuer(&bid).filter(|p| *p != pid)); }
            if !shim {
                let innen = format!("{}/", pfad.trim_end_matches('/'));
                pids.extend(laufende_apps().into_iter().filter(|(p, pf, _)| *p != pid && pf.starts_with(&innen)).map(|x| x.0));
            }
            for p in pids {
                let w = warte_hier(p, if p == pid { &chrome_vorher } else { &[] }, Duration::from_millis(0), space);
                if !w.is_empty() { neu = w; neu_pid = p; break; }
            }
            if neu.is_empty() { std::thread::sleep(Duration::from_millis(50)); }
        }
        let pid = neu_pid;
        freigeben(chrome_vorher);
        if neu.is_empty() {
            let anderswo = fenster(pid);
            let gibt = anderswo.iter().any(|w| standard(*w));
            freigeben(anderswo);
            return fertig("fehlt", Vec::new(), if gibt { "Fenster erschien in anderem Space" } else { "kein Fenster" });
        }
        let v = neu.into_iter().filter_map(|w| { let i = identitaet(pid, w); CFRelease(w); i }).collect();
        fertig("gestartet", v, "")
    }
}

pub fn starten(apps: &[String], prior_energy: &str, space: u64, space_jetzt: fn() -> u64) -> serde_json::Value {
    let _aktion = SITZUNGS_AKTION.lock().unwrap_or_else(|e| e.into_inner());
    let aktuell = space_jetzt();
    if space == 0 || aktuell != space {
        return serde_json::json!({ "ok": false, "error": format!("Der Fokus-Schreibtisch hat sich vor dem Start geändert ({space} → {aktuell}). Es wurde nichts verändert."), "space": space, "space_nach": aktuell });
    }
    // A running session is ended cleanly first (profile switch).
    if SITZUNG.lock().map(|g| g.is_some()).unwrap_or(false) || sitzungs_datei().is_some_and(|p| p.exists()) {
        let _ = beenden_innen();
    }
    let t0 = Instant::now();
    let eigen = std::process::id() as i32;
    let sitzung_id = format!("fs-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0));
    crate::virtual_workspace::trace(&format!("FOCUS_SESSION_START session={sitzung_id} space={space} selected_apps={:?}", apps));
    // ONE snapshot: on-screen windows of this desktop + running apps.
    let schirm = auf_dem_schreibtisch(space);
    let laufend = laufende_apps();
    // Selected apps are known BEFORE any minimize: their windows are never
    // cleanup candidates, and newly launched ones are not in the snapshot.
    let gewaehlt = |pfad: &str| apps.iter().any(|a| a == pfad);
    let ziele: Vec<(i32, String)> = laufend.iter().filter(|(p, pf, _)| *p != eigen && !gewaehlt(pf)).map(|(p, pf, _)| (*p, pf.clone())).collect();
    // A: minimize (own thread, starts immediately).
    let min = std::thread::spawn(move || {
        let mut out: Vec<FensterIdentitaet> = Vec::new();
        let mut erst: Option<u64> = None;
        unsafe {
            let pool = objc_autoreleasePoolPush();
            for (pid, _) in ziele {
                let ws = fenster(pid);
                for w in ws.iter().copied().filter(|w| nutzbar_hier(pid, *w, &ws, &schirm)) {
                    if AXUIElementSetAttributeValue(w, cfs("AXMinimized").0, kCFBooleanTrue) == 0 {
                        erst.get_or_insert(t0.elapsed().as_millis() as u64);
                        if let Some(i) = identitaet(pid, w) { out.push(i); }
                    }
                }
                freigeben(ws);
            }
            // Verify the Dock really got them (an accepted AX write is not a
            // minimize: Grapher once stayed visible -> restore reported
            // "not_minimized"). Bounded wait, one retry via the native
            // minimize button, honest log for the rest.
            let mini = |i: &FensterIdentitaet| bool_attr(i.ax as Id, "AXMinimized");
            let frist = Instant::now() + Duration::from_millis(500);
            while Instant::now() < frist && !out.iter().all(|i| mini(i)) {
                std::thread::sleep(Duration::from_millis(30));
            }
            let mut nochmal = false;
            for i in out.iter().filter(|i| !mini(i)) {
                let k = attr(i.ax as Id, "AXMinimizeButton");
                if !k.0.is_null() && aktion(k.0, "AXPress") { nochmal = true; }
            }
            let frist = Instant::now() + Duration::from_millis(if nochmal { 500 } else { 0 });
            while Instant::now() < frist && !out.iter().all(|i| mini(i)) {
                std::thread::sleep(Duration::from_millis(30));
            }
            for i in &out {
                if mini(i) {
                    crate::virtual_workspace::trace(&format!("WINDOW_MINIMIZED pid={} window={} space={space}", i.pid, i.wid));
                } else {
                    crate::virtual_workspace::trace(&format!("WINDOW_MINIMIZE_FAILED pid={} window={} space={space}", i.pid, i.wid));
                }
            }
            objc_autoreleasePoolPop(pool);
        }
        (out, erst, t0.elapsed().as_millis() as u64)
    });
    // B: every selected app in parallel - no app waits for another.
    let laeufe: Vec<_> = apps.iter().map(|a| {
        let pid = laufend.iter().find(|(_, p, _)| p == a).map(|x| x.0);
        let a = a.clone();
        std::thread::spawn(move || {
            unsafe {
                let pool = objc_autoreleasePoolPush();
                let r = app_vorbereiten(a, pid, t0, space, space_jetzt);
                objc_autoreleasePoolPop(pool);
                r
            }
        })
    }).collect();
    let (minimiert, erstes_min, min_fertig) = min.join().unwrap_or_default();
    let mut s = Sitzung {
        profil: PROFIL.lock().map(|g| if g.is_empty() { "Standard".to_string() } else { g.clone() }).unwrap_or_else(|_| "Standard".into()),
        fenster_app: Vec::new(),
        id: sitzung_id.clone(),
        space,
        selected_apps: apps.to_vec(),
        prior_energy: prior_energy.to_string(),
        minimiert,
        erstellt: Vec::new(),
    };
    let mut apps_json = Vec::new();
    let mut layout_jobs: Vec<(i32, u32, String, String)> = Vec::new();
    let mut ergebnisse: Vec<(String, AppErgebnis)> = Vec::new();
    for (pfad, h) in apps.iter().zip(laeufe) {
        if let Ok(r) = h.join() { ergebnisse.push((pfad.clone(), r)); }
    }
    // N/N rule: every selected app is verified against the REAL WindowServer
    // state (window on exactly focusSpace AND ordered in = not hidden, not
    // minimized) before Focus counts as ready. One targeted bounded wait for
    // windows still being ordered in - no fixed sleep.
    let frist = Instant::now() + Duration::from_millis(800);
    loop {
        let offen = ergebnisse.iter().any(|(pf, r)| erfolg(&r.art) && sichtbar_hier(pf, r, space).is_none());
        if !offen || Instant::now() > frist { break; }
        std::thread::sleep(Duration::from_millis(50));
    }
    for (pfad, mut r) in ergebnisse {
        let bid = bundle_info(&pfad).0;
        for w in &r.erstellt {
            crate::virtual_workspace::trace(&format!("FOCUS_APP_WINDOW_CREATED app={} bundle={bid} window={} pid={} space={space} session={sitzung_id}", r.name, w.wid, w.pid));
        }
        if erfolg(&r.art) {
            match sichtbar_hier(&pfad, &r, space) {
                Some((wid, f)) => {
                    crate::virtual_workspace::trace(&format!("FOCUS_APP_WINDOW_VERIFIED app={} bundle={bid} window={wid} space={space} frame={},{},{}x{} reason={}", r.name, f.0, f.1, f.2, f.3, r.art));
                    // Frame as the app / the user's layout tool (BetterTouchTool)
                    // placed it. Focus only reads frames - it never writes one.
                    crate::virtual_workspace::trace(&format!("FOCUS_LAYOUT_FRAME app={} bundle={bid} window={wid} space={space} frame={},{},{}x{} reason=observed_not_written", r.name, f.0, f.1, f.2, f.3));
                }
                None => {
                    r.art = "ohne_fenster";
                    r.grund = "kein sichtbares Fenster auf diesem Schreibtisch".into();
                }
            }
        }
        if !erfolg(&r.art) {
            crate::virtual_workspace::trace(&format!("FOCUS_APP_FAILED app={} bundle={bid} window=- space={space} frame=- reason={}", r.name, r.grund));
        }
        apps_json.push(serde_json::json!({ "name": r.name, "art": r.art, "ms": r.ms, "grund": r.grund }));
        for w in &r.erstellt { s.fenster_app.push((w.wid, bid.clone())); }
        if erfolg(&r.art) && !r.erstellt.is_empty() { layout_jobs.push((r.erstellt[0].pid, r.erstellt[0].wid, bid.clone(), r.name.clone())); }
        s.erstellt.extend(r.erstellt);
    }
    // Layout: all session windows settle in parallel (one bounded settle).
    let profil = s.profil.clone();
    let jobs: Vec<_> = layout_jobs.into_iter().map(|(pid, wid, bid, name)| {
        let profil = profil.clone();
        std::thread::spawn(move || unsafe {
            let pool = objc_autoreleasePoolPush();
            layout_anwenden(&FensterIdentitaet { pid, wid, ax: 0 }, &bid, &name, &profil, space);
            objc_autoreleasePoolPop(pool);
        })
    }).collect();
    for j in jobs { let _ = j.join(); }
    let space_nach = space_jetzt();
    let antwort = serde_json::json!({
        "session": sitzung_id, "space": s.space, "focusSpace": s.space,
        "selectedApps": s.selected_apps, "priorEnergyMode": s.prior_energy,
        "space_nach": space_nach, "space_wechsel": space != 0 && space_nach != 0 && space != space_nach,
        "minimiert": s.minimiert.len(), "erstellt": s.erstellt.len(),
        "apps": apps_json,
        "ms": { "erstes_minimieren": erstes_min, "minimieren_fertig": min_fertig, "bereit": t0.elapsed().as_millis() as u64 },
    });
    crate::virtual_workspace::trace(&format!("[FOKUS] start {antwort}"));
    sitzung_speichern(&s);
    if let Ok(mut g) = SITZUNG.lock() {
        *g = Some(s);
    }
    antwort
}

/// End of the focus session: close only the session's windows, then restore
/// only the windows it minimized. Nothing is forced.
pub fn beenden() -> serde_json::Value {
    let _aktion = SITZUNGS_AKTION.lock().unwrap_or_else(|e| e.into_inner());
    beenden_innen()
}

fn beenden_innen() -> serde_json::Value {
    // In-process session, else the one a previous Noki saved (restart).
    let Some(s) = SITZUNG.lock().ok().and_then(|mut g| g.take()).or_else(sitzung_von_datei) else {
        return serde_json::json!({ "geschlossen": 0, "wartet": 0, "zurueck": 0 });
    };
    let space_jetzt = || crate::cgs::aktiver_space().map(|x| x.0).unwrap_or(0);
    let space_start = space_jetzt();
    crate::virtual_workspace::trace(&format!(
        "FOCUS_SESSION_END session={} focus_space={} current_space={space_start} minimized={} created={}",
        s.id, s.space, s.minimiert.len(), s.erstellt.len()));
    let (mut geschlossen, mut wartet, mut zurueck) = (0, 0, 0);
    unsafe {
        let pool = objc_autoreleasePoolPush();
        // 1. Restore FIRST: the focus Desktop gets the user's windows back
        //    before any session window disappears.
        let mut zurueckgeholt: Vec<(i32, Id)> = Vec::new();
        for i in &s.minimiert {
            let Some(w) = identitaet_fenster(i) else {
                crate::virtual_workspace::trace(&format!(
                    "WINDOW_RESTORE_SKIPPED pid={} window={} reason=identity_missing space={}",
                    i.pid, i.wid, s.space
                ));
                continue;
            };
            // Still there AND still minimized: otherwise the user decided.
            let vorhanden = !attr(w, "AXRole").0.is_null();
            let mini = vorhanden && bool_attr(w, "AXMinimized");
            let code = if mini {
                AXUIElementSetAttributeValue(w, cfs("AXMinimized").0, kCFBooleanFalse)
            } else {
                -1
            };
            if code == 0 {
                zurueck += 1;
                crate::virtual_workspace::trace(&format!(
                    "WINDOW_RESTORED pid={} window={} space={}", i.pid, i.wid, s.space
                ));
                zurueckgeholt.push((i.pid, w));
            } else {
                let reason = if !vorhanden { "window_missing" } else if !mini { "not_minimized" } else { "ax_unminimize_failed" };
                crate::virtual_workspace::trace(&format!(
                    "WINDOW_RESTORE_SKIPPED pid={} window={} reason={} ax_error={} space={}",
                    i.pid, i.wid, reason, code, s.space
                ));
                CFRelease(w);
            }
        }
        // 2. Close exactly the session's windows. If the FRONTMOST app owns
        //    one, closing its last window here makes macOS follow that app to
        //    its other Space (Chrome -> its full-screen Space). Hand the front
        //    to a restored window on THIS Desktop first (AXRaise + AXFrontmost
        //    of an app that has a window here: no Space switch). Only while
        //    the user is on the focus Desktop.
        let vorn = crate::blende::vorn_pid();
        if s.erstellt.iter().any(|i| i.pid == vorn) {
            match zurueckgeholt.iter().find(|(p, _)| *p != vorn) {
                Some((pid, w)) if space_start == s.space => {
                    let b = space_jetzt();
                    let _ = aktion(*w, "AXRaise");
                    let app = Cf(AXUIElementCreateApplication(*pid));
                    let _ = AXUIElementSetAttributeValue(app.0, cfs("AXFrontmost").0, kCFBooleanTrue);
                    let frist = Instant::now() + Duration::from_millis(400);
                    while Instant::now() < frist && crate::blende::vorn_pid() != *pid {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    space_action("focus_end_front_handoff", &format!("pid{pid}"), &format!("from_pid{vorn}"), b, space_jetzt());
                }
                _ => crate::virtual_workspace::trace(&format!(
                    "SPACE_RISK focus_end_close_of_frontmost pid={vorn} no_handoff current_space={space_start} focus_space={}", s.space)),
            }
        }
        for (_, w) in zurueckgeholt.drain(..) { CFRelease(w); }
        // Remember the REAL final frame of every session window (whatever
        // the user or BetterTouchTool made of it) before it closes.
        let mut rahmen_liste: Vec<(String, Rect)> = Vec::new();
        for i in &s.erstellt {
            let Some(bid) = s.fenster_app.iter().find(|(w, _)| *w == i.wid).map(|x| x.1.clone()) else { continue };
            if rahmen_liste.iter().any(|(b, _)| *b == bid) { continue; }
            if !crate::cgs::spaces_des_fensters(i.wid as i64).is_some_and(|sp| sp.as_slice() == [s.space]) { continue; }
            if let Some(w) = identitaet_fenster(i) {
                if !bool_attr(w, "AXMinimized") {
                    if let Some(f) = rahmen(w) {
                        crate::virtual_workspace::trace(&format!("FOCUS_LAYOUT_FRAME app={bid} bundle={bid} window={} space={} frame={},{},{}x{} reason=saved_at_end profile={}",
                            i.wid, s.space, f.x as i64, f.y as i64, f.w as i64, f.h as i64, s.profil));
                        rahmen_liste.push((bid, f));
                    }
                }
                CFRelease(w);
            }
        }
        layout_speichern(&s.profil, &rahmen_liste);
        let mut offen: Vec<Id> = Vec::new();
        for i in &s.erstellt {
            let Some(w) = identitaet_fenster(i) else {
                crate::virtual_workspace::trace(&format!("FOCUS_CLOSE_SKIPPED pid={} window={} reason=identity_missing", i.pid, i.wid));
                continue;
            };
            let mut knopf = attr(w, "AXCloseButton");
            // A window this SESSION created can carry the app's own new-
            // document chooser sheet (Grapher "Neu …": no close button while
            // it is up - measured). Cancel exactly that sheet, then close.
            if knopf.0.is_null() || !bool_attr(knopf.0, "AXEnabled") {
                if blatt_abbrechen(w) {
                    let frist = Instant::now() + Duration::from_millis(600);
                    loop {
                        knopf = attr(w, "AXCloseButton");
                        if (!knopf.0.is_null() && bool_attr(knopf.0, "AXEnabled")) || attr(w, "AXRole").0.is_null() || Instant::now() > frist { break; }
                        std::thread::sleep(Duration::from_millis(30));
                    }
                }
            }
            let b = space_jetzt();
            if attr(w, "AXRole").0.is_null() {
                // The chooser window itself was cancelled: it is gone.
                space_action("focus_end_cancel_session_chooser", &format!("pid{}", i.pid), &i.wid.to_string(), b, space_jetzt());
                geschlossen += 1;
                CFRelease(w);
                continue;
            }
            if knopf.0.is_null() {
                crate::virtual_workspace::trace(&format!("FOCUS_CLOSE_SKIPPED pid={} window={} reason=no_close_button", i.pid, i.wid));
                CFRelease(w);
                continue;
            }
            // Judge by the result, not the AX return code: Chrome reports an
            // error for a press that DID close the window (measured: window
            // 32743 gone, geschlossen=0).
            let ok = aktion(knopf.0, "AXPress");
            space_action(if ok { "focus_end_close_session_window" } else { "focus_end_close_session_window_ax_error" },
                &format!("pid{}", i.pid), &i.wid.to_string(), b, space_jetzt());
            offen.push(w);
        }
        // Did the windows really close? A save sheet keeps them open - that
        // is the app's decision, never overridden.
        let frist = Instant::now() + Duration::from_millis(1200);
        while !offen.is_empty() && Instant::now() < frist {
            if offen.iter().all(|w| attr(*w, "AXRole").0.is_null()) { break; }
            std::thread::sleep(Duration::from_millis(40));
        }
        for w in offen.drain(..) {
            if attr(w, "AXRole").0.is_null() { geschlossen += 1; } else { wartet += 1; }
            CFRelease(w);
        }
        objc_autoreleasePoolPop(pool);
    }
    sitzung_datei_loeschen();
    let space_ende = space_jetzt();
    let antwort = serde_json::json!({ "space": s.space, "focusSpace": s.space,
        "selectedApps": s.selected_apps, "priorEnergyMode": s.prior_energy,
        "space_vorher": space_start, "space_nach": space_ende,
        "geschlossen": geschlossen, "wartet": wartet, "zurueck": zurueck });
    crate::virtual_workspace::trace(&format!("[FOKUS] ende {antwort}"));
    antwort
}
