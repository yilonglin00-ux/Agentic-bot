//! JARVIS – Desktop-App (Phase 3.14)
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
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, SystemTime},
};

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
fn spawn_state_watcher(app: tauri::AppHandle, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        let Some(path) = runtime_state_file() else {
            log::warn!("[Noki Watcher] JARVIS_RUNTIME_DIR nicht gesetzt");
            return;
        };

        let mut last_state = String::new();

        while !stop.load(Ordering::Relaxed) {
            if let Ok(metadata) = fs::metadata(&path) {
                if metadata.len() <= MAX_STATE_BYTES as u64 {
                    if let Ok(raw) = fs::read_to_string(&path) {
                        let state = raw.trim();

                        if VALID_STATES.contains(&state) && state != last_state {
                            let _ = app.emit(
                                "noki://state",
                                serde_json::json!({
                                  "state": state,
                                  "source": "jarvis",
                                }),
                            );

                            last_state = state.to_string();
                        }
                    }
                }
            }

            thread::sleep(POLL_INTERVAL);
        }
    });
}

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
        let Some((k, v)) = feld.split_once('=') else { continue };
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

fn spawn_voice_watcher(app: tauri::AppHandle, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        let Some(path) = runtime_dir().map(|d| d.join("visual.voice")) else {
            return;
        };

        let mut letzter = -1.0_f64;
        let mut war_aktiv = false;

        while !stop.load(Ordering::Relaxed) {
            let mut jetzt: Option<(f64, bool)> = None;
            if let Ok(meta) = fs::metadata(&path) {
                if meta.len() <= MAX_VOICE_BYTES as u64 {
                    if let Ok(raw) = fs::read_to_string(&path) {
                        jetzt = voice_sample(&raw);
                    }
                }
            }

            if let Some((pegel, aktiv)) = jetzt {
                // Nur melden, wenn sich etwas aendert. Ein stehender Pegel
                // verfaellt im Frontend nach seiner Frist von selbst — genau
                // dafuer ist sie da.
                let anders = aktiv != war_aktiv || (pegel - letzter).abs() > 0.004;
                if aktiv && anders {
                    let _ = app.emit(
                        "noki://voice",
                        serde_json::json!({ "level": pegel, "active": true }),
                    );
                } else if !aktiv && war_aktiv {
                    let _ = app.emit(
                        "noki://voice",
                        serde_json::json!({ "level": 0.0, "active": false }),
                    );
                }
                letzter = pegel;
                war_aktiv = aktiv;
            } else if war_aktiv {
                let _ = app.emit(
                    "noki://voice",
                    serde_json::json!({ "level": 0.0, "active": false }),
                );
                war_aktiv = false;
                letzter = -1.0;
            }

            thread::sleep(VOICE_INTERVAL);
        }
    });
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
fn spawn_heartbeat_watcher(app: tauri::AppHandle, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        let Some(path) = runtime_dir().map(|d| d.join("jarvis-core.heartbeat")) else {
            return;
        };

        let mut lebte: Option<bool> = None;

        while !stop.load(Ordering::Relaxed) {
            let lebt = fs::metadata(&path)
                .and_then(|m| m.modified())
                .map(|t| {
                    SystemTime::now()
                        .duration_since(t)
                        .map(|d| d < BEAT_FRISCH)
                        .unwrap_or(true)
                })
                .unwrap_or(false);

            if lebte != Some(lebt) {
                // Der erste Durchlauf meldet nichts: dass beim Start schon
                // ein Core lief, ist kein Ereignis.
                if lebte.is_some() {
                    let _ = app.emit(
                        "noki://umwelt",
                        serde_json::json!({
                            "ereignis": if lebt { "jarvis_offen" } else { "jarvis_weg" }
                        }),
                    );
                }
                lebte = Some(lebt);
            }

            thread::sleep(BEAT_INTERVAL);
        }
    });
}

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

            // Das eigene Panel ist kein Hindernis fuer sich selbst.
            if zahl_i64(d, kCGWindowOwnerPID) == Some(eigene_pid) {
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
                id: zahl_i64(d, kCGWindowNumber).unwrap_or(0),
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
    struct P { x: f64, y: f64 }
    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    struct S { w: f64, h: f64 }
    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    struct R { o: P, s: S }

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> u8;
        fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
        fn AXUIElementCopyAttributeValue(el: CFTypeRef, attr: CFTypeRef, out: *mut CFTypeRef) -> i32;
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
        if e == 0 && !v.is_null() { Some(v) } else { None }
    }
    unsafe fn rahmen(el: CFTypeRef) -> Option<Rahmen> {
        let p = attr(el, "AXPosition")?;
        let Some(s) = attr(el, "AXSize") else { CFRelease(p); return None; };
        let (mut pp, mut ss) = (P::default(), S::default());
        let ok = AXValueGetValue(p, 1, &mut pp as *mut P as *mut c_void) != 0
              && AXValueGetValue(s, 2, &mut ss as *mut S as *mut c_void) != 0;
        CFRelease(p);
        CFRelease(s);
        if ok { Some((pp.x, pp.y, ss.w, ss.h)) } else { None }
    }
    /// Textattribut (z. B. AXTitle), leer wenn keins.
    unsafe fn text(el: CFTypeRef, name: &str) -> String {
        let Some(v) = attr(el, name) else { return String::new(); };
        let mut buf = [0u8; 256];
        let ok = CFGetTypeID(v) == CFStringGetTypeID()
            && CFStringGetCString(v, buf.as_mut_ptr(), buf.len() as isize, 0x0800_0100) != 0;
        CFRelease(v);
        if !ok { return String::new(); }
        let e = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[..e]).into_owned()
    }
    /// Wahrheitsattribut (z. B. AXEnabled); None wenn nicht vorhanden.
    unsafe fn wahr(el: CFTypeRef, name: &str) -> Option<bool> {
        let v = attr(el, name)?;
        let b = if CFGetTypeID(v) == CFBooleanGetTypeID() { Some(CFBooleanGetValue(v) != 0) } else { None };
        CFRelease(v);
        b
    }
    pub fn vertraut() -> bool { unsafe { AXIsProcessTrusted() != 0 } }
    /// Gibt es das sichtbare CG-Fenster mit dieser Nummer noch?
    pub fn existiert(id: i64) -> bool { unsafe { cg_fenster(id).is_some() } }
    unsafe fn zahl(d: CFTypeRef, k: CFTypeRef) -> Option<i64> {
        let v = CFDictionaryGetValue(d, k);
        let mut n: i64 = 0;
        if !v.is_null() && CFNumberGetValue(v, 4, &mut n as *mut i64 as *mut c_void) != 0 { Some(n) } else { None }
    }
    /// PID und Rahmen des sichtbaren CG-Fensters mit dieser Nummer — frisch.
    unsafe fn cg_fenster(id: i64) -> Option<(i32, Rahmen)> {
        let liste = CGWindowListCopyWindowInfo(1, 0);
        if liste.is_null() { return None; }
        let mut aus = None;
        for i in 0..CFArrayGetCount(liste) {
            let d = CFArrayGetValueAtIndex(liste, i);
            if d.is_null() || zahl(d, kCGWindowNumber) != Some(id) { continue; }
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
        if AXIsProcessTrusted() == 0 { return None; }
        let (pid, fr) = cg_fenster(id)?;
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() { return None; }
        let (mut treffer, mut n): (Option<CFTypeRef>, u32) = (None, 0);
        if let Some(fs) = attr(app, "AXWindows") {
            for i in 0..CFArrayGetCount(fs) {
                let w = CFArrayGetValueAtIndex(fs, i);
                if let Some(r) = rahmen(w) {
                    if (r.0 - fr.0).abs() <= 2.0 && (r.1 - fr.1).abs() <= 2.0
                        && (r.2 - fr.2).abs() <= 2.0 && (r.3 - fr.3).abs() <= 2.0 {
                        n += 1;
                        if treffer.is_none() { treffer = Some(CFRetain(w)); }
                    }
                }
            }
            CFRelease(fs);
        }
        CFRelease(app);
        let w = treffer?;
        let k = if n == 1 { attr(w, "AXCloseButton") } else { None };
        let titel = if k.is_some() { text(w, "AXTitle") } else { String::new() };
        CFRelease(w);
        let k = k?;
        if wahr(k, "AXEnabled") == Some(false) { CFRelease(k); return None; }
        match rahmen(k) {
            Some(r) if r.2 > 2.0 && r.3 > 2.0 => Some((k, r, titel)),
            _ => { CFRelease(k); None }
        }
    }
    pub unsafe fn druecken(k: CFTypeRef) -> bool {
        let a = cfs("AXPress");
        let e = AXUIElementPerformAction(k, a);
        CFRelease(a);
        e == 0
    }
    pub unsafe fn freigeben(k: CFTypeRef) { CFRelease(k); }
}

/// "Fenster schliessen": nur normale Fenster (Ebene 0), nie JARVIS selbst
/// (sichtbare_fenster laesst die eigene PID weg), mit eindeutigem, aktivem
/// AXCloseButton. Sichtbar ist "App — Titel", nie eine interne Nummer.
pub fn schliessbare_fenster() -> Vec<(i64, String)> {
    #[cfg(target_os = "macos")]
    {
        sichtbare_fenster().into_iter().filter(|f| f.ebene == 0).filter_map(|f| unsafe {
            let (k, _, titel) = ax::knopf(f.id)?;
            ax::freigeben(k);
            let l = if titel.is_empty() { f.app.clone() } else { format!("{} — {}", f.app, titel) };
            Some((f.id, l.chars().take(70).collect()))
        }).collect()
    }
    #[cfg(not(target_os = "macos"))]
    { Vec::new() }
}

/// Bedienungshilfen (AX) fuer "Fenster schliessen": bei JEDEM Einstieg live
/// geprueft (keine gespeicherte Wahrheit). Fehlt die Freigabe, zeigt macOS
/// einmal je Programmlauf den echten Freigabe-Dialog fuer diese App.
const AX_EINSTELLUNGEN: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
static AX_GEFRAGT: AtomicBool = AtomicBool::new(false);
fn ax_einstieg() -> bool {
    let ok = lesezeichen::ax_vertraut(false);
    eprintln!("[AX] Fenster schliessen: AXIsProcessTrusted = {}", ok);
    if !ok && !AX_GEFRAGT.swap(true, Ordering::Relaxed) { let _ = lesezeichen::ax_vertraut(true); }
    ok
}
/// Systemeinstellungen > Datenschutz & Sicherheit > Bedienungshilfen.
fn ax_einstellungen_oeffnen() {
    let _ = lesezeichen::ax_vertraut(true);
    let _ = std::process::Command::new("/usr/bin/open").arg(AX_EINSTELLUNGEN).spawn();
}

/// Das Untermenue "Fenster schliessen" (vom Fenster-Beobachter gefuellt).
struct SchliessMenue(tauri::menu::Submenu<tauri::Wry>);

fn schliess_menue_fuellen(app: &tauri::AppHandle, m: &tauri::menu::Submenu<tauri::Wry>, liste: &[(i64, String)]) {
    if let Ok(alt) = m.items() {
        for it in alt.iter() { let _ = m.remove(it); }
    }
    if liste.is_empty() {
        // Freigabe fehlt -> anklickbarer Weg zur Freigabe; sonst ehrlich "keins".
        let item = if lesezeichen::ax_vertraut(false) {
            MenuItem::with_id(app, "fz_keins", "Kein schließbares Fenster", false, None::<&str>)
        } else {
            MenuItem::with_id(app, "fz_freigabe", "Bedienungshilfen für Noki aktivieren …", true, None::<&str>)
        };
        if let Ok(i) = item { let _ = m.append(&i); }
        return;
    }
    for (id, label) in liste {
        if let Ok(i) = MenuItem::with_id(app, format!("fz_{}", id), label, true, None::<&str>) { let _ = m.append(&i); }
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
fn base64(daten: &[u8]) -> String {
    const Z: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity((daten.len() + 2) / 3 * 4);
    for c in daten.chunks(3) {
        let n = ((c[0] as u32) << 16) | ((*c.get(1).unwrap_or(&0) as u32) << 8) | *c.get(2).unwrap_or(&0) as u32;
        s.push(Z[(n >> 18) as usize & 63] as char);
        s.push(Z[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { Z[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { Z[n as usize & 63] as char } else { '=' });
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
static AUFNAHME_ANGEFRAGT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
            unsafe { CGRequestScreenCaptureAccess(); }
            return Err("berechtigung: Bildschirmaufnahme-Berechtigung fehlt".into());
        }
        let mut n: u32 = 0;
        unsafe { CGGetActiveDisplayList(0, std::ptr::null_mut(), &mut n); }
        if n == 0 { return Err("display: kein Bildschirm aktiv".into()); }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    { Err("init: nur unter macOS".into()) }
}

fn schreibtisch_datei(praefix: &str, endung: &str) -> Result<std::path::PathBuf, String> {
    let stempel = std::process::Command::new("/bin/date").arg("+%Y-%m-%d-%H%M%S").output().ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "jetzt".into());
    let home = std::env::var("HOME").map_err(|_| "pfad: HOME fehlt".to_string())?;
    // Alle Noki-Aufnahmen gesammelt in "Schreibtisch/Noki Kamera" (wird bei Bedarf angelegt).
    let dir = std::path::PathBuf::from(home).join("Desktop").join("Noki Kamera");
    std::fs::create_dir_all(&dir).map_err(|e| format!("pfad: Ordner Noki Kamera nicht anlegbar ({e})"))?;
    let probe = dir.join(".noki-schreibtest");
    std::fs::write(&probe, b"").map_err(|e| format!("pfad: Noki Kamera nicht beschreibbar ({e})"))?;
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
    #[repr(C)] #[derive(Clone, Copy)] struct Rechteck { x: f64, y: f64, w: f64, h: f64 }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFArrayCreate(a: *const c_void, v: *const *const c_void, n: isize, cb: *const c_void) -> *const c_void;
        fn CFArrayGetCount(a: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(a: *const c_void, i: isize) -> *const c_void;
        fn CFDictionaryGetValue(d: *const c_void, k: *const c_void) -> *const c_void;
        fn CFNumberGetValue(n: *const c_void, t: i32, v: *mut c_void) -> u8;
        fn CFStringCreateWithCString(a: *const c_void, s: *const std::os::raw::c_char, enc: u32) -> *const c_void;
        fn CFURLCreateFromFileSystemRepresentation(a: *const c_void, b: *const u8, n: isize, dir: u8) -> *const c_void;
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
        fn CGImageDestinationCreateWithURL(url: *const c_void, typ: *const c_void, n: usize, opt: *const c_void) -> *const c_void;
        fn CGImageDestinationAddImage(d: *const c_void, i: *const c_void, p: *const c_void);
        fn CGImageDestinationFinalize(d: *const c_void) -> bool;
    }
    unsafe fn cfs(s: &str) -> *const c_void {
        let c = std::ffi::CString::new(s).unwrap_or_default();
        CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), 0x0800_0100)
    }
    pub fn ohne_noki(pfad: &std::path::Path, jpeg: bool) -> Option<Result<(), String>> {
        unsafe {
            let h = dlopen(b"/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics\0".as_ptr() as *const _, 1);
            if h.is_null() { return None; }
            let s_info = dlsym(h, b"CGWindowListCopyWindowInfo\0".as_ptr() as *const _);
            let s_bild = dlsym(h, b"CGWindowListCreateImageFromArray\0".as_ptr() as *const _);
            if s_info.is_null() || s_bild.is_null() { return None; }
            let liste: extern "C" fn(u32, u32) -> *const c_void = std::mem::transmute(s_info);
            let erzeugen: extern "C" fn(Rechteck, *const c_void, u32) -> *const c_void = std::mem::transmute(s_bild);
            let info = liste(1, 0);   // nur sichtbare Fenster
            if info.is_null() { return Some(Err("init: Fensterliste leer".into())); }
            let (k_nr, k_pid) = (cfs("kCGWindowNumber"), cfs("kCGWindowOwnerPID"));
            let ich = std::process::id() as i64;
            let mut ids: Vec<*const c_void> = vec![];
            for i in 0..CFArrayGetCount(info) {
                let d = CFArrayGetValueAtIndex(info, i);
                let (mut nr, mut pid) = (0i64, 0i64);
                let n = CFDictionaryGetValue(d, k_nr);
                let p = CFDictionaryGetValue(d, k_pid);
                if !n.is_null() { let _ = CFNumberGetValue(n, 4, &mut nr as *mut i64 as *mut c_void); }
                if !p.is_null() { let _ = CFNumberGetValue(p, 4, &mut pid as *mut i64 as *mut c_void); }
                if nr > 0 && pid != ich { ids.push(nr as usize as *const c_void); }
            }
            CFRelease(k_nr); CFRelease(k_pid); CFRelease(info);
            let arr = CFArrayCreate(std::ptr::null(), ids.as_ptr(), ids.len() as isize, std::ptr::null());
            // Genau der Hauptbildschirm (sonst die Vereinigung aller Fensterrahmen,
            // die ueber den Rand ragen kann -> Freeze-Bild saesse verschoben).
            let img = erzeugen(CGDisplayBounds(CGMainDisplayID()), arr, 0);
            CFRelease(arr);
            if img.is_null() { return Some(Err("init: Aufnahme leer".into())); }
            let b = pfad.as_os_str().as_encoded_bytes();
            let url = CFURLCreateFromFileSystemRepresentation(std::ptr::null(), b.as_ptr(), b.len() as isize, 0);
            let typ = cfs(if jpeg { "public.jpeg" } else { "public.png" });
            let dst = if url.is_null() { std::ptr::null() } else { CGImageDestinationCreateWithURL(url, typ, 1, std::ptr::null()) };
            let ok = !dst.is_null() && { CGImageDestinationAddImage(dst, img, std::ptr::null()); CGImageDestinationFinalize(dst) };
            if !dst.is_null() { CFRelease(dst); }
            CFRelease(typ);
            if !url.is_null() { CFRelease(url); }
            CGImageRelease(img);
            Some(if ok { Ok(()) } else { Err("pfad: Bild nicht geschrieben".into()) })
        }
    }
}

#[tauri::command]
fn noki_bildschirmfoto(zweck: String) -> Result<serde_json::Value, String> {
    aufnahme_erlaubt()?;
    let freeze = zweck == "freeze";
    let pfad = if freeze { std::env::temp_dir().join("noki-freeze.jpg") }
               else { schreibtisch_datei("Noki-Screenshot", "png")? };
    // Alle Fenster AUSSER den eigenen: Noki muss dafuer nicht mehr
    // ausgeblendet werden (vorher war er dabei kurz komplett weg).
    #[cfg(target_os = "macos")]
    let nativ = bild::ohne_noki(&pfad, freeze);
    #[cfg(not(target_os = "macos"))]
    let nativ: Option<Result<(), String>> = None;
    match nativ {
        Some(r) => r?,
        None => {   // API fehlt: macOS-eigenes screencapture (Noki ist dann mit im Bild)
            let aus = std::process::Command::new("/usr/sbin/screencapture")
                .args(["-x", "-m", "-t", if freeze { "jpg" } else { "png" }])
                .arg(&pfad).output().map_err(|e| format!("init: {e}"))?;
            if !aus.status.success() {
                return Err(format!("init: Aufnahme konnte nicht starten ({})", String::from_utf8_lossy(&aus.stderr).trim()));
            }
        }
    }
    if !pfad.exists() { return Err("init: Aufnahme fehlgeschlagen".into()); }
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
struct ModusMenue(tauri::menu::MenuItem<tauri::Wry>, tauri::menu::MenuItem<tauri::Wry>);

fn aufnahme_stoppen(a: &Aufnahme) -> Result<std::path::PathBuf, String> {
    let lauf = a.0.lock().map_err(|_| "Sperre".to_string())?.take();
    let Some((mut kind, pfad)) = lauf else { return Err("keine Aufnahme aktiv".into()) };
    let _ = std::process::Command::new("/bin/kill").args(["-INT", &kind.id().to_string()]).status();
    let mut fertig = false;
    for _ in 0..100 {
        if let Ok(Some(_)) = kind.try_wait() { fertig = true; break; }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    if !fertig { let _ = kind.kill(); let _ = kind.wait(); }
    if std::fs::metadata(&pfad).map(|m| m.len()).unwrap_or(0) == 0 {
        return Err("init: Aufnahme nicht finalisiert".into());
    }
    Ok(pfad)
}

fn aufnahme_menue(app: &tauri::AppHandle, laeuft: bool) {
    if let Some(m) = app.try_state::<ModusMenue>() {
        let _ = m.1.set_text(if laeuft { "Screen Recording beenden" } else { "Screen Recording" });
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
    if a.0.lock().map_err(|_| "init: Sperre".to_string())?.is_some() {
        return Err("laeuft: Aufnahme läuft bereits".into());
    }
    aufnahme_erlaubt()?;
    let pfad = schreibtisch_datei("Noki-Recording", "mov")?;
    let err_pfad = std::env::temp_dir().join("noki-recording.err");
    let err_datei = std::fs::File::create(&err_pfad).map_err(|e| format!("init: {e}"))?;
    let mut kind = std::process::Command::new("/usr/sbin/screencapture")
        .args(["-v", "-x", "-m"]).arg(&pfad)
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(err_datei)
        .spawn().map_err(|e| format!("init: {e}"))?;
    std::thread::sleep(std::time::Duration::from_millis(500));
    if let Ok(Some(_)) = kind.try_wait() {
        let grund = std::fs::read_to_string(&err_pfad).unwrap_or_default();
        return Err(format!("init: Aufnahme konnte nicht starten ({})", grund.trim()));
    }
    *a.0.lock().map_err(|_| "init: Sperre".to_string())? = Some((kind, pfad.clone()));
    aufnahme_menue(&app, true);
    Ok(serde_json::json!({ "pfad": pfad.to_string_lossy() }))
}

// ---- Globale Tastenkuerzel Ctrl+1..4 -----------------------------------
// Nativ ueber Carbon RegisterEventHotKey (keine neue Abhaengigkeit): wirkt
// unabhaengig vom Fokus, auch im Vollbild. Jede Taste loest GENAU das
// Ereignis des Menuepunkts aus — keine zweite Umsetzung.
fn shortcut_ausloesen(app: &tauri::AppHandle, n: u32) {
    // Nur ein Arbeits-Panel zur Zeit: Aktionen ohne eigenes Panel (Kamera,
    // Fenster schliessen) raeumen offene Noki-Panels vorher zentral weg.
    if matches!(n, 1 | 2 | 4) { let _ = app.emit("noki://panels_zu", serde_json::json!({})); }
    match n {
        1 => { let _ = app.emit("noki://kamera", serde_json::json!({ "was": "screenshot" })); }
        2 => { let _ = app.emit("noki://kamera", serde_json::json!({ "was": "recording" })); }
        3 => { let _ = app.emit("noki://freeze", serde_json::json!({})); }
        4 => {
            #[cfg(target_os = "macos")]
            regler::fenster_auswahl(app);
        }
        5 => ablage_shortcut(app),
        6 => werkzeug_zeigen(app, "clip"),
        7 => werkzeug_zeigen(app, "timer"),
        8 => werkzeug_zeigen(app, "platz"),
        9 => werkzeug_zeigen(app, "fokus"),
        _ => {}
    }
}

/// Schnellzugriff (Klick auf Noki): loest GENAU die bestehende Aktion des
/// Kuerzels Ctrl+n aus — keine zweite Umsetzung. Auf dem Hauptthread, weil
/// z. B. "Fenster schliessen" ein natives Menue zeigt.
#[tauri::command]
fn noki_schnell(app: tauri::AppHandle, n: u32) {
    let h = app.clone();
    let _ = app.run_on_main_thread(move || shortcut_ausloesen(&h, n));
}

#[tauri::command]
fn datei_oeffnen(pfad: String) -> bool {
    std::process::Command::new("/usr/bin/open").arg(&pfad).spawn().is_ok()
}

#[tauri::command]
fn datei_finder(pfad: String) -> bool {
    std::process::Command::new("/usr/bin/open").args(["-R", &pfad]).spawn().is_ok()
}

#[tauri::command]
fn datei_bearbeiten(pfad: String) -> bool {
    let p = std::path::Path::new(&pfad);
    let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
    let app = if ["png", "jpg", "jpeg", "webp", "gif", "tiff"].contains(&ext.as_str()) {
        "Preview"
    } else {
        "QuickTime Player"
    };
    std::process::Command::new("/usr/bin/open").args(["-a", app, &pfad]).spawn().is_ok()
}

#[tauri::command]
fn kamera_ordner_oeffnen() -> bool {
    if let Ok(home) = std::env::var("HOME") {
        let dir = std::path::PathBuf::from(home).join("Desktop").join("Noki Kamera");
        let _ = std::fs::create_dir_all(&dir);
        std::process::Command::new("/usr/bin/open").arg(&dir).spawn().is_ok()
    } else {
        false
    }
}

#[tauri::command]
fn noki_utility_vorn(app: tauri::AppHandle) {
    let h = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Ok(mut hit) = h.state::<NokiHitStore>().0.write() { hit.drag = true; }
        if let Some(win) = h.get_webview_window(FENSTER) {
            let _ = win.set_ignore_cursor_events(false);
            // Auch bei Ebene "normal"/"hinten": das Bedienfenster (Einstellungen,
            // Shortcuts) muss VOR der gerade aktiven App liegen. Zurueck zur
            // gewaehlten Ebene: noki_utility_zurueck beim Schliessen.
            ebene_setzen(&win, "vorn");
            #[cfg(target_os = "macos")]
            if let Ok(nw) = win.ns_window() {
                unsafe {
                    extern "C" { fn sel_registerName(n: *const i8) -> *const std::ffi::c_void; fn objc_msgSend(); }
                    let vor: unsafe extern "C" fn(*mut std::ffi::c_void, *const std::ffi::c_void) = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                    vor(nw, sel_registerName(b"orderFrontRegardless\0".as_ptr() as _));
                }
            }
            #[cfg(target_os = "macos")]
            unsafe {
                use std::ffi::c_void;
                extern "C" { fn objc_getClass(n: *const i8) -> *mut c_void; fn sel_registerName(n: *const i8) -> *const c_void; fn objc_msgSend(); }
                let get: unsafe extern "C" fn(*mut c_void, *const c_void) -> *mut c_void = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                let activate: unsafe extern "C" fn(*mut c_void, *const c_void, bool) = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                let nsapp = get(objc_getClass(b"NSApplication\0".as_ptr() as _), sel_registerName(b"sharedApplication\0".as_ptr() as _));
                activate(nsapp, sel_registerName(b"activateIgnoringOtherApps:\0".as_ptr() as _), true);
            }
            let _ = win.set_focus();
        }
    });
}

// ---- Noki Ablage: echter Ordner als Speicherort --------------------------
// ~/Documents/Noki Ablage ist ein normaler lokaler Ordner (Finder, Sichern-
// Dialog). Was dort fertig gespeichert wird, nimmt die BESTEHENDE Ablage auf
// (ablage_aufnehmen: Lesezeichen, keine Duplikate). Keine Kopie, keine DB.
fn noki_ablage_ordner() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(|h| PathBuf::from(h).join("Documents").join("Noki"))
}
/// Halbfertige Downloads und temporaere Dateien nie uebernehmen.
fn ablage_temp(p: &std::path::Path) -> bool {
    let n = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    n.is_empty() || n.starts_with('.') || n.starts_with("~$")
        || ["download", "crdownload", "part", "partial", "tmp", "temp", "opdownload"].iter().any(|e| n.ends_with(&format!(".{}", e)))
}
fn spawn_ablage_ordner(app: tauri::AppHandle) {
    let Some(dir) = noki_ablage_ordner() else { return };
    // Frueherer Name "Noki Ablage" -> "Noki" (so heisst er in Finder und
    // Sichern-Dialog). Die Lesezeichen der Eintraege folgen dem Umbenennen.
    if let Some(alt) = dir.parent().map(|d| d.join("Noki Ablage")) {
        if alt.is_dir() && !dir.exists() { let _ = fs::rename(&alt, &dir); }
    }
    let neu = !dir.exists();
    let _ = fs::create_dir_all(&dir);
    if neu {
        // Eine Seitenleisten-API ohne private Schnittstellen gibt es nicht:
        // Ordner einmal zeigen, der Nutzer zieht ihn in die Favoriten.
        let _ = std::process::Command::new("/usr/bin/open").arg(&dir).spawn();
        eprintln!("[ABLAGE] Ordner angelegt: {} — einmal in die Finder-Seitenleiste ziehen", dir.display());
    }
    thread::spawn(move || {
        let lesen = |d: &PathBuf| -> Vec<(PathBuf, u64, u64)> {
            fs::read_dir(d).map(|rd| rd.flatten().filter_map(|e| {
                let m = e.metadata().ok()?;
                let t = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64;
                Some((e.path(), if m.is_dir() { 0 } else { m.len() }, t))
            }).collect()).unwrap_or_default()
        };
        // Vorhandenes gilt als bekannt: ein entfernter Eintrag kehrt nicht zurueck.
        let mut bekannt: std::collections::HashSet<PathBuf> = lesen(&dir).into_iter().map(|e| e.0).collect();
        let mut kandidat: std::collections::HashMap<PathBuf, (u64, u64, u32)> = std::collections::HashMap::new();
        loop {
            thread::sleep(Duration::from_millis(1500));
            let jetzt = lesen(&dir);
            let da: std::collections::HashSet<PathBuf> = jetzt.iter().map(|e| e.0.clone()).collect();
            bekannt.retain(|p| da.contains(p));
            kandidat.retain(|p, _| da.contains(p));
            let mut fertig = Vec::new();
            for (p, len, mt) in jetzt {
                if bekannt.contains(&p) || ablage_temp(&p) { continue; }
                // Fertig = zwei ruhige Runden (~3 s gleiche Groesse/Zeit), Datei nicht leer.
                let e = kandidat.entry(p.clone()).or_insert((len, mt, 0));
                if e.0 == len && e.1 == mt { e.2 += 1; } else { *e = (len, mt, 0); }
                if e.2 >= 2 && (len > 0 || p.is_dir()) { fertig.push(p); }
            }
            for p in &fertig { kandidat.remove(p); bekannt.insert(p.clone()); }
            if !fertig.is_empty() {
                eprintln!("[ABLAGE] Noki Ablage: {} neue Datei(en) uebernommen", fertig.len());
                ablage_aufnehmen(&app, &fertig);
            }
        }
    });
}
/// Kompatible installierte Apps fuer einen Eintrag (Launch Services).
#[tauri::command]
fn ablage_apps(app: tauri::AppHandle, id: u64) -> Vec<serde_json::Value> {
    let Some(p) = ablage_pfad(&app, id) else { return Vec::new() };
    lesezeichen::apps_fuer_datei(&p).into_iter()
        .map(|(n, a, s)| serde_json::json!({ "name": n, "app": a, "standard": s })).collect()
}
/// Nur auf ausdruecklichen Klick: mit der gewaehlten App oeffnen.
#[tauri::command]
fn ablage_oeffnen_mit(app: tauri::AppHandle, id: u64, programm: String) -> bool {
    if !programm.ends_with(".app") || !std::path::Path::new(&programm).exists() { return false; }
    ablage_pfad(&app, id)
        .map(|p| std::process::Command::new("/usr/bin/open").args(["-a", &programm, &p]).spawn().is_ok())
        .unwrap_or(false)
}
fn im_noki_ordner(p: &str) -> bool {
    noki_ablage_ordner().map(|d| {
        let d = fs::canonicalize(&d).unwrap_or(d);
        fs::canonicalize(p).map(|x| x.starts_with(&d)).unwrap_or(false)
    }).unwrap_or(false)
}
/// Inhalt von ~/Documents/Noki fuer "Einstellungen > Noki-Ordner" — direkt
/// gelesen, keine zweite Verwaltung. Neueste zuerst, Temporaeres ausgelassen.
#[tauri::command]
fn noki_ordner_liste() -> Vec<serde_json::Value> {
    let Some(dir) = noki_ablage_ordner() else { return Vec::new() };
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
#[tauri::command]
fn noki_datei_apps(pfad: String) -> Vec<serde_json::Value> {
    if !im_noki_ordner(&pfad) { return Vec::new(); }
    lesezeichen::apps_fuer_datei(&pfad).into_iter()
        .map(|(n, a, s)| serde_json::json!({ "name": n, "app": a, "standard": s })).collect()
}
#[tauri::command]
fn noki_datei_oeffnen_mit(pfad: String, programm: String) -> bool {
    im_noki_ordner(&pfad) && programm.ends_with(".app") && std::path::Path::new(&programm).exists()
        && std::process::Command::new("/usr/bin/open").args(["-a", &programm, &pfad]).spawn().is_ok()
}
#[tauri::command]
fn noki_ordner_finder() -> bool {
    noki_ablage_ordner().map(|d| std::process::Command::new("/usr/bin/open").arg("-R").arg(&d).spawn().is_ok()).unwrap_or(false)
}
/// "Dateien auswählen …": normaler macOS-Dateidialog (im Noki-Ordner), die
/// Auswahl kommt als Verweis in die bestehende Ablage — nichts wird kopiert.
#[tauri::command]
fn noki_dateien_waehlen(app: tauri::AppHandle) {
    let Some(dir) = noki_ablage_ordner() else { return };
    thread::spawn(move || {
        let ort = format!("set d to POSIX file \"{}\"", dir.to_string_lossy().replace('"', ""));
        let aus = std::process::Command::new("/usr/bin/osascript").args(["-e", &ort,
            "-e", "set f to choose file with prompt \"Dateien für Noki auswählen\" default location d with multiple selections allowed",
            "-e", "set o to \"\"", "-e", "repeat with x in f", "-e", "set o to o & POSIX path of x & linefeed", "-e", "end repeat", "-e", "return o"])
            .output();
        let Ok(aus) = aus else { return };
        let pfade: Vec<PathBuf> = String::from_utf8_lossy(&aus.stdout).lines().filter(|l| !l.trim().is_empty()).map(PathBuf::from).collect();
        if !pfade.is_empty() { ablage_aufnehmen(&app, &pfade); }
    });
}
/// "Datei löschen": nur auf bestaetigten Klick und NUR fuer Dateien im
/// Noki-Ordner — in den Papierkorb (wiederherstellbar), Eintrag entfernen.
#[tauri::command]
fn ablage_loeschen(app: tauri::AppHandle, id: u64) -> bool {
    let (Some(p), Some(dir)) = (ablage_pfad(&app, id), noki_ablage_ordner()) else { return false };
    let dir = fs::canonicalize(&dir).unwrap_or(dir);
    if !std::path::Path::new(&p).starts_with(&dir) { return false; }
    let ok = lesezeichen::in_papierkorb(&p);
    if ok { ablage_entfernen(app, id); }
    ok
}
/// "Dateien löschen …" (Einstellungen, bestaetigt): alles im Noki-Ordner in
/// den Papierkorb, zugehoerige Eintraege entfernen. Andere Eintraege bleiben.
#[tauri::command]
fn noki_ordner_leeren(app: tauri::AppHandle) -> usize {
    let Some(dir) = noki_ablage_ordner() else { return 0 };
    let dir = fs::canonicalize(&dir).unwrap_or(dir);
    let mut n = 0;
    if let Ok(rd) = fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if !ablage_temp(&p) && lesezeichen::in_papierkorb(&p.to_string_lossy()) { n += 1; }
        }
    }
    {
        let st = app.state::<Ablage>();
        let mut l = st.0.lock().unwrap();
        l.retain(|e| !std::path::Path::new(&e.pfad).starts_with(&dir));
        ablage_speichern(&app, &l);
    }
    ablage_melden(&app, &[], None, None);
    n
}
#[tauri::command]
fn noki_ablage_ordner_oeffnen() -> bool {
    noki_ablage_ordner().map(|d| { let _ = fs::create_dir_all(&d); std::process::Command::new("/usr/bin/open").arg(&d).spawn().is_ok() }).unwrap_or(false)
}

// ---- Kompakte Noki-Uebersicht: eigenes natives Fenster -------------------
// Schneller Launcher (^/° doppelt). Echtes NSWindow (Mission Control, Fokus,
// Ampel), getrennt vom Overlay; jede Aktion ist der bestehende Kuerzel-Weg.
const KOMPAKT: &str = "kompakt";
fn kompakt_offen(app: &tauri::AppHandle) -> bool {
    app.get_webview_window(KOMPAKT).map_or(false, |w| w.is_visible().unwrap_or(false))
}
fn app_nach_vorn() {
    #[cfg(target_os = "macos")]
    unsafe {
        use std::ffi::c_void;
        extern "C" { fn objc_getClass(n: *const i8) -> *mut c_void; fn sel_registerName(n: *const i8) -> *const c_void; fn objc_msgSend(); }
        let get: unsafe extern "C" fn(*mut c_void, *const c_void) -> *mut c_void = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let activate: unsafe extern "C" fn(*mut c_void, *const c_void, bool) = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let nsapp = get(objc_getClass(b"NSApplication\0".as_ptr() as _), sel_registerName(b"sharedApplication\0".as_ptr() as _));
        activate(nsapp, sel_registerName(b"activateIgnoringOtherApps:\0".as_ptr() as _), true);
    }
}
fn kompakt_zeigen(app: &tauri::AppHandle) {
    // Nur ein Noki-Fenster zur Zeit: Settings/Arbeits-Panels im Overlay schliessen.
    let _ = app.emit("noki://panels_zu", serde_json::json!({}));
    let h = app.clone();
    let _ = app.run_on_main_thread(move || {
        let win = match h.get_webview_window(KOMPAKT) {
            Some(w) => w,
            None => match tauri::WebviewWindowBuilder::new(&h, KOMPAKT, tauri::WebviewUrl::App("kompakt.html".into()))
                .title("Noki").inner_size(320.0, 470.0).resizable(false).maximizable(false)
                .title_bar_style(tauri::TitleBarStyle::Overlay).hidden_title(true)
                .theme(Some(tauri::Theme::Dark)).center().visible(false).build()
            {
                Ok(w) => w,
                Err(e) => { eprintln!("[KOMPAKT] Fenster nicht erstellt: {}", e); return; }
            },
        };
        let _ = win.unminimize();
        let _ = win.show();
        app_nach_vorn();
        let _ = win.set_focus();
        let _ = h.emit("noki://kompakt_auf", serde_json::json!({}));
    });
}
fn kompakt_schliessen(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window(KOMPAKT) { let _ = w.close(); }
}
#[tauri::command]
fn kompakt_auf(app: tauri::AppHandle) { kompakt_zeigen(&app); }
#[tauri::command]
fn kompakt_zu(app: tauri::AppHandle) { kompakt_schliessen(&app); }
/// Klick in der kompakten Uebersicht: Fenster zu, DANN die bestehende Aktion
/// (n = 1..9 wie ^/° + n, 0 = Einstellungen). Nativ, weil die Seite samt
/// ihrem JavaScript mit dem Fenster verschwindet.
#[tauri::command]
fn kompakt_aktion(app: tauri::AppHandle, n: u32) {
    kompakt_schliessen(&app);
    if n == 0 { einstellungen_oeffnen(app); return; }
    let h = app.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(140));      // Fenster ist weg, bevor z. B. ein Foto entsteht
        let h2 = h.clone();
        let _ = h.run_on_main_thread(move || shortcut_ausloesen(&h2, n));
    });
}

/// Live-Stand der Freigabe "Bedienungshilfen" fuer DIESEN Prozess (nie gecacht).
#[tauri::command]
fn ax_status(app: tauri::AppHandle) -> serde_json::Value {
    let ok = lesezeichen::ax_vertraut(false);
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    eprintln!("[AX] Status: bundle={} exe={} pid={} AXIsProcessTrusted={}", app.config().identifier, exe, std::process::id(), ok);
    serde_json::json!({ "trusted": ok, "pid": std::process::id(), "bundle": app.config().identifier, "exe": exe })
}

/// Echten macOS-Freigabedialog ausloesen und Bedienungshilfen oeffnen.
#[tauri::command]
fn ax_freigabe() { ax_einstellungen_oeffnen(); }

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
        _ => if *lage.ebene.lock().unwrap() == "hinten" { "vorn" } else { "hinten" },
    };
    *lage.ebene.lock().unwrap() = neu;
    lage.durchgang.store(false, Ordering::Relaxed);
    if let Some(win) = app.get_webview_window(FENSTER) {
        lage_anwenden(&win, &lage);
    }
    neu.to_string()
}


#[cfg(target_os = "macos")]
mod tasten {
    use std::ffi::c_void;
    use std::sync::{Mutex, OnceLock};
    #[repr(C)] #[derive(Clone, Copy)] struct HotKeyId { signatur: u32, id: u32 }
    #[repr(C)] struct EventTyp { klasse: u32, art: u32 }
    #[link(name = "Carbon", kind = "framework")]
    extern "C" {
        fn GetApplicationEventTarget() -> *mut c_void;
        fn RegisterEventHotKey(code: u32, mods: u32, id: HotKeyId, ziel: *mut c_void, opt: u32, aus: *mut *mut c_void) -> i32;
        fn InstallEventHandler(ziel: *mut c_void, h: extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> i32,
                               n: u32, liste: *const EventTyp, user: *mut c_void, aus: *mut *mut c_void) -> i32;
        fn GetEventParameter(ev: *mut c_void, name: u32, typ: u32, aus_typ: *mut u32, groesse: usize,
                             aus_groesse: *mut usize, daten: *mut c_void) -> i32;
        fn UnregisterEventHotKey(r: *mut c_void) -> i32;
        fn LMGetKbdType() -> u8;
        fn KBGetLayoutType(typ: i16) -> u32;
    }
    static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
    static ZULETZT: Mutex<[Option<std::time::Instant>; 24]> = Mutex::new([None; 24]);
    // Noki-Praefix: physische Taste links neben "1" (^/° im deutschen Layout).
    // Registriert wird der KEYCODE, nicht das Zeichen (Dead-Key): ISO = 10,
    // ANSI = 50. Als Hotkey erreicht der Druck keine App — kein ^, kein Akzent.
    // Die Ziffern 1..5 sind nur FENSTER_MS lang nach dem Praefix belegt.
    const PRAEFIX: u32 = 10;
    const FENSTER_MS: u64 = 1300;
    const DOPPEL_MS: u128 = 420;
    // Nach dem Schliessen per Einzeltipp kurz gesperrt: kein Wieder-Oeffnen
    // durch einen nachgeschobenen zweiten Tipp.
    static SPERRE: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    static ZIFFERN: Mutex<Vec<usize>> = Mutex::new(Vec::new());
    static RUNDE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    fn ziffern_frei() {
        if let Ok(mut z) = ZIFFERN.lock() {
            for r in z.drain(..) { unsafe { UnregisterEventHotKey(r as *mut c_void); } }
        }
    }
    fn ziffern_belegen() {
        let mut z = match ZIFFERN.lock() { Ok(z) => z, Err(_) => return };
        if !z.is_empty() { return; }
        unsafe {
            let ziel = GetApplicationEventTarget();
            for (n, code) in [(1u32, 18u32), (2, 19), (3, 20), (4, 21), (5, 23), (6, 22), (7, 26), (8, 28), (9, 25)] {
                let mut r = std::ptr::null_mut();
                if RegisterEventHotKey(code, 0, HotKeyId { signatur: vz(b"NOKI"), id: PRAEFIX + n }, ziel, 0, &mut r) == 0 {
                    z.push(r as usize);
                }
            }
        }
    }
    const fn vz(s: &[u8; 4]) -> u32 { ((s[0] as u32) << 24) | ((s[1] as u32) << 16) | ((s[2] as u32) << 8) | (s[3] as u32) }
    extern "C" fn gedrueckt(_c: *mut c_void, ev: *mut c_void, _u: *mut c_void) -> i32 {
        let mut hk = HotKeyId { signatur: 0, id: 0 };
        let st = unsafe { GetEventParameter(ev, vz(b"----"), vz(b"hkid"), std::ptr::null_mut(),
            std::mem::size_of::<HotKeyId>(), std::ptr::null_mut(), &mut hk as *mut HotKeyId as *mut c_void) };
        if st != 0 || hk.signatur != vz(b"NOKI") || hk.id < PRAEFIX || hk.id > PRAEFIX + 9 { return -9874; }   // nicht unseres
        // Genau EINMAL je Tastendruck: Prellen abfangen (Praefix knapp, damit
        // kurze Folge Praefix/Ziffer moeglich bleibt; Ziffern 400 ms).
        let sperre = if hk.id == PRAEFIX { 80 } else { 400 };
        if hk.id == PRAEFIX {
            if let Ok(s) = SPERRE.lock() { if s.map_or(false, |t| std::time::Instant::now() < t) { return 0; } }
            // Kompakte Uebersicht offen: EIN Tipp schliesst sie (Taste verbraucht,
            // Ziffern bleiben frei, kein ^ erreicht eine App).
            if let Some(app) = APP.get() {
                if super::kompakt_offen(app) {
                    super::kompakt_schliessen(app);
                    ziffern_frei();
                    RUNDE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if let Ok(mut z) = ZULETZT.lock() { z[PRAEFIX as usize] = None; }
                    if let Ok(mut s) = SPERRE.lock() { *s = Some(std::time::Instant::now() + std::time::Duration::from_millis(DOPPEL_MS as u64)); }
                    return 0;
                }
            }
        }
        let mut doppelt = false;
        if let Ok(mut z) = ZULETZT.lock() {
            let jetzt = std::time::Instant::now();
            let vorher = z[hk.id as usize];
            if vorher.map_or(false, |t| jetzt.duration_since(t).as_millis() < sperre) { return 0; }
            // Doppeltipp auf die Praefix-Taste (< DOPPEL_MS) -> Noki Einstellungen.
            // Der dritte Tipp beginnt wieder von vorn (Zeitstempel geloescht).
            doppelt = hk.id == PRAEFIX && vorher.map_or(false, |t| jetzt.duration_since(t).as_millis() < DOPPEL_MS);
            z[hk.id as usize] = if doppelt { None } else { Some(jetzt) };
        }
        if doppelt {
            ziffern_frei();
            RUNDE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if let Some(app) = APP.get() { super::kompakt_zeigen(app); }
            return 0;
        }
        if hk.id == PRAEFIX {
            // Praefix: Ziffern kurz belegen, danach verfallen sie von selbst.
            ziffern_belegen();
            let runde = RUNDE.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            if let Some(app) = APP.get() {
                let h = app.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(FENSTER_MS));
                    if RUNDE.load(std::sync::atomic::Ordering::SeqCst) == runde { let _ = h.run_on_main_thread(ziffern_frei); }
                });
            }
            return 0;
        }
        // Ziffer im Praefix-Fenster: ein Kuerzel = eine Aktion, Ziffern sofort wieder frei.
        ziffern_frei();
        RUNDE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(app) = APP.get() { super::shortcut_ausloesen(app, hk.id - PRAEFIX); }
        0
    }
    pub fn registrieren(app: &tauri::AppHandle) {
        let _ = APP.set(app.clone());
        unsafe {
            let ziel = GetApplicationEventTarget();
            let typ = EventTyp { klasse: vz(b"keyb"), art: 5 };          // kEventHotKeyPressed
            let mut h = std::ptr::null_mut();
            if InstallEventHandler(ziel, gedrueckt, 1, &typ, std::ptr::null_mut(), &mut h) != 0 {
                eprintln!("[SHORTCUT] Tasten-Handler nicht installiert"); return;
            }
            // Ctrl+1..5 sind nicht mehr belegt (bleiben macOS). Nur die Praefix-Taste.
            let iso = KBGetLayoutType(LMGetKbdType() as i16) == vz(b"ISO ");
            let code = if iso { 10 } else { 50 };
            let mut r = std::ptr::null_mut();
            let st = RegisterEventHotKey(code, 0, HotKeyId { signatur: vz(b"NOKI"), id: PRAEFIX }, ziel, 0, &mut r);
            if st != 0 { eprintln!("[SHORTCUT] Praefix-Taste ^/° (Keycode {}) nicht registriert (Fehler {})", code, st); }
            else { eprintln!("[SHORTCUT] Praefix-Taste ^/° registriert (Keycode {}, {}), danach 1..9; Shortcuts im Arbeit-Menue", code, if iso { "ISO" } else { "ANSI" }); }
        }
    }
}

// ---- Maus folgen ------------------------------------------------------
struct MausMenue(tauri::menu::MenuItem<tauri::Wry>);

/// Die Seite meldet, ob "Maus folgen" laeuft -> Menuetext.
#[tauri::command]
fn noki_maus_menue(app: tauri::AppHandle, an: bool) {
    if let Some(m) = app.try_state::<MausMenue>() {
        let _ = m.0.set_text(if an { "Maus folgen beenden" } else { "Maus folgen" });
    }
}

// ---- Groesse (stufenlos) ---------------------------------------------
// Der Regler sitzt DIREKT im Untermenue "Groesse" (natives NSMenuItem mit
// eigener Ansicht, mod regler). Noki gleitet selbst zum Wert.
struct GroesseWert(std::sync::Mutex<f64>);

fn groesse_setzen(app: &tauri::AppHandle, faktor: f64) {
    if !faktor.is_finite() { return; }
    let f = faktor.clamp(0.45, 2.5);
    if let Some(g) = app.try_state::<GroesseWert>() { if let Ok(mut v) = g.0.lock() { *v = f; } }
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
    #[repr(C)] #[derive(Clone, Copy)] struct R { x: f64, y: f64, w: f64, h: f64 }
    // NSTextAlignment: auf Apple Silicon gelten die iOS-Werte (Mitte 1, rechts 2).
    #[cfg(target_arch = "aarch64")] const MITTE: isize = 1;
    #[cfg(target_arch = "aarch64")] const RECHTS: isize = 2;
    #[cfg(not(target_arch = "aarch64"))] const MITTE: isize = 2;
    #[cfg(not(target_arch = "aarch64"))] const RECHTS: isize = 1;
    static APP: std::sync::OnceLock<tauri::AppHandle> = std::sync::OnceLock::new();
    static WERT: AtomicUsize = AtomicUsize::new(0);

    fn sel(n: &str) -> Sel { let c = CString::new(n).unwrap_or_default(); unsafe { sel_registerName(c.as_ptr()) } }
    fn klasse(n: &str) -> Id { let c = CString::new(n).unwrap_or_default(); unsafe { objc_getClass(c.as_ptr()) } }
    unsafe fn f() -> unsafe extern "C" fn() { objc_msgSend as unsafe extern "C" fn() }
    unsafe fn m0(o: Id, s: &str) -> Id { let g: unsafe extern "C" fn(Id, Sel) -> Id = std::mem::transmute(f()); g(o, sel(s)) }
    unsafe fn m1(o: Id, s: &str, a: Id) -> Id { let g: unsafe extern "C" fn(Id, Sel, Id) -> Id = std::mem::transmute(f()); g(o, sel(s), a) }
    unsafe fn mf(o: Id, s: &str, a: f64) -> Id { let g: unsafe extern "C" fn(Id, Sel, f64) -> Id = std::mem::transmute(f()); g(o, sel(s), a) }
    unsafe fn mi(o: Id, s: &str, a: isize) { let g: unsafe extern "C" fn(Id, Sel, isize) = std::mem::transmute(f()); g(o, sel(s), a) }
    unsafe fn mr(o: Id, s: &str, r: R) -> Id { let g: unsafe extern "C" fn(Id, Sel, R) -> Id = std::mem::transmute(f()); g(o, sel(s), r) }
    unsafe fn gd(o: Id, s: &str) -> f64 { let g: unsafe extern "C" fn(Id, Sel) -> f64 = std::mem::transmute(f()); g(o, sel(s)) }
    unsafe fn text(s: &str) -> Id {
        let c = CString::new(s).unwrap_or_default();
        let g: unsafe extern "C" fn(Id, Sel, *const c_char) -> Id = std::mem::transmute(f());
        g(klasse("NSString"), sel("stringWithUTF8String:"), c.as_ptr())
    }
    // Normal (1.00) liegt exakt in der Mitte: links 0.45-1.00, rechts 1.00-2.50.
    fn auf_faktor(p: f64) -> f64 { if p <= 0.5 { 0.45 + p / 0.5 * 0.55 } else { 1.0 + (p - 0.5) / 0.5 * 1.5 } }
    fn auf_pos(k: f64) -> f64 { if k <= 1.0 { (k - 0.45) / 0.55 * 0.5 } else { 0.5 + (k - 1.0) / 1.5 * 0.5 } }

    extern "C" fn geaendert(_this: Id, _cmd: Sel, regler: Id) {
        unsafe {
            let mut p = gd(regler, "doubleValue");
            if (p - 0.5).abs() < 0.012 { p = 0.5; mf(regler, "setDoubleValue:", 0.5); }   // an Normal einrasten
            let k = if p == 0.5 { 1.0 } else { auf_faktor(p) };
            let w = WERT.load(Ordering::Relaxed) as Id;
            if !w.is_null() { m1(w, "setStringValue:", text(&format!("{:.2}×", k))); }
            if let Some(app) = APP.get() { super::groesse_setzen(app, k); }
        }
    }

    unsafe fn beschriftung(s: &str, r: R, pt: f64, fett: bool, leise: bool, ausr: isize) -> Id {
        let t = m1(klasse("NSTextField"), "labelWithString:", text(s));
        mr(t, "setFrame:", r);
        m1(t, "setFont:", mf(klasse("NSFont"), if fett { "boldSystemFontOfSize:" } else { "systemFontOfSize:" }, pt));
        if leise { m1(t, "setTextColor:", m0(klasse("NSColor"), "secondaryLabelColor")); }
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
            if let Some(app) = APP.get() { let _ = tauri::Emitter::emit(app, "noki://schliessen", serde_json::json!({ "id": id as i64 })); }
        }
    }
    // Hinweis-Eintrag "Bedienungshilfen fuer Noki aktivieren": oeffnet die Freigabe.
    extern "C" fn freigabe_oeffnen(_this: Id, _cmd: Sel, _e: Id) { super::ax_einstellungen_oeffnen(); }
    pub fn fenster_auswahl(app: &tauri::AppHandle) {
        let _ = APP.set(app.clone());
        let frei = super::ax_einstieg();        // live pruefen, ggf. macOS-Dialog
        let liste = super::schliessbare_fenster();
        unsafe {
            let mut ziel = FZ_ZIEL.load(Ordering::Relaxed) as Id;
            if ziel.is_null() {
                let mut kl = klasse("NokiFensterZiel");
                if kl.is_null() {
                    let n = CString::new("NokiFensterZiel").unwrap_or_default();
                    let t = CString::new("v@:@").unwrap_or_default();
                    kl = objc_allocateClassPair(klasse("NSObject"), n.as_ptr(), 0);
                    class_addMethod(kl, sel("waehlen:"), fenster_gewaehlt as *const c_void, t.as_ptr());
                    class_addMethod(kl, sel("freigabe:"), freigabe_oeffnen as *const c_void, t.as_ptr());
                    objc_registerClassPair(kl);
                }
                ziel = m0(m0(kl, "alloc"), "init");
                FZ_ZIEL.store(ziel as usize, Ordering::Relaxed);
            }
            let menue = m1(m0(klasse("NSMenu"), "alloc"), "initWithTitle:", text("Fenster schließen"));
            mi(menue, "setAutoenablesItems:", 0);
            let neu: unsafe extern "C" fn(Id, Sel, Id, Sel, Id) -> Id = std::mem::transmute(f());
            if liste.is_empty() {
                // Getrennt: Freigabe fehlt (anklickbar -> Einstellungen) vs. wirklich kein Fenster.
                let frei = frei || super::ax::vertraut();
                let hinweis = if frei { "Kein schließbares Fenster" } else { "Bedienungshilfen für Noki aktivieren …" };
                let aktion = if frei { std::ptr::null() } else { sel("freigabe:") };
                let e = neu(m0(klasse("NSMenuItem"), "alloc"), sel("initWithTitle:action:keyEquivalent:"), text(hinweis), aktion, text(""));
                if frei { mi(e, "setEnabled:", 0); } else { m1(e, "setTarget:", ziel); }
                m1(menue, "addItem:", e);
            }
            for (id, label) in liste {
                let e = neu(m0(klasse("NSMenuItem"), "alloc"), sel("initWithTitle:action:keyEquivalent:"), text(&label), sel("waehlen:"), text(""));
                m1(e, "setTarget:", ziel);
                mi(e, "setTag:", id as isize);
                m1(menue, "addItem:", e);
            }
            #[repr(C)] #[derive(Clone, Copy)] struct P { x: f64, y: f64 }
            let ort: unsafe extern "C" fn(Id, Sel) -> P = std::mem::transmute(f());
            let p = ort(klasse("NSEvent"), sel("mouseLocation"));
            let auf: unsafe extern "C" fn(Id, Sel, Id, P, Id) -> bool = std::mem::transmute(f());
            let _ = auf(menue, sel("popUpMenuPositioningItem:atLocation:inView:"), std::ptr::null_mut(), p, std::ptr::null_mut());
        }
    }

    /// Haengt den Regler in das ANGEZEIGTE Menue: Status-Item -> "Noki
    /// Einstellungen" -> "Groesse". Die Menueflaeche ist die des Systems.
    pub fn einbauen(app: &tauri::AppHandle, status_item: Id) {
        unsafe {
            let _ = APP.set(app.clone());
            let menue = m0(status_item, "menu");
            if menue.is_null() { return; }
            let ein = m1(menue, "itemWithTitle:", text("Noki Einstellungen"));
            if ein.is_null() { return; }
            let em = m0(ein, "submenu");
            if em.is_null() { return; }
            let gr = m1(em, "itemWithTitle:", text("Größe"));
            if gr.is_null() { return; }
            let gm = m0(gr, "submenu");
            if gm.is_null() { return; }
            let mut kl = klasse("NokiGroesseZiel");
            if kl.is_null() {
                let n = CString::new("NokiGroesseZiel").unwrap_or_default();
                let t = CString::new("v@:@").unwrap_or_default();
                kl = objc_allocateClassPair(klasse("NSObject"), n.as_ptr(), 0);
                class_addMethod(kl, sel("wert:"), geaendert as *const c_void, t.as_ptr());
                objc_registerClassPair(kl);
            }
            let ziel = m0(m0(kl, "alloc"), "init");   // lebt so lange wie die App
            let start = app.try_state::<super::GroesseWert>().and_then(|g| g.0.lock().ok().map(|v| *v)).unwrap_or(1.0);
            let ansicht = mr(m0(klasse("NSView"), "alloc"), "initWithFrame:", R { x: 0.0, y: 0.0, w: 264.0, h: 66.0 });
            let titel = beschriftung("Größe", R { x: 16.0, y: 42.0, w: 120.0, h: 18.0 }, 13.0, true, false, 0);
            let wert = beschriftung(&format!("{:.2}×", start), R { x: 140.0, y: 42.0, w: 108.0, h: 18.0 }, 13.0, true, false, RECHTS);
            let regler = mr(m0(klasse("NSSlider"), "alloc"), "initWithFrame:", R { x: 14.0, y: 18.0, w: 236.0, h: 24.0 });
            mf(regler, "setMinValue:", 0.0);
            mf(regler, "setMaxValue:", 1.0);
            mf(regler, "setDoubleValue:", auf_pos(start));
            mi(regler, "setContinuous:", 1);
            m1(regler, "setTarget:", ziel);
            let g: unsafe extern "C" fn(Id, Sel, Sel) = std::mem::transmute(f());
            g(regler, sel("setAction:"), sel("wert:"));
            let links = beschriftung("0.45×", R { x: 16.0, y: 2.0, w: 60.0, h: 14.0 }, 11.0, false, true, 0);
            let mitte = beschriftung("Normal", R { x: 102.0, y: 2.0, w: 60.0, h: 14.0 }, 11.0, false, true, MITTE);
            let rechts = beschriftung("2.50×", R { x: 188.0, y: 2.0, w: 60.0, h: 14.0 }, 11.0, false, true, RECHTS);
            let marke = mr(m0(klasse("NSView"), "alloc"), "initWithFrame:", R { x: 131.5, y: 40.0, w: 1.0, h: 5.0 });
            mi(marke, "setWantsLayer:", 1);
            m1(m0(marke, "layer"), "setBackgroundColor:", m0(m0(klasse("NSColor"), "tertiaryLabelColor"), "CGColor"));
            for v in [titel, wert, regler, links, mitte, rechts, marke] { m1(ansicht, "addSubview:", v); }
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
        let _ = m.0.set_text(if freeze { "Freeze beenden" } else { "Freeze" });
    }
    if FREEZE_SPERRE.swap(freeze, Ordering::Relaxed) == freeze { return; }
    let Some(win) = app.get_webview_window(FENSTER) else { return };
    if freeze {
        let _ = win.set_ignore_cursor_events(false);
        #[cfg(target_os = "macos")]
        freeze_fenster(&win, true);
    } else {
        #[cfg(target_os = "macos")]
        freeze_fenster(&win, false);
        lage_anwenden(&win, &app.state::<Lage>());   // Ebene wie gewaehlt
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
        let hol: unsafe extern "C" fn(*mut c_void, *const c_void) -> *mut c_void = std::mem::transmute(f);
        let lvl: unsafe extern "C" fn(*mut c_void, *const c_void, isize) = std::mem::transmute(f);
        let akt: unsafe extern "C" fn(*mut c_void, *const c_void, usize) -> bool = std::mem::transmute(f);
        let ws = hol(objc_getClass(b"NSWorkspace\0".as_ptr() as *const _), sel(b"sharedWorkspace\0"));
        if an {
            let vorn = hol(ws, sel(b"frontmostApplication\0"));
            let alt = FREEZE_VORHER_APP.swap(if vorn.is_null() { 0 } else { objc_retain(vorn) as usize }, Ordering::Relaxed);
            if alt != 0 { objc_release(alt as *mut c_void); }
            if let Ok(nw) = win.ns_window() { lvl(nw, sel(b"setLevel:\0"), 23); }
        } else {
            let alt = FREEZE_VORHER_APP.swap(0, Ordering::Relaxed);
            if alt != 0 {
                let a = alt as *mut c_void;
                // Nur zurueckholen, wenn ein Klick waehrend Freeze JARVIS nach vorn geholt hat.
                let jetzt = hol(ws, sel(b"frontmostApplication\0"));
                let ich = hol(objc_getClass(b"NSRunningApplication\0".as_ptr() as *const _), sel(b"currentApplication\0"));
                if jetzt == ich { let _ = akt(a, sel(b"activateWithOptions:\0"), 2); }
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
        Some(SchliessKnopf { x: r.0 + r.2 / 2.0, y: r.1 + r.3 / 2.0, w: r.2, h: r.3, id })
    }
    #[cfg(not(target_os = "macos"))]
    { let _ = id; None }
}

/// Die EINE Schliess-Aktion aller Effekte. Gedrueckt wird nur, wenn der
/// Knopf noch dort liegt, wo Noki ihn trifft (x/y Schirmpunkte, +-8 P).
#[tauri::command]
fn noki_fenster_schliessen(id: i64, x: f64, y: f64) -> bool {
    #[cfg(target_os = "macos")]
    unsafe {
        // Fehler getrennt benennen — nicht alles ist "Freigabe fehlt".
        if !ax::vertraut() { eprintln!("[AX] schliessen {}: Bedienungshilfen-Freigabe fehlt", id); return false; }
        let Some((k, r, _)) = ax::knopf(id) else {
            let grund = if ax::existiert(id) { "Schliessknopf nicht eindeutig oder inaktiv" } else { "Zielfenster verschwunden" };
            eprintln!("[AX] schliessen {}: {}", id, grund);
            return false;
        };
        let da = ((r.0 + r.2 / 2.0) - x).abs() <= 8.0 && ((r.1 + r.3 / 2.0) - y).abs() <= 8.0;
        let ok = da && ax::druecken(k);
        if !da { eprintln!("[AX] schliessen {}: Knopf liegt nicht mehr an der Zielstelle", id); }
        else if !ok { eprintln!("[AX] schliessen {}: AXPress fehlgeschlagen", id); }
        ax::freigeben(k);
        ok
    }
    #[cfg(not(target_os = "macos"))]
    { let _ = (id, x, y); false }
}

/// Meldet die Fensterliste ins Frontend, aber nur wenn sie sich geaendert
/// hat. Fenster bewegen sich in Menschentempo; 4 Hz reicht dafuer
/// vollstaendig und kostet nichts. Ein 60-Hz-Takt waere hier reine
/// Verschwendung — die Kollision selbst rechnet das Frontend jedes Bild.
const FENSTER_INTERVAL: Duration = Duration::from_millis(250);

fn spawn_fenster_watcher(app: tauri::AppHandle, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        let mut zuletzt: Option<Vec<Fenster>> = None;
        let mut menue: Option<Vec<(i64, String)>> = None;
        let mut menue_ax: Option<bool> = None;     // Freigabe gewechselt -> Menue neu (auch bei gleicher Liste)
        let mut takt: u32 = 0;
        while !stop.load(Ordering::Relaxed) {
            // Im Vollbild gibt es kein "hinter einem Fenster": keine Verdecker melden.
            let jetzt = if im_vollbild() { Vec::new() } else { sichtbare_fenster() };
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
const SPOTIFY_INTERVAL: Duration = Duration::from_millis(2000);

/// Deutet die Antwort von `player state`. Eigene Funktion, damit der
/// Selbsttest sie ohne laufendes Spotify pruefen kann.
pub fn spotify_spielt_aus_ausgabe(roh: &str) -> bool {
    roh.trim().eq_ignore_ascii_case("playing")
}

/// Spielt EIN bestimmter Player gerade? Genau derselbe Weg wie bisher —
/// erst `pgrep`, damit kein AppleScript eine nicht laufende App startet,
/// dann der Spielstand. Nur der Programmname ist jetzt ein Parameter.
#[cfg(target_os = "macos")]
fn player_spielt(app: &str) -> bool {
    use std::process::Command;
    let laeuft = Command::new("/usr/bin/pgrep")
        .args(["-x", app])
        .output()
        .map(|a| a.status.success() && !a.stdout.is_empty())
        .unwrap_or(false);
    if !laeuft {
        return false;
    }
    let skript = format!("tell application \"{}\" to player state as string", app);
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
    thread::spawn(move || {
        let mut zuletzt: Option<bool> = None;
        while !stop.load(Ordering::Relaxed) {
            let jetzt = spotify_spielt();
            if zuletzt != Some(jetzt) {
                let _ = app.emit("noki://spotify", serde_json::json!({ "spielt": jetzt }));
                eprintln!("[RUST] Spotify spielt: {}", jetzt);
                zuletzt = Some(jetzt);
            }
            thread::sleep(SPOTIFY_INTERVAL);
        }
    });
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct NokiHitZustand {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub drag: bool,
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
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let typ = match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "heic" | "webp" | "tif" | "tiff" | "bmp" | "svg" => "bild",
        "zip" | "rar" | "7z" | "tar" | "gz" | "tgz" | "dmg" => "archiv",
        "pdf" | "doc" | "docx" | "txt" | "md" | "rtf" | "pages" | "key" | "numbers" | "xls" | "xlsx"
        | "ppt" | "pptx" | "csv" => "dokument",
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
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
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
pub fn ablage_neu(liste: &mut Vec<AblageEintrag>, pfade: &[PathBuf], jetzt: u64) -> (Vec<u64>, Option<u64>) {
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
        let name = echt.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| s.clone());
        let bm = lesezeichen::erstellen(&s).map(|b| ablage_hex(&b)).unwrap_or_default();
        liste.insert(0, AblageEintrag { id, name, typ, ext, pfad: s, bm, zeit: jetzt, da: true });
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
        let t = if liste.is_empty() { "Ablage".to_string() } else { format!("Ablage ({})", liste.len()) };
        let _ = m.0.set_text(t);
    }
    let _ = app.emit("noki://ablage", serde_json::json!({ "liste": liste, "neu": neu, "dup": dup, "offen": offen }));
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
        (Some((x, y)), Some(st)) => st.0.read().map(|h| {
            h.w > 0
                && x >= (h.x - 14) as f64
                && x <= (h.x + h.w + 14) as f64
                && y >= (h.y - 14) as f64
                && y <= (h.y + h.h + 14) as f64
        }).unwrap_or(false),
        _ => false,
    };
    eprintln!("[RUST] ablage drop: {} Pfad(e), ueber Noki: {}", pfade.len(), ueber);
    if ueber && !pfade.is_empty() {
        ablage_aufnehmen(app, &pfade);
    }
}

fn ablage_pfad(app: &tauri::AppHandle, id: u64) -> Option<String> {
    let st = app.state::<Ablage>();
    let mut l = st.0.lock().unwrap();
    let e = l.iter_mut().find(|e| e.id == id)?;
    ablage_aufloesen(e);
    if e.da { Some(e.pfad.clone()) } else { None }
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
        .map(|p| std::process::Command::new("/usr/bin/open").arg(&p).spawn().is_ok())
        .unwrap_or(false)
}

/// Nur auf ausdruecklichen Klick: im Finder zeigen (markieren).
#[tauri::command]
fn ablage_finder(app: tauri::AppHandle, id: u64) -> bool {
    ablage_pfad(&app, id)
        .map(|p| std::process::Command::new("/usr/bin/open").args(["-R", &p]).spawn().is_ok())
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
        eprintln!("[ABLAGE] Ctrl+5: {} Datei(en) aus der Finder-Auswahl", pfade.len());
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
        match std::process::Command::new("/usr/bin/osascript").args(["-e", skript]).output() {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .map(PathBuf::from)
                .filter(|p| p.exists())
                .collect(),
            Ok(o) => {
                eprintln!("[ABLAGE] Finder-Auswahl nicht lesbar: {}", String::from_utf8_lossy(&o.stderr).trim());
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
    if was == "shortcuts" { kompakt_zeigen(app); return; }
    let _ = app.emit("noki://werkzeug", serde_json::json!({ "was": was }));
}

fn werk_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    let d = app.path().app_config_dir().ok()?;
    let _ = fs::create_dir_all(&d);
    Some(d)
}

fn jetzt_s() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Energie wie aus dem Menue setzen (Haken wandert mit) — fuer den Fokusmodus.
#[tauri::command]
fn noki_energie(app: tauri::AppHandle, stufe: String) {
    if !["still", "sparend", "normal", "energisch"].contains(&stufe.as_str()) { return; }
    let _ = app.emit("noki://energie", serde_json::json!({ "stufe": stufe }));
    if let Some(m) = app.try_state::<EnergieMenue>() {
        let ziel = format!("energie_{}", stufe);
        for (k, item) in &m.0 { let _ = item.set_checked(*k == ziel); }
    }
}

// ---- Zwischenablage-Verlauf ---------------------------------------------
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct ClipEintrag {
    pub id: u64,
    pub art: String,
    pub text: String,
    #[serde(default)] pub titel: String,
    #[serde(default)] pub bild: String,
    #[serde(default)] pub hash: String,
    pub zeit: u64,
    #[serde(default)] pub lang: usize,
}
pub struct Clip { pub liste: Mutex<Vec<ClipEintrag>>, pub eigen: std::sync::atomic::AtomicI64 }
const CLIP_MAX: usize = 20;
const CLIP_TEXT_MAX: usize = 100_000;

pub fn clip_ist_link(t: &str) -> bool {
    let s = t.trim();
    (s.starts_with("http://") || s.starts_with("https://")) && s.len() > 10 && !s.contains(char::is_whitespace)
}

/// Vorn einordnen: unmittelbar gleicher Inhalt wird nicht doppelt gespeichert,
/// ueber CLIP_MAX faellt der aelteste heraus. (aufgenommen?, herausgefallen)
pub fn clip_einordnen(l: &mut Vec<ClipEintrag>, e: ClipEintrag) -> (bool, Vec<ClipEintrag>) {
    if let Some(f) = l.first() {
        let gleich = if e.art == "bild" { f.art == "bild" && f.hash == e.hash } else { f.art != "bild" && f.text == e.text };
        if gleich { return (false, Vec::new()); }
    }
    l.insert(0, e);
    let mut weg = Vec::new();
    while l.len() > CLIP_MAX { if let Some(x) = l.pop() { weg.push(x); } }
    (true, weg)
}

fn clip_datei(app: &tauri::AppHandle) -> Option<PathBuf> { Some(werk_dir(app)?.join("zwischenablage.json")) }
fn clip_bilddir(app: &tauri::AppHandle) -> Option<PathBuf> {
    let d = werk_dir(app)?.join("zwischenablage");
    let _ = fs::create_dir_all(&d);
    Some(d)
}
fn clip_speichern(app: &tauri::AppHandle, l: &[ClipEintrag]) {
    if let (Some(d), Ok(s)) = (clip_datei(app), serde_json::to_string(l)) { let _ = fs::write(d, s); }
}
fn clip_laden(app: &tauri::AppHandle) -> Vec<ClipEintrag> {
    clip_datei(app).and_then(|d| fs::read_to_string(d).ok()).and_then(|r| serde_json::from_str(&r).ok()).unwrap_or_default()
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
        let n = ((c[0] as u32) << 16) | ((*c.get(1).unwrap_or(&0) as u32) << 8) | (*c.get(2).unwrap_or(&0) as u32);
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    s
}
fn fnv(d: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in d { h ^= *b as u64; h = h.wrapping_mul(0x100000001b3); }
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
    let _ = app.emit("noki://clip", serde_json::json!({ "liste": clip_fuer_ui(&l) }));
}

fn clip_aufnehmen(app: &tauri::AppHandle, inh: lesezeichen::PbInhalt) {
    let st = app.state::<Clip>();
    let id = st.liste.lock().unwrap().iter().map(|e| e.id).max().unwrap_or(0) + 1;
    let e = match inh {
        lesezeichen::PbInhalt::Text(t, titel) => {
            if t.trim().is_empty() { return; }
            let lang = t.chars().count();
            let t: String = if lang > CLIP_TEXT_MAX { t.chars().take(CLIP_TEXT_MAX).collect() } else { t };
            let art = if clip_ist_link(&t) { "link" } else { "text" };
            ClipEintrag { id, art: art.into(), text: t, titel, bild: String::new(), hash: String::new(), zeit: jetzt_s(), lang }
        }
        lesezeichen::PbInhalt::Bild(daten) => {
            let hash = fnv(&daten);
            if st.liste.lock().unwrap().first().map_or(false, |f| f.art == "bild" && f.hash == hash) { return; }
            let dir = match clip_bilddir(app) { Some(d) => d, None => return };
            let roh = dir.join(format!("{id}.roh"));
            let png = dir.join(format!("{id}.png"));
            let th = dir.join(format!("{id}_t.png"));
            if fs::write(&roh, &daten).is_err() { return; }
            let _ = std::process::Command::new("/usr/bin/sips").args(["-s", "format", "png"]).arg(&roh).arg("--out").arg(&png).output();
            let _ = fs::remove_file(&roh);
            if !png.exists() { return; }
            let _ = std::process::Command::new("/usr/bin/sips").args(["-Z", "160"]).arg(&png).arg("--out").arg(&th).output();
            ClipEintrag { id, art: "bild".into(), text: String::new(), titel: String::new(),
                          bild: png.to_string_lossy().into_owned(), hash, zeit: jetzt_s(), lang: 0 }
        }
    };
    let (neu, weg) = {
        let mut l = st.liste.lock().unwrap();
        let r = clip_einordnen(&mut l, e.clone());
        if r.0 { clip_speichern(app, &l); }
        r
    };
    if !neu { clip_dateien_weg(&e); return; }
    for x in &weg { clip_dateien_weg(x); }
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
            if z == letzter { continue; }
            letzter = z;
            if app.state::<Clip>().eigen.load(Ordering::Relaxed) == z { continue; }
            if let Some(inh) = lesezeichen::pb_lesen() { clip_aufnehmen(&app, inh); }
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
    app.state::<Clip>().liste.lock().unwrap().iter().find(|e| e.id == id).map(|e| e.text.clone()).unwrap_or_default()
}

/// Nur auf Klick: Eintrag wieder in die Zwischenablage legen (und nach vorn holen).
#[tauri::command]
fn clip_kopieren(app: tauri::AppHandle, id: u64) -> bool {
    let st = app.state::<Clip>();
    let e = {
        let mut l = st.liste.lock().unwrap();
        let i = match l.iter().position(|e| e.id == id) { Some(i) => i, None => return false };
        let e = l.remove(i);
        l.insert(0, e.clone());
        clip_speichern(&app, &l);
        e
    };
    let ok = if e.art == "bild" { lesezeichen::pb_bild_setzen(&e.bild) } else { lesezeichen::pb_text_setzen(&e.text) };
    st.eigen.store(lesezeichen::pb_zaehler(), Ordering::Relaxed);
    clip_melden(&app);
    ok
}

/// Nur auf Klick, nur http(s): Link im Standardbrowser oeffnen.
#[tauri::command]
fn clip_link_oeffnen(app: tauri::AppHandle, id: u64) -> bool {
    let u = app.state::<Clip>().liste.lock().unwrap().iter()
        .find(|e| e.id == id && e.art == "link").map(|e| e.text.trim().to_string());
    match u {
        Some(u) if clip_ist_link(&u) => std::process::Command::new("/usr/bin/open").arg(&u).spawn().is_ok(),
        _ => false,
    }
}

#[tauri::command]
fn clip_entfernen(app: tauri::AppHandle, id: u64) {
    {
        let st = app.state::<Clip>();
        let mut l = st.liste.lock().unwrap();
        if let Some(i) = l.iter().position(|e| e.id == id) { let e = l.remove(i); clip_dateien_weg(&e); }
        clip_speichern(&app, &l);
    }
    clip_melden(&app);
}

#[tauri::command]
fn clip_leeren(app: tauri::AppHandle) {
    {
        let st = app.state::<Clip>();
        let mut l = st.liste.lock().unwrap();
        for e in l.iter() { clip_dateien_weg(e); }
        l.clear();
        clip_speichern(&app, &l);
    }
    clip_melden(&app);
}

// ---- Arbeitsplatz-Profile -------------------------------------------------
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct PlatzApp { pub name: String, pub bundle: String, pub pfad: String, pub fenster: Vec<[f64; 4]> }
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct PlatzProfil { pub apps: Vec<PlatzApp>, pub zeit: u64 }
const PROFILE: [&str; 3] = ["Uni", "Coding", "Normal"];

fn platz_datei(app: &tauri::AppHandle) -> Option<PathBuf> { Some(werk_dir(app)?.join("arbeitsplatz.json")) }
fn platz_alle(app: &tauri::AppHandle) -> std::collections::HashMap<String, PlatzProfil> {
    platz_datei(app).and_then(|d| fs::read_to_string(d).ok()).and_then(|r| serde_json::from_str(&r).ok()).unwrap_or_default()
}

#[tauri::command]
fn platz_liste(app: tauri::AppHandle) -> serde_json::Value {
    let a = platz_alle(&app);
    let profile: Vec<serde_json::Value> = PROFILE.iter().map(|n| {
        let p = a.get(*n);
        serde_json::json!({ "name": n,
            "apps": p.map(|p| p.apps.iter().map(|x| x.name.clone()).collect::<Vec<_>>()).unwrap_or_default(),
            "zeit": p.map(|p| p.zeit).unwrap_or(0) })
    }).collect();
    serde_json::json!({ "ax": lesezeichen::ax_vertraut(false), "profile": profile })
}

/// Speichert die offenen Apps und ihre Fenster (Position/Groesse, globale
/// Koordinaten = Display). Fensterlagen brauchen die Freigabe Bedienungshilfen;
/// ohne sie wird nur die App-Liste gemerkt. Keine Inhalte, nichts wird beendet.
#[tauri::command]
fn platz_speichern(app: tauri::AppHandle, profil: String) -> serde_json::Value {
    if !PROFILE.contains(&profil.as_str()) { return serde_json::json!({ "ok": false }); }
    let mut ax = lesezeichen::ax_vertraut(false);
    if !ax { ax = lesezeichen::ax_vertraut(true); }   // einmalig nachfragen — auf ausdruecklichen Klick
    let apps: Vec<PlatzApp> = lesezeichen::apps_mit_fenstern(ax).into_iter()
        .map(|(name, bundle, pfad, fenster)| PlatzApp { name, bundle, pfad, fenster }).collect();
    let n = apps.len();
    let f: usize = apps.iter().map(|a| a.fenster.len()).sum();
    let mut alle = platz_alle(&app);
    alle.insert(profil, PlatzProfil { apps, zeit: jetzt_s() });
    let ok = match (platz_datei(&app), serde_json::to_string_pretty(&alle)) {
        (Some(d), Ok(s)) => fs::write(d, s).is_ok(),
        _ => false,
    };
    serde_json::json!({ "ok": ok, "apps": n, "fenster": f, "ax": ax })
}

/// Profil laden: fehlende Apps oeffnen (im Hintergrund), offene weiterverwenden
/// (keine Duplikatfenster), Fenster anordnen. Meldet das Ergebnis per Event.
#[tauri::command]
fn platz_laden(app: tauri::AppHandle, profil: String) {
    if !PROFILE.contains(&profil.as_str()) { return; }
    thread::spawn(move || {
        let prof = platz_alle(&app).get(&profil).cloned().unwrap_or_default();
        let ax = lesezeichen::ax_vertraut(false);
        let (mut offen, mut fehlt, mut gesetzt) = (Vec::new(), Vec::new(), 0usize);
        for a in &prof.apps {
            let mut pid = if a.bundle.is_empty() { None } else { lesezeichen::pid_fuer_bundle(&a.bundle) };
            if pid.is_none() {
                let per_id = !a.bundle.is_empty() && std::process::Command::new("/usr/bin/open")
                    .args(["-g", "-b", &a.bundle]).status().map(|s| s.success()).unwrap_or(false);
                let per_pfad = !per_id && !a.pfad.is_empty() && std::path::Path::new(&a.pfad).exists()
                    && std::process::Command::new("/usr/bin/open").args(["-g", "-a", &a.pfad]).status().map(|s| s.success()).unwrap_or(false);
                if !per_id && !per_pfad { fehlt.push(a.name.clone()); continue; }
                offen.push(a.name.clone());
                for _ in 0..30 {
                    thread::sleep(Duration::from_millis(200));
                    pid = lesezeichen::pid_fuer_bundle(&a.bundle);
                    if pid.is_some() { break; }
                }
            }
            if let (Some(p), true) = (pid, ax) {
                if !a.fenster.is_empty() { gesetzt += lesezeichen::fenster_setzen(p, &a.fenster, 4000); }
            }
        }
        let _ = app.emit("noki://platz", serde_json::json!({ "profil": profil, "geoeffnet": offen, "fehlt": fehlt,
            "gesetzt": gesetzt, "ax": ax, "leer": prof.apps.is_empty() }));
    });
}

// ---- Fokusmodus -------------------------------------------------------------
/// Installierte Apps (Programme-Ordner), keine feste Liste.
#[tauri::command]
fn fokus_apps() -> Vec<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let eigene = format!("{home}/Applications");
    let dirs = ["/Applications", "/Applications/Utilities", "/System/Applications", "/System/Applications/Utilities", eigene.as_str()];
    let mut v: Vec<(String, String)> = Vec::new();
    for d in dirs {
        if let Ok(rd) = fs::read_dir(d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().map_or(false, |x| x == "app") {
                    let n = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    v.push((n, p.to_string_lossy().into_owned()));
                }
            }
        }
    }
    v.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    v.dedup_by(|a, b| a.0 == b.0);
    v.into_iter().map(|(n, p)| serde_json::json!({ "name": n, "pfad": p })).collect()
}

/// Oeffnet ausgewaehlte .app-Pakete (nur echte Programme). Rueckgabe: nicht gefunden.
#[tauri::command]
fn apps_oeffnen(pfade: Vec<String>) -> Vec<String> {
    let mut fehlt = Vec::new();
    for p in pfade {
        if !p.ends_with(".app") || !std::path::Path::new(&p).exists()
            || std::process::Command::new("/usr/bin/open").args(["-a", &p]).spawn().is_err() {
            fehlt.push(p);
        }
    }
    fehlt
}

/// Kleine lokale Werkzeug-Daten (Fokus-Auswahl, laufender Timer, Einstellungen).
fn werk_daten_pfad(app: &tauri::AppHandle, k: &str) -> Option<PathBuf> {
    if !["fokus", "timer", "einstellungen", "freeze", "kamera"].contains(&k) { return None; }
    Some(werk_dir(app)?.join(format!("werk-{k}.json")))
}
#[tauri::command]
fn werk_daten_lesen(app: tauri::AppHandle, schluessel: String) -> serde_json::Value {
    werk_daten_pfad(&app, &schluessel).and_then(|d| fs::read_to_string(d).ok())
        .and_then(|r| serde_json::from_str(&r).ok()).unwrap_or(serde_json::Value::Null)
}
#[tauri::command]
fn werk_daten_schreiben(app: tauri::AppHandle, schluessel: String, wert: serde_json::Value) -> bool {
    match (werk_daten_pfad(&app, &schluessel), serde_json::to_string(&wert)) {
        (Some(d), Ok(s)) => fs::write(d, s).is_ok(),
        _ => false,
    }
}

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
    #[repr(C)] #[derive(Clone, Copy)] struct Punkt { x: f64, y: f64 }
    #[repr(C)] #[derive(Clone, Copy)] struct Rahmen { x: f64, y: f64, w: f64, h: f64 }

    unsafe fn k(n: &[u8]) -> Id { objc_getClass(n.as_ptr() as *const c_char) }
    unsafe fn s(n: &[u8]) -> Id { sel_registerName(n.as_ptr() as *const c_char) }
    fn msg() -> *const c_void { objc_msgSend as unsafe extern "C" fn() as *const c_void }

    unsafe fn url(pfad: &str) -> Id {
        let c = match CString::new(pfad) { Ok(c) => c, Err(_) => return std::ptr::null_mut() };
        let f: extern "C" fn(Id, Id, *const c_char) -> Id = std::mem::transmute(msg());
        let ns = f(k(b"NSString\0"), s(b"stringWithUTF8String:\0"), c.as_ptr());
        if ns.is_null() { return ns; }
        let g: extern "C" fn(Id, Id, Id) -> Id = std::mem::transmute(msg());
        g(k(b"NSURL\0"), s(b"fileURLWithPath:\0"), ns)
    }

    unsafe fn ns_text(ns: Id) -> String {
        if ns.is_null() { return String::new(); }
        let f: extern "C" fn(Id, Id) -> *const c_char = std::mem::transmute(msg());
        let c = f(ns, s(b"UTF8String\0"));
        if c.is_null() { String::new() } else { CStr::from_ptr(c).to_string_lossy().into_owned() }
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
                let std_pfad = if std_url.is_null() { String::new() } else { ns_text(g(std_url, s(b"path\0"))) };
                let arr = f1(ws, s(b"URLsForApplicationsToOpenURL:\0"), u);
                if !arr.is_null() {
                    for i in 0..n(arr, s(b"count\0")) {
                        let p = ns_text(g(at(arr, s(b"objectAtIndex:\0"), i), s(b"path\0")));
                        if p.is_empty() { continue; }
                        let name = std::path::Path::new(&p).file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
                        let st = p == std_pfad;
                        v.push((name, p, st));
                    }
                }
            }
            objc_autoreleasePoolPop(pool);
        }
        v.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.to_lowercase().cmp(&b.0.to_lowercase())));
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
                let t: extern "C" fn(Id, Id, Id, *mut Id, *mut Id) -> i8 = std::mem::transmute(msg());
                let fm = g(k(b"NSFileManager\0"), s(b"defaultManager\0"));
                ok = t(fm, s(b"trashItemAtURL:resultingItemURL:error:\0"), u, std::ptr::null_mut(), std::ptr::null_mut()) != 0;
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
                let f: extern "C" fn(Id, Id, u64, Id, Id, *mut Id) -> Id = std::mem::transmute(msg());
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
        if id.is_null() { return None; }
        let fu: extern "C" fn(Id, Id) -> *const c_char = std::mem::transmute(msg());
        let c = fu(id, s(b"UTF8String\0"));
        if c.is_null() { None } else { Some(CStr::from_ptr(c).to_string_lossy().into_owned()) }
    }

    /// Bundle-Id der App, die gerade vorne ist (NSWorkspace).
    pub fn vorne_bundle() -> Option<String> {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let f: extern "C" fn(Id, Id) -> Id = std::mem::transmute(msg());
            let ws = f(k(b"NSWorkspace\0"), s(b"sharedWorkspace\0"));
            let a = if ws.is_null() { ws } else { f(ws, s(b"frontmostApplication\0")) };
            let out = if a.is_null() { None } else { text(f(a, s(b"bundleIdentifier\0"))) };
            objc_autoreleasePoolPop(pool);
            out
        }
    }

    // Zieh-Quelle: erlaubt ausschliesslich NSDragOperationCopy (1).
    extern "C" fn nur_kopieren(_s: Id, _c: Id, _sess: Id, _ctx: isize) -> usize { 1 }
    static QUELLE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    unsafe fn quelle() -> Id {
        *QUELLE.get_or_init(|| {
            #[allow(unused_unsafe)]
            unsafe {
                let c = objc_allocateClassPair(k(b"NSObject\0"), b"NokiAblageZiehen\0".as_ptr() as *const c_char, 0);
                if c.is_null() { return 0; }
                class_addMethod(c, s(b"draggingSession:sourceOperationMaskForDraggingContext:\0"),
                                nur_kopieren as *const c_void, b"Q@:@q\0".as_ptr() as *const c_char);
                let pr = objc_getProtocol(b"NSDraggingSource\0".as_ptr() as *const c_char);
                if !pr.is_null() { class_addProtocol(c, pr); }
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
        let view = if nswin.is_null() { nswin } else { f0(nswin, s(b"contentView\0")) };
        let src = quelle();
        let mut ok = false;
        if !u.is_null() && !view.is_null() && !src.is_null() {
            let fp: extern "C" fn(Id, Id) -> Punkt = std::mem::transmute(msg());
            let pw = fp(nswin, s(b"mouseLocationOutsideOfEventStream\0"));
            let fc: extern "C" fn(Id, Id, Punkt, Id) -> Punkt = std::mem::transmute(msg());
            let pv = fc(view, s(b"convertPoint:fromView:\0"), pw, std::ptr::null_mut());
            let item = fi(f0(k(b"NSDraggingItem\0"), s(b"alloc\0")), s(b"initWithPasteboardWriter:\0"), u);
            let ws = f0(k(b"NSWorkspace\0"), s(b"sharedWorkspace\0"));
            let fs: extern "C" fn(Id, Id, *const c_char) -> Id = std::mem::transmute(msg());
            let cp = CString::new(pfad).unwrap_or_default();
            let ns = fs(k(b"NSString\0"), s(b"stringWithUTF8String:\0"), cp.as_ptr());
            let icon = if ws.is_null() || ns.is_null() { std::ptr::null_mut() } else { fi(ws, s(b"iconForFile:\0"), ns) };
            let fr: extern "C" fn(Id, Id, Rahmen, Id) = std::mem::transmute(msg());
            fr(item, s(b"setDraggingFrame:contents:\0"), Rahmen { x: pv.x - 16.0, y: pv.y - 16.0, w: 32.0, h: 32.0 }, icon);
            let fwn: extern "C" fn(Id, Id) -> isize = std::mem::transmute(msg());
            let wn = fwn(nswin, s(b"windowNumber\0"));
            let fup: extern "C" fn(Id, Id) -> f64 = std::mem::transmute(msg());
            let ts = fup(f0(k(b"NSProcessInfo\0"), s(b"processInfo\0")), s(b"systemUptime\0"));
            let fe: extern "C" fn(Id, Id, usize, Punkt, usize, f64, isize, Id, isize, isize, f32) -> Id = std::mem::transmute(msg());
            let ev = fe(k(b"NSEvent\0"),
                        s(b"mouseEventWithType:location:modifierFlags:timestamp:windowNumber:context:eventNumber:clickCount:pressure:\0"),
                        6, pw, 0, ts, wn, std::ptr::null_mut(), 0, 1, 1.0);   // 6 = LeftMouseDragged
            let arr = fi(k(b"NSArray\0"), s(b"arrayWithObject:\0"), item);
            if !ev.is_null() && !item.is_null() && !arr.is_null() {
                let fb: extern "C" fn(Id, Id, Id, Id, Id) -> Id = std::mem::transmute(msg());
                ok = !fb(view, s(b"beginDraggingSessionWithItems:event:source:\0"), arr, ev, src).is_null();
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
        if o.is_null() { return o; }
        let f: extern "C" fn(Id, Id) -> Id = std::mem::transmute(msg());
        f(o, s(sel))
    }
    unsafe fn m1(o: Id, sel: &[u8], a: Id) -> Id {
        if o.is_null() { return o; }
        let f: extern "C" fn(Id, Id, Id) -> Id = std::mem::transmute(msg());
        f(o, s(sel), a)
    }
    unsafe fn anzahl(arr: Id) -> usize {
        if arr.is_null() { return 0; }
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
    pub enum PbInhalt { Text(String, String), Bild(Vec<u8>) }
    pub fn pb_zaehler() -> i64 {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let pb = m0(k(b"NSPasteboard\0"), b"generalPasteboard\0");
            let n = if pb.is_null() { -1 } else {
                let f: extern "C" fn(Id, Id) -> isize = std::mem::transmute(msg());
                f(pb, s(b"changeCount\0")) as i64
            };
            objc_autoreleasePoolPop(pool);
            n
        }
    }
    unsafe fn hat_typ(typen: Id, t: &str) -> bool {
        if typen.is_null() { return false; }
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
            let geheim = ["org.nspasteboard.ConcealedType", "org.nspasteboard.TransientType",
                          "org.nspasteboard.AutoGeneratedType", "public.file-url"].iter().any(|t| hat_typ(typen, t));
            let mut out = None;
            if !pb.is_null() && !geheim {
                if let Some(t) = text(m1(pb, b"stringForType:\0", ns("public.utf8-plain-text"))) {
                    let titel = text(m1(pb, b"stringForType:\0", ns("public.url-name"))).unwrap_or_default();
                    out = Some(PbInhalt::Text(t, titel));
                } else {
                    for typ in ["public.png", "public.tiff"] {
                        let d = m1(pb, b"dataForType:\0", ns(typ));
                        if d.is_null() { continue; }
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
                ok = f(pb, s(b"setString:forType:\0"), ns(t), ns("public.utf8-plain-text")) != 0;
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
            if !frage { return AXIsProcessTrusted(); }
            let pool = objc_autoreleasePoolPush();
            let fb: extern "C" fn(Id, Id, i8) -> Id = std::mem::transmute(msg());
            let ja = fb(k(b"NSNumber\0"), s(b"numberWithBool:\0"), 1);
            let fd: extern "C" fn(Id, Id, Id, Id) -> Id = std::mem::transmute(msg());
            let opt = fd(k(b"NSDictionary\0"), s(b"dictionaryWithObject:forKey:\0"), ja, ns("AXTrustedCheckOptionPrompt"));
            let r = AXIsProcessTrustedWithOptions(opt);
            objc_autoreleasePoolPop(pool);
            r
        }
    }
    unsafe fn ax_fenster_lesen(pid: i32) -> Vec<[f64; 4]> {
        let mut out = Vec::new();
        let el = AXUIElementCreateApplication(pid);
        if el.is_null() { return out; }
        let mut wins: Id = std::ptr::null_mut();
        if AXUIElementCopyAttributeValue(el, ns("AXWindows"), &mut wins) == 0 && !wins.is_null() {
            for i in 0..CFArrayGetCount(wins) {
                let w = CFArrayGetValueAtIndex(wins, i);
                let mut mini: Id = std::ptr::null_mut();
                let mut minimiert = false;
                if AXUIElementCopyAttributeValue(w, ns("AXMinimized"), &mut mini) == 0 && !mini.is_null() {
                    let fb: extern "C" fn(Id, Id) -> i8 = std::mem::transmute(msg());
                    minimiert = fb(mini, s(b"boolValue\0")) != 0;
                    CFRelease(mini);
                }
                if minimiert { continue; }
                let mut pv: Id = std::ptr::null_mut();
                let mut sv: Id = std::ptr::null_mut();
                let a = AXUIElementCopyAttributeValue(w, ns("AXPosition"), &mut pv);
                let b = AXUIElementCopyAttributeValue(w, ns("AXSize"), &mut sv);
                let mut p = [0f64; 2];
                let mut z = [0f64; 2];
                if a == 0 && b == 0 && !pv.is_null() && !sv.is_null()
                    && AXValueGetValue(pv, 1, p.as_mut_ptr() as *mut c_void)
                    && AXValueGetValue(sv, 2, z.as_mut_ptr() as *mut c_void)
                    && z[0] > 40.0 && z[1] > 40.0 {
                    out.push([p[0], p[1], z[0], z[1]]);
                }
                if !pv.is_null() { CFRelease(pv); }
                if !sv.is_null() { CFRelease(sv); }
            }
            CFRelease(wins);
        }
        CFRelease(el);
        out
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
                if a.is_null() || fpol(a, s(b"activationPolicy\0")) != 0 { continue; }
                let pid = pid_von(a);
                if pid == eigen { continue; }
                let name = text(m0(a, b"localizedName\0")).unwrap_or_default();
                let bundle = text(m0(a, b"bundleIdentifier\0")).unwrap_or_default();
                let pfad = text(m0(m0(a, b"bundleURL\0"), b"path\0")).unwrap_or_default();
                let fen = if ax { ax_fenster_lesen(pid) } else { Vec::new() };
                if ax && fen.is_empty() { continue; }
                if !ax && bundle == "com.apple.finder" { continue; }
                out.push((name, bundle, pfad, fen));
            }
            objc_autoreleasePoolPop(pool);
        }
        out
    }
    pub fn pid_fuer_bundle(b: &str) -> Option<i32> {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let arr = m1(k(b"NSRunningApplication\0"), b"runningApplicationsWithBundleIdentifier:\0", ns(b));
            let r = if anzahl(arr) > 0 { let a = an_stelle(arr, 0); if a.is_null() { None } else { Some(pid_von(a)) } } else { None };
            objc_autoreleasePoolPop(pool);
            r
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
                    if AXUIElementCopyAttributeValue(el, ns("AXWindows"), &mut wins) == 0 && !wins.is_null() {
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
                                let _ = AXUIElementSetAttributeValue(w, ns("AXPosition"), pv);   // nach der Groesse nochmals
                                if a == 0 || b == 0 { gesetzt += 1; }
                                if !pv.is_null() { CFRelease(pv); }
                                if !sv.is_null() { CFRelease(sv); }
                            }
                            fertig = true;
                        }
                        CFRelease(wins);
                    }
                    if fertig || t0.elapsed().as_millis() as u64 > warte_ms { break; }
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
        if bm.is_empty() { return None; }
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let fd: extern "C" fn(Id, Id, *const u8, usize) -> Id = std::mem::transmute(msg());
            let d = fd(k(b"NSData\0"), s(b"dataWithBytes:length:\0"), bm.as_ptr(), bm.len());
            let mut out = None;
            if !d.is_null() {
                let fr: extern "C" fn(Id, Id, Id, u64, Id, *mut i8, *mut Id) -> Id = std::mem::transmute(msg());
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
    pub fn erstellen(_: &str) -> Option<Vec<u8>> { None }
    pub fn apps_fuer_datei(_: &str) -> Vec<(String, String, bool)> { Vec::new() }
    pub fn in_papierkorb(_: &str) -> bool { false }
    pub fn aufloesen(_: &[u8]) -> Option<String> { None }
    pub fn vorne_bundle() -> Option<String> { None }
    pub enum PbInhalt { Text(String, String), Bild(Vec<u8>) }
    pub fn pb_zaehler() -> i64 { 0 }
    pub fn pb_lesen() -> Option<PbInhalt> { None }
    pub fn pb_text_setzen(_: &str) -> bool { false }
    pub fn pb_bild_setzen(_: &str) -> bool { false }
    pub fn ax_vertraut(_: bool) -> bool { false }
    pub fn apps_mit_fenstern(_: bool) -> Vec<(String, String, String, Vec<[f64; 4]>)> { Vec::new() }
    pub fn pid_fuer_bundle(_: &str) -> Option<i32> { None }
    pub fn fenster_setzen(_: i32, _: &[[f64; 4]], _: u64) -> usize { 0 }
}

fn spawn_mouse_watcher(
    app: tauri::AppHandle,
    hit_store: Arc<std::sync::RwLock<NokiHitZustand>>,
    stop: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        let mut last_x = -9999.0;
        let mut last_y = -9999.0;
        let mut ignoring = true;
        while !stop.load(Ordering::Relaxed) {
            if let Some((x, y)) = maus_schirm_position() {
                if (x - last_x).abs() >= 1.0 || (y - last_y).abs() >= 1.0 {
                    last_x = x;
                    last_y = y;
                    let _ = app.emit("noki://maus", serde_json::json!({ "x": x, "y": y }));
                }

                if let Ok(hit) = hit_store.read() {
                    // Waehrend Freeze nimmt das Overlay JEDE Eingabe selbst an:
                    // nichts erreicht die eingefrorenen Fenster darunter.
                    let inside = FREEZE_SPERRE.load(Ordering::Relaxed)
                        || hit.drag
                        || (hit.w > 0
                            && hit.h > 0
                            && x >= (hit.x - 14) as f64
                            && x <= (hit.x + hit.w + 14) as f64
                            && y >= (hit.y - 14) as f64
                            && y <= (hit.y + hit.h + 14) as f64);

                    if inside && ignoring {
                        if let Some(win) = app.get_webview_window(FENSTER) {
                            let _ = win.set_ignore_cursor_events(false);
                            ignoring = false;
                            eprintln!("[RUST] set_ignore_cursor_events(false) — interactive over Noki");
                        }
                    } else if !inside && !ignoring {
                        if let Some(win) = app.get_webview_window(FENSTER) {
                            let _ = win.set_ignore_cursor_events(true);
                            ignoring = true;
                            eprintln!("[RUST] set_ignore_cursor_events(true) — pass-through outside Noki");
                        }
                    }
                }
            }
            thread::sleep(Duration::from_millis(25)); // ~40 Hz
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
    let Some(datei) = einstellungen_datei(app) else {
        return;
    };
    let _ = fs::write(
        datei,
        serde_json::json!({ "alle_schreibtische": an }).to_string(),
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
        let aus: unsafe extern "C" fn(*mut c_void, *const c_void, *mut c_void) = std::mem::transmute(f);
        let vor: unsafe extern "C" fn(*mut c_void, *const c_void) = std::mem::transmute(f);
        let alt = get(nw, sel(b"collectionBehavior\0"));
        let neu = if an { alt | AUX } else { alt & !AUX };
        if neu == alt { return; }
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
    extern "C" {
        fn dlopen(p: *const std::os::raw::c_char, m: i32) -> *mut c_void;
        fn dlsym(h: *mut c_void, s: *const std::os::raw::c_char) -> *mut c_void;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFNumberCreate(a: *const c_void, t: isize, v: *const c_void) -> *const c_void;
        fn CFNumberGetValue(n: *const c_void, t: i32, v: *mut c_void) -> u8;
        fn CFArrayCreate(a: *const c_void, v: *const *const c_void, n: isize, cb: *const c_void) -> *const c_void;
        fn CFArrayGetCount(a: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(a: *const c_void, i: isize) -> *const c_void;
        fn CFRelease(o: *const c_void);
        fn CFDictionaryGetValue(d: *const c_void, k: *const c_void) -> *const c_void;
        fn CFStringCreateWithCString(a: *const c_void, s: *const std::os::raw::c_char, enc: u32) -> *const c_void;
        static kCFTypeArrayCallBacks: c_void;
    }
    /// Alle Spaces in macOS-Reihenfolge (Mission Control), je Bildschirm.
    pub fn space_reihenfolge() -> Option<Vec<u64>> {
        let f: extern "C" fn(i32) -> *const c_void = unsafe { std::mem::transmute(sym(b"CGSCopyManagedDisplaySpaces\0")?) };
        unsafe {
            let d = f(cid()?);
            if d.is_null() { return None; }
            let k_sp = CFStringCreateWithCString(std::ptr::null(), b"Spaces\0".as_ptr() as *const _, 0x0800_0100);
            let k_id = CFStringCreateWithCString(std::ptr::null(), b"ManagedSpaceID\0".as_ptr() as *const _, 0x0800_0100);
            let mut r = vec![];
            for i in 0..CFArrayGetCount(d) {
                let sp = CFDictionaryGetValue(CFArrayGetValueAtIndex(d, i), k_sp);
                if sp.is_null() { continue; }
                for j in 0..CFArrayGetCount(sp) {
                    let n = CFDictionaryGetValue(CFArrayGetValueAtIndex(sp, j), k_id);
                    if n.is_null() { continue; }
                    let mut v: i64 = 0;
                    let _ = CFNumberGetValue(n, 4, &mut v as *mut i64 as *mut c_void);
                    r.push(v as u64);
                }
            }
            CFRelease(k_sp); CFRelease(k_id); CFRelease(d);
            Some(r)
        }
    }
    fn sym(n: &[u8]) -> Option<*mut c_void> {
        unsafe {
            let h = dlopen(b"/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics\0".as_ptr() as *const _, 1);
            if h.is_null() { return None; }
            let p = dlsym(h, n.as_ptr() as *const _);
            if p.is_null() { None } else { Some(p) }
        }
    }
    fn cid() -> Option<i32> {
        let f: extern "C" fn() -> i32 = unsafe { std::mem::transmute(sym(b"CGSMainConnectionID\0")?) };
        Some(f())
    }
    pub fn space_typ(sid: u64) -> Option<i32> {
        let f: extern "C" fn(i32, u64) -> i32 = unsafe { std::mem::transmute(sym(b"CGSSpaceGetType\0")?) };
        Some(f(cid()?, sid))
    }
    /// (aktiver Space, Typ) — Typ 4 = Vollbild, 0 = Schreibtisch.
    pub fn aktiver_space() -> Option<(u64, i32)> {
        let f: extern "C" fn(i32) -> u64 = unsafe { std::mem::transmute(sym(b"CGSGetActiveSpace\0")?) };
        let a = f(cid()?);
        Some((a, space_typ(a)?))
    }
    unsafe fn liste(v: &[i64]) -> *const c_void {
        let n: Vec<*const c_void> = v.iter().map(|x| CFNumberCreate(std::ptr::null(), 4, x as *const i64 as *const c_void)).collect();
        let a = CFArrayCreate(std::ptr::null(), n.as_ptr(), n.len() as isize, &kCFTypeArrayCallBacks as *const c_void);
        for x in n { CFRelease(x); }
        a
    }
    pub fn spaces_des_fensters(wid: i64) -> Option<Vec<u64>> {
        let f: extern "C" fn(i32, i32, *const c_void) -> *const c_void = unsafe { std::mem::transmute(sym(b"CGSCopySpacesForWindows\0")?) };
        unsafe {
            let w = liste(&[wid]);
            let sp = f(cid()?, 7, w);
            CFRelease(w);
            if sp.is_null() { return None; }
            let mut r = vec![];
            for i in 0..CFArrayGetCount(sp) {
                let mut v: i64 = 0;
                let _ = CFNumberGetValue(CFArrayGetValueAtIndex(sp, i), 4, &mut v as *mut i64 as *mut c_void);
                r.push(v as u64);
            }
            CFRelease(sp);
            Some(r)
        }
    }
    fn fenster_spaces(name: &[u8], wid: i64, sids: &[u64]) -> bool {
        let Some(p) = sym(name) else { return false };
        let Some(c) = cid() else { return false };
        let f: extern "C" fn(i32, *const c_void, *const c_void) = unsafe { std::mem::transmute(p) };
        unsafe {
            let w = liste(&[wid]);
            let s = liste(&sids.iter().map(|x| *x as i64).collect::<Vec<_>>());
            f(c, w, s);
            CFRelease(w); CFRelease(s);
        }
        true
    }
    pub fn hinzufuegen(wid: i64, sid: u64) -> bool { fenster_spaces(b"CGSAddWindowsToSpaces\0", wid, &[sid]) }
    pub fn entfernen(wid: i64, sids: &[u64]) -> bool { fenster_spaces(b"CGSRemoveWindowsFromSpaces\0", wid, sids) }
}

#[cfg(target_os = "macos")]
fn vollbild_spaces_verlassen(wid: i64) {
    let Some(sp) = cgs::spaces_des_fensters(wid) else { eprintln!("[VOLLBILD] CGS nicht verfuegbar"); return };
    let voll: Vec<u64> = sp.into_iter().filter(|s| cgs::space_typ(*s) == Some(4)).collect();
    if !voll.is_empty() && !cgs::entfernen(wid, &voll) { eprintln!("[VOLLBILD] Entfernen nicht verfuegbar"); }
}

#[cfg(target_os = "macos")]
fn fenster_nummer(win: &WebviewWindow) -> Option<i64> {
    use std::ffi::c_void;
    #[link(name = "objc")]
    extern "C" { fn sel_registerName(n: *const std::os::raw::c_char) -> *const c_void; fn objc_msgSend(); }
    let nw = win.ns_window().ok()?;
    unsafe {
        let f: unsafe extern "C" fn(*mut c_void, *const c_void) -> isize = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
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
    if NUTZER_VERBORGEN.load(Ordering::Relaxed) || !win.is_visible().unwrap_or(false) { return "verborgen"; }
    let heim = NOKI_SPACE.load(Ordering::Relaxed);
    if heim == 0 { return "unbekannt"; }
    let Some(wid) = fenster_nummer(win) else { return "kein Fenster" };
    let Some(sp) = cgs::spaces_des_fensters(wid) else { return "CGS fehlt" };
    let fremd: Vec<u64> = sp.iter().copied().filter(|s| *s != heim).collect();
    if !fremd.is_empty() { cgs::entfernen(wid, &fremd); }
    "passt"
}

#[cfg(not(target_os = "macos"))]
fn vollbild_space_abgleich(_: &WebviewWindow) -> &'static str { "" }


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
    let Some(win) = app.get_webview_window(FENSTER) else { return false };
    if NUTZER_VERBORGEN.load(Ordering::Relaxed) || !win.is_visible().unwrap_or(false) { return false; }
    #[cfg(target_os = "macos")]
    if let (Some((aktiv, _)), Some(wid)) = (cgs::aktiver_space(), fenster_nummer(&win)) {
        if let Some(sp) = cgs::spaces_des_fensters(wid) { return sp.contains(&aktiv); }
    }
    true
}

fn sicht_menue(app: &tauri::AppHandle) {
    let sichtbar = noki_sichtbar(app);
    let v = if sichtbar { 2 } else { 1 };
    if SICHT_ZULETZT.swap(v, Ordering::Relaxed) != v {
        if let Some(m) = app.try_state::<SichtMenue>() {
            let _ = m.0.set_text(if sichtbar { "Noki verbergen" } else { "Noki zeigen" });
        }
    }
}

/// Klick auf den einen Eintrag. Schreibtisch: Fenster zeigen/verbergen (mit
/// der bisherigen Ausblendbewegung). Vollbild: die Vollbild-Freigabe.
fn sicht_umschalten(app: &tauri::AppHandle) {
    if noki_sichtbar(app) {
        noki_verbergen(app.clone());                        // ueberall: sofort verborgen
    } else {
        // "Noki zeigen": direkt im AKTIVEN Space (Schreibtisch oder Vollbild) —
        // die EINZIGE Stelle, an der er ohne Flug erscheint.
        let versteckt = app.get_webview_window(FENSTER).map_or(true, |w| !w.is_visible().unwrap_or(false));
        #[cfg(target_os = "macos")]
        if let Some(w) = app.get_webview_window(FENSTER) { space_holen(&w); }
        if versteckt { noki_zeigen(app); }
    }
    sicht_menue(app);
}

/// Space-Wechsel des NUTZERS (auf einen Schreibtisch): Nokis Fenster lebt in
/// genau einem Space. Weicht der aktive Schreibtisch davon ab, bereitet die
/// Seite den Eintritt vor (noki://space vorbereiten), erst DANN wird das
/// Fenster verlegt und der SPEED-Eintritt gestartet. Noki wechselt nie autonom.
static SPACE_BEREIT: AtomicBool = AtomicBool::new(false);
static NOKI_WID: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

#[tauri::command]
fn noki_space_bereit() { SPACE_BEREIT.store(true, Ordering::Relaxed); }

fn space_waechter(app: tauri::AppHandle) {
    // JEDER echte Space-Wechsel (Schreibtisch/Vollbild in beliebiger Richtung)
    // wird geflogen: EXIT im alten Space, Umzug, ENTER im neuen. Der Space-Typ
    // allein loest nie ein direktes Erscheinen aus.
    #[cfg(target_os = "macos")]
    thread::spawn(move || loop {
        thread::sleep(std::time::Duration::from_millis(120));
        let Some((sid, typ)) = cgs::aktiver_space() else { continue };
        if NUTZER_VERBORGEN.load(Ordering::Relaxed) || !NOKI_GEZEIGT.load(Ordering::Relaxed) { continue; }
        if !app.state::<Lage>().alle_spaces.load(Ordering::Relaxed) { continue; }   // bleibt, wo er ist
        let mut wid = NOKI_WID.load(Ordering::Relaxed);
        if wid == 0 {
            let (tx, rx) = std::sync::mpsc::channel();
            let h = app.clone();
            let _ = app.run_on_main_thread(move || {
                let _ = tx.send(h.get_webview_window(FENSTER).and_then(|w| fenster_nummer(&w)).unwrap_or(0));
            });
            wid = rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap_or(0);
            if wid == 0 { continue; }
            NOKI_WID.store(wid, Ordering::Relaxed);
        }
        let heim = NOKI_SPACE.load(Ordering::Relaxed);
        if heim == 0 {
            let sp = cgs::spaces_des_fensters(wid).unwrap_or_default();
            NOKI_SPACE.store(if sp.contains(&sid) { sid } else { sp.first().copied().unwrap_or(sid) }, Ordering::Relaxed);
            continue;
        }
        if heim == sid { continue; }
        let dir = cgs::space_reihenfolge()
            .and_then(|v| Some(if v.iter().position(|x| *x == sid)? > v.iter().position(|x| *x == heim)? { 1 } else { -1 }))
            .unwrap_or(1);
        SPACE_BEREIT.store(false, Ordering::Relaxed);
        let _ = app.emit("noki://space", serde_json::json!({ "phase": "vorbereiten", "richtung": dir }));
        for _ in 0..40 {
            if SPACE_BEREIT.load(Ordering::Relaxed) { break; }
            thread::sleep(std::time::Duration::from_millis(10));
        }
        let h = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(w) = h.get_webview_window(FENSTER) { space_umziehen(&w, wid, sid, typ); }
            NOKI_SPACE.store(sid, Ordering::Relaxed);                 // neuer Aufenthaltsort
            let _ = h.emit("noki://space", serde_json::json!({ "phase": "eintreten", "richtung": dir }));
            sicht_menue(&h);
        });
        thread::sleep(std::time::Duration::from_millis(700));      // ein Transfer je Wechsel
    });
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

/// "Noki zeigen" auf einem Schreibtisch: das Fenster DIREKT in den aktiven
/// Schreibtisch holen (kein Transfer, kein Sprung zum alten Space).
#[cfg(target_os = "macos")]
fn space_holen(win: &WebviewWindow) {
    let (Some((sid, typ)), Some(wid)) = (cgs::aktiver_space(), fenster_nummer(win)) else { return };
    let schon = cgs::spaces_des_fensters(wid).map_or(false, |v| v.len() == 1 && v[0] == sid);
    if !schon { space_umziehen(win, wid, sid, typ); }
    NOKI_SPACE.store(sid, Ordering::Relaxed);
}

/// Fenster in GENAU diesen Space verlegen. Vollbild-Flags nur fuer ein
/// Vollbild-Ziel; jede andere Mitgliedschaft wird entfernt (kein Klon).
#[cfg(target_os = "macos")]
fn space_umziehen(win: &WebviewWindow, wid: i64, sid: u64, typ: i32) {
    let voll = typ == 4;
    if voll { vollbild_bit(win, true); }
    cgs::hinzufuegen(wid, sid);
    let andere: Vec<u64> = cgs::spaces_des_fensters(wid).unwrap_or_default().into_iter().filter(|s| *s != sid).collect();
    if !andere.is_empty() { cgs::entfernen(wid, &andere); }
    if !voll { vollbild_bit(win, false); }
    lage_anwenden(win, &win.state::<Lage>());   // Vollbild: vorne; Schreibtisch: Nutzerwahl
}

/// Ab App-Start: Vollbild-Zuordnung abgleichen und Menuetext nachfuehren
/// (Space-/Vollbild-Wechsel, Show/Hide von aussen). Ein Faden, 0,7 s.
fn sicht_waechter(app: tauri::AppHandle) {
    thread::spawn(move || loop {
        thread::sleep(std::time::Duration::from_millis(700));
        let h2 = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(w) = h2.get_webview_window(FENSTER) {
                if NOKI_GEZEIGT.load(Ordering::Relaxed) && !NUTZER_VERBORGEN.load(Ordering::Relaxed)
                    && !w.is_visible().unwrap_or(true) {
                    let _ = w.show();
                    eprintln!("[SICHT] Noki war ohne 'Noki verbergen' unsichtbar - wieder gezeigt");
                }
                let _ = vollbild_space_abgleich(&w);
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
fn lage_anwenden(win: &WebviewWindow, lage: &Lage) {
    // Die NATIVE Ebene folgt nur der Nutzerwahl ("Ansicht") und ist im
    // Vollbild immer vorne. Vor/hinter einem Fenster ist allein Nokis eigene
    // Tiefe im Overlay (Maskierung) — das ganze Fenster wird dafuer NIE mehr
    // nach hinten gelegt (vorher verschwand Noki so hinter jeder App).
    if im_vollbild() { ebene_setzen(win, "vorn"); return; }
    ebene_setzen(win, *lage.ebene.lock().unwrap());
}

fn im_vollbild() -> bool {
    #[cfg(target_os = "macos")]
    { cgs::aktiver_space().map_or(false, |(_, t)| t == 4) }
    #[cfg(not(target_os = "macos"))]
    { false }
}

fn ebene_setzen(win: &WebviewWindow, ebene: &str) {
    match ebene {
        "vorn" => {
            let _ = win.set_always_on_bottom(false);
            let _ = win.set_always_on_top(true);
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
    // work_area() liefert unter macOS die visibleFrame des Bildschirms,
    // bereits in die von oben gezaehlten Koordinaten umgerechnet.
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

    // Fenster deckt den gesamten aktuellen Bildschirm / Desktop-Bereich ab
    let _ = win.set_position(*m_pos);
    let _ = win.set_size(*m_size);
    let _ = win.show();
    NOKI_GEZEIGT.store(true, Ordering::Relaxed);
    let _ = win.set_ignore_cursor_events(true);
    // Abschnitt 29: die gespeicherte Schreibtischwahl gilt ab dem ersten
    // Bild. Sie steht neben der Ebene, nicht darin.
    alle_spaces_anwenden(&win, app.state::<Lage>().alle_spaces.load(Ordering::Relaxed));

    if let Some(pos) = ort_lesen(&app) {
        let log_x = (pos.x as f64 / scale).round() as i32;
        let log_y = (pos.y as f64 / scale).round() as i32;
        eprintln!("[RUST] noki_bereit returning saved pos: ({}, {}), alle Schreibtische: {}",
                  log_x, log_y, app.state::<Lage>().alle_spaces.load(Ordering::Relaxed));
        Some((log_x, log_y))
    } else {
        let def_x = (m_size.width as f64 / scale * 0.5).round() as i32;
        let def_y = (m_size.height as f64 / scale - 14.0).round() as i32;
        eprintln!("[RUST] noki_bereit returning default pos: ({}, {})", def_x, def_y);
        Some((def_x, def_y))
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


#[tauri::command]
fn noki_drag_status(app: tauri::AppHandle, state: tauri::State<NokiHitStore>, aktiv: bool) {
    eprintln!("[RUST] noki_drag_status: aktiv={}", aktiv);
    if let Ok(mut hit) = state.0.write() {
        hit.drag = aktiv;
    }
    if let Some(win) = app.get_webview_window(FENSTER) {
        if aktiv {
            let _ = win.set_ignore_cursor_events(false);
        }
    }
}

#[tauri::command]
fn noki_log(msg: String) {
    eprintln!("[FRONTEND LOG] {}", msg);
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

/// Speichert den aktuellen Noki-Ort dauerhaft.
#[tauri::command]
fn noki_ort_speichern(app: tauri::AppHandle, x: Option<i32>, y: Option<i32>) {
    let Some(datei) = ort_datei(&app) else {
        return;
    };
    if let (Some(x_val), Some(y_val)) = (x, y) {
        let win = app.get_webview_window(FENSTER);
        let scale = win.as_ref()
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
    Some(if im_vollbild() { "vorn".to_string() } else { (*lage.ebene.lock().unwrap()).to_string() })
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
        ort_merken(&app, &win);
        // Ein Durchgang ueberlebt das Verbergen nicht: sonst kaeme Noki
        // hinter allen Fenstern zurueck (Abschnitt 21).
        let lage = app.state::<Lage>();
        lage.durchgang.store(false, Ordering::Relaxed);
        lage_anwenden(&win, &lage);
        NUTZER_VERBORGEN.store(true, Ordering::Relaxed);
        let _ = win.hide();
    }
    sicht_menue(&app);
}

/// Ziehen neben der Figur verschiebt das ganze Panel ueber den Schreibtisch.
/// Das Fenster hat keine Titelleiste — ohne das liesse es sich nicht bewegen.
#[tauri::command]
fn noki_fenster_ziehen(app: tauri::AppHandle) {
    if FREEZE_SPERRE.load(Ordering::Relaxed) { return; }   // eingefroren bleibt alles, wo es ist
    if let Some(win) = app.get_webview_window(FENSTER) {
        let _ = win.start_dragging();
    }
}

fn noki_zeigen(app: &tauri::AppHandle) {
    let Some(win) = app.get_webview_window(FENSTER) else {
        return;
    };
    if let Some(p) = ort_lesen(app) {
        if let Some(k) = ort_klemmen(&win, p) {
            let _ = win.set_position(k);
        }
    }
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
    NUTZER_VERBORGEN.store(false, Ordering::Relaxed);
    #[cfg(target_os = "macos")]
    space_holen(&win);
    let _ = win.show();
    NOKI_GEZEIGT.store(true, Ordering::Relaxed);
    // Die Spawn-Blende gehoert dem Frontend (Abschnitt 16) — hier wird nur
    // gemeldet, dass es sie spielen soll.
    let _ = app.emit("noki://zeigen", serde_json::json!({}));
    sicht_menue(app);
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
    let kreis = |x: f32, y: f32, cx: f32, cy: f32, r: f32| {
        ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r
    };

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
        .manage(Lage {
            ebene: Mutex::new("vorn"),
            durchgang: AtomicBool::new(false),
            // Der echte Wert kommt in setup() aus der Einstellungsdatei.
            alle_spaces: AtomicBool::new(true),
        })
        .manage(NokiHitStore(hit_store))
        .invoke_handler(tauri::generate_handler![
            noki_bereit,
            noki_alle_schreibtische,
            noki_verbergen,
            noki_fenster_ziehen,
            noki_fenster_setzen,
            noki_fenster_bewegen,
            noki_fenster_position_holen,
            noki_schirm_info,
            noki_ort_speichern,
            noki_maus_position,
            noki_hit_zone,
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
            ablage_liste,
            ablage_oeffnen,
            ablage_finder,
            ablage_entfernen,
            ablage_leeren,
            ablage_ziehen,
            noki_schnell,
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
            werk_daten_lesen,
            werk_daten_schreiben,
            noki_utility_vorn,
            noki_utility_zurueck,
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
            noki_datei_apps,
            noki_datei_oeffnen_mit,
            noki_ordner_finder,
            noki_dateien_waehlen,
            datei_oeffnen,
            datei_finder,
            datei_bearbeiten,
            kamera_ordner_oeffnen,
            einstellungen_oeffnen,
            noki_ebene_umschalten,
        ])
        .setup(move |app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            // Kein Dock-Symbol, kein Programm-Menue: JARVIS ist auf dem
            // Schreibtisch praesent, nicht in der Programmleiste. Der Weg
            // zur Bedienung ist die Menueleiste (Abschnitt 16).
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
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
            // Abschnitt 29: Nokis obere Leiste ist unter macOS die
            // Menueleiste — dort liegen schon Zeigen, Verbergen und die
            // Ebenen. Die Schreibtischwahl gehoert genau dorthin und
            // braucht keine eigene Einstellungsseite.
            let alle_an = alle_spaces_lesen(app.handle());
            app.state::<Lage>().alle_spaces.store(alle_an, Ordering::Relaxed);
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
            let energie_stufen: [(&str, &str); 4] = [
                ("still", "Still"),
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
            let ein_oeffnen = MenuItem::with_id(app, "einstellungen_oeffnen", "Einstellungen öffnen …", true, None::<&str>)?;
            let einstellungen = Submenu::with_id_and_items(
                app, "einstellungen", "Noki Einstellungen", true, &[&ein_oeffnen, &aktionen, &energie, &groesse])?;
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
                if ablage_start.is_empty() { "Ablage".to_string() } else { format!("Ablage ({})", ablage_start.len()) },
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
            let w_platz = MenuItem::with_id(app, "werk_platz", "Arbeitsplatz", true, None::<&str>)?;
            let w_fokus = MenuItem::with_id(app, "werk_fokus", "Fokusmodus", true, None::<&str>)?;
            let arbeit = Submenu::with_id_and_items(app, "arbeit", "Arbeit", true,
                &[&w_shortcuts, &fz, &m_freeze, &kamera, &ablage_item, &w_clip, &w_timer, &w_platz, &w_fokus])?;
            let menu = Menu::with_items(
                app,
                &[
                    &einstellungen,
                    &ansicht,
                    &arbeit,
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

            TrayIconBuilder::with_id("jarvis")
                .icon(tray_symbol())
                .icon_as_template(true)
                .tooltip("JARVIS")
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
                    "werk_fokus" => werkzeug_zeigen(app, "fokus"),
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
            if let Some(tray) = app.tray_by_id("jarvis") {
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
                        log::warn!("[JARVIS] Frontend hat sich nicht gemeldet — Fenster wird trotzdem gezeigt");
                        let _ = win.show();
                    }
                });
            }

            spawn_state_watcher(app.handle().clone(), Arc::clone(&watcher_stop));
            spawn_voice_watcher(app.handle().clone(), Arc::clone(&watcher_stop));
            spawn_heartbeat_watcher(app.handle().clone(), Arc::clone(&watcher_stop));
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
            #[cfg(debug_assertions)]
            if let Ok(plan) = std::env::var("NOKI_TEST_TASTE") {
                let h = app.handle().clone();
                thread::spawn(move || {
                    let mut t0 = 0u64;
                    for teil in plan.split(',').filter(|s| !s.is_empty()) {
                        let mut it = teil.split('@');
                        let was = it.next().unwrap_or("");
                        let t: u64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(t0);
                        thread::sleep(Duration::from_secs(t.saturating_sub(t0)));
                        t0 = t;
                        // "m" = Menuepunkt "Ablage" (derselbe Weg wie der Klick oben im Menue).
                        if was == "m" {
                            eprintln!("[TEST] Menue Ablage bei {} s", t);
                            ablage_melden(&h, &[], None, Some("umschalten"));
                            continue;
                        }
                        if was == "settings" { einstellungen_oeffnen(h.clone()); continue; }
                        if was == "shortcuts" { werkzeug_zeigen(&h, "shortcuts"); continue; }
                        let n: u32 = was.parse().unwrap_or(0);
                        eprintln!("[TEST] Taste Ctrl+{} bei {} s", n, t);
                        shortcut_ausloesen(&h, n);
                    }
                });
            }

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(move |app, event| match event {
            tauri::RunEvent::Exit => {
                stop.store(true, Ordering::Relaxed);
                // Laufende Bildschirmaufnahme sauber abschliessen.
                if let Some(a) = app.try_state::<Aufnahme>() { let _ = aufnahme_stoppen(&a); }
            }
            // Ohne Dock-Symbol und ohne Titelleiste gibt es keinen Weg, das
            // Fenster zu schliessen — ein Schliessversuch von aussen soll
            // die App trotzdem nicht beenden, sondern Noki nur verbergen.
            tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } if label == FENSTER => {
                api.prevent_close();
                if let Some(win) = app.get_webview_window(FENSTER) {
                    ort_merken(app, &win);
                    NUTZER_VERBORGEN.store(true, Ordering::Relaxed);   // Schliessen = Nutzerwunsch
                    let _ = win.hide();
                }
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
        assert_eq!((l[0].typ.as_str(), l[1].typ.as_str(), l[2].typ.as_str()), ("ordner", "bild", "dokument"));
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
        for e in l2.iter_mut() { ablage_aufloesen(e); }
        let ea = l2.iter().find(|e| e.ext == "pdf").unwrap();
        assert!(ea.da && ea.pfad.ends_with("a-umbenannt.pdf"), "Lesezeichen folgt nicht: {}", ea.pfad);

        // Geloescht: kein Absturz, Eintrag bleibt als "nicht verfuegbar".
        fs::remove_file(&b).unwrap();
        for e in l2.iter_mut() { ablage_aufloesen(e); }
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
        let p = schreibtisch_datei("Noki-Screenshot", "png").unwrap();
        assert_eq!(p.parent().and_then(|d| d.file_name()).unwrap(), "Noki Kamera");
        assert!(p.parent().unwrap().is_dir());
        let n = p.file_name().unwrap().to_string_lossy().into_owned();
        assert!(n.starts_with("Noki-Screenshot-") && n.ends_with(".png") && n.len() == "Noki-Screenshot-2026-09-14-120000.png".len(), "{n}");
    }

    #[test]
    fn zwischenablage_regeln() {
        let t = |id: u64, s: &str| ClipEintrag { id, art: if clip_ist_link(s) { "link".into() } else { "text".into() },
                                                 text: s.into(), ..Default::default() };
        let mut l = Vec::new();
        assert!(clip_einordnen(&mut l, t(1, "hallo")).0);
        assert!(!clip_einordnen(&mut l, t(2, "hallo")).0, "unmittelbar gleicher Inhalt doppelt");
        assert!(clip_einordnen(&mut l, t(3, "https://example.org/x")).0);
        assert_eq!(l[0].art, "link");
        assert!(!clip_ist_link("https://a b") && !clip_ist_link("hallo welt"));
        for i in 0..30 { clip_einordnen(&mut l, t(10 + i, &format!("e{i}"))); }
        assert_eq!(l.len(), CLIP_MAX);
        assert_eq!(b64(b"Noki"), "Tm9raQ==");
        assert_eq!(b64(b"ab"), "YWI=");
    }

    #[test]
    fn fokus_apps_gefunden() {
        let a = fokus_apps();
        assert!(a.len() > 5, "nur {} Apps", a.len());
        assert!(a.iter().all(|x| x["pfad"].as_str().unwrap_or("").ends_with(".app")));
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
        assert_eq!(ort_in_sicht((500, 500), FENSTER_GR, &[winzig]), Some((0, 0)));
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
}
