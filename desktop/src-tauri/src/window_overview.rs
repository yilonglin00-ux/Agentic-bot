//! Shortcut 9: a quiet, reliable window overview.
//!
//! Opening this panel reads WindowServer/AX state and orders Noki's overlay
//! above all applications, including native macOS fullscreen spaces.
//! Mutating a window happens only upon explicit card / traffic-light interaction.

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager, WebviewWindow};

pub const LABEL: &str = "window-overview";
static OPEN: AtomicBool = AtomicBool::new(false);
static GENERATION: AtomicU64 = AtomicU64::new(0);
static BOUNDS: Mutex<[f64; 4]> = Mutex::new([0.0; 4]);
static OPEN_INSTANT: Mutex<Option<Instant>> = Mutex::new(None);
static LAST_TOGGLE: Mutex<Option<Instant>> = Mutex::new(None);
/// THE per-window frame record of the overview (inventory, live loop and
/// tab pictures all read and write only this): latest sharp frame, its
/// content hash, a generation that advances only on NEW content, when it
/// was last captured and when its content last changed.
///
/// The page loads pictures as raw PNG bytes from `nokibild://` (see
/// `bild_antwort`): no base64, no JSON-escaped megabytes over the IPC.
#[derive(Clone)]
struct Bild {
    png: Arc<Vec<u8>>,
    hash: u64,
    gen: u64,
    erfasst: Instant,
    geaendert: Instant,
}
static BILDER: Mutex<Option<HashMap<i64, Bild>>> = Mutex::new(None);

impl Bild {
    fn url(&self, id: i64) -> String {
        bild_url(&format!("w/{id}/{}", self.gen))
    }
}

/// URL of a picture served by `bild_antwort` (custom scheme per platform).
fn bild_url(pfad: &str) -> String {
    if cfg!(windows) { format!("http://nokibild.localhost/{pfad}") } else { format!("nokibild://localhost/{pfad}") }
}

fn bild(id: i64) -> Option<Bild> {
    BILDER.lock().ok().and_then(|g| g.as_ref().and_then(|m| m.get(&id).cloned()))
}

/// Records one capture. Returns the new generation if the content changed
/// (then `png` must be the new picture), None if it was the same content.
fn bild_merken(id: i64, png: Vec<u8>, hash: u64) -> Option<u64> {
    let mut g = BILDER.lock().ok()?;
    let m = g.get_or_insert_with(HashMap::new);
    let jetzt = Instant::now();
    if let Some(b) = m.get_mut(&id) {
        b.erfasst = jetzt;
        if b.hash == hash || png.is_empty() {
            return None;
        }
        b.png = Arc::new(png);
        b.hash = hash;
        b.gen += 1;
        b.geaendert = jetzt;
        return Some(b.gen);
    }
    if png.is_empty() {
        return None;
    }
    if m.len() > 96 {
        if let Some(alt) = m.iter().min_by_key(|(_, b)| b.erfasst).map(|(k, _)| *k) {
            m.remove(&alt);
        }
    }
    m.insert(id, Bild { png: Arc::new(png), hash, gen: 1, erfasst: jetzt, geaendert: jetzt });
    Some(1)
}

/// `nokibild://localhost/w/<wid>/<gen>` -> that window's latest frame;
/// `.../t/<wid>/<hash>` -> the remembered picture of one Chrome tab.
pub fn bild_antwort(pfad: &str) -> Option<Arc<Vec<u8>>> {
    let teile: Vec<&str> = pfad.trim_start_matches('/').split('/').collect();
    match teile.as_slice() {
        ["w", id, _gen] => bild(id.parse().ok()?).map(|b| b.png),
        ["t", id, hash] => {
            let (id, hash): (i64, u64) = (id.parse().ok()?, hash.parse().ok()?);
            let g = TAB_BILD.lock().ok()?;
            g.as_ref()?.iter().find(|((i, _), (h, _))| *i == id && *h == hash).map(|(_, (_, p))| p.clone())
        }
        _ => None,
    }
}

/// Safety bound for a runaway WindowServer list - far above any real set of
/// user windows; hitting it is logged, never silent.
const MAX_EINTRAEGE: usize = 400;
/// Windows captured synchronously per inventory (the top of the list).
const ERSTE_BILDER: usize = 24;

#[derive(serde::Serialize, Clone, Debug)]
pub struct OverviewWindow {
    pub id: i64,
    pub pid: i32,
    pub app: String,
    pub title: String,
    pub preview: String,
    pub icon: String,
    pub minimized: bool,
    /// Chrome window: the page may offer its tabs (horizontal paging).
    pub chrome: bool,
    /// Quality state of the picture: ms since the window's content last
    /// changed (0 = unknown), whether it is on the Space the user sees, and
    /// whether its app stops painting while hidden (Chromium/Electron/CEF
    /// throttle occluded windows: no new pixels exist - the card must then
    /// say "Standbild" instead of posing as live).
    pub stand_ms: u64,
    pub auf_space: bool,
    pub drosselt: bool,
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CaptureRect {
    origin: [f64; 2],
    size: [f64; 2],
}

#[cfg(target_os = "macos")]
impl CaptureRect {
    fn null() -> Self {
        Self {
            origin: [f64::INFINITY, f64::INFINITY],
            size: [0.0, 0.0],
        }
    }
}

pub fn is_open() -> bool {
    OPEN.load(Ordering::Acquire)
}

static ZEIGER_DRIN: AtomicBool = AtomicBool::new(false);
static ZEIGER_ZULETZT: Mutex<Option<Instant>> = Mutex::new(None);

/// Pointer (global points) from the event tap. Inside the panel: its local
/// position at most every 25 ms; leaving it: one final "outside".
pub fn zeiger(app: &tauri::AppHandle, x: f64, y: f64) {
    if !is_open() {
        return;
    }
    let b = BOUNDS.lock().map(|g| *g).unwrap_or([0.0; 4]);
    let drin = b[2] > 0.0 && x >= b[0] && y >= b[1] && x < b[0] + b[2] && y < b[1] + b[3];
    let war = ZEIGER_DRIN.swap(drin, Ordering::Relaxed);
    if !drin {
        if war {
            let _ = app.emit_to(LABEL, "noki://overview-pointer", serde_json::json!({ "x": -1, "y": -1 }));
        }
        return;
    }
    if let Ok(mut g) = ZEIGER_ZULETZT.lock() {
        if war && g.is_some_and(|t| t.elapsed() < Duration::from_millis(25)) {
            return;
        }
        *g = Some(Instant::now());
    }
    let _ = app.emit_to(LABEL, "noki://overview-pointer", serde_json::json!({ "x": x - b[0], "y": y - b[1] }));
}

/// Scroll event from the event tap over the panel: point deltas (dx =
/// horizontal axis 2, dy = vertical axis 1), scroll phase (1 began,
/// 2 changed, 4 ended, 8 cancelled, 128 may-begin) and momentum phase
/// (1 begin, 2 continue, 3 end). Panel-local position.
pub fn rad(app: &tauri::AppHandle, x: f64, y: f64, dx: i64, dy: i64, phase: i64, schwung: i64) -> bool {
    if !is_open() {
        return false;
    }
    let b = BOUNDS.lock().map(|g| *g).unwrap_or([0.0; 4]);
    if !(b[2] > 0.0 && x >= b[0] && y >= b[1] && x < b[0] + b[2] && y < b[1] + b[3]) {
        return false;
    }
    let _ = app.emit_to(LABEL, "noki://overview-wheel", serde_json::json!({
        "x": x - b[0], "y": y - b[1], "dx": dx, "dy": dy, "phase": phase, "momentum": schwung
    }));
    true
}

pub fn contains(x: f64, y: f64) -> bool {
    BOUNDS.lock().is_ok_and(|b| {
        x >= b[0] - 2.0 && y >= b[1] - 2.0 && x <= b[0] + b[2] + 2.0 && y <= b[1] + b[3] + 2.0
    })
}

pub fn close(app: &tauri::AppHandle) {
    let was_open = OPEN.swap(false, Ordering::AcqRel);
    if let Ok(mut b) = BOUNDS.lock() {
        *b = [0.0; 4];
    }
    if let Ok(mut o) = OPEN_INSTANT.lock() {
        *o = None;
    }
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.emit("noki://overview-close", serde_json::json!({}));
        let _ = w.hide();
    }
    if was_open {
        let _ = app.emit("noki://overview-calm", serde_json::json!({ "active": false }));
    }
}

#[cfg(target_os = "macos")]
fn setup_window_macos(win: &WebviewWindow) {
    use std::ffi::c_void;
    #[link(name = "objc")]
    extern "C" {
        fn sel_registerName(n: *const std::os::raw::c_char) -> *const c_void;
        fn objc_msgSend();
    }
    if let Ok(nw) = win.ns_window() {
        unsafe {
            let f = objc_msgSend as unsafe extern "C" fn();
            let sel = |n: &[u8]| sel_registerName(n.as_ptr() as *const _);
            let get: unsafe extern "C" fn(*mut c_void, *const c_void) -> usize =
                std::mem::transmute(f);
            let set: unsafe extern "C" fn(*mut c_void, *const c_void, usize) =
                std::mem::transmute(f);
            let set_lvl: unsafe extern "C" fn(*mut c_void, *const c_void, i64) =
                std::mem::transmute(f);

            // NSWindowCollectionBehaviorFullScreenAuxiliary (1 << 8) only.
            // CanJoinAllSpaces (1 << 0) + MoveToActiveSpace (1 << 1) together
            // throw NSInternalInconsistencyException; Rust cannot catch it and
            // the WHOLE Noki process aborted on Shortcut 9 ("Noki verschwindet").
            // CanJoinAllSpaces alone never reaches native fullscreen Spaces
            // (sticky windows ignore CGS membership) - `toggle` moves the
            // panel into exactly the active Space instead.
            let alt = get(nw, sel(b"collectionBehavior\0"));
            set(nw, sel(b"setCollectionBehavior:\0"), panel_behavior(alt));

            // Clicks must not ACTIVATE Noki: in native fullscreen, activating
            // Noki made macOS leave the fullscreen Space (measured 7709 -> 1
            // within 500 ms), so a red/yellow/green press there hit nothing.
            let responds: unsafe extern "C" fn(*mut c_void, *const c_void, *const c_void) -> bool =
                std::mem::transmute(f);
            let prevent = sel(b"_setPreventsActivation:\0");
            if responds(nw, sel(b"respondsToSelector:\0"), prevent) {
                let set_bool: unsafe extern "C" fn(*mut c_void, *const c_void, bool) = std::mem::transmute(f);
                set_bool(nw, prevent, true);
            }

            // Level 3 (floating): above every normal window and native
            // fullscreen content, BELOW the Miniatur (4) and Noki's
            // character (5) - Noki is never covered by the panel.
            set_lvl(nw, sel(b"setLevel:\0"), PANEL_EBENE);
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn setup_window_macos(_: &WebviewWindow) {}

/// Panel window level: floating (3). Noki's character window is 5 and the
/// Miniatur 4 (compact and LARGE), so both always lie above the panel.
const PANEL_EBENE: i64 = 3;

/// Re-asserts the panel level after Tauri's async always-on-top/order-front.
#[cfg(target_os = "macos")]
fn ebene_ueber_noki(win: &WebviewWindow) {
    use std::ffi::c_void;
    #[link(name = "objc")]
    extern "C" {
        fn sel_registerName(n: *const std::os::raw::c_char) -> *const c_void;
        fn objc_msgSend();
    }
    if let Ok(nw) = win.ns_window() {
        unsafe {
            let get: unsafe extern "C" fn(*mut c_void, *const c_void) -> i64 =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let set: unsafe extern "C" fn(*mut c_void, *const c_void, i64) =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let soll = PANEL_EBENE;
            if get(nw, sel_registerName(b"level\0".as_ptr() as *const _)) != soll {
                set(nw, sel_registerName(b"setLevel:\0".as_ptr() as *const _), soll);
            }
        }
    }
}

const CAN_JOIN_ALL_SPACES: usize = 1 << 0;
const MOVE_TO_ACTIVE_SPACE: usize = 1 << 1;
const FULLSCREEN_AUXILIARY: usize = 1 << 8;
const STATIONARY: usize = 1 << 4;

/// Collection behavior of the overview panel, derived from whatever the
/// window had. Never both CanJoinAllSpaces and MoveToActiveSpace (AppKit
/// exception -> process abort), and neither is needed: `toggle` places the
/// panel on exactly the active Space.
fn panel_behavior(alt: usize) -> usize {
    (alt & !(CAN_JOIN_ALL_SPACES | MOVE_TO_ACTIVE_SPACE)) | FULLSCREEN_AUXILIARY
}

/// Normal Desktops: the panel is on EVERY Desktop at once (sticky), so it
/// is already there when a Space swipe ends - no poll, no move, no late
/// appearance. MoveToActiveSpace stays cleared (combined -> abort).
fn panel_behavior_alle(alt: usize) -> usize {
    // Stationary (1 << 4): without it macOS slides an all-Spaces window
    // WITH the Desktop animation - the panel slid out with the old Desktop
    // and slid in with the new one. Stationary windows stay put: the first
    // frame on the new Desktop is the final geometry.
    (alt & !MOVE_TO_ACTIVE_SPACE) | CAN_JOIN_ALL_SPACES | STATIONARY | FULLSCREEN_AUXILIARY
}

/// Puts the panel on the Space the user stands on. Desktop: sticky on all
/// Desktops (first leaving any fullscreen Space it had joined). Native
/// fullscreen: sticky windows never enter it, so sticky off and CGS
/// membership in exactly that Space (verified by `space_halten`).
#[cfg(target_os = "macos")]
fn panel_verankern(app: &tauri::AppHandle, w: &WebviewWindow, wid: i64, sid: u64, typ: i32, nachher: bool) {
    // Screen-fixed: the panel lives in Noki's own overlay space, which is
    // no Desktop - the Space slide never carries it (a sticky+stationary
    // window WAS carried: CG frame x 7 -> -1527 during a swipe, then back
    // to 7 on the new Desktop = the visible slide-out/slide-in). Shown over
    // every Desktop and native fullscreen Space, so a Space change needs no
    // move, no re-order and no re-attachment at all.
    if let Some(o) = super::cgs::overlay_space() {
        // Sticky windows ignore CGS membership - never sticky here.
        behavior_setzen(w, panel_behavior);
        if panel_ins_overlay(wid, o) {
            if nachher {
                space_halten(app, wid, o, TYP_OVERLAY);
            }
            return;
        }
    }
    let voll = typ == 4;
    behavior_setzen(w, |alt| panel_behavior(alt));
    if voll {
        panel_umziehen(w, wid, sid, typ);
        if nachher {
            space_halten(app, wid, sid, typ);
        }
    } else {
        panel_umziehen(w, wid, sid, typ);
        behavior_setzen(w, panel_behavior_alle);
        if nachher {
            space_halten(app, wid, sid, typ);
        }
    }
}

/// `space_halten` type marker for the overlay space.
const TYP_OVERLAY: i32 = -1;

/// Membership in exactly the overlay space. Managed Spaces are left only
/// once the overlay really holds the window (else it would be on none).
#[cfg(target_os = "macos")]
fn panel_ins_overlay(wid: i64, o: u64) -> bool {
    use super::cgs;
    let mut jetzt = cgs::spaces_inkl_overlay(wid).unwrap_or_default();
    if jetzt == [o] {
        return true;
    }
    if !jetzt.contains(&o) {
        cgs::hinzufuegen(wid, o);
        jetzt = cgs::spaces_inkl_overlay(wid).unwrap_or_default();
        if !jetzt.contains(&o) {
            eprintln!("[OVERVIEW] panel could not join overlay space={o}");
            return false;
        }
    }
    let andere: Vec<u64> = jetzt.into_iter().filter(|s| *s != o).collect();
    if !andere.is_empty() {
        cgs::entfernen(wid, &andere);
    }
    true
}

#[cfg(target_os = "macos")]
fn behavior_setzen(w: &WebviewWindow, f: impl Fn(usize) -> usize) {
    use std::ffi::c_void;
    #[link(name = "objc")]
    extern "C" {
        fn sel_registerName(n: *const std::os::raw::c_char) -> *const c_void;
        fn objc_msgSend();
    }
    let Ok(nw) = w.ns_window() else { return };
    unsafe {
        let m = objc_msgSend as unsafe extern "C" fn();
        let get: unsafe extern "C" fn(*mut c_void, *const c_void) -> usize = std::mem::transmute(m);
        let set: unsafe extern "C" fn(*mut c_void, *const c_void, usize) = std::mem::transmute(m);
        let alt = get(nw, sel_registerName(b"collectionBehavior\0".as_ptr() as *const _));
        let neu = f(alt);
        if neu != alt {
            set(nw, sel_registerName(b"setCollectionBehavior:\0".as_ptr() as *const _), neu);
        }
    }
}

/// Membership in exactly the active Space (own window: CGS add/remove
/// works). Unlike Noki's `space_umziehen` this never applies Noki's
/// front/back layer preference to the panel.
#[cfg(target_os = "macos")]
fn panel_umziehen(w: &WebviewWindow, wid: i64, sid: u64, typ: i32) {
    // Order matters (measured): every order-front of this window parks it
    // on the fullscreen Space's parent Desktop, so the CGS membership is
    // set AFTER the last order-front and nothing re-orders it afterwards.
    super::vollbild_bit(w, typ == 4);
    super::cgs::hinzufuegen(wid, sid);
    let drin = super::cgs::spaces_des_fensters(wid).unwrap_or_default().contains(&sid);
    // Leave the other Spaces only once the target Space really holds it -
    // otherwise the panel would end up on no Space at all.
    if drin {
        let andere: Vec<u64> = super::cgs::spaces_des_fensters(wid)
            .unwrap_or_default()
            .into_iter()
            .filter(|s| *s != sid)
            .collect();
        if !andere.is_empty() {
            super::cgs::entfernen(wid, &andere);
        }
    } else {
        eprintln!("[OVERVIEW] panel could not join space={sid} typ={typ}");
    }
}

fn panel(app: &tauri::AppHandle) -> Option<WebviewWindow> {
    if let Some(w) = app.get_webview_window(LABEL) {
        return Some(w);
    }
    let w = tauri::WebviewWindowBuilder::new(
        app,
        LABEL,
        tauri::WebviewUrl::App("window-overview.html".into()),
    )
    .title("Noki Fenster")
    .inner_size(379.0, 640.0)
    .resizable(false)
    .decorations(false)
    .transparent(true)
    // No window shadow: on a transparent window macOS drew it around the
    // whole window rect - the large, faint outer layer around the panel.
    .shadow(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .visible(false)
    .focused(false)
    // Cards and traffic lights must act on the FIRST click; without this
    // the first click only activated Noki (yellow needed two clicks).
    .accept_first_mouse(true)
    .build()
    .ok()?;

    let h = app.clone();
    w.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            OPEN.store(false, Ordering::Release);
            if let Ok(mut b) = BOUNDS.lock() {
                *b = [0.0; 4];
            }
            if let Ok(mut o) = OPEN_INSTANT.lock() {
                *o = None;
            }
            let _ = h.emit("noki://overview-calm", serde_json::json!({ "active": false }));
        }
    });
    Some(w)
}

pub fn toggle(app: &tauri::AppHandle) {
    let now = Instant::now();
    if let Ok(mut last) = LAST_TOGGLE.lock() {
        if let Some(prev) = *last {
            if now.duration_since(prev) < Duration::from_millis(250) {
                return;
            }
        }
        *last = Some(now);
    }

    if is_open() {
        close(app);
        return;
    }

    if OPEN.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return;
    }

    let Some(w) = panel(app) else {
        OPEN.store(false, Ordering::Release);
        return;
    };

    let spalte = place(app, &w, LETZTE_ANZAHL.load(Ordering::Relaxed));
    // always_on_top first: it resets the NSWindow level, which
    // setup_window_macos then raises to 25 (above native fullscreen).
    let _ = w.set_always_on_top(true);
    let _ = w.set_shadow(false);
    setup_window_macos(&w);

    // One window, one Space: the panel lives exactly on the Space the user
    // stands on (normal Desktop or native fullscreen), like Noki's overlay.
    // On a Space change it follows (`space_gewechselt`); it never closes
    // unless the user closes it.
    #[cfg(target_os = "macos")]
    let ziel = super::fenster_nummer(&w).zip(super::cgs::aktiver_space());
    #[cfg(target_os = "macos")]
    if let Some((wid, (sid, typ))) = ziel {
        panel_verankern(app, &w, wid, sid, typ, false);
    }

    if w.show().is_err() {
        close(app);
        return;
    }
    super::nach_vorn_holen(&w);
    // Last step, after every order-front: joining a fullscreen Space needs
    // the re-order of the VISIBLE window (an earlier join was undone by
    // orderFrontRegardless, which parked the panel on Desktop 1).
    #[cfg(target_os = "macos")]
    if let Some((wid, (sid, typ))) = ziel {
        panel_verankern(app, &w, wid, sid, typ, true);
    }
    if let Ok(mut o) = OPEN_INSTANT.lock() {
        *o = Some(now);
    }
    // Shortcut 9 does not touch Noki's movement (no CALM_ANCHORED lease).
    let _ = w.emit("noki://overview-open", serde_json::json!({ "spalte": spalte }));
}

/// Keeps the visible panel on exactly `sid` and above Noki after Tauri's
/// ASYNC order-front/always-on-top (which park a window on a fullscreen
/// Space's parent Desktop and reset its level to Noki's layer 5).
/// Bounded, verified retries; a newer open/follow supersedes it.
#[cfg(target_os = "macos")]
fn space_halten(app: &tauri::AppHandle, wid: i64, sid: u64, typ: i32) {
    let h = app.clone();
    let gen = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    std::thread::spawn(move || {
        for warte in [25u64, 60, 120, 250] {
            std::thread::sleep(Duration::from_millis(warte));
            if !is_open() || GENERATION.load(Ordering::Relaxed) != gen {
                return;
            }
            let hl = h.clone();
            let _ = h.run_on_main_thread(move || {
                if let Some(w) = hl.get_webview_window(LABEL) {
                    ebene_ueber_noki(&w);
                }
            });
            let hh = h.clone();
            if typ == TYP_OVERLAY {
                if super::cgs::spaces_inkl_overlay(wid).unwrap_or_default() != vec![sid] {
                    let _ = h.run_on_main_thread(move || {
                        if hh.get_webview_window(LABEL).is_some() {
                            panel_ins_overlay(wid, sid);
                        }
                    });
                }
                continue;
            }
            if typ != 4 {
                // Desktop: sticky. Re-assert (Tauri's async show may reset it).
                let _ = h.run_on_main_thread(move || {
                    if let Some(w) = hh.get_webview_window(LABEL) {
                        behavior_setzen(&w, panel_behavior_alle);
                    }
                });
                continue;
            }
            if super::cgs::spaces_des_fensters(wid).unwrap_or_default() == vec![sid] {
                continue;
            }
            let _ = h.run_on_main_thread(move || {
                if let Some(w) = hh.get_webview_window(LABEL) {
                    panel_umziehen(&w, wid, sid, typ);
                }
            });
        }
        eprintln!("[OVERVIEW] space={sid} typ={typ} panel_spaces={:?}", super::cgs::spaces_des_fensters(wid));
    });
}

/// Mission Control closed: its scale transform stays on Noki's overlay
/// space (the Dock only resets managed Spaces). Fresh space, panel moved
/// over - an open panel therefore never stays shrunk/shifted.
#[cfg(target_os = "macos")]
pub fn nach_mission_control(app: &tauri::AppHandle) {
    let wid = if is_open() {
        app.get_webview_window(LABEL).and_then(|w| super::fenster_nummer(&w))
    } else {
        None
    };
    let neu = super::cgs::overlay_space_erneuern(wid);
    eprintln!("[OVERVIEW] overlay space renewed after mission control -> {neu:?} panel={wid:?}");
}

/// Double-Control restore: the (still open) panel comes back on the Space
/// the user stands on, anchored exactly like a Space follow.
pub fn global_zeigen(app: &tauri::AppHandle) {
    if !is_open() {
        return;
    }
    let Some(w) = app.get_webview_window(LABEL) else { return };
    let _ = w.show();
    space_gewechselt(app);
}

/// The user changed Space while the overview is open. It FOLLOWS: same
/// column (re-laid out for this Space's Miniatur), fresh inventory. It is
/// only ever closed by the user (Shortcut 9, Esc).
pub fn space_gewechselt(app: &tauri::AppHandle) {
    // Globally hidden (Double-Control): stays hidden; restore re-anchors.
    if !is_open() || super::global_verborgen() {
        return;
    }
    let Some(w) = app.get_webview_window(LABEL) else { return };
    let spalte = place(app, &w, LETZTE_ANZAHL.load(Ordering::Relaxed));
    #[cfg(target_os = "macos")]
    if let Some((wid, (sid, typ))) = super::fenster_nummer(&w).zip(super::cgs::aktiver_space()) {
        let im_overlay = super::cgs::overlay_space()
            .is_some_and(|o| super::cgs::spaces_inkl_overlay(wid).unwrap_or_default() == [o]);
        if im_overlay {
            // Already on screen above every Space: nothing to attach, move
            // or re-order. `place` above is idempotent (same rect -> no-op).
        } else if typ == 4 {
            panel_verankern(app, &w, wid, sid, typ, false);
            super::nach_vorn_holen(&w);
            panel_verankern(app, &w, wid, sid, typ, true);
        } else {
            // Already here (sticky on every Desktop) - never re-ordered, so
            // it neither blinks nor waits; only leaves a fullscreen Space.
            panel_verankern(app, &w, wid, sid, typ, false);
        }
        eprintln!("[OVERVIEW] follow space={sid} typ={typ} ms_since_open={:?}",
            OPEN_INSTANT.lock().ok().and_then(|o| o.map(|t| t.elapsed().as_millis())));
    }
    let _ = w.emit("noki://overview-refresh", serde_json::json!({ "spalte": spalte }));
}

/// While the Miniatur is LARGE (hover) it lies above the overview; back to
/// the panel's own level when it is compact again. Nothing closes or moves.
static MINIATUR_GROSS: AtomicBool = AtomicBool::new(false);
pub fn miniatur_gross(app: &tauri::AppHandle, gross: bool) {
    MINIATUR_GROSS.store(gross, Ordering::Relaxed);
    if !is_open() {
        return;
    }
    let h = app.clone();
    let _ = app.run_on_main_thread(move || {
        #[cfg(target_os = "macos")]
        if let Some(w) = h.get_webview_window(LABEL) {
            ebene_ueber_noki(&w);
        }
        let _ = &h;
    });
}

/// Column geometry, shared with the panel page (it sizes the cards).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct Column {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    /// Height for three cards; `panel_h(n)` shrinks it for fewer windows.
    pub h: f64,
    pub card_w: f64,
    pub card_h: f64,
}

// Panel metrics (points). Must match window-overview.html.
const RAND: f64 = 8.0; // distance to screen edges
const MINI_ABSTAND: f64 = 10.0; // distance to the Miniatur
const PAD: f64 = 6.0; // panel inner padding
const KOPF: f64 = 24.0; // "Offene Fenster" header
const LUECKE: f64 = 6.0; // gap between cards
const RINNE: f64 = 4.0; // scrollbar gutter right of the cards
const KARTE_RAHMEN: f64 = 26.0; // single name row under the preview
const KARTE_MIN: f64 = KARTE_RAHMEN + 64.0;
const MINIATUR_BREITE_STANDARD: f64 = 353.0;

impl Column {
    fn chrome() -> f64 { KOPF + 2.0 * PAD }
    /// Panel height for `n` windows: 1-3 cards fully visible, more scroll.
    pub fn panel_h(&self, n: usize) -> f64 {
        let k = n.clamp(1, 3) as f64;
        Self::chrome() + k * self.card_h + (k - 1.0) * LUECKE
    }
}

static SPALTE: Mutex<Option<Column>> = Mutex::new(None);
/// Usable area of the last NORMAL Desktop (points, monitor-relative).
static NORMAL_FLAECHE: Mutex<Option<[f64; 4]>> = Mutex::new(None);

/// Left vertical column. `work` = usable screen area (x, y, w, h) and
/// `mini` = compact Miniatur rect, both in the same point space. The PANEL
/// (outer outline incl. padding) is exactly as wide as the Miniatur; card
/// height shrinks so that three always fit. Never intersects the Miniatur.
pub(crate) fn column_layout(work: [f64; 4], mini: Option<[f64; 4]>, panel_w: f64) -> Column {
    // Same left edge as a left-docked Miniatur: one visual column.
    let links_x = mini.filter(|m| m[2] >= 2.0 && m[0] < work[0] + 60.0)
        .map_or(work[0] + RAND, |m| m[0]);
    column_layout_ab(work, mini, panel_w, links_x)
}

fn column_layout_ab(work: [f64; 4], mini: Option<[f64; 4]>, panel_w: f64, x_start: f64) -> Column {
    let [wx, wy, ww, wh] = work;
    let w = panel_w.clamp(240.0, (ww - 2.0 * RAND).max(240.0));
    let card_w = w - 2.0 * PAD - RINNE;
    let bevorzugt = KARTE_RAHMEN + (card_w * 0.625).round();
    let noetig_min = Column::chrome() + 3.0 * KARTE_MIN + 2.0 * LUECKE;
    let (oben, unten) = (wy + RAND, wy + wh - RAND);
    let mut x = x_start;
    let mut seg = (oben, unten);
    if let Some([mx, my, mw, mh]) = mini.filter(|m| m[2] >= 2.0 && m[3] >= 2.0) {
        let spalte_trifft = mx < x + w && mx + mw > x;
        if spalte_trifft {
            let ueber = (oben, (my - MINI_ABSTAND).min(unten));
            let unter = ((my + mh + MINI_ABSTAND).max(oben), unten);
            seg = if ueber.1 - ueber.0 >= unter.1 - unter.0 { ueber } else { unter };
            if seg.1 - seg.0 < noetig_min {
                // Not enough height beside it: smallest offset that keeps
                // the column left, just right of the Miniatur.
                x = mx + mw + MINI_ABSTAND;
                seg = (oben, unten);
            }
        }
    }
    x = x.min(wx + ww - RAND - w).max(wx);
    let verfuegbar = seg.1 - seg.0;
    // Three cards always fit; never taller than the Miniatur-like aspect.
    let card_h = ((verfuegbar - Column::chrome() - 2.0 * LUECKE) / 3.0)
        .min(bevorzugt)
        .max(40.0)
        .floor();
    let h = Column::chrome() + 3.0 * card_h + 2.0 * LUECKE;
    Column { x, y: seg.0, w, h, card_w, card_h }
}

fn place(app: &tauri::AppHandle, w: &WebviewWindow, n: usize) -> Option<Column> {
    let monitor = app
        .get_webview_window(super::FENSTER)
        .and_then(|m| m.current_monitor().ok().flatten())
        .or_else(|| w.primary_monitor().ok().flatten())?;
    let scale = monitor.scale_factor();
    if let Ok(mut k) = PANEL_SKALA.lock() {
        *k = scale;
    }
    let work = monitor.work_area();
    let mpos = monitor.position();
    // Everything in logical points relative to the monitor's top-left,
    // recomputed from scratch every time (never from the previous frame).
    // The usable area of a NORMAL Desktop is authoritative: in native
    // fullscreen the menu bar/Dock insets vanish and the panel used to
    // grow and move up; there the cached normal-Desktop area is reused.
    let jetzt_pt = [
        (work.position.x - mpos.x) as f64 / scale,
        (work.position.y - mpos.y) as f64 / scale,
        work.size.width as f64 / scale,
        work.size.height as f64 / scale,
    ];
    #[cfg(target_os = "macos")]
    let normal = super::cgs::aktiver_space().map_or(true, |(_, typ)| typ != 4);
    #[cfg(not(target_os = "macos"))]
    let normal = true;
    let work_pt = {
        let mut g = NORMAL_FLAECHE.lock().unwrap_or_else(|e| e.into_inner());
        let groesse_gleich = g.is_some_and(|a| (a[2] - jetzt_pt[2]).abs() < 1.0);
        if normal || !groesse_gleich {
            *g = Some(jetzt_pt);
            jetzt_pt
        } else {
            g.unwrap_or(jetzt_pt)
        }
    };
    #[cfg(target_os = "macos")]
    let (mini, breite) = {
        let r = super::vorschau::kompakt_rahmen();
        let b = super::vorschau::kompakt_breite();
        (
            (r[2] >= 2).then(|| [r[0] as f64, r[1] as f64, r[2] as f64, r[3] as f64]),
            if b >= 2 { b as f64 } else { MINIATUR_BREITE_STANDARD },
        )
    };
    #[cfg(not(target_os = "macos"))]
    let (mini, breite) = (None, MINIATUR_BREITE_STANDARD);
    // Canonical size on EVERY open: the column always reserves the compact
    // Miniatur, also when it is hidden or was never shown (Shortcut 4 may
    // show it at any moment). Last resort without any known frame: the
    // standard compact frame bottom-left (measured 7,680 353x254 on a
    // 1470x956 display = 22 pt above the work area's bottom).
    let mini = mini.or_else(|| Some([work_pt[0] + 7.0, work_pt[1] + work_pt[3] - 22.0 - 254.0, MINIATUR_BREITE_STANDARD, 254.0]));
    // Fixed left column: independent of where Noki stands (following him
    // was the "panel drifts to the right after a swipe").
    let col = column_layout(work_pt, mini, breite);
    if let Ok(mut g) = SPALTE.lock() {
        *g = Some(col);
    }
    groesse_setzen(w, &col, n, scale, [mpos.x as f64, mpos.y as f64]);
    Some(col)
}

/// The ONLY place that sets the panel's native frame. Pure function of the
/// column (display work area + Miniatur + metrics); an unchanged rect is
/// not re-applied, so a Space change never nudges the window.
fn groesse_setzen(w: &WebviewWindow, col: &Column, n: usize, scale: f64, mpos: [f64; 2]) {
    let h = col.panel_h(n);
    let ziel = [mpos[0] / scale + col.x, mpos[1] / scale + col.y, col.w, h];
    if BOUNDS.lock().is_ok_and(|b| *b == ziel) {
        return;
    }
    let _ = w.set_size(tauri::LogicalSize::new(col.w, h));
    let _ = w.set_position(tauri::PhysicalPosition::new(
        (mpos[0] + col.x * scale).round() as i32,
        (mpos[1] + col.y * scale).round() as i32,
    ));
    if let Ok(mut b) = BOUNDS.lock() {
        *b = [mpos[0] / scale + col.x, mpos[1] / scale + col.y, col.w, h];
    }
    super::virtual_workspace::trace(&format!(
        "[OVERVIEW] geometry x={} y={} w={} h={} card={}x{}", col.x, col.y, col.w, h, col.card_w, col.card_h));
}

/// The page reports how many cards it rendered: 1-3 cards are shown in
/// full without an empty tail; more scroll inside the same height.
#[tauri::command]
pub fn window_overview_fit(app: tauri::AppHandle, n: usize) {
    LETZTE_ANZAHL.store(n, Ordering::Relaxed);
    let Some(w) = app.get_webview_window(LABEL) else { return };
    let Some(col) = SPALTE.lock().ok().and_then(|g| *g) else { return };
    let Some(m) = w.current_monitor().ok().flatten() else { return };
    let p = m.position();
    groesse_setzen(&w, &col, n, m.scale_factor(), [p.x as f64, p.y as f64]);
}

/// Current column geometry (the page may load after the open event).
#[tauri::command]
pub fn window_overview_geometry() -> Option<Column> {
    SPALTE.lock().ok().and_then(|g| *g)
}

static LETZTE_ANZAHL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(3);

#[tauri::command]
pub async fn window_overview_list(voll: Option<bool>) -> Vec<OverviewWindow> {
    let voll = voll.unwrap_or(true);
    tauri::async_runtime::spawn_blocking(move || list(voll)).await.unwrap_or_default()
}

#[tauri::command]
pub fn window_overview_close(app: tauri::AppHandle) {
    close(&app);
}

#[tauri::command]
pub fn window_overview_state() -> bool {
    is_open()
}

/// Off the main thread: AX close/activate with its bounded waits (up to ~1 s
/// of front verification) used to block every Noki window's input.
#[tauri::command]
pub async fn window_overview_action(
    app: tauri::AppHandle,
    id: i64,
    pid: i32,
    action: String,
) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _busy = super::UiBusy::neu("window_overview_action");
        window_overview_action_sync(app, id, pid, action)
    }).await.unwrap_or_else(|e| Err(e.to_string()))
}
fn window_overview_action_sync(
    app: tauri::AppHandle,
    id: i64,
    pid: i32,
    action: String,
) -> Result<bool, String> {
    if id <= 0 || pid <= 0 {
        return Err("Ungültiges Fenster".into());
    }
    if !target_exists(pid, id) {
        return Err("Fenster ist nicht mehr verfügbar".into());
    }
    // Ask Noki: Noki's own window - never AX/activation on Noki's process
    // from here; the existing window is acted on directly.
    if pid as u32 == std::process::id() {
        if super::ask_nutzerfenster() != Some(id) {
            return Err("Das Fenster ist nicht mehr geöffnet".into());
        }
        if action == "activate" {
            if let Some(target_spaces) = super::cgs::spaces_des_fensters(id) {
                let current_space = super::cgs::aktiver_space().map(|x| x.0);
                if !target_spaces.is_empty() && !target_spaces.contains(&current_space.unwrap_or(0)) {
                    let _ = super::cgs::direkt_zum_space(target_spaces[0]);
                }
            }
        }
        return if super::ask_fensteraktion(&app, &action) { Ok(true) }
            else { Err("Ask Noki ließ sich nicht erreichen".into()) };
    }
    let ok = match action.as_str() {
        "activate" => {
            // The overview stays open (only the user closes it); if the
            // window lives on another Space the panel follows there.
            // If target window resides on another desktop or space, navigate directly without intermediate stops
            let mut ziel_space = super::cgs::aktiver_space().map(|x| x.0).unwrap_or(0);
            if let Some(target_spaces) = super::cgs::spaces_des_fensters(id) {
                let current_space = super::cgs::aktiver_space().map(|x| x.0);
                if !target_spaces.is_empty()
                    && !target_spaces.contains(&current_space.unwrap_or(0))
                {
                    ziel_space = target_spaces[0];
                    let _ = super::cgs::direkt_zum_space(ziel_space);
                    // Raise only once the switch has landed: arriving on the
                    // Space, macOS re-fronts the app's key window (measured:
                    // the other VS Code window won over the clicked one).
                    for _ in 0..40 {
                        if super::cgs::aktiver_space().map(|x| x.0) == Some(ziel_space) { break; }
                        std::thread::sleep(Duration::from_millis(25));
                    }
                    std::thread::sleep(Duration::from_millis(120));
                }
            }
            let _ = super::vorschau::fernbedienung::fenster_zeigen(pid, id);
            // Exactly this window becomes the app's MAIN window before the
            // activation (macOS then makes it key), and is raised again after
            // it: an Electron app re-fronts its last active window on
            // activation (measured: VS Code card "probe.txt" -> the user's
            // other VS Code window ended up in front).
            let _ = super::vorschau::fernbedienung::fenster_heben_und_main(pid, id);
            // Exactly THIS window to the front (window-specific front process,
            // measured 30/30 exact in the Miniatur arrival order), then raise
            // it once more; app-wide activation only as a fallback.
            let exakt_vorn = fenster_front_exakt(pid, id);
            let active = exakt_vorn || super::blende::aktivieren_direkt(pid);
            let _ = super::vorschau::fernbedienung::fenster_heben_und_main(pid, id);
            // Success = the on-screen WindowServer order (front first) of
            // the Space just reached, stable for a moment: no other window
            // of this app above the clicked one.
            let _ = ziel_space;
            let vorn = || {
                let alle = super::cgs::alle_fenster();
                let echt = |w: &i64| alle.iter().any(|f| f.0 == *w && f.1 == pid && f.6 >= 120 && f.7 >= 80);
                super::cgs::stapel_vorn_zuerst().into_iter().find(echt) == Some(id)
            };
            let mut exact = false;
            let mut stabil = 0;
            for _ in 0..16 {
                if vorn() { stabil += 1; if stabil >= 4 { exact = true; break; } }
                else { stabil = 0; let _ = super::vorschau::fernbedienung::fenster_heben_und_main(pid, id); }
                std::thread::sleep(Duration::from_millis(60));
            }
            super::virtual_workspace::trace(&format!(
                "[OVERVIEW] activate wid={id} pid={pid} app_active={active} exact_front={exact}"));
            // An Electron app may re-front its previously focused window up
            // to ~1 s after activation (measured VS Code). Hold the user's
            // choice for 2 s in the background - only while this app is
            // still frontmost and no newer Shortcut 9 choice was made.
            static WAHL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let meine = WAHL.fetch_add(1, Ordering::SeqCst) + 1;
            std::thread::spawn(move || {
                let mut korrigiert = 0;
                for _ in 0..20 {
                    std::thread::sleep(Duration::from_millis(100));
                    if WAHL.load(Ordering::SeqCst) != meine { break; }
                    let vorn = super::blende::vorn_pid();
                    if vorn != pid {
                        super::virtual_workspace::trace(&format!("[OVERVIEW] activate wid={id} hold_end front_pid={vorn}"));
                        break;
                    }
                    let alle = super::cgs::alle_fenster();
                    let echt = |w: &i64| alle.iter().any(|f| f.0 == *w && f.1 == pid && f.6 >= 120 && f.7 >= 80);
                    if super::cgs::stapel_vorn_zuerst().into_iter().find(echt) != Some(id) {
                        let _ = super::vorschau::fernbedienung::fenster_heben_und_main(pid, id);
                        korrigiert += 1;
                    }
                }
                if korrigiert > 0 {
                    super::virtual_workspace::trace(&format!("[OVERVIEW] activate wid={id} refronted_after_app={korrigiert}"));
                }
            });
            exact || active
        }
        "close" => {
            // Shortcut 9 + red X: the user chose exactly THIS window - native
            // close of it by pid + window id (also on another Space / full
            // screen; a save dialog stays the app's decision). No Space
            // switch, never the whole app. The Cmd-W fallback raises and
            // focuses the window - only allowed on the Space the user stands
            // on (raising a hidden window of the frontmost app switches Space).
            let space_before = super::cgs::aktiver_space().map(|x| x.0).unwrap_or(0);
            let hier = super::cgs::spaces_des_fensters(id).is_some_and(|s| s.contains(&space_before));
            let res = super::vorschau::fernbedienung::fenster_schliessen(pid, id)
                || (hier && super::vorschau::fernbedienung::fenster_schliessen_kurzbefehl(pid, id));
            super::space_action_log("overview_manual_close", &format!("pid{pid}"), id, space_before,
                super::cgs::aktiver_space().map(|x| x.0).unwrap_or(0));
            if let Ok(mut g) = BILDER.lock() {
                if let Some(m) = g.as_mut() {
                    m.remove(&id);
                }
            }
            res
        }
        "minimize" => super::vorschau::fernbedienung::fenster_minimieren(pid, id),
        "enlarge" => super::vorschau::fernbedienung::fenster_vergroessern(pid, id),
        _ => return Err("Unbekannte Fensteraktion".into()),
    };
    if ok {
        Ok(true)
    } else {
        Err("macOS hat diese Fensteraktion nicht bestätigt".into())
    }
}

/// `_SLPSSetFrontProcessWithOptions(psn, wid, kCPSUserGenerated)`: makes the
/// app frontmost WITH this exact window as its front window (no other window
/// of the app jumps ahead).
#[cfg(target_os = "macos")]
pub(crate) fn fenster_front_exakt(pid: i32, wid: i64) -> bool {
    use std::ffi::c_void;
    extern "C" {
        fn dlopen(p: *const i8, m: i32) -> *mut c_void;
        fn dlsym(h: *mut c_void, s: *const i8) -> *mut c_void;
    }
    #[repr(C)]
    struct Psn { hi: u32, lo: u32 }
    unsafe {
        let sky = dlopen(b"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight\0".as_ptr() as _, 1);
        let alle = dlopen(std::ptr::null(), 1);
        if sky.is_null() || alle.is_null() { return false; }
        let fp = dlsym(alle, b"GetProcessForPID\0".as_ptr() as _);
        let ff = dlsym(sky, b"_SLPSSetFrontProcessWithOptions\0".as_ptr() as _);
        if fp.is_null() || ff.is_null() { return false; }
        let get: unsafe extern "C" fn(i32, *mut Psn) -> i32 = std::mem::transmute(fp);
        let front: unsafe extern "C" fn(*mut Psn, u32, u32) -> i32 = std::mem::transmute(ff);
        let mut psn = Psn { hi: 0, lo: 0 };
        get(pid, &mut psn) == 0 && front(&mut psn, wid as u32, 0x200) == 0
    }
}
#[cfg(not(target_os = "macos"))]
pub(crate) fn fenster_front_exakt(_pid: i32, _wid: i64) -> bool { false }

#[cfg(not(target_os = "macos"))]
fn list(_voll: bool) -> Vec<OverviewWindow> {
    Vec::new()
}

#[cfg(not(target_os = "macos"))]
fn target_exists(_pid: i32, _id: i64) -> bool {
    false
}

#[cfg(target_os = "macos")]
fn target_exists(pid: i32, id: i64) -> bool {
    super::cgs::spaces_des_fensters(id).is_some()
        || super::vorschau::fernbedienung::fenster_vorhanden(pid, id)
}

#[cfg(target_os = "macos")]
unsafe fn fast_ax_info(pid: i32, wid: i64) -> (String, String, String, Option<[f64; 4]>) {
    use std::ffi::c_void;
    if !super::vorschau::fernbedienung::vertraut() || pid <= 0 || wid <= 0 {
        return (String::new(), String::new(), String::new(), None);
    }
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXUIElementCreateApplication(pid: i32) -> *mut c_void;
        fn AXUIElementCopyAttributeValue(
            el: *mut c_void,
            attr: *const c_void,
            val: *mut *const c_void,
        ) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(o: *const c_void);
        fn CFArrayGetCount(a: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(a: *const c_void, i: isize) -> *const c_void;
        fn CFStringCreateWithCString(a: *const c_void, s: *const i8, enc: u32) -> *const c_void;
        fn CFStringGetCString(s: *const c_void, b: *mut u8, len: isize, enc: u32) -> u8;
        fn CFNumberGetValue(n: *const c_void, typ: i32, val: *mut c_void) -> u8;
    }
    unsafe fn cfstr(s: &str) -> *const c_void {
        let c = std::ffi::CString::new(s).unwrap();
        CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), 0x08000100)
    }
    unsafe fn text_attr(el: *mut c_void, name: &str) -> String {
        let attr = cfstr(name);
        let mut val: *const c_void = std::ptr::null();
        let mut out = String::new();
        if AXUIElementCopyAttributeValue(el, attr, &mut val) == 0 && !val.is_null() {
            let mut b = [0u8; 512];
            if CFStringGetCString(val, b.as_mut_ptr(), b.len() as isize, 0x08000100) != 0 {
                let n = b.iter().position(|x| *x == 0).unwrap_or(b.len());
                out = String::from_utf8_lossy(&b[..n]).into_owned();
            }
            CFRelease(val);
        }
        CFRelease(attr);
        out
    }
    let app = AXUIElementCreateApplication(pid);
    if app.is_null() {
        return (String::new(), String::new(), String::new(), None);
    }
    let win_attr = cfstr("AXWindows");
    let mut wins: *const c_void = std::ptr::null();
    let mut res = (String::new(), String::new(), String::new(), None);
    if AXUIElementCopyAttributeValue(app, win_attr, &mut wins) == 0 && !wins.is_null() {
        // Apps don't provide an "AXWindowNumber" attribute; the CGWindowID
        // of an AX window comes from _AXUIElementGetWindow. (The old lookup
        // never matched, so role/subrole filters and minimized detection
        // silently did nothing.)
        #[link(name = "ApplicationServices", kind = "framework")]
        extern "C" {
            fn _AXUIElementGetWindow(el: *mut c_void, wid: *mut u32) -> i32;
        }
        for i in 0..CFArrayGetCount(wins) {
            let w = CFArrayGetValueAtIndex(wins, i) as *mut c_void;
            let mut found: u32 = 0;
            let found_id = if _AXUIElementGetWindow(w, &mut found) == 0 { found as i64 } else { 0 };
            if found_id == wid {
                let role = text_attr(w, "AXRole");
                let subrole = text_attr(w, "AXSubrole");
                let title = text_attr(w, "AXTitle");
                res = (role, subrole, title, ax_lichter(w));
                break;
            }
        }
        CFRelease(wins);
    }
    CFRelease(win_attr);
    CFRelease(app as *const c_void);
    res
}

/// `voll`: complete set for a (re)build of the rail - every card gets its
/// last valid frame at once (no capture wait). Otherwise (2.5 s poll) only
/// structure: pictures are sent solely for windows that have none yet; the
/// visible cards get their fresh content from the live loop.
#[cfg(target_os = "macos")]
fn list(voll: bool) -> Vec<OverviewWindow> {
    use std::ffi::{c_void, CString};
    type C = *const c_void;
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGWindowListCopyWindowInfo(o: u32, w: u32) -> C;
        fn CGRectMakeWithDictionaryRepresentation(d: C, r: *mut CaptureRect) -> u8;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFArrayGetCount(a: C) -> isize;
        fn CFArrayGetValueAtIndex(a: C, i: isize) -> C;
        fn CFDictionaryGetValue(d: C, k: C) -> C;
        fn CFNumberGetValue(n: C, t: i32, v: *mut c_void) -> u8;
        fn CFStringCreateWithCString(a: C, s: *const i8, e: u32) -> C;
        fn CFStringGetCString(s: C, b: *mut u8, n: isize, e: u32) -> u8;
        fn CFRelease(o: C);
    }
    unsafe fn key(s: &str) -> C {
        let c = CString::new(s).unwrap();
        CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), 0x08000100)
    }
    unsafe fn num(d: C, k: C) -> i64 {
        let mut n = 0i64;
        let v = CFDictionaryGetValue(d, k);
        if !v.is_null() {
            let _ = CFNumberGetValue(v, 4, &mut n as *mut _ as *mut c_void);
        }
        n
    }
    unsafe fn text(d: C, k: C) -> String {
        let v = CFDictionaryGetValue(d, k);
        let mut b = [0u8; 512];
        if v.is_null() || CFStringGetCString(v, b.as_mut_ptr(), b.len() as isize, 0x08000100) == 0 {
            return String::new();
        }
        let n = b.iter().position(|x| *x == 0).unwrap_or(b.len());
        String::from_utf8_lossy(&b[..n]).into_owned()
    }

    let own = std::process::id() as i64;
    let ask = super::ask_nutzerfenster();
    let active_space = super::cgs::aktiver_space().map(|x| x.0);
    let mut items = Vec::new();

    unsafe {
        let keys = [
            key("kCGWindowNumber"),
            key("kCGWindowOwnerPID"),
            key("kCGWindowLayer"),
            key("kCGWindowBounds"),
            key("kCGWindowOwnerName"),
            key("kCGWindowName"),
            key("kCGWindowIsOnscreen"),
        ];
        // 16 = kCGWindowListExcludeDesktopElements
        let a = CGWindowListCopyWindowInfo(16, 0);
        if a.is_null() {
            for k in keys {
                CFRelease(k);
            }
            return Vec::new();
        }
        let total = CFArrayGetCount(a);
        for i in 0..total {
            // Never silently cut user windows (was: a hard stop at 36).
            // Only a runaway WindowServer list is bounded - and said so.
            if items.len() >= MAX_EINTRAEGE {
                eprintln!("[OVERVIEW] inventory capped at {MAX_EINTRAEGE} entries (of {total} WindowServer windows)");
                break;
            }
            let d = CFArrayGetValueAtIndex(a, i);
            let id = num(d, keys[0]);
            let pid = num(d, keys[1]) as i32;
            // Noki's own surfaces never - except Ask Noki while it is really
            // shown (the one shared rule, same as the Miniatur's).
            let ist_ask = ask.is_some_and(|w| w == id) && pid as i64 == own;
            if id <= 0 || pid <= 0 || (pid as i64 == own && !ist_ask) || num(d, keys[2]) != 0 {
                continue;
            }

            let b = CFDictionaryGetValue(d, keys[3]);
            let mut r = CaptureRect::default();
            if b.is_null()
                || CGRectMakeWithDictionaryRepresentation(b, &mut r) == 0
                || r.size[0] < 120.0
                || r.size[1] < 80.0
            {
                continue;
            }

            if ist_ask {
                let spaces = super::cgs::spaces_des_fensters(id).unwrap_or_default();
                let on_active = active_space.is_some_and(|sid| spaces.contains(&sid));
                if let Ok(mut g) = FENSTER_INFO.lock() {
                    let m = g.get_or_insert_with(HashMap::new);
                    let alt = m.get(&id).and_then(|e| e.1);
                    m.insert(id, (r, alt));
                }
                items.push((
                    OverviewWindow {
                        id, pid,
                        app: "Ask Noki".into(),
                        title: "Ask Noki".into(),
                        preview: String::new(),
                        icon: app_icon(pid),
                        minimized: false,
                        chrome: false,
                        stand_ms: 0,
                        auf_space: on_active,
                        drosselt: false,
                    },
                    false, on_active, i, r,
                ));
                continue;
            }
            let app = text(d, keys[4]);
            if app.is_empty()
                || matches!(
                    app.as_str(),
                    "Window Server"
                        | "Dock"
                        | "Control Center"
                        | "Notification Center"
                        | "SystemUIServer"
                        | "Screenshot"
                )
            {
                continue;
            }
            let t_man = Instant::now();
            let is_man = manageable_app(pid);
            let d_man = t_man.elapsed();
            if !is_man {
                continue;
            }

            // Fast AX lookup without 1.2s offspace token scanning
            let t_ax = Instant::now();
            let (role, sub, ax_title, lichter) = fast_ax_info(pid, id);
            if let Ok(mut g) = FENSTER_INFO.lock() {
                let m = g.get_or_insert_with(HashMap::new);
                // Keep the last AX-known lights while the window is off-Space.
                let alt = m.get(&id).and_then(|e| e.1);
                m.insert(id, (r, lichter.or(alt)));
            }
            let d_ax = t_ax.elapsed();
            if !role.is_empty() && role != "AXWindow" {
                continue;
            }
            if !sub.is_empty()
                && !matches!(
                    sub.as_str(),
                    "AXStandardWindow" | "AXDialog" | "AXSystemDialog"
                )
            {
                continue;
            }

            let cg_title = text(d, keys[5]);
            let merk_titel = if !ax_title.is_empty() { ax_title.clone() } else { cg_title.clone() };
            if let Ok(mut g) = FENSTER_TITEL.lock() { g.get_or_insert_with(HashMap::new).insert(id, merk_titel); }
            // Helper surfaces (e.g. Chrome's fullscreen toolbar strip) have
            // neither an AX window nor a title - never a user window.
            if role.is_empty() && ax_title.is_empty() && cg_title.is_empty() {
                continue;
            }
            let final_title = if !ax_title.is_empty() {
                ax_title
            } else if !cg_title.is_empty() {
                cg_title
            } else {
                app.clone()
            };

            let t_sp = Instant::now();
            let spaces = super::cgs::spaces_des_fensters(id).unwrap_or_default();
            let on_active = active_space.is_some_and(|sid| spaces.contains(&sid));
            // Minimized/hidden windows keep their Space membership but are
            // not ordered in; windows on another Space ARE ordered in.
            // Spotify/Safari/Finder/Goodnotes keep CLOSED windows listed
            // (with title): closed = not ordered in AND unknown to AX.
            let ordered_in = super::cgs::fenster_eingeordnet(id);
            let d_sp = t_sp.elapsed();
            if !ordered_in && role.is_empty() {
                continue;
            }
            if spaces.is_empty() && role.is_empty() {
                continue;
            }
            let minimized = !ordered_in;
            let d_cap = Duration::ZERO;
            let preview_uri = String::new();

            let t_ico = Instant::now();
            let icon_uri = app_icon(pid);
            let d_ico = t_ico.elapsed();

            if d_man.as_millis() > 20 || d_ax.as_millis() > 20 || d_sp.as_millis() > 20 || d_cap.as_millis() > 20 || d_ico.as_millis() > 20 {
                eprintln!("[TIMING] app='{}' man={}ms ax={}ms sp={}ms cap={}ms ico={}ms",
                    app, d_man.as_millis(), d_ax.as_millis(), d_sp.as_millis(), d_cap.as_millis(), d_ico.as_millis());
            }

            items.push((
                OverviewWindow {
                    id,
                    pid,
                    app,
                    title: final_title,
                    preview: preview_uri,
                    icon: icon_uri,
                    minimized,
                    chrome: {
                        let b = ist_chrome(pid);
                        if b { if let Ok(mut g) = BROWSER_FENSTER.lock() { g.get_or_insert_with(Default::default).insert(id); } }
                        b
                    },
                    stand_ms: 0,
                    auf_space: on_active,
                    drosselt: drosselt_verdeckt(pid),
                },
                minimized,
                on_active,
                i,
                r,
            ));
        }
        CFRelease(a);
        for k in keys {
            CFRelease(k);
        }
    }

    // Sort order:
    // 1. Visible windows on active space (first 3 cards in exact z-order)
    // 2. Visible windows on other spaces
    // 3. Minimized windows
    items.sort_by_key(|(_, min, on_act, z, _)| {
        let group = if *min {
            2
        } else if *on_act {
            0
        } else {
            1
        };
        (group, *z)
    });

    // Previews: the last valid frame of every window is known (one frame
    // record per window). Only windows WITHOUT one are captured here, in
    // parallel; everything else is refreshed by the live loop while its
    // card is visible. CGWindowListCreateImage returns the last painted
    // backing store also for windows on another Space.
    let t_cap = Instant::now();
    let mut jobs: Vec<(usize, i64, CaptureRect)> = Vec::new();
    for (k, (w, minimized, _, _, r)) in items.iter_mut().enumerate() {
        match bild(w.id) {
            Some(b) => {
                w.stand_ms = b.geaendert.elapsed().as_millis() as u64;
                if voll {
                    w.preview = b.url(w.id);
                }
            }
            // First pictures only for the cards near the top; cards further
            // down get theirs from the live loop once scrolled into view
            // (a long list must not mean one capture burst per window).
            None if !*minimized && jobs.len() < ERSTE_BILDER => jobs.push((k, w.id, *r)),
            None => {}
        }
        if !voll {
            w.icon = String::new(); // the card already has it
        }
    }
    let (mw, mh) = vorschau_px();
    let lichter: HashMap<i64, Option<[f64; 4]>> = FENSTER_INFO.lock().ok()
        .and_then(|g| g.as_ref().map(|m| m.iter().map(|(k, v)| (*k, v.1)).collect()))
        .unwrap_or_default();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, String)>> = Mutex::new(Vec::new());
    std::thread::scope(|sc| {
        for _ in 0..4 {
            sc.spawn(|| loop {
                let k = next.fetch_add(1, Ordering::Relaxed);
                let Some((idx, id, r)) = jobs.get(k).copied() else { break };
                if let Some((png, h)) = unsafe { capture_scharf(id, r, lichter.get(&id).copied().flatten(), mw, mh, bild(id).map(|b| b.hash)) } {
                    bild_merken(id, png, h);
                    if let Some(b) = bild(id) {
                        if let Ok(mut v) = results.lock() { v.push((idx, b.url(id))); }
                    }
                }
            });
        }
    });
    let erfasst = jobs.len();
    for (idx, img) in results.into_inner().unwrap_or_default() {
        if !img.is_empty() { tab_bild_merken(items[idx].0.id, None); }
        items[idx].0.preview = img;
    }
    let ohne = items.iter().filter(|(w, ..)| w.preview.is_empty()).count();
    eprintln!("[OVERVIEW] inventory voll={voll} windows={} captured={erfasst} without_preview={} capture_ms={}", items.len(), ohne, t_cap.elapsed().as_millis());

    items.into_iter().map(|(w, _, _, _, _)| w).collect()
}

/// The window's own traffic lights (close ... zoom/fullscreen) as a rect in
/// POINTS relative to the window's top-left, from the exact AX buttons.
#[cfg(target_os = "macos")]
unsafe fn ax_lichter(w: *mut std::ffi::c_void) -> Option<[f64; 4]> {
    use std::ffi::c_void;
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXUIElementCopyAttributeValue(el: *mut c_void, attr: *const c_void, val: *mut *const c_void) -> i32;
        fn AXValueGetValue(v: *const c_void, t: u32, out: *mut c_void) -> bool;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(o: *const c_void);
        fn CFStringCreateWithCString(a: *const c_void, s: *const i8, enc: u32) -> *const c_void;
    }
    let attr = |el: *mut c_void, name: &str| -> *const c_void {
        let c = std::ffi::CString::new(name).unwrap();
        let k = CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), 0x08000100);
        let mut v: *const c_void = std::ptr::null();
        let ok = AXUIElementCopyAttributeValue(el, k, &mut v) == 0;
        CFRelease(k);
        if ok { v } else { std::ptr::null() }
    };
    let rahmen = |el: *mut c_void| -> Option<[f64; 4]> {
        let (p, g) = (attr(el, "AXPosition"), attr(el, "AXSize"));
        let mut pt = [0f64; 2];
        let mut sz = [0f64; 2];
        let ok = !p.is_null() && !g.is_null()
            && AXValueGetValue(p, 1, pt.as_mut_ptr() as *mut c_void)
            && AXValueGetValue(g, 2, sz.as_mut_ptr() as *mut c_void);
        if !p.is_null() { CFRelease(p); }
        if !g.is_null() { CFRelease(g); }
        ok.then_some([pt[0], pt[1], sz[0], sz[1]])
    };
    let fenster = rahmen(w)?;
    let mut u: Option<[f64; 4]> = None;
    for name in ["AXCloseButton", "AXMinimizeButton", "AXZoomButton", "AXFullScreenButton"] {
        let b = attr(w, name);
        if b.is_null() { continue; }
        if let Some(r) = rahmen(b as *mut c_void) {
            if r[2] > 0.0 && r[3] > 0.0 {
                u = Some(match u {
                    None => r,
                    Some(a) => {
                        let x0 = a[0].min(r[0]);
                        let y0 = a[1].min(r[1]);
                        [x0, y0, (a[0] + a[2]).max(r[0] + r[2]) - x0, (a[1] + a[3]).max(r[1] + r[3]) - y0]
                    }
                });
            }
        }
        CFRelease(b);
    }
    let u = u?;
    let rel = [u[0] - fenster[0], u[1] - fenster[1], u[2], u[3]];
    // Plausible only inside the window's top-left corner.
    (rel[0] >= 0.0 && rel[1] >= 0.0 && rel[0] + rel[2] < 160.0 && rel[1] + rel[3] < 90.0).then_some(rel)
}

/// Per window: CG frame (points) and, once known, its own traffic lights.
static FENSTER_INFO: Mutex<Option<HashMap<i64, (CaptureRect, Option<[f64; 4]>)>>> = Mutex::new(None);
/// Device pixels per point of the panel's screen (set by `place`).
static PANEL_SKALA: Mutex<f64> = Mutex::new(2.0);

/// Card preview box in device pixels (card width x card height minus name row).
fn vorschau_px() -> (usize, usize) {
    let k = PANEL_SKALA.lock().map(|g| *g).unwrap_or(2.0);
    let c = SPALTE.lock().ok().and_then(|g| *g);
    let (w, h) = c.map_or((339.0, 170.0), |c| (c.card_w, (c.card_h - KARTE_RAHMEN).max(40.0)));
    (((w * k).round() as usize).max(16), ((h * k).round() as usize).max(16))
}

/// Sharp live preview of one window: native-resolution backing store, ONE
/// high-quality downscale to exactly the card's device pixels (the page
/// then shows it 1:1 - no second resampling), and the window's own traffic
/// lights replaced by Noki's monitor symbol (the card has the real ones).
/// Returns (data URI, content hash).
#[cfg(target_os = "macos")]
unsafe fn capture_scharf(id: i64, r: CaptureRect, lichter: Option<[f64; 4]>, max_w: usize, max_h: usize, bekannt: Option<u64>) -> Option<(Vec<u8>, u64)> {
    use std::ffi::c_void;
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGWindowListCreateImage(r: CaptureRect, o: u32, w: u32, i: u32) -> *const c_void;
        fn CGImageGetWidth(i: *const c_void) -> usize;
        fn CGImageGetHeight(i: *const c_void) -> usize;
        fn CGColorSpaceCreateDeviceRGB() -> *const c_void;
        fn CGColorSpaceRelease(s: *const c_void);
        fn CGBitmapContextCreate(d: *mut c_void, w: usize, h: usize, b: usize, row: usize, s: *const c_void, info: u32) -> *mut c_void;
        fn CGContextSetInterpolationQuality(c: *mut c_void, q: i32);
        fn CGContextDrawImage(c: *mut c_void, r: CaptureRect, i: *const c_void);
        fn CGBitmapContextCreateImage(c: *mut c_void) -> *const c_void;
        fn CGContextRelease(c: *const c_void);
        fn CGImageRelease(i: *const c_void);
        fn CGImageGetColorSpace(i: *const c_void) -> *const c_void;
        fn CGColorSpaceGetModel(s: *const c_void) -> i32;
        fn CGColorSpaceRetain(s: *const c_void) -> *const c_void;
    }
    // kCGWindowListOptionIncludingWindow; BoundsIgnoreFraming | BestResolution
    let mut im = CGWindowListCreateImage(CaptureRect::null(), 8, id as u32, 1 | 8);
    if im.is_null() {
        im = CGWindowListCreateImage(r, 8, id as u32, 1 | 8);
    }
    if im.is_null() {
        return None;
    }
    let (sw, sh) = (CGImageGetWidth(im), CGImageGetHeight(im));
    if sw < 2 || sh < 2 {
        CGImageRelease(im);
        return None;
    }
    let s = (max_w as f64 / sw as f64).min(max_h as f64 / sh as f64).min(1.0);
    let (tw, th) = (((sw as f64 * s).round() as usize).max(1), ((sh as f64 * s).round() as usize).max(1));
    let mut buf = vec![0u8; tw * th * 4];
    // Scale in the window's OWN colour space. A DeviceRGB context made CG
    // colour-match every native frame first (vImage lookup tables plus
    // un-/premultiply over ~22 MB): measured ~50% of Noki's CPU with the
    // overview open. Same pixels, no conversion.
    let quelle_cs = CGImageGetColorSpace(im);
    let cs = if !quelle_cs.is_null() && CGColorSpaceGetModel(quelle_cs) == 1 {
        CGColorSpaceRetain(quelle_cs)
    } else {
        CGColorSpaceCreateDeviceRGB()
    };
    // kCGImageAlphaPremultipliedLast; row 0 of `buf` is the TOP row.
    let ctx = CGBitmapContextCreate(buf.as_mut_ptr() as *mut c_void, tw, th, 8, tw * 4, cs, 1);
    CGColorSpaceRelease(cs);
    if ctx.is_null() {
        CGImageRelease(im);
        return None;
    }
    CGContextSetInterpolationQuality(ctx, 3); // kCGInterpolationHigh
    CGContextDrawImage(ctx, CaptureRect { origin: [0.0, 0.0], size: [tw as f64, th as f64] }, im);
    CGImageRelease(im);
    // Pixels per point of THIS preview (window width in points from CG).
    let k = if r.size[0] > 1.0 { tw as f64 / r.size[0] } else { s };
    let rect = lichter
        .map(|l| [l[0] * k, l[1] * k, l[2] * k, l[3] * k])
        .or_else(|| ampel_finden(&buf, tw, th, k));
    if let Some(rc) = rect {
        ampel_ersetzen(ctx, &buf, tw, th, rc, k);
    }
    // During a Space slide WindowServer can hand back an all-black image of
    // a window (seen on a real swipe: Terminal card black for one refresh,
    // fine again after). Never replace a valid picture with that: report
    // "unchanged" so the card keeps its last good frame.
    if let Some(alt) = bekannt {
        if fast_schwarz(&buf) {
            CGContextRelease(ctx);
            return Some((Vec::new(), alt));
        }
    }
    // EVERY byte: a sparse sample (every 13th byte) missed a terminal
    // counter's 3 px digits entirely - the card then kept an old frame.
    let hash = inhalt_hash(&buf);
    // Unchanged content: no encoding, no transfer (the card keeps it).
    if bekannt == Some(hash) {
        CGContextRelease(ctx);
        return Some((Vec::new(), hash));
    }
    let out = CGBitmapContextCreateImage(ctx);
    CGContextRelease(ctx);
    if out.is_null() {
        return None;
    }
    let png = png_bytes(out);
    CGImageRelease(out);
    (!png.is_empty()).then_some((png, hash))
}

/// Blank capture: fewer than 0.2 % of sampled pixels brighter than
/// near-black. Real dark windows (Terminal, video) still have text or
/// highlights well above that.
fn fast_schwarz(buf: &[u8]) -> bool {
    let (mut n, mut hell) = (0usize, 0usize);
    for p in buf.chunks_exact(4).step_by(7) {
        n += 1;
        if p[0] as u32 + p[1] as u32 + p[2] as u32 > 24 {
            hell += 1;
        }
    }
    n > 0 && hell * 500 < n
}

/// Content hash over every byte of a frame (word-wise, ~0.1 ms per card).
fn inhalt_hash(buf: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut worte = buf.chunks_exact(8);
    for w in &mut worte {
        let v = u64::from_le_bytes([w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7]]);
        h = (h ^ v).wrapping_mul(0x100000001b3).rotate_left(29);
    }
    for b in worte.remainder() {
        h = (h ^ *b as u64).wrapping_mul(0x100000001b3);
    }
    h
}

/// Fallback when AX gave no button rects (window on another Space): the
/// coloured red and green circles in the top-left corner (px, top-left).
fn ampel_finden(buf: &[u8], tw: usize, th: usize, k: f64) -> Option<[f64; 4]> {
    let (mx, my) = (((100.0 * k) as usize).min(tw), ((50.0 * k) as usize).min(th));
    let mut rot: Option<[usize; 4]> = None;
    let mut gruen: Option<[usize; 4]> = None;
    let dazu = |b: &mut Option<[usize; 4]>, x: usize, y: usize| {
        *b = Some(match *b {
            None => [x, y, x, y],
            Some([a, c, d, e]) => [a.min(x), c.min(y), d.max(x), e.max(y)],
        });
    };
    for y in 0..my {
        for x in 0..mx {
            let i = (y * tw + x) * 4;
            let (r, g, b) = (buf[i] as i32, buf[i + 1] as i32, buf[i + 2] as i32);
            if r > 200 && g < 125 && b < 125 {
                dazu(&mut rot, x, y);
            } else if g > 150 && r < 130 && b < 130 {
                dazu(&mut gruen, x, y);
            }
        }
    }
    let (r, g) = (rot?, gruen?);
    let (rh, gh) = ((r[3] - r[1]) as f64, (g[3] - g[1]) as f64);
    let (rcy, gcy) = ((r[1] + r[3]) as f64 / 2.0, (g[1] + g[3]) as f64 / 2.0);
    // Three equal circles in a row: similar size, same line, green right of red.
    let plausibel = rh >= 3.0
        && (rh - gh).abs() <= rh * 0.5 + 1.0
        && (rcy - gcy).abs() <= 3.0 * k
        && g[0] > r[2]
        && (g[0] - r[2]) as f64 <= 45.0 * k;
    plausibel.then(|| {
        let y0 = r[1].min(g[1]);
        [r[0] as f64, y0 as f64, (g[2] - r[0]) as f64 + 1.0, (r[3].max(g[3]) - y0) as f64 + 1.0]
    })
}

/// Paint over the preview's own traffic lights with the adjacent title-bar
/// colour and draw Noki's monitor symbol there instead (CG, bottom-left).
#[cfg(target_os = "macos")]
unsafe fn ampel_ersetzen(ctx: *mut std::ffi::c_void, buf: &[u8], tw: usize, th: usize, rc: [f64; 4], k: f64) {
    use std::ffi::c_void;
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGContextSetRGBFillColor(c: *mut c_void, r: f64, g: f64, b: f64, a: f64);
        fn CGContextSetRGBStrokeColor(c: *mut c_void, r: f64, g: f64, b: f64, a: f64);
        fn CGContextFillRect(c: *mut c_void, r: CaptureRect);
        fn CGContextSetLineWidth(c: *mut c_void, w: f64);
        fn CGContextStrokeRect(c: *mut c_void, r: CaptureRect);
        fn CGContextMoveToPoint(c: *mut c_void, x: f64, y: f64);
        fn CGContextAddLineToPoint(c: *mut c_void, x: f64, y: f64);
        fn CGContextStrokePath(c: *mut c_void);
        fn CGContextSetLineCap(c: *mut c_void, cap: i32);
    }
    if tw < 4 || th < 4 {
        return;
    }
    let rand = 2.0 * k;
    let (x0, y0) = ((rc[0] - rand).max(0.0), (rc[1] - rand).max(0.0));
    let (x1, y1) = ((rc[0] + rc[2] + rand).min(tw as f64), (rc[1] + rc[3] + rand).min(th as f64));
    // Colour right next to the lights, on their centre line (title bar).
    let sx = ((x1 + 3.0 * k) as usize).min(tw - 1);
    let sy = ((rc[1] + rc[3] / 2.0) as usize).min(th - 1);
    let i = (sy * tw + sx) * 4;
    let (r, g, b) = (buf[i] as f64 / 255.0, buf[i + 1] as f64 / 255.0, buf[i + 2] as f64 / 255.0);
    let cg = |x: f64, y: f64, w: f64, h: f64| CaptureRect { origin: [x, th as f64 - y - h], size: [w, h] };
    CGContextSetRGBFillColor(ctx, r, g, b, 1.0);
    CGContextFillRect(ctx, cg(x0, y0, x1 - x0, y1 - y0));
    // The screen indicator is Noki's own SF Symbol overlay on the card
    // (window-overview.html), never drawn into the picture.
}

// ---------------------------------------------------------------------
//  Chrome tabs inside ONE window card (horizontal paging in the page)
// ---------------------------------------------------------------------

#[derive(serde::Serialize, Clone, Debug)]
pub struct TabInfo {
    /// Stable key for activation: CDP target id, or "ax:<index>" (AX route).
    pub key: String,
    pub title: String,
    pub url: String,
    pub active: bool,
    /// Sharp page picture; empty if none is known yet (never a wrong tab).
    pub preview: String,
}

/// Window title per window id (set by `list`) - the active tab's title of a
/// Chrome window; used to file live frames under the right tab.
static FENSTER_TITEL: Mutex<Option<HashMap<i64, String>>> = Mutex::new(None);
/// Last sharp picture per (window, tab title), taken while that tab was the
/// visible one (regular Chrome renders only its active tab).
static TAB_BILD: Mutex<Option<HashMap<(i64, String), (u64, Arc<Vec<u8>>)>>> = Mutex::new(None);
/// Last known tab-strip order (CDP target ids) per window.
static TAB_REIHE: Mutex<Option<HashMap<i64, Vec<String>>>> = Mutex::new(None);

/// Windows of tab browsers (set by `list`): only their frames are filed.
static BROWSER_FENSTER: Mutex<Option<std::collections::HashSet<i64>>> = Mutex::new(None);

/// The window's title NOW (WindowServer; a browser's = its shown tab).
#[cfg(target_os = "macos")]
fn titel_jetzt(id: i64) -> Option<String> {
    use std::ffi::c_void;
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" { fn CGWindowListCopyWindowInfo(o: u32, w: u32) -> *const c_void; }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFArrayGetCount(a: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(a: *const c_void, i: isize) -> *const c_void;
        fn CFDictionaryGetValue(d: *const c_void, k: *const c_void) -> *const c_void;
        fn CFStringCreateWithCString(a: *const c_void, s: *const i8, e: u32) -> *const c_void;
        fn CFStringGetCString(s: *const c_void, b: *mut u8, n: isize, e: u32) -> u8;
        fn CFRelease(o: *const c_void);
    }
    unsafe {
        let a = CGWindowListCopyWindowInfo(8, id as u32); // IncludingWindow
        if a.is_null() { return None; }
        let mut out = None;
        if CFArrayGetCount(a) > 0 {
            let d = CFArrayGetValueAtIndex(a, 0);
            let k = CFStringCreateWithCString(std::ptr::null(), c"kCGWindowName".as_ptr(), 0x08000100);
            let v = CFDictionaryGetValue(d, k);
            let mut b = [0u8; 1024];
            if !v.is_null() && CFStringGetCString(v, b.as_mut_ptr(), b.len() as isize, 0x08000100) != 0 {
                let n = b.iter().position(|x| *x == 0).unwrap_or(b.len());
                out = Some(String::from_utf8_lossy(&b[..n]).into_owned()).filter(|t| !t.trim().is_empty());
            }
            CFRelease(k);
        }
        CFRelease(a);
        out
    }
}
#[cfg(not(target_os = "macos"))]
fn titel_jetzt(_id: i64) -> Option<String> { None }

/// Files the window's CURRENT frame under the tab shown in it RIGHT NOW
/// (fresh title - the 2.5 s inventory title could name the previous tab
/// right after a tab switch and file a wrong picture under it).
fn ist_browser_fenster(id: i64) -> bool {
    BROWSER_FENSTER.lock().ok().is_some_and(|g| g.as_ref().is_some_and(|s| s.contains(&id)))
}

/// `vorher`: the title read right BEFORE the frame was captured; the frame
/// is filed only if the same tab was shown before and after.
fn tab_bild_merken(id: i64, vorher: Option<String>) {
    if !ist_browser_fenster(id) {
        return;
    }
    let titel = titel_jetzt(id);
    if vorher.is_some() && vorher != titel {
        return;
    }
    let titel = titel.map(|t| tab_titel(&t));
    let Some(b) = bild(id) else { return };
    if let (Some(t), Ok(mut g)) = (titel, TAB_BILD.lock()) {
        let m = g.get_or_insert_with(HashMap::new);
        m.insert((id, t.clone()), (b.hash, b.png.clone()));
        if m.len() > 64 {
            if let Some(k) = m.keys().next().cloned() { m.remove(&k); }
        }
        drop(g);
        tab_bild_sichern(id, &t, b.png);
    }
}

/// Last real tab pictures also survive a Noki restart (Chrome keeps its
/// window ids): exact window + real tab title -> PNG, bounded (64 files),
/// written at most every 5 s per tab. Read only when memory has nothing.
fn tab_cache_ordner() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join("Library/Caches/com.noki.desktop/tabbilder")
}

fn tab_datei(id: i64, titel: &str) -> std::path::PathBuf {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in titel.bytes() { h = (h ^ b as u64).wrapping_mul(0x100000001b3); }
    tab_cache_ordner().join(format!("{id}_{h:016x}.png"))
}

fn tab_bild_sichern(id: i64, titel: &str, png: Arc<Vec<u8>>) {
    static ZULETZT: Mutex<Option<HashMap<(i64, String), Instant>>> = Mutex::new(None);
    let frei = ZULETZT.lock().map(|mut g| {
        let m = g.get_or_insert_with(HashMap::new);
        let k = (id, titel.to_string());
        let f = m.get(&k).map_or(true, |t| t.elapsed() >= Duration::from_secs(5));
        if f { m.insert(k, Instant::now()); }
        f
    }).unwrap_or(false);
    if !frei || png.is_empty() {
        return;
    }
    let datei = tab_datei(id, titel);
    std::thread::spawn(move || {
        let ordner = tab_cache_ordner();
        let _ = std::fs::create_dir_all(&ordner);
        let tmp = datei.with_extension("tmp");
        if std::fs::write(&tmp, png.as_slice()).is_ok() {
            let _ = std::fs::rename(&tmp, &datei);
        }
        // Bound: keep the 64 newest pictures.
        if let Ok(rd) = std::fs::read_dir(&ordner) {
            let mut alle: Vec<(std::time::SystemTime, std::path::PathBuf)> = rd.flatten()
                .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
                .collect();
            if alle.len() > 64 {
                alle.sort();
                for (_, p) in alle.iter().take(alle.len() - 64) { let _ = std::fs::remove_file(p); }
            }
        }
    });
}

/// Remembered picture of one real tab: memory first, then the disk cache.
fn tab_bild_suchen(id: i64, titel: &str) -> Option<u64> {
    if let Some((h, _)) = TAB_BILD.lock().ok().and_then(|g| g.as_ref().and_then(|m| m.get(&(id, titel.to_string())).cloned())) {
        return Some(h);
    }
    let png = std::fs::read(tab_datei(id, titel)).ok().filter(|p| p.len() > 64)?;
    let h = inhalt_hash(&png);
    if let Ok(mut g) = TAB_BILD.lock() {
        g.get_or_insert_with(HashMap::new).insert((id, titel.to_string()), (h, Arc::new(png)));
    }
    Some(h)
}

/// Tab buttons of exactly this window from its AX tab strip, in strip
/// order: (title, selected, element). Web content is never walked.
#[cfg(target_os = "macos")]
unsafe fn ax_tabs(pid: i32, wid: i64) -> Vec<(String, bool, *mut std::ffi::c_void)> {
    use std::ffi::c_void;
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXUIElementCreateApplication(pid: i32) -> *mut c_void;
        fn AXUIElementCopyAttributeValue(el: *mut c_void, attr: *const c_void, val: *mut *const c_void) -> i32;
        fn _AXUIElementGetWindow(el: *mut c_void, wid: *mut u32) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(o: *const c_void);
        fn CFRetain(o: *const c_void) -> *const c_void;
        fn CFArrayGetCount(a: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(a: *const c_void, i: isize) -> *const c_void;
        fn CFStringCreateWithCString(a: *const c_void, s: *const i8, enc: u32) -> *const c_void;
        fn CFStringGetCString(s: *const c_void, b: *mut u8, len: isize, enc: u32) -> u8;
        fn CFGetTypeID(o: *const c_void) -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFNumberGetValue(n: *const c_void, t: i32, v: *mut c_void) -> u8;
        fn CFNumberGetTypeID() -> usize;
        fn CFEqual(a: *const c_void, b: *const c_void) -> bool;
        static kCFBooleanTrue: *const c_void;
    }
    let attr = |el: *mut c_void, name: &str| -> *const c_void {
        let c = std::ffi::CString::new(name).unwrap();
        let k = CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), 0x08000100);
        let mut v: *const c_void = std::ptr::null();
        let ok = AXUIElementCopyAttributeValue(el, k, &mut v) == 0;
        CFRelease(k);
        if ok { v } else { std::ptr::null() }
    };
    let text = |el: *mut c_void, name: &str| -> String {
        let v = attr(el, name);
        if v.is_null() { return String::new(); }
        let mut out = String::new();
        if CFGetTypeID(v) == CFStringGetTypeID() {
            let mut b = [0u8; 512];
            if CFStringGetCString(v, b.as_mut_ptr(), b.len() as isize, 0x08000100) != 0 {
                let n = b.iter().position(|x| *x == 0).unwrap_or(b.len());
                out = String::from_utf8_lossy(&b[..n]).into_owned();
            }
        }
        CFRelease(v);
        out
    };
    let mut out = Vec::new();
    // Exact window, also off-Space / fullscreen (remote token): the AX
    // window list only knows the current Space - that is why only the
    // Chrome window whose Space the user stood on ever showed its tabs.
    let fenster = super::vorschau::fernbedienung::ax_fenster_fuer(pid, wid)
        .map_or(std::ptr::null_mut(), |p| p as *mut c_void);
    if fenster.is_null() { return out; }
    // Chrome exposes the SAME tab strip several times in a window's AX tree
    // (measured on the user's fullscreen windows: 4 identical AXTabGroups -
    // 2 real tabs shown as "1 von 8"), and web pages add their own ARIA
    // tabs inside web content/landmarks. Only the FIRST browser tab strip
    // counts: direct AXTabButton children of the first AXTabGroup outside
    // any web area or landmark, in document order.
    let mut stapel: Vec<(*mut c_void, usize)> = vec![(fenster, 0)];
    let mut besucht = 0usize;
    let start = Instant::now();
    while let Some((el, tiefe)) = stapel.pop() {
        besucht += 1;
        if !out.is_empty() || besucht > 4000 || start.elapsed() > Duration::from_millis(400) {
            CFRelease(el as *const c_void);
            continue;
        }
        let rolle = text(el, "AXRole");
        let sub = text(el, "AXSubrole");
        if rolle == "AXWebArea" || sub.starts_with("AXLandmark") || tiefe > 14 {
            CFRelease(el as *const c_void);
            continue;
        }
        let kinder = attr(el, "AXChildren");
        if rolle == "AXTabGroup" && !kinder.is_null() {
            // Chrome: tab buttons are direct children. Safari nests them one
            // level deeper (AXGroup/AXScrollArea of the tab bar) and may put
            // the page title into AXDescription. Never below that.
            // Every collected button holds ONE retain of its own.
            let mut knoepfe: Vec<*mut c_void> = Vec::new();
            for i in 0..CFArrayGetCount(kinder) {
                let c = CFArrayGetValueAtIndex(kinder, i) as *mut c_void;
                if c.is_null() { continue; }
                if text(c, "AXSubrole") == "AXTabButton" || text(c, "AXRole") == "AXRadioButton" {
                    knoepfe.push(CFRetain(c) as *mut c_void); continue;
                }
                if matches!(text(c, "AXRole").as_str(), "AXGroup" | "AXScrollArea") {
                    let enkel = attr(c, "AXChildren");
                    if enkel.is_null() { continue; }
                    for j in 0..CFArrayGetCount(enkel) {
                        let e = CFArrayGetValueAtIndex(enkel, j) as *mut c_void;
                        if !e.is_null() && (text(e, "AXSubrole") == "AXTabButton" || text(e, "AXRole") == "AXRadioButton") {
                            knoepfe.push(CFRetain(e) as *mut c_void);
                        }
                    }
                    CFRelease(enkel);
                }
            }
            for c in knoepfe {
                let v = attr(c, "AXValue");
                let mut sel: i64 = 0;
                if !v.is_null() {
                    if CFGetTypeID(v) == CFNumberGetTypeID() { CFNumberGetValue(v, 4, &mut sel as *mut _ as *mut c_void); }
                    else if CFEqual(v, kCFBooleanTrue) { sel = 1; }
                    CFRelease(v);
                }
                let mut titel = text(c, "AXTitle");
                if titel.trim().is_empty() { titel = text(c, "AXDescription"); }
                out.push((tab_titel(&titel), sel == 1, c));   // its retain moves into `out`
            }
        }
        CFRelease(el as *const c_void);
        if kinder.is_null() { continue; }
        if out.is_empty() {
            // Reverse push = document order when popping.
            for i in (0..CFArrayGetCount(kinder)).rev() {
                let c = CFArrayGetValueAtIndex(kinder, i) as *mut c_void;
                if !c.is_null() { stapel.push((CFRetain(c) as *mut c_void, tiefe + 1)); }
            }
        }
        CFRelease(kinder);
    }
    out
}

/// Tab title as the page shows it: Chrome appends its hover-card status to
/// the AX title (" – Arbeitsspeichernutzung – 209 MB", " - Memory usage…").
fn tab_titel(roh: &str) -> String {
    // Separators = a dash preceded by any whitespace (Chrome uses a
    // NO-BREAK SPACE before its "– Arbeitsspeichernutzung – 31,6 MB"). Cut
    // at the first separator whose following segment is that memory status.
    let chars: Vec<(usize, char)> = roh.char_indices().collect();
    let trenner: Vec<usize> = chars.windows(2)
        .filter(|w| w[0].1.is_whitespace() && (w[1].1 == '–' || w[1].1 == '-'))
        .map(|w| w[0].0)
        .collect();
    for (k, &pos) in trenner.iter().enumerate() {
        let ende = trenner.get(k + 1).copied().unwrap_or(roh.len());
        let segment = roh[pos..ende].to_lowercase();
        // Memory status of Chrome's tab hover card: usage while loaded,
        // "Inaktiver Tab – 31,5 MB freigegeben" once Memory Saver unloaded
        // it. Without cutting the latter a tab's key changed with its
        // memory state and its last real picture was never found again.
        if segment.contains("arbeitsspeicher") || segment.contains("memory usage")
            || segment.contains("inaktiver tab") || segment.contains("inactive tab")
            || segment.contains("freigegeben") || segment.contains(" freed")
        {
            return roh[..pos].trim().to_string();
        }
    }
    roh.trim().to_string()
}

/// Apps whose windows paint NOTHING new while hidden on another Space
/// (Chromium occlusion throttling: Chrome, Electron apps like VS Code,
/// CEF apps like Spotify - measured: Spotify playing, 150 SCK frames in 5 s,
/// 1 unique). Noki's own browser runs with occlusion throttling disabled.
#[cfg(target_os = "macos")]
fn drosselt_verdeckt(pid: i32) -> bool {
    static CACHE: Mutex<Option<HashMap<i32, bool>>> = Mutex::new(None);
    if let Some(v) = CACHE.lock().ok().and_then(|g| g.as_ref().and_then(|m| m.get(&pid).copied())) {
        return v;
    }
    let v = !super::noki_browser::ist_noki_browser(pid)
        && super::lesezeichen::app_fuer_pid(pid).is_some_and(|(_, _, pfad)| {
            let fw = std::path::Path::new(&pfad).join("Contents/Frameworks");
            ["Electron Framework.framework", "Chromium Embedded Framework.framework", "Google Chrome Framework.framework"]
                .iter()
                .any(|f| fw.join(f).exists())
        });
    if let Ok(mut g) = CACHE.lock() {
        g.get_or_insert_with(HashMap::new).insert(pid, v);
    }
    v
}

#[cfg(target_os = "macos")]
/// Browser with a real per-window tab strip Noki can read via AX: Chrome
/// (and Noki's own browser) and Safari. Chrome uses AXTabButton/number;
/// Safari uses AXRadioButton/CFBoolean in the exact window's AXTabGroup.
fn ist_chrome(pid: i32) -> bool {
    super::noki_browser::ist_noki_browser(pid)
        || super::lesezeichen::app_fuer_pid(pid)
            .is_some_and(|(_, b, _)| matches!(b.as_str(), "com.google.Chrome" | "com.apple.Safari" | "com.apple.SafariTechnologyPreview"))
}

#[cfg(target_os = "macos")]
fn ist_safari(pid: i32) -> bool {
    super::lesezeichen::app_fuer_pid(pid)
        .is_some_and(|(_, b, _)| matches!(b.as_str(), "com.apple.Safari" | "com.apple.SafariTechnologyPreview"))
}

fn tab_schluessel(titel: &str, vorkommen: usize) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in titel.bytes() { h = (h ^ b as u64).wrapping_mul(0x100000001b3); }
    format!("safari:{h:016x}:{vorkommen}")
}

#[cfg(target_os = "macos")]
#[derive(Clone)]
struct SafariTab {
    key: String,
    title: String,
    url: String,
    active: bool,
    index: usize,
}

/// Safari's native tab bar is not Chrome's AXTabGroup/AXTabButton model.
/// Its scripting window id is the same stable CGWindowID used by the rail,
/// so this reads only the tabs belonging to the exact requested window.
#[cfg(target_os = "macos")]
fn safari_tabs(id: i64) -> Option<Vec<SafariTab>> {
    if id <= 0 { return None; }
    let script = format!(r#"
tell application "Safari"
  if not (exists window id {id}) then error "window missing"
  -- Safari keeps CLOSED windows in its scripting model for a while: a
  -- closed window is not visible (and not minimized).
  if (not (visible of window id {id})) and (not (miniaturized of window id {id})) then error "window missing"
  tell window id {id}
    set activeIndex to index of current tab
    set answer to ""
    repeat with i from 1 to count of tabs
      set t to tab i
      set answer to answer & i & (character id 31) & (name of t as string) & (character id 31) & (URL of t as string) & (character id 31) & ((i = activeIndex) as string) & (character id 30)
    end repeat
    return answer
  end tell
end tell
"#);
    let output = std::process::Command::new("/usr/bin/osascript")
        .arg("-e").arg(script).output().ok()?;
    if !output.status.success() {
        // The window no longer exists: that is an authoritative EMPTY tab
        // set (its entries must vanish), not "unreadable right now".
        if String::from_utf8_lossy(&output.stderr).contains("window missing") { return Some(Vec::new()); }
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut tabs = Vec::new();
    for record in text.split('\u{1e}').filter(|s| !s.trim().is_empty()) {
        let fields: Vec<&str> = record.trim().split('\u{1f}').collect();
        if fields.len() != 4 { continue; }
        let index = fields[0].trim().parse::<usize>().ok()?;
        let title = fields[1].trim().to_string();
        let url = fields[2].trim().to_string();
        let identity = if url.is_empty() { title.clone() } else { url.clone() };
        let occurrence = seen.entry(identity.clone()).or_default();
        let key = tab_schluessel(&identity, *occurrence);
        *occurrence += 1;
        tabs.push(SafariTab {
            key, title, url,
            active: fields[3].trim().eq_ignore_ascii_case("true"),
            index,
        });
    }
    (!tabs.is_empty()).then_some(tabs)
}

#[cfg(target_os = "macos")]
fn safari_tab_aktivieren(id: i64, key: &str) -> bool {
    let Some(tabs) = safari_tabs(id) else { return false };
    let Some(index) = tabs.iter().find(|t| t.key == key).map(|t| t.index) else { return false };
    let script = format!(r#"
tell application "Safari"
  if not (exists window id {id}) then error "window missing"
  tell window id {id}
    set current tab to tab {index}
    return index of current tab
  end tell
end tell
"#);
    let Ok(output) = std::process::Command::new("/usr/bin/osascript")
        .arg("-e").arg(script).output() else { return false };
    output.status.success()
        && String::from_utf8_lossy(&output.stdout).trim().parse::<usize>().ok() == Some(index)
}

#[cfg(target_os = "macos")]
fn tab_index_fuer_schluessel(
    liste: &[(String, bool, *mut std::ffi::c_void)], key: &str,
) -> Option<usize> {
    if let Some(i) = key.strip_prefix("ax:").and_then(|s| s.parse::<usize>().ok()) {
        return (i < liste.len()).then_some(i);
    }
    if !key.starts_with("safari:") { return None; }
    let mut gesehen: HashMap<&str, usize> = HashMap::new();
    liste.iter().position(|(titel, _, _)| {
        let vorkommen = gesehen.entry(titel.as_str()).or_default();
        let passt = tab_schluessel(titel, *vorkommen) == key;
        *vorkommen += 1;
        passt
    })
}

/// Tabs of exactly this Chrome window, with sharp previews where known.
#[tauri::command]
/// `None` = could not be read right now (keep what is known); `Some` = the
/// window's real tabs (a single tab means: no paging).
pub async fn window_overview_tabs(id: i64, pid: i32) -> Option<Vec<TabInfo>> {
    tauri::async_runtime::spawn_blocking(move || tabs(id, pid)).await.ok().flatten()
}

#[cfg(target_os = "macos")]
fn tabs(id: i64, pid: i32) -> Option<Vec<TabInfo>> {
    if !ist_chrome(pid) { return Some(Vec::new()); }
    let (mw, mh) = vorschau_px();
    if ist_safari(pid) {
        let tabs = safari_tabs(id)?;
        if tabs.len() < 2 { return Some(Vec::new()); }
        return Some(tabs.into_iter().map(|t| TabInfo {
            preview: tab_bild_suchen(id, &t.title)
                .map(|h| bild_url(&format!("t/{id}/{h}"))).unwrap_or_default(),
            key: t.key, title: t.title, url: t.url, active: t.active,
        }).collect());
    }
    if super::noki_browser::ist_noki_browser(pid) {
        let mut tabs = super::noki_browser::tabs_fuer_fenster(id)?;
        if tabs.len() < 2 { return Some(Vec::new()); }
        // CDP lists targets by recency, not by tab strip. The strip order
        // comes from this exact window's AX tab buttons (titles, matched
        // one by one); off-Space the last known order is kept.
        let strip: Vec<String> = unsafe { ax_tabs(pid, id) }.into_iter().map(|(t, _, el)| {
            #[link(name = "CoreFoundation", kind = "framework")]
            extern "C" { fn CFRelease(o: *const std::ffi::c_void); }
            unsafe { CFRelease(el as *const std::ffi::c_void); }
            t
        }).collect();
        let reihe: Vec<String> = if strip.len() == tabs.len() {
            let mut frei: Vec<Option<&super::noki_browser::Tab>> = tabs.iter().map(Some).collect();
            let r: Vec<String> = strip.iter().filter_map(|titel| {
                let i = frei.iter().position(|t| t.is_some_and(|t| &t.titel == titel))?;
                frei[i].take().map(|t| t.id.clone())
            }).collect();
            if r.len() == tabs.len() {
                if let Ok(mut g) = TAB_REIHE.lock() { g.get_or_insert_with(HashMap::new).insert(id, r.clone()); }
            }
            r
        } else {
            TAB_REIHE.lock().ok().and_then(|g| g.as_ref().and_then(|m| m.get(&id).cloned())).unwrap_or_default()
        };
        if !reihe.is_empty() {
            let pos = |t: &super::noki_browser::Tab| reihe.iter().position(|k| *k == t.id).unwrap_or(usize::MAX);
            tabs.sort_by_key(pos);
        }
        // Previews in parallel (background tabs render in this browser).
        let bilder: Mutex<HashMap<String, String>> = Mutex::new(HashMap::new());
        std::thread::scope(|sc| {
            for t in &tabs {
                let bilder = &bilder;
                sc.spawn(move || {
                    if let Some(b) = super::noki_browser::tab_bild(&t.id, mw, mh) {
                        if let Ok(mut g) = bilder.lock() { g.insert(t.id.clone(), b); }
                    }
                });
            }
        });
        let bilder = bilder.into_inner().unwrap_or_default();
        return Some(tabs.into_iter().map(|t| TabInfo {
            preview: bilder.get(&t.id).cloned().unwrap_or_default(),
            key: t.id, title: t.titel, url: t.url, active: t.aktiv,
        }).collect());
    }
    // Regular Chrome: AX tab strip (only renders its active tab; the other
    // pictures are the ones last seen while that tab was active).
    let liste = unsafe { ax_tabs(pid, id) };
    let n = liste.len();
    let out: Vec<TabInfo> = liste.iter().enumerate().map(|(i, (titel, aktiv, _))| {
        let key = format!("ax:{i}");
        TabInfo {
        key,
        title: titel.clone(),
        url: String::new(),
        active: *aktiv,
        preview: tab_bild_suchen(id, titel).map(|h| bild_url(&format!("t/{id}/{h}"))).unwrap_or_default(),
    }}).collect();
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" { fn CFRelease(o: *const std::ffi::c_void); }
    for (_, _, el) in liste { unsafe { CFRelease(el as *const std::ffi::c_void); } }
    // An empty strip means "not readable now" (e.g. the app did not answer
    // within the budget), not "no tabs" - never wipe known tabs for that.
    if n == 0 { None } else if n < 2 { Some(Vec::new()) } else { Some(out) }
}

#[cfg(not(target_os = "macos"))]
fn tabs(_id: i64, _pid: i32) -> Option<Vec<TabInfo>> { Some(Vec::new()) }

/// Explicit click on a paged tab preview: select EXACTLY that tab in
/// EXACTLY that window, then bring the window front (same path as a card
/// click). Browsing the pages never changes Chrome; only this does.
#[tauri::command]
pub async fn window_overview_tab_activate(app: tauri::AppHandle, id: i64, pid: i32, key: String) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _busy = super::UiBusy::neu("window_overview_tab_activate");
        window_overview_tab_activate_sync(app, id, pid, key)
    }).await.unwrap_or_else(|e| Err(e.to_string()))
}
fn window_overview_tab_activate_sync(app: tauri::AppHandle, id: i64, pid: i32, key: String) -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        let gewaehlt = if key.starts_with("safari:") && ist_safari(pid) {
            safari_tab_aktivieren(id, &key)
        } else if key.starts_with("ax:") {
            unsafe {
                #[link(name = "ApplicationServices", kind = "framework")]
                extern "C" {
                    fn AXUIElementPerformAction(el: *mut std::ffi::c_void, a: *const std::ffi::c_void) -> i32;
                }
                #[link(name = "CoreFoundation", kind = "framework")]
                extern "C" {
                    fn CFRelease(o: *const std::ffi::c_void);
                    fn CFStringCreateWithCString(a: *const std::ffi::c_void, s: *const i8, enc: u32) -> *const std::ffi::c_void;
                }
                let liste = ax_tabs(pid, id);
                let mut ok = false;
                let ziel_index = tab_index_fuer_schluessel(&liste, &key);
                let ziel = ziel_index.and_then(|i| liste.get(i));
                if let Some((_, schon, el)) = ziel {
                    let c = std::ffi::CString::new("AXPress").unwrap();
                    let a = CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), 0x08000100);
                    ok = *schon || AXUIElementPerformAction(*el, a) == 0;
                    CFRelease(a);
                    if ok && !*schon {
                        ok = false;
                        for _ in 0..15 {
                            let neu = ax_tabs(pid, id);
                            let aktiv = tab_index_fuer_schluessel(&neu, &key)
                                .and_then(|i| neu.get(i)).is_some_and(|(_, an, _)| *an);
                            for (_, _, e) in neu { CFRelease(e as *const std::ffi::c_void); }
                            if aktiv { ok = true; break; }
                            std::thread::sleep(Duration::from_millis(20));
                        }
                    }
                }
                for (_, _, el) in liste { CFRelease(el as *const std::ffi::c_void); }
                ok
            }
        } else {
            super::noki_browser::tab_aktivieren(&key)
        };
        if !gewaehlt {
            return Err("Der Tab ließ sich nicht auswählen".into());
        }
        window_overview_action_sync(app, id, pid, "activate".into())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, id, pid, key);
        Err("Nur auf macOS".into())
    }
}

#[derive(serde::Serialize)]
pub struct LiveFrame {
    pub id: i64,
    pub preview: String,
    /// Frame generation of this window (advances only on new content).
    pub gen: u64,
}

/// Space-swipe fast path: while a Dock gesture runs (and 300 ms after it)
/// no card is captured. The cards keep their last valid frame; WindowServer
/// is not asked for window images in the middle of the slide (it can hand
/// back an all-black one there). Set by the helper ("WISCH 1/0").
static GESTE_AKTIV: AtomicBool = AtomicBool::new(false);
static GESTE_ENDE: Mutex<Option<Instant>> = Mutex::new(None);
pub fn geste(aktiv: bool) {
    GESTE_AKTIV.store(aktiv, Ordering::Relaxed);
    if let Ok(mut g) = GESTE_ENDE.lock() {
        *g = (!aktiv).then(Instant::now);
    }
}
pub(crate) fn geste_laeuft() -> bool {
    GESTE_AKTIV.load(Ordering::Relaxed)
        || GESTE_ENDE.lock().ok().and_then(|g| *g).is_some_and(|t| t.elapsed() < Duration::from_millis(300))
}

/// Live loop of the page: fresh sharp frames for the VISIBLE cards only;
/// windows whose content did not change are not resent.
#[tauri::command]
pub async fn window_overview_frames(ids: Vec<i64>) -> Vec<LiveFrame> {
    if geste_laeuft() {
        return Vec::new();
    }
    tauri::async_runtime::spawn_blocking(move || frames(ids)).await.unwrap_or_default()
}

/// (rounds, changed frames, capture ms, window start) for the rate audit.
static LIVE_STAT: Mutex<(u64, u64, u64, Option<Instant>)> = Mutex::new((0, 0, 0, None));
/// Per window over the same 5 s window: (requested, captured, failed,
/// unique new frames) - the live-chain audit (source -> card).
static LIVE_FENSTER: Mutex<Option<HashMap<i64, [u32; 4]>>> = Mutex::new(None);

#[cfg(target_os = "macos")]
fn frames(ids: Vec<i64>) -> Vec<LiveFrame> {
    if !is_open() || super::global_verborgen() {
        return Vec::new();
    }
    let t0 = Instant::now();
    let (mw, mh) = vorschau_px();
    let info = FENSTER_INFO.lock().ok().and_then(|g| g.clone()).unwrap_or_default();
    // Pacing per window from ITS content: changing -> every round (~8/s);
    // unchanged for 1.5 s -> probed every 300 ms; the first change found
    // puts it straight back on every round. Nothing is ever resent twice.
    let jobs: Vec<(i64, CaptureRect, Option<[f64; 4]>, Option<u64>)> = ids
        .iter()
        .take(6)
        .filter(|id| bild(**id).map_or(true, |b| {
            b.geaendert.elapsed() < Duration::from_millis(1500) || b.erfasst.elapsed() >= Duration::from_millis(300)
        }))
        .filter_map(|id| info.get(id).map(|(r, l)| (*id, *r, *l, bild(*id).map(|b| b.hash))))
        .collect();
    let out: Mutex<Vec<(i64, Vec<u8>, u64)>> = Mutex::new(Vec::new());
    let titel_vorher: Mutex<HashMap<i64, Option<String>>> = Mutex::new(HashMap::new());
    std::thread::scope(|sc| {
        for (id, r, l, bekannt) in &jobs {
            let out = &out;
            let titel_vorher = &titel_vorher;
            sc.spawn(move || {
                if ist_browser_fenster(*id) {
                    if let Ok(mut g) = titel_vorher.lock() { g.insert(*id, titel_jetzt(*id)); }
                }
                if let Some((uri, h)) = unsafe { capture_scharf(*id, *r, *l, mw, mh, *bekannt) } {
                    if let Ok(mut v) = out.lock() {
                        v.push((*id, uri, h));
                    }
                }
            });
        }
    });
    let titel_vorher = titel_vorher.into_inner().unwrap_or_default();
    let mut neu = Vec::new();
    let ergebnisse = out.into_inner().unwrap_or_default();
    if let Ok(mut g) = LIVE_FENSTER.lock() {
        let m = g.get_or_insert_with(HashMap::new);
        for id in &ids { m.entry(*id).or_default()[0] += 1; }
        for (id, ..) in &jobs {
            let e = m.entry(*id).or_default();
            if ergebnisse.iter().any(|(i, ..)| i == id) { e[1] += 1 } else { e[2] += 1 }
        }
    }
    for (id, png, h) in ergebnisse {
        if let Some(gen) = bild_merken(id, png, h) {
            tab_bild_merken(id, titel_vorher.get(&id).cloned().flatten());
            if let Ok(mut g) = LIVE_FENSTER.lock() { g.get_or_insert_with(HashMap::new).entry(id).or_default()[3] += 1; }
            neu.push(LiveFrame { id, preview: bild_url(&format!("w/{id}/{gen}")), gen });
        }
    }
    if let Ok(mut st) = LIVE_STAT.lock() {
        st.0 += 1;
        st.1 += neu.len() as u64;
        st.2 += t0.elapsed().as_millis() as u64;
        let seit = *st.3.get_or_insert_with(Instant::now);
        if seit.elapsed() >= Duration::from_secs(5) {
            let s = seit.elapsed().as_secs_f64();
            let pro: String = LIVE_FENSTER.lock().ok().and_then(|mut g| g.take()).unwrap_or_default()
                .iter().map(|(id, c)| format!(" {id}:req={} cap={} fail={} unique/s={:.1}", c[0], c[1], c[2], c[3] as f64 / s))
                .collect();
            super::virtual_workspace::trace(&format!(
                "[OVERVIEW] live rounds/s={:.1} changed_frames/s={:.1} capture_ms_avg={:.0} cards={} px={}x{}{pro}",
                st.0 as f64 / s, st.1 as f64 / s, st.2 as f64 / st.0.max(1) as f64, jobs.len(), mw, mh
            ));
            *st = (0, 0, 0, Some(Instant::now()));
        }
    }
    neu
}

#[cfg(not(target_os = "macos"))]
fn frames(_ids: Vec<i64>) -> Vec<LiveFrame> {
    Vec::new()
}

#[cfg(target_os = "macos")]
fn png_data(image: *const std::ffi::c_void) -> String {
    let b = png_bytes(image);
    if b.is_empty() { String::new() } else { format!("data:image/png;base64,{}", super::base64(&b)) }
}

#[cfg(target_os = "macos")]
fn png_bytes(image: *const std::ffi::c_void) -> Vec<u8> {
    use std::ffi::c_void;
    type C = *const c_void;
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFDataCreateMutable(a: C, n: isize) -> C;
        fn CFDataGetLength(d: C) -> isize;
        fn CFDataGetBytePtr(d: C) -> *const u8;
        fn CFStringCreateWithCString(a: C, s: *const i8, e: u32) -> C;
        fn CFRelease(o: C);
    }
    #[link(name = "ImageIO", kind = "framework")]
    extern "C" {
        fn CGImageDestinationCreateWithData(d: C, t: C, n: usize, o: C) -> C;
        fn CGImageDestinationAddImage(d: C, i: C, p: C);
        fn CGImageDestinationFinalize(d: C) -> bool;
    }
    if image.is_null() {
        return Vec::new();
    }
    unsafe {
        let data = CFDataCreateMutable(std::ptr::null(), 0);
        let c = std::ffi::CString::new("public.png").unwrap();
        let typ = CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), 0x08000100);
        let dst = CGImageDestinationCreateWithData(data, typ, 1, std::ptr::null());
        let mut out = Vec::new();
        if !dst.is_null() {
            CGImageDestinationAddImage(dst, image, std::ptr::null());
            if CGImageDestinationFinalize(dst) {
                let n = CFDataGetLength(data);
                let p = CFDataGetBytePtr(data);
                if n > 0 && !p.is_null() {
                    out = std::slice::from_raw_parts(p, n as usize).to_vec();
                }
            }
            CFRelease(dst);
        }
        CFRelease(typ);
        CFRelease(data);
        out
    }
}

static APP_ICONS: Mutex<Option<HashMap<i32, String>>> = Mutex::new(None);

#[cfg(target_os = "macos")]
fn app_icon(pid: i32) -> String {
    if let Ok(icons) = APP_ICONS.lock() {
        if let Some(map) = icons.as_ref() {
            if let Some(cached) = map.get(&pid) {
                return cached.clone();
            }
        }
    }
    unsafe {
        use std::ffi::c_void;
        #[link(name = "objc")]
        extern "C" {
            fn objc_getClass(n: *const i8) -> *mut c_void;
            fn sel_registerName(n: *const i8) -> *const c_void;
            fn objc_msgSend();
            fn objc_autoreleasePoolPush() -> *mut c_void;
            fn objc_autoreleasePoolPop(p: *mut c_void);
        }
        let msg = objc_msgSend as unsafe extern "C" fn();
        let s = |n: &[u8]| sel_registerName(n.as_ptr() as _);
        let pool = objc_autoreleasePoolPush();
        let f: unsafe extern "C" fn(*mut c_void, *const c_void, i32) -> *mut c_void =
            std::mem::transmute(msg);
        let app = f(
            objc_getClass(b"NSRunningApplication\0".as_ptr() as _),
            s(b"runningApplicationWithProcessIdentifier:\0"),
            pid,
        );
        let g: unsafe extern "C" fn(*mut c_void, *const c_void) -> *mut c_void =
            std::mem::transmute(msg);
        let icon = if app.is_null() {
            std::ptr::null_mut()
        } else {
            g(app, s(b"icon\0"))
        };
        let out = icon_png(icon);
        objc_autoreleasePoolPop(pool);
        if !out.is_empty() {
            if let Ok(mut icons) = APP_ICONS.lock() {
                let map = icons.get_or_insert_with(HashMap::new);
                map.insert(pid, out.clone());
            }
        }
        out
    }
}

#[cfg(target_os = "macos")]
fn manageable_app(pid: i32) -> bool {
    unsafe {
        use std::ffi::c_void;
        #[link(name = "objc")]
        extern "C" {
            fn objc_getClass(n: *const i8) -> *mut c_void;
            fn sel_registerName(n: *const i8) -> *const c_void;
            fn objc_msgSend();
        }
        let msg = objc_msgSend as unsafe extern "C" fn();
        let f: unsafe extern "C" fn(*mut c_void, *const c_void, i32) -> *mut c_void =
            std::mem::transmute(msg);
        let app = f(
            objc_getClass(b"NSRunningApplication\0".as_ptr() as _),
            sel_registerName(b"runningApplicationWithProcessIdentifier:\0".as_ptr() as _),
            pid,
        );
        if app.is_null() {
            return false;
        }
        let policy: unsafe extern "C" fn(*mut c_void, *const c_void) -> isize =
            std::mem::transmute(msg);
        policy(app, sel_registerName(b"activationPolicy\0".as_ptr() as _)) <= 1
    }
}

#[cfg(test)]
mod tests {
    use super::{column_layout, panel_behavior, Column, CAN_JOIN_ALL_SPACES, FULLSCREEN_AUXILIARY, MOVE_TO_ACTIVE_SPACE};
    fn schnitt(a: [f64; 4], b: [f64; 4]) -> bool {
        a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]
    }
    fn rect(c: &Column, n: usize) -> [f64; 4] { [c.x, c.y, c.w, c.panel_h(n)] }
    fn inside(r: [f64; 4], work: [f64; 4]) -> bool {
        r[0] >= work[0] && r[1] >= work[1] && r[0] + r[2] <= work[0] + work[2] + 0.01
            && r[1] + r[3] <= work[1] + work[3] + 0.01
    }

    const WORK: [f64; 4] = [0.0, 33.0, 1470.0, 923.0];

    #[test]
    fn chrome_tab_titles_lose_the_memory_status() {
        use super::tab_titel;
        assert_eq!(tab_titel("Sign into Codex – Arbeitsspeichernutzung – 31,6 MB"), "Sign into Codex");
        assert_eq!(tab_titel("noki design jarvis - Work und Code korrigieren – Hohe Arbeitsspeichernutzung – 1,2 GB"),
                   "noki design jarvis - Work und Code korrigieren");
        assert_eq!(tab_titel("google one abo - Google Search - Memory usage - 45 MB"), "google one abo - Google Search");
        assert_eq!(tab_titel("A - B – C"), "A - B – C");
        // exact titles from the user's Chrome (NO-BREAK SPACE before "–")
        assert_eq!(tab_titel("noki design jarvis - Work und Code korrigieren\u{a0}– Hohe Arbeitsspeichernutzung\u{a0}– 1.021 MB"),
                   "noki design jarvis - Work und Code korrigieren");
        assert_eq!(tab_titel("ChatGPT Plugins | Browse and add plugins to ChatGPT\u{a0}– Arbeitsspeichernutzung\u{a0}– 339 MB"),
                   "ChatGPT Plugins | Browse and add plugins to ChatGPT");
    }

    #[test]
    fn panel_behavior_never_combines_all_spaces_and_move_to_active() {
        for alt in [0usize, CAN_JOIN_ALL_SPACES, MOVE_TO_ACTIVE_SPACE,
                    CAN_JOIN_ALL_SPACES | MOVE_TO_ACTIVE_SPACE | (1 << 4), usize::MAX] {
            let b = panel_behavior(alt);
            assert_eq!(b & (CAN_JOIN_ALL_SPACES | MOVE_TO_ACTIVE_SPACE), 0, "alt={alt:#x}");
            assert_ne!(b & FULLSCREEN_AUXILIARY, 0);
        }
    }

    #[test]
    fn unloaded_chrome_tab_keeps_its_identity() {
        // exact title from the user's Chrome window A (Memory Saver)
        assert_eq!(super::tab_titel("Sign into Codex\u{a0}– Inaktiver Tab\u{a0}– 31,5 MB freigegeben"), "Sign into Codex");
        assert_eq!(super::tab_titel("Sign into Codex – Inactive tab – 31.5 MB freed"), "Sign into Codex");
        // page titles that merely contain a dash stay whole
        assert_eq!(super::tab_titel("Work - Code korrigieren"), "Work - Code korrigieren");
    }

    #[test]
    fn content_hash_sees_a_single_changed_pixel_anywhere() {
        let a = vec![17u8; 674 * 380 * 4];
        let base = super::inhalt_hash(&a);
        for i in [0usize, 5, 13, 26, 4097, a.len() - 1] {
            let mut b = a.clone();
            b[i] ^= 1;
            assert_ne!(super::inhalt_hash(&b), base, "byte {i}");
        }
    }

    #[test]
    fn desktop_panel_is_sticky_but_never_move_to_active() {
        for alt in [0usize, MOVE_TO_ACTIVE_SPACE, usize::MAX] {
            let b = super::panel_behavior_alle(alt);
            assert_ne!(b & CAN_JOIN_ALL_SPACES, 0);
            assert_ne!(b & super::STATIONARY, 0, "stays put during Space animations");
            assert_eq!(b & MOVE_TO_ACTIVE_SPACE, 0, "alt={alt:#x}");
            assert_ne!(b & FULLSCREEN_AUXILIARY, 0);
        }
    }

    #[test]
    fn column_is_left_miniature_wide_and_three_cards_fit() {
        let mini = [7.0, 680.0, 353.0, 254.0];
        let c = column_layout(WORK, Some(mini), 353.0);
        assert_eq!(c.w, 353.0, "panel outline == Miniatur width");
        assert_eq!(c.x, 7.0, "same left edge as the Miniatur");
        assert!(c.card_w < c.w);
        for n in [1, 2, 3, 4, 8, 30] {
            let r = rect(&c, n);
            assert!(inside(r, WORK), "n={n} {r:?}");
            assert!(!schnitt(r, mini), "n={n} overlaps Miniatur {r:?}");
        }
        assert!(c.card_h >= 120.0, "card_h={}", c.card_h);
    }

    #[test]
    fn column_never_overlaps_miniature_anywhere() {
        for mini in [[7.0, 680.0, 353.0, 254.0], [7.0, 41.0, 353.0, 254.0],
                     [1110.0, 680.0, 353.0, 254.0], [1110.0, 41.0, 353.0, 254.0],
                     [560.0, 350.0, 353.0, 254.0], [7.0, 350.0, 353.0, 254.0]] {
            let c = column_layout(WORK, Some(mini), 353.0);
            for n in [1, 3, 9] {
                let r = rect(&c, n);
                assert!(inside(r, WORK), "{mini:?} n={n} {r:?}");
                assert!(!schnitt(r, mini), "{mini:?} n={n} {r:?}");
            }
            assert!(c.x + c.w <= 1470.0 / 2.0 + 30.0, "stays on the left: {mini:?} x={}", c.x);
        }
    }

    #[test]
    fn layout_is_deterministic_and_never_accumulates() {
        let mini = [7.0, 680.0, 353.0, 254.0];
        let a = column_layout(WORK, Some(mini), 353.0);
        for _ in 0..100 {
            assert_eq!(column_layout(WORK, Some(mini), 353.0), a);
        }
    }

    #[test]
    fn column_without_miniature_uses_full_height() {
        let c = column_layout(WORK, None, 353.0);
        assert!(inside(rect(&c, 3), WORK));
        assert!(c.card_h > 200.0, "{}", c.card_h);
    }

    #[test]
    fn small_screens_still_fit_three_cards() {
        let work = [0.0, 25.0, 1024.0, 640.0];
        let mini = [7.0, 430.0, 353.0, 230.0];
        let c = column_layout(work, Some(mini), 353.0);
        assert!(inside(rect(&c, 3), work));
        assert!(!schnitt(rect(&c, 3), mini));
    }

    #[test]
    fn test_current_window_list_fast() {
        let t0 = std::time::Instant::now();
        let windows = super::list(true);
        let elapsed = t0.elapsed();
        println!(
            "TOTAL WINDOWS FOUND: {} in {}ms",
            windows.len(),
            elapsed.as_millis()
        );
        for w in &windows {
            println!(
                "  [WIN] id={} pid={} app='{}' title='{}' preview_len={} min={}",
                w.id,
                w.pid,
                w.app,
                w.title,
                w.preview.len(),
                w.minimized
            );
        }
    }
    #[test]
    fn test_50_regression_cycles() {
        println!("==> Starting 50 cycles of window overview inventory & preview verification");
        for cycle in 1..=50 {
            let t0 = std::time::Instant::now();
            let windows = super::list(true);
            let elapsed = t0.elapsed();
            assert!(!windows.is_empty(), "Cycle {} must find windows", cycle);
            if let Some(spot) = windows.iter().find(|w| w.app.to_lowercase().contains("spotify")) {
                assert!(!spot.preview.is_empty(), "Cycle {} Spotify preview must not be empty", cycle);
            }
            if cycle % 10 == 0 || cycle == 1 {
                println!("Cycle {}/50: {} windows retrieved in {}ms", cycle, windows.len(), elapsed.as_millis());
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        println!("==> All 50 cycles passed successfully!");
    }
}

/// NSImage -> small PNG data URL (shared by window icons and app-file icons).
#[cfg(target_os = "macos")]
unsafe fn icon_png(icon: *mut std::ffi::c_void) -> String {
    use std::ffi::c_void;
    #[link(name = "objc")]
    extern "C" {
        fn sel_registerName(n: *const i8) -> *const c_void;
        fn objc_msgSend();
    }
    let msg = objc_msgSend as unsafe extern "C" fn();
    let s = |n: &[u8]| sel_registerName(n.as_ptr() as _);
        let h: unsafe extern "C" fn(
            *mut c_void,
            *const c_void,
            *mut c_void,
            *mut c_void,
            *mut c_void,
        ) -> *const c_void = std::mem::transmute(msg);
        let image = if icon.is_null() {
            std::ptr::null()
        } else {
            h(
                icon,
                s(b"CGImageForProposedRect:context:hints:\0"),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        let mut final_image = image;
        let mut small_ctx = std::ptr::null_mut();
        let mut small_im = std::ptr::null();
        let mut cs = std::ptr::null();
        if !image.is_null() {
            #[link(name = "CoreGraphics", kind = "framework")]
            extern "C" {
                fn CGImageGetWidth(i: *const c_void) -> usize;
                fn CGImageGetHeight(i: *const c_void) -> usize;
                fn CGColorSpaceCreateDeviceRGB() -> *const c_void;
                fn CGColorSpaceRelease(s: *const c_void);
                fn CGBitmapContextCreate(
                    d: *mut c_void,
                    w: usize,
                    h: usize,
                    b: usize,
                    row: usize,
                    s: *const c_void,
                    info: u32,
                ) -> *mut c_void;
                fn CGContextDrawImage(c: *mut c_void, r: CaptureRect, i: *const c_void);
                fn CGBitmapContextCreateImage(c: *mut c_void) -> *const c_void;
                fn CGContextRelease(c: *const c_void);
                fn CGImageRelease(i: *const c_void);
            }
            let sw = CGImageGetWidth(image);
            let sh = CGImageGetHeight(image);
            if sw > 64 || sh > 64 {
                cs = CGColorSpaceCreateDeviceRGB();
                small_ctx = CGBitmapContextCreate(std::ptr::null_mut(), 48, 48, 8, 48 * 4, cs, 1);
                if !small_ctx.is_null() {
                    CGContextDrawImage(
                        small_ctx,
                        CaptureRect {
                            origin: [0.0, 0.0],
                            size: [48.0, 48.0],
                        },
                        image,
                    );
                    small_im = CGBitmapContextCreateImage(small_ctx);
                    if !small_im.is_null() {
                        final_image = small_im;
                    }
                }
            }
        }
        let out = png_data(final_image);
        if !small_im.is_null() {
            #[link(name = "CoreGraphics", kind = "framework")]
            extern "C" {
                fn CGImageRelease(i: *const c_void);
            }
            CGImageRelease(small_im);
        }
        if !small_ctx.is_null() {
            #[link(name = "CoreGraphics", kind = "framework")]
            extern "C" {
                fn CGContextRelease(c: *const c_void);
            }
            CGContextRelease(small_ctx);
        }
        if !cs.is_null() {
            #[link(name = "CoreGraphics", kind = "framework")]
            extern "C" {
                fn CGColorSpaceRelease(s: *const c_void);
            }
            CGColorSpaceRelease(cs);
        }
    out
}

/// Icon of an application bundle (or any file) by path, as PNG data URL.
#[cfg(target_os = "macos")]
pub fn datei_icon(pfad: &str) -> String {
    unsafe {
        use std::ffi::c_void;
        #[link(name = "objc")]
        extern "C" {
            fn objc_getClass(n: *const i8) -> *mut c_void;
            fn sel_registerName(n: *const i8) -> *const c_void;
            fn objc_msgSend();
            fn objc_autoreleasePoolPush() -> *mut c_void;
            fn objc_autoreleasePoolPop(p: *mut c_void);
        }
        let msg = objc_msgSend as unsafe extern "C" fn();
        let s = |n: &[u8]| sel_registerName(n.as_ptr() as _);
        let pool = objc_autoreleasePoolPush();
        let g: unsafe extern "C" fn(*mut c_void, *const c_void) -> *mut c_void = std::mem::transmute(msg);
        let fs: unsafe extern "C" fn(*mut c_void, *const c_void, *const i8) -> *mut c_void = std::mem::transmute(msg);
        let fi: unsafe extern "C" fn(*mut c_void, *const c_void, *mut c_void) -> *mut c_void = std::mem::transmute(msg);
        let cp = std::ffi::CString::new(pfad).unwrap_or_default();
        let ns = fs(objc_getClass(b"NSString\0".as_ptr() as _), s(b"stringWithUTF8String:\0"), cp.as_ptr());
        let ws = g(objc_getClass(b"NSWorkspace\0".as_ptr() as _), s(b"sharedWorkspace\0"));
        let icon = if ws.is_null() || ns.is_null() { std::ptr::null_mut() } else { fi(ws, s(b"iconForFile:\0"), ns) };
        let out = if icon.is_null() { String::new() } else { icon_png(icon) };
        objc_autoreleasePoolPop(pool);
        out
    }
}
#[cfg(not(target_os = "macos"))]
pub fn datei_icon(_: &str) -> String { String::new() }
