//! Tastatur fuer ein Eingabefeld auf Noki Schreibtisch.
//!
//! Beginnt NUR ausdruecklich: der Nutzer klickt in der Miniatur (oder der
//! Vollansicht) auf ein Feld, und Chrome bestaetigt per Bedienungshilfen,
//! dass genau dieses Feld in genau diesem Noki-Fenster den Fokus hat.
//!
//! Solange es laeuft, nimmt Nokis bestehender Tasten-Abgriff jede physische
//! Taste an sich (sie erreicht KEIN lokales Programm) und reicht eine Kopie
//! ausschliesslich an den Zielprozess weiter - aber nur nach einer frischen
//! Pruefung, dass der Fokus noch immer genau dieses Feld in genau diesem
//! Fenster ist. Chrome ist ein Prozess fuer alle Fenster: ohne diese
//! Pruefung landeten die Zeichen im Chrome-Fenster des Nutzers (gemessen:
//! Chromes Fokusfenster war vorher ein Fenster auf dem Nutzer-Schreibtisch).
//! Schlaegt sie fehl, endet der Modus und die Taste verfaellt (fail closed).
//!
//! Getippte Zeichen werden nie protokolliert - nur Anzahl und Gruende.

use crate::vorschau::fernbedienung::{self, Feld};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// Markiert weitergereichte Kopien (kCGEventSourceUserData), damit Nokis
/// eigener Abgriff sie nie ein zweites Mal verarbeitet.
pub const MARKE: i64 = 0x4E4F_4B49_5450; // "NOKITP"

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventCreateCopy(ev: *mut c_void) -> *mut c_void;
    #[cfg(debug_assertions)]
    fn CGEventCreateKeyboardEvent(src: *const c_void, key: u16, down: bool) -> *mut c_void;
    #[cfg(debug_assertions)]
    fn CGEventSetFlags(ev: *mut c_void, flags: u64);
    fn CGEventGetIntegerValueField(ev: *mut c_void, feld: u32) -> i64;
    fn CGEventGetFlags(ev: *mut c_void) -> u64;
    fn CGEventKeyboardGetUnicodeString(
        ev: *mut c_void, max: usize, laenge: *mut usize, text: *mut u16,
    );
}

/// Acceptance-only injection at the input-router boundary. This constructs
/// the same keyDown/keyUp objects the event tap hands to `taste`; delivery,
/// exact-focus verification, modifier preservation, ownership transactions,
/// and the generated-event marker all remain production code paths.
#[cfg(debug_assertions)]
pub fn debug_taste(code: u16, flags: u64) -> bool {
    unsafe {
        let mut handled = true;
        for (kind, down) in [(10, true), (11, false)] {
            let ev = CGEventCreateKeyboardEvent(std::ptr::null(), code, down);
            if ev.is_null() { handled = false; continue; }
            CGEventSetFlags(ev, flags);
            handled &= taste(kind, ev);
            CFRelease(ev as *const c_void);
        }
        handled
    }
}
/// Acceptance-only: text as real keyDown/keyUp objects (unicode string set)
/// handed to `taste` - the same router boundary as the event tap, so a test
/// can never type into whatever app happens to be frontmost.
#[cfg(debug_assertions)]
pub fn debug_text(text: &str) -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventKeyboardSetUnicodeString(ev: *mut c_void, n: usize, s: *const u16);
    }
    let mut handled = true;
    for ch in text.chars() {
        let (code, t): (u16, Vec<u16>) = match ch {
            '\n' => (36, vec![13]),
            '\t' => (48, vec![9]),
            c => (0x31, c.encode_utf16(&mut [0u16; 2]).to_vec()),
        };
        unsafe {
            for (kind, down) in [(10, true), (11, false)] {
                let ev = CGEventCreateKeyboardEvent(std::ptr::null(), code, down);
                if ev.is_null() { handled = false; continue; }
                CGEventSetFlags(ev, 0);
                if code == 0x31 { CGEventKeyboardSetUnicodeString(ev, t.len(), t.as_ptr()); }
                handled &= taste(kind, ev);
                CFRelease(ev as *const c_void);
            }
        }
        std::thread::sleep(Duration::from_millis(8));
    }
    handled
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(o: *const c_void);
}

struct Ziel {
    pid: i32,
    wid: i64,
    feld: Arc<Feld>,
    /// Chromes Fokusfenster vor dem Klick (oft eines des Nutzers).
    vorher: Option<Feld>,
}

enum Nach {
    Nichts,
    /// Tab/Enter koennen den Fokus bewegen: danach neu bestimmen.
    FokusPruefen,
}

#[derive(Clone, Copy, PartialEq)]
enum Aktion { Taste, Kopieren, Ausschneiden, Einsetzen, AllesMarkieren, Signal(i32), FensterZu }

struct Auftrag {
    aktion: Aktion,
    kopie: usize,
    runter: bool,
    generation: u64,
    nach: Nach,
    /// Zeitpunkt, an dem Noki die Taste abgegriffen hat (Latenzmessung).
    seit: std::time::Instant,
}

/// Abgriff -> Zustellung je Taste (ms) der laufenden Sitzung.
static LATENZ: std::sync::Mutex<Vec<f64>> = std::sync::Mutex::new(Vec::new());
unsafe impl Send for Auftrag {}

static AKTIV: AtomicBool = AtomicBool::new(false);
static INTERACTION: AtomicBool = AtomicBool::new(false);
static GENERATION: AtomicU64 = AtomicU64::new(0);
static ZIEL: Mutex<Option<Ziel>> = Mutex::new(None);
/// Tasten, deren Druck weitergereicht wurde: nur deren Loslassen folgt.
static UNTEN: Mutex<Vec<i64>> = Mutex::new(Vec::new());
static ANZAHL: AtomicUsize = AtomicUsize::new(0);
static KANAL: OnceLock<Sender<Auftrag>> = OnceLock::new();
static LETZTER_HEARTBEAT_MS: AtomicU64 = AtomicU64::new(0);
/// Zwischen dem Klick in die Noki-Ansicht und dem Nachweis des Feldfokus
/// vergehen 50-300 ms. Tasten in dieser Zeit gehoeren weder dem lokalen
/// Programm noch schon dem Feld: sie werden gehalten und danach an das Feld
/// gegeben - oder verworfen, wenn kein Feld getroffen wurde. Nie lokal.
static WARTEN_BIS: Mutex<Option<std::time::Instant>> = Mutex::new(None);
static PUFFER: Mutex<Vec<(usize, bool, Nach)>> = Mutex::new(Vec::new());
const WARTEN_MS: u64 = 1200;
static WAECHTER: OnceLock<()> = OnceLock::new();

pub fn aktiv() -> bool {
    AKTIV.load(Ordering::SeqCst)
}

pub fn interaction_aktiv() -> bool {
    INTERACTION.load(Ordering::SeqCst)
}

pub fn letzter_heartbeat_ms() -> u64 {
    LETZTER_HEARTBEAT_MS.load(Ordering::Relaxed)
}

fn heartbeat() {
    LETZTER_HEARTBEAT_MS.store(
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64).unwrap_or(0),
        Ordering::Relaxed,
    );
}

/// Prozess, in den gerade getippt wird (falls der Modus laeuft).
pub fn ziel_pid() -> Option<i32> {
    if !aktiv() { return None; }
    ziel().map(|z| z.0)
}

/// Window of the running typing session, if any.
pub fn ziel_wid() -> Option<i64> {
    if !aktiv() { return None; }
    ziel().map(|z| z.1)
}

#[cfg(debug_assertions)]
pub fn debug_ziel_wid() -> Option<i64> {
    if !aktiv() { return None; }
    ziel().map(|z| z.1)
}

#[cfg(debug_assertions)]
pub fn debug_generation() -> u64 {
    GENERATION.load(Ordering::SeqCst)
}

/// Physischer Druck IN der Noki-Ansicht (aus Nokis Tasten-Abgriff).
pub fn klick_in_ansicht() {
    let war = INTERACTION.swap(true, Ordering::SeqCst);
    crate::vorschau::interaction_anzeige(true);
    if !war {
        crate::virtual_workspace::trace(
            "[INTERACTION] enter origin=explicit_workspace_click keyboard=isolated",
        );
    }
    if let Ok(mut w) = WARTEN_BIS.lock() {
        *w = Some(std::time::Instant::now() + Duration::from_millis(WARTEN_MS));
    }
}

pub fn interaction_beenden(grund: &str) {
    let war = INTERACTION.swap(false, Ordering::SeqCst);
    beenden(grund);
    warten_beenden(grund);
    auswahl_verwerfen(grund);
    crate::vorschau::interaction_anzeige(false);
    if war {
        crate::virtual_workspace::trace(&format!("[INTERACTION] exit reason={grund}"));
    }
}

/// Haelt der Abgriff gerade Tasten (unbestaetigter Klick oder eine von
/// Noki gehaltene Seitentext-Auswahl)?
pub fn haelt() -> bool {
    wartet() || auswahl_aktiv()
}

// ---------------------------------------------------------------------
//  Gehaltene Auswahl auf gewoehnlichem Seitentext
// ---------------------------------------------------------------------
//  Chrome nimmt fuer nicht editierbaren Seitentext keine Hintergrund-Auswahl
//  an. Die Auswahl stammt deshalb aus Chromes echten Zeichen (Text und
//  Rahmen), die Markierung zeichnet Noki. Solange sie steht, gehoeren Tasten
//  Noki: Cmd+C kopiert genau diesen Text, alles andere bleibt WIRKUNGSLOS -
//  nie lokal (ein Backspace darf nichts beim Nutzer loeschen), nie destruktiv.

static AUSWAHL: Mutex<Option<(i32, i64, String)>> = Mutex::new(None);

fn auswahl_aktiv() -> bool {
    AUSWAHL.lock().map(|g| g.is_some()).unwrap_or(false)
}

pub fn auswahl_halten(pid: i32, wid: i64, text: String) {
    beenden("page_selection");
    let n = text.chars().count();
    if let Ok(mut g) = AUSWAHL.lock() { *g = Some((pid, wid, text)); }
    crate::vorschau::tippen_anzeige(true);
    crate::virtual_workspace::trace(&format!("[AUSWAHL] page_text held chars={n} wid={wid}"));
    waechter();
}

pub fn auswahl_verwerfen(grund: &str) {
    let alt = AUSWAHL.lock().ok().and_then(|mut g| g.take());
    if alt.is_some() {
        crate::vorschau::markierung(&[]);
        if !aktiv() { crate::vorschau::tippen_anzeige(false); }
        crate::virtual_workspace::trace(&format!("[AUSWAHL] page_text cleared reason={grund}"));
    }
}

fn auswahl_taste(kind: u32, ev: *mut c_void) -> bool {
    let code = unsafe { CGEventGetIntegerValueField(ev, 9) };
    let flags = unsafe { CGEventGetFlags(ev) };
    if kind == 11 {
        return UNTEN.lock().map(|mut u| {
            if let Some(i) = u.iter().position(|c| *c == code) { u.remove(i); true } else { false }
        }).unwrap_or(false);
    }
    if flags & CMD != 0 && (code == 48 || code == 49) {
        auswahl_verwerfen("system_shortcut");
        return false;
    }
    if let Ok(mut u) = UNTEN.lock() { if !u.contains(&code) { u.push(code); } }
    if code == 53 {
        auswahl_verwerfen("escape");
        return true;
    }
    if flags & CMD != 0 && zeichen(ev).to_lowercase() == "c" {
        let text = AUSWAHL.lock().ok().and_then(|g| g.as_ref().map(|a| a.2.clone())).unwrap_or_default();
        let n = text.chars().count();
        std::thread::spawn(move || {
            let ok = crate::lesezeichen::pb_text_setzen(&text);
            crate::vorschau::hinweis(if ok { "Kopiert" } else { "Kopieren fehlgeschlagen" });
            crate::virtual_workspace::trace(&format!("[AUSWAHL] copy chars={n} ok={ok}"));
        });
    }
    true
}

fn wartet() -> bool {
    let offen = WARTEN_BIS.lock().ok().and_then(|w| *w)
        .is_some_and(|t| std::time::Instant::now() < t);
    if !offen { puffer_verwerfen(None); }
    offen
}

/// Der Klick hat kein Eingabefeld getroffen: gehaltene Tasten verfallen.
pub fn warten_beenden(grund: &str) {
    puffer_verwerfen(Some(grund));
}

fn puffer_verwerfen(grund: Option<&str>) {
    if let Ok(mut w) = WARTEN_BIS.lock() { *w = None; }
    let alt: Vec<_> = PUFFER.lock().map(|mut p| p.drain(..).collect()).unwrap_or_default();
    if !alt.is_empty() {
        crate::virtual_workspace::trace(&format!(
            "[TIPPEN] held_keys_dropped={} reason={}", alt.len(), grund.unwrap_or("timeout")
        ));
    }
    for (kopie, ..) in alt { unsafe { CFRelease(kopie as *const c_void) }; }
}

fn ziel() -> Option<(i32, i64, Arc<Feld>)> {
    ZIEL.lock().ok().and_then(|g| g.as_ref().map(|z| (z.pid, z.wid, z.feld.clone())))
}

/// Ausdruecklicher Beginn: nur aus der Fernbedienung, nachdem der Fokus des
/// Feldes nachgewiesen ist.
pub fn beginnen(pid: i32, wid: i64, feld: Feld, vorher: Option<Feld>) {
    ask_beenden("other_session");
    // VS Code: the verified field decides editor vs. integrated terminal;
    // the keys then go through this window's Companion bridge.
    if crate::vscode_bruecke::ist_vscode(pid) {
        match crate::vscode_bruecke::fuer_fenster(wid) {
            Some(b) => {
                // The point decides (see vscode_art_vorgeben); the field's
                // own context only when the point said nothing.
                let am_punkt = VS_ART_VORGABE.lock().ok().and_then(|mut g| g.take())
                    .filter(|(w, _, t)| *w == wid && t.elapsed() < Duration::from_secs(2))
                    .and_then(|(_, a, _)| a);
                let terminal = am_punkt.unwrap_or_else(||
                    crate::vscode_bruecke::ist_terminal_kontext(&fernbedienung::feld_kontext(&feld)));
                let art = if terminal { crate::vscode_bruecke::Art::Terminal } else { crate::vscode_bruecke::Art::Editor };
                beginnen_vscode(pid, wid, art, b);
                return;
            }
            None => {
                crate::vorschau::hinweis("VS Code nimmt Tasten auf einem anderen Schreibtisch nur über „Noki Companion“ an (desktop/vscode-bridge/installieren.sh).");
                crate::virtual_workspace::trace(&format!("[VSCODE_BRIDGE] none_for wid={wid} fallback=pid_events"));
            }
        }
    }
    vscode_beenden("other_session");
    GENERATION.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut g) = ZIEL.lock() {
        // Ein erneuter Klick in ein Feld behaelt das URSPRUENGLICHE
        // Fokusfenster - sonst gaebe das Ende Noki ein Fenster "zurueck".
        let vorher = match g.take() {
            Some(alt) if alt.pid == pid && alt.vorher.is_some() => alt.vorher,
            _ => vorher,
        };
        *g = Some(Ziel { pid, wid, feld: Arc::new(feld), vorher });
    }
    if let Ok(mut u) = UNTEN.lock() { u.clear(); }
    ANZAHL.store(0, Ordering::SeqCst);
    let war = AKTIV.swap(true, Ordering::SeqCst);
    // Gehaltene Tasten gehen jetzt, in Reihenfolge, an das Feld.
    if let Ok(mut w) = WARTEN_BIS.lock() { *w = None; }
    let gehalten: Vec<_> = PUFFER.lock().map(|mut p| p.drain(..).collect()).unwrap_or_default();
    let generation = GENERATION.load(Ordering::SeqCst);
    arbeiter();
    for (kopie, runter, nach) in gehalten {
        einreihen(Auftrag { aktion: Aktion::Taste, kopie, runter, generation, nach, seit: std::time::Instant::now() });
    }
    crate::vorschau::tippen_anzeige(true);
    // Show WHERE keys go: the app may not draw its caret while inactive.
    // Terminal: no outline - its own caret language. Inactive, Terminal
    // draws only a hollow cell; the white block follows the REAL insertion
    // point (AX), never the pointer.
    let terminal = crate::lesezeichen::app_fuer_pid(pid).is_some_and(|a| a.1 == "com.apple.Terminal");
    let ring = if terminal { None }
        else { ZIEL.lock().ok().and_then(|g| g.as_ref().and_then(|z| fernbedienung::feld_rahmen(&z.feld))) };
    crate::vorschau::fokusring(ring);
    if terminal { einfuegemarke_folgen(pid, GENERATION.load(Ordering::SeqCst)); }
    crate::virtual_workspace::trace(&format!(
        "[TIPPEN] start wid={wid} pid={pid} renewed={war} capture=explicit_editable_click focus_ring={} terminal_caret={terminal}", ring.is_some()
    ));
    arbeiter();
    waechter();
}

/// While THIS typing session targets Terminal: the caret block tracks the
/// real insertion point (typing, shell output). Bounded to the session - one
/// AX read per 90 ms, sent only on change; ends with the session.
fn einfuegemarke_folgen(pid: i32, generation: u64) {
    std::thread::spawn(move || {
        let mut zuletzt: Option<[f64; 4]> = None;
        let mut erst = true;
        loop {
            if !AKTIV.load(Ordering::SeqCst) || GENERATION.load(Ordering::SeqCst) != generation { break; }
            let feld = ZIEL.lock().ok().and_then(|g| g.as_ref().filter(|z| z.pid == pid).map(|z| z.feld.clone()));
            let Some(feld) = feld else { break };
            let r = fernbedienung::einfuege_rahmen(&feld);
            if erst || r != zuletzt {
                crate::vorschau::einfuegemarke(r);
                if erst {
                    crate::virtual_workspace::trace(&format!("[TIPPEN] terminal_caret shown={} rect={r:?} source=ax_insertion_point", r.is_some()));
                }
                zuletzt = r; erst = false;
            }
            std::thread::sleep(std::time::Duration::from_millis(90));
        }
        // A newer session of the same Terminal keeps its own block.
        let weiter = AKTIV.load(Ordering::SeqCst)
            && ZIEL.lock().ok().is_some_and(|g| g.as_ref().is_some_and(|z| z.pid == pid));
        if !weiter { crate::vorschau::einfuegemarke(None); }
    });
}

/// Endet sofort und in jedem Fall. Mehrfacher Aufruf ist harmlos.
pub fn beenden(grund: &str) {
    if grund != "browser_session" { browser_beenden(grund); }
    if grund != "ask_session" { ask_beenden(grund); }
    if grund != "vscode_session" { vscode_beenden(grund); }
    auswahl_verwerfen(grund);
    if !AKTIV.swap(false, Ordering::SeqCst) {
        return;
    }
    crate::vorschau::fokusring(None);
    crate::vorschau::einfuegemarke(None);
    GENERATION.fetch_add(1, Ordering::SeqCst);
    // Kurz halten: nie waehrend einer AX-Anfrage (der Arbeiter klont nur).
    let alt = ZIEL.lock().ok().and_then(|mut g| g.take());
    // Chromes vorheriges Hauptfenster zurueckgeben - verzoegert und nicht im
    // Tasten-Abgriff, und nur, wenn der Nutzer Chrome nicht selbst vorn hat.
    if let Some(Ziel { pid, vorher: Some(fenster), .. }) = alt {
        let generation = GENERATION.load(Ordering::SeqCst);
        let _ = std::thread::Builder::new().name("noki-tippen-zurueck".into()).spawn(move || {
            std::thread::sleep(Duration::from_millis(250));
            // Ein neues Tippen hat begonnen: dessen Fokus nicht wegnehmen.
            if aktiv() || GENERATION.load(Ordering::SeqCst) != generation { return; }
            if crate::programm_vorn(pid) { return; }
            let ok = fernbedienung::hauptfenster_zurueck(&fenster);
            crate::virtual_workspace::trace(&format!("[TIPPEN] chrome_focus_restored={ok}"));
        });
    }
    if let Ok(mut u) = UNTEN.lock() { u.clear(); }
    crate::vorschau::tippen_anzeige(false);
    crate::virtual_workspace::trace(&format!(
        "[TIPPEN] end reason={grund} keys_forwarded={} latency_ms={}", ANZAHL.load(Ordering::SeqCst),
        LATENZ.lock().map(|mut l| {
            if l.is_empty() { return "-".to_string(); }
            l.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let q = |f: f64| l[((l.len() as f64 - 1.0) * f).round() as usize];
            let t = format!("p50={:.1},p95={:.1},max={:.1}", q(0.5), q(0.95), l[l.len() - 1]);
            l.clear();
            t
        }).unwrap_or_default()
    ));
}

/// Nur, wenn noch dieselbe Sitzung laeuft (kein Ende einer neueren).
fn beenden_wenn(generation: u64, grund: &str) {
    if GENERATION.load(Ordering::SeqCst) == generation { beenden(grund); }
}

/// Character for a keycode in the CURRENT keyboard layout (fallback when an
/// event carries no unicode string, e.g. synthesized events).
fn zeichen_aus_layout(code: i64, shift: bool) -> String {
    #[link(name = "Carbon", kind = "framework")]
    extern "C" {
        fn TISCopyCurrentKeyboardLayoutInputSource() -> *mut c_void;
        fn TISGetInputSourceProperty(src: *mut c_void, key: *const c_void) -> *const c_void;
        static kTISPropertyUnicodeKeyLayoutData: *const c_void;
        fn LMGetKbdType() -> u8;
        fn UCKeyTranslate(layout: *const c_void, code: u16, action: u16, mods: u32, kbd: u32, opts: u32,
            dead: *mut u32, max: usize, len: *mut usize, chars: *mut u16) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" { fn CFDataGetBytePtr(d: *const c_void) -> *const c_void; }
    unsafe {
        let src = TISCopyCurrentKeyboardLayoutInputSource();
        if src.is_null() { return String::new(); }
        let daten = TISGetInputSourceProperty(src, kTISPropertyUnicodeKeyLayoutData);
        let mut out = String::new();
        if !daten.is_null() {
            let mut dead = 0u32; let mut len = 0usize; let mut buf = [0u16; 4];
            let mods = if shift { 2 } else { 0 }; // shiftKey >> 8
            if UCKeyTranslate(CFDataGetBytePtr(daten), code as u16, 0, mods, LMGetKbdType() as u32, 1,
                &mut dead, 4, &mut len, buf.as_mut_ptr()) == 0 {
                out = String::from_utf16_lossy(&buf[..len]);
            }
        }
        CFRelease(src as *const c_void);
        out
    }
}

fn zeichen(ev: *mut c_void) -> String {
    let mut buf = [0u16; 8];
    let mut n = 0usize;
    unsafe { CGEventKeyboardGetUnicodeString(ev, buf.len(), &mut n, buf.as_mut_ptr()) };
    String::from_utf16_lossy(&buf[..n.min(buf.len())])
}

const CMD: u64 = 0x10_0000;
const CTRL: u64 = 0x4_0000;
const F_TASTEN: [i64; 16] = [122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111, 105, 107, 113, 106];

/// Entscheidung im Tasten-Abgriff (kind 10 = keyDown, 11 = keyUp).
/// `true` = verbraucht (erreicht kein lokales Programm).
/// Tut selbst nichts Teures: Pruefung und Zustellung macht der Arbeiter.
// ---------------------------------------------------------------------
//  Browser typing session (Noki Browser): keys go to the page's focused
//  element via the browser's own DevTools queue - no OS focus, no app
//  activation, no Space switch. Started only after the browser verified
//  that the click focused an editable element of exactly this page.
// ---------------------------------------------------------------------
static BROWSER_ZIEL: Mutex<Option<(i32, i64)>> = Mutex::new(None);
/// Address mode: Some(typed text) while the address field is the target.
/// Address mode: (text, everything selected). Like Chrome, a click into the
/// omnibox selects the whole URL; the next character replaces it.
static BROWSER_ADRESSE: Mutex<Option<(String, bool)>> = Mutex::new(None);

pub fn beginnen_browser_adresse(pid: i32, wid: i64) {
    beginnen_browser(pid, wid);
    // Native route: while the omnibox holds focus inside Chrome, keys posted
    // to Chrome's PROCESS land in it with Chrome's own semantics (inline
    // autocomplete, Backspace/Delete of the suggestion, Enter) - measured:
    // no activation, no Space switch. Otherwise the AX value route.
    let mut feld = fernbedienung::adressfeld_lesen(pid, wid);
    if feld.as_ref().is_some_and(|f| !f.1) && fernbedienung::adressfeld_fokussieren(pid, wid) {
        feld = fernbedienung::adressfeld_lesen(pid, wid);
    }
    let nativ = feld.as_ref().is_some_and(|f| f.1);
    ADRESSE_NATIV.store(nativ, Ordering::SeqCst);
    if nativ {
        let wert = feld.map(|f| f.0).unwrap_or_default();
        let _ = fernbedienung::adressfeld_auswahl(pid, wid, 0, wert.encode_utf16().count() as isize);
        if let Ok(mut a) = BROWSER_ADRESSE.lock() { *a = Some((wert, true)); }
    } else {
        let url = crate::noki_browser::zustand(wid).and_then(|z| z["url"].as_str().map(str::to_string)).unwrap_or_default();
        if let Ok(mut a) = BROWSER_ADRESSE.lock() { *a = Some((url.clone(), true)); }
        let _ = fernbedienung::adressfeld_setzen(pid, wid, &url, true);
    }
    crate::virtual_workspace::trace(&format!("[TIPPEN] browser_address_mode wid={wid} route={}", if nativ { "native_omnibox" } else { "ax_value" }));
}

static ADRESSE_NATIV: AtomicBool = AtomicBool::new(false);

/// Native omnibox route: every key goes ONLY to Chrome's process.
fn adress_taste_nativ(pid: i32, wid: i64, code: i64, flags: u64, ev: *mut c_void) -> bool {
    let laenge = || fernbedienung::adressfeld_lesen(pid, wid).map(|f| f.0.encode_utf16().count() as isize).unwrap_or(0);
    match code {
        53 => {
            fernbedienung::taste_mit_text(pid, 53, "");
            fernbedienung::taste_mit_text(pid, 53, "");
            if let Ok(mut a) = BROWSER_ADRESSE.lock() { *a = None; }
            browser_beenden("escape");
        }
        36 | 76 => {
            let wert = fernbedienung::adressfeld_lesen(pid, wid).map(|f| f.0).unwrap_or_default();
            crate::virtual_workspace::trace(&format!("[TIPPEN] browser_address_enter wid={wid} route=native_omnibox text={wert:?}"));
            fernbedienung::taste_mit_text(pid, 36, "\r");
            if let Ok(mut a) = BROWSER_ADRESSE.lock() { *a = None; }
            browser_beenden("navigate");
        }
        48 => { if let Ok(mut a) = BROWSER_ADRESSE.lock() { *a = None; } browser_beenden("tab"); }
        _ if flags & CMD != 0 => match code {
            0 => { let n = laenge(); let _ = fernbedienung::adressfeld_auswahl(pid, wid, 0, n); }
            8 | 7 => {
                if let Some((wert, _, (loc, len))) = fernbedienung::adressfeld_lesen(pid, wid) {
                    let u: Vec<u16> = wert.encode_utf16().collect();
                    let (a, b) = (loc.max(0) as usize, (loc + len).max(0) as usize);
                    if b > a && b <= u.len() {
                        crate::lesezeichen::pb_text_setzen(&String::from_utf16_lossy(&u[a..b]));
                        if code == 7 { fernbedienung::taste_mit_text(pid, 117, ""); }
                    }
                }
            }
            9 => {
                if let Some(crate::lesezeichen::PbInhalt::Text(t, _)) = crate::lesezeichen::pb_lesen() {
                    let t: String = t.chars().filter(|c| !c.is_control()).collect();
                    if !t.is_empty() { fernbedienung::taste_mit_text(pid, 9, &t); }
                }
            }
            _ => {}
        },
        _ if flags & CTRL != 0 => {}
        51 | 117 | 115 | 119 | 123 | 124 | 125 | 126 => fernbedienung::taste_mit_text(pid, code, ""),
        _ => {
            let mut z = zeichen(ev);
            if z.is_empty() { z = zeichen_aus_layout(code, flags & 0x2_0000 != 0); }
            if !z.is_empty() && !z.chars().any(|c| c.is_control()) { fernbedienung::taste_mit_text(pid, code, &z); }
        }
    }
    true
}

/// Address mode key handling. true = consumed.
fn adress_taste(pid: i32, wid: i64, code: i64, flags: u64, ev: *mut c_void) -> bool {
    if ADRESSE_NATIV.load(Ordering::SeqCst) { return adress_taste_nativ(pid, wid, code, flags, ev); }
    let mut g = match BROWSER_ADRESSE.lock() { Ok(g) => g, Err(_) => return true };
    let Some((text, markiert)) = g.as_mut() else { return false };
    match code {
        53 => { drop(g); adresse_beenden(pid, wid, true); return true; }
        36 | 76 => {
            let ziel = text.trim().to_string();
            crate::virtual_workspace::trace(&format!("[TIPPEN] browser_address_enter wid={wid} text={ziel:?}"));
            drop(g);
            if !ziel.is_empty() { crate::noki_browser::navigieren(wid, &ziel); }
            adresse_beenden(pid, wid, false);
            return true;
        }
        51 | 117 => { if *markiert { text.clear(); } else { text.pop(); } *markiert = false; }
        _ if flags & CMD != 0 => {
            if code == 0 { *markiert = !text.is_empty(); }
            else if code == 9 {
                if let Some(crate::lesezeichen::PbInhalt::Text(t, _)) = crate::lesezeichen::pb_lesen() {
                    if *markiert { text.clear(); *markiert = false; }
                    text.push_str(t.trim());
                }
            }
            else if code == 8 && *markiert { crate::lesezeichen::pb_text_setzen(text); }
            else { return true; }
        }
        _ if flags & CTRL != 0 => { return true; }
        123 | 124 | 115 | 119 => { *markiert = false; }
        _ => {
            let mut z = zeichen(ev);
            if z.is_empty() { z = zeichen_aus_layout(code, flags & 0x2_0000 != 0); }
            if z.is_empty() || z.chars().any(|c| c.is_control()) { return true; }
            if *markiert { text.clear(); *markiert = false; }
            text.push_str(&z);
        }
    }
    let (anzeige, sel) = (text.clone(), *markiert);
    drop(g);
    let _ = fernbedienung::adressfeld_setzen(pid, wid, &anzeige, sel);
    true
}

fn adresse_beenden(pid: i32, wid: i64, zuruecksetzen: bool) {
    if let Ok(mut a) = BROWSER_ADRESSE.lock() { *a = None; }
    if zuruecksetzen {
        if let Some(url) = crate::noki_browser::zustand(wid).and_then(|z| z["url"].as_str().map(str::to_string)) {
            let _ = fernbedienung::adressfeld_setzen(pid, wid, &url, false);
        }
    }
    browser_beenden(if zuruecksetzen { "escape" } else { "navigate" });
}

pub fn beginnen_browser(pid: i32, wid: i64) {
    beenden("browser_session");
    if let Ok(mut g) = BROWSER_ZIEL.lock() { *g = Some((pid, wid)); }
    if let Ok(mut u) = UNTEN.lock() { u.clear(); }
    crate::vorschau::tippen_anzeige(true);
    crate::virtual_workspace::trace(&format!("[TIPPEN] start wid={wid} pid={pid} route=noki_browser_page capture=verified_editable"));
}

pub fn browser_beenden(grund: &str) {
    if let Ok(mut a) = BROWSER_ADRESSE.lock() { *a = None; }
    let alt = BROWSER_ZIEL.lock().ok().and_then(|mut g| g.take());
    if let Some((_, wid)) = alt {
        crate::vorschau::tippen_anzeige(false);
        crate::virtual_workspace::trace(&format!("[TIPPEN] end wid={wid} route=noki_browser_page reason={grund}"));
    }
}

fn browser_taste(kind: u32, ev: *mut c_void, wid: i64) -> bool {
    let code = unsafe { CGEventGetIntegerValueField(ev, 9) };
    let flags = unsafe { CGEventGetFlags(ev) };
    if kind == 11 { return true; } // key up: already consumed with its down
    if BROWSER_ADRESSE.lock().map(|a| a.is_some()).unwrap_or(false) {
        let pid = BROWSER_ZIEL.lock().ok().and_then(|g| *g).map(|z| z.0).unwrap_or(0);
        if flags & CMD != 0 && (code == 48 || code == 49) { adresse_beenden(pid, wid, true); return false; }
        return adress_taste(pid, wid, code, flags, ev);
    }
    if code == 53 { browser_beenden("escape"); return true; }
    if flags & CMD != 0 {
        if code == 48 || code == 49 { browser_beenden("system_shortcut"); return false; }
        let z = match zeichen(ev).to_lowercase() {
            z if !z.is_empty() => z,
            _ => match code { 0 => "a", 8 => "c", 7 => "x", 9 => "v", 6 => "z", _ => "" }.to_string(),
        };
        let befehl = z.chars().next().filter(|c| "acxvz".contains(*c));
        if let Some(b) = befehl { crate::noki_browser::taste(wid, code, String::new(), Some(b)); }
        return true; // other Cmd shortcuts: consumed, never local
    }
    if flags & CTRL != 0 { return true; }
    let mut z = zeichen(ev);
    if z.is_empty() { z = zeichen_aus_layout(code, flags & 0x2_0000 != 0); }
    crate::noki_browser::taste(wid, code, z, None);
    true
}

// ---------------------------------------------------------------------
//  VS Code typing session (Noki Companion bridge): keys go through the VS
//  Code API to exactly this window's active editor or active integrated
//  terminal. PID-posted keys are dropped by an inactive VS Code on another
//  Space (measured), and activating it would move the user. Started only
//  after the click verified a real VS Code input field of this window AND
//  the window's own bridge answered.
// ---------------------------------------------------------------------
/// Editor/terminal decided by the element under the click point, handed to
/// the session start of exactly this window (consumed once, 2 s valid).
static VS_ART_VORGABE: Mutex<Option<(i64, Option<bool>, std::time::Instant)>> = Mutex::new(None);
pub fn vscode_art_vorgeben(wid: i64, terminal: Option<bool>) {
    if let Ok(mut g) = VS_ART_VORGABE.lock() { *g = Some((wid, terminal, std::time::Instant::now())); }
}

struct VsZiel { pid: i32, wid: i64, art: crate::vscode_bruecke::Art, b: crate::vscode_bruecke::Bruecke }
static VS_ZIEL: Mutex<Option<VsZiel>> = Mutex::new(None);

pub fn beginnen_vscode(pid: i32, wid: i64, art: crate::vscode_bruecke::Art, b: crate::vscode_bruecke::Bruecke) {
    beenden("vscode_session");
    if let Ok(mut g) = VS_ZIEL.lock() { *g = Some(VsZiel { pid, wid, art, b }); }
    if let Ok(mut u) = UNTEN.lock() { u.clear(); }
    ANZAHL.store(0, Ordering::SeqCst);
    crate::vorschau::tippen_anzeige(true);
    crate::virtual_workspace::trace(&format!(
        "[TIPPEN] start wid={wid} pid={pid} remoteTarget={} route=vscode_bridge capture=verified_editable", art.name()));
    waechter();
}

pub fn vscode_beenden(grund: &str) {
    let alt = VS_ZIEL.lock().ok().and_then(|mut g| g.take());
    if let Some(z) = alt {
        crate::vorschau::tippen_anzeige(false);
        crate::virtual_workspace::trace(&format!(
            "[TIPPEN] end wid={} remoteTarget={} reason={grund} keys_forwarded={}", z.wid, z.art.name(), ANZAHL.load(Ordering::SeqCst)));
    }
}

/// The ONE current remote keyboard target (sessions are exclusive: every
/// start ends the others first).
pub fn remote_ziel() -> &'static str {
    if ask_ziel().is_some() { return "ASK_NOKI_INPUT"; }
    if let Some(a) = VS_ZIEL.lock().ok().and_then(|g| g.as_ref().map(|z| z.art)) { return a.name(); }
    if BROWSER_ZIEL.lock().ok().is_some_and(|g| g.is_some()) { return "NOKI_BROWSER_PAGE"; }
    if aktiv() { return "AX_FIELD"; }
    "NONE"
}

fn vscode_taste(kind: u32, ev: *mut c_void) -> bool {
    use crate::vscode_bruecke::Art;
    let Some((pid, wid, art, b)) = VS_ZIEL.lock().ok().and_then(|g| g.as_ref().map(|z| (z.pid, z.wid, z.art, z.b.clone()))) else { return false };
    if kind == 11 { return true; } // key up: consumed with its down
    let code = unsafe { CGEventGetIntegerValueField(ev, 9) };
    let flags = unsafe { CGEventGetFlags(ev) };
    let shift = flags & 0x2_0000 != 0;
    let senden = |key: &str, text: &str| {
        crate::vscode_bruecke::senden(&b, art, key, text, shift, pid, wid);
        ANZAHL.fetch_add(1, Ordering::SeqCst);
    };
    if code == 53 {
        if art == Art::Editor { senden("Escape", ""); }
        vscode_beenden("escape");
        return true;
    }
    if flags & CMD != 0 {
        if code == 48 || code == 49 { vscode_beenden("system_shortcut"); return false; }
        let z = kuerzel_zeichen(ev, code);
        match (z.as_str(), art) {
            ("a", Art::Editor) => senden("selectAll", ""),
            ("c", _) => senden("copy", ""),
            ("x", Art::Editor) => senden("cut", ""),
            ("v", _) => senden("paste", ""),
            ("s", Art::Editor) => senden("save", ""),
            ("z", Art::Editor) => senden(if shift { "redo" } else { "undo" }, ""),
            _ => {}
        }
        return true; // other Cmd shortcuts: consumed, never local
    }
    if flags & CTRL != 0 {
        if matches!(code, 123..=126) || code == 49 { vscode_beenden("system_shortcut"); return false; }
        if art == Art::Terminal {
            let z = zeichen_aus_layout(code, false);
            if !z.is_empty() { senden("ctrl", &z); }
        }
        return true;
    }
    if F_TASTEN.contains(&code) { return true; }
    let name = match code {
        51 => "Backspace", 117 => "Delete", 36 | 76 => "Enter", 48 => "Tab",
        123 => "ArrowLeft", 124 => "ArrowRight", 125 => "ArrowDown", 126 => "ArrowUp",
        115 => "Home", 119 => "End",
        _ => "",
    };
    if !name.is_empty() {
        senden(name, "");
    } else {
        let mut z = zeichen(ev);
        if z.is_empty() { z = zeichen_aus_layout(code, shift); }
        let z: String = z.chars().filter(|c| !c.is_control()).collect();
        if !z.is_empty() { senden("text", &z); }
    }
    true
}

// ---------------------------------------------------------------------
//  Ask Noki typing session: Noki's own window. Keys go into Ask's focused
//  page element (ask_fern) - never PID-posted to Noki (that would come back
//  through this very tap), no activation, no Space switch. Started only
//  after Ask's page confirmed that the Miniatur click focused an editable.
// ---------------------------------------------------------------------
static ASK_ZIEL: Mutex<Option<i64>> = Mutex::new(None);

pub fn beginnen_ask(wid: i64) {
    beenden("ask_session");
    if let Ok(mut g) = ASK_ZIEL.lock() { *g = Some(wid); }
    if let Ok(mut u) = UNTEN.lock() { u.clear(); }
    ANZAHL.store(0, Ordering::SeqCst);
    crate::vorschau::tippen_anzeige(true);
    crate::virtual_workspace::trace(&format!("[TIPPEN] start wid={wid} route=ask_webview capture=verified_editable"));
    waechter();
}

pub fn ask_beenden(grund: &str) {
    let alt = ASK_ZIEL.lock().ok().and_then(|mut g| g.take());
    if let Some(wid) = alt {
        crate::vorschau::tippen_anzeige(false);
        crate::virtual_workspace::trace(&format!(
            "[TIPPEN] end wid={wid} route=ask_webview reason={grund} keys_forwarded={}", ANZAHL.load(Ordering::SeqCst)));
    }
}

/// The letter of a Cmd shortcut. The event's own character decides - unless
/// it is empty or a CONTROL character (measured: Cmd+S arrived as "\u{13}"),
/// then the ANSI keycode does.
fn kuerzel_zeichen(ev: *mut c_void, code: i64) -> String {
    let z = zeichen(ev).to_lowercase();
    if !z.is_empty() && !z.chars().any(|c| c.is_control()) { return z; }
    match code { 0 => "a", 8 => "c", 7 => "x", 9 => "v", 1 => "s", 6 => "z", 12 => "q", 13 => "w", 17 => "t", 45 => "n", _ => "" }.to_string()
}

fn ask_ziel() -> Option<i64> {
    ASK_ZIEL.lock().ok().and_then(|g| *g)
}

fn ask_taste(kind: u32, ev: *mut c_void) -> bool {
    let code = unsafe { CGEventGetIntegerValueField(ev, 9) };
    let flags = unsafe { CGEventGetFlags(ev) };
    if kind == 11 { return true; } // key up: consumed with its down
    let shift = flags & 0x2_0000 != 0;
    let alt = flags & 0x8_0000 != 0;
    if code == 53 {
        // Esc ends the remote session ("Esc beendet") and is NOT passed on:
        // Ask's own Escape hides the Ask window (measured).
        ask_beenden("escape");
        return true;
    }
    if flags & CMD != 0 {
        if code == 48 || code == 49 { ask_beenden("system_shortcut"); return false; }
        let z = kuerzel_zeichen(ev, code);
        if let Some(b) = z.chars().next().filter(|c| "acxvz".contains(*c)) {
            crate::ask_fern::befehl(b, shift);
        }
        return true; // other Cmd shortcuts: consumed, never local
    }
    if flags & CTRL != 0 || F_TASTEN.contains(&code) { return true; }
    let name = match code {
        51 => "Backspace", 117 => "Delete", 36 | 76 => "Enter", 48 => "Tab",
        123 => "ArrowLeft", 124 => "ArrowRight", 125 => "ArrowDown", 126 => "ArrowUp",
        115 => "Home", 119 => "End",
        _ => "",
    };
    if !name.is_empty() {
        crate::ask_fern::taste(name, "", shift, alt);
    } else {
        let mut z = zeichen(ev);
        if z.is_empty() { z = zeichen_aus_layout(code, shift); }
        let z: String = z.chars().filter(|c| !c.is_control()).collect();
        if z.is_empty() { return true; }
        let erstes = z.chars().next().map(String::from).unwrap_or_default();
        crate::ask_fern::taste(&erstes, &z, shift, alt);
    }
    ANZAHL.fetch_add(1, Ordering::SeqCst);
    true
}

pub fn taste(kind: u32, ev: *mut c_void) -> bool {
    if ask_ziel().is_some() {
        return ask_taste(kind, ev);
    }
    if VS_ZIEL.lock().ok().is_some_and(|g| g.is_some()) {
        return vscode_taste(kind, ev);
    }
    if let Some((_, wid)) = BROWSER_ZIEL.lock().ok().and_then(|g| *g) {
        return browser_taste(kind, ev, wid);
    }
    if !aktiv() && !wartet() && !interaction_aktiv() {
        return auswahl_aktiv() && auswahl_taste(kind, ev);
    }
    let code = unsafe { CGEventGetIntegerValueField(ev, 9) };
    let flags = unsafe { CGEventGetFlags(ev) };
    let generation = GENERATION.load(Ordering::SeqCst);
    if !aktiv() && !wartet() {
        if kind == 11 {
            let war_unten = UNTEN.lock().map(|mut keys| {
                keys.iter().position(|k| *k == code)
                    .map(|i| { keys.remove(i); true }).unwrap_or(false)
            }).unwrap_or(false);
            if war_unten {
                if let Some(front_wid) = crate::vorschau::aktives_oder_vorderstes_fenster() {
                    if let Some(pid) = crate::vorschau::fenster_prozess(front_wid) {
                        let _ = fernbedienung::taste_an_prozess(pid, ev as usize, MARKE);
                    }
                }
                return true;
            }
            return false;
        }
        if code == 53 {
            interaction_beenden("escape");
            return true;
        }
        if flags & CMD != 0 && matches!(code, 48 | 49) {
            interaction_beenden("system_shortcut");
            return false;
        }
        if let Some(front_wid) = crate::vorschau::aktives_oder_vorderstes_fenster() {
            if let Some(pid) = crate::vorschau::fenster_prozess(front_wid) {
                if pid > 0 && pid != std::process::id() as i32 {
                    let ok = fernbedienung::taste_an_prozess(pid, ev as usize, MARKE);
                    if ok {
                        if let Ok(mut keys) = UNTEN.lock() {
                            if !keys.contains(&code) { keys.push(code); }
                        }
                        crate::virtual_workspace::trace(&format!(
                            "[INTERACTION] key_routed_to_front wid={front_wid} pid={pid} keycode={code}"
                        ));
                        return true;
                    }
                }
            }
        }
        interaction_beenden("no_verified_remote_target");
        return false;
    }
    if kind == 11 {
        let war_unten = UNTEN.lock().map(|mut u| {
            if let Some(i) = u.iter().position(|c| *c == code) { u.remove(i); true } else { false }
        }).unwrap_or(false);
        if war_unten { senden(ev, false, generation, Nach::Nichts); }
        // Das Loslassen einer Taste, deren Druck vor dem Modus lokal war,
        // gehoert dem lokalen Programm.
        return war_unten;
    }
    if code == 53 {
        if aktiv() { beenden("escape"); } else { warten_beenden("escape"); }
        return true;
    }
    let cmd = flags & CMD != 0;
    let ctrl = flags & CTRL != 0;
    if cmd {
        // Systemkuerzel bleiben System: App-Umschalter und Spotlight
        // beenden das Tippen und gehen unveraendert weiter.
        if code == 48 || code == 49 {
            beenden("system_shortcut");
            warten_beenden("system_shortcut");
            return false;
        }
        // The character decides (any layout); without one (synthetic or
        // dead-key events) fall back to the ANSI keycode. Before, an empty
        // character let Cmd-N through to Terminal, which then opened a new
        // window on the user's Desktop (measured).
        let z = match zeichen(ev).to_lowercase() {
            z if !z.is_empty() => z,
            _ => match code { 0 => "a", 8 => "c", 7 => "x", 9 => "v", 12 => "q", 13 => "w", 17 => "t", 45 => "n", _ => "" }.to_string(),
        };
        let semantisch = match z.as_str() {
            "c" => Some(Aktion::Kopieren), "x" => Some(Aktion::Ausschneiden), "v" => Some(Aktion::Einsetzen),
            "a" if echter_raum() && flags & (CTRL | 0x8_0000 | 0x2_0000) == 0 => Some(Aktion::AllesMarkieren),
            _ => None,
        };
        if let Some(aktion) = semantisch {
            bearbeiten_(ev, aktion, generation);
        } else if echter_raum() && z == "w" && flags & (CTRL | 0x8_0000 | 0x2_0000) == 0 {
            // Cmd-W: exactly THIS window, via its own close button (AX).
            // A PID Cmd-W is dropped by an inactive app (measured).
            auftrag_ohne_taste(ev, Aktion::FensterZu, generation);
            crate::virtual_workspace::trace("[TIPPEN] shortcut command=close_window route=ax_exact_window");
        } else if echter_raum() && (z == "n" || z == "t") {
            // Measured: a background "New Window/Tab" is ALWAYS born on the
            // user's CURRENT Desktop (Terminal created a new window there
            // instead of a tab in the hidden one). Delivering it would put a
            // window in front of the user; a Space trip is forbidden. Fail
            // closed and say why.
            crate::vorschau::hinweis("Neue Fenster und Tabs entstehen bei macOS auf dem aktuellen Schreibtisch – öffne sie über „Zum Schreibtisch“.");
            crate::virtual_workspace::trace(&format!(
                "[TIPPEN] shortcut_blocked command=cmd_{z} reason=macos_creates_on_active_space forwarded=false"));
        } else if z == "q" {
            // A remote workspace may close one exact tab/window (Cmd-W),
            // but it never quits the whole shared application process.
            // This is the one destructive app shortcut Noki reserves.
            crate::virtual_workspace::trace("[TIPPEN] shortcut_reserved command=quit forwarded=false");
        } else {
            // Once an exact owned field/window has been verified, app
            // shortcuts belong to that app. The copied CGEvent retains the
            // complete Command/Option/Shift/Fn state. Cmd-N/T/W/F/K/+/-,
            // and app-specific equivalents therefore use the same generic
            // route as ordinary typing. The worker re-verifies the exact
            // target immediately before posting it to the target PID.
            senden(ev, true, generation, Nach::Nichts);
            crate::virtual_workspace::trace(&format!(
                "[TIPPEN] app_shortcut keycode={code} command=true forwarded=true"
            ));
        }
        return true;
    }
    if ctrl {
        if matches!(code, 123..=126) || code == 49 {
            // Ctrl+Pfeil gehoert Mission Control, Ctrl+Leertaste der
            // Eingabequelle.
            beenden("system_shortcut");
            warten_beenden("system_shortcut");
            return false;
        }
        // Measured (macOS 26.6, REAL_SPACE): AppKit drops PID-posted
        // Control/Command keys while the target app is inactive - even with
        // the modifier physically held. Terminal's job-control keys are
        // therefore delivered like the tty driver itself does it: the
        // signal goes to the foreground process group of EXACTLY this
        // window's tty (resolved uniquely, else fail closed).
        if terminal_ziel() {
            let signal = match code { 8 => Some(libc::SIGINT), 6 => Some(libc::SIGTSTP), 42 => Some(libc::SIGQUIT), _ => None };
            if let Some(sig) = signal {
                auftrag_ohne_taste(ev, Aktion::Signal(sig), generation);
                crate::virtual_workspace::trace(&format!("[TIPPEN] ctrl_key keycode={code} route=tty_signal sig={sig}"));
                return true;
            }
        }
        // Ctrl+C/D/L/Z/A/E/K/U ... are terminal/editor keys, not Cmd
        // shortcuts: forward the exact physical event (Control flag kept)
        // to the verified field. Before, every Ctrl combo was swallowed -
        // Ctrl+C never interrupted a command in the Noki Terminal.
        senden(ev, true, generation, Nach::Nichts);
        crate::virtual_workspace::trace(&format!("[TIPPEN] ctrl_key keycode={code} forwarded=true"));
        return true;
    }
    if F_TASTEN.contains(&code) {
        return true;
    }
    let nach = if matches!(code, 48 | 36 | 76) { Nach::FokusPruefen } else { Nach::Nichts };
    senden(ev, true, generation, nach);
    true
}

fn senden(ev: *mut c_void, runter: bool, generation: u64, nach: Nach) {
    let kopie = unsafe { CGEventCreateCopy(ev) };
    if kopie.is_null() {
        return;
    }
    let code = unsafe { CGEventGetIntegerValueField(ev, 9) };
    if runter {
        if let Ok(mut u) = UNTEN.lock() { if !u.contains(&code) { u.push(code); } }
    }
    if !aktiv() {
        // Noch kein nachgewiesenes Feld: halten (hoechstens 64 Ereignisse).
        if let Ok(mut p) = PUFFER.lock() {
            if p.len() < 64 { p.push((kopie as usize, runter, nach)); return; }
        }
        unsafe { CFRelease(kopie as *const c_void) };
        return;
    }
    einreihen(Auftrag { aktion: Aktion::Taste, kopie: kopie as usize, runter, generation, nach, seit: std::time::Instant::now() });
}

/// Cmd+C/X/V als semantische Handlung - in derselben Reihenfolge wie die
/// Tasten davor (derselbe Arbeiter).
fn bearbeiten_(ev: *mut c_void, aktion: Aktion, generation: u64) {
    let kopie = unsafe { CGEventCreateCopy(ev) };
    if kopie.is_null() { return; }
    if !aktiv() { unsafe { CFRelease(kopie as *const c_void) }; return; }
    let code = unsafe { CGEventGetIntegerValueField(ev, 9) };
    if let Ok(mut u) = UNTEN.lock() { if !u.contains(&code) { u.push(code); } }
    einreihen(Auftrag { aktion, kopie: kopie as usize, runter: true, generation, nach: Nach::Nichts, seit: std::time::Instant::now() });
}

fn echter_raum() -> bool {
    crate::virtual_workspace::backend() == crate::virtual_workspace::Backend::RealSpace
}

fn terminal_ziel() -> bool {
    ziel().is_some_and(|(pid, ..)| crate::lesezeichen::app_fuer_pid(pid).is_some_and(|a| a.1 == "com.apple.Terminal"))
}

/// Eine Handlung statt einer Taste - in derselben Reihenfolge und mit
/// derselben Zielpruefung wie Tasten (derselbe Arbeiter).
fn auftrag_ohne_taste(ev: *mut c_void, aktion: Aktion, generation: u64) {
    let kopie = unsafe { CGEventCreateCopy(ev) };
    if kopie.is_null() { return; }
    if !aktiv() { unsafe { CFRelease(kopie as *const c_void) }; return; }
    let code = unsafe { CGEventGetIntegerValueField(ev, 9) };
    if let Ok(mut u) = UNTEN.lock() { if !u.contains(&code) { u.push(code); } }
    einreihen(Auftrag { aktion, kopie: kopie as usize, runter: true, generation, nach: Nach::Nichts, seit: std::time::Instant::now() });
}

/// Die tty GENAU dieses Terminal-Fensters (des gewaehlten Tabs) und ihre
/// Vordergrund-Prozessgruppe. Zuordnung ohne Automation/AppleScript:
/// Terminal -> login (je Tab eine tty) -> Shell. Eindeutig macht es das
/// Arbeitsverzeichnis der Shell (AXDocument des Fensters) und, falls noetig,
/// der Name des Vordergrundprozesses im Fenstertitel. Nicht eindeutig ->
/// None (fail closed): nie ein Signal an einen fremden Tab.
fn terminal_vordergrund(pid: i32, wid: i64) -> Option<(i32, String)> {
    let (titel, dok, _) = fernbedienung::fenster_titel_dokument(pid, wid)?;
    let pfad = dok.strip_prefix("file://")?;
    let pfad = prozent_dekodieren(pfad);
    let ziel_dir = std::fs::canonicalize(pfad.trim_end_matches('/')).ok()?;
    let alle = alle_prozesse();
    let uid = unsafe { libc::getuid() };
    let info = |p: i32| alle.iter().find(|i| i.0 == p);
    let mut kandidaten: Vec<(i32, String)> = vec![];
    for login in alle.iter().filter(|i| i.1 == pid) {
        for shell in alle.iter().filter(|i| i.1 == login.0 && i.4 == uid) {
            let Some(cwd) = prozess_cwd(shell.0) else { continue };
            if std::fs::canonicalize(&cwd).ok().as_ref() != Some(&ziel_dir) { continue; }
            let tpgid = shell.2;
            // Only a foreground group led by one of the user's own processes.
            if tpgid <= 0 || info(tpgid).is_some_and(|i| i.4 != uid) { continue; }
            let name = info(tpgid).map(|i| i.3.clone()).unwrap_or_default();
            kandidaten.push((tpgid, name));
        }
    }
    if kandidaten.len() > 1 {
        kandidaten.retain(|(_, n)| !n.is_empty() && titel.contains(n.trim_start_matches('-')));
    }
    if kandidaten.len() == 1 { kandidaten.pop() } else {
        crate::virtual_workspace::trace(&format!("[TIPPEN] tty_resolve wid={wid} candidates={} result=ambiguous", kandidaten.len()));
        None
    }
}

fn prozent_dekodieren(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) { out.push(v); i += 3; continue; }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// (pid, ppid, tpgid, name, uid) aller Prozesse. `login` gehoert root -
/// deshalb hier ohne Nutzerfilter; signalisiert wird nur eine Gruppe des
/// Nutzers (siehe `terminal_vordergrund`).
fn alle_prozesse() -> Vec<(i32, i32, i32, String, u32)> {
    let mut pids = vec![0i32; 8192];
    let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr() as *mut c_void, (pids.len() * 4) as i32) };
    if n <= 0 { return vec![]; }
    pids.truncate(n as usize);
    pids.into_iter().filter_map(|p| {
        let mut i: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let g = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        let r = unsafe { libc::proc_pidinfo(p, libc::PROC_PIDTBSDINFO, 0, &mut i as *mut _ as *mut c_void, g) };
        if r != g {
            // Root-owned processes (Terminal's `login`) refuse the full info
            // (EPERM); the short info still carries parent and owner.
            let mut k: libc::proc_bsdshortinfo = unsafe { std::mem::zeroed() };
            let g = std::mem::size_of::<libc::proc_bsdshortinfo>() as i32;
            let r = unsafe { libc::proc_pidinfo(p, libc::PROC_PIDT_SHORTBSDINFO, 0, &mut k as *mut _ as *mut c_void, g) };
            if r != g { return None; }
            let name: Vec<u8> = k.pbsi_comm.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
            return Some((p, k.pbsi_ppid as i32, 0, String::from_utf8_lossy(&name).into_owned(), k.pbsi_uid));
        }
        let name: Vec<u8> = i.pbi_comm.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
        Some((p, i.pbi_ppid as i32, i.e_tpgid as i32, String::from_utf8_lossy(&name).into_owned(), i.pbi_uid))
    }).collect()
}

fn prozess_cwd(p: i32) -> Option<String> {
    let mut v: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let g = std::mem::size_of::<libc::proc_vnodepathinfo>() as i32;
    let r = unsafe { libc::proc_pidinfo(p, libc::PROC_PIDVNODEPATHINFO, 0, &mut v as *mut _ as *mut c_void, g) };
    if r != g { return None; }
    let roh: Vec<u8> = v.pvi_cdir.vip_path.iter().flatten().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    (!roh.is_empty()).then(|| String::from_utf8_lossy(&roh).into_owned())
}

fn einreihen(auftrag: Auftrag) {
    // A typed shortcut (Cmd-O, Cmd-S ...) can open a dialog: Noki-caused.
    if let Some((pid, wid, _)) = ziel() { crate::kind_fenster::transaktion(pid, wid, crate::fenster_rahmen(wid)); }
    match KANAL.get() {
        Some(tx) => {
            // The event tap must not block, but a busy target must not make
            // typed keys disappear. `Sender::send` on an unbounded channel
            // is non-blocking with respect to the consumer.
            if let Err(e) = tx.send(auftrag) {
                let a = e.0;
                unsafe { CFRelease(a.kopie as *const c_void) };
            }
        }
        None => unsafe { CFRelease(auftrag.kopie as *const c_void) },
    }
}

fn arbeiter() {
    KANAL.get_or_init(|| {
        let (tx, rx) = channel::<Auftrag>();
        let _ = std::thread::Builder::new().name("noki-tippen".into()).spawn(move || {
            while let Ok(a) = rx.recv() {
                heartbeat();
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| zustellen(&a)));
                unsafe { CFRelease(a.kopie as *const c_void) };
                if r.is_err() { beenden("fehler"); }
                heartbeat();
            }
        });
        tx
    });
}

fn zustellen(a: &Auftrag) {
    if !aktiv() || GENERATION.load(Ordering::SeqCst) != a.generation {
        return; // Modus beendet: die Taste verfaellt, sie geht nirgendwo hin.
    }
    // A target that stopped answering must not receive a backlog of keys
    // seconds later (or starve the router): end the session cleanly.
    if a.seit.elapsed() > std::time::Duration::from_millis(2500) {
        beenden_wenn(a.generation, "target_unresponsive");
        crate::vorschau::hinweis("Das Programm reagiert gerade nicht – Tippen beendet.");
        return;
    }
    let Some((pid, wid, feld)) = ziel() else { return };
    crate::vorschau::fern_markieren();
    crate::vorschau::lebhaft(wid);
    if a.runter && !fernbedienung::fokus_ist(pid, wid, &feld) {
        beenden_wenn(a.generation, "focus_not_verified");
        return;
    }
    match a.aktion {
        Aktion::Taste => {
            if fernbedienung::taste_an_prozess(pid, a.kopie, MARKE) && a.runter {
                ANZAHL.fetch_add(1, Ordering::SeqCst);
                if let Ok(mut l) = LATENZ.lock() { l.push(a.seit.elapsed().as_secs_f64() * 1000.0); }
            }
        }
        Aktion::Kopieren | Aktion::Ausschneiden => {
            let text = fernbedienung::fokus_auswahl_text(pid).unwrap_or_default();
            let ok = !text.is_empty() && crate::lesezeichen::pb_text_setzen(&text);
            if ok && a.aktion == Aktion::Ausschneiden {
                // Ausschneiden = Kopieren + Loeschen der echten Auswahl.
                fernbedienung::taste_senden(pid, 51, 8, MARKE);
            }
            crate::virtual_workspace::trace(&format!(
                "[TIPPEN] {} chars={} ok={ok}",
                if a.aktion == Aktion::Kopieren { "copy" } else { "cut" }, text.chars().count()
            ));
        }
        Aktion::AllesMarkieren => {
            let ok = fernbedienung::fokus_alles_markieren(pid);
            crate::virtual_workspace::trace(&format!("[TIPPEN] select_all route=ax_selected_range ok={ok}"));
        }
        Aktion::Signal(sig) => {
            let ziel = terminal_vordergrund(pid, wid);
            let ok = ziel.as_ref().is_some_and(|(pg, _)| unsafe { libc::kill(-pg, sig) } == 0);
            crate::virtual_workspace::trace(&format!(
                "[TIPPEN] tty_signal wid={wid} sig={sig} pgid={} fg={:?} ok={ok}",
                ziel.as_ref().map(|z| z.0).unwrap_or(0), ziel.as_ref().map(|z| z.1.as_str()).unwrap_or("")));
        }
        Aktion::FensterZu => {
            let tabs = fernbedienung::fenster_titel_dokument(pid, wid).is_some_and(|t| t.2);
            // With tabs Cmd-W closes only the tab - AX can't target that
            // tab safely yet: fail closed instead of closing all tabs.
            let ok = !tabs && fernbedienung::fenster_schliessen(pid, wid);
            crate::virtual_workspace::trace(&format!("[TIPPEN] close_window wid={wid} tabs={tabs} ok={ok}"));
            if ok { beenden_wenn(a.generation, "window_closed"); }
            else if tabs { crate::vorschau::hinweis("Tabs lassen sich in der Miniatur noch nicht einzeln schließen."); }
        }
        Aktion::Einsetzen => {
            let text = match crate::lesezeichen::pb_lesen() {
                Some(crate::lesezeichen::PbInhalt::Text(t, _)) => t,
                _ => String::new(),
            };
            let mehrzeilig = fernbedienung::fokus_rolle(pid) == "AXTextArea";
            let ok = !text.is_empty() && fernbedienung::text_an_prozess(pid, &text, mehrzeilig, MARKE);
            crate::virtual_workspace::trace(&format!("[TIPPEN] paste chars={} ok={ok}", text.chars().count()));
        }
    }
    // VS Code & co. paint nothing on a hidden Space: bring the result in.
    crate::vorschau::eingabe_zeichnen(pid, wid);
    if matches!(a.nach, Nach::FokusPruefen) {
        std::thread::sleep(Duration::from_millis(140));
        if !aktiv() || GENERATION.load(Ordering::SeqCst) != a.generation { return; }
        match fernbedienung::fokus_eingabe(pid, wid) {
            Some(neu) => {
                if let Ok(mut g) = ZIEL.lock() {
                    if let Some(z) = g.as_mut() { z.feld = Arc::new(neu); }
                }
            }
            // Inactive Electron apps (VS Code) report NO focused UI element
            // at all (measured): then the SAME field must still report
            // AXFocused in exactly this window - only then does it go on.
            None => {
                let feld = ZIEL.lock().ok().and_then(|g| g.as_ref().map(|z| Arc::clone(&z.feld)));
                if feld.is_some_and(|f| fernbedienung::fokus_ist(pid, wid, &f)) {
                    crate::virtual_workspace::trace("[TIPPEN] refocus_check same_field_still_focused=true");
                } else {
                    // Enter hat z. B. navigiert: das Feld hat keinen Fokus mehr.
                    beenden_wenn(a.generation, "focus_left_field");
                }
            }
        }
    }
}

/// Prueft laufend, ob das Ziel noch gueltig ist - damit der Modus auch
/// ohne weiteren Tastendruck endet (Fenster zu, Ansicht weg, Backend weg).
fn waechter() {
    WAECHTER.get_or_init(|| {
        let _ = std::thread::Builder::new().name("noki-tippen-waechter".into()).spawn(|| loop {
            std::thread::sleep(Duration::from_millis(300));
            if let Some(wid) = AUSWAHL.lock().ok().and_then(|g| g.as_ref().map(|a| a.1)) {
                if !crate::vorschau::sichtbar() { auswahl_verwerfen("view_hidden"); }
                else if !crate::vorschau::aufgenommene().contains(&wid) { auswahl_verwerfen("window_gone"); }
            }
            if let Some(wid) = VS_ZIEL.lock().ok().and_then(|g| g.as_ref().map(|z| z.wid)) {
                if !crate::vorschau::sichtbar() { vscode_beenden("view_hidden"); }
                else if !crate::vorschau::aufgenommene().contains(&wid) { vscode_beenden("window_gone"); }
                else if crate::stimme_laeuft() { vscode_beenden("voice_owns_keyboard"); }
            }
            if let Some(wid) = ask_ziel() {
                if !crate::vorschau::sichtbar() { ask_beenden("view_hidden"); }
                else if !crate::ask_fern::ist_ask(wid) { ask_beenden("ask_hidden"); }
                else if !crate::vorschau::aufgenommene().contains(&wid) { ask_beenden("window_gone"); }
                else if crate::stimme_laeuft() { ask_beenden("voice_owns_keyboard"); }
            }
            if let Some((_, wid)) = BROWSER_ZIEL.lock().ok().and_then(|g| *g) {
                if !crate::vorschau::sichtbar() { browser_beenden("view_hidden"); }
                else if !crate::vorschau::aufgenommene().contains(&wid) { browser_beenden("window_gone"); }
            }
            if !aktiv() { continue; }
            let generation = GENERATION.load(Ordering::SeqCst);
            let Some((pid, wid, feld)) = ziel() else { continue };
            // `ready()` describes the LEGACY virtual display only and is
            // always false under REAL_SPACE - checking it there ended every
            // REAL_SPACE typing session within 300 ms (`backend_not_ready`).
            let grund = if !echter_raum() && !crate::virtual_workspace::ready() {
                Some("backend_not_ready")
            } else if !crate::vorschau::sichtbar() {
                Some("view_hidden")
            } else if !crate::vorschau::aufgenommene().contains(&wid) {
                Some("window_gone")
            } else if crate::stimme_laeuft() {
                Some("voice_owns_keyboard")
            } else if !fernbedienung::fokus_ist(pid, wid, &feld) {
                Some("focus_lost")
            } else {
                None
            };
            if let Some(g) = grund { beenden_wenn(generation, g); }
        });
    });
}
