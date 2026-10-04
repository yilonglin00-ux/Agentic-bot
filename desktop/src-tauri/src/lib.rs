//! Noki – Desktop-App (Phase 3.14)
//!
//! Nach aussen heisst diese App JARVIS. Ihr sichtbarer Teil ist Noki: ein
//! rahmenloses, durchsichtiges Panel ohne Titelleiste und ohne Dock-Symbol.
//! Bedient wird sie ueber die Menueleiste.
//!
//! Die native Seite tut bewusst wenig. Sie
//!
//!   1. LIEST drei bereits vorhandene Dateien des JARVIS-Cores und meldet
//!      sie als Ereignisse ins Frontend (Zustand, Sprechpegel, Lebenszeichen),
//!   2. verwaltet das FENSTER (zeigen, verbergen, verschieben, Vordergrund),
//!   3. haelt ein Symbol in der Menueleiste.
//!
//! Sie entscheidet nichts ueber Nokis Verhalten — was ein Zustand oder ein
//! Ereignis bedeutet, steht ausschliesslich im Frontend. Es gibt weiterhin
//! KEINEN Rueckkanal in den Action Layer: hier wird nur gelesen, nie
//! geschrieben, und nichts ausgefuehrt.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, SystemTime},
};

pub mod arbeitsplatz;
pub mod ask_fern;
pub mod attachments;
pub mod capability;
pub mod cloud_engine;
pub mod code_agent;
pub mod code_terminal;
pub mod data_analysis;
pub mod doc_chunks;
pub mod doc_output;
mod document_pipeline;
pub mod intelligence;
#[cfg(target_os = "macos")]
mod fokus_sitzung;
pub mod intent;
pub mod task_plan;
pub mod code_vorschau;
pub mod code_cli;
pub mod terminal_absicht;
pub mod mcp;
pub mod mcp_client;
pub mod mcp_policy;
mod memory;
mod noki_talk;
mod talk_kern;
mod kamera_kern;
mod kamera_galerie;
pub mod model_manager;
pub mod modell_rollen;
pub mod model_registry;
pub mod pdf;
mod permissions;
pub mod providers;
pub mod quality;
pub mod reasoning;
mod research;
pub mod router;
pub mod runtime_registry;
pub mod shell_terminal;
pub mod specialist;
pub mod tool_select;
pub mod vorschau;
pub mod fern_tippen;
pub mod noki_browser;
#[cfg(target_os = "macos")]
pub mod kind_fenster;
#[cfg(target_os = "macos")]
pub mod mission_control;
pub mod virtual_workspace;
pub mod vscode_bruecke;
pub mod web_gateway;
pub mod window_overview;
mod working_context;

use tauri::{
    image::Image,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::TrayIconBuilder,
    Emitter, Manager, PhysicalPosition, WebviewWindow,
};

const VALID_STATES: [&str; 5] = ["STANDBY", "LISTENING", "PROCESSING", "SPEAKING", "ERROR"];

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const MAX_STATE_BYTES: usize = 64;

/// Der Sprechpegel wird vom Core mit 30 Hz veroeffentlicht
/// (lib/visual_envelope.py, CONTROL_RATE = 30). Schneller zu lesen brächte
/// nichts, langsamer verschluckte Silben.
const VOICE_INTERVAL: Duration = Duration::from_millis(33);
const MAX_VOICE_BYTES: usize = 256;

/// Das Lebenszeichen ist eine LEERE Datei; gewertet wird nur ihre mtime.
/// Der Leser wartet bewusst laenger als der Schreiber (2 s Takt), damit ein
/// einzelner verzoegerter Tick nicht als "weg" gilt.
const BEAT_INTERVAL: Duration = Duration::from_millis(1000);
const BEAT_FRISCH: Duration = Duration::from_secs(6);

const FENSTER: &str = "main";

/// Temporary P0 trace for every native operation that can change the active
/// macOS Space. Keep the schema stable so reports from unrelated paths can be
/// correlated without guessing from prose logs.
pub(crate) fn space_action_log(reason: &str, app: &str, window: impl std::fmt::Display, before: u64, after: u64) {
    virtual_workspace::trace(&format!(
        "SPACE_ACTION reason={reason} app={app} window={window} before_space={before} after_space={after}"
    ));
    // macOS finishes an activation-driven Space switch 150-500 ms AFTER the
    // call returns ("Current Space" flips at the end of the slide), so the
    // immediate after_space above reads "same" for exactly the switches we
    // hunt. Remember the action for SPACE_AUDIT and re-check once, late.
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64).unwrap_or(0);
    if let Ok(mut g) = LETZTE_SPACE_AKTION.lock() { *g = (ms, format!("{reason}/{app}")); }
    #[cfg(target_os = "macos")]
    if before != 0 {
        let (reason, app, window) = (reason.to_string(), app.to_string(), window.to_string());
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(700));
            let spaet = cgs::aktiver_space().map(|x| x.0).unwrap_or(0);
            if spaet != 0 && spaet != before {
                virtual_workspace::trace(&format!(
                    "SPACE_ACTION_LATE_SWITCH reason={reason} app={app} window={window} before_space={before} after_700ms={spaet}"
                ));
            }
        });
    }
}
/// (ms since epoch, "reason/app") of the last SPACE_ACTION - SPACE_AUDIT
/// prints it next to every physical change.
static LETZTE_SPACE_AKTION: Mutex<(u64, String)> = Mutex::new((0, String::new()));
fn letzte_space_aktion() -> (u64, String) {
    LETZTE_SPACE_AKTION.lock().map(|g| g.clone()).unwrap_or_default()
}
const ASK_FENSTER: &str = "ask";
/// Noki Einstellungen: ein eigenes, normales Fenster (Ebene 0), NICHT Teil
/// des Character-Overlays. Inhalt und Logik bleiben im Hauptfenster; es
/// spiegelt nur das gerenderte DOM hierher (settings.html).
const EINST_FENSTER: &str = "einstellungen";
static ASK_WID: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// THE one rule for Noki-owned windows in the user window inventory that
/// the Miniatur capture set and Shortcut 9 both read: Ask Noki is a real
/// user window and counts exactly while WindowServer has it ordered in on
/// a Space (shown; not hidden by Shortcut 0, not minimized). Every other
/// Noki surface (character overlay, panels, capture helper) stays excluded.
#[cfg(target_os = "macos")]
pub(crate) fn ask_nutzerfenster() -> Option<i64> {
    let wid = ASK_WID.load(Ordering::Relaxed);
    (wid > 0
        && cgs::fenster_eingeordnet(wid)
        && cgs::spaces_des_fensters(wid).is_some_and(|s| !s.is_empty()))
    .then_some(wid)
}
#[cfg(not(target_os = "macos"))]
pub(crate) fn ask_nutzerfenster() -> Option<i64> { None }

pub(crate) fn ask_fenster(app: &tauri::AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(ASK_FENSTER)
}

/// Window actions on the EXISTING Ask window from Shortcut 9 (never a
/// rebuild, never a Space adoption - Shortcut 9 already navigated to its
/// Space - and no open payload: chat, session and project stay untouched).
/// "close" is the Shortcut 0 hide, so the conversation survives.
pub(crate) fn ask_fensteraktion(app: &tauri::AppHandle, aktion: &str) -> bool {
    if aktion == "close" {
        ask_fenster_verbergen(app.clone());
        return true;
    }
    let Some(win) = ask_fenster(app) else { return false };
    let aktion = aktion.to_string();
    app.run_on_main_thread(move || {
        match aktion.as_str() {
            "minimize" => { let _ = win.minimize(); }
            "enlarge" => {
                if win.is_maximized().unwrap_or(false) { let _ = win.unmaximize(); } else { let _ = win.maximize(); }
            }
            _ => {
                let _ = win.unminimize();
                let _ = win.show();
                let _ = win.set_focus();
            }
        }
    })
    .is_ok()
}
static ASK_FRONTEND_READY: AtomicBool = AtomicBool::new(false);
static ASK_SHOW_PENDING: AtomicBool = AtomicBool::new(false);
static ASK_OPEN_PAYLOAD: Mutex<Option<serde_json::Value>> = Mutex::new(None);

/// Die eigene Benutzerkennung, ohne zusaetzliche Abhaengigkeit: das
/// Heimatverzeichnis gehoert dem angemeldeten Benutzer, und dessen uid steht
/// in den Dateiangaben. Fuer eine einzige Zahl waere `libc` zu viel.
#[cfg(unix)]
fn eigene_uid() -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    let home = std::env::var_os("HOME")?;
    fs::metadata(home).ok().map(|m| m.uid())
}

#[cfg(not(unix))]
fn eigene_uid() -> Option<u32> {
    None
}

/// Wo der Core seine Laufzeitdateien ablegt.
///
/// Aus einer Shell gestartet steht `JARVIS_RUNTIME_DIR` in der Umgebung und
/// gilt unveraendert. Aus dem Finder gestartet erbt die App keine
/// Shell-Umgebung — dann wird derselbe Pfad abgeleitet wie in `lib/core.sh`:
///
/// ```text
/// ${TMPDIR:-/tmp}/jarvis-$UID
/// ```
///
/// Das ist die Spiegelung genau EINER Zeile, keine zweite Definition. Aendert
/// sich die Zeile dort, aendert sie sich hier mit; massgeblich bleibt
/// `lib/core.sh`.
fn runtime_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("JARVIS_RUNTIME_DIR") {
        return Some(PathBuf::from(dir));
    }
    let tmp = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    Some(tmp.join(format!("jarvis-{}", eigene_uid()?)))
}

fn runtime_state_file() -> Option<PathBuf> {
    runtime_dir().map(|dir| dir.join("visual.state"))
}

// =====================================================================
//  1 · ZUSTAND  (Phase 8.2, unveraendert)
// =====================================================================
// =====================================================================
//  2 · SPRECHPEGEL  (Phase 3.14, Abschnitt 12)
//
//  Die Quelle ist die BESTEHENDE TTS-Huellkurve des Cores. lib/voice.sh
//  laesst lib/visual_envelope.py die WAV vor dem Abspielen analysieren und
//  veroeffentlicht daraus 30-mal je Sekunde eine Zeile der Form
//
//      owner=<pid> generation=<n> sequence=<n> energy=0.123456 active=1
//
//  in $JARVIS_RUNTIME_DIR/visual.voice. Hier wird sie nur gelesen und als
//  Zahl weitergereicht. Es wird KEIN Audio analysiert, kein Mikrofon
//  geoeffnet und keine neue Pipeline gebaut; owner und generation sind
//  Angelegenheit des Schreibers und werden hier bewusst ignoriert.
// =====================================================================
fn voice_sample(raw: &str) -> Option<(f64, bool)> {
    let mut energy = None;
    let mut active = false;
    for feld in raw.split_whitespace() {
        let Some((k, v)) = feld.split_once('=') else {
            continue;
        };
        match k {
            "energy" => energy = v.parse::<f64>().ok(),
            "active" => active = v == "1",
            _ => {}
        }
    }
    let e = energy?;
    if !e.is_finite() {
        return None;
    }
    Some((e.clamp(0.0, 1.0), active))
}

// =====================================================================
//  3 · UMGEBUNG  (Phase 3.14, Abschnitt 19)
//
//  Die einzige heute angeschlossene Umgebungsquelle ist das VORHANDENE
//  Lebenszeichen des JARVIS-Cores (lib/visual.sh, jarvis-core.heartbeat).
//  Es ist eine leere Datei; gewertet wird ausschliesslich ihre mtime.
//
//  Bewusst NICHT gebaut: Fensterbeobachtung, Browser-Tabs, Bildschirm-
//  aufnahme, Accessibility. Wer solche Ereignisse spaeter melden will,
//  meldet sie ueber dieselbe Schnittstelle (noki://umwelt) mit einem Wort
//  aus dem festen Wortschatz des Frontends — hier wird nichts ausgelesen,
//  was der Benutzer nicht ohnehin selbst gestartet hat.
// =====================================================================
// =====================================================================
//  3b · MAUSPOSITION  (Abschnitt 9)
//
//  Liefert die Cursorposition auf dem Schreibtisch fuer Blickkontakt.
// =====================================================================
#[cfg(target_os = "macos")]
pub fn maus_schirm_position() -> Option<(f64, f64)> {
    #[repr(C)]
    struct CGPoint {
        x: f64,
        y: f64,
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventCreate(source: *const std::ffi::c_void) -> *mut std::ffi::c_void;
        fn CGEventGetLocation(event: *mut std::ffi::c_void) -> CGPoint;
        fn CFRelease(cf: *const std::ffi::c_void);
    }
    unsafe {
        let ev = CGEventCreate(std::ptr::null());
        if ev.is_null() {
            return None;
        }
        let pt = CGEventGetLocation(ev);
        CFRelease(ev);
        Some((pt.x, pt.y))
    }
}

#[cfg(not(target_os = "macos"))]
pub fn maus_schirm_position() -> Option<(f64, f64)> {
    None
}

// =====================================================================
//  3c · SICHTBARE FENSTER  (Abschnitt 20)
//
//  Noki soll auf dem Schreibtisch nicht durch offene Fenster hindurch
//  laufen. Dafuer braucht er ausschliesslich GEOMETRIE: wo ein sichtbares
//  Fenster liegt und wie gross es ist.
//
//  Quelle ist CGWindowListCopyWindowInfo. Sie ist bewusst gewaehlt, weil
//  sie OHNE zusaetzliche Berechtigung auskommt: Bildschirmaufnahme wird
//  nur fuer kCGWindowName (den Fenstertitel) verlangt, und genau der wird
//  hier NICHT gelesen. Abgefragt werden Rechteck, Ebene, Deckkraft und
//  der Programmname des Besitzers. Kein Titel, kein Inhalt, kein Bild,
//  kein Bedienungshilfen-Zugriff.
//
//  Die Rechtecke kommen bereits in logischen Punkten und im selben
//  Schreibtischsystem wie CGEventGetLocation: Ursprung links oben am
//  Hauptbildschirm, y nach unten. Genau darin rechnet auch der
//  Mausbeobachter — es wird also nichts umgedreht.
// =====================================================================
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub struct Fenster {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// Fensternummer des Systems. Nur zum Wiedererkennen ueber Takte
    /// hinweg — Noki merkt sich damit, an welche Kante er sich lehnt.
    pub id: i64,
    /// Programmname des Besitzers (z. B. "Safari"). Ohne Berechtigung
    /// verfuegbar, im Gegensatz zum Fenstertitel. Nokis Verhalten haengt
    /// bewusst nicht davon ab; er dient der Nachvollziehbarkeit.
    pub app: String,
    /// Platz in der Fensterreihenfolge, 0 = ganz vorn.
    /// `CGWindowListCopyWindowInfo` mit `kCGWindowListOptionOnScreenOnly`
    /// liefert die Fenster von vorn nach hinten; der Index in dieser
    /// Liste ist damit die tatsaechliche Z-Reihenfolge des Systems.
    /// Hier steht nur die TATSACHE — was daraus folgt, entscheidet das
    /// Frontend an einer Stelle (Abschnitt 26).
    pub rang: i32,
    /// Fensterebene (`kCGWindowLayer`): 0 = gewoehnliches Programmfenster,
    /// 1..3 = schwebende Paneele. Wird hier nur durchgereicht.
    pub ebene: i64,
}

/// Hoechste Fensterebene, die noch als gewoehnliches Fenster gilt.
/// 0 = normale Programmfenster, 1..3 = schwebende Paneele. Darueber
/// liegen Menueleiste (24), Kontrollzentrum (25) und das eigene Panel;
/// darunter Schreibtisch-Widgets, die hinter allem liegen und deshalb
/// kein Hindernis sind.
#[cfg(target_os = "macos")]
const FENSTER_EBENE_MAX: i64 = 3;
#[cfg(target_os = "macos")]
const FENSTER_MIN_KANTE: f64 = 60.0;
/// Mehr als das braucht niemand, und die Liste bleibt klein.
const FENSTER_MAX_ANZAHL: usize = 24;

#[cfg(target_os = "macos")]
pub fn sichtbare_fenster() -> Vec<Fenster> {
    use std::ffi::c_void;

    type CFTypeRef = *const c_void;

    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    struct CGPointD {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    struct CGSizeD {
        width: f64,
        height: f64,
    }
    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    struct CGRectD {
        origin: CGPointD,
        size: CGSizeD,
    }

    // Nur was gerade auf dem Bildschirm ist, ohne Schreibtischelemente.
    const NUR_SICHTBAR: u32 = 1 << 0;
    const OHNE_SCHREIBTISCH: u32 = 1 << 4;
    const KEIN_FENSTER: u32 = 0;
    const CF_SINT64: i32 = 4;
    const CF_DOUBLE: i32 = 13;
    const UTF8: u32 = 0x0800_0100;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGWindowListCopyWindowInfo(option: u32, relativ_zu: u32) -> CFTypeRef;
        fn CGRectMakeWithDictionaryRepresentation(dict: CFTypeRef, rect: *mut CGRectD) -> u8;
        static kCGWindowBounds: CFTypeRef;
        static kCGWindowLayer: CFTypeRef;
        static kCGWindowAlpha: CFTypeRef;
        static kCGWindowOwnerPID: CFTypeRef;
        static kCGWindowOwnerName: CFTypeRef;
        static kCGWindowNumber: CFTypeRef;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFArrayGetCount(a: CFTypeRef) -> isize;
        fn CFArrayGetValueAtIndex(a: CFTypeRef, i: isize) -> CFTypeRef;
        fn CFDictionaryGetValue(d: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
        fn CFNumberGetValue(n: CFTypeRef, art: i32, ziel: *mut c_void) -> u8;
        fn CFStringGetCString(s: CFTypeRef, puffer: *mut u8, laenge: isize, kodierung: u32) -> u8;
        fn CFRelease(cf: CFTypeRef);
    }

    unsafe fn zahl_i64(d: CFTypeRef, schluessel: CFTypeRef) -> Option<i64> {
        let v = CFDictionaryGetValue(d, schluessel);
        if v.is_null() {
            return None;
        }
        let mut aus: i64 = 0;
        if CFNumberGetValue(v, CF_SINT64, &mut aus as *mut i64 as *mut c_void) != 0 {
            Some(aus)
        } else {
            None
        }
    }

    unsafe fn zahl_f64(d: CFTypeRef, schluessel: CFTypeRef) -> Option<f64> {
        let v = CFDictionaryGetValue(d, schluessel);
        if v.is_null() {
            return None;
        }
        let mut aus: f64 = 0.0;
        if CFNumberGetValue(v, CF_DOUBLE, &mut aus as *mut f64 as *mut c_void) != 0 {
            Some(aus)
        } else {
            None
        }
    }

    unsafe fn text(d: CFTypeRef, schluessel: CFTypeRef) -> String {
        let v = CFDictionaryGetValue(d, schluessel);
        if v.is_null() {
            return String::new();
        }
        let mut puffer = [0u8; 128];
        if CFStringGetCString(v, puffer.as_mut_ptr(), puffer.len() as isize, UTF8) == 0 {
            return String::new();
        }
        let ende = puffer.iter().position(|&c| c == 0).unwrap_or(puffer.len());
        String::from_utf8_lossy(&puffer[..ende]).into_owned()
    }

    let eigene_pid = std::process::id() as i64;
    let mut aus: Vec<Fenster> = Vec::new();

    unsafe {
        let liste = CGWindowListCopyWindowInfo(NUR_SICHTBAR | OHNE_SCHREIBTISCH, KEIN_FENSTER);
        if liste.is_null() {
            return aus;
        }
        let n = CFArrayGetCount(liste);
        for i in 0..n {
            if aus.len() >= FENSTER_MAX_ANZAHL {
                break;
            }
            let d = CFArrayGetValueAtIndex(liste, i);
            if d.is_null() {
                continue;
            }

            let fenster_id = zahl_i64(d, kCGWindowNumber).unwrap_or(0);
            // Das transparente Character-Overlay ist kein Hindernis fuer
            // sich selbst. Das eigenstaendige Ask-Fenster dagegen IST ein
            // normales Desktop-Objekt und bleibt deshalb in der Liste.
            if zahl_i64(d, kCGWindowOwnerPID) == Some(eigene_pid)
                && fenster_id != ASK_WID.load(Ordering::Relaxed)
            {
                continue;
            }
            // Menueleiste, Kontrollzentrum, Statusanzeigen liegen darueber,
            // Schreibtisch-Widgets darunter. Beides ist kein Hindernis.
            let ebene = zahl_i64(d, kCGWindowLayer).unwrap_or(9999);
            if !(0..=FENSTER_EBENE_MAX).contains(&ebene) {
                continue;
            }
            // Unsichtbare und fast unsichtbare Fenster zaehlen nicht.
            if zahl_f64(d, kCGWindowAlpha).unwrap_or(0.0) < 0.15 {
                continue;
            }

            let bounds = CFDictionaryGetValue(d, kCGWindowBounds);
            if bounds.is_null() {
                continue;
            }
            let mut r = CGRectD::default();
            if CGRectMakeWithDictionaryRepresentation(bounds, &mut r) == 0 {
                continue;
            }
            if r.size.width < FENSTER_MIN_KANTE || r.size.height < FENSTER_MIN_KANTE {
                continue;
            }

            aus.push(Fenster {
                x: r.origin.x.round() as i32,
                y: r.origin.y.round() as i32,
                w: r.size.width.round() as i32,
                h: r.size.height.round() as i32,
                id: fenster_id,
                app: text(d, kCGWindowOwnerName),
                rang: aus.len() as i32,
                ebene,
            });
        }
        CFRelease(liste);
    }

    aus
}

#[cfg(not(target_os = "macos"))]
pub fn sichtbare_fenster() -> Vec<Fenster> {
    Vec::new()
}

// =====================================================================
//  3d · SCHLIESSEN-KNOPF  (Effekte Schwingen / Plasma)
//
//  Ziel UND Ausloesung laufen ueber die Bedienungshilfen (AX): der rote
//  Knopf (AXCloseButton) des Fensters mit dieser CG-Fensternummer. Das
//  AX-Fenster wird ueber PID + identischen Rahmen gefunden — nur wenn es
//  GENAU EINES gibt. Ohne Bedienungshilfen-Freigabe, ohne Knopf oder bei
//  Mehrdeutigkeit: None/false. Es wird NIE blind geklickt.
// =====================================================================
#[derive(serde::Serialize, Clone, Debug)]
pub struct SchliessKnopf {
    /// Knopfmitte und -groesse in Schirmpunkten (wie `Fenster`).
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub id: i64,
}

#[cfg(target_os = "macos")]
mod ax {
    use std::ffi::{c_void, CString};
    pub type CFTypeRef = *const c_void;
    type Rahmen = (f64, f64, f64, f64);
    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    struct P {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    struct S {
        w: f64,
        h: f64,
    }
    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    struct R {
        o: P,
        s: S,
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> u8;
        fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
        fn AXUIElementCopyAttributeValue(
            el: CFTypeRef,
            attr: CFTypeRef,
            out: *mut CFTypeRef,
        ) -> i32;
        fn AXUIElementPerformAction(el: CFTypeRef, action: CFTypeRef) -> i32;
        fn AXValueGetValue(v: CFTypeRef, typ: u32, out: *mut c_void) -> u8;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(alloc: CFTypeRef, s: *const i8, enc: u32) -> CFTypeRef;
        fn CFArrayGetCount(a: CFTypeRef) -> isize;
        fn CFArrayGetValueAtIndex(a: CFTypeRef, i: isize) -> CFTypeRef;
        fn CFDictionaryGetValue(d: CFTypeRef, k: CFTypeRef) -> CFTypeRef;
        fn CFNumberGetValue(n: CFTypeRef, art: i32, ziel: *mut c_void) -> u8;
        fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
        fn CFRelease(cf: CFTypeRef);
        fn CFGetTypeID(cf: CFTypeRef) -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFBooleanGetTypeID() -> usize;
        fn CFBooleanGetValue(b: CFTypeRef) -> u8;
        fn CFStringGetCString(s: CFTypeRef, puffer: *mut u8, laenge: isize, kodierung: u32) -> u8;
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGWindowListCopyWindowInfo(option: u32, relativ_zu: u32) -> CFTypeRef;
        fn CGRectMakeWithDictionaryRepresentation(dict: CFTypeRef, rect: *mut R) -> u8;
        static kCGWindowBounds: CFTypeRef;
        static kCGWindowOwnerPID: CFTypeRef;
        static kCGWindowNumber: CFTypeRef;
    }

    unsafe fn cfs(s: &str) -> CFTypeRef {
        let c = CString::new(s).unwrap_or_default();
        CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), 0x0800_0100)
    }
    /// Attributwert (+1, der Aufrufer gibt frei).
    unsafe fn attr(el: CFTypeRef, name: &str) -> Option<CFTypeRef> {
        let k = cfs(name);
        let mut v: CFTypeRef = std::ptr::null();
        let e = AXUIElementCopyAttributeValue(el, k, &mut v);
        CFRelease(k);
        if e == 0 && !v.is_null() {
            Some(v)
        } else {
            None
        }
    }
    unsafe fn rahmen(el: CFTypeRef) -> Option<Rahmen> {
        let p = attr(el, "AXPosition")?;
        let Some(s) = attr(el, "AXSize") else {
            CFRelease(p);
            return None;
        };
        let (mut pp, mut ss) = (P::default(), S::default());
        let ok = AXValueGetValue(p, 1, &mut pp as *mut P as *mut c_void) != 0
            && AXValueGetValue(s, 2, &mut ss as *mut S as *mut c_void) != 0;
        CFRelease(p);
        CFRelease(s);
        if ok {
            Some((pp.x, pp.y, ss.w, ss.h))
        } else {
            None
        }
    }
    /// Textattribut (z. B. AXTitle), leer wenn keins.
    unsafe fn text(el: CFTypeRef, name: &str) -> String {
        let Some(v) = attr(el, name) else {
            return String::new();
        };
        let mut buf = [0u8; 256];
        let ok = CFGetTypeID(v) == CFStringGetTypeID()
            && CFStringGetCString(v, buf.as_mut_ptr(), buf.len() as isize, 0x0800_0100) != 0;
        CFRelease(v);
        if !ok {
            return String::new();
        }
        let e = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[..e]).into_owned()
    }
    /// Wahrheitsattribut (z. B. AXEnabled); None wenn nicht vorhanden.
    unsafe fn wahr(el: CFTypeRef, name: &str) -> Option<bool> {
        let v = attr(el, name)?;
        let b = if CFGetTypeID(v) == CFBooleanGetTypeID() {
            Some(CFBooleanGetValue(v) != 0)
        } else {
            None
        };
        CFRelease(v);
        b
    }
    pub fn vertraut() -> bool {
        unsafe { AXIsProcessTrusted() != 0 }
    }
    /// Gibt es das sichtbare CG-Fenster mit dieser Nummer noch?
    pub fn existiert(id: i64) -> bool {
        unsafe { cg_fenster(id).is_some() }
    }
    unsafe fn zahl(d: CFTypeRef, k: CFTypeRef) -> Option<i64> {
        let v = CFDictionaryGetValue(d, k);
        let mut n: i64 = 0;
        if !v.is_null() && CFNumberGetValue(v, 4, &mut n as *mut i64 as *mut c_void) != 0 {
            Some(n)
        } else {
            None
        }
    }
    /// PID und Rahmen des sichtbaren CG-Fensters mit dieser Nummer — frisch.
    unsafe fn cg_fenster(id: i64) -> Option<(i32, Rahmen)> {
        let liste = CGWindowListCopyWindowInfo(1, 0);
        if liste.is_null() {
            return None;
        }
        let mut aus = None;
        for i in 0..CFArrayGetCount(liste) {
            let d = CFArrayGetValueAtIndex(liste, i);
            if d.is_null() || zahl(d, kCGWindowNumber) != Some(id) {
                continue;
            }
            let b = CFDictionaryGetValue(d, kCGWindowBounds);
            let mut r = R::default();
            if let Some(pid) = zahl(d, kCGWindowOwnerPID) {
                if !b.is_null() && CGRectMakeWithDictionaryRepresentation(b, &mut r) != 0 {
                    aus = Some((pid as i32, (r.o.x, r.o.y, r.s.w, r.s.h)));
                }
            }
            break;
        }
        CFRelease(liste);
        aus
    }
    /// Der Schliessen-Knopf (+1), sein Rahmen und der Fenstertitel — nur bei
    /// eindeutigem Fenster und aktivem (schliessbarem) Knopf.
    pub unsafe fn knopf(id: i64) -> Option<(CFTypeRef, Rahmen, String)> {
        if AXIsProcessTrusted() == 0 {
            return None;
        }
        let (pid, fr) = cg_fenster(id)?;
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return None;
        }
        let (mut treffer, mut n): (Option<CFTypeRef>, u32) = (None, 0);
        if let Some(fs) = attr(app, "AXWindows") {
            for i in 0..CFArrayGetCount(fs) {
                let w = CFArrayGetValueAtIndex(fs, i);
                if let Some(r) = rahmen(w) {
                    if (r.0 - fr.0).abs() <= 2.0
                        && (r.1 - fr.1).abs() <= 2.0
                        && (r.2 - fr.2).abs() <= 2.0
                        && (r.3 - fr.3).abs() <= 2.0
                    {
                        n += 1;
                        if treffer.is_none() {
                            treffer = Some(CFRetain(w));
                        }
                    }
                }
            }
            CFRelease(fs);
        }
        CFRelease(app);
        let w = treffer?;
        let k = if n == 1 {
            attr(w, "AXCloseButton")
        } else {
            None
        };
        let titel = if k.is_some() {
            text(w, "AXTitle")
        } else {
            String::new()
        };
        CFRelease(w);
        let k = k?;
        if wahr(k, "AXEnabled") == Some(false) {
            CFRelease(k);
            return None;
        }
        match rahmen(k) {
            Some(r) if r.2 > 2.0 && r.3 > 2.0 => Some((k, r, titel)),
            _ => {
                CFRelease(k);
                None
            }
        }
    }
    pub unsafe fn druecken(k: CFTypeRef) -> bool {
        let a = cfs("AXPress");
        let e = AXUIElementPerformAction(k, a);
        CFRelease(a);
        e == 0
    }
    pub unsafe fn freigeben(k: CFTypeRef) {
        CFRelease(k);
    }
}

/// "Fenster schliessen": nur normale Fenster (Ebene 0), nie JARVIS selbst
/// (sichtbare_fenster laesst die eigene PID weg), mit eindeutigem, aktivem
/// AXCloseButton. Sichtbar ist "App — Titel", nie eine interne Nummer.
pub fn schliessbare_fenster() -> Vec<(i64, String)> {
    #[cfg(target_os = "macos")]
    {
        sichtbare_fenster()
            .into_iter()
            .filter(|f| f.ebene == 0)
            .filter_map(|f| unsafe {
                let (k, _, titel) = ax::knopf(f.id)?;
                ax::freigeben(k);
                let l = if titel.is_empty() {
                    f.app.clone()
                } else {
                    format!("{} — {}", f.app, titel)
                };
                Some((f.id, l.chars().take(70).collect()))
            })
            .collect()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

/// Bedienungshilfen (AX) fuer "Fenster schliessen": bei JEDEM Einstieg live
/// geprueft (keine gespeicherte Wahrheit). Fehlt die Freigabe, zeigt macOS
/// einmal je Programmlauf den echten Freigabe-Dialog fuer diese App.
const AX_EINSTELLUNGEN: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
static AX_GEFRAGT: AtomicBool = AtomicBool::new(false);
fn ax_einstieg() -> bool {
    let ok = lesezeichen::ax_vertraut(false);
    eprintln!("[AX] Fenster schliessen: AXIsProcessTrusted = {}", ok);
    if !ok && !AX_GEFRAGT.swap(true, Ordering::Relaxed) {
        let _ = lesezeichen::ax_vertraut(true);
    }
    ok
}
/// Systemeinstellungen > Datenschutz & Sicherheit > Bedienungshilfen.
fn ax_einstellungen_oeffnen() {
    let _ = lesezeichen::ax_vertraut(true);
    let _ = std::process::Command::new("/usr/bin/open")
        .arg(AX_EINSTELLUNGEN)
        .spawn();
}

/// Das Untermenue "Fenster schliessen" (vom Fenster-Beobachter gefuellt).
struct SchliessMenue(tauri::menu::Submenu<tauri::Wry>);

fn schliess_menue_fuellen(
    app: &tauri::AppHandle,
    m: &tauri::menu::Submenu<tauri::Wry>,
    liste: &[(i64, String)],
) {
    if let Ok(alt) = m.items() {
        for it in alt.iter() {
            let _ = m.remove(it);
        }
    }
    if liste.is_empty() {
        // Freigabe fehlt -> anklickbarer Weg zur Freigabe; sonst ehrlich "keins".
        let item = if lesezeichen::ax_vertraut(false) {
            MenuItem::with_id(
                app,
                "fz_keins",
                "Kein schließbares Fenster",
                false,
                None::<&str>,
            )
        } else {
            MenuItem::with_id(
                app,
                "fz_freigabe",
                "Bedienungshilfen für Noki aktivieren …",
                true,
                None::<&str>,
            )
        };
        if let Ok(i) = item {
            let _ = m.append(&i);
        }
        return;
    }
    for (id, label) in liste {
        if let Ok(i) = MenuItem::with_id(app, format!("fz_{}", id), label, true, None::<&str>) {
            let _ = m.append(&i);
        }
    }
}

// =====================================================================
//  3e · BILDSCHIRMFOTO  (Aktionen Screenshot / Freeze)
//
//  macOS-eigenes `screencapture` (keine neue Abhaengigkeit), nur auf
//  ausdrueckliche Nutzeraktion. Ohne Bildschirmaufnahme-Freigabe wird
//  einmal nachgefragt und ein verstaendlicher Fehler geliefert — nie blind.
//  zweck "datei":  PNG auf den Schreibtisch, Rueckgabe { pfad }
//  zweck "freeze": JPEG ins Frontend (eingefrorenes Desktopbild), { bild }
// =====================================================================
pub(crate) fn base64(daten: &[u8]) -> String {
    const Z: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity((daten.len() + 2) / 3 * 4);
    for c in daten.chunks(3) {
        let n = ((c[0] as u32) << 16)
            | ((*c.get(1).unwrap_or(&0) as u32) << 8)
            | *c.get(2).unwrap_or(&0) as u32;
        s.push(Z[(n >> 18) as usize & 63] as char);
        s.push(Z[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 {
            Z[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        s.push(if c.len() > 2 {
            Z[n as usize & 63] as char
        } else {
            '='
        });
    }
    s
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
    fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
}

// Fehlerarten mit Praefix (das Frontend waehlt danach die Meldung):
//   berechtigung: / neustart: / display: / pfad: / init: / laeuft:
// Geprueft wird die Bildschirmaufnahme (TCC ScreenCapture) des LAUFENDEN
// Prozesses — bei jedem Aufruf neu, nie gecacht. Keine Kamera-Freigabe.
static AUFNAHME_ANGEFRAGT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn aufnahme_erlaubt() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let ok = unsafe { CGPreflightScreenCaptureAccess() };
        if !ok {
            // Schon einmal angefragt: der Nutzer hat evtl. gerade erlaubt —
            // macOS gibt die Freigabe diesem Prozess erst nach Neustart.
            if AUFNAHME_ANGEFRAGT.swap(true, std::sync::atomic::Ordering::Relaxed) {
                return Err("neustart: JARVIS muss nach Änderung der Bildschirmaufnahme-Berechtigung neu gestartet werden.".into());
            }
            unsafe {
                CGRequestScreenCaptureAccess();
            }
            return Err("berechtigung: Bildschirmaufnahme-Berechtigung fehlt".into());
        }
        let mut n: u32 = 0;
        unsafe {
            CGGetActiveDisplayList(0, std::ptr::null_mut(), &mut n);
        }
        if n == 0 {
            return Err("display: kein Bildschirm aktiv".into());
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("init: nur unter macOS".into())
    }
}

fn schreibtisch_datei(praefix: &str, endung: &str) -> Result<std::path::PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "pfad: HOME fehlt".to_string())?;
    schreibtisch_datei_in(
        praefix,
        endung,
        &std::path::PathBuf::from(home).join("Desktop"),
    )
}

fn schreibtisch_datei_in(
    praefix: &str,
    endung: &str,
    desktop: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    let stempel = std::process::Command::new("/bin/date")
        .arg("+%Y-%m-%d-%H%M%S")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "jetzt".into());
    // Alle Noki-Aufnahmen gesammelt in "Schreibtisch/Noki Kamera" (wird bei Bedarf angelegt).
    let dir = desktop.join("Noki Kamera");
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("pfad: Ordner Noki Kamera nicht anlegbar ({e})"))?;
    let probe = dir.join(".noki-schreibtest");
    std::fs::write(&probe, b"")
        .map_err(|e| format!("pfad: Noki Kamera nicht beschreibbar ({e})"))?;
    let _ = std::fs::remove_file(&probe);
    Ok(dir.join(format!("{}-{}.{}", praefix, stempel, endung)))
}

// Bildschirmbild ohne die eigenen Fenster (JARVIS/Noki), direkt als Datei.
// Die Funktion wird per dlsym geholt: fehlt sie, gilt der Rueckfall (None).
#[cfg(target_os = "macos")]
mod bild {
    use std::ffi::c_void;
    extern "C" {
        fn dlopen(p: *const std::os::raw::c_char, m: i32) -> *mut c_void;
        fn dlsym(h: *mut c_void, s: *const std::os::raw::c_char) -> *mut c_void;
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Rechteck {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFArrayCreate(
            a: *const c_void,
            v: *const *const c_void,
            n: isize,
            cb: *const c_void,
        ) -> *const c_void;
        fn CFArrayGetCount(a: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(a: *const c_void, i: isize) -> *const c_void;
        fn CFDictionaryGetValue(d: *const c_void, k: *const c_void) -> *const c_void;
        fn CFNumberGetValue(n: *const c_void, t: i32, v: *mut c_void) -> u8;
        fn CFStringCreateWithCString(
            a: *const c_void,
            s: *const std::os::raw::c_char,
            enc: u32,
        ) -> *const c_void;
        fn CFURLCreateFromFileSystemRepresentation(
            a: *const c_void,
            b: *const u8,
            n: isize,
            dir: u8,
        ) -> *const c_void;
        fn CFRelease(o: *const c_void);
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGImageRelease(i: *const c_void);
        fn CGMainDisplayID() -> u32;
        fn CGDisplayBounds(d: u32) -> Rechteck;
    }
    #[link(name = "ImageIO", kind = "framework")]
    extern "C" {
        fn CGImageDestinationCreateWithURL(
            url: *const c_void,
            typ: *const c_void,
            n: usize,
            opt: *const c_void,
        ) -> *const c_void;
        fn CGImageDestinationAddImage(d: *const c_void, i: *const c_void, p: *const c_void);
        fn CGImageDestinationFinalize(d: *const c_void) -> bool;
    }
    unsafe fn cfs(s: &str) -> *const c_void {
        let c = std::ffi::CString::new(s).unwrap_or_default();
        CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), 0x0800_0100)
    }
    pub fn ohne_noki(pfad: &std::path::Path, jpeg: bool) -> Option<Result<(), String>> {
        unsafe {
            let h = dlopen(
                b"/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics\0".as_ptr()
                    as *const _,
                1,
            );
            if h.is_null() {
                return None;
            }
            let s_info = dlsym(h, b"CGWindowListCopyWindowInfo\0".as_ptr() as *const _);
            let s_bild = dlsym(
                h,
                b"CGWindowListCreateImageFromArray\0".as_ptr() as *const _,
            );
            if s_info.is_null() || s_bild.is_null() {
                return None;
            }
            let liste: extern "C" fn(u32, u32) -> *const c_void = std::mem::transmute(s_info);
            let erzeugen: extern "C" fn(Rechteck, *const c_void, u32) -> *const c_void =
                std::mem::transmute(s_bild);
            let info = liste(1, 0); // nur sichtbare Fenster
            if info.is_null() {
                return Some(Err("init: Fensterliste leer".into()));
            }
            let (k_nr, k_pid) = (cfs("kCGWindowNumber"), cfs("kCGWindowOwnerPID"));
            let ich = std::process::id() as i64;
            let mut ids: Vec<*const c_void> = vec![];
            for i in 0..CFArrayGetCount(info) {
                let d = CFArrayGetValueAtIndex(info, i);
                let (mut nr, mut pid) = (0i64, 0i64);
                let n = CFDictionaryGetValue(d, k_nr);
                let p = CFDictionaryGetValue(d, k_pid);
                if !n.is_null() {
                    let _ = CFNumberGetValue(n, 4, &mut nr as *mut i64 as *mut c_void);
                }
                if !p.is_null() {
                    let _ = CFNumberGetValue(p, 4, &mut pid as *mut i64 as *mut c_void);
                }
                if nr > 0 && pid != ich {
                    ids.push(nr as usize as *const c_void);
                }
            }
            CFRelease(k_nr);
            CFRelease(k_pid);
            CFRelease(info);
            let arr = CFArrayCreate(
                std::ptr::null(),
                ids.as_ptr(),
                ids.len() as isize,
                std::ptr::null(),
            );
            // Genau der Hauptbildschirm (sonst die Vereinigung aller Fensterrahmen,
            // die ueber den Rand ragen kann -> Freeze-Bild saesse verschoben).
            let img = erzeugen(CGDisplayBounds(CGMainDisplayID()), arr, 0);
            CFRelease(arr);
            if img.is_null() {
                return Some(Err("init: Aufnahme leer".into()));
            }
            let b = pfad.as_os_str().as_encoded_bytes();
            let url = CFURLCreateFromFileSystemRepresentation(
                std::ptr::null(),
                b.as_ptr(),
                b.len() as isize,
                0,
            );
            let typ = cfs(if jpeg { "public.jpeg" } else { "public.png" });
            let dst = if url.is_null() {
                std::ptr::null()
            } else {
                CGImageDestinationCreateWithURL(url, typ, 1, std::ptr::null())
            };
            let ok = !dst.is_null() && {
                CGImageDestinationAddImage(dst, img, std::ptr::null());
                CGImageDestinationFinalize(dst)
            };
            if !dst.is_null() {
                CFRelease(dst);
            }
            CFRelease(typ);
            if !url.is_null() {
                CFRelease(url);
            }
            CGImageRelease(img);
            Some(if ok {
                Ok(())
            } else {
                Err("pfad: Bild nicht geschrieben".into())
            })
        }
    }
}

#[tauri::command]
fn noki_bildschirmfoto(zweck: String) -> Result<serde_json::Value, String> {
    aufnahme_erlaubt()?;
    let freeze = zweck == "freeze";
    let pfad = if freeze {
        std::env::temp_dir().join("noki-freeze.jpg")
    } else {
        schreibtisch_datei("Noki-Screenshot", "png")?
    };
    // Alle Fenster AUSSER den eigenen: Noki muss dafuer nicht mehr
    // ausgeblendet werden (vorher war er dabei kurz komplett weg).
    #[cfg(target_os = "macos")]
    let nativ = bild::ohne_noki(&pfad, freeze);
    #[cfg(not(target_os = "macos"))]
    let nativ: Option<Result<(), String>> = None;
    match nativ {
        Some(r) => r?,
        None => {
            // API fehlt: macOS-eigenes screencapture (Noki ist dann mit im Bild)
            let aus = std::process::Command::new("/usr/sbin/screencapture")
                .args(["-x", "-m", "-t", if freeze { "jpg" } else { "png" }])
                .arg(&pfad)
                .output()
                .map_err(|e| format!("init: {e}"))?;
            if !aus.status.success() {
                return Err(format!(
                    "init: Aufnahme konnte nicht starten ({})",
                    String::from_utf8_lossy(&aus.stderr).trim()
                ));
            }
        }
    }
    if !pfad.exists() {
        return Err("init: Aufnahme fehlgeschlagen".into());
    }
    if freeze {
        let daten = std::fs::read(&pfad).map_err(|e| format!("init: {e}"))?;
        let _ = std::fs::remove_file(&pfad);
        return Ok(serde_json::json!({ "bild": base64(&daten) }));
    }
    Ok(serde_json::json!({ "pfad": pfad.to_string_lossy() }))
}

// Screen Recording: macOS-eigenes `screencapture -v` als Kindprozess; Stop
// per SIGINT, danach schreibt screencapture die .mov fertig. Nie parallel.
struct Aufnahme(std::sync::Mutex<Option<(std::process::Child, std::path::PathBuf)>>);
/// Menuepunkte, deren Text der Zustand umschaltet: (Freeze, Screen Recording).
struct ModusMenue(
    tauri::menu::MenuItem<tauri::Wry>,
    tauri::menu::MenuItem<tauri::Wry>,
);

fn aufnahme_stoppen(a: &Aufnahme) -> Result<std::path::PathBuf, String> {
    let lauf = a.0.lock().map_err(|_| "Sperre".to_string())?.take();
    let Some((mut kind, pfad)) = lauf else {
        return Err("keine Aufnahme aktiv".into());
    };
    let _ = std::process::Command::new("/bin/kill")
        .args(["-INT", &kind.id().to_string()])
        .status();
    let mut fertig = false;
    for _ in 0..100 {
        if let Ok(Some(_)) = kind.try_wait() {
            fertig = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    if !fertig {
        let _ = kind.kill();
        let _ = kind.wait();
    }
    if std::fs::metadata(&pfad).map(|m| m.len()).unwrap_or(0) == 0 {
        return Err("init: Aufnahme nicht finalisiert".into());
    }
    Ok(pfad)
}

fn aufnahme_menue(app: &tauri::AppHandle, laeuft: bool) {
    if let Some(m) = app.try_state::<ModusMenue>() {
        let _ = m.1.set_text(if laeuft {
            "Screen Recording beenden"
        } else {
            "Screen Recording"
        });
    }
}

#[tauri::command]
async fn noki_aufnahme(app: tauri::AppHandle, start: bool) -> Result<serde_json::Value, String> {
    let a = app.state::<Aufnahme>();
    if !start {
        let r = aufnahme_stoppen(&a);
        aufnahme_menue(&app, false);
        return r.map(|p| serde_json::json!({ "pfad": p.to_string_lossy() }));
    }
    if a.0
        .lock()
        .map_err(|_| "init: Sperre".to_string())?
        .is_some()
    {
        return Err("laeuft: Aufnahme läuft bereits".into());
    }
    aufnahme_erlaubt()?;
    let pfad = schreibtisch_datei("Noki-Recording", "mov")?;
    let err_pfad = std::env::temp_dir().join("noki-recording.err");
    let err_datei = std::fs::File::create(&err_pfad).map_err(|e| format!("init: {e}"))?;
    let mut kind = std::process::Command::new("/usr/sbin/screencapture")
        .args(["-v", "-x", "-m"])
        .arg(&pfad)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(err_datei)
        .spawn()
        .map_err(|e| format!("init: {e}"))?;
    std::thread::sleep(std::time::Duration::from_millis(500));
    if let Ok(Some(_)) = kind.try_wait() {
        let grund = std::fs::read_to_string(&err_pfad).unwrap_or_default();
        return Err(format!(
            "init: Aufnahme konnte nicht starten ({})",
            grund.trim()
        ));
    }
    *a.0.lock().map_err(|_| "init: Sperre".to_string())? = Some((kind, pfad.clone()));
    aufnahme_menue(&app, true);
    Ok(serde_json::json!({ "pfad": pfad.to_string_lossy() }))
}

// ---- Globale Tastenkuerzel Ctrl+1..4 -----------------------------------
// Nativ ueber Carbon RegisterEventHotKey (keine neue Abhaengigkeit): wirkt
// unabhaengig vom Fokus, auch im Vollbild. Jede Taste loest GENAU das
// Ereignis des Menuepunkts aus — keine zweite Umsetzung.
/// Werkzeug-Panels hinter ^/° + Ziffer. 8 = Arbeitsplatz-Fokus; 9 besitzt
/// sein eigenes, natives Fenster und wird nicht als Werkzeug-Panel geführt.
fn werkzeug_fuer(n: u32) -> Option<&'static str> {
    match n {
        6 => Some("clip"),
        7 => Some("timer"),
        // Shortcut 8: schneller Arbeitsplatz-Wechsel (Wahl Uni/Coding/Normal,
        // bei laufendem Fokus sofort Beenden). Die Konfiguration bleibt in
        // Einstellungen › Arbeitsplatz-Fokus (Menue "werk_platz").
        8 => Some("platz_schnell"),
        _ => None,
    }
}

/// Laeuft gerade eine Sprachaufnahme? Nur daran haengt, ob die Leertaste
/// ausnahmsweise Noki gehoert.
static STIMME_LAEUFT: AtomicBool = AtomicBool::new(false);

/// Ist genau dieses Programm (pid) gerade das vordere des Nutzers?
#[cfg(target_os = "macos")]
pub(crate) fn programm_vorn(pid: i32) -> bool {
    let vorn = lesezeichen::vorne_bundle();
    vorn.is_some() && lesezeichen::app_fuer_pid(pid).map(|(_, b, _)| b) == vorn
}

/// Besitzt die Spracheingabe gerade die Tastatur? (Fern-Tippen weicht dann.)
pub(crate) fn stimme_laeuft() -> bool {
    STIMME_LAEUFT.load(Ordering::Relaxed)
}

/// ^/° + Leertaste: Noki hoert zu.
///
/// Ab hier - und nur ab hier - ist die blosse Leertaste kurzzeitig Nokis
/// Stopp-Taste. Vorher und nachher tippt sie ganz normal.
fn stimme_starten(app: &tauri::AppHandle) {
    if STIMME_LAEUFT.swap(true, Ordering::Relaxed) {
        return; // schon am Zuhoeren: ein zweiter Start waere ein zweiter Lauf
    }
    tasten::stopp_taste(true);
    let _ = app.emit("noki://stimme", serde_json::json!({ "was": "start" }));
    // Noki Talk: local dictation into the focused field (same trigger).
    // The microphone starts in parallel to the short start sound, so no
    // first word is lost.
    noki_talk::ton(true);
    noki_talk::starten(app);
}

/// Leertaste waehrend des Zuhoerens: Aufnahme beenden, Text auswerten.
fn stimme_stoppen(app: &tauri::AppHandle, abbruch: bool) {
    // `swap` ist zugleich die Wiederholungssperre: eine gehaltene Taste
    // schickt viele keyDown, beenden laesst sich aber nur einmal.
    if !STIMME_LAEUFT.swap(false, Ordering::Relaxed) {
        return;
    }
    tasten::stopp_taste(false);
    let _ = app.emit(
        "noki://stimme",
        serde_json::json!({ "was": if abbruch { "abbruch" } else { "stop" } }),
    );
    if !abbruch { noki_talk::ton(false); }
    noki_talk::stoppen(abbruch);
}

/// Noki Talk ended on its own (error, no microphone): the Space key is
/// Space again and the next Option-twice starts a new dictation.
pub(crate) fn stimme_extern_beendet() {
    if STIMME_LAEUFT.swap(false, Ordering::Relaxed) {
        tasten::stopp_taste(false);
    }
}

/// Die Oberflaeche meldet, dass das Zuhoeren von sich aus endete (Fehler,
/// Abbruch, fehlende Freigabe). Dann muss die Leertaste sofort zurueck.
/// Klickflaeche des Transkriptfelds. w/h = 0 gibt sie wieder frei.
#[tauri::command]
fn noki_stimme_zone(state: tauri::State<NokiHitStore>, x: i32, y: i32, w: i32, h: i32) {
    if let Ok(mut hit) = state.0.write() {
        hit.sx = x;
        hit.sy = y;
        hit.sw = w;
        hit.sh = h;
    }
}

/// Klickflaeche der Noki Einstellungen. w/h = 0 gibt sie frei.
#[tauri::command]
fn noki_einst_zone(state: tauri::State<NokiHitStore>, x: i32, y: i32, w: i32, h: i32) {
    if let Ok(mut hit) = state.0.write() {
        hit.ex = x;
        hit.ey = y;
        hit.ew = w;
        hit.eh = h;
    }
}

/// Klickflaeche der Sprechblase. w/h = 0 gibt sie frei.
#[tauri::command]
fn noki_blase_zone(state: tauri::State<NokiHitStore>, x: i32, y: i32, w: i32, h: i32) {
    if let Ok(mut hit) = state.0.write() {
        hit.bx = x;
        hit.by = y;
        hit.bw = w;
        hit.bh = h;
    }
}

#[tauri::command]
fn noki_stimme_beendet() {
    if STIMME_LAEUFT.swap(false, Ordering::Relaxed) {
        tasten::stopp_taste(false);
    }
}

fn shortcut_ausloesen(app: &tauri::AppHandle, n: u32) {
    virtual_workspace::trace(&format!("[SHORTCUT] action n={n}"));
    if n == 0
        && !app
            .try_state::<std::sync::Arc<intelligence::Intelligence>>()
            .is_some_and(|state| state.ask_enabled())
    {
        return;
    }
    // Nur ein Arbeits-Panel zur Zeit: Aktionen ohne eigenes Panel (Kamera,
    // Nokis Buero) raeumen offene Noki-Panels vorher zentral weg.
    if matches!(n, 1 | 2) {
        let _ = app.emit("noki://panels_zu", serde_json::json!({}));
    }
    match n {
        0 => {
            let _ = app.emit("noki://ask", serde_json::json!({}));
        } // Ask Noki (dieselbe zentrale UI wie Klick auf Noki)
        1 => {
            let _ = app.emit("noki://kamera", serde_json::json!({ "was": "screenshot" }));
        }
        2 => {
            let _ = app.emit("noki://kamera", serde_json::json!({ "was": "recording" }));
        }
        3 => {
            let _ = app.emit("noki://freeze", serde_json::json!({}));
        }
        4 => {
            // Kuerzel 4 veraendert ausschliesslich die Sichtbarkeit der
            // Miniatur. Den echten Schreibtisch besucht nur ein bewusster
            // Klick auf dessen Inhalt; insbesondere wird hier weder ein
            // Herkunfts-Space gemerkt noch ein Space-Wechsel angefordert.
            // Full view open: 4 closes it first, then the same press hides
            // the Miniatur - one press, everything closed.
            #[cfg(target_os = "macos")]
            if vorschau::voll_aktiv() { vorschau::voll(false); }
            let _ = app.emit("noki://vorschau_toggle", serde_json::json!({}));
        }
        5 => ablage_shortcut(app),
        6..=8 => {
            if let Some(w) = werkzeug_fuer(n) {
                werkzeug_zeigen(app, w)
            }
        }
        9 => window_overview::toggle(app),
        _ => {}
    }
}

/// Physical key delivery is deliberately independent of WebKit, AX and the
/// interaction workers. The event-tap/Carbon callback only performs a
/// bounded non-blocking enqueue; a dedicated router executes backend-safe
/// actions directly and schedules only genuinely UI-bound tools on main.
// Do not pin a dead Sender in a OnceLock. A router thread is deliberately
// replaceable: if it ever exits, the next shortcut installs a fresh lane.
static SHORTCUT_SENDER: std::sync::Mutex<Option<std::sync::mpsc::Sender<u32>>> =
    std::sync::Mutex::new(None);
static SHORTCUT_EMPFANGEN: AtomicU64 = AtomicU64::new(0);
static SHORTCUT_ABGESCHLOSSEN: AtomicU64 = AtomicU64::new(0);
static SHORTCUT_FEHLER: AtomicU64 = AtomicU64::new(0);
static SHORTCUT_HEARTBEAT_MS: AtomicU64 = AtomicU64::new(0);
static SHORTCUT_RECOVERIES: AtomicU64 = AtomicU64::new(0);
// The event tap owns the healthy physical path; Carbon is its fallback.  At
// the exact instant a timed-out tap is re-enabled, both paths can report the
// same Digit9 press.  Collapse only that sub-human duplicate window, while
// keeping deliberate rapid toggles responsive.
static SHORTCUT9_LAST: std::sync::Mutex<Option<std::time::Instant>> =
    std::sync::Mutex::new(None);

fn shortcut_router_starten(app: &tauri::AppHandle) -> Option<std::sync::mpsc::Sender<u32>> {
        // A physical shortcut is never optional. `sync_channel(32)` used to
        // drop the 33rd event while the main thread was busy (for example
        // during a Space animation). Keep the callback non-blocking and
        // retain every tiny command until the dedicated lane can drain it.
        let (tx, rx) = std::sync::mpsc::channel::<u32>();
        let h = app.clone();
        let worker = std::thread::Builder::new().name("noki-shortcut".into()).spawn(move || {
            while let Ok(n) = rx.recv() {
                SHORTCUT_HEARTBEAT_MS.store(
                    SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64).unwrap_or(0),
                    Ordering::Relaxed,
                );
                if n <= 4 {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        shortcut_ausloesen(&h, n)
                    }));
                    if result.is_err() { SHORTCUT_FEHLER.fetch_add(1, Ordering::Relaxed); }
                    SHORTCUT_ABGESCHLOSSEN.fetch_add(1, Ordering::Relaxed);
                } else {
                    // Count completion where the action really ran, not when
                    // it was merely appended to Tauri's main-thread queue.
                    let hh = h.clone();
                    if h.run_on_main_thread(move || {
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            shortcut_ausloesen(&hh, n)
                        }));
                        if result.is_err() { SHORTCUT_FEHLER.fetch_add(1, Ordering::Relaxed); }
                        SHORTCUT_ABGESCHLOSSEN.fetch_add(1, Ordering::Relaxed);
                        SHORTCUT_HEARTBEAT_MS.store(
                            SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_millis() as u64).unwrap_or(0),
                            Ordering::Relaxed,
                        );
                    }).is_err() {
                        SHORTCUT_FEHLER.fetch_add(1, Ordering::Relaxed);
                        SHORTCUT_ABGESCHLOSSEN.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        });
        match worker {
            Ok(_) => Some(tx),
            Err(e) => {
                SHORTCUT_FEHLER.fetch_add(1, Ordering::Relaxed);
                eprintln!("[HEALTH] shortcut_router spawn_failed={e}");
                None
            }
        }
}

fn shortcut_einreihen(app: &tauri::AppHandle, n: u32) {
    if n == 9 {
        let now = std::time::Instant::now();
        let duplicate = SHORTCUT9_LAST.lock().ok().is_some_and(|mut last| {
            let duplicate = last.is_some_and(|t| now.duration_since(t).as_millis() < 250);
            if !duplicate { *last = Some(now); }
            duplicate
        });
        if duplicate { return; }
    }
    // This is an unbounded channel, so send never waits for Browser/AX/WebKit.
    // Serialising replacement prevents a failed old sender from erasing a
    // freshly installed one from a concurrent callback.
    let mut slot = SHORTCUT_SENDER.lock().unwrap_or_else(|e| e.into_inner());
    for versuch in 0..2 {
        if slot.is_none() { *slot = shortcut_router_starten(app); }
        let Some(sender) = slot.as_ref() else { break };
        if sender.send(n).is_ok() {
            SHORTCUT_EMPFANGEN.fetch_add(1, Ordering::Relaxed);
            return;
        }
        *slot = None;
        SHORTCUT_RECOVERIES.fetch_add(1, Ordering::Relaxed);
        eprintln!("[RECOVERY] subsystem=SHORTCUT_ROUTER action=recreate attempt={}", versuch + 1);
    }
    SHORTCUT_FEHLER.fetch_add(1, Ordering::Relaxed);
    eprintln!("[HEALTH] shortcut_router disconnected action=not_delivered");
}

fn shortcut_health() -> serde_json::Value {
    let rein = SHORTCUT_EMPFANGEN.load(Ordering::Relaxed);
    let raus = SHORTCUT_ABGESCHLOSSEN.load(Ordering::Relaxed);
    serde_json::json!({
        "lastHeartbeat": SHORTCUT_HEARTBEAT_MS.load(Ordering::Relaxed),
        "queueDepth": rein.saturating_sub(raus),
        "completedCount": raus,
        "errorCount": SHORTCUT_FEHLER.load(Ordering::Relaxed),
        "recoveryCount": SHORTCUT_RECOVERIES.load(Ordering::Relaxed),
    })
}

/// Schnellzugriff (Klick auf Noki): loest GENAU die bestehende Aktion des
/// Kuerzels Ctrl+n aus — keine zweite Umsetzung. Auf dem Hauptthread, weil
/// einzelne bestehende Aktionen native UI beruehren.
#[tauri::command]
fn noki_schnell(app: tauri::AppHandle, n: u32) {
    let h = app.clone();
    let _ = app.run_on_main_thread(move || shortcut_ausloesen(&h, n));
}

/// Direkter, vom Shortcut unabhaengiger Zugang fuer die bestehende
/// Fenster-Aktion (Menue/Ask-Werkzeug). Shortcut 4 ruft dies nie auf.
#[tauri::command]
fn noki_fenster_auswahl(app: tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let h = app.clone();
        let _ = app.run_on_main_thread(move || regler::fenster_auswahl(&h));
    }
}

#[tauri::command]
fn datei_oeffnen(pfad: String) -> bool {
    std::process::Command::new("/usr/bin/open")
        .arg(&pfad)
        .spawn()
        .is_ok()
}

#[tauri::command]
fn datei_finder(pfad: String) -> bool {
    std::process::Command::new("/usr/bin/open")
        .args(["-R", &pfad])
        .spawn()
        .is_ok()
}

#[tauri::command]
fn datei_bearbeiten(pfad: String) -> bool {
    let p = std::path::Path::new(&pfad);
    let ext = p
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    let app = if ["png", "jpg", "jpeg", "webp", "gif", "tiff"].contains(&ext.as_str()) {
        "Preview"
    } else {
        "QuickTime Player"
    };
    std::process::Command::new("/usr/bin/open")
        .args(["-a", app, &pfad])
        .args(vscode_bruecke::open_zusatz(app))
        .spawn()
        .is_ok()
}

#[tauri::command]
fn kamera_ordner_oeffnen() -> bool {
    if let Ok(home) = std::env::var("HOME") {
        let dir = std::path::PathBuf::from(home)
            .join("Desktop")
            .join("Noki Kamera");
        let _ = std::fs::create_dir_all(&dir);
        std::process::Command::new("/usr/bin/open")
            .arg(&dir)
            .spawn()
            .is_ok()
    } else {
        false
    }
}

#[tauri::command]
fn noki_utility_vorn(app: tauri::AppHandle, halten: Option<bool>) {
    // halten=false (Noki Einstellungen): KEIN bildschirmweiter Griff - das
    // Fenster faengt nur in seinem eigenen Rechteck (ex..eh), alles daneben
    // geht an die anderen Apps. Die Shortcut-Uebersicht behaelt ihren Griff
    // (Aussenklick schliesst sie, wie ein Popover).
    let halten = halten.unwrap_or(true);
    let h = app.clone();
    let _ = app.run_on_main_thread(move || {
        if halten {
            if let Ok(mut hit) = h.state::<NokiHitStore>().0.write() {
                hit.drag = true;
            }
            DRAG_BESTAETIGT_MS.store(jetzt_epoch_ms(), Ordering::Relaxed);
        }
        if let Some(win) = h.get_webview_window(FENSTER) {
            #[cfg(target_os = "macos")]
            let space_before = cgs::aktiver_space().map(|x| x.0).unwrap_or(0);
            overlay_durchlaessig(&win, false);
            // Auch bei Ebene "normal"/"hinten": das Bedienfenster (Einstellungen,
            // Shortcuts) muss VOR der gerade aktiven App liegen. Zurueck zur
            // gewaehlten Ebene: noki_utility_zurueck beim Schliessen.
            ebene_setzen(&win, "vorn");
            #[cfg(target_os = "macos")]
            if let Ok(nw) = win.ns_window() {
                unsafe {
                    extern "C" {
                        fn sel_registerName(n: *const i8) -> *const std::ffi::c_void;
                        fn objc_msgSend();
                    }
                    let vor: unsafe extern "C" fn(*mut std::ffi::c_void, *const std::ffi::c_void) =
                        std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                    vor(
                        nw,
                        sel_registerName(b"orderFrontRegardless\0".as_ptr() as _),
                    );
                }
            }
            #[cfg(target_os = "macos")]
            unsafe {
                use std::ffi::c_void;
                extern "C" {
                    fn objc_getClass(n: *const i8) -> *mut c_void;
                    fn sel_registerName(n: *const i8) -> *const c_void;
                    fn objc_msgSend();
                }
                let get: unsafe extern "C" fn(*mut c_void, *const c_void) -> *mut c_void =
                    std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                let activate: unsafe extern "C" fn(*mut c_void, *const c_void, bool) =
                    std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                let nsapp = get(
                    objc_getClass(b"NSApplication\0".as_ptr() as _),
                    sel_registerName(b"sharedApplication\0".as_ptr() as _),
                );
                activate(
                    nsapp,
                    sel_registerName(b"activateIgnoringOtherApps:\0".as_ptr() as _),
                    true,
                );
            }
            let _ = win.set_focus();
            #[cfg(target_os = "macos")]
            space_action_log("noki_utility_order_front_activate_focus", "Noki", fenster_nummer(&win).unwrap_or(0), space_before, cgs::aktiver_space().map(|x| x.0).unwrap_or(0));
        }
    });
}

// ---- Noki Ablage: echter Ordner als Speicherort --------------------------
// ~/Documents/Noki Ablage ist ein normaler lokaler Ordner (Finder, Sichern-
// Dialog). Was dort fertig gespeichert wird, nimmt die BESTEHENDE Ablage auf
// (ablage_aufnehmen: Lesezeichen, keine Duplikate). Keine Kopie, keine DB.
fn noki_ablage_ordner() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join("Documents").join("Noki"))
}
/// Halbfertige Downloads und temporaere Dateien nie uebernehmen.
fn ablage_temp(p: &std::path::Path) -> bool {
    let n = p
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    n.is_empty()
        || n.starts_with('.')
        || n.starts_with("~$")
        || [
            "download",
            "crdownload",
            "part",
            "partial",
            "tmp",
            "temp",
            "opdownload",
        ]
        .iter()
        .any(|e| n.ends_with(&format!(".{}", e)))
}
fn spawn_ablage_ordner(app: tauri::AppHandle) {
    let Some(dir) = noki_ablage_ordner() else {
        return;
    };
    // Frueherer Name "Noki Ablage" -> "Noki" (so heisst er in Finder und
    // Sichern-Dialog). Die Lesezeichen der Eintraege folgen dem Umbenennen.
    if let Some(alt) = dir.parent().map(|d| d.join("Noki Ablage")) {
        if alt.is_dir() && !dir.exists() {
            let _ = fs::rename(&alt, &dir);
        }
    }
    let neu = !dir.exists();
    let _ = fs::create_dir_all(&dir);
    if neu {
        // Eine Seitenleisten-API ohne private Schnittstellen gibt es nicht:
        // Ordner einmal zeigen, der Nutzer zieht ihn in die Favoriten.
        let _ = std::process::Command::new("/usr/bin/open")
            .arg(&dir)
            .spawn();
        eprintln!(
            "[ABLAGE] Ordner angelegt: {} — einmal in die Finder-Seitenleiste ziehen",
            dir.display()
        );
    }
    thread::spawn(move || {
        let lesen = |d: &PathBuf| -> Vec<(PathBuf, u64, u64)> {
            fs::read_dir(d)
                .map(|rd| {
                    rd.flatten()
                        .filter_map(|e| {
                            let m = e.metadata().ok()?;
                            let t = m
                                .modified()
                                .ok()?
                                .duration_since(std::time::UNIX_EPOCH)
                                .ok()?
                                .as_millis() as u64;
                            Some((e.path(), if m.is_dir() { 0 } else { m.len() }, t))
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        // Vorhandenes gilt als bekannt: ein entfernter Eintrag kehrt nicht zurueck.
        let mut bekannt: std::collections::HashSet<PathBuf> =
            lesen(&dir).into_iter().map(|e| e.0).collect();
        let mut kandidat: std::collections::HashMap<PathBuf, (u64, u64, u32)> =
            std::collections::HashMap::new();
        loop {
            thread::sleep(Duration::from_millis(1500));
            let jetzt = lesen(&dir);
            let da: std::collections::HashSet<PathBuf> =
                jetzt.iter().map(|e| e.0.clone()).collect();
            bekannt.retain(|p| da.contains(p));
            kandidat.retain(|p, _| da.contains(p));
            let mut fertig = Vec::new();
            for (p, len, mt) in jetzt {
                if bekannt.contains(&p) || ablage_temp(&p) {
                    continue;
                }
                // Fertig = zwei ruhige Runden (~3 s gleiche Groesse/Zeit), Datei nicht leer.
                let e = kandidat.entry(p.clone()).or_insert((len, mt, 0));
                if e.0 == len && e.1 == mt {
                    e.2 += 1;
                } else {
                    *e = (len, mt, 0);
                }
                if e.2 >= 2 && (len > 0 || p.is_dir()) {
                    fertig.push(p);
                }
            }
            for p in &fertig {
                kandidat.remove(p);
                bekannt.insert(p.clone());
            }
            if !fertig.is_empty() {
                eprintln!(
                    "[ABLAGE] Noki Ablage: {} neue Datei(en) uebernommen",
                    fertig.len()
                );
                ablage_aufnehmen(&app, &fertig);
            }
        }
    });
}
/// Kompatible installierte Apps fuer einen Eintrag (Launch Services).
#[tauri::command]
fn ablage_apps(app: tauri::AppHandle, id: u64) -> Vec<serde_json::Value> {
    let Some(p) = ablage_pfad(&app, id) else {
        return Vec::new();
    };
    lesezeichen::apps_fuer_datei(&p)
        .into_iter()
        .map(|(n, a, s)| serde_json::json!({ "name": n, "app": a, "standard": s }))
        .collect()
}
/// Nur auf ausdruecklichen Klick: mit der gewaehlten App oeffnen.
#[tauri::command]
fn ablage_oeffnen_mit(app: tauri::AppHandle, id: u64, programm: String) -> bool {
    if !programm.ends_with(".app") || !std::path::Path::new(&programm).exists() {
        return false;
    }
    ablage_pfad(&app, id)
        .map(|p| {
            std::process::Command::new("/usr/bin/open")
                .args(["-a", &programm, &p])
                .args(vscode_bruecke::open_zusatz(&programm))
                .spawn()
                .is_ok()
        })
        .unwrap_or(false)
}
fn im_noki_ordner(p: &str) -> bool {
    noki_ablage_ordner()
        .map(|d| {
            let d = fs::canonicalize(&d).unwrap_or(d);
            fs::canonicalize(p)
                .map(|x| x.starts_with(&d))
                .unwrap_or(false)
        })
        .unwrap_or(false)
}
/// Inhalt von ~/Documents/Noki fuer "Einstellungen > Noki-Ordner" — direkt
/// gelesen, keine zweite Verwaltung. Neueste zuerst, Temporaeres ausgelassen.
#[tauri::command]
fn noki_ordner_liste() -> Vec<serde_json::Value> {
    let Some(dir) = noki_ablage_ordner() else {
        return Vec::new();
    };
    let mut v: Vec<(u64, serde_json::Value)> = fs::read_dir(&dir).map(|rd| rd.flatten().filter_map(|e| {
        let p = e.path();
        if ablage_temp(&p) { return None; }
        let m = e.metadata().ok()?;
        let t = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
        let (typ, ext) = ablage_typ(&p);
        Some((t, serde_json::json!({ "name": e.file_name().to_string_lossy(), "pfad": p.to_string_lossy(), "typ": typ, "ext": ext,
                                     "groesse": if m.is_dir() { 0 } else { m.len() }, "zeit": t })))
    }).collect()).unwrap_or_default();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v.into_iter().map(|x| x.1).collect()
}
/// Dokumente: eine Datei aus dem Noki-Ordner Noki in die Hand geben —
/// derselbe Weg wie Drop/Finder (Verweis in der Ablage, keine Kopie).
#[tauri::command]
fn noki_ordner_halten(app: tauri::AppHandle, pfad: String) -> bool {
    if !im_noki_ordner(&pfad) || !std::path::Path::new(&pfad).exists() {
        return false;
    }
    ablage_aufnehmen(&app, &[PathBuf::from(pfad)]);
    true
}
#[tauri::command]
fn noki_datei_apps(pfad: String) -> Vec<serde_json::Value> {
    if !im_noki_ordner(&pfad) {
        return Vec::new();
    }
    lesezeichen::apps_fuer_datei(&pfad)
        .into_iter()
        .map(|(n, a, s)| serde_json::json!({ "name": n, "app": a, "standard": s }))
        .collect()
}
#[tauri::command]
fn noki_datei_oeffnen_mit(pfad: String, programm: String) -> bool {
    im_noki_ordner(&pfad)
        && programm.ends_with(".app")
        && std::path::Path::new(&programm).exists()
        && std::process::Command::new("/usr/bin/open")
            .args(["-a", &programm, &pfad])
            .args(vscode_bruecke::open_zusatz(&programm))
            .spawn()
            .is_ok()
}
#[tauri::command]
fn noki_ordner_finder() -> bool {
    noki_ablage_ordner()
        .map(|d| {
            std::process::Command::new("/usr/bin/open")
                .arg("-R")
                .arg(&d)
                .spawn()
                .is_ok()
        })
        .unwrap_or(false)
}
/// "Dateien auswählen …": normaler macOS-Dateidialog (im Noki-Ordner), die
/// Auswahl kommt als Verweis in die bestehende Ablage — nichts wird kopiert.
#[tauri::command]
fn noki_dateien_waehlen(app: tauri::AppHandle) {
    let Some(dir) = noki_ablage_ordner() else {
        return;
    };
    thread::spawn(move || {
        let ort = format!(
            "set d to POSIX file \"{}\"",
            dir.to_string_lossy().replace('"', "")
        );
        let aus = std::process::Command::new("/usr/bin/osascript").args(["-e", &ort,
            "-e", "set f to choose file with prompt \"Dateien für Noki auswählen\" default location d with multiple selections allowed",
            "-e", "set o to \"\"", "-e", "repeat with x in f", "-e", "set o to o & POSIX path of x & linefeed", "-e", "end repeat", "-e", "return o"])
            .output();
        let Ok(aus) = aus else { return };
        let pfade: Vec<PathBuf> = String::from_utf8_lossy(&aus.stdout)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(PathBuf::from)
            .collect();
        if !pfade.is_empty() {
            ablage_aufnehmen(&app, &pfade);
        }
    });
}
/// "Datei löschen": nur auf bestaetigten Klick und NUR fuer Dateien im
/// Noki-Ordner — in den Papierkorb (wiederherstellbar), Eintrag entfernen.
#[tauri::command]
fn ablage_loeschen(app: tauri::AppHandle, id: u64) -> bool {
    let (Some(p), Some(dir)) = (ablage_pfad(&app, id), noki_ablage_ordner()) else {
        return false;
    };
    let dir = fs::canonicalize(&dir).unwrap_or(dir);
    if !std::path::Path::new(&p).starts_with(&dir) {
        return false;
    }
    let ok = lesezeichen::in_papierkorb(&p);
    if ok {
        ablage_entfernen(app, id);
    }
    ok
}
/// "Dateien löschen …" (Einstellungen, bestaetigt): alles im Noki-Ordner in
/// den Papierkorb, zugehoerige Eintraege entfernen. Andere Eintraege bleiben.
#[tauri::command]
fn noki_ordner_leeren(app: tauri::AppHandle) -> usize {
    let Some(dir) = noki_ablage_ordner() else {
        return 0;
    };
    let dir = fs::canonicalize(&dir).unwrap_or(dir);
    let mut geloescht = Vec::new();
    if let Ok(rd) = fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if !ablage_temp(&p) && lesezeichen::in_papierkorb(&p.to_string_lossy()) {
                geloescht.push(p);
            }
        }
    }
    {
        let st = app.state::<Ablage>();
        let mut l = st.0.lock().unwrap();
        // Nur Eintraege entfernen, deren Datei tatsaechlich im Papierkorb
        // gelandet ist. Bei einem einzelnen Finder-/Rechtefehler bleibt das
        // Dokument sichtbar und kann erneut versucht werden.
        l.retain(|e| {
            let p = std::path::Path::new(&e.pfad);
            !geloescht.iter().any(|weg| p == weg)
        });
        ablage_speichern(&app, &l);
    }
    ablage_melden(&app, &[], None, None);
    geloescht.len()
}
#[tauri::command]
fn noki_ablage_ordner_oeffnen() -> bool {
    noki_ablage_ordner()
        .map(|d| {
            let _ = fs::create_dir_all(&d);
            std::process::Command::new("/usr/bin/open")
                .arg(&d)
                .spawn()
                .is_ok()
        })
        .unwrap_or(false)
}

// ---- Kompakte Noki-Uebersicht: eigenes natives Fenster -------------------
// Schneller Launcher (^/° doppelt). Echtes NSWindow (Mission Control, Fokus,
// Ampel), getrennt vom Overlay; jede Aktion ist der bestehende Kuerzel-Weg.
const KOMPAKT: &str = "kompakt";
fn kompakt_offen(app: &tauri::AppHandle) -> bool {
    app.get_webview_window(KOMPAKT)
        .map_or(false, |w| w.is_visible().unwrap_or(false))
}
fn app_nach_vorn() {
    #[cfg(target_os = "macos")]
    unsafe {
        let space_before = cgs::aktiver_space().map(|x| x.0).unwrap_or(0);
        use std::ffi::c_void;
        extern "C" {
            fn objc_getClass(n: *const i8) -> *mut c_void;
            fn sel_registerName(n: *const i8) -> *const c_void;
            fn objc_msgSend();
        }
        let get: unsafe extern "C" fn(*mut c_void, *const c_void) -> *mut c_void =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let activate: unsafe extern "C" fn(*mut c_void, *const c_void, bool) =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let nsapp = get(
            objc_getClass(b"NSApplication\0".as_ptr() as _),
            sel_registerName(b"sharedApplication\0".as_ptr() as _),
        );
        activate(
            nsapp,
            sel_registerName(b"activateIgnoringOtherApps:\0".as_ptr() as _),
            true,
        );
        space_action_log("noki_app_activate", "Noki", 0, space_before, cgs::aktiver_space().map(|x| x.0).unwrap_or(0));
    }
}
fn kompakt_zeigen(app: &tauri::AppHandle) {
    // Produktregel: Genau ein Hauptfenster. Keine Hilfs- oder Zweitfenster oeffnen.
    let _ = app.emit(
        "noki://panels_zu",
        serde_json::json!({ "ask_behalten": true }),
    );
    let _ = app.emit("noki://ask", serde_json::json!({}));
}
fn kompakt_schliessen(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window(KOMPAKT) {
        let _ = w.close();
    }
}
#[tauri::command]
fn kompakt_auf(app: tauri::AppHandle) {
    kompakt_zeigen(&app);
}
#[tauri::command]
fn kompakt_zu(app: tauri::AppHandle) {
    kompakt_schliessen(&app);
}
/// Klick in der kompakten Uebersicht: Fenster zu, DANN die bestehende Aktion
/// (n = 1..9 wie ^/° + n, 0 = Einstellungen). Nativ, weil die Seite samt
/// ihrem JavaScript mit dem Fenster verschwindet.
#[tauri::command]
fn kompakt_aktion(app: tauri::AppHandle, n: u32) {
    kompakt_schliessen(&app);
    if n == 10 {
        einstellungen_oeffnen(app);
        return;
    } // 0 ist jetzt Ask Noki
    let h = app.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(140)); // Fenster ist weg, bevor z. B. ein Foto entsteht
        let h2 = h.clone();
        let _ = h.run_on_main_thread(move || shortcut_ausloesen(&h2, n));
    });
}

/// Live-Stand der Freigabe "Bedienungshilfen" fuer DIESEN Prozess (nie gecacht).
#[tauri::command]
fn ax_status(app: tauri::AppHandle) -> serde_json::Value {
    let ok = lesezeichen::ax_vertraut(false);
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    eprintln!(
        "[AX] Status: bundle={} exe={} pid={} AXIsProcessTrusted={}",
        app.config().identifier,
        exe,
        std::process::id(),
        ok
    );
    serde_json::json!({ "trusted": ok, "pid": std::process::id(), "bundle": app.config().identifier, "exe": exe })
}

/// Echten macOS-Freigabedialog ausloesen und Bedienungshilfen oeffnen.
#[tauri::command]
fn ax_freigabe() {
    ax_einstellungen_oeffnen();
}

/// Bedienfenster geschlossen: wieder die vom Nutzer gewaehlte Ebene.
#[tauri::command]
fn noki_utility_zurueck(app: tauri::AppHandle) {
    let h = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(win) = h.get_webview_window(FENSTER) {
            lage_anwenden(&win, &h.state::<Lage>());
        }
    });
}

/// Zeigt (und fokussiert) oder verbirgt das Einstellungsfenster. Normales
/// Fenster: keine Ebene ueber anderen Apps, kein orderFront ausser diesem
/// einen ausdruecklichen Oeffnen, kein Schliessen bei Fokusverlust.
#[tauri::command]
fn einstellungen_fenster(app: tauri::AppHandle, zeigen: bool) {
    let h = app.clone();
    let _ = app.run_on_main_thread(move || {
        let win = h.get_webview_window(EINST_FENSTER);
        if !zeigen {
            if let Some(w) = win {
                if w.is_visible().unwrap_or(false) {
                    eprintln!("[EINST] Fenster verbergen");
                    let _ = w.hide();
                }
            }
            return;
        }
        let win = win.or_else(|| {
            tauri::WebviewWindowBuilder::new(&h, EINST_FENSTER, tauri::WebviewUrl::App("settings.html".into()))
                .title("Noki Einstellungen")
                .inner_size(860.0, 660.0)
                .min_inner_size(720.0, 520.0)
                .resizable(true)
                .decorations(true)
                .shadow(true)
                .center()
                .visible(false)
                .build()
                .map_err(|e| eprintln!("[EINST] Fenster konnte nicht angelegt werden: {e}"))
                .ok()
        });
        let Some(w) = win else { return };
        #[cfg(target_os = "macos")]
        let space_before = cgs::aktiver_space().map(|x| x.0).unwrap_or(0);
        // Oeffnet IMMER auf dem aktuellen Schreibtisch: oeffentliches AppKit-
        // Verhalten MoveToActiveSpace (1 << 1). Ohne dieses Bit holte macOS
        // den Nutzer beim Zeigen auf den Space, auf dem das Fenster zuletzt
        // war - der Wechsel loeste dort die Space-Aufraeumung aus und schloss
        // die Einstellungen sofort wieder. Kein joinAllSpaces, kein CGS.
        #[cfg(target_os = "macos")]
        if let Ok(nw) = w.ns_window() {
            unsafe {
                use std::ffi::c_void;
                extern "C" {
                    fn sel_registerName(n: *const i8) -> *const c_void;
                    fn objc_msgSend();
                }
                let get: unsafe extern "C" fn(*mut c_void, *const c_void) -> u64 = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                let set: unsafe extern "C" fn(*mut c_void, *const c_void, u64) = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                let alt = get(nw, sel_registerName(b"collectionBehavior\0".as_ptr() as _));
                // MoveToActiveSpace an, CanJoinAllSpaces (1) aus.
                set(nw, sel_registerName(b"setCollectionBehavior:\0".as_ptr() as _), (alt | 2) & !1);
            }
        }
        let _ = w.set_zoom(match oberflaeche_wert(&h).get("groesse").and_then(|v| v.as_str()) { Some("kompakt") => 0.9, Some("gross") => 1.12, _ => 1.0 });
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        #[cfg(target_os = "macos")]
        space_action_log("settings_rehome_show_focus", "Noki", fenster_nummer(&w).unwrap_or(0), space_before, cgs::aktiver_space().map(|x| x.0).unwrap_or(0));
        eprintln!("[EINST] Fenster zeigen");
    });
}

/// Darstellung › Oberflaeche: Farbe, Schrift, Material, UI-Groesse. Liegt
/// in einstellungen.json, gilt fuer jede Noki-Oberflaeche (Ereignis an alle
/// Fenster). Die UI-Groesse ist der Webview-Zoom von Ask und Einstellungen -
/// nie das Character-Fenster, nie Nokis Groesse.
fn oberflaeche_wert(app: &tauri::AppHandle) -> serde_json::Value {
    einstellungen_datei(app)
        .and_then(|d| fs::read_to_string(d).ok())
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("oberflaeche").cloned())
        .unwrap_or_else(|| serde_json::json!({}))
}
fn oberflaeche_zoom(app: &tauri::AppHandle, wert: &serde_json::Value) {
    let z = match wert.get("groesse").and_then(|v| v.as_str()) { Some("kompakt") => 0.9, Some("gross") => 1.12, _ => 1.0 };
    for label in [ASK_FENSTER, EINST_FENSTER] {
        if let Some(w) = app.get_webview_window(label) { let _ = w.set_zoom(z); }
    }
}
#[tauri::command]
fn oberflaeche_lesen(app: tauri::AppHandle) -> serde_json::Value {
    oberflaeche_wert(&app)
}
#[tauri::command]
fn oberflaeche_setzen(app: tauri::AppHandle, wert: serde_json::Value, speichern: Option<bool>) {
    // Waehrend eines Farbzugs nur verteilen; gespeichert wird beim Loslassen.
    if speichern != Some(false) { einstellungen_setzen(&app, "oberflaeche", wert.clone()); }
    oberflaeche_zoom(&app, &wert);
    let _ = app.emit("noki://oberflaeche", wert);
}

/// Name, Version und Build fuer "Allgemein".
#[tauri::command]
fn noki_app_info(app: tauri::AppHandle) -> serde_json::Value {
    #[cfg(target_os = "macos")]
    let schirm = schirm_freigabe();
    #[cfg(not(target_os = "macos"))]
    let schirm = false;
    serde_json::json!({
        "name": app.package_info().name,
        "version": app.package_info().version.to_string(),
        "build": env!("NOKI_BUILD_ID"),
        // Nur Abfragen ohne Dialog (Preflight), nie eine neue Anfrage.
        "rechte": {
            "bedienungshilfen": lesezeichen::ax_vertraut(false),
            "bildschirmaufnahme": schirm,
        },
    })
}

/// Die wirklich registrierten Kuerzel (aus dem Tastenmodul, keine Kopie).
#[tauri::command]
fn kuerzel_info() -> serde_json::Value {
    #[cfg(target_os = "macos")]
    { serde_json::json!({ "praefix": tasten::praefix_text(), "ask": tasten::ASK_ZIFFER }) }
    #[cfg(not(target_os = "macos"))]
    { serde_json::json!({ "praefix": "^/°", "ask": 0 }) }
}

#[tauri::command]
fn einstellungen_oeffnen(app: tauri::AppHandle) {
    let _ = app.emit("noki://einstellungen", serde_json::json!({}));
}

#[tauri::command]
fn noki_ebene_umschalten(app: tauri::AppHandle, ebene: Option<String>) -> String {
    let lage = app.state::<Lage>();
    let neu: &'static str = match ebene.as_deref() {
        Some("hinten") => "hinten",
        Some("vorn") => "vorn",
        _ => {
            if *lage.ebene.lock().unwrap() == "hinten" {
                "vorn"
            } else {
                "hinten"
            }
        }
    };
    *lage.ebene.lock().unwrap() = neu;
    lage.durchgang.store(false, Ordering::Relaxed);
    einstellungen_setzen(&app, "ebene", serde_json::Value::String(neu.to_string()));
    if let Some(win) = app.get_webview_window(FENSTER) {
        lage_anwenden(&win, &lage);
    }
    neu.to_string()
}

#[cfg(target_os = "macos")]
mod tasten {
    use std::ffi::c_void;
    use std::sync::{Mutex, OnceLock};
    use tauri::{Emitter, Manager};
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct HotKeyId {
        signatur: u32,
        id: u32,
    }
    #[repr(C)]
    struct EventTyp {
        klasse: u32,
        art: u32,
    }
    #[link(name = "Carbon", kind = "framework")]
    extern "C" {
        fn GetApplicationEventTarget() -> *mut c_void;
        fn RegisterEventHotKey(
            code: u32,
            mods: u32,
            id: HotKeyId,
            ziel: *mut c_void,
            opt: u32,
            aus: *mut *mut c_void,
        ) -> i32;
        fn InstallEventHandler(
            ziel: *mut c_void,
            h: extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> i32,
            n: u32,
            liste: *const EventTyp,
            user: *mut c_void,
            aus: *mut *mut c_void,
        ) -> i32;
        fn GetEventParameter(
            ev: *mut c_void,
            name: u32,
            typ: u32,
            aus_typ: *mut u32,
            groesse: usize,
            aus_groesse: *mut usize,
            daten: *mut c_void,
        ) -> i32;
        fn UnregisterEventHotKey(r: *mut c_void) -> i32;
        fn LMGetKbdType() -> u8;
        fn KBGetLayoutType(typ: i16) -> u32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(o: *const c_void);
        fn CFMachPortCreateRunLoopSource(
            alloc: *const c_void,
            port: *mut c_void,
            order: isize,
        ) -> *mut c_void;
        fn CFRunLoopGetCurrent() -> *mut c_void;
        fn CFRunLoopAddSource(run_loop: *mut c_void, source: *mut c_void, mode: *const c_void);
        fn CFRunLoopRun();
        static kCFRunLoopCommonModes: *const c_void;
    }
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn CGEventTapCreate(
            tap: u32,
            place: u32,
            options: u32,
            events: u64,
            callback: extern "C" fn(*mut c_void, u32, *mut c_void, *mut c_void) -> *mut c_void,
            user: *mut c_void,
        ) -> *mut c_void;
        fn CGEventTapEnable(tap: *mut c_void, enabled: bool);
        fn CGEventTapIsEnabled(tap: *mut c_void) -> bool;
        fn CGEventGetIntegerValueField(event: *mut c_void, field: u32) -> i64;
        fn CGEventGetFlags(event: *mut c_void) -> u64;
        fn CGEventGetLocation(event: *mut c_void) -> CgPoint;
        fn CGEventSetLocation(event: *mut c_void, point: CgPoint);
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CgPoint { x: f64, y: f64 }

    static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
    static ZULETZT: Mutex<[Option<std::time::Instant>; 24]> = Mutex::new([None; 24]);
    // Noki-Praefix: physische Taste links neben "1" (^/° im deutschen Layout).
    // Registriert wird der KEYCODE, nicht das Zeichen (Dead-Key): ISO = 10,
    // ANSI = 50. Als Hotkey erreicht der Druck keine App — kein ^, kein Akzent.
    // Die Ziffern 1..5 sind nur FENSTER_MS lang nach dem Praefix belegt.
    const PRAEFIX: u32 = 10;
    /// Ziffer nach dem Praefix, die Ask Noki umschaltet. Einzige Quelle -
    /// auch die Anzeige in den Einstellungen liest sie (kuerzel_info).
    pub const ASK_ZIFFER: u32 = 0;
    /// Lesbare Form der registrierten Praefix-Taste (ISO ^, ANSI `).
    pub fn praefix_text() -> &'static str {
        if PRAEFIX_CODE.load(std::sync::atomic::Ordering::Relaxed) == 50 { "`" } else { "^/°" }
    }
    const FENSTER_MS: u64 = 1300;
    const DOPPEL_MS: u128 = 420;
    // Nach dem Schliessen per Einzeltipp kurz gesperrt: kein Wieder-Oeffnen
    // durch einen nachgeschobenen zweiten Tipp.
    static SPERRE: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    static PREFIX_UNTIL: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    /// Der tatsaechlich registrierte Keycode der Praefix-Taste (ISO 10 / ANSI 50).
    static PRAEFIX_CODE: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(10);
    /// Carbon-Registrierung der Praefix-Taste (Rueckfallweg), zum Umziehen.
    static PRAEFIX_REF: Mutex<usize> = Mutex::new(0);
    /// Zuletzt gesehener Tastaturtyp eines ECHTEN Tastendrucks (-1 = keiner).
    static TASTATUR_TYP: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(-1);

    /// Keycode der Taste links neben der 1 fuer einen Tastaturtyp:
    /// ISO 10 (^/°), sonst 50 (`/~).
    fn praefix_fuer_typ(typ: i64) -> i64 {
        if unsafe { KBGetLayoutType(typ as i16) } == vz(b"ISO ") { 10 } else { 50 }
    }
    fn layout_datei() -> Option<std::path::PathBuf> {
        std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h)
            .join("Library/Application Support/com.noki.desktop/praefix_keycode"))
    }
    /// Beim Start: der zuletzt an einem ECHTEN Tastendruck bestaetigte Keycode.
    /// LMGetKbdType allein fragt nur den LETZTEN Tastendruck irgendeines
    /// Ursprungs ab - nach einer virtuellen/ferngesteuerten/synthetischen
    /// Taste meldete es ANSI, der Praefix lag dann auf 50 (auf ISO die
    /// "<"-Taste) und ALLE Noki-Kuerzel waren tot.
    fn praefix_gemerkt() -> Option<i64> {
        let t = std::fs::read_to_string(layout_datei()?).ok()?;
        match t.trim() { "10" => Some(10), "50" => Some(50), _ => None }
    }
    /// Die Bauart der angeschlossenen Tastatur laut IOKit ("StandardType":
    /// 0 ANSI, 1 ISO, 2 JIS) - unabhaengig davon, welcher Tastendruck
    /// zuletzt irgendwo kam. ISO -> 10, sonst 50; None ohne Angabe.
    fn praefix_ioreg() -> Option<i64> {
        #[link(name = "IOKit", kind = "framework")]
        extern "C" {
            fn IOServiceMatching(name: *const std::os::raw::c_char) -> *mut c_void;
            fn IOServiceGetMatchingServices(port: u32, matching: *mut c_void, it: *mut u32) -> i32;
            fn IOIteratorNext(it: u32) -> u32;
            fn IOObjectRelease(o: u32) -> i32;
            fn IORegistryEntryCreateCFProperty(e: u32, key: *const c_void, a: *const c_void, o: u32) -> *const c_void;
        }
        #[link(name = "CoreFoundation", kind = "framework")]
        extern "C" {
            fn CFStringCreateWithCString(a: *const c_void, s: *const std::os::raw::c_char, e: u32) -> *const c_void;
            fn CFNumberGetValue(n: *const c_void, t: i32, v: *mut c_void) -> u8;
        }
        unsafe {
            let mut it = 0u32;
            if IOServiceGetMatchingServices(0, IOServiceMatching(b"AppleHIDKeyboardEventDriverV2\0".as_ptr() as *const _), &mut it) != 0 {
                return None;
            }
            let key = CFStringCreateWithCString(std::ptr::null(), b"StandardType\0".as_ptr() as *const _, 0x0800_0100);
            let mut ergebnis = None;
            loop {
                let e = IOIteratorNext(it);
                if e == 0 { break; }
                let v = IORegistryEntryCreateCFProperty(e, key, std::ptr::null(), 0);
                IOObjectRelease(e);
                if !v.is_null() {
                    let mut n: i64 = -1;
                    CFNumberGetValue(v, 4, &mut n as *mut i64 as *mut c_void);
                    CFRelease(v);
                    if n >= 0 { ergebnis = Some(if n == 1 { 10 } else { 50 }); if n == 1 { break; } }
                }
            }
            IOObjectRelease(it);
            CFRelease(key);
            ergebnis
        }
    }
    /// Praefix auf `code` umziehen (Tap-Erkennung sofort, Carbon-Rueckfall
    /// auf dem Hauptfaden) und fuer den naechsten Start merken.
    fn praefix_umziehen(code: i64) {
        let alt = PRAEFIX_CODE.swap(code, std::sync::atomic::Ordering::AcqRel);
        if alt == code { return; }
        eprintln!("[SHORTCUT] Praefix-Taste folgt der echten Tastatur: Keycode {alt} -> {code}");
        if let Some(d) = layout_datei() {
            if let Some(p) = d.parent() { let _ = std::fs::create_dir_all(p); }
            let _ = std::fs::write(&d, code.to_string());
        }
        if let Some(app) = APP.get() {
            let _ = app.run_on_main_thread(move || { let _ = unsafe { praefix_carbon(code) }; });
        }
    }
    unsafe fn praefix_carbon(code: i64) -> i32 {
        let Ok(mut halter) = PRAEFIX_REF.lock() else { return -1 };
        if *halter != 0 {
            UnregisterEventHotKey(*halter as *mut c_void);
            *halter = 0;
        }
        let mut r = std::ptr::null_mut();
        let st = RegisterEventHotKey(code as u32, 0, HotKeyId { signatur: vz(b"NOKI"), id: PRAEFIX },
            GetApplicationEventTarget(), 0, &mut r);
        if st == 0 { *halter = r as usize; }
        st
    }
    static ZIFFERN: Mutex<Vec<usize>> = Mutex::new(Vec::new());
    static RUNDE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    static TAP_KURZBEFEHL_KEYUP: std::sync::atomic::AtomicI64 =
        std::sync::atomic::AtomicI64::new(-1);
    /// Die Leertaste als Stopp-Taste - ausschliesslich, solange Noki
    /// zuhoert.
    ///
    /// Sie wird registriert, wenn das Zuhoeren beginnt, und sofort wieder
    /// freigegeben, wenn es endet. Nur so bleibt die Leertaste im Alltag das,
    /// was sie ist: eine Leertaste. Ein dauerhaft belegter Hotkey waere in
    /// jedem Textfeld eine Zumutung.
    static STOPP_TASTE: Mutex<Option<usize>> = Mutex::new(None);

    pub fn stopp_taste(an: bool) {
        let Ok(mut halter) = STOPP_TASTE.lock() else { return };
        unsafe {
            if let Some(r) = halter.take() {
                UnregisterEventHotKey(r as *mut c_void);
            }
            if !an {
                return;
            }
            let ziel = GetApplicationEventTarget();
            let mut r = std::ptr::null_mut();
            if RegisterEventHotKey(
                49,
                0,
                HotKeyId { signatur: vz(b"NOKI"), id: PRAEFIX + 13 },
                ziel,
                0,
                &mut r,
            ) == 0
            {
                *halter = Some(r as usize);
            }
        }
    }

    fn ziffern_frei() {
        if let Ok(mut until) = PREFIX_UNTIL.lock() {
            *until = None;
        }
        if let Ok(mut z) = ZIFFERN.lock() {
            for r in z.drain(..) {
                unsafe {
                    UnregisterEventHotKey(r as *mut c_void);
                }
            }
        }
    }
    /// DOUBLE TAP CONTROL = Noki aus-/einblenden (replaces prefix + Right
    /// Shift). Only two PURE Control taps count: Control down and up alone
    /// (no other modifier, no key, no click, no scroll in between), each
    /// tap short, the second starting soon after the first. It fires on the
    /// SECOND release, so a held Control (Ctrl+click, Ctrl+C ...) never does.
    pub(super) mod doppel_control {
        pub const TAP_MAX_MS: u64 = 350;      // down -> up of one tap
        pub const PAUSE_MAX_MS: u64 = 400;    // first up -> second down
        const CTRL: u64 = 0x40000;
        const ANDERE: u64 = 0x20000 | 0x80000 | 0x100000; // Shift, Option, Command
        #[derive(Clone, Copy, Debug, Default, PartialEq)]
        pub struct Stand { pub unten_seit: Option<u64>, pub sauber: bool, pub erster_hoch: Option<u64> }
        pub enum Ereignis { Flags { code: i64, flags: u64 }, Anderes }
        /// Returns (new state, fire).
        pub fn schritt(s: Stand, e: Ereignis, jetzt: u64) -> (Stand, bool) {
            schritt_fuer(s, e, jetzt, (59, 62), CTRL, ANDERE)
        }
        /// Same pure-double-tap detector for the OPTION keys (58/61):
        /// "Noki hoert zu". Option with any other key never counts.
        pub fn schritt_option(s: Stand, e: Ereignis, jetzt: u64) -> (Stand, bool) {
            schritt_fuer(s, e, jetzt, (58, 61), 0x80000, 0x20000 | 0x40000 | 0x100000)
        }
        /// Noki Talk: ONE pure Option tap (down -> up alone, short). Only fed
        /// while a recording runs - outside a recording a single Option tap
        /// does nothing at all.
        pub fn schritt_option_einzel(mut s: Stand, e: Ereignis, jetzt: u64) -> (Stand, bool) {
            match e {
                Ereignis::Anderes => { s.sauber = false; (s, false) }
                Ereignis::Flags { code, flags } => {
                    if !(code == 58 || code == 61) || flags & (0x20000 | 0x40000 | 0x100000) != 0 {
                        return (Stand::default(), false);
                    }
                    if flags & 0x80000 != 0 {
                        s.unten_seit = Some(jetzt); s.sauber = true;
                        (s, false)
                    } else {
                        let tap = s.sauber && s.unten_seit.is_some_and(|t| jetzt.saturating_sub(t) <= TAP_MAX_MS);
                        (Stand::default(), tap)
                    }
                }
            }
        }
        fn schritt_fuer(mut s: Stand, e: Ereignis, jetzt: u64, codes: (i64, i64), taste: u64, andere: u64) -> (Stand, bool) {
            match e {
                Ereignis::Anderes => {
                    // any key/click/scroll: nothing in flight counts any more
                    s.sauber = false;
                    s.erster_hoch = None;
                    (s, false)
                }
                Ereignis::Flags { code, flags } => {
                    let ist_ctrl = code == codes.0 || code == codes.1;
                    if !ist_ctrl || flags & andere != 0 {
                        return (Stand::default(), false);
                    }
                    if flags & taste != 0 {
                        // Control down: a second tap must start soon after the first
                        if s.erster_hoch.is_some_and(|t| jetzt.saturating_sub(t) > PAUSE_MAX_MS) {
                            s.erster_hoch = None;
                        }
                        s.unten_seit = Some(jetzt);
                        s.sauber = true;
                        (s, false)
                    } else {
                        let tap = s.sauber && s.unten_seit.is_some_and(|t| jetzt.saturating_sub(t) <= TAP_MAX_MS);
                        s.unten_seit = None;
                        s.sauber = false;
                        if !tap {
                            s.erster_hoch = None;
                            return (s, false);
                        }
                        if s.erster_hoch.is_some() {
                            return (Stand::default(), true);
                        }
                        s.erster_hoch = Some(jetzt);
                        (s, false)
                    }
                }
            }
        }
    }
    static DOPPEL_STAND: std::sync::Mutex<doppel_control::Stand> =
        std::sync::Mutex::new(doppel_control::Stand { unten_seit: None, sauber: false, erster_hoch: None });
    fn jetzt_ms() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
    }
    /// Feeds one tap event into the detector; true = Double-Control.
    fn doppel_control_ereignis(kind: u32, event: *mut c_void) -> bool {
        use doppel_control::{schritt, Ereignis};
        let e = if kind == 12 {
            let (code, flags) = unsafe { (CGEventGetIntegerValueField(event, 9), CGEventGetFlags(event)) };
            Ereignis::Flags { code, flags }
        } else if matches!(kind, 10 | 1 | 3 | 25 | 22) {
            Ereignis::Anderes
        } else {
            return false; // mouse moves/drags/key-ups say nothing
        };
        let Ok(mut g) = DOPPEL_STAND.lock() else { return false };
        let (neu, feuer) = schritt(*g, e, jetzt_ms());
        *g = neu;
        feuer
    }

    static DOPPEL_OPTION: std::sync::Mutex<doppel_control::Stand> =
        std::sync::Mutex::new(doppel_control::Stand { unten_seit: None, sauber: false, erster_hoch: None });
    /// Double-Option detector (same event feed as Double-Control). The tap
    /// itself is never swallowed: Option keeps working everywhere.
    static EINZEL_OPTION: std::sync::Mutex<doppel_control::Stand> =
        std::sync::Mutex::new(doppel_control::Stand { unten_seit: None, sauber: false, erster_hoch: None });
    /// One pure Option tap (Noki Talk stop) - see schritt_option_einzel.
    fn einzel_option_ereignis(kind: u32, event: *mut c_void) -> bool {
        use doppel_control::{schritt_option_einzel, Ereignis};
        let e = if kind == 12 {
            let (code, flags) = unsafe { (CGEventGetIntegerValueField(event, 9), CGEventGetFlags(event)) };
            Ereignis::Flags { code, flags }
        } else if matches!(kind, 10 | 1 | 3 | 25 | 22) {
            Ereignis::Anderes
        } else {
            return false;
        };
        let Ok(mut g) = EINZEL_OPTION.lock() else { return false };
        let (neu, feuer) = schritt_option_einzel(*g, e, jetzt_ms());
        *g = neu;
        feuer
    }
    fn doppel_option_ereignis(kind: u32, event: *mut c_void) -> bool {
        use doppel_control::{schritt_option, Ereignis};
        let e = if kind == 12 {
            let (code, flags) = unsafe { (CGEventGetIntegerValueField(event, 9), CGEventGetFlags(event)) };
            Ereignis::Flags { code, flags }
        } else if matches!(kind, 10 | 1 | 3 | 25 | 22) {
            Ereignis::Anderes
        } else {
            return false;
        };
        let Ok(mut g) = DOPPEL_OPTION.lock() else { return false };
        let (neu, feuer) = schritt_option(*g, e, jetzt_ms());
        *g = neu;
        feuer
    }

    fn ziffer_von_keycode(code: i64) -> Option<u32> {
        match code {
            29 => Some(0), 18 => Some(1), 19 => Some(2), 20 => Some(3),
            21 => Some(4), 23 => Some(5), 22 => Some(6), 26 => Some(7),
            28 => Some(8), 25 => Some(9), _ => None,
        }
    }

    /// Receive prefix + 0..9 on the event-tap thread. Carbon remains only as
    /// a permission/failure fallback; the healthy path is independent of the
    /// Tauri/AppKit main event loop.
    fn event_tap_shortcut(code: i64, wiederholung: bool) -> bool {
        let praefix = PRAEFIX_CODE.load(std::sync::atomic::Ordering::Relaxed);
        if code == praefix {
            if wiederholung { return true; }
            if SPERRE.lock().ok().and_then(|g| *g)
                .is_some_and(|t| std::time::Instant::now() < t)
            {
                return true;
            }
            if let Some(app) = APP.get() {
                if super::kompakt_offen(app) {
                    let h = app.clone();
                    let _ = app.run_on_main_thread(move || super::kompakt_schliessen(&h));
                    ziffern_frei();
                    RUNDE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if let Ok(mut s) = SPERRE.lock() {
                        *s = Some(std::time::Instant::now()
                            + std::time::Duration::from_millis(DOPPEL_MS as u64));
                    }
                    return true;
                }
            }
            let jetzt = std::time::Instant::now();
            let doppelt = ZULETZT.lock().ok().is_some_and(|mut z| {
                let vorher = z.get(PRAEFIX as usize).copied().flatten();
                let doppelt = vorher.is_some_and(|t| jetzt.duration_since(t).as_millis() < DOPPEL_MS);
                if let Some(slot) = z.get_mut(PRAEFIX as usize) {
                    *slot = if doppelt { None } else { Some(jetzt) };
                }
                doppelt
            });
            if doppelt {
                ziffern_frei();
                if let Ok(mut until) = PREFIX_UNTIL.lock() { *until = None; }
                RUNDE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if let Some(app) = APP.get() {
                    let _ = app.emit("noki://einstellungen_toggle", serde_json::json!({}));
                }
            } else {
                // No temporary Carbon registrations on the healthy path: the
                // same independent event tap receives the following key.
                if let Ok(mut until) = PREFIX_UNTIL.lock() {
                    *until = Some(jetzt + std::time::Duration::from_millis(FENSTER_MS));
                }
                RUNDE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            return true;
        }

        let aktiv = PREFIX_UNTIL.lock().ok().and_then(|g| *g)
            .is_some_and(|until| std::time::Instant::now() < until);
        if !aktiv { return false; }
        let n = ziffer_von_keycode(code);
        if let Some(n) = n {
            if wiederholung { return true; }
            super::virtual_workspace::trace(&format!("[SHORTCUT] tap prefix+{n}"));
            ziffern_frei();
            // The prefix gesture is complete.  Without clearing its
            // double-tap timestamp, the prefix of a rapid next shortcut was
            // mistaken for "open Einstellungen" and Digit9 had no owner.
            if let Ok(mut z) = ZULETZT.lock() {
                if let Some(slot) = z.get_mut(PRAEFIX as usize) { *slot = None; }
            }
            RUNDE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if let Some(app) = APP.get() {
                let h = app.clone();
                if n == ASK_ZIFFER {
                    let _ = app.run_on_main_thread(move || super::ask_umschalten(&h));
                } else {
                    super::shortcut_einreihen(&h, n);
                }
            }
            return true;
        }
        // ^/° + Leertaste startet "Noki hoert zu" NICHT mehr (jetzt: Option
        // zweimal). Die Leertaste bleibt hier unberuehrt.
        false
    }
    #[cfg(test)]
    #[test]
    fn double_control_only_for_two_pure_taps() {
        use doppel_control::{schritt, Ereignis::{Anderes, Flags}, Stand};
        let dn = |t| (Flags { code: 59, flags: 0x40000 }, t);
        let up = |t| (Flags { code: 59, flags: 0 }, t);
        let lauf = |ev: Vec<(doppel_control::Ereignis, u64)>| {
            let mut s = Stand::default(); let mut n = 0;
            for (e, t) in ev { let (x, f) = schritt(s, e, t); s = x; if f { n += 1; } }
            n
        };
        assert_eq!(lauf(vec![dn(0), up(90), dn(220), up(300)]), 1, "double tap");
        assert_eq!(lauf(vec![dn(0), up(90)]), 0, "single tap");
        assert_eq!(lauf(vec![dn(0), up(900)]), 0, "held Control");
        assert_eq!(lauf(vec![dn(0), (Anderes, 50), up(90), dn(200), up(260)]), 0, "Ctrl+C then tap");
        assert_eq!(lauf(vec![dn(0), up(80), dn(150), (Anderes, 170), up(200)]), 0, "tap then Ctrl+click");
        // Double-Option ("Noki hoert zu"): same rules on keycodes 58/61.
        let odn = |t| (Flags { code: 58, flags: 0x80000 }, t);
        let oup = |t| (Flags { code: 58, flags: 0 }, t);
        let olauf = |ev: Vec<(doppel_control::Ereignis, u64)>| {
            let mut s = Stand::default(); let mut n = 0;
            for (e, t) in ev { let (x, f) = doppel_control::schritt_option(s, e, t); s = x; if f { n += 1; } }
            n
        };
        assert_eq!(olauf(vec![odn(0), oup(90), odn(220), oup(300)]), 1, "option double tap");
        assert_eq!(olauf(vec![odn(0), oup(90)]), 0, "single option");
        assert_eq!(olauf(vec![odn(0), oup(90), odn(700), oup(780)]), 0, "slow option taps");
        assert_eq!(olauf(vec![odn(0), (Anderes, 40), oup(90), odn(200), oup(260)]), 0, "Option+key then tap");
        assert_eq!(lauf(vec![odn(0), oup(90), odn(220), oup(300)]), 0, "option never fires Double-Control");
        // Noki Talk stop: ONE pure Option tap.
        let elauf = |ev: Vec<(doppel_control::Ereignis, u64)>| {
            let mut s = Stand::default(); let mut n = 0;
            for (e, t) in ev { let (x, f) = doppel_control::schritt_option_einzel(s, e, t); s = x; if f { n += 1; } }
            n
        };
        assert_eq!(elauf(vec![odn(0), oup(90)]), 1, "single option tap stops");
        assert_eq!(elauf(vec![odn(0), oup(900)]), 0, "held Option is no tap");
        assert_eq!(elauf(vec![odn(0), (Anderes, 40), oup(90)]), 0, "Option+key is no tap");
        assert_eq!(elauf(vec![(Flags { code: 58, flags: 0x80000 | 0x100000 }, 0), oup(60)]), 0, "Option+Cmd is no tap");
        assert_eq!(elauf(vec![dn(0), up(60)]), 0, "Control is no Option tap");
        assert_eq!(lauf(vec![dn(0), up(80), dn(700), up(760)]), 0, "too slow");
        assert_eq!(lauf(vec![(Flags { code: 59, flags: 0x40000 | 0x100000 }, 0), up(50), dn(100), up(150)]), 0, "Ctrl+Cmd");
        assert_eq!(lauf(vec![dn(0), up(80), dn(160), up(220), dn(300), up(360), dn(420), up(480)]), 2, "two double taps");
        assert_eq!(lauf(vec![(Flags { code: 62, flags: 0x40000 }, 0), (Flags { code: 62, flags: 0 }, 60), (Flags { code: 62, flags: 0x40000 }, 150), (Flags { code: 62, flags: 0 }, 210)]), 1, "right Control");
        assert_eq!([29, 18, 19, 20, 21, 23, 22, 26, 28, 25]
            .map(ziffer_von_keycode),
            [Some(0), Some(1), Some(2), Some(3), Some(4), Some(5),
             Some(6), Some(7), Some(8), Some(9)]);
        assert_eq!(ziffer_von_keycode(49), None);
    }
    /// Der Abgriff selbst - damit macOS ihn nach einer Zeitueberschreitung
    /// wieder einschalten laesst (sonst waeren Pfeile und Leertaste still tot).
    static TAP: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    /// keyUp der Taste, die den Schwarm beendet hat: geht nur durch.
    static SCHWARM_TASTE_HOCH: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(-1);
    /// Bis wann (ms) der Carbon-Praefix den Schwarm-Abbruchdruck ignoriert.
    static SCHWARM_PRAEFIX_BIS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    /// Loslassen des Modifiers, der den Schwarm beendet hat.
    static SCHWARM_MOD_LOS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static TAP_WATCHDOG: std::sync::OnceLock<()> = std::sync::OnceLock::new();

    /// Rueckruf des Ereignis-Abgriffs. KEINE Panik darf hier hinaus: ein
    /// `extern "C"`-Rahmen bricht sonst den ganzen Prozess ab.
    extern "C" fn modifier_event(
        proxy: *mut c_void,
        kind: u32,
        event: *mut c_void,
        user: *mut c_void,
    ) -> *mut c_void {
        // kCGEventTapDisabledByTimeout / ...ByUserInput: sofort wieder an.
        if kind == 0xFFFF_FFFE || kind == 0xFFFF_FFFF {
            let tap = TAP.load(std::sync::atomic::Ordering::Relaxed);
            if tap != 0 {
                unsafe { CGEventTapEnable(tap as *mut c_void, true) };
                eprintln!("[SHORTCUT] Ereignis-Abgriff war abgeschaltet ({kind:#x}) - wieder an");
            }
            return event;
        }
        match std::panic::catch_unwind(|| modifier_event_innen(proxy, kind, event, user)) {
            Ok(r) => r,
            Err(_) => {
                eprintln!("[SHORTCUT] Fehler im Tasten-Abgriff abgefangen - Taste unveraendert durchgereicht");
                event
            }
        }
    }

    fn modifier_event_innen(
        _proxy: *mut c_void,
        kind: u32,
        event: *mut c_void,
        _user: *mut c_void,
    ) -> *mut c_void {
        if event.is_null() {
            return event;
        }
        // Timer-Schwarm: die ERSTE echte Taste (keyDown oder Modifier,
        // auch ein einzelnes Control) beendet ihn sofort vom Tap-Thread.
        // Sie geht unveraendert an das Programm vorn; Nokis eigene Kuerzel
        // und der Doppel-Control-Zaehler sehen sie nicht.
        if (kind == 10 || kind == 12)
            && super::SCHWARM_AKTIV.load(std::sync::atomic::Ordering::Relaxed)
            && unsafe { CGEventGetIntegerValueField(event, 41) } != std::process::id() as i64
            && unsafe { CGEventGetIntegerValueField(event, 42) } != super::fern_tippen::MARKE
            && super::SCHWARM_AKTIV.swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            if let Some(app) = APP.get() {
                let code = unsafe { CGEventGetIntegerValueField(event, 9) };
                let _ = tauri::Emitter::emit(app, "noki://schwarm_stop",
                    serde_json::json!({ "grund": format!("taste-{code}") }));
            }
            if let Ok(mut g) = DOPPEL_STAND.lock() { *g = doppel_control::Stand::default(); }
            if kind == 10 {
                let code = unsafe { CGEventGetIntegerValueField(event, 9) };
                SCHWARM_TASTE_HOCH.store(code, std::sync::atomic::Ordering::Release);
                // Die Praefix-Taste ist zusaetzlich als Carbon-Hotkey belegt:
                // dieser eine Druck darf auch dort kein Praefix-Fenster oeffnen.
                if code == PRAEFIX_CODE.load(std::sync::atomic::Ordering::Relaxed) {
                    SCHWARM_PRAEFIX_BIS.store(jetzt_ms() + 600, std::sync::atomic::Ordering::Release);
                }
            } else {
                // Das Loslassen dieses Modifiers darf kein erster Tap sein.
                SCHWARM_MOD_LOS.store(true, std::sync::atomic::Ordering::Release);
            }
            return event;
        }
        if kind == 12 && SCHWARM_MOD_LOS.swap(false, std::sync::atomic::Ordering::AcqRel) {
            if let Ok(mut g) = DOPPEL_STAND.lock() { *g = doppel_control::Stand::default(); }
            return event;
        }
        if kind == 11 && SCHWARM_TASTE_HOCH
            .compare_exchange(unsafe { CGEventGetIntegerValueField(event, 9) }, -1,
                std::sync::atomic::Ordering::AcqRel, std::sync::atomic::Ordering::Relaxed).is_ok()
        {
            return event;
        }
        // Noki Talk: Option twice starts; while recording ONE Option tap
        // stops (Space stops too, Esc cancels - see the stop key below).
        // Only one detector is fed at a time, so the stop tap can never be
        // the first half of a new double tap.
        if super::stimme_laeuft() {
            if let Ok(mut g) = DOPPEL_OPTION.lock() { *g = doppel_control::Stand::default(); }
            if einzel_option_ereignis(kind, event) {
                if let Some(app) = APP.get() {
                    let h = app.clone();
                    let _ = app.run_on_main_thread(move || super::stimme_stoppen(&h, false));
                }
            }
        } else {
            if let Ok(mut g) = EINZEL_OPTION.lock() { *g = doppel_control::Stand::default(); }
            if doppel_option_ereignis(kind, event) {
                if let Some(app) = APP.get() {
                    let h = app.clone();
                    let _ = app.run_on_main_thread(move || {
                        if !super::stimme_laeuft() { super::stimme_starten(&h) }
                    });
                }
            }
        }
        // the Option key-up itself passes on unchanged
        if doppel_control_ereignis(kind, event) {
            RUNDE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if let Some(app) = APP.get() {
                let h = app.clone();
                let _ = app.run_on_main_thread(move || super::global_sicht_umschalten(&h));
            }
            // the Control key-up itself passes on unchanged
        }
        if kind == 22 {
            // Trackpad gesture phases never reach the WebView's DOM: hand
            // them to the window overview (tab paging) when over it.
            if let Some(app) = APP.get() {
                let p = unsafe { CGEventGetLocation(event) };
                let f = |n: u32| unsafe { CGEventGetIntegerValueField(event, n) };
                if super::window_overview::rad(app, p.x, p.y, f(97), f(96), f(99), f(123)) {
                    // Shortcut 9 owns scrolling inside its bounds.  The
                    // WebView receives the normalized event above; letting
                    // the original event continue would also scroll the
                    // Desktop/application behind the non-activating panel.
                    return std::ptr::null_mut();
                }
            }
            return event;
        }
        // Maustaste irgendwo AUSSERHALB der Noki-Ansicht beendet das
        // Fern-Tippen sofort (wie ein Klick in ein anderes Feld). Innerhalb
        // entscheidet der Helfer: Eingabefeld -> weiter, sonst -> Ende.
        // Ein offenes echtes Menue eines Noki-Fensters: macOS gibt den naechsten
        // Klick sonst dem Menue-Tracking dieses Programms (es schliesst nur).
        // Klicks IN der Noki-Ansicht nimmt Noki selbst und leitet sie ueber den
        // normalen Klickweg der Ansicht (Menue-Ebene liegt oben) weiter.
        if matches!(kind, 1 | 2 | 3 | 4) {
            static MENUE_DRUCK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            let p = unsafe { CGEventGetLocation(event) };
            let offen = super::vorschau::menue_offen_oeffentlich();
            let drin = super::vorschau::punkt_in_ansicht(p.x, p.y);
            if kind == 1 && offen && drin {
                MENUE_DRUCK.store(true, std::sync::atomic::Ordering::SeqCst);
                super::vorschau::tap_druck(true, p.x, p.y);
                return std::ptr::null_mut();
            }
            if kind == 2 && MENUE_DRUCK.swap(false, std::sync::atomic::Ordering::SeqCst) {
                super::vorschau::tap_druck(false, p.x, p.y);
                return std::ptr::null_mut();
            }
            if matches!(kind, 3 | 4) && offen && drin {
                return std::ptr::null_mut();
            }
            if kind == 2 || kind == 4 { return event; }
        }
        if matches!(kind, 1 | 3 | 25) {
            let p = unsafe { CGEventGetLocation(event) };
            if super::vorschau::punkt_in_ansicht(p.x, p.y) {
                super::fern_tippen::klick_in_ansicht();
            } else {
                super::fern_tippen::interaction_beenden("click_outside");
            }
            return event;
        }
        if kind == 10 || kind == 11 {
            // Eigene Weiterreichung an Noki Schreibtisch: nie ein zweites Mal.
            if unsafe { CGEventGetIntegerValueField(event, 42) } == super::fern_tippen::MARKE {
                return event;
            }
            // Von Noki selbst erzeugte Tasten (z. B. Rollen per Pfeil) sind
            // nie ein Kuerzel des Nutzers (41 = kCGEventSourceUnixProcessID).
            if unsafe { CGEventGetIntegerValueField(event, 41) } == std::process::id() as i64 {
                return event;
            }
            // Die Praefix-Taste folgt der ECHTEN Tastatur: nur Hardware-
            // Tasten (Quell-PID 0), nur wenn sich der Typ aendert - einmal
            // pro Tastaturwechsel, kein Polling.
            if kind == 10 && unsafe { CGEventGetIntegerValueField(event, 41) } == 0 {
                let typ = unsafe { CGEventGetIntegerValueField(event, 10) };
                if typ > 0 && TASTATUR_TYP.swap(typ, std::sync::atomic::Ordering::AcqRel) != typ {
                    praefix_umziehen(praefix_fuer_typ(typ));
                }
            }
            // Receipt happens before typing/browser/AX routing, so a stuck
            // target cannot starve Noki's own shortcuts.
            let code = unsafe { CGEventGetIntegerValueField(event, 9) };
            let wiederholung = unsafe { CGEventGetIntegerValueField(event, 8) } != 0;
            if kind == 10 && event_tap_shortcut(code, wiederholung) {
                TAP_KURZBEFEHL_KEYUP.store(code, std::sync::atomic::Ordering::Release);
                return std::ptr::null_mut();
            }
            if kind == 11 && TAP_KURZBEFEHL_KEYUP
                .compare_exchange(code, -1, std::sync::atomic::Ordering::AcqRel,
                    std::sync::atomic::Ordering::Relaxed).is_ok()
            {
                return std::ptr::null_mut();
            }
            // Esc schliesst zuerst ein offenes echtes Menue auf Noki Schreibtisch.
            if kind == 10 && code == 53 && super::window_overview::is_open() {
                if let Some(app) = APP.get() {
                    let h = app.clone();
                    let _ = app.run_on_main_thread(move || super::window_overview::close(&h));
                }
                return std::ptr::null_mut();
            }
            if kind == 10 && code == 53
                && super::vorschau::menue_offen_oeffentlich()
            {
                std::thread::spawn(super::vorschau::menues_schliessen_oeffentlich);
                return std::ptr::null_mut();
            }
            if super::fern_tippen::interaction_aktiv() || super::fern_tippen::haelt() {
                if super::STIMME_LAEUFT.load(std::sync::atomic::Ordering::Relaxed) {
                    super::fern_tippen::beenden("voice_owns_keyboard");
                } else {
                    let code = unsafe { CGEventGetIntegerValueField(event, 9) };
                    let praefix_offen = PREFIX_UNTIL.lock().ok().and_then(|g| *g)
                        .is_some_and(|t| std::time::Instant::now() < t);
                    // Nokis eigene Kuerzel bleiben Nokis Kuerzel: die
                    // Praefix-Taste und, solange ihr Fenster offen ist,
                    // Ziffern und Pfeile. Alles andere gehoert dem Feld.
                    let noki_kuerzel = code == PRAEFIX_CODE.load(std::sync::atomic::Ordering::Relaxed)
                        || (praefix_offen && matches!(code,
                            18 | 19 | 20 | 21 | 23 | 22 | 26 | 28 | 25 | 29 | 123 | 124 | 125 | 126));
                    if !noki_kuerzel {
                        return if super::fern_tippen::taste(kind, event) {
                            std::ptr::null_mut()
                        } else {
                            event
                        };
                    }
                }
            }
            if kind == 11 {
                return event;
            }
            // Esc schliesst die Vollansicht von Noki Schreibtisch (wenn nicht
            // gerade getippt wird - dann beendet Esc zuerst das Tippen).
            if unsafe { CGEventGetIntegerValueField(event, 9) } == 53
                && super::vorschau::voll_aktiv()
                && !super::STIMME_LAEUFT.load(std::sync::atomic::Ordering::Relaxed)
            {
                super::vorschau::voll(false);
                return std::ptr::null_mut();
            }
        }
        if matches!(kind, 5 | 6 | 7 | 27) {
            let point = unsafe { CGEventGetLocation(event) };
            // The overview panel never activates Noki, so WebKit sees no
            // hover there: feed it the pointer (throttled, panel only).
            if let Some(app) = APP.get() {
                super::window_overview::zeiger(app, point.x, point.y);
            }
            if let Some((x, y)) = super::virtual_workspace::guard_pointer(point.x, point.y) {
                unsafe { CGEventSetLocation(event, CgPoint { x, y }) };
                super::virtual_workspace::trace(&format!(
                    "[POINTER] virtual_entry=blocked from={:.0},{:.0} to={:.0},{:.0}",
                    point.x, point.y, x, y
                ));
            }
            return event;
        }
        // keyDown: NUR waehrend des Zuhoerens gehoert die Leertaste Noki.
        //
        // Ein modifikatorloser Carbon-Hotkey auf die Leertaste laesst sich
        // nicht registrieren (gemessen) - deshalb dieser Weg. Der Tap sieht
        // ohnehin schon flagsChanged; er bekommt keyDown dazu und gibt
        // ALLES unveraendert weiter, ausser dieser einen Taste in diesem
        // einen Zustand. Steht STIMME_LAEUFT nicht, ist der Zweig eine
        // einzige Abfrage und die Leertaste bleibt eine Leertaste.
        if kind == 10 {
            // PFEIL IM PRAEFIX-FENSTER: einen Schreibtisch weiter.
            //
            // Nur solange das Fenster nach ^/° offen ist. Danach ist ein
            // Pfeil wieder ein Pfeil - der Abgriff gibt ihn unveraendert
            // weiter, wie jede andere Taste auch.
            let code = unsafe { CGEventGetIntegerValueField(event, 9) };
            if matches!(code, 123 | 124 | 125 | 126) {
                let offen = PREFIX_UNTIL
                    .lock()
                    .ok()
                    .and_then(|g| *g)
                    .map_or(false, |t| std::time::Instant::now() < t);
                // Gehaltene Taste: macOS schickt denselben Druck wieder und
                // wieder. Ein Kuerzel ist EIN Druck - sonst rauscht der
                // Bildschirm durch die Schreibtische.
                let wiederholung =
                    unsafe { CGEventGetIntegerValueField(event, 8) } != 0;
                if offen && !wiederholung {
                    // 124 = rechts, 125 = runter -> vorwaerts.
                    let vor = code == 124 || code == 125;
                    // NUR die Rolle waehlen (`arbeitsplatz_waehlen`) - keine
                    // Navigation, kein Umzug, kein Verbergen. Frueher stand
                    // hier ein ECHTER Schreibtischwechsel, waehrend die
                    // Pruefung ueber einen Nebenweg nur die Wahl testete. Die Arbeit wird nur
                    // eingereiht; dieser Rueckruf tut selbst nichts Teures.
                    if let Some(app) = APP.get() {
                        super::arbeitsplatz_wahl_planen(app, vor);
                    }
                    // Das Fenster bleibt fuer den naechsten Pfeil offen.
                    if let Ok(mut until) = PREFIX_UNTIL.lock() {
                        *until = Some(std::time::Instant::now()
                            + std::time::Duration::from_millis(FENSTER_MS));
                    }
                    return std::ptr::null_mut(); // verbraucht
                }
                if offen {
                    return std::ptr::null_mut(); // Wiederholung: still schlucken
                }
            }
            if !super::STIMME_LAEUFT.load(std::sync::atomic::Ordering::Relaxed) {
                return event;
            }
            if code != 49 && code != 53 {
                return event; // alles andere tippt weiter ganz normal
            }
            if let Some(app) = APP.get() {
                let h = app.clone();
                let abbruch = code == 53; // Esc bricht ab, ohne auszufuehren
                let _ = app.run_on_main_thread(move || super::stimme_stoppen(&h, abbruch));
            }
            return std::ptr::null_mut(); // geschluckt: erreicht keine App
        }
        if kind != 12 {
            return event;
        } // flagsChanged; never swallow normal typing
        let (code, flags) = unsafe {
            (
                CGEventGetIntegerValueField(event, 9),
                CGEventGetFlags(event),
            )
        };
        // (prefix + Right Shift no longer toggles Noki: Double-Control does,
        // detected at the top of this callback for every event kind.)
        let _ = (code, flags);
        event
    }
    fn right_shift_monitor() {
        TAP_WATCHDOG.get_or_init(|| {
            std::thread::spawn(|| loop {
                std::thread::sleep(std::time::Duration::from_secs(2));
                let tap = TAP.load(std::sync::atomic::Ordering::Acquire);
                if tap != 0 && unsafe { !CGEventTapIsEnabled(tap as *mut c_void) } {
                    unsafe { CGEventTapEnable(tap as *mut c_void, true) };
                    eprintln!("[RECOVERY] subsystem=SHORTCUT_EVENT_TAP action=reenable");
                }
            });
        });
        std::thread::spawn(|| loop { unsafe {
            // Option 0 statt "nur zuhoeren": nur so laesst sich die
            // Leertaste im Zuhoer-Zustand wirklich abfangen. Maske jetzt
            // flagsChanged UND keyDown.
            let mouse_mask = (1u64 << 5) | (1u64 << 6) | (1u64 << 7) | (1u64 << 27);
            // keyUp (11) und Maustasten (1, 3, 25) nur fuer das Fern-Tippen:
            // ohne aktiven Modus gehen sie unveraendert durch.
            let fern_mask = (1u64 << 11) | (1u64 << 1) | (1u64 << 2) | (1u64 << 3) | (1u64 << 4) | (1u64 << 25);
            // 22 = scroll wheel: only read (phases for the overview's tab
            // paging), always passed on unchanged.
            let tap = CGEventTapCreate(1, 0, 0, (1 << 12) | (1 << 10) | (1 << 22) | mouse_mask | fern_mask,
                modifier_event, std::ptr::null_mut());
            if tap.is_null() {
                eprintln!("[SHORTCUT] Right-Shift-Monitor nicht verfügbar (Input-Monitoring/Bedienungshilfen); retry=2s");
                std::thread::sleep(std::time::Duration::from_secs(2));
                continue;
            }
            let source = CFMachPortCreateRunLoopSource(std::ptr::null(), tap, 0);
            if source.is_null() {
                eprintln!("[SHORTCUT] Right-Shift-Runloop nicht verfügbar; retry=2s");
                CFRelease(tap);
                std::thread::sleep(std::time::Duration::from_secs(2));
                continue;
            }
            CFRunLoopAddSource(CFRunLoopGetCurrent(), source, kCFRunLoopCommonModes);
            TAP.store(tap as usize, std::sync::atomic::Ordering::Release);
            CGEventTapEnable(tap, true);
            eprintln!("[SHORTCUT] ^/° → physische rechte Shift aktiv (Keycode 60)");
            CFRunLoopRun();
            let _ = TAP.compare_exchange(tap as usize, 0,
                std::sync::atomic::Ordering::AcqRel, std::sync::atomic::Ordering::Relaxed);
            CFRelease(source);
            CFRelease(tap);
            eprintln!("[RECOVERY] subsystem=SHORTCUT_EVENT_TAP action=recreate");
            std::thread::sleep(std::time::Duration::from_millis(250));
        }});
    }
    fn ziffern_belegen() {
        let mut z = match ZIFFERN.lock() {
            Ok(z) => z,
            Err(_) => return,
        };
        if !z.is_empty() {
            return;
        }
        unsafe {
            let ziel = GetApplicationEventTarget();
            for (n, code) in [
                (1u32, 18u32),
                (2, 19),
                (3, 20),
                (4, 21),
                (5, 23),
                (6, 22),
                (7, 26),
                (8, 28),
                (9, 25),
                (10, 29),
                // (11 = ^/° + Leertaste war "Noki hoert zu" - entfernt; das
                // Zuhoeren startet jetzt mit Option zweimal.)
                // Die PFEILTASTEN stehen hier bewusst NICHT.
                //
                // Gemessen am laufenden Programm: `RegisterEventHotKey`
                // nimmt sie ohne Fehler an (kein "liess sich nicht belegen"),
                // stellt sie aber NIE zu - macOS behaelt die Pfeile fuer
                // sich. Das Kuerzel war damit tot: kein Ereignis, keine
                // Wirkung, kein Protokolleintrag. Sie laufen deshalb ueber
                // den Ereignis-Abgriff, der ohnehin schon Tastendruecke
                // sieht (siehe `modifier_event`).
            ] {
                // 1..9: Aktionen   // 10: 0 = Ask Noki   // 0: eigene ID PRAEFIX+10 – PRAEFIX+0 waere die Praefix-Taste selbst
                let mut r = std::ptr::null_mut();
                if RegisterEventHotKey(
                    code,
                    0,
                    HotKeyId {
                        signatur: vz(b"NOKI"),
                        id: PRAEFIX + n,
                    },
                    ziel,
                    0,
                    &mut r,
                ) == 0
                {
                    z.push(r as usize);
                } else {
                    eprintln!("[SHORTCUT] Kuerzel {n} (Keycode {code}) liess sich NICHT belegen");
                }
            }
        }
    }
    const fn vz(s: &[u8; 4]) -> u32 {
        ((s[0] as u32) << 24) | ((s[1] as u32) << 16) | ((s[2] as u32) << 8) | (s[3] as u32)
    }
    /// Carbon-Rueckruf auf dem Hauptthread. Genau wie beim Abgriff: keine
    /// Panik darf hinaus - sie beendete Noki (zehn Absturzberichte,
    /// `index out of bounds` in diesem Rahmen).
    extern "C" fn gedrueckt(c: *mut c_void, ev: *mut c_void, u: *mut c_void) -> i32 {
        match std::panic::catch_unwind(|| gedrueckt_innen(c, ev, u)) {
            Ok(r) => r,
            Err(_) => {
                eprintln!("[SHORTCUT] Fehler im Kuerzel-Handler abgefangen - Noki laeuft weiter");
                0
            }
        }
    }

    fn gedrueckt_innen(_c: *mut c_void, ev: *mut c_void, _u: *mut c_void) -> i32 {
        let mut hk = HotKeyId { signatur: 0, id: 0 };
        let st = unsafe {
            GetEventParameter(
                ev,
                vz(b"----"),
                vz(b"hkid"),
                std::ptr::null_mut(),
                std::mem::size_of::<HotKeyId>(),
                std::ptr::null_mut(),
                &mut hk as *mut HotKeyId as *mut c_void,
            )
        };
        if st != 0 || hk.signatur != vz(b"NOKI") || hk.id < PRAEFIX || hk.id > PRAEFIX + 13 {
            return -9874;
        } // nicht unseres
          // Genau EINMAL je Tastendruck: Prellen abfangen (Praefix knapp, damit
          // kurze Folge Praefix/Ziffer moeglich bleibt; Ziffern 400 ms).
          // Wiederholungssperre. 400 ms sind fuer eine AKTION richtig (eine Ziffer
          // soll nicht zweimal feuern), fuer einen UMSCHALTER aber falsch: wer
          // Noki mit Cmd+0 oeffnet und gleich wieder schliesst, liegt fast immer
          // unter 400 ms - der zweite Druck wurde schlicht verschluckt, und es sah
          // aus, als schliesse der Shortcut nicht. Fuer den Umschalter bleibt nur
          // eine echte Auto-Repeat-Sperre stehen.
        let sperre = if hk.id == PRAEFIX || hk.id == PRAEFIX + 10 {
            80
        } else {
            400
        };
        if hk.id == PRAEFIX
            && SCHWARM_PRAEFIX_BIS.swap(0, std::sync::atomic::Ordering::AcqRel) > jetzt_ms()
        {
            return 0; // war der Druck, der den Timer-Schwarm beendet hat
        }
        if hk.id == PRAEFIX {
            if let Ok(s) = SPERRE.lock() {
                if s.map_or(false, |t| std::time::Instant::now() < t) {
                    return 0;
                }
            }
            // Kompakte Uebersicht offen: EIN Tipp schliesst sie (Taste verbraucht,
            // Ziffern bleiben frei, kein ^ erreicht eine App).
            if let Some(app) = APP.get() {
                if super::kompakt_offen(app) {
                    // Fensterarbeit NICHT im Rueckruf selbst: eingereiht.
                    let h = app.clone();
                    let _ = app.run_on_main_thread(move || super::kompakt_schliessen(&h));
                    ziffern_frei();
                    RUNDE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if let Ok(mut z) = ZULETZT.lock() {
                        if let Some(slot) = z.get_mut(PRAEFIX as usize) {
                            *slot = None;
                        }
                    }
                    if let Ok(mut s) = SPERRE.lock() {
                        *s = Some(
                            std::time::Instant::now()
                                + std::time::Duration::from_millis(DOPPEL_MS as u64),
                        );
                    }
                    return 0;
                }
            }
        }
        // NIEMALS mit [] in diese Liste greifen.
        //
        // Diese Funktion ist ein `extern "C"`-Rueckruf auf dem Hauptthread.
        // Eine Panik kann daraus nicht herauslaufen - Rust bricht den ganzen
        // Prozess ab (SIGABRT). Genau das ist passiert: eine Kuerzel-Kennung
        // jenseits der Listenlaenge, ein Griff ins Leere, und Noki war weg.
        // Zehn Absturzberichte an einem Abend, und fuer den Nutzer sah es
        // aus, als beende sich Noki beim Druecken des Kuerzels.
        //
        // Ein unbekanntes Kuerzel ist ein Grund, nichts zu tun - kein Grund,
        // das Programm zu beenden.
        let Ok(mut z) = ZULETZT.lock() else { return 0 };
        let Some(&vorher) = z.get(hk.id as usize) else {
            eprintln!("[SHORTCUT] unbekannte Kuerzel-Kennung {} - ignoriert", hk.id);
            return 0;
        };
        let mut doppelt = false;
        {
            let jetzt = std::time::Instant::now();
            if vorher.map_or(false, |t| jetzt.duration_since(t).as_millis() < sperre) {
                if hk.id == PRAEFIX + 10 {
                    eprintln!("[CMD0] Druck verworfen (Auto-Repeat-Sperre {sperre}ms)");
                }
                return 0;
            }
            // Doppeltipp auf die Praefix-Taste (< DOPPEL_MS) -> Noki Einstellungen Toggle.
            // Der dritte Tipp beginnt wieder von vorn (Zeitstempel geloescht).
            doppelt = hk.id == PRAEFIX
                && vorher.map_or(false, |t| jetzt.duration_since(t).as_millis() < DOPPEL_MS);
            if let Some(slot) = z.get_mut(hk.id as usize) {
                *slot = if doppelt { None } else { Some(jetzt) };
            }
        }
        drop(z);
        if doppelt {
            ziffern_frei();
            RUNDE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if let Some(app) = APP.get() {
                eprintln!("[SHORTCUT] ^/° doppelt -> Einstellungen Toggle");
                let _ = app.emit("noki://einstellungen_toggle", serde_json::json!({}));
            }
            return 0;
        }
        if hk.id == PRAEFIX {
            // Praefix: Ziffern kurz belegen, danach verfallen sie von selbst.
            ziffern_belegen();
            if let Ok(mut until) = PREFIX_UNTIL.lock() {
                *until =
                    Some(std::time::Instant::now() + std::time::Duration::from_millis(FENSTER_MS));
            }
            let runde = RUNDE.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            if let Some(app) = APP.get() {
                let h = app.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(FENSTER_MS));
                    if RUNDE.load(std::sync::atomic::Ordering::SeqCst) == runde {
                        let _ = h.run_on_main_thread(ziffern_frei);
                    }
                });
            }
            return 0;
        }
        // Ziffer im Praefix-Fenster: ein Kuerzel = eine Aktion, Ziffern sofort wieder frei.
        ziffern_frei();
        if let Ok(mut z) = ZULETZT.lock() {
            if let Some(slot) = z.get_mut(PRAEFIX as usize) { *slot = None; }
        }
        RUNDE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(app) = APP.get() {
            let n = hk.id - PRAEFIX;
            let h = app.clone();
            if n == 13 {
                // Leertaste waehrend des Zuhoerens: beenden und auswerten.
                let _ = app.run_on_main_thread(move || super::stimme_stoppen(&h, false));
            } else if n == 12 || n == 10 {
                // ^/° + 0: Ask Noki auf <-> zu, immer dasselbe Fenster.
                let _ = app.run_on_main_thread(move || super::ask_umschalten(&h));
            } else {
                super::shortcut_einreihen(&h, n);
            }
        }
        0
    }
    pub fn registrieren(app: &tauri::AppHandle) {
        let _ = APP.set(app.clone());
        right_shift_monitor();
        unsafe {
            let ziel = GetApplicationEventTarget();
            let typ = EventTyp {
                klasse: vz(b"keyb"),
                art: 5,
            }; // kEventHotKeyPressed
            let mut h = std::ptr::null_mut();
            if InstallEventHandler(ziel, gedrueckt, 1, &typ, std::ptr::null_mut(), &mut h) != 0 {
                eprintln!("[SHORTCUT] Tasten-Handler nicht installiert");
                return;
            }
            // Ctrl+1..5 sind nicht mehr belegt (bleiben macOS). Nur die Praefix-Taste.
            // Zuerst der an einer echten Taste bestaetigte Keycode; nur ohne
            // ihn der Typ des letzten Tastendrucks. Der Tap korrigiert beim
            // ersten echten Tastendruck (praefix_umziehen).
            let gemerkt = praefix_gemerkt();
            let quelle = if gemerkt.is_some() { "gemerkt" } else if praefix_ioreg().is_some() { "iokit" } else { "letzter_tastendruck" };
            let code = gemerkt.or_else(praefix_ioreg)
                .unwrap_or_else(|| praefix_fuer_typ(LMGetKbdType() as i64));
            eprintln!("[SHORTCUT] Praefix-Quelle={quelle}");
            let iso = code == 10;
            PRAEFIX_CODE.store(code, std::sync::atomic::Ordering::Relaxed);
            let st = praefix_carbon(code);
            if st != 0 {
                eprintln!(
                    "[SHORTCUT] Praefix-Taste ^/° (Keycode {}) nicht registriert (Fehler {})",
                    code, st
                );
            } else {
                eprintln!("[SHORTCUT] Praefix-Taste ^/° registriert (Keycode {}, {}), danach 1..9", code, if iso { "ISO" } else { "ANSI" });
            }

            // Cmd+0 wird bewusst NICHT mehr belegt: der Umschalter ist ^/° + 0.
            super::CMD0_NATIV.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// Besitzt der globale Carbon-Hotkey Cmd+0? Dann duerfen die WebViews NICHT
/// zusaetzlich auf dieselbe Taste reagieren - sonst entscheidet der eine Pfad
/// "verbergen" und der andere im selben Tastendruck "zeigen".
pub static CMD0_NATIV: AtomicBool = AtomicBool::new(false);

#[tauri::command]
fn noki_cmd0_nativ() -> bool {
    CMD0_NATIV.load(Ordering::Relaxed)
}

// ---- Maus folgen ------------------------------------------------------
struct MausMenue(tauri::menu::MenuItem<tauri::Wry>);

/// Die Seite meldet, ob "Maus folgen" laeuft -> Menuetext.
#[tauri::command]
fn noki_maus_menue(app: tauri::AppHandle, an: bool) {
    if let Some(m) = app.try_state::<MausMenue>() {
        let _ = m.0.set_text(if an {
            "Maus folgen beenden"
        } else {
            "Maus folgen"
        });
    }
}

// ---- Groesse (stufenlos) ---------------------------------------------
// Der Regler sitzt DIREKT im Untermenue "Groesse" (natives NSMenuItem mit
// eigener Ansicht, mod regler). Noki gleitet selbst zum Wert.
struct GroesseWert(std::sync::Mutex<f64>);

fn groesse_setzen(app: &tauri::AppHandle, faktor: f64) {
    if !faktor.is_finite() {
        return;
    }
    let f = faktor.clamp(0.45, 2.5);
    if let Some(g) = app.try_state::<GroesseWert>() {
        if let Ok(mut v) = g.0.lock() {
            *v = f;
        }
    }
    let _ = app.emit("noki://groesse", serde_json::json!({ "faktor": f }));
}

#[cfg(target_os = "macos")]
mod regler {
    use std::ffi::{c_void, CString};
    use std::os::raw::c_char;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tauri::Manager;
    type Id = *mut c_void;
    type Sel = *const c_void;
    #[link(name = "objc")]
    extern "C" {
        fn objc_getClass(n: *const c_char) -> Id;
        fn sel_registerName(n: *const c_char) -> Sel;
        fn objc_msgSend();
        fn objc_allocateClassPair(sup: Id, n: *const c_char, extra: usize) -> Id;
        fn objc_registerClassPair(c: Id);
        fn class_addMethod(c: Id, s: Sel, imp: *const c_void, typ: *const c_char) -> bool;
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct R {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }
    // NSTextAlignment: auf Apple Silicon gelten die iOS-Werte (Mitte 1, rechts 2).
    #[cfg(target_arch = "aarch64")]
    const MITTE: isize = 1;
    #[cfg(target_arch = "aarch64")]
    const RECHTS: isize = 2;
    #[cfg(not(target_arch = "aarch64"))]
    const MITTE: isize = 2;
    #[cfg(not(target_arch = "aarch64"))]
    const RECHTS: isize = 1;
    static APP: std::sync::OnceLock<tauri::AppHandle> = std::sync::OnceLock::new();
    static WERT: AtomicUsize = AtomicUsize::new(0);

    fn sel(n: &str) -> Sel {
        let c = CString::new(n).unwrap_or_default();
        unsafe { sel_registerName(c.as_ptr()) }
    }
    fn klasse(n: &str) -> Id {
        let c = CString::new(n).unwrap_or_default();
        unsafe { objc_getClass(c.as_ptr()) }
    }
    unsafe fn f() -> unsafe extern "C" fn() {
        objc_msgSend as unsafe extern "C" fn()
    }
    unsafe fn m0(o: Id, s: &str) -> Id {
        let g: unsafe extern "C" fn(Id, Sel) -> Id = std::mem::transmute(f());
        g(o, sel(s))
    }
    unsafe fn m1(o: Id, s: &str, a: Id) -> Id {
        let g: unsafe extern "C" fn(Id, Sel, Id) -> Id = std::mem::transmute(f());
        g(o, sel(s), a)
    }
    unsafe fn mf(o: Id, s: &str, a: f64) -> Id {
        let g: unsafe extern "C" fn(Id, Sel, f64) -> Id = std::mem::transmute(f());
        g(o, sel(s), a)
    }
    unsafe fn mi(o: Id, s: &str, a: isize) {
        let g: unsafe extern "C" fn(Id, Sel, isize) = std::mem::transmute(f());
        g(o, sel(s), a)
    }
    unsafe fn mr(o: Id, s: &str, r: R) -> Id {
        let g: unsafe extern "C" fn(Id, Sel, R) -> Id = std::mem::transmute(f());
        g(o, sel(s), r)
    }
    unsafe fn gd(o: Id, s: &str) -> f64 {
        let g: unsafe extern "C" fn(Id, Sel) -> f64 = std::mem::transmute(f());
        g(o, sel(s))
    }
    unsafe fn text(s: &str) -> Id {
        let c = CString::new(s).unwrap_or_default();
        let g: unsafe extern "C" fn(Id, Sel, *const c_char) -> Id = std::mem::transmute(f());
        g(klasse("NSString"), sel("stringWithUTF8String:"), c.as_ptr())
    }
    // Normal (1.00) liegt exakt in der Mitte: links 0.45-1.00, rechts 1.00-2.50.
    fn auf_faktor(p: f64) -> f64 {
        if p <= 0.5 {
            0.45 + p / 0.5 * 0.55
        } else {
            1.0 + (p - 0.5) / 0.5 * 1.5
        }
    }
    fn auf_pos(k: f64) -> f64 {
        if k <= 1.0 {
            (k - 0.45) / 0.55 * 0.5
        } else {
            0.5 + (k - 1.0) / 1.5 * 0.5
        }
    }

    extern "C" fn geaendert(_this: Id, _cmd: Sel, regler: Id) {
        unsafe {
            let mut p = gd(regler, "doubleValue");
            if (p - 0.5).abs() < 0.012 {
                p = 0.5;
                mf(regler, "setDoubleValue:", 0.5);
            } // an Normal einrasten
            let k = if p == 0.5 { 1.0 } else { auf_faktor(p) };
            let w = WERT.load(Ordering::Relaxed) as Id;
            if !w.is_null() {
                m1(w, "setStringValue:", text(&format!("{:.2}×", k)));
            }
            if let Some(app) = APP.get() {
                super::groesse_setzen(app, k);
            }
        }
    }

    unsafe fn beschriftung(s: &str, r: R, pt: f64, fett: bool, leise: bool, ausr: isize) -> Id {
        let t = m1(klasse("NSTextField"), "labelWithString:", text(s));
        mr(t, "setFrame:", r);
        m1(
            t,
            "setFont:",
            mf(
                klasse("NSFont"),
                if fett {
                    "boldSystemFontOfSize:"
                } else {
                    "systemFontOfSize:"
                },
                pt,
            ),
        );
        if leise {
            m1(
                t,
                "setTextColor:",
                m0(klasse("NSColor"), "secondaryLabelColor"),
            );
        }
        mi(t, "setAlignment:", ausr);
        t
    }

    // ---- Ctrl+4: dieselbe Auswahl wie "Fenster schliessen", frisch gebaut
    // (kein Wiederverwenden des eingehaengten Untermenues -> keine AppKit-
    // Ausnahme). Ein Eintrag sendet genau das Ereignis des Menuepunkts.
    static FZ_ZIEL: AtomicUsize = AtomicUsize::new(0);
    extern "C" fn fenster_gewaehlt(_this: Id, _cmd: Sel, eintrag: Id) {
        unsafe {
            let g: unsafe extern "C" fn(Id, Sel) -> isize = std::mem::transmute(f());
            let id = g(eintrag, sel("tag"));
            if let Some(app) = APP.get() {
                let _ = tauri::Emitter::emit(
                    app,
                    "noki://schliessen",
                    serde_json::json!({ "id": id as i64 }),
                );
            }
        }
    }
    // Hinweis-Eintrag "Bedienungshilfen fuer Noki aktivieren": oeffnet die Freigabe.
    extern "C" fn freigabe_oeffnen(_this: Id, _cmd: Sel, _e: Id) {
        super::ax_einstellungen_oeffnen();
    }
    pub fn fenster_auswahl(app: &tauri::AppHandle) {
        let _ = APP.set(app.clone());
        let frei = super::ax_einstieg(); // live pruefen, ggf. macOS-Dialog
        let liste = super::schliessbare_fenster();
        unsafe {
            let mut ziel = FZ_ZIEL.load(Ordering::Relaxed) as Id;
            if ziel.is_null() {
                let mut kl = klasse("NokiFensterZiel");
                if kl.is_null() {
                    let n = CString::new("NokiFensterZiel").unwrap_or_default();
                    let t = CString::new("v@:@").unwrap_or_default();
                    kl = objc_allocateClassPair(klasse("NSObject"), n.as_ptr(), 0);
                    class_addMethod(
                        kl,
                        sel("waehlen:"),
                        fenster_gewaehlt as *const c_void,
                        t.as_ptr(),
                    );
                    class_addMethod(
                        kl,
                        sel("freigabe:"),
                        freigabe_oeffnen as *const c_void,
                        t.as_ptr(),
                    );
                    objc_registerClassPair(kl);
                }
                ziel = m0(m0(kl, "alloc"), "init");
                FZ_ZIEL.store(ziel as usize, Ordering::Relaxed);
            }
            let menue = m1(
                m0(klasse("NSMenu"), "alloc"),
                "initWithTitle:",
                text("Fenster schließen"),
            );
            mi(menue, "setAutoenablesItems:", 0);
            let neu: unsafe extern "C" fn(Id, Sel, Id, Sel, Id) -> Id = std::mem::transmute(f());
            if liste.is_empty() {
                // Getrennt: Freigabe fehlt (anklickbar -> Einstellungen) vs. wirklich kein Fenster.
                let frei = frei || super::ax::vertraut();
                let hinweis = if frei {
                    "Kein schließbares Fenster"
                } else {
                    "Bedienungshilfen für Noki aktivieren …"
                };
                let aktion = if frei {
                    std::ptr::null()
                } else {
                    sel("freigabe:")
                };
                let e = neu(
                    m0(klasse("NSMenuItem"), "alloc"),
                    sel("initWithTitle:action:keyEquivalent:"),
                    text(hinweis),
                    aktion,
                    text(""),
                );
                if frei {
                    mi(e, "setEnabled:", 0);
                } else {
                    m1(e, "setTarget:", ziel);
                }
                m1(menue, "addItem:", e);
            }
            for (id, label) in liste {
                let e = neu(
                    m0(klasse("NSMenuItem"), "alloc"),
                    sel("initWithTitle:action:keyEquivalent:"),
                    text(&label),
                    sel("waehlen:"),
                    text(""),
                );
                m1(e, "setTarget:", ziel);
                mi(e, "setTag:", id as isize);
                m1(menue, "addItem:", e);
            }
            #[repr(C)]
            #[derive(Clone, Copy)]
            struct P {
                x: f64,
                y: f64,
            }
            let ort: unsafe extern "C" fn(Id, Sel) -> P = std::mem::transmute(f());
            let p = ort(klasse("NSEvent"), sel("mouseLocation"));
            let auf: unsafe extern "C" fn(Id, Sel, Id, P, Id) -> bool = std::mem::transmute(f());
            let _ = auf(
                menue,
                sel("popUpMenuPositioningItem:atLocation:inView:"),
                std::ptr::null_mut(),
                p,
                std::ptr::null_mut(),
            );
        }
    }

    /// Haengt den Regler in das ANGEZEIGTE Menue: Status-Item -> "Noki
    /// Einstellungen" -> "Groesse". Die Menueflaeche ist die des Systems.
    pub fn einbauen(app: &tauri::AppHandle, status_item: Id) {
        unsafe {
            let _ = APP.set(app.clone());
            let menue = m0(status_item, "menu");
            if menue.is_null() {
                return;
            }
            let ein = m1(menue, "itemWithTitle:", text("Noki Einstellungen"));
            if ein.is_null() {
                return;
            }
            let em = m0(ein, "submenu");
            if em.is_null() {
                return;
            }
            let gr = m1(em, "itemWithTitle:", text("Größe"));
            if gr.is_null() {
                return;
            }
            let gm = m0(gr, "submenu");
            if gm.is_null() {
                return;
            }
            let mut kl = klasse("NokiGroesseZiel");
            if kl.is_null() {
                let n = CString::new("NokiGroesseZiel").unwrap_or_default();
                let t = CString::new("v@:@").unwrap_or_default();
                kl = objc_allocateClassPair(klasse("NSObject"), n.as_ptr(), 0);
                class_addMethod(kl, sel("wert:"), geaendert as *const c_void, t.as_ptr());
                objc_registerClassPair(kl);
            }
            let ziel = m0(m0(kl, "alloc"), "init"); // lebt so lange wie die App
            let start = app
                .try_state::<super::GroesseWert>()
                .and_then(|g| g.0.lock().ok().map(|v| *v))
                .unwrap_or(1.0);
            let ansicht = mr(
                m0(klasse("NSView"), "alloc"),
                "initWithFrame:",
                R {
                    x: 0.0,
                    y: 0.0,
                    w: 264.0,
                    h: 66.0,
                },
            );
            let titel = beschriftung(
                "Größe",
                R {
                    x: 16.0,
                    y: 42.0,
                    w: 120.0,
                    h: 18.0,
                },
                13.0,
                true,
                false,
                0,
            );
            let wert = beschriftung(
                &format!("{:.2}×", start),
                R {
                    x: 140.0,
                    y: 42.0,
                    w: 108.0,
                    h: 18.0,
                },
                13.0,
                true,
                false,
                RECHTS,
            );
            let regler = mr(
                m0(klasse("NSSlider"), "alloc"),
                "initWithFrame:",
                R {
                    x: 14.0,
                    y: 18.0,
                    w: 236.0,
                    h: 24.0,
                },
            );
            mf(regler, "setMinValue:", 0.0);
            mf(regler, "setMaxValue:", 1.0);
            mf(regler, "setDoubleValue:", auf_pos(start));
            mi(regler, "setContinuous:", 1);
            m1(regler, "setTarget:", ziel);
            let g: unsafe extern "C" fn(Id, Sel, Sel) = std::mem::transmute(f());
            g(regler, sel("setAction:"), sel("wert:"));
            let links = beschriftung(
                "0.45×",
                R {
                    x: 16.0,
                    y: 2.0,
                    w: 60.0,
                    h: 14.0,
                },
                11.0,
                false,
                true,
                0,
            );
            let mitte = beschriftung(
                "Normal",
                R {
                    x: 102.0,
                    y: 2.0,
                    w: 60.0,
                    h: 14.0,
                },
                11.0,
                false,
                true,
                MITTE,
            );
            let rechts = beschriftung(
                "2.50×",
                R {
                    x: 188.0,
                    y: 2.0,
                    w: 60.0,
                    h: 14.0,
                },
                11.0,
                false,
                true,
                RECHTS,
            );
            let marke = mr(
                m0(klasse("NSView"), "alloc"),
                "initWithFrame:",
                R {
                    x: 131.5,
                    y: 40.0,
                    w: 1.0,
                    h: 5.0,
                },
            );
            mi(marke, "setWantsLayer:", 1);
            m1(
                m0(marke, "layer"),
                "setBackgroundColor:",
                m0(m0(klasse("NSColor"), "tertiaryLabelColor"), "CGColor"),
            );
            for v in [titel, wert, regler, links, mitte, rechts, marke] {
                m1(ansicht, "addSubview:", v);
            }
            let eintrag = m0(m0(klasse("NSMenuItem"), "alloc"), "init");
            m1(eintrag, "setView:", ansicht);
            m1(gm, "addItem:", eintrag);
            WERT.store(wert as usize, Ordering::Relaxed);
        }
    }
}

/// Das Frontend meldet, ob Freeze gerade aktiv ist -> Menuetext und
/// Eingabesperre: waehrend Freeze faengt das Noki-Overlay (volle Flaeche,
/// ueber dem Dock, unter der Menueleiste) alle Maus-Eingaben ab. Nur Noki
/// selbst bleibt bedienbar; "Freeze beenden" im Noki-Menue bleibt erreichbar.
static FREEZE_SPERRE: AtomicBool = AtomicBool::new(false);
static FREEZE_VORHER_APP: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[tauri::command]
fn noki_modus_menue(app: tauri::AppHandle, freeze: bool) {
    if let Some(m) = app.try_state::<ModusMenue>() {
        let _ =
            m.0.set_text(if freeze { "Freeze beenden" } else { "Freeze" });
    }
    if FREEZE_SPERRE.swap(freeze, Ordering::Relaxed) == freeze {
        return;
    }
    let Some(win) = app.get_webview_window(FENSTER) else {
        return;
    };
    if freeze {
        overlay_durchlaessig(&win, false);
        #[cfg(target_os = "macos")]
        freeze_fenster(&win, true);
    } else {
        #[cfg(target_os = "macos")]
        freeze_fenster(&win, false);
        lage_anwenden(&win, &app.state::<Lage>()); // Ebene wie gewaehlt
                                                   // Durchlaessigkeit uebernimmt wieder der Maus-Waechter (je 25 ms).
    }
}

/// Fensterebene ueber dem Dock (23, unter der Menueleiste 24) und die
/// vorherige App merken bzw. am Ende wieder nach vorn holen.
#[cfg(target_os = "macos")]
fn freeze_fenster(win: &WebviewWindow, an: bool) {
    use std::ffi::c_void;
    #[link(name = "objc")]
    extern "C" {
        fn sel_registerName(n: *const std::os::raw::c_char) -> *const c_void;
        fn objc_getClass(n: *const std::os::raw::c_char) -> *mut c_void;
        fn objc_msgSend();
        fn objc_retain(o: *mut c_void) -> *mut c_void;
        fn objc_release(o: *mut c_void);
    }
    unsafe {
        let f = objc_msgSend as unsafe extern "C" fn();
        let sel = |n: &[u8]| sel_registerName(n.as_ptr() as *const _);
        let hol: unsafe extern "C" fn(*mut c_void, *const c_void) -> *mut c_void =
            std::mem::transmute(f);
        let lvl: unsafe extern "C" fn(*mut c_void, *const c_void, isize) = std::mem::transmute(f);
        let akt: unsafe extern "C" fn(*mut c_void, *const c_void, usize) -> bool =
            std::mem::transmute(f);
        let ws = hol(
            objc_getClass(b"NSWorkspace\0".as_ptr() as *const _),
            sel(b"sharedWorkspace\0"),
        );
        if an {
            let vorn = hol(ws, sel(b"frontmostApplication\0"));
            let alt = FREEZE_VORHER_APP.swap(
                if vorn.is_null() {
                    0
                } else {
                    objc_retain(vorn) as usize
                },
                Ordering::Relaxed,
            );
            if alt != 0 {
                objc_release(alt as *mut c_void);
            }
            if let Ok(nw) = win.ns_window() {
                lvl(nw, sel(b"setLevel:\0"), 23);
            }
        } else {
            let alt = FREEZE_VORHER_APP.swap(0, Ordering::Relaxed);
            if alt != 0 {
                let a = alt as *mut c_void;
                // Nur zurueckholen, wenn ein Klick waehrend Freeze JARVIS nach vorn geholt hat.
                let jetzt = hol(ws, sel(b"frontmostApplication\0"));
                let ich = hol(
                    objc_getClass(b"NSRunningApplication\0".as_ptr() as *const _),
                    sel(b"currentApplication\0"),
                );
                if jetzt == ich {
                    let space_before = cgs::aktiver_space().map(|x| x.0).unwrap_or(0);
                    let _ = akt(a, sel(b"activateWithOptions:\0"), 2);
                    space_action_log("freeze_restore_front_app", "previous_front_app", 0, space_before, cgs::aktiver_space().map(|x| x.0).unwrap_or(0));
                }
                objc_release(a);
            }
        }
    }
}

/// Ziel fuer Schwingen/Plasma: wo genau der rote Knopf dieses Fensters liegt.
#[tauri::command]
fn noki_schliess_ziel(id: i64) -> Option<SchliessKnopf> {
    #[cfg(target_os = "macos")]
    unsafe {
        let (k, r, _) = ax::knopf(id)?;
        ax::freigeben(k);
        Some(SchliessKnopf {
            x: r.0 + r.2 / 2.0,
            y: r.1 + r.3 / 2.0,
            w: r.2,
            h: r.3,
            id,
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = id;
        None
    }
}

/// Die EINE Schliess-Aktion aller Effekte. Gedrueckt wird nur, wenn der
/// Knopf noch dort liegt, wo Noki ihn trifft (x/y Schirmpunkte, +-8 P).
#[tauri::command]
fn noki_fenster_schliessen(id: i64, x: f64, y: f64) -> bool {
    #[cfg(target_os = "macos")]
    unsafe {
        // Fehler getrennt benennen — nicht alles ist "Freigabe fehlt".
        if !ax::vertraut() {
            eprintln!("[AX] schliessen {}: Bedienungshilfen-Freigabe fehlt", id);
            return false;
        }
        let Some((k, r, _)) = ax::knopf(id) else {
            let grund = if ax::existiert(id) {
                "Schliessknopf nicht eindeutig oder inaktiv"
            } else {
                "Zielfenster verschwunden"
            };
            eprintln!("[AX] schliessen {}: {}", id, grund);
            return false;
        };
        let da = ((r.0 + r.2 / 2.0) - x).abs() <= 8.0 && ((r.1 + r.3 / 2.0) - y).abs() <= 8.0;
        let ok = da && ax::druecken(k);
        if !da {
            eprintln!(
                "[AX] schliessen {}: Knopf liegt nicht mehr an der Zielstelle",
                id
            );
        } else if !ok {
            eprintln!("[AX] schliessen {}: AXPress fehlgeschlagen", id);
        }
        ax::freigeben(k);
        ok
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (id, x, y);
        false
    }
}

/// Meldet die Fensterliste ins Frontend, aber nur wenn sie sich geaendert
/// hat. Fenster bewegen sich in Menschentempo; 4 Hz reicht dafuer
/// vollstaendig und kostet nichts. Ein 60-Hz-Takt waere hier reine
/// Verschwendung — die Kollision selbst rechnet das Frontend jedes Bild.
const FENSTER_INTERVAL: Duration = Duration::from_millis(750);

fn spawn_fenster_watcher(app: tauri::AppHandle, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        let mut zuletzt: Option<Vec<Fenster>> = None;
        let mut menue: Option<Vec<(i64, String)>> = None;
        let mut menue_ax: Option<bool> = None; // Freigabe gewechselt -> Menue neu (auch bei gleicher Liste)
        let mut takt: u32 = 0;
        while !stop.load(Ordering::Relaxed) {
            // Im Vollbild gibt es kein "hinter einem Fenster": keine Verdecker melden.
            let jetzt = if im_vollbild() {
                Vec::new()
            } else {
                sichtbare_fenster()
            };
            let neu = zuletzt.as_ref() != Some(&jetzt);
            if neu {
                let _ = app.emit("noki://fenster", &jetzt);
                zuletzt = Some(jetzt);
            }
            // "Fenster schliessen": bei jeder Aenderung und alle ~3 s (Titel);
            // neu gebaut nur, wenn sich die Eintraege wirklich aendern.
            if neu || takt % 12 == 0 {
                let eintraege = schliessbare_fenster();
                let ax_jetzt = lesezeichen::ax_vertraut(false);
                if menue.as_ref() != Some(&eintraege) || menue_ax != Some(ax_jetzt) {
                    menue_ax = Some(ax_jetzt);
                    if let Some(m) = app.try_state::<SchliessMenue>() {
                        schliess_menue_fuellen(&app, &m.0, &eintraege);
                    }
                    menue = Some(eintraege);
                }
            }
            takt = takt.wrapping_add(1);
            thread::sleep(FENSTER_INTERVAL);
        }
    });
}

// =====================================================================
//  KONTEXT: SPOTIFY  (Abschnitt 30)
//
//  Noki soll sichtbar auf das reagieren, was auf dem Rechner passiert.
//  Der erste Fall: spielt Spotify gerade, traegt er Kopfhoerer.
//
//  Es gab im Projekt bisher KEINE Spielstandsinfrastruktur — nur eine
//  Namensliste erlaubter Apps fuer `app.open`. Gebaut wird deshalb die
//  kleinste read-only Bruecke, die es unter macOS gibt:
//
//    1. Laeuft der Prozess ueberhaupt?  `pgrep -x Spotify`
//       Ohne diesen Schritt wuerde der zweite die App STARTEN, statt sie
//       nur zu fragen — `tell application` oeffnet eine geschlossene App.
//    2. Wenn ja: `player state` per AppleScript. Read-only, liefert
//       playing / paused / stopped.
//
//  Keine Spotify-API, kein Konto, kein Netz. macOS fragt beim ersten
//  Zugriff einmal nach der Automations-Erlaubnis; verweigert der Nutzer
//  sie, liefert die Abfrage schlicht "nicht spielend" und Noki traegt
//  eben keine Kopfhoerer.
#[cfg(target_os = "macos")]
const SPOTIFY_INTERVAL: Duration = Duration::from_millis(4000);

/// Deutet die Antwort von `player state`. Eigene Funktion, damit der
/// Selbsttest sie ohne laufendes Spotify pruefen kann.
pub fn spotify_spielt_aus_ausgabe(roh: &str) -> bool {
    roh.trim().eq_ignore_ascii_case("playing")
}

/// Spielt EIN bestimmter Player gerade? In-Process Bundle-Pruefung
/// ohne pgrep-Fork/Exec, damit im Leerlauf keine unnoetigen Prozesse starten.
#[cfg(target_os = "macos")]
fn player_spielt(app: &str) -> bool {
    let bundle = match app {
        "Spotify" => "com.spotify.client",
        "Music" => "com.apple.Music",
        _ => return false,
    };
    if lesezeichen::pid_fuer_bundle(bundle).is_none() {
        return false;
    }
    let skript = format!("tell application \"{}\" to player state as string", app);
    use std::process::Command;
    Command::new("/usr/bin/osascript")
        .args(["-e", &skript])
        .output()
        .ok()
        .filter(|a| a.status.success())
        .map(|a| spotify_spielt_aus_ausgabe(&String::from_utf8_lossy(&a.stdout)))
        .unwrap_or(false)
}

/// Musik laeuft, wenn IRGENDEIN bekannter Player spielt. Bisher zaehlte
/// allein Spotify — wer ueber Apple Music hoerte, bekam nie Kopfhoerer zu
/// sehen, und von aussen sah das aus, als sei die Darstellung kaputt.
/// Es bleibt derselbe Zustand, dasselbe Ereignis, derselbe Empfaenger:
/// nur die Quelle ist breiter.
#[cfg(target_os = "macos")]
fn spotify_spielt() -> bool {
    player_spielt("Spotify") || player_spielt("Music")
}

#[cfg(not(target_os = "macos"))]
fn spotify_spielt() -> bool {
    false
}

/// Meldet den Spielstand ins Frontend — nur, wenn er sich geaendert hat.
/// Alle zwei Sekunden: Musik startet in Menschentempo, und der Renderer
/// liest ohnehin nur den zuletzt gemeldeten Stand.
#[cfg(target_os = "macos")]
fn spawn_spotify_watcher(app: tauri::AppHandle, stop: Arc<AtomicBool>) {
    // Ereignisgesteuert: Spotify und Musik melden jeden Wiedergabewechsel als
    // verteilte Mitteilung. Gemessen: das fruehere osascript alle 4 s startete
    // im Leerlauf dauernd einen Prozess (LaunchServices + AppleScript). Die
    // Abfrage bleibt nur als seltene Absicherung (60 s) und als erster Stand.
    #[cfg(target_os = "macos")]
    musik_mitteilungen::abonnieren();
    thread::spawn(move || {
        let mut zuletzt: Option<bool> = None;
        let mut abfrage_faellig = std::time::Instant::now();
        while !stop.load(Ordering::Relaxed) {
            let gemeldet = musik_mitteilungen::stand();
            let jetzt = if std::time::Instant::now() >= abfrage_faellig || zuletzt.is_none() {
                abfrage_faellig = std::time::Instant::now() + Duration::from_secs(60);
                let j = spotify_spielt();
                musik_mitteilungen::setzen(j);
                j
            } else {
                gemeldet.unwrap_or(zuletzt.unwrap_or(false))
            };
            if zuletzt != Some(jetzt) {
                virtual_workspace::trace(&format!("[MUSIK] spielt={jetzt}"));
                zuletzt = Some(jetzt);
            }
            // Always (re)send the current state: an event missed by the
            // page (start race, reload) left Noki without headphones while
            // music played until the next play/pause. Cheap: cached state,
            // no osascript (that stays at 60 s).
            let _ = app.emit("noki://spotify", serde_json::json!({ "spielt": jetzt }));
            musik_mitteilungen::warten(Duration::from_secs(10));
        }
    });
}

/// Verteilte Mitteilungen von Spotify/Musik ("Player State": Playing/...).
#[cfg(target_os = "macos")]
mod musik_mitteilungen {
    use block2::RcBlock;
    use objc2::{msg_send, runtime::{AnyClass, AnyObject}};
    use std::ptr::NonNull;
    use std::sync::{Condvar, Mutex};

    static STAND: Mutex<Option<bool>> = Mutex::new(None);
    static WECKER: Condvar = Condvar::new();

    pub fn stand() -> Option<bool> { STAND.lock().ok().and_then(|g| *g) }
    pub fn setzen(v: bool) { if let Ok(mut g) = STAND.lock() { *g = Some(v); } }
    /// Schlafen, bis eine Mitteilung kommt oder die Zeit um ist.
    pub fn warten(d: std::time::Duration) {
        if let Ok(g) = STAND.lock() {
            let vorher = *g;
            let _ = WECKER.wait_timeout_while(g, d, |s| *s == vorher);
        }
    }

    fn ns(s: &str) -> *mut AnyObject {
        let c = std::ffi::CString::new(s).unwrap_or_default();
        match AnyClass::get(c"NSString") {
            Some(k) => unsafe { msg_send![k, stringWithUTF8String: c.as_ptr()] },
            None => std::ptr::null_mut(),
        }
    }

    pub fn abonnieren() {
        let Some(k) = AnyClass::get(c"NSDistributedNotificationCenter") else { return };
        unsafe {
            let center: *mut AnyObject = msg_send![k, defaultCenter];
            if center.is_null() { return; }
            for name in ["com.spotify.client.PlaybackStateChanged", "com.apple.Music.playerInfo"] {
                let block = RcBlock::new(|note: NonNull<AnyObject>| {
                    let info: *mut AnyObject = msg_send![note.as_ptr(), userInfo];
                    if info.is_null() { return; }
                    let wert: *mut AnyObject = msg_send![info, objectForKey: ns("Player State")];
                    if wert.is_null() { return; }
                    let utf8: *const std::ffi::c_char = msg_send![wert, UTF8String];
                    if utf8.is_null() { return; }
                    let spielt = std::ffi::CStr::from_ptr(utf8).to_string_lossy().eq_ignore_ascii_case("playing");
                    if let Ok(mut g) = STAND.lock() { *g = Some(spielt); }
                    WECKER.notify_all();
                });
                let _: *mut AnyObject = msg_send![center, addObserverForName: ns(name),
                    object: std::ptr::null_mut::<AnyObject>(), queue: std::ptr::null_mut::<AnyObject>(),
                    usingBlock: &*block];
                std::mem::forget(block);
            }
        }
    }
}
#[cfg(not(target_os = "macos"))]
mod musik_mitteilungen {
    pub fn stand() -> Option<bool> { None }
    pub fn setzen(_v: bool) {}
    pub fn warten(d: std::time::Duration) { std::thread::sleep(d) }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct NokiHitZustand {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub drag: bool,
    /// Zweites klickbares Rechteck: die Arbeitsplatz-Vorschau unten links.
    /// Ohne sie waere das Overlay dort durchklickbar und weder der
    /// Schliessknopf noch der Sprung zu Nokis Schreibtisch erreichbar.
    pub vx: i32,
    pub vy: i32,
    pub vw: i32,
    pub vh: i32,
    /// Drittes Rechteck: das Transkriptfeld der Stimme. Das Overlay ist
    /// ueberall sonst durchklickbar - ohne diese Flaeche erreicht kein
    /// Mausrad den gesprochenen Text, egal was das CSS sagt. Nur dieses
    /// Feld, nicht die ganze Tafel: alles andere bleibt durchlaessig.
    pub sx: i32,
    pub sy: i32,
    pub sw: i32,
    pub sh: i32,
    /// Wo die Miniatur STEHT - nicht, wo das Overlay Klicks faengt.
    ///
    /// Das sind zwei verschiedene Fragen, und sie wurden verwechselt. Die
    /// Trefferzone `vx..vh` MUSS leer bleiben (das Overlay liegt ueber der
    /// Miniatur und schluckte sonst jeden Klick auf sie). Der Beobachter
    /// "Druck NEBEN die grosse Miniatur -> wieder kompakt" fragte aber genau
    /// dieses leere Rechteck ab - und bekam fuer JEDEN Druck "nicht drin".
    /// Ein Klick MITTEN in die grosse Miniatur liess sie deshalb
    /// zusammenklappen. Hier steht ihre wirkliche Flaeche.
    pub fx: i32,
    pub fy: i32,
    pub fw: i32,
    pub fh: i32,
    /// Viertes Rechteck: Nokis Sprechblase (Schliessen + Blaettern).
    pub bx: i32,
    pub by: i32,
    pub bw: i32,
    pub bh: i32,
    /// Fuenftes Rechteck: Noki Einstellungen. Eigenes Rechteck statt Teil
    /// der Vereinigung mit Noki - sonst wurde die Huelle aus Figur und
    /// Fenster (oft halber Bildschirm) zur unsichtbaren Klicksperre.
    pub ex: i32,
    pub ey: i32,
    pub ew: i32,
    pub eh: i32,
    /// Open panels (Ablage, Schnellzugriff, Werk, camera, Ask/Work), each its
    /// OWN rect. They used to be merged with Noki's figure into one bounding
    /// box - with Noki on one side of the screen and a panel on the other
    /// that box was an invisible click wall over everything in between.
    pub zonen: Vec<[i32; 4]>,
}

pub struct NokiHitStore(pub Arc<std::sync::RwLock<NokiHitZustand>>);

// =====================================================================
//  ABLAGE — Noki haelt REFERENZEN auf Dateien, nie die Dateien selbst.
//  Gespeichert werden nur Lesezeichen (NSURL-Bookmark, folgt Umbenennen/
//  Verschieben), Anzeigename, Typ, letzter Pfad und Zeitpunkt — in
//  <app_config_dir>/ablage.json. Keine Dateioperation ausser Oeffnen und
//  Im-Finder-Zeigen, und die nur auf ausdruecklichen Klick. Entfernen
//  loescht ausschliesslich den Eintrag.
// =====================================================================
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct AblageEintrag {
    pub id: u64,
    pub name: String,
    pub typ: String,
    pub ext: String,
    pub pfad: String,
    pub bm: String,
    pub zeit: u64,
    #[serde(default)]
    pub da: bool,
}
pub struct Ablage(pub Mutex<Vec<AblageEintrag>>);
pub struct AblageMenue(pub MenuItem<tauri::Wry>);

fn ablage_typ(p: &std::path::Path) -> (String, String) {
    if p.is_dir() {
        return ("ordner".into(), String::new());
    }
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let typ = match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "heic" | "webp" | "tif" | "tiff" | "bmp" | "svg" => "bild",
        "zip" | "rar" | "7z" | "tar" | "gz" | "tgz" | "dmg" => "archiv",
        "pdf" | "doc" | "docx" | "txt" | "md" | "rtf" | "pages" | "key" | "numbers" | "xls"
        | "xlsx" | "ppt" | "pptx" | "csv" => "dokument",
        _ => "datei",
    };
    (typ.into(), ext)
}

fn ablage_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect()
}

fn ablage_unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// Aktueller Ort der Referenz: erst das Lesezeichen, sonst der letzte Pfad.
/// Im Papierkorb gilt eine Datei als nicht verfuegbar.
pub fn ablage_aufloesen(e: &mut AblageEintrag) {
    if let Some(p) = ablage_unhex(&e.bm).and_then(|b| lesezeichen::aufloesen(&b)) {
        if !p.is_empty() && !p.contains("/.Trash/") {
            e.pfad = p;
        }
    }
    let pf = std::path::Path::new(&e.pfad);
    e.da = pf.exists() && !e.pfad.contains("/.Trash/");
    if e.da {
        if let Some(n) = pf.file_name() {
            e.name = n.to_string_lossy().into_owned();
        }
    }
}

/// Nimmt Pfade auf (neueste zuerst). Dieselbe Datei wird nicht doppelt
/// aufgenommen, sondern nach vorn geholt. Rueckgabe: (neue Ids, Duplikat-Id).
pub fn ablage_neu(
    liste: &mut Vec<AblageEintrag>,
    pfade: &[PathBuf],
    jetzt: u64,
) -> (Vec<u64>, Option<u64>) {
    let mut neu = Vec::new();
    let mut dup = None;
    for p in pfade {
        let echt = fs::canonicalize(p).unwrap_or_else(|_| p.clone());
        if !echt.exists() {
            continue;
        }
        let s = echt.to_string_lossy().into_owned();
        if let Some(i) = liste.iter().position(|e| e.pfad == s) {
            let e = liste.remove(i);
            dup = Some(e.id);
            liste.insert(0, e);
            continue;
        }
        let (typ, ext) = ablage_typ(&echt);
        let id = liste.iter().map(|e| e.id).max().unwrap_or(0) + 1;
        let name = echt
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| s.clone());
        let bm = lesezeichen::erstellen(&s)
            .map(|b| ablage_hex(&b))
            .unwrap_or_default();
        liste.insert(
            0,
            AblageEintrag {
                id,
                name,
                typ,
                ext,
                pfad: s,
                bm,
                zeit: jetzt,
                da: true,
            },
        );
        neu.push(id);
    }
    (neu, dup)
}

pub fn ablage_aus_json(roh: &str) -> Vec<AblageEintrag> {
    serde_json::from_str(roh).unwrap_or_default()
}

fn ablage_datei(app: &tauri::AppHandle) -> Option<PathBuf> {
    let dir = app.path().app_config_dir().ok()?;
    let _ = fs::create_dir_all(&dir);
    Some(dir.join("ablage.json"))
}

fn ablage_speichern(app: &tauri::AppHandle, liste: &[AblageEintrag]) {
    if let (Some(d), Ok(s)) = (ablage_datei(app), serde_json::to_string_pretty(liste)) {
        let _ = fs::write(d, s);
    }
}

fn ablage_laden(app: &tauri::AppHandle) -> Vec<AblageEintrag> {
    let mut l = ablage_datei(app)
        .and_then(|d| fs::read_to_string(d).ok())
        .map(|r| ablage_aus_json(&r))
        .unwrap_or_default();
    for e in l.iter_mut() {
        ablage_aufloesen(e);
    }
    l
}

/// Liste an Noki melden und die Zahl im Menue nachfuehren.
fn ablage_melden(app: &tauri::AppHandle, neu: &[u64], dup: Option<u64>, offen: Option<&str>) {
    let liste = {
        let st = app.state::<Ablage>();
        let mut l = st.0.lock().unwrap();
        for e in l.iter_mut() {
            ablage_aufloesen(e);
        }
        l.clone()
    };
    if let Some(m) = app.try_state::<AblageMenue>() {
        let t = if liste.is_empty() {
            "Dokumente".to_string()
        } else {
            format!("Dokumente ({})", liste.len())
        };
        let _ = m.0.set_text(t);
    }
    let _ = app.emit(
        "noki://ablage",
        serde_json::json!({ "liste": liste, "neu": neu, "dup": dup, "offen": offen }),
    );
}

fn ablage_aufnehmen(app: &tauri::AppHandle, pfade: &[PathBuf]) {
    let jetzt = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (neu, dup) = {
        let st = app.state::<Ablage>();
        let mut l = st.0.lock().unwrap();
        for e in l.iter_mut() {
            ablage_aufloesen(e);
        }
        let r = ablage_neu(&mut l, pfade, jetzt);
        ablage_speichern(app, &l);
        r
    };
    ablage_melden(app, &neu, dup, None);
}

/// Drop aus dem Finder: nur, wenn der Zeiger beim Loslassen wirklich ueber
/// Noki (bzw. seinem offenen Ablage-Panel) steht — dasselbe Trefferfeld,
/// das das Fenster ueberhaupt erst empfangsbereit macht.
fn ablage_drop(app: &tauri::AppHandle, pfade: Vec<PathBuf>) {
    let ueber = match (maus_schirm_position(), app.try_state::<NokiHitStore>()) {
        (Some((x, y)), Some(st)) => {
            st.0.read()
                .map(|h| {
                    h.w > 0
                        && x >= (h.x - 14) as f64
                        && x <= (h.x + h.w + 14) as f64
                        && y >= (h.y - 14) as f64
                        && y <= (h.y + h.h + 14) as f64
                })
                .unwrap_or(false)
        }
        _ => false,
    };
    eprintln!(
        "[RUST] ablage drop: {} Pfad(e), ueber Noki: {}",
        pfade.len(),
        ueber
    );
    if ueber && !pfade.is_empty() {
        ablage_aufnehmen(app, &pfade);
    }
}

fn ablage_pfad(app: &tauri::AppHandle, id: u64) -> Option<String> {
    let st = app.state::<Ablage>();
    let mut l = st.0.lock().unwrap();
    let e = l.iter_mut().find(|e| e.id == id)?;
    ablage_aufloesen(e);
    if e.da {
        Some(e.pfad.clone())
    } else {
        None
    }
}

#[tauri::command]
fn ablage_liste(app: tauri::AppHandle) -> Vec<AblageEintrag> {
    let st = app.state::<Ablage>();
    let mut l = st.0.lock().unwrap();
    for e in l.iter_mut() {
        ablage_aufloesen(e);
    }
    l.clone()
}

/// Nur auf ausdruecklichen Klick: mit der Standard-App oeffnen.
#[tauri::command]
fn ablage_oeffnen(app: tauri::AppHandle, id: u64) -> bool {
    ablage_pfad(&app, id)
        .map(|p| {
            std::process::Command::new("/usr/bin/open")
                .arg(&p)
                .spawn()
                .is_ok()
        })
        .unwrap_or(false)
}

/// Nur auf ausdruecklichen Klick: im Finder zeigen (markieren).
#[tauri::command]
fn ablage_finder(app: tauri::AppHandle, id: u64) -> bool {
    ablage_pfad(&app, id)
        .map(|p| {
            std::process::Command::new("/usr/bin/open")
                .args(["-R", &p])
                .spawn()
                .is_ok()
        })
        .unwrap_or(false)
}

/// Entfernt NUR den Eintrag — die Datei selbst bleibt unberuehrt.
#[tauri::command]
fn ablage_entfernen(app: tauri::AppHandle, id: u64) {
    {
        let st = app.state::<Ablage>();
        let mut l = st.0.lock().unwrap();
        l.retain(|e| e.id != id);
        ablage_speichern(&app, &l);
    }
    ablage_melden(&app, &[], None, None);
}

/// Leert NUR die Liste — keine Datei wird angefasst.
#[tauri::command]
fn ablage_leeren(app: tauri::AppHandle) {
    {
        let st = app.state::<Ablage>();
        let mut l = st.0.lock().unwrap();
        l.clear();
        ablage_speichern(&app, &l);
    }
    ablage_melden(&app, &[], None, None);
}

/// Ctrl+5: Ist der Finder (inkl. Schreibtisch) vorne und dort etwas
/// ausgewaehlt, kommen GENAU diese Dateien in die Ablage. Sonst — andere App
/// vorne, keine Auswahl, keine Freigabe — oeffnet/schliesst sich die Ablage.
fn ablage_shortcut(app: &tauri::AppHandle) {
    let h = app.clone();
    thread::spawn(move || {
        let pfade = finder_auswahl();
        eprintln!(
            "[ABLAGE] Ctrl+5: {} Datei(en) aus der Finder-Auswahl",
            pfade.len()
        );
        if pfade.is_empty() {
            ablage_melden(&h, &[], None, Some("umschalten"));
        } else {
            ablage_aufnehmen(&h, &pfade);
        }
    });
}

/// Die AKTUELLE Finder-Auswahl — nur wenn der Finder gerade vorne ist (keine
/// alte Auswahl aus einem Hintergrundfenster, kein Zwischenablage-Inhalt).
fn finder_auswahl() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        if lesezeichen::vorne_bundle().as_deref() != Some("com.apple.finder") {
            return Vec::new();
        }
        let skript = "tell application \"Finder\"\nset o to \"\"\nrepeat with a in (get selection as alias list)\nset o to o & POSIX path of a & linefeed\nend repeat\nreturn o\nend tell";
        match std::process::Command::new("/usr/bin/osascript")
            .args(["-e", skript])
            .output()
        {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .map(PathBuf::from)
                .filter(|p| p.exists())
                .collect(),
            Ok(o) => {
                eprintln!(
                    "[ABLAGE] Finder-Auswahl nicht lesbar: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                );
                Vec::new()
            }
            Err(_) => Vec::new(),
        }
    }
    #[cfg(not(target_os = "macos"))]
    Vec::new()
}

/// Echtes macOS-Drag-out: startet eine native Ziehsitzung mit der Datei-URL
/// (wie ein Finder-Zug). Nur "Kopieren" — das Original wird nie verschoben;
/// der Eintrag bleibt in der Ablage.
#[tauri::command]
fn ablage_ziehen(app: tauri::AppHandle, id: u64) -> bool {
    let p = match ablage_pfad(&app, id) {
        Some(p) => p,
        None => return false,
    };
    #[cfg(target_os = "macos")]
    if let Some(win) = app.get_webview_window(FENSTER) {
        if let Ok(nw) = win.ns_window() {
            let nw = nw as usize;
            let _ = app.run_on_main_thread(move || {
                let ok = unsafe { lesezeichen::ziehen(nw as *mut std::ffi::c_void, &p) };
                eprintln!("[ABLAGE] Drag-out {}: {}", p, ok);
            });
            return true;
        }
    }
    let _ = p;
    false
}

// =====================================================================
//  ARBEIT — Werkzeuge hinter dem Menue "Arbeit": Zwischenablage-Verlauf,
//  Arbeitsplatz-Profile, Fokus-Apps, Energie fuer den Fokus. Alles lokal
//  in <app_config_dir>; keine Netzwerkverbindung, keine JARVIS-Anbindung.
// =====================================================================
pub struct EnergieMenue(pub Vec<(String, CheckMenuItem<tauri::Wry>)>);

fn werkzeug_zeigen(app: &tauri::AppHandle, was: &str) {
    // Die Shortcut-Uebersicht IST die kompakte Noki-Uebersicht (eigenes Fenster).
    if was == "shortcuts" {
        kompakt_zeigen(app);
        return;
    }
    let _ = app.emit("noki://werkzeug", serde_json::json!({ "was": was }));
}

fn werk_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    let d = app.path().app_config_dir().ok()?;
    let _ = fs::create_dir_all(&d);
    Some(d)
}

fn jetzt_s() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Energie wie aus dem Menue setzen (Haken wandert mit) — fuer den Fokusmodus.
#[tauri::command]
fn noki_energie(app: tauri::AppHandle, stufe: String) {
    // Alte Stufe "still" (z. B. aus einem gespeicherten Fokus-Profil) ist jetzt Stromsparend.
    let stufe = if stufe == "still" { "sparend".to_string() } else { stufe };
    if !["sparend", "normal", "energisch"].contains(&stufe.as_str()) {
        return;
    }
    let _ = app.emit("noki://energie", serde_json::json!({ "stufe": stufe }));
    if let Some(m) = app.try_state::<EnergieMenue>() {
        let ziel = format!("energie_{}", stufe);
        for (k, item) in &m.0 {
            let _ = item.set_checked(*k == ziel);
        }
    }
}

// ---- Zwischenablage-Verlauf ---------------------------------------------
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct ClipEintrag {
    pub id: u64,
    pub art: String,
    pub text: String,
    #[serde(default)]
    pub titel: String,
    #[serde(default)]
    pub bild: String,
    #[serde(default)]
    pub hash: String,
    pub zeit: u64,
    #[serde(default)]
    pub lang: usize,
}
pub struct Clip {
    pub liste: Mutex<Vec<ClipEintrag>>,
    pub eigen: std::sync::atomic::AtomicI64,
}
const CLIP_MAX: usize = 20;
const CLIP_TEXT_MAX: usize = 100_000;

pub fn clip_ist_link(t: &str) -> bool {
    let s = t.trim();
    (s.starts_with("http://") || s.starts_with("https://"))
        && s.len() > 10
        && !s.contains(char::is_whitespace)
}

/// Vorn einordnen: unmittelbar gleicher Inhalt wird nicht doppelt gespeichert,
/// ueber CLIP_MAX faellt der aelteste heraus. (aufgenommen?, herausgefallen)
pub fn clip_einordnen(l: &mut Vec<ClipEintrag>, e: ClipEintrag) -> (bool, Vec<ClipEintrag>) {
    if let Some(f) = l.first() {
        let gleich = if e.art == "bild" {
            f.art == "bild" && f.hash == e.hash
        } else {
            f.art != "bild" && f.text == e.text
        };
        if gleich {
            return (false, Vec::new());
        }
    }
    l.insert(0, e);
    let mut weg = Vec::new();
    while l.len() > CLIP_MAX {
        if let Some(x) = l.pop() {
            weg.push(x);
        }
    }
    (true, weg)
}

fn clip_datei(app: &tauri::AppHandle) -> Option<PathBuf> {
    Some(werk_dir(app)?.join("zwischenablage.json"))
}
fn clip_bilddir(app: &tauri::AppHandle) -> Option<PathBuf> {
    let d = werk_dir(app)?.join("zwischenablage");
    let _ = fs::create_dir_all(&d);
    Some(d)
}
fn clip_speichern(app: &tauri::AppHandle, l: &[ClipEintrag]) {
    if let (Some(d), Ok(s)) = (clip_datei(app), serde_json::to_string(l)) {
        let _ = fs::write(d, s);
    }
}
fn clip_laden(app: &tauri::AppHandle) -> Vec<ClipEintrag> {
    clip_datei(app)
        .and_then(|d| fs::read_to_string(d).ok())
        .and_then(|r| serde_json::from_str(&r).ok())
        .unwrap_or_default()
}
fn clip_dateien_weg(e: &ClipEintrag) {
    if e.art == "bild" && !e.bild.is_empty() {
        let _ = fs::remove_file(&e.bild);
        let _ = fs::remove_file(e.bild.replace(".png", "_t.png"));
    }
}
pub fn b64(d: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity((d.len() + 2) / 3 * 4);
    for c in d.chunks(3) {
        let n = ((c[0] as u32) << 16)
            | ((*c.get(1).unwrap_or(&0) as u32) << 8)
            | (*c.get(2).unwrap_or(&0) as u32);
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        s.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    s
}
fn fnv(d: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in d {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}{}", h, d.len())
}

/// Fuer die Oberflaeche: Text nur als Vorschau, Bild als kleines Thumbnail.
fn clip_fuer_ui(l: &[ClipEintrag]) -> Vec<serde_json::Value> {
    l.iter().map(|e| {
        let vorschau: String = e.text.chars().take(400).collect();
        let thumb = if e.art == "bild" {
            fs::read(e.bild.replace(".png", "_t.png")).map(|d| format!("data:image/png;base64,{}", b64(&d))).unwrap_or_default()
        } else { String::new() };
        serde_json::json!({ "id": e.id, "art": e.art, "text": vorschau, "titel": e.titel, "zeit": e.zeit, "lang": e.lang, "thumb": thumb })
    }).collect()
}
fn clip_melden(app: &tauri::AppHandle) {
    let l = app.state::<Clip>().liste.lock().unwrap().clone();
    let _ = app.emit(
        "noki://clip",
        serde_json::json!({ "liste": clip_fuer_ui(&l) }),
    );
}

fn clip_aufnehmen(app: &tauri::AppHandle, inh: lesezeichen::PbInhalt) {
    let st = app.state::<Clip>();
    let id = st
        .liste
        .lock()
        .unwrap()
        .iter()
        .map(|e| e.id)
        .max()
        .unwrap_or(0)
        + 1;
    let e = match inh {
        lesezeichen::PbInhalt::Text(t, titel) => {
            if t.trim().is_empty() {
                return;
            }
            let lang = t.chars().count();
            let t: String = if lang > CLIP_TEXT_MAX {
                t.chars().take(CLIP_TEXT_MAX).collect()
            } else {
                t
            };
            let art = if clip_ist_link(&t) { "link" } else { "text" };
            ClipEintrag {
                id,
                art: art.into(),
                text: t,
                titel,
                bild: String::new(),
                hash: String::new(),
                zeit: jetzt_s(),
                lang,
            }
        }
        lesezeichen::PbInhalt::Bild(daten) => {
            let hash = fnv(&daten);
            if st
                .liste
                .lock()
                .unwrap()
                .first()
                .map_or(false, |f| f.art == "bild" && f.hash == hash)
            {
                return;
            }
            let dir = match clip_bilddir(app) {
                Some(d) => d,
                None => return,
            };
            let roh = dir.join(format!("{id}.roh"));
            let png = dir.join(format!("{id}.png"));
            let th = dir.join(format!("{id}_t.png"));
            if fs::write(&roh, &daten).is_err() {
                return;
            }
            let _ = std::process::Command::new("/usr/bin/sips")
                .args(["-s", "format", "png"])
                .arg(&roh)
                .arg("--out")
                .arg(&png)
                .output();
            let _ = fs::remove_file(&roh);
            if !png.exists() {
                return;
            }
            let _ = std::process::Command::new("/usr/bin/sips")
                .args(["-Z", "160"])
                .arg(&png)
                .arg("--out")
                .arg(&th)
                .output();
            ClipEintrag {
                id,
                art: "bild".into(),
                text: String::new(),
                titel: String::new(),
                bild: png.to_string_lossy().into_owned(),
                hash,
                zeit: jetzt_s(),
                lang: 0,
            }
        }
    };
    let (neu, weg) = {
        let mut l = st.liste.lock().unwrap();
        let r = clip_einordnen(&mut l, e.clone());
        if r.0 {
            clip_speichern(app, &l);
        }
        r
    };
    if !neu {
        clip_dateien_weg(&e);
        return;
    }
    for x in &weg {
        clip_dateien_weg(x);
    }
    clip_melden(app);
}

/// Beobachtet die Zwischenablage (Aenderungszaehler, 2x je Sekunde). Nur lokal.
/// Als geheim/fluechtig markierte Eintraege (Passwort-Manager) und Dateien
/// werden nicht gemerkt; was Noki selbst hineinlegt, ebenfalls nicht.
fn clip_waechter(app: tauri::AppHandle) {
    thread::spawn(move || {
        let mut letzter = lesezeichen::pb_zaehler();
        loop {
            thread::sleep(Duration::from_millis(500));
            let z = lesezeichen::pb_zaehler();
            if z == letzter {
                continue;
            }
            letzter = z;
            if app.state::<Clip>().eigen.load(Ordering::Relaxed) == z {
                continue;
            }
            if let Some(inh) = lesezeichen::pb_lesen() {
                clip_aufnehmen(&app, inh);
            }
        }
    });
}

#[tauri::command]
fn clip_liste(app: tauri::AppHandle) -> Vec<serde_json::Value> {
    let l = app.state::<Clip>().liste.lock().unwrap().clone();
    clip_fuer_ui(&l)
}

#[tauri::command]
fn clip_voll(app: tauri::AppHandle, id: u64) -> String {
    app.state::<Clip>()
        .liste
        .lock()
        .unwrap()
        .iter()
        .find(|e| e.id == id)
        .map(|e| e.text.clone())
        .unwrap_or_default()
}

/// Nur auf Klick: Eintrag wieder in die Zwischenablage legen (und nach vorn holen).
#[tauri::command]
fn clip_kopieren(app: tauri::AppHandle, id: u64) -> bool {
    let st = app.state::<Clip>();
    let e = {
        let mut l = st.liste.lock().unwrap();
        let i = match l.iter().position(|e| e.id == id) {
            Some(i) => i,
            None => return false,
        };
        let e = l.remove(i);
        l.insert(0, e.clone());
        clip_speichern(&app, &l);
        e
    };
    let ok = if e.art == "bild" {
        lesezeichen::pb_bild_setzen(&e.bild)
    } else {
        lesezeichen::pb_text_setzen(&e.text)
    };
    st.eigen.store(lesezeichen::pb_zaehler(), Ordering::Relaxed);
    clip_melden(&app);
    ok
}

/// Nur auf Klick, nur http(s): Link im Standardbrowser oeffnen.
#[tauri::command]
fn clip_link_oeffnen(app: tauri::AppHandle, id: u64) -> bool {
    let u = app
        .state::<Clip>()
        .liste
        .lock()
        .unwrap()
        .iter()
        .find(|e| e.id == id && e.art == "link")
        .map(|e| e.text.trim().to_string());
    match u {
        Some(u) if clip_ist_link(&u) => std::process::Command::new("/usr/bin/open")
            .arg(&u)
            .spawn()
            .is_ok(),
        _ => false,
    }
}

#[tauri::command]
fn clip_entfernen(app: tauri::AppHandle, id: u64) {
    {
        let st = app.state::<Clip>();
        let mut l = st.liste.lock().unwrap();
        if let Some(i) = l.iter().position(|e| e.id == id) {
            let e = l.remove(i);
            clip_dateien_weg(&e);
        }
        clip_speichern(&app, &l);
    }
    clip_melden(&app);
}

#[tauri::command]
fn clip_leeren(app: tauri::AppHandle) {
    {
        let st = app.state::<Clip>();
        let mut l = st.liste.lock().unwrap();
        for e in l.iter() {
            clip_dateien_weg(e);
        }
        l.clear();
        clip_speichern(&app, &l);
    }
    clip_melden(&app);
}

// ---- Arbeitsplatz-Profile -------------------------------------------------
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct PlatzApp {
    pub name: String,
    pub bundle: String,
    pub pfad: String,
    pub fenster: Vec<[f64; 4]>,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct PlatzProfil {
    pub apps: Vec<PlatzApp>,
    pub zeit: u64,
}
const PROFILE: [&str; 3] = ["Uni", "Coding", "Normal"];

fn platz_datei(app: &tauri::AppHandle) -> Option<PathBuf> {
    Some(werk_dir(app)?.join("arbeitsplatz.json"))
}
fn platz_alle(app: &tauri::AppHandle) -> std::collections::HashMap<String, PlatzProfil> {
    platz_datei(app)
        .and_then(|d| fs::read_to_string(d).ok())
        .and_then(|r| serde_json::from_str(&r).ok())
        .unwrap_or_default()
}

#[tauri::command]
fn platz_liste(app: tauri::AppHandle) -> serde_json::Value {
    let a = platz_alle(&app);
    let profile: Vec<serde_json::Value> = PROFILE.iter().map(|n| {
        let p = a.get(*n);
        serde_json::json!({ "name": n,
            "apps": p.map(|p| p.apps.iter().map(|x| x.name.clone()).collect::<Vec<_>>()).unwrap_or_default(),
            "pfade": p.map(|p| p.apps.iter().map(|x| x.pfad.clone()).collect::<Vec<_>>()).unwrap_or_default(),
            "zeit": p.map(|p| p.zeit).unwrap_or(0) })
    }).collect();
    serde_json::json!({ "ax": lesezeichen::ax_vertraut(false), "profile": profile })
}

/// Speichert die offenen Apps und ihre Fenster (Position/Groesse, globale
/// Koordinaten = Display). Fensterlagen brauchen die Freigabe Bedienungshilfen;
/// ohne sie wird nur die App-Liste gemerkt. Keine Inhalte, nichts wird beendet.
#[tauri::command]
fn platz_speichern(app: tauri::AppHandle, profil: String) -> serde_json::Value {
    if !PROFILE.contains(&profil.as_str()) {
        return serde_json::json!({ "ok": false });
    }
    let mut ax = lesezeichen::ax_vertraut(false);
    if !ax {
        ax = lesezeichen::ax_vertraut(true);
    } // einmalig nachfragen — auf ausdruecklichen Klick
    let apps: Vec<PlatzApp> = lesezeichen::apps_mit_fenstern(ax)
        .into_iter()
        .map(|(name, bundle, pfad, fenster)| PlatzApp {
            name,
            bundle,
            pfad,
            fenster,
        })
        .collect();
    let n = apps.len();
    let f: usize = apps.iter().map(|a| a.fenster.len()).sum();
    let mut alle = platz_alle(&app);
    alle.insert(
        profil,
        PlatzProfil {
            apps,
            zeit: jetzt_s(),
        },
    );
    let ok = match (platz_datei(&app), serde_json::to_string_pretty(&alle)) {
        (Some(d), Ok(s)) => fs::write(d, s).is_ok(),
        _ => false,
    };
    serde_json::json!({ "ok": ok, "apps": n, "fenster": f, "ax": ax })
}

/// Profil laden: fehlende Apps oeffnen (im Hintergrund), offene weiterverwenden
/// (keine Duplikatfenster), Fenster anordnen. Meldet das Ergebnis per Event.
/// Icons der gewaehlten Fokus-Apps (klein, PNG-Daten-URL), auf Abruf.
/// Off the main thread: rendering up to 60 icons (NSWorkspace + PNG) used to
/// run synchronously there and froze every Noki window while the Focus
/// app picker opened.
#[tauri::command]
async fn app_icons(pfade: Vec<String>) -> std::collections::HashMap<String, String> {
    tauri::async_runtime::spawn_blocking(move || app_icons_laden(pfade)).await.unwrap_or_default()
}
fn app_icons_laden(pfade: Vec<String>) -> std::collections::HashMap<String, String> {
    let _busy = UiBusy::neu("app_icons");
    static CACHE: Mutex<Option<std::collections::HashMap<String, String>>> = Mutex::new(None);
    let mut out = std::collections::HashMap::new();
    for p in pfade.into_iter().take(60) {
        let hit = CACHE.lock().ok().and_then(|g| g.as_ref().and_then(|m| m.get(&p).cloned()));
        let v = hit.unwrap_or_else(|| {
            let v = window_overview::datei_icon(&p);
            if let Ok(mut g) = CACHE.lock() { g.get_or_insert_with(Default::default).insert(p.clone(), v.clone()); }
            v
        });
        out.insert(p, v);
    }
    out
}

/// Arbeitsplatz-Fokus Start: eine Fenster-Sitzung auf dem AKTUELLEN
/// Schreibtisch (siehe fokus_sitzung.rs). `apps` = gewaehlte App-Bundles.
#[tauri::command]
async fn fokus_sitzung_start(app: tauri::AppHandle, apps: Vec<String>, prior_energy: String, profil: Option<String>) -> serde_json::Value {
    #[cfg(target_os = "macos")]
    {
        fokus_sitzung::profil_merken(profil.as_deref().unwrap_or(""));
        let r = tauri::async_runtime::spawn_blocking(move || {
            // Native full screen: the session never starts inside it. First
            // the nearest normal Desktop becomes active (verified arrival),
            // then the unchanged start runs there - that Desktop is the
            // session's focusSpace for start AND Beenden.
            let space = match vollbild_zum_schreibtisch() {
                Ok(s) => s,
                Err(e) => return serde_json::json!({ "ok": false, "error": e }),
            };
            fokus_sitzung::starten(&apps, &prior_energy, space, || cgs::aktiver_space().map(|(s, _)| s).unwrap_or(0))
        }).await.unwrap_or_default();
        noki_vorn_sichern_app(&app);
        r
    }
    #[cfg(not(target_os = "macos"))]
    { let _ = (app, apps, prior_energy, profil); serde_json::json!({}) }
}

/// The Space the focus session starts on. A normal Desktop: itself. Native
/// full screen (type 4): the nearest normal Desktop in Mission-Control order
/// - to the right first, else to the left - never Noki's own reserved
/// Desktop while another exists. The switch is the existing visible
/// navigation (`sichtbar_zum_space`: one Ctrl+Arrow step at a time, each
/// arrival read back from WindowServer); it returns only when the target
/// is really active, so nothing of the session runs before that.
#[cfg(target_os = "macos")]
fn vollbild_zum_schreibtisch() -> Result<u64, String> {
    let (aktiv, typ) = cgs::aktiver_space().ok_or("Aktiver Schreibtisch nicht lesbar")?;
    if typ != 4 {
        return Ok(aktiv);
    }
    let ordnung = cgs::space_reihenfolge().ok_or("Schreibtisch-Reihenfolge nicht lesbar")?;
    let i = ordnung.iter().position(|s| *s == aktiv).ok_or("Vollbild-Space steht nicht in der Reihenfolge")?;
    let reserviert = ARBEITSPLATZ.lock().ok().and_then(|g| g.as_ref().map(|r| r.id));
    let normal = |s: &u64| cgs::space_typ(*s) == Some(arbeitsplatz::TYP_SCHREIBTISCH);
    let suche = |ohne_reserviert: bool| {
        let ok = |s: &&u64| normal(s) && !(ohne_reserviert && Some(**s) == reserviert);
        ordnung[i + 1..].iter().find(ok).or_else(|| ordnung[..i].iter().rev().find(ok)).copied()
    };
    let ziel = suche(true).or_else(|| suche(false)).ok_or("Kein normaler Schreibtisch vorhanden")?;
    virtual_workspace::trace(&format!("FOCUS_FULLSCREEN_EXIT from={aktiv} to={ziel}"));
    cgs::sichtbar_zum_space(ziel)?;
    let jetzt = cgs::aktiver_space().map(|(s, _)| s).unwrap_or(0);
    if jetzt != ziel {
        return Err(format!("Schreibtisch {ziel} nicht erreicht (aktuell {jetzt})"));
    }
    Ok(ziel)
}

/// Trockenlauf des Fokus-Starts: nur Bericht, kein Fenster wird beruehrt.
#[tauri::command]
async fn fokus_sitzung_pruefen(apps: Vec<String>) -> serde_json::Value {
    #[cfg(target_os = "macos")]
    { tauri::async_runtime::spawn_blocking(move || fokus_sitzung::pruefen(&apps)).await.unwrap_or_default() }
    #[cfg(not(target_os = "macos"))]
    { let _ = apps; serde_json::json!({}) }
}

/// Laeuft eine Fokus-Sitzung (auch eine, die ein vorheriger Noki-Prozess
/// gespeichert hat)? Das Frontend fragt beim Laden - Beenden bleibt so nach
/// einem Neustart moeglich.
#[tauri::command]
async fn fokus_sitzung_status() -> serde_json::Value {
    #[cfg(target_os = "macos")]
    { tauri::async_runtime::spawn_blocking(fokus_sitzung::status).await.unwrap_or_default() }
    #[cfg(not(target_os = "macos"))]
    { serde_json::json!({ "aktiv": false }) }
}

/// Arbeitsplatz-Fokus Ende: nur Sitzungsfenster schliessen, nur selbst
/// minimierte Fenster zurueck.
#[tauri::command]
async fn fokus_sitzung_ende(app: tauri::AppHandle) -> serde_json::Value {
    #[cfg(target_os = "macos")]
    {
        let r = tauri::async_runtime::spawn_blocking(fokus_sitzung::beenden).await.unwrap_or_default();
        noki_vorn_sichern_app(&app);
        r
    }
    #[cfg(not(target_os = "macos"))]
    { let _ = app; serde_json::json!({}) }
}

#[tauri::command]
fn platz_laden(app: tauri::AppHandle, profil: String) {
    if !PROFILE.contains(&profil.as_str()) {
        return;
    }
    thread::spawn(move || {
        let prof = platz_alle(&app).get(&profil).cloned().unwrap_or_default();
        let ax = lesezeichen::ax_vertraut(false);
        let (mut offen, mut fehlt, mut gesetzt) = (Vec::new(), Vec::new(), 0usize);
        for a in &prof.apps {
            let mut pid = if a.bundle.is_empty() {
                None
            } else {
                lesezeichen::pid_fuer_bundle(&a.bundle)
            };
            if pid.is_none() {
                let per_id = !a.bundle.is_empty()
                    && std::process::Command::new("/usr/bin/open")
                        .args(["-g", "-b", &a.bundle])
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false);
                let per_pfad = !per_id
                    && !a.pfad.is_empty()
                    && std::path::Path::new(&a.pfad).exists()
                    && std::process::Command::new("/usr/bin/open")
                        .args(["-g", "-a", &a.pfad])
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false);
                if !per_id && !per_pfad {
                    fehlt.push(a.name.clone());
                    continue;
                }
                offen.push(a.name.clone());
                for _ in 0..30 {
                    thread::sleep(Duration::from_millis(200));
                    pid = lesezeichen::pid_fuer_bundle(&a.bundle);
                    if pid.is_some() {
                        break;
                    }
                }
            }
            if let (Some(p), true) = (pid, ax) {
                if !a.fenster.is_empty() {
                    gesetzt += lesezeichen::fenster_setzen(p, &a.fenster, 4000);
                }
            }
        }
        let _ = app.emit(
            "noki://platz",
            serde_json::json!({ "profil": profil, "geoeffnet": offen, "fehlt": fehlt,
            "gesetzt": gesetzt, "ax": ax, "leer": prof.apps.is_empty() }),
        );
    });
}

// ---- Fokusmodus -------------------------------------------------------------
fn app_plist_xml(app_path: &std::path::Path) -> String {
    let plist = app_path.join("Contents/Info.plist");
    if let Ok(xml) = fs::read_to_string(&plist) {
        if xml.contains("<plist") { return xml; }
    }
    std::process::Command::new("/usr/bin/plutil")
        .args(["-convert", "xml1", "-o", "-"])
        .arg(plist)
        .output().ok().filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

fn app_plist_string(xml: &str, key: &str) -> Option<String> {
    let marker = format!("<key>{key}</key>");
    let after = xml.split(&marker).nth(1)?;
    let value = after.split("<string>").nth(1)?.split("</string>").next()?.trim();
    (!value.is_empty() && !value.contains("$(")).then(|| value.to_owned())
}

fn app_plist_bool(xml: &str, key: &str) -> bool {
    let marker = format!("<key>{key}</key>");
    xml.split(&marker).nth(1).is_some_and(|s| {
        let s = s.trim_start();
        s.starts_with("<true/>") || s.starts_with("<true />")
    })
}

fn app_plist_aliases(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    // The picker promises name/display-name search. Bundle identifiers are
    // implementation metadata and produce surprising matches ("chr" also
    // matched unrelated apps through reverse-DNS vendor strings).
    for key in ["CFBundleDisplayName", "CFBundleName"] {
        if let Some(value) = app_plist_string(xml, key) { out.push(value); }
    }
    out.sort();
    out.dedup();
    out
}

/// Installierte Apps (Programme-Ordner), keine feste Liste.
/// The names macOS shows for an app in the user's languages ("Rechner" for
/// Calculator.app, "Erinnerungen" for Reminders.app). Bundle metadata only
/// has the English base name, so "Öffne Rechner" matched nothing and the
/// resolver asked "Meinst du Reminders oder Print Center?". Read from the
/// app's own localization table (`InfoPlist.loctable`, or the classic
/// `<lang>.lproj/InfoPlist.strings`) for the user's preferred languages -
/// independent of which languages Noki's own bundle declares.
#[cfg(target_os = "macos")]
fn app_anzeigename(pfad: &str) -> Vec<String> {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};
    unsafe fn ns(s: &str) -> *mut AnyObject {
        let c = std::ffi::CString::new(s).unwrap_or_default();
        match AnyClass::get(c"NSString") { Some(k) => msg_send![k, stringWithUTF8String: c.as_ptr()], None => std::ptr::null_mut() }
    }
    unsafe fn text(o: *mut AnyObject) -> Option<String> {
        if o.is_null() { return None; }
        let ist_string: bool = msg_send![o, isKindOfClass: AnyClass::get(c"NSString")?];
        if !ist_string { return None; }
        let u: *const std::os::raw::c_char = msg_send![o, UTF8String];
        (!u.is_null()).then(|| std::ffi::CStr::from_ptr(u).to_string_lossy().into_owned())
    }
    unsafe fn dict(datei: &str) -> *mut AnyObject {
        if !std::path::Path::new(datei).exists() { return std::ptr::null_mut(); }
        match AnyClass::get(c"NSDictionary") { Some(k) => msg_send![k, dictionaryWithContentsOfFile: ns(datei)], None => std::ptr::null_mut() }
    }
    unsafe fn wert(d: *mut AnyObject, key: &str) -> *mut AnyObject {
        if d.is_null() { return std::ptr::null_mut(); }
        let ist_dict: bool = match AnyClass::get(c"NSDictionary") { Some(k) => msg_send![d, isKindOfClass: k], None => false };
        if !ist_dict { return std::ptr::null_mut(); }
        msg_send![d, objectForKey: ns(key)]
    }
    let mut out: Vec<String> = Vec::new();
    unsafe {
        // User's preferred languages ("de-DE" -> "de"), at most three.
        let mut sprachen: Vec<String> = Vec::new();
        if let Some(k) = AnyClass::get(c"NSLocale") {
            let arr: *mut AnyObject = msg_send![k, preferredLanguages];
            if !arr.is_null() {
                let n: usize = msg_send![arr, count];
                for i in 0..n.min(3) {
                    let o: *mut AnyObject = msg_send![arr, objectAtIndex: i];
                    if let Some(t) = text(o) {
                        let code = t.split('-').next().unwrap_or("").to_string();
                        if !code.is_empty() && !sprachen.contains(&code) { sprachen.push(code); }
                    }
                }
            }
        }
        let res = format!("{pfad}/Contents/Resources");
        let tabelle = dict(&format!("{res}/InfoPlist.loctable"));
        for sprache in &sprachen {
            for quelle in [wert(tabelle, sprache), dict(&format!("{res}/{sprache}.lproj/InfoPlist.strings"))] {
                for key in ["CFBundleDisplayName", "CFBundleName"] {
                    if let Some(n) = text(wert(quelle, key)) {
                        let n = n.trim().to_string();
                        if !n.is_empty() && !out.contains(&n) { out.push(n); }
                    }
                }
            }
        }
    }
    out
}
#[cfg(not(target_os = "macos"))]
fn app_anzeigename(_pfad: &str) -> Vec<String> { Vec::new() }

/// Async: a sync command runs on the MAIN thread, and the first scan reads
/// every app bundle's plist - seconds in which no Noki window (Settings ›
/// Arbeitsplatz-Fokus included) could take a click.
#[tauri::command]
async fn fokus_apps() -> Vec<serde_json::Value> {
    tauri::async_runtime::spawn_blocking(fokus_apps_laden).await.unwrap_or_default()
}
fn fokus_apps_laden() -> Vec<serde_json::Value> {
    static CACHE: OnceLock<Mutex<Option<(std::time::Instant, Vec<serde_json::Value>)>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    if let Some((at, apps)) = cache.lock().ok().and_then(|g| g.clone()) {
        if at.elapsed() < Duration::from_secs(60) {
            return apps;
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let eigene = format!("{home}/Applications");
    let dirs = [
        "/Applications",
        "/Applications/Utilities",
        "/System/Applications",
        "/System/Applications/Utilities",
        "/System/Library/CoreServices",
        eigene.as_str(),
    ];
    let mut v: Vec<(String, String, Vec<String>)> = Vec::new();
    for d in dirs {
        if let Ok(rd) = fs::read_dir(d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().map_or(false, |x| x == "app") {
                    let datei_name = p
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let xml = app_plist_xml(&p);
                    let package = app_plist_string(&xml, "CFBundlePackageType").unwrap_or_default();
                    let executable = app_plist_string(&xml, "CFBundleExecutable").unwrap_or_default();
                    let background = app_plist_bool(&xml, "LSBackgroundOnly");
                    let ui_element = app_plist_bool(&xml, "LSUIElement");
                    let system_core = p.starts_with("/System/Library/CoreServices");
                    let bundle = app_plist_string(&xml, "CFBundleIdentifier").unwrap_or_default();
                    // Bundle metadata is the primary classification.  The
                    // CoreServices directory is a component repository, not
                    // an app catalogue; Finder is its one normal desktop app.
                    let user_facing_location = !system_core || bundle == "com.apple.finder";
                    let executable_exists = !executable.is_empty()
                        && p.join("Contents/MacOS").join(&executable).is_file();
                    let gui_package = package.is_empty() || package == "APPL"
                        || bundle == "com.apple.finder";
                    if background || ui_element || !user_facing_location || !executable_exists
                        || !gui_package { continue; }
                    let mut aliases = app_plist_aliases(&xml);
                    for lokal in app_anzeigename(&p.to_string_lossy()) {
                        if !aliases.iter().any(|a| a.eq_ignore_ascii_case(&lokal)) { aliases.push(lokal); }
                    }
                    let n = app_plist_string(&xml, "CFBundleDisplayName")
                        .or_else(|| app_plist_string(&xml, "CFBundleName"))
                        .unwrap_or(datei_name);
                    let classifier = format!("{} {} {}", n, executable, bundle).to_lowercase();
                    let invisible_name = [" agent", "agent ", "helper", "uiagent", "xpc", "updater"]
                        .iter().any(|token| classifier.contains(token));
                    if invisible_name { continue; }
                    v.push((n, p.to_string_lossy().into_owned(), aliases));
                }
            }
        }
    }
    v.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    v.dedup_by(|a, b| a.0 == b.0);
    let apps: Vec<_> = v
        .into_iter()
        .map(|(n, p, aliases)| serde_json::json!({ "name": n, "pfad": p, "aliases": aliases }))
        .collect();
    if let Ok(mut guard) = cache.lock() {
        *guard = Some((std::time::Instant::now(), apps.clone()));
    }
    apps
}

/// Oeffnet ausgewaehlte .app-Pakete (nur echte Programme). Rueckgabe: nicht gefunden.
#[tauri::command]
fn apps_oeffnen(pfade: Vec<String>) -> Vec<String> {
    let mut fehlt = Vec::new();
    for p in pfade {
        if !p.ends_with(".app")
            || !std::path::Path::new(&p).exists()
            || std::process::Command::new("/usr/bin/open")
                .args(["-a", &p])
                .spawn()
                .is_err()
        {
            fehlt.push(p);
        }
    }
    fehlt
}

/// Opens a resolved local resource without granting read access.
#[tauri::command]
fn lokale_ressource_oeffnen(
    app: tauri::AppHandle,
    kind: String,
    pfad: String,
) -> Result<serde_json::Value, String> {
    if !matches!(kind.as_str(), "app.open" | "file.open" | "directory.open") {
        return Err("Unbekannter lokaler Open-Typ.".into());
    }
    let resolved =
        fs::canonicalize(&pfad).map_err(|_| "Lokales Ziel wurde nicht gefunden.".to_string())?;
    if code_agent::is_sensitive_path(&resolved) {
        return Err("Geschütztes lokales Ziel darf nicht geöffnet werden.".into());
    }
    if kind == "app.open" && resolved.extension().and_then(|x| x.to_str()) != Some("app") {
        return Err("Das lokale Ziel ist keine installierte App.".into());
    }
    // Waehrend einer Arbeitsplatz-Aufgabe geht auch dieser Weg ueber Nokis
    // Schreibtisch. Sonst holte der Standard-Oeffner (bei Quelltext etwa
    // Visual Studio Code) sein Fenster auf den Schreibtisch des Nutzers.
    if arbeitsplatz_aufgabe_laeuft() {
        let ziel = resolved.to_string_lossy().into_owned();
        let r = arbeitsplatz_oeffnen_blockierend(app, ziel, "".into(), None);
        if r.get("ok").and_then(|b| b.as_bool()) == Some(true) {
            return Ok(serde_json::json!({
                "opened": true, "kind": kind, "path": resolved, "wo": "arbeitsplatz"
            }));
        }
        return Err(r
            .get("grund")
            .and_then(|g| g.as_str())
            .unwrap_or("Lokales Ziel konnte auf Nokis Schreibtisch nicht geöffnet werden.")
            .to_owned());
    }
    if !std::process::Command::new("/usr/bin/open")
        .arg(&resolved)
        .args(vscode_bruecke::open_zusatz(&resolved.to_string_lossy()))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        return Err("Lokales Ziel konnte nicht geöffnet werden.".into());
    }
    Ok(serde_json::json!({"opened": true, "kind": kind, "path": resolved}))
}

fn browser_such_url(query: &str) -> Result<String, String> {
    let query = query.trim();
    if query.is_empty() || query.chars().count() > 500 || query.chars().any(char::is_control) {
        return Err("Ungültige Browser-Suche.".into());
    }
    let encoded = query
        .as_bytes()
        .iter()
        .map(|b| match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (*b as char).to_string()
            }
            b' ' => "+".into(),
            x => format!("%{x:02X}"),
        })
        .collect::<String>();
    Ok(format!("https://duckduckgo.com/?q={encoded}"))
}

/// Uses the user's default browser. The query is encoded locally and no page content is read.
#[tauri::command]
fn browser_suche(app: tauri::AppHandle, query: String) -> Result<serde_json::Value, String> {
    let url = browser_such_url(&query)?;
    // Laeuft eine Aufgabe auf Nokis Arbeitsplatz, entsteht das Fenster dort
    // und nicht beim Nutzer. Derselbe geprueft native Weg wie browser.search.
    if arbeitsplatz_aufgabe_laeuft() {
        let r = arbeitsplatz_suchen_blockierend(app, query.clone());
        if r.get("ok").and_then(|b| b.as_bool()) == Some(true) {
            return Ok(serde_json::json!({
                "opened": true, "query": query, "verified": true, "wo": "arbeitsplatz"
            }));
        }
    }
    let ok = std::process::Command::new("/usr/bin/open")
        .arg(&url)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        return Err("Browser-Suche konnte nicht geöffnet werden.".into());
    }
    Ok(serde_json::json!({"opened": true, "query": query, "verified": true}))
}

/// Oeffnet einen App-Deep-Link (z. B. `spotify:search:...`). Desktop zuerst:
/// das ist der Weg, auf dem die installierte App die Bitte KOMPLETT bekommt,
/// statt nur zu starten. Nur Schemata aus der Adapter-Registry sind erlaubt.
#[tauri::command]
fn app_uri_oeffnen(app: tauri::AppHandle, uri: String) -> Result<serde_json::Value, String> {
    let uri = uri.trim();
    if uri.chars().count() > 2000 || !capability::is_known_app_uri(uri) {
        return Err("Dieser App-Link steht nicht in der Noki-Zielliste.".into());
    }
    // Waehrend einer Arbeitsplatz-Aufgabe nie ueber den aktivierenden Weg:
    // `activate` und `open -a` holen das Programm auf den Schreibtisch des
    // Nutzers. Der Arbeitsplatz-Oeffner prueft dasselbe Ziel gegen dieselbe
    // Zielliste und laesst das Fenster auf Nokis Schreibtisch entstehen.
    if arbeitsplatz_aufgabe_laeuft() {
        let r = arbeitsplatz_oeffnen_blockierend(app, uri.to_owned(), "".into(), None);
        if r.get("ok").and_then(|b| b.as_bool()) == Some(true) {
            return Ok(serde_json::json!({"opened": true, "uri": uri, "wo": "arbeitsplatz"}));
        }
        return Err(r
            .get("grund")
            .and_then(|g| g.as_str())
            .unwrap_or("Der App-Link konnte auf Nokis Schreibtisch nicht geöffnet werden.")
            .to_owned());
    }
    let ok = if uri.starts_with("spotify:") {
        #[cfg(target_os = "macos")]
        {
            let script = format!(
                "tell application \"Spotify\"\nactivate\nopen location \"{}\"\nend tell",
                uri.replace('"', "\\\"")
            );
            let osascript_ok = std::process::Command::new("osascript")
                .arg("-e")
                .arg(&script)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if osascript_ok {
                true
            } else {
                std::process::Command::new("/usr/bin/open")
                    .arg("-a")
                    .arg("Spotify")
                    .arg(uri)
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false)
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            std::process::Command::new("/usr/bin/open")
                .arg(uri)
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
    } else {
        std::process::Command::new("/usr/bin/open")
            .arg(uri)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    if !ok {
        return Err("Die App konnte den Link nicht öffnen.".into());
    }
    Ok(serde_json::json!({"opened": true, "uri": uri}))
}

/// Die erlaubten Hauptorte im Noki-Dateibrowser (Desktop, Dokumente, Downloads, Bilder, Musik, Noki).
/// Weder $HOME als eigene Kachel, noch / oder andere Systemordner.
fn erlaubte_browser_orte() -> Vec<(&'static str, PathBuf)> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut res = Vec::new();
    let orte = [
        ("Desktop", "Desktop"),
        ("Dokumente", "Documents"),
        ("Downloads", "Downloads"),
        ("Bilder", "Pictures"),
        ("Musik", "Music"),
    ];
    for (label, sub) in &orte {
        let p = std::path::Path::new(&home).join(sub);
        if p.is_dir() {
            let canon = fs::canonicalize(&p).unwrap_or(p);
            res.push((*label, canon));
        }
    }
    // Den tatsaechlich vorhandenen Noki-Ordner direkt unter ~/Documents
    // verwenden. Kein synthetischer Pfad wird angezeigt oder freigeschaltet.
    let documents = std::path::Path::new(&home).join("Documents");
    let noki_p = fs::read_dir(&documents).ok().and_then(|eintraege| {
        eintraege.flatten().map(|e| e.path()).find(|p| {
            p.is_dir()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.eq_ignore_ascii_case("noki"))
                    .unwrap_or(false)
        })
    });
    if let Some(noki_p) = noki_p {
        let canon = fs::canonicalize(&noki_p).unwrap_or(noki_p);
        res.push(("Noki", canon));
    }
    res
}

fn ist_in_erlaubtem_browser_ort(p: &Path) -> bool {
    let roots = erlaubte_browser_orte();
    roots.iter().any(|(_, r)| p.starts_with(r))
}

#[tauri::command]
fn browser_debug_log(msg: String) {
    browser_log(&msg);
}

fn photos_helper_binary() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let bundled = dir.join("../Helpers/NokiPhotos.app/Contents/MacOS/noki-photos");
            if bundled.is_file() {
                return Some(bundled);
            }
            let dev = dir.join("../../target/photos-bin/NokiPhotos.app/Contents/MacOS/noki-photos");
            if dev.is_file() {
                return Some(dev);
            }
        }
    }
    let dev_app = PathBuf::from(
        "/Users/yilonglin/NOKI/desktop/target/photos-bin/NokiPhotos.app/Contents/MacOS/noki-photos",
    );
    if dev_app.is_file() {
        return Some(dev_app);
    }
    let local_app = PathBuf::from(
        "/Users/yilonglin/NOKI/.local/photos/NokiPhotos.app/Contents/MacOS/noki-photos",
    );
    if local_app.is_file() {
        return Some(local_app);
    }
    None
}

fn browser_log(msg: &str) {
    eprintln!("{msg}");
    if let Ok(mut f) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/Users/yilonglin/NOKI/desktop/noki_live.log")
    {
        use std::io::Write;
        let _ = writeln!(f, "{msg}");
    }
}

#[cfg(target_os = "macos")]
extern "C" {
    fn noki_photos_status() -> i32;
    fn noki_photos_request() -> i32;
    fn noki_photos_list(offset: i32, limit: i32) -> *mut std::ffi::c_char;
    fn noki_photos_thumb(
        asset_id: *const std::ffi::c_char,
        target_size: i32,
    ) -> *mut std::ffi::c_char;
    fn noki_photos_export(
        asset_id: *const std::ffi::c_char,
        target_path: *const std::ffi::c_char,
    ) -> i32;
    fn noki_photos_free_string(s: *mut std::ffi::c_char);
}

fn photos_adapter_list(
    offset: usize,
    limit: usize,
) -> (String, Option<String>, usize, Vec<serde_json::Value>) {
    #[cfg(target_os = "macos")]
    {
        let raw = unsafe { noki_photos_list(offset as i32, limit as i32) };
        if !raw.is_null() {
            let s = unsafe { std::ffi::CStr::from_ptr(raw) }.to_string_lossy();
            let parsed: serde_json::Value =
                serde_json::from_str(&s).unwrap_or(serde_json::json!({}));
            unsafe { noki_photos_free_string(raw) };
            let status = parsed["status"].as_str().unwrap_or("unknown").to_string();
            let hinweis = parsed["hinweis"].as_str().map(str::to_string);
            let assets = parsed["assets"].as_array().cloned().unwrap_or_default();
            let total = parsed["count"].as_u64().unwrap_or(assets.len() as u64) as usize;
            browser_log(&format!(
                "[PHOTOS_DEBUG] in_process=yes photos_auth_status={} photos_asset_count={} photos_offset={} photos_result_count={}",
                status, total, offset, assets.len()
            ));
            if status == "authorized" || status == "limited" || !assets.is_empty() {
                return (status, hinweis, total, assets);
            }
        }
    }

    let Some(helper) = photos_helper_binary() else {
        browser_log("[PHOTOS_DEBUG] photos_helper_path=none photos_helper_exists=no photos_helper_launch_ok=no photos_auth_status=unavailable photos_asset_count=0 photos_result_count=0 error=helper_not_found");
        return (
            "unavailable".into(),
            Some("Fotos-Adapter nicht verfügbar".into()),
            0,
            Vec::new(),
        );
    };
    let helper_path_str = helper.to_string_lossy().into_owned();
    let helper_exists = helper.is_file();
    let cmd_res = std::process::Command::new(&helper)
        .arg("list")
        .arg(offset.to_string())
        .arg(limit.to_string())
        .output();
    let Ok(out) = cmd_res else {
        browser_log(&format!("[PHOTOS_DEBUG] in_process=no photos_helper_path={} photos_helper_exists={} photos_helper_launch_ok=no photos_auth_status=error photos_asset_count=0 photos_result_count=0 error=spawn_failed", helper_path_str, if helper_exists { "yes" } else { "no" }));
        return (
            "error".into(),
            Some("Fotos-Adapter konnte nicht ausgeführt werden".into()),
            0,
            Vec::new(),
        );
    };
    let stderr_str = String::from_utf8_lossy(&out.stderr);
    if !stderr_str.is_empty() {
        browser_log(&format!(
            "[PHOTOS_DEBUG] helper_stderr: {}",
            stderr_str.trim()
        ));
    }
    let val: serde_json::Value =
        serde_json::from_slice(&out.stdout).unwrap_or(serde_json::json!({}));
    let status = val["status"].as_str().unwrap_or("unknown").to_string();
    let hinweis = val["hinweis"].as_str().map(str::to_string);
    let assets = val["assets"].as_array().cloned().unwrap_or_default();
    let asset_count = val["count"].as_u64().unwrap_or(assets.len() as u64);
    browser_log(&format!(
        "[PHOTOS_DEBUG] in_process=no photos_helper_path={} photos_helper_exists={} photos_helper_launch_ok={} photos_auth_status={} photos_asset_count={} photos_result_count={} error={}",
        helper_path_str,
        if helper_exists { "yes" } else { "no" },
        if out.status.success() { "yes" } else { "no" },
        status,
        asset_count,
        assets.len(),
        val["error"].as_str().unwrap_or("none")
    ));
    (status, hinweis, asset_count as usize, assets)
}

const GALERIE_SEITENGROESSE: usize = 40;

fn foto_eintrag(a: &serde_json::Value) -> Option<serde_json::Value> {
    let id = a["id"].as_str()?;
    Some(serde_json::json!({
        "name": a["name"].as_str().unwrap_or("Foto.jpg"),
        "pfad": format!("photos://asset/{id}"),
        "asset_id": id,
        "ordner": false,
        "paket": false,
        "groesse": 0,
        "mime": "image/jpeg",
        "bild": true,
        "quelle": "photos",
        "erstellt": a["created"].as_str().unwrap_or(""),
        "breite": a["width"].as_u64().unwrap_or(0),
        "hoehe": a["height"].as_u64().unwrap_or(0),
    }))
}

#[tauri::command]
async fn datei_galerie_seite(offset: usize) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (status, hinweis, total, assets) = photos_adapter_list(offset, GALERIE_SEITENGROESSE);
        let eintraege: Vec<_> = assets.iter().filter_map(foto_eintrag).collect();
        Ok(serde_json::json!({
            "photos_status": status,
            "photos_hinweis": hinweis,
            "photos_total": total,
            "photos_next_offset": offset + assets.len(),
            "eintraege": eintraege,
        }))
    })
    .await
    .map_err(|e| e.to_string())?
}

fn photos_adapter_thumb(asset_id: &str) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        if let Ok(c_id) = std::ffi::CString::new(asset_id) {
            let raw = unsafe { noki_photos_thumb(c_id.as_ptr(), 260) };
            if !raw.is_null() {
                let s = unsafe { std::ffi::CStr::from_ptr(raw) }
                    .to_string_lossy()
                    .into_owned();
                unsafe { noki_photos_free_string(raw) };
                if s.starts_with("data:image/") {
                    browser_log(&format!(
                        "[PHOTOS_DEBUG] in_process_thumb_success asset_id={} bytes={}",
                        asset_id,
                        s.len()
                    ));
                    return Ok(s);
                }
            }
        }
    }

    let helper =
        photos_helper_binary().ok_or_else(|| "Fotos-Adapter nicht verfügbar.".to_string())?;
    let out = std::process::Command::new(&helper)
        .arg("thumb")
        .arg(asset_id)
        .output()
        .map_err(|e| format!("Fotos-Adapter Fehler: {e}"))?;

    if !out.status.success() {
        let err_msg = String::from_utf8_lossy(&out.stderr);
        browser_log(&format!(
            "[PHOTOS_DEBUG] thumbnail_failed asset_id={} error={}",
            asset_id,
            err_msg.trim()
        ));
        return Err(format!("Thumbnail-Fehler: {err_msg}"));
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.starts_with("data:image/") {
        browser_log(&format!(
            "[PHOTOS_DEBUG] thumbnail_success asset_id={} bytes={}",
            asset_id,
            s.len()
        ));
        Ok(s)
    } else {
        browser_log(&format!(
            "[PHOTOS_DEBUG] thumbnail_failed asset_id={} error=invalid_format",
            asset_id
        ));
        Err("Ungültiges Thumbnail-Format".into())
    }
}

fn photos_cache_dir() -> PathBuf {
    std::env::var("HOME")
        .map(|h| PathBuf::from(h).join("Library/Caches/com.noki.desktop/photos"))
        .unwrap_or_else(|_| std::env::temp_dir().join("noki_photos"))
}

fn ist_noki_foto_cache_datei(path: &Path) -> bool {
    let root = photos_cache_dir();
    let root = fs::canonicalize(&root).unwrap_or(root);
    path.is_file() && path.starts_with(root)
}

fn photos_adapter_export(asset_id: &str) -> Result<String, String> {
    let safe_id: String = asset_id
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    let cache_dir = photos_cache_dir();
    let _ = fs::create_dir_all(&cache_dir);
    let target = cache_dir.join(format!("{safe_id}.jpg"));
    let target_str = target.to_string_lossy().into_owned();

    #[cfg(target_os = "macos")]
    {
        if let (Ok(c_id), Ok(c_tgt)) = (
            std::ffi::CString::new(asset_id),
            std::ffi::CString::new(target_str.as_str()),
        ) {
            let ok = unsafe { noki_photos_export(c_id.as_ptr(), c_tgt.as_ptr()) };
            if ok == 1 && target.is_file() {
                browser_log(&format!(
                    "[PHOTOS_DEBUG] in_process_export_success asset_id={} target={}",
                    asset_id, target_str
                ));
                return Ok(target_str);
            }
        }
    }

    let helper =
        photos_helper_binary().ok_or_else(|| "Fotos-Adapter nicht verfügbar.".to_string())?;
    let out = std::process::Command::new(&helper)
        .arg("export")
        .arg(asset_id)
        .arg(&target_str)
        .output()
        .map_err(|e| format!("Fotos-Adapter Fehler: {e}"))?;

    if !out.status.success() || !target.is_file() {
        let err_msg = String::from_utf8_lossy(&out.stderr);
        return Err(format!("Foto konnte nicht exportiert werden: {err_msg}"));
    }
    Ok(target_str)
}

/// Vorschaubild EINER angefragten Bild- oder PDF-Datei, als data-URI.
/// Kein Massen-Scan: der Browser fragt pro sichtbarer Kachel genau einmal.
#[tauri::command]
async fn datei_thumbnail(pfad: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if pfad.starts_with("photos://asset/") {
            let asset_id = pfad.strip_prefix("photos://asset/").unwrap_or(&pfad);
            return photos_adapter_thumb(asset_id);
        }
        let p = fs::canonicalize(&pfad).map_err(|_| "Datei nicht gefunden.".to_string())?;
        if code_agent::is_sensitive_path(&p) {
            return Err("Geschützte Datei.".into());
        }
        if !ist_in_erlaubtem_browser_ort(&p) && !ist_noki_foto_cache_datei(&p) {
            return Err("Außerhalb der erlaubten Bereiche.".into());
        }
        let (mime, _, bild) = attachments::mime_for(&p);
        let ext = p
            .extension()
            .and_then(|x| x.to_str())
            .unwrap_or("")
            .to_lowercase();
        if !bild && ext != "pdf" {
            return Err("Kein Bild.".into());
        }
        let meta = fs::metadata(&p).map_err(|e| e.to_string())?;
        if meta.len() > 64 * 1024 * 1024 {
            return Err("Bild zu groß für die Vorschau.".into());
        }
        // Direkte data-URI fuer Standard-Webformate unter 1.5 MB
        if bild
            && (ext == "png" || ext == "jpg" || ext == "jpeg" || ext == "gif" || ext == "webp")
            && meta.len() < 1_500_000
        {
            if let Ok(bytes) = fs::read(&p) {
                return Ok(format!("data:{mime};base64,{}", b64(&bytes)));
            }
        }
        static THUMB_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let seq = THUMB_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // Fuer HEIC, HEIF, TIFF oder groessere Bilder via sips ein schnelles 160px JPEG
        if bild {
            let tmp_path = std::env::temp_dir().join(format!(
                "noki_th_{}_{}_{}.jpg",
                std::process::id(),
                seq,
                fast_rand()
            ));
            let ok = std::process::Command::new("/usr/bin/sips")
                .args(["-s", "format", "jpeg", "-Z", "160"])
                .arg(&p)
                .args(["--out", tmp_path.to_str().unwrap_or("")])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if ok && tmp_path.exists() {
                let bytes = fs::read(&tmp_path).unwrap_or_default();
                let _ = fs::remove_file(&tmp_path);
                if !bytes.is_empty() {
                    return Ok(format!("data:image/jpeg;base64,{}", b64(&bytes)));
                }
            }
        }
        // Fuer PDF via qlmanage
        if ext == "pdf" {
            let tmp_dir =
                std::env::temp_dir().join(format!("noki_qlth_{}_{}", std::process::id(), seq));
            let _ = fs::create_dir_all(&tmp_dir);
            let ok = std::process::Command::new("/usr/bin/qlmanage")
                .args(["-t", "-s", "160", "-o"])
                .arg(&tmp_dir)
                .arg(&p)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if ok {
                let file_name = p.file_name().unwrap_or_default().to_string_lossy();
                let gen = tmp_dir.join(format!("{file_name}.png"));
                if gen.exists() {
                    let bytes = fs::read(&gen).unwrap_or_default();
                    let _ = fs::remove_file(&gen);
                    let _ = fs::remove_dir(&tmp_dir);
                    if !bytes.is_empty() {
                        return Ok(format!("data:image/png;base64,{}", b64(&bytes)));
                    }
                }
                let _ = fs::remove_dir(&tmp_dir);
            }
        }
        // Letzter Fallback: Datei direkt lesen falls lesbar
        if bild {
            if let Ok(bytes) = fs::read(&p) {
                return Ok(format!("data:{mime};base64,{}", b64(&bytes)));
            }
        }
        Err("Thumbnail konnte nicht erstellt werden.".into())
    })
    .await
    .map_err(|e| e.to_string())?
}

fn fast_rand() -> u32 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(12345);
    now ^ (std::process::id() << 16)
}

/// Detaillierte Vorschau einer ausgewählten Datei direkt in Ask Noki.
/// Unterstützt Bilder (inkl. HEIC/HEIF/PNG/JPG/WEBP), PDF (Erste Seite + Text),
/// CSV (strukturierte Tabelle), TXT/MD/Code (vollständiger Text) und Office.
#[tauri::command]
async fn datei_vorschau(pfad: String) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if pfad.starts_with("photos://asset/") {
            let asset_id = pfad.strip_prefix("photos://asset/").unwrap_or(&pfad);
            let thumb = photos_adapter_thumb(asset_id).ok();
            return Ok(serde_json::json!({
                "pfad": pfad,
                "name": "Foto",
                "groesse": 0,
                "mime": "image/jpeg",
                "art": "bild",
                "bild_uri": thumb,
                "text_inhalt": None::<String>,
                "csv_tabelle": None::<Vec<Vec<String>>>,
                "metadaten": {"quelle": "Apple-Fotomediathek"},
            }));
        }
        let p = fs::canonicalize(&pfad).map_err(|_| "Datei nicht gefunden.".to_string())?;
        if code_agent::is_sensitive_path(&p) {
            return Err("Geschützte Datei darf nicht eingesehen werden.".into());
        }
        if !ist_in_erlaubtem_browser_ort(&p) && !ist_noki_foto_cache_datei(&p) {
            return Err("Außerhalb der erlaubten Bereiche.".into());
        }
        let meta = fs::metadata(&p).map_err(|e| e.to_string())?;
        let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let groesse = meta.len();
        let (mime, _, bild) = attachments::mime_for(&p);
        let ext = p
            .extension()
            .and_then(|x| x.to_str())
            .unwrap_or("")
            .to_lowercase();

        let mut art = if bild { "bild" } else { "dokument" };
        let mut bild_uri: Option<String> = None;
        let mut text_inhalt: Option<String> = None;
        let mut csv_tabelle: Option<Vec<Vec<String>>> = None;
        let mut metadaten: std::collections::HashMap<String, String> = std::collections::HashMap::new();

        metadaten.insert("groesse".into(), format_groesse(groesse));
        metadaten.insert("typ".into(), ext.to_uppercase());

        let name_lower = name.to_lowercase();
        let paket = name_lower.ends_with(".photoslibrary")
            || name_lower.ends_with(".photolibrary")
            || name_lower.ends_with(".photoboothlibrary")
            || name_lower.starts_with("photo booth")
            || name_lower.contains("photos library");
        if paket {
            metadaten.insert("typ".into(), "Mediathek".into());
            metadaten.insert("hinweis".into(), "Fotos-Mediatheken werden von macOS als Paket verwaltet und sind nicht als Einzeldateien browsbar.".into());
            return Ok(serde_json::json!({
                "pfad": p.to_string_lossy(),
                "name": name,
                "groesse": groesse,
                "mime": "application/x-apple-photoslibrary",
                "art": "paket",
                "bild_uri": null,
                "text_inhalt": null,
                "csv_tabelle": null,
                "metadaten": metadaten,
            }));
        }

        static PREV_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let seq = PREV_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        if bild {
            art = "bild";
            // Bildabmessungen via sips
            if let Ok(out) = std::process::Command::new("/usr/bin/sips")
                .args(["-g", "pixelWidth", "-g", "pixelHeight"])
                .arg(&p)
                .output()
            {
                let s = String::from_utf8_lossy(&out.stdout);
                let w = s.lines().find(|l| l.contains("pixelWidth")).and_then(|l| l.split(':').nth(1)).map(str::trim).unwrap_or("");
                let h = s.lines().find(|l| l.contains("pixelHeight")).and_then(|l| l.split(':').nth(1)).map(str::trim).unwrap_or("");
                if !w.is_empty() && !h.is_empty() {
                    metadaten.insert("abmessungen".into(), format!("{w} × {h} px"));
                }
            }
            // Bild-Preview URI: bis 3 MB direkt fuer Webformate, sonst sips mit max 1200px
            if (ext == "png" || ext == "jpg" || ext == "jpeg" || ext == "webp" || ext == "svg" || ext == "gif") && groesse < 3_000_000 {
                if let Ok(bytes) = fs::read(&p) {
                    bild_uri = Some(format!("data:{mime};base64,{}", b64(&bytes)));
                }
            }
            if bild_uri.is_none() {
                let tmp_path = std::env::temp_dir().join(format!("noki_pv_{}_{}_{}.jpg", std::process::id(), seq, fast_rand()));
                let ok = std::process::Command::new("/usr/bin/sips")
                    .args(["-s", "format", "jpeg", "-Z", "1200"])
                    .arg(&p)
                    .args(["--out", tmp_path.to_str().unwrap_or("")])
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false);
                if ok && tmp_path.exists() {
                    if let Ok(bytes) = fs::read(&tmp_path) {
                        bild_uri = Some(format!("data:image/jpeg;base64,{}", b64(&bytes)));
                    }
                    let _ = fs::remove_file(&tmp_path);
                }
            }
        } else if ext == "pdf" {
            art = "pdf";
            // PDF Seite 1 QuickLook Vorschau
            let tmp_dir = std::env::temp_dir().join(format!("noki_pdf_{}_{}", std::process::id(), seq));
            let _ = fs::create_dir_all(&tmp_dir);
            let ok = std::process::Command::new("/usr/bin/qlmanage")
                .args(["-t", "-s", "600", "-o"])
                .arg(&tmp_dir)
                .arg(&p)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if ok {
                let gen = tmp_dir.join(format!("{name}.png"));
                if gen.exists() {
                    if let Ok(bytes) = fs::read(&gen) {
                        bild_uri = Some(format!("data:image/png;base64,{}", b64(&bytes)));
                    }
                    let _ = fs::remove_file(&gen);
                }
                let _ = fs::remove_dir(&tmp_dir);
            }
            // PDF Text via spotlight_text
            if let Ok(txt) = document_pipeline::spotlight_text(&p) {
                if !txt.trim().is_empty() {
                    let chars: String = txt.chars().take(8_000).collect();
                    text_inhalt = Some(chars);
                }
            }
            // PDF Seitenzahl
            if let Ok(out) = std::process::Command::new("/usr/bin/mdls")
                .args(["-name", "kMDItemNumberOfPages", "-raw"])
                .arg(&p)
                .output()
            {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if s != "(null)" && !s.is_empty() {
                    metadaten.insert("seiten".into(), format!("{s} Seiten"));
                }
            }
        } else if ext == "csv" || ext == "tsv" {
            art = "csv";
            if let Ok(content) = fs::read_to_string(&p) {
                let delim = if ext == "tsv" { '\t' } else if content.lines().next().map_or(false, |l| l.contains(';') && !l.contains(',')) { ';' } else { ',' };
                let mut rows = Vec::new();
                for line in content.lines().take(30) {
                    let cols: Vec<String> = line.split(delim).take(10).map(|c| c.trim().trim_matches('"').to_string()).collect();
                    if !cols.is_empty() {
                        rows.push(cols);
                    }
                }
                metadaten.insert("zeilen".into(), format!("ca. {} Zeilen", content.lines().count()));
                text_inhalt = Some(content.chars().take(20000).collect());
                csv_tabelle = Some(rows);
            }
        } else if ext == "docx" || ext == "doc" || ext == "rtf" || ext == "odt" {
            art = "office";
            if let Ok(doc) = document_pipeline::extract(&p, None) {
                text_inhalt = Some(doc.text);
            }
            // QuickLook preview
            let tmp_dir = std::env::temp_dir().join(format!("noki_doc_{}_{}", std::process::id(), seq));
            let _ = fs::create_dir_all(&tmp_dir);
            let ok = std::process::Command::new("/usr/bin/qlmanage")
                .args(["-t", "-s", "600", "-o"])
                .arg(&tmp_dir)
                .arg(&p)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if ok {
                let gen = tmp_dir.join(format!("{name}.png"));
                if gen.exists() {
                    if let Ok(bytes) = fs::read(&gen) {
                        bild_uri = Some(format!("data:image/png;base64,{}", b64(&bytes)));
                    }
                    let _ = fs::remove_file(&gen);
                }
                let _ = fs::remove_dir(&tmp_dir);
            }
        } else if attachments::is_audio(&p) {
            art = "audio";
            if let Ok(out) = std::process::Command::new("/usr/bin/mdls")
                .args(["-name", "kMDItemDurationSeconds", "-raw"])
                .arg(&p)
                .output()
            {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if let Ok(sec) = s.parse::<f64>() {
                    let m = (sec / 60.0).floor() as u32;
                    let s = (sec % 60.0).round() as u32;
                    metadaten.insert("dauer".into(), format!("{m}:{s:02} min"));
                }
            }
        } else {
            // Text, Markdown, Quellcode, Log, JSON, XML, etc.
            art = "text";
            if let Ok(content) = fs::read_to_string(&p) {
                metadaten.insert("zeilen".into(), format!("{} Zeilen", content.lines().count()));
                metadaten.insert("zeichen".into(), format!("{} Zeichen", content.chars().count()));
                text_inhalt = Some(content.chars().take(40000).collect());
            }
        }

        Ok(serde_json::json!({
            "pfad": p.to_string_lossy(),
            "name": name,
            "groesse": groesse,
            "mime": mime,
            "art": art,
            "bild_uri": bild_uri,
            "text_inhalt": text_inhalt,
            "csv_tabelle": csv_tabelle,
            "metadaten": metadaten,
        }))
    })
    .await
    .map_err(|e| e.to_string())?
}

fn format_groesse(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}

/// Startansicht des Dateibrowsers: genau die 5 erlaubten Hauptorte.
/// Weder Benutzerordner/Home als eigene Kachel, noch / oder andere Systemordner.
#[tauri::command]
fn datei_browser_start() -> serde_json::Value {
    let orte = erlaubte_browser_orte();
    let eintraege: Vec<serde_json::Value> = orte
        .into_iter()
        .map(|(label, p)| {
            serde_json::json!({
                "name": label,
                "pfad": p.to_string_lossy(),
                "ordner": true,
                "paket": false,
                "groesse": 0,
                "mime": "inode/directory",
                "bild": false,
            })
        })
        .collect();
    eprintln!("[BROWSER] start orte={}", eintraege.len());
    serde_json::json!({ "pfad": "", "start": true, "oben": null, "eintraege": eintraege })
}

/// Verzeichnis-Metadaten fuer den EINGEBAUTEN Dateibrowser.
///
/// Sicherheitsgrenze: Navigation ist ausschließlich innerhalb der 5 Hauptorte erlaubt.
/// Beim Erreichen des Hauptorts führt "Zurück" zur Noki-Startansicht (oben = None).
/// Systemwurzel (/), /Users, $HOME und fremde Pfade sind blockiert.
#[tauri::command]
async fn datei_browser_liste(pfad: Option<String>) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let angefragt = match pfad.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
            Some(p) => p.to_string(),
            None => {
                eprintln!("[BROWSER] location=none resolved=none exists=no permission=no raw_count=0 filtered_count=0 image_count=0 error=kein_pfad");
                return Err("Kein Pfad angegeben.".into());
            }
        };
        let basis_raw = std::path::PathBuf::from(&angefragt);
        let basis = match fs::canonicalize(&basis_raw) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("[BROWSER] location={} resolved={} exists=no permission=no raw_count=0 filtered_count=0 image_count=0 error={e}", angefragt, basis_raw.display());
                return Err("Ordner nicht gefunden.".to_string());
            }
        };
        if code_agent::is_sensitive_path(&basis) {
            eprintln!("[BROWSER] location={} resolved={} exists=yes permission=no raw_count=0 filtered_count=0 image_count=0 error=geschuetzt", angefragt, basis.display());
            return Err("Dieser Ordner ist geschützt.".into());
        }

        // Sicherheitsgrenze im Backend: Darf nicht außerhalb der 5 erlaubten Hauptorte liegen.
        let roots = erlaubte_browser_orte();
        let matched_root = roots
            .iter()
            .filter(|(_, r)| basis.starts_with(r))
            .max_by_key(|(_, r)| r.as_os_str().len());
        let Some((root_label, root_path)) = matched_root else {
            eprintln!("[BROWSER] location={} resolved={} exists=yes permission=no raw_count=0 filtered_count=0 image_count=0 error=ausserhalb_erlaubter_orte", angefragt, basis.display());
            return Err("Navigation außerhalb der erlaubten Bereiche ist nicht zulässig.".into());
        };

        if !basis.is_dir() {
            eprintln!("[BROWSER] location={} resolved={} exists=yes permission=yes raw_count=0 filtered_count=0 image_count=0 error=kein_ordner", root_label, basis.display());
            return Err("Das ist kein Ordner.".into());
        }

        let logical_location = if basis.ends_with("Pictures") || basis.to_string_lossy().ends_with("/Pictures") {
            "Pictures"
        } else {
            *root_label
        };

        let rd = match fs::read_dir(&basis) {
            Ok(rd) => rd,
            Err(e) => {
                let verweigert = e.kind() == std::io::ErrorKind::PermissionDenied;
                eprintln!(
                    "[BROWSER] location={} resolved={} exists=yes permission=no raw_count=0 filtered_count=0 image_count=0 error={e}",
                    logical_location,
                    basis.display()
                );
                return Err(if verweigert {
                    "Zugriff nicht verfügbar. Bitte Noki in den Systemeinstellungen für diesen Ordner freigeben.".to_string()
                } else {
                    "Ordner konnte nicht geladen werden.".to_string()
                });
            }
        };

        let mut raw_count = 0usize;
        let mut filtered_count = 0usize;
        let mut image_count = 0usize;
        let mut eintraege: Vec<serde_json::Value> = Vec::new();

        let is_pictures = logical_location == "Pictures";
        let mut photos_status = "none".to_string();
        let mut photos_hinweis: Option<String> = None;
        let mut photos_asset_count = 0usize;
        let mut photos_total = 0usize;

        if is_pictures {
            let (st, hinw, total, assets) = photos_adapter_list(0, GALERIE_SEITENGROESSE);
            photos_status = st;
            photos_hinweis = hinw;
            photos_total = total;
            photos_asset_count = assets.len();
            for a in assets {
                if let Some(entry) = foto_eintrag(&a) {
                    image_count += 1;
                    filtered_count += 1;
                    eintraege.push(entry);
                }
            }
        }

        for e in rd.flatten() {
            raw_count += 1;
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || code_agent::is_sensitive_path(&p) {
                continue;
            }
            let Ok(meta) = e.metadata() else { continue };
            let name_lower = name.to_lowercase();
            let paket = name_lower.ends_with(".photoslibrary")
                || name_lower.ends_with(".photolibrary")
                || name_lower.ends_with(".photoboothlibrary")
                || name_lower.starts_with("photo booth")
                || name_lower.contains("photos library");

            if paket {
                if is_pictures {
                    // In der Galerie direkt unter Bilder keine unöffenbaren Mediatheken-Pakete anzeigen
                    continue;
                }
                filtered_count += 1;
                eintraege.push(serde_json::json!({
                    "name": name,
                    "pfad": p.to_string_lossy(),
                    "ordner": false,
                    "paket": true,
                    "nicht_unterstuetzt": true,
                    "hinweis": "macOS-Paket (als Paket nicht direkt browsbar)",
                    "groesse": meta.len(),
                    "mime": "application/x-apple-photoslibrary",
                    "bild": false,
                }));
                continue;
            }

            let ist_ordner = meta.is_dir();
            let (mime, lesbar, bild) = attachments::mime_for(&p);
            let ist_audio = attachments::is_audio(&p);
            if !ist_ordner && !lesbar && !bild && !ist_audio {
                continue;
            }

            if bild {
                image_count += 1;
            }
            filtered_count += 1;

            eintraege.push(serde_json::json!({
                "name": name,
                "pfad": p.to_string_lossy(),
                "ordner": ist_ordner,
                "paket": false,
                "groesse": if ist_ordner { 0 } else { meta.len() },
                "mime": if ist_ordner { "inode/directory" } else { mime },
                "bild": bild,
                "quelle": "lokal",
            }));
        }

        // PhotoKit liefert bereits Datum absteigend. Nur lokale Dateien/Ordner
        // alphabetisch sortieren; sonst würde der Browser die Fotoordnung zerstören.
        let local_start = photos_asset_count.min(eintraege.len());
        eintraege[local_start..].sort_by(|a, b| {
            let (ao, bo) = (a["ordner"].as_bool().unwrap_or(false), b["ordner"].as_bool().unwrap_or(false));
            bo.cmp(&ao).then_with(|| {
                a["name"].as_str().unwrap_or("").to_lowercase()
                    .cmp(&b["name"].as_str().unwrap_or("").to_lowercase())
            })
        });
        if !is_pictures { eintraege.truncate(500); }

        browser_log(&format!(
            "[BROWSER] location={} resolved={} exists=yes permission=yes raw_count={} filtered_count={} image_count={} error=none",
            logical_location,
            basis.display(),
            raw_count,
            filtered_count,
            image_count
        ));
        if is_pictures {
            browser_log(&format!(
                "[PHOTOS_DEBUG] pictures_path={} pictures_file_count={} raw_count={} photos_auth_status={} photos_asset_count={} photos_result_count={} total_entries={}",
                basis.display(),
                image_count.saturating_sub(photos_asset_count),
                raw_count,
                photos_status,
                photos_asset_count,
                eintraege.len(),
                eintraege.len()
            ));
        }

        // Ist basis genau der Hauptort (z. B. ~/Pictures), führt "Zurück" zur Startseite (oben = None).
        // Ist basis ein Unterordner, führt "Zurück" zum Parent, solange dieser innerhalb des Hauptorts liegt.
        let oben = if &basis == root_path {
            None
        } else {
            basis.parent().and_then(|p| {
                if p.starts_with(root_path) {
                    Some(p.to_string_lossy().into_owned())
                } else {
                    None
                }
            })
        };

        Ok(serde_json::json!({
            "pfad": basis.to_string_lossy(),
            "start": false,
            "oben": oben,
            "ist_galerie": is_pictures,
            "photos_status": photos_status,
            "photos_hinweis": photos_hinweis,
            "photos_total": photos_total,
            "photos_next_offset": photos_asset_count,
            "eintraege": eintraege,
        }))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Native Dateiauswahl. Der Nutzer waehlt - Noki durchsucht nichts selbst.
/// Genau deshalb steht hier ein Systemdialog und kein Verzeichnis-Scan.
///
/// `async` + `spawn_blocking` ist hier kein Stil, sondern der Fix: ein
/// SYNCHRONER Tauri-Befehl laeuft auf dem Main Thread. `choose file` blockiert
/// bis der Nutzer entscheidet - und damit stand die macOS-Runloop aller
/// Fenster still, sichtbar als Regenbogenkreis, sobald die Maus ueber Noki kam.
/// Der Dialog gehoert ohnehin einem eigenen Prozess; Noki muss nur warten,
/// nicht blockieren.
#[tauri::command]
async fn anhang_waehlen() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(anhang_waehlen_blockierend)
        .await
        .map_err(|e| e.to_string())?
}

fn anhang_waehlen_blockierend() -> Result<Vec<String>, String> {
    let script = r#"set theFiles to choose file with prompt "Dateien an diesen Noki-Chat anhängen" with multiple selections allowed
set out to ""
repeat with f in theFiles
  set out to out & POSIX path of f & linefeed
end repeat
return out"#;
    let output = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", script])
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        // Abbrechen im Dialog ist kein Fehler, nur eine leere Auswahl.
        return Ok(Vec::new());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect())
}

#[tauri::command]
fn anhang_hinzufuegen(
    store: tauri::State<'_, std::sync::Arc<attachments::AttachmentStore>>,
    conversation_id: String,
    pfade: Vec<String>,
) -> Result<Vec<attachments::Attachment>, String> {
    if conversation_id.trim().is_empty() {
        return Err("Kein Chat ausgewählt.".into());
    }
    if pfade.len() > 12 {
        return Err("Höchstens 12 Dateien auf einmal.".into());
    }
    let mut added = Vec::new();
    let mut fehler = Vec::new();
    for p in pfade {
        let reale_datei = if p.starts_with("photos://asset/") {
            let asset_id = p.strip_prefix("photos://asset/").unwrap_or(&p);
            match photos_adapter_export(asset_id) {
                Ok(path) => path,
                Err(e) => {
                    fehler.push(e);
                    continue;
                }
            }
        } else {
            p
        };
        match store.add(&conversation_id, &reale_datei) {
            Ok(a) => added.push(a),
            Err(e) => fehler.push(e),
        }
    }
    if added.is_empty() && !fehler.is_empty() {
        return Err(fehler.join(" "));
    }
    Ok(added)
}

#[tauri::command]
fn anhang_liste(
    store: tauri::State<'_, std::sync::Arc<attachments::AttachmentStore>>,
    conversation_id: String,
) -> Vec<attachments::Attachment> {
    store.list(&conversation_id)
}

#[tauri::command]
fn anhang_entfernen(
    store: tauri::State<'_, std::sync::Arc<attachments::AttachmentStore>>,
    conversation_id: String,
    id: u64,
) -> bool {
    store.remove(&conversation_id, id)
}

/// Sichtbarer Nachweis statt Vertrauen: was wurde wirklich versucht?
#[tauri::command]
fn noki_aktions_log() -> Vec<capability::ActionAudit> {
    capability::action_log()
}

/// Oeffnet EINE vom Intent gebaute Suchseite eines bekannten Dienstes.
///
/// Die URL entsteht nie aus gelesenem Inhalt, sondern ausschliesslich aus der
/// Ziel-Registry (`site_search_url`) plus dem Suchbegriff des Nutzers. Hier wird
/// das trotzdem ein zweites Mal geprueft: https, bekannter Host, keine
/// Steuerzeichen. Eine Webseite kann so keine eigene Adresse unterschieben.
#[tauri::command]
fn browser_url_oeffnen(url: String, app: Option<String>) -> Result<serde_json::Value, String> {
    let url = url.trim();
    if url.chars().count() > 2000 || url.chars().any(char::is_control) {
        return Err("Ungültige Adresse.".into());
    }
    let rest = url
        .strip_prefix("https://")
        .ok_or("Nur https-Adressen werden geöffnet.")?;
    let host = rest
        .split('/')
        .next()
        .unwrap_or("")
        .split('@')
        .next_back()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_lowercase();
    if !capability::is_known_web_host(&host) {
        return Err("Dieser Dienst steht nicht in der Noki-Zielliste.".into());
    }
    // Hat der Nutzer einen Browser beim Namen genannt, bekommt der die
    // Adresse. Erlaubt sind nur installierte Apps, die die Registry als
    // Browser fuehrt - sonst waere `open -a` ein offenes Scheunentor.
    let browser = app.as_deref().map(str::trim).filter(|a| !a.is_empty());
    let browser = match browser {
        None => None,
        Some(path) => {
            let name = std::path::Path::new(path)
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            let ok = capability::adapter_for(name)
                .is_some_and(|a| capability::is_browser_key(a.key))
                && std::path::Path::new(path).is_dir();
            if !ok {
                return Err("Dieser Browser steht nicht in der Noki-Zielliste.".into());
            }
            Some(path.to_owned())
        }
    };
    let mut cmd = std::process::Command::new("/usr/bin/open");
    if let Some(b) = &browser {
        cmd.arg("-a").arg(b);
    }
    let ok = cmd
        .arg(url)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        return Err("Adresse konnte nicht geöffnet werden.".into());
    }
    Ok(serde_json::json!({"opened": true, "url": url, "host": host, "browser": browser}))
}

/// Kleine lokale Werkzeug-Daten (Fokus-Auswahl, laufender Timer, Einstellungen).
fn werk_daten_pfad(app: &tauri::AppHandle, k: &str) -> Option<PathBuf> {
    if ![
        "fokus",
        "timer",
        "einstellungen",
        "freeze",
        "kamera",
        "vorschau",
    ]
    .contains(&k)
    {
        return None;
    }
    Some(werk_dir(app)?.join(format!("werk-{k}.json")))
}
#[tauri::command]
fn werk_daten_lesen(app: tauri::AppHandle, schluessel: String) -> serde_json::Value {
    werk_daten_pfad(&app, &schluessel)
        .and_then(|d| fs::read_to_string(d).ok())
        .and_then(|r| serde_json::from_str(&r).ok())
        .unwrap_or(serde_json::Value::Null)
}
#[tauri::command]
fn werk_daten_schreiben(
    app: tauri::AppHandle,
    schluessel: String,
    wert: serde_json::Value,
) -> bool {
    if schluessel == "vorschau" {
        if let Some(b) = wert.get("sichtbar").and_then(|v| v.as_bool()) {
            MINIATUR_NUTZER_SICHTBAR.store(b, Ordering::Relaxed);
        }
    }
    match (
        werk_daten_pfad(&app, &schluessel),
        serde_json::to_string(&wert),
    ) {
        (Some(d), Ok(s)) => fs::write(d, s).is_ok(),
        _ => false,
    }
}

/// USER_SHOWN / USER_HIDDEN: the Miniatur preference the USER set (Shortcut
/// 4, hide). Only the page's explicit preference writes change it - a
/// temporary hide (same Space, swipe) never does.
static MINIATUR_NUTZER_SICHTBAR: AtomicBool = AtomicBool::new(true);

/// Dauerhafte Datei-Lesezeichen ueber Foundation (NSURL bookmarkData).
/// Die App ist nicht sandboxed: ein normales Lesezeichen genuegt, es folgt
/// Umbenennen und Verschieben und braucht keine Zugriffsfreigabe.
#[cfg(target_os = "macos")]
mod lesezeichen {
    use std::ffi::{c_char, c_void, CStr, CString};
    type Id = *mut c_void;
    #[link(name = "objc")]
    #[allow(clashing_extern_declarations)]
    extern "C" {
        fn objc_getClass(n: *const c_char) -> Id;
        fn sel_registerName(n: *const c_char) -> Id;
        fn objc_msgSend();
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(p: *mut c_void);
    }
    #[link(name = "Foundation", kind = "framework")]
    extern "C" {}
    #[link(name = "AppKit", kind = "framework")]
    extern "C" {}
    #[link(name = "objc")]
    #[allow(clashing_extern_declarations)]
    extern "C" {
        fn objc_allocateClassPair(sup: Id, n: *const c_char, extra: usize) -> Id;
        fn objc_registerClassPair(c: Id);
        fn class_addMethod(c: Id, sel: Id, imp: *const c_void, typ: *const c_char) -> bool;
        fn objc_getProtocol(n: *const c_char) -> Id;
        fn class_addProtocol(c: Id, p: Id) -> bool;
    }
    #[link(name = "ApplicationServices", kind = "framework")]
    #[allow(clashing_extern_declarations)]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
        fn AXIsProcessTrustedWithOptions(o: Id) -> bool;
        fn AXUIElementCreateApplication(pid: i32) -> Id;
        fn AXUIElementCopyAttributeValue(e: Id, a: Id, v: *mut Id) -> i32;
        fn AXUIElementSetAttributeValue(e: Id, a: Id, v: Id) -> i32;
        fn AXValueCreate(t: u32, p: *const c_void) -> Id;
        fn AXValueGetValue(v: Id, t: u32, p: *mut c_void) -> bool;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    #[allow(clashing_extern_declarations)]
    extern "C" {
        fn CFArrayGetCount(a: Id) -> isize;
        fn CFArrayGetValueAtIndex(a: Id, i: isize) -> Id;
        fn CFRelease(x: Id);
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Punkt {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Rahmen {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }

    unsafe fn k(n: &[u8]) -> Id {
        objc_getClass(n.as_ptr() as *const c_char)
    }
    unsafe fn s(n: &[u8]) -> Id {
        sel_registerName(n.as_ptr() as *const c_char)
    }
    fn msg() -> *const c_void {
        objc_msgSend as unsafe extern "C" fn() as *const c_void
    }

    unsafe fn url(pfad: &str) -> Id {
        let c = match CString::new(pfad) {
            Ok(c) => c,
            Err(_) => return std::ptr::null_mut(),
        };
        let f: extern "C" fn(Id, Id, *const c_char) -> Id = std::mem::transmute(msg());
        let ns = f(k(b"NSString\0"), s(b"stringWithUTF8String:\0"), c.as_ptr());
        if ns.is_null() {
            return ns;
        }
        let g: extern "C" fn(Id, Id, Id) -> Id = std::mem::transmute(msg());
        g(k(b"NSURL\0"), s(b"fileURLWithPath:\0"), ns)
    }

    unsafe fn ns_text(ns: Id) -> String {
        if ns.is_null() {
            return String::new();
        }
        let f: extern "C" fn(Id, Id) -> *const c_char = std::mem::transmute(msg());
        let c = f(ns, s(b"UTF8String\0"));
        if c.is_null() {
            String::new()
        } else {
            CStr::from_ptr(c).to_string_lossy().into_owned()
        }
    }
    /// Apps, die diese Datei oeffnen koennen (NSWorkspace, macOS 12+):
    /// (Name, .app-Pfad, Standard-App?). Standard zuerst, dann alphabetisch.
    pub fn apps_fuer_datei(pfad: &str) -> Vec<(String, String, bool)> {
        let mut v: Vec<(String, String, bool)> = Vec::new();
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let u = url(pfad);
            if !u.is_null() {
                let g: extern "C" fn(Id, Id) -> Id = std::mem::transmute(msg());
                let f1: extern "C" fn(Id, Id, Id) -> Id = std::mem::transmute(msg());
                let n: extern "C" fn(Id, Id) -> usize = std::mem::transmute(msg());
                let at: extern "C" fn(Id, Id, usize) -> Id = std::mem::transmute(msg());
                let ws = g(k(b"NSWorkspace\0"), s(b"sharedWorkspace\0"));
                let std_url = f1(ws, s(b"URLForApplicationToOpenURL:\0"), u);
                let std_pfad = if std_url.is_null() {
                    String::new()
                } else {
                    ns_text(g(std_url, s(b"path\0")))
                };
                let arr = f1(ws, s(b"URLsForApplicationsToOpenURL:\0"), u);
                if !arr.is_null() {
                    for i in 0..n(arr, s(b"count\0")) {
                        let p = ns_text(g(at(arr, s(b"objectAtIndex:\0"), i), s(b"path\0")));
                        if p.is_empty() {
                            continue;
                        }
                        let name = std::path::Path::new(&p)
                            .file_stem()
                            .map(|x| x.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let st = p == std_pfad;
                        v.push((name, p, st));
                    }
                }
            }
            objc_autoreleasePoolPop(pool);
        }
        v.sort_by(|a, b| {
            b.2.cmp(&a.2)
                .then(a.0.to_lowercase().cmp(&b.0.to_lowercase()))
        });
        v.dedup_by(|a, b| a.0 == b.0);
        v
    }

    /// In den Papierkorb legen (NSFileManager, wiederherstellbar).
    pub fn in_papierkorb(pfad: &str) -> bool {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let u = url(pfad);
            let mut ok = false;
            if !u.is_null() {
                let g: extern "C" fn(Id, Id) -> Id = std::mem::transmute(msg());
                let t: extern "C" fn(Id, Id, Id, *mut Id, *mut Id) -> i8 =
                    std::mem::transmute(msg());
                let fm = g(k(b"NSFileManager\0"), s(b"defaultManager\0"));
                ok = t(
                    fm,
                    s(b"trashItemAtURL:resultingItemURL:error:\0"),
                    u,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                ) != 0;
            }
            objc_autoreleasePoolPop(pool);
            ok
        }
    }

    pub fn erstellen(pfad: &str) -> Option<Vec<u8>> {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let u = url(pfad);
            let mut out = None;
            if !u.is_null() {
                let f: extern "C" fn(Id, Id, u64, Id, Id, *mut Id) -> Id =
                    std::mem::transmute(msg());
                let mut err: Id = std::ptr::null_mut();
                let d = f(u, s(b"bookmarkDataWithOptions:includingResourceValuesForKeys:relativeToURL:error:\0"),
                          0, std::ptr::null_mut(), std::ptr::null_mut(), &mut err);
                if !d.is_null() {
                    let fl: extern "C" fn(Id, Id) -> usize = std::mem::transmute(msg());
                    let fb: extern "C" fn(Id, Id) -> *const u8 = std::mem::transmute(msg());
                    let n = fl(d, s(b"length\0"));
                    let b = fb(d, s(b"bytes\0"));
                    if !b.is_null() && n > 0 {
                        out = Some(std::slice::from_raw_parts(b, n).to_vec());
                    }
                }
            }
            objc_autoreleasePoolPop(pool);
            out
        }
    }

    unsafe fn text(id: Id) -> Option<String> {
        if id.is_null() {
            return None;
        }
        let fu: extern "C" fn(Id, Id) -> *const c_char = std::mem::transmute(msg());
        let c = fu(id, s(b"UTF8String\0"));
        if c.is_null() {
            None
        } else {
            Some(CStr::from_ptr(c).to_string_lossy().into_owned())
        }
    }

    /// Bundle-Id der App, die gerade vorne ist (NSWorkspace).
    pub fn vorne_bundle() -> Option<String> {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let f: extern "C" fn(Id, Id) -> Id = std::mem::transmute(msg());
            let ws = f(k(b"NSWorkspace\0"), s(b"sharedWorkspace\0"));
            let a = if ws.is_null() {
                ws
            } else {
                f(ws, s(b"frontmostApplication\0"))
            };
            let out = if a.is_null() {
                None
            } else {
                text(f(a, s(b"bundleIdentifier\0")))
            };
            objc_autoreleasePoolPop(pool);
            out
        }
    }

    // Zieh-Quelle: erlaubt ausschliesslich NSDragOperationCopy (1).
    extern "C" fn nur_kopieren(_s: Id, _c: Id, _sess: Id, _ctx: isize) -> usize {
        1
    }
    static QUELLE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    unsafe fn quelle() -> Id {
        *QUELLE.get_or_init(|| {
            #[allow(unused_unsafe)]
            unsafe {
                let c = objc_allocateClassPair(
                    k(b"NSObject\0"),
                    b"NokiAblageZiehen\0".as_ptr() as *const c_char,
                    0,
                );
                if c.is_null() {
                    return 0;
                }
                class_addMethod(
                    c,
                    s(b"draggingSession:sourceOperationMaskForDraggingContext:\0"),
                    nur_kopieren as *const c_void,
                    b"Q@:@q\0".as_ptr() as *const c_char,
                );
                let pr = objc_getProtocol(b"NSDraggingSource\0".as_ptr() as *const c_char);
                if !pr.is_null() {
                    class_addProtocol(c, pr);
                }
                objc_registerClassPair(c);
                let f: extern "C" fn(Id, Id) -> Id = std::mem::transmute(msg());
                f(f(c, s(b"alloc\0")), s(b"init\0")) as usize
            }
        }) as Id
    }

    /// Startet am Noki-Fenster eine native Ziehsitzung mit der Datei-URL.
    /// Muss auf dem Hauptthread laufen, waehrend die Maustaste gedrueckt ist.
    pub unsafe fn ziehen(nswin: Id, pfad: &str) -> bool {
        let pool = objc_autoreleasePoolPush();
        let f0: extern "C" fn(Id, Id) -> Id = std::mem::transmute(msg());
        let fi: extern "C" fn(Id, Id, Id) -> Id = std::mem::transmute(msg());
        let u = url(pfad);
        let view = if nswin.is_null() {
            nswin
        } else {
            f0(nswin, s(b"contentView\0"))
        };
        let src = quelle();
        let mut ok = false;
        if !u.is_null() && !view.is_null() && !src.is_null() {
            let fp: extern "C" fn(Id, Id) -> Punkt = std::mem::transmute(msg());
            let pw = fp(nswin, s(b"mouseLocationOutsideOfEventStream\0"));
            let fc: extern "C" fn(Id, Id, Punkt, Id) -> Punkt = std::mem::transmute(msg());
            let pv = fc(
                view,
                s(b"convertPoint:fromView:\0"),
                pw,
                std::ptr::null_mut(),
            );
            let item = fi(
                f0(k(b"NSDraggingItem\0"), s(b"alloc\0")),
                s(b"initWithPasteboardWriter:\0"),
                u,
            );
            let ws = f0(k(b"NSWorkspace\0"), s(b"sharedWorkspace\0"));
            let fs: extern "C" fn(Id, Id, *const c_char) -> Id = std::mem::transmute(msg());
            let cp = CString::new(pfad).unwrap_or_default();
            let ns = fs(k(b"NSString\0"), s(b"stringWithUTF8String:\0"), cp.as_ptr());
            let icon = if ws.is_null() || ns.is_null() {
                std::ptr::null_mut()
            } else {
                fi(ws, s(b"iconForFile:\0"), ns)
            };
            let fr: extern "C" fn(Id, Id, Rahmen, Id) = std::mem::transmute(msg());
            fr(
                item,
                s(b"setDraggingFrame:contents:\0"),
                Rahmen {
                    x: pv.x - 16.0,
                    y: pv.y - 16.0,
                    w: 32.0,
                    h: 32.0,
                },
                icon,
            );
            let fwn: extern "C" fn(Id, Id) -> isize = std::mem::transmute(msg());
            let wn = fwn(nswin, s(b"windowNumber\0"));
            let fup: extern "C" fn(Id, Id) -> f64 = std::mem::transmute(msg());
            let ts = fup(
                f0(k(b"NSProcessInfo\0"), s(b"processInfo\0")),
                s(b"systemUptime\0"),
            );
            let fe: extern "C" fn(
                Id,
                Id,
                usize,
                Punkt,
                usize,
                f64,
                isize,
                Id,
                isize,
                isize,
                f32,
            ) -> Id = std::mem::transmute(msg());
            let ev = fe(k(b"NSEvent\0"),
                        s(b"mouseEventWithType:location:modifierFlags:timestamp:windowNumber:context:eventNumber:clickCount:pressure:\0"),
                        6, pw, 0, ts, wn, std::ptr::null_mut(), 0, 1, 1.0); // 6 = LeftMouseDragged
            let arr = fi(k(b"NSArray\0"), s(b"arrayWithObject:\0"), item);
            if !ev.is_null() && !item.is_null() && !arr.is_null() {
                let fb: extern "C" fn(Id, Id, Id, Id, Id) -> Id = std::mem::transmute(msg());
                ok = !fb(
                    view,
                    s(b"beginDraggingSessionWithItems:event:source:\0"),
                    arr,
                    ev,
                    src,
                )
                .is_null();
            }
        }
        objc_autoreleasePoolPop(pool);
        ok
    }

    // ---- kleine Helfer ---------------------------------------------------
    unsafe fn ns(t: &str) -> Id {
        let c = CString::new(t).unwrap_or_default();
        let f: extern "C" fn(Id, Id, *const c_char) -> Id = std::mem::transmute(msg());
        f(k(b"NSString\0"), s(b"stringWithUTF8String:\0"), c.as_ptr())
    }
    unsafe fn m0(o: Id, sel: &[u8]) -> Id {
        if o.is_null() {
            return o;
        }
        let f: extern "C" fn(Id, Id) -> Id = std::mem::transmute(msg());
        f(o, s(sel))
    }
    unsafe fn m1(o: Id, sel: &[u8], a: Id) -> Id {
        if o.is_null() {
            return o;
        }
        let f: extern "C" fn(Id, Id, Id) -> Id = std::mem::transmute(msg());
        f(o, s(sel), a)
    }
    unsafe fn anzahl(arr: Id) -> usize {
        if arr.is_null() {
            return 0;
        }
        let f: extern "C" fn(Id, Id) -> usize = std::mem::transmute(msg());
        f(arr, s(b"count\0"))
    }
    unsafe fn an_stelle(arr: Id, i: usize) -> Id {
        let f: extern "C" fn(Id, Id, usize) -> Id = std::mem::transmute(msg());
        f(arr, s(b"objectAtIndex:\0"), i)
    }
    unsafe fn pid_von(a: Id) -> i32 {
        let f: extern "C" fn(Id, Id) -> i32 = std::mem::transmute(msg());
        f(a, s(b"processIdentifier\0"))
    }

    // ---- Zwischenablage (NSPasteboard) ------------------------------------
    pub enum PbInhalt {
        Text(String, String),
        Bild(Vec<u8>),
    }
    pub fn pb_zaehler() -> i64 {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let pb = m0(k(b"NSPasteboard\0"), b"generalPasteboard\0");
            let n = if pb.is_null() {
                -1
            } else {
                let f: extern "C" fn(Id, Id) -> isize = std::mem::transmute(msg());
                f(pb, s(b"changeCount\0")) as i64
            };
            objc_autoreleasePoolPop(pool);
            n
        }
    }
    unsafe fn hat_typ(typen: Id, t: &str) -> bool {
        if typen.is_null() {
            return false;
        }
        let f: extern "C" fn(Id, Id, Id) -> i8 = std::mem::transmute(msg());
        f(typen, s(b"containsObject:\0"), ns(t)) != 0
    }
    /// Aktueller Inhalt: Text/Link (mit Titel, falls vorhanden) oder Bild.
    /// Geheim/fluechtig markierte Inhalte und Dateien liefern nichts.
    pub fn pb_lesen() -> Option<PbInhalt> {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let pb = m0(k(b"NSPasteboard\0"), b"generalPasteboard\0");
            let typen = m0(pb, b"types\0");
            let geheim = [
                "org.nspasteboard.ConcealedType",
                "org.nspasteboard.TransientType",
                "org.nspasteboard.AutoGeneratedType",
                "public.file-url",
            ]
            .iter()
            .any(|t| hat_typ(typen, t));
            let mut out = None;
            if !pb.is_null() && !geheim {
                if let Some(t) = text(m1(pb, b"stringForType:\0", ns("public.utf8-plain-text"))) {
                    let titel = text(m1(pb, b"stringForType:\0", ns("public.url-name")))
                        .unwrap_or_default();
                    out = Some(PbInhalt::Text(t, titel));
                } else {
                    for typ in ["public.png", "public.tiff"] {
                        let d = m1(pb, b"dataForType:\0", ns(typ));
                        if d.is_null() {
                            continue;
                        }
                        let fl: extern "C" fn(Id, Id) -> usize = std::mem::transmute(msg());
                        let fb: extern "C" fn(Id, Id) -> *const u8 = std::mem::transmute(msg());
                        let n = fl(d, s(b"length\0"));
                        let b = fb(d, s(b"bytes\0"));
                        if !b.is_null() && n > 0 && n < 40_000_000 {
                            out = Some(PbInhalt::Bild(std::slice::from_raw_parts(b, n).to_vec()));
                        }
                        break;
                    }
                }
            }
            objc_autoreleasePoolPop(pool);
            out
        }
    }
    pub fn pb_text_setzen(t: &str) -> bool {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let pb = m0(k(b"NSPasteboard\0"), b"generalPasteboard\0");
            let mut ok = false;
            if !pb.is_null() {
                let fc: extern "C" fn(Id, Id) -> isize = std::mem::transmute(msg());
                fc(pb, s(b"clearContents\0"));
                let f: extern "C" fn(Id, Id, Id, Id) -> i8 = std::mem::transmute(msg());
                ok = f(
                    pb,
                    s(b"setString:forType:\0"),
                    ns(t),
                    ns("public.utf8-plain-text"),
                ) != 0;
            }
            objc_autoreleasePoolPop(pool);
            ok
        }
    }
    pub fn pb_bild_setzen(pfad: &str) -> bool {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let pb = m0(k(b"NSPasteboard\0"), b"generalPasteboard\0");
            let d = m1(k(b"NSData\0"), b"dataWithContentsOfFile:\0", ns(pfad));
            let mut ok = false;
            if !pb.is_null() && !d.is_null() {
                let fc: extern "C" fn(Id, Id) -> isize = std::mem::transmute(msg());
                fc(pb, s(b"clearContents\0"));
                let f: extern "C" fn(Id, Id, Id, Id) -> i8 = std::mem::transmute(msg());
                ok = f(pb, s(b"setData:forType:\0"), d, ns("public.png")) != 0;
            }
            objc_autoreleasePoolPop(pool);
            ok
        }
    }

    // ---- Arbeitsplatz: Apps und Fenster (Bedienungshilfen / AX) -----------
    /// Freigabe "Bedienungshilfen"? Mit frage=true zeigt macOS einmalig den Dialog.
    pub fn ax_vertraut(frage: bool) -> bool {
        unsafe {
            if !frage {
                return AXIsProcessTrusted();
            }
            let pool = objc_autoreleasePoolPush();
            let fb: extern "C" fn(Id, Id, i8) -> Id = std::mem::transmute(msg());
            let ja = fb(k(b"NSNumber\0"), s(b"numberWithBool:\0"), 1);
            let fd: extern "C" fn(Id, Id, Id, Id) -> Id = std::mem::transmute(msg());
            let opt = fd(
                k(b"NSDictionary\0"),
                s(b"dictionaryWithObject:forKey:\0"),
                ja,
                ns("AXTrustedCheckOptionPrompt"),
            );
            let r = AXIsProcessTrustedWithOptions(opt);
            objc_autoreleasePoolPop(pool);
            r
        }
    }
    unsafe fn ax_fenster_lesen(pid: i32) -> Vec<[f64; 4]> {
        let mut out = Vec::new();
        let el = AXUIElementCreateApplication(pid);
        if el.is_null() {
            return out;
        }
        let mut wins: Id = std::ptr::null_mut();
        if AXUIElementCopyAttributeValue(el, ns("AXWindows"), &mut wins) == 0 && !wins.is_null() {
            for i in 0..CFArrayGetCount(wins) {
                let w = CFArrayGetValueAtIndex(wins, i);
                let mut mini: Id = std::ptr::null_mut();
                let mut minimiert = false;
                if AXUIElementCopyAttributeValue(w, ns("AXMinimized"), &mut mini) == 0
                    && !mini.is_null()
                {
                    let fb: extern "C" fn(Id, Id) -> i8 = std::mem::transmute(msg());
                    minimiert = fb(mini, s(b"boolValue\0")) != 0;
                    CFRelease(mini);
                }
                if minimiert {
                    continue;
                }
                let mut pv: Id = std::ptr::null_mut();
                let mut sv: Id = std::ptr::null_mut();
                let a = AXUIElementCopyAttributeValue(w, ns("AXPosition"), &mut pv);
                let b = AXUIElementCopyAttributeValue(w, ns("AXSize"), &mut sv);
                let mut p = [0f64; 2];
                let mut z = [0f64; 2];
                if a == 0
                    && b == 0
                    && !pv.is_null()
                    && !sv.is_null()
                    && AXValueGetValue(pv, 1, p.as_mut_ptr() as *mut c_void)
                    && AXValueGetValue(sv, 2, z.as_mut_ptr() as *mut c_void)
                    && z[0] > 40.0
                    && z[1] > 40.0
                {
                    out.push([p[0], p[1], z[0], z[1]]);
                }
                if !pv.is_null() {
                    CFRelease(pv);
                }
                if !sv.is_null() {
                    CFRelease(sv);
                }
            }
            CFRelease(wins);
        }
        CFRelease(el);
        out
    }
    extern "C" {
        fn AXUIElementSetMessagingTimeout(element: Id, timeout: f32) -> i32;
    }
    pub fn intelligence_context(with_title: bool) -> Option<(String, Option<String>)> {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let ws = m0(k(b"NSWorkspace\0"), b"sharedWorkspace\0");
            let app = m0(ws, b"frontmostApplication\0");
            if app.is_null() || pid_von(app) == std::process::id() as i32 {
                objc_autoreleasePoolPop(pool);
                return None;
            }
            let name = text(m0(app, b"localizedName\0")).unwrap_or_default();
            let mut title = None;
            if with_title && AXIsProcessTrusted() {
                let el = AXUIElementCreateApplication(pid_von(app));
                if !el.is_null() {
                    AXUIElementSetMessagingTimeout(el, 0.2);
                    let mut window: Id = std::ptr::null_mut();
                    if AXUIElementCopyAttributeValue(el, ns("AXFocusedWindow"), &mut window) == 0
                        && !window.is_null()
                    {
                        let mut value: Id = std::ptr::null_mut();
                        if AXUIElementCopyAttributeValue(window, ns("AXTitle"), &mut value) == 0
                            && !value.is_null()
                        {
                            title = text(value);
                            CFRelease(value);
                        }
                        CFRelease(window);
                    }
                    CFRelease(el);
                }
            }
            objc_autoreleasePoolPop(pool);
            Some((name, title))
        }
    }
    /// Normale laufende Apps (ohne Noki) mit ihren Fenstern: (Name, Bundle, Pfad, Fenster).
    pub fn apps_mit_fenstern(ax: bool) -> Vec<(String, String, String, Vec<[f64; 4]>)> {
        let mut out = Vec::new();
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let ws = m0(k(b"NSWorkspace\0"), b"sharedWorkspace\0");
            let arr = m0(ws, b"runningApplications\0");
            let fpol: extern "C" fn(Id, Id) -> isize = std::mem::transmute(msg());
            let eigen = std::process::id() as i32;
            for i in 0..anzahl(arr) {
                let a = an_stelle(arr, i);
                if a.is_null() || fpol(a, s(b"activationPolicy\0")) != 0 {
                    continue;
                }
                let pid = pid_von(a);
                if pid == eigen {
                    continue;
                }
                let name = text(m0(a, b"localizedName\0")).unwrap_or_default();
                let bundle = text(m0(a, b"bundleIdentifier\0")).unwrap_or_default();
                let pfad = text(m0(m0(a, b"bundleURL\0"), b"path\0")).unwrap_or_default();
                let fen = if ax {
                    ax_fenster_lesen(pid)
                } else {
                    Vec::new()
                };
                if ax && fen.is_empty() {
                    continue;
                }
                if !ax && bundle == "com.apple.finder" {
                    continue;
                }
                out.push((name, bundle, pfad, fen));
            }
            objc_autoreleasePoolPop(pool);
        }
        out
    }
    pub fn pid_fuer_bundle(b: &str) -> Option<i32> {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let arr = m1(
                k(b"NSRunningApplication\0"),
                b"runningApplicationsWithBundleIdentifier:\0",
                ns(b),
            );
            let r = if anzahl(arr) > 0 {
                let a = an_stelle(arr, 0);
                if a.is_null() {
                    None
                } else {
                    Some(pid_von(a))
                }
            } else {
                None
            };
            objc_autoreleasePoolPop(pool);
            r
        }
    }
    /// Every running instance of a bundle (`open -n` second instances).
    pub fn pids_fuer_bundle(b: &str) -> Vec<i32> {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let arr = m1(k(b"NSRunningApplication\0"), b"runningApplicationsWithBundleIdentifier:\0", ns(b));
            let mut v = Vec::new();
            for i in 0..anzahl(arr) {
                let a = an_stelle(arr, i);
                if !a.is_null() { v.push(pid_von(a)); }
            }
            objc_autoreleasePoolPop(pool);
            v
        }
    }
    pub fn app_fuer_pid(gesucht: i32) -> Option<(String, String, String)> {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let ws = m0(k(b"NSWorkspace\0"), b"sharedWorkspace\0");
            let arr = m0(ws, b"runningApplications\0");
            let mut out = None;
            for i in 0..anzahl(arr) {
                let a = an_stelle(arr, i);
                if !a.is_null() && pid_von(a) == gesucht {
                    out = Some((
                        text(m0(a, b"localizedName\0")).unwrap_or_default(),
                        text(m0(a, b"bundleIdentifier\0")).unwrap_or_default(),
                        text(m0(m0(a, b"bundleURL\0"), b"path\0")).unwrap_or_default(),
                    ));
                    break;
                }
            }
            objc_autoreleasePoolPop(pool);
            out
        }
    }
    /// Fenster einer App der Reihe nach auf die gespeicherten Lagen setzen
    /// (wartet bis zu warte_ms, bis die App Fenster hat). Rueckgabe: gesetzt.
    pub fn fenster_setzen(pid: i32, ziele: &[[f64; 4]], warte_ms: u64) -> usize {
        let mut gesetzt = 0;
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let el = AXUIElementCreateApplication(pid);
            if !el.is_null() {
                let t0 = std::time::Instant::now();
                loop {
                    let mut wins: Id = std::ptr::null_mut();
                    let mut fertig = false;
                    if AXUIElementCopyAttributeValue(el, ns("AXWindows"), &mut wins) == 0
                        && !wins.is_null()
                    {
                        let n = CFArrayGetCount(wins);
                        if n > 0 {
                            for (i, z) in ziele.iter().enumerate().take(n as usize) {
                                let w = CFArrayGetValueAtIndex(wins, i as isize);
                                let p = [z[0], z[1]];
                                let g = [z[2], z[3]];
                                let pv = AXValueCreate(1, p.as_ptr() as *const c_void);
                                let sv = AXValueCreate(2, g.as_ptr() as *const c_void);
                                let a = AXUIElementSetAttributeValue(w, ns("AXPosition"), pv);
                                let b = AXUIElementSetAttributeValue(w, ns("AXSize"), sv);
                                let _ = AXUIElementSetAttributeValue(w, ns("AXPosition"), pv); // nach der Groesse nochmals
                                if a == 0 || b == 0 {
                                    gesetzt += 1;
                                }
                                if !pv.is_null() {
                                    CFRelease(pv);
                                }
                                if !sv.is_null() {
                                    CFRelease(sv);
                                }
                            }
                            fertig = true;
                        }
                        CFRelease(wins);
                    }
                    if fertig || t0.elapsed().as_millis() as u64 > warte_ms {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                CFRelease(el);
            }
            objc_autoreleasePoolPop(pool);
        }
        gesetzt
    }

    /// Aktueller Pfad zum Lesezeichen, ohne Rueckfrage-Dialog (WithoutUI).
    pub fn aufloesen(bm: &[u8]) -> Option<String> {
        if bm.is_empty() {
            return None;
        }
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let fd: extern "C" fn(Id, Id, *const u8, usize) -> Id = std::mem::transmute(msg());
            let d = fd(
                k(b"NSData\0"),
                s(b"dataWithBytes:length:\0"),
                bm.as_ptr(),
                bm.len(),
            );
            let mut out = None;
            if !d.is_null() {
                let fr: extern "C" fn(Id, Id, Id, u64, Id, *mut i8, *mut Id) -> Id =
                    std::mem::transmute(msg());
                let mut alt: i8 = 0;
                let mut err: Id = std::ptr::null_mut();
                let u = fr(k(b"NSURL\0"),
                           s(b"URLByResolvingBookmarkData:options:relativeToURL:bookmarkDataIsStale:error:\0"),
                           d, 1 << 8, std::ptr::null_mut(), &mut alt, &mut err);
                if !u.is_null() {
                    let fp: extern "C" fn(Id, Id) -> Id = std::mem::transmute(msg());
                    let p = fp(u, s(b"path\0"));
                    if !p.is_null() {
                        let fu: extern "C" fn(Id, Id) -> *const c_char = std::mem::transmute(msg());
                        let c = fu(p, s(b"UTF8String\0"));
                        if !c.is_null() {
                            out = Some(CStr::from_ptr(c).to_string_lossy().into_owned());
                        }
                    }
                }
            }
            objc_autoreleasePoolPop(pool);
            out
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod lesezeichen {
    pub fn erstellen(_: &str) -> Option<Vec<u8>> {
        None
    }
    pub fn apps_fuer_datei(_: &str) -> Vec<(String, String, bool)> {
        Vec::new()
    }
    pub fn in_papierkorb(_: &str) -> bool {
        false
    }
    pub fn aufloesen(_: &[u8]) -> Option<String> {
        None
    }
    pub fn vorne_bundle() -> Option<String> {
        None
    }
    pub enum PbInhalt {
        Text(String, String),
        Bild(Vec<u8>),
    }
    pub fn pb_zaehler() -> i64 {
        0
    }
    pub fn pb_lesen() -> Option<PbInhalt> {
        None
    }
    pub fn pb_text_setzen(_: &str) -> bool {
        false
    }
    pub fn pb_bild_setzen(_: &str) -> bool {
        false
    }
    pub fn ax_vertraut(_: bool) -> bool {
        false
    }
    pub fn apps_mit_fenstern(_: bool) -> Vec<(String, String, String, Vec<[f64; 4]>)> {
        Vec::new()
    }
    pub fn pid_fuer_bundle(_: &str) -> Option<i32> {
        None
    }
    pub fn pids_fuer_bundle(_: &str) -> Vec<i32> { Vec::new() }
    pub fn fenster_setzen(_: i32, _: &[[f64; 4]], _: u64) -> usize {
        0
    }
}

/// Ist eine Maustaste gedrueckt? Rein lesend - der Klick selbst laeuft
/// unveraendert an sein Ziel (Finder, Chrome, Schreibtisch). Genau das
/// verlangt Abschnitt 8: beobachten, nicht abfangen.
#[cfg(target_os = "macos")]
fn maus_gedrueckt() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceButtonState(state: u32, button: u32) -> bool;
    }
    // 1 = kCGEventSourceStateCombinedSessionState, 0 = linke Taste.
    unsafe { CGEventSourceButtonState(1, 0) || CGEventSourceButtonState(1, 1) }
}

#[cfg(not(target_os = "macos"))]
fn maus_gedrueckt() -> bool {
    false
}

/// Last time the page confirmed `drag` (panel tick every 250 ms, pickup).
static DRAG_BESTAETIGT_MS: AtomicU64 = AtomicU64::new(0);
/// The character overlay's REAL click-through state (true = ignores the
/// mouse). Dead-button root cause (Arbeitsplatz-Fokus, 2026-10-03): the mouse
/// watcher kept its own copy, while drag/utility/freeze/startup wrote the
/// window directly. A drag true->false inside one 30 ms tick (trackpad tap
/// on Noki, panel closing right after its 250 ms re-assert, two IPC calls
/// flushed together after a main-thread stall) left the overlay opaque while
/// the watcher still believed it was click-through: the invisible
/// full-screen overlay then took every click above the Settings window. One
/// writer, one state - no second copy.
static OVERLAY_DURCHLAESSIG: AtomicBool = AtomicBool::new(true);
fn overlay_durchlaessig(win: &WebviewWindow, ignorieren: bool) {
    let _ = win.set_ignore_cursor_events(ignorieren);
    OVERLAY_DURCHLAESSIG.store(ignorieren, Ordering::SeqCst);
}
/// UI hang root cause (2026-10-03): `drag` makes the full-screen character
/// overlay take EVERY click on the whole screen (popover "click beside
/// closes"). Open Werk/Schnell panels re-assert it every 250 ms. When the
/// page stopped running (overlay on a hidden Space, page busy, panel state
/// gone) the last `true` stayed forever: nothing anywhere was clickable or
/// typeable until Settings/panels were reopened. Now a lease: it counts only
/// while a button is held or the page confirmed it within 800 ms.
fn drag_gilt(drag: bool) -> bool {
    drag && (maus_gedrueckt() || jetzt_epoch_ms().saturating_sub(DRAG_BESTAETIGT_MS.load(Ordering::Relaxed)) <= 800)
}
fn spawn_mouse_watcher(
    app: tauri::AppHandle,
    hit_store: Arc<std::sync::RwLock<NokiHitZustand>>,
    stop: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        let mut last_x = -9999.0;
        let mut last_y = -9999.0;
        let mut taste_vorher = false;
        let mut sperre_vorher = (false, false, 0usize);
        while !stop.load(Ordering::Relaxed) {
            if let Some((x, y)) = maus_schirm_position() {
                if (x - last_x).abs() >= 1.0 || (y - last_y).abs() >= 1.0 {
                    last_x = x;
                    last_y = y;
                    let _ = app.emit("noki://maus", serde_json::json!({ "x": x, "y": y }));
                }

                // Grosse Miniatur + Druck NEBEN sie = zurueck auf kompakt.
                // Gefragt wird nur, solange sie gross ist, und nur auf der
                // Flanke: ein gehaltener Knopf ist ein Klick, nicht viele.
                // Es entsteht kein zweiter Beobachter - diese Schleife gab es
                // schon, sie beantwortet jetzt eine Frage mehr.
                // Waehrend eines Fernvorgangs klickt NOKI selbst (am echten
                // Fenster) - das ist kein Klick des Nutzers neben die Miniatur.
                if VORSCHAU_GROSS.load(Ordering::Relaxed)
                    && !vorschau::fern_beschaeftigt()
                    && !eigener_wechsel_laeuft()
                {
                    let taste = maus_gedrueckt();
                    if taste && !taste_vorher {
                        // Gefragt wird die FLAECHE der Miniatur, nicht die
                        // (absichtlich leere) Trefferzone des Overlays.
                        let drin = hit_store
                            .read()
                            .map(|h| {
                                h.fw > 0
                                    && h.fh > 0
                                    && x >= h.fx as f64
                                    && x <= (h.fx + h.fw) as f64
                                    && y >= h.fy as f64
                                    && y <= (h.fy + h.fh) as f64
                            })
                            .unwrap_or(false);
                        if !drin {
                            let _ = app.emit("noki://vorschau_kompakt", serde_json::json!({}));
                        }
                    }
                    taste_vorher = taste;
                } else {
                    taste_vorher = false;
                }

                if let Ok(hit) = hit_store.read() {
                    // Ausserhalb der Figur bleibt das bildschirmgrosse
                    // Character-Overlay durchklickbar. Nur Pickup/Freeze
                    // benoetigen echte Pointer-Ereignisse im Overlay.
                    let drag = drag_gilt(hit.drag);
                    let sperre = (drag, FREEZE_SPERRE.load(Ordering::Relaxed), hit.zonen.len());
                    if sperre != sperre_vorher {
                        // State changes only - no polling spam.
                        virtual_workspace::trace(&format!(
                            "INPUT_BLOCK_STATE overlay={} drag={} drag_raw={} catcher_screenwide={} freeze={} panels={} settings_zone={} window=main",
                            if sperre.0 || sperre.1 { "screenwide" } else { "zones" }, sperre.0, hit.drag, sperre.0, sperre.1, sperre.2, hit.ew > 0));
                        sperre_vorher = sperre;
                    }
                    let inside = FREEZE_SPERRE.load(Ordering::Relaxed)
                        || drag
                        || hit.zonen.iter().any(|z| x >= z[0] as f64 && x <= (z[0] + z[2]) as f64 && y >= z[1] as f64 && y <= (z[1] + z[3]) as f64)
                        || (hit.w > 0
                            && hit.h > 0
                            && x >= (hit.x - 14) as f64
                            && x <= (hit.x + hit.w + 14) as f64
                            && y >= (hit.y - 14) as f64
                            && y <= (hit.y + hit.h + 14) as f64)
                        || (hit.vw > 0
                            && hit.vh > 0
                            && x >= hit.vx as f64
                            && x <= (hit.vx + hit.vw) as f64
                            && y >= hit.vy as f64
                            && y <= (hit.vy + hit.vh) as f64)
                        || (hit.sw > 0
                            && hit.sh > 0
                            && x >= hit.sx as f64
                            && x <= (hit.sx + hit.sw) as f64
                            && y >= hit.sy as f64
                            && y <= (hit.sy + hit.sh) as f64)
                        || (hit.bw > 0
                            && hit.bh > 0
                            && x >= hit.bx as f64
                            && x <= (hit.bx + hit.bw) as f64
                            && y >= hit.by as f64
                            && y <= (hit.by + hit.bh) as f64)
                        || (hit.ew > 0
                            && hit.eh > 0
                            && x >= hit.ex as f64
                            && x <= (hit.ex + hit.ew) as f64
                            && y >= hit.ey as f64
                            && y <= (hit.ey + hit.eh) as f64);
                    // Compared against the window's REAL state (any writer),
                    // never a private copy of it.
                    if inside == OVERLAY_DURCHLAESSIG.load(Ordering::SeqCst) {
                        if let Some(win) = app.get_webview_window(FENSTER) {
                            overlay_durchlaessig(&win, !inside);
                        }
                    }
                }
            }
            thread::sleep(Duration::from_millis(30)); // ~33 Hz
        }
    });
}

// =====================================================================
//  4 · FENSTERORT  (Abschnitt 17)
//
//  Beim Verbergen wird der Ort gemerkt, beim Wiederkommen geprueft. Ein
//  Ort, der durch eine Aufloesungs- oder Bildschirmaenderung ungueltig
//  geworden ist, wird auf den naechsten sichtbaren Bereich gezogen —
//  Noki erscheint nie ausserhalb des Sichtbaren.
// =====================================================================
fn ort_datei(app: &tauri::AppHandle) -> Option<PathBuf> {
    let dir = app.path().app_config_dir().ok()?;
    let _ = fs::create_dir_all(&dir);
    Some(dir.join("fensterort.json"))
}

// =====================================================================
//  ALLE SCHREIBTISCHE  (Abschnitt 29)
//
//  Ein NSWindow gehoert normalerweise zu dem Space, auf dem es erzeugt
//  wurde. Genau deshalb war Noki bisher nur auf einem Schreibtisch zu
//  sehen. macOS kennt dafuer das Collection-Behavior-Bit
//  NSWindowCollectionBehaviorCanJoinAllSpaces; tao setzt ausschliesslich
//  DIESES Bit und laesst alle uebrigen Flags stehen
//  (set_visible_on_all_workspaces). Es gibt also keine zweite Instanz,
//  kein Polling und keine Space-Erkennung.
//
//  WICHTIG: Space-Zugehoerigkeit und Fensterebene sind zwei
//  verschiedene Dinge. set_always_on_top/-bottom aendern unter macOS nur
//  setLevel und fassen collectionBehavior nicht an — FRONT/BEHIND
//  bleibt davon unberuehrt und umgekehrt.
fn einstellungen_datei(app: &tauri::AppHandle) -> Option<PathBuf> {
    let dir = app.path().app_config_dir().ok()?;
    let _ = fs::create_dir_all(&dir);
    Some(dir.join("einstellungen.json"))
}

/// Liest den gespeicherten Wert aus dem Dateiinhalt.
///
/// Fehlt die Datei oder der Schluessel, gilt `true`: Noki ist als
/// Schreibtischbegleiter gedacht, und ein Begleiter, der beim ersten
/// Space-Wechsel verschwindet, ist keiner. Ein ausdruecklich
/// gespeichertes `false` wird selbstverstaendlich respektiert.
pub fn alle_spaces_aus_json(roh: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(roh)
        .ok()
        .and_then(|v| v.get("alle_schreibtische").and_then(|b| b.as_bool()))
        .unwrap_or(true)
}

fn alle_spaces_lesen(app: &tauri::AppHandle) -> bool {
    einstellungen_datei(app)
        .and_then(|d| fs::read_to_string(d).ok())
        .map(|roh| alle_spaces_aus_json(&roh))
        .unwrap_or(true)
}

fn alle_spaces_schreiben(app: &tauri::AppHandle, an: bool) {
    einstellungen_setzen(app, "alle_schreibtische", serde_json::Value::Bool(an));
}

/// Einen Schluessel in der vorhandenen Einstellungsdatei setzen, ohne die
/// uebrigen zu verlieren. Frueher schrieb jeder Schreiber die ganze Datei
/// mit genau seinem einen Feld — der jeweils andere Wert war danach weg.
fn einstellungen_setzen(app: &tauri::AppHandle, schluessel: &str, wert: serde_json::Value) {
    let Some(datei) = einstellungen_datei(app) else {
        return;
    };
    let alt = fs::read_to_string(&datei).unwrap_or_default();
    let _ = fs::write(datei, einstellungen_mischen(&alt, schluessel, wert));
}

/// Der reine Teil davon: alter Dateiinhalt + ein Schluessel = neuer Inhalt.
/// Unlesbarer oder fehlender Inhalt beginnt ein frisches Objekt.
pub fn einstellungen_mischen(alt: &str, schluessel: &str, wert: serde_json::Value) -> String {
    let mut roh = serde_json::from_str::<serde_json::Value>(alt)
        .ok()
        .filter(|v| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    if let Some(o) = roh.as_object_mut() {
        o.insert(schluessel.to_string(), wert);
    }
    roh.to_string()
}

/// Der WUNSCH des Nutzers, nicht der momentane Fensterzustand.
///
/// `true`  = "Noki zeigen" wurde zuletzt gewaehlt (oder noch nie etwas),
/// `false` = "Noki verbergen" bzw. das Fenster wurde geschlossen.
///
/// Fehlt der Schluessel, gilt `true`: ein Schreibtischbegleiter, der nach
/// einem Neustart erst wieder eingeschaltet werden muss, ist keiner.
/// Die vom Nutzer gewaehlte NATIVE Fensterebene des Character-Overlays.
///
/// Grundstellung ist "vorn": Noki ist ein Schreibtischbegleiter, der vor
/// den Programmfenstern lebt. Frueher stand hier "normal" — eine dritte
/// Stellung, die das Menue gar nicht anbieten kann (es kennt nur vorn und
/// hinten). Das Overlay lag damit beim Start auf der gewoehnlichen
/// Fensterebene, verschwand hinter jedem Fenster, und der Menuetext
/// behauptete trotzdem, Noki sei vorn.
pub fn ebene_aus_json(roh: &str) -> &'static str {
    match serde_json::from_str::<serde_json::Value>(roh)
        .ok()
        .and_then(|v| v.get("ebene").and_then(|b| b.as_str().map(str::to_owned)))
        .as_deref()
    {
        Some("hinten") => "hinten",
        _ => "vorn",
    }
}

fn ebene_lesen(app: &tauri::AppHandle) -> &'static str {
    einstellungen_datei(app)
        .and_then(|d| fs::read_to_string(d).ok())
        .map(|roh| ebene_aus_json(&roh))
        .unwrap_or("vorn")
}

pub fn sicht_wunsch_aus_json(roh: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(roh)
        .ok()
        .and_then(|v| v.get("noki_sichtbar").and_then(|b| b.as_bool()))
        .unwrap_or(true)
}

fn sicht_wunsch_lesen(app: &tauri::AppHandle) -> bool {
    einstellungen_datei(app)
        .and_then(|d| fs::read_to_string(d).ok())
        .map(|roh| sicht_wunsch_aus_json(&roh))
        .unwrap_or(true)
}

/// EINZIGE Stelle, an der sich der Nutzerwunsch aendert. Space-, Vollbild-
/// und Programmwechsel rufen sie NIE — die verschieben nur das Fenster.
fn sicht_wunsch_setzen(app: &tauri::AppHandle, gezeigt: bool) {
    NUTZER_VERBORGEN.store(!gezeigt, Ordering::Relaxed);
    if gezeigt {
        NOKI_GEZEIGT.store(true, Ordering::Relaxed);
    }
    einstellungen_setzen(app, "noki_sichtbar", serde_json::Value::Bool(gezeigt));
    eprintln!(
        "[SICHT] Nutzerwunsch: {}",
        if gezeigt { "zeigen" } else { "verbergen" }
    );
}

/// Wendet die Einstellung auf das Fenster an. Nur das eine Bit — die
/// Ebene bleibt, wie sie ist.
fn alle_spaces_anwenden(win: &WebviewWindow, an: bool) {
    // NIE auf allen Schreibtischen zugleich (an = "Noki folgt dem Nutzer").
    // Das Fenster gehoert EINEM Schreibtisch; Vollbild regelt der Abgleich.
    let _ = an;
    let _ = win.set_visible_on_all_workspaces(false);
    let _ = vollbild_space_abgleich(win);
}

// VOLLBILD: ueber dem Vollbild-Space einer ANDEREN App erscheint Nokis
// (tao-)Fenster nur mit FullScreenAuxiliary (1<<8) UND CanJoinAllApplications
// (1<<18, macOS 13+) — gemessen: ohne 1<<18 bleibt es trotz 1<<8 draussen.
// Standard: AUS — Noki bleibt im Vollbild verborgen. Erst "Vollbild > Noki
// zeigen" setzt beide Bits (plus CanJoinAllSpaces).
// Nur fuer diese Sitzung: nie gespeichert, jeder App-Start beginnt mit AUS.
// Nokis EINER Aufenthalts-Space (Schreibtisch ODER Vollbild, 0 = noch
// unbekannt). Jeder echte Wechsel weg davon wird geflogen; nur "Noki
// zeigen" setzt ihn direkt auf den aktiven Space.
static NOKI_SPACE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(target_os = "macos")]
fn vollbild_bit(win: &WebviewWindow, an: bool) {
    use std::ffi::c_void;
    #[link(name = "objc")]
    extern "C" {
        fn sel_registerName(n: *const std::os::raw::c_char) -> *const c_void;
        fn objc_msgSend();
    }
    const AUX: usize = (1 << 8) | (1 << 18);
    let Ok(nw) = win.ns_window() else { return };
    unsafe {
        let f = objc_msgSend as unsafe extern "C" fn();
        let sel = |n: &[u8]| sel_registerName(n.as_ptr() as *const _);
        let get: unsafe extern "C" fn(*mut c_void, *const c_void) -> usize = std::mem::transmute(f);
        let set: unsafe extern "C" fn(*mut c_void, *const c_void, usize) = std::mem::transmute(f);
        let aus: unsafe extern "C" fn(*mut c_void, *const c_void, *mut c_void) =
            std::mem::transmute(f);
        let vor: unsafe extern "C" fn(*mut c_void, *const c_void) = std::mem::transmute(f);
        let alt = get(nw, sel(b"collectionBehavior\0"));
        let neu = if an { alt | AUX } else { alt & !AUX };
        if neu == alt {
            return;
        }
        set(nw, sel(b"setCollectionBehavior:\0"), neu);
        // Neu einordnen, damit die Aenderung im AKTUELLEN Space sofort gilt
        // (ohne die App zu aktivieren — der Vollbild-App bleibt der Fokus).
        // AUS: einmal beigetretene Vollbild-Spaces behaelt WindowServer auch
        // nach dem Loeschen der Bits (gemessen) — dort ausdruecklich
        // herausnehmen. Nur Spaces vom Typ Vollbild (4), nie Schreibtische.
        // AN: neu einordnen, damit er den aktuellen Vollbild-Space betritt.
        // AUS: NICHT neu einordnen — das legte ihn gemessen sofort wieder hinein.
        if an && win.is_visible().unwrap_or(false) {
            aus(nw, sel(b"orderOut:\0"), std::ptr::null_mut());
            vor(nw, sel(b"orderFrontRegardless\0"));
        }
        // Solange CanJoinAllSpaces gesetzt ist, haelt WindowServer die
        // Vollbild-Spaces fest (gemessen) — kurz aus, entfernen, wieder an.
        if !an {
            set(nw, sel(b"setCollectionBehavior:\0"), neu & !1);
            vollbild_spaces_verlassen(get(nw, sel(b"windowNumber\0")) as i64);
            set(nw, sel(b"setCollectionBehavior:\0"), neu);
        }
    }
}

// Private CGS/SkyLight-Aufrufe NUR hier, per dlsym geladen: fehlt ein
// Symbol (kuenftiges macOS), liefern die Funktionen None/false — kein Crash.
#[cfg(target_os = "macos")]
mod cgs {
    use std::ffi::c_void;
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    type DesktopTopology = (String, Vec<crate::arbeitsplatz::Schreibtisch>);

    struct PendingTopology {
        signature: String,
        first_seen: Instant,
        observations: u8,
    }

    #[derive(Default)]
    struct TopologyState {
        published: Option<DesktopTopology>,
        pending_shrink: Option<PendingTopology>,
        /// Last time the system topology was actually READ (any result).
        gelesen: Option<Instant>,
    }

    static TOPOLOGY: OnceLock<Mutex<TopologyState>> = OnceLock::new();
    extern "C" {
        fn dlopen(p: *const std::os::raw::c_char, m: i32) -> *mut c_void;
        fn dlsym(h: *mut c_void, s: *const std::os::raw::c_char) -> *mut c_void;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFNumberCreate(a: *const c_void, t: isize, v: *const c_void) -> *const c_void;
        fn CFNumberGetValue(n: *const c_void, t: i32, v: *mut c_void) -> u8;
        fn CFArrayCreate(
            a: *const c_void,
            v: *const *const c_void,
            n: isize,
            cb: *const c_void,
        ) -> *const c_void;
        fn CFArrayGetCount(a: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(a: *const c_void, i: isize) -> *const c_void;
        fn CFRelease(o: *const c_void);
        fn CFDictionaryGetValue(d: *const c_void, k: *const c_void) -> *const c_void;
        fn CFStringCreateWithCString(
            a: *const c_void,
            s: *const std::os::raw::c_char,
            enc: u32,
        ) -> *const c_void;
        fn CFStringGetCString(s: *const c_void, b: *mut u8, n: isize, enc: u32) -> u8;
        fn CFGetTypeID(o: *const c_void) -> usize;
        fn CFStringGetTypeID() -> usize;
        static kCFTypeArrayCallBacks: c_void;
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGWindowListCopyWindowInfo(opt: u32, rel: u32) -> *const c_void;
    }

    /// CFString -> String. Leer, wenn es keiner ist (der erste Schreibtisch
    /// hat gemessen gar keine uuid).
    unsafe fn text(o: *const c_void) -> String {
        if o.is_null() || CFGetTypeID(o) != CFStringGetTypeID() {
            return String::new();
        }
        let mut b = [0u8; 128];
        if CFStringGetCString(o, b.as_mut_ptr(), b.len() as isize, 0x0800_0100) == 0 {
            return String::new();
        }
        let n = b.iter().position(|c| *c == 0).unwrap_or(0);
        String::from_utf8_lossy(&b[..n]).into_owned()
    }
    unsafe fn schluessel(name: &[u8]) -> *const c_void {
        CFStringCreateWithCString(std::ptr::null(), name.as_ptr() as *const _, 0x0800_0100)
    }
    unsafe fn zahl(o: *const c_void) -> i64 {
        let mut v: i64 = 0;
        if !o.is_null() {
            CFNumberGetValue(o, 4, &mut v as *mut i64 as *mut c_void);
        }
        v
    }

    /// Alle echten Schreibtische des Bildschirms, auf dem der aktive Space
    /// liegt, samt Belegung. `uuid` ist die einzige Kennung, die eine Sitzung
    /// ueberdauert; `ManagedSpaceID` ist fluechtig.
    fn desktops_raw() -> Option<DesktopTopology> {
        let f: extern "C" fn(i32) -> *const c_void =
            unsafe { std::mem::transmute(sym(b"CGSCopyManagedDisplaySpaces\0")?) };
        let aktiv = aktiver_space().map(|(a, _)| a).unwrap_or(0);
        let belegung = fremde_fenster_pro_space();
        unsafe {
            let d = f(cid()?);
            if d.is_null() {
                return None;
            }
            let k_sp = schluessel(b"Spaces\0");
            let k_id = schluessel(b"ManagedSpaceID\0");
            let k_ty = schluessel(b"type\0");
            let k_uu = schluessel(b"uuid\0");
            let k_di = schluessel(b"Display Identifier\0");
            let mut gefunden: Option<(String, Vec<crate::arbeitsplatz::Schreibtisch>)> = None;
            let mut erster: Option<(String, Vec<crate::arbeitsplatz::Schreibtisch>)> = None;
            for i in 0..CFArrayGetCount(d) {
                let disp = CFArrayGetValueAtIndex(d, i);
                let display = text(CFDictionaryGetValue(disp, k_di));
                let sp = CFDictionaryGetValue(disp, k_sp);
                if sp.is_null() {
                    continue;
                }
                let mut liste = vec![];
                let mut hat_aktiven = false;
                for j in 0..CFArrayGetCount(sp) {
                    let e = CFArrayGetValueAtIndex(sp, j);
                    let id = zahl(CFDictionaryGetValue(e, k_id)) as u64;
                    let typ = zahl(CFDictionaryGetValue(e, k_ty)) as i32;
                    let mut uuid = text(CFDictionaryGetValue(e, k_uu));
                    // macOS omits the UUID field for the primary Desktop on
                    // some releases.  Its ManagedSpaceID is session-local,
                    // so it must never become the persisted identity (that
                    // used to make Desktop 1 disappear after filtering).
                    // There is exactly one primary Desktop per display; the
                    // display identity therefore gives it a stable key while
                    // the live list resolves the current numeric ID.
                    if typ == crate::arbeitsplatz::TYP_SCHREIBTISCH && uuid.is_empty() {
                        uuid = format!("primary-desktop:{display}");
                    }
                    if id == aktiv {
                        hat_aktiven = true;
                    }
                    liste.push(crate::arbeitsplatz::Schreibtisch {
                        id,
                        uuid,
                        typ,
                        fremde_fenster: belegung
                            .iter()
                            .find(|(s, _)| *s == id)
                            .map(|(_, n)| *n)
                            .unwrap_or(0),
                    });
                }
                if erster.is_none() {
                    erster = Some((display.clone(), liste.clone()));
                }
                if hat_aktiven {
                    gefunden = Some((display, liste));
                }
            }
            for k in [k_sp, k_id, k_ty, k_uu, k_di] {
                CFRelease(k);
            }
            CFRelease(d);
            gefunden.or(erster)
        }
    }

    fn normal_signature(display: &str, list: &[crate::arbeitsplatz::Schreibtisch]) -> String {
        let mut entries = list.iter()
            .filter(|s| s.typ == crate::arbeitsplatz::TYP_SCHREIBTISCH)
            .map(|s| format!("{}:{}", s.uuid, s.id))
            .collect::<Vec<_>>();
        entries.sort();
        format!("{display}|{}", entries.join(","))
    }

    fn topology_valid(display: &str, list: &[crate::arbeitsplatz::Schreibtisch]) -> bool {
        if display.is_empty() || list.is_empty() { return false; }
        let mut ids = HashSet::new();
        let mut normal = 0usize;
        for s in list {
            if s.id == 0 || !ids.insert(s.id) { return false; }
            if s.typ == crate::arbeitsplatz::TYP_SCHREIBTISCH {
                normal += 1;
                if !crate::arbeitsplatz::persistente_identitaet(&s.uuid) { return false; }
            }
        }
        normal > 0
    }

    /// Publish only coherent topology snapshots. A smaller normal-Desktop
    /// set is held back until the exact same live result survives three
    /// observations over at least 500 ms. This prevents a half-swipe or a
    /// transient Mission Control enumeration from deleting Desktops from the
    /// selector, while a real Desktop removal still converges quickly.
    pub fn desktops() -> Option<DesktopTopology> {
        let candidate = desktops_raw();
        let state = TOPOLOGY.get_or_init(|| Mutex::new(TopologyState::default()));
        let mut state = state.lock().ok()?;
        state.gelesen = Some(Instant::now());
        let Some((display, list)) = candidate else {
            return state.published.clone();
        };
        if !topology_valid(&display, &list) {
            return state.published.clone();
        }

        let candidate_normal = list.iter()
            .filter(|s| s.typ == crate::arbeitsplatz::TYP_SCHREIBTISCH).count();
        let published_normal = state.published.as_ref()
            .filter(|(old_display, _)| old_display == &display)
            .map(|(_, old)| old.iter().filter(|s| s.typ == crate::arbeitsplatz::TYP_SCHREIBTISCH).count())
            .unwrap_or(0);
        if published_normal > 0 && candidate_normal < published_normal {
            let signature = normal_signature(&display, &list);
            let now = Instant::now();
            match state.pending_shrink.as_mut() {
                Some(p) if p.signature == signature => {
                    p.observations = p.observations.saturating_add(1);
                    if p.observations < 3 || now.duration_since(p.first_seen) < Duration::from_millis(500) {
                        return state.published.clone();
                    }
                }
                _ => {
                    state.pending_shrink = Some(PendingTopology {
                        signature,
                        first_seen: now,
                        observations: 1,
                    });
                    return state.published.clone();
                }
            }
        }
        state.pending_shrink = None;
        state.published = Some((display, list));
        state.published.clone()
    }

    /// Returns the last coherent published topology immediately (<1µs)
    /// without expensive CGWindowList/WindowServer queries. Falls back to desktops()
    /// only on cold start if no topology has been published yet.
    /// The validated topology, never older than 1 s. It used to return the
    /// cache forever once one existed: Desktops added later (measured: 4/5)
    /// stayed invisible to the Shortcut cycle until an unrelated path (a
    /// fullscreen discovery) happened to re-read the system. A re-read goes
    /// through `desktops()`' validation: growth is published at once, a
    /// shrink only after 3 consistent reads over 500 ms (never a transient
    /// partial list over the last valid one).
    pub fn published_topology() -> Option<DesktopTopology> {
        let state = TOPOLOGY.get_or_init(|| Mutex::new(TopologyState::default()));
        let (cached, frisch) = state.lock().ok()
            .map(|s| (s.published.clone(), s.gelesen.is_some_and(|t| t.elapsed() < Duration::from_millis(1000))))
            .unwrap_or((None, false));
        if cached.is_some() && frisch {
            cached
        } else {
            desktops().or(cached)
        }
    }

    /// Wie viele FREMDE Fenster auf jedem Space liegen.
    ///
    /// Ausgenommen sind: Nokis eigene Fenster, Hilfsfenster ohne eigene
    /// Ebene, winzige Fenster — und vor allem Fenster, die auf MEHREREN
    /// Spaces zugleich liegen. Letztere sind Helfer mit CanJoinAllSpaces;
    /// gemessen liegt so eines auf jedem Schreibtisch und liesse sonst
    /// keinen einzigen je als frei erscheinen.
    pub fn fremde_fenster_pro_space() -> Vec<(u64, u32)> {
        let mut roh: Vec<(String, u64)> = vec![];
        unsafe {
            let wins = CGWindowListCopyWindowInfo(0, 0);
            if wins.is_null() {
                return vec![];
            }
            let k_num = schluessel(b"kCGWindowNumber\0");
            let k_lay = schluessel(b"kCGWindowLayer\0");
            let k_own = schluessel(b"kCGWindowOwnerName\0");
            let k_bnd = schluessel(b"kCGWindowBounds\0");
            let k_w = schluessel(b"Width\0");
            let k_h = schluessel(b"Height\0");
            for i in 0..CFArrayGetCount(wins) {
                let w = CFArrayGetValueAtIndex(wins, i);
                if zahl(CFDictionaryGetValue(w, k_lay)) != 0 {
                    continue;
                }
                let besitzer = text(CFDictionaryGetValue(w, k_own));
                if besitzer == "Noki" {
                    continue;
                }
                let b = CFDictionaryGetValue(w, k_bnd);
                if !b.is_null() {
                    let bw = zahl(CFDictionaryGetValue(b, k_w));
                    let bh = zahl(CFDictionaryGetValue(b, k_h));
                    if bw < 80 || bh < 80 {
                        continue;
                    }
                }
                let wid = zahl(CFDictionaryGetValue(w, k_num));
                let Some(spaces) = spaces_des_fensters(wid) else {
                    continue;
                };
                if spaces.len() != 1 {
                    continue; // schwimmt ueber alle Schreibtische
                }
                roh.push((besitzer, spaces[0]));
            }
            for k in [k_num, k_lay, k_own, k_bnd, k_w, k_h] {
                CFRelease(k);
            }
            CFRelease(wins);
        }
        // Allgegenwaertige Hilfsfenster aussortieren - sonst waere kein
        // Schreibtisch je leer.
        crate::arbeitsplatz::belegung(&roh, 3)
    }

    /// Alle ECHTEN Programmfenster, die dem angefragten Space angehoeren, mit
    /// allem, was zur Herkunftspruefung gebraucht wird.
    ///
    /// "Echt" heisst hier: eigene Fensterebene (layer 0) und gross genug, um
    /// ein Arbeitsfenster zu sein. Palettchen, Schatten und Hilfsflaechen
    /// sind keine Schreibtisch-Bewohner. Ein normales Fenster mit mehreren
    /// Mitgliedschaften ist auf jedem dieser Desktops physisch sichtbar und
    /// gehoert deshalb auch in deren visuelle Komposition; Besitz- und
    /// Reservierungslogik bleiben davon getrennt.
    /// Rueckgabe je Fenster: (id, pid, besitzer, titel, x, y, w, h, space).
    #[allow(clippy::type_complexity)]
    pub fn fenster_auf_space(sid: u64) -> Vec<(i64, i32, String, String, i64, i64, i64, i64)> {
        let mut out = vec![];
        unsafe {
            let wins = CGWindowListCopyWindowInfo(0, 0);
            if wins.is_null() {
                return out;
            }
            let k_num = schluessel(b"kCGWindowNumber\0");
            let k_lay = schluessel(b"kCGWindowLayer\0");
            let k_own = schluessel(b"kCGWindowOwnerName\0");
            let k_pid = schluessel(b"kCGWindowOwnerPID\0");
            let k_nam = schluessel(b"kCGWindowName\0");
            let k_bnd = schluessel(b"kCGWindowBounds\0");
            let k_x = schluessel(b"X\0");
            let k_y = schluessel(b"Y\0");
            let k_w = schluessel(b"Width\0");
            let k_h = schluessel(b"Height\0");
            for i in 0..CFArrayGetCount(wins) {
                let w = CFArrayGetValueAtIndex(wins, i);
                if zahl(CFDictionaryGetValue(w, k_lay)) != 0 {
                    continue;
                }
                let b = CFDictionaryGetValue(w, k_bnd);
                if b.is_null() {
                    continue;
                }
                let (bw, bh) = (
                    zahl(CFDictionaryGetValue(b, k_w)),
                    zahl(CFDictionaryGetValue(b, k_h)),
                );
                if bw < 80 || bh < 80 {
                    continue;
                }
                let wid = zahl(CFDictionaryGetValue(w, k_num));
                match spaces_des_fensters(wid) {
                    Some(sp) if sp.contains(&sid) => {}
                    _ => continue,
                }
                out.push((
                    wid,
                    zahl(CFDictionaryGetValue(w, k_pid)) as i32,
                    text(CFDictionaryGetValue(w, k_own)),
                    text(CFDictionaryGetValue(w, k_nam)),
                    zahl(CFDictionaryGetValue(b, k_x)),
                    zahl(CFDictionaryGetValue(b, k_y)),
                    bw,
                    bh,
                ));
            }
            for k in [k_num, k_lay, k_own, k_pid, k_nam, k_bnd, k_x, k_y, k_w, k_h] {
                CFRelease(k);
            }
            CFRelease(wins);
        }
        out
    }

    /// All ordinary windows, independent of physical Space membership.
    /// Registry ownership is decided elsewhere; this is observation only.
    #[allow(clippy::type_complexity)]
    /// Sichtbare Fenster von vorn nach hinten (nur die Nummern). Anders als
    /// `alle_fenster` ist DIESE Reihenfolge die echte Stapelung.
    pub fn stapel_vorn_zuerst() -> Vec<i64> {
        let mut out = vec![];
        unsafe {
            let wins = CGWindowListCopyWindowInfo(1, 0); // OnScreenOnly
            if wins.is_null() { return out; }
            let k_num = schluessel(b"kCGWindowNumber\0");
            for i in 0..CFArrayGetCount(wins) {
                out.push(zahl(CFDictionaryGetValue(CFArrayGetValueAtIndex(wins, i), k_num)));
            }
            CFRelease(k_num);
            CFRelease(wins);
        }
        out
    }

    pub fn alle_fenster() -> Vec<(i64, i32, String, String, i64, i64, i64, i64, i64)> {
        let mut out = vec![];
        unsafe {
            let wins = CGWindowListCopyWindowInfo(0, 0);
            if wins.is_null() { return out; }
            let k_num = schluessel(b"kCGWindowNumber\0");
            let k_lay = schluessel(b"kCGWindowLayer\0");
            let k_own = schluessel(b"kCGWindowOwnerName\0");
            let k_pid = schluessel(b"kCGWindowOwnerPID\0");
            let k_nam = schluessel(b"kCGWindowName\0");
            let k_bnd = schluessel(b"kCGWindowBounds\0");
            let k_x = schluessel(b"X\0"); let k_y = schluessel(b"Y\0");
            let k_w = schluessel(b"Width\0"); let k_h = schluessel(b"Height\0");
            for i in 0..CFArrayGetCount(wins) {
                let w = CFArrayGetValueAtIndex(wins, i);
                if zahl(CFDictionaryGetValue(w, k_lay)) != 0 { continue; }
                let b = CFDictionaryGetValue(w, k_bnd);
                if b.is_null() { continue; }
                let bw = zahl(CFDictionaryGetValue(b, k_w));
                let bh = zahl(CFDictionaryGetValue(b, k_h));
                if bw < 80 || bh < 80 { continue; }
                out.push((
                    zahl(CFDictionaryGetValue(w, k_num)),
                    zahl(CFDictionaryGetValue(w, k_pid)) as i32,
                    text(CFDictionaryGetValue(w, k_own)),
                    text(CFDictionaryGetValue(w, k_nam)),
                    zahl(CFDictionaryGetValue(b, k_x)),
                    zahl(CFDictionaryGetValue(b, k_y)), bw, bh, i as i64,
                ));
            }
            for k in [k_num,k_lay,k_own,k_pid,k_nam,k_bnd,k_x,k_y,k_w,k_h] { CFRelease(k); }
            CFRelease(wins);
        }
        out
    }

    /// Fenster-IDs eines Programms, die auf GENAU EINEM Space liegen.
    /// Fenster, die ueber alle Schreibtische schwimmen, werden nie verschoben
    /// - sie gehoeren keinem Schreibtisch und wuerden ueberall verschwinden.
    pub fn fenster_von(besitzer: &str) -> Vec<(i64, u64)> {
        let mut out = vec![];
        unsafe {
            let wins = CGWindowListCopyWindowInfo(0, 0);
            if wins.is_null() {
                return out;
            }
            let k_num = schluessel(b"kCGWindowNumber\0");
            let k_lay = schluessel(b"kCGWindowLayer\0");
            let k_own = schluessel(b"kCGWindowOwnerName\0");
            let k_bnd = schluessel(b"kCGWindowBounds\0");
            let k_w = schluessel(b"Width\0");
            let k_h = schluessel(b"Height\0");
            for i in 0..CFArrayGetCount(wins) {
                let w = CFArrayGetValueAtIndex(wins, i);
                if zahl(CFDictionaryGetValue(w, k_lay)) != 0 {
                    continue;
                }
                if !text(CFDictionaryGetValue(w, k_own)).eq_ignore_ascii_case(besitzer) {
                    continue;
                }
                let b = CFDictionaryGetValue(w, k_bnd);
                if !b.is_null()
                    && (zahl(CFDictionaryGetValue(b, k_w)) < 80
                        || zahl(CFDictionaryGetValue(b, k_h)) < 80)
                {
                    continue;
                }
                let wid = zahl(CFDictionaryGetValue(w, k_num));
                if let Some(sp) = spaces_des_fensters(wid) {
                    if sp.len() == 1 {
                        out.push((wid, sp[0]));
                    }
                }
            }
            for k in [k_num, k_lay, k_own, k_bnd, k_w, k_h] {
                CFRelease(k);
            }
            CFRelease(wins);
        }
        out
    }

    /// Den Bildschirm WIRKLICH SICHTBAR auf einen Space schalten.
    ///
    /// Gemessen auf macOS 26.6, mit Bildschirmfotos belegt: die private
    /// Funktion `CGSManagedDisplaySetCurrentSpace` schreibt nur die
    /// Buchfuehrung um. Danach meldet das System "Schreibtisch 3", waehrend
    /// der Nutzer unveraendert seinen eigenen Schreibtisch sieht - und ein
    /// Fenster von Nokis Schreibtisch mitten darauf. Genau dieses Zwitterbild
    /// hat der Nutzer gemeldet, und genau daran hat die vorige Abnahme
    /// vorbeigemessen: sie verglich die Kennung, die sie selbst gerade
    /// geschrieben hatte. `CGSShowSpaces`/`CGSHideSpaces` aendern daran
    /// nichts (unter SIP wirkungslos).
    ///
    /// Was nachweislich wirkt, ist der Weg, den macOS selbst geht: die
    /// Tastenkombination Strg+Pfeil. Sie wird vom System VOR den Programmen
    /// behandelt, loest die echte Schreibtisch-Animation aus, und danach
    /// stimmen Bild und Kennung ueberein.
    ///
    /// Gezielt wird ueber die STELLE in der Mission-Control-Reihenfolge, die
    /// jedes Mal frisch gelesen wird (`mru-spaces` sortiert sie um). Die
    /// Identitaet bleibt die uuid; die Reihenfolge ist nur der Weg dorthin.
    static NAV_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    static NAV_WER: std::sync::Mutex<&'static str> = std::sync::Mutex::new("-");
    struct SpaceActionTrace { reason: &'static str, window: u64, before: u64 }
    impl Drop for SpaceActionTrace {
        fn drop(&mut self) {
            crate::space_action_log(self.reason, "Noki", self.window, self.before,
                aktiver_space().map(|x| x.0).unwrap_or(0));
        }
    }
    fn navigation_merken(wer: &'static str) {
        let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64).unwrap_or(0);
        NAV_MS.store(ms, std::sync::atomic::Ordering::SeqCst);
        if let Ok(mut g) = NAV_WER.lock() { *g = wer; }
    }
    /// (Zeit in ms seit Epoche, Aufrufer) des letzten Space-Wechsels durch Noki.
    pub fn letzte_navigation() -> (u64, &'static str) {
        (NAV_MS.load(std::sync::atomic::Ordering::SeqCst), NAV_WER.lock().map(|g| *g).unwrap_or("-"))
    }

    pub fn sichtbar_zum_space(sid: u64) -> Result<(), String> {
        let _trace = SpaceActionTrace { reason: "visible_space_navigation", window: sid, before: aktiver_space().map(|x| x.0).unwrap_or(0) };
        navigation_merken("sichtbar_zum_space");
        // Jeder sichtbare Wechsel nennt sich. Ein Schreibtischwechsel, den
        // der Nutzer nicht verlangt hat, ist ein Fehler - und ohne diese
        // Zeile war nicht zu sehen, WER ihn ausgeloest hat.
        eprintln!("[NAVIGATION] sichtbarer Wechsel nach {sid}");

        // EIN SCHRITT, DANN NEU SEHEN.
        //
        // Vorher wurde die Entfernung EINMAL berechnet ("das Ziel liegt zwei
        // Schritte rechts") und dann blind so oft gewischt. Das war falsch,
        // und der Nutzer hat den Schaden gemeldet: `mru-spaces` sortiert die
        // Reihenfolge waehrend des Wischens um. Nach dem ersten Schritt
        // stimmte die Rechnung nicht mehr - mal kam der Nutzer gar nicht an,
        // mal landete er auf einem fremden oder einem Vollbild-Schreibtisch
        // und blieb dort stehen.
        //
        // Jetzt gilt: Lage lesen, EINEN Schritt in die richtige Richtung,
        // Ankunft abwarten, wieder Lage lesen. Die Schleife endet nur, wenn
        // der Bildschirm WIRKLICH auf dem Ziel steht - oder mit einem
        // ehrlichen Fehler, wenn Zeit oder Schritte aufgebraucht sind.
        // Nichts wird vorausgesetzt, was sich zwischendurch aendern kann.
        let frist = std::time::Instant::now() + std::time::Duration::from_secs(8);
        let mut schritte = 0u32;
        loop {
            let jetzt = aktiver_space().map(|(a, _)| a).unwrap_or(0);
            if jetzt == sid {
                return sichtbar_bestaetigen(sid);
            }
            if std::time::Instant::now() >= frist {
                return Err(format!(
                    "Ziel-Schreibtisch {sid} nicht erreicht (Zeit abgelaufen, zuletzt auf {jetzt})"
                ));
            }
            // Grosszuegig, aber endlich: mehr Schritte als es Schreibtische
            // gibt, damit ein Umsortieren unterwegs verkraftet wird - und
            // trotzdem keine Endlosschleife.
            let ordnung = space_reihenfolge().ok_or("Reihenfolge nicht lesbar")?;
            if schritte > (ordnung.len() as u32 + 4) * 2 {
                return Err(format!(
                    "Ziel-Schreibtisch {sid} nicht erreicht (zu viele Schritte, zuletzt auf {jetzt})"
                ));
            }
            let i_jetzt = ordnung
                .iter()
                .position(|x| *x == jetzt)
                .ok_or("Der aktuelle Schreibtisch steht nicht in der Reihenfolge")?;
            let i_ziel = ordnung
                .iter()
                .position(|x| *x == sid)
                .ok_or("Ziel-Schreibtisch steht nicht in der Reihenfolge")?;
            let links = i_ziel < i_jetzt;
            // REAL_SPACE uses the same native Space shortcut as the user.
            // The older private DockSwipe packet is intentionally not used:
            // macOS can accept the post while leaving CGSGetActiveSpace at
            // the invalid transition sentinel 0/type 3.  Because that helper
            // returned only "posted", its fallback was never reached.
            if !ctrl_pfeil(links) {
                return Err("Schreibtischwechsel nicht moeglich. Noki braucht die Freigabe „Bedienungshilfen“.".into());
            }
            schritte += 1;
            // Auf die Ankunft GENAU DIESES Schritts warten. Erst danach wird
            // wieder gelesen - sonst zaehlte die naechste Runde eine
            // Reihenfolge, die noch zum vorigen Bild gehoert.
            let bis = std::time::Instant::now() + std::time::Duration::from_millis(900);
            while std::time::Instant::now() < bis {
                if aktiver_space().map(|(a, _)| a) != Some(jetzt) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
    }

    /// (Bildschirm-Index, Stelle auf diesem Bildschirm, Anzahl seiner Spaces)
    /// in der Mission-Control-Ordnung - frisch gelesen.
    fn space_stelle(sid: u64) -> Option<(usize, usize, usize)> {
        let f: extern "C" fn(i32) -> *const c_void =
            unsafe { std::mem::transmute(sym(b"CGSCopyManagedDisplaySpaces\0")?) };
        unsafe {
            let d = f(cid()?);
            if d.is_null() {
                return None;
            }
            let k_sp = schluessel(b"Spaces\0");
            let k_id = schluessel(b"ManagedSpaceID\0");
            let mut out = None;
            for i in 0..CFArrayGetCount(d) {
                let sp = CFDictionaryGetValue(CFArrayGetValueAtIndex(d, i), k_sp);
                if sp.is_null() {
                    continue;
                }
                let n = CFArrayGetCount(sp);
                for j in 0..n {
                    if zahl(CFDictionaryGetValue(CFArrayGetValueAtIndex(sp, j), k_id)) as u64 == sid {
                        out = Some((i as usize, j as usize, n as usize));
                    }
                }
            }
            CFRelease(k_sp);
            CFRelease(k_id);
            CFRelease(d);
            out
        }
    }

    /// DIREKT auf genau diesen Space - fuer "Zum Schreibtisch N".
    ///
    /// `sichtbar_zum_space` geht Schritt fuer Schritt (Ctrl+Pfeil) und zeigt
    /// dabei jeden Schreibtisch dazwischen; von Schreibtisch 3 nach 1 kam der
    /// Nutzer ueber 2. Die Tastenkuerzel "Zu Schreibtisch N" sind auf diesem
    /// Mac aus (symbolic hotkeys 118+ disabled), und sie einzuschalten waere
    /// eine Aenderung an den Einstellungen des Nutzers.
    ///
    /// Gemessen (macOS 26.6): Mission Control fuehrt in der Dock-AX eine
    /// Spaces-Leiste (`mc.spaces.list`), deren Knoepfe GENAU der Reihenfolge
    /// von CGSCopyManagedDisplaySpaces folgen (auch Vollbild-Spaces). AXPress
    /// auf den Knopf fuehrt ohne Zwischenhalt hin: Schreibtisch 2 -> 4 ueber
    /// Vollbild und Schreibtisch 3 hinweg, beobachtet nur [1, 6963]. Ein
    /// Druck in den ersten ~50 ms der Oeffnungsanimation wird still
    /// verworfen - daher Pause und begrenzte Wiederholung.
    ///
    /// Fail closed: stimmt die Knopfzahl nicht mit der Space-Liste ueberein,
    /// wird nichts gedrueckt, Mission Control geschlossen, und es gibt einen
    /// ehrlichen Fehler - nie ein "naechstgelegener" Schreibtisch.
    pub fn direkt_zum_space(sid: u64) -> Result<(), String> {
        let _trace = SpaceActionTrace { reason: "direct_space_navigation", window: sid, before: aktiver_space().map(|x| x.0).unwrap_or(0) };
        navigation_merken("direkt_zum_space");
        #[link(name = "ApplicationServices", kind = "framework")]
        extern "C" {
            fn AXUIElementCreateApplication(pid: i32) -> *const c_void;
            fn AXUIElementCopyAttributeValue(e: *const c_void, a: *const c_void, v: *mut *const c_void) -> i32;
            fn AXUIElementPerformAction(e: *const c_void, a: *const c_void) -> i32;
        }
        unsafe fn attr(e: *const c_void, name: &[u8]) -> Option<*const c_void> {
            let k = schluessel(name);
            let mut v: *const c_void = std::ptr::null();
            let ok = AXUIElementCopyAttributeValue(e, k, &mut v) == 0 && !v.is_null();
            CFRelease(k);
            ok.then_some(v)
        }
        /// Kinder mit genau dieser AXIdentifier (retained, in Reihenfolge).
        unsafe fn kinder_mit(e: *const c_void, id: &str) -> Vec<*const c_void> {
            let mut out = vec![];
            let Some(k) = attr(e, b"AXChildren\0") else { return out };
            for i in 0..CFArrayGetCount(k) {
                let c = CFArrayGetValueAtIndex(k, i);
                if let Some(v) = attr(c, b"AXIdentifier\0") {
                    let gleich = text(v) == id;
                    CFRelease(v);
                    if gleich {
                        extern "C" { fn CFRetain(o: *const c_void) -> *const c_void; }
                        out.push(CFRetain(c));
                    }
                }
            }
            CFRelease(k);
            out
        }
        unsafe fn freigeben(v: Vec<*const c_void>) { for e in v { CFRelease(e); } }
        fn umschalten() -> bool {
            unsafe {
                let h = dlopen(
                    b"/System/Library/Frameworks/ApplicationServices.framework/ApplicationServices\0"
                        .as_ptr() as *const _, 1);
                if h.is_null() { return false; }
                let p = dlsym(h, b"CoreDockSendNotification\0".as_ptr() as *const _);
                if p.is_null() { return false; }
                let f: extern "C" fn(*const c_void, i32) = std::mem::transmute(p);
                let n = schluessel(b"com.apple.expose.awake\0");
                f(n, 0);
                CFRelease(n);
                true
            }
        }
        let t0 = std::time::Instant::now();
        let start = aktiver_space().map(|(a, _)| a).unwrap_or(0);
        eprintln!("[NAVIGATION] direkt nach {sid} (von {start}) via mission_control");
        if start == sid {
            return sichtbar_bestaetigen(sid);
        }
        let (anzeige, stelle, anzahl) = space_stelle(sid).ok_or("Ziel-Space steht in keiner Liste")?;
        let dock_pid = crate::lesezeichen::pid_fuer_bundle("com.apple.dock").ok_or("Dock laeuft nicht")?;
        unsafe {
            let dock = AXUIElementCreateApplication(dock_pid);
            if dock.is_null() { return Err("Dock nicht erreichbar".into()); }
            let offen = |dock: *const c_void| { let v = kinder_mit(dock, "mc"); let ja = !v.is_empty(); freigeben(v); ja };
            let selbst_geoeffnet = !offen(dock);
            if selbst_geoeffnet && !umschalten() {
                CFRelease(dock);
                return Err("Mission Control nicht erreichbar".into());
            }
            // Knopf suchen: mc -> mc.display[anzeige] -> mc.spaces -> mc.spaces.list.
            let mut knopf: Option<*const c_void> = None;
            let mut grund = String::from("Mission Control oeffnete nicht");
            let bis = std::time::Instant::now() + std::time::Duration::from_millis(1200);
            while knopf.is_none() && std::time::Instant::now() < bis {
                let mc = kinder_mit(dock, "mc");
                if let Some(&m) = mc.first() {
                    let anz = kinder_mit(m, "mc.display");
                    if let Some(&a) = anz.get(anzeige) {
                        let sp = kinder_mit(a, "mc.spaces");
                        if let Some(&s) = sp.first() {
                            let li = kinder_mit(s, "mc.spaces.list");
                            if let Some(&l) = li.first() {
                                if let Some(k) = attr(l, b"AXChildren\0") {
                                    let n = CFArrayGetCount(k) as usize;
                                    if n == anzahl {
                                        extern "C" { fn CFRetain(o: *const c_void) -> *const c_void; }
                                        knopf = Some(CFRetain(CFArrayGetValueAtIndex(k, stelle as isize)));
                                    } else {
                                        grund = format!("Spaces-Leiste hat {n} Knoepfe, erwartet {anzahl}");
                                    }
                                    CFRelease(k);
                                }
                            }
                            freigeben(li);
                        }
                        freigeben(sp);
                    }
                    freigeben(anz);
                }
                freigeben(mc);
                if knopf.is_none() {
                    // Gezaehlte Knoepfe, die nicht passen: nicht warten, abbrechen.
                    if grund.starts_with("Spaces-Leiste") { break; }
                    std::thread::sleep(std::time::Duration::from_millis(15));
                }
            }
            let Some(knopf) = knopf else {
                if selbst_geoeffnet && offen(dock) { umschalten(); }
                CFRelease(dock);
                crate::virtual_workspace::trace(&format!("[NAVIGATION] direct target={sid} aborted={grund}"));
                return Err(grund);
            };
            let press = schluessel(b"AXPress\0");
            let mut gesehen: Vec<u64> = vec![start];
            let mut angekommen = false;
            let mut drucke = 0;
            for _ in 0..4 {
                // Der Oeffnungsanimation Zeit geben (ein zu frueher Druck
                // wird gemessen still verworfen).
                std::thread::sleep(std::time::Duration::from_millis(if drucke == 0 { 250 } else { 200 }));
                if AXUIElementPerformAction(knopf, press) != 0 { continue; }
                drucke += 1;
                let bis = std::time::Instant::now() + std::time::Duration::from_millis(700);
                while std::time::Instant::now() < bis {
                    let jetzt = aktiver_space().map(|(a, _)| a).unwrap_or(0);
                    if jetzt != 0 && gesehen.last() != Some(&jetzt) { gesehen.push(jetzt); }
                    if jetzt == sid { angekommen = true; break; }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                if angekommen || !offen(dock) { break; }
            }
            CFRelease(press);
            CFRelease(knopf);
            if !angekommen && offen(dock) { umschalten(); }
            CFRelease(dock);
            let zwischen = gesehen.iter().filter(|s| **s != start && **s != sid).count();
            crate::virtual_workspace::trace(&format!(
                "[NAVIGATION] direct target={sid} start={start} presses={drucke} seen={gesehen:?} intermediate={zwischen} arrived={angekommen} ms={}",
                t0.elapsed().as_millis()
            ));
            if !angekommen {
                return Err(format!("Ziel-Schreibtisch {sid} nicht erreicht (zuletzt auf {})",
                    gesehen.last().copied().unwrap_or(0)));
            }
        }
        sichtbar_bestaetigen(sid)
    }

    /// Ein Schreibtischschritt als Dock-Wischgeste - dieselbe Ereignisart,
    /// die das Trackpad erzeugt, nur mit sehr hoher Geschwindigkeit: macOS
    /// schaltet dann OHNE Gleitanimation um (gemessen: 1-2 Bilder statt
    /// ~0,5 s je Schritt). Felder wie bei InstantSpaceSwitcher; die Phasen
    /// brauchen ~10 ms Abstand, sonst verwirft sie das Dock.
    #[allow(dead_code)] // legacy experiment retained for comparison; never used by REAL_SPACE
    fn dock_wisch(rechts: bool) -> bool {
        #[link(name = "CoreGraphics", kind = "framework")]
        extern "C" {
            fn CGEventCreate(src: *const c_void) -> *const c_void;
            fn CGEventSetIntegerValueField(ev: *const c_void, feld: u32, wert: i64);
            fn CGEventSetDoubleValueField(ev: *const c_void, feld: u32, wert: f64);
            fn CGEventPost(tap: u32, ev: *const c_void);
        }
        let v = if rechts { 2000.0 } else { -2000.0 };
        let winzig = f32::from_bits(1) as f64; // FLT_TRUE_MIN
        let fort = if rechts { winzig } else { -winzig };
        for phase in [1i64, 2, 4] {
            unsafe {
                let ev = CGEventCreate(std::ptr::null());
                if ev.is_null() {
                    return false;
                }
                CGEventSetIntegerValueField(ev, 55, 30); // kCGSEventDockControl
                CGEventSetIntegerValueField(ev, 110, 23); // kIOHIDEventTypeDockSwipe
                CGEventSetIntegerValueField(ev, 132, phase);
                CGEventSetDoubleValueField(ev, 124, fort);
                CGEventSetIntegerValueField(ev, 123, 1); // horizontal
                CGEventSetDoubleValueField(ev, 129, v);
                CGEventSetDoubleValueField(ev, 130, v);
                CGEventPost(1, ev); // kCGSessionEventTap
                CFRelease(ev);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        true
    }

    /// Ctrl+Pfeil als echtes Tastenereignis (Session-Ebene) - Rueckfall.
    fn ctrl_pfeil(links: bool) -> bool {
        #[link(name = "CoreGraphics", kind = "framework")]
        extern "C" {
            fn CGEventSourceCreate(state: i32) -> *const c_void;
            fn CGEventCreateKeyboardEvent(src: *const c_void, key: u16, down: bool) -> *const c_void;
            fn CGEventSetFlags(ev: *const c_void, flags: u64);
            fn CGEventPost(tap: u32, ev: *const c_void);
        }
        // Control | NumericPad | SecondaryFn - so meldet die echte
        // Tastatur einen Pfeil mit gedrueckter Ctrl-Taste.
        const FLAGS: u64 = 0x0004_0000 | 0x0020_0000 | 0x0080_0000;
        let code: u16 = if links { 123 } else { 124 };
        unsafe {
            let src = CGEventSourceCreate(1); // kCGEventSourceStateHIDSystemState
            let ab = CGEventCreateKeyboardEvent(src, code, true);
            let auf = CGEventCreateKeyboardEvent(src, code, false);
            if ab.is_null() || auf.is_null() {
                return false;
            }
            CGEventSetFlags(ab, FLAGS);
            CGEventSetFlags(auf, FLAGS);
            CGEventPost(0, ab); // kCGHIDEventTap
            CGEventPost(0, auf);
            CFRelease(ab);
            CFRelease(auf);
            if !src.is_null() {
                CFRelease(src);
            }
        }
        true
    }

    /// Die Space-ID springt schon am ANFANG der Schreibtisch-Animation um.
    /// Wer dann die Uebergangsflaeche wegnimmt, laesst sie mit dem alten
    /// Schreibtisch hinausgleiten (gemessen: Aufnahme v1, 22:09). Sichtbar
    /// angekommen ist der Nutzer erst, wenn WindowServer KEIN normales
    /// Fenster eines anderen Schreibtischs mehr auf dem Bildschirm fuehrt.
    /// Das ist eine Messung des Bildschirms, keine Buchfuehrung - und sie
    /// laeuft nur waehrend dieses einen ausdruecklichen Besuchs.
    fn sichtbar_bestaetigen(sid: u64) -> Result<(), String> {
        for _ in 0..40 {
            if nur_ziel_sichtbar(sid) {
                // Ein Bild Luft, damit das Fenster auch gezeichnet ist.
                std::thread::sleep(std::time::Duration::from_millis(34));
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
        Err("Ziel-Schreibtisch wurde nicht sichtbar bestaetigt".into())
    }

    /// true, wenn jedes sichtbare Fenster der Normalebene zum Ziel gehoert
    /// (oder ueberall schwimmt). Waehrend der Animation sind die Fenster des
    /// verlassenen Schreibtischs noch "onscreen" - genau das wird erkannt.
    pub fn nur_ziel_sichtbar(sid: u64) -> bool {
        unsafe {
            // kCGWindowListOptionOnScreenOnly
            let wins = CGWindowListCopyWindowInfo(1, 0);
            if wins.is_null() {
                return false;
            }
            let k_num = schluessel(b"kCGWindowNumber\0");
            let k_lay = schluessel(b"kCGWindowLayer\0");
            let k_pid = schluessel(b"kCGWindowOwnerPID\0");
            let helfer = crate::vorschau::helfer_pid();
            let mut ok = true;
            for i in 0..CFArrayGetCount(wins) {
                let w = CFArrayGetValueAtIndex(wins, i);
                if zahl(CFDictionaryGetValue(w, k_lay)) != 0 {
                    continue;
                }
                // Die Uebergangsflaeche selbst liegt ueber allem - sie ist
                // kein Beleg fuer irgendeinen Schreibtisch.
                if helfer != 0 && zahl(CFDictionaryGetValue(w, k_pid)) == helfer {
                    continue;
                }
                match spaces_des_fensters(zahl(CFDictionaryGetValue(w, k_num))) {
                    Some(sp) if !sp.is_empty() && !sp.contains(&sid) => {
                        ok = false;
                        break;
                    }
                    _ => {}
                }
            }
            for k in [k_num, k_lay, k_pid] {
                CFRelease(k);
            }
            CFRelease(wins);
            ok
        }
    }

    /// NICHT BENUTZEN fuer einen Schreibtischwechsel.
    ///
    /// Diese Funktion schreibt ausschliesslich die Buchfuehrung um. Gemessen
    /// auf macOS 26.6 erscheint danach weder der Zielschreibtisch, noch
    /// entsteht ein dort wirklich sichtbares Fenster: die so erzeugten
    /// Fenster meldeten `onscreen=false` auf jedem Schreibtisch. Sie bleibt
    /// nur als Beleg dieser Messung stehen; jeder echte Wechsel laeuft ueber
    /// `sichtbar_zum_space`.
    #[allow(dead_code)]
    pub fn zum_space(display: &str, sid: u64) -> bool {
        let Some(p) = sym(b"CGSManagedDisplaySetCurrentSpace\0") else {
            return false;
        };
        let Some(c) = cid() else { return false };
        let f: extern "C" fn(i32, *const c_void, u64) = unsafe { std::mem::transmute(p) };
        unsafe {
            let mut roh = display.as_bytes().to_vec();
            roh.push(0);
            let d = CFStringCreateWithCString(std::ptr::null(), roh.as_ptr() as *const _, 0x0800_0100);
            if d.is_null() {
                return false;
            }
            f(c, d, sid);
            CFRelease(d);
        }
        true
    }
    /// Front-to-back window order of ONE Space (WindowServer's per-Space
    /// list). Unlike CGWindowList it is valid for a HIDDEN Space and follows
    /// an AXRaise there (measured 2026-09-26).
    pub fn fenster_reihe(sid: u64) -> Vec<i64> {
        let Some(p) = sym(b"CGSCopyWindowsWithOptionsAndTags\0") else { return vec![] };
        let Some(c) = cid() else { return vec![] };
        let f: extern "C" fn(i32, i32, *const c_void, i32, *mut u64, *mut u64) -> *const c_void =
            unsafe { std::mem::transmute(p) };
        unsafe {
            let n = sid as i64;
            let zahl_obj = CFNumberCreate(std::ptr::null(), 4, &n as *const i64 as *const c_void);
            let arr = CFArrayCreate(std::ptr::null(), &zahl_obj, 1, &kCFTypeArrayCallBacks as *const c_void);
            CFRelease(zahl_obj);
            let (mut a, mut b) = (0u64, 0u64);
            let r = f(c, 0, arr, 2, &mut a, &mut b);
            CFRelease(arr);
            if r.is_null() { return vec![]; }
            let out = (0..CFArrayGetCount(r)).map(|i| zahl(CFArrayGetValueAtIndex(r, i))).collect();
            CFRelease(r);
            out
        }
    }

    /// Alle Spaces in macOS-Reihenfolge (Mission Control), je Bildschirm.
    pub fn space_reihenfolge() -> Option<Vec<u64>> {
        let f: extern "C" fn(i32) -> *const c_void =
            unsafe { std::mem::transmute(sym(b"CGSCopyManagedDisplaySpaces\0")?) };
        unsafe {
            let d = f(cid()?);
            if d.is_null() {
                return None;
            }
            let k_sp = CFStringCreateWithCString(
                std::ptr::null(),
                b"Spaces\0".as_ptr() as *const _,
                0x0800_0100,
            );
            let k_id = CFStringCreateWithCString(
                std::ptr::null(),
                b"ManagedSpaceID\0".as_ptr() as *const _,
                0x0800_0100,
            );
            let mut r = vec![];
            for i in 0..CFArrayGetCount(d) {
                let sp = CFDictionaryGetValue(CFArrayGetValueAtIndex(d, i), k_sp);
                if sp.is_null() {
                    continue;
                }
                for j in 0..CFArrayGetCount(sp) {
                    let n = CFDictionaryGetValue(CFArrayGetValueAtIndex(sp, j), k_id);
                    if n.is_null() {
                        continue;
                    }
                    let mut v: i64 = 0;
                    let _ = CFNumberGetValue(n, 4, &mut v as *mut i64 as *mut c_void);
                    r.push(v as u64);
                }
            }
            CFRelease(k_sp);
            CFRelease(k_id);
            CFRelease(d);
            Some(r)
        }
    }
    fn sym(n: &[u8]) -> Option<*mut c_void> {
        unsafe {
            let h = dlopen(
                b"/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics\0".as_ptr()
                    as *const _,
                1,
            );
            if h.is_null() {
                return None;
            }
            let p = dlsym(h, n.as_ptr() as *const _);
            if p.is_null() {
                None
            } else {
                Some(p)
            }
        }
    }
    fn cid() -> Option<i32> {
        let f: extern "C" fn() -> i32 =
            unsafe { std::mem::transmute(sym(b"CGSMainConnectionID\0")?) };
        Some(f())
    }
    pub fn space_typ(sid: u64) -> Option<i32> {
        let f: extern "C" fn(i32, u64) -> i32 =
            unsafe { std::mem::transmute(sym(b"CGSSpaceGetType\0")?) };
        Some(f(cid()?, sid))
    }
    /// (aktiver Space, Typ) — Typ 4 = Vollbild, 0 = Schreibtisch.
    /// Der Bildschirm und der Schreibtisch, auf dem der Nutzer GERADE steht.
    ///
    /// Gelesen aus derselben Quelle wie die Schreibtischliste: dem Eintrag
    /// "Current Space" des Bildschirms in `CGSCopyManagedDisplaySpaces`.
    ///
    /// Vorher stand hier `CGSGetActiveSpace`. Das ist eine andere Frage, als
    /// sie klingt: die Antwort gilt der VERBINDUNG des fragenden Programms,
    /// nicht dem Bildschirm. Gemessen lieferte sie fuer Noki - dessen Fenster
    /// ueber Schreibtische wandert - reihum 1, 2730, 2796, 2803, waehrend der
    /// Bildschirm unveraendert auf Schreibtisch 1 stand. Daran hingen zwei
    /// gemeldete Fehler zugleich: der Waechter hielt jeden dieser Spruenge
    /// fuer einen Umzug des Nutzers und flog die Figur quer durch alle
    /// Schreibtische, und die Pruefung nach dem Klick verglich das Ziel mit
    /// einer falschen Zahl und meldete "nicht angekommen", obwohl der
    /// Wechsel stattgefunden hatte.
    ///
    /// Rueckgabe: (Bildschirm-Kennung, ManagedSpaceID, Typ).
    pub fn aktiver_space_voll() -> Option<(String, u64, i32)> {
        let f: extern "C" fn(i32) -> *const c_void =
            unsafe { std::mem::transmute(sym(b"CGSCopyManagedDisplaySpaces\0")?) };
        unsafe {
            let d = f(cid()?);
            if d.is_null() {
                return None;
            }
            let k_cur = schluessel(b"Current Space\0");
            let k_id = schluessel(b"ManagedSpaceID\0");
            let k_ty = schluessel(b"type\0");
            let k_di = schluessel(b"Display Identifier\0");
            let mut out = None;
            for i in 0..CFArrayGetCount(d) {
                let disp = CFArrayGetValueAtIndex(d, i);
                let cur = CFDictionaryGetValue(disp, k_cur);
                if cur.is_null() {
                    continue;
                }
                let id = zahl(CFDictionaryGetValue(cur, k_id)) as u64;
                if id == 0 {
                    continue;
                }
                let typ = zahl(CFDictionaryGetValue(cur, k_ty)) as i32;
                out = Some((text(CFDictionaryGetValue(disp, k_di)), id, typ));
                break; // ein Bildschirm; bei mehreren gilt der erste gemeldete
            }
            for k in [k_cur, k_id, k_ty, k_di] {
                CFRelease(k);
            }
            CFRelease(d);
            out
        }
    }

    pub fn aktiver_space() -> Option<(u64, i32)> {
        if let Some((_, id, typ)) = aktiver_space_voll() {
            // `type` fehlt in manchen Faellen im "Current Space"-Eintrag;
            // dann gilt die Angabe aus der Schreibtischliste.
            let typ = if typ == 0 { space_typ(id).unwrap_or(0) } else { typ };
            return Some((id, typ));
        }
        // Notnagel: besser eine grobe Antwort als gar keine.
        let f: extern "C" fn(i32) -> u64 =
            unsafe { std::mem::transmute(sym(b"CGSGetActiveSpace\0")?) };
        let a = f(cid()?);
        Some((a, space_typ(a)?))
    }
    unsafe fn liste(v: &[i64]) -> *const c_void {
        let n: Vec<*const c_void> = v
            .iter()
            .map(|x| CFNumberCreate(std::ptr::null(), 4, x as *const i64 as *const c_void))
            .collect();
        let a = CFArrayCreate(
            std::ptr::null(),
            n.as_ptr(),
            n.len() as isize,
            &kCFTypeArrayCallBacks as *const c_void,
        );
        for x in n {
            CFRelease(x);
        }
        a
    }
    /// The ONE Space a window belongs to (None if on several / none).
    pub fn fenster_auf_space_id(wid: i64) -> Option<u64> {
        spaces_des_fensters(wid).filter(|s| s.len() == 1).map(|s| s[0])
    }

    pub fn spaces_des_fensters(wid: i64) -> Option<Vec<u64>> {
        spaces_maske(wid, 7)
    }
    /// Like `spaces_des_fensters`, but mask 15 also reports Noki's own
    /// (unmanaged) overlay space - mask 7 never lists it (measured).
    pub fn spaces_inkl_overlay(wid: i64) -> Option<Vec<u64>> {
        spaces_maske(wid, 15)
    }
    /// Noki's own screen-fixed space: created once per process, shown,
    /// absolute level 0. It is no Desktop, so the Space-swipe animation
    /// never moves its windows (measured: a window in it keeps its CG frame
    /// and pixels through the whole slide, while a sticky/stationary window
    /// is carried along x 7 -> -1527). Window levels still order it against
    /// Desktop windows (level 5 stays above level 3). Destroyed with the
    /// connection when Noki quits.
    static OVERLAY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    /// Mission Control scales every shown Space - this one too - and the
    /// Dock resets only its managed Spaces on exit: the overlay space stayed
    /// shrunk and shifted (measured on the Miniatur: 706x483 at 7,451 shown
    /// as 637x436 at 79,453). Replace it with a fresh, untransformed space,
    /// move `wid` over (if given) and destroy the old one.
    pub fn overlay_space_erneuern(wid: Option<i64>) -> Option<u64> {
        use std::sync::atomic::Ordering;
        let alt = OVERLAY.swap(0, Ordering::AcqRel);
        let neu = overlay_space()?;
        if alt != 0 {
            if let Some(w) = wid {
                if spaces_inkl_overlay(w).unwrap_or_default().contains(&alt) {
                    hinzufuegen(w, neu);
                    entfernen(w, &[alt]);
                }
            }
            if let (Some(c), Some(p)) = (cid(), sym(b"CGSSpaceDestroy\0")) {
                let destroy: extern "C" fn(i32, u64) = unsafe { std::mem::transmute(p) };
                destroy(c, alt);
            }
        }
        Some(neu)
    }
    pub fn overlay_space() -> Option<u64> {
        use std::sync::atomic::Ordering;
        let da = OVERLAY.load(Ordering::Acquire);
        if da != 0 {
            return Some(da);
        }
        let c = cid()?;
        let create: extern "C" fn(i32, i32, *const c_void) -> u64 =
            unsafe { std::mem::transmute(sym(b"CGSSpaceCreate\0")?) };
        let level: extern "C" fn(i32, u64, i32) -> i32 =
            unsafe { std::mem::transmute(sym(b"CGSSpaceSetAbsoluteLevel\0")?) };
        let show: extern "C" fn(i32, *const c_void) =
            unsafe { std::mem::transmute(sym(b"CGSShowSpaces\0")?) };
        let sid = create(c, 1, std::ptr::null());
        if sid == 0 {
            return None;
        }
        level(c, sid, 0);
        unsafe {
            let l = liste(&[sid as i64]);
            show(c, l);
            CFRelease(l);
        }
        OVERLAY.store(sid, Ordering::Release);
        Some(sid)
    }
    fn spaces_maske(wid: i64, maske: i32) -> Option<Vec<u64>> {
        let f: extern "C" fn(i32, i32, *const c_void) -> *const c_void =
            unsafe { std::mem::transmute(sym(b"CGSCopySpacesForWindows\0")?) };
        unsafe {
            let w = liste(&[wid]);
            let sp = f(cid()?, maske, w);
            CFRelease(w);
            if sp.is_null() {
                return None;
            }
            let mut r = vec![];
            for i in 0..CFArrayGetCount(sp) {
                let mut v: i64 = 0;
                let _ = CFNumberGetValue(
                    CFArrayGetValueAtIndex(sp, i),
                    4,
                    &mut v as *mut i64 as *mut c_void,
                );
                r.push(v as u64);
            }
            CFRelease(sp);
            Some(r)
        }
    }
    /// Ist das Fenster bei WindowServer eingeordnet? Minimierte und
    /// ausgeblendete Fenster behalten ihre Space-Mitgliedschaft, sind dort
    /// aber nirgends zu sehen - und ScreenCaptureKit liefert fuer sie nur
    /// ein bildloses `.suspended`. Fenster auf einem inaktiven Space sind
    /// dagegen eingeordnet (gemessen, macOS 26.6). Unbekannt zaehlt als ja.
    pub fn fenster_eingeordnet(wid: i64) -> bool {
        let Some(p) = sym(b"CGSWindowIsOrderedIn\0") else { return true };
        let f: extern "C" fn(i32, u32, *mut bool) -> i32 = unsafe { std::mem::transmute(p) };
        let Some(c) = cid() else { return true };
        let mut drin = true;
        f(c, wid as u32, &mut drin) != 0 || drin
    }
    fn fenster_spaces(name: &[u8], wid: i64, sids: &[u64]) -> bool {
        let before = aktiver_space().map(|x| x.0).unwrap_or(0);
        let Some(p) = sym(name) else { return false };
        let Some(c) = cid() else { return false };
        let f: extern "C" fn(i32, *const c_void, *const c_void) = unsafe { std::mem::transmute(p) };
        unsafe {
            let w = liste(&[wid]);
            let s = liste(&sids.iter().map(|x| *x as i64).collect::<Vec<_>>());
            f(c, w, s);
            CFRelease(w);
            CFRelease(s);
        }
        crate::space_action_log(if name.starts_with(b"CGSAdd") { "window_add_to_space" } else { "window_remove_from_space" }, "Noki", wid, before,
            aktiver_space().map(|x| x.0).unwrap_or(0));
        true
    }
    pub fn hinzufuegen(wid: i64, sid: u64) -> bool {
        fenster_spaces(b"CGSAddWindowsToSpaces\0", wid, &[sid])
    }
    pub fn entfernen(wid: i64, sids: &[u64]) -> bool {
        fenster_spaces(b"CGSRemoveWindowsFromSpaces\0", wid, sids)
    }
}

#[cfg(target_os = "macos")]
fn vollbild_spaces_verlassen(wid: i64) {
    let Some(sp) = cgs::spaces_des_fensters(wid) else {
        eprintln!("[VOLLBILD] CGS nicht verfuegbar");
        return;
    };
    let voll: Vec<u64> = sp
        .into_iter()
        .filter(|s| cgs::space_typ(*s) == Some(4))
        .collect();
    if !voll.is_empty() && !cgs::entfernen(wid, &voll) {
        eprintln!("[VOLLBILD] Entfernen nicht verfuegbar");
    }
}

#[cfg(target_os = "macos")]
fn fenster_nummer(win: &WebviewWindow) -> Option<i64> {
    use std::ffi::c_void;
    #[link(name = "objc")]
    extern "C" {
        fn sel_registerName(n: *const std::os::raw::c_char) -> *const c_void;
        fn objc_msgSend();
    }
    let nw = win.ns_window().ok()?;
    unsafe {
        let f: unsafe extern "C" fn(*mut c_void, *const c_void) -> isize =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        Some(f(nw, sel_registerName(b"windowNumber\0".as_ptr() as *const _)) as i64)
    }
}

/// Abgleich im AKTUELLEN Vollbild-Space: Mitgliedschaft = Schalter.
/// AppKit-Flags allein reichen nicht (gemessen: nach einmal "an" tritt das
/// Fenster neuen Vollbild-Spaces weiter bei, obwohl die Bits geloescht sind).
/// Normale Schreibtische (Typ != 4) werden nie angefasst.
#[cfg(target_os = "macos")]
fn vollbild_space_abgleich(win: &WebviewWindow) -> &'static str {
    // Nie ein Klon: Mitglied nur in Nokis EINEM Space. Die Vollbild-Flags
    // setzt allein der Umzug (space_umziehen), nicht der Space-Typ.
    if NUTZER_VERBORGEN.load(Ordering::Relaxed) || !win.is_visible().unwrap_or(false) {
        return "verborgen";
    }
    let heim = NOKI_SPACE.load(Ordering::Relaxed);
    if heim == 0 {
        return "unbekannt";
    }
    let Some(wid) = fenster_nummer(win) else {
        return "kein Fenster";
    };
    let Some(sp) = cgs::spaces_des_fensters(wid) else {
        return "CGS fehlt";
    };
    // Waere das Fenster im Heim-Space gar nicht (mehr) Mitglied, wuerde das
    // Entfernen aller "fremden" Mitgliedschaften es aus JEDEM Space werfen:
    // sichtbar im Sinne von is_visible(), aber auf keinem Schreibtisch zu
    // sehen. Genau so ging Noki beim Space-Wechsel verloren. Stattdessen
    // gilt dann der aktive Space als neues Zuhause.
    if !sp.contains(&heim) {
        if let Some((aktiv, _)) = cgs::aktiver_space() {
            if sp.contains(&aktiv) {
                NOKI_SPACE.store(aktiv, Ordering::Relaxed);
            } else if !NUTZER_VERBORGEN.load(Ordering::Relaxed) {
                // Auf keinem Space: zurueckholen statt liegenlassen.
                cgs::hinzufuegen(wid, aktiv);
                NOKI_SPACE.store(aktiv, Ordering::Relaxed);
                eprintln!("[SICHT] Fenster war in keinem Space - in den aktiven geholt");
            }
        }
        return "neu verortet";
    }
    let fremd: Vec<u64> = sp.iter().copied().filter(|s| *s != heim).collect();
    if !fremd.is_empty() {
        cgs::entfernen(wid, &fremd);
    }
    "passt"
}

#[cfg(not(target_os = "macos"))]
fn vollbild_space_abgleich(_: &WebviewWindow) -> &'static str {
    ""
}

#[cfg(not(target_os = "macos"))]
fn vollbild_bit(_: &WebviewWindow, _: bool) {}

// EIN Sichtbarkeits-Menuepunkt: "Noki zeigen" ODER "Noki verbergen" — der
// Text folgt Nokis TATSAECHLICHER Sichtbarkeit (Fenster sichtbar und, im
// Vollbild-Space, dort freigegeben), nicht dem letzten Klick.
struct SichtMenue(tauri::menu::MenuItem<tauri::Wry>);
// Nur der Nutzer ("Noki verbergen" bzw. Fenster schliessen) darf Noki
// unsichtbar machen. Keine Aktion; der Waechter zeigt ihn sonst wieder.
static NUTZER_VERBORGEN: AtomicBool = AtomicBool::new(false);
static NOKI_GEZEIGT: AtomicBool = AtomicBool::new(false);
static SICHT_ZULETZT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

fn noki_sichtbar(app: &tauri::AppHandle) -> bool {
    // ECHTE Sichtbarkeit im AKTIVEN Space (Fenster sichtbar und Mitglied dort).
    let Some(win) = app.get_webview_window(FENSTER) else {
        return false;
    };
    if NUTZER_VERBORGEN.load(Ordering::Relaxed) || !win.is_visible().unwrap_or(false) {
        return false;
    }
    #[cfg(target_os = "macos")]
    if let (Some((aktiv, _)), Some(wid)) = (cgs::aktiver_space(), fenster_nummer(&win)) {
        if let Some(sp) = cgs::spaces_des_fensters(wid) {
            return sp.contains(&aktiv);
        }
    }
    true
}

fn sicht_menue(app: &tauri::AppHandle) {
    let sichtbar = noki_sichtbar(app);
    let v = if sichtbar { 2 } else { 1 };
    if SICHT_ZULETZT.swap(v, Ordering::Relaxed) != v {
        if let Some(m) = app.try_state::<SichtMenue>() {
            let _ = m.0.set_text(if sichtbar {
                "Noki verbergen"
            } else {
                "Noki zeigen"
            });
        }
    }
}

/// Klick auf den einen Eintrag. Schreibtisch: Fenster zeigen/verbergen (mit
/// der bisherigen Ausblendbewegung). Vollbild: die Vollbild-Freigabe.
fn sicht_umschalten(app: &tauri::AppHandle) {
    if noki_sichtbar(app) {
        noki_verbergen(app.clone()); // ueberall: sofort verborgen
    } else {
        // "Noki zeigen": direkt im AKTIVEN Space (Schreibtisch oder Vollbild) —
        // die EINZIGE Stelle, an der er ohne Flug erscheint.
        let versteckt = app
            .get_webview_window(FENSTER)
            .map_or(true, |w| !w.is_visible().unwrap_or(false));
        // Der Wunsch gilt ab jetzt, egal ob das Fenster nur im falschen
        // Space stand oder wirklich verborgen war. Ohne das blieb
        // NUTZER_VERBORGEN stehen, und der Space-Waechter (der genau
        // darauf prueft) hat Noki beim naechsten Wechsel zurueckgelassen.
        sicht_wunsch_setzen(app, true);
        #[cfg(target_os = "macos")]
        if let Some(w) = app.get_webview_window(FENSTER) {
            space_holen(&w);
        }
        if versteckt {
            noki_zeigen(app);
        }
    }
    sicht_menue(app);
}

/// Space-Wechsel des NUTZERS (auf einen Schreibtisch): Nokis Fenster lebt in
/// genau einem Space. Weicht der aktive Schreibtisch davon ab, bereitet die
/// Seite den Eintritt vor (noki://space vorbereiten), erst DANN wird das
/// Fenster verlegt und der SPEED-Eintritt gestartet. Noki wechselt nie autonom.
static SPACE_BEREIT: AtomicBool = AtomicBool::new(false);
static SPACE_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static NOKI_WID: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// Overlay unsichtbar schalten, bis die Oberflaeche am Ziel frisch
/// gezeichnet hat. Nur Fenster-Alpha: kein Umzug, keine Bewegung, kein
/// Einfluss auf die Figur. Die Notbremse gibt es nach 400 ms frei.
static OVERLAY_VERDECKT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn overlay_alpha(w: &WebviewWindow, a: f64) {
    #[cfg(target_os = "macos")]
    if let Ok(nw) = w.ns_window() {
        unsafe {
            extern "C" {
                fn sel_registerName(n: *const i8) -> *const std::ffi::c_void;
                fn objc_msgSend();
            }
            let set: unsafe extern "C" fn(*mut std::ffi::c_void, *const std::ffi::c_void, f64) =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            set(nw as *mut _, sel_registerName(b"setAlphaValue:\0".as_ptr() as _), a);
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (w, a);
}

static OVERLAY_SEIT: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
fn overlay_ms() -> u128 {
    OVERLAY_SEIT.lock().ok().and_then(|g| g.map(|t| t.elapsed().as_millis())).unwrap_or(0)
}
fn overlay_verdecken(app: &tauri::AppHandle, w: &WebviewWindow) {
    let lauf = OVERLAY_VERDECKT.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
    if let Ok(mut g) = OVERLAY_SEIT.lock() { *g = Some(std::time::Instant::now()); }
    overlay_alpha(w, 0.0);
    // KEINE vorzeitige Freigabe mehr nach 400 ms. Das WebView zeichnet auf
    // dem unsichtbaren Ausgangs-Space nicht; sein Puffer zeigt bis zum ersten
    // frischen Bild am Ziel noch den ALTEN Noki an der alten Stelle. Kam das
    // erste Bild spaeter als 400 ms (gemessen: 523/667 ms beim Wischen auf
    // einen Vollbild-Space, unter Last auch sonst), gab der Timer genau diesen
    // alten Inhalt frei: der "Ghost-Noki" vor dem Einflug. Freigegeben wird
    // jetzt nur noch durch noki_space_gezeichnet (frisches Bild am Ziel);
    // die Notbremse unten bleibt fuer den Fall, dass gar nichts mehr kommt.
    let _ = lauf;
    // NOTBREMSE. Ein unsichtbares Overlay ist ein verschwundener Noki - der
    // Nutzer kann ihn dann nur durch Neustarten zurueckholen, und genau das
    // hat er gemeldet. Die Kette oben gibt frei, WENN nichts dazwischenkommt;
    // diese hier gibt frei, WAS AUCH IMMER dazwischenkommt. Sie ist nicht an
    // den Zaehler gebunden, denn ein verlorener Zaehlerstand ist genau der
    // Fall, den sie abfangen soll.
    let h2 = app.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(1600));
        if OVERLAY_VERDECKT.load(Ordering::SeqCst) == lauf {
            crate::virtual_workspace::trace(&format!("[SPACE_OVERLAY] reveal=notbremse ms={}", overlay_ms()));
            sicht_spur("space_overlay_notbremse", "alpha0", "alpha1", "native");
        }
        overlay_freigeben(&h2);
    });
}

fn overlay_freigeben(app: &tauri::AppHandle) {
    let h = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(w) = h.get_webview_window(FENSTER) {
            overlay_alpha(&w, 1.0);
        }
    });
}

/// Die Oberflaeche hat nach dem Umzug zwei frische Bilder gezeichnet.
#[tauri::command]
fn noki_space_gezeichnet(app: tauri::AppHandle, epoch: Option<u64>) {
    if let Some(ep) = epoch {
        if ep < SPACE_EPOCH.load(Ordering::SeqCst) {
            return;
        }
    }
    OVERLAY_VERDECKT.fetch_add(1, Ordering::SeqCst);
    crate::virtual_workspace::trace(&format!("[SPACE_OVERLAY] reveal=frisch ms={}", overlay_ms()));
    overlay_freigeben(&app);
}

#[tauri::command]
fn noki_space_bereit(epoch: Option<u64>) {
    if let Some(ep) = epoch {
        if ep < SPACE_EPOCH.load(Ordering::SeqCst) {
            return;
        }
    }
    SPACE_BEREIT.store(true, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
//  NOKIS ARBEITSPLATZ — ein echter, reservierter macOS-Schreibtisch
//
//  Kein Nachbau und kein Vollbildfenster: Noki bekommt einen der echten
//  Schreibtische, zu dem der Nutzer auch mit dem Trackpad wischen kann.
//  macOS nennt ihn weiter "Schreibtisch 4" o.ae. — das ist nur Beschriftung.
//  Wer der Arbeitsplatz IST, steht in der uuid, nicht in der Nummer und
//  nicht in der Position (beide aendern sich beim Umsortieren).
//
//  Die Auswahl selbst liegt in arbeitsplatz.rs und ist dort geprueft.
// ---------------------------------------------------------------------------
static ARBEITSPLATZ: std::sync::Mutex<Option<arbeitsplatz::Reservierung>> =
    std::sync::Mutex::new(None);
static ARBEITSPLATZ_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Laeuft gerade eine Aufgabe auf Nokis Arbeitsplatz? Dann gehoert Nokis
/// AUFENTHALT dem Arbeitsplatz - nicht dem Schreibtisch, auf dem der Nutzer
/// gerade zufaellig steht. Ohne diesen einen Besitzer zog ihn die allgemeine
/// "folge dem Nutzer"-Regel bei jedem Space-Wechsel hin und her.
static ARBEITSPLATZ_AUFGABE: AtomicBool = AtomicBool::new(false);

/// Seit wann die Ortsbindung gilt (Unix-Sekunden, 0 = keine).
///
/// Die Bindung wird normalerweise vom Aufgabenende geloest. Wenn dieser
/// Ruf ausbleibt - Absturz, vergessener Pfad, abgebrochene Aktion -, klebte
/// die Figur bisher fuer immer auf Schreibtisch 4 und wirkte, als komme sie
/// von einem fremden Schreibtisch. Eine Bindung, die sich nur von aussen
/// loesen laesst, ist keine Bindung, sondern eine Falle. Geprueft wird im
/// bereits vorhandenen Waechtertakt; ein zweiter Zeitgeber entsteht nicht.
static ARBEITSPLATZ_AUFGABE_SEIT: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Der Space, an den die laufende Ortsbindung gebunden ist (0 = unbekannt,
/// dann gilt die aktuelle Reservierung).
static ARBEITSPLATZ_AUFGABE_SPACE: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// So lange darf eine Ortsbindung ohne Lebenszeichen bestehen.
const ORTSBINDUNG_FRIST: u64 = 180;

fn jetzt_sekunden() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Bindet Nokis Aufenthalt an seinen Schreibtisch, solange dort gearbeitet wird.
fn ortsbindung_setzen() {
    // Die Bindung gilt dem Schreibtisch, auf dem die Aufgabe WIRKLICH laeuft.
    // Eine spaetere Wahl (LINKS-VON-1 + Pfeil) aendert nur die Rolle fuer
    // kuenftige Aktionen - sie darf die Figur nicht hinterherziehen.
    // `try_lock`: einige Aufrufer halten die Reservierung vielleicht noch.
    let sid = ARBEITSPLATZ
        .try_lock()
        .ok()
        .and_then(|g| g.as_ref().map(|r| r.id))
        .unwrap_or(0);
    ARBEITSPLATZ_AUFGABE_SPACE.store(sid, Ordering::Relaxed);
    ARBEITSPLATZ_AUFGABE.store(true, Ordering::Relaxed);
    ARBEITSPLATZ_AUFGABE_SEIT.store(jetzt_sekunden(), Ordering::Relaxed);
}

/// Loest die Bindung - genau einmal. Rueckgabe: ob sie ueberhaupt bestand.
fn ortsbindung_loesen() -> bool {
    ARBEITSPLATZ_AUFGABE_SEIT.store(0, Ordering::Relaxed);
    ARBEITSPLATZ_AUFGABE.swap(false, Ordering::Relaxed)
}

/// Schaltet NOKI gerade selbst den Bildschirm um?
///
/// Gemessen: um ein Fenster auf seinem Schreibtisch entstehen zu lassen,
/// muss Noki den Bildschirm kurz dorthin schalten und wieder zurueck. Der
/// Space-Waechter sah darin zweimal "der Nutzer ist umgezogen" und flog die
/// Figur jedes Mal hin und her - das gemeldete wilde Fliegen ganz ohne
/// Arbeit. Ein beobachteter Wechsel ist nur dann eine Nutzerbewegung, wenn
/// Noki ihn nicht selbst ausgeloest hat. Gezaehlt statt geschaltet, damit
/// verschachtelte Klammern sich nicht gegenseitig aufheben.
static EIGENER_WECHSEL: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Haelt die Klammer, solange sie lebt - auch wenn dazwischen etwas
/// fehlschlaegt und frueh zurueckkehrt.
struct EigenerWechsel;

impl EigenerWechsel {
    fn neu() -> Self {
        EIGENER_WECHSEL.fetch_add(1, Ordering::SeqCst);
        EigenerWechsel
    }
}

impl Drop for EigenerWechsel {
    fn drop(&mut self) {
        EIGENER_WECHSEL.fetch_sub(1, Ordering::SeqCst);
    }
}

fn eigener_wechsel_laeuft() -> bool {
    EIGENER_WECHSEL.load(Ordering::SeqCst) > 0
}

/// Wohin ein Umzug der Figur gerade unterwegs ist (0 = keiner).
///
/// Abschnitt 8: zwei Ereignisse zum selben Ziel sind ein Umzug, nicht zwei.
/// Ohne diese Sperre konnten ein Waechterereignis und sein 420-ms-Nachklang
/// denselben Flug zweimal anstossen und sich gegenseitig ueberholen.
static UMZUG_ZIEL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Ist die Miniatur gerade zu sehen? NUR zur Auskunft (Spur, Regressionstest).
///
/// Ausdruecklich KEINE Eingangsgroesse fuer irgendeine Bewegung: Abschnitt 6
/// verlangt, dass Sichtbarkeit und Aufenthalt nichts miteinander zu tun
/// haben. Der Wert steht hier, damit sich genau das nachweisen laesst.
static VORSCHAU_SICHTBAR: AtomicBool = AtomicBool::new(false);
static TARGET_EPOCH: AtomicU64 = AtomicU64::new(0);
/// Current preview-target epoch (for audit traces outside this module).
pub(crate) fn target_epoch_jetzt() -> u64 { TARGET_EPOCH.load(Ordering::SeqCst) }


/// Laeuft gerade eine Aufgabe auf Nokis Arbeitsplatz?
///
/// Der Unterschied ist nicht kosmetisch: solange sie laeuft, darf KEIN
/// allgemeiner Oeffner ein Programm auf dem Schreibtisch des Nutzers nach
/// vorn holen. Gemessen holt `open` ohne `-g` (und ebenso `open -a` und ein
/// AppleScript-`activate`) das Programm in den Vordergrund, und macOS
/// schaltet dabei auf den Schreibtisch, auf dem dieses Programm Fenster hat.
/// Dann steht ein fremdes Vollfenster beim Nutzer - genau das, was Nokis
/// Arbeitsplatz verhindern soll. Die Aktion wird deshalb nicht verboten,
/// sondern auf den Arbeitsplatz-Weg umgeleitet.
fn arbeitsplatz_aufgabe_laeuft() -> bool {
    ARBEITSPLATZ_AUFGABE.load(Ordering::Relaxed)
}

fn arbeitsplatz_uuid_lesen(app: &tauri::AppHandle) -> String {
    einstellungen_datei(app)
        .and_then(|d| fs::read_to_string(d).ok())
        .and_then(|roh| serde_json::from_str::<serde_json::Value>(&roh).ok())
        .and_then(|v| {
            v.get("noki_arbeitsplatz_uuid")
                .and_then(|x| x.as_str().map(str::to_owned))
        })
        .unwrap_or_default()
}

// =====================================================================
//  AUTHORITATIVE PREVIEW TARGET OWNER (Single Target Owner)
// =====================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetChangeSource {
    UserShortcut,
    UserSelector,
    /// Shortcut 4 SHOW while the user stands on the target: one
    /// deterministic step to the next non-physical normal Desktop.
    UserShowRequest,
    RestorePersistedTarget,
    SystemRefresh,
    SpaceWatcher,
    CaptureRecovery,
}

impl std::fmt::Display for TargetChangeSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UserShortcut => write!(f, "USER_SHORTCUT"),
            Self::UserSelector => write!(f, "USER_SELECTOR"),
            Self::UserShowRequest => write!(f, "USER_SHOW_REQUEST"),
            Self::RestorePersistedTarget => write!(f, "RESTORE_PERSISTED_TARGET"),
            Self::SystemRefresh => write!(f, "SYSTEM_REFRESH"),
            Self::SpaceWatcher => write!(f, "SPACE_WATCHER"),
            Self::CaptureRecovery => write!(f, "CAPTURE_RECOVERY"),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TargetOwnerSnapshot {
    pub uuid: String,
    pub space_id: u64,
    pub desktop_number: usize,
    pub display: String,
    pub epoch: u64,
    pub physical_current: u64,
}

struct TargetOwnerState {
    uuid: String,
    space_id: u64,
    desktop_number: usize,
    display: String,
    epoch: u64,
    is_ready: bool,
}

static TARGET_OWNER: Mutex<Option<TargetOwnerState>> = Mutex::new(None);

fn persist_target_debounced(app: &tauri::AppHandle, uuid: String) {
    static PERSIST_CHANNEL: std::sync::OnceLock<std::sync::mpsc::SyncSender<String>> = std::sync::OnceLock::new();
    let tx = PERSIST_CHANNEL.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::sync_channel::<String>(1);
        let h = app.clone();
        thread::Builder::new()
            .name("target-persist".into())
            .spawn(move || {
                while let Ok(u) = rx.recv() {
                    let mut latest = u;
                    while let Ok(newer) = rx.try_recv() {
                        latest = newer;
                    }
                    thread::sleep(Duration::from_millis(50));
                    while let Ok(newer) = rx.try_recv() {
                        latest = newer;
                    }
                    einstellungen_setzen(&h, "noki_arbeitsplatz_uuid", serde_json::Value::String(latest));
                }
            })
            .ok();
        tx
    });
    let _ = tx.try_send(uuid);
}

pub(crate) fn set_preview_target(
    app: &tauri::AppHandle,
    requested_uuid: &str,
    source: TargetChangeSource,
    reason: &str,
) -> Result<TargetOwnerSnapshot, String> {
    let now_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let physical_current = LETZT_SID.load(Ordering::Relaxed);

    let mut g = TARGET_OWNER.lock().map_err(|e| e.to_string())?;

    // HARD RULE: Only USER_* sources (or initial RESTORE_PERSISTED_TARGET) can change target UUID once ready!
    if let Some(current) = g.as_ref() {
        if current.is_ready && !requested_uuid.is_empty() && requested_uuid != current.uuid {
            match source {
                TargetChangeSource::UserShortcut
                | TargetChangeSource::UserSelector
                | TargetChangeSource::UserShowRequest => {
                    // Allowed: explicit user intent
                }
                _ => {
                    virtual_workspace::trace(&format!(
                        "[TARGET_OWNER_REJECT] Autonomous target change BLOCKED! oldTarget={} attemptedNewTarget={} source={} reason={} physicalCurrentSpace={} epoch={}",
                        current.uuid, requested_uuid, source, reason, physical_current, current.epoch
                    ));
                    return Ok(TargetOwnerSnapshot {
                        uuid: current.uuid.clone(),
                        space_id: current.space_id,
                        desktop_number: current.desktop_number,
                        display: current.display.clone(),
                        epoch: current.epoch,
                        physical_current,
                    });
                }
            }
        }
    }

    let (display, liste) = cgs::published_topology()
        .or_else(|| cgs::desktops())
        .ok_or_else(|| "Schreibtische nicht lesbar".to_string())?;

    let resolved_item = liste.iter().find(|s| s.uuid == requested_uuid && s.typ == arbeitsplatz::TYP_SCHREIBTISCH);

    let (target_uuid, target_id, target_num) = if let Some(item) = resolved_item {
        let num = arbeitsplatz::sichtbare_nummer(&liste, item.id).unwrap_or(1);
        (item.uuid.clone(), item.id, num)
    } else if let Some(current) = g.as_ref() {
        // If requested target is temporarily missing or resolving fails, RETAIN last valid state
        (current.uuid.clone(), current.space_id, current.desktop_number)
    } else {
        // Cold start initial discovery fallback: pick first eligible normal desktop
        let fallback = liste.iter().find(|s| s.typ == arbeitsplatz::TYP_SCHREIBTISCH && arbeitsplatz::persistente_identitaet(&s.uuid))
            .ok_or_else(|| "Kein normaler Schreibtisch vorhanden".to_string())?;
        let num = arbeitsplatz::sichtbare_nummer(&liste, fallback.id).unwrap_or(1);
        (fallback.uuid.clone(), fallback.id, num)
    };

    let old_uuid = g.as_ref().map(|s| s.uuid.clone()).unwrap_or_else(|| "-".to_string());
    let old_epoch = g.as_ref().map(|s| s.epoch).unwrap_or(0);
    let new_epoch = if old_uuid != target_uuid || source == TargetChangeSource::UserShortcut {
        TARGET_EPOCH.fetch_add(1, Ordering::SeqCst) + 1
    } else {
        old_epoch
    };

    let new_state = TargetOwnerState {
        uuid: target_uuid.clone(),
        space_id: target_id,
        display: display.clone(),
        desktop_number: target_num,
        epoch: new_epoch,
        is_ready: true,
    };

    virtual_workspace::trace(&format!(
        "[TARGET_CHANGE] oldTarget={} newTarget={} spaceId={} desktopNum={} source={} reason={} timestamp={} physicalCurrentSpace={} targetEpoch={}",
        old_uuid, target_uuid, target_id, target_num, source, reason, now_ts, physical_current, new_epoch
    ));

    // Keep legacy ARBEITSPLATZ in sync for any readers
    let gen = ARBEITSPLATZ_GEN.fetch_add(1, Ordering::Relaxed) + 1;
    let res = arbeitsplatz::Reservierung {
        uuid: target_uuid.clone(),
        display: display.clone(),
        id: target_id,
        generation: gen,
    };
    if let Ok(mut ap) = ARBEITSPLATZ.lock() {
        *ap = Some(res);
    }

    if matches!(source, TargetChangeSource::UserShortcut | TargetChangeSource::UserSelector
        | TargetChangeSource::UserShowRequest | TargetChangeSource::RestorePersistedTarget) {
        persist_target_debounced(app, target_uuid.clone());
    }

    *g = Some(new_state);
    Ok(TargetOwnerSnapshot {
        uuid: target_uuid,
        space_id: target_id,
        desktop_number: target_num,
        display,
        epoch: new_epoch,
        physical_current,
    })
}

pub(crate) fn get_preview_target() -> Option<TargetOwnerSnapshot> {
    let physical_current = LETZT_SID.load(Ordering::Relaxed);
    TARGET_OWNER.lock().ok()?.as_ref().map(|t| TargetOwnerSnapshot {
        uuid: t.uuid.clone(),
        space_id: t.space_id,
        desktop_number: t.desktop_number,
        display: t.display.clone(),
        epoch: t.epoch,
        physical_current,
    })
}

pub(crate) fn reconcile_target_topology(_app: &tauri::AppHandle, _source: TargetChangeSource, _reason: &str) -> Option<TargetOwnerSnapshot> {
    let mut g = TARGET_OWNER.lock().ok()?;
    let current = g.as_mut()?;
    if !current.is_ready { return None; }

    let (_, liste) = cgs::published_topology().or_else(|| cgs::desktops())?;
    // Re-resolve existing current.uuid against fresh topology:
    if let Some(item) = liste.iter().find(|s| s.uuid == current.uuid && s.typ == arbeitsplatz::TYP_SCHREIBTISCH) {
        let num = arbeitsplatz::sichtbare_nummer(&liste, item.id).unwrap_or(current.desktop_number);
        current.space_id = item.id;
        current.desktop_number = num;
        if let Ok(mut ap) = ARBEITSPLATZ.lock() {
            if let Some(r) = ap.as_mut() {
                r.id = item.id;
            }
        }
    }
    let physical_current = LETZT_SID.load(Ordering::Relaxed);
    Some(TargetOwnerSnapshot {
        uuid: current.uuid.clone(),
        space_id: current.space_id,
        desktop_number: current.desktop_number,
        display: current.display.clone(),
        epoch: current.epoch,
        physical_current,
    })
}

/// Authoritative target reservation lookup: backed strictly by TARGET_OWNER.
/// Never autonomously modifies target or falls back to selecting other desktops.
#[cfg(target_os = "macos")]
fn arbeitsplatz_sichern(app: &tauri::AppHandle) -> Result<arbeitsplatz::Reservierung, String> {
    if let Some(target) = get_preview_target() {
        return Ok(arbeitsplatz::Reservierung {
            uuid: target.uuid,
            display: target.display,
            id: target.space_id,
            generation: target.epoch,
        });
    }
    let persisted_uuid = arbeitsplatz_uuid_lesen(app);
    let snapshot = set_preview_target(
        app,
        &persisted_uuid,
        TargetChangeSource::RestorePersistedTarget,
        "arbeitsplatz_sichern_init",
    )?;
    Ok(arbeitsplatz::Reservierung {
        uuid: snapshot.uuid,
        display: snapshot.display,
        id: snapshot.space_id,
        generation: snapshot.epoch,
    })
}

#[cfg(not(target_os = "macos"))]
fn arbeitsplatz_sichern(_app: &tauri::AppHandle) -> Result<arbeitsplatz::Reservierung, String> {
    Err("Nur auf macOS".into())
}

/// Einweg-Navigation fuer einen bewussten Klick auf die Desktop-Miniatur.
///
/// Das ist die EINE native Handlung fuer "besuche Nokis Schreibtisch" -
/// getrennt von Kuerzel 4, das nie hierher fuehrt. Gezielt wird nie ueber
/// die Position ("Schreibtisch 4"), sondern ueber die reservierte uuid:
///   * uuid vorhanden und nicht leer,
///   * der Schreibtisch existiert in der AKTUELLEN Liste noch,
///   * sein `type` ist 0 (ein echter Schreibtisch, kein Vollbild-Space),
///   * die fluechtige ManagedSpaceID wird bei jedem Aufruf neu aufgeloest.
/// Ein Erfolg wird erst gemeldet, wenn WindowServer den Ziel-Space
/// tatsaechlich als aktiv bestaetigt; sonst bleibt die Miniatur offen.
/// Nach dem Besuch: Nokis Overlay auf den Schreibtisch legen, auf dem der
/// Nutzer jetzt SICHTBAR steht - wieder "ein Fenster, ein Schreibtisch".
///
/// REAL_SPACE laesst die normale macOS-Animation sichtbar. Eine vergroesserte
/// Preview waere wieder ein nachgebauter Desktop und ist fuer den bewussten
/// Besuch deshalb ausdruecklich ausgeschlossen.
#[cfg(target_os = "macos")]
fn overlay_hierher(app: &tauri::AppHandle) {
    let (tx, rx) = std::sync::mpsc::channel();
    let h = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(w) = h.get_webview_window(FENSTER) {
            if let (Some(wid), Some((sid, typ))) = (fenster_nummer(&w), cgs::aktiver_space()) {
                // Unsichtbar umziehen: das WebView hat auf dem alten Space
                // nicht gezeichnet (siehe overlay_verdecken).
                overlay_verdecken(&h, &w);
                space_umziehen(&w, wid, sid, typ);
                NOKI_SPACE.store(sid, Ordering::Relaxed);
            }
            let _ = vollbild_space_abgleich(&w);
        }
        let _ = tx.send(());
    });
    let _ = rx.recv_timeout(Duration::from_millis(600));
}


/// ONLY an explicit primary click on the footer ("Zum Schreibtisch N") may
/// navigate a Space. The helper's footer message grants a one-shot
/// authorization (3 s); `arbeitsplatz_besuchen` consumes it. No other input
/// (scroll, drag, content click, keys, app bar) can ever reach navigation.
static FUSS_FREIGABE: Mutex<Option<(std::time::Instant, &'static str)>> = Mutex::new(None);
pub(crate) fn fuss_freigabe_erteilen(quelle: &'static str) {
    if let Ok(mut g) = FUSS_FREIGABE.lock() { *g = Some((std::time::Instant::now(), quelle)); }
    virtual_workspace::trace(&format!("[NAVIGATION] authorization granted source={quelle}"));
}
fn fuss_freigabe_nehmen() -> Option<&'static str> {
    FUSS_FREIGABE.lock().ok().and_then(|mut g| g.take())
        .filter(|(t, _)| t.elapsed() < Duration::from_secs(3)).map(|(_, q)| q)
}

fn arbeitsplatz_besuchen(app: &tauri::AppHandle) -> serde_json::Value {
    let Some(quelle) = fuss_freigabe_nehmen() else {
        virtual_workspace::trace("[NAVIGATION] refused reason=no_explicit_footer_click");
        return serde_json::json!({ "ok": false, "grund": "Nur der Knopf „Zum Schreibtisch“ wechselt den Schreibtisch." });
    };
    virtual_workspace::trace(&format!("[NAVIGATION] visit source={quelle}"));
    if virtual_workspace::backend() == virtual_workspace::Backend::LegacyVirtualDisplay {
        let d = virtual_workspace::diagnostic();
        let ready = virtual_workspace::ready();
        virtual_workspace::trace(&format!(
            "[ARBEITSPLATZ] besuchen backend=virtual state={} action=open_full_view space_trip=false",
            d["state"].as_str().unwrap_or("UNKNOWN")
        ));
        // Dieselbe Komposition wie die Miniatur, gross - kein Space-Wechsel.
        if ready { vorschau::voll(true); }
        return serde_json::json!({
            "ok": ready,
            "backend": "LEGACY_VIRTUAL_DISPLAY",
            "state": d["state"],
            "action": "OPEN_FULL",
            "grund": if virtual_workspace::ready() {
                ""
            } else {
                "Noki Schreibtisch ist noch nicht bereit."
            }
        });
    }
    #[cfg(target_os = "macos")]
    {
        let r = match arbeitsplatz_sichern(app) {
            Ok(r) => r,
            Err(e) => return serde_json::json!({ "ok": false, "grund": e }),
        };
        // Die Reservierung noch einmal gegen die Liste halten, die JETZT
        // gilt. arbeitsplatz_sichern darf notfalls neu waehlen; ein Besuch
        // darf nur auf einen geprueften echten Schreibtisch fuehren.
        if r.uuid.is_empty() {
            return serde_json::json!({ "ok": false, "grund": "Arbeitsplatz ohne UUID." });
        }
        match cgs::desktops() {
            Some((_, liste)) => {
                match liste.iter().find(|s| s.uuid == r.uuid) {
                    Some(s) if s.typ != arbeitsplatz::TYP_SCHREIBTISCH => {
                        return serde_json::json!({
                            "ok": false, "grund": "Arbeitsplatz ist kein echter Schreibtisch mehr.",
                            "uuid": r.uuid, "typ": s.typ
                        });
                    }
                    Some(s) if s.id != r.id => {
                        return serde_json::json!({
                            "ok": false, "grund": "Arbeitsplatz-Kennung veraltet.",
                            "uuid": r.uuid, "arbeitsplatz": s.id
                        });
                    }
                    Some(_) => {}
                    None => {
                        return serde_json::json!({
                            "ok": false, "grund": "Arbeitsplatz existiert nicht mehr.", "uuid": r.uuid
                        });
                    }
                }
            }
            None => return serde_json::json!({ "ok": false, "grund": "Schreibtische nicht lesbar" }),
        }
        let vorher = cgs::aktiver_space().map(|(a, _)| a).unwrap_or(0);
        if vorher == 0 {
            return serde_json::json!({
                "ok": false,
                "grund": "macOS meldet gerade keinen stabilen aktiven Space; Navigation wurde nicht gestartet.",
                "aktiv_vorher": 0,
                "arbeitsplatz": r.id,
                "uuid": r.uuid
            });
        }
        // NUR Debug-Build: NOKI_TEST_BESUCH=fehler laesst die Navigation
        // scheitern, ohne den Space anzufassen. Ein Fehlschlag ist sonst am
        // laufenden System nicht herstellbar - und genau sein Ruecklauf ist
        // die Zusage, dass eine misslungene Reise die Miniatur nicht frisst.
        #[cfg(debug_assertions)]
        if std::env::var("NOKI_TEST_BESUCH").as_deref() == Ok("fehler") {
            return serde_json::json!({
                "ok": false, "grund": "Ziel-Space wurde nicht aktiv.",
                "aktiv_vorher": vorher, "aktiv_nachher": vorher,
                "arbeitsplatz": r.id, "uuid": r.uuid
            });
        }
        if vorher == r.id {
            return serde_json::json!({
                "ok": true, "schon_da": true, "aktiv_vorher": vorher,
                "aktiv_nachher": vorher, "arbeitsplatz": r.id, "uuid": r.uuid
            });
        }
        // GRUND: EXPLICIT_PREVIEW_VISIT. Der einzige Weg, der den Nutzer
        // absichtlich auf Nokis Schreibtisch stehen laesst - und der einzige
        // Aufrufer des SICHTBAREN Wechsels. Alles andere (Waechter, Freeze,
        // Kuerzel 4, Groesse, Aufgabenende) kommt hier nie an.
        //
        // Der Waechter schweigt fuer die Dauer der Reise: die Zwischen-
        // schreibtische sind kein Wechsel des Nutzers (gemessen: ohne die
        // Klammer zog er das Overlay mitten im Besuch auf Schreibtisch 1).
        // What the Miniatur shows right now (native per-Space order).
        let reihe_vorher: Vec<i64> = cgs::fenster_reihe(r.id).into_iter()
            .filter(|w| vorschau::aufgenommene().contains(w)).collect();
        let fehler = {
            let _eigener = EigenerWechsel::neu();
            // REAL_SPACE zeigt den normalen macOS-Space-Wechsel. Keine
            // vergroesserte Preview und keine Compositor-Flaeche darf den
            // echten Schreibtisch ersetzen oder seine Animation verdecken.
            // "Zum Schreibtisch N" fuehrt DIREKT dorthin - nie Schritt fuer
            // Schritt ueber die Schreibtische dazwischen.
            let mut f = cgs::direkt_zum_space(r.id).err();
            // Arrive at EXACTLY the layout the Miniatur showed. On arrival
            // macOS re-activates the app last active on that Space, and app
            // activation lifts all its windows (measured: Safari jumped over
            // GoodNotes). Restore the pre-visit native order bottom -> top.
            if f.is_none() { ankunft_ordnung_herstellen(r.id, &reihe_vorher); }
            overlay_hierher(app);
            // Die Klammer hat den beobachteten Space mitgeschrieben. Den
            // Stand "auf dem Arbeitsplatz" ebenso - sonst bliebe beim
            // spaeteren Weggehen die Meldung aus und die Miniatur weg.
            let jetzt = cgs::aktiver_space().map(|(a, _)| a).unwrap_or(0);
            LETZT_SID.store(jetzt, Ordering::Relaxed);
            LETZT_AUF_ARBEITSPLATZ.store(jetzt == r.id, Ordering::Relaxed);
            if f.is_none() && jetzt != r.id {
                f = Some("Ziel-Schreibtisch wurde nicht aktiv.".into());
            }
            // Auf dem echten Ziel ist die Miniatur kein Desktop-im-Desktop.
            VORSCHAU_AUF_NOKI.store(false, Ordering::Relaxed);
            vorschau::spaces(vorschau_spaces(r.id, false));
            f
        };
        let nachher = cgs::aktiver_space().map(|(a, _)| a).unwrap_or(0);
        let erreicht = fehler.is_none() && nachher == r.id;
        serde_json::json!({
            "ok": erreicht,
            "grund": if erreicht { String::new() }
                     else { fehler.unwrap_or_else(|| "Ziel-Schreibtisch wurde nicht aktiv.".into()) },
            "aktiv_vorher": vorher, "aktiv_nachher": nachher,
            "arbeitsplatz": r.id, "uuid": r.uuid
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        serde_json::json!({ "ok": false, "grund": "Nur auf macOS" })
    }
}

/// After an explicit visit: make the real Space's window order equal the
/// order the Miniatur showed before the visit. Every window is on the Space
/// the user now stands on - no Space switch is possible here.
#[cfg(target_os = "macos")]
fn ankunft_ordnung_herstellen(sid: u64, vorher: &[i64]) {
    let jetzt = |sid| cgs::fenster_reihe(sid).into_iter().filter(|w| vorher.contains(w)).collect::<Vec<_>>();
    // 1. Let macOS finish ITS arrival: it re-activates the app last active
    //    on this Space right after arriving (that re-activation, landing
    //    after our first pass, was the 1-in-6 mismatch). Wait until the
    //    frontmost app is stable for 250 ms (bounded).
    let t0 = std::time::Instant::now();
    let mut vorn = blende::vorn_pid();
    let mut stabil_seit = std::time::Instant::now();
    while t0.elapsed() < Duration::from_millis(1200) {
        thread::sleep(Duration::from_millis(25));
        let v = blende::vorn_pid();
        if v != vorn { vorn = v; stabil_seit = std::time::Instant::now(); }
        if stabil_seit.elapsed() >= Duration::from_millis(250) { break; }
    }
    // 2. Bottom -> top, each step VERIFIED before the next one; at most 3
    //    passes (bounded correction, never a loop).
    let mut nachher = jetzt(sid);
    let mut schritte = 0;
    let mut durchgaenge = 0;
    while nachher != vorher && durchgaenge < 3 {
        durchgaenge += 1;
        let alle = cgs::alle_fenster();
        for wid in vorher.iter().rev() {
            let Some(pid) = alle.iter().find(|f| f.0 == *wid).map(|f| f.1) else { continue };
            // Exact window front via WindowServer (accepted as user-initiated).
            // A plain NSRunningApplication activation from Noki (a background
            // process) is ignored by macOS - measured: 0/1 exact with it.
            vorschau::fenster_vorn_nativ(pid, *wid);
            let _ = vorschau::fernbedienung::fenster_heben(pid, *wid);
            schritte += 1;
            let bis = std::time::Instant::now() + Duration::from_millis(300);
            while std::time::Instant::now() < bis && jetzt(sid).first() != Some(wid) {
                thread::sleep(Duration::from_millis(15));
            }
        }
        thread::sleep(Duration::from_millis(120));
        nachher = jetzt(sid);
    }
    virtual_workspace::trace(&format!(
        "[NAVIGATION] arrival_order preview={vorher:?} real={nachher:?} match={} steps={schritte} passes={durchgaenge} settle_ms={}",
        nachher == vorher, t0.elapsed().as_millis()));
}

/// Groesse des Schreibtischs, auf dem Nokis Arbeitsplatz liegt.
#[cfg(target_os = "macos")]
fn anzeige_flaeche() -> (i32, i32, i32, i32) {
    #[repr(C)]
    struct P {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    struct S {
        w: f64,
        h: f64,
    }
    #[repr(C)]
    struct R {
        o: P,
        s: S,
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGMainDisplayID() -> u32;
        fn CGDisplayBounds(d: u32) -> R;
    }
    unsafe {
        let r = CGDisplayBounds(CGMainDisplayID());
        (
            r.o.x as i32,
            r.o.y as i32,
            (r.s.w as i32).max(640),
            (r.s.h as i32).max(480),
        )
    }
}

/// An explicit click in Noki is allowed to make Noki the active application.
/// This changes the menu-bar identity only; it never activates a captured
/// target application and never navigates a physical Space.
#[cfg(target_os = "macos")]
pub(crate) fn noki_interaction_aktivieren(app: &tauri::AppHandle) {
    let _ = app.run_on_main_thread(|| unsafe {
        use std::ffi::c_void;
        extern "C" {
            fn objc_getClass(name: *const i8) -> *mut c_void;
            fn sel_registerName(name: *const i8) -> *const c_void;
            fn objc_msgSend();
        }
        let cls = objc_getClass(b"NSApplication\0".as_ptr() as *const _);
        if cls.is_null() { return; }
        let shared: unsafe extern "C" fn(*mut c_void, *const c_void) -> *mut c_void =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let activate: unsafe extern "C" fn(*mut c_void, *const c_void, bool) =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let nsapp = shared(cls, sel_registerName(b"sharedApplication\0".as_ptr() as *const _));
        if !nsapp.is_null() {
            activate(nsapp, sel_registerName(b"activateIgnoringOtherApps:\0".as_ptr() as *const _), true);
            let front = blende::vorn_pid();
            virtual_workspace::trace(&format!(
                "[INTERACTION] active_application=Noki verified={} front_pid={front} space_navigation=false",
                front == std::process::id() as i32,
            ));
        }
    });
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn noki_interaction_aktivieren(_app: &tauri::AppHandle) {}

/// Fenster, die Noki fuer die LAUFENDE Aufgabe beansprucht hat. Nur diese
/// darf er bewegen. Fremde Fenster des Nutzers bleiben unangetastet - auch
/// dann, wenn sie derselben App gehoeren wie ein beanspruchtes Fenster.
static TASK_FENSTER: std::sync::Mutex<Vec<i64>> = std::sync::Mutex::new(Vec::new());

/// Ein Fenster, das NOKI auf seinem Schreibtisch erzeugt hat.
///
/// Warum nicht nur die Fensternummer: sie allein ist keine Herkunft. macOS
/// vergibt Fensternummern nach einem Neustart des besitzenden Programms neu,
/// und eine gemerkte Nummer zeigte dann auf ein fremdes Fenster. Prozess und
/// Programmname zusammen machen aus der Nummer eine pruefbare Aussage:
/// "dieses eine Fenster, das ich selbst dort geoeffnet habe".
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
struct EigenesFenster {
    fenster: i64,
    pid: i32,
    app: String,
    bundle_id: String,
    titel: String,
    created_by_noki: bool,
    explicitly_assigned_to_noki: bool,
    last_virtual_frame: Option<[f64; 4]>,
    desired_frame: Option<[f64; 4]>,
    /// Previous non-maximized geometry. `Some` means Noki Workspace
    /// maximize is active; this never maps to macOS native fullscreen.
    maximized_from: Option<[f64; 4]>,
    last_z_order: i64,
    restoration_kind: String,
    restoration_target: String,
    application_path: String,
    visible: bool,
    lifecycle_state: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct WorkspaceRegistry {
    version: u32,
    backend: String,
    windows: Vec<EigenesFenster>,
    // JSON/serde_json's regular number path does not deserialize u128.
    // The old type serialized successfully but every V2 load then failed
    // the untagged enum match, silently turning cold start into an empty
    // Workspace. Unix milliseconds fit in u64 for centuries.
    last_healthy_unix_ms: u64,
}
impl Default for WorkspaceRegistry {
    fn default() -> Self {
        Self { version: 2, backend: "VIRTUAL_DISPLAY_BACKEND".into(), windows: vec![], last_healthy_unix_ms: 0 }
    }
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum RegistryDisk {
    V2(WorkspaceRegistry),
    Legacy(Vec<EigenesFenster>),
}

/// Fenster, die auf Nokis reserviertem Schreibtisch von Noki erzeugt wurden.
///
/// Diese Liste ist der INHALT des Schreibtischs und lebt laenger als eine
/// Aufgabe. Eine beendete Aufgabe raeumt ihre Fenster nicht weg - der
/// Schreibtisch behaelt, was auf ihm steht, bis das Fenster wirklich
/// geschlossen wird. `TASK_FENSTER` ist das Gegenstueck: nur die laufende
/// Aufgabe. Beide zusammen zu leeren hiesse, den Schreibtisch mit der
/// Aufgabe zu verwechseln.
///
/// Befuellt wird sie ausschliesslich aus dem Vorher/Nachher-Vergleich beim
/// eigenen Start - nie durch ein Scannen aller Fenster auf dem Space.
static ARBEITSPLATZ_FENSTER: std::sync::Mutex<Vec<EigenesFenster>> =
    std::sync::Mutex::new(Vec::new());

/// Short-lived proof that a window launch was explicitly requested from the
/// Noki UI. Merely observing a new window is never ownership evidence.
#[cfg(target_os = "macos")]
#[derive(Debug)]
struct NokiLaunchTransaction {
    id: u64,
    target_app: String,
    target_bundle: String,
    expected_pid: Option<i32>,
    started: std::time::Instant,
    deadline: std::time::Instant,
    before: std::collections::HashMap<i64, bool>,
    claimed: Vec<i64>,
}

#[cfg(target_os = "macos")]
static NOKI_LAUNCH_TRANSACTION: std::sync::Mutex<Option<NokiLaunchTransaction>> =
    std::sync::Mutex::new(None);
#[cfg(target_os = "macos")]
static NOKI_LAUNCH_SEQUENCE: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(1);

#[cfg(target_os = "macos")]
struct NokiLaunchGuard { id: u64 }

#[cfg(target_os = "macos")]
impl NokiLaunchGuard {
    fn begin(target_app: &str, target_bundle: &str) -> Self {
        let id = NOKI_LAUNCH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let now = std::time::Instant::now();
        let before = cgs::alle_fenster().into_iter()
            .map(|f| (f.0, cgs::fenster_eingeordnet(f.0))).collect();
        let expected_pid = (!target_bundle.is_empty())
            .then(|| lesezeichen::pid_fuer_bundle(target_bundle)).flatten();
        let tx = NokiLaunchTransaction {
            id, target_app: target_app.to_owned(), target_bundle: target_bundle.to_owned(),
            expected_pid, started: now, deadline: now + Duration::from_secs(12),
            before, claimed: Vec::new(),
        };
        if let Ok(mut active) = NOKI_LAUNCH_TRANSACTION.lock() { *active = Some(tx); }
        virtual_workspace::trace(&format!(
            "[OWNERSHIP] launch_begin id={id} app={target_app} bundle={target_bundle} origin=NOKI timeout_ms=12000"
        ));
        Self { id }
    }
}

#[cfg(target_os = "macos")]
impl Drop for NokiLaunchGuard {
    fn drop(&mut self) {
        if let Ok(mut active) = NOKI_LAUNCH_TRANSACTION.lock() {
            if active.as_ref().is_some_and(|tx| tx.id == self.id) {
                if let Some(tx) = active.take() {
                    virtual_workspace::trace(&format!(
                        "[OWNERSHIP] launch_end id={} app={} claimed={:?} age_ms={}",
                        tx.id, tx.target_app, tx.claimed, tx.started.elapsed().as_millis()
                    ));
                }
            }
        }
    }
}

/// Does the app have an ordered-in window on ANOTHER Desktop (not the
/// current one, not Noki's)? AX lists only current-Space windows.
#[cfg(target_os = "macos")]
fn fenster_auf_anderen_spaces(pid: i32) -> bool {
    let d = virtual_workspace::info();
    let ax = vorschau::fernbedienung::ax_fenster_rahmen(pid).into_iter()
        .filter(|r| r[2] >= 200.0 && r[3] >= 150.0).count();
    let cg = cgs::alle_fenster().into_iter().filter(|f| f.1 == pid && f.6 >= 200 && f.7 >= 150
        && !d.as_ref().is_some_and(|d| f.4 >= d.x as i64 && f.5 >= d.y as i64)
        && cgs::fenster_eingeordnet(f.0)).count();
    cg > ax
}

/// Does the app show a window on the user's CURRENT Desktop (main display)?
/// AX lists only current-Space windows (measured), so any AX window frame
/// on the main display proves it.
#[cfg(target_os = "macos")]
fn app_auf_aktuellem_schreibtisch(pid: i32) -> bool {
    let (mx, my, mw, mh) = anzeige_flaeche();
    vorschau::fernbedienung::ax_fenster_rahmen(pid).into_iter().any(|r|
        r[2] >= 200.0 && r[3] >= 150.0 && r[0] >= mx as f64 - 50.0 && r[1] >= my as f64 - 50.0
            && r[0] < (mx + mw) as f64 && r[1] < (my + mh) as f64)
}

/// The app's own "New Window" for a running app, moved in the creation
/// callback (see `kind_fenster::erzeugung`), verified, registered.
#[cfg(target_os = "macos")]
fn menue_neues_fenster(app: &tauri::AppHandle, pid: i32, pfad: &str, name: &str) -> Result<String, String> {
    let Some(display) = virtual_workspace::info() else { return Err("Noki Schreibtisch ist nicht bereit.".into()) };
    let vorn_vorher = blende::vorn_pid();
    let nutzer_fenster = if vorn_vorher == pid { vorschau::fernbedienung::fokus_fenster(pid) } else { None };
    let versatz = 40.0 * (workspace_registry_refresh(app).len() % 5) as f64;
    let rahmen = [display.x as f64 + 60.0 + versatz, display.y as f64 + 50.0 + versatz,
                  display.width as f64 * 0.62, display.height as f64 * 0.7];
    let vorher: std::collections::HashMap<i64, bool> = cgs::alle_fenster().into_iter()
        .filter(|f| f.1 == pid).map(|f| (f.0, cgs::fenster_eingeordnet(f.0))).collect();
    kind_fenster::erzeugung(pid, rahmen);
    thread::sleep(Duration::from_millis(30)); // observer registered on its run loop
    let gedrueckt = vorschau::fernbedienung::menue_punkt(pid,
        &["Ablage", "Datei", "File", "Shell", "Fenster", "Window"],
        &["Neues Fenster", "New Window", "Neues Fenster öffnen", "Neue Sitzung in neuem Fenster",
          "New Session in New Window", "Neues Finder-Fenster", "New Finder Window",
          "Neues Dokument", "New Document", "Neu", "New"]);
    if !gedrueckt {
        kind_fenster::erzeugung_ende(pid);
        virtual_workspace::trace(&format!("[LEISTE] open app={name} route=menu_new_window result=no_menu_item"));
        return Err(format!("{name} bietet kein „Neues Fenster“ an und läuft bereits mit deinem Fenster – Noki nimmt dir deins nicht weg."));
    }
    let t0 = std::time::Instant::now();
    let mut neu = None;
    while t0.elapsed() < Duration::from_secs(5) && neu.is_none() {
        neu = cgs::alle_fenster().into_iter().find(|f| f.1 == pid && f.6 >= 300 && f.7 >= 200
            && vorher.get(&f.0).copied() != Some(true) && cgs::fenster_eingeordnet(f.0));
        if neu.is_none() { thread::sleep(Duration::from_millis(10)); }
    }
    let Some(f) = neu else {
        kind_fenster::erzeugung_ende(pid);
        if vorn_vorher > 0 && vorn_vorher != pid && blende::vorn_pid() == pid { blende::vorn_zurueck(vorn_vorher); }
        virtual_workspace::trace(&format!("[LEISTE] open app={name} route=menu_new_window result=no_window"));
        return Err(format!("{name} hat kein neues Fenster geöffnet."));
    };
    let wid = f.0;
    let mut auf_noki = false;
    for i in 0..60 {
        auf_noki = cgs::alle_fenster().iter().any(|g| g.0 == wid && g.4 >= display.x as i64 && g.5 >= display.y as i64
            && g.4 < (display.x + display.width) as i64 && g.5 < (display.y + display.height) as i64);
        if auf_noki { break; }
        if i % 5 == 4 { let _ = vorschau::fernbedienung::ax_verschieben(pid, wid, rahmen[0], rahmen[1], rahmen[2], rahmen[3]); }
        thread::sleep(Duration::from_millis(20));
    }
    kind_fenster::erzeugung_ende(pid);
    vordergrund_bewahren(pid, vorn_vorher, nutzer_fenster, wid);
    virtual_workspace::trace(&format!("[LEISTE] open app={name} route=menu_new_window_callback_move wid={wid} on_noki_display={auf_noki} user_windows_touched=false space_trip=false"));
    if !auf_noki {
        let _ = vorschau::fernbedienung::fenster_schliessen(pid, wid);
        return Err(format!("{name}: das neue Fenster ließ sich nicht auf Noki Schreibtisch holen."));
    }
    let ist = cgs::alle_fenster().into_iter().find(|g| g.0 == wid).map(|g| [g.4 as f64, g.5 as f64, g.6 as f64, g.7 as f64]).unwrap_or(rahmen);
    if !virtuellen_registry_eintrag_sichern(app, wid, pid, pfad, pfad, ist) {
        let _ = vorschau::fernbedienung::fenster_schliessen(pid, wid);
        return Err(format!("{name}: neues Fenster hatte keinen gültigen Noki-Launch-Nachweis."));
    }
    if let Ok(mut r) = ARBEITSPLATZ_FENSTER.lock() {
        if let Some(e) = r.iter_mut().find(|e| e.fenster == wid && e.pid == pid) { e.restoration_kind = "app".into(); }
    }
    arbeitsplatz_fenster_sichern(app);
    vorschau_neu_setzen(app);
    let live = workspace_registry_refresh(app);
    let _ = noki_fenster_vorn(pid, wid, &live);
    Ok(format!("{name} ist jetzt im Noki Schreibtisch"))
}

/// A second process instance started by THIS transaction has a new pid.
#[cfg(target_os = "macos")]
fn launch_transaction_neue_pid(pid: i32) {
    if let Ok(mut g) = NOKI_LAUNCH_TRANSACTION.lock() {
        if let Some(tx) = g.as_mut() { tx.expected_pid = Some(pid); }
    }
}

/// Noki's own second instances (pid) - terminated when their last Noki
/// window closes; never the user's instance.
static NOKI_INSTANZEN: std::sync::Mutex<Vec<i32>> = std::sync::Mutex::new(Vec::new());

#[cfg(target_os = "macos")]
fn zweite_instanz_starten(app: &tauri::AppHandle, pfad: &str, name: &str, bundle: &str) -> Result<String, String> {
    let Some(display) = virtual_workspace::info() else { return Err("Noki Schreibtisch ist nicht bereit.".into()) };
    let vorher: Vec<i32> = lesezeichen::pids_fuer_bundle(bundle);
    let _ = std::process::Command::new("/usr/bin/open").args(["-n", "-g", "-j", "-F", "-a", pfad]).status();
    let t0 = std::time::Instant::now();
    let mut neu = None;
    while t0.elapsed() < Duration::from_secs(10) && neu.is_none() {
        thread::sleep(Duration::from_millis(40));
        let pid = lesezeichen::pids_fuer_bundle(bundle).into_iter().find(|p| !vorher.contains(p));
        if let Some(pid) = pid {
            neu = cgs::alle_fenster().into_iter()
                .find(|f| f.1 == pid && f.6 >= 300 && f.7 >= 200 && !f.3.trim().is_empty())
                .map(|f| (pid, f.0, f.6, f.7));
        }
    }
    let Some((pid, wid, w, h)) = neu else {
        virtual_workspace::trace(&format!("[LEISTE] open app={name} route=second_instance result=no_window"));
        return Err(format!("{name}: die zweite, unsichtbare Instanz hat kein Fenster geöffnet."));
    };
    if let Ok(mut g) = NOKI_INSTANZEN.lock() { g.push(pid); }
    launch_transaction_neue_pid(pid);
    let versatz = 40.0 * (workspace_registry_refresh(app).len() % 5) as f64;
    let rahmen = [display.x as f64 + 60.0 + versatz, display.y as f64 + 50.0 + versatz,
                  (w as f64).min(display.width as f64 * 0.8), (h as f64).min(display.height as f64 * 0.8)];
    // Still hidden: move first, verify, only then unhide.
    let mut auf_noki = false;
    for _ in 0..40 {
        let _ = vorschau::fernbedienung::ax_verschieben(pid, wid, rahmen[0], rahmen[1], rahmen[2], rahmen[3]);
        thread::sleep(Duration::from_millis(25));
        auf_noki = cgs::alle_fenster().iter().any(|f| f.0 == wid && f.4 >= display.x as i64 && f.5 >= display.y as i64
            && f.4 < (display.x + display.width) as i64 && f.5 < (display.y + display.height) as i64);
        if auf_noki { break; }
    }
    virtual_workspace::trace(&format!("[LEISTE] open app={name} route=second_instance pid={pid} wid={wid} on_noki_display={auf_noki} user_instance_touched=false"));
    if !auf_noki {
        let _ = std::process::Command::new("/bin/kill").args(["-TERM", &pid.to_string()]).status();
        return Err(format!("{name}: das Fenster der zweiten Instanz ließ sich nicht auf Noki Schreibtisch holen."));
    }
    let ist = cgs::alle_fenster().into_iter().find(|f| f.0 == wid).map(|f| [f.4 as f64, f.5 as f64, f.6 as f64, f.7 as f64]).unwrap_or(rahmen);
    if !virtuellen_registry_eintrag_sichern(app, wid, pid, pfad, pfad, ist) {
        let _ = std::process::Command::new("/bin/kill").args(["-TERM", &pid.to_string()]).status();
        return Err(format!("{name}: Fenster der zweiten Instanz hatte keinen gültigen Noki-Launch-Nachweis."));
    }
    if let Ok(mut r) = ARBEITSPLATZ_FENSTER.lock() {
        if let Some(e) = r.iter_mut().find(|e| e.fenster == wid && e.pid == pid) { e.restoration_kind = "second_instance".into(); }
    }
    arbeitsplatz_fenster_sichern(app);
    let _ = vorschau::fernbedienung::fenster_zeigen(pid, wid);
    vorschau_neu_setzen(app);
    let live = workspace_registry_refresh(app);
    let _ = noki_fenster_vorn(pid, wid, &live);
    Ok(format!("{name} ist jetzt im Noki Schreibtisch (eigene Noki-Instanz)"))
}

#[cfg(target_os = "macos")]
fn launch_transaction_claims(fenster: i64, pid: i32, bundle: &str) -> bool {
    let Ok(mut active) = NOKI_LAUNCH_TRANSACTION.lock() else { return false };
    let Some(tx) = active.as_mut() else {
        virtual_workspace::trace(&format!(
            "[OWNERSHIP] reject wid={fenster} pid={pid} bundle={bundle} reason=no_noki_launch_transaction origin=USER"
        ));
        return false;
    };
    if std::time::Instant::now() > tx.deadline {
        virtual_workspace::trace(&format!(
            "[OWNERSHIP] reject wid={fenster} pid={pid} bundle={bundle} reason=expired_transaction id={}", tx.id
        ));
        *active = None;
        return false;
    }
    let bundle_ok = tx.target_bundle.is_empty() || tx.target_bundle == bundle
        || (tx.target_bundle.starts_with("com.google.Chrome.app.") && bundle == "com.google.Chrome")
        || (tx.target_bundle.starts_with("com.microsoft.edgemac.app.") && bundle == "com.microsoft.edgemac");
    let pid_ok = tx.expected_pid.is_none_or(|expected| expected == pid);
    let causal_window = tx.before.get(&fenster).copied() != Some(true);
    let unclaimed = !tx.claimed.contains(&fenster);
    let ok = bundle_ok && pid_ok && causal_window && unclaimed;
    virtual_workspace::trace(&format!(
        "[OWNERSHIP] correlate id={} wid={fenster} pid={pid} bundle={bundle} bundle_ok={bundle_ok} pid_ok={pid_ok} causal={causal_window} result={ok}",
        tx.id
    ));
    if ok { tx.claimed.push(fenster); }
    ok
}

fn arbeitsplatz_fenster_datei(app: &tauri::AppHandle) -> Option<PathBuf> {
    let dir = app.path().app_config_dir().ok()?;
    let _ = fs::create_dir_all(&dir);
    Some(dir.join("arbeitsplatz-fenster.json"))
}

/// Der Schreibtisch ueberlebt den Programmstart.
///
/// Ohne diese Datei war die Miniatur nach jedem Noki-Neustart leer, obwohl
/// auf Schreibtisch 4 noch ein echtes Fenster stand: die Herkunft lebte nur
/// im Arbeitsspeicher. Genau das war der gemeldete Fehler "Fenster da, aber
/// nicht in der Vorschau". Gespeichert wird die HERKUNFT, nicht der Inhalt.
fn arbeitsplatz_fenster_laden(app: &tauri::AppHandle) {
    let Some(datei) = arbeitsplatz_fenster_datei(app) else {
        virtual_workspace::trace("[REGISTRY] load failed reason=no_config_path");
        return;
    };
    let roh = match fs::read_to_string(&datei) {
        Ok(roh) => roh,
        Err(error) => {
            virtual_workspace::trace(&format!(
                "[REGISTRY] load failed path={} reason={error}", datei.display()
            ));
            return;
        }
    };
    let disk = match serde_json::from_str::<RegistryDisk>(&roh) {
        Ok(disk) => disk,
        Err(error) => {
            virtual_workspace::trace(&format!(
                "[REGISTRY] load failed path={} reason=parse:{error}", datei.display()
            ));
            return;
        }
    };
    let liste = match disk { RegistryDisk::V2(r) => r.windows, RegistryDisk::Legacy(v) => v };
    if let Ok(mut g) = ARBEITSPLATZ_FENSTER.lock() {
        *g = liste;
    }
    virtual_workspace::trace(&format!("[REGISTRY] loaded windows={}",
        ARBEITSPLATZ_FENSTER.lock().map(|g| g.len()).unwrap_or(0)));
}

fn arbeitsplatz_fenster_sichern(app: &tauri::AppHandle) {
    let (Some(datei), Ok(g)) = (arbeitsplatz_fenster_datei(app), ARBEITSPLATZ_FENSTER.lock()) else {
        return;
    };
    let registry = WorkspaceRegistry {
        windows: g.clone(),
        last_healthy_unix_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis().min(u64::MAX as u128) as u64).unwrap_or(0),
        ..WorkspaceRegistry::default()
    };
    if let Ok(text) = serde_json::to_string_pretty(&registry) {
        // Never expose a truncated/half-written registry to the next cold
        // start.  `fs::write(datei, ..)` truncates first; a SIGTERM/SIGKILL
        // in that small window previously turned a healthy Workspace into
        // an apparently legitimate empty one.  Same-directory rename is
        // atomic on APFS and keeps the previous complete generation until
        // the replacement is fully written.
        let tmp = datei.with_extension("json.tmp");
        if fs::write(&tmp, text).is_ok() {
            if let Err(error) = fs::rename(&tmp, &datei) {
                virtual_workspace::trace(&format!(
                    "[REGISTRY] save failed path={} reason={error}", datei.display()
                ));
                let _ = fs::remove_file(tmp);
            }
        }
    }
}

/// Last known frame of a registered Noki window.
pub(crate) fn fenster_rahmen(wid: i64) -> Option<[f64; 4]> {
    arbeitsplatz_fenster_liste().into_iter().find(|e| e.fenster == wid && wid != 0).and_then(|e| e.last_virtual_frame)
}

/// Register windows that `kind_fenster` moved onto Noki's display as
/// children of the operated Noki window, and bring them to the front.
#[cfg(target_os = "macos")]
pub(crate) fn kind_fenster_anstossen() {
    thread::spawn(|| {
        thread::sleep(Duration::from_millis(150));
        let Some(app) = WACHE_APP.get() else { return };
        let mut neu = false;
        for (pid, wid, parent) in kind_fenster::erfasste() {
            let ok = workspace_child_window_registrieren(app, parent, wid, pid);
            virtual_workspace::trace(&format!("[OWNERSHIP] child_registered wid={wid} parent={parent} ok={ok}"));
            if ok {
                neu = true;
                let live = workspace_registry_refresh(app);
                let _ = noki_fenster_vorn(pid, wid, &live);
            }
        }
        if neu { vorschau_neu_setzen(app); leiste_anstossen(app); }
    });
}

/// PID of a registered Noki window (0 if unknown).
pub(crate) fn fenster_pid(wid: i64) -> i32 {
    arbeitsplatz_fenster_liste().into_iter().find(|e| e.fenster == wid && wid != 0).map(|e| e.pid).unwrap_or(0)
}

fn arbeitsplatz_fenster_liste() -> Vec<EigenesFenster> {
    ARBEITSPLATZ_FENSTER.lock().map(|g| g.clone()).unwrap_or_default()
}

/// Persist geometry only for the exact registered Noki window. Called by
/// the virtual preview's AX window-drag route after its final live update.
#[cfg(target_os = "macos")]
pub(crate) fn workspace_fenster_geometrie(app: &tauri::AppHandle, wid: i64, frame: [f64; 4]) {
    let mut changed = false;
    if let Ok(mut registry) = ARBEITSPLATZ_FENSTER.lock() {
        if let Some(entry) = registry.iter_mut().find(|e| e.fenster == wid && e.fenster != 0) {
            entry.desired_frame = Some(frame);
            entry.last_virtual_frame = Some(frame);
            // A manual move leaves workspace-maximized mode.
            entry.maximized_from = None;
            changed = true;
        }
    }
    if changed { arbeitsplatz_fenster_sichern(app); }
}

#[cfg(target_os = "macos")]
fn virtuellen_registry_eintrag_sichern(
    app: &tauri::AppHandle,
    fenster: i64,
    pid: i32,
    ziel: &str,
    programm: &str,
    frame: [f64; 4],
) -> bool {
    let observed = cgs::alle_fenster();
    let detail = observed.iter().find(|(wid, p, ..)| *wid == fenster && *p == pid);
    let (name, bundle_id, application_path) = lesezeichen::app_fuer_pid(pid)
        .unwrap_or_else(|| (String::new(), String::new(), programm.to_owned()));
    if !launch_transaction_claims(fenster, pid, &bundle_id) {
        return false;
    }
    let (owner, titel, z) = detail
        .map(|(_, _, owner, title, _, _, _, _, z)| (owner.clone(), title.clone(), *z))
        .unwrap_or_default();
    let entry = EigenesFenster {
        fenster,
        pid,
        app: if name.is_empty() { owner } else { name },
        bundle_id,
        titel,
        created_by_noki: true,
        explicitly_assigned_to_noki: false,
        last_virtual_frame: Some(frame),
        desired_frame: Some(frame),
        maximized_from: None,
        last_z_order: z,
        restoration_kind: "browser_url".into(),
        restoration_target: ziel.to_owned(),
        application_path: if application_path.is_empty() { programm.to_owned() } else { application_path },
        visible: true,
        lifecycle_state: "live".into(),
    };
    if let Ok(mut registry) = ARBEITSPLATZ_FENSTER.lock() {
        if let Some(old) = registry.iter_mut().find(|e| e.fenster == fenster && e.pid == pid) {
            *old = entry;
        } else {
            registry.push(entry);
        }
    }
    if let Ok(mut task) = TASK_FENSTER.lock() {
        if !task.contains(&fenster) { task.push(fenster); }
    }
    arbeitsplatz_fenster_sichern(app);
    true
}

/// A window created by a drag/session inside an already-owned Noki window
/// inherits ownership only when its exact parent, PID, bundle and virtual
/// display placement are all proven. This is the child-window exception to
/// the normal launch-transaction rule (for example a detached Chrome tab).
#[cfg(target_os = "macos")]
pub(crate) fn workspace_child_window_registrieren(
    app: &tauri::AppHandle, parent_wid: i64, child_wid: i64, pid: i32,
) -> bool {
    if parent_wid <= 0 || child_wid <= 0 || parent_wid == child_wid || pid <= 0 { return false; }
    let Some(parent) = arbeitsplatz_fenster_liste().into_iter()
        .find(|e| e.fenster == parent_wid && e.pid == pid && e.lifecycle_state == "live")
    else { return false };
    let Some((_, child_pid, owner, title, x, y, w, h, z)) = cgs::alle_fenster().into_iter()
        .find(|f| f.0 == child_wid && f.1 == pid) else { return false };
    let Some(display) = virtual_workspace::info() else { return false };
    if x < display.x as i64 || y < display.y as i64
        || x >= (display.x + display.width) as i64 || y >= (display.y + display.height) as i64 {
        return false;
    }
    let (name, bundle, path) = lesezeichen::app_fuer_pid(child_pid)
        .unwrap_or_else(|| (owner.clone(), String::new(), parent.application_path.clone()));
    if !parent.bundle_id.is_empty() && bundle != parent.bundle_id { return false; }
    // Real windows only (see kind_fenster): popovers, tooltips and help
    // tags must never become separate Noki windows.
    let (rolle, sub) = vorschau::fernbedienung::fenster_art(child_pid, child_wid);
    let echt = rolle == "AXSheet" || (rolle == "AXWindow"
        && matches!(sub.as_str(), "AXStandardWindow" | "AXDialog" | "AXSystemDialog"));
    if !echt {
        virtual_workspace::trace(&format!("[OWNERSHIP] child_rejected wid={child_wid} pid={pid} role={rolle}/{sub} reason=not_a_real_window"));
        return false;
    }
    let frame = [x as f64, y as f64, w as f64, h as f64];
    let child = EigenesFenster {
        fenster: child_wid, pid: child_pid,
        app: if name.is_empty() { owner } else { name }, bundle_id: bundle, titel: title,
        created_by_noki: true, explicitly_assigned_to_noki: false,
        last_virtual_frame: Some(frame), desired_frame: Some(frame), maximized_from: None,
        last_z_order: z, restoration_kind: "child_window".into(),
        restoration_target: parent.restoration_target,
        application_path: if path.is_empty() { parent.application_path } else { path },
        visible: true, lifecycle_state: "live".into(),
    };
    if let Ok(mut registry) = ARBEITSPLATZ_FENSTER.lock() {
        if registry.iter().any(|e| e.fenster == child_wid && e.pid == child_pid) { return true; }
        registry.push(child);
    } else { return false; }
    arbeitsplatz_fenster_sichern(app);
    virtual_workspace::trace(&format!(
        "[OWNERSHIP] child_claim parent={parent_wid} wid={child_wid} pid={pid} cause=explicit_noki_interaction"
    ));
    true
}

#[cfg(target_os = "macos")]
fn browser_executable(programm: &str) -> Option<PathBuf> {
    let app = PathBuf::from(programm);
    let name = app.file_stem()?.to_str()?;
    if !app.is_dir() || capability::adapter_for(name).is_none_or(|a| !capability::is_browser_key(a.key)) {
        return None;
    }
    let exe = app.join("Contents/MacOS").join(name);
    exe.is_file().then_some(exe)
}

/// Creates one explicitly requested browser window without activating an app
/// or navigating a physical Space. A window is owned only after its new
/// WindowServer identity was observed and AX verified its virtual geometry.
#[cfg(target_os = "macos")]
fn virtuelles_browserfenster_erstellen(
    ziel: &str,
    programm: &str,
    frame: [f64; 4],
) -> Result<(i64, i32), String> {
    let exe = browser_executable(programm).ok_or("Kein sicherer Browser-Adapter.")?;
    let bundle_hint = lesezeichen::apps_mit_fenstern(false).into_iter()
        .find(|(_, _, path, _)| path == programm).map(|(_, bundle, _, _)| bundle);
    let running_pid = bundle_hint.as_deref().and_then(lesezeichen::pid_fuer_bundle)
        .ok_or("Browser läuft nicht; sicherer Hintergrundstart ist nicht nachgewiesen.")?;
    let before: std::collections::HashSet<i64> = cgs::alle_fenster().into_iter()
        .filter(|(_, pid, ..)| *pid == running_pid).map(|(wid, ..)| wid).collect();
    let origin = cgs::aktiver_space();
    let mut child = std::process::Command::new(exe)
        .arg("--new-window")
        .arg(format!("--window-position={},{}", frame[0].round() as i32, frame[1].round() as i32))
        .arg(format!("--window-size={},{}", frame[2].round() as i32, frame[3].round() as i32))
        .arg(ziel)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn().map_err(|e| format!("Browserfenster konnte nicht erzeugt werden: {e}"))?;
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while std::time::Instant::now() < deadline {
        if cgs::aktiver_space() != origin {
            let _ = child.kill();
            virtual_workspace::trace("[WORKSPACE] create aborted reason=physical_space_changed");
            return Err("Fenstererzeugung hätte den physischen Schreibtisch gewechselt.".into());
        }
        for (wid, pid, _, _, _, _, _, _, _) in cgs::alle_fenster() {
            if pid != running_pid || before.contains(&wid) { continue; }
            if vorschau::fernbedienung::ax_verschieben(pid, wid, frame[0], frame[1], frame[2], frame[3]) {
                thread::sleep(Duration::from_millis(80));
                let verified = cgs::alle_fenster().into_iter().any(|(w, p, _, _, x, y, ww, hh, _)| {
                    w == wid && p == pid && (x as f64 - frame[0]).abs() <= 3.0
                        && (y as f64 - frame[1]).abs() <= 3.0
                        && (ww as f64 - frame[2]).abs() <= 6.0
                        && (hh as f64 - frame[3]).abs() <= 6.0
                });
                if verified {
                    virtual_workspace::trace(&format!("[WORKSPACE] created wid={wid} pid={pid} geometry=verified space_trip=false"));
                    return Ok((wid, pid));
                }
            }
        }
        thread::sleep(Duration::from_millis(80));
    }
    let _ = child.kill();
    Err("Browserfenster wurde nicht sicher auf Noki Schreibtisch sichtbar.".into())
}

/// Was die Komposition zeigen kann: eingeordnete Fenster. Ein minimiertes
/// Noki-Fenster bleibt registriert (Besitz unveraendert), liefert aber kein
/// Bild - mit ihm konnte die atomare Komposition nie fertig werden, und der
/// Kaltstart endete in FAILED mit einer leeren Miniatur (gemessen).
#[cfg(target_os = "macos")]
fn vorschau_faehig(mut ids: Vec<i64>) -> Vec<i64> {
    ids.retain(|w| cgs::fenster_eingeordnet(*w));
    ids
}

#[cfg(target_os = "macos")]
fn workspace_registry_refresh(app: &tauri::AppHandle) -> Vec<i64> {
    let observed = cgs::alle_fenster();
    // Ownership never changes while the virtual display is not READY.
    // Measured: a refresh during cold-start `Initializing` (display not yet
    // recreated, windows evacuated by WindowServer) logged
    // `release ... window_left_virtual_display` for Noki's own Chrome window
    // and persisted it as closed; the window then sat unregistered on the
    // recreated display - invisible to the user and missing from Miniatur.
    if !virtual_workspace::ready() || virtual_workspace::info().is_none() {
        let mut live: Vec<i64> = arbeitsplatz_fenster_liste().into_iter()
            .filter(|e| e.fenster != 0 && observed.iter().any(|f| f.0 == e.fenster && f.1 == e.pid))
            .map(|e| e.fenster).collect();
        live.sort_unstable(); live.dedup();
        return live;
    }
    let mut live = Vec::new();
    let mut changed = false;
    if let Ok(mut registry) = ARBEITSPLATZ_FENSTER.lock() {
        for entry in registry.iter_mut() {
            let exact = observed.iter().find(|(wid, pid, ..)|
                *wid == entry.fenster && *pid == entry.pid);
            if let Some((wid,pid,owner,title,x,y,w,h,z)) = exact {
                let app_info = lesezeichen::app_fuer_pid(*pid);
                let identity_ok = entry.bundle_id.is_empty()
                    || app_info.as_ref().is_some_and(|(_, bundle, _)| bundle == &entry.bundle_id);
                let on_virtual_display = virtual_workspace::info().is_some_and(|d| {
                    *x >= d.x as i64 && *y >= d.y as i64
                        && *x < (d.x + d.width) as i64 && *y < (d.y + d.height) as i64
                });
                let minimized_from_virtual = !cgs::fenster_eingeordnet(*wid)
                    && entry.last_virtual_frame.is_some_and(|r| virtual_workspace::info().is_some_and(|d| {
                        r[0] >= d.x as f64 && r[1] >= d.y as f64
                            && r[0] < (d.x + d.width) as f64 && r[1] < (d.y + d.height) as f64
                    }));
                // Safari/Finder behalten geschlossene Fenster in der Liste:
                // nicht eingeordnet UND fuer AX nicht vorhanden = geschlossen
                // (ein minimiertes Fenster kennt AX weiterhin).
                let geschlossen = identity_ok && !cgs::fenster_eingeordnet(*wid)
                    && !vorschau::fernbedienung::fenster_vorhanden(*pid, *wid);
                if identity_ok && !geschlossen && (on_virtual_display || minimized_from_virtual) {
                    live.push(*wid);
                    let frame = [*x as f64,*y as f64,*w as f64,*h as f64];
                    if entry.titel != *title || entry.last_virtual_frame != Some(frame)
                        || entry.last_z_order != *z || entry.lifecycle_state != "live" {
                        entry.titel = title.clone(); entry.app = owner.clone();
                        entry.last_virtual_frame = Some(frame); entry.last_z_order = *z;
                        entry.visible = true; entry.lifecycle_state = "live".into(); changed = true;
                    }
                    continue;
                }
                // During a normal Noki shutdown WindowServer removes the
                // virtual display first and moves its still-live windows onto
                // a real display.  Preserve that exact pid+window+bundle
                // identity so the next startup migration can return it to the
                // newly-created virtual display.  This is not a claim of a
                // newly observed window: only an already-live Noki-created
                // registry entry qualifies.
                if identity_ok && !geschlossen && entry.created_by_noki
                    && entry.lifecycle_state == "live"
                {
                    live.push(*wid);
                    virtual_workspace::trace(&format!(
                        "[OWNERSHIP] retain wid={wid} pid={pid} reason=virtual_display_lifecycle origin=NOKI"
                    ));
                    continue;
                }
                if identity_ok && !geschlossen && !on_virtual_display && !minimized_from_virtual {
                    virtual_workspace::trace(&format!(
                        "[OWNERSHIP] release wid={wid} pid={pid} reason=window_left_virtual_display origin=USER"
                    ));
                }
            }
            if entry.fenster != 0 || entry.pid != 0 || entry.lifecycle_state != "closed" {
                entry.fenster = 0; entry.pid = 0; entry.visible = false;
                // A window that disappears while Noki is running was closed
                // by the user (or by an explicit test action).  Persist that
                // intent: cold start must not resurrect it.  Crash recovery
                // is different—the last persisted state is still `live`.
                entry.lifecycle_state = "closed".into(); changed = true;
            }
        }
    }
    if changed { arbeitsplatz_fenster_sichern(app); }
    live.sort_unstable(); live.dedup();
    live
}

/// USER_DESKTOP guard: "Schreibtisch bleibt Schreibtisch".
///
/// macOS restores a window where the app last saw it. After Noki hosted an
/// app, a plain Dock/Spotlight launch can land on Noki's invisible display
/// (measured: App Store 750866, Motion PWA). Such a window has USER origin:
/// Noki never claims it - it hands it back to the user's main display. The
/// helper reports every app launch/activation (`APPAKTIV`), so this runs
/// within ~100 ms of a normal launch instead of on a slow poll.
///
/// Windows of an app that HAS a live Noki window are different: apps place
/// dialogs/child windows next to their parent. Such a window is claimed as a
/// Noki child only when Noki itself operated that exact process within the
/// last 10 s (click/drag in the Miniatur) - causal evidence, never a
/// bundle-wide claim. Otherwise it is left untouched.
/// `alle` (startup only): every unregistered window of an app without Noki
/// windows is returned - after migration nothing Noki-owned is unregistered.
#[cfg(target_os = "macos")]
fn fremde_fenster_zurueckgeben(alle: bool) {
    static GESEHEN: std::sync::Mutex<Vec<(i64, std::time::Instant)>> = std::sync::Mutex::new(Vec::new());
    if !virtual_workspace::ready() && !alle { return; }
    let Some(d) = virtual_workspace::info() else { return };
    if NOKI_LAUNCH_TRANSACTION.lock().map(|t| t.is_some()).unwrap_or(true) { return; }
    let (mx, my, mw, mh) = anzeige_flaeche();
    // Clamshell: the virtual display can become main - never "return" onto it.
    if mx == d.x && my == d.y { return; }
    let registry = arbeitsplatz_fenster_liste();
    let eigen = std::process::id() as i32;
    let jetzt = std::time::Instant::now();
    let mut kandidaten = Vec::new();
    for (wid, pid, owner, _, x, y, w, h, _) in cgs::alle_fenster() {
        let auf_noki = x >= d.x as i64 && y >= d.y as i64
            && x < (d.x + d.width) as i64 && y < (d.y + d.height) as i64;
        if !auf_noki || w < 160 || h < 100 || pid == eigen || pid <= 0 { continue; }
        if owner.to_lowercase().contains("noki") { continue; }
        // System UI (Mission Control/Exposé surfaces are Dock windows,
        // measured: return attempts failed) is never a user app window.
        if matches!(owner.as_str(), "Dock" | "WindowManager" | "Control Center" | "Kontrollzentrum"
            | "SystemUIServer" | "Window Server" | "Notification Center" | "Mitteilungszentrale"
            | "loginwindow" | "screencaptureui" | "Screenshot" | "Bildschirmfoto") { continue; }
        if registry.iter().any(|e| e.fenster == wid && e.pid == pid) { continue; }
        if !cgs::fenster_eingeordnet(wid) { continue; }
        kandidaten.push((wid, pid, owner, x, y, w, h));
    }
    let faellig: Vec<_> = {
        let Ok(mut g) = GESEHEN.lock() else { return };
        g.retain(|(wid, _)| kandidaten.iter().any(|k| k.0 == *wid));
        for k in &kandidaten {
            if !g.iter().any(|(wid, _)| *wid == k.0) { g.push((k.0, jetzt)); }
        }
        kandidaten.into_iter().filter(|k| alle || g.iter()
            .any(|(wid, t)| *wid == k.0 && jetzt.duration_since(*t) >= Duration::from_millis(0)))
            .collect()
    };
    let mut i = 0usize;
    for (wid, pid, owner, x, y, w, h) in faellig {
        let noki_eltern = registry.iter().filter(|e| e.pid == pid && e.fenster != 0 && e.lifecycle_state == "live")
            .max_by_key(|e| -e.last_z_order).map(|e| e.fenster);
        if let Some(eltern) = noki_eltern {
            if alle { continue; }
            // Causality: the window must have APPEARED after Noki's own
            // operation on this app - never an older window that was
            // already sitting on the display.
            let erst_gesehen = GESEHEN.lock().ok().and_then(|g| g.iter().find(|e| e.0 == wid).map(|e| e.1));
            let nach_bedienung = vorschau::noki_bedient_seit(pid).zip(erst_gesehen)
                .is_some_and(|(b, e)| e + Duration::from_millis(200) >= b);
            if nach_bedienung && vorschau::noki_bedient_kuerzlich(pid, Duration::from_secs(10)) {
                if let Some(app) = WACHE_APP.get() {
                    let ok = workspace_child_window_registrieren(app, eltern, wid, pid);
                    if ok { vorschau_neu_setzen(app); leiste_anstossen(app); }
                    virtual_workspace::trace(&format!(
                        "[OWNERSHIP] child_window wid={wid} pid={pid} app={owner} parent={eltern} claimed={ok} cause=noki_operated_parent_app"
                    ));
                }
            }
            continue;
        }
        let versatz = 30.0 * (i % 6) as f64; i += 1;
        let nw = (w as f64).min(mw as f64 - 80.0);
        let nh = (h as f64).min(mh as f64 - 100.0);
        let (nx, ny) = (mx as f64 + 60.0 + versatz, my as f64 + 60.0 + versatz);
        // Verified by the real frame, not the AX return code: measured Chrome
        // 746885 acknowledged the move and stayed on the virtual display.
        let mut ok = false;
        for _ in 0..3 {
            let _ = vorschau::fernbedienung::ax_verschieben(pid, wid, nx, ny, nw, nh)
                || vorschau::fernbedienung::ax_lage(pid, wid, nx, ny);
            thread::sleep(Duration::from_millis(60));
            ok = cgs::alle_fenster().into_iter().find(|f| f.0 == wid)
                .is_some_and(|f| !(f.4 >= d.x as i64 && f.5 >= d.y as i64
                    && f.4 < (d.x + d.width) as i64 && f.5 < (d.y + d.height) as i64));
            if ok { break; }
        }
        // The app now remembers a user-display frame again.
        if let Some((_, bundle, _)) = lesezeichen::app_fuer_pid(pid) { noki_lage_vergessen(&bundle); }
        virtual_workspace::trace(&format!(
            "[OWNERSHIP] return_user_window wid={wid} pid={pid} app={owner} from={x},{y},{w}x{h} to={nx:.0},{ny:.0} ok={ok} reason={} origin=USER claimed=false",
            if alle { "unregistered_at_startup" } else { "user_launch_on_virtual_display" }
        ));
    }
}

#[cfg(target_os = "macos")]
static WACHE_APP: std::sync::OnceLock<tauri::AppHandle> = std::sync::OnceLock::new();
static WACHE_SCHNELL_BIS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn jetzt_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Helper saw an app launch/activation: check every 100 ms for 3 s.
pub(crate) fn schreibtisch_wache_wecken() {
    WACHE_SCHNELL_BIS.store(jetzt_ms() + 3000, Ordering::SeqCst);
}

/// Guard thread: 1 Hz at rest (one CGWindowList read, no capture work),
/// 10 Hz for 3 s after any app launch/activation.
#[cfg(target_os = "macos")]
fn schreibtisch_wache_starten(app: &tauri::AppHandle) {
    let _ = WACHE_APP.set(app.clone());
    static GESTARTET: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    GESTARTET.get_or_init(|| {
        let _ = thread::Builder::new().name("noki-schreibtisch-wache".into()).spawn(|| {
            let mut letzte = std::time::Instant::now();
            loop {
                thread::sleep(Duration::from_millis(100));
                let schnell = jetzt_ms() < WACHE_SCHNELL_BIS.load(Ordering::SeqCst);
                if !schnell && letzte.elapsed() < Duration::from_millis(1000) { continue; }
                letzte = std::time::Instant::now();
                let _ = std::panic::catch_unwind(|| fremde_fenster_zurueckgeben(false));
            }
        });
    });
}

/// Apps remember the frame of their last closed window (Spotify reuses the
/// same window, Chrome PWAs store a placement). When Noki closes its own
/// window it now leaves it on Noki's display, so the app's NEXT Noki open is
/// born there - zero exposure on the user's desktop. This memo records that
/// fact per bundle until the app shows a window to the user again.
static NOKI_LAGE_MEMO: std::sync::Mutex<Option<Vec<String>>> = std::sync::Mutex::new(None);
fn noki_lage_datei() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join("Library/Application Support/com.noki.desktop/noki-lage.json")
}
fn noki_lage_mit<R>(f: impl FnOnce(&mut Vec<String>) -> (R, bool)) -> Option<R> {
    let mut g = NOKI_LAGE_MEMO.lock().ok()?;
    let liste = g.get_or_insert_with(|| fs::read_to_string(noki_lage_datei()).ok()
        .and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default());
    let (r, geaendert) = f(liste);
    if geaendert {
        if let Ok(t) = serde_json::to_string(liste) { let _ = fs::write(noki_lage_datei(), t); }
    }
    Some(r)
}
fn noki_lage_merken(bundle: &str) {
    if bundle.is_empty() { return; }
    noki_lage_mit(|l| { let neu = !l.iter().any(|b| b == bundle); if neu { l.push(bundle.to_owned()); } ((), neu) });
}
fn noki_lage_vergessen(bundle: &str) {
    noki_lage_mit(|l| { let n = l.len(); l.retain(|b| b != bundle); ((), l.len() != n) });
}
fn noki_lage_gemerkt(bundle: &str) -> bool {
    noki_lage_mit(|l| (l.iter().any(|b| b == bundle), false)).unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn virtuelle_fenster_migrieren(app: &tauri::AppHandle) -> (Vec<i64>, usize) {
    if virtual_workspace::backend() != virtual_workspace::Backend::LegacyVirtualDisplay { return (vec![], 0); }
    let Some(display) = virtual_workspace::info() else { return (vec![], 0); };
    let mut entries = arbeitsplatz_fenster_liste();
    let expected = entries.iter().filter(|entry|
        !matches!(entry.lifecycle_state.as_str(), "closed" | "missing")
    ).count();
    let observed = cgs::alle_fenster();
    let mut migrated = Vec::new();
    let mut app_geschlossen = 0usize;
    for (index, entry) in entries.iter_mut().enumerate() {
        if matches!(entry.lifecycle_state.as_str(), "closed" | "missing") {
            continue;
        }
        let exact = observed.iter().find(|(wid,pid,..)| *wid == entry.fenster && *pid == entry.pid);
        // Dialogs/sheets belong to one session of their parent: never
        // re-adopted at cold start (stale child entries were the source of
        // tiny ghost windows).
        if entry.restoration_kind == "child_window" {
            entry.fenster = 0; entry.pid = 0; entry.visible = false;
            entry.lifecycle_state = "closed".into();
            app_geschlossen += 1;
            continue;
        }
        if exact.is_none() && matches!(entry.restoration_kind.as_str(), "app" | "second_instance") {
            // Ein von Noki geoeffnetes Programmfenster, das nicht mehr
            // existiert (Programm beendet), ist geschlossen - kein Fehler,
            // und es wird nicht eigenmaechtig neu gestartet.
            entry.fenster = 0; entry.pid = 0; entry.visible = false;
            entry.lifecycle_state = "closed".into();
            app_geschlossen += 1;
            continue;
        }
        if exact.is_none() {
            entry.fenster = 0; entry.pid = 0; entry.visible = false;
            if entry.created_by_noki && entry.restoration_kind == "browser_url"
                && !entry.restoration_target.is_empty() && !entry.application_path.is_empty() {
                let fallback_offset = ((index % 4) as f64) * (display.width as f64 / 22.0);
                let fallback = [display.x as f64 + display.width as f64 / 14.0 + fallback_offset,
                    display.y as f64 + display.height as f64 / 12.0 + fallback_offset,
                    display.width as f64 * 0.62, display.height as f64 * 0.66];
                let desired = entry.desired_frame.unwrap_or(fallback);
                let _ = desired;
                // Both tested Chrome creation routes (direct --new-window
                // and non-activating AppleScript) can switch the user's
                // physical Space before a new WindowServer id exists.  A
                // post-hoc origin check is too late.  Until an adapter can
                // prove background creation, fail before launching anything.
                entry.lifecycle_state = "restore_failed_unsafe_create".into();
                virtual_workspace::trace(&format!(
                    "[REGISTRY] restore failed app={} reason=unsafe_background_create_blocked space_trip=false",
                    entry.app
                ));
            } else {
                entry.lifecycle_state = if entry.restoration_target.is_empty() {
                    "restore_blocked_no_intent".into()
                } else { "restore_pending".into() };
            }
            continue;
        }
        let (wid,pid,owner,title,fx,fy,fw,fh,z) = exact.unwrap();
        let app_info = lesezeichen::app_fuer_pid(*pid);
        if !entry.bundle_id.is_empty()
            && !app_info.as_ref().is_some_and(|(_,bundle,_)| bundle == &entry.bundle_id) {
            entry.lifecycle_state = "identity_mismatch".into(); continue;
        }
        // Steht das Fenster schon auf Noki Schreibtisch (auch im Vollbild,
        // z. B. ein YouTube-Video) oder ist es dort minimiert, gibt es nichts
        // zu verschieben. Gemessen: ein erzwungenes Verschieben scheiterte
        // genau dann, und der Kaltstart endete mit zwei "Fehlern" in FAILED.
        let schon_dort = *fx >= display.x as i64 && *fy >= display.y as i64
            && *fx + *fw <= (display.x + display.width) as i64
            && *fy + *fh <= (display.y + display.height) as i64;
        if schon_dort || !cgs::fenster_eingeordnet(*wid) {
            migrated.push(*wid); entry.fenster = *wid; entry.pid = *pid;
            entry.app = owner.clone(); entry.titel = title.clone(); entry.last_z_order = *z;
            if schon_dort { entry.last_virtual_frame = Some([*fx as f64, *fy as f64, *fw as f64, *fh as f64]); }
            entry.visible = true; entry.lifecycle_state = "live".into();
            continue;
        }
        let fallback_offset = ((index % 4) as f64) * (display.width as f64 / 22.0);
        let fallback = [display.x as f64 + display.width as f64 / 14.0 + fallback_offset,
            display.y as f64 + display.height as f64 / 12.0 + fallback_offset,
            display.width as f64 * 0.62, display.height as f64 * 0.66];
        let mut desired = entry.desired_frame.unwrap_or(fallback);
        // Old physical-space geometry is converted into a safe virtual slot.
        if desired[0] < display.x as f64 || desired[1] < display.y as f64
            || desired[0] + desired[2] > (display.x + display.width) as f64
            || desired[1] + desired[3] > (display.y + display.height) as f64 { desired = fallback; }
        if vorschau::fernbedienung::ax_verschieben(*pid, *wid,
            desired[0],desired[1],desired[2],desired[3]) {
            migrated.push(*wid); entry.fenster = *wid; entry.pid = *pid;
            entry.app = owner.clone(); entry.titel = title.clone(); entry.last_z_order = *z;
            entry.desired_frame = Some(desired); entry.last_virtual_frame = Some(desired);
            entry.visible = true; entry.lifecycle_state = "live".into();
        } else { entry.lifecycle_state = "move_failed".into(); }
    }
    if let Ok(mut registry) = ARBEITSPLATZ_FENSTER.lock() { *registry = entries; }
    arbeitsplatz_fenster_sichern(app);
    if let Ok(mut task) = TASK_FENSTER.lock() {
        for wid in &migrated { if !task.contains(wid) { task.push(*wid); } }
    }
    let failures = expected.saturating_sub(migrated.len() + app_geschlossen);
    virtual_workspace::trace(&format!(
        "[WORKSPACE] migration requested={} moved={} failures={} windows={:?}",
        expected, migrated.len(), failures, migrated
    ));
    (migrated, failures)
}

#[cfg(target_os = "macos")]
pub(crate) fn virtuelle_bereitschaft(app: &tauri::AppHandle) {
    let (mut ids, failures) = virtuelle_fenster_migrieren(app);
    // After migration every Noki-owned window is registered again; anything
    // else on the virtual display is a user window that must not stay hidden.
    fremde_fenster_zurueckgeben(true);
    schreibtisch_wache_starten(app);
    vorschau_spaces_richten(app);
    for id in TASK_FENSTER.lock().map(|g| g.clone()).unwrap_or_default() {
        if !ids.contains(&id) { ids.push(id); }
    }
    let alle = ids.len();
    let ids = vorschau_faehig(ids);
    virtual_workspace::trace(&format!(
        "[WORKSPACE] startup_retry preview=true windows={:?} minimized_or_hidden={} shield=false space_trip=false",
        ids, alle - ids.len()
    ));
    let result = vorschau::setzen(app, ids.clone(), 520, 30, "virtual-display".into(), String::new());
    {
        // Apps picker: warm its cache off the startup path.
        let h = app.clone();
        thread::spawn(move || { thread::sleep(Duration::from_secs(4)); leiste_befehl(&h, "katalog", "vorwaermen"); });
    }
    // Gemessen: mit 11 Fenstern dauerte die Aufnahme laenger als die frueheren
    // 3 s - der Start endete FAILED und JEDE Eingabe wurde verworfen. Die
    // Frist waechst mit der Fensterzahl; READY erst, wenn wirklich alles laeuft.
    let frist = Duration::from_millis(3000 + 700 * ids.len() as u64);
    let deadline = std::time::Instant::now() + frist;
    let bereit = |ids: &Vec<i64>| !vorschau::retarget_aktiv() && vorschau::aufgenommene().len() == ids.len();
    while !bereit(&ids) && std::time::Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    let composition_ready = bereit(&ids) && result["ok"].as_bool().unwrap_or(false);
    let empty_valid = arbeitsplatz_fenster_liste().is_empty();
    // A window that cannot be restored is reported, but must not disable
    // the whole Workspace: FAILED dropped every input for the windows that
    // DID come back (measured after a closed dialog stayed registered).
    let _ = empty_valid;
    if failures > 0 {
        virtual_workspace::trace(&format!("[WORKSPACE] restore_failures={failures} workspace_stays_usable=true"));
    }
    let ok = composition_ready;
    virtual_workspace::finish_workspace(ok, ids.len(), failures);
}

/// Was auf Nokis Schreibtisch SICHTBAR ist - unabhaengig davon, wem es
/// gehoert.
///
/// Zwei Fragen, die lange verwechselt wurden:
///   * SICHTBARKEIT: "Was steht auf dem Schreibtisch?" Das entscheidet,
///     was die Miniatur zeigt. Sie ist ein Blick auf den echten
///     Schreibtisch - und ein Blick zeigt auch, was man nicht besitzt.
///   * BESITZ: "Was darf Noki anfassen?" Das entscheidet ueber Tippen,
///     Schliessen, Verschieben. Dafuer bleibt `ARBEITSPLATZ_FENSTER`
///     massgeblich, und daran wird nichts gelockert.
///
/// Vorher entschied der Besitz auch ueber die Anzeige. Dadurch blieb die
/// Miniatur leer, obwohl auf Schreibtisch 3 sichtbar Fenster standen - der
/// gemeldete Fehler "zeigt nur das Hintergrundbild".
///
/// Aussortiert wird nur, was auch der Nutzer dort nicht als Fenster sieht:
/// Nokis eigene Flaechen, Hilfsfenster ohne eigene Ebene und Winzlinge.
#[cfg(target_os = "macos")]
fn window_filter_spur(wid: i64, grund: &'static str, text: impl FnOnce() -> String) {
    // The inventory is sampled repeatedly. Logging the same stable rejection
    // on every pass created thousands of lines per soak and could itself put
    // pressure on stderr/terminal consumers. Keep the evidence, bounded.
    static LETZTE: Mutex<Vec<(i64, &'static str, std::time::Instant)>> = Mutex::new(Vec::new());
    let jetzt = std::time::Instant::now();
    let schreiben = LETZTE.lock().is_ok_and(|mut l| {
        l.retain(|e| jetzt.duration_since(e.2) < Duration::from_secs(60));
        if l.iter().any(|e| e.0 == wid && e.1 == grund
            && jetzt.duration_since(e.2) < Duration::from_secs(30))
        {
            false
        } else {
            l.push((wid, grund, jetzt));
            true
        }
    });
    if schreiben { eprintln!("{}", text()); }
}

#[cfg(target_os = "macos")]
pub(crate) fn sichtbare_fenster_auf_arbeitsplatz(arbeitsplatz: u64) -> Vec<i64> {
    let eigene = std::process::id() as i32;
    let ask = ask_nutzerfenster();
    let candidaten = cgs::fenster_auf_space(arbeitsplatz);
    let gefiltert: Vec<(i64, i32, String, String, i64, i64, i64, i64)> = candidaten
        .iter()
        .filter(|(wid, pid, app, titel, _x, _y, bw, bh)| {
            // Noki's own surfaces never; Ask Noki is the one user window.
            if (*pid == eigene || app.eq_ignore_ascii_case("Noki")) && Some(*wid) != ask {
                return false;
            }
            if titel.eq_ignore_ascii_case("Software Cursor") {
                window_filter_spur(*wid, "software_cursor", || format!("[WINDOW_FILTER] drop wid={wid} pid={pid} app={app:?} title={titel:?} size={bw}x{bh} reason=software_cursor"));
                return false;
            }
            let app_low = app.to_lowercase();
            if matches!(
                app_low.as_str(),
                "cua driver" | "widgetter" | "widgetwall" | "atoll" | "macscope"
                    | "window server" | "dock" | "control center" | "notification center"
                    | "mitteilungszentrale" | "systemuiserver"
            ) {
                window_filter_spur(*wid, "system_auxiliary", || format!("[WINDOW_FILTER] drop wid={wid} pid={pid} app={app:?} title={titel:?} size={bw}x{bh} reason=system_auxiliary"));
                return false;
            }
            if *bw < 140 || *bh < 120 {
                window_filter_spur(*wid, "too_small", || format!("[WINDOW_FILTER] drop wid={wid} pid={pid} app={app:?} title={titel:?} size={bw}x{bh} reason=too_small"));
                return false;
            }
            let ist_browser_oder_web = app_low.contains("safari")
                || app_low.contains("chrome")
                || app_low.contains("chromium")
                || app_low.contains("brave")
                || app_low.contains("arc")
                || app_low.contains("edge")
                || app_low.contains("firefox")
                || app_low.contains("youtube");
            if ist_browser_oder_web {
                let titel_leer = titel.trim().is_empty();
                // Context-aware search popup filter:
                // Only drop if it is a secondary, untitled, compact popup under a PID
                // that ALREADY has a larger main window with a title!
                if titel_leer && *bh < 250 {
                    let hat_hauptfenster = candidaten.iter().any(|c| {
                        c.0 != *wid && c.1 == *pid && !c.3.trim().is_empty() && c.6 >= *bw && c.7 > *bh
                    });
                    if hat_hauptfenster {
                        window_filter_spur(*wid, "search_suggestion_popup", || format!("[WINDOW_FILTER] drop wid={wid} pid={pid} app={app:?} title={titel:?} size={bw}x{bh} reason=search_suggestion_popup"));
                        return false;
                    }
                }
            }
            if !cgs::fenster_eingeordnet(*wid) {
                window_filter_spur(*wid, "ordered_out_or_minimized", || format!("[WINDOW_FILTER] drop wid={wid} pid={pid} app={app:?} title={titel:?} size={bw}x{bh} reason=ordered_out_or_minimized"));
                return false;
            }
            true
        })
        .cloned()
        .collect();

    // Canonical YouTube Identity & Bounds-Deduplizierung:
    let mut dedupliziert: Vec<(i64, i32, String, String, i64, i64, i64, i64)> = Vec::new();
    let noki_yt_wid = crate::noki_browser::youtube_fenster();

    for fenster in gefiltert {
        let (wid, pid, app, titel, x, y, bw, bh) = &fenster;
        let ist_yt = app.to_lowercase().contains("youtube")
            || titel.to_lowercase().contains("youtube")
            || noki_yt_wid == Some(*wid);

        let existiert_bereits = dedupliziert.iter_mut().find(|vorh| {
            let dx = (vorh.4 - *x).abs();
            let dy = (vorh.5 - *y).abs();
            let dw = (vorh.6 - *bw).abs();
            let dh = (vorh.7 - *bh).abs();
            // Two titled windows of different apps at the same frame are two
            // real windows (e.g. both maximized) and must both be shown.
            let schatten = vorh.1 == *pid || titel.trim().is_empty() || vorh.3.trim().is_empty();
            schatten && dx <= 12 && dy <= 12 && dw <= 12 && dh <= 12
        });

        if let Some(vorh) = existiert_bereits {
            window_filter_spur(*wid, "duplicate_bounds", || format!("[WINDOW_FILTER] duplicate bounds match wid={wid} with existing wid={} size={bw}x{bh}", vorh.0));
            if noki_yt_wid == Some(*wid) {
                *vorh = fenster;
            } else if !titel.trim().is_empty() && vorh.3.trim().is_empty() {
                *vorh = fenster;
            }
        } else {
            if ist_yt {
                let ist_yt_schatten = dedupliziert.iter().any(|v| {
                    let v_yt = v.2.to_lowercase().contains("youtube")
                        || v.3.to_lowercase().contains("youtube")
                        || noki_yt_wid == Some(v.0);
                    if !v_yt { return false; }
                    let dx = (v.4 - *x).abs();
                    let dy = (v.5 - *y).abs();
                    let dx_dy_nah = dx <= 30 && dy <= 30;
                    let titel_leer = titel.trim().is_empty();
                    let schatten = v.1 == *pid || titel_leer || v.3.trim().is_empty();
                    (dx_dy_nah && schatten) || (titel_leer && v.1 == *pid)
                });
                if ist_yt_schatten {
                    window_filter_spur(*wid, "youtube_shadow", || format!("[WINDOW_FILTER] drop duplicate YouTube shadow wid={wid} pid={pid} app={app:?} title={titel:?}"));
                    continue;
                }
            }
            dedupliziert.push(fenster);
        }
    }

    dedupliziert.into_iter().map(|(wid, ..)| wid).collect()
}

#[cfg(target_os = "macos")]
/// Idle fast path: when the first scan equals the set confirmed last time
/// for the same Desktop, it is stable by definition - no 4 more scans 60 ms
/// apart (measured: that re-confirmation every second was the largest item
/// of Noki's idle CPU). Also for a Desktop already CONFIRMED empty: the
/// answer equals the accepted state. Any difference takes the full
/// confirmation below (incl. the empty-snapshot guards).
fn stabile_sichtbare_fenster(arbeitsplatz: u64) -> Option<Vec<i64>> {
    static BESTAETIGT: std::sync::Mutex<Option<(u64, Vec<i64>)>> = std::sync::Mutex::new(None);
    let mut erst = sichtbare_fenster_auf_arbeitsplatz(arbeitsplatz);
    erst.sort_unstable();
    if BESTAETIGT.lock().ok().is_some_and(|g| g.as_ref().is_some_and(|(a, v)| *a == arbeitsplatz && *v == erst))
    {
        return Some(erst);
    }
    let r = stabile_sichtbare_fenster_voll(arbeitsplatz, erst);
    if let Ok(mut g) = BESTAETIGT.lock() {
        *g = r.as_ref().map(|v| {
            let mut v = v.clone();
            v.sort_unstable();
            (arbeitsplatz, v)
        });
    }
    r
}

fn stabile_sichtbare_fenster_voll(arbeitsplatz: u64, erst: Vec<i64>) -> Option<Vec<i64>> {
    let mut vorher = erst;
    for _ in 0..4 {
        std::thread::sleep(Duration::from_millis(60));
        let mut jetzt = sichtbare_fenster_auf_arbeitsplatz(arbeitsplatz);
        jetzt.sort_unstable();
        if jetzt == vorher {
            // Guard 1: Real space still has raw layer 0 windows, but filter dropped all -> INVALID SNAPSHOT
            let raw_candidaten = cgs::fenster_auf_space(arbeitsplatz);
            // Sticky all-Spaces overlays (Widgetter, Cua Driver) and ordered-out
            // (closed/minimized) windows are not content of this Desktop:
            // counting them made a genuinely empty Desktop "invalid" forever.
            let raw_count = raw_candidaten.iter().filter(|f| {
                f.6 >= 80 && f.7 >= 80
                    && cgs::fenster_eingeordnet(f.0)
                    && cgs::spaces_des_fensters(f.0).is_some_and(|s| s.len() == 1)
            }).count();
            if jetzt.is_empty() && raw_count > 0 {
                eprintln!("[VORSCHAU] invalid snapshot: filtered=0 but raw_windows={raw_count}; retaining last valid");
                return None;
            }
            // Guard 2: Real space is confirmed empty only after at least 2 independent checks over >=300ms
            static LEER_BESTAETIGUNG: std::sync::Mutex<Option<(u64, u8, std::time::Instant)>> =
                std::sync::Mutex::new(None);
            if jetzt.is_empty() {
                let mut bestaetigt = false;
                if let Ok(mut g) = LEER_BESTAETIGUNG.lock() {
                    match g.as_mut() {
                        Some((sid, n, seit)) if *sid == arbeitsplatz => {
                            *n = n.saturating_add(1);
                            bestaetigt = *n >= 2 && seit.elapsed() >= Duration::from_millis(300);
                        }
                        _ => *g = Some((arbeitsplatz, 1, std::time::Instant::now())),
                    }
                    if raw_count == 0 || bestaetigt {
                        *g = None;
                        bestaetigt = true;
                    }
                }
                if !bestaetigt {
                    eprintln!("[VORSCHAU] empty inventory unconfirmed (count < 2); last valid structure retained");
                    return None;
                }
            } else {
                if let Ok(mut g) = LEER_BESTAETIGUNG.lock() { *g = None; }
            }
            return Some(jetzt);
        }
        vorher = jetzt;
    }
    None
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn sichtbare_fenster_auf_arbeitsplatz(_arbeitsplatz: u64) -> Vec<i64> {
    vec![]
}

/// Gleicht den gemerkten Schreibtisch-Inhalt mit der Wirklichkeit ab.
///
/// Die Regel steht in Abschnitt 3 der Vorgabe und ist bewusst einseitig:
///   * gemerkt + existiert noch + liegt noch auf Nokis Space + gleiche
///     Herkunft (pid/app) -> bleibt,
///   * gemerkt + verschwunden -> raus,
///   * unbekanntes fremdes Fenster -> bleibt fremd.
/// Es wird NIE ein Fenster neu beansprucht, nur weil es dort liegt. Ein
/// Abgleich, der aus Anwesenheit Besitz machte, waere genau die Enteignung,
/// die Abschnitt 19 verbietet.
///
/// Rueckgabe: die Fenster, die aufgenommen werden duerfen.
#[cfg(target_os = "macos")]
fn arbeitsplatz_fenster_abgleichen(app: &tauri::AppHandle, arbeitsplatz: u64) -> Vec<i64> {
    let echte = cgs::fenster_auf_space(arbeitsplatz);
    let vorher = arbeitsplatz_fenster_liste();
    let behalten: Vec<EigenesFenster> = vorher
        .iter()
        .filter(|e| {
            echte.iter().any(|(wid, pid, owner, ..)| {
                *wid == e.fenster && *pid == e.pid && owner.as_str() == e.app
            })
        })
        .cloned()
        .collect();
    let verloren = vorher.len().saturating_sub(behalten.len());
    if verloren > 0 {
        if let Ok(mut g) = ARBEITSPLATZ_FENSTER.lock() {
            *g = behalten.clone();
        }
        arbeitsplatz_fenster_sichern(app);
        eprintln!("[ARBEITSPLATZ] {verloren} gemerkte Fenster gibt es nicht mehr");
    }
    // Aufgabenfenster, die nicht mehr existieren, sind auch keine Aufgabe mehr.
    if let Ok(mut task) = TASK_FENSTER.lock() {
        task.retain(|w| behalten.iter().any(|e| e.fenster == *w));
    }
    // Fremdes melden, nicht vereinnahmen (Abschnitt 19) - aber nur, wenn sich
    // die Lage aendert. Ein Abgleich laeuft bei jedem Zeigen der Miniatur;
    // eine Meldung je Abgleich waere Laerm, keine Auskunft.
    let fremde = echte
        .iter()
        .filter(|(wid, ..)| !behalten.iter().any(|e| e.fenster == *wid))
        .count();
    static FREMDE_ZULETZT: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(usize::MAX);
    if FREMDE_ZULETZT.swap(fremde, Ordering::Relaxed) != fremde && fremde > 0 {
        eprintln!("[ARBEITSPLATZ] {fremde} fremde Fenster auf Nokis Schreibtisch - bleiben fremd");
    }
    behalten.into_iter().map(|e| e.fenster).collect()
}

#[cfg(not(target_os = "macos"))]
fn arbeitsplatz_fenster_abgleichen(_app: &tauri::AppHandle, _arbeitsplatz: u64) -> Vec<i64> {
    vec![]
}

/// Wartet, bis das gerade gestartete Programm sein NEUES Fenster gezeigt
/// hat - und laesst den Aufrufer erst dann zum Nutzer zurueckwechseln.
///
/// Warum nicht einfach fest schlafen: eine feste Wartezeit ist entweder zu
/// kurz oder unnoetig lang. Zu kurz ist der gefaehrliche Fall - dann steht
/// Noki schon wieder auf dem Schreibtisch des Nutzers, wenn das Programm
/// endlich sein Fenster aufmacht, und macOS legt es auf den Schreibtisch,
/// der GERADE aktiv ist: den des Nutzers. Genau so entsteht das gemeldete
/// fremde Vollfenster. Gewartet wird deshalb auf das Ereignis, nicht auf
/// die Uhr.
///
/// Rueckgabe: (neue Fenster auf Nokis Schreibtisch, neue Fenster desselben
/// Programms woanders). Die zweite Liste wird NICHT beansprucht - sie ist
/// der Beweis, dass etwas danebenging, und wird gemeldet statt verschwiegen.
#[cfg(target_os = "macos")]
fn neue_fenster_abwarten(
    besitzer: &str,
    vorher: &[i64],
    arbeitsplatz: u64,
    frist: Duration,
) -> (Vec<i64>, Vec<(i64, u64)>) {
    let start = std::time::Instant::now();
    let mut hier: Vec<i64> = vec![];
    let mut anderswo: Vec<(i64, u64)> = vec![];
    let mut erster_treffer: Option<std::time::Instant> = None;
    while start.elapsed() < frist {
        hier.clear();
        anderswo.clear();
        for (wid, space) in cgs::fenster_von(besitzer) {
            if vorher.contains(&wid) {
                continue;
            }
            if space == arbeitsplatz {
                hier.push(wid);
            } else {
                anderswo.push((wid, space));
            }
        }
        if !hier.is_empty() {
            // Ein Programm oeffnet gern mehrere Fenster kurz hintereinander.
            // Nach dem ersten noch kurz nachfassen, dann ist Schluss.
            match erster_treffer {
                None => erster_treffer = Some(std::time::Instant::now()),
                Some(t) if t.elapsed() >= Duration::from_millis(600) => break,
                Some(_) => {}
            }
        }
        thread::sleep(Duration::from_millis(120));
    }
    (hier, anderswo)
}

/// Bringt den Nutzer auf den Schreibtisch zurueck, von dem er kam.
///
/// Geprueft wird die WIRKLICHKEIT, nicht die Absicht: steht er schon dort,
/// geschieht nichts; steht er woanders, wird zurueckgefuehrt. Damit bleibt
/// eine Arbeitsplatz-Vorbereitung auch dann begrenzt, wenn der Hinweg sich
/// selbst falsch eingeschaetzt hat.
// ---------------------------------------------------------------------
//  BLENDE — der interne Schreibtischwechsel wird NICHT SICHTBAR.
//
//  Manche Programme geben ihr erstes echtes Fenster nur dort, wo der
//  Bildschirm gerade WIRKLICH steht. Gemessen (macOS 26.6): mit blosser
//  CGS-Buchfuehrung entstanden Fenster mit `onscreen=false` auf JEDEM
//  Schreibtisch - Gespenster. Der echte Wechsel ist also noetig.
//
//  Noetig ist aber nur der WECHSEL, nicht sein ANBLICK. Die Blende haelt
//  ein Standbild des Ausgangsschreibtischs vor den Bildschirm, solange die
//  Vorbereitung laeuft: kein Wischen, kein Aufblitzen von Nokis
//  Schreibtisch, kein Zwischenschreibtisch, kein schwarzes Bild.
//
//  Ebene 2 - bewusst UNTER Nokis Fenster (Floating, 3) und unter der
//  Miniatur (4). Noki bleibt also lebendig sichtbar, die Sprachtafel
//  bleibt lesbar und die Miniatur zeigt weiter, was auf seinem
//  Schreibtisch geschieht. Nur die gewoehnlichen Fenster darunter frieren
//  zum Standbild ein. Ueber der Blende liegt weiterhin die Menueleiste
//  (24) - also auch keine stehengebliebene Uhr.
//
//  Gelingt der Schnappschuss nicht, wird NICHTS vorgetaeuscht: die Blende
//  bleibt aus, der Wechsel geschieht wie bisher sichtbar, und das steht im
//  Protokoll. Ein schwarzes Rechteck waere schlimmer als ein Wischer.
#[cfg(target_os = "macos")]
mod blende {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
    use std::sync::Mutex;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct NsPunkt { x: f64, y: f64 }
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct NsGroesse { w: f64, h: f64 }
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct NsRahmen { o: NsPunkt, g: NsGroesse }

    #[link(name = "objc")]
    extern "C" {
        fn sel_registerName(n: *const std::os::raw::c_char) -> *const c_void;
        fn objc_getClass(n: *const std::os::raw::c_char) -> *mut c_void;
        fn objc_msgSend();
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGWindowListCreateImage(
            rect: NsRahmen, option: u32, relative: u32, imageoption: u32,
        ) -> *mut c_void;
        fn CGImageRelease(img: *const c_void);
        fn CGWindowLevelForKey(key: i32) -> i32;
    }

    /// Das offene Blendenfenster. Nur vom Hauptthread beruehrt.
    static FENSTER: Mutex<usize> = Mutex::new(0);
    /// Wer VOR der Vorbereitung vorn war. Ein Programm, das sein erstes
    /// Fenster erzeugt, holt sich dabei den Vordergrund - das ist der
    /// "Fokussprung", den der Nutzer sonst behaelt, obwohl er seinen
    /// Schreibtisch nie verlassen hat. 0 = niemanden zurueckholen.
    static VORHER_VORN: AtomicI32 = AtomicI32::new(0);
    /// Zaehlt jedes Auf/Zu. Die Notbremse schliesst nur IHRE eigene Blende.
    static LAUF: AtomicU64 = AtomicU64::new(0);

    unsafe fn sel(n: &[u8]) -> *const c_void { sel_registerName(n.as_ptr() as *const _) }
    unsafe fn klasse(n: &[u8]) -> *mut c_void { objc_getClass(n.as_ptr() as *const _) }
    unsafe fn ruf0(o: *mut c_void, s: *const c_void) -> *mut c_void {
        let f: unsafe extern "C" fn(*mut c_void, *const c_void) -> *mut c_void =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(o, s)
    }
    unsafe fn ruf_bool(o: *mut c_void, s: *const c_void, w: bool) {
        let f: unsafe extern "C" fn(*mut c_void, *const c_void, bool) =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(o, s, w)
    }
    unsafe fn ruf_zahl(o: *mut c_void, s: *const c_void, w: i64) {
        let f: unsafe extern "C" fn(*mut c_void, *const c_void, i64) =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(o, s, w)
    }
    unsafe fn ruf_ptr(o: *mut c_void, s: *const c_void, w: *mut c_void) {
        let f: unsafe extern "C" fn(*mut c_void, *const c_void, *mut c_void) =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(o, s, w)
    }
    unsafe fn rahmen_von(o: *mut c_void, s: *const c_void) -> NsRahmen {
        let f: unsafe extern "C" fn(*mut c_void, *const c_void) -> NsRahmen =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(o, s)
    }

    /// Hoehe der Menueleiste des Hauptbildschirms.
    ///
    /// `visibleFrame` endet genau unter ihr; die Differenz zur Oberkante von
    /// `frame` IST ihre Hoehe. Kein geratener Zahlenwert - der wandert mit
    /// jeder macOS-Fassung.
    unsafe fn menueleiste() -> f64 {
        let haupt = ruf0(klasse(b"NSScreen\0"), sel(b"mainScreen\0"));
        if haupt.is_null() { return 0.0; }
        let ganz = rahmen_von(haupt, sel(b"frame\0"));
        let sichtbar = rahmen_von(haupt, sel(b"visibleFrame\0"));
        let h = (ganz.o.y + ganz.g.h) - (sichtbar.o.y + sichtbar.g.h);
        if h.is_finite() && h > 0.0 { h } else { 0.0 }
    }

    /// Die Vereinigung aller Bildschirme - bei einem Bildschirm genau er.
    unsafe fn gesamtflaeche() -> NsRahmen {
        let screens = ruf0(klasse(b"NSScreen\0"), sel(b"screens\0"));
        let anzahl: i64 = {
            let f: unsafe extern "C" fn(*mut c_void, *const c_void) -> i64 =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            f(screens, sel(b"count\0"))
        };
        let mut l = f64::MAX; let mut u = f64::MAX;
        let mut r = f64::MIN; let mut o = f64::MIN;
        for i in 0..anzahl {
            let s: *mut c_void = {
                let f: unsafe extern "C" fn(*mut c_void, *const c_void, i64) -> *mut c_void =
                    std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                f(screens, sel(b"objectAtIndex:\0"), i)
            };
            let fr = rahmen_von(s, sel(b"frame\0"));
            l = l.min(fr.o.x); u = u.min(fr.o.y);
            r = r.max(fr.o.x + fr.g.w); o = o.max(fr.o.y + fr.g.h);
        }
        if anzahl == 0 { return NsRahmen::default(); }
        NsRahmen { o: NsPunkt { x: l, y: u }, g: NsGroesse { w: r - l, h: o - u } }
    }

    /// Standbild des JETZIGEN Bildschirms vor die Szene haengen.
    ///
    /// Muss auf dem Hauptthread laufen (AppKit). Gibt `true` zurueck, wenn
    /// die Blende wirklich steht - nur dann darf sich der Aufrufer darauf
    /// verlassen, dass der Wechsel unsichtbar bleibt.
    unsafe fn auf_im_hauptthread() -> bool {
        {
            let g = FENSTER.lock();
            if g.map(|v| *v != 0).unwrap_or(true) { return false; } // steht schon
        }
        // kCGWindowListOptionOnScreenOnly (1) | kCGWindowListExcludeDesktopElements(16)?
        // NEIN: der Schreibtischhintergrund gehoert mit aufs Standbild, sonst
        // fehlte er und es entstuende genau das "erst Hintergrund"-Bild, das
        // an der Miniatur schon einmal falsch war.
        let flaeche = gesamtflaeche();
        if flaeche.g.w < 1.0 || flaeche.g.h < 1.0 {
            return false;
        }
        // AUSDRUECKLICH die Bildschirmflaeche, nicht "unendlich".
        //
        // Gemessen und mit zwei Bildschirmfotos belegt: mit einem
        // Unendlich-Rechteck lieferte `CGWindowListCreateImage` kein Vollbild,
        // sondern einen schmalen, gestauchten Streifen am oberen Rand - genau
        // das schwarze Bild, das die Blende verhindern soll. Mit der
        // ausdruecklichen Flaeche stimmt der Ausschnitt.
        //
        // Bei EINEM Bildschirm sind Fensterkoordinaten (unten links) und
        // Anzeigekoordinaten (oben links) deckungsgleich - beide (0,0,B,H).
        // Bei mehreren Bildschirmen waere hier eine Umrechnung noetig; dann
        // deckt die Blende den Hauptbildschirm und meldet das im Protokoll.
        //
        // A bridge must preserve the entire origin frame.  Leaving the menu
        // bar live exposed the target application's menu for one or two
        // frames even when the desktop below was covered; that is still a
        // visible Space leak.  Capture and cover the complete display.
        let leiste = menueleiste();
        let hoehe = flaeche.g.h.max(1.0);
        let ausschnitt = flaeche;
        let bild = CGWindowListCreateImage(ausschnitt, 1, 0, 0);
        if bild.is_null() {
            eprintln!("[BLENDE] Kein Schnappschuss (Bildschirmaufnahme nicht freigegeben?). \
                       Der Wechsel bleibt sichtbar - es wird nichts vorgetaeuscht.");
            return false;
        }

        // DIREKT AUF DIE EBENE, nicht ueber NSImageView.
        //
        // Gemessen und mit einem Bildschirmfoto belegt: ueber `NSImage`
        // (initWithCGImage:size: + setImageScaling:) landete der
        // Schnappschuss als schmaler, gestauchter Streifen am oberen Rand -
        // darunter Schwarz. Der Grund ist der Massstab: der Schnappschuss
        // kommt in BILDPUNKTEN (2940x1912), die Ansicht rechnet in PUNKTEN
        // (1470x956), und `NSImage` traegt seine Groesse selbst mit. Eine
        // CALayer nimmt das CGImage ohne diese Zwischenrechnung und
        // `resize` dehnt es exakt auf die Flaeche - unabhaengig vom
        // Massstab des Bildschirms.
        let sicht = {
            let a = ruf0(klasse(b"NSView\0"), sel(b"alloc\0"));
            let f: unsafe extern "C" fn(*mut c_void, *const c_void, NsRahmen) -> *mut c_void =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            f(a, sel(b"initWithFrame:\0"),
              NsRahmen { o: NsPunkt { x: 0.0, y: 0.0 },
                         g: NsGroesse { w: flaeche.g.w, h: hoehe } })
        };
        ruf_bool(sicht, sel(b"setWantsLayer:\0"), true);
        let ebene = ruf0(sicht, sel(b"layer\0"));
        if ebene.is_null() { CGImageRelease(bild as *const c_void); return false; }
        ruf_ptr(ebene, sel(b"setContents:\0"), bild);
        let resize = {
            let f: unsafe extern "C" fn(
                *mut c_void, *const c_void, *const std::os::raw::c_char,
            ) -> *mut c_void = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            f(klasse(b"NSString\0"), sel(b"stringWithUTF8String:\0"),
              b"resize\0".as_ptr() as *const _)
        };
        ruf_ptr(ebene, sel(b"setContentsGravity:\0"), resize);
        CGImageRelease(bild as *const c_void);

        let fenster = {
            let a = ruf0(klasse(b"NSWindow\0"), sel(b"alloc\0"));
            let f: unsafe extern "C" fn(
                *mut c_void, *const c_void, NsRahmen, u64, u64, bool,
            ) -> *mut c_void = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            // NSWindowStyleMaskBorderless = 0, NSBackingStoreBuffered = 2
            let r = NsRahmen {
                o: NsPunkt { x: flaeche.o.x, y: flaeche.o.y },
                g: NsGroesse { w: flaeche.g.w, h: hoehe },
            };
            f(a, sel(b"initWithContentRect:styleMask:backing:defer:\0"), r, 0, 2, false)
        };
        if fenster.is_null() { return false; }
        ruf_ptr(fenster, sel(b"setContentView:\0"), sicht);
        // `setContentView:` behaelt die Ansicht selbst - unser eigener
        // Anspruch aus `alloc` ist damit erledigt.
        ruf0(sicht, sel(b"release\0"));
        ruf_bool(fenster, sel(b"setOpaque:\0"), true);
        ruf_bool(fenster, sel(b"setHasShadow:\0"), false);
        // This window is also the input firewall while the live preview
        // panel is absent from the origin Space.  Ignoring mouse events let
        // the remainder of the initiating click (and trackpad momentum)
        // land on the user's current application.  The shield therefore
        // owns and consumes physical input for its short lifetime.
        ruf_bool(fenster, sel(b"setIgnoresMouseEvents:\0"), false);
        // Gemessen: mit `false` blieb nach jeder Aktion ein Fenster
        // 1470x923 als (unsichtbares) WindowServer-Fenster stehen - samt
        // seines Bildspeichers. Ein Begleiter, der bei jeder Aufgabe einen
        // Vollbild-Puffer liegen laesst, ist ein Leck. `close` gibt es jetzt
        // wirklich frei; danach wird der Zeiger nicht mehr angefasst.
        ruf_bool(fenster, sel(b"setReleasedWhenClosed:\0"), true);
        // Use WindowServer's maximum window level, not a guessed screen-
        // saver value.  Space-transition surfaces can be above level 1000;
        // that allowed complete target frames to leak through the shield.
        let maximal = CGWindowLevelForKey(14); // kCGMaximumWindowLevelKey
        ruf_zahl(fenster, sel(b"setLevel:\0"), maximal as i64);
        // CanJoinAllSpaces(1<<0) | Stationary(1<<4) | IgnoresCycle(1<<6)
        // | FullScreenAuxiliary(1<<8): sie MUSS ueber den Wechsel hinweg
        // stehenbleiben, sonst waere sie genau dann weg, wenn sie gebraucht
        // wird. `Stationary` verhindert zusaetzlich, dass Mission Control sie
        // mitwischt.
        ruf_zahl(fenster, sel(b"setCollectionBehavior:\0"),
                 (1 << 0) | (1 << 4) | (1 << 6) | (1 << 8));
        ruf0(fenster, sel(b"orderFrontRegardless\0"));
        ruf0(fenster, sel(b"displayIfNeeded\0"));

        if let Ok(mut g) = FENSTER.lock() { *g = fenster as usize; }
        // Wer jetzt vorn ist, ist nachher wieder vorn.
        VORHER_VORN.store(vordergrund_pid(), Ordering::SeqCst);
        eprintln!("[BLENDE] steht: {:.0}x{:.0} bei {:.0}/{:.0} level={} (Menueleiste {:.0} bedeckt)",
                  flaeche.g.w, hoehe, flaeche.o.x, flaeche.o.y, maximal, leiste);
        true
    }

    /// Prozessnummer des Programms, das gerade vorn ist (0 = unbekannt).
    unsafe fn vordergrund_pid() -> i32 {
        let ws = ruf0(klasse(b"NSWorkspace\0"), sel(b"sharedWorkspace\0"));
        if ws.is_null() { return 0; }
        let app = ruf0(ws, sel(b"frontmostApplication\0"));
        if app.is_null() { return 0; }
        let f: unsafe extern "C" fn(*mut c_void, *const c_void) -> i32 =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(app, sel(b"processIdentifier\0"))
    }

    pub(crate) fn vorn_pid() -> i32 { unsafe { vordergrund_pid() } }

    /// Den Vordergrund an ein Programm zurueckgeben (dessen Fenster auf dem
    /// aktuellen Schreibtisch des Nutzers liegt - kein Schreibtischwechsel).
    /// Gemessen (macOS 26): `activateWithOptions:` aus einem Hintergrund-
    /// programm wird ignoriert (kooperative Aktivierung) - Chrome blieb 15 s
    /// vorn. Dann aktiviert LaunchServices das Programm des Nutzers (`open -b`,
    /// keine Zusatzrechte; sein Fenster liegt auf dem aktuellen Schreibtisch).
    pub(crate) fn vorn_zurueck(pid: i32) -> bool {
        if pid <= 0 { return false; }
        if vorn_zurueck_direkt(pid) {
            std::thread::sleep(std::time::Duration::from_millis(60));
            if vorn_pid() == pid { return true; }
        }
        let Some((_, bundle, _)) = super::lesezeichen::app_fuer_pid(pid) else { return false };
        if bundle.is_empty() { return false; }
        let ok = std::process::Command::new("/usr/bin/open").args(["-b", &bundle])
            .status().map(|s| s.success()).unwrap_or(false);
        for _ in 0..20 {
            if vorn_pid() == pid { return true; }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        ok && vorn_pid() == pid
    }

    /// Direct NSRunningApplication activation only - never `open -b`
    /// (that sends a reopen event which can create/show windows).
    pub(crate) fn aktivieren_direkt(pid: i32) -> bool { pid > 0 && vorn_zurueck_direkt(pid) }

    fn vorn_zurueck_direkt(pid: i32) -> bool {
        unsafe {
            let app: *mut c_void = {
                let f: unsafe extern "C" fn(*mut c_void, *const c_void, i32) -> *mut c_void =
                    std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                f(klasse(b"NSRunningApplication\0"),
                  sel(b"runningApplicationWithProcessIdentifier:\0"), pid)
            };
            if app.is_null() { return false; }
            // NSApplicationActivateIgnoringOtherApps = 1 << 1
            let f: unsafe extern "C" fn(*mut c_void, *const c_void, u64) -> bool =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            f(app, sel(b"activateWithOptions:\0"), 1 << 1)
        }
    }

    unsafe fn zu_im_hauptthread() {
        let alt = FENSTER.lock().map(|mut g| std::mem::replace(&mut *g, 0)).unwrap_or(0);
        if alt == 0 { return; }
        let w = alt as *mut c_void;
        ruf_ptr(w, sel(b"orderOut:\0"), std::ptr::null_mut());
        ruf0(w, sel(b"close\0"));
        // Den Vordergrund zurueckgeben. Der Nutzer hat seinen Schreibtisch
        // nie verlassen - dann soll auch sein Programm wieder vorn sein.
        // Nur, wenn sich wirklich etwas verschoben hat.
        let vorher = VORHER_VORN.swap(0, Ordering::SeqCst);
        if vorher > 0 && vorher != vordergrund_pid() {
            let app: *mut c_void = {
                let f: unsafe extern "C" fn(*mut c_void, *const c_void, i32) -> *mut c_void =
                    std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                f(klasse(b"NSRunningApplication\0"),
                  sel(b"runningApplicationWithProcessIdentifier:\0"), vorher)
            };
            if !app.is_null() {
                eprintln!("[BLENDE] Vordergrund zurueck an pid {vorher}");
                // NSApplicationActivateIgnoringOtherApps = 1 << 1
                let f: unsafe extern "C" fn(*mut c_void, *const c_void, u64) -> bool =
                    std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                f(app, sel(b"activateWithOptions:\0"), 1 << 1);
            }
        }
    }

    /// Blende hoch. Blockiert den Aufrufer, bis sie WIRKLICH steht - sonst
    /// begaenne der Wechsel vor dem Standbild und der Nutzer saehe ihn doch.
    pub fn auf(app: &tauri::AppHandle) -> bool {
        let (tx, rx) = std::sync::mpsc::channel::<bool>();
        if app.run_on_main_thread(move || {
            let ok = unsafe { auf_im_hauptthread() };
            let _ = tx.send(ok);
        }).is_err() { return false; }
        let ok = match rx.recv_timeout(std::time::Duration::from_millis(2500)) {
            Ok(v) => v,
            Err(e) => { eprintln!("[BLENDE] Hauptthread antwortet nicht: {e}"); false }
        };
        if !ok {
            eprintln!("[BLENDE] keine Blende - der Wechsel bleibt sichtbar.");
            return false;
        }
        let lauf = LAUF.fetch_add(1, Ordering::SeqCst) + 1;
        // NOTBREMSE. Eine haengende Blende waere ein eingefrorener
        // Bildschirm - schlimmer als jeder sichtbare Wischer. Sie faellt
        // deshalb in JEDEM Fall von selbst.
        let a = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(20));
            if LAUF.load(Ordering::SeqCst) == lauf {
                eprintln!("[BLENDE] Notbremse: die Blende stand zu lange und wird geloest.");
                zu(&a);
            }
        });
        true
    }

    pub fn zu(app: &tauri::AppHandle) {
        LAUF.fetch_add(1, Ordering::SeqCst);
        let _ = app.run_on_main_thread(|| unsafe { zu_im_hauptthread() });
    }

    /// Holt ausschliesslich die Ziel-App nach vorn, waehrend die Blende
    /// bereits steht. Das ist fuer Chromes Web-Oberflaeche noetig: DOM und
    /// Titel reagieren auch im Hintergrund, der sichtbare Surface-Puffer
    /// wird dort aber nicht zuverlaessig neu veroeffentlicht. Beim Fallen
    /// der Blende gibt `zu_im_hauptthread` den Vordergrund an VORHER_VORN
    /// zurueck; fuer den Nutzer bleibt die Reise damit unsichtbar.
    pub(crate) fn ziel_vorn(app: &tauri::AppHandle, pid: i32) -> bool {
        if pid <= 0 { return false; }
        let (tx, rx) = std::sync::mpsc::channel::<bool>();
        if app.run_on_main_thread(move || {
            let ok = unsafe {
                let ziel: *mut c_void = {
                    let f: unsafe extern "C" fn(*mut c_void, *const c_void, i32) -> *mut c_void =
                        std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                    f(klasse(b"NSRunningApplication\0"),
                      sel(b"runningApplicationWithProcessIdentifier:\0"), pid)
                };
                if ziel.is_null() { false } else {
                    // NSApplicationActivateIgnoringOtherApps = 1 << 1
                    let f: unsafe extern "C" fn(*mut c_void, *const c_void, u64) -> bool =
                        std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                    let aktiviert = f(ziel, sel(b"activateWithOptions:\0"), 1 << 1);
                    // Activation may reorder windows.  Reassert the input
                    // firewall before any generated remote event is sent.
                    if let Ok(g) = FENSTER.lock() {
                        if *g != 0 {
                            ruf0(*g as *mut c_void, sel(b"orderFrontRegardless\0"));
                            ruf0(*g as *mut c_void, sel(b"displayIfNeeded\0"));
                        }
                    }
                    aktiviert
                }
            };
            let _ = tx.send(ok);
        }).is_err() { return false; }
        rx.recv_timeout(std::time::Duration::from_millis(1000)).unwrap_or(false)
    }

    /// RAII: die Blende geht auf JEDEM Rueckweg dieser Funktion wieder weg -
    /// auch bei einem fruehen `return` oder einem Fehler.
    pub struct Halten(pub tauri::AppHandle, pub bool);
    impl Drop for Halten {
        fn drop(&mut self) {
            if self.1 { zu(&self.0); }
        }
    }
}

// ---------------------------------------------------------------------
//  STOCKUNGS-WACHE (nur Debug, NOKI_STOCKUNG=1)
//
//  Ein "es ruckelt" laesst sich nicht aus dem Quelltext lesen. Diese Wache
//  misst, was der Nutzer spuert: wie lange der HAUPTTHREAD braucht, um eine
//  leere Aufgabe anzunehmen. Genau dort haengen Nokis Zeichenschritt, die
//  Maus und jedes synchrone Tauri-Kommando.
#[cfg(debug_assertions)]
/// Debug/acceptance only: return the test Mac to an exact Space after a
/// test trip. Production navigation stays footer-only (guard test).
#[cfg(all(debug_assertions, target_os = "macos"))]
fn test_exakter_space(id: u64) {
    let vorher = cgs::aktiver_space().map(|s| s.0).unwrap_or(0);
    let ok = cgs::direkt_zum_space(id);
    let nachher = cgs::aktiver_space().map(|s| s.0).unwrap_or(0);
    virtual_workspace::trace(&format!(
        "[TEST] exact_space target={id} before={vorher} after={nachher} result={ok:?}"
    ));
}

/// Real liveness of the two event loops the shortcuts and the Miniatur UI
/// depend on (the helper's own pulse is tracked separately).
pub(crate) static MAIN_PULS_MS: AtomicU64 = AtomicU64::new(0);
pub(crate) static MAIN_LAG_MAX_MS: AtomicU64 = AtomicU64::new(0);
pub(crate) static UI_PULS_MS: AtomicU64 = AtomicU64::new(0);

#[tauri::command]
fn noki_ui_puls() {
    UI_PULS_MS.store(jetzt_epoch_ms(), Ordering::Relaxed);
}

/// Name of the last synchronous (main-thread) operation that ran long.
static UI_BUSY_LETZT: Mutex<&'static str> = Mutex::new("-");
fn ui_busy_letzt() -> &'static str { UI_BUSY_LETZT.lock().map(|g| *g).unwrap_or("-") }
/// Guard for work that must stay on the main thread: logs UI_BUSY_BEGIN/END
/// only when it really took >= 250 ms (no spam).
pub(crate) struct UiBusy(&'static str, std::time::Instant);
impl UiBusy {
    pub(crate) fn neu(grund: &'static str) -> Self { UiBusy(grund, std::time::Instant::now()) }
}
impl Drop for UiBusy {
    fn drop(&mut self) {
        let ms = self.1.elapsed().as_millis();
        if ms >= 250 {
            if let Ok(mut g) = UI_BUSY_LETZT.lock() { *g = self.0; }
            virtual_workspace::trace(&format!("UI_BUSY_BEGIN reason={} timestamp={}", self.0, jetzt_epoch_ms() as u128 - ms));
            virtual_workspace::trace(&format!("UI_BUSY_END reason={} duration_ms={ms}", self.0));
        }
    }
}
fn jetzt_epoch_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn hauptfaden_puls(app: &tauri::AppHandle) {
    let h = app.clone();
    let _ = std::thread::Builder::new().name("noki-main-pulse".into()).spawn(move || loop {
        let t0 = std::time::Instant::now();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        if h.run_on_main_thread(move || { let _ = tx.send(()); }).is_err() { return; }
        if rx.recv_timeout(std::time::Duration::from_secs(60)).is_ok() {
            let lag = t0.elapsed().as_millis() as u64;
            MAIN_PULS_MS.store(jetzt_epoch_ms(), Ordering::Relaxed);
            MAIN_LAG_MAX_MS.fetch_max(lag, Ordering::Relaxed);
            if lag >= 1000 { eprintln!("[HEALTH] main_thread_lag_ms={lag}"); }
            // Every window's input (Settings, panels, Ask) waits on this
            // thread: make >=300 ms visible with the operation that held it.
            if lag >= 300 {
                virtual_workspace::trace(&format!("UI_BUSY_END reason=main_thread_stall last_sync_op={} duration_ms={lag}", ui_busy_letzt()));
            }
        } else {
            eprintln!("[HEALTH] main_thread_blocked_s=60");
        }
        std::thread::sleep(std::time::Duration::from_millis(1000));
    });
}

#[cfg(debug_assertions)]
fn stockungs_wache(app: &tauri::AppHandle) {
    if std::env::var("NOKI_STOCKUNG").ok().as_deref() != Some("1") {
        return;
    }
    let h = app.clone();
    std::thread::spawn(move || loop {
        let t0 = std::time::Instant::now();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        if h.run_on_main_thread(move || { let _ = tx.send(()); }).is_err() {
            return;
        }
        let _ = rx.recv_timeout(std::time::Duration::from_secs(30));
        let ms = t0.elapsed().as_millis();
        if ms >= 120 {
            eprintln!("[STOCKUNG] Hauptthread {ms} ms blockiert");
        }
        std::thread::sleep(std::time::Duration::from_millis(60));
    });
}
#[cfg(not(debug_assertions))]
fn stockungs_wache(_: &tauri::AppHandle) {}

/// Misst einen Abschnitt und meldet ihn, wenn er die Oberflaeche aufhaelt.
/// Bewusst ohne Makro-Zauber: der Name steht im Aufruf, damit im Protokoll
/// die STELLE steht und nicht nur eine Zeilennummer.
fn takt<T>(name: &str, f: impl FnOnce() -> T) -> T {
    let t0 = std::time::Instant::now();
    let r = f();
    let ms = t0.elapsed().as_millis();
    if ms >= 40 {
        eprintln!("[TAKT] {name} {ms} ms");
    }
    r
}

/// EINE HANDLUNG AUF NOKIS SCHREIBTISCH - UNSICHTBAR FUER DEN NUTZER.
///
/// Gemessen: ein Zeigerereignis, das an einen Prozess zugestellt wird,
/// dessen Fenster auf einem UNSICHTBAREN Schreibtisch liegt, wird von
/// Chrome verworfen. Es kommt an, es wirkt nur nicht. Semantisch ueber die
/// Bedienungshilfen zu druecken ist der bessere Weg - aber Chrome legt in
/// seinem Webinhalt kein betaetigbares Element offen (`ax=false`, an vier
/// Stellen geprueft).
///
/// Bleibt der Weg, den der Nutzer ohnehin schon kennt: derselbe verdeckte
/// Vorgang wie beim Oeffnen. Standbild vor den Bildschirm, kurz wirklich
/// hinueber, die Handlung ausfuehren, zurueck, Standbild weg. Der Nutzer
/// sieht dabei durchgehend seinen eigenen Schreibtisch.
///
/// Der Zeiger wird NICHT bewegt: zugestellt wird weiter an den Prozess.
/// Der Unterschied ist allein, dass sein Fenster in diesem Moment wirklich
/// sichtbar ist - und genau daran hing es.
///
/// Laeuft auf dem AUFRUFENDEN Faden (dem Lesefaden des Helfers), nie auf
/// dem Hauptfaden. Er ist eng begrenzt: kein Warten auf Fenster, kein
/// Wiederholen - hin, tun, zurueck.
/// IMMER NUR EINER. Zwei verdeckte Vorgaenge gleichzeitig schalten den
/// Bildschirm gegeneinander: gemessen landete der Nutzer dabei auf einem
/// VOLLBILD-Space und blieb dort stehen. Der Weg hin und zurueck gehoert
/// deshalb genau einem Aufrufer, bis er fertig ist.
static FERN_SPERRE: Mutex<()> = Mutex::new(());

#[cfg(target_os = "macos")]
pub(crate) fn auf_arbeitsplatz_handeln<T>(
    app: &tauri::AppHandle,
    tun: impl FnOnce() -> T,
) -> Option<T> {
    if virtual_workspace::backend() == virtual_workspace::Backend::LegacyVirtualDisplay {
        virtual_workspace::trace(&format!(
            "[SAFETY] legacy_bridge=blocked backend=virtual state={:?} shield=false space_trip=false",
            virtual_workspace::readiness()
        ));
        return None;
    }
    // Der verdeckte Vorgang war eine Zeitlang abgeschaltet, weil er den
    // Nutzer stehenlassen konnte: `sichtbar_zum_space` rechnete die
    // Entfernung EINMAL aus und wischte dann blind so oft - waehrend
    // `mru-spaces` die Reihenfolge unter ihm umsortierte. Zweimal gemessen
    // landete der Nutzer dabei auf einem fremden Schreibtisch.
    //
    // Der Wechsel geht jetzt Schritt fuer Schritt und prueft nach jedem
    // Schritt neu, wo er steht. Danach gemessen: ZEHN Vorgaenge
    // hintereinander, zehnmal wieder genau am Ausgangsschreibtisch
    // (`[FERNTEST] 1..10/10 ... OK`, null Abweichungen). Deshalb ist er
    // wieder der Normalfall. NOKI_FERN_BRUECKE=0 schaltet ihn ab.
    if std::env::var("NOKI_FERN_BRUECKE").ok().as_deref() == Some("0") {
        return None;
    }
    // Eine vergiftete Sperre (Panik in einem frueheren Vorgang) darf die
    // Fernbedienung nicht bis zum Neustart lahmlegen: sie wird uebernommen.
    let _einer = FERN_SPERRE.lock().unwrap_or_else(|e| e.into_inner());
    let r = arbeitsplatz_sichern(app).ok()?;
    let herkunft = cgs::aktiver_space().map(|(a, _)| a).unwrap_or(0);
    if cfg!(debug_assertions)
        && std::env::var("NOKI_ACCEPTANCE").ok().as_deref() != Some("0") {
        eprintln!(
            "[ACCEPT] SPACE origin={} target={} targetUUID={} pid={}",
            herkunft, r.id, r.uuid, std::process::id()
        );
    }
    if herkunft == r.id {
        return Some(tun()); // schon dort: nichts zu verdecken
    }
    let _eigener = EigenerWechsel::neu();
    let blende_an = blende::auf(app);
    // Fail closed: without a fully presented, input-owning shield there is
    // no safe hidden bridge.  Never expose the target Space merely to make
    // a remote action appear successful.
    if !blende_an {
        eprintln!("[FERN] keine sichere Blende - Handlung verworfen");
        return None;
    }
    let _blende = blende::Halten(app.clone(), blende_an);
    if let Err(e) = cgs::sichtbar_zum_space(r.id) {
        eprintln!("[FERN] Hinweg misslang: {e} - zurueck zur Herkunft");
        // Auch ein halber Hinweg endet dort, wo der Nutzer stand.
        zurueck_zur_herkunft(herkunft);
        return None;
    }
    // `sichtbar_zum_space` returns only after `sichtbar_bestaetigen` has
    // observed a complete target frame and already allowed one render frame.
    // The former additional 120 ms sleep was therefore a fixed duplicate.
    // Was auch immer die Handlung tut - der Rueckweg findet statt.
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(tun));
    // Dem Programm einen Augenblick geben, die Handlung auszufuehren,
    // solange sein Fenster noch sichtbar ist.
    // Give the target event loop several frames to consume the PID event;
    // the previous unconditional 300 ms pause dominated click latency.
    thread::sleep(Duration::from_millis(120));
    vorschau::phase(vorschau::FernPhase::Rueckweg);
    zurueck_zur_herkunft(herkunft);
    // The target Space notification can arrive after the physical return
    // and otherwise leave the helper believing it is still on Noki's
    // Desktop (hidden, with stale hover state).  Reconcile from the verified
    // current Space while the shield is still up.
    vorschau_spaces_richten(app);
    let ordnung = cgs::space_reihenfolge().unwrap_or_default();
    let i = ordnung.iter().position(|x| *x == herkunft);
    let links = i.and_then(|i| i.checked_sub(1)).and_then(|j| ordnung.get(j)) == Some(&r.id);
    let rechts = i.and_then(|i| ordnung.get(i + 1)) == Some(&r.id);
    vorschau::ort_erzwingen("nutzer", links, rechts);
    if cfg!(debug_assertions)
        && std::env::var("NOKI_ACCEPTANCE").ok().as_deref() != Some("0") {
        let zurueck = cgs::aktiver_space().map(|(a, _)| a).unwrap_or(0);
        eprintln!("[ACCEPT] SPACE return={} expectedOrigin={}", zurueck, herkunft);
    }
    match out {
        Ok(v) => Some(v),
        Err(_) => {
            eprintln!("[FERN] Handlung abgebrochen (Fehler abgefangen) - Nutzer ist zurueck");
            None
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn auf_arbeitsplatz_handeln<T>(
    _app: &tauri::AppHandle,
    tun: impl FnOnce() -> T,
) -> Option<T> {
    Some(tun())
}

/// DEN SCHREIBTISCH WAEHLEN, DEN NOKI BENUTZT.
///
/// `LINKS-VON-1 + Pfeil`. Das ist eine ROLLENAENDERUNG, keine Reise:
///   * der Nutzer bleibt, wo er steht - kein sichtbarer Wechsel;
///   * kein Fenster wird verschoben, geschlossen oder beansprucht; was auf
///     dem neuen Schreibtisch steht, gehoert weiter dem, dem es gehoert;
///   * die Figur fliegt nicht mit - sie hat mit der Wahl nichts zu tun;
///   * ab sofort zeigt die Miniatur diesen Schreibtisch, der Fussknopf nennt
///     ihn, und kuenftige Aktionen landen dort.
///
/// Identitaet ist die uuid. Die Nummer wird jedes Mal frisch aus der
/// lebenden Mission-Control-Ordnung berechnet - sie ist Darstellung.
#[cfg(target_os = "macos")]
fn arbeitsplatz_waehlen(app: &tauri::AppHandle, vor: bool) {
    if virtual_workspace::backend() == virtual_workspace::Backend::LegacyVirtualDisplay {
        virtuelles_fenster_waehlen(app, vor);
        return;
    }
    let Some((display, liste)) = cgs::published_topology() else {
        eprintln!("[ARBEITSPLATZ] Wahl: Schreibtische nicht lesbar");
        return;
    };
    // The physical Desktop LIVE (the watcher's copy can lag a fast swipe
    // by up to 120 ms) - it is excluded from the candidates below.
    let nutzer = cgs::aktiver_space().map(|(a, _)| a).filter(|a| *a != 0)
        .unwrap_or_else(|| LETZT_SID.load(Ordering::Relaxed));
    if nutzer == 0 {
        eprintln!("[ARBEITSPLATZ] Wahl: ungueltiger aktiver Space 0 - Auswahl unveraendert");
        return;
    }
    let jetzt_uuid = get_preview_target()
        .map(|t| t.uuid)
        .unwrap_or_else(|| arbeitsplatz_uuid_lesen(app));
    let Some(ziel) = arbeitsplatz::naechster_arbeitsplatz(&liste, &jetzt_uuid, nutzer, vor) else {
        eprintln!("[ARBEITSPLATZ] Wahl: kein anderer Schreibtisch zur Auswahl");
        return;
    };
    if ziel == jetzt_uuid {
        return;
    }
    let snapshot = match set_preview_target(
        app,
        &ziel,
        TargetChangeSource::UserShortcut,
        if vor { "shortcut_forward" } else { "shortcut_backward" },
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[ARBEITSPLATZ] set_preview_target failed: {e}");
            return;
        }
    };
    if let Ok(mut t) = TASK_FENSTER.lock() {
        t.clear();
    }
    // Label, membership and the page follow only once the new Desktop is
    // published (retarget worker) - never an intermediate "Schreibtisch N".
    eprintln!("[ARBEITSPLATZ] gewaehlt: Space {} ({}) = Schreibtisch {} epoch={}", snapshot.space_id, snapshot.uuid, snapshot.desktop_number, snapshot.epoch);

#[derive(Clone)]
struct RetargetJob {
    epoch: u64,
    target_id: u64,
    ziel_uuid: String,
    nutzer_uuid: String,
    nummer: usize,
}

#[cfg(target_os = "macos")]
fn retarget_einreihen(app: &tauri::AppHandle, job: RetargetJob) {
    static RETARGET_CHANNEL: std::sync::OnceLock<std::sync::mpsc::Sender<RetargetJob>> =
        std::sync::OnceLock::new();
    let tx = RETARGET_CHANNEL.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<RetargetJob>();
        let h = app.clone();
        thread::Builder::new()
            .name("noki-retarget".into())
            .spawn(move || {
                while let Ok(mut current) = rx.recv() {
                    while let Ok(newer) = rx.try_recv() {
                        current = newer;
                    }
                    if TARGET_EPOCH.load(Ordering::SeqCst) != current.epoch {
                        eprintln!("[RETARGET] epoch {} superseded; dropping async retarget", current.epoch);
                        continue;
                    }
                    let t_start = std::time::Instant::now();
                    let ids = sichtbare_fenster_auf_arbeitsplatz(current.target_id);
                    let ms_ids = t_start.elapsed().as_millis();
                    if TARGET_EPOCH.load(Ordering::SeqCst) != current.epoch {
                        continue;
                    }
                    // ATOMIC: build + publish the new Desktop while the old
                    // composition stays visible; only then membership, label
                    // and page (no flash, no wrong intermediate label).
                    let t0 = std::time::Instant::now();
                    // Label + app bar of THIS target travel with the retarget
                    // and are applied in the same swap as its picture. The
                    // app bar here is the AX-free one (never blocks input).
                    vorschau::ziel_meta(&format!("Zum Schreibtisch {}", current.nummer),
                        &leiste_schnell(&ids, current.target_id));
                    let ms_meta = t0.elapsed().as_millis();
                    let _ = vorschau::setzen(&h, ids, 520, 30, current.ziel_uuid.clone(), current.nutzer_uuid);
                    while t0.elapsed() < Duration::from_millis(2500)
                        && vorschau::veroeffentlichte_uuid() != current.ziel_uuid
                        && TARGET_EPOCH.load(Ordering::SeqCst) == current.epoch
                    {
                        thread::sleep(Duration::from_millis(15));
                    }
                    if TARGET_EPOCH.load(Ordering::SeqCst) != current.epoch {
                        continue; // a newer press owns the Miniatur
                    }
                    let ms_publish = t0.elapsed().as_millis();
                    vorschau_spaces_richten(&h);
                    let navi_text = format!("Zum Schreibtisch {}", current.nummer);
                    vorschau::navi(&navi_text);
                    // Exact app bar (AX lights, scaling) after the swap.
                    leiste_anstossen(&h);
                    virtual_workspace::trace(&format!(
                        "[ARBEITSPLATZ] cycle published target={} nummer={} ms={} ids_ms={ms_ids} meta_ms={ms_meta} publish_ms={ms_publish}",
                        current.target_id, current.nummer, t_start.elapsed().as_millis()));
                    let _ = h.emit("noki://arbeitsplatz_gewaehlt", serde_json::json!({
                        "epoch": current.epoch, "uuid": current.ziel_uuid, "id": current.target_id,
                        "nummer": current.nummer, "auf_arbeitsplatz": false, "navi_text": navi_text
                    }));
                }
            })
            .ok();
        tx
    });
    let _ = tx.send(job);
}

    // ASYNC PREVIEW BUILD: coalescing background worker retrieves target windows and triggers atomic swap
    let nutzer_uuid = liste.iter().find(|s| s.id == nutzer)
        .map(|s| s.uuid.clone()).unwrap_or_default();
    retarget_einreihen(app, RetargetJob {
        epoch: snapshot.epoch,
        target_id: snapshot.space_id,
        ziel_uuid: snapshot.uuid.clone(),
        nutzer_uuid,
        nummer: snapshot.desktop_number,
    });
}

#[cfg(not(target_os = "macos"))]
fn arbeitsplatz_waehlen(_app: &tauri::AppHandle, _vor: bool) {}


// ---------------------------------------------------------------------
//  App-Leiste von Noki Schreibtisch
// ---------------------------------------------------------------------
//  Quelle ist ausschliesslich die Registrierung (Besitz), nie die Liste
//  aller Programme. Aktiv = vorderstes registriertes Fenster der echten
//  Stapelung - dieselbe Wahrheit, die auch ^+Pfeil benutzt.

/// Fenster nach vorn: echt (AXRaise), und wenn macOS es ueber Programm-
/// grenzen nicht vorn stapelt, auf Ebene der Komposition.
#[cfg(target_os = "macos")]
fn noki_fenster_vorn(pid: i32, wid: i64, live: &[i64]) -> bool {
    if virtual_workspace::backend() == virtual_workspace::Backend::RealSpace {
        return vorschau::real_nach_vorn(pid, wid);
    }
    vorschau::fernbedienung::hauptfenster_leihen(pid);
    let ok = vorschau::fernbedienung::fenster_heben(pid, wid);
    thread::sleep(Duration::from_millis(60));
    // Immer die ausdrueckliche Wahl festhalten: die echte Stapelung
    // zwischen NICHT aktiven Programmen folgt AXRaise nicht verlaesslich
    // (gemessen: Claude "vorn", Treffer landeten trotzdem im VS-Code-Fenster).
    let _ = live;
    vorschau::vorn_setzen(wid);
    vorschau::stapel();
    ok
}

/// Aktives Noki-Fenster: gewaehltes Kompositions-Vorn, sonst echte Stapelung.
#[cfg(target_os = "macos")]
fn noki_aktiv(live: &[i64]) -> i64 {
    let v = vorschau::vorn();
    if v != 0 && live.contains(&v) { return v; }
    cgs::stapel_vorn_zuerst().into_iter().find(|w| live.contains(w)).unwrap_or(0)
}

/// Die Leiste neu an den Helfer geben (nur wenn sich etwas geaendert hat).
#[cfg(target_os = "macos")]
pub(crate) fn leiste_aktualisieren(app: &tauri::AppHandle) {
    if virtual_workspace::backend() == virtual_workspace::Backend::RealSpace {
        real_leiste_aktualisieren();
        return;
    }
    if !virtual_workspace::ready() { return; }
    let live = workspace_registry_refresh(app);
    // Child-window observers for every app that hosts a Noki window.
    let pids: Vec<i32> = arbeitsplatz_fenster_liste().into_iter()
        .filter(|e| e.fenster != 0 && live.contains(&e.fenster)).map(|e| e.pid).collect();
    kind_fenster::beobachten(&pids);
    let aktiv = noki_aktiv(&live);
    let eintraege: Vec<serde_json::Value> = arbeitsplatz_fenster_liste().into_iter()
        .filter(|e| e.fenster != 0 && live.contains(&e.fenster))
        .map(|e| {
            // Chrome-Web-Apps: eigenes Symbol/Name (der Starter), nicht "Chrome".
            let webapp = e.restoration_target.ends_with(".app") && !e.restoration_target.ends_with("Google Chrome.app")
                && e.bundle_id == "com.google.Chrome";
            let pfad = if webapp { e.restoration_target.clone() }
                else if e.application_path.ends_with(".app") { e.application_path.clone() } else {
                lesezeichen::app_fuer_pid(e.pid).map(|(_, _, p)| p).unwrap_or_default()
            };
            let app_name = if webapp {
                std::path::Path::new(&pfad).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or(e.app.clone())
            } else { e.app.clone() };
            let ampeln = vorschau::fernbedienung::ampel_rahmen(e.pid, e.fenster);
            serde_json::json!({
                "wid": e.fenster, "app": app_name, "titel": e.titel, "pfad": pfad,
                "aktiv": e.fenster == aktiv, "minimiert": !cgs::fenster_eingeordnet(e.fenster),
                "maximiert": e.maximized_from.is_some(),
                "ampeln": ampeln.map(|a| a.iter().flatten().copied().collect::<Vec<f64>>()),
                "skalierbar": vorschau::fernbedienung::skalierbar(e.pid, e.fenster),
            })
        }).collect();
    vorschau::leiste(&serde_json::Value::Array(eintraege).to_string());
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn leiste_aktualisieren(_app: &tauri::AppHandle) {}

/// Nie auf dem Eingabeweg: nur anstossen; ein eigener Faden gleicht
/// hoechstens viermal je Sekunde ab (Rollen bleibt unberuehrt).
pub(crate) fn leiste_anstossen(app: &tauri::AppHandle) {
    static OFFEN: AtomicBool = AtomicBool::new(false);
    static FADEN: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    OFFEN.store(true, Ordering::SeqCst);
    FADEN.get_or_init(|| {
        let h = app.clone();
        let _ = thread::Builder::new().name("noki-leiste".into()).spawn(move || loop {
            thread::sleep(Duration::from_millis(250));
            if OFFEN.swap(false, Ordering::SeqCst) {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| leiste_aktualisieren(&h)));
            }
        });
    });
}

/// Befehle aus der Leiste (Helfer). Jeder prueft Besitz ueber die Registrierung.
#[cfg(target_os = "macos")]
pub(crate) fn leiste_befehl(app: &tauri::AppHandle, was: &str, arg: &str) {
    // Backend-aware readiness. `virtual_workspace::ready()` describes ONLY
    // the legacy virtual display and is always false under REAL_SPACE - this
    // gate used to make every app-bar command a silent no-op there.
    let echt = virtual_workspace::backend() == virtual_workspace::Backend::RealSpace;
    if echt {
        if !real_space_bereit() { return; }
        if matches!(was, "vorn" | "zu" | "min" | "max") {
            real_leiste_befehl(app, was, arg);
            leiste_aktualisieren(app);
            return;
        }
    } else if !virtual_workspace::ready() { return; }
    let eintrag = |wid: i64| arbeitsplatz_fenster_liste().into_iter()
        .find(|e| e.fenster == wid && e.fenster != 0);
    match was {
        "vorn" => {
            let Some(e) = arg.parse::<i64>().ok().and_then(eintrag) else { return };
            // Nur ein nicht angezeigtes Fenster (minimiert / Programm
            // ausgeblendet) wird zurueckgeholt - sonst bleibt alles, wie es ist.
            let zurueck = !cgs::fenster_eingeordnet(e.fenster)
                && vorschau::fernbedienung::fenster_zeigen(e.pid, e.fenster);
            let live = workspace_registry_refresh(app);
            let war_vorn = noki_aktiv(&live) == e.fenster;
            let ok = noki_fenster_vorn(e.pid, e.fenster, &live);
            // Nur ein WECHSEL ist eine Nachricht wert - nicht jeder Klick
            // in das ohnehin vordere Fenster.
            if !war_vorn || zurueck {
                vorschau::hinweis_fenster(&e.application_path, &e.app, &e.titel);
            }
            virtual_workspace::trace(&format!(
                "[LEISTE] front wid={} raised={ok} unminimized={zurueck} space_trip=false", e.fenster
            ));
        }
        "zu" => {
            let Some(e) = arg.parse::<i64>().ok().and_then(eintrag) else { return };
            // The app remembers the frame of the window closed here. It stays
            // on Noki's display ON PURPOSE: the next Noki open of this app is
            // then born on Noki's display (zero exposure on the user's
            // desktop). Previously the window was parked on the user display
            // first, which guaranteed a visible flash on every Noki reopen.
            // A later normal Dock launch that restores onto Noki's display is
            // handed back by the user-desktop guard within ~100 ms.
            let vorher = cgs::alle_fenster().into_iter().find(|f| f.0 == e.fenster && f.1 == e.pid);
            let restore_position_cleaned = false;
            if e.created_by_noki { noki_lage_merken(&e.bundle_id); }
            // NUR dieses eine registrierte Fenster - nie das Programm beenden.
            let gedrueckt = vorschau::fernbedienung::fenster_schliessen(e.pid, e.fenster);
            let mut weg = false;
            for _ in 0..10 {
                if !vorschau::fernbedienung::fenster_vorhanden(e.pid, e.fenster)
                    || !cgs::alle_fenster().iter().any(|f| f.0 == e.fenster) {
                    weg = true;
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
            // A few apps acknowledge AXCloseButton without closing. The
            // secondary route is allowed only after AX re-verifies this
            // exact window as focused; it sends Cmd-W to that PID, never a
            // global shortcut and never Quit.
            let kurzbefehl = !weg
                && vorschau::fernbedienung::fenster_schliessen_kurzbefehl(e.pid, e.fenster);
            if kurzbefehl {
                for _ in 0..30 {
                    if !vorschau::fernbedienung::fenster_vorhanden(e.pid, e.fenster)
                        || !cgs::alle_fenster().iter().any(|f| f.0 == e.fenster) {
                        weg = true;
                        break;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
            }
            virtual_workspace::trace(&format!(
                "[LEISTE] close wid={} app={} pressed={gedrueckt} shortcut={kurzbefehl} gone={weg} restore_position_cleaned={restore_position_cleaned} app_quit=false", e.fenster, e.app
            ));
            if !weg && restore_position_cleaned {
                if let Some(f) = vorher {
                    let _ = vorschau::fernbedienung::ax_lage(e.pid, e.fenster, f.4 as f64, f.5 as f64);
                    let _ = vorschau::fernbedienung::fenster_zeigen(e.pid, e.fenster);
                }
            }
            if weg {
                // Manche Programme (Safari, Finder) behalten ein geschlossenes
                // Fenster in der WindowServer-Liste. AX hat das Schliessen
                // nachgewiesen - also gilt es als geschlossen, sonst taete ^+Pfeil
                // so, als gaebe es das Fenster noch.
                if let Ok(mut registry) = ARBEITSPLATZ_FENSTER.lock() {
                    if let Some(r) = registry.iter_mut().find(|r| r.fenster == e.fenster && r.pid == e.pid) {
                        r.fenster = 0; r.pid = 0; r.visible = false;
                        r.lifecycle_state = "closed".into();
                    }
                }
                arbeitsplatz_fenster_sichern(app);
                // Noki's OWN second instance (Calendar): no window left in
                // Noki -> end that process. The user's instance is a
                // different pid and never touched.
                if e.restoration_kind == "second_instance"
                    && NOKI_INSTANZEN.lock().map(|g| g.contains(&e.pid)).unwrap_or(false)
                    && !arbeitsplatz_fenster_liste().iter().any(|r| r.pid == e.pid && r.fenster != 0)
                {
                    let _ = std::process::Command::new("/bin/kill").args(["-TERM", &e.pid.to_string()]).status();
                    virtual_workspace::trace(&format!("[LEISTE] close second_instance pid={} terminated=true user_instance_touched=false", e.pid));
                }
                vorschau_neu_setzen(app);
                vorschau::hinweis(&format!("„{}“ geschlossen", e.titel.chars().take(40).collect::<String>()));
            } else {
                vorschau::hinweis("Das Fenster wartet auf eine Antwort");
            }
        }
        "min" => {
            let Some(e) = arg.parse::<i64>().ok().and_then(eintrag) else { return };
            let ok = vorschau::fernbedienung::fenster_minimieren(e.pid, e.fenster);
            if ok {
                // Keep registry ownership, remove only the suspended source
                // from the visible composition.
                thread::sleep(Duration::from_millis(100));
                vorschau_neu_setzen(app);
                vorschau::hinweis(&format!("„{}“ minimiert", e.titel.chars().take(40).collect::<String>()));
            }
            virtual_workspace::trace(&format!(
                "[LEISTE] minimize wid={} ok={ok} registered=true space_trip=false", e.fenster
            ));
        }
        "max" => {
            let Some(e) = arg.parse::<i64>().ok().and_then(eintrag) else { return };
            let Some(display) = virtual_workspace::info() else { return };
            let observed = cgs::alle_fenster();
            let current = observed.iter().find(|(wid, pid, ..)| *wid == e.fenster && *pid == e.pid)
                .map(|(_,_,_,_,x,y,w,h,_)| [*x as f64,*y as f64,*w as f64,*h as f64]);
            let mut restoring = false;
            let mut target = [display.x as f64, display.y as f64,
                              display.width as f64, display.height as f64];
            let previous_mode = ARBEITSPLATZ_FENSTER.lock().ok()
                .and_then(|registry| registry.iter()
                    .find(|x| x.fenster == e.fenster && x.pid == e.pid)
                    .and_then(|x| x.maximized_from));
            if let Some(previous) = previous_mode {
                        target = previous; restoring = true;
            }
            if !restoring && current.is_none() { return; }
            let ok = vorschau::fernbedienung::ax_verschieben(
                e.pid, e.fenster, target[0], target[1], target[2], target[3]);
            if ok {
                if let Ok(mut registry) = ARBEITSPLATZ_FENSTER.lock() {
                    if let Some(entry) = registry.iter_mut().find(|x| x.fenster == e.fenster && x.pid == e.pid) {
                        entry.maximized_from = if restoring { None } else { current };
                        entry.desired_frame = Some(target);
                        entry.last_virtual_frame = Some(target);
                    }
                }
                arbeitsplatz_fenster_sichern(app);
                vorschau::hinweis(if restoring { "Fenster wiederhergestellt" } else { "Im Noki Schreibtisch maximiert" });
            }
            virtual_workspace::trace(&format!(
                "[LEISTE] workspace_maximize wid={} restore={restoring} ok={ok} native_fullscreen=false space_trip=false frame={:?}",
                e.fenster, target
            ));
        }
        "katalog" => {
            // The picker must open at once: answer from the cached list
            // (first time: a fast list without the per-app route checks) and
            // recompute exact statuses in the background for the next open.
            // Before, the synchronous AX/window checks for every running app
            // took seconds; a second click then closed the late panel.
            static KATALOG: Mutex<Option<String>> = Mutex::new(None);
            let bauen = |app: &tauri::AppHandle, genau: bool| -> String {
                let live = workspace_registry_refresh(app);
                let offen: Vec<String> = arbeitsplatz_fenster_liste().into_iter()
                    .filter(|e| e.fenster != 0 && live.contains(&e.fenster))
                    .map(|e| e.application_path).collect();
                let mut liste: Vec<serde_json::Value> = fokus_apps_laden().into_iter()
                    .map(|a| {
                        let pfad = a["pfad"].as_str().unwrap_or("");
                        let name = a["name"].as_str().unwrap_or("");
                        let status = if genau { katalog_zustand(pfad, name, &offen) }
                            else if offen.iter().any(|p| p == pfad) { "Im Noki Schreibtisch geöffnet".into() }
                            else { "In Noki öffnen".into() };
                        serde_json::json!({ "name": a["name"], "pfad": a["pfad"],
                                            "aliases": a["aliases"], "status": status })
                    }).collect();
                for (name, key, icon) in WEB_EINTRAEGE {
                    if !std::path::Path::new(icon).exists() { continue; }
                    let klein = name.to_lowercase();
                    let pos = liste.iter().position(|a| a["name"].as_str().is_some_and(|n| n.to_lowercase() > klein))
                        .unwrap_or(liste.len());
                    liste.insert(pos, serde_json::json!({ "name": name, "pfad": format!("web:{key}"),
                        "icon": icon, "aliases": [format!("{} browser", name)],
                        "status": "Browser-Version im Noki-Chrome" }));
                }
                serde_json::Value::Array(liste).to_string()
            };
            if arg == "vorwaermen" {
                // Startup: fill the cache so even the FIRST open is instant.
                let genau = bauen(app, true);
                if let Ok(mut g) = KATALOG.lock() { *g = Some(genau); }
                return;
            }
            let t0 = std::time::Instant::now();
            let sofort = KATALOG.lock().ok().and_then(|g| g.clone()).unwrap_or_else(|| bauen(app, false));
            vorschau::katalog(&sofort);
            virtual_workspace::trace(&format!("[LEISTE] katalog shown_ms={} source=cache_or_fast", t0.elapsed().as_millis()));
            let h = app.clone();
            thread::spawn(move || {
                let genau = bauen(&h, true);
                if let Ok(mut g) = KATALOG.lock() { *g = Some(genau); }
            });
            return;
        }
        // YouTube = the Noki YouTube window (Noki Browser, app mode) - never
        // the installed PWA, which freezes on the hidden Space.
        "neu" if echt && (arg.ends_with("/YouTube.app") || arg == "web:youtube") => {
            // A Noki YouTube window that is NOT on the Noki Space (born on the
            // user's Desktop by an earlier launch) is a stray: it made this
            // branch answer "already open" forever while the Miniatur never
            // got the smooth YouTube. Close it and create the real one.
            let noki_sid = vorschau::noki_space_id();
            let vorhanden = noki_browser::youtube_fenster().filter(|w| {
                // Unknown membership never closes anything.
                let fremd = noki_sid != 0
                    && cgs::spaces_des_fensters(*w).is_some_and(|s| !s.is_empty() && !s.contains(&noki_sid));
                if fremd { noki_browser::youtube_schliessen(*w); }
                !fremd
            });
            if let Some(w) = vorhanden {
                virtual_workspace::trace(&format!("[LEISTE] new app=youtube route=noki_youtube existing_wid={w}"));
                real_leiste_befehl(app, "vorn", &w.to_string());
                vorschau::hinweis("YouTube ist im Noki Schreibtisch geöffnet.");
            } else if vorschau::noki_space_id() != 0 && cgs::aktiver_space().map(|a| a.0) == Some(vorschau::noki_space_id()) {
                let r = noki_browser::youtube_starten();
                virtual_workspace::trace(&format!("[LEISTE] new app=youtube route=noki_youtube created={r:?}"));
            } else {
                noki_browser::youtube_vormerken();
                virtual_workspace::trace("[LEISTE] new app=youtube route=noki_youtube deferred=until_noki_space");
                vorschau::hinweis("YouTube öffnet sich, sobald du im Noki Schreibtisch bist – über „Zum Schreibtisch“.");
            }
        }
        "neu" if echt => {
            // App-bar input never switches a Space. Creating a window on the
            // hidden Noki Space needs a Space trip (macOS creates windows on
            // the ACTIVE Space) - so under REAL_SPACE it fails closed.
            virtual_workspace::trace(&format!("[LEISTE] new app={arg} refused=would_need_space_trip"));
            vorschau::hinweis("Neue Fenster öffnest du im echten Noki Schreibtisch – über „Zum Schreibtisch“.");
            return;
        }
        "neu" if arg.starts_with("web:") => {
            let key = &arg[4..];
            let meldung = match WEB_EINTRAEGE.iter().find(|e| e.1 == key) {
                Some((name, _, _)) => {
                    let _launch = NokiLaunchGuard::begin("/Applications/Google Chrome.app", "com.google.Chrome");
                    match noki_chrome_oeffnen(app, name, Some(web_adresse(key))) { Ok(m) => m, Err(m) => m }
                }
                None => "Unbekannter Eintrag.".into(),
            };
            vorschau::hinweis(&meldung);
        }
        "neu" => {
            let meldung = match virtuelles_app_oeffnen(app, arg) {
                Ok(m) => m,
                Err(m) => m,
            };
            vorschau::hinweis(&meldung);
        }
        _ => return,
    }
    leiste_aktualisieren(app);
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn leiste_befehl(_app: &tauri::AppHandle, _was: &str, _arg: &str) {}

/// REAL_SPACE readiness: a Noki Space is resolved and the preview runs.
#[cfg(target_os = "macos")]
fn real_space_bereit() -> bool {
    ARBEITSPLATZ.lock().ok().and_then(|g| g.as_ref().map(|r| r.id)).unwrap_or(0) != 0
        && vorschau::laeuft()
}

/// REAL_SPACE app bar: exactly the windows of the Noki Space (the same set
/// the preview shows); active = the native per-Space top window.
#[cfg(target_os = "macos")]
fn real_leiste_aktualisieren() {
    if !real_space_bereit() { return; }
    let ap = ARBEITSPLATZ.lock().ok().and_then(|g| g.as_ref().map(|r| r.id)).unwrap_or(0);
    vorschau::leiste(&real_leiste_json(&vorschau::aufgenommene(), ap));
}

/// Last exact app bar per Space: (sorted window set, json).
#[cfg(target_os = "macos")]
static LEISTE_JE_SPACE: Mutex<Option<std::collections::HashMap<u64, (Vec<i64>, String)>>> = Mutex::new(None);

/// App bar for a retarget WITHOUT any AX round trip (never on the input
/// path): the exact bar last computed for this Desktop if its window set is
/// unchanged, else names/icons only (lights/scaling follow right after the
/// swap from `real_leiste_json`). Measured: the AX bar ran serially before
/// every Shortcut retarget (p90 889 ms, max 1395 ms input -> swap).
#[cfg(target_os = "macos")]
pub(crate) fn leiste_schnell(live: &[i64], ap: u64) -> String {
    let mut satz = live.to_vec();
    satz.sort_unstable();
    if let Some((ids, json)) = LEISTE_JE_SPACE.lock().ok().and_then(|g| g.as_ref().and_then(|m| m.get(&ap).cloned())) {
        if ids == satz {
            return json;
        }
    }
    let aktiv = cgs::fenster_reihe(ap).into_iter().find(|w| live.contains(w)).unwrap_or(0);
    let alle = cgs::alle_fenster();
    let eintraege: Vec<serde_json::Value> = live.iter().filter_map(|wid| {
        let (_, pid, owner, titel, ..) = alle.iter().find(|f| f.0 == *wid)?.clone();
        let (mut name, _, pfad) = lesezeichen::app_fuer_pid(pid).unwrap_or((owner, String::new(), String::new()));
        if Some(*wid) == ask_nutzerfenster() { name = "Ask Noki".into(); }
        Some(serde_json::json!({
            "wid": wid, "app": name, "titel": titel, "pfad": pfad, "icon": "",
            "aktiv": *wid == aktiv, "minimiert": false, "maximiert": false,
            "ampeln": serde_json::Value::Null, "skalierbar": false,
        }))
    }).collect();
    serde_json::Value::Array(eintraege).to_string()
}

/// App bar entries for exactly these windows of Space `ap` (front first).
#[cfg(target_os = "macos")]
pub(crate) fn real_leiste_json(live: &[i64], ap: u64) -> String {
    let json = real_leiste_json_bauen(live, ap);
    let mut satz = live.to_vec();
    satz.sort_unstable();
    if let Ok(mut g) = LEISTE_JE_SPACE.lock() {
        g.get_or_insert_with(Default::default).insert(ap, (satz, json.clone()));
    }
    json
}

#[cfg(target_os = "macos")]
fn real_leiste_json_bauen(live: &[i64], ap: u64) -> String {
    let live = live.to_vec();
    let aktiv = cgs::fenster_reihe(ap).into_iter().find(|w| live.contains(w)).unwrap_or(0);
    let alle = cgs::alle_fenster();
    let eintraege: Vec<serde_json::Value> = live.iter().filter_map(|wid| {
        let (_, pid, owner, titel, ..) = alle.iter().find(|f| f.0 == *wid)?.clone();
        // Only real windows: an invisible helper window without an AX
        // element (Safari) must not become a ghost chip.
        if !vorschau::fernbedienung::fenster_vorhanden(pid, *wid) { return None; }
        let (mut name, _, mut pfad) = lesezeichen::app_fuer_pid(pid).unwrap_or((owner, String::new(), String::new()));
        // The Noki YouTube window is its own app in the bar (name + icon),
        // although a Noki Browser window underneath.
        let mut icon = String::new();
        if Some(*wid) == ask_nutzerfenster() {
            name = "Ask Noki".into();
        }
        if noki_browser::ist_youtube_fenster(*wid) {
            name = "YouTube".into();
            pfad = "noki:youtube".into();
            icon = if std::path::Path::new("/Applications/YouTube.app").exists() { "/Applications/YouTube.app".into() }
                else { "/Applications/Google Chrome.app".into() };
        }
        let ampeln = vorschau::fernbedienung::ampel_rahmen(pid, *wid);
        Some(serde_json::json!({
            "wid": wid, "app": name, "titel": titel, "pfad": pfad, "icon": icon,
            "aktiv": *wid == aktiv, "minimiert": !cgs::fenster_eingeordnet(*wid),
            "maximiert": false,
            "ampeln": ampeln.map(|a| a.iter().flatten().copied().collect::<Vec<f64>>()),
            "skalierbar": vorschau::fernbedienung::skalierbar(pid, *wid),
        }))
    }).collect();
    serde_json::Value::Array(eintraege).to_string()
}

/// REAL_SPACE app-bar commands on EXACTLY one window of the Noki Space.
#[cfg(target_os = "macos")]
fn real_leiste_befehl(app: &tauri::AppHandle, was: &str, arg: &str) {
    let Ok(wid) = arg.parse::<i64>() else { return };
    // Only a window that really is part of the Noki Space composition.
    if !vorschau::aufgenommene().contains(&wid) {
        virtual_workspace::trace(&format!("[LEISTE] {was} wid={wid} refused=not_on_noki_space"));
        return;
    }
    let Some((_, pid, owner, titel, ..)) = cgs::alle_fenster().into_iter().find(|f| f.0 == wid) else { return };
    let name = lesezeichen::app_fuer_pid(pid).map(|a| a.0).unwrap_or(owner);
    let pfad = lesezeichen::app_fuer_pid(pid).map(|a| a.2).unwrap_or_default();
    match was {
        "vorn" => {
            if pid == blende::vorn_pid() {
                // Raising a hidden window of the frontmost app makes macOS
                // follow it to that Space (measured) - fail closed, say why.
                vorschau::hinweis(&format!("{name} ist gerade vorne aktiv – erst ein anderes Programm wählen."));
                virtual_workspace::trace(&format!("[LEISTE] front wid={wid} refused=target_app_frontmost"));
                return;
            }
            let zurueck = !cgs::fenster_eingeordnet(wid) && vorschau::fernbedienung::fenster_zeigen(pid, wid);
            let ok = vorschau::real_nach_vorn(pid, wid);
            vorschau::hinweis_fenster(&pfad, &name, &titel);
            virtual_workspace::trace(&format!("[LEISTE] front wid={wid} native_top={ok} unminimized={zurueck} space_trip=false"));
        }
        "zu" => {
            let gedrueckt = vorschau::fernbedienung::fenster_schliessen(pid, wid);
            let mut weg = false;
            for _ in 0..10 {
                if !cgs::alle_fenster().iter().any(|f| f.0 == wid) || !vorschau::fernbedienung::fenster_vorhanden(pid, wid) { weg = true; break; }
                thread::sleep(Duration::from_millis(50));
            }
            if weg { vorschau_neu_setzen(app); } else {
                vorschau::hinweis(&format!("{name} hat das Fenster nicht geschlossen (evtl. ungesicherte Änderungen)."));
            }
            virtual_workspace::trace(&format!("[LEISTE] close wid={wid} app={name} pressed={gedrueckt} gone={weg} app_quit=false"));
        }
        "min" => {
            let ok = vorschau::fernbedienung::fenster_minimieren(pid, wid);
            if ok { thread::sleep(Duration::from_millis(100)); vorschau_neu_setzen(app); }
            virtual_workspace::trace(&format!("[LEISTE] minimize wid={wid} ok={ok}"));
        }
        _ => {
            vorschau::hinweis("Maximieren gibt es im echten Noki Schreibtisch – über „Zum Schreibtisch“.");
        }
    }
}

/// Komposition = eingeordnete registrierte Fenster + offene echte Menues.
#[cfg(target_os = "macos")]
fn vorschau_neu_setzen(app: &tauri::AppHandle) {
    if virtual_workspace::backend() == virtual_workspace::Backend::RealSpace {
        // REAL_SPACE has one authority: the windows currently belonging to
        // the selected physical Space.  The legacy ownership registry may
        // decide what Noki is allowed to mutate, never what the portal shows.
        let _ = noki_vorschau_setzen(app.clone(), None, Some(520), Some(8));
        return;
    }
    let mut ids = vorschau_faehig(workspace_registry_refresh(app));
    ids.extend(vorschau::menues());
    let _ = vorschau::setzen(app, ids, 520, 30, "virtual-display".into(), String::new());
}

#[cfg(target_os = "macos")]
fn bundle_id_von(pfad: &str) -> Option<String> {
    let out = std::process::Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Print CFBundleIdentifier", &format!("{pfad}/Contents/Info.plist")])
        .output().ok()?;
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!id.is_empty()).then_some(id)
}

/// Ausdrueckliche Web-Eintraege im Apps-Picker (Name, Schluessel, Symbol).
/// "Spotify" / "Claude" / "ChatGPT" bedeuten immer die echte App; die
/// Browser-Fassung gibt es nur, wenn der Nutzer DIESEN Eintrag waehlt.
const WEB_EINTRAEGE: [(&str, &str, &str); 4] = [
    ("YouTube Web", "youtube", "/Applications/Google Chrome.app"),
    ("Spotify Web", "spotify", "/Applications/Spotify.app"),
    ("Claude Web", "claude", "/Applications/Claude.app"),
    ("ChatGPT Web", "chatgpt", "/Applications/ChatGPT.app"),
];

fn web_adresse(key: &str) -> &'static str {
    match key {
        "youtube" => "https://www.youtube.com/",
        "spotify" => "https://open.spotify.com/",
        "claude" => "https://claude.ai/new",
        _ => "https://chatgpt.com/",
    }
}

/// Chromium-basiert (Electron / CEF)? Solche Programme liefern ihren
/// Accessibility-Baum erst mit `--force-renderer-accessibility` (gemessen:
/// Spotify zeigt sonst nur leere Gruppen - keine Knoepfe, keine Regler).
fn chromium_basiert(pfad: &str) -> bool {
    let f = std::path::Path::new(pfad).join("Contents/Frameworks");
    ["Electron Framework.framework", "Chromium Embedded Framework.framework"].iter().any(|n| f.join(n).exists())
}


/// Nach dem Anlegen eines Noki-Fensters: holt sich das Programm selbst den
/// Vordergrund (Chrome tut das gemessen bis ~1 s spaeter), bekommt ihn das
/// Programm des Nutzers zurueck. War der Nutzer selbst in diesem Programm,
/// wird sein eigenes Fenster wieder Hauptfenster. Nie ein Schreibtischwechsel:
/// das Programm des Nutzers hat sein Fenster auf dem aktuellen Schreibtisch.
#[cfg(target_os = "macos")]
fn vordergrund_bewahren(pid: i32, vorn_vorher: i32, nutzer_fenster: Option<i64>, noki_fenster: i64) {
    thread::spawn(move || {
        // Electron-Programme (VS Code, Figma) fokussieren ihr neues Fenster
        // gemessen noch ~3 s spaeter ein zweites Mal - deshalb 6 s. Tippt der
        // Nutzer inzwischen selbst in Noki, gehoert der Fokus ihm.
        let bis = std::time::Instant::now() + Duration::from_millis(6000);
        let mut zurueck = 0;
        while std::time::Instant::now() < bis {
            if fern_tippen::aktiv() { break; }
            let jetzt = blende::vorn_pid();
            if vorn_vorher > 0 && vorn_vorher != pid && jetzt == pid {
                blende::vorn_zurueck(vorn_vorher);
                zurueck += 1;
            }
            if vorn_vorher == pid {
                if let Some(u) = nutzer_fenster.filter(|u| *u != noki_fenster) {
                    if vorschau::fernbedienung::fokus_fenster(pid) == Some(noki_fenster) {
                        vorschau::fernbedienung::fenster_vorn(pid, u);
                        zurueck += 1;
                    }
                }
            }
            thread::sleep(Duration::from_millis(25));
        }
        virtual_workspace::trace(&format!(
            "[LEISTE] focus_guard pid={pid} user_front={vorn_vorher} restored={zurueck} front_now={} space_trip=false",
            blende::vorn_pid()
        ));
    });
}


/// Noki Chrome: das Noki-eigene Chrome-Fenster (vorhanden oder neu) nach
/// vorn, optional mit einer Adresse in einem neuen Tab.
#[cfg(target_os = "macos")]
fn noki_chrome_oeffnen(app: &tauri::AppHandle, was: &str, url: Option<&str>) -> Result<String, String> {
    const CHROME: &str = "/Applications/Google Chrome.app";
    const CHROME_ID: &str = "com.google.Chrome";
    if !std::path::Path::new(CHROME).exists() {
        return Err(format!("{was}: Google Chrome ist nicht installiert."));
    }
    let live = workspace_registry_refresh(app);
    // Nur ein echtes Browserfenster - nie das Fenster einer Chrome-Web-App.
    let ist_browserfenster = |e: &EigenesFenster| e.bundle_id == CHROME_ID
        && (e.restoration_target.is_empty() || !e.restoration_target.ends_with(".app")
            || e.restoration_target.ends_with("Google Chrome.app"));
    let eigenes = arbeitsplatz_fenster_liste().into_iter()
        .find(|e| e.fenster != 0 && live.contains(&e.fenster) && ist_browserfenster(e));
    let (pid, wid, frisch) = match eigenes {
        Some(e) => {
            if !cgs::fenster_eingeordnet(e.fenster) { vorschau::fernbedienung::fenster_zeigen(e.pid, e.fenster); }
            (e.pid, e.fenster, false)
        }
        None => match lesezeichen::pid_fuer_bundle(CHROME_ID) {
            Some(pid) => {
                // Chrome's own "New Window", moved in the creation callback
                // (same route and Space precondition as every running app).
                if !(app_auf_aktuellem_schreibtisch(pid) || !fenster_auf_anderen_spaces(pid)) {
                    virtual_workspace::trace(&format!("[LEISTE] open app={was} route=noki_chrome result=refused reason=space_switch_risk"));
                    return Err(format!("{was}: Chrome hat nur Fenster auf einem anderen Schreibtisch – ein neues Fenster würde dich dorthin wechseln lassen."));
                }
                let _ = menue_neues_fenster(app, pid, CHROME, "Chrome")?;
                let live = workspace_registry_refresh(app);
                let e = arbeitsplatz_fenster_liste().into_iter()
                    .find(|e| e.fenster != 0 && live.contains(&e.fenster) && ist_browserfenster(e))
                    .ok_or("Chrome: kein Fenster auf Noki Schreibtisch.")?;
                (e.pid, e.fenster, true)
            }
            None => {
                // Chrome laeuft nicht: der gemessene Hintergrundstart.
                virtuelles_programm_starten(app, CHROME, "Chrome", CHROME_ID, None)?;
                let live = workspace_registry_refresh(app);
                let e = arbeitsplatz_fenster_liste().into_iter()
                    .find(|e| e.fenster != 0 && live.contains(&e.fenster) && ist_browserfenster(e))
                    .ok_or("Chrome: kein Fenster auf Noki Schreibtisch.")?;
                (e.pid, e.fenster, true)
            }
        },
    };
    let vorn_vorher = blende::vorn_pid();
    let nutzer_fenster = if vorn_vorher == pid { vorschau::fernbedienung::fokus_fenster(pid) } else { None };
    let _ = vorschau::fernbedienung::fenster_heben(pid, wid);
    let mut ok = true;
    if !frisch {
        // Ein neues Fenster hat seinen leeren Tab schon.
        ok = vorschau::fernbedienung::neuer_tab(pid, wid);
        thread::sleep(Duration::from_millis(250));
    }
    if let Some(url) = url.filter(|_| ok) {
        if frisch { thread::sleep(Duration::from_millis(200)); }
        ok = vorschau::fernbedienung::adresse_oeffnen(pid, wid, url, fern_tippen::MARKE);
        if ok {
            // Nachweis: der Titel wechselt vom leeren Tab weg.
            ok = false;
            for _ in 0..40 {
                let t = vorschau::fernbedienung::fenster_titel(pid, wid).to_lowercase();
                if !t.is_empty() && !t.starts_with("neuer tab") && !t.starts_with("new tab") { ok = true; break; }
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
    // Wer vorher in Chrome gearbeitet hat, bekommt sein Fenster zurueck.
    if let Some(u) = nutzer_fenster.filter(|u| *u != wid) { vorschau::fernbedienung::fenster_vorn(pid, u); }
    vorschau::stapel();
    vorschau_neu_setzen(app);
    let live = workspace_registry_refresh(app);
    let _ = noki_fenster_vorn(pid, wid, &live);
    virtual_workspace::trace(&format!(
        "[LEISTE] open app={was} route=noki_chrome wid={wid} new_window={frisch} url={} ok={ok} space_trip=false",
        url.unwrap_or("-")
    ));
    if !ok {
        return Err(match url { Some(_) => format!("{was} ließ sich im Noki-Chrome-Fenster nicht laden."),
                               None => "Chrome: kein neuer Tab möglich.".into() });
    }
    Ok(match (url, frisch) {
        (Some(_), _) => format!("{was} ist im Noki Schreibtisch geöffnet"),
        (None, true) => "Chrome ist im Noki Schreibtisch geöffnet".into(),
        (None, false) => "Neuer Tab im Noki-Chrome-Fenster".into(),
    })
}

/// Programm laeuft nicht: im Hintergrund starten (keine Aktivierung), sein
/// neues Fenster auf Noki Schreibtisch holen und registrieren.
#[cfg(target_os = "macos")]
fn virtuelles_programm_starten(app: &tauri::AppHandle, pfad: &str, name: &str, bundle: &str, skript: Option<&str>) -> Result<(i32, i64), String> {
    let Some(display) = virtual_workspace::info() else { return Err("Noki Schreibtisch ist nicht bereit.".into()) };
    let live = workspace_registry_refresh(app);
    let vorher: std::collections::HashSet<i64> = cgs::alle_fenster().into_iter().map(|f| f.0).collect();
    let space_vorher = cgs::aktiver_space();
    match skript {
        Some(sk) => { let _ = std::process::Command::new("/usr/bin/osascript").args(["-e", sk]).status(); }
        None => {
            // -F: ohne Wiederherstellung alter Fenster - sonst oeffnet macOS
            // sie auf dem Schreibtisch, auf dem sie zuletzt lagen, und holt den
            // Nutzer beim Aktivieren dorthin.
            let mut args = vec!["-g", "-j", "-F", "-a", pfad];
            if chromium_basiert(pfad) { args.extend(["--args", "--force-renderer-accessibility"]); }
            let _ = std::process::Command::new("/usr/bin/open").args(&args).status();
        }
    }
    // Das NEUE Fenster dieses Programms finden (nie ein vorhandenes).
    let frist = std::time::Instant::now() + Duration::from_secs(10);
    let mut neu: Vec<(i64, i32, i64, i64)> = vec![];
    while std::time::Instant::now() < frist && neu.is_empty() {
        thread::sleep(Duration::from_millis(60));
        let pid = lesezeichen::pid_fuer_bundle(bundle).unwrap_or(0);
        neu = cgs::alle_fenster().into_iter()
            .filter(|f| f.1 == pid && pid > 0 && !vorher.contains(&f.0))
            .map(|f| (f.0, f.1, f.6, f.7)).take(3).collect();
    }
    if cgs::aktiver_space() != space_vorher {
        virtual_workspace::trace(&format!("[LEISTE] open app={name} WARNING physical_space_changed"));
    }
    if neu.is_empty() {
        virtual_workspace::trace(&format!("[LEISTE] open app={name} result=no_window"));
        return Err(format!("{name} hat kein Fenster geöffnet."));
    }
    let mut erster = None;
    for (i, (wid, pid, w, h)) in neu.iter().enumerate() {
        // Honest diagnostics: a cold-launched app that is not hidden at this
        // point has shown its window on the user's display.
        let sichtbar_vorher = !vorschau::fernbedienung::programm_ausgeblendet(*pid)
            && cgs::fenster_eingeordnet(*wid)
            && !cgs::alle_fenster().iter().any(|f| f.0 == *wid && f.4 >= display.x as i64 && f.5 >= display.y as i64);
        virtual_workspace::trace(&format!("[EXPOSURE] app={name} wid={wid} route=cold_hidden visible_on_user_display_before_move={sichtbar_vorher}"));
        let w = (*w as f64).min(display.width as f64 * 0.8);
        let h = (*h as f64).min(display.height as f64 * 0.8);
        let versatz = 40.0 * (i as f64 + (live.len() % 5) as f64);
        let rahmen = [display.x as f64 + 60.0 + versatz, display.y as f64 + 50.0 + versatz, w, h];
        let _ = vorschau::fernbedienung::ax_verschieben(*pid, *wid, rahmen[0], rahmen[1], rahmen[2], rahmen[3]);
        // Massgeblich ist, wo das Fenster WIRKLICH steht: nicht skalierbare
        // Fenster (z. B. Rechner) lehnen nur die Groesse ab - gemessen stand
        // das Fenster danach richtig auf Noki Schreibtisch.
        let mut ist = None;
        let mut ok = false;
        for _ in 0..50 {
            ist = cgs::alle_fenster().into_iter().find(|f| f.0 == *wid);
            ok = ist.as_ref().is_some_and(|f| f.4 >= display.x as i64 && f.5 >= display.y as i64
                && f.4 < (display.x + display.width) as i64 && f.5 < (display.y + display.height) as i64);
            if ok { break; }
            thread::sleep(Duration::from_millis(20));
        }
        let rahmen = ist.map(|f| [f.4 as f64, f.5 as f64, f.6 as f64, f.7 as f64]).unwrap_or(rahmen);
        virtual_workspace::trace(&format!("[LEISTE] open app={name} wid={wid} on_noki_display={ok} space_trip=false"));
        if !ok { continue; }
        if !virtuellen_registry_eintrag_sichern(app, *wid, *pid, pfad, pfad, rahmen) {
            let _ = vorschau::fernbedienung::fenster_schliessen(*pid, *wid);
            continue;
        }
        if let Ok(mut r) = ARBEITSPLATZ_FENSTER.lock() {
            if let Some(e) = r.iter_mut().find(|e| e.fenster == *wid && e.pid == *pid) {
                e.restoration_kind = "app".into();
            }
        }
        arbeitsplatz_fenster_sichern(app);
        erster.get_or_insert((*pid, *wid));
    }
    let Some((pid, wid)) = erster else { return Err(format!("{name}: Fenster ließ sich nicht auf Noki Schreibtisch holen.")) };
    // Im Hintergrund (-j) gestartet: jetzt, wo das Fenster auf Noki
    // Schreibtisch steht, einblenden - ohne Aktivierung.
    let _ = vorschau::fernbedienung::fenster_zeigen(pid, wid);
    Ok((pid, wid))
}

#[cfg(target_os = "macos")]
fn pid_von(bundle: &str) -> i32 { lesezeichen::pid_fuer_bundle(bundle).unwrap_or(0) }

/// Zeigt das Programm dem Nutzer ein Fenster? Sichtbar auf irgendeinem
/// Schreibtisch (eingeordnet), minimiert (AX kennt es), oder das Programm ist
/// ausgeblendet (Cmd-H: das ist Absicht des Nutzers).
pub(crate) fn prozess_hat_noki_fenster(pid: i32) -> bool {
    let alle: Vec<i64> = cgs::alle_fenster().into_iter().filter(|f| f.1 == pid).map(|f| f.0).collect();
    arbeitsplatz_fenster_liste().into_iter().any(|e| e.pid == pid && e.fenster != 0 && alle.contains(&e.fenster))
}

#[cfg(target_os = "macos")]
#[cfg(target_os = "macos")]
pub(crate) fn nutzer_hat_fenster(pid: i32) -> bool {
    let noki: Vec<i64> = arbeitsplatz_fenster_liste().into_iter().map(|e| e.fenster).collect();
    let auf_noki = |f: &(i64, i32, String, String, i64, i64, i64, i64, i64)| virtual_workspace::info()
        .is_some_and(|d| f.4 >= d.x as i64 && f.5 >= d.y as i64);
    // Space membership is deliberately not a prerequisite here. A normal
    // user window on another Space, a minimized window, or a temporarily
    // hidden Electron window is still user-owned. False-positive fail-closed
    // is safer than resizing or relocating that window.
    // A titled window that is neither ordered in nor known to AX is CLOSED:
    // Spotify/Safari/Finder keep closed windows in the WindowServer list with
    // their title (measured: Spotify "Spotify Premium", onscreen=0, AXWindows
    // empty). Counting it as the user's made every Noki open of a closed
    // Spotify fail with single_window_in_use_by_user.
    let sichtbar = cgs::alle_fenster().into_iter()
        .any(|f| f.1 == pid && f.6 >= 200 && f.7 >= 150 && !noki.contains(&f.0)
             && !auf_noki(&f) && (cgs::fenster_eingeordnet(f.0)
                 || (!f.3.trim().is_empty() && vorschau::fernbedienung::fenster_vorhanden(pid, f.0))));
    let ax_beim_nutzer = vorschau::fernbedienung::ax_fenster_rahmen(pid).into_iter().any(|r| {
        r[2] >= 200.0 && r[3] >= 150.0
            && !virtual_workspace::info().is_some_and(|d| r[0] >= d.x as f64 && r[1] >= d.y as f64)
    });
    sichtbar || ax_beim_nutzer || vorschau::fernbedienung::programm_ausgeblendet(pid)
}

/// Programm laeuft ohne sichtbares Fenster: "wieder oeffnen" (ohne
/// Aktivierung), sein Fenster sofort auf Noki Schreibtisch holen, pruefen,
/// registrieren. Der Vordergrund des Nutzers bleibt.
#[cfg(target_os = "macos")]
fn programm_fenster_wieder_oeffnen(app: &tauri::AppHandle, pid: i32, pfad: &str, name: &str) -> Result<String, String> {
    let Some(display) = virtual_workspace::info() else { return Err("Noki Schreibtisch ist nicht bereit.".into()) };
    let vorn_vorher = blende::vorn_pid();
    let vorher: std::collections::HashMap<i64, bool> = cgs::alle_fenster().into_iter()
        .filter(|f| f.1 == pid).map(|f| (f.0, cgs::fenster_eingeordnet(f.0))).collect();
    let t0 = std::time::Instant::now();
    // Never adopt a merely-present unregistered window, even if the app or
    // macOS restored it on the virtual display. Only a state transition after
    // this explicit Noki request can become the transaction's candidate.
    let mut ziel = None;
    let _ = std::process::Command::new("/usr/bin/open").args(["-g", "-a", pfad]).status();
    while t0.elapsed() < Duration::from_secs(6) && ziel.is_none() {
        ziel = cgs::alle_fenster().into_iter().find(|f| f.1 == pid && f.6 >= 300 && f.7 >= 200
            && cgs::fenster_eingeordnet(f.0) && vorher.get(&f.0).copied() != Some(true));
        if ziel.is_none() { thread::sleep(Duration::from_millis(5)); }
    }
    let Some(f) = ziel else {
        virtual_workspace::trace(&format!("[LEISTE] open app={name} route=reopen result=no_window"));
        return Err(format!("{name} hat kein Fenster geöffnet."));
    };
    let wid = f.0;
    let gefunden_ms = t0.elapsed().as_millis();
    let w = (f.6 as f64).min(display.width as f64 - 80.0);
    let h = (f.7 as f64).min(display.height as f64 - 60.0);
    let rahmen = [display.x as f64 + 60.0, display.y as f64 + 40.0, w, h];
    let mut bewegt = false;
    while !bewegt && t0.elapsed() < Duration::from_secs(8) {
        bewegt = vorschau::fernbedienung::ax_verschieben(pid, wid, rahmen[0], rahmen[1], rahmen[2], rahmen[3])
            || cgs::alle_fenster().iter().any(|g| g.0 == wid && g.4 >= display.x as i64);
        if !bewegt { thread::sleep(Duration::from_millis(5)); }
    }
    // WindowServer meldet die neue Lage mit Verzoegerung (gemessen: nach
    // 80 ms noch die alte) - bis zu 1 s nachsehen.
    let mut ist = None;
    let mut auf_noki = false;
    let mut nachgesetzt = 0;
    for i in 0..100 {
        ist = cgs::alle_fenster().into_iter().find(|g| g.0 == wid);
        auf_noki = ist.as_ref().is_some_and(|g| g.4 >= display.x as i64 && g.5 >= display.y as i64
            && g.4 < (display.x + display.width) as i64 && g.5 < (display.y + display.height) as i64);
        if auf_noki { break; }
        // Gemessen Spotify: nach dem Wiederoeffnen setzt es seine gemerkte
        // Lage NACH unserem ersten Verschieben noch einmal (wie Finder).
        if i % 5 == 4 {
            nachgesetzt += 1;
            let _ = vorschau::fernbedienung::ax_verschieben(pid, wid, rahmen[0], rahmen[1], rahmen[2], rahmen[3]);
        }
        thread::sleep(Duration::from_millis(20));
    }
    virtual_workspace::trace(&format!(
        "[LEISTE] open app={name} route=native_reopen wid={wid} found_ms={gefunden_ms} moved_ms={} re_moves={nachgesetzt} on_noki_display={auf_noki} frame={:?} display=({},{},{},{}) web_fallback=false space_trip=false",
        t0.elapsed().as_millis(), ist.as_ref().map(|g| (g.4, g.5, g.6, g.7)),
        display.x, display.y, display.width, display.height));
    vordergrund_bewahren(pid, vorn_vorher, None, wid);
    if !auf_noki {
        // Noki hat dieses Fenster geoeffnet (vorher war keins beim Nutzer):
        // nie beim Nutzer stehen lassen.
        let zu = vorschau::fernbedienung::fenster_schliessen(pid, wid);
        virtual_workspace::trace(&format!("[LEISTE] open app={name} own_window_closed={zu}"));
        return Err(format!("{name}: das Fenster ließ sich nicht auf Noki Schreibtisch holen."));
    }
    let rahmen = ist.map(|g| [g.4 as f64, g.5 as f64, g.6 as f64, g.7 as f64]).unwrap_or(rahmen);
    if !virtuellen_registry_eintrag_sichern(app, wid, pid, pfad, pfad, rahmen) {
        let _ = vorschau::fernbedienung::fenster_schliessen(pid, wid);
        return Err(format!("{name}: wiederhergestelltes Fenster hatte keinen gültigen Noki-Launch-Nachweis."));
    }
    if let Ok(mut r) = ARBEITSPLATZ_FENSTER.lock() {
        if let Some(e) = r.iter_mut().find(|e| e.fenster == wid && e.pid == pid) {
            e.restoration_kind = "app".into();
        }
    }
    arbeitsplatz_fenster_sichern(app);
    vorschau_neu_setzen(app);
    let live = workspace_registry_refresh(app);
    let _ = noki_fenster_vorn(pid, wid, &live);
    Ok(format!("{name} ist jetzt im Noki Schreibtisch"))
}


/// Chrome-Web-App (z. B. "Motion by Mosaic"): das Programmpaket ist nur ein
/// Starter (`app_mode_loader`); das Fenster gehoert dem Chrome-Prozess. Also:
/// Starter im Hintergrund oeffnen, das NEUE Chrome-Fenster finden, sofort auf
/// Noki Schreibtisch legen, pruefen, unter dem Starter registrieren.
#[cfg(target_os = "macos")]
fn chrome_webapp_oeffnen(app: &tauri::AppHandle, pfad: &str, name: &str, webapp_id: &str) -> Result<String, String> {
    const CHROME_ID: &str = "com.google.Chrome";
    let Some(display) = virtual_workspace::info() else { return Err("Noki Schreibtisch ist nicht bereit.".into()) };
    // Running PWA shell: `open -g` only re-activates the user's window and
    // "Datei > Neues Fenster" shows the new window on the user's desktop
    // first (and activating the shell switched the user's Space when its
    // windows lived on another Desktop). Never create a window there.
    if lesezeichen::pid_fuer_bundle(webapp_id).is_some() {
        return Err(format!("{name} ist schon auf deinem Schreibtisch offen – Noki öffnet kein zweites Fenster, das dort zuerst erscheinen würde."));
    }
    let vorn_vorher = blende::vorn_pid();
    let vorher: std::collections::HashMap<i64, bool> = cgs::alle_fenster().into_iter()
        .map(|f| (f.0, cgs::fenster_eingeordnet(f.0))).collect();
    let _ = std::process::Command::new("/usr/bin/open").args(["-g", "-a", pfad]).status();
    let t0 = std::time::Instant::now();
    let mut neu = None;
    while t0.elapsed() < Duration::from_secs(8) && neu.is_none() {
        // Gemessen: das Fenster einer Chrome-Web-App gehoert dem Starter-
        // Prozess der Web-App (App-Shim, "Motion by Mosaic"), nicht Chrome.
        let pids: Vec<i32> = [lesezeichen::pid_fuer_bundle(webapp_id), lesezeichen::pid_fuer_bundle(CHROME_ID)]
            .into_iter().flatten().filter(|p| *p > 0).collect();
        neu = cgs::alle_fenster().into_iter().find(|f| pids.contains(&f.1) && f.6 >= 300 && f.7 >= 200
            && vorher.get(&f.0).copied() != Some(true) && cgs::fenster_eingeordnet(f.0));
        if neu.is_none() { thread::sleep(Duration::from_millis(5)); }
    }
    let Some(f) = neu else {
        virtual_workspace::trace(&format!("[LEISTE] open app={name} route=chrome_webapp result=no_window"));
        return Err(format!("{name} hat kein neues Fenster geöffnet (vielleicht ist es schon auf deinem Schreibtisch offen)."));
    };
    let (wid, pid) = (f.0, f.1);
    let rahmen = [display.x as f64 + 80.0, display.y as f64 + 60.0,
                  (f.6 as f64).min(display.width as f64 - 120.0), (f.7 as f64).min(display.height as f64 - 90.0)];
    let mut bewegt = false;
    while !bewegt && t0.elapsed() < Duration::from_secs(10) {
        bewegt = vorschau::fernbedienung::ax_verschieben(pid, wid, rahmen[0], rahmen[1], rahmen[2], rahmen[3]);
        if !bewegt { thread::sleep(Duration::from_millis(5)); }
    }
    let mut ist = None;
    let mut auf_noki = false;
    for _ in 0..50 {
        ist = cgs::alle_fenster().into_iter().find(|g| g.0 == wid);
        auf_noki = ist.as_ref().is_some_and(|g| g.4 >= display.x as i64 && g.5 >= display.y as i64);
        if auf_noki { break; }
        thread::sleep(Duration::from_millis(20));
    }
    virtual_workspace::trace(&format!(
        "[LEISTE] open app={name} route=chrome_webapp wid={wid} moved_ms={} on_noki_display={auf_noki} space_trip=false",
        t0.elapsed().as_millis()));
    vordergrund_bewahren(pid, vorn_vorher, None, wid);
    if !auf_noki { return Err(format!("{name}: das Fenster ließ sich nicht auf Noki Schreibtisch holen.")); }
    let rahmen = ist.map(|g| [g.4 as f64, g.5 as f64, g.6 as f64, g.7 as f64]).unwrap_or(rahmen);
    if !virtuellen_registry_eintrag_sichern(app, wid, pid, pfad, pfad, rahmen) {
        let _ = vorschau::fernbedienung::fenster_schliessen(pid, wid);
        return Err(format!("{name}: Web-App-Fenster hatte keinen gültigen Noki-Launch-Nachweis."));
    }
    arbeitsplatz_fenster_sichern(app);
    vorschau_neu_setzen(app);
    let live = workspace_registry_refresh(app);
    let _ = noki_fenster_vorn(pid, wid, &live);
    Ok(format!("{name} ist jetzt im Noki Schreibtisch"))
}

/// How a window can be created so that it is visible ONLY on Noki's display
/// from its first frame (PART 1 "zero visible exposure"). Measured
/// 2026-09-25 with a 4 ms on-screen sampler:
///   * app not running, `open -g -j -F` (launched hidden, moved, unhidden):
///     0 exposed samples (Calculator);
///   * a running app's "New Window" menu item: always visible on the user
///     display first (ChatGPT 8 samples, TextEdit, Safari, Mail); hiding the
///     app first does not help - creating the window unhides it; making a
///     Noki window the app's main/key window does not steer placement;
///   * reopening a remembered window / cold-launching a Chrome PWA: born at
///     the app's remembered frame - on Noki's display if Noki closed it last
///     (Motion PWA born at 1600,1050, 0 exposed samples).
/// Everything else is refused with a concrete reason instead of flashing.
#[cfg(target_os = "macos")]
enum SichererWeg { KaltVerborgen, PwaKalt, Wiedereroeffnen(i32), ZweiteInstanz, MenueSchnell(i32) }

/// Chrome's own remembered placement of a PWA window (read-only).
#[cfg(target_os = "macos")]
fn chrome_pwa_platzierung_auf_noki(app_id: &str) -> bool {
    let Some(d) = virtual_workspace::info() else { return false };
    let basis = PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join("Library/Application Support/Google/Chrome");
    let Ok(profile) = fs::read_dir(&basis) else { return false };
    let schluessel = format!("_crx_{app_id}");
    profile.flatten().filter_map(|p| fs::read_to_string(p.path().join("Preferences")).ok())
        .filter_map(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .filter_map(|v| v["browser"]["app_window_placement"][&schluessel].as_object().cloned())
        .any(|o| {
            let (l, t) = (o.get("left").and_then(|v| v.as_i64()).unwrap_or(-1), o.get("top").and_then(|v| v.as_i64()).unwrap_or(-1));
            l >= d.x as i64 && t >= d.y as i64 && l < (d.x + d.width) as i64 && t < (d.y + d.height) as i64
        })
}

#[cfg(target_os = "macos")]
fn sicherer_weg(bundle: &str, name: &str) -> Result<SichererWeg, String> {
    let Some(d) = virtual_workspace::info() else { return Err("Noki Schreibtisch ist nicht bereit.".into()) };
    let auf_noki = |x: i64, y: i64| x >= d.x as i64 && y >= d.y as i64
        && x < (d.x + d.width) as i64 && y < (d.y + d.height) as i64;
    let laeuft = lesezeichen::pid_fuer_bundle(bundle);
    if let Some(app_id) = bundle.strip_prefix("com.google.Chrome.app.") {
        if laeuft.is_some() {
            return Err(format!("{name} ist schon auf deinem Schreibtisch offen. Ein zweites Fenster würde dort zuerst erscheinen – Noki öffnet deshalb keins.{}",
                if name.eq_ignore_ascii_case("youtube") { " „YouTube Web“ öffnet sich direkt im Noki Schreibtisch." } else { "" }));
        }
        if noki_lage_gemerkt(bundle) || chrome_pwa_platzierung_auf_noki(app_id) { return Ok(SichererWeg::PwaKalt); }
        return Err(format!("{name} würde an seiner letzten Stelle auf deinem Schreibtisch aufgehen – Noki öffnet es dort nicht.{}",
            if name.eq_ignore_ascii_case("youtube") { " „YouTube Web“ öffnet sich direkt im Noki Schreibtisch." } else { "" }));
    }
    let Some(pid) = laeuft else {
        // `open -g -j` keeps the first window hidden only if the app honors
        // hidden launch: AppKit apps do (Calculator, Notes: 0 exposed
        // samples); Spotify/CEF does not (6 samples at its saved frame).
        // Such an app opens invisibly only when Noki closed it last (its
        // saved frame is then on Noki's display).
        if bundle.starts_with("com.apple.") || noki_lage_gemerkt(bundle) {
            return Ok(SichererWeg::KaltVerborgen);
        }
        return Err(format!("{name} startet immer sichtbar an seiner letzten Stelle auf deinem Schreibtisch – Noki öffnet es deshalb nicht."));
    };
    // Calendar: no New Window/New Tab exists (menus audited), but a
    // SECOND hidden process instance is safe - Calendar is an EventKit
    // client (data lives in CalendarAgent), `open -n -g -j` starts it with
    // 0 exposed frames and the user's instance/window stays untouched
    // (measured 2026-09-25).
    if bundle == "com.apple.iCal" { return Ok(SichererWeg::ZweiteInstanz); }
    // Running app WITH a window on the user's current Desktop: its own
    // "New Window", moved onto Noki's display inside the AXWindowCreated
    // callback (measured ChatGPT: 1 exposed sample, ~10 ms, instead of the
    // 3-8 samples of a post-hoc move). The current-Desktop window is the
    // precondition: activating the app then never switches Spaces.
    let kein_space_risiko = app_auf_aktuellem_schreibtisch(pid) || !fenster_auf_anderen_spaces(pid);
    if nutzer_hat_fenster(pid) && kein_space_risiko {
        return Ok(SichererWeg::MenueSchnell(pid));
    }
    if nutzer_hat_fenster(pid) {
        return Err(if bundle == "com.apple.iCal" {
            "Kalender hat gerade dein Fenster offen, und macOS erlaubt kein zweites unabhängiges Kalenderfenster – Noki nimmt dir deins nicht weg.".into()
        } else {
            format!("{name} läuft mit deinem Fenster. Ein neues Fenster würde zuerst auf deinem Schreibtisch erscheinen – Noki öffnet es deshalb nicht. Beende {name} (⌘Q), dann öffnet Noki es unsichtbar direkt hier.")
        });
    }
    // Closed windows the app keeps and reopens at their old frame.
    let geschlossen: Vec<(i64, i64)> = cgs::alle_fenster().into_iter()
        .filter(|f| f.1 == pid && f.6 >= 300 && f.7 >= 200 && !f.3.trim().is_empty()
            && !cgs::fenster_eingeordnet(f.0) && !vorschau::fernbedienung::fenster_vorhanden(pid, f.0))
        .map(|f| (f.4, f.5)).collect();
    if !geschlossen.is_empty() && geschlossen.iter().all(|(x, y)| auf_noki(*x, *y)) {
        return Ok(SichererWeg::Wiedereroeffnen(pid));
    }
    // No user window at all: the app's own "New Window" (callback move);
    // an app without windows cannot pull the user to another Space.
    if kein_space_risiko && vorschau::fernbedienung::menue_punkt_vorhanden(pid,
        &["Ablage", "Datei", "File", "Shell", "Fenster", "Window"],
        &["Neues Fenster", "New Window", "Neues Fenster öffnen", "Neue Sitzung in neuem Fenster", "New Session in New Window",
          "Neues Finder-Fenster", "New Finder Window", "Neues Dokument", "New Document", "Neu", "New"]) {
        return Ok(SichererWeg::MenueSchnell(pid));
    }
    // Single-window app whose closed window sits on the user display
    // (Spotify): reopen with the creation observer armed.
    if kein_space_risiko && !geschlossen.is_empty() { return Ok(SichererWeg::Wiedereroeffnen(pid)); }
    Err(format!("{name} läuft bereits, und sein nächstes Fenster würde auf deinem Schreibtisch erscheinen. Beende {name} (⌘Q), dann öffnet Noki es unsichtbar direkt im Noki Schreibtisch."))
}

/// Picker line for an entry - the same decision as opening, so a normal
/// clickable entry is one that really opens.
#[cfg(target_os = "macos")]
fn katalog_zustand(pfad: &str, name: &str, offen: &[String]) -> String {
    if offen.iter().any(|p| p == pfad) { return "Im Noki Schreibtisch geöffnet".into(); }
    let Some(bundle) = bundle_id_von(pfad) else { return "Nicht lesbar".into() };
    if bundle == "com.google.Chrome" { return "In Noki öffnen".into(); }
    match sicherer_weg(&bundle, name) {
        _ if name.eq_ignore_ascii_case("youtube") && virtual_workspace::backend() == virtual_workspace::Backend::RealSpace
            => if noki_browser::youtube_fenster().is_some() { "Im Noki Schreibtisch geöffnet".into() } else { "Noki YouTube · eigenes Fenster".into() },
        Ok(_) if name.eq_ignore_ascii_case("youtube") => "Installierte YouTube App · in Noki öffnen".into(),
        Ok(_) => "In Noki öffnen".into(),
        Err(_) if lesezeichen::pid_fuer_bundle(&bundle).is_some() => "Eingeschränkt · läuft auf deinem Schreibtisch".into(),
        Err(_) => "Eingeschränkt · würde auf deinem Schreibtisch aufgehen".into(),
    }
}

/// Programm auf Noki Schreibtisch oeffnen - nur ueber nachweislich sichere Wege.
/// Native entries stay native. Browser versions exist only as explicitly
/// named `... Web` picker entries; there is no silent fallback.
///   * Chrome: Noki-eigenes Chrome-Fenster; gibt es keins, legt
///     "Datei > Neues Fenster" (AX, gemessen ohne Schreibtischwechsel) ein
///     NEUES an, das sofort auf Noki Schreibtisch kommt. Die Fenster des
///     Nutzers bleiben, wo sie sind.
///   * Programm laeuft nicht: im Hintergrund starten und sein Fenster holen.
///   * Programm laeuft: bekannter nicht aktivierender "neues Fenster"-Weg,
///     sonst fail closed. Fremde Fenster werden nie verschoben.
#[cfg(target_os = "macos")]
fn virtuelles_app_oeffnen(app: &tauri::AppHandle, pfad: &str) -> Result<String, String> {
    let bekannt = fokus_apps_laden().iter().any(|a| a["pfad"].as_str() == Some(pfad));
    if !bekannt || !pfad.ends_with(".app") {
        return Err("Unbekanntes Programm.".into());
    }
    let name = std::path::Path::new(pfad).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let bundle = bundle_id_von(pfad).ok_or("Programm nicht lesbar.")?;
    if virtual_workspace::backend() == virtual_workspace::Backend::RealSpace {
        // The picker is shared UI, but REAL_SPACE must not fall through to
        // virtual-display relocation, AX focus or synthetic window control.
        // On the actual Noki Space this is a normal macOS launch. From a
        // different Desktop the existing bounded, explicit transaction lets
        // the window be born there and returns to the user's origin.
        let ergebnis = arbeitsplatz_oeffnen_blockierend(
            app.clone(), pfad.to_owned(), name.clone(), Some(pfad.to_owned()),
        );
        if ergebnis["ok"].as_bool().unwrap_or(false) {
            vorschau_neu_setzen(app);
            return Ok(format!("{name} ist jetzt im Noki Schreibtisch"));
        }
        return Err(ergebnis["grund"].as_str()
            .unwrap_or("Das Programm konnte im Noki Schreibtisch nicht geöffnet werden.")
            .to_owned());
    }
    if virtual_workspace::info().is_none() { return Err("Noki Schreibtisch ist nicht bereit.".into()) };
    let _launch = NokiLaunchGuard::begin(pfad, &bundle);
    if bundle == "com.google.Chrome" {
        return noki_chrome_oeffnen(app, "Chrome", None);
    }
    // Stufe 1: Es gibt schon ein Noki-eigenes Fenster dieses Programms ->
    // wiederverwenden (nach vorn), kein zweites anlegen.
    {
        let live = workspace_registry_refresh(app);
        let eigenes = arbeitsplatz_fenster_liste().into_iter().find(|e| e.fenster != 0 && live.contains(&e.fenster)
            && (e.restoration_target == pfad || e.bundle_id == bundle));
        if let Some(e) = eigenes {
            if !cgs::fenster_eingeordnet(e.fenster) { vorschau::fernbedienung::fenster_zeigen(e.pid, e.fenster); }
            let _ = noki_fenster_vorn(e.pid, e.fenster, &live);
            vorschau::stapel();
            virtual_workspace::trace(&format!("[LEISTE] open app={name} route=reuse_noki_window wid={} space_trip=false", e.fenster));
            return Ok(format!("{name} ist schon im Noki Schreibtisch – nach vorn geholt"));
        }
    }
    let ist_browser = capability::adapter_for(&name).is_some_and(|a| capability::is_browser_key(a.key))
        || matches!(bundle.as_str(), "com.apple.Safari" | "org.mozilla.firefox" | "company.thebrowser.Browser" | "com.brave.Browser" | "com.microsoft.edgemac");
    if ist_browser {
        let live = workspace_registry_refresh(app);
        if let Some(e) = arbeitsplatz_fenster_liste().into_iter()
            .find(|e| e.fenster != 0 && live.contains(&e.fenster) && e.bundle_id == bundle) {
            if !cgs::fenster_eingeordnet(e.fenster) { vorschau::fernbedienung::fenster_zeigen(e.pid, e.fenster); }
            let _ = vorschau::fernbedienung::fenster_heben(e.pid, e.fenster);
            let ok = vorschau::fernbedienung::neuer_tab(e.pid, e.fenster);
            vorschau::stapel();
            virtual_workspace::trace(&format!("[LEISTE] open app={name} route=new_tab_in_noki_window wid={} ok={ok}", e.fenster));
            return if ok { Ok(format!("Neuer Tab in {name}")) } else { Err(format!("{name}: kein neuer Tab möglich.")) };
        }
    }
    let weg = sicherer_weg(&bundle, &name);
    virtual_workspace::trace(&format!("[LEISTE] open app={name} safe_route={} exposure_policy=zero",
        match &weg { Ok(SichererWeg::KaltVerborgen) => "cold_hidden".to_string(), Ok(SichererWeg::ZweiteInstanz) => "second_instance_hidden".into(), Ok(SichererWeg::MenueSchnell(_)) => "menu_new_window_callback_move".into(), Ok(SichererWeg::PwaKalt) => "pwa_remembered_on_noki".into(),
                     Ok(SichererWeg::Wiedereroeffnen(_)) => "reopen_remembered_on_noki".into(), Err(e) => format!("refused:{e}") }));
    match weg? {
        SichererWeg::PwaKalt => chrome_webapp_oeffnen(app, pfad, &name, &bundle),
        SichererWeg::Wiedereroeffnen(pid) => {
            if let Some(d) = virtual_workspace::info() {
                kind_fenster::erzeugung(pid, [d.x as f64 + 60.0, d.y as f64 + 40.0, d.width as f64 * 0.6, d.height as f64 * 0.7]);
            }
            let r = programm_fenster_wieder_oeffnen(app, pid, pfad, &name);
            kind_fenster::erzeugung_ende(pid);
            r
        }
        SichererWeg::ZweiteInstanz => zweite_instanz_starten(app, pfad, &name, &bundle),
        SichererWeg::MenueSchnell(pid) => menue_neues_fenster(app, pid, pfad, &name),
        SichererWeg::KaltVerborgen => {
            let (pid, wid) = virtuelles_programm_starten(app, pfad, &name, &bundle, None)?;
            vorschau_neu_setzen(app);
            let live = workspace_registry_refresh(app);
            let _ = noki_fenster_vorn(pid, wid, &live);
            Ok(format!("{name} ist jetzt im Noki Schreibtisch"))
        }
    }
}

/// LINKS-VON-1 + Pfeil im virtuellen Backend.
///
/// Frueher waehlte das Kuerzel, WELCHER Mission-Control-Schreibtisch Noki
/// gehoert. Im virtuellen Backend gibt es genau EINEN Noki Schreibtisch; ihn
/// als "Schreibtisch 3" auszugeben waere erfunden. Die Entsprechung von
/// "naechster/vorheriger" ist deshalb: das naechste/vorherige Fenster AUF
/// Noki Schreibtisch kommt nach vorn (feste Reihenfolge = Registrierung).
///   * rechts/runter = naechstes, links/hoch = vorheriges, mit Umlauf;
///   * AXRaise nur innerhalb des Programms - kein Space, kein Zeiger, das
///     vordere Programm des Nutzers bleibt vorn;
///   * die Miniatur erscheint und sagt, welches Fenster jetzt vorn ist;
///   * leerer Schreibtisch: die Miniatur erscheint mit ihrem Leerzustand.
#[cfg(target_os = "macos")]
fn virtuelles_fenster_waehlen(app: &tauri::AppHandle, vor: bool) {
    if !virtual_workspace::ready() {
        virtual_workspace::trace(&format!(
            "[ARBEITSPLATZ] Wahl vor={vor} backend=virtual state={:?} action=none reason=not_ready",
            virtual_workspace::workspace_readiness()
        ));
        return;
    }
    let _ = app.emit("noki://workspace_shortcut", serde_json::json!({ "show": true }));
    let live = workspace_registry_refresh(app);
    // Feste Reihenfolge: die Registrierung (nicht die wechselnde Stapelung),
    // sonst pendelte "weiter" zwischen den beiden vordersten Fenstern.
    let liste: Vec<(i64, i32, String)> = arbeitsplatz_fenster_liste().into_iter()
        .filter(|e| e.fenster != 0 && live.contains(&e.fenster))
        .map(|e| (e.fenster, e.pid, e.titel))
        .collect();
    if liste.is_empty() {
        vorschau::hinweis("Noki Schreibtisch ist leer");
        virtual_workspace::trace(&format!("[ARBEITSPLATZ] Wahl vor={vor} windows=0 action=show_empty"));
        return;
    }
    let aktiv = noki_aktiv(&live);
    let vorn = liste.iter().position(|(id, ..)| *id == aktiv).unwrap_or(0);
    let n = liste.len();
    let ziel = if vor { (vorn + 1) % n } else { (vorn + n - 1) % n };
    let (wid, pid, titel) = &liste[ziel];
    // Ausdruecklich gewaehlt: ein minimiertes Noki-Fenster kommt zurueck
    // (auf Noki Schreibtisch - die virtuelle Anzeige, kein Space-Wechsel).
    let war_minimiert = !cgs::fenster_eingeordnet(*wid)
        && vorschau::fernbedienung::fenster_zeigen(*pid, *wid);
    let gehoben = noki_fenster_vorn(*pid, *wid, &live);
    let name: String = titel.chars().take(48).collect();
    let eintrag = arbeitsplatz_fenster_liste().into_iter().find(|e| e.fenster == *wid);
    vorschau::hinweis_fenster(
        eintrag.as_ref().map(|e| e.application_path.as_str()).unwrap_or(""),
        eintrag.as_ref().map(|e| e.app.as_str()).unwrap_or(""),
        &format!("{}/{} · {}", ziel + 1, n, if name.is_empty() { "Fenster".to_string() } else { name }));
    virtual_workspace::trace(&format!(
        "[ARBEITSPLATZ] Wahl vor={vor} backend=virtual windows={n} from={} to={} wid={wid} raised={gehoben} unminimized={war_minimiert} space_navigation=false",
        vorn + 1, ziel + 1
    ));
    leiste_aktualisieren(app);
}

/// Reiht eine Wahl (LINKS-VON-1 + Pfeil) fuer EINEN eigenen Arbeiter ein.
///
/// Der Tastenrueckruf darf nichts Teures tun und nichts nach aussen werfen.
/// Hier wird nur ein Richtungsbit abgelegt; der Arbeiter liest die
/// Schreibtische, schreibt die Wahl und meldet sie der Oberflaeche - und
/// faengt jeden Fehler selbst. Hoechstens vier Druecke stehen an; was
/// darueber kommt, waere ohnehin nur Tastenprellen.
pub(crate) fn arbeitsplatz_wahl_planen(app: &tauri::AppHandle, vor: bool) {
    static KANAL: std::sync::OnceLock<std::sync::mpsc::Sender<bool>> =
        std::sync::OnceLock::new();
    let tx = KANAL.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<bool>();
        let h = app.clone();
        let _ = thread::Builder::new()
            .name("noki-wahl".into())
            .spawn(move || {
                while let Ok(vor) = rx.recv() {
                    let t0 = std::time::Instant::now();
                    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        arbeitsplatz_waehlen(&h, vor)
                    }));
                    if r.is_err() {
                        eprintln!("[ARBEITSPLATZ] Wahl: Fehler abgefangen - Noki laeuft weiter");
                    }
                    eprintln!("[ARBEITSPLATZ] Wahl vor={vor} in {}ms", t0.elapsed().as_millis());
                }
            });
        tx
    });
    let _ = tx.send(vor);
}

#[cfg(target_os = "macos")]
fn zurueck_zur_herkunft(herkunft: u64) {
    if herkunft == 0 {
        return;
    }
    if cgs::aktiver_space().map(|(a, _)| a) == Some(herkunft) {
        return;
    }
    if let Err(e) = cgs::sichtbar_zum_space(herkunft) {
        eprintln!("[ARBEITSPLATZ] Rueckweg zu Schreibtisch {herkunft} misslang: {e}");
    }
}

#[cfg(not(target_os = "macos"))]
fn zurueck_zur_herkunft(_herkunft: u64) {}

fn arbeitsplatz_fenster_merken(app: &tauri::AppHandle, arbeitsplatz: u64, ids: &[i64]) {
    if let Ok(mut task) = TASK_FENSTER.lock() {
        for id in ids {
            if !task.contains(id) { task.push(*id); }
        }
    }
    // Die Herkunft wird JETZT festgehalten, solange sie noch bekannt ist -
    // spaeter liesse sie sich nicht mehr rekonstruieren, ohne aus blosser
    // Anwesenheit Besitz zu machen.
    #[cfg(target_os = "macos")]
    {
        let echte = cgs::fenster_auf_space(arbeitsplatz);
        if let Ok(mut desk) = ARBEITSPLATZ_FENSTER.lock() {
            for id in ids {
                if desk.iter().any(|e| e.fenster == *id) {
                    continue;
                }
                let Some((_, pid, owner, ..)) = echte.iter().find(|(w, ..)| w == id) else {
                    continue; // ohne nachweisbare Herkunft wird nichts gemerkt
                };
                let (name, bundle_id, application_path) = lesezeichen::app_fuer_pid(*pid)
                    .unwrap_or_else(|| (owner.clone(), String::new(), String::new()));
                let detail = cgs::alle_fenster().into_iter().find(|(wid, ..)| wid == id);
                let (titel, frame, z) = detail.map(|(_,_,_,t,x,y,w,h,z)|
                    (t, Some([x as f64,y as f64,w as f64,h as f64]), z))
                    .unwrap_or_default();
                desk.push(EigenesFenster {
                    fenster: *id, pid: *pid, app: if name.is_empty() { owner.clone() } else { name },
                    bundle_id, titel, created_by_noki: true,
                    explicitly_assigned_to_noki: false,
                    last_virtual_frame: frame, desired_frame: frame, last_z_order: z,
                    maximized_from: None,
                    restoration_kind: String::new(), restoration_target: String::new(),
                    application_path, visible: true, lifecycle_state: "live".into(),
                });
            }
        }
        arbeitsplatz_fenster_sichern(app);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, arbeitsplatz);
}

/// Fuehrt eine Oeffnen-Aktion AUF Nokis Arbeitsplatz aus.
///
/// Gemessen auf macOS 26.6: ein Programm kann die Space-Zugehoerigkeit nur
/// SEINER EIGENEN Fenster aendern. `CGSAddWindowsToSpaces` und
/// `CGSMoveWindowsToManagedSpace` lassen ein fremdes Fenster nachweislich
/// stehen, wo es ist. Ein Fenster nachtraeglich auf Nokis Schreibtisch zu
/// ziehen ist also nicht moeglich.
///
/// Was nachweislich geht: das Fenster dort ENTSTEHEN lassen. macOS legt ein
/// neues Fenster auf den Schreibtisch, der beim Start aktiv ist. Deshalb:
/// Herkunft merken -> zu Noki wechseln -> oeffnen -> zurueckwechseln. Der
/// kurze sichtbare Wechsel ist der Preis dafuer und wird nicht versteckt.
#[tauri::command]
async fn noki_arbeitsplatz_oeffnen(
    app: tauri::AppHandle,
    ziel: String,
    besitzer: String,
    programm: Option<String>,
) -> serde_json::Value {
    // Das Warten gehoert in einen Arbeitsfaden, nie in den Hauptthread.
    match tauri::async_runtime::spawn_blocking(move || {
        arbeitsplatz_oeffnen_blockierend(app, ziel, besitzer, programm)
    })
    .await
    {
        Ok(v) => v,
        Err(e) => serde_json::json!({ "ok": false, "grund": format!("{e}") }),
    }
}

fn arbeitsplatz_oeffnen_blockierend(
    app: tauri::AppHandle,
    ziel: String,
    besitzer: String,
    programm: Option<String>,
) -> serde_json::Value {
    #[cfg(target_os = "macos")]
    {
        let ziel = ziel.trim();
        if ziel.is_empty() || ziel.chars().count() > 2000 || ziel.chars().any(char::is_control) {
            return serde_json::json!({ "ok": false, "grund": "Ungueltiges Ziel." });
        }
        // Repository-owned deterministic remote-input fixture.  This is a
        // debug-only exception, exact-path checked; production keeps the
        // normal web/app allow-list unchanged.
        let debug_fern_ziel = cfg!(debug_assertions) && ziel.strip_prefix("file://")
            .is_some_and(|p| std::fs::canonicalize(p).ok()
                == std::fs::canonicalize("/Users/yilonglin/NOKI/desktop/tests/fern-ziel.html").ok());
        // DIESELBEN Schranken wie die vorhandenen Oeffner. Der Arbeitsplatz
        // entscheidet WO etwas geschieht - er darf nicht zum Schlupfloch
        // werden, durch das WAS an der Zielliste vorbeikommt.
        let erlaubt = if let Some(rest) = ziel.strip_prefix("https://") {
            let host = rest
                .split('/')
                .next()
                .unwrap_or("")
                .split('@')
                .next_back()
                .unwrap_or("")
                .split(':')
                .next()
                .unwrap_or("")
                .to_lowercase();
            capability::is_known_web_host(&host)
        } else if ziel.starts_with('/') {
            std::path::Path::new(ziel).exists()
        } else if debug_fern_ziel {
            true
        } else {
            capability::is_known_app_uri(ziel)
        };
        if !erlaubt {
            return serde_json::json!({ "ok": false, "grund": "Ziel steht nicht in der Noki-Zielliste." });
        }
        if virtual_workspace::backend() == virtual_workspace::Backend::LegacyVirtualDisplay {
            if !virtual_workspace::backend_ready() {
                virtual_workspace::trace("[WORKSPACE] open=consumed reason=backend_not_ready");
                return serde_json::json!({
                    "ok": false, "grund": "Noki Schreibtisch wird noch initialisiert.",
                    "backend": "LEGACY_VIRTUAL_DISPLAY", "state": "INITIALIZING"
                });
            }
            let programm = programm.or_else(|| {
                fokus_apps_laden().into_iter().find_map(|a| {
                    (a.get("name")?.as_str()? == "Google Chrome")
                        .then(|| a.get("pfad")?.as_str().map(str::to_owned)).flatten()
                })
            });
            let Some(programm) = programm else {
                return serde_json::json!({"ok": false, "grund": "Kein sicherer Browser für Noki Schreibtisch gefunden."});
            };
            let bundle = bundle_id_von(&programm).unwrap_or_default();
            let _launch = NokiLaunchGuard::begin(&programm, &bundle);
            let Some(display) = virtual_workspace::info() else {
                return serde_json::json!({"ok": false, "grund": "Noki Schreibtisch hat noch keine Anzeige."});
            };
            let n = arbeitsplatz_fenster_liste().len() as i32;
            let offset = (n % 4) as f64 * (display.width as f64 / 22.0);
            let frame = [
                display.x as f64 + display.width as f64 / 14.0 + offset,
                display.y as f64 + display.height as f64 / 12.0 + offset,
                display.width as f64 * 0.62,
                display.height as f64 * 0.66,
            ];
            return match virtuelles_browserfenster_erstellen(ziel, &programm, frame) {
                Ok((wid, pid)) => {
                    if !virtuellen_registry_eintrag_sichern(&app, wid, pid, ziel, &programm, frame) {
                        let _ = vorschau::fernbedienung::fenster_schliessen(pid, wid);
                        return serde_json::json!({
                            "ok": false, "backend": "LEGACY_VIRTUAL_DISPLAY",
                            "space_trip": false, "grund": "Fenster hatte keinen gültigen Noki-Launch-Nachweis."
                        });
                    }
                    let ids = vorschau_faehig(workspace_registry_refresh(&app));
                    let preview = vorschau::setzen(&app, ids.clone(), 520, 30,
                        "virtual-display".into(), String::new());
                    let deadline = std::time::Instant::now() + Duration::from_secs(3);
                    while vorschau::retarget_aktiv() && std::time::Instant::now() < deadline {
                        thread::sleep(Duration::from_millis(20));
                    }
                    let ready = !vorschau::retarget_aktiv()
                        && vorschau::aufgenommene().len() == ids.len()
                        && preview["ok"].as_bool().unwrap_or(false);
                    virtual_workspace::finish_workspace(ready, ids.len(), usize::from(!ready));
                    serde_json::json!({
                        "ok": ready, "backend": "LEGACY_VIRTUAL_DISPLAY",
                        "neue_fenster": [wid], "space_trip": false,
                        "grund": if ready { "" } else { "Fenster entstand, aber die Vorschau wurde nicht bereit." }
                    })
                }
                Err(grund) => serde_json::json!({
                    "ok": false, "backend": "LEGACY_VIRTUAL_DISPLAY",
                    "space_trip": false, "grund": grund
                }),
            };
        }
        let r = match arbeitsplatz_sichern(&app) {
            Ok(r) => r,
            Err(e) => return serde_json::json!({ "ok": false, "grund": e }),
        };
        let herkunft = cgs::aktiver_space().map(|(a, _)| a).unwrap_or(0);
        if herkunft == 0 {
            return serde_json::json!({
                "ok": false,
                "backend": "REAL_SPACE",
                "grund": "macOS meldet gerade keinen stabilen aktiven Space; Öffnen wurde nicht gestartet.",
                "arbeitsplatz": r.id,
                "herkunft": 0,
                "sichtbar_gewechselt": false
            });
        }
        // Ab hier schaltet NOKI den Bildschirm, nicht der Nutzer. Die Klammer
        // haelt bis zum Ende dieser Funktion - also ueber Hinweg, Oeffnen und
        // Rueckweg. Ohne sie flog die Figur bei jeder Arbeitsplatz-Aktion
        // zweimal quer ueber die Schreibtische.
        let _eigener = EigenerWechsel::neu();
        // Die Beschriftung eines Tools ist fuer Menschen, nicht fuer die
        // WindowServer-Identitaet. Bei einem konkreten .app-Pfad ist dessen
        // Bundle-Name die belastbare Owner-Bezeichnung ("Google Chrome",
        // nicht etwa ein gekuerztes "Chrome"). Das verhindert sowohl
        // verpasste eigene Fenster als auch eine unsaubere Zuordnung nach
        // einer UI-Textaenderung.
        let besitzer = programm.as_deref()
            .and_then(|p| std::path::Path::new(p).file_stem())
            .and_then(|n| n.to_str())
            .filter(|n| !n.is_empty())
            .unwrap_or(besitzer.as_str())
            .to_owned();
        // Bei einem Dateipfad ohne genannten Besitzer entscheidet macOS, wer
        // oeffnet. Ohne diesen Namen liesse sich hinterher nicht sagen, WELCHE
        // Fenster neu sind - und ein von Noki erzeugtes Fenster bliebe
        // unbeansprucht statt in der Miniatur zu erscheinen.
        let besitzer = if besitzer.is_empty() && ziel.starts_with('/') {
            lesezeichen::apps_fuer_datei(ziel)
                .into_iter()
                .find(|(_, _, standard)| *standard)
                .map(|(name, _, _)| name)
                .unwrap_or_default()
        } else {
            besitzer
        };
        let jetzige = cgs::fenster_von(&besitzer);
        let vorher: Vec<i64> = jetzige.iter().map(|(w, _)| *w).collect();
        // ENTSCHEIDEN, BEVOR DER BILDSCHIRM UMSCHALTET.
        //
        // Gemessen am laufenden Programm: laeuft das Ziel schon, dann reicht
        // `open -g` die Adresse an sein VORHANDENES Fenster weiter und
        // erzeugt kein neues. Vorher wurde trotzdem der ganze Weg gegangen -
        // Blende hoch, Schreibtisch wechseln, NEUN SEKUNDEN auf ein Fenster
        // warten, das nie kommen konnte, Schreibtisch zurueck. Mit Spotify
        // gemessen: 9,5 s Standbild fuer eine Aktion, die gar nicht gelingen
        // konnte. Genau das war das gemeldete Einfrieren.
        //
        // Der Zustand ist VORHER bekannt, also wird er vorher gelesen:
        //   * Fenster steht schon auf Nokis Schreibtisch -> die Adresse geht
        //     dorthin. Kein Wechsel, keine Blende, kein Warten - und
        //     "Suche X auf Spotify" wirkt endlich dort, wo es soll.
        //   * Fenster steht woanders -> das kann nicht gelingen. Sofort und
        //     ehrlich sagen, ohne den Bildschirm anzufassen.
        //   * Kein Fenster -> frischer Start, der Weg unten gilt weiter.
        //
        // Der Browserzweig ist ausgenommen: er startet das Programm selbst
        // mit `--new-window` und bekommt sein eigenes Fenster.
        let browser_neues_fenster = programm.as_deref().is_some_and(|p| {
            p.ends_with(".app")
                && std::path::Path::new(p).is_dir()
                && (ziel.starts_with("https://") || debug_fern_ziel)
                && std::path::Path::new(p)
                    .file_stem()
                    .and_then(|n| n.to_str())
                    .and_then(capability::adapter_for)
                    .is_some_and(|a| capability::is_browser_key(a.key))
        });
        if !browser_neues_fenster && !jetzige.is_empty() {
            let hier: Vec<i64> = jetzige.iter().filter(|(_, sp)| *sp == r.id).map(|(w, _)| *w).collect();
            if !hier.is_empty() {
                // Ein blosses "oeffne <Programm>" ist hier schon erfuellt: das
                // Fenster STEHT auf Nokis Schreibtisch. Dann wird gar nichts
                // aufgerufen - `open` wuerde das Programm nur anstupsen, und
                // macOS holt den Nutzer bei dieser Gelegenheit auf den
                // Schreibtisch mit dem Fenster (gemessen: der Nutzer landete
                // ohne jeden Wechsel von Noki auf Schreibtisch 3).
                let blosses_programm = ziel.starts_with('/') && ziel.ends_with(".app");
                if blosses_programm {
                    ortsbindung_setzen();
                    return serde_json::json!({
                        "ok": true, "grund": "",
                        "arbeitsplatz": r.id, "herkunft": herkunft,
                        "sichtbar_gewechselt": false, "neue_fenster": Vec::<i64>::new(),
                        "fremd_platziert": Vec::<serde_json::Value>::new(),
                        "weg": "steht schon auf Nokis Schreibtisch",
                        "aufgabe_besitzt": TASK_FENSTER.lock().map(|g| g.len()).unwrap_or(0)
                    });
                }
                // Eine ADRESSE (Suche, Deeplink) muss dagegen wirklich
                // hinueber. `-g` haelt das Programm aus dem Vordergrund,
                // aber macOS darf trotzdem auf den Schreibtisch seines
                // Fensters springen. Also: Blende hoch, absetzen, und falls
                // der Bildschirm gewandert ist, zurueck - der Nutzer merkt
                // davon nichts. Ohne Sprung faellt die Blende sofort wieder.
                let blende_an = blende::auf(&app);
                let _blende = blende::Halten(app.clone(), blende_an);
                let gestartet = std::process::Command::new("/usr/bin/open")
                    .arg("-g").arg(ziel).status().map(|s| s.success()).unwrap_or(false);
                // macOS braucht einen Moment, bis es ggf. umschaltet.
                let mut gesprungen = false;
                for _ in 0..24 {
                    if cgs::aktiver_space().map(|(a, _)| a) != Some(herkunft) {
                        gesprungen = true;
                        break;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
                if gesprungen {
                    zurueck_zur_herkunft(herkunft);
                }
                if gestartet {
                    ortsbindung_setzen();
                }
                return serde_json::json!({
                    "ok": gestartet,
                    "grund": if gestartet { String::new() }
                             else { format!("{besitzer} hat die Anfrage nicht angenommen.") },
                    "arbeitsplatz": r.id, "herkunft": herkunft,
                    "sichtbar_gewechselt": false, "zurueckgeholt": gesprungen,
                    "neue_fenster": Vec::<i64>::new(),
                    "fremd_platziert": Vec::<serde_json::Value>::new(),
                    "weg": "vorhandenes Fenster auf Nokis Schreibtisch",
                    "aufgabe_besitzt": TASK_FENSTER.lock().map(|g| g.len()).unwrap_or(0)
                });
            }
            return serde_json::json!({
                "ok": false,
                "grund": format!("{besitzer} läuft bereits bei dir und kann Noki kein eigenes Fenster geben – deshalb öffnet Noki es nicht noch einmal."),
                "arbeitsplatz": r.id, "herkunft": herkunft,
                "sichtbar_gewechselt": false, "neue_fenster": Vec::<i64>::new(),
                "fremd_platziert": Vec::<serde_json::Value>::new(),
                "weg": "kein Wechsel - es haette nicht gelingen koennen",
                "aufgabe_besitzt": TASK_FENSTER.lock().map(|g| g.len()).unwrap_or(0)
            });
        }
        // GRUND: BOUNDED_WORKSPACE_PREP. Auch hier muss der Wechsel ECHT
        // sein. Mit der blossen Buchfuehrung entstanden Fenster, die macOS
        // zwar Schreibtisch 3 zuordnete, die dort aber nie erschienen:
        // gemessen `onscreen=false` auf JEDEM Schreibtisch. Das war das
        // "Fenster, das es gar nicht gibt" - ein Gespenst, das die Miniatur
        // zu Recht nicht zeichnen konnte. Der kurze sichtbare Wechsel ist
        // der Preis dafuer, dass das Fenster wirklich dort steht.
        // AUSFUEHRUNGSORT != NAVIGATION DES NUTZERS.
        //
        // Der Wechsel bleibt echt - er muss es sein, sonst entstehen
        // Gespenster (siehe Kommentar oben). Aber er wird nicht mehr GEZEIGT.
        // Vor dem ersten Schritt haengt ein Standbild des Ausgangs-
        // schreibtischs vor die Szene; erst nach dem Rueckweg faellt es. Aus
        // Sicht des Nutzers steht sein Schreibtisch die ganze Zeit still,
        // waehrend Noki und die Miniatur darueber weiterlaufen.
        //
        // `Halten` loest die Blende auf JEDEM Rueckweg dieser Funktion, auch
        // beim fruehen `return` des Browser-Zweigs.
        let blende_an = herkunft != r.id && blende::auf(&app);
        let _blende = blende::Halten(app.clone(), blende_an);
        let gewechselt = herkunft != r.id && cgs::sichtbar_zum_space(r.id).is_ok();
        if gewechselt {
            thread::sleep(Duration::from_millis(250));
        }
        // Ein laufendes Programm oeffnet eine Adresse in seinem VORHANDENEN
        // Fenster - und das steht oft auf dem Schreibtisch des Nutzers.
        // Gemessen: `open -g <url>` und auch `open -a ... --args --new-window`
        // erzeugten kein neues Fenster, der Treffer landete beim Nutzer.
        // Der Programmaufruf mit `--new-window` erzeugte dagegen ein frisches
        // Fenster auf dem gerade aktiven Schreibtisch - also auf Nokis.
        let chrom = programm.as_deref().filter(|p| {
            p.ends_with(".app") && std::path::Path::new(p).is_dir()
                && (ziel.starts_with("https://") || debug_fern_ziel)
        });
        if let Some(bundle) = chrom {
            let name = std::path::Path::new(bundle)
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_owned();
            let exe = std::path::Path::new(bundle)
                .join("Contents/MacOS")
                .join(&name);
            if exe.is_file() && capability::adapter_for(&name).is_some_and(|a| capability::is_browser_key(a.key)) {
                // Nokis Schreibtisch soll ein Schreibtisch BLEIBEN: ein
                // formatfuellendes Fenster verdeckte den Hintergrund und
                // liesse die Miniatur wieder wie ein einzelnes Programm
                // aussehen. Also gross, aber nicht bildschirmfuellend - und
                // jedes weitere Fenster versetzt daneben.
                let (bx, by, bw, bh) = anzeige_flaeche();
                let n = TASK_FENSTER.lock().map(|g| g.len()).unwrap_or(0) as i32;
                let fw = (bw as f64 * 0.62).round() as i32;
                let fh = (bh as f64 * 0.66).round() as i32;
                let versatz = (n % 4) * (bw / 22);
                let fx = bx + bw / 14 + versatz;
                let fy = by + bh / 12 + versatz;
                let gestartet = std::process::Command::new(&exe)
                    .arg("--new-window")
                    .arg(format!("--window-position={fx},{fy}"))
                    .arg(format!("--window-size={fw},{fh}"))
                    .arg(ziel)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .is_ok();
                // Erst zurueck, wenn das Fenster wirklich HIER steht.
                let (neue, daneben) =
                    neue_fenster_abwarten(&besitzer, &vorher, r.id, Duration::from_millis(9000));
                arbeitsplatz_fenster_merken(&app, r.id, &neue);
                if !daneben.is_empty() {
                    eprintln!(
                        "[ARBEITSPLATZ] {besitzer}: {} Fenster ausserhalb von Nokis Schreibtisch entstanden: {daneben:?}",
                        daneben.len()
                    );
                }
                // Der Nutzer bekommt seinen Schreibtisch zurueck. Massgeblich
                // ist, wo er JETZT steht - nicht, was der Hinweg gemeldet hat.
                // Ein Hinweg, der als misslungen galt und trotzdem wirkte,
                // liess den Nutzer sonst auf Nokis Schreibtisch stehen.
                zurueck_zur_herkunft(herkunft);
                // Abschnitt 19: entstand das Fenster NICHT auf Nokis
                // Schreibtisch, ist die Aktion nicht gelungen - auch wenn das
                // Programm startete. Gemessen: steht das Programm im Vollbild,
                // legt macOS sein neues Fenster in einen NEUEN Vollbild-Space
                // statt auf den gerade aktiven Schreibtisch. Ein stilles "ok"
                // hiesse hier, dem Nutzer ein Fenster unterzuschieben, von dem
                // Noki nichts mehr weiss. Lieber eine ehrliche Grenze.
                let verfehlt = gestartet && neue.is_empty();
                if gestartet && !verfehlt {
                    ortsbindung_setzen();
                }
                return serde_json::json!({
                    "ok": gestartet && !verfehlt,
                    "grund": if verfehlt {
                        "Das Programm hat sein Fenster ausserhalb von Nokis Schreibtisch geoeffnet (Vollbild?). Nokis Schreibtisch bleibt sauber, der Schreibtisch des Nutzers auch."
                    } else { "" },
                    "arbeitsplatz": r.id, "herkunft": herkunft,
                    "sichtbar_gewechselt": gewechselt, "neue_fenster": neue,
                    "fremd_platziert": daneben.iter().map(|(w, s)| serde_json::json!({"fenster": w, "space": s})).collect::<Vec<_>>(),
                    "weg": "neues Fenster",
                    "aufgabe_besitzt": TASK_FENSTER.lock().map(|g| g.len()).unwrap_or(0)
                });
            }
        }
        // `-g` = NICHT in den Vordergrund holen. Gemessen: ein normales
        // `open -a` aktiviert das Programm, und macOS schaltet den Nutzer
        // dann auf den Space, auf dem dieses Programm Fenster hat
        // ("beim Wechsel zu einem Programm zu einem Space mit offenen
        // Fenstern wechseln", standardmaessig an). Im Versuch landete der
        // Nutzer dabei sogar auf dem VOLLBILD-Space von Chrome - genau die
        // gemeldete Uebernahme des Bildschirms. Mit `-g` blieb er auf
        // seinem eigenen Schreibtisch, und das neue Fenster entstand
        // trotzdem auf Nokis Arbeitsplatz.
        let gestartet = std::process::Command::new("/usr/bin/open")
            .arg("-g")
            .arg(ziel)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        // Neue Fenster dieses Programms gehoeren jetzt der Aufgabe. NUR sie -
        // was der Nutzer vorher offen hatte, bleibt seins, und was trotz
        // allem woanders entstanden ist, wird nicht beansprucht.
        let (neue, daneben) =
            neue_fenster_abwarten(&besitzer, &vorher, r.id, Duration::from_millis(9000));
        arbeitsplatz_fenster_merken(&app, r.id, &neue);
        if !daneben.is_empty() {
            eprintln!(
                "[ARBEITSPLATZ] {besitzer}: {} Fenster ausserhalb von Nokis Schreibtisch entstanden: {daneben:?}",
                daneben.len()
            );
        }
        // NUR ein Fenster auf Nokis Schreibtisch zaehlt als Erfolg.
        //
        // Gemessen: laeuft das Programm schon (Spotify mit einem Fenster auf
        // dem Schreibtisch des Nutzers), dann oeffnet `open -g` kein zweites
        // Fenster - es reicht die Adresse an das VORHANDENE weiter. Danach
        // stand auf Nokis Schreibtisch nichts, beim Nutzer aber sehr wohl
        // etwas, und Noki meldete trotzdem Erfolg. Genau so entstand der
        // gemeldete "Spotify oeffnet beim Nutzer"-Fehler.
        //
        // Ein leeres Ergebnis ist deshalb ein Misserfolg - auch dann, wenn
        // sonst nirgends ein Fenster auftauchte. Lieber eine ehrliche
        // Grenze als ein Fenster, von dem Noki nichts weiss.
        let verfehlt = gestartet && neue.is_empty();
        if gestartet && !verfehlt {
            ortsbindung_setzen();
        }
        zurueck_zur_herkunft(herkunft);
        serde_json::json!({
            "ok": gestartet && !verfehlt,
            "grund": if !verfehlt { String::new() }
                     else if !daneben.is_empty() {
                         format!("{besitzer} hat sein Fenster ausserhalb von Nokis Schreibtisch geoeffnet (Vollbild?).")
                     } else {
                         format!("{besitzer} läuft bereits bei dir und kann Noki kein eigenes Fenster geben – deshalb öffnet Noki es nicht noch einmal.")
                     },
            "arbeitsplatz": r.id,
            "herkunft": herkunft,
            "sichtbar_gewechselt": gewechselt,
            "neue_fenster": neue,
            "fremd_platziert": daneben.iter().map(|(w, s)| serde_json::json!({"fenster": w, "space": s})).collect::<Vec<_>>(),
            "aufgabe_besitzt": TASK_FENSTER.lock().map(|g| g.len()).unwrap_or(0)
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, ziel, besitzer);
        serde_json::json!({ "ok": false })
    }
}

/// Eine allgemeine Browser-Suche darf nicht mehr ueber den direkten
/// Standardbrowser-Oeffner auf dem Nutzer-Space landen. Auf diesem Mac ist
/// Chrome als verifizierter Browseradapter vorhanden; dessen neuer Prozess-
/// Fensterweg erzeugt ein eigenes Fenster auf Nokis reserviertem Space.
#[tauri::command]
async fn noki_arbeitsplatz_suchen(app: tauri::AppHandle, query: String) -> serde_json::Value {
    match tauri::async_runtime::spawn_blocking(move || arbeitsplatz_suchen_blockierend(app, query))
        .await
    {
        Ok(v) => v,
        Err(e) => serde_json::json!({ "ok": false, "grund": format!("{e}") }),
    }
}

fn arbeitsplatz_suchen_blockierend(app: tauri::AppHandle, query: String) -> serde_json::Value {
    let url = match browser_such_url(&query) {
        Ok(url) => url,
        Err(grund) => return serde_json::json!({ "ok": false, "grund": grund }),
    };
    let chrome = fokus_apps_laden().into_iter().find_map(|a| {
        let name = a.get("name")?.as_str()?;
        if name.eq_ignore_ascii_case("Google Chrome") {
            a.get("pfad")?.as_str().map(str::to_owned)
        } else {
            None
        }
    });
    let Some(chrome) = chrome else {
        return serde_json::json!({
            "ok": false,
            "grund": "Kein isoliert startbarer Browser für Nokis Schreibtisch gefunden."
        });
    };
    arbeitsplatz_oeffnen_blockierend(app, url, "Google Chrome".into(), Some(chrome))
}

/// Aufgabenbesitz beenden.
///
/// Drei Dinge werden hier bewusst NICHT zusammengeworfen:
///   * `TASK_FENSTER` - die laufende Aufgabe. Die endet hier.
///   * `ARBEITSPLATZ_FENSTER` - der INHALT des Schreibtischs. Der bleibt;
///     ein Fenster hoert nicht auf, dort zu stehen, nur weil die Aufgabe
///     fertig ist. Beides zusammen zu leeren war der Grund, warum die
///     Miniatur nach getaner Arbeit leer wirkte.
///   * die Reservierung selbst. Der Schreibtisch ist Infrastruktur, kein
///     Verbrauchsmaterial einer einzelnen Aufgabe.
///
/// Ausserdem faellt hier die Ortsbindung (Abschnitt 7) - genau einmal. Bleibt
/// sie haengen, klebt die Figur fuer immer auf Schreibtisch 4 und wirkt, als
/// "komme sie von einem anderen Schreibtisch".
#[tauri::command]
fn noki_arbeitsplatz_aufgabe_beenden() -> serde_json::Value {
    let n = TASK_FENSTER
        .lock()
        .map(|mut g| {
            let n = g.len();
            g.clear();
            n
        })
        .unwrap_or(0);
    let war = ortsbindung_loesen();
    if war {
        eprintln!("[ARBEITSPLATZ] Ortsbindung geloest ({n} Aufgabenfenster freigegeben)");
    }
    serde_json::json!({
        "ok": true, "freigegeben": n, "ortsbindung_geloest": war,
        "arbeitsplatz": "bleibt reserviert",
        "schreibtisch_fenster": arbeitsplatz_fenster_liste().len()
    })
}

/// Freigabe "Bildschirmaufnahme" fuer DIESEN Prozess. Eigenstaendig - nicht
/// zu verwechseln mit Bedienungshilfen, Apple Events, Mikrofon oder Kamera.
/// Nur die Vorschau haengt daran; Arbeitsplatz, Kuerzel 4 und alle Aktionen
/// laufen ohne sie weiter.
#[cfg(target_os = "macos")]
fn schirm_freigabe() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
    }
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// Fragt EINMAL nach - danach entscheidet die Systemeinstellung. Wiederholtes
/// Nachfragen waere Belaestigung und wird deshalb gesperrt.
static SCHIRM_GEFRAGT: AtomicBool = AtomicBool::new(false);

#[tauri::command]
fn noki_schirm_freigabe(fragen: bool) -> serde_json::Value {
    #[cfg(target_os = "macos")]
    {
        #[link(name = "CoreGraphics", kind = "framework")]
        extern "C" {
            fn CGRequestScreenCaptureAccess() -> bool;
        }
        let vorher = schirm_freigabe();
        if !vorher && fragen && !SCHIRM_GEFRAGT.swap(true, Ordering::Relaxed) {
            unsafe {
                CGRequestScreenCaptureAccess();
            }
        }
        let jetzt = schirm_freigabe();
        serde_json::json!({
            "stand": if jetzt { "GEWAEHRT" } else if SCHIRM_GEFRAGT.load(Ordering::Relaxed) { "VERWEIGERT" } else { "UNBEKANNT" },
            "gewaehrt": jetzt,
            "gefragt": SCHIRM_GEFRAGT.load(Ordering::Relaxed),
            "hinweis": if jetzt { "" } else {
                "Vorschau braucht die Freigabe „Bildschirmaufnahme“ in den Systemeinstellungen → Datenschutz & Sicherheit."
            }
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = fragen;
        serde_json::json!({ "stand": "UNBEKANNT", "gewaehrt": false })
    }
}

/// Vorschau auf die aufgabeneigenen Fenster richten. Ohne Argument nimmt sie
/// genau das, was die laufende Aufgabe beansprucht hat - nie mehr.
#[tauri::command]
fn noki_vorschau_setzen(
    app: tauri::AppHandle,
    fenster: Option<Vec<i64>>,
    breite: Option<u32>,
    fps: Option<u32>,
) -> serde_json::Value {
    #[cfg(target_os = "macos")]
    if !schirm_freigabe() {
        return serde_json::json!({
            "ok": false, "grund": "freigabe",
            "hinweis": "Vorschau braucht die Freigabe „Bildschirmaufnahme“."
        });
    }
    #[cfg(target_os = "macos")]
    if virtual_workspace::backend() == virtual_workspace::Backend::LegacyVirtualDisplay {
        if !virtual_workspace::ready() {
            // Remember the user's requested visible state. READY performs
            // exactly one retry; no per-interaction backend fallback.
            VORSCHAU_SICHTBAR.store(true, Ordering::Relaxed);
            virtual_workspace::trace(&format!(
                "[VORSCHAU] request=safely_consumed backend=virtual state={:?}",
                virtual_workspace::readiness()
            ));
            return serde_json::json!({
                "ok": false, "grund": "Virtueller Schreibtisch wird noch initialisiert.",
                "state": format!("{:?}", virtual_workspace::readiness()).to_uppercase()
            });
        }
        let ids = vorschau_faehig(fenster.unwrap_or_else(|| {
            TASK_FENSTER.lock().map(|g| g.clone()).unwrap_or_default()
        }));
        VORSCHAU_SICHTBAR.store(true, Ordering::Relaxed);
        virtual_workspace::trace(&format!(
            "[VORSCHAU] backend=virtual state=READY windows={:?} physical_space_query=false", ids
        ));
        return vorschau::setzen(
            &app, ids, breite.unwrap_or(520).clamp(120, 1400),
            fps.unwrap_or(30).clamp(1, 60), "virtual-display".into(), String::new(),
        );
    }
    // Ohne explizite Auswahl ist die Vorschau immer die GESAMTE Noki-
    // Arbeitsflaeche, nicht nur das zuletzt geoeffnete Aufgabenfenster.
    // Ohne ausdrueckliche Auswahl zeigt die Miniatur den SCHREIBTISCH: alles,
    // was Noki dort erzeugt hat und was es dort noch gibt. Der Abgleich
    // laeuft bei jedem Zeigen - eine gemerkte Liste allein waere nach einem
    // Neustart leer und nach einem Fensterschluss falsch.
    // The WANT is recorded before any transient refusal below.  The space
    // watchdog only heals while this flag is set; storing it after the early
    // returns meant one refused call (user still reported on the Noki Space
    // right after a return swipe) left the Miniatur dead until restart.
    VORSCHAU_SICHTBAR.store(true, Ordering::Relaxed);
    let target = match get_preview_target() {
        Some(t) => t,
        None => {
            let res = match arbeitsplatz_sichern(&app) {
                Ok(r) => r,
                Err(e) => return serde_json::json!({ "ok": false, "grund": e }),
            };
            TargetOwnerSnapshot {
                uuid: res.uuid,
                space_id: res.id,
                desktop_number: 1,
                display: res.display,
                epoch: res.generation,
                physical_current: LETZT_SID.load(Ordering::Relaxed),
            }
        }
    };
    let ziel_uuid = target.uuid;
    let target_id = target.space_id;
    let nutzer = LETZT_SID.load(Ordering::Relaxed);
    let nutzer_uuid = cgs::published_topology()
        .or_else(|| cgs::desktops())
        .and_then(|(_, liste)| liste.into_iter().find(|s| s.id == nutzer).map(|s| s.uuid))
        .unwrap_or_default();
    let ids = match fenster {
        Some(f) => f,
        None => {
            takt("vorschau_setzen/abgleichen", || {
                let _ = arbeitsplatz_fenster_abgleichen(&app, target_id);
            });
            sichtbare_fenster_auf_arbeitsplatz(target_id)
        }
    };
    VORSCHAU_SICHTBAR.store(true, Ordering::Relaxed);
    takt("vorschau_setzen/senden", || vorschau::setzen(
        &app,
        ids,
        breite.unwrap_or(520).clamp(120, 1400),
        fps.unwrap_or(8).clamp(1, 15),
        ziel_uuid,
        nutzer_uuid,
    ))
}

/// Shortcut 4 SHOW while the user physically stands on the preview target.
///
/// The Miniatur must never show the Desktop the user is standing on, so an
/// explicit SHOW takes ONE deterministic step to the next normal Desktop that
/// is not the physical one (source USER_SHOW_REQUEST). The helper is
/// retargeted first and membership follows only after it published the new
/// Desktop - otherwise its old composition (= the current Desktop) would
/// flash as a recursive picture before the atomic swap.
#[tauri::command]
fn noki_vorschau_zeigen_anfrage(app: tauri::AppHandle) -> serde_json::Value {
    #[cfg(target_os = "macos")]
    {
        if virtual_workspace::backend() == virtual_workspace::Backend::LegacyVirtualDisplay {
            return serde_json::json!({ "ok": true, "gewechselt": false, "auf_arbeitsplatz": false });
        }
        let Some((_, liste)) = cgs::published_topology().or_else(|| cgs::desktops()) else {
            return serde_json::json!({ "ok": false, "grund": "Schreibtische nicht lesbar" });
        };
        let mut nutzer = LETZT_SID.load(Ordering::Relaxed);
        if nutzer == 0 {
            nutzer = cgs::aktiver_space().map(|(a, _)| a).unwrap_or(0);
        }
        let jetzt = get_preview_target().map(|t| t.uuid).unwrap_or_else(|| arbeitsplatz_uuid_lesen(&app));
        let jetzt_id = liste.iter().find(|s| s.uuid == jetzt).map(|s| s.id);
        if nutzer == 0 || jetzt_id != Some(nutzer) {
            return serde_json::json!({ "ok": true, "gewechselt": false, "auf_arbeitsplatz": false });
        }
        let Some(ziel) = arbeitsplatz::naechster_ausser_physisch(&liste, &jetzt, nutzer) else {
            virtual_workspace::trace("[SHORTCUT4] show_on_target no_other_normal_desktop -> stays hidden");
            return serde_json::json!({ "ok": true, "gewechselt": false, "auf_arbeitsplatz": true });
        };
        let snap = match set_preview_target(&app, &ziel, TargetChangeSource::UserShowRequest, "shortcut4_show_on_target") {
            Ok(s) => s,
            Err(e) => return serde_json::json!({ "ok": false, "grund": e }),
        };
        if let Ok(mut t) = TASK_FENSTER.lock() {
            t.clear();
        }
        let nutzer_uuid = liste.iter().find(|s| s.id == nutzer).map(|s| s.uuid.clone()).unwrap_or_default();
        let nummer = snap.desktop_number;
        vorschau::navi(&format!("Zum Schreibtisch {nummer}"));
        let h = app.clone();
        thread::spawn(move || {
            let t0 = std::time::Instant::now();
            let ids = sichtbare_fenster_auf_arbeitsplatz(snap.space_id);
            vorschau::ziel_meta(&format!("Zum Schreibtisch {nummer}"), &leiste_schnell(&ids, snap.space_id));
            let _ = vorschau::setzen(&h, ids, 520, 30, snap.uuid.clone(), nutzer_uuid);
            while t0.elapsed() < Duration::from_millis(2500)
                && vorschau::veroeffentlichte_uuid() != snap.uuid
            {
                if TARGET_EPOCH.load(Ordering::SeqCst) != snap.epoch {
                    return; // a newer explicit choice owns the Miniatur now
                }
                thread::sleep(Duration::from_millis(20));
            }
            let publiziert = vorschau::veroeffentlichte_uuid() == snap.uuid;
            virtual_workspace::trace(&format!(
                "[SHORTCUT4] show_on_target retarget={} published={} ms={}",
                snap.space_id, publiziert, t0.elapsed().as_millis()
            ));
            vorschau_spaces_richten(&h);
            leiste_anstossen(&h);
            let _ = h.emit(
                "noki://arbeitsplatz_gewaehlt",
                serde_json::json!({
                    "epoch": snap.epoch, "uuid": snap.uuid, "id": snap.space_id,
                    "nummer": nummer, "auf_arbeitsplatz": false,
                    "navi_text": format!("Zum Schreibtisch {nummer}")
                }),
            );
        });
        return serde_json::json!({ "ok": true, "gewechselt": true, "nummer": nummer, "auf_arbeitsplatz": false });
    }
    #[allow(unreachable_code)]
    {
        let _ = app;
        serde_json::json!({ "ok": true, "gewechselt": false, "auf_arbeitsplatz": false })
    }
}

/// Aufnahme beenden - beim Verbergen der Vorschau und am Aufgabenende.
/// Die Oberflaeche meldet, ob die Miniatur gerade gross ist. Nur eine
/// Auskunft: sie schaltet keinen Space, keine Aufnahme und keine Bewegung.
#[tauri::command]
fn noki_vorschau_gross(gross: bool) {
    VORSCHAU_GROSS.store(gross, Ordering::Relaxed);
    vorschau::modus(gross);
}

#[tauri::command]
fn noki_vorschau_stop(user_hidden: Option<bool>) -> serde_json::Value {
    // On the virtual-display backend there is no physical "user is on Noki's
    // Space" state. Ignore legacy automatic suppression; otherwise it stops
    // healthy ScreenCaptureKit streams and leaves the native panel showing an
    // indefinitely retained frame. Only an explicit user Hide may enter the
    // hidden state and stop capture.
    if virtual_workspace::backend() == virtual_workspace::Backend::LegacyVirtualDisplay
        && user_hidden != Some(true) {
        virtual_workspace::trace(
            "[VORSCHAU] auto_stop_ignored backend=virtual visible_state=VISIBLE",
        );
        return serde_json::json!({ "ok": true, "aufnahme": vorschau::aufgenommene().len(), "ignored": true });
    }
    VORSCHAU_SICHTBAR.store(false, Ordering::Relaxed);
    // NICHT die Groesse zuruecksetzen: ein Groessenwechsel haelt die Aufnahme
    // kurz an und startet sie mit neuer Zielbreite neu. Wurde hier auch
    // "gross" geloescht, war der Beobachter fuer den Klick daneben genau
    // nach dem Vergroessern wieder aus - und das Zurueckschnappen blieb aus.
    // Ob die Miniatur gross ist, meldet die Oberflaeche getrennt.
    vorschau::stoppen()
}

/// Abschnitt 1: WELCHE Fenster liegen wirklich auf Nokis Schreibtisch, und
/// was haelt Noki von jedem einzelnen? Die Tabelle trennt bewusst
/// "existiert", "gehoert Noki" und "wird aufgenommen" - genau an dieser
/// Trennung entscheidet sich, wo ein Fenster aus der Kette faellt.
#[tauri::command]
fn noki_arbeitsplatz_diagnose(app: tauri::AppHandle) -> serde_json::Value {
    #[cfg(target_os = "macos")]
    {
        if virtual_workspace::backend() == virtual_workspace::Backend::LegacyVirtualDisplay {
            return serde_json::json!({
                "ok": virtual_workspace::ready(),
                "workspace": virtual_workspace::diagnostic(),
                "aufgenommen": vorschau::aufgenommene(),
                "aufgabe_fenster": TASK_FENSTER.lock().map(|g| g.clone()).unwrap_or_default(),
                "aufgabe_laeuft": arbeitsplatz_aufgabe_laeuft(),
            });
        }
        let r = match arbeitsplatz_sichern(&app) {
            Ok(r) => r,
            Err(e) => return serde_json::json!({ "ok": false, "grund": e }),
        };
        let task: Vec<i64> = TASK_FENSTER.lock().map(|g| g.clone()).unwrap_or_default();
        let besitz = arbeitsplatz_fenster_liste();
        let aufnahme = vorschau::aufgenommene();
        let sichtbar = sichtbare_fenster_auf_arbeitsplatz(r.id);
        let echte: Vec<serde_json::Value> = cgs::fenster_auf_space(r.id)
            .into_iter()
            .map(|(wid, pid, owner, titel, x, y, w, h)| {
                let eigen = besitz.iter().any(|e| e.fenster == wid);
                serde_json::json!({
                    "fenster": wid, "pid": pid, "app": owner, "titel": titel,
                    "rahmen": [x, y, w, h],
                    "besitz": if task.contains(&wid) { "aufgabe" }
                              else if eigen { "arbeitsplatz" } else { "fremd" },
                    "sichtbar": sichtbar.contains(&wid),
                    "aufgenommen": aufnahme.contains(&wid),
                })
            })
            .collect();
        // Gemerkte Fenster, die es auf dem Schreibtisch nicht mehr gibt.
        let verwaist: Vec<i64> = besitz
            .iter()
            .map(|e| e.fenster)
            .filter(|w| !echte.iter().any(|e| e["fenster"] == *w))
            .collect();
        return serde_json::json!({
            "ok": true, "arbeitsplatz": r.id, "uuid": r.uuid,
            "aktiv": cgs::aktiver_space().map(|(a, _)| a).unwrap_or(0),
            "echte_fenster": echte,
            "gemerkt": besitz.iter().map(|e| serde_json::json!({
                "fenster": e.fenster, "pid": e.pid, "app": e.app
            })).collect::<Vec<_>>(),
            "aufgabe_fenster": task,
            "aufgenommen": aufnahme,
            "verwaist": verwaist,
            "aufgabe_laeuft": arbeitsplatz_aufgabe_laeuft(),
            "interaction_health": vorschau::interaction_health(),
            "browser_health": noki_browser::health(),
            "shortcut_health": shortcut_health(),
        });
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        serde_json::json!({ "ok": false })
    }
}

#[tauri::command]
async fn noki_arbeitsplatz_besuchen(app: tauri::AppHandle) -> serde_json::Value {
    match tauri::async_runtime::spawn_blocking(move || arbeitsplatz_besuchen(&app)).await {
        Ok(v) => {
            virtual_workspace::trace(&format!("[REAL_SPACE] visit_result={v}"));
            v
        }
        Err(e) => serde_json::json!({ "ok": false, "grund": format!("{e}") }),
    }
}

/// Lage des Arbeitsplatzes — fuer Einstellungen, Tests und die Vorschau.
#[tauri::command]
fn noki_arbeitsplatz_lage(app: tauri::AppHandle) -> serde_json::Value {
    #[cfg(target_os = "macos")]
    {
        if virtual_workspace::backend() == virtual_workspace::Backend::LegacyVirtualDisplay {
            let diagnostic = virtual_workspace::diagnostic();
            return serde_json::json!({
                "ok": virtual_workspace::ready(),
                "rolle": arbeitsplatz::ROLLE,
                "backend": "LEGACY_VIRTUAL_DISPLAY",
                "state": match virtual_workspace::workspace_readiness() {
                    virtual_workspace::Readiness::Initializing => "INITIALIZING",
                    virtual_workspace::Readiness::Ready => "READY",
                    virtual_workspace::Readiness::Failed => "FAILED",
                },
                "backend_state": diagnostic["state"],
                "workspace_state": diagnostic["workspace_state"],
                "window_count": diagnostic["window_count"],
                "restore_failures": diagnostic["restore_failures"],
                "virtuell": diagnostic,
                "nummer": serde_json::Value::Null,
                "schreibtische": [],
            });
        }
        let (display, liste) = match cgs::desktops() {
            Some(x) => x,
            None => return serde_json::json!({ "ok": false, "grund": "nicht lesbar" }),
        };
        let aktiv = cgs::aktiver_space().map(|(a, _)| a).unwrap_or(0);
        let res = ARBEITSPLATZ.lock().ok().and_then(|g| g.clone())
            .or_else(|| {
                let u = arbeitsplatz_uuid_lesen(&app);
                liste.iter().find(|s| s.uuid == u && s.typ == arbeitsplatz::TYP_SCHREIBTISCH).map(|s| {
                    arbeitsplatz::Reservierung {
                        uuid: s.uuid.clone(),
                        display: display.clone(),
                        id: s.id,
                        generation: 1,
                    }
                })
            })
            .or_else(|| arbeitsplatz_sichern(&app).ok());
        let nummer = res
            .as_ref()
            .and_then(|r| arbeitsplatz::sichtbare_nummer(&liste, r.id));
        let auf_arbeitsplatz = res.as_ref().map(|r| r.id == aktiv).unwrap_or(false);
        let navi_text = if auf_arbeitsplatz {
            "Auf Noki Schreibtisch".to_string()
        } else {
            format!("Zum Schreibtisch {}", nummer.unwrap_or(1))
        };
        let result = serde_json::json!({
            "ok": res.is_some(),
            "backend": "REAL_SPACE",
            "rolle": arbeitsplatz::ROLLE,
            "display": display,
            "aktiv": aktiv,
            "auf_arbeitsplatz": auf_arbeitsplatz,
            "navi_text": navi_text,
            "arbeitsplatz": res,
            "nummer": nummer,
            "schreibtische": liste,
        });
        virtual_workspace::trace(&format!("[REAL_SPACE] discovery={result}"));
        result
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        serde_json::json!({ "ok": false })
    }
}

/// Zuletzt beobachteter Space. Modulweit, weil auch eine Klammer
/// (Nokis eigener Wechsel) ihn fortschreiben muss - sonst gaebe es nach
/// der Klammer einen Nachzuegler, der den ganzen Weg noch einmal fliegt.
static LETZT_SID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Stand der EINEN Frage, die die Miniatur etwas angeht: steht der Nutzer
/// auf Nokis Schreibtisch? Alles andere ist fuer sie kein Ereignis.
static LETZT_AUF_ARBEITSPLATZ: AtomicBool = AtomicBool::new(false);
/// Die erste Meldung geht immer hinaus - sonst erfuehre die Oberflaeche den
/// Ausgangszustand nie.
static ERSTE_MELDUNG: AtomicBool = AtomicBool::new(true);

/// Ist die Miniatur gerade GROSS? Nur dann wird auf einen Klick daneben
/// geachtet (Abschnitt 9): kein Beobachter, wenn es nichts zu beobachten gibt.
static VORSCHAU_GROSS: AtomicBool = AtomicBool::new(false);

static MITGLIED_WECK: (Mutex<bool>, std::sync::Condvar) = (Mutex::new(false), std::sync::Condvar::new());
fn mitglied_wecken() {
    if let Ok(mut g) = MITGLIED_WECK.0.lock() { *g = true; MITGLIED_WECK.1.notify_all(); }
}
/// Sleeps up to `dauer`; true when a membership event woke it.
fn mitglied_warten(dauer: Duration) -> bool {
    let Ok(g) = MITGLIED_WECK.0.lock() else { thread::sleep(dauer); return false };
    let (mut g, _) = MITGLIED_WECK.1.wait_timeout_while(g, dauer, |w| !*w).unwrap_or_else(|e| e.into_inner());
    std::mem::replace(&mut *g, false)
}

fn space_waechter(app: tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    vorschau::fernbedienung::ax_zeitlimit_setzen();
    // JEDER echte Space-Wechsel (Schreibtisch/Vollbild in beliebiger Richtung)
    // wird geflogen: EXIT im alten Space, Umzug, ENTER im neuen. Der Space-Typ
    // allein loest nie ein direktes Erscheinen aus.
    #[cfg(target_os = "macos")]
    {
        let h = app.clone();
        let mut nachschauen = false;
        thread::spawn(move || loop {
            // REAL_SPACE: the Miniatur must follow the real Space closely.
            // Event-driven first (AX minimize/restore/create/destroy wake
            // this loop at once, then once more after the minimize
            // animation); the 1 s tick is only the fallback.
            let echt_raum = virtual_workspace::backend() == virtual_workspace::Backend::RealSpace;
            let warte = if !echt_raum { 2000 } else if nachschauen { 450 } else { 1000 };
            let geweckt = mitglied_warten(Duration::from_millis(warte));
            nachschauen = geweckt;
            if !VORSCHAU_SICHTBAR.load(Ordering::Relaxed) {
                continue;
            }
            // Space swipe running (Dock gesture + 300 ms): no full window
            // enumerations while WindowServer animates the slide. The next
            // round after the gesture catches up.
            if window_overview::geste_laeuft() {
                nachschauen = true;
                continue;
            }
            if virtual_workspace::backend() == virtual_workspace::Backend::LegacyVirtualDisplay {
                // Virtual capture membership is explicit. Never rediscover it
                // through a physical Space UUID or trigger a backend bridge.
                let mut echt = workspace_registry_refresh(&h);
                for id in TASK_FENSTER.lock().map(|g| g.clone()).unwrap_or_default() {
                    if !echt.contains(&id) { echt.push(id); }
                }
                echt.sort_unstable(); echt.dedup();
                // Die Leiste (AX-Ampelrahmen + App-Infos je Fenster) nur neu
                // berechnen, wenn sich die Fenstermenge aendert - sonst alle
                // 10 s als Absicherung. Gemessen: alle 2 s war sie der groesste
                // Posten im Leerlauf (LaunchServices + Bedienungshilfen).
                {
                    static LETZTE: std::sync::Mutex<Vec<i64>> = std::sync::Mutex::new(Vec::new());
                    static RUNDE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                    let neu = LETZTE.lock().map(|mut g| { let n = *g != echt; if n { *g = echt.clone(); } n }).unwrap_or(true);
                    if neu || RUNDE.fetch_add(1, Ordering::Relaxed) % 5 == 4 { leiste_aktualisieren(&h); }
                }
                let mut echt = vorschau_faehig(echt);
                // Offene echte Kontextmenues gehoeren fuer ihre Dauer dazu.
                echt.extend(vorschau::menues());
                echt.sort_unstable(); echt.dedup();
                let helper_gesund = vorschau::helfer_gesundheit_pruefen();
                if (echt != vorschau::aufgenommene() || !helper_gesund) && virtual_workspace::ready()
                    && !vorschau::retarget_aktiv() {
                    let _ = vorschau::setzen(&h, echt, 520, 30,
                        "virtual-display".into(), String::new());
                }
                continue;
            }
            // A hidden remote visit temporarily makes target-only browser
            // surfaces visible.  They are transaction ephemera, not a new
            // Desktop membership snapshot; capturing them caused repeated
            // retarget/start-stream failures and stale cross-contamination.
            if eigener_wechsel_laeuft() {
                continue;
            }
            let ap_id = ARBEITSPLATZ
                .lock()
                .ok()
                .and_then(|g| g.as_ref().map(|r| r.id))
                .unwrap_or(0);
            if ap_id == 0 {
                continue;
            }
            // The user stands on the Noki Space: the Miniatur is suppressed
            // there and must never capture the user's own current Space.
            if cgs::aktiver_space().map(|a| a.0) == Some(ap_id) {
                continue;
            }
            let Some(echt) = stabile_sichtbare_fenster(ap_id) else {
                continue;
            };
            {
                // Membership events for exactly the apps on the Noki Space.
                let alle = cgs::alle_fenster();
                // Never an AX observer on Noki itself (Ask is in the set).
                let pids: Vec<i32> = cgs::fenster_reihe(ap_id).iter()
                    .filter_map(|w| alle.iter().find(|f| f.0 == *w).map(|f| f.1))
                    .filter(|p| *p != std::process::id() as i32).collect();
                kind_fenster::mitglieder_beobachten(&pids, mitglied_wecken);
            }
            {
                // App bar: on a change of the window set, else every 10 s.
                static LETZTE: std::sync::Mutex<Vec<i64>> = std::sync::Mutex::new(Vec::new());
                static RUNDE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                let neu = LETZTE.lock().map(|mut g| { let n = *g != echt; if n { *g = echt.clone(); } n }).unwrap_or(true);
                if neu || RUNDE.fetch_add(1, Ordering::Relaxed) % 10 == 9 { leiste_anstossen(&h); }
            }
            let helper_gesund = vorschau::helfer_gesundheit_pruefen();
            if echt != vorschau::aufgenommene() || !helper_gesund {
                let (ziel_uuid, nutzer_uuid) = {
                    let ziel = get_preview_target().map(|t| t.uuid)
                        .or_else(|| ARBEITSPLATZ.lock().ok().and_then(|g| g.as_ref().map(|r| r.uuid.clone())))
                        .unwrap_or_default();
                    let nutzer = LETZT_SID.load(Ordering::Relaxed);
                    let aktuell = cgs::published_topology().or_else(|| cgs::desktops())
                        .and_then(|(_, liste)| liste.into_iter().find(|s| s.id == nutzer).map(|s| s.uuid))
                        .unwrap_or_default();
                    (ziel, aktuell)
                };
                let _ = vorschau::setzen(&h, echt, 520, 30, ziel_uuid, nutzer_uuid);
            }
        });
    }
    #[cfg(target_os = "macos")]
    thread::spawn(move || loop {
        thread::sleep(std::time::Duration::from_millis(120));
        let Some((sid, typ)) = cgs::aktiver_space() else {
            continue;
        };
        // Die Vorschau muss auch dann auf manuelle Space-Wechsel reagieren,
        // wenn die Figur verborgen ist oder gerade nicht allen Spaces folgt.
        let arbeitsplatz_id = if virtual_workspace::backend()
            == virtual_workspace::Backend::LegacyVirtualDisplay { None } else {
            get_preview_target().map(|t| t.space_id)
                .or_else(|| ARBEITSPLATZ.lock().ok().and_then(|g| g.as_ref().map(|r| r.id)))
        };
        // Nokis eigener Kurzbesuch auf seinem Schreibtisch ist KEIN Umzug des
        // Nutzers. Waehrend der Klammer wird der beobachtete Space nur
        // mitgeschrieben - weder fliegt die Figur, noch meldet die Miniatur
        // eine Ankunft, die gar nicht stattgefunden hat.
        if eigener_wechsel_laeuft() {
            LETZT_SID.store(sid, Ordering::Relaxed);
            continue;
        }
        {
            // Ein Wechsel zwischen zwei GEWOEHNLICHEN Schreibtischen aendert an
            // Nokis Schreibtisch nichts: nicht seine Fenster, nicht sein
            // Hintergrundbild, nicht die Sichtbarkeit der Miniatur. Gemeldet
            // wird deshalb nur, was sich wirklich geaendert hat - ob der
            // Nutzer auf Nokis Schreibtisch steht oder nicht. Vorher ging bei
            // jedem Schreibtischwechsel eine Meldung hinaus, die Miniatur
            // wurde abgeraeumt und neu aufgebaut, und genau das sah der
            // Nutzer als Flackern.
            let auf_arbeitsplatz = arbeitsplatz_id == Some(sid);
            let vorher_auf = LETZT_AUF_ARBEITSPLATZ.swap(auf_arbeitsplatz, Ordering::Relaxed);
            let sid_vorher = LETZT_SID.swap(sid, Ordering::Relaxed);
            let sid_neu = sid_vorher != sid;
            // Z-order parity for ORDINARY swipes: what the Miniatur showed
            // (native order of the target, restricted to its windows) is
            // snapshotted while the user is elsewhere. Arriving by a normal
            // swipe, macOS re-activates the Space's last active app and lifts
            // its windows (measured: VS Code jumped over GoodNotes that had
            // been brought to front in the Miniatur). Restore that order with
            // the same verified pass the footer visit uses.
            // RECONCILIATION (self-heal): once the Space is stable, the
            // Miniatur must be visible iff the user wants it and the user is
            // not on its target. TEMP hides (same Space, swipe) never touch
            // the preference; a lost "show" is repaired here, not by the user.
            static STABIL_SEIT: Mutex<Option<std::time::Instant>> = Mutex::new(None);
            static HEIL_ZULETZT: Mutex<Option<std::time::Instant>> = Mutex::new(None);
            static HEIL_TAKT: AtomicU32 = AtomicU32::new(0);
            {
                let mut st = STABIL_SEIT.lock().unwrap_or_else(|e| e.into_inner());
                if sid_neu || st.is_none() { *st = Some(std::time::Instant::now()); }
                let stabil = st.is_some_and(|t| t.elapsed() >= Duration::from_millis(1500));
                drop(st);
                let pruefen = stabil && HEIL_TAKT.fetch_add(1, Ordering::Relaxed) % 8 == 0;
                if pruefen && virtual_workspace::backend() != virtual_workspace::Backend::LegacyVirtualDisplay {
                    let soll = MINIATUR_NUTZER_SICHTBAR.load(Ordering::Relaxed)
                        && arbeitsplatz_id.is_some_and(|ap| ap != 0 && ap != sid)
                        && matches!(typ, 0 | 4)
                        && vorschau::laeuft()
                        && !vorschau::fern_beschaeftigt();
                    if soll && !vorschau::wirklich_sichtbar() {
                        let frei = HEIL_ZULETZT.lock().ok().and_then(|g| *g)
                            .map_or(true, |t| t.elapsed() >= Duration::from_secs(3));
                        if frei {
                            if let Ok(mut g) = HEIL_ZULETZT.lock() { *g = Some(std::time::Instant::now()); }
                            virtual_workspace::trace(&format!(
                                "[VORSCHAU] SELF_HEAL phys={sid} target={:?} ort_letzt={:?} helper={:?} want_rust={}",
                                arbeitsplatz_id, vorschau::ort_letzt(), vorschau::helfer_uebergang(),
                                VORSCHAU_SICHTBAR.load(Ordering::Relaxed)));
                            let nummer = get_preview_target().map(|t| t.desktop_number);
                            let _ = app.emit("noki://arbeitsplatz_aktiv", serde_json::json!({
                                "aktiv": false, "space": sid, "arbeitsplatz": arbeitsplatz_id,
                                "nummer": nummer, "navi_text": format!("Zum Schreibtisch {}", nummer.unwrap_or(1))
                            }));
                            vorschau::ort_cache_leeren();
                            vorschau_spaces_richten(&app);
                        }
                    }
                }
            }
            static MINIATUR_REIHE: Mutex<(u64, Vec<i64>)> = Mutex::new((0, Vec::new()));
            static REIHE_TAKT: AtomicU32 = AtomicU32::new(0);
            if let Some(ap) = arbeitsplatz_id {
                if !auf_arbeitsplatz && !sid_neu && VORSCHAU_SICHTBAR.load(Ordering::Relaxed)
                    && REIHE_TAKT.fetch_add(1, Ordering::Relaxed) % 4 == 0
                {
                    let gezeigt = vorschau::aufgenommene();
                    let reihe: Vec<i64> = cgs::fenster_reihe(ap).into_iter()
                        .filter(|w| gezeigt.contains(w)).collect();
                    if let Ok(mut g) = MINIATUR_REIHE.lock() { *g = (ap, reihe); }
                }
                if sid_neu && auf_arbeitsplatz && !vorher_auf && VORSCHAU_SICHTBAR.load(Ordering::Relaxed) {
                    let snap = MINIATUR_REIHE.lock().ok()
                        .and_then(|g| (g.0 == ap && g.1.len() >= 2).then(|| g.1.clone()));
                    if let Some(snap) = snap {
                        thread::spawn(move || ankunft_ordnung_herstellen(ap, &snap));
                    }
                }
            }
            if sid_neu && sid_vorher != 0 {
                // TEMPORARY correlation (Space-switch audit): every physical
                // change Noki did NOT start itself, next to the last Miniatur
                // content interaction. Only the footer may switch a Space.
                let jetzt_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64).unwrap_or(0);
                let alter = |t: u64| if t == 0 { -1 } else { jetzt_ms.saturating_sub(t) as i64 };
                let (aktion_ms, aktion) = letzte_space_aktion();
                virtual_workspace::trace(&format!(
                    "[SPACE_AUDIT] change {sid_vorher}->{sid} own=false content_interaction_ms_ago={} fern_busy={} typing={} last_nav_call_ms_ago={} nav_caller={} frontmost_pid={} last_space_action={} last_space_action_ms_ago={}",
                    alter(vorschau::letzte_fern_ms()), vorschau::fern_beschaeftigt(), fern_tippen::aktiv(),
                    alter(cgs::letzte_navigation().0), cgs::letzte_navigation().1, blende::vorn_pid(),
                    if aktion.is_empty() { "-" } else { aktion.as_str() }, alter(aktion_ms)));
            }
            if sid_neu && window_overview::is_open() {
                // The overview never closes on its own: it follows the user to
                // the new Space (also native fullscreen in/out) and reloads.
                let h = app.clone();
                let _ = app.run_on_main_thread(move || window_overview::space_gewechselt(&h));
            }
            if sid_neu {
                // Visible => frontmost on every Space change (also full screen
                // in/out): re-assert the layer, order front without activation.
                noki_vorn_sichern_app(&app);
                // Every Space change re-reads the real topology (validated).
                let _ = cgs::desktops();
                reconcile_target_topology(&app, TargetChangeSource::SpaceWatcher, "space_watcher_sid_neu");
                // Neu angelegte Schreibtische bekommen die Miniatur ebenso.
                SPACE_GEWECHSELT.store(true, Ordering::Relaxed);
                vorschau_spaces_richten(&app);
            }
            let erste = ERSTE_MELDUNG.load(Ordering::Relaxed);
            if sid_neu && (vorher_auf != auf_arbeitsplatz || ERSTE_MELDUNG.swap(false, Ordering::Relaxed)) {
                let nummer = get_preview_target().map(|t| t.desktop_number).or_else(|| {
                    arbeitsplatz_id.and_then(|id| {
                        cgs::published_topology().or_else(|| cgs::desktops())
                            .and_then(|(_, liste)| arbeitsplatz::sichtbare_nummer(&liste, id))
                    })
                });
                let navi_text = if auf_arbeitsplatz {
                    "Auf Noki Schreibtisch".to_string()
                } else {
                    format!("Zum Schreibtisch {}", nummer.unwrap_or(1))
                };
                vorschau::navi(&navi_text);
                let stand = serde_json::json!({
                    "aktiv": auf_arbeitsplatz,
                    "space": sid,
                    "arbeitsplatz": arbeitsplatz_id,
                    "nummer": nummer,
                    "navi_text": navi_text
                });
                let _ = app.emit("noki://arbeitsplatz_aktiv", stand.clone());
                // Beim Wegwischen bestaetigen wir nach der kurzen Systemanimation den Zustand einmalig
                let h = app.clone();
                thread::spawn(move || {
                    thread::sleep(Duration::from_millis(350));
                    let _ = h.emit("noki://arbeitsplatz_aktiv", stand);
                });
            }
        }
        if NUTZER_VERBORGEN.load(Ordering::Relaxed) || !NOKI_GEZEIGT.load(Ordering::Relaxed) || global_verborgen() {
            continue;
        }
        if !app.state::<Lage>().alle_spaces.load(Ordering::Relaxed) {
            continue;
        } // bleibt, wo er ist
        let mut wid = NOKI_WID.load(Ordering::Relaxed);
        if wid == 0 {
            let (tx, rx) = std::sync::mpsc::channel();
            let h = app.clone();
            let _ = app.run_on_main_thread(move || {
                let _ = tx.send(
                    h.get_webview_window(FENSTER)
                        .and_then(|w| fenster_nummer(&w))
                        .unwrap_or(0),
                );
            });
            wid = rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap_or(0);
            if wid == 0 {
                continue;
            }
            NOKI_WID.store(wid, Ordering::Relaxed);
        }
        // Genau EIN Besitzer bestimmt, wo Noki sich aufhaelt. Waehrend einer
        // Arbeitsplatz-Aufgabe ist das der Arbeitsplatz; sonst folgt er wie
        // bisher dem Nutzer.
        // Eine Ortsbindung ohne Lebenszeichen verfaellt. Sonst bliebe die
        // Figur nach einer abgebrochenen Aktion fuer immer dort stehen.
        let seit = ARBEITSPLATZ_AUFGABE_SEIT.load(Ordering::Relaxed);
        if seit != 0 && jetzt_sekunden().saturating_sub(seit) > ORTSBINDUNG_FRIST && ortsbindung_loesen() {
            eprintln!("[ARBEITSPLATZ] Ortsbindung abgelaufen - Noki bewegt sich wieder normal");
        }
        let aufgabe = ARBEITSPLATZ_AUFGABE.load(Ordering::Relaxed);
        let gebunden = ARBEITSPLATZ_AUFGABE_SPACE.load(Ordering::Relaxed);
        let ziel_space = match (aufgabe, arbeitsplatz_id) {
            (true, _) if gebunden != 0 => gebunden,
            (true, Some(a)) if a != 0 => a,
            _ => sid,
        };
        let heim = NOKI_SPACE.load(Ordering::Relaxed);
        if heim == 0 {
            let sp = cgs::spaces_des_fensters(wid).unwrap_or_default();
            NOKI_SPACE.store(
                if sp.contains(&sid) {
                    sid
                } else {
                    sp.first().copied().unwrap_or(sid)
                },
                Ordering::Relaxed,
            );
            continue;
        }
        if heim == ziel_space {
            continue;
        }
        // Abschnitt 8: laeuft bereits ein Umzug zu genau diesem Ziel, ist
        // dieses Ereignis sein Echo und kein zweiter Auftrag.
        if UMZUG_ZIEL.load(Ordering::SeqCst) == ziel_space {
            continue;
        }
        UMZUG_ZIEL.store(ziel_space, Ordering::SeqCst);
        eprintln!("[WAECHTER] umzug heim={heim} -> ziel={ziel_space} wid={wid} aufgabe={aufgabe}");

        // Ask Noki's z-order is NOT touched by a Space change: macOS keeps
        // the per-Space window order itself as long as nobody changes Ask's
        // level (see ask_ebene_einrichten). Forcing it to the front on the
        // way back made it jump above windows it was behind before.

        let dir = cgs::space_reihenfolge()
            .and_then(|v| {
                let pos_sid = v.iter().position(|x| *x == ziel_space)?;
                let pos_heim = v.iter().position(|x| *x == heim)?;
                Some(if pos_sid > pos_heim { 1 } else { -1 })
            })
            .unwrap_or(if sid > heim { 1 } else { -1 });

        let epoch = SPACE_EPOCH.fetch_add(1, Ordering::SeqCst) + 1;
        SPACE_BEREIT.store(false, Ordering::Relaxed);
        // Die Miniatur darf auf Nokis eigenem Schreibtisch NIE zu sehen
        // sein - auch nicht fuer ein paar Bilder. Sie haengt im selben
        // Overlay wie die Figur und fliegt sonst mit ihr dort ein. Deshalb
        // wird das Ziel schon HIER mitgeteilt, vor dem Umzug: die native
        // Seite wartet ohnehin auf die Rueckmeldung der Oberflaeche, und in
        // dieser Pause kann sie die Flaeche wegnehmen. Ein nachtraegliches
        // Verbergen waere immer einen Wimpernschlag zu spaet.
        let _ = app.emit(
            "noki://space",
            serde_json::json!({
                "phase": "vorbereiten", "richtung": dir, "space": ziel_space,
                "epoch": epoch,
                "ziel_arbeitsplatz": arbeitsplatz_id == Some(ziel_space)
            }),
        );
        for _ in 0..40 {
            if SPACE_BEREIT.load(Ordering::Relaxed) {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(10));
        }

        let h = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(w) = h.get_webview_window(FENSTER) {
                // Das WebView zeichnet nicht, solange es auf einem unsichtbaren
                // Space liegt. Am Ziel zeigte WindowServer deshalb ein Bild
                // lang den ALTEN Inhalt - samt Miniatur (gemessen: Aufnahme
                // v9, Bild 65). Auf JEDEM Schreibtisch darf dieses Bild nie
                // auftauchen: unsichtbar umziehen, sichtbar erst nach dem ersten
                // frischen Bild am Ziel (noki_space_gezeichnet).
                overlay_verdecken(&h, &w);
                space_umziehen(&w, wid, ziel_space, typ);
            }
            NOKI_SPACE.store(ziel_space, Ordering::Relaxed); // neuer Aufenthaltsort
            UMZUG_ZIEL.store(0, Ordering::SeqCst);

            let _ = h.emit(
                "noki://space",
                serde_json::json!({ "phase": "eintreten", "richtung": dir, "space": sid, "epoch": epoch }),
            );
            sicht_menue(&h);
        });
        thread::sleep(std::time::Duration::from_millis(120)); // kurze Reaktionszeit fuer schnelle Wischgesten
    });
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

/// "Noki zeigen" auf einem Schreibtisch: das Fenster DIREKT in den aktiven
/// Schreibtisch holen (kein Transfer, kein Sprung zum alten Space).
#[cfg(target_os = "macos")]
fn space_holen(win: &WebviewWindow) {
    let (Some((sid, typ)), Some(wid)) = (cgs::aktiver_space(), fenster_nummer(win)) else {
        return;
    };
    let schon = cgs::spaces_des_fensters(wid).map_or(false, |v| v.len() == 1 && v[0] == sid);
    if !schon {
        space_umziehen(win, wid, sid, typ);
    }
    NOKI_SPACE.store(sid, Ordering::Relaxed);
}

/// Fenster in GENAU diesen Space verlegen. Vollbild-Flags nur fuer ein
/// Vollbild-Ziel; jede andere Mitgliedschaft wird entfernt (kein Klon).
#[cfg(target_os = "macos")]
fn space_umziehen(win: &WebviewWindow, wid: i64, sid: u64, typ: i32) {
    let voll = typ == 4;
    if voll {
        vollbild_bit(win, true);
    }
    cgs::hinzufuegen(wid, sid);
    let andere: Vec<u64> = cgs::spaces_des_fensters(wid)
        .unwrap_or_default()
        .into_iter()
        .filter(|s| *s != sid)
        .collect();
    if !andere.is_empty() {
        cgs::entfernen(wid, &andere);
    }
    if !voll {
        vollbild_bit(win, false);
    }
    lage_anwenden(win, &win.state::<Lage>()); // Vollbild: vorne; Schreibtisch: Nutzerwahl
}

// =====================================================================
//  GLOBAL VISIBILITY (Double-Control)
//
//  First Double-Control: remember which Noki surfaces are visible, then
//  hide ALL of them - character window (with every panel drawn in it:
//  settings, Ablage, timer, clipboard ...), Ask Noki, the window overview
//  and the Miniatur (its temporary suppression, never its preference or
//  target). Nothing is closed, reset or terminated; Noki's own state keeps
//  running. Second Double-Control: exactly that snapshot comes back - a
//  surface that was hidden before stays hidden.
// =====================================================================
static GLOBAL_VERBORGEN: AtomicBool = AtomicBool::new(false);
/// (character window, Ask Noki, window overview) visible before hiding.
static GLOBAL_SCHNAPPSCHUSS: Mutex<(bool, bool, bool)> = Mutex::new((false, false, false));

pub(crate) fn global_verborgen() -> bool {
    GLOBAL_VERBORGEN.load(Ordering::Relaxed)
}

fn global_sicht_umschalten(app: &tauri::AppHandle) {
    let sichtbar = |label: &str| app.get_webview_window(label).is_some_and(|w| w.is_visible().unwrap_or(false));
    if !GLOBAL_VERBORGEN.load(Ordering::SeqCst) {
        let snap = (sichtbar(FENSTER), sichtbar(ASK_FENSTER), window_overview::is_open() && sichtbar(window_overview::LABEL));
        if let Ok(mut g) = GLOBAL_SCHNAPPSCHUSS.lock() { *g = snap; }
        GLOBAL_VERBORGEN.store(true, Ordering::SeqCst);
        // Miniatur first (its page lives in the character window).
        let _ = app.emit("noki://global_sicht", serde_json::json!({ "weg": true }));
        // Noki Einstellungen ist jetzt ein eigenes Fenster: gleiche Regel.
        EINST_GLOBAL.store(sichtbar(EINST_FENSTER), Ordering::SeqCst);
        for label in [ASK_FENSTER, window_overview::LABEL, EINST_FENSTER, FENSTER] {
            if let Some(w) = app.get_webview_window(label) { let _ = w.hide(); }
        }
        virtual_workspace::trace(&format!(
            "[SICHT] global hide snapshot noki={} ask={} overview={}", snap.0, snap.1, snap.2));
        if snap.0 { sicht_spur("double_control_hide", "visible", "hidden", "native"); }
    } else {
        let snap = GLOBAL_SCHNAPPSCHUSS.lock().map(|g| *g).unwrap_or((true, false, false));
        GLOBAL_VERBORGEN.store(false, Ordering::SeqCst);
        if snap.0 {
            if let Some(w) = app.get_webview_window(FENSTER) {
                // Back on the Space the user stands on NOW (he may have
                // swiped while hidden), in its own layer.
                #[cfg(target_os = "macos")]
                space_holen(&w);
                let _ = w.show();
            }
        }
        if snap.1 {
            if let Some(w) = app.get_webview_window(ASK_FENSTER) { let _ = w.show(); }
        }
        if snap.2 {
            window_overview::global_zeigen(app);
        }
        let _ = app.emit("noki://global_sicht", serde_json::json!({ "weg": false }));
        virtual_workspace::trace(&format!(
            "[SICHT] global restore noki={} ask={} overview={}", snap.0, snap.1, snap.2));
        if snap.0 { sicht_spur("double_control_restore", "hidden", "visible", "native"); }
        if EINST_GLOBAL.swap(false, Ordering::SeqCst) {
            if let Some(w) = app.get_webview_window(EINST_FENSTER) { let _ = w.show(); }
        }
    }
}
static EINST_GLOBAL: AtomicBool = AtomicBool::new(false);

/// Ab App-Start: Vollbild-Zuordnung abgleichen und Menuetext nachfuehren
/// (Space-/Vollbild-Wechsel, Show/Hide von aussen). Ein Faden, 0,7 s.
fn sicht_waechter(app: tauri::AppHandle) {
    thread::spawn(move || loop {
        thread::sleep(std::time::Duration::from_millis(1000));
        let h2 = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(w) = h2.get_webview_window(FENSTER) {
                if NOKI_GEZEIGT.load(Ordering::Relaxed)
                    && !NUTZER_VERBORGEN.load(Ordering::Relaxed)
                    && !global_verborgen()
                    && !w.is_visible().unwrap_or(true)
                {
                    let _ = w.show();
                    eprintln!("[SICHT] Noki war ohne 'Noki verbergen' unsichtbar - wieder gezeigt");
                }
                let _ = vollbild_space_abgleich(&w);
                // Vordergrund heisst Vordergrund. Ein Programmwechsel oder
                // ein Space-Umzug ordnet Nokis Fenster innerhalb seiner
                // Ebene wieder nach hinten; dann schiebt sich ein Fenster
                // davor. Waehrend eines Durchgangs (Noki laeuft ABSICHTLICH
                // hinter einem Fenster entlang) bleibt das aus.
                // Also in native full screen and during a Durchgang (the
                // native layer no longer goes back for it): visible => front.
                noki_vorn_sichern(&w);
            }
            sicht_menue(&h2);
        });
    });
}

fn ort_merken(app: &tauri::AppHandle, win: &WebviewWindow) {
    let (Some(datei), Ok(pos)) = (ort_datei(app), win.outer_position()) else {
        return;
    };
    let _ = fs::write(
        datei,
        serde_json::json!({ "x": pos.x, "y": pos.y }).to_string(),
    );
}

fn ort_lesen(app: &tauri::AppHandle) -> Option<PhysicalPosition<i32>> {
    let datei = ort_datei(app)?;
    let roh = fs::read_to_string(datei).ok()?;
    let v: serde_json::Value = serde_json::from_str(&roh).ok()?;
    Some(PhysicalPosition::new(
        v.get("x")?.as_i64()? as i32,
        v.get("y")?.as_i64()? as i32,
    ))
}

/// Ein Bildschirm: linke obere Ecke und Groesse, beides in Bildpunkten.
type Schirm = ((i32, i32), (i32, i32));

/// Zieht einen Ort in den sichtbaren Bereich.
///
/// Liegt die Fenstermitte noch auf einem Bildschirm, wird nur so weit
/// geklemmt, dass das Fenster ganz darauf passt. Liegt sie auf keinem mehr —
/// weil ein Bildschirm abgezogen oder die Aufloesung geaendert wurde —, wird
/// der naechstgelegene Bildschirm genommen. Rueckgabe None heisst: kein
/// Bildschirm bekannt, dann bleibt der Ort unangetastet.
///
/// Bewusst als reine Funktion, ohne Fenster: nur so laesst sich genau der
/// Fall pruefen, um den es geht — ein Ort, den es nicht mehr gibt.
fn ort_in_sicht(p: (i32, i32), fenster: (i32, i32), schirme: &[Schirm]) -> Option<(i32, i32)> {
    if schirme.is_empty() {
        return None;
    }
    let (bw, bh) = fenster;
    let mitte = (p.0 + bw / 2, p.1 + bh / 2);
    let mut bester: Option<(i64, (i32, i32))> = None;

    for &((l, o), (mw, mh)) in schirme {
        let (r, u) = (l + mw, o + mh);
        let x = p.0.clamp(l, (r - bw).max(l));
        let y = p.1.clamp(o, (u - bh).max(o));

        // Mitte noch auf diesem Bildschirm? Dann bleibt der Ort, wie er ist.
        if mitte.0 >= l && mitte.0 < r && mitte.1 >= o && mitte.1 < u {
            return Some((x, y));
        }

        // Sonst den naechstgelegenen merken.
        let dx = (mitte.0 - (x + bw / 2)) as i64;
        let dy = (mitte.1 - (y + bh / 2)) as i64;
        let d = dx * dx + dy * dy;
        if bester.as_ref().map(|(bd, _)| d < *bd).unwrap_or(true) {
            bester = Some((d, (x, y)));
        }
    }

    bester.map(|(_, p)| p)
}

fn ort_klemmen(win: &WebviewWindow, p: PhysicalPosition<i32>) -> Option<PhysicalPosition<i32>> {
    let groesse = win.outer_size().ok()?;
    let schirme: Vec<Schirm> = win
        .available_monitors()
        .ok()?
        .iter()
        .map(|m| {
            (
                (m.position().x, m.position().y),
                (m.size().width as i32, m.size().height as i32),
            )
        })
        .collect();
    let (x, y) = ort_in_sicht(
        (p.x, p.y),
        (groesse.width as i32, groesse.height as i32),
        &schirme,
    )?;
    Some(PhysicalPosition::new(x, y))
}

// =====================================================================
//  5 · BEFEHLE AUS DEM FRONTEND
// =====================================================================
/// Sichtbarkeitsebene des Panels (Abschnitt 14).
///
///   "vorn"    – ueber gewoehnlichen Fenstern
///   "normal"  – wie jedes andere Fenster, kann verdeckt werden
///   "hinten"  – hinter gewoehnlichen Fenstern, dicht am Schreibtisch
struct Lage {
    ebene: Mutex<&'static str>,
    /// Abschnitt 21: waehrend Noki HINTER einem fremden Fenster
    /// entlanglaeuft, wird das Panel kurz nach hinten gelegt, damit das
    /// Fenster ihn wirklich verdeckt. Die vom Nutzer gewaehlte Ebene
    /// bleibt in `ebene` stehen und wird danach wiederhergestellt — die
    /// Menueauswahl wird also nie ueberschrieben, nur voruebergehend
    /// ueberlagert.
    durchgang: AtomicBool,
    /// Abschnitt 29: sichtbar auf allen Schreibtischen. Steht bewusst
    /// NEBEN der Ebene, nicht darin — die beiden haben nichts
    /// miteinander zu tun.
    alle_spaces: AtomicBool,
}

/// Setzt die tatsaechliche Fensterebene: Durchgang schlaegt die gewaehlte
/// Ebene, sonst gilt die Auswahl aus der Menueleiste.
/// Timer-Parade: echte Figur + Effekt-Klone liegen fuer die Dauer der Parade
/// gemeinsam auf der schwebenden Ebene ueber normalen App-Fenstern. Genau
/// EIN Besitzer (noki_parade_ebene); jede andere Ebenen-Anwendung waehrend
/// der Parade (Einstellungen schliessen, Durchgang ...) respektiert das.
static PARADE_EBENE: AtomicBool = AtomicBool::new(false);

/// Ein/aus der temporaeren Parade-Ebene. Kein Fokus, keine Aktivierung
/// (orderFrontRegardless), Klicks gehen weiter durch (Trefferzone bleibt
/// Sache des Frontends). Aus = sofort wieder die Nutzerwahl.
#[tauri::command]
fn noki_parade_ebene(app: tauri::AppHandle, aktiv: bool) {
    PARADE_EBENE.store(aktiv, Ordering::Relaxed);
    let h = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(win) = h.get_webview_window(FENSTER) {
            lage_anwenden(&win, &h.state::<Lage>());
        }
    });
}

fn lage_anwenden(win: &WebviewWindow, lage: &Lage) {
    if PARADE_EBENE.load(Ordering::Relaxed) {
        ebene_setzen(win, "vorn");
        return;
    }
    // Die NATIVE Ebene folgt nur der Nutzerwahl ("Ansicht") und ist im
    // Vollbild immer vorne. Vor/hinter einem Fenster ist allein Nokis eigene
    // Tiefe im Overlay (Maskierung) — das ganze Fenster wird dafuer NIE mehr
    // nach hinten gelegt (vorher verschwand Noki so hinter jeder App).
    if im_vollbild() {
        ebene_setzen(win, "vorn");
        return;
    }
    ebene_setzen(win, *lage.ebene.lock().unwrap());
}

fn im_vollbild() -> bool {
    #[cfg(target_os = "macos")]
    {
        cgs::aktiver_space().map_or(false, |(_, t)| t == 4)
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Ein sichtbares Fenster innerhalb SEINER Ebene ganz nach vorn ordnen.
///
/// `set_always_on_top` setzt nur das NSWindow-Level (Floating). Das reicht
/// gegenueber gewoehnlichen Fenstern — aber nach einem Space-Umzug oder
/// einem Programmwechsel steht Nokis Fenster innerhalb dieser Ebene
/// trotzdem hinten, und dann schiebt sich ein Fenster davor. `orderFront:`
/// waere an die Aktivierung des eigenen Programms gebunden; Noki soll aber
/// nach vorn kommen, OHNE die Anwendung des Nutzers zu unterbrechen —
/// genau dafuer gibt es `orderFrontRegardless`.
///
/// Bewusst NICHT ScreenSaver-/Status-Level: Systemdialoge, Passwortfenster
/// und die Menueleiste liegen ueber der Floating-Ebene und bleiben es.
#[cfg(target_os = "macos")]
fn nach_vorn_holen(win: &WebviewWindow) {
    use std::ffi::c_void;
    #[link(name = "objc")]
    extern "C" {
        fn sel_registerName(n: *const std::os::raw::c_char) -> *const c_void;
        fn objc_msgSend();
    }
    if !win.is_visible().unwrap_or(false) {
        return; // ein verborgenes Fenster nach vorn zu holen hiesse: zeigen
    }
    let Ok(nw) = win.ns_window() else { return };
    unsafe {
        let f: unsafe extern "C" fn(*mut c_void, *const c_void) =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(
            nw,
            sel_registerName(b"orderFrontRegardless\0".as_ptr() as *const _),
        );
    }
}

#[cfg(not(target_os = "macos"))]
fn nach_vorn_holen(_: &WebviewWindow) {}

/// VISIBLE => FRONTMOST (2026-10-03). Only while Noki is visible (a hidden
/// Noki stays hidden - nothing here shows a window) and the effective layer
/// is "vorn" (user choice, native full screen or the timer round): the
/// floating level is re-asserted if anything lowered it, then the window is
/// ordered front with orderFrontRegardless - no activation, the user's app
/// keeps keyboard focus. Called on events (Space change, focus session
/// start/end) and from the existing 1 s visibility watcher; no new loop.
/// A "Durchgang" no longer changes the native layer (Noki's depth is his
/// own masking), so it does not block this either.
fn noki_vorn_sichern(win: &WebviewWindow) {
    if !win.is_visible().unwrap_or(false) || global_verborgen() || !NOKI_GEZEIGT.load(Ordering::Relaxed) {
        return;
    }
    let lage = win.state::<Lage>();
    let vorn = PARADE_EBENE.load(Ordering::Relaxed) || im_vollbild() || *lage.ebene.lock().unwrap() == "vorn";
    if !vorn {
        return; // the user explicitly chose "In den Hintergrund"
    }
    #[cfg(target_os = "macos")]
    {
        use std::ffi::c_void;
        #[link(name = "objc")]
        extern "C" {
            fn sel_registerName(n: *const std::os::raw::c_char) -> *const c_void;
            fn objc_msgSend();
        }
        if let Ok(nw) = win.ns_window() {
            let level: i64 = unsafe {
                let f: unsafe extern "C" fn(*mut c_void, *const c_void) -> i64 =
                    std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                f(nw, sel_registerName(b"level\0".as_ptr() as *const _))
            };
            // 3 = NSFloatingWindowLevel; Freeze (23) stays as it is.
            if level < 3 {
                virtual_workspace::trace(&format!("NOKI_FRONT level_repaired from={level}"));
                ebene_setzen(win, "vorn");
                return;
            }
        }
    }
    nach_vorn_holen(win);
}
fn noki_vorn_sichern_app(app: &tauri::AppHandle) {
    let h = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(w) = h.get_webview_window(FENSTER) { noki_vorn_sichern(&w); }
    });
}

fn ebene_setzen(win: &WebviewWindow, ebene: &str) {
    match ebene {
        "vorn" => {
            let _ = win.set_always_on_bottom(false);
            let _ = win.set_always_on_top(true);
            nach_vorn_holen(win);
        }
        "hinten" => {
            let _ = win.set_always_on_top(false);
            let _ = win.set_always_on_bottom(true);
        }
        _ => {
            let _ = win.set_always_on_top(false);
            let _ = win.set_always_on_bottom(false);
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub struct SchirmInfo {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub scale: f64,
    // Sichtbare Arbeitsflaeche desselben Bildschirms, ebenfalls in
    // logischen Punkten und in denselben Schreibtischkoordinaten wie
    // x/y. Unter macOS ist das die visibleFrame: ohne Menueleiste und
    // ohne Dock. Daraus leitet das Frontend WALK_Y ab — Noki laeuft auf
    // dem sichtbaren Schreibtisch, nicht hinter dem Dock.
    pub ax: i32,
    pub ay: i32,
    pub aw: i32,
    pub ah: i32,
}

/// Liefert Information ueber den aktuellen Bildschirm in logischen Punkten.
#[tauri::command]
fn noki_schirm_info(app: tauri::AppHandle) -> Option<SchirmInfo> {
    let win = app.get_webview_window(FENSTER)?;
    let monitor = win
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| win.primary_monitor().ok().flatten())?;
    let pos = monitor.position();
    let size = monitor.size();
    let scale = monitor.scale_factor();
    let arbeit = monitor.work_area();
    Some(SchirmInfo {
        x: (pos.x as f64 / scale).round() as i32,
        y: (pos.y as f64 / scale).round() as i32,
        w: (size.width as f64 / scale).round() as i32,
        h: (size.height as f64 / scale).round() as i32,
        scale,
        ax: (arbeit.position.x as f64 / scale).round() as i32,
        ay: (arbeit.position.y as f64 / scale).round() as i32,
        aw: (arbeit.size.width as f64 / scale).round() as i32,
        ah: (arbeit.size.height as f64 / scale).round() as i32,
    })
}

/// Das Frontend meldet, dass Noki steht. Erst dann wird das Fenster
/// sichtbar — sonst blitzt beim Start ein leeres Panel auf.
/// Rueckgabe: (x, y) der linken oberen Fensterecke in logischen Punkten.
#[tauri::command]
fn noki_bereit(app: tauri::AppHandle) -> Option<(i32, i32)> {
    let win = app.get_webview_window(FENSTER)?;
    let monitor = win
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| win.primary_monitor().ok().flatten())?;
    let scale = monitor.scale_factor();
    let m_pos = monitor.position();
    let m_size = monitor.size();

    // Nur das transparente Character-Fenster deckt den Desktop ab. Ask
    // Noki bleibt ein separates, normales NSWindow.
    let _ = win.set_position(*m_pos);
    let _ = win.set_size(*m_size);
    let _ = win.set_decorations(false);
    let _ = win.set_shadow(false);
    let _ = win.set_resizable(false);
    // Ein ausdruecklich gespeichertes "Noki verbergen" gilt auch nach dem
    // Neustart. Ohne Wunsch (Normalfall) erscheint er wie bisher.
    if !NUTZER_VERBORGEN.load(Ordering::Relaxed) {
        let _ = win.show();
        NOKI_GEZEIGT.store(true, Ordering::Relaxed);
    }
    overlay_durchlaessig(&win, true);
    // Die gewaehlte Ebene gilt ab dem ersten Bild. Ohne das stand sie bis
    // zum ersten Tiefenwechsel nur in tauri.conf.json — und der erste
    // noki_ebene_durchgang(false) setzte sie dann auf den damaligen
    // Grundwert herunter.
    lage_anwenden(&win, &app.state::<Lage>());
    alle_spaces_anwenden(
        &win,
        app.state::<Lage>().alle_spaces.load(Ordering::Relaxed),
    );

    #[cfg(target_os = "macos")]
    if virtual_workspace::backend() != virtual_workspace::Backend::LegacyVirtualDisplay {
        let ap_id = ARBEITSPLATZ.lock().ok().and_then(|g| g.as_ref().map(|r| r.id));
        let sid = cgs::aktiver_space().map(|(a, _)| a).unwrap_or(0);
        let auf_ap = ap_id.is_some() && ap_id == Some(sid);
        let nummer = ap_id.and_then(|id| {
            cgs::desktops().and_then(|(_, liste)| arbeitsplatz::sichtbare_nummer(&liste, id))
        });
        let _ = app.emit("noki://arbeitsplatz_aktiv", serde_json::json!({
            "aktiv": auf_ap,
            "space": sid,
            "arbeitsplatz": ap_id,
            "nummer": nummer
        }));
    }

    if let Some(pos) = ort_lesen(&app) {
        let log_x = (pos.x as f64 / scale).round() as i32;
        let log_y = (pos.y as f64 / scale).round() as i32;
        eprintln!(
            "[RUST] noki_bereit returning saved pos: ({}, {}), alle Schreibtische: {}",
            log_x,
            log_y,
            app.state::<Lage>().alle_spaces.load(Ordering::Relaxed)
        );
        Some((log_x, log_y))
    } else {
        let def_x = (m_size.width as f64 / scale * 0.5).round() as i32;
        let def_y = (m_size.height as f64 / scale - 14.0).round() as i32;
        eprintln!(
            "[RUST] noki_bereit returning default pos: ({}, {})",
            def_x, def_y
        );
        Some((def_x, def_y))
    }
}

#[tauri::command]
fn noki_panel_zonen(state: tauri::State<NokiHitStore>, zonen: Vec<[i32; 4]>) {
    if let Ok(mut hit) = state.0.write() {
        hit.zonen = zonen.into_iter().filter(|z| z[2] > 0 && z[3] > 0).collect();
    }
}
#[tauri::command]
fn noki_hit_zone(state: tauri::State<NokiHitStore>, x: i32, y: i32, w: i32, h: i32) {
    if let Ok(mut hit) = state.0.write() {
        hit.x = x;
        hit.y = y;
        hit.w = w;
        hit.h = h;
    }
}

/// Design corner of the compact Miniatur (CSS #nokiVorschau: left 7px,
/// bottom 22px on the full main display).
const MINIATUR_RAND_LINKS: i32 = 7;
const MINIATUR_RAND_UNTEN: i32 = 22;

/// THE position of the compact Miniatur: computed from the main display's
/// bounds every single time - never from the DOM rect, a previous frame, an
/// offset or a transition. The DOM rect drifted with CSS transitions and
/// stage offsets during Space/fullscreen changes (Miniatur slightly up/right).
fn miniatur_ecke(h: i32) -> (i32, i32) {
    let (dx, dy, _dw, dh) = anzeige_flaeche();
    (dx + MINIATUR_RAND_LINKS, dy + dh - MINIATUR_RAND_UNTEN - h)
}

/// Klickflaeche der Arbeitsplatz-Vorschau. w/h = 0 schaltet sie ab.
#[tauri::command]
fn noki_vorschau_zone(
    app: tauri::AppHandle,
    state: tauri::State<NokiHitStore>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    halten: Option<bool>,
    auf_noki: Option<bool>,
) {
    // Das Rechteck stellt NUR das Anzeigefenster - es ist KEINE Trefferzone
    // des Character-Overlays mehr.
    //
    // Die Miniatur ist ein eigenes Fenster des Helfers und faengt ihre
    // Zeiger selbst (Tracking-Bereich fuer Hover, mouseDown fuer den Klick).
    // Das Overlay liegt jedoch eine Ebene darueber: solange es dieselbe
    // Flaeche als Trefferzone meldete, wurde es dort undurchlaessig und
    // schluckte jeden Klick auf die Miniatur - und weil sein eigenes
    // Vorschau-Element laengst unsichtbar ist, geschah gar nichts. Genau
    // daran scheiterte "Klick oeffnet Nokis Schreibtisch".
    // Only the SIZE comes from the page; the corner is canonical.
    let (x, y) = if w > 0 && h > 0 {
        let (kx, ky) = miniatur_ecke(h);
        if (kx, ky) != (x, y) {
            static SPUR: Mutex<Option<std::time::Instant>> = Mutex::new(None);
            if SPUR.lock().is_ok_and(|mut t| { let f = t.is_none_or(|t| t.elapsed() > Duration::from_secs(10)); if f { *t = Some(std::time::Instant::now()); } f }) {
                virtual_workspace::trace(&format!(
                    "[MINIATUR_POS] dom_rect={x},{y} canonical={kx},{ky} size={w}x{h} used=canonical"));
            }
        }
        (kx, ky)
    } else {
        (x, y)
    };
    if let Ok(mut hit) = state.0.write() {
        hit.vx = 0;
        hit.vy = 0;
        hit.vw = 0;
        hit.vh = 0;
        // ... ihre wirkliche Lage wird trotzdem gemerkt: der Beobachter
        // "Druck neben die grosse Miniatur" braucht sie, und nur er.
        hit.fx = x;
        hit.fy = y;
        hit.fw = w;
        hit.fh = h;
    }
    // Das Anzeigefenster folgt demselben Rechteck. `halten`: nur auf Nokis
    // Schreibtisch unterdrueckt - das Fenster bleibt auf den Nutzer-
    // Schreibtischen stehen (dort ist es Mitglied, hier nicht) und ist beim
    // Zurueckwischen schon da, statt erst nachzuladen.
    if !(w == 0 && halten == Some(true)) {
        vorschau::rahmen(x, y, w, h);
    }
    VORSCHAU_AUF_NOKI.store(w > 0 && auf_noki == Some(true), Ordering::Relaxed);
    vorschau_spaces_richten(&app);
}

/// Will der Nutzer die Miniatur AUSDRUECKLICH auf Nokis Schreibtisch sehen
/// (Kuerzel 4 dort)? Nur diese Aussage der Oberflaeche nimmt Nokis
/// Schreibtisch in die Mitgliedschaft auf - nie eine Messung, nie ein
/// Space-Wechsel. Sonst koennte ein Wechsel-Ereignis die Miniatur genau
/// dorthin legen, wo sie nie auftauchen darf.
static VORSCHAU_AUF_NOKI: AtomicBool = AtomicBool::new(false);
/// Ein echter Desktop-Wechsel seit dem letzten Space-Abgleich.
static SPACE_GEWECHSELT: AtomicBool = AtomicBool::new(true);

/// Mitgliedschaft des Anzeigefensters: jeder normale Schreibtisch AUSSER
/// Nokis eigenem sowie native Vollbild-Spaces. Vollbild ist ein HOST fuer
/// die Miniatur (`fullScreenAuxiliary`), aber niemals ein Preview-Kandidat.
/// Nur eine ausdrueckliche Nutzerwahl verbirgt die Miniatur global.
fn vorschau_spaces_richten(app: &tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    {
        if virtual_workspace::backend() == virtual_workspace::Backend::LegacyVirtualDisplay {
            // This function is event-bound to physical Desktop changes.
            // Re-send even if the desired list is unchanged so the helper
            // can repair WindowServer membership without touching the
            // user's explicit VISIBLE/HIDDEN preference.
            // Erzwungen nur nach einem ECHTEN Desktop-Wechsel. Gemessen: jede
            // Hover-Aenderung der Miniatur kam ueber die Zonenmeldung der
            // Oberflaeche hier an und liess den Helfer synchron WindowServer
            // nach Spaces fragen - bei ausgelastetem WindowServer startete die
            // Hover-Animation dadurch bis zu 1,9 s zu spaet.
            if SPACE_GEWECHSELT.swap(false, Ordering::Relaxed) {
                vorschau::spaces_erzwingen(vorschau_spaces(0, true));
            } else {
                vorschau::spaces(vorschau_spaces(0, true));
            }
            vorschau::noki_space(0);
            vorschau::ort("nutzer", false, false);
            let _ = app;
            return;
        }
        let arbeitsplatz = get_preview_target().map(|t| t.space_id)
            .or_else(|| ARBEITSPLATZ.lock().ok().and_then(|g| g.as_ref().map(|r| r.id))).unwrap_or(0);
        let mut aktiv = LETZT_SID.load(Ordering::Relaxed);
        if aktiv == 0 {
            aktiv = cgs::aktiver_space().map(|(a, _)| a).unwrap_or(0);
        }
        let dort_gewollt = aktiv == arbeitsplatz && VORSCHAU_AUF_NOKI.load(Ordering::Relaxed);
        if aktiv != arbeitsplatz {
            VORSCHAU_AUF_NOKI.store(false, Ordering::Relaxed);
        }
        vorschau::spaces(vorschau_spaces(arbeitsplatz, false));
        vorschau::noki_space(arbeitsplatz);
        vorschau_ort_melden(arbeitsplatz, aktiv, dort_gewollt);
    }
    let _ = app;
}

/// Ort an den Helfer. Nokis Schreibtisch sofort (die Miniatur muss dort
/// weg); ein Nutzer-Schreibtisch erst, wenn er SICHTBAR erreicht ist - sonst
/// tauschte die hereingleitende Miniatur mitten in der Animation ihr Fenster.
#[cfg(target_os = "macos")]
fn vorschau_ort_melden(arbeitsplatz: u64, aktiv: u64, dort_gewollt: bool) {
    let ordnung = cgs::space_reihenfolge().unwrap_or_default();
    let i = ordnung.iter().position(|x| *x == aktiv);
    let links = i.and_then(|i| i.checked_sub(1)).and_then(|j| ordnung.get(j)) == Some(&arbeitsplatz);
    let rechts = i.and_then(|i| ordnung.get(i + 1)) == Some(&arbeitsplatz);
    if aktiv == arbeitsplatz && arbeitsplatz != 0 {
        // Der verdeckte Fernvorgang steht nur hinter der Blende dort; fuer
        // den Nutzer bleibt die Miniatur auf seinem Schreibtisch.
        if vorschau::fern_beschaeftigt() {
            return;
        }
        vorschau::ort(if dort_gewollt { "noki_gewollt" } else { "noki" }, false, false);
        return;
    }
    thread::spawn(move || {
        for _ in 0..4 {
            if cgs::nur_ziel_sichtbar(aktiv) {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
        // Inzwischen weitergewischt? Dann gilt die neuere Meldung.
        if cgs::aktiver_space().map(|(a, _)| a) == Some(aktiv) {
            vorschau::ort("nutzer", links, rechts);
        }
    });
}

#[cfg(target_os = "macos")]
fn vorschau_spaces(arbeitsplatz: u64, mit_arbeitsplatz: bool) -> Vec<u64> {
    cgs::space_reihenfolge()
        .unwrap_or_default()
        .into_iter()
        // Hosting and cycling are intentionally different sets. Native
        // fullscreen Spaces host the auxiliary panel when SHOWN, but the
        // cycling logic in `arbeitsplatz::naechster_arbeitsplatz` continues
        // to admit type-0 Desktops only.
        .filter(|s| cgs::space_typ(*s).is_some_and(|typ|
            vorschau_space_host(typ, *s, arbeitsplatz, mit_arbeitsplatz)))
        .collect()
}

#[cfg(target_os = "macos")]
fn vorschau_space_host(typ: i32, id: u64, arbeitsplatz: u64, mit_arbeitsplatz: bool) -> bool {
    match typ {
        arbeitsplatz::TYP_SCHREIBTISCH =>
            mit_arbeitsplatz || id != arbeitsplatz || arbeitsplatz == 0,
        // Native fullscreen is an overlay host, never a preview-cycle item.
        4 => true,
        _ => false,
    }
}

#[tauri::command]
fn noki_vorschau_label(text: String) {
    vorschau::label(&text);
}

/// Beschriftung des Navigationsknopfes im Fussband der Miniatur.
#[tauri::command]
fn noki_vorschau_navi(text: String) {
    vorschau::navi(&text);
}

#[tauri::command]
fn noki_drag_status(app: tauri::AppHandle, state: tauri::State<NokiHitStore>, aktiv: bool) {
    eprintln!("[RUST] noki_drag_status: aktiv={}", aktiv);
    if let Ok(mut hit) = state.0.write() {
        hit.drag = aktiv;
    }
    if aktiv { DRAG_BESTAETIGT_MS.store(jetzt_epoch_ms(), Ordering::Relaxed); }
    if let Some(win) = app.get_webview_window(FENSTER) {
        if aktiv {
            overlay_durchlaessig(&win, false);
        }
    }
}

/// NOKI_VISIBILITY: eine Zeile je echter Sichtbarkeitsaenderung der Figur
/// (Seite oder nativ). Nutzerwunsch, Space-Darstellung und Fensterlage sind
/// getrennte Felder - ein Space-Ereignis schreibt nie den Nutzerwunsch.
pub(crate) fn sicht_spur(grund: &str, vorher: &str, nachher: &str, quelle: &str) {
    crate::virtual_workspace::trace(&format!(
        "NOKI_VISIBILITY reason={grund} user_visibility={} space={} source={quelle} before={vorher} after={nachher}",
        if NUTZER_VERBORGEN.load(Ordering::Relaxed) { "hidden" } else { "shown" },
        NOKI_SPACE.load(Ordering::Relaxed)
    ));
}

#[tauri::command]
fn noki_sicht_spur(grund: String, vorher: String, nachher: String, nutzer: Option<String>) {
    sicht_spur(&grund, &vorher, &nachher, &format!("page(versteckt={})", nutzer.unwrap_or_default()));
}

#[tauri::command]
fn noki_log(msg: String) {
    eprintln!("[FRONTEND LOG] {}", msg);
    if msg.starts_with("INPUT_BLOCK_STATE") || msg.starts_with("UI_BUSY") { virtual_workspace::trace(&msg); }
}

/// Laeuft gerade der Timer-Schwarm? Gesetzt vom Frontend; der Event-Tap
/// beendet ihn dann mit der ersten echten Taste (Kuerzel bleiben still).
static SCHWARM_AKTIV: AtomicBool = AtomicBool::new(false);
#[tauri::command]
fn noki_schwarm(an: bool) {
    SCHWARM_AKTIV.store(an, Ordering::SeqCst);
}

/// Thermischer Zustand des Mac, oeffentlich und ohne sudo
/// (NSProcessInfo.thermalState): 0 nominal, 1 fair, 2 serious, 3 critical.
/// Die Autonomie liest das selten (alle paar Sekunden), nie je Bild.
#[tauri::command]
fn noki_thermal() -> i64 {
    use objc2::{msg_send, runtime::{AnyClass, AnyObject}};
    let Some(k) = AnyClass::get(c"NSProcessInfo") else { return 0 };
    unsafe {
        let info: *mut AnyObject = msg_send![k, processInfo];
        if info.is_null() { return 0; }
        let z: isize = msg_send![info, thermalState];
        z as i64
    }
}

/// Fragt oder setzt "auf allen Schreibtischen".
///
/// Ohne Argument wird nur gelesen, mit Argument gesetzt, angewendet und
/// gespeichert. Rueckgabe ist immer der Stand danach.
#[tauri::command]
fn noki_alle_schreibtische(app: tauri::AppHandle, an: Option<bool>) -> bool {
    let lage = app.state::<Lage>();
    if let Some(neu) = an {
        lage.alle_spaces.store(neu, Ordering::Relaxed);
        if let Some(win) = app.get_webview_window(FENSTER) {
            alle_spaces_anwenden(&win, neu);
        }
        alle_spaces_schreiben(&app, neu);
        eprintln!("[RUST] alle Schreibtische: {}", neu);
    }
    lage.alle_spaces.load(Ordering::Relaxed)
}

/// Setzt das Noki-Fenster an eine logische Desktop-Position.
#[tauri::command]
fn noki_fenster_setzen(app: tauri::AppHandle, x: i32, y: i32) -> Option<(i32, i32)> {
    let win = app.get_webview_window(FENSTER)?;
    let scale = win
        .current_monitor()
        .ok()
        .flatten()
        .map(|m| m.scale_factor())
        .unwrap_or(1.0);

    let phys_pos = PhysicalPosition::new(
        (x as f64 * scale).round() as i32,
        (y as f64 * scale).round() as i32,
    );
    let geklemmt = ort_klemmen(&win, phys_pos).unwrap_or(phys_pos);
    let _ = win.set_position(geklemmt);

    let log_x = (geklemmt.x as f64 / scale).round() as i32;
    let log_y = (geklemmt.y as f64 / scale).round() as i32;
    Some((log_x, log_y))
}

/// Verschiebt das Fenster um ein relatives Delta in logischen Punkten.
#[tauri::command]
fn noki_fenster_bewegen(app: tauri::AppHandle, dx: i32, dy: i32) -> Option<(i32, i32)> {
    let win = app.get_webview_window(FENSTER)?;
    let pos = win.outer_position().ok()?;
    let scale = win
        .current_monitor()
        .ok()
        .flatten()
        .map(|m| m.scale_factor())
        .unwrap_or(1.0);

    let phys_dx = (dx as f64 * scale).round() as i32;
    let phys_dy = (dy as f64 * scale).round() as i32;
    let mut neue_pos = PhysicalPosition::new(pos.x + phys_dx, pos.y + phys_dy);
    if let Some(k) = ort_klemmen(&win, neue_pos) {
        neue_pos = k;
    }
    let _ = win.set_position(neue_pos);

    let log_x = (neue_pos.x as f64 / scale).round() as i32;
    let log_y = (neue_pos.y as f64 / scale).round() as i32;
    Some((log_x, log_y))
}

/// Liefert die aktuelle Fensterposition in logischen Punkten.
#[tauri::command]
fn noki_fenster_position_holen(app: tauri::AppHandle) -> Option<(i32, i32)> {
    let win = app.get_webview_window(FENSTER)?;
    let pos = win.outer_position().ok()?;
    let scale = win
        .current_monitor()
        .ok()
        .flatten()
        .map(|m| m.scale_factor())
        .unwrap_or(1.0);
    Some((
        (pos.x as f64 / scale).round() as i32,
        (pos.y as f64 / scale).round() as i32,
    ))
}

/// Schaltet natives Vollbild auf demselben Hauptfenster um. NSWindow merkt
/// dabei Groesse und Position des normalen Fensters und stellt beides beim
/// Verlassen selbst wieder her.
#[tauri::command]
fn noki_toggle_fullscreen(app: tauri::AppHandle, aktiv: Option<bool>) -> Result<bool, String> {
    if let Some(win) = app.get_webview_window(ASK_FENSTER) {
        let ist_voll = win.is_fullscreen().unwrap_or(false);
        let neu = aktiv.unwrap_or(!ist_voll);
        win.set_fullscreen(neu)
            .map_err(|e| format!("Vollbild konnte nicht umgeschaltet werden: {e}"))?;
        eprintln!("[RUST] noki_toggle_fullscreen: {}", neu);
        Ok(neu)
    } else {
        Err("Fenster nicht gefunden".into())
    }
}

/// Liefert den aktuellen Vollbild-Status.
#[tauri::command]
fn noki_fullscreen_status(app: tauri::AppHandle) -> bool {
    app.get_webview_window(ASK_FENSTER)
        .and_then(|win| win.is_fullscreen().ok())
        .unwrap_or(false)
}

/// Speichert den aktuellen Noki-Ort dauerhaft.
#[tauri::command]
fn noki_ort_speichern(app: tauri::AppHandle, x: Option<i32>, y: Option<i32>) {
    let Some(datei) = ort_datei(&app) else {
        return;
    };
    if let (Some(x_val), Some(y_val)) = (x, y) {
        let win = app.get_webview_window(FENSTER);
        let scale = win
            .as_ref()
            .and_then(|w| w.current_monitor().ok().flatten())
            .map(|m| m.scale_factor())
            .unwrap_or(1.0);
        let phys_x = (x_val as f64 * scale).round() as i32;
        let phys_y = (y_val as f64 * scale).round() as i32;
        let _ = fs::write(
            datei,
            serde_json::json!({ "x": phys_x, "y": phys_y }).to_string(),
        );
    } else if let Some(win) = app.get_webview_window(FENSTER) {
        ort_merken(&app, &win);
    }
}

/// Liefert die aktuelle globale Cursorposition auf dem Bildschirm in logischen Punkten.
#[tauri::command]
fn noki_maus_position() -> Option<(f64, f64)> {
    maus_schirm_position()
}

/// Abschnitt 21: Noki laeuft hinter einem Fenster entlang (oder nicht
/// mehr). Nur diese eine Umschaltung — die Ebenenauswahl des Nutzers
/// bleibt unangetastet und gilt sofort wieder, sobald der Durchgang
/// endet. Rueckgabe: die jetzt wirksame Ebene, damit sich das Frontend
/// nicht auf ein stilles Fehlschlagen verlaesst.
#[tauri::command]
fn noki_ebene_durchgang(app: tauri::AppHandle, aktiv: bool) -> Option<String> {
    let win = app.get_webview_window(FENSTER)?;
    let lage = app.state::<Lage>();
    lage.durchgang.store(aktiv, Ordering::Relaxed);
    lage_anwenden(&win, &lage);
    // Rueckgabe ist die NATIVE Ebene — die bleibt unabhaengig vom Durchgang.
    let _ = aktiv;
    Some(if im_vollbild() {
        "vorn".to_string()
    } else {
        (*lage.ebene.lock().unwrap()).to_string()
    })
}

/// Der Erststand der sichtbaren Fenster. Danach meldet der Beobachter nur
/// noch Aenderungen (noki://fenster) — ohne diesen Aufruf wuesste das
/// Frontend bis zur ersten Fensterbewegung nichts von seiner Umgebung.
#[tauri::command]
fn noki_fenster_liste() -> Vec<Fenster> {
    sichtbare_fenster()
}

/// Doppelklick auf Noki (Abschnitt 15). Die Ausblendbewegung ist zu diesem
/// Zeitpunkt bereits gelaufen; hier verschwindet nur noch das Fenster.
/// JARVIS selbst laeuft weiter.
#[tauri::command]
fn noki_verbergen(app: tauri::AppHandle) {
    if let Some(win) = app.get_webview_window(FENSTER) {
        // Ein Durchgang ueberlebt das Verbergen nicht: sonst kaeme Noki
        // hinter allen Fenstern zurueck (Abschnitt 21).
        let lage = app.state::<Lage>();
        lage.durchgang.store(false, Ordering::Relaxed);
        lage_anwenden(&win, &lage);
        sicht_wunsch_setzen(&app, false);
        if win.is_visible().unwrap_or(false) { sicht_spur("menue_verbergen", "visible", "hidden", "native"); }
        let _ = win.hide();
    }
    sicht_menue(&app);
}

fn noki_zeigen(app: &tauri::AppHandle) {
    let Some(win) = app.get_webview_window(FENSTER) else {
        return;
    };
    // Wiederkommen heisst: in der vom Nutzer gewaehlten Ebene, nie in
    // einem haengengebliebenen Durchgang.
    {
        let lage = app.state::<Lage>();
        lage.durchgang.store(false, Ordering::Relaxed);
        lage_anwenden(&win, &lage);
        // Ausdruecklich mit: das Bit ueberlebt zwar ein Verbergen, aber
        // hier steht damit an einer Stelle, dass beides zusammengehoert
        // und keines das andere ersetzt.
        alle_spaces_anwenden(&win, lage.alle_spaces.load(Ordering::Relaxed));
    }
    sicht_wunsch_setzen(app, true);
    #[cfg(target_os = "macos")]
    space_holen(&win);
    let _ = win.unminimize();
    let _ = win.show();
    let _ = win.set_focus();
    NOKI_GEZEIGT.store(true, Ordering::Relaxed);
    // Die Spawn-Blende gehoert dem Frontend (Abschnitt 16) — hier wird nur
    // gemeldet, dass es sie spielen soll.
    let _ = app.emit("noki://zeigen", serde_json::json!({}));
    sicht_menue(app);
}

/// Oeffnet die persistente Ask-/Code-Arbeitsflaeche als normales natives
/// Fenster. Der Character bleibt im separaten Desktop-Overlay; uebergeben
/// werden nur erlaubte Kontextdaten, niemals DOM- oder Character-Bounds.
#[tauri::command]
fn ask_fenster_zeigen(app: tauri::AppHandle, payload: Option<serde_json::Value>) -> bool {
    eprintln!("[CMD0] zeigen angefordert");
    if !app
        .try_state::<std::sync::Arc<intelligence::Intelligence>>()
        .is_some_and(|state| state.ask_enabled())
    {
        return false;
    }
    // GENAU EIN Ask-Fenster. Es wird in tauri.conf.json angelegt und hier
    // nur noch hervorgeholt — verborgen, minimiert, im falschen Space oder
    // ausserhalb aller Bildschirme. Neu gebaut wird es einzig dann, wenn es
    // gar nicht (mehr) existiert; der Verlauf lebt im Fenster, also ist
    // Wiederverwenden immer der erste Weg.
    let win = match app.get_webview_window(ASK_FENSTER) {
        Some(w) => w,
        None => match ask_fenster_bauen(&app) {
            Some(w) => w,
            None => {
                eprintln!("[ASK] Fenster fehlt und liess sich nicht anlegen");
                return false;
            }
        },
    };

    // 1. ZUERST Space des Fensters an aktuellen Space binden VOR show / unminimize / activate!
    ask_space_anpassen(&win);

    // 2. Ort und Groesse pruefen
    ask_ort_pruefen(&win);

    if let Ok(mut pending) = ASK_OPEN_PAYLOAD.lock() {
        *pending = Some(payload.unwrap_or_else(|| serde_json::json!({})));
    }
    ASK_SHOW_PENDING.store(true, Ordering::Release);
    if ASK_FRONTEND_READY.load(Ordering::Acquire) {
        ask_frontend_open_senden(&win);
    }
    true
}

fn ask_frontend_open_senden(win: &WebviewWindow) {
    if !ASK_SHOW_PENDING.load(Ordering::Acquire) {
        return;
    }
    let payload = ASK_OPEN_PAYLOAD
        .lock()
        .ok()
        .and_then(|pending| pending.clone())
        .unwrap_or_else(|| serde_json::json!({}));
    let _ = win.emit("noki://ask-open", payload);
}

/// Phase 1: ask.html has installed its event listeners. The open payload may
/// now be delivered, but the native window stays hidden until phase 2.
#[tauri::command]
fn ask_frontend_ready(app: tauri::AppHandle) -> bool {
    ASK_FRONTEND_READY.store(true, Ordering::Release);
    if let Some(win) = app.get_webview_window(ASK_FENSTER) {
        ask_frontend_open_senden(&win);
        return true;
    }
    false
}

/// Phase 2: the existing Ask UI has mounted #askNoki. Only now may the native
/// window become visible, so a missed/failed bootstrap can never show black.
#[tauri::command]
fn ask_frontend_mounted(app: tauri::AppHandle) -> bool {
    if !ASK_SHOW_PENDING.swap(false, Ordering::AcqRel)
        || !app
            .try_state::<std::sync::Arc<intelligence::Intelligence>>()
            .is_some_and(|state| state.ask_enabled())
    {
        return false;
    }
    let Some(win) = app.get_webview_window(ASK_FENSTER) else {
        return false;
    };
    let app_handle = app.clone();
    let win_handle = win.clone();
    let _ = app.run_on_main_thread(move || {
        #[cfg(target_os = "macos")]
        let space_before = cgs::aktiver_space().map(|x| x.0).unwrap_or(0);
        ask_space_anpassen(&win_handle);
        ask_ort_pruefen(&win_handle);
        let _ = win_handle.unminimize();
        let _ = win_handle.show();

        // App und Fenster aktivieren & fokussieren.
        #[cfg(target_os = "macos")]
        unsafe {
            use std::ffi::c_void;
            extern "C" {
                fn objc_getClass(n: *const i8) -> *mut c_void;
                fn sel_registerName(n: *const i8) -> *const c_void;
                fn objc_msgSend();
            }
            let get: unsafe extern "C" fn(*mut c_void, *const c_void) -> *mut c_void =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let activate: unsafe extern "C" fn(*mut c_void, *const c_void, bool) =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let nsapp = get(
                objc_getClass(b"NSApplication\0".as_ptr() as _),
                sel_registerName(b"sharedApplication\0".as_ptr() as _),
            );
            if !nsapp.is_null() {
                activate(
                    nsapp,
                    sel_registerName(b"activateIgnoringOtherApps:\0".as_ptr() as _),
                    true,
                );
            }
        }
        let _ = win_handle.set_focus();
        #[cfg(target_os = "macos")]
        space_action_log("ask_rehome_show_activate_focus", "Noki", fenster_nummer(&win_handle).unwrap_or(0), space_before, cgs::aktiver_space().map(|x| x.0).unwrap_or(0));
        let _ = app_handle.emit("noki://ask-opened", serde_json::json!({}));
    });

    eprintln!(
        "[ASK] zeigen: sichtbar={:?} minimiert={:?} fokus={:?} pos={:?} groesse={:?} {}",
        win.is_visible(),
        win.is_minimized(),
        win.is_focused(),
        win.outer_position().map(|p| (p.x, p.y)),
        win.outer_size().map(|s| (s.width, s.height)),
        ask_space_bericht(&win)
    );

    true
}

/// Notfallweg: das Ask-Fenster existiert nicht mehr (zerstoert, nie
/// angelegt). Dieselbe Seite, dasselbe Label — kein zweites Fenster.
fn ask_fenster_bauen(app: &tauri::AppHandle) -> Option<WebviewWindow> {
    ASK_FRONTEND_READY.store(false, Ordering::Release);
    tauri::WebviewWindowBuilder::new(app, ASK_FENSTER, tauri::WebviewUrl::App("ask.html".into()))
        .title("Ask Noki")
        .inner_size(980.0, 720.0)
        .min_inner_size(760.0, 540.0)
        .resizable(true)
        .decorations(true)
        .shadow(true)
        .center()
        .visible(false)
        .build()
        .map_err(|e| eprintln!("[ASK] Fenster konnte nicht angelegt werden: {e}"))
        .ok()
        .inspect(ask_ebene_einrichten)
        .inspect(|w| { oberflaeche_zoom(app, &oberflaeche_wert(app)); let _ = w; })
}

/// Liegt das Fenster ausserhalb jedes Bildschirms oder hat es keine
/// brauchbare Groesse, kaeme `show()` zwar durch, zu sehen waere aber
/// nichts. Dann zurueck auf den aktuellen Monitor. Keine feste Annahme
/// ueber den Bildschirm: gerechnet wird mit dem, den das Fenster meldet.
fn ask_ort_pruefen(win: &WebviewWindow) {
    let Ok(groesse) = win.outer_size() else {
        return;
    };
    if groesse.width < 80 || groesse.height < 80 {
        let _ = win.set_size(tauri::LogicalSize::new(980.0, 720.0));
    }
    let monitor = win
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| win.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else {
        return;
    };
    let Ok(pos) = win.outer_position() else {
        return;
    };
    // FULLY inside the visible area (without menu bar and Dock): never a
    // clipped edge. Shrink only if the screen is smaller than the window,
    // never below its minimum size (760x540 pt).
    let wa = monitor.work_area();
    let k = monitor.scale_factor();
    let groesse = win.outer_size().unwrap_or(groesse);
    let min_w = (760.0 * k) as u32;
    let min_h = (540.0 * k) as u32;
    let w = groesse.width.min(wa.size.width).max(min_w.min(wa.size.width));
    let h = groesse.height.min(wa.size.height).max(min_h.min(wa.size.height));
    if w != groesse.width || h != groesse.height {
        let _ = win.set_size(tauri::PhysicalSize::new(w, h));
    }
    let x = pos.x.clamp(wa.position.x, wa.position.x + wa.size.width as i32 - w as i32);
    let y = pos.y.clamp(wa.position.y, wa.position.y + wa.size.height as i32 - h as i32);
    if x != pos.x || y != pos.y {
        eprintln!("[ASK] in die sichtbare Flaeche gerueckt ({},{}) -> ({x},{y})", pos.x, pos.y);
        let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
    }
}

/// Ask Noki is a normal macOS window (NSNormalWindowLevel = 0).
/// WindowServer maintains its relative Z-order naturally: user focus on another
/// app brings that app forward; closing utilities or swiping spaces preserves
/// the exact relative Z-order without pushing Ask to the bottom.
fn ask_ebene_einrichten(win: &WebviewWindow) {
    #[cfg(target_os = "macos")]
    if let Ok(nw) = win.ns_window() {
        use std::ffi::c_void;
        #[link(name = "objc")]
        extern "C" {
            fn sel_registerName(n: *const std::os::raw::c_char) -> *const c_void;
            fn objc_msgSend();
        }
        unsafe {
            let set: unsafe extern "C" fn(*mut c_void, *const c_void, i64) =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            set(nw, sel_registerName(b"setLevel:\0".as_ptr() as *const _), 0);
        }
    }
    ask_verdeckt_weiter_zeichnen(win);
    #[cfg(not(target_os = "macos"))]
    let _ = win;
}

/// Ask Noki on a Space the user is not looking at stays a real, LIVE window
/// for the Miniatur and Shortcut 9. WebKit otherwise treats an occluded
/// window as invisible and suspends rendering updates - the capture then
/// keeps the frame from before the answer started. Only window OCCLUSION is
/// ignored; a hidden (ordered-out) Ask is still invisible to WebKit, and an
/// unchanged page still paints nothing (Deep Idle unaffected).
fn ask_verdeckt_weiter_zeichnen(win: &WebviewWindow) {
    #[cfg(target_os = "macos")]
    let _ = win.with_webview(|wv| unsafe {
        use std::ffi::c_void;
        #[link(name = "objc")]
        extern "C" {
            fn sel_registerName(n: *const std::os::raw::c_char) -> *const c_void;
            fn objc_msgSend();
        }
        let wk = wv.inner() as *mut c_void;
        if wk.is_null() { return; }
        let antwortet: unsafe extern "C" fn(*mut c_void, *const c_void, *const c_void) -> bool =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let setzen: unsafe extern "C" fn(*mut c_void, *const c_void, bool) =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let sel = sel_registerName(b"_setWindowOcclusionDetectionEnabled:\0".as_ptr() as *const _);
        let ok = antwortet(wk, sel_registerName(b"respondsToSelector:\0".as_ptr() as *const _), sel);
        if ok { setzen(wk, sel, false); }
        eprintln!("[ASK] occlusion_detection_off={ok}");
    });
    #[cfg(not(target_os = "macos"))]
    let _ = win;
}

static ASK_FOKUS_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Das Ask-Fenster in GENAU diesen Space holen. Dieselben Space-Aufrufe wie
/// beim Character (`space_umziehen`), aber ohne dessen Ebenen- und
/// Vollbild-Behandlung: Ask Noki bleibt ein gewoehnliches Fenster.
#[cfg(target_os = "macos")]
fn ask_space_holen(wid: i64, sid: u64) {
    let schon = cgs::spaces_des_fensters(wid).map_or(false, |v| v.len() == 1 && v[0] == sid);
    if schon {
        return;
    }
    cgs::hinzufuegen(wid, sid);
    let andere: Vec<u64> = cgs::spaces_des_fensters(wid)
        .unwrap_or_default()
        .into_iter()
        .filter(|s| *s != sid)
        .collect();
    if !andere.is_empty() {
        cgs::entfernen(wid, &andere);
    }
}

#[cfg(target_os = "macos")]
fn ask_space_anpassen(win: &WebviewWindow) {
    if let Some(wid) = fenster_nummer(win) {
        ASK_WID.store(wid, Ordering::Relaxed);
        let _ = win.set_visible_on_all_workspaces(false);
        if let Some((aktiv, _)) = cgs::aktiver_space() {
            ask_space_holen(wid, aktiv);
        }
    }
    if let Ok(nw) = win.ns_window() {
        unsafe {
            extern "C" {
                fn sel_registerName(n: *const i8) -> *const std::ffi::c_void;
                fn objc_msgSend();
            }
            let f = objc_msgSend as unsafe extern "C" fn();
            let get: unsafe extern "C" fn(*mut std::ffi::c_void, *const std::ffi::c_void) -> usize =
                std::mem::transmute(f);
            let set: unsafe extern "C" fn(*mut std::ffi::c_void, *const std::ffi::c_void, usize) =
                std::mem::transmute(f);
            let cur = get(
                nw as *mut _,
                sel_registerName(b"collectionBehavior\0".as_ptr() as _),
            );
            // Ensure CanJoinAllSpaces (1 << 0) and MoveToActiveSpace (1 << 1) are unset
            set(
                nw as *mut _,
                sel_registerName(b"setCollectionBehavior:\0".as_ptr() as _),
                cur & !(1 | (1 << 1)),
            );
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn ask_space_anpassen(_win: &WebviewWindow) {}

/// "sichtbar, aber auf einem anderen Schreibtisch" ist ein FEHLER und muss
/// sich im Protokoll von "sichtbar" unterscheiden lassen.
fn ask_space_bericht(win: &WebviewWindow) -> String {
    #[cfg(target_os = "macos")]
    {
        let Some(wid) = fenster_nummer(win) else {
            return "space=? (kein Fenster)".into();
        };
        let Some((aktiv, _)) = cgs::aktiver_space() else {
            return "space=? (CGS fehlt)".into();
        };
        let sp = cgs::spaces_des_fensters(wid).unwrap_or_default();
        return format!("space={aktiv} mitglied={sp:?} hier={}", sp.contains(&aktiv));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = win;
        String::new()
    }
}

#[tauri::command]
fn ask_fenster_verbergen(app: tauri::AppHandle) {
    eprintln!("[CMD0] verbergen angefordert");
    ASK_FOKUS_GEN.fetch_add(1, Ordering::SeqCst);
    ASK_SHOW_PENDING.store(false, Ordering::Release);
    if let Ok(mut pending) = ASK_OPEN_PAYLOAD.lock() {
        *pending = None;
    }
    if let Some(win) = app.get_webview_window(ASK_FENSTER) {
        let _ = app.emit("noki://ask-closed", serde_json::json!({}));
        let win_handle = win.clone();
        let app_handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            if win_handle.is_fullscreen().unwrap_or(false) {
                let _ = win_handle.set_fullscreen(false);
                let h = app_handle.clone();
                std::thread::spawn(move || {
                    thread::sleep(Duration::from_millis(700));
                    let h2 = h.clone();
                    let _ = h.run_on_main_thread(move || {
                        if let Some(w) = h2.get_webview_window(ASK_FENSTER) {
                            let _ = w.hide();
                            #[cfg(target_os = "macos")]
                            if let Ok(nw) = w.ns_window() {
                                use std::ffi::c_void;
                                #[link(name = "objc")]
                                extern "C" {
                                    fn sel_registerName(n: *const std::os::raw::c_char) -> *const c_void;
                                    fn objc_msgSend();
                                }
                                unsafe {
                                    let f: unsafe extern "C" fn(*mut c_void, *const c_void, *mut c_void) =
                                        std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                                    f(nw, sel_registerName(b"orderOut:\0".as_ptr() as *const _), std::ptr::null_mut());
                                }
                            }
                        }
                    });
                });
                return;
            }
            let _ = win_handle.hide();
            #[cfg(target_os = "macos")]
            if let Ok(nw) = win_handle.ns_window() {
                use std::ffi::c_void;
                #[link(name = "objc")]
                extern "C" {
                    fn sel_registerName(n: *const std::os::raw::c_char) -> *const c_void;
                    fn objc_msgSend();
                }
                unsafe {
                    let f: unsafe extern "C" fn(*mut c_void, *const c_void, *mut c_void) =
                        std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                    f(nw, sel_registerName(b"orderOut:\0".as_ptr() as *const _), std::ptr::null_mut());
                }
            }
        });
    }
}

/// Cmd+0 ist ein ECHTER Umschalter fuer die Ask-/Work-Flaeche.
///
/// Sichtbares Fenster -> verbergen. Verborgenes oder minimiertes Fenster ->
/// ueber den vorhandenen Weg (noki://ask -> openNative) wieder hervorholen.
/// Es entsteht dabei nie ein zweites Fenster: gezeigt und verborgen wird
/// immer dasselbe Label, Chat, Sitzung und Kontext bleiben erhalten.
/// Bloss fokussieren waere kein Umschalter — darum hier die Sichtpruefung.
static CMD0_PRESS: AtomicU32 = AtomicU32::new(0);

fn ask_umschalten(app: &tauri::AppHandle) {
    if !app
        .try_state::<std::sync::Arc<intelligence::Intelligence>>()
        .is_some_and(|state| state.ask_enabled())
    {
        virtual_workspace::trace("[SHORTCUT] action n=0 ignored: Ask Noki disabled");
        return;
    }
    let nr = CMD0_PRESS.fetch_add(1, Ordering::Relaxed) + 1;
    let win = app.get_webview_window(ASK_FENSTER);
    let before = win
        .as_ref()
        .map(|w| w.is_visible().unwrap_or(false))
        .unwrap_or(false);
    let minimiert = win
        .as_ref()
        .map(|w| w.is_minimized().unwrap_or(false))
        .unwrap_or(false);
    let aktion = if before && !minimiert { "hide" } else { "show" };
    eprintln!("[CMD0] PRESS #{nr} before={before} minimiert={minimiert} action={aktion}");
    if aktion == "hide" {
        ask_fenster_verbergen(app.clone());
    } else {
        ask_fenster_zeigen(app.clone(), None);
    }
    // Was the decision still true a moment later, or did something re-show the
    // window inside the same press? That is the only way to see a hide->show.
    let h = app.clone();
    std::thread::spawn(move || {
        for ms in [40u64, 150, 500, 1000] {
            thread::sleep(Duration::from_millis(match ms {
                40 => 40,
                150 => 110,
                500 => 350,
                _ => 500,
            }));
            let jetzt = h
                .get_webview_window(ASK_FENSTER)
                .map(|w| w.is_visible().unwrap_or(false))
                .unwrap_or(false);
            eprintln!(
                "[CMD0] PRESS #{nr} nach {ms}ms sichtbar={jetzt} (erwartet={})",
                aktion == "show"
            );
        }
    });
}

// =====================================================================
//  6 · SYMBOL DER MENUELEISTE
//
//  Gerechnet statt geladen: ein Bilddecoder waere eine zusaetzliche
//  Abhaengigkeit fuer 44 x 44 Punkte. Als macOS-Vorlagenbild zaehlt
//  ohnehin nur der Alphakanal — die Form, nicht die Farbe.
//
//  Gezeichnet wird Nokis Silhouette: Antenne, Kuppe, Kopf, Visier.
// =====================================================================
fn tray_symbol() -> Image<'static> {
    const N: u32 = 44;
    let mut px = vec![0u8; (N * N * 4) as usize];

    // Abstand zu einer abgerundeten Box (dieselbe Formel wie im Shader).
    let box_d = |x: f32, y: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32| {
        let dx = (x - cx).abs() - (hw - r);
        let dy = (y - cy).abs() - (hh - r);
        let ax = dx.max(0.0);
        let ay = dy.max(0.0);
        (ax * ax + ay * ay).sqrt() + dx.max(dy).min(0.0) - r
    };
    let kreis =
        |x: f32, y: f32, cx: f32, cy: f32, r: f32| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r;

    for j in 0..N {
        for i in 0..N {
            let (x, y) = (i as f32 + 0.5, j as f32 + 0.5);

            let antenne = box_d(x, y, 22.0, 10.0, 1.6, 6.0, 1.4);
            let kuppe = kreis(x, y, 22.0, 5.0, 3.2);
            let kopf = box_d(x, y, 22.0, 27.0, 14.0, 11.0, 7.0);
            let koerper = kopf.min(antenne).min(kuppe);

            // Visier als Aussparung, zwei Augen als Fuellung darin.
            let visier = box_d(x, y, 22.0, 26.0, 9.5, 4.6, 3.2);
            let auge_l = kreis(x, y, 17.6, 26.0, 1.9);
            let auge_r = kreis(x, y, 26.4, 26.0, 1.9);

            // Weiche Kante ueber eine halbe Zelle — sonst franst das
            // Symbol in der Menueleiste aus.
            let deckung = |d: f32| (0.5 - d).clamp(0.0, 1.0);
            let mut a = deckung(koerper);
            a *= 1.0 - deckung(visier);
            a = a.max(deckung(auge_l.min(auge_r)));

            let o = ((j * N + i) * 4) as usize;
            px[o] = 0;
            px[o + 1] = 0;
            px[o + 2] = 0;
            px[o + 3] = (a * 255.0).round() as u8;
        }
    }

    Image::new_owned(px, N, N)
}

// =====================================================================
//  7 · START
// =====================================================================
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let stop = Arc::new(AtomicBool::new(false));
    let watcher_stop = Arc::clone(&stop);
    let hit_store = Arc::new(std::sync::RwLock::new(NokiHitZustand::default()));
    let watcher_hit = Arc::clone(&hit_store);

    tauri::Builder::default()
        // Shortcut 9 card pictures as raw PNG bytes (no base64 over the IPC).
        // Live preview of a Noki code project (read-only, no IPC for the page).
        .register_uri_scheme_protocol("nokicode", |_ctx, anfrage| code_vorschau::protokoll(anfrage))
        // Noki Kamera: Vorschaubilder + Dateien der Galerie (eigener Thread, Range).
        .register_asynchronous_uri_scheme_protocol("nokimedien", |ctx, anfrage, antworter| kamera_galerie::protokoll(ctx.app_handle(), anfrage, antworter))
        .register_uri_scheme_protocol("nokibild", |_ctx, anfrage| {
            let antwort = tauri::http::Response::builder();
            match window_overview::bild_antwort(anfrage.uri().path()) {
                Some(png) => antwort
                    .header("Content-Type", "image/png")
                    // Every live frame has its own URL: without this WebKit
                    // kept each one in its memory cache (panel WebContent
                    // grew by >200 MB in minutes).
                    .header("Cache-Control", "no-store")
                    .body(png.as_ref().clone())
                    .unwrap_or_default(),
                None => antwort.status(404).body(Vec::new()).unwrap_or_default(),
            }
        })
        .manage(Lage {
            // Der echte Wert kommt in setup() aus der Einstellungsdatei.
            ebene: Mutex::new("vorn"),
            durchgang: AtomicBool::new(false),
            // Der echte Wert kommt in setup() aus der Einstellungsdatei.
            alle_spaces: AtomicBool::new(true),
        })
        .manage(NokiHitStore(hit_store))
        .manage(std::sync::Arc::new(attachments::AttachmentStore::default()))
        .manage(intelligence::Intelligence::new())
        .invoke_handler(tauri::generate_handler![
            noki_talk::noki_talk_status,
            noki_talk::noki_talk_einstellung,
            noki_talk::noki_talk_engine_starten,
            noki_talk::noki_talk_verlauf,
            noki_talk::noki_talk_audio,
            noki_talk::noki_talk_kopieren,
            noki_talk::noki_talk_loeschen,
            noki_talk::noki_talk_mikrofon_freigabe,
            kamera_galerie::kamera_medien,
            kamera_galerie::kamera_bild_speichern,
            kamera_galerie::kamera_loeschen,
            kamera_galerie::kamera_video_export,
            kamera_galerie::kamera_video_abbrechen,
            intelligence::code_vorschau_oeffnen,
            intelligence::code_projekt_zeigen,
            intelligence::code_projekte,
            intelligence::code_projekt_info,
            intelligence::code_projekt_dateiinhalt,
            intelligence::code_projekt_aktivieren,
            intelligence::code_sitzungen,
            intelligence::code_sitzung_anlegen,
            intelligence::code_sitzung_schliessen,
            intelligence::code_sitzung_modus,
            intelligence::code_sitzung_projekt,
            intelligence::code_sitzung_loesen,
            intelligence::code_terminal_gespraech,
            intelligence::code_terminal_gespraech_loeschen,
            intelligence::code_sitzung_abbrechen,
            intelligence::code_sitzung_senden,
            intelligence::code_befehl_pruefen,
            intelligence::code_projekt_loeschen,
            intelligence::intelligence_settings,
            intelligence::noki_modell_rollen,
            intelligence::noki_modell_rolle_setzen,
            intelligence::intelligence_providers,
            intelligence::intelligence_mcp_connectors,
            intelligence::intelligence_mcp_execute,
            intelligence::intelligence_mcp_config,
            intelligence::intelligence_mcp_set_server,
            intelligence::intelligence_providers_status,
            intelligence::intelligence_providers_set_enabled,
            intelligence::intelligence_engine_set_mode,
            intelligence::intelligence_engine_toggle_provider,
            intelligence::intelligence_engine_toggle_model,
            intelligence::intelligence_renew_attestation,
            intelligence::intelligence_engine_run_benchmarks,
            intelligence::intelligence_save,
            intelligence::intelligence_context,
            intelligence::intelligence_chat,
            intelligence::intelligence_code,
            intelligence::intelligence_cancel,
            intelligence::intelligence_observe,
            intelligence::intelligence_feedback,
            intelligence::intelligence_status,
            intelligence::intelligence_load,
            intelligence::intelligence_unload,
            intelligence::intelligence_open_source,
            intelligence::intelligence_memory_list,
            intelligence::intelligence_memory_delete,
            intelligence::intelligence_memory_clear,
            intelligence::intelligence_tool_execute,
            browser_suche,
            intelligence::intelligence_mode,
            intelligence::intelligence_denken,
            intelligence::intelligence_assistant_mode,
            intelligence::intelligence_code_terminal_open,
            intelligence::intelligence_code_terminal_status,
            intelligence::intelligence_code_overview,
            intelligence::intelligence_code_view_attach,
            intelligence::intelligence_code_view_send,
            intelligence::intelligence_code_view_detach,
            intelligence::intelligence_code_style,
            intelligence::intelligence_shell_spawn,
            intelligence::intelligence_shell_write,
            intelligence::intelligence_shell_resize,
            intelligence::intelligence_shell_close,
            intelligence::intelligence_shell_list,
            intelligence::intelligence_task,
            intelligence::intelligence_notify,
            intelligence::intelligence_seen,
            intelligence::intelligence_prewarm,
            intelligence::intelligence_voice_start,
            intelligence::intelligence_voice_diag,
            intelligence::intelligence_voice_stop,
            noki_bereit,
            ask_fenster_zeigen,
            ask_fenster_verbergen,
            ask_frontend_ready,
            ask_frontend_mounted,
            noki_alle_schreibtische,
            noki_verbergen,
            noki_fenster_setzen,
            noki_fenster_bewegen,
            noki_fenster_position_holen,
            noki_toggle_fullscreen,
            noki_fullscreen_status,
            noki_schirm_info,
            noki_ort_speichern,
            noki_maus_position,
            noki_hit_zone,
            noki_vorschau_zone,
            noki_drag_status,
            noki_fenster_liste,
            noki_schliess_ziel,
            noki_bildschirmfoto,
            noki_maus_menue,
            noki_space_bereit,
            noki_aufnahme,
            noki_modus_menue,
            noki_fenster_schliessen,
            noki_ebene_durchgang,
            noki_log,
            noki_thermal,
            noki_schwarm,
            ablage_liste,
            ablage_oeffnen,
            ablage_finder,
            ablage_entfernen,
            ablage_leeren,
            ablage_ziehen,
            noki_schnell,
            noki_arbeitsplatz_besuchen,
            noki_arbeitsplatz_diagnose,
            noki_arbeitsplatz_lage,
            noki_schirm_freigabe,
            noki_vorschau_setzen,
            noki_vorschau_zeigen_anfrage,
            noki_vorschau_stop,
            noki_vorschau_gross,
            noki_ui_puls,
            noki_stimme_beendet,
            noki_stimme_zone,
            noki_blase_zone,
            noki_einst_zone,
            noki_panel_zonen,
            noki_vorschau_label,
            noki_vorschau_navi,
            noki_space_gezeichnet,
            noki_arbeitsplatz_oeffnen,
            noki_arbeitsplatz_suchen,
            noki_arbeitsplatz_aufgabe_beenden,
            noki_fenster_auswahl,
            noki_energie,
            clip_liste,
            clip_voll,
            clip_kopieren,
            clip_link_oeffnen,
            clip_entfernen,
            clip_leeren,
            platz_liste,
            platz_speichern,
            platz_laden,
            fokus_apps,
            apps_oeffnen,
            lokale_ressource_oeffnen,
            browser_url_oeffnen,
            app_uri_oeffnen,
            anhang_waehlen,
            datei_browser_liste,
            datei_galerie_seite,
            datei_browser_start,
            browser_debug_log,
            datei_thumbnail,
            datei_vorschau,
            anhang_hinzufuegen,
            anhang_liste,
            anhang_entfernen,
            noki_aktions_log,
            noki_cmd0_nativ,
            werk_daten_lesen,
            werk_daten_schreiben,
            noki_utility_vorn,
            noki_utility_zurueck,
            noki_parade_ebene,
            ax_status,
            ax_freigabe,
            kompakt_auf,
            kompakt_zu,
            kompakt_aktion,
            ablage_apps,
            ablage_oeffnen_mit,
            noki_ablage_ordner_oeffnen,
            ablage_loeschen,
            noki_ordner_leeren,
            noki_ordner_liste,
            noki_ordner_halten,
            noki_datei_apps,
            noki_datei_oeffnen_mit,
            noki_ordner_finder,
            noki_dateien_waehlen,
            datei_oeffnen,
            datei_finder,
            datei_bearbeiten,
            kamera_ordner_oeffnen,
            einstellungen_oeffnen,
            einstellungen_fenster,
            oberflaeche_lesen,
            oberflaeche_setzen,
            fokus_sitzung_start,
            app_icons,
            fokus_sitzung_ende,
            fokus_sitzung_pruefen,
            fokus_sitzung_status,
            noki_sicht_spur,
            noki_app_info,
            kuerzel_info,
            noki_ebene_umschalten,
            window_overview::window_overview_list,
            window_overview::window_overview_close,
            window_overview::window_overview_fit,
            window_overview::window_overview_geometry,
            window_overview::window_overview_frames,
            window_overview::window_overview_tabs,
            window_overview::window_overview_tab_activate,
            window_overview::window_overview_state,
            window_overview::window_overview_action,
            ask_fern::ask_fern_feld,
            ask_fern::ask_fern_kopieren,
        ])
        .setup(move |app| {
            {
                // `noki code` service for Code Space terminals.
                let w = app.state::<std::sync::Arc<intelligence::Intelligence>>().inner().clone();
                intelligence::code_dienst_starten(app.handle().clone(), w);
            }
            // Mission Control open/closed -> Miniatur temporary visibility.
            #[cfg(target_os = "macos")]
            mission_control::starten();
            // USER preference of the Miniatur (Shortcut 4) from its store.
            if let Some(b) = werk_daten_lesen(app.handle().clone(), "vorschau".into())
                .get("sichtbar").and_then(|v| v.as_bool())
            {
                MINIATUR_NUTZER_SICHTBAR.store(b, Ordering::Relaxed);
            }
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            // Darstellung › UI-Groesse fuer das (aus der Konfiguration angelegte) Ask-Fenster.
            oberflaeche_zoom(app.handle(), &oberflaeche_wert(app.handle()));
            // Normale macOS-App: Dock, App-Menue und natives Hauptfenster.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Regular);

            // The persisted Workspace is part of startup input, not output
            // of backend initialization.  Load it BEFORE the helper can
            // publish BACKEND_READY: otherwise the READY callback races
            // this long setup block, sees an empty in-memory registry and
            // atomically overwrites the valid disk registry with [].
            arbeitsplatz_fenster_laden(app.handle());

            // Backend selection is made once per process.  Virtual startup
            // remains INITIALIZING until the helper publishes its display
            // identity; input and workspace shortcuts fail closed meanwhile.
            virtual_workspace::start(app.handle().clone());

            // P0 Startup Invariant: Resolve and validate target Desktop BEFORE any
            // watcher or helper process is spawned.
            // 1. Read current real macOS Space topology from CGS once synchronously.
            // 2. Resolve the single Noki target Desktop (persisted UUID or validated free desktop).
            // 3. Atomically lock ARBEITSPLATZ.
            // 4. Seed LETZT_SID and LETZT_AUF_ARBEITSPLATZ so the first tick of space_waechter
            //    does not trigger false space switches or spurious retargeting.
            #[cfg(target_os = "macos")]
            if virtual_workspace::backend() != virtual_workspace::Backend::LegacyVirtualDisplay {
                let mut resolved = None;
                for _ in 0..12 {
                    if let Ok(res) = arbeitsplatz_sichern(app.handle()) {
                        resolved = Some(res);
                        break;
                    }
                    thread::sleep(Duration::from_millis(25));
                }
                if let Some(res) = resolved {
                    let aktiver = cgs::aktiver_space().map(|(a, _)| a).unwrap_or(0);
                    if aktiver != 0 {
                        LETZT_SID.store(aktiver, Ordering::Relaxed);
                        LETZT_AUF_ARBEITSPLATZ.store(aktiver == res.id, Ordering::Relaxed);
                    }
                    eprintln!("[STARTUP] target desktop resolved: Space {} ({})", res.id, res.uuid);
                } else {
                    eprintln!("[STARTUP] target desktop discovery pending");
                }
            }

            if let Some(win) = app.get_webview_window(FENSTER) {
                let _ = win.set_decorations(false);
                let _ = win.set_shadow(false);
                let _ = win.set_resizable(false);
                overlay_durchlaessig(&win, true);
            }
            if let Some(win) = app.get_webview_window(ASK_FENSTER) {
                let _ = win.set_decorations(true);
                let _ = win.set_shadow(true);
                let _ = win.set_resizable(true);
                ask_space_anpassen(&win);
                ask_ebene_einrichten(&win);
            }
            #[cfg(debug_assertions)]
            if let Ok(p) = std::env::var("NOKI_INTELLIGENCE_SELFTEST") { intelligence::selftest(app.handle().clone(), p.into()); }
            #[cfg(debug_assertions)]
            {
                // Laufzeit-Identitaet: die Build-ID kommt aus der Quelle (gleicher Stand = gleiche ID),
                // der Zeitstempel aus der tatsaechlich laufenden Binaerdatei — so ist ein Rebuild
                // auch dann sichtbar, wenn sich am Quellstand nichts geaendert hat.
                let exe = std::env::current_exe().ok();
                let ts = exe.as_ref().and_then(|p| std::fs::metadata(p).ok()).and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
                log::info!("noki-build id={} git_head={} build_timestamp={} bundle={:?}",
                    env!("NOKI_BUILD_ID"), env!("NOKI_BUILD_ID").split('-').next().unwrap_or("?"), ts,
                    exe.and_then(|p| p.parent().and_then(|p| p.parent()).and_then(|p| p.parent()).map(|p| p.to_path_buf())));
            }
            #[cfg(debug_assertions)]
            if let Ok(p) = std::env::var("NOKI_PANEL_PROBE") { intelligence::panel_probe(app.handle().clone(), p.into()); }
            #[cfg(debug_assertions)]
            if let Ok(p) = std::env::var("NOKI_VOICE_PROBE") { intelligence::voice_probe(app.handle().clone(), p.into()); }
            #[cfg(debug_assertions)]
            if let Ok(p) = std::env::var("NOKI_STABLE_PROBE") { intelligence::stable_probe(app.handle().clone(), p.into()); }
            #[cfg(debug_assertions)]
            if let Ok(p) = std::env::var("NOKI_CHAT_PROBE") { intelligence::chat_probe(app.handle().clone(), p.into()); }
            #[cfg(debug_assertions)]
            if let Ok(p) = std::env::var("NOKI_MODEL_MODE_PROBE") { intelligence::model_mode_probe(app.handle().clone(), p.into()); }
            #[cfg(debug_assertions)]
            if let Ok(p) = std::env::var("NOKI_PERF_PROBE") { intelligence::perf_probe(app.handle().clone(), p.into()); }
            #[cfg(debug_assertions)]
            if let Ok(p) = std::env::var("NOKI_COLD_PROBE") { intelligence::cold_probe(app.handle().clone(), p.into()); }
            #[cfg(debug_assertions)]
            if let Ok(p) = std::env::var("NOKI_SYNTH_PROBE") { intelligence::synth_probe(app.handle().clone(), p.into()); }
            // Fenster-Smoke ohne Klick: zeigt Ask, verbirgt es, zeigt es
            // erneut und meldet jedes Mal den echten Fensterzustand samt
            // Anzahl der Ask-Fenster. Nur Debug-Build, nur mit Umgebungsvariable.
            #[cfg(debug_assertions)]
            if std::env::var("NOKI_ASK_PROBE").is_ok() {
                let h = app.handle().clone();
                thread::spawn(move || {
                    for runde in 1..=2 {
                        thread::sleep(Duration::from_millis(2500));
                        let hh = h.clone();
                        let _ = h.run_on_main_thread(move || {
                            // Genau der Weg des Nutzers: Kurzbefehl -> noki://ask
                            // -> Frontend -> openNative -> ask_fenster_zeigen.
                            shortcut_ausloesen(&hh, 0);
                        });
                        thread::sleep(Duration::from_millis(600));
                        let hh = h.clone();
                        let _ = h.run_on_main_thread(move || {
                            let ok = hh
                                .get_webview_window(ASK_FENSTER)
                                .and_then(|w| w.is_visible().ok())
                                .unwrap_or(false);
                            let n = hh
                                .webview_windows()
                                .keys()
                                .filter(|l| l.as_str() == ASK_FENSTER)
                                .count();
                            let sichtbar = hh
                                .get_webview_window(ASK_FENSTER)
                                .and_then(|w| w.is_visible().ok())
                                .unwrap_or(false);
                            eprintln!(
                                "[ASK-PROBE] runde={runde} zeigen={ok} fenster={n} sichtbar={sichtbar}"
                            );
                        });
                        thread::sleep(Duration::from_millis(800));
                        let hv = h.clone();
                        let _ = h.run_on_main_thread(move || {
                            ask_fenster_verbergen(hv.clone());
                            let sichtbar = hv
                                .get_webview_window(ASK_FENSTER)
                                .and_then(|w| w.is_visible().ok())
                                .unwrap_or(false);
                            eprintln!("[ASK-PROBE] nach verbergen sichtbar={sichtbar}");
                        });
                    }
                });
            }
            #[cfg(debug_assertions)]
            if let Ok(p) = std::env::var("NOKI_RENDER_PROBE") { intelligence::render_probe(app.handle().clone(), p.into()); }
            #[cfg(debug_assertions)]
            if let Ok(p) = std::env::var("NOKI_RUNTIME_PROBE") { intelligence::runtime_probe(app.handle().clone(), p.into()); }
            #[cfg(debug_assertions)]
            if let (Ok(a), Ok(p)) = (std::env::var("NOKI_GOLDEN_AUDIO"), std::env::var("NOKI_GOLDEN_PROBE")) { intelligence::golden_probe(app.handle().clone(), a.into(), p.into()); }
            #[cfg(debug_assertions)]
            if let Ok(p) = std::env::var("NOKI_RESEARCH_PROBE") { intelligence::research_probe(app.handle().clone(), p.into()); }
            intelligence::benachrichtigung_beobachten(app.handle().clone());
            // Temporaere Diagnose: welche App/welcher Prozess laeuft, und vertraut macOS ihr (AX)?
            eprintln!("[AX] Start: bundle={} exe={} pid={} AXIsProcessTrusted={}",
                app.config().identifier,
                std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default(),
                std::process::id(), lesezeichen::ax_vertraut(false));

            // EIN Sichtbarkeitseintrag (Text wechselt mit der echten Sichtbarkeit).
            // Abschnitt 25: der Doppelklick legt Noki schlafen — Verbergen
            // laeuft deshalb ueber diesen Eintrag.
            let sicht = MenuItem::with_id(app, "sicht", "Noki zeigen", true, None::<&str>)?;
            app.manage(SichtMenue(sicht.clone()));
            sicht_waechter(app.handle().clone());
            space_waechter(app.handle().clone());
            noki_browser::waechter_starten();
            // Abschnitt 29: Nokis obere Leiste ist unter macOS die
            // Menueleiste — dort liegen schon Zeigen, Verbergen und die
            // Ebenen. Die Schreibtischwahl gehoert genau dorthin und
            // braucht keine eigene Einstellungsseite.
            let alle_an = alle_spaces_lesen(app.handle());
            app.state::<Lage>().alle_spaces.store(alle_an, Ordering::Relaxed);
            // Der Sichtbarkeitswunsch ueberlebt den Neustart: wer Noki
            // ausdruecklich verborgen hat, bekommt ihn nicht von selbst
            // wieder. Alles andere startet sichtbar.
            let wunsch_an = sicht_wunsch_lesen(app.handle());
            NUTZER_VERBORGEN.store(!wunsch_an, Ordering::Relaxed);
            // Vordergrund/Hintergrund ueberlebt den Neustart ebenfalls.
            *app.state::<Lage>().ebene.lock().unwrap() = ebene_lesen(app.handle());
            let spaces = CheckMenuItem::with_id(
                app, "alle_spaces", "Auf allen Schreibtischen",
                true, alle_an, None::<&str>)?;
            // EINE Trennlinie, und die nur vor dem Beenden. Vorher waren es
            // drei; sie zerlegten das Menue in vier Bloecke, und das las
            // sich wie mehrere gestapelte Flaechen — "Fenster im Fenster".
            // Die Zusammengehoerigkeit der drei Ebenen traegt schon ihr
            // Haekchen, dafuer braucht es keine eigene Kammer.
            // Abschnitt 10/11/23: an die frei gewordene Stelle tritt das
            // Aktionen-Menue. Es enthaelt INSZENIERTE Verhaltensweisen —
            // ausdruecklich nicht Nokis autonome Tempostufen. NORMAL,
            // FAST, FLITZEN und SPEED entscheidet er weiterhin selbst;
            // einen Knopf, der eine dieser Stufen einschaltet, gibt es
            // hier bewusst nicht.
            //
            // Die Leiste schickt nur den NAMEN. Ob eine Aktion gerade
            // moeglich ist — er schlaeft, wird getragen, steht schon am
            // Boden, oder eine andere Aktion laeuft —, entscheidet Noki
            // selbst; die native Seite kennt seinen Zustand nicht und
            // soll ihn auch nicht nachbilden.
            //
            // Einen eigenen Punkt "Speed" gibt es hier NICHT mehr. Er war
            // die Vorfuehrung einer Geschwindigkeit, und "Einmal durch den
            // Desktop" benutzt dieselbe Stufe — hin, wenden, zurueck — und
            // ist dabei eine ganze Handlung statt nur eines Tempos. Zwei
            // Knoepfe fuer denselben Schub waeren einer zu viel. Nokis
            // AUTONOMES Vollgas bleibt davon unberuehrt: er waehlt SPEED
            // weiterhin selbst. Die Liste hier ist die native Seite
            // derselben Aufzaehlung, die drueben AKTIONEN heisst; sie
            // stehen bewusst gleich: {desktop, fallen}.
            let a_desktop = MenuItem::with_id(
                app, "aktion_desktop", "Einmal durch den Desktop", true, None::<&str>)?;
            let a_fallen = MenuItem::with_id(
                app, "aktion_fallen", "Runterfallen", true, None::<&str>)?;
            // Maus folgen: Umschalter, Text wechselt zu "Maus folgen beenden".
            let a_maus = MenuItem::with_id(app, "aktion_maus", "Maus folgen", true, None::<&str>)?;
            app.manage(MausMenue(a_maus.clone()));
            let aktionen = Submenu::with_id_and_items(
                app, "aktionen", "Aktionen", true,
                &[&a_desktop, &a_fallen, &a_maus],
            )?;
            // Eigene Hauptpunkte: Freeze (Umschalter, Text wechselt zu
            // "Freeze beenden") und Kamera (Screenshot / Screen Recording).
            let m_freeze = MenuItem::with_id(app, "freeze_umschalten", "Freeze", true, None::<&str>)?;
            let k_foto = MenuItem::with_id(app, "kamera_screenshot", "Screenshot", true, None::<&str>)?;
            let k_rec = MenuItem::with_id(app, "kamera_recording", "Screen Recording", true, None::<&str>)?;
            let kamera = Submenu::with_id_and_items(app, "kamera", "Kamera", true, &[&k_foto, &k_rec])?;
            app.manage(ModusMenue(m_freeze.clone(), k_rec.clone()));
            app.manage(Aufnahme(std::sync::Mutex::new(None)));
            // "Fenster schliessen": die Eintraege (App — Titel) fuellt der
            // Fenster-Beobachter. Ein Klick meldet NUR diese Fensternummer
            // (noki://schliessen); Noki waehlt danach Plasma oder Schwingen.
            let fz = Submenu::with_id(app, "fenster_schliessen", "Fenster schließen", true)?;
            app.manage(SchliessMenue(fz.clone()));
            // Abschnitt 2: Energie gehoert genau hierher — neben
            // Aktionen, im selben Robotermenue. Die Stufen sind dieselben
            // wie in window.NokiEnergie; die native Seite haelt nur den
            // Haken, entschieden wird drueben.
            let energie_stufen: [(&str, &str); 3] = [
                ("sparend", "Stromsparend"),
                ("normal", "Normal"),
                ("energisch", "Sehr energisch"),
            ];
            let energie_items: Vec<CheckMenuItem<_>> = energie_stufen
                .iter()
                .map(|(id, label)| {
                    CheckMenuItem::with_id(
                        app,
                        format!("energie_{}", id),
                        *label,
                        true,
                        *id == "normal",
                        None::<&str>,
                    )
                    .expect("Energieeintrag")
                })
                .collect();
            let energie_refs: Vec<&dyn tauri::menu::IsMenuItem<_>> =
                energie_items.iter().map(|i| i as &dyn tauri::menu::IsMenuItem<_>).collect();
            let energie = Submenu::with_id_and_items(
                app, "energie", "Energie", true, &energie_refs,
            )?;
            // Groesse: stufenloser Regler in einem kleinen Fenster (ein natives
            // Menue kann keinen Schieberegler). Der Eintrag zeigt den Wert.
            let groesse = Submenu::with_id(app, "groesse", "Größe", true)?;   // Regler: regler::einbauen
            app.manage(GroesseWert(std::sync::Mutex::new(1.0)));
            // Aktionen, Energie und Groesse leben jetzt in den Noki Einstellungen
            // (Seite "Darstellung"). Die Eintraege bleiben als Objekte bestehen
            // (Haken/Texte werden weiter gepflegt), stehen aber nicht mehr im Menue.
            let _ = (&aktionen, &energie, &groesse);
            // Ein Klick oeffnet die Einstellungen direkt (bzw. holt das offene
            // Fenster nach vorn) — keine Unterebene "Einstellungen öffnen" mehr.
            let einstellungen = MenuItem::with_id(app, "einstellungen_oeffnen", "Noki Einstellungen", true, None::<&str>)?;
            // Ansicht: EIN Ebenen-Eintrag, der die NAECHSTE Aktion zeigt (Abschnitt 14:
            // "hinten" = unter gewoehnlichen Fenstern), dazu die Schreibtischwahl.
            let ebene_jetzt: &'static str = *app.state::<Lage>().ebene.lock().unwrap();
            let ebene_item = MenuItem::with_id(app, "ebene_wechsel",
                if ebene_jetzt == "hinten" { "In den Vordergrund" } else { "In den Hintergrund" }, true, None::<&str>)?;
            let ansicht = Submenu::with_id_and_items(app, "ansicht", "Ansicht", true, &[&ebene_item, &spaces])?;
            let trenner = PredefinedMenuItem::separator(app)?;
            let beenden = MenuItem::with_id(app, "beenden", "Noki beenden", true, None::<&str>)?;
            // Ablage: Referenzen auf Dateien, die Noki haelt (Zahl im Eintrag).
            let ablage_start = ablage_laden(app.handle());
            let ablage_item = MenuItem::with_id(app, "ablage",
                if ablage_start.is_empty() { "Dokumente".to_string() } else { format!("Dokumente ({})", ablage_start.len()) },
                true, None::<&str>)?;
            app.manage(Ablage(Mutex::new(ablage_start)));
            app.manage(AblageMenue(ablage_item.clone()));
            // Zwischenablage-Verlauf: gespeicherte Eintraege laden, dann beobachten.
            app.manage(Clip { liste: Mutex::new(clip_laden(app.handle())), eigen: std::sync::atomic::AtomicI64::new(-1) });
            clip_waechter(app.handle().clone());
            // "Arbeit": die Arbeitswerkzeuge gebuendelt (dieselben Eintraege/Aktionen wie bisher).
            let w_shortcuts = MenuItem::with_id(app, "werk_shortcuts", "Noki Shortcuts", true, None::<&str>)?;
            let w_clip = MenuItem::with_id(app, "werk_clip", "Zwischenablage", true, None::<&str>)?;
            let w_timer = MenuItem::with_id(app, "werk_timer", "Timer", true, None::<&str>)?;
            let w_platz = MenuItem::with_id(app, "werk_platz", "Arbeitsplatz-Fokus", true, None::<&str>)?;
            // Das Menue "Arbeit" gibt es nicht mehr. Die Werkzeuge selbst bleiben
            // (Shortcuts ^0-9, Schnellzugriff, Einstellungen); ihre Menueobjekte
            // werden weiter gepflegt, nur nicht mehr angezeigt.
            let _ = (&w_shortcuts, &fz, &m_freeze, &kamera, &ablage_item, &w_clip, &w_timer, &w_platz);
            let menu = Menu::with_items(
                app,
                &[
                    &einstellungen,
                    &ansicht,
                    &sicht,
                    &trenner,
                    &beenden,
                ],
            )?;

            let energie_haken: Vec<(String, CheckMenuItem<_>)> = energie_stufen
                .iter()
                .zip(energie_items.iter())
                .map(|((id, _), item)| (format!("energie_{}", id), item.clone()))
                .collect();
            app.manage(EnergieMenue(energie_haken.clone()));
            let spaces_item = spaces.clone();
            let ebene_item_h = ebene_item.clone();

            TrayIconBuilder::with_id("noki")
                .icon(tray_symbol())
                .icon_as_template(true)
                .tooltip("Noki")
                .menu(&menu)
                .on_menu_event(move |app, event| match event.id().as_ref() {
                    "einstellungen_oeffnen" => {
                        let _ = app.emit("noki://einstellungen", serde_json::json!({}));
                    }
                    "sicht" => sicht_umschalten(app),
                    // Abschnitt 11: EIN Kanal fuer alle Aktionen, mit dem
                    // Namen als Nutzlast. Eine neue Aktion ist damit ein
                    // Menueeintrag hier und ein Zweig drueben — keine
                    // zweite Bruecke.
                    // Der Haken wandert mit; die Stufe selbst setzt das
                    // Frontend ueber window.NokiEnergie (noki://energie).
                    id if id.starts_with("energie_") => {
                        let stufe = &id["energie_".len()..];
                        let _ = app.emit(
                            "noki://energie",
                            serde_json::json!({ "stufe": stufe }),
                        );
                        for (k, item) in &energie_haken {
                            let _ = item.set_checked(k == id);
                        }
                    }
                    "fz_freigabe" => ax_einstellungen_oeffnen(),
                    id if id.starts_with("fz_") => {
                        if let Ok(n) = id["fz_".len()..].parse::<i64>() {
                            let _ = app.emit("noki://schliessen", serde_json::json!({ "id": n }));
                        }
                    }
                    "freeze_umschalten" => {
                        let _ = app.emit("noki://freeze", serde_json::json!({}));
                    }
                    id if id.starts_with("kamera_") => {
                        let was = if id == "kamera_recording" { "recording" } else { "screenshot" };
                        let _ = app.emit("noki://kamera", serde_json::json!({ "was": was }));
                    }
                    id if id.starts_with("aktion_") => {
                        let name = &id["aktion_".len()..];
                        let _ = app.emit(
                            "noki://aktion",
                            serde_json::json!({ "name": name }),
                        );
                    }
                    "ebene_wechsel" => {
                        let lage = app.state::<Lage>();
                        let neu: &'static str = if *lage.ebene.lock().unwrap() == "hinten" { "vorn" } else { "hinten" };
                        *lage.ebene.lock().unwrap() = neu;
                        // Eine ausdrueckliche Auswahl beendet einen laufenden Durchgang.
                        lage.durchgang.store(false, Ordering::Relaxed);
                        einstellungen_setzen(app, "ebene", serde_json::Value::String(neu.to_string()));
                        let _ = ebene_item_h.set_text(if neu == "hinten" { "In den Vordergrund" } else { "In den Hintergrund" });
                        if let Some(win) = app.get_webview_window(FENSTER) {
                            lage_anwenden(&win, &lage);   // im Vollbild bleibt es vorne
                        }
                    }
                    "alle_spaces" => {
                        let lage = app.state::<Lage>();
                        let neu = !lage.alle_spaces.load(Ordering::Relaxed);
                        lage.alle_spaces.store(neu, Ordering::Relaxed);
                        let _ = spaces_item.set_checked(neu);
                        if let Some(win) = app.get_webview_window(FENSTER) {
                            // Nur das eine Bit: die gewaehlte Ebene bleibt
                            // unangetastet und wirkt unveraendert weiter.
                            alle_spaces_anwenden(&win, neu);
                        }
                        alle_spaces_schreiben(app, neu);
                    }
                    "werk_shortcuts" => werkzeug_zeigen(app, "shortcuts"),
                    "werk_clip" => werkzeug_zeigen(app, "clip"),
                    "werk_timer" => werkzeug_zeigen(app, "timer"),
                    "werk_platz" => werkzeug_zeigen(app, "platz"),
                    "ablage" => ablage_melden(app, &[], None, Some("umschalten")),
                    "beenden" => {
                        if let Some(win) = app.get_webview_window(FENSTER) {
                            ort_merken(app, &win);
                        }
                        app.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;
            #[cfg(target_os = "macos")]
            tasten::registrieren(app.handle());
            #[cfg(target_os = "macos")]
            if let Some(tray) = app.tray_by_id("noki") {
                let h = app.handle().clone();
                let _ = tray.with_inner_tray_icon(move |t| {
                    if let Some(si) = t.ns_status_item() {
                        regler::einbauen(&h, (&*si as *const _) as *mut std::ffi::c_void);
                    }
                });
            }

            // Sicherheitsnetz: normalerweise meldet das Frontend mit
            // noki_bereit, dass Noki steht, und erst dann wird das Fenster
            // sichtbar (sonst blitzt ein leeres Panel auf). Bleibt diese
            // Meldung aus — kein WebGL, ein Fehler in der Seite —, darf die
            // App nicht unsichtbar im Hintergrund haengen bleiben.
            {
                let handle = app.handle().clone();
                thread::spawn(move || {
                    thread::sleep(Duration::from_secs(4));
                    if let Some(win) = handle.get_webview_window(FENSTER) {
                        if win.is_visible().unwrap_or(false) {
                            return;
                        }
                        log::warn!("[Noki] Frontend hat sich nicht gemeldet — Fenster wird trotzdem gezeigt");
                        let _ = win.show();
                    }
                });
            }

            spawn_mouse_watcher(app.handle().clone(), watcher_hit, Arc::clone(&watcher_stop));
            spawn_fenster_watcher(app.handle().clone(), Arc::clone(&watcher_stop));
            // NUR Debug-Build: Live-Diagnose "Fenster schliessen" ohne Tray-Klick.
            // NOKI_TEST_SCHLIESSEN=<App> meldet das erste schliessbare Fenster
            // dieser App — genau derselbe Weg wie ein Menueklick.
            #[cfg(debug_assertions)]
            if let Ok(app_name) = std::env::var("NOKI_TEST_SCHLIESSEN") {
                let h = app.handle().clone();
                thread::spawn(move || {
                    let n: u32 = std::env::var("NOKI_TEST_ANZAHL").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
                    thread::sleep(Duration::from_secs(8));
                    for _ in 0..n {
                        let liste = schliessbare_fenster();
                        eprintln!("[TEST] schliessbar: {:?}", liste);
                        // Nur eigene Testfenster (Titel enthaelt "noki_test") — nie fremde.
                        if let Some((id, l)) = liste.iter().find(|(_, l)| l.contains(&app_name) && l.contains("noki_test")) {
                            eprintln!("[TEST] waehle {} {}", id, l);
                            let _ = h.emit("noki://schliessen", serde_json::json!({ "id": id }));
                        }
                        thread::sleep(Duration::from_secs(35));
                    }
                });
            }
            #[cfg(target_os = "macos")]
            spawn_spotify_watcher(app.handle().clone(), Arc::clone(&watcher_stop));
            // Noki Ablage: echter Ordner, neue fertige Dateien -> bestehende Ablage.
            // Der Schreibtisch-Inhalt ueberlebt den Programmstart; abgeglichen
            // wird er beim ersten Zeigen der Miniatur gegen die Wirklichkeit.
            if virtual_workspace::backend() == virtual_workspace::Backend::LegacyVirtualDisplay
                && virtual_workspace::ready()
            {
                // If READY won the race with persisted-window loading, this
                // side performs the single synchronization. Otherwise the
                // READY callback above does it after loading has completed.
                virtuelle_bereitschaft(app.handle());
            }
            spawn_ablage_ordner(app.handle().clone());
            // NUR Debug-Build: NOKI_TEST_ABLAGE=<pfad1>:<pfad2> nimmt diese Dateien
            // nach 7 s auf — derselbe Weg wie ein Drop, ohne echten Finder-Zug.
            #[cfg(debug_assertions)]
            if let Ok(liste) = std::env::var("NOKI_TEST_ABLAGE") {
                let h = app.handle().clone();
                thread::spawn(move || {
                    thread::sleep(Duration::from_secs(7));
                    let pfade: Vec<PathBuf> = liste.split(':').filter(|s| !s.is_empty()).map(PathBuf::from).collect();
                    eprintln!("[TEST] ablage: {:?}", pfade);
                    ablage_aufnehmen(&h, &pfade);
                });
            }
            // NUR Debug-Build: NOKI_TEST_TASTE=<n>@<s>,... loest das Kuerzel Ctrl+n
            // nach s Sekunden aus — derselbe Weg wie der echte Tastendruck.
            stockungs_wache(app.handle());
            hauptfaden_puls(app.handle());
            #[cfg(debug_assertions)]
            if let Ok(plan) = std::env::var("NOKI_TEST_TASTE") {
                let h = app.handle().clone();
                thread::spawn(move || {
                    let mut t0 = 0u64;
                    // NOKI_TEST_KANAL=<datei>: nach dem festen Plan weitere
                    // Befehle zeilenweise aus dieser Datei (angehaengt), ohne
                    // Zeitangabe sofort. Nur Debug-Build, wie der Plan selbst.
                    let feste: Vec<String> = plan.split(',').filter(|s| !s.is_empty()).map(str::to_owned).collect();
                    let kanal = std::env::var("NOKI_TEST_KANAL").ok().into_iter().flat_map(|pfad| {
                        // Only lines appended AFTER this start count. A stale
                        // channel (env left in launchd by an old run) replayed
                        // `wswahl:vor` x100 on every cold start = visible
                        // Desktop cycling without any user input.
                        let mut gelesen = fs::metadata(&pfad).map(|m| m.len() as usize).unwrap_or(0);
                        eprintln!("[TEST] kanal {pfad} start_offset={gelesen}");
                        std::iter::from_fn(move || loop {
                            let text = fs::read_to_string(&pfad).unwrap_or_default();
                            if let Some(zeile) = text.get(gelesen..).and_then(|rest| rest.split_once('\n')).map(|(z, _)| z.to_owned()) {
                                gelesen += zeile.len() + 1;
                                if !zeile.trim().is_empty() { return Some(zeile.trim().to_owned()); }
                                continue;
                            }
                            thread::sleep(Duration::from_millis(150));
                        })
                    });
                    for teil in feste.into_iter().chain(kanal) {
                        let teil = teil.as_str();
                        let mut it = teil.split('@');
                        let was = it.next().unwrap_or("");
                        let t: u64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(t0);
                        thread::sleep(Duration::from_secs(t.saturating_sub(t0)));
                        t0 = t;
                        // "m" = Menuepunkt "Ablage" (derselbe Weg wie der Klick oben im Menue).
                        // Debug only: evaluate JS in Noki's window; results via noki_log.
                        if let Some(code) = was.strip_prefix("js:") {
                            if let Some(w) = h.get_webview_window(FENSTER) {
                                let _ = w.eval(code);
                            }
                            continue;
                        }
                        if was == "m" {
                            eprintln!("[TEST] Menue Ablage bei {} s", t);
                            ablage_melden(&h, &[], None, Some("umschalten"));
                            continue;
                        }
                        // "klick" = derselbe Weg wie ein Klick auf den
                        // Schreibtisch-Inhalt; "diag" = Stand der Vorschau;
                        // "wsopen:<ziel>" = eine Aufgabe auf Nokis Schreibtisch.
                        // Alle drei nur im Debug-Build, ueber NOKI_TEST_TASTE.
                        // "sprich:<Satz>" spielt einen FERTIGEN Transkript-
                        // Satz ein - denselben Ereignisweg, den der
                        // Spracherkenner nimmt. Nur so laesst sich die
                        // Kette ab "verstanden" ohne Mikrofon pruefen.
                        if let Some(satz) = was.strip_prefix("sprich:") {
                            let _ = h.emit(
                                "intelligence-voice",
                                serde_json::json!({ "text": satz, "final": true }),
                            );
                            eprintln!("[TEST] sprich {satz}");
                            continue;
                        }
                        // Same state transition as the helper's
                        // `INTERACTION an` message. This debug-only hook lets
                        // acceptance verify active-app identity and the key
                        // firewall even when UI automation cannot address the
                        // borderless helper window as an accessibility app.
                        if was == "interaction" {
                            fern_tippen::klick_in_ansicht();
                            noki_interaction_aktivieren(&h);
                            continue;
                        }
                        if was == "klick" { let _ = h.emit("noki://spur_klick", serde_json::json!({})); continue; }
                        if was == "groesse" { let _ = h.emit("noki://test_groesse", serde_json::json!({})); continue; }
                        if was == "kuerzel4" { let _ = h.emit("noki://test_kuerzel4", serde_json::json!({})); continue; }
                        // "voll:an|aus" = derselbe Weg wie "Zum Schreibtisch" / Schliessen.
                        if let Some(an) = was.strip_prefix("voll:") { vorschau::voll(an == "an"); continue; }
                        // "leiste:<neu|vorn|zu|min|max>:<arg>" nimmt GENAU den
                        // Weg eines Klicks in der App-Leiste / im Apps-Picker.
                        if let Some(rest) = was.strip_prefix("leiste:") {
                            if let Some((w, a)) = rest.split_once(':') {
                                eprintln!("[TEST] leiste {w} {a}");
                                leiste_befehl(&h, w, a);
                            }
                            continue;
                        }
                        // "wswahl:vor|zurueck" nimmt GENAU den Weg, den
                        // LINKS-VON-1 + Pfeil nimmt. Die Praefix-Taste selbst
                        // laesst sich von aussen nicht nachstellen (sie wird
                        // als flagsChanged gelesen, nicht als Tastendruck) -
                        // geprueft wird deshalb alles dahinter.
                        // "fern:<n>" fuehrt n verdeckte Vorgaenge aus - genau
                        // die Bewegung, die ein Klick in die Miniatur macht.
                        // Geprueft wird damit die ZUSAGE: jeder Vorgang endet
                        // wieder genau am Ausgangsschreibtisch.
                        if let Some(args) = was.strip_prefix("fernclick:") {
                            let z: Vec<&str> = args.split(':').collect();
                            if z.len() == 4 || z.len() == 5 {
                                let ok = vorschau::debug_fern_klick(
                                    &h,
                                    z[0].parse().unwrap_or(0),
                                    z[1].parse().unwrap_or(0.0),
                                    z[2].parse().unwrap_or(0.0),
                                    z[3].parse().unwrap_or(0),
                                    z.get(4).and_then(|k| k.parse().ok()).unwrap_or(1),
                                );
                                eprintln!("[TEST] fernclick {} queued={ok}", z[3]);
                            }
                            continue;
                        }
                        if let Some(args) = was.strip_prefix("fernzieh:") {
                            let z: Vec<f64> = args.split(':').filter_map(|v| v.parse().ok()).collect();
                            if z.len() == 5 || z.len() == 6 {
                                let ok = vorschau::debug_fern_ziehen(&h, z[0] as i64, z[1], z[2], z[3], z[4], z.get(5).copied().unwrap_or(0.0) as i64);
                                eprintln!("[TEST] fernzieh queued={ok}");
                            }
                            continue;
                        }
                        if let Some(args) = was.strip_prefix("fernschwebe:") {
                            let z: Vec<f64> = args.split(':').filter_map(|v| v.parse().ok()).collect();
                            if z.len() == 3 { let _ = vorschau::debug_fern_schweben(z[0] as i64, z[1], z[2]); }
                            continue;
                        }
                        if let Some(args) = was.strip_prefix("fernrad:") {
                            let z: Vec<&str> = args.split(':').collect();
                            if z.len() == 5 {
                                let ok = vorschau::debug_fern_rad(
                                    &h,
                                    z[0].parse().unwrap_or(0),
                                    z[1].parse().unwrap_or(0.0),
                                    z[2].parse().unwrap_or(0.0),
                                    z[3].parse().unwrap_or(0),
                                    z[4].parse().unwrap_or(0),
                                );
                                eprintln!("[TEST] fernrad {},{} queued={ok}", z[3], z[4]);
                            }
                            continue;
                        }
                        // "ferntaste:<keycode>:<flags>" enters at the same
                        // router boundary as the physical event tap. Debug
                        // acceptance only; the production delivery path is
                        // unchanged and still re-verifies the exact target.
                        if let Some(t) = was.strip_prefix("ferntext:") {
                            let t = t.replace("\\n", "\n");
                            let ok = fern_tippen::debug_text(&t);
                            eprintln!("[TEST] ferntext chars={} handled={ok} target={}", t.chars().count(), fern_tippen::remote_ziel());
                            continue;
                        }
                        if was == "remoteziel" { eprintln!("[TEST] remoteTarget={}", fern_tippen::remote_ziel()); continue; }
                        if was == "overview" {
                            let h2 = h.clone();
                            let _ = h.run_on_main_thread(move || window_overview::toggle(&h2));
                            continue;
                        }
                        if let Some(w) = was.strip_prefix("vsczustand:").and_then(|w| w.parse::<i64>().ok()) {
                            let z = vscode_bruecke::fuer_fenster(w).and_then(|b| vscode_bruecke::zustand(&b));
                            eprintln!("[TEST] vsczustand wid={w} {}", z.map(|v| v.to_string()).unwrap_or_else(|| "none".into()));
                            continue;
                        }
                        if let Some(args) = was.strip_prefix("ferntaste:") {
                            let z: Vec<&str> = args.split(':').collect();
                            if z.len() == 2 {
                                let code = z[0].parse().unwrap_or(0);
                                let flags = z[1].parse().unwrap_or(0);
                                let ok = fern_tippen::debug_taste(code, flags);
                                eprintln!("[TEST] ferntaste code={code} flags={flags:#x} handled={ok}");
                            }
                            continue;
                        }
                        // Deterministic acceptance sequence: deliver the real
                        // remote click first, wait until that exact owned
                        // window is the verified typing target, then enter at
                        // the normal key router boundary. This removes only a
                        // test-channel race; all production checks and event
                        // delivery remain unchanged.
                        if let Some(args) = was.strip_prefix("ferntasteauf:") {
                            let z: Vec<&str> = args.split(':').collect();
                            if z.len() == 5 {
                                let wid = z[0].parse().unwrap_or(0);
                                let x = z[1].parse().unwrap_or(0.0);
                                let y = z[2].parse().unwrap_or(0.0);
                                let code = z[3].parse().unwrap_or(0);
                                let flags = z[4].parse().unwrap_or(0);
                                let generation = fern_tippen::debug_generation();
                                let queued = vorschau::debug_fern_klick(&h, wid, x, y, 9000 + code as u64, 1);
                                thread::spawn(move || {
                                    let bis = std::time::Instant::now() + Duration::from_millis(1500);
                                    while std::time::Instant::now() < bis
                                        && (fern_tippen::debug_generation() == generation
                                            || fern_tippen::debug_ziel_wid() != Some(wid))
                                    {
                                        thread::sleep(Duration::from_millis(10));
                                    }
                                    let locked = fern_tippen::debug_generation() != generation
                                        && fern_tippen::debug_ziel_wid() == Some(wid);
                                    let ok = locked && fern_tippen::debug_taste(code, flags);
                                    eprintln!("[TEST] ferntasteauf wid={wid} code={code} flags={flags:#x} queued={queued} locked={locked} handled={ok}");
                                });
                            }
                            continue;
                        }
                        if let Some(n) = was.strip_prefix("fern:") {
                            // "fern:<n>[:<ms>]" - optional Verweildauer am Ziel.
                            let (n, ms) = n.split_once(':').unwrap_or((n, "0"));
                            let verweilen = Duration::from_millis(ms.parse().unwrap_or(0));
                            let n: u32 = n.parse().unwrap_or(1);
                            let hh = h.clone();
                            thread::spawn(move || {
                                for i in 1..=n {
                                    let vorher = cgs::aktiver_space().map(|(a, _)| a).unwrap_or(0);
                                    let ok = auf_arbeitsplatz_handeln(&hh, || thread::sleep(verweilen)).is_some();
                                    thread::sleep(Duration::from_millis(250));
                                    let nachher = cgs::aktiver_space().map(|(a, _)| a).unwrap_or(0);
                                    eprintln!(
                                        "[FERNTEST] {i}/{n} ausgefuehrt={ok} vorher={vorher} nachher={nachher} {}",
                                        if vorher == nachher { "OK" } else { "ABWEICHUNG" }
                                    );
                                }
                            });
                            continue;
                        }
                        if was == "zustand" { vorschau::zustand_anfordern(); continue; }
                        // Raw helper command (scroll acceptance: testrad / rollmessung).
                        if let Some(c) = was.strip_prefix("helfer:") { vorschau::befehl(c); continue; }
                        if was == "youtube:start" {
                            let r = noki_browser::youtube_starten();
                            virtual_workspace::trace(&format!("[TEST] youtube start -> {r:?}"));
                            continue;
                        }
                        if let Some(url) = was.strip_prefix("browser:start:") {
                            let r = noki_browser::starten(url);
                            virtual_workspace::trace(&format!("[TEST] browser start -> {r:?}"));
                            continue;
                        }
                        if let Some(a) = was.strip_prefix("axmiss:") {
                            let t: Vec<i64> = a.split(':').filter_map(|x| x.parse().ok()).collect();
                            if t.len() == 3 { vorschau::fernbedienung::fehlschlag_simulieren(t[0] as i32, t[1], t[2] as u64); }
                            continue;
                        }
                        if let Some(w) = was.strip_prefix("browser:state:") {
                            let z = noki_browser::zustand(w.parse().unwrap_or(0));
                            virtual_workspace::trace(&format!("[TEST] browser state {w} -> {}", z.unwrap_or_default()));
                            continue;
                        }
                        // "besuchen" = exactly the footer's backend ("Zum Schreibtisch N").
                        if was == "besuchen" {
                            fuss_freigabe_erteilen("EXPLICIT_FOOTER_CLICK");
                            let v = arbeitsplatz_besuchen(&h);
                            virtual_workspace::trace(&format!("[TEST] besuchen -> {v}"));
                            continue;
                        }
                        // Acceptance-only recovery to an exact, already
                        // observed Space.  Arrow direction is not a stable
                        // inverse when a fullscreen Space sits beside the
                        // Noki Desktop; tests must verify the id, not guess a
                        // second swipe.  This hook is compiled only inside
                        // the existing debug channel and is never exposed to
                        // production UI input.
                        if let Some(id) = was.strip_prefix("space:").and_then(|s| s.parse::<u64>().ok()) {
                            test_exakter_space(id);
                            continue;
                        }
                        if let Some(richtung) = was.strip_prefix("wswahl:") {
                            let vor = richtung == "vor";
                            arbeitsplatz_wahl_planen(&h, vor);
                            continue;
                        }
                        // "wsopen" = eine echte Oeffnen-Aktion AUF Nokis
                        // Schreibtisch, mit demselben Befehl wie eine Aufgabe.
                        if let Some(ziel) = was.strip_prefix("wsopen:") {
                            let r = arbeitsplatz_oeffnen_blockierend(
                                h.clone(), ziel.to_owned(), "Google Chrome".into(),
                                Some("/Applications/Google Chrome.app".into()));
                            eprintln!("[TEST] wsopen {ziel} -> {r}");
                            // Wie im Frontend: die Aktion ist abgesetzt, die
                            // Ortsbindung faellt genau einmal.
                            let _ = noki_arbeitsplatz_aufgabe_beenden();
                            continue;
                        }
                        if let Some(rest) = was.strip_prefix("wsdiag") {
                            let _ = rest;
                            let diagnose = noki_arbeitsplatz_diagnose(h.clone());
                            eprintln!("[TEST] wsdiag {diagnose}");
                            virtual_workspace::trace(&format!("[HEALTH_SNAPSHOT] {diagnose}"));
                            continue;
                        }
                        if was == "diag" {
                            #[cfg(target_os = "macos")]
                            eprintln!("[TEST] aktiver_space={:?} arbeitsplatz={:?}",
                                cgs::aktiver_space(),
                                ARBEITSPLATZ.lock().ok().and_then(|g| g.as_ref().map(|r| (r.id, r.uuid.clone()))));
                            let _ = h.emit("noki://spur_diag", serde_json::json!({}));
                            continue;
                        }
                        if was == "settings" { einstellungen_oeffnen(h.clone()); continue; }
                        if let Some(liste) = was.strip_prefix("focus:") {
                            let apps: Vec<String> = liste.split('+').filter_map(|a| match a {
                                "chrome" => Some("/Applications/Google Chrome.app"),
                                "githubdesktop" => Some("/Applications/GitHub Desktop.app"),
                                "github" => Some("/Applications/GitHub.app"),
                                "grapher" => Some("/System/Applications/Utilities/Grapher.app"),
                                "craft" => Some("/Applications/Craft.app"),
                                "goodnotes" => Some("/Applications/Goodnotes.app"),
                                "activity" => Some("/System/Applications/Utilities/Activity Monitor.app"),
                                _ => None,
                            }).map(str::to_string).collect();
                            let space = cgs::aktiver_space().map(|x| x.0).unwrap_or(0);
                            let r = fokus_sitzung::starten(&apps, "normal", space, || cgs::aktiver_space().map(|x| x.0).unwrap_or(0));
                            virtual_workspace::trace(&format!("[TEST] focus acceptance -> {r}"));
                            continue;
                        }
                        if was == "focusend" {
                            let r = fokus_sitzung::beenden();
                            virtual_workspace::trace(&format!("[TEST] focus end -> {r}"));
                            continue;
                        }
                        // kurz:<n> — Kuerzel n ueber denselben Router wie die echte Taste.
                        if let Some(n) = was.strip_prefix("kurz:").and_then(|n| n.parse::<u32>().ok()) { shortcut_einreihen(&h, n); continue; }
                        if was == "shortcuts" { werkzeug_zeigen(&h, "shortcuts"); continue; }
                        let n: u32 = was.parse().unwrap_or(0);
                        eprintln!("[TEST] Taste Ctrl+{} bei {} s", n, t);
                        shortcut_einreihen(&h, n);
                    }
                });
            }

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(move |app, event| match event {
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => noki_zeigen(app),
            tauri::RunEvent::Exit => {
                stop.store(true, Ordering::Relaxed);
                // Noki Talk: microphone closed, local Whisper model unloaded.
                noki_talk::alles_beenden();
                #[cfg(target_os = "macos")]
                vorschau::fernbedienung::ax_manuell_alle_aus();
                #[cfg(target_os = "macos")]
                blende::zu(app);
                virtual_workspace::stop();
                // Laufende Bildschirmaufnahme sauber abschliessen.
                if let Some(a) = app.try_state::<Aufnahme>() { let _ = aufnahme_stoppen(&a); }
                // Lokales Modell vor dem Prozessende freigeben (sonst bricht ggml/Metal beim Beenden ab).
                if let Some(i) = app.try_state::<std::sync::Arc<intelligence::Intelligence>>() { i.shutdown(); }
            }
            // Schliessen verbirgt das eine persistente Hauptfenster. Dock-
            // oder Menueleisten-Aktion zeigt exakt dieselbe Instanz wieder.
            tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } if label == FENSTER => {
                api.prevent_close();
                if let Some(win) = app.get_webview_window(FENSTER) {
                    sicht_wunsch_setzen(app, false);   // Schliessen = Nutzerwunsch
                    let _ = win.hide();
                }
            }
            // Der native Close-Knopf verbirgt Ask, damit Verlauf und
            // Terminal-Sitzung in derselben Window-Instanz erhalten bleiben.
            tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } if label == ASK_FENSTER => {
                api.prevent_close();
                ask_fenster_verbergen(app.clone());
            }
            // Einstellungen: Schliessen verbirgt (Fenster bleibt fuer schnelles
            // Wieder-Oeffnen); das Hauptfenster setzt seinen Zustand zurueck.
            tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } if label == EINST_FENSTER => {
                api.prevent_close();
                if let Some(w) = app.get_webview_window(EINST_FENSTER) { let _ = w.hide(); }
                let _ = app.emit_to(FENSTER, "noki://einstellungen_zu", serde_json::json!({}));
            }
            // Nur Display-/Space-Aenderungen des transparenten Overlays
            // aktualisieren den Desktop-/Monitorraum des Characters.
            tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_),
                ..
            } if label == FENSTER => {
                let _ = app.emit("noki://fensterraum", serde_json::json!({}));
            }
            // Ablage: Dateien aus dem Finder auf Noki fallen lassen.
            tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }),
                ..
            } if label == FENSTER => ablage_drop(app, paths),
            _ => {}
        });
}

#[cfg(all(test, target_os = "macos"))]
mod ablage_tests {
    use super::*;

    #[test]
    fn ablage_referenz_zyklus() {
        let dir = std::env::temp_dir().join(format!("noki_ablage_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.pdf");
        fs::write(&a, b"inhalt-a").unwrap();
        let b = dir.join("b.png");
        fs::write(&b, b"inhalt-b").unwrap();
        let o = dir.join("ordner");
        fs::create_dir_all(&o).unwrap();

        // Mehrere Dateien auf einmal, neueste zuerst, jede mit Lesezeichen.
        let mut l = Vec::new();
        let (neu, dup) = ablage_neu(&mut l, &[a.clone(), b.clone(), o.clone()], 1);
        assert_eq!(neu.len(), 3);
        assert!(dup.is_none());
        assert_eq!(
            (l[0].typ.as_str(), l[1].typ.as_str(), l[2].typ.as_str()),
            ("ordner", "bild", "dokument")
        );
        assert!(l.iter().all(|e| !e.bm.is_empty()), "Lesezeichen fehlt");

        // Dieselbe Datei erneut: nicht doppelt, sondern nach vorn.
        let (neu2, dup2) = ablage_neu(&mut l, &[a.clone()], 2);
        assert!(neu2.is_empty());
        assert_eq!(l.len(), 3);
        assert_eq!(dup2, Some(l[0].id));
        assert_eq!(l[0].name, "a.pdf");

        // Persistenz-Rundlauf ueber JSON.
        let roh = serde_json::to_string(&l).unwrap();
        let mut l2 = ablage_aus_json(&roh);
        assert_eq!(l2.len(), 3);

        // Umbenannt: das Lesezeichen findet die Datei wieder.
        let a2 = dir.join("a-umbenannt.pdf");
        fs::rename(&a, &a2).unwrap();
        for e in l2.iter_mut() {
            ablage_aufloesen(e);
        }
        let ea = l2.iter().find(|e| e.ext == "pdf").unwrap();
        assert!(
            ea.da && ea.pfad.ends_with("a-umbenannt.pdf"),
            "Lesezeichen folgt nicht: {}",
            ea.pfad
        );

        // Geloescht: kein Absturz, Eintrag bleibt als "nicht verfuegbar".
        fs::remove_file(&b).unwrap();
        for e in l2.iter_mut() {
            ablage_aufloesen(e);
        }
        assert!(!l2.iter().find(|e| e.ext == "png").unwrap().da);

        // Aus der Ablage entfernen laesst das Original unberuehrt.
        l2.retain(|e| e.ext != "pdf");
        assert_eq!(fs::read(&a2).unwrap(), b"inhalt-a");
        l2.clear();
        assert!(a2.exists() && o.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn kamera_ordner() {
        let desktop =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.local/test-desktop");
        let p = schreibtisch_datei_in("Noki-Screenshot", "png", &desktop).unwrap();
        assert_eq!(
            p.parent().and_then(|d| d.file_name()).unwrap(),
            "Noki Kamera"
        );
        assert!(p.parent().unwrap().is_dir());
        let n = p.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            n.starts_with("Noki-Screenshot-")
                && n.ends_with(".png")
                && n.len() == "Noki-Screenshot-2026-09-14-120000.png".len(),
            "{n}"
        );
    }

    #[test]
    fn zwischenablage_regeln() {
        let t = |id: u64, s: &str| ClipEintrag {
            id,
            art: if clip_ist_link(s) {
                "link".into()
            } else {
                "text".into()
            },
            text: s.into(),
            ..Default::default()
        };
        let mut l = Vec::new();
        assert!(clip_einordnen(&mut l, t(1, "hallo")).0);
        assert!(
            !clip_einordnen(&mut l, t(2, "hallo")).0,
            "unmittelbar gleicher Inhalt doppelt"
        );
        assert!(clip_einordnen(&mut l, t(3, "https://example.org/x")).0);
        assert_eq!(l[0].art, "link");
        assert!(!clip_ist_link("https://a b") && !clip_ist_link("hallo welt"));
        for i in 0..30 {
            clip_einordnen(&mut l, t(10 + i, &format!("e{i}")));
        }
        assert_eq!(l.len(), CLIP_MAX);
        assert_eq!(b64(b"Noki"), "Tm9raQ==");
        assert_eq!(b64(b"ab"), "YWI=");
    }

    #[test]
    fn arbeitsplatz_fokus_auf_8_und_9_frei() {
        assert_eq!(super::werkzeug_fuer(8), Some("platz_schnell"));
        assert_eq!(super::werkzeug_fuer(9), None);
        assert_eq!(super::werkzeug_fuer(7), Some("timer"));
    }

    /// Kuerzel 4 ist eine reine Sichtbarkeitsschaltung.
    ///
    /// Geprueft wird die Quelle des Zweiges selbst: er darf NUR das
    /// Vorschau-Ereignis aussenden. Jeder Space-Aufruf, jeder Besuch und
    /// jeder Rueckweg an dieser Stelle waere genau der Fehler, den der
    /// Nutzer gemeldet hat - dass eine Sichtbarkeitstaste den Bildschirm
    /// wegschaltet. Ein Ausfuehrungstest kaeme dafuer zu spaet: er
    /// braeuchte einen laufenden WindowServer.
    #[test]
    fn kuerzel_vier_schaltet_nur_die_vorschau() {
        let quelle = include_str!("lib.rs");
        let start = quelle
            .find("fn shortcut_ausloesen")
            .expect("shortcut_ausloesen vorhanden");
        let rumpf = &quelle[start..];
        let vier = rumpf.find("\n        4 => {").expect("Zweig 4 vorhanden");
        let fuenf = rumpf.find("\n        5 => ").expect("Zweig 5 vorhanden");
        assert!(vier < fuenf);
        let zweig = &rumpf[vier..fuenf];
        assert!(
            zweig.contains("noki://vorschau_toggle"),
            "Kuerzel 4 sendet das Sichtbarkeits-Ereignis"
        );
        for verboten in [
            "zum_space",
            "arbeitsplatz_besuchen",
            "rueckweg",
            "ARBEITSPLATZ_AUFGABE",
            "office_toggle",
            "aktiviere",
        ] {
            assert!(
                !zweig.contains(verboten),
                "Kuerzel 4 darf {verboten} nicht beruehren: {zweig}"
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn miniatur_hostet_normale_und_native_vollbild_spaces() {
        let noki = 200;
        assert!(super::vorschau_space_host(0, 100, noki, false));
        assert!(!super::vorschau_space_host(0, noki, noki, false));
        assert!(super::vorschau_space_host(0, noki, noki, true));
        assert!(super::vorschau_space_host(4, 300, noki, false));
        assert!(!super::vorschau_space_host(2, 400, noki, false));
        assert!(!super::vorschau_space_host(7, 500, noki, false));
    }

    #[test]
    fn real_space_besuch_hat_keine_vollbild_preview() {
        let html = include_str!("../../index.html");
        let start = html.find("function vorschauBesuchen()").unwrap();
        let end = html[start..].find("\n  /// Zeigen.").map(|i| start + i).unwrap();
        let visit = &html[start..end];
        let guard = "if (vorschau.backend === \"LEGACY_VIRTUAL_DISPLAY\")";
        let guarded = visit.find(guard).expect("Vollbildweg ist Legacy-only");
        let invoke = visit.find("noki_arbeitsplatz_besuchen").unwrap();
        assert!(guarded < invoke);
        assert!(!visit[..guarded].contains("noki-vorschau-navigation"));
    }

    /// Abschnitt 6: Kuerzel 4 darf die Figur NICHT bewegen.
    ///
    /// Der einzige Weg, auf dem die Figur den Schreibtisch wechselt, ist der
    /// Umzugsblock im Space-Waechter. Geprueft wird deshalb, dass der
    /// Kuerzel-4-Zweig weder ihn noch eine seiner Zutaten beruehrt - und
    /// dass die Miniatur-Sichtbarkeit dort, wo ueber einen Umzug entschieden
    /// wird, gar nicht vorkommt. Waere sie eine Eingangsgroesse, koennte ein
    /// Sichtbarkeitswechsel einen Flug ausloesen; genau das wurde gemeldet.
    #[test]
    fn kuerzel_vier_bewegt_noki_nicht_zwischen_schreibtischen() {
        let quelle = include_str!("lib.rs");
        let start = quelle.find("fn shortcut_ausloesen").unwrap();
        let rumpf = &quelle[start..];
        let vier = rumpf.find("\n        4 => {").unwrap();
        let fuenf = rumpf.find("\n        5 => ").unwrap();
        let zweig = &rumpf[vier..fuenf];
        for verboten in ["NOKI_SPACE", "space_umziehen", "UMZUG_ZIEL", "noki://space", "EigenerWechsel"] {
            assert!(!zweig.contains(verboten), "Kuerzel 4 darf {verboten} nicht beruehren");
        }
        // Die Umzugsentscheidung kennt die Sichtbarkeit der Miniatur nicht.
        let w0 = quelle.find("fn space_waechter").unwrap();
        let w1 = quelle[w0..].find("\n}\n").map(|i| w0 + i).unwrap_or(quelle.len());
        let waechter = &quelle[w0..w1];
        let entscheidung = &waechter[waechter.find("let ziel_space").unwrap()..];
        assert!(
            !entscheidung.contains("VORSCHAU_SICHTBAR"),
            "die Sichtbarkeit der Miniatur darf keinen Umzug bestimmen"
        );
    }

    /// Abschnitt 8: zwei Ereignisse zum selben Ziel sind EIN Umzug.
    #[test]
    fn ein_laufender_umzug_wird_nicht_doppelt_angestossen() {
        let ziel = 2730u64;
        super::UMZUG_ZIEL.store(0, Ordering::SeqCst);
        let anstossen = |z: u64| {
            if super::UMZUG_ZIEL.load(Ordering::SeqCst) == z {
                return false;
            }
            super::UMZUG_ZIEL.store(z, Ordering::SeqCst);
            true
        };
        assert!(anstossen(ziel), "der erste Auftrag zaehlt");
        assert!(!anstossen(ziel), "sein Echo nicht");
        assert!(!anstossen(ziel), "und auch das zweite Echo nicht");
        super::UMZUG_ZIEL.store(0, Ordering::SeqCst); // angekommen
        assert!(anstossen(ziel), "danach ist ein neuer Auftrag wieder moeglich");
        super::UMZUG_ZIEL.store(0, Ordering::SeqCst);
    }

    /// Abschnitt 7: Nokis eigener Kurzbesuch ist kein Umzug des Nutzers.
    #[test]
    fn nokis_eigener_bildschirmwechsel_gilt_nicht_als_nutzerbewegung() {
        assert!(!super::eigener_wechsel_laeuft());
        {
            let _a = super::EigenerWechsel::neu();
            assert!(super::eigener_wechsel_laeuft());
            {
                let _b = super::EigenerWechsel::neu();
                assert!(super::eigener_wechsel_laeuft(), "verschachtelt bleibt es gesetzt");
            }
            assert!(super::eigener_wechsel_laeuft(), "die aeussere Klammer haelt weiter");
        }
        assert!(!super::eigener_wechsel_laeuft(), "am Ende ist die Klammer zu");
    }

    /// Abschnitt 2/3: der Schreibtisch-Inhalt ueberlebt das Aufgabenende.
    ///
    /// Die Aufgabe ist voruebergehend, der Schreibtisch nicht. Wurden beide
    /// zusammen geleert, sah der Nutzer auf Schreibtisch 4 ein echtes
    /// Fenster, in der Miniatur aber nichts.
    #[test]
    fn das_aufgabenende_raeumt_den_schreibtisch_nicht_leer() {
        let inhalt = vec![
            super::EigenesFenster { fenster: 367466, pid: 80249, app: "Google Chrome".into(), ..Default::default() },
        ];
        if let Ok(mut g) = super::ARBEITSPLATZ_FENSTER.lock() {
            *g = inhalt.clone();
        }
        if let Ok(mut t) = super::TASK_FENSTER.lock() {
            *t = vec![367466];
        }
        let r = super::noki_arbeitsplatz_aufgabe_beenden();
        assert_eq!(r["freigegeben"], 1, "die Aufgabe gibt ihr Fenster frei");
        assert_eq!(r["schreibtisch_fenster"], 1, "der Schreibtisch behaelt es");
        assert!(super::TASK_FENSTER.lock().unwrap().is_empty());
        assert_eq!(super::arbeitsplatz_fenster_liste(), inhalt);
        assert!(!super::ARBEITSPLATZ_AUFGABE.load(Ordering::Relaxed), "die Ortsbindung faellt");
        if let Ok(mut g) = super::ARBEITSPLATZ_FENSTER.lock() {
            g.clear();
        }
    }

    /// Abschnitt 3/19: der Abgleich behaelt Eigenes und vereinnahmt nichts.
    #[test]
    fn der_abgleich_behaelt_eigenes_und_laesst_fremdes_fremd() {
        let gemerkt = vec![
            super::EigenesFenster { fenster: 100, pid: 7, app: "Google Chrome".into(), ..Default::default() },
            super::EigenesFenster { fenster: 101, pid: 7, app: "Google Chrome".into(), ..Default::default() },
            super::EigenesFenster { fenster: 102, pid: 9, app: "Code".into(), ..Default::default() },
        ];
        // Was wirklich auf dem Schreibtisch liegt.
        let echte: Vec<(i64, i32, String)> = vec![
            (100, 7, "Google Chrome".into()), // unveraendert eigenes
            (101, 8, "Google Chrome".into()), // Nummer neu vergeben: FREMD
            (200, 9, "Code".into()),          // fremdes Fenster des Nutzers
        ];
        let behalten: Vec<i64> = gemerkt
            .iter()
            .filter(|e| {
                echte
                    .iter()
                    .any(|(w, p, o)| *w == e.fenster && *p == e.pid && o.as_str() == e.app)
            })
            .map(|e| e.fenster)
            .collect();
        assert_eq!(behalten, vec![100], "nur das nachweislich eigene bleibt");
        assert!(!behalten.contains(&101), "eine neu vergebene Nummer ist keine Herkunft");
        assert!(!behalten.contains(&102), "ein geschlossenes Fenster faellt heraus");
        assert!(!behalten.contains(&200), "ein fremdes Fenster wird nie beansprucht");
    }

    /// Abschnitt 8: Freeze (Kuerzel 3) ist vollstaendig unbeteiligt.
    ///
    /// Der Nutzer vermutet, waehrend eines schlechten Laufs versehentlich
    /// Kuerzel 3 gedrueckt zu haben. Geprueft wird deshalb an der Quelle,
    /// dass dieser Zweig und die dahinterliegende Freeze-Mechanik nichts
    /// beruehren, was mit Schreibtischen, der Miniatur oder dem Arbeitsplatz
    /// zu tun hat. Freeze holt hoechstens das zuvor vorderste PROGRAMM
    /// zurueck - es verschiebt kein Fenster und wechselt keinen Space.
    #[test]
    fn freeze_ist_von_der_arbeitsplatz_navigation_vollstaendig_getrennt() {
        let quelle = include_str!("lib.rs");
        let start = quelle.find("fn shortcut_ausloesen").unwrap();
        let rumpf = &quelle[start..];
        let drei = rumpf.find("\n        3 => {").unwrap();
        let vier = rumpf.find("\n        4 => {").unwrap();
        assert!(drei < vier);
        let zweig = &rumpf[drei..vier];
        assert!(zweig.contains("noki://freeze"), "Kuerzel 3 loest Freeze aus");
        for verboten in [
            "zum_space", "sichtbar_zum_space", "arbeitsplatz", "ARBEITSPLATZ",
            "vorschau", "VORSCHAU", "NOKI_SPACE", "space_umziehen", "UMZUG_ZIEL",
        ] {
            assert!(
                !zweig.contains(verboten),
                "Kuerzel 3 darf {verboten} nicht beruehren: {zweig}"
            );
        }
        // Und die Freeze-Mechanik selbst.
        let f0 = quelle.find("fn freeze_fenster").unwrap();
        let f1 = quelle[f0..].find("\n/// Ziel fuer Schwingen").unwrap();
        let freeze = &quelle[f0..f0 + f1];
        for verboten in [
            "zum_space", "sichtbar_zum_space", "CGSManagedDisplaySetCurrentSpace",
            "hinzufuegen", "entfernen", "ARBEITSPLATZ", "vorschau",
        ] {
            assert!(
                !freeze.contains(verboten),
                "freeze_fenster darf {verboten} nicht beruehren"
            );
        }
    }

    /// Abschnitt 5/26: jeder sichtbare Schreibtischwechsel hat einen GRUND.
    ///
    /// Erlaubt sind genau zwei, und beide liegen in genau einer Funktion:
    ///   * EXPLICIT_PREVIEW_VISIT  - `arbeitsplatz_besuchen` (der Klick),
    ///   * BOUNDED_WORKSPACE_PREP  - `noki_arbeitsplatz_oeffnen` (hin, Fenster
    ///     erzeugen, zurueck).
    /// Jede weitere Stelle waere eine zweite Moeglichkeit, den Nutzer
    /// ungefragt umzusetzen - Waechter, Freeze, Kuerzel 4, Groesse und
    /// Aufgabenende duerfen hier nie auftauchen.
    #[test]
    fn jeder_sichtbare_schreibtischwechsel_hat_einen_erlaubten_grund() {
        let ganze = include_str!("lib.rs");
        // Nur der Produktionsteil zaehlt: die Testtexte weiter unten nennen
        // die Funktion ja selbst und waeren sonst ihre eigenen "Aufrufer".
        let quelle = &ganze[..ganze.find("\nmod ablage_tests {").unwrap_or(ganze.len())];
        // In welcher Funktion steht eine Fundstelle?
        let funktion_um = |i: usize| -> &str {
            // Der naechste "fn " VOR der Fundstelle benennt die Funktion,
            // in der sie steht.
            let davor = &quelle[..i];
            let Some(start) = davor.rfind("fn ") else { return "?" };
            let name = &quelle[start + 3..];
            let ende = name.find(['(', '<', ' ']).unwrap_or(0);
            &name[..ende]
        };
        let mut gruende: Vec<&str> = vec![];
        for (i, _) in quelle.match_indices("cgs::sichtbar_zum_space(") {
            gruende.push(funktion_um(i));
        }
        assert!(!gruende.is_empty(), "es gibt Aufrufer");
        for f in &gruende {
            assert!(
                // EXPLICIT_PREVIEW_VISIT bzw. BOUNDED_WORKSPACE_PREP; der
                // Rueckweg ist der zweite Halbsatz derselben Vorbereitung.
                *f == "arbeitsplatz_besuchen"
                    || *f == "arbeitsplatz_oeffnen_blockierend"
                    // BOUNDED_REMOTE_INTERACTION: der Nutzer hat in der
                    // Miniatur auf ein echtes Fenster geklickt. macOS nimmt
                    // ein Zeigerereignis nur an, solange das Fenster wirklich
                    // sichtbar ist - der Wechsel ist also Teil SEINER
                    // Handlung, und die Blende haelt ihn unsichtbar.
                    || *f == "auf_arbeitsplatz_handeln"
                    || *f == "zurueck_zur_herkunft",
                "unerlaubter Aufrufer des sichtbaren Wechsels: {f}"
            );
        }
        // Im Besuch selbst: der Hinweg und - falls er unter der
        // Uebergangsflaeche scheitert - der Rueckweg dorthin, wo der Nutzer
        // stand. Beides gehoert zu DERSELBEN ausdruecklichen Handlung; ein
        // dritter Wechsel waere ein zweiter Wille.
        // Der ausdrueckliche Besuch geht DIREKT zum gewaehlten Space
        // (Mission Control), nie schrittweise ueber Nachbarn. Arbeitsplatz-
        // Fokus navigiert dagegen nie und benutzt auch keinen Rueckhol-Waechter.
        assert!(
            !gruende.contains(&"arbeitsplatz_besuchen"),
            "der Besuch wechselt nie schrittweise"
        );
        let direkt: Vec<&str> = quelle.match_indices("cgs::direkt_zum_space(").map(|(i, _)| funktion_um(i)).collect();
        // The debug-only acceptance return hook is compiled out of release.
        let direkt: Vec<&str> = direkt.into_iter().filter(|f| *f != "test_exakter_space").collect();
        assert_eq!(direkt, vec!["arbeitsplatz_besuchen"], "nur der ausdrueckliche Fussknopf-Besuch springt direkt");
        // Die Vorbereitung merkt sich die Herkunft und kehrt dorthin zurueck.
        let o = quelle.find("fn arbeitsplatz_oeffnen_blockierend").unwrap();
        let ende = quelle[o..].find("\n/// Eine allgemeine Browser-Suche").unwrap();
        let oeffner = &quelle[o..o + ende];
        assert!(oeffner.contains("let herkunft = cgs::aktiver_space()"), "Herkunft wird gemerkt");
        // Drei Wege fuehren den Nutzer zur Herkunft zurueck: der Browser-
        // zweig, der allgemeine Zweig - und der Fall, in dem NOKI gar nicht
        // gewechselt hat, macOS den Nutzer beim Zustellen einer Adresse aber
        // von sich aus auf den Schreibtisch des Zielfensters gezogen hat.
        assert_eq!(
            oeffner.matches("zurueck_zur_herkunft(herkunft)").count(),
            3,
            "jeder Weg fuehrt den Nutzer zur Herkunft zurueck"
        );
        assert!(
            oeffner.contains("EigenerWechsel::neu()"),
            "und der eigene Wechsel gilt nicht als Nutzerbewegung"
        );
        // AUSFUEHRUNGSORT != NAVIGATION DES NUTZERS.
        //
        // Der Wechsel darf echt sein - gesehen werden darf er nicht. Die
        // Blende muss VOR dem Hinweg stehen, sonst saehe der Nutzer genau
        // den ersten Schritt, den sie verdecken soll. Und sie muss als
        // `Halten` gefuehrt werden, damit sie auch beim fruehen `return` des
        // Browser-Zweigs wieder faellt.
        let b = oeffner.find("blende::auf(&app)").expect("die Blende geht hoch");
        let hin = oeffner.find("cgs::sichtbar_zum_space(r.id)").expect("es gibt den Hinweg");
        assert!(b < hin, "die Blende steht VOR dem ersten sichtbaren Schritt");
        assert!(
            oeffner.contains("blende::Halten(app.clone(), blende_an)"),
            "die Blende faellt auf JEDEM Rueckweg dieser Funktion"
        );
        // Der AUSDRUECKLICHE Besuch ist das Gegenteil: dort WILL der Nutzer
        // hin, und dann soll er den Weg auch sehen. Eine Blende waere hier
        // ein verschluckter Schreibtischwechsel.
        let bv = quelle.find("fn arbeitsplatz_besuchen").expect("es gibt den Besuch");
        let bende = quelle[bv..].find("\nfn ").map(|i| bv + i).unwrap_or(quelle.len());
        assert!(
            !quelle[bv..bende].contains("blende::"),
            "der ausdrueckliche Besuch wird NICHT verdeckt"
        );
        let besuch = &quelle[bv..bende];
        let real = &besuch[besuch.find("let r = match arbeitsplatz_sichern")
            .expect("REAL_SPACE-Besuch ist vorhanden")..];
        assert!(
            !real.contains("vorschau::abdecken") && !real.contains("vorschau::voll"),
            "REAL_SPACE zeigt den echten Space statt eines Vollbild-Compositors"
        );
    }

    /// Die Blende taeuscht nichts vor und bleibt nie haengen.
    ///
    /// Zwei Zusagen, die im Quelltext nachweisbar sein muessen, weil ein
    /// Fehler hier den ganzen Bildschirm betrifft: ohne Schnappschuss wird
    /// die Blende NICHT gezeigt (lieber ein sichtbarer Wechsel als ein
    /// schwarzes Bild), und es gibt eine Notbremse, die sie in jedem Fall
    /// wieder loest.
    #[test]
    fn die_blende_ist_ehrlich_und_kann_nicht_haengenbleiben() {
        let quelle = include_str!("lib.rs");
        let a = quelle.find("mod blende {").expect("es gibt die Blende");
        let e = quelle[a..].find("\n}\n").map(|i| a + i).unwrap_or(quelle.len());
        let m = &quelle[a..e];
        assert!(
            m.contains("if bild.is_null()") && m.contains("return false;"),
            "ohne Schnappschuss wird nichts vorgetaeuscht"
        );
        assert!(m.contains("Notbremse"), "es gibt eine Notbremse");
        assert!(
            m.contains("setLevel:") && m.contains("CGWindowLevelForKey(14)"),
            "hoechste WindowServer-Ebene: auch Space-Uebergangsflaechen liegen darunter"
        );
        assert!(
            m.contains("setIgnoresMouseEvents:\\0\"), false)"),
            "die Blende nimmt waehrend des Vorgangs jede physische Eingabe selbst an"
        );
    }

    /// Ein Fenster gehoert Noki nur mit ausdruecklicher Herkunft.
    ///
    /// `neue_fenster_abwarten` teilt die Fenster nach dem Schreibtisch, auf
    /// dem sie WIRKLICH entstanden sind. Hier wird die Regel geprueft, die
    /// darueber entscheidet: vorher vorhanden = fremd, anderswo entstanden =
    /// nicht beansprucht. Nur das dritte Fenster ist Nokis.
    #[test]
    fn nur_neu_und_auf_nokis_schreibtisch_gilt_als_nokis_fenster() {
        let arbeitsplatz = 2730u64;
        let vorher = vec![291833i64]; // das Code-Fenster des Nutzers, Space 1
        let jetzt: Vec<(i64, u64)> = vec![
            (291833, 1),    // unveraendert das des Nutzers
            (360900, 1),    // neu, aber beim Nutzer gelandet
            (360901, 2730), // neu und auf Nokis Schreibtisch
        ];
        let mut hier = vec![];
        let mut anderswo = vec![];
        for (wid, space) in jetzt {
            if vorher.contains(&wid) {
                continue;
            }
            if space == arbeitsplatz {
                hier.push(wid);
            } else {
                anderswo.push((wid, space));
            }
        }
        assert_eq!(hier, vec![360901], "nur das dort entstandene Fenster");
        assert_eq!(anderswo, vec![(360900, 1)], "das andere wird gemeldet, nicht beansprucht");
        assert!(!hier.contains(&291833), "ein fremdes Fenster wird nie Nokis");
    }

    #[test]
    fn registry_claim_requires_explicit_expiring_launch_intent() {
        let source = include_str!("lib.rs");
        let start = source.find("fn virtuellen_registry_eintrag_sichern").unwrap();
        let end = source[start..].find("\n}\n").map(|n| start + n).unwrap();
        let body = &source[start..end];
        assert!(body.contains("launch_transaction_claims"));
        let tx = &source[source.find("struct NokiLaunchTransaction").unwrap()..start];
        assert!(tx.contains("deadline"));
        assert!(tx.contains("before:"));
        assert!(tx.contains("target_bundle"));
        assert!(tx.contains("impl Drop for NokiLaunchGuard"));
    }

    #[test]
    fn native_and_web_picker_entries_never_silently_alias() {
        let names: Vec<_> = WEB_EINTRAEGE.iter().map(|e| e.0).collect();
        for name in ["YouTube Web", "Claude Web", "ChatGPT Web", "Spotify Web"] {
            assert!(names.contains(&name));
        }
        let source = include_str!("lib.rs");
        let production = &source[..source.find("\nmod ablage_tests {").unwrap_or(source.len())];
        assert!(!production.contains("fn web_ziel("), "native picker entries must not route through a web fallback");
    }

    #[test]
    fn existing_user_window_is_a_hard_single_window_boundary() {
        let source = include_str!("lib.rs");
        let user_start = source.find("pub(crate) fn nutzer_hat_fenster").unwrap();
        let user_end = source[user_start..].find("\n}\n").map(|n| user_start + n).unwrap();
        let user_body = &source[user_start..user_end];
        assert!(user_body.contains("!noki.contains(&f.0)"));
        assert!(user_body.contains("!auf_noki(&f)"));
        let reopen_start = source.find("fn programm_fenster_wieder_oeffnen").unwrap();
        let reopen_end = source[reopen_start..].find("\n}\n")
            .map(|n| reopen_start + n).unwrap();
        let reopen = &source[reopen_start..reopen_end];
        assert!(reopen.contains("let mut ziel = None"));
        assert!(!reopen.contains("Steht schon ein (nicht registriertes) Fenster"));
    }

    #[test]
    fn virtual_display_recreation_keeps_exact_noki_window_identity() {
        let source = include_str!("lib.rs");
        let start = source.find("fn workspace_registry_refresh").unwrap();
        let end = source[start..].find("fn workspace_registry_watcher_starten")
            .map(|n| start + n).unwrap();
        let body = &source[start..end];
        assert!(body.contains("entry.created_by_noki"));
        assert!(body.contains("reason=virtual_display_lifecycle origin=NOKI"));
        assert!(body.contains("live.push(*wid)"));
    }
    #[test]
    fn fokus_apps_gefunden() {
        let a = fokus_apps_laden();
        assert!(a.len() > 5, "nur {} Apps", a.len());
        assert!(a
            .iter()
            .all(|x| x["pfad"].as_str().unwrap_or("").ends_with(".app")));
        let names: Vec<_> = a.iter().filter_map(|x| x["name"].as_str()).collect();
        assert!(names.iter().any(|n| *n == "Finder"), "Finder fehlt: {names:?}");
        for helper in ["AirPlayUIAgent", "WiFiAgent", "AirPort Base Station Agent"] {
            assert!(!names.iter().any(|n| *n == helper), "Helper sichtbar: {helper}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pegel_wird_gelesen() {
        let (e, a) = voice_sample("owner=42 generation=7 sequence=3 energy=0.512000 active=1\n")
            .expect("Zeile ist gueltig");
        assert!((e - 0.512).abs() < 1e-9);
        assert!(a);
    }

    #[test]
    fn pegel_wird_geklemmt() {
        let (e, _) = voice_sample("energy=4.0 active=1").unwrap();
        assert_eq!(e, 1.0);
        let (e2, _) = voice_sample("energy=-3.0 active=0").unwrap();
        assert_eq!(e2, 0.0);
    }

    #[test]
    fn ruhe_wird_erkannt() {
        let (e, a) = voice_sample("owner=1 generation=1 sequence=9 energy=0.000000 active=0")
            .expect("Zeile ist gueltig");
        assert_eq!(e, 0.0);
        assert!(!a);
    }

    #[test]
    fn unfug_wird_abgewiesen() {
        assert!(voice_sample("").is_none());
        assert!(voice_sample("STANDBY").is_none());
        assert!(voice_sample("energy=viel active=1").is_none());
        assert!(voice_sample("energy=NaN active=1").is_none());
    }

    #[test]
    fn zustandsliste_bleibt_das_enum() {
        // Dieselben fuenf Werte wie lib/visual.sh. Eine sechste Zeile hier
        // waere eine zweite Zustandsarchitektur.
        assert_eq!(
            VALID_STATES,
            ["STANDBY", "LISTENING", "PROCESSING", "SPEAKING", "ERROR"]
        );
    }

    #[test]
    fn laufzeitverzeichnis_ohne_umgebung() {
        // Ohne gesetzte Variable muss der Pfad dem entsprechen, was
        // lib/core.sh ableitet — sonst findet die aus dem Finder gestartete
        // App die Dateien des Cores nicht.
        let dir = runtime_dir().expect("Pfad ableitbar");
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        if std::env::var_os("JARVIS_RUNTIME_DIR").is_none() {
            assert!(name.starts_with("jarvis-"), "unerwarteter Name: {name}");
            assert!(name["jarvis-".len()..].chars().all(|c| c.is_ascii_digit()));
        }
    }

    // ---- Abschnitt 17: der gemerkte Ort ----
    const HAUPT: Schirm = ((0, 0), (1920, 1080));
    const ZWEIT: Schirm = ((1920, 0), (1440, 900));
    const FENSTER_GR: (i32, i32) = (460, 400);

    #[test]
    fn gueltiger_ort_bleibt_stehen() {
        assert_eq!(
            ort_in_sicht((300, 200), FENSTER_GR, &[HAUPT]),
            Some((300, 200))
        );
    }

    #[test]
    fn ort_am_rand_wird_hereingezogen() {
        // Halb ueber der rechten Kante: die Mitte liegt noch auf dem
        // Bildschirm, also wird nur so weit geklemmt, dass es passt.
        assert_eq!(
            ort_in_sicht((1800, 900), FENSTER_GR, &[HAUPT]),
            Some((1920 - 460, 1080 - 400))
        );
    }

    #[test]
    fn ort_auf_abgezogenem_bildschirm_kommt_zurueck() {
        // Noki stand auf dem zweiten Bildschirm; der ist weg. Er darf nicht
        // dort erscheinen, wo nichts mehr ist.
        let p = ort_in_sicht((2400, 300), FENSTER_GR, &[HAUPT]).expect("Ort");
        assert!(p.0 >= 0 && p.0 + 460 <= 1920, "x ausserhalb: {p:?}");
        assert!(p.1 >= 0 && p.1 + 400 <= 1080, "y ausserhalb: {p:?}");
    }

    #[test]
    fn zweiter_bildschirm_bleibt_erlaubt() {
        assert_eq!(
            ort_in_sicht((2100, 300), FENSTER_GR, &[HAUPT, ZWEIT]),
            Some((2100, 300))
        );
    }

    #[test]
    fn kleinere_aufloesung_zieht_herein() {
        // Aufloesung von 1920x1080 auf 1280x800 gewechselt.
        let klein: Schirm = ((0, 0), (1280, 800));
        let p = ort_in_sicht((1700, 950), FENSTER_GR, &[klein]).expect("Ort");
        assert_eq!(p, (1280 - 460, 800 - 400));
    }

    #[test]
    fn ohne_bildschirm_wird_nichts_geaendert() {
        assert_eq!(ort_in_sicht((10, 10), FENSTER_GR, &[]), None);
    }

    #[test]
    fn fenster_groesser_als_bildschirm_bleibt_oben_links() {
        let winzig: Schirm = ((0, 0), (300, 200));
        assert_eq!(
            ort_in_sicht((500, 500), FENSTER_GR, &[winzig]),
            Some((0, 0))
        );
    }

    #[test]
    fn symbol_hat_die_erwartete_groesse() {
        let bild = tray_symbol();
        assert_eq!(bild.width(), 44);
        assert_eq!(bild.height(), 44);
        assert_eq!(bild.rgba().len(), 44 * 44 * 4);
        // Es muss ueberhaupt etwas zu sehen sein.
        let sichtbar = bild.rgba().chunks(4).filter(|p| p[3] > 128).count();
        assert!(sichtbar > 200, "Symbol ist fast leer: {sichtbar}");
    }

    const NOKI_FENSTER_GR: (i32, i32) = (320, 260);

    #[test]
    fn noki_fenster_in_sicht_normal() {
        assert_eq!(
            ort_in_sicht((500, 400), NOKI_FENSTER_GR, &[HAUPT]),
            Some((500, 400))
        );
    }

    #[test]
    fn noki_fenster_in_sicht_klemmen() {
        assert_eq!(
            ort_in_sicht((1900, 1050), NOKI_FENSTER_GR, &[HAUPT]),
            Some((1920 - 320, 1080 - 260))
        );
    }

    // ---- Alle Schreibtische (Abschnitt 29) ----
    // ---- Spotify (Abschnitt 30) ----
    #[test]
    fn spotify_spielstand_wird_gedeutet() {
        assert!(spotify_spielt_aus_ausgabe("playing"));
        assert!(spotify_spielt_aus_ausgabe("playing\n"));
        assert!(spotify_spielt_aus_ausgabe("  Playing  "));
        assert!(!spotify_spielt_aus_ausgabe("paused"));
        assert!(!spotify_spielt_aus_ausgabe("stopped"));
        assert!(!spotify_spielt_aus_ausgabe(""));
        // Eine Fehlermeldung ist kein "playing".
        assert!(!spotify_spielt_aus_ausgabe("execution error: ..."));
    }

    #[test]
    fn einstellungen_mischen_behaelt_die_anderen_werte() {
        // Der eine Schluessel wird gesetzt, der andere ueberlebt.
        let a = einstellungen_mischen(
            "{\"alle_schreibtische\": false}",
            "noki_sichtbar",
            serde_json::Value::Bool(false),
        );
        assert!(!alle_spaces_aus_json(&a));
        assert!(!sicht_wunsch_aus_json(&a));
        // Und umgekehrt.
        let b = einstellungen_mischen(&a, "alle_schreibtische", serde_json::Value::Bool(true));
        assert!(alle_spaces_aus_json(&b));
        assert!(!sicht_wunsch_aus_json(&b));
        // Kaputter oder leerer Inhalt beginnt sauber neu.
        let c = einstellungen_mischen("kein json", "noki_sichtbar", serde_json::Value::Bool(true));
        assert!(sicht_wunsch_aus_json(&c));
        assert!(alle_spaces_aus_json(&c));
    }

    #[test]
    fn ebene_grundstellung_ist_vordergrund() {
        // Ohne Angabe vorn: das Menue kennt nur vorn und hinten, eine
        // dritte Stellung waere von dort nie erreichbar.
        assert_eq!(ebene_aus_json(""), "vorn");
        assert_eq!(ebene_aus_json("{}"), "vorn");
        assert_eq!(ebene_aus_json("kein json"), "vorn");
        assert_eq!(ebene_aus_json("{\"ebene\": \"normal\"}"), "vorn");
        // Nur ein ausdrueckliches "hinten" legt ihn nach hinten.
        assert_eq!(ebene_aus_json("{\"ebene\": \"hinten\"}"), "hinten");
        assert_eq!(ebene_aus_json("{\"ebene\": \"vorn\"}"), "vorn");
        // Und die Auswahl steht neben den anderen Schluesseln.
        let d = einstellungen_mischen(
            "{\"noki_sichtbar\": false}",
            "ebene",
            serde_json::Value::String("hinten".into()),
        );
        assert_eq!(ebene_aus_json(&d), "hinten");
        assert!(!sicht_wunsch_aus_json(&d));
    }

    #[test]
    fn sicht_wunsch_standard_und_gespeichert() {
        // Ohne Angabe sichtbar — sonst muesste man Noki nach jedem Start
        // erst einschalten.
        assert!(sicht_wunsch_aus_json(""));
        assert!(sicht_wunsch_aus_json("{}"));
        assert!(sicht_wunsch_aus_json("kein json"));
        assert!(sicht_wunsch_aus_json("{\"alle_schreibtische\": false}"));
        // Ein ausdruecklicher Wunsch gilt, in beide Richtungen.
        assert!(!sicht_wunsch_aus_json("{\"noki_sichtbar\": false}"));
        assert!(sicht_wunsch_aus_json("{\"noki_sichtbar\": true}"));
        // Beide Schluessel stehen unabhaengig nebeneinander.
        let beides = "{\"alle_schreibtische\": false, \"noki_sichtbar\": false}";
        assert!(!alle_spaces_aus_json(beides));
        assert!(!sicht_wunsch_aus_json(beides));
    }

    #[test]
    fn alle_spaces_ohne_datei_ist_ein() {
        // Fehlt die Datei, liefert der Aufrufer einen leeren String bzw.
        // gar nichts — beides muss den sinnvollen Standard ergeben.
        assert!(alle_spaces_aus_json(""));
        assert!(alle_spaces_aus_json("{}"));
        assert!(alle_spaces_aus_json("kein json"));
        assert!(alle_spaces_aus_json("{\"anderes\": 1}"));
    }

    #[test]
    fn alle_spaces_gespeicherter_wert_gilt() {
        assert!(!alle_spaces_aus_json("{\"alle_schreibtische\": false}"));
        assert!(alle_spaces_aus_json("{\"alle_schreibtische\": true}"));
        // Ein falscher Typ ist kein "false", sondern eine fehlende Angabe.
        assert!(alle_spaces_aus_json("{\"alle_schreibtische\": \"nein\"}"));
    }

    #[test]
    fn ebene_und_spaces_sind_unabhaengig() {
        // Der Kern der Anforderung: Space-Zugehoerigkeit und Fensterebene
        // stehen nebeneinander. Beides muss gleichzeitig darstellbar sein
        // und darf sich gegenseitig nicht veraendern.
        let lage = Lage {
            ebene: Mutex::new("vorn"),
            durchgang: AtomicBool::new(false),
            alle_spaces: AtomicBool::new(true),
        };
        // alle Schreibtische + VORN
        assert_eq!(*lage.ebene.lock().unwrap(), "vorn");
        assert!(lage.alle_spaces.load(Ordering::Relaxed));

        // alle Schreibtische + HINTEN (Durchgang hinter einem Fenster)
        lage.durchgang.store(true, Ordering::Relaxed);
        assert!(lage.durchgang.load(Ordering::Relaxed));
        assert!(lage.alle_spaces.load(Ordering::Relaxed));
        assert_eq!(*lage.ebene.lock().unwrap(), "vorn");

        // Das Umschalten der Schreibtische laesst Ebene und Durchgang stehen.
        lage.alle_spaces.store(false, Ordering::Relaxed);
        assert!(lage.durchgang.load(Ordering::Relaxed));
        assert_eq!(*lage.ebene.lock().unwrap(), "vorn");

        // Und das Umschalten der Ebene laesst die Schreibtische stehen.
        *lage.ebene.lock().unwrap() = "hinten";
        lage.durchgang.store(false, Ordering::Relaxed);
        assert!(!lage.alle_spaces.load(Ordering::Relaxed));
    }

    #[test]
    fn schirm_info_serialisierung() {
        let info = SchirmInfo {
            x: 0,
            y: 0,
            w: 1440,
            h: 900,
            scale: 2.0,
            ax: 0,
            ay: 25,
            aw: 1440,
            ah: 875,
        };
        let s = serde_json::to_string(&info).unwrap();
        let de: SchirmInfo = serde_json::from_str(&s).unwrap();
        assert_eq!(info, de);
    }

    #[test]
    fn maus_schirm_position_liefert_wert() {
        #[cfg(target_os = "macos")]
        {
            let pos = maus_schirm_position();
            assert!(pos.is_some());
            let (x, y) = pos.unwrap();
            assert!(x.is_finite() && y.is_finite());
        }
    }

    #[test]
    fn datei_browser_start_enthaelt_standard_orte() {
        let res = super::datei_browser_start();
        assert_eq!(res["start"], true);
        assert_eq!(res["pfad"], "");
        assert_eq!(res["oben"], serde_json::Value::Null);
        let eintraege = res["eintraege"]
            .as_array()
            .expect("eintraege muss Array sein");
        assert!(
            eintraege.len() >= 5 && eintraege.len() <= 6,
            "Startansicht muss Hauptorte enthalten"
        );
        let namen: Vec<&str> = eintraege
            .iter()
            .filter_map(|e| e["name"].as_str())
            .collect();
        assert!(!namen.contains(&"Benutzerordner (~)"));
        assert!(!namen.contains(&"Filme"));
        assert!(namen.contains(&"Desktop"));
        assert!(namen.contains(&"Dokumente"));
        assert!(namen.contains(&"Downloads"));
        assert!(namen.contains(&"Bilder"));
        assert!(namen.contains(&"Musik"));
    }

    #[test]
    fn datei_browser_liste_und_vorschau_funktionalitaet() {
        tauri::async_runtime::block_on(async {
            let home = std::env::var("HOME").unwrap();

            // 1. Liste Pictures: oben muss None sein (Grenze)
            let pictures_path = home.clone() + "/Pictures";
            let res = super::datei_browser_liste(Some(pictures_path.clone()))
                .await
                .unwrap();
            assert_eq!(res["start"], false);
            assert_eq!(
                res["oben"],
                serde_json::Value::Null,
                "Hauptort darf kein oberhalb liegendes Ziel haben"
            );
            let eintraege = res["eintraege"].as_array().unwrap();
            if let Some(paket) = eintraege.iter().find(|e| e["paket"] == true) {
                assert_eq!(paket["nicht_unterstuetzt"], true);
                assert_eq!(paket["ordner"], false);
            }

            // 2. Ungueltige / geschuetzte / ausserhalb liegende Pfade muessen blockiert werden
            assert!(super::datei_browser_liste(Some("/".into())).await.is_err());
            assert!(super::datei_browser_liste(Some("/Users".into()))
                .await
                .is_err());
            assert!(super::datei_browser_liste(Some(home.clone()))
                .await
                .is_err());
            assert!(
                super::datei_browser_liste(Some("/nicht/existierender/pfad".into()))
                    .await
                    .is_err()
            );
            assert!(super::datei_browser_liste(Some(home.clone() + "/.ssh"))
                .await
                .is_err());

            // 3. Thumbnail & Vorschau mit realen Testdateien
            let img_path = home.clone() + "/Downloads/IMG_8634.jpg";
            if std::path::Path::new(&img_path).exists() {
                let thumb = super::datei_thumbnail(img_path.clone()).await.unwrap();
                assert!(thumb.starts_with("data:image/"));

                let vorschau = super::datei_vorschau(img_path).await.unwrap();
                assert_eq!(vorschau["art"], "bild");
                assert!(vorschau["bild_uri"]
                    .as_str()
                    .unwrap()
                    .starts_with("data:image/"));
                assert!(vorschau["metadaten"]["groesse"].as_str().is_some());
            }

            let pdf_path = home.clone() + "/Downloads/MacBook-Shortcuts-Uebersicht.pdf";
            if std::path::Path::new(&pdf_path).exists() {
                let thumb = super::datei_thumbnail(pdf_path.clone()).await.unwrap();
                assert!(thumb.starts_with("data:image/"));

                let vorschau = super::datei_vorschau(pdf_path).await.unwrap();
                assert_eq!(vorschau["art"], "pdf");
                assert!(vorschau["bild_uri"]
                    .as_str()
                    .unwrap()
                    .starts_with("data:image/"));
            }

            // 4. Temporäre Text & CSV Vorschau innerhalb eines erlaubten Ortes (Downloads)
            let tmp_txt = std::path::PathBuf::from(&home)
                .join("Downloads")
                .join("noki_test_preview.txt");
            std::fs::write(&tmp_txt, "Zeile 1: Hallo Noki\nZeile 2: Test Vorschau").unwrap();
            let vorschau_txt = super::datei_vorschau(tmp_txt.to_string_lossy().into_owned())
                .await
                .unwrap();
            assert_eq!(vorschau_txt["art"], "text");
            assert!(vorschau_txt["text_inhalt"]
                .as_str()
                .unwrap()
                .contains("Hallo Noki"));
            let _ = std::fs::remove_file(&tmp_txt);

            let tmp_csv = std::path::PathBuf::from(&home)
                .join("Downloads")
                .join("noki_test_preview.csv");
            std::fs::write(
                &tmp_csv,
                "Name,Alter,Stadt\nAlice,30,Berlin\nBob,25,Hamburg",
            )
            .unwrap();
            let vorschau_csv = super::datei_vorschau(tmp_csv.to_string_lossy().into_owned())
                .await
                .unwrap();
            assert_eq!(vorschau_csv["art"], "csv");
            let tabelle = vorschau_csv["csv_tabelle"].as_array().unwrap();
            assert_eq!(tabelle.len(), 3);
            assert_eq!(tabelle[0][0], "Name");
            let _ = std::fs::remove_file(&tmp_csv);
        });
    }

    #[test]
    fn anhang_store_lebenszyklus() {
        let store = super::attachments::AttachmentStore::default();
        let home = std::env::var("HOME").unwrap();
        let dl = home + "/Downloads/IMG_8634.jpg";
        if std::path::Path::new(&dl).exists() {
            let att = store.add("chat_test_123", &dl).expect("anhang hinzufügen");
            assert_eq!(att.is_image, true);
            assert_eq!(att.name, "IMG_8634.jpg");

            let liste = store.list("chat_test_123");
            assert_eq!(liste.len(), 1);
            assert_eq!(liste[0].id, att.id);

            assert!(store.remove("chat_test_123", att.id));
            assert_eq!(store.list("chat_test_123").len(), 0);
        }
    }
}
