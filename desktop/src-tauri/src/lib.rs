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

/// Das Untermenue "Fenster schliessen" (vom Fenster-Beobachter gefuellt).
struct SchliessMenue(tauri::menu::Submenu<tauri::Wry>);

fn schliess_menue_fuellen(app: &tauri::AppHandle, m: &tauri::menu::Submenu<tauri::Wry>, liste: &[(i64, String)]) {
    if let Ok(alt) = m.items() {
        for it in alt.iter() { let _ = m.remove(it); }
    }
    if liste.is_empty() {
        #[cfg(target_os = "macos")]
        let hinweis = if ax::vertraut() { "Kein schließbares Fenster" } else { "Bedienungshilfen-Freigabe fehlt" };
        #[cfg(not(target_os = "macos"))]
        let hinweis = "Kein schließbares Fenster";
        if let Ok(i) = MenuItem::with_id(app, "fz_keins", hinweis, false, None::<&str>) { let _ = m.append(&i); }
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
    let dir = std::path::PathBuf::from(home).join("Desktop");
    let probe = dir.join(".noki-schreibtest");
    std::fs::write(&probe, b"").map_err(|e| format!("pfad: Schreibtisch nicht beschreibbar ({e})"))?;
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
    match n {
        1 => { let _ = app.emit("noki://kamera", serde_json::json!({ "was": "screenshot" })); }
        2 => { let _ = app.emit("noki://kamera", serde_json::json!({ "was": "recording" })); }
        3 => { let _ = app.emit("noki://freeze", serde_json::json!({})); }
        4 => {
            #[cfg(target_os = "macos")]
            regler::fenster_auswahl(app);
        }
        _ => {}
    }
}

/// Belegt macOS "Zu Schreibtisch N wechseln" (Standard Ctrl+N) die Taste?
#[cfg(target_os = "macos")]
fn systemkuerzel_belegt(n: u32) -> bool {
    let home = std::env::var("HOME").unwrap_or_default();
    std::process::Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", &format!("Print :AppleSymbolicHotKeys:{}:enabled", 117 + n)])
        .arg(format!("{}/Library/Preferences/com.apple.symbolichotkeys.plist", home))
        .output().map(|o| String::from_utf8_lossy(&o.stdout).trim() == "true").unwrap_or(false)
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
    }
    static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
    static ZULETZT: Mutex<[Option<std::time::Instant>; 5]> = Mutex::new([None; 5]);
    const fn vz(s: &[u8; 4]) -> u32 { ((s[0] as u32) << 24) | ((s[1] as u32) << 16) | ((s[2] as u32) << 8) | (s[3] as u32) }
    extern "C" fn gedrueckt(_c: *mut c_void, ev: *mut c_void, _u: *mut c_void) -> i32 {
        let mut hk = HotKeyId { signatur: 0, id: 0 };
        let st = unsafe { GetEventParameter(ev, vz(b"----"), vz(b"hkid"), std::ptr::null_mut(),
            std::mem::size_of::<HotKeyId>(), std::ptr::null_mut(), &mut hk as *mut HotKeyId as *mut c_void) };
        if st != 0 || hk.signatur != vz(b"NOKI") || hk.id == 0 || hk.id > 4 { return -9874; }   // nicht unseres
        // Genau EINMAL je Tastendruck: Tastenwiederholung/Prellen abfangen.
        if let Ok(mut z) = ZULETZT.lock() {
            let jetzt = std::time::Instant::now();
            if z[hk.id as usize].map_or(false, |t| jetzt.duration_since(t).as_millis() < 400) { return 0; }
            z[hk.id as usize] = Some(jetzt);
        }
        if let Some(app) = APP.get() { super::shortcut_ausloesen(app, hk.id); }
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
            for (n, code) in [(1u32, 18u32), (2, 19), (3, 20), (4, 21)] {
                if super::systemkuerzel_belegt(n) {
                    eprintln!("[SHORTCUT] Shortcut Control+{} ist bereits belegt (macOS: Zu Schreibtisch {} wechseln) - nicht registriert", n, n);
                    continue;
                }
                let mut r = std::ptr::null_mut();
                let st = RegisterEventHotKey(code, 0x1000, HotKeyId { signatur: vz(b"NOKI"), id: n }, ziel, 0, &mut r);   // controlKey
                if st != 0 { eprintln!("[SHORTCUT] Shortcut Control+{} ist bereits belegt (Fehler {})", n, st); }
                else { eprintln!("[SHORTCUT] Control+{} registriert", n); }
            }
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
    pub fn fenster_auswahl(app: &tauri::AppHandle) {
        let _ = APP.set(app.clone());
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
                    objc_registerClassPair(kl);
                }
                ziel = m0(m0(kl, "alloc"), "init");
                FZ_ZIEL.store(ziel as usize, Ordering::Relaxed);
            }
            let menue = m1(m0(klasse("NSMenu"), "alloc"), "initWithTitle:", text("Fenster schließen"));
            mi(menue, "setAutoenablesItems:", 0);
            let neu: unsafe extern "C" fn(Id, Sel, Id, Sel, Id) -> Id = std::mem::transmute(f());
            if liste.is_empty() {
                let hinweis = if super::ax::vertraut() { "Kein schließbares Fenster" } else { "Bedienungshilfen-Freigabe fehlt" };
                let e = neu(m0(klasse("NSMenuItem"), "alloc"), sel("initWithTitle:action:keyEquivalent:"), text(hinweis), std::ptr::null(), text(""));
                mi(e, "setEnabled:", 0);
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
        let Some((k, r, _)) = ax::knopf(id) else { return false; };
        let da = ((r.0 + r.2 / 2.0) - x).abs() <= 8.0 && ((r.1 + r.3 / 2.0) - y).abs() <= 8.0;
        let ok = da && ax::druecken(k);
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
                if menue.as_ref() != Some(&eintraege) {
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
            let einstellungen = Submenu::with_id_and_items(
                app, "einstellungen", "Noki Einstellungen", true, &[&aktionen, &energie, &groesse])?;
            // Ansicht: EIN Ebenen-Eintrag, der die NAECHSTE Aktion zeigt (Abschnitt 14:
            // "hinten" = unter gewoehnlichen Fenstern), dazu die Schreibtischwahl.
            let ebene_jetzt: &'static str = *app.state::<Lage>().ebene.lock().unwrap();
            let ebene_item = MenuItem::with_id(app, "ebene_wechsel",
                if ebene_jetzt == "hinten" { "In den Vordergrund" } else { "In den Hintergrund" }, true, None::<&str>)?;
            let ansicht = Submenu::with_id_and_items(app, "ansicht", "Ansicht", true, &[&ebene_item, &spaces])?;
            let trenner = PredefinedMenuItem::separator(app)?;
            let beenden = MenuItem::with_id(app, "beenden", "Noki beenden", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &einstellungen,
                    &ansicht,
                    &fz,
                    &m_freeze,
                    &kamera,
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
            let spaces_item = spaces.clone();
            let ebene_item_h = ebene_item.clone();

            TrayIconBuilder::with_id("jarvis")
                .icon(tray_symbol())
                .icon_as_template(true)
                .tooltip("JARVIS")
                .menu(&menu)
                .on_menu_event(move |app, event| match event.id().as_ref() {
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
            _ => {}
        });
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
