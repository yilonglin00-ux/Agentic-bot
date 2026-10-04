//! Die kleine Arbeitsplatz-Vorschau unten links.
//!
//! Sie zeigt ECHTE Bilder der Fenster, die Noki fuer die laufende Aufgabe auf
//! seinem eigenen Schreibtisch geoeffnet hat - auch dann, wenn dieser
//! Schreibtisch gerade nicht sichtbar ist. Der Nutzer bleibt auf seinem
//! Schreibtisch und sieht trotzdem, woran Noki arbeitet.
//!
//! Aufgenommen wird ausschliesslich, was in `fenster` steht: die
//! aufgabeneigenen Fenster. Nie der Bildschirm des Nutzers, nie ein fremdes
//! Programm - das ist die Datenschutzgrenze dieser Funktion.
//!
//! Die Aufnahme selbst macht ein kleiner Helfer (NokiSchirm.app), weil die
//! Freigabe "Bildschirmaufnahme" an einer Bundle-Identitaet haengt und
//! ScreenCaptureKit eine Swift-API mit Delegates ist.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use tauri::{Emitter, Manager};

struct Lauf {
    kind: Child,
    ein: ChildStdin,
    /// Was gerade aufgenommen wird - damit ein unveraendertes `setzen`
    /// keinen Neustart ausloest.
    fenster: Vec<i64>,
    /// Noch nicht dargestelltes Retarget. Erst READY aus dem Helfer macht
    /// diese Liste zur interaktiven Wahrheit.
    wartend: Option<Vec<i64>>,
    /// Identitaet der tatsaechlich veroeffentlichten Komposition und des
    /// noch unsichtbar vorbereiteten Ziels. Nur fuer die debug-gesteuerte
    /// Abnahme-Auditspur; die Fensterliste bleibt die Laufzeit-Wahrheit.
    veroeffentlicht_uuid: String,
    wartend_uuid: Option<String>,
    wartend_nutzer_uuid: String,
    pausiert: bool,
}

static LAUF: Mutex<Option<Lauf>> = Mutex::new(None);
static RETARGET_AKTIV: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Real liveness signals from the helper.  These are updated only by bytes
/// actually received from the helper; diagnostics must never manufacture a
/// green heartbeat from the time at which they were queried.
static HELFER_LETZTE_NACHRICHT_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static PRAESENTATION_HEARTBEAT_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static CAPTURE_HEARTBEAT_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static COMPOSITOR_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// What the helper actually presented on top. This is deliberately separate
/// from `VORN`, which is only an optional user-selected compositor override.
static COMPOSITOR_FRONT_WID: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
static HELFER_RECOVERIES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static HELFER_SOFT_RECOVERY_FOR_HEARTBEAT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn abnahme() -> bool {
    cfg!(debug_assertions)
        && std::env::var("NOKI_ACCEPTANCE").ok().as_deref() != Some("0")
}

pub fn retarget_aktiv() -> bool {
    RETARGET_AKTIV.load(std::sync::atomic::Ordering::SeqCst)
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Base64 ohne zusaetzliche Abhaengigkeit - die Bilder gehen als Text durch
/// denselben Ereignisweg wie alles andere.
fn b64(roh: &[u8]) -> String {
    let mut s = String::with_capacity(roh.len().div_ceil(3) * 4);
    for p in roh.chunks(3) {
        let (a, b, c) = (p[0] as u32, *p.get(1).unwrap_or(&0) as u32, *p.get(2).unwrap_or(&0) as u32);
        let n = (a << 16) | (b << 8) | c;
        s.push(B64[(n >> 18) as usize & 63] as char);
        s.push(B64[(n >> 12) as usize & 63] as char);
        s.push(if p.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if p.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    s
}

/// Pfad des Aufnahme-Helfers im App-Bundle; im Entwicklungsbaum daneben.
fn helfer_pfad() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let mut kandidaten = vec![];
    if let Some(macos) = exe.parent() {
        // Noki.app/Contents/MacOS/Noki -> Contents/Helpers/NokiSchirm.app/...
        if let Some(contents) = macos.parent() {
            kandidaten.push(
                contents
                    .join("Helpers/NokiSchirm.app/Contents/MacOS/nokischirm"),
            );
        }
        kandidaten.push(macos.join("nokischirm"));
    }
    kandidaten.push(std::path::PathBuf::from(
        "/Users/yilonglin/NOKI/.local/schirm/NokiSchirm.app/Contents/MacOS/nokischirm",
    ));
    kandidaten.push(std::path::PathBuf::from(
        "/Users/yilonglin/NOKI/desktop/schirm/nokischirm",
    ));
    kandidaten.into_iter().find(|p| p.is_file())
}

/// Was die eigene Anzeige des Helfers zuletzt bekommen soll - damit ein
/// neu gestarteter Helfer sofort denselben Stand hat.
struct Anzeige {
    rahmen: [i32; 4],
    desktop: Option<[i32; 4]>,
    desktop_anzeige: u32,
    label: String,
    navi: String,
    gross: bool,
    spaces: Vec<u64>,
    noki_space: u64,
}

static ANZEIGE: Mutex<Anzeige> = Mutex::new(Anzeige {
    rahmen: [0, 0, 0, 0],
    desktop: None,
    desktop_anzeige: 0,
    label: String::new(),
    navi: String::new(),
    gross: false,
    spaces: Vec::new(),
    noki_space: 0,
});

/// Ein Befehl an den Helfer - nur, wenn er laeuft. Sonst genuegt der
/// gemerkte Stand in ANZEIGE: der naechste Start bekommt ihn mit.
pub(crate) fn befehl(zeile: &str) {
    if let Ok(mut g) = LAUF.lock() {
        if let Some(l) = g.as_mut() {
            let _ = l.ein.write_all(format!("{zeile}\n").as_bytes());
            let _ = l.ein.flush();
        }
    }
}

fn stand_senden(l: &mut Lauf) {
    let Ok(a) = ANZEIGE.lock() else { return };
    let r = a.rahmen;
    let mut z = format!("rahmen {} {} {} {}\nmodus {}\n", r[0], r[1], r[2], r[3],
        if a.gross { "gross" } else { "kompakt" });
    if let Some(d) = a.desktop {
        z.push_str(&format!("desktop {} {} {} {} {}\n", d[0], d[1], d[2], d[3], a.desktop_anzeige));
    }
    if !a.label.is_empty() {
        z.push_str(&format!("label {}\n", a.label));
    }
    if !a.navi.is_empty() {
        z.push_str(&format!("navi {}\n", a.navi));
    }
    if !a.spaces.is_empty() {
        z.push_str(&format!("spaces {}\n", liste(&a.spaces)));
    }
    if a.noki_space != 0 {
        z.push_str(&format!("noki_space {}\n", a.noki_space));
    }
    let _ = l.ein.write_all(z.as_bytes());
    let _ = l.ein.flush();
}

/// The Noki Space the Miniatur shows (0 = unknown).
pub fn noki_space_id() -> u64 {
    ANZEIGE.lock().map(|a| a.noki_space).unwrap_or(0)
}

pub fn noki_space(sid: u64) {
    if let Ok(mut a) = ANZEIGE.lock() {
        if a.noki_space == sid {
            return;
        }
        a.noki_space = sid;
    }
    befehl(&format!("noki_space {sid}"));
}

fn liste<T: ToString>(v: &[T]) -> String {
    v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",")
}

/// Width of the COMPACT Miniatur last shown (points); 0 = never shown.
static KOMPAKT_BREITE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// Last compact Miniatur rect that was really shown (screen points,
/// top-left) - also while it is temporarily hidden, so layouts that must
/// not collide with it stay deterministic.
static KOMPAKT_LETZT: Mutex<[i32; 4]> = Mutex::new([0; 4]);

/// Compact Miniatur rect (screen points, top-left): current, else the last
/// one shown; w = 0 only if it was never shown.
pub fn kompakt_rahmen() -> [i32; 4] {
    let jetzt = ANZEIGE.lock().map(|a| a.rahmen).unwrap_or([0; 4]);
    if jetzt[2] >= 2 { return jetzt; }
    let letzt = KOMPAKT_LETZT.lock().map(|g| *g).unwrap_or([0; 4]);
    if letzt[2] >= 2 { return letzt; }
    // Not shown yet in THIS process (fresh start, or hidden via Shortcut 4
    // since start): the frame saved by an earlier run. Measured 2026-10-03:
    // without it Shortcut 9 laid out against "no Miniatur" (h=759 instead
    // of 627) and the Miniatur, once shown, covered the panel's bottom.
    kompakt_gespeichert().unwrap_or([0; 4])
}
fn kompakt_datei() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(std::path::Path::new(&home).join("Library/Application Support/com.noki.desktop/miniatur-kompakt.json"))
}
fn kompakt_gespeichert() -> Option<[i32; 4]> {
    let v: Vec<i32> = serde_json::from_str(&std::fs::read_to_string(kompakt_datei()?).ok()?).ok()?;
    (v.len() == 4 && v[2] >= 2 && v[3] >= 2).then(|| [v[0], v[1], v[2], v[3]])
}

/// Width of the compact Miniatur, also while it is hidden (last shown).
pub fn kompakt_breite() -> i32 {
    KOMPAKT_BREITE.load(std::sync::atomic::Ordering::Relaxed)
}

/// Platz der Miniatur in Bildschirmpunkten (oben links). w = 0 verbirgt.
pub fn rahmen(x: i32, y: i32, w: i32, h: i32) {
    if w >= 2 {
        KOMPAKT_BREITE.store(w, std::sync::atomic::Ordering::Relaxed);
        let neu = KOMPAKT_LETZT.lock().map(|mut g| { let n = *g != [x, y, w, h]; *g = [x, y, w, h]; n }).unwrap_or(false);
        let gross = ANZEIGE.lock().map(|a| a.gross).unwrap_or(false);
        if neu && !gross && kompakt_gespeichert() != Some([x, y, w, h]) {
            if let (Some(p), Ok(t)) = (kompakt_datei(), serde_json::to_string(&[x, y, w, h])) {
                let tmp = p.with_extension("json.tmp");
                if std::fs::write(&tmp, t).is_ok() { let _ = std::fs::rename(&tmp, &p); }
            }
        }
    }
    if let Ok(mut a) = ANZEIGE.lock() {
        // Verbergen wird immer zugestellt (idempotent im Helfer): ein
        // Helfer, der wegen Hover gross stehen blieb, bekam den zweiten
        // Shortcut-4-Druck sonst nie zu sehen.
        if a.rahmen == [x, y, w, h] && w >= 2 && h >= 2 {
            return;
        }
        a.rahmen = [x, y, w, h];
    }
    if w < 2 || h < 2 {
        // Die Ansicht verschwindet: keine Vollansicht und keine Tastatur,
        // die an etwas Unsichtbares geht.
        VOLL.store(false, std::sync::atomic::Ordering::SeqCst);
        crate::fern_tippen::interaction_beenden("ansicht_verborgen");
    }
    befehl(&format!("rahmen {x} {y} {w} {h}"));
}

pub fn desktop(x: i32, y: i32, w: i32, h: i32) {
    desktop_von(x, y, w, h, 0);
}

/// Wie `desktop`, mit der Anzeige-ID: der Helfer liest deren Grenzen nach
/// einer Bildschirm-Neuordnung selbst frisch nach.
pub fn desktop_von(x: i32, y: i32, w: i32, h: i32, anzeige: u32) {
    if let Ok(mut a) = ANZEIGE.lock() {
        a.desktop = Some([x, y, w, h]);
        if anzeige != 0 { a.desktop_anzeige = anzeige; }
    }
    befehl(&format!("desktop {x} {y} {w} {h} {anzeige}"));
}

/// Wo die native Ansicht WIRKLICH steht (oben-links-Punkte; kompakt, gross
/// oder voll). Nur der Helfer kennt das - er meldet jede Aenderung.
static IST_RAHMEN: Mutex<[f64; 4]> = Mutex::new([0.0; 4]);

/// Ein Menue der App-Leiste (Nokis eigenes) ist offen: Klicks darin sind Noki.
static LEISTENMENUE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn punkt_in_ansicht(x: f64, y: f64) -> bool {
    if LEISTENMENUE.load(std::sync::atomic::Ordering::SeqCst) { return true; }
    IST_RAHMEN.lock().map(|r| r[2] >= 2.0 && x >= r[0] && y >= r[1]
        && x < r[0] + r[2] && y < r[1] + r[3]).unwrap_or(false)
}

static VOLL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// "Noki Schreibtisch oeffnen": dieselbe native Ansicht, gross. Keine
/// zweite Darstellung - dieselben Ebenen, Stroeme und Fernbedienung.
pub fn voll(an: bool) {
    if !an { VOLL.store(false, std::sync::atomic::Ordering::SeqCst); }
    befehl(if an { "voll an" } else { "voll aus" });
}

pub fn voll_aktiv() -> bool {
    VOLL.load(std::sync::atomic::Ordering::SeqCst)
}

/// App-Leiste: registrierte Fenster (JSON), nur bei Aenderung gesendet.
static LEISTE_LETZT: Mutex<String> = Mutex::new(String::new());

pub fn leiste(json: &str) {
    if let Ok(mut l) = LEISTE_LETZT.lock() {
        if *l == json && laeuft() { return; }
        *l = json.to_string();
    }
    befehl(&format!("leiste {json}"));
}

/// Installierte Programme fuer das "+" der Leiste (aus Nokis App-Erkennung).
pub fn katalog(json: &str) {
    befehl(&format!("katalog {json}"));
}

/// Kurze Rueckmeldung ueber der Buehne (z. B. welches Fenster vorn ist).
/// Kurze Fenster-Identitaet im Fussband: [Symbol] Programm · Fenstertitel.
pub fn hinweis_fenster(pfad: &str, app: &str, titel: &str) {
    let rein = |t: &str| t.replace(['\t', '\n'], " ");
    befehl(&format!("hinweis_fenster {}\t{}\t{}", rein(pfad), rein(app), rein(titel)));
}

pub fn hinweis(text: &str) {
    befehl(&format!("hinweis {}", text.replace('\n', " ")));
}

/// Stapelung sofort neu lesen (nach einem AXRaise auf Noki Schreibtisch).
pub fn stapel() {
    befehl("stapel");
}

/// Exact window to the front of the CURRENT Space (after an explicit visit).
pub fn fenster_vorn_nativ(pid: i32, wid: i64) {
    befehl(&format!("fenstervorn {pid} {wid}"));
}

/// Consistency audit: the helper reports its exact current model.
pub fn zustand_anfordern() {
    befehl("zustand");
}

/// Vorderes Fenster auf Ebene der Komposition. macOS stapelt Fenster
/// inaktiver Programme je Programm: ein Hintergrundprogramm kann sein
/// Fenster nicht ueber die eines anderen heben (gemessen: YouTube-App hinter
/// Chrome trotz AXRaise). Der Nutzer sieht Noki Schreibtisch nur durch diese
/// Komposition - dort liegt das gewaehlte Fenster vorn, und Klicks folgen der
/// sichtbaren Reihenfolge. 0 = echte Stapelung gilt.
static VORN: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

pub fn vorn_setzen(wid: i64) {
    VORN.store(wid, std::sync::atomic::Ordering::SeqCst);
    befehl(&format!("vorn {wid}"));
}

/// Eingabe an dieses Fenster (Tippen): die Aufnahme sofort auf vollen Takt.
/// Hoechstens alle 300 ms ein Befehl.
/// Same as `lebhaft`, without its 300 ms rate limit (the input lane is
/// already spaced to >= 250 ms and must not lose its trailing bursts).
fn lebhaft_jetzt(wid: i64) { befehl(&format!("lebhaft {wid}")); }

pub fn lebhaft(wid: i64) {
    static ZULETZT: Mutex<Option<(i64, std::time::Instant)>> = Mutex::new(None);
    let senden = ZULETZT.lock().is_ok_and(|mut z| {
        let neu = z.is_none_or(|(w, t)| w != wid || t.elapsed() > std::time::Duration::from_millis(300));
        if neu { *z = Some((wid, std::time::Instant::now())); }
        neu
    });
    if senden { befehl(&format!("lebhaft {wid}")); }
}

pub fn vorn() -> i64 {
    VORN.load(std::sync::atomic::Ordering::SeqCst)
}

pub fn aktives_oder_vorderstes_fenster() -> Option<i64> {
    // Only a window the Miniatur really SHOWS. Space membership alone also
    // matches all-Spaces overlays (measured: "Cua Driver" 22548, full
    // screen, in front of every Space) - keys were routed into it.
    let gezeigt = aufgenommene();
    let v = vorn();
    if v > 0 && gezeigt.contains(&v) && ziel_erlaubt(v).is_some() {
        return Some(v);
    }
    let sid = noki_space_id();
    if sid != 0 {
        if let Some(first) = crate::cgs::fenster_reihe(sid).into_iter()
            .find(|w| gezeigt.contains(w) && ziel_erlaubt(*w).is_some()) {
            return Some(first);
        }
    }
    None
}

/// Leise Rueckmeldung: die Tastatur geht gerade an Noki Schreibtisch.
/// Focus ring around the verified remote text field (virtual coords).
/// Terminal-style block caret at the real insertion point (virtual coords).
pub fn einfuegemarke(r: Option<[f64; 4]>) {
    match r {
        Some(r) => befehl(&format!("caret {:.1} {:.1} {:.1} {:.1}", r[0], r[1], r[2], r[3])),
        None => befehl("caret aus"),
    }
}

pub fn fokusring(r: Option<[f64; 4]>) {
    match r {
        Some(r) => befehl(&format!("fokusring {:.0} {:.0} {:.0} {:.0}", r[0], r[1], r[2], r[3])),
        None => befehl("fokusring aus"),
    }
}

pub fn tippen_anzeige(an: bool) {
    befehl(if an { "tippen an" } else { "tippen aus" });
}

pub fn interaction_anzeige(an: bool) {
    befehl(if an { "interaction an" } else { "interaction aus" });
}

/// Ist die Ansicht gerade ueberhaupt da?
pub fn sichtbar() -> bool {
    laeuft() && ANZEIGE.lock().map(|a| a.rahmen[2] >= 2 && a.rahmen[3] >= 2).unwrap_or(false)
}

pub fn label(text: &str) {
    let t = text.replace('\n', " ");
    if let Ok(mut a) = ANZEIGE.lock() {
        a.label = t.clone();
    }
    befehl(&format!("label {t}"));
}

/// Beschriftung des Navigationsknopfes ("Zum Schreibtisch N").
/// Label + app bar of the NEXT target, sent right before its retarget: the
/// helper applies them in the same swap as the new picture (P0: the app bar
/// lagged the picture by up to seconds - it was refreshed separately).
pub fn ziel_meta(navi: &str, leiste_json: &str) {
    let t = navi.replace('\n', " ");
    if let Ok(mut a) = ANZEIGE.lock() { a.navi = t.clone(); }
    if let Ok(mut l) = LEISTE_LETZT.lock() { *l = leiste_json.to_string(); }
    let leiste: serde_json::Value = serde_json::from_str(leiste_json).unwrap_or(serde_json::json!([]));
    befehl(&format!("ziel_meta {}", serde_json::json!({ "navi": t, "leiste": leiste })));
}

pub fn navi(text: &str) {
    let t = text.replace('\n', " ");
    if let Ok(mut a) = ANZEIGE.lock() { a.navi = t.clone(); }
    befehl(&format!("navi {t}"));
}

pub fn modus(gross: bool) {
    if let Ok(mut a) = ANZEIGE.lock() {
        a.gross = gross;
    }
    befehl(if gross { "modus gross" } else { "modus kompakt" });
}

/// Auf welchen Spaces das Anzeigefenster Mitglied ist. Nie leer.
pub fn spaces(v: Vec<u64>) {
    if v.is_empty() {
        return;
    }
    if let Ok(mut a) = ANZEIGE.lock() {
        if a.spaces == v {
            return;
        }
        a.spaces = v.clone();
    }
    eprintln!("[VORSCHAU] Spaces: {}", liste(&v));
    befehl(&format!("spaces {}", liste(&v)));
}

/// Re-send even when the set is unchanged. A physical Desktop switch can
/// make WindowServer drop a panel membership transiently; the list is still
/// the same, but the helper must revalidate the actual window membership.
pub fn spaces_erzwingen(v: Vec<u64>) {
    if v.is_empty() { return; }
    if let Ok(mut a) = ANZEIGE.lock() { a.spaces = v.clone(); }
    befehl(&format!("spaces {}", liste(&v)));
}

/// Wo der Nutzer steht und ob links/rechts daneben Nokis Schreibtisch
/// liegt. Der Helfer blendet die klebende Miniatur damit schon beim
/// BEGINN einer Geste Richtung Noki aus.
/// Last `ort` Rust sent. The helper ALSO changes its place by itself (a
/// swipe gesture ending on Noki's Desktop -> `noki`); every helper
/// TRANSITION report clears this cache. Otherwise the next identical
/// "ort nutzer" was deduplicated away and the Miniatur stayed hidden after
/// fast swipes through the target (intermittent "Miniatur verschwindet").
static ORT_LETZT: Mutex<String> = Mutex::new(String::new());
/// Last transition state the helper reported (diagnostics / self-heal).
static HELFER_UEBERGANG: Mutex<String> = Mutex::new(String::new());
pub fn helfer_uebergang() -> String {
    HELFER_UEBERGANG.lock().map(|g| g.clone()).unwrap_or_default()
}
/// Forget the last-sent `ort` so the next one reaches the helper for sure.
pub fn ort_cache_leeren() {
    if let Ok(mut l) = ORT_LETZT.lock() { l.clear(); }
}
/// Last `ort` Rust sent (diagnostics).
pub fn ort_letzt() -> String {
    ORT_LETZT.lock().map(|g| g.clone()).unwrap_or_default()
}
/// Is the Miniatur really on screen right now (helper window ordered in on
/// the current Space, not transparent)? WindowServer truth, not our flags.
pub fn wirklich_sichtbar() -> bool {
    use std::ffi::c_void;
    type C = *const c_void;
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" { fn CGWindowListCopyWindowInfo(o: u32, w: u32) -> C; }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFArrayGetCount(a: C) -> isize;
        fn CFArrayGetValueAtIndex(a: C, i: isize) -> C;
        fn CFDictionaryGetValue(d: C, k: C) -> C;
        fn CFNumberGetValue(n: C, t: i32, v: *mut c_void) -> u8;
        fn CFStringCreateWithCString(a: C, s: *const i8, e: u32) -> C;
        fn CFRelease(o: C);
    }
    let pid = helfer_pid();
    if pid == 0 { return false; }
    unsafe {
        let key = |s: &str| { let c = std::ffi::CString::new(s).unwrap(); CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), 0x08000100) };
        let (kp, ka) = (key("kCGWindowOwnerPID"), key("kCGWindowAlpha"));
        let a = CGWindowListCopyWindowInfo(1, 0); // on-screen only
        let mut ja = false;
        if !a.is_null() {
            for i in 0..CFArrayGetCount(a) {
                let d = CFArrayGetValueAtIndex(a, i);
                let mut p: i64 = 0;
                let v = CFDictionaryGetValue(d, kp);
                if v.is_null() || CFNumberGetValue(v, 4, &mut p as *mut _ as *mut c_void) == 0 || p != pid { continue; }
                let mut al: f64 = 1.0;
                let va = CFDictionaryGetValue(d, ka);
                if !va.is_null() { CFNumberGetValue(va, 13, &mut al as *mut _ as *mut c_void); }
                if al > 0.05 { ja = true; break; }
            }
            CFRelease(a);
        }
        CFRelease(kp); CFRelease(ka);
        ja && IST_RAHMEN.lock().map(|r| r[2] >= 2.0).unwrap_or(false)
    }
}

pub fn ort(ort: &str, noki_links: bool, noki_rechts: bool) {
    let z = format!("ort {ort} {} {}", noki_links as u8, noki_rechts as u8);
    if let Ok(mut l) = ORT_LETZT.lock() {
        if *l == z && laeuft() {
            return;
        }
        *l = z.clone();
    }
    befehl(&z);
}

/// Same observation, but bypass the sender-side de-duplication.  The native
/// helper also observes Space notifications itself; after a hidden bridge
/// it may have changed to `noki` while Rust's last-sent cache still says
/// `nutzer`.  Physical-origin verification must therefore reassert reality.
pub fn ort_erzwingen(ort: &str, noki_links: bool, noki_rechts: bool) {
    befehl(&format!("ort {ort} {} {}", noki_links as u8, noki_rechts as u8));
}

static ABGEDECKT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Uebergangsflaeche: die echte Komposition bildschirmfuellend und klebend.
/// Wartet auf die Bestaetigung des Helfers (Ende der 160-ms-Animation),
/// hoechstens 400 ms. false = kein Helfer, der Besuch laeuft ohne Maske.
pub fn abdecken() -> bool {
    use std::sync::atomic::Ordering;
    if !laeuft() {
        return false;
    }
    ABGEDECKT.store(false, Ordering::SeqCst);
    befehl("abdecken");
    for _ in 0..40 {
        if ABGEDECKT.load(Ordering::SeqCst) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}

/// Flaeche ueber dem erreichten Schreibtisch ausblenden und die normale
/// Mitgliedschaft (ohne Nokis Schreibtisch) wiederherstellen.
pub fn aufdecken(spaces_ohne_noki: Vec<u64>) {
    if let Ok(mut a) = ANZEIGE.lock() {
        if !spaces_ohne_noki.is_empty() {
            a.spaces = spaces_ohne_noki.clone();
        }
    }
    befehl(&format!("aufdecken {}", liste(&spaces_ohne_noki)));
}

/// HARD RULE at the one choke point every window set passes: a target
/// Desktop shows ONLY windows that are members of exactly its Space
/// (resolved from the target's persistent UUID). Measured 2026-09-29: the
/// Spotify-only Desktop 3 was published with VS Code (Desktop 2) in it.
/// Foreign windows are dropped here, whichever caller sent them; open real
/// context menus of the target's apps stay (they are part of it).
#[cfg(target_os = "macos")]
fn fenster_des_ziels_erzwingen(fenster: &mut Vec<i64>, ziel_uuid: &str) {
    if ziel_uuid.is_empty() || ziel_uuid == "virtual-display" {
        return;
    }
    let Some(sid) = crate::cgs::published_topology()
        .or_else(crate::cgs::desktops)
        .and_then(|(_, l)| l.into_iter().find(|s| s.uuid == ziel_uuid).map(|s| s.id))
    else {
        return;
    };
    let menues = menues();
    let vorher = fenster.len();
    fenster.retain(|w| {
        if menues.contains(w) {
            return true;
        }
        let sp = crate::cgs::spaces_des_fensters(*w).unwrap_or_default();
        let gehoert = sp.contains(&sid);
        if !gehoert {
            let bt = std::backtrace::Backtrace::force_capture().to_string();
            let wer: Vec<&str> = bt.lines().filter(|l| l.contains("app_lib::") && !l.contains("fenster_des_ziels"))
                .take(3).map(|l| l.trim()).collect();
            crate::virtual_workspace::trace(&format!(
                "[INVENTORY] DROP foreign wid={w} spaces={sp:?} target_space={sid} target={ziel_uuid} caller={wer:?}"
            ));
        }
        gehoert
    });
    if fenster.len() != vorher {
        crate::virtual_workspace::trace(&format!("[INVENTORY] target_space={sid} kept={fenster:?}"));
    }
    // Audit: every CHANGED window set with its sender (root-cause trail).
    static LETZTER: std::sync::Mutex<(u64, Vec<i64>)> = std::sync::Mutex::new((0, Vec::new()));
    if let Ok(mut g) = LETZTER.lock() {
        if g.0 != sid || g.1 != *fenster {
            *g = (sid, fenster.clone());
            let bt = std::backtrace::Backtrace::force_capture().to_string();
            let wer: Vec<String> = bt.lines().filter(|l| l.contains("app_lib::") && !l.contains("fenster_des_ziels") && !l.contains("vorschau::setzen"))
                .take(2).map(|l| l.trim().chars().take(90).collect()).collect();
            crate::virtual_workspace::trace(&format!(
                "[INVENTORY] set target_space={sid} epoch={} ids={fenster:?} caller={wer:?}",
                crate::target_epoch_jetzt()));
        }
    }
}
#[cfg(not(target_os = "macos"))]
fn fenster_des_ziels_erzwingen(_: &mut Vec<i64>, _: &str) {}

/// Startet den Helfer, falls er nicht laeuft, und richtet ihn auf genau
/// diese Fenster aus. Ein leeres `fenster` haelt keinen Strom offen - die
/// Miniatur zeigt dann den leeren Schreibtisch.
pub fn setzen(
    app: &tauri::AppHandle,
    mut fenster: Vec<i64>,
    _breite: u32,
    _fps: u32,
    ziel_uuid: String,
    nutzer_uuid: String,
) -> serde_json::Value {
    // WindowServer liefert dieselbe Fenstermenge nicht in stabiler
    // Reihenfolge. Die Stapelung kommt ohnehin separat aus `stapel()`;
    // hier ist nur die Zielmenge gemeint. Ohne Kanonisierung sah derselbe
    // Schreibtisch fortlaufend wie ein neues Retarget aus und sperrte in
    // diesen kurzen Phasen berechtigte Klicks und Rollbewegungen.
    fenster.sort_unstable();
    fenster.dedup();
    fenster_des_ziels_erzwingen(&mut fenster, &ziel_uuid);
    let mut g = match LAUF.lock() {
        Ok(g) => g,
        Err(_) => return serde_json::json!({ "ok": false, "grund": "belegt" }),
    };
    if g.is_none() {
        let Some(bin) = helfer_pfad() else {
            return serde_json::json!({ "ok": false, "grund": "Aufnahme-Helfer fehlt." });
        };
        // Das erste READY muss bereits zu genau diesem Ziel gehoeren. Ohne
        // Startargument meldete der Helfer zuerst READY 0 und konnte damit
        // ein unmittelbar folgendes Retarget zu frueh bestaetigen.
        let mut kind = match Command::new(&bin)
            .arg("--windows")
            .arg(liste(&fenster))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
        {
            Ok(k) => k,
            Err(e) => return serde_json::json!({ "ok": false, "grund": format!("{e}") }),
        };
        let aus = kind.stdout.take();
        let ein = match kind.stdin.take() {
            Some(e) => e,
            None => return serde_json::json!({ "ok": false, "grund": "kein stdin" }),
        };
        let kind_pid = kind.id();
        HELFER_LETZTE_NACHRICHT_MS.store(jetzt_ms(), std::sync::atomic::Ordering::Release);
        PRAESENTATION_HEARTBEAT_MS.store(jetzt_ms(), std::sync::atomic::Ordering::Release);
        if let Some(aus) = aus {
            let h = app.clone();
            std::thread::spawn(move || leser(h, aus, kind_pid));
        }
        RETARGET_AKTIV.store(true, std::sync::atomic::Ordering::SeqCst);
        let mut l = Lauf {
            kind,
            ein,
            fenster: vec![],
            wartend: Some(fenster.clone()),
            veroeffentlicht_uuid: String::new(),
            wartend_uuid: Some(ziel_uuid.clone()),
            wartend_nutzer_uuid: nutzer_uuid.clone(),
            pausiert: false,
        };
        if abnahme() {
            eprintln!(
                "[ACCEPT] RETARGET request oldPublishedUUID=- requestedTargetUUID={} currentUserUUID={}",
                ziel_uuid, nutzer_uuid
            );
        }
        stand_senden(&mut l);
        *g = Some(l);
        // Ab jetzt zeigt der Helfer die Miniatur; die Oberflaeche behaelt
        // nur Zustand und Masse.
        let _ = app.emit("noki://vorschau_nativ", serde_json::json!({ "an": true }));
    }
    let l = g.as_mut().unwrap();
    let liste_s = liste(&fenster);
    let kraft = !ziel_uuid.is_empty() && (ziel_uuid != l.veroeffentlicht_uuid || Some(&ziel_uuid) != l.wartend_uuid.as_ref());
    if l.wartend.as_ref() == Some(&fenster) && !kraft {
        return serde_json::json!({ "ok": true, "aufnahme": l.fenster.len(), "fenster": l.fenster, "retarget": true });
    }
    if l.wartend.is_some() && !kraft && l.wartend.as_ref() == Some(&fenster) {
        return serde_json::json!({ "ok": false, "grund": "retarget_busy", "fenster": l.fenster });
    }
    let muss_senden = kraft
        || l.pausiert
        || fenster != l.fenster;
    if !muss_senden {
        return serde_json::json!({ "ok": true, "aufnahme": l.fenster.len(), "fenster": l.fenster });
    }
    l.pausiert = false;
    if liste_s != liste(&l.fenster) || fenster.is_empty() {
        eprintln!("[VORSCHAU] Aufnahme: {liste_s}");
    }
    l.wartend = Some(fenster.clone());
    l.wartend_uuid = Some(ziel_uuid.clone());
    l.wartend_nutzer_uuid = nutzer_uuid.clone();
    if abnahme() {
        eprintln!(
            "[ACCEPT] RETARGET request oldPublishedUUID={} requestedTargetUUID={} currentUserUUID={}",
            if l.veroeffentlicht_uuid.is_empty() { "-" } else { &l.veroeffentlicht_uuid },
            ziel_uuid, nutzer_uuid
        );
    }
    RETARGET_AKTIV.store(true, std::sync::atomic::Ordering::SeqCst);
    // A UUID change is a retarget even when both Desktops happen to have
    // the same ID set (especially two empty Desktops).  Publishing the UUID
    // immediately used to pair the old composition with the new footer.
    let kraft = !ziel_uuid.is_empty() && ziel_uuid != l.veroeffentlicht_uuid;
    let _ = l.ein.write_all(format!("{} {liste_s}\n", if kraft { "set!" } else { "set" }).as_bytes());
    let _ = l.ein.flush();
    // Die abgeglichene Liste geht zurueck: sie ist die einzige Wahrheit
    // darueber, WAS auf Nokis Schreibtisch steht. Ein abgerissener
    // Aufnahmestrom ist es ausdruecklich nicht (Abschnitt 4).
    serde_json::json!({ "ok": true, "aufnahme": l.fenster.len(), "fenster": l.fenster, "retarget": true })
}

/// Haelt die Aufnahme an. Der Helfer bleibt am Leben (seine Anzeige
/// behaelt das letzte Bild), damit die Miniatur beim Wiederkommen sofort
/// steht; Verbergen ist Sache des Rahmens. Endet Noki, endet er mit.
pub fn stoppen() -> serde_json::Value {
    if let Ok(mut g) = LAUF.lock() {
        if let Some(l) = g.as_mut() {
            l.pausiert = true;
            l.fenster.clear();
        }
    }
    befehl("pause");
    serde_json::json!({ "ok": true, "aufnahme": 0 })
}

pub fn fortsetzen() {
    if let Ok(mut g) = LAUF.lock() {
        if let Some(l) = g.as_mut() {
            l.pausiert = false;
        }
    }
    befehl("resume");
}

/// Prozesskennung des Helfers (0 = laeuft nicht) - damit seine eigene
/// Anzeige nie als "fremder Schreibtisch sichtbar" gilt.
pub fn helfer_pid() -> i64 {
    LAUF.lock().ok().and_then(|g| g.as_ref().map(|l| l.kind.id() as i64)).unwrap_or(0)
}

/// UUID of the Desktop whose composition the helper has atomically
/// published (empty until the first READY).
pub fn veroeffentlichte_uuid() -> String {
    LAUF.lock().ok().and_then(|g| g.as_ref().map(|l| l.veroeffentlicht_uuid.clone())).unwrap_or_default()
}

pub fn laeuft() -> bool {
    LAUF.lock().map(|g| g.is_some()).unwrap_or(false)
}

/// Validate the helper process and its real main-thread pulse.  A soft stream
/// rebuild is attempted first.  Only a helper whose presentation loop stayed
/// silent for 12 seconds is replaced.  The inventory owner immediately calls
/// `setzen` with the exact real window set afterwards.
/// Schlaeft das Hauptdisplay (auch waehrend DarkWake)?
fn display_schlaeft() -> bool {
    #[cfg(target_os = "macos")]
    {
        #[link(name = "CoreGraphics", kind = "framework")]
        extern "C" {
            fn CGMainDisplayID() -> u32;
            fn CGDisplayIsAsleep(d: u32) -> u32;
        }
        return unsafe { CGDisplayIsAsleep(CGMainDisplayID()) } != 0;
    }
    #[allow(unreachable_code)]
    false
}
static LETZTE_PRUEFUNG_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static WACH_AB_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn helfer_gesundheit_pruefen() -> bool {
    let jetzt = jetzt_ms();
    // Ein alter Puls nach Ruhezustand/DarkWake ist kein Haenger: alle
    // Prozesse standen still. Gemessen (2026-10-03, pmset-Log): genau dann
    // ersetzte dieser Waechter einen gesunden Helfer; der neue Helfer
    // entstand bei schlafendem Display und WindowServer gab seiner Tafel
    // den Aufwach-Zoom 0.902 um die Schirmmitte mit (Miniatur rechts/oben).
    // Deshalb: bei schlafendem Display nie ersetzen, und nach einer Luecke
    // des Waechters selbst gilt der Aufwachzeitpunkt als frischer Puls.
    let vorher = LETZTE_PRUEFUNG_MS.swap(jetzt, std::sync::atomic::Ordering::AcqRel);
    if display_schlaeft() {
        WACH_AB_MS.store(jetzt, std::sync::atomic::Ordering::Release);
        return true;
    }
    if vorher != 0 && jetzt.saturating_sub(vorher) > 15_000 {
        WACH_AB_MS.store(jetzt, std::sync::atomic::Ordering::Release);
        crate::virtual_workspace::trace(&format!(
            "[RECOVERY] subsystem=PREVIEW_HELPER action=wake_grace gap_ms={}", jetzt.saturating_sub(vorher)));
    }
    let mut g = match LAUF.lock() { Ok(g) => g, Err(e) => e.into_inner() };
    let Some(l) = g.as_mut() else { return false };
    match l.kind.try_wait() {
        Ok(Some(status)) => {
            let pid = l.kind.id();
            eprintln!("[RECOVERY] subsystem=PREVIEW_HELPER pid={pid} exited={status}");
            *g = None;
            RETARGET_AKTIV.store(false, std::sync::atomic::Ordering::SeqCst);
            return false;
        }
        Err(e) => {
            eprintln!("[RECOVERY] subsystem=PREVIEW_HELPER status_error={e}");
        }
        Ok(None) => {}
    }
    let puls = PRAESENTATION_HEARTBEAT_MS.load(std::sync::atomic::Ordering::Acquire)
        .max(WACH_AB_MS.load(std::sync::atomic::Ordering::Acquire));
    let alter = jetzt.saturating_sub(puls);
    if alter > 5_000
        && HELFER_SOFT_RECOVERY_FOR_HEARTBEAT.swap(puls, std::sync::atomic::Ordering::AcqRel) != puls
    {
        let _ = l.ein.write_all(b"neustart\n");
        let _ = l.ein.flush();
        eprintln!("[RECOVERY] subsystem=CAPTURE_PIPELINE action=soft_restart pulse_age_ms={alter}");
    }
    if alter <= 12_000 { return true; }

    let pid = l.kind.id();
    let _ = l.kind.kill();
    let _ = l.kind.wait();
    *g = None;
    drop(g);
    RETARGET_AKTIV.store(false, std::sync::atomic::Ordering::SeqCst);
    HELFER_RECOVERIES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    bad_event_melden("PREVIEW_PRESENTATION", &format!("pulse_age_ms={alter} pid={pid}"));
    crate::virtual_workspace::trace(&format!(
        "[RECOVERY] subsystem=PREVIEW_HELPER action=replace pid={pid} pulse_age_ms={alter}"));
    false
}

/// Welche Fenster gerade wirklich aufgenommen werden.
///
/// Das ist die dritte Stufe der Kette aus Abschnitt 1 - getrennt von
/// "existiert" und "gehoert Noki". Ohne diese Auskunft liesse sich nicht
/// sagen, ob ein fehlendes Bild an der Auswahl oder an der Aufnahme liegt.
pub fn aufgenommene() -> Vec<i64> {
    LAUF.lock()
        .ok()
        .and_then(|g| g.as_ref().map(|l| l.fenster.clone()))
        .unwrap_or_default()
}

/// Liest das Protokoll des Helfers: Kopfzeile, dann genau so viele Bytes.
/// DARF dieses Fenster ferngesteuert werden? Gibt die Prozessnummer zurueck.
///
/// Die Grenze ist absichtlich eng und stuetzt sich nur auf Dinge, die
/// nachweisbar sind:
///   * das Fenster wird GERADE aufgenommen - es steht also in der Miniatur,
///     und die Aufnahmeliste enthaelt ausschliesslich Fenster von Nokis
///     Schreibtisch, nie Nokis eigene Flaechen und keine Helferfenster;
///   * es ist ein gewoehnliches Fenster (Ebene 0), also keine Systemflaeche,
///     kein Menue, kein Overlay;
///   * es lebt noch und hat eine Prozessnummer.
///
/// Alles andere wird abgelehnt - lieber keine Wirkung als eine Wirkung an
/// der falschen Stelle.
fn ziel_erlaubt(wid: i64) -> Option<i32> {
    if wid <= 0 {
        return None;
    }
    let noki_space = noki_space_id();
    let auf_space = noki_space != 0 && crate::cgs::spaces_des_fensters(wid)
        .is_some_and(|sp| sp.contains(&noki_space));
    if !auf_space && !aufgenommene().contains(&wid) {
        return None;
    }
    // Ein offenes Menue eines Noki-Fensters ist Teil der Oberflaeche.
    if let Some(pid) = menue_pid(wid) { return Some(pid); }
    fenster_prozess(wid)
}

/// Prozessnummer und Ebene eines Fensters - direkt aus der Fensterliste.
#[cfg(target_os = "macos")]
pub(crate) fn fenster_prozess(wid: i64) -> Option<i32> {
    use std::ffi::c_void;
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGWindowListCopyWindowInfo(o: u32, w: u32) -> *const c_void;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFArrayGetCount(a: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(a: *const c_void, i: isize) -> *const c_void;
        fn CFDictionaryGetValue(d: *const c_void, k: *const c_void) -> *const c_void;
        fn CFStringCreateWithCString(a: *const c_void, s: *const i8, e: u32) -> *const c_void;
        fn CFNumberGetValue(n: *const c_void, t: i32, v: *mut i64) -> bool;
        fn CFRelease(o: *const c_void);
    }
    unsafe {
        let schl = |n: &str| {
            let c = std::ffi::CString::new(n).unwrap();
            CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), 0x0800_0100)
        };
        let zahl = |v: *const c_void| -> i64 {
            let mut o = 0i64;
            if !v.is_null() && CFNumberGetValue(v, 4, &mut o) { o } else { -1 }
        };
        // kCGWindowListOptionAll = 0
        let liste = CGWindowListCopyWindowInfo(0, 0);
        if liste.is_null() {
            return None;
        }
        let (k_num, k_pid, k_lay) = (schl("kCGWindowNumber"), schl("kCGWindowOwnerPID"), schl("kCGWindowLayer"));
        let mut out = None;
        for i in 0..CFArrayGetCount(liste) {
            let w = CFArrayGetValueAtIndex(liste, i);
            if w.is_null() || zahl(CFDictionaryGetValue(w, k_num)) != wid {
                continue;
            }
            if zahl(CFDictionaryGetValue(w, k_lay)) != 0 {
                break; // keine gewoehnliche Fensterebene: nicht bedienbar
            }
            let pid = zahl(CFDictionaryGetValue(w, k_pid));
            if pid > 0 {
                out = Some(pid as i32);
            }
            break;
        }
        for k in [k_num, k_pid, k_lay] {
            CFRelease(k);
        }
        CFRelease(liste);
        out
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn fenster_prozess(_: i64) -> Option<i32> { None }

/// ---------------------------------------------------------------------
///  DIE EINE FERNBEDIENUNGS-STELLE.
///
///  Frueher lief ein Klick in der Miniatur direkt im Leser des Helfers -
///  demselben Faden, der die Bilder abholt. Jeder Klick wartete dort einen
///  kompletten Schreibtisch-Hin-und-Rueckweg ab; die Bilder stauten sich,
///  der Helfer blockierte beim Schreiben, die Miniatur stand. Rollen startete
///  je Rastergruppe einen eigenen Faden, die sich an der Sperre anstellten -
///  eine unbegrenzte Schlange von Schreibtischwechseln.
///
///  Jetzt:
///    * der Leser legt nur ab und kehrt SOFORT zurueck;
///    * EIN Arbeiter fuehrt hoechstens EINEN Vorgang zugleich aus
///      (LEER -> VORBEREITEN -> AKTIV -> RUECKWEG -> NACHLAUF -> LEER);
///    * waehrend er arbeitet, wartet hoechstens EIN Klick (der neueste),
///      und alle Rollbewegungen werden zu EINER Summe zusammengelegt;
///    * jeder Ausgang - Erfolg, Fehler, Panik - raeumt auf: Zustand LEER,
///      Helfer bekommt `fern aus` und prueft den Hover gegen die echte
///      Zeigerlage.
/// ---------------------------------------------------------------------
#[derive(Clone, Copy, Debug)]
struct FernKlick {
    pid: i32,
    wid: i64,
    x: f64,
    y: f64,
    klicks: i64,
    seq: u64,
}

#[derive(Clone, Copy, Debug)]
struct FernRad {
    pid: i32,
    wid: i64,
    x: f64,
    y: f64,
    dx: i32,
    dy: i32,
}

/// Eine laufende Zieh-Geste (Auswahl). Nur der neueste Punkt zaehlt.
#[derive(Clone, Copy, Debug)]
struct FernZiehen {
    pid: i32,
    wid: i64,
    x: f64,
    y: f64,
    /// Beginn einer neuen Geste (Start-Punkt = x/y).
    anfang: bool,
    ende: bool,
    /// 0 = content/title auto classification; resize mask is
    /// left=1, right=2, top=4, bottom=8.
    art: i64,
}

#[derive(Default)]
struct FernArbeit {
    ziehen: Option<FernZiehen>,
    /// Exactly one pending click transaction.  It may contain several
    /// deliberate releases for the same target process, so a slow hidden
    /// bridge cannot collapse Count 1..10 into only its newest release.
    klick: Option<Vec<FernKlick>>,
    rad: Option<FernRad>,
    rad_seit: Option<std::time::Instant>,
}

impl FernArbeit {
    /// Keep one pending transaction, but preserve every deliberate release
    /// inside it.  The worker can deliver the whole batch during one safe
    /// target activation instead of starting one Space round-trip per click.
    fn klick_ablegen(&mut self, k: FernKlick) -> bool {
        match self.klick.as_mut() {
            Some(v) if v.first().is_some_and(|a| a.pid == k.pid) => {
                v.push(k);
                true
            }
            Some(_) => false, // fail closed across processes; never retarget a queued click
            None => {
                self.klick = Some(vec![k]);
                false
            }
        }
    }

    /// Rollbewegung aufaddieren; die Stelle ist die zuletzt gemeldete.
    fn rad_ablegen(&mut self, r: FernRad) {
        match self.rad.as_mut() {
            Some(a) if a.pid == r.pid => {
                a.dx = a.dx.saturating_add(r.dx);
                a.dy = a.dy.saturating_add(r.dy);
                a.x = r.x;
                a.y = r.y;
            }
            _ => {
                self.rad = Some(r);
                self.rad_seit = Some(std::time::Instant::now());
            }
        }
        if self.rad_seit.is_none() {
            self.rad_seit = Some(std::time::Instant::now());
        }
    }

    fn leer(&self) -> bool {
        self.klick.is_none() && self.rad.is_none() && self.ziehen.is_none()
    }

    /// Anfang und Ende gehen nie verloren; dazwischen zaehlt nur der neueste Punkt.
    fn ziehen_ablegen(&mut self, z: FernZiehen) {
        match self.ziehen.as_mut() {
            Some(alt) if !z.anfang && alt.wid == z.wid => {
                alt.x = z.x; alt.y = z.y; alt.ende |= z.ende;
                if alt.art == 0 { alt.art = z.art; }
            }
            _ => self.ziehen = Some(z),
        }
    }

    /// Alles, was ansteht, fuer EINEN Vorgang entnehmen.
    fn entnehmen(&mut self) -> (Option<Vec<FernKlick>>, Option<FernRad>) {
        self.rad_seit = None;
        let rad = self.rad.take().filter(|r| r.dx != 0 || r.dy != 0);
        (self.klick.take(), rad)
    }
}

/// Phasen, nur fuer Auskunft und Protokoll.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum FernPhase {
    Leer = 0,
    Vorbereiten = 1,
    Aktiv = 2,
    Rueckweg = 3,
    Nachlauf = 4,
}

static FERN_ARBEIT: Mutex<FernArbeit> = Mutex::new(FernArbeit { ziehen: None, klick: None, rad: None, rad_seit: None });
static FERN_WECKER: std::sync::Condvar = std::sync::Condvar::new();
static FERN_PHASE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
static FERN_ARBEITER: std::sync::OnceLock<()> = std::sync::OnceLock::new();
/// Click-lane generation: the supervisor abandons a worker stuck inside one
/// operation and starts a fresh one; the stale thread exits on return.
static FERN_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static FERN_TIMEOUTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
// Scroll has its own bounded, coalescing lane. A slow AX tree must never
// keep clicks (and therefore acquisition of a typing target) behind it.
static RAD_ARBEIT: Mutex<FernArbeit> = Mutex::new(FernArbeit { ziehen: None, klick: None, rad: None, rad_seit: None });
static RAD_WECKER: std::sync::Condvar = std::sync::Condvar::new();
static RAD_PHASE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
static RAD_ARBEITER: std::sync::OnceLock<()> = std::sync::OnceLock::new();
static RAD_WATCHDOG: std::sync::OnceLock<()> = std::sync::OnceLock::new();
static RAD_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
static RAD_AKTIV_WID: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
static RAD_ISOLIERT_WID: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
static RAD_ISOLIERT_BIS_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static FERN_AKTIV: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static FERN_LETZTER_HEARTBEAT_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static RAD_LETZTER_HEARTBEAT_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static FERN_ABGESCHLOSSEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static RAD_ABGESCHLOSSEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static RAD_TIMEOUTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Rollen wird so lange gesammelt, bevor es EINEN Vorgang bekommt.
const RAD_SAMMELN_MS: u64 = 140;

pub(crate) fn phase(p: FernPhase) {
    FERN_PHASE.store(p as u8, std::sync::atomic::Ordering::SeqCst);
}

/// Laeuft gerade ein Fernvorgang? (Der Klick-daneben-Beobachter fragt das:
/// Nokis eigener Klick am echten Fenster ist kein Klick des Nutzers.)
/// Noki Browser: viewport top (global y) per window, cached.
static BROWSER_OBEN: Mutex<Vec<(i64, f64, [f64; 4])>> = Mutex::new(Vec::new());

/// How often the emergency return guard had to act (must stay 0).
pub static GUARD_AUSLOESUNGEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// PID of the frontmost application (only a query - no shield involved).
fn vorderstes_programm() -> i32 { crate::blende::vorn_pid() }

/// TEMPORARY Space-switch audit: epoch ms of the last Miniatur content
/// interaction (click/scroll/typed key).
static LETZTE_FERN_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub fn fern_markieren() {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64).unwrap_or(0);
    LETZTE_FERN_MS.store(ms, std::sync::atomic::Ordering::SeqCst);
}
pub fn letzte_fern_ms() -> u64 { LETZTE_FERN_MS.load(std::sync::atomic::Ordering::SeqCst) }

pub fn fern_beschaeftigt() -> bool {
    FERN_PHASE.load(std::sync::atomic::Ordering::SeqCst) != FernPhase::Leer as u8
        || RAD_PHASE.load(std::sync::atomic::Ordering::SeqCst) != FernPhase::Leer as u8
}

static FIRST_BAD_EVENT: Mutex<Option<(String, u64)>> = Mutex::new(None);

pub fn bad_event_melden(subsystem: &str, details: &str) {
    if let Ok(mut g) = FIRST_BAD_EVENT.lock() {
        if g.is_none() {
            let ms = jetzt_ms();
            eprintln!("[FIRST_BAD_EVENT] subsystem={subsystem} details={details} ms={ms}");
            *g = Some((format!("subsystem={subsystem} details={details}"), ms));
        }
    }
}

pub fn first_bad_event() -> Option<(String, u64)> {
    FIRST_BAD_EVENT.lock().ok().and_then(|g| g.clone())
}

pub fn interaction_health() -> serde_json::Value {
    let f = arbeit();
    let r = rad_arbeit();
    let helper_ok = LAUF.lock().is_ok_and(|g| g.as_ref().is_some());
    let aufnahme_len = aufgenommene().len();
    serde_json::json!({
        "first_bad_event": first_bad_event().map(|(s, t)| serde_json::json!({ "event": s, "timestamp": t })),
        "PREVIEW_VISUAL": {
            "lastHeartbeat": PRAESENTATION_HEARTBEAT_MS.load(std::sync::atomic::Ordering::Acquire),
            "alive": helper_ok,
            "failureCount": HELFER_RECOVERIES.load(std::sync::atomic::Ordering::Relaxed),
        },
        "CAPTURE_PIPELINE": {
            "lastHeartbeat": CAPTURE_HEARTBEAT_MS.load(std::sync::atomic::Ordering::Acquire),
            "queueDepth": 0,
            "activeStreams": aufnahme_len,
            "retargetActive": retarget_aktiv(),
        },
        "WINDOW_INVENTORY": {
            "lastHeartbeat": HELFER_LETZTE_NACHRICHT_MS.load(std::sync::atomic::Ordering::Acquire),
            "capturedCount": aufnahme_len,
        },
        "COMPOSITOR": {
            "lastHeartbeat": PRAESENTATION_HEARTBEAT_MS.load(std::sync::atomic::Ordering::Acquire),
            "publishedGeneration": COMPOSITOR_GENERATION.load(std::sync::atomic::Ordering::Acquire),
            "frontWid": COMPOSITOR_FRONT_WID.load(std::sync::atomic::Ordering::Acquire),
            "frontOverrideWid": vorn(),
        },
        "CLICK_ROUTER": {
            "lastHeartbeat": FERN_LETZTER_HEARTBEAT_MS.load(std::sync::atomic::Ordering::Relaxed),
            "queueDepth": usize::from(f.klick.is_some()) + usize::from(f.ziehen.is_some()),
            "currentOperation": FERN_PHASE.load(std::sync::atomic::Ordering::Relaxed),
            "completedCount": FERN_ABGESCHLOSSEN.load(std::sync::atomic::Ordering::Relaxed),
            "timeoutCount": FERN_TIMEOUTS.load(std::sync::atomic::Ordering::Relaxed),
            "generation": FERN_GENERATION.load(std::sync::atomic::Ordering::Relaxed),
        },
        "SCROLL_ROUTER": {
            "lastHeartbeat": RAD_LETZTER_HEARTBEAT_MS.load(std::sync::atomic::Ordering::Relaxed),
            "queueDepth": usize::from(r.rad.is_some()),
            "currentOperation": RAD_PHASE.load(std::sync::atomic::Ordering::Relaxed),
            "completedCount": RAD_ABGESCHLOSSEN.load(std::sync::atomic::Ordering::Relaxed),
            "timeoutCount": RAD_TIMEOUTS.load(std::sync::atomic::Ordering::Relaxed),
        },
        "TYPING_ROUTER": {
            "lastHeartbeat": crate::fern_tippen::letzter_heartbeat_ms(),
            "active": crate::fern_tippen::aktiv(),
            "interactionActive": crate::fern_tippen::interaction_aktiv(),
        },
        "SHORTCUT_ROUTER": crate::shortcut_health(),
        "BROWSER_QUEUE": crate::noki_browser::health(),
        "GENERIC_AX_QUEUE": {
            "lastHeartbeat": FERN_LETZTER_HEARTBEAT_MS.load(std::sync::atomic::Ordering::Relaxed),
            "queueDepth": usize::from(f.klick.is_some()),
        },
        "FRONTEND_EVENT_LOOP": {
            "lastHeartbeat": crate::UI_PULS_MS.load(std::sync::atomic::Ordering::Relaxed),
            "ageMs": jetzt_ms().saturating_sub(crate::UI_PULS_MS.load(std::sync::atomic::Ordering::Relaxed)),
        },
        "MAIN_THREAD": {
            "lastHeartbeat": crate::MAIN_PULS_MS.load(std::sync::atomic::Ordering::Relaxed),
            "ageMs": jetzt_ms().saturating_sub(crate::MAIN_PULS_MS.load(std::sync::atomic::Ordering::Relaxed)),
            "maxLagMs": crate::MAIN_LAG_MAX_MS.load(std::sync::atomic::Ordering::Relaxed),
        },
        "BACKEND_IPC": {
            "lastHeartbeat": HELFER_LETZTE_NACHRICHT_MS.load(std::sync::atomic::Ordering::Acquire),
            "healthy": helper_ok,
        },
    })
}

fn arbeit<'a>() -> std::sync::MutexGuard<'a, FernArbeit> {
    // Auch eine vergiftete Sperre bleibt benutzbar - nie bis zum Neustart tot.
    FERN_ARBEIT.lock().unwrap_or_else(|e| e.into_inner())
}

fn rad_arbeit<'a>() -> std::sync::MutexGuard<'a, FernArbeit> {
    RAD_ARBEIT.lock().unwrap_or_else(|e| e.into_inner())
}

fn jetzt_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// The helper's remote-input presentation is shared by independent lanes.
/// Only the last lane leaving may release it.
struct FernAktivGuard;
impl FernAktivGuard {
    fn neu() -> Self {
        if FERN_AKTIV.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            befehl("fern an");
        }
        Self
    }
}
impl Drop for FernAktivGuard {
    fn drop(&mut self) {
        if FERN_AKTIV.fetch_sub(1, std::sync::atomic::Ordering::SeqCst) == 1 {
            befehl("fern aus");
        }
    }
}

/// Last explicit Noki operation per process (click/drag). A new window of
/// that process appearing on the virtual display shortly after is caused by
/// Noki (dialog, child window) - see `crate::fremde_fenster_zurueckgeben`.
static NOKI_BEDIENT: Mutex<Vec<(i32, std::time::Instant)>> = Mutex::new(Vec::new());
fn bedient_merken(pid: i32, wid: i64) {
    crate::kind_fenster::transaktion(pid, wid, crate::fenster_rahmen(wid));
    if let Ok(mut g) = NOKI_BEDIENT.lock() {
        g.retain(|e| e.1.elapsed() < std::time::Duration::from_secs(60) && e.0 != pid);
        g.push((pid, std::time::Instant::now()));
    }
}
pub fn noki_bedient_seit(pid: i32) -> Option<std::time::Instant> {
    NOKI_BEDIENT.lock().ok().and_then(|g| g.iter().find(|e| e.0 == pid).map(|e| e.1))
}
pub fn noki_bedient_kuerzlich(pid: i32, innerhalb: std::time::Duration) -> bool {
    NOKI_BEDIENT.lock().map(|g| g.iter().any(|e| e.0 == pid && e.1.elapsed() < innerhalb)).unwrap_or(false)
}

/// Apps whose windows the black-frame gate caught blank while inactive
/// (measured: ChatGPT/Codex paints ONLY while it is the active app).
static AKTIV_ZEICHNER: Mutex<Vec<i32>> = Mutex::new(Vec::new());

/// Explicit click on a Noki window of such an app: make exactly that Noki
/// window the app's main window and activate the app, so it renders live
/// while the user operates it. Safe measured precondition: the app has a
/// window on Noki's display, so activation moves focus to Noki's display
/// only (0 physical Space changes); user windows are not raised on the
/// user's Desktop and nothing is created.
fn live_zeichnen_fuer(pid: i32, wid: i64) {
    let betroffen = AKTIV_ZEICHNER.lock().map(|g| g.contains(&pid)).unwrap_or(false);
    if !betroffen { return; }
    std::thread::spawn(move || {
        let main = fernbedienung::fenster_vorn(pid, wid);
        let ok = crate::blende::aktivieren_direkt(pid);
        crate::virtual_workspace::trace(&format!(
            "[CAPTURE] live_render_activation pid={pid} wid={wid} main={main} activated={ok} reason=app_paints_only_when_active space_navigation=false"));
    });
}

/// After input to a window on a Space the user is not looking at, the
/// Miniatur's capture of exactly that window runs at full rate for a short
/// burst - now and trailing at 0.4 / 1.2 / 2.5 s for output that arrives
/// after the key (a terminal command's output, a save). ONE lane, latest
/// window wins, no queue; it sleeps again 2.5 s after the last input.
///
/// No window is resized for this. Measured 2026-10-01: a 1-pt resize does
/// NOT make an occluded Chromium/Electron window (VS Code) paint, and two
/// overlapping resize wakes had left VS Code 1 pt smaller (883 -> 882).
/// VS Code paints on a hidden Space only when started with
/// --disable-backgrounding-occluded-windows (desktop/vscode-bridge).
pub fn eingabe_zeichnen(pid: i32, wid: i64) {
    if pid <= 0 || wid <= 0 || !chromium_ereignis_app(pid) { return; }
    let nutzer = crate::cgs::aktiver_space().map(|a| a.0);
    if crate::cgs::spaces_des_fensters(wid).is_some_and(|s| nutzer.is_some_and(|u| s.contains(&u))) {
        return; // visible to the user: it paints by itself
    }
    if let Ok(mut g) = ZEICHNEN.0.lock() {
        let letzter = g.as_ref().filter(|z| z.1 == wid).and_then(|z| z.3);
        *g = Some((pid, wid, std::time::Instant::now(), letzter));
    }
    ZEICHNEN.1.notify_one();
    ZEICHNER.get_or_init(|| {
        let _ = std::thread::Builder::new().name("noki-eingabe-zeichnen".into()).spawn(|| loop {
            let (pid, wid, eingabe, letzter) = {
                let Ok(mut g) = ZEICHNEN.0.lock() else { return };
                while g.is_none() {
                    g = match ZEICHNEN.1.wait(g) { Ok(g) => g, Err(_) => return };
                }
                g.unwrap()
            };
            let jetzt = std::time::Instant::now();
            if eingabe.elapsed() > std::time::Duration::from_millis(2600) {
                if let Ok(mut g) = ZEICHNEN.0.lock() {
                    if g.is_some_and(|z| z.2 == eingabe) { *g = None; }
                }
                continue;
            }
            let faellig = match letzter {
                None => true,
                Some(w) => (w < eingabe && w.elapsed() >= std::time::Duration::from_millis(250))
                    || [400u64, 1200, 2500].iter().any(|ms| {
                        let p = eingabe + std::time::Duration::from_millis(*ms);
                        p <= jetzt && w < p
                    }),
            };
            if faellig && ziel_erlaubt(wid) == Some(pid) {
                let t0 = std::time::Instant::now();
                lebhaft_jetzt(wid);
                let ok = true;
                if let Ok(mut g) = ZEICHNEN.0.lock() {
                    if let Some(z) = g.as_mut().filter(|z| z.1 == wid) { z.3 = Some(std::time::Instant::now()); }
                }
                static SPUR: Mutex<Option<std::time::Instant>> = Mutex::new(None);
                if SPUR.lock().is_ok_and(|mut s| {
                    let f = s.is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(2));
                    if f { *s = Some(std::time::Instant::now()); }
                    f
                }) {
                    crate::virtual_workspace::trace(&format!(
                        "[RENDER] capture_burst wid={wid} pid={pid} reason=input_on_hidden_space resize=false ok={ok} ms={}",
                        t0.elapsed().as_millis()));
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        });
    });
}
/// (pid, wid, last input, last wake) of the one window whose input result
/// is being brought in; the lane sleeps on the condvar while None.
static ZEICHNEN: (Mutex<Option<(i32, i64, std::time::Instant, Option<std::time::Instant>)>>, std::sync::Condvar) =
    (Mutex::new(None), std::sync::Condvar::new());
static ZEICHNER: std::sync::OnceLock<()> = std::sync::OnceLock::new();

fn escape_an_prozess(pid: i32) {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventCreateKeyboardEvent(src: *const std::ffi::c_void, key: u16, down: bool) -> *mut std::ffi::c_void;
        fn CGEventSetFlags(ev: *mut std::ffi::c_void, flags: u64);
        fn CGEventSetIntegerValueField(ev: *mut std::ffi::c_void, feld: u32, wert: i64);
        fn CGEventKeyboardSetUnicodeString(ev: *mut std::ffi::c_void, n: usize, s: *const u16);
        fn CGEventPostToPid(pid: i32, ev: *mut std::ffi::c_void);
        fn CFRelease(p: *const std::ffi::c_void);
    }
    unsafe {
        for runter in [true, false] {
            let ev = CGEventCreateKeyboardEvent(std::ptr::null(), 53, runter);
            if ev.is_null() { return; }
            CGEventSetFlags(ev, 0);
            CGEventSetIntegerValueField(ev, 42, crate::fern_tippen::MARKE);
            let esc: u16 = 27;
            CGEventKeyboardSetUnicodeString(ev, 1, &esc);
            CGEventPostToPid(pid, ev);
            CFRelease(ev);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}

/// Transient popover window of `pid` stacked above `wid`, the click point
/// outside it: dismiss it (AXCancel) and verify it is gone.
fn popover_ausserhalb_schliessen(pid: i32, wid: i64, x: f64, y: f64) -> bool {
    let sid = noki_space_id();
    if sid == 0 { return false; }
    let reihe = crate::cgs::fenster_reihe(sid);
    let Some(pos) = reihe.iter().position(|w| *w == wid) else {
        crate::virtual_workspace::trace(&format!("[CLICK] popover_check wid={wid} not_in_order"));
        return false
    };
    let alle = crate::cgs::alle_fenster();
    let Some(haupt) = alle.iter().find(|f| f.0 == wid) else { return false };
    let flaeche = (haupt.6 * haupt.7).max(1) as f64;
    // Only a popover the user can SEE in the Miniatur counts. Invisible
    // ordered-in helper windows (Safari 822557, no image) are excluded from
    // the composition; treating one as a popover swallowed address-field
    // clicks and sent Safari an Escape.
    // The inventory filter itself decides (evaluated now, not at the last
    // 1 s tick, so a just-opened popover counts): it keeps real popovers
    // (GoodNotes) and drops Safari's address-suggestion helper.
    let gezeigt = crate::sichtbare_fenster_auf_arbeitsplatz(sid);
    for w in reihe[..pos].iter().filter(|w| gezeigt.contains(*w)) {
        let Some(f) = alle.iter().find(|f| f.0 == *w && f.1 == pid) else { continue };
        let (fx, fy, fw, fh) = (f.4 as f64, f.5 as f64, f.6 as f64, f.7 as f64);
        if fw * fh > flaeche * 0.6 || fw < 40.0 || fh < 40.0 { continue; }
        if x >= fx && x <= fx + fw && y >= fy && y <= fy + fh { return false; }
        let gone = |ms: u64| {
            for _ in 0..ms / 50 {
                std::thread::sleep(std::time::Duration::from_millis(50));
                if !crate::cgs::alle_fenster().iter().any(|g| g.0 == *w) || !crate::cgs::fenster_eingeordnet(*w) { return true; }
            }
            false
        };
        // Escape to exactly this process (marked, never seen as a user key)
        // is the standard dismissal and needs no AX lookup (~1 s token scan
        // for a popover). Measured GoodNotes: AXCancel accepted but ignored,
        // Escape closes; AXCancel stays as fallback for other apps.
        escape_an_prozess(pid);
        let mut weg = gone(400);
        let mut weg_art = "escape";
        if !weg && fernbedienung::popover_abbrechen(pid, *w) {
            weg = gone(400);
            weg_art = "ax_cancel";
        }
        crate::virtual_workspace::trace(&format!("[CLICK] popover_dismiss wid={wid} popover={w} gone={weg} route={weg_art}"));
        return weg;
    }
    false
}

fn fern_klick_ablegen(app: &tauri::AppHandle, k: FernKlick) {
    bedient_merken(k.pid, k.wid);
    // Explicit Miniatur action on VS Code: make sure it paints while hidden.
    if crate::vscode_bruecke::ist_vscode(k.pid) { crate::vscode_bruecke::live_sicherstellen(); }
    eingabe_zeichnen(k.pid, k.wid);
    // Process activation can make macOS follow an off-Space application.
    // REAL_SPACE therefore uses capture restart + exact-window AXRaise only;
    // the legacy virtual display may still use its measured activation path.
    if crate::virtual_workspace::backend()
        == crate::virtual_workspace::Backend::LegacyVirtualDisplay
    {
        live_zeichnen_fuer(k.pid, k.wid);
    }
    eprintln!("[CLICK] userSeq={} queued wid={}", k.seq, k.wid);
    if arbeit().klick_ablegen(k) {
        eprintln!("[FERN] Klick an wartende Transaktion angehaengt");
    }
    FERN_WECKER.notify_one();
    arbeiter_sicherstellen(app);
}

fn fern_ziehen_ablegen(app: &tauri::AppHandle, z: FernZiehen) {
    bedient_merken(z.pid, z.wid);
    // Ein Anfang wartet, bis ein noch ausstehender Anfang ODER ein
    // ausstehendes Ende verarbeitet ist - sonst ueberschrieb ein schneller
    // naechster Druck das Ende der vorigen Geste (gemessen: Resize ohne
    // gesicherten Endrahmen).
    for _ in 0..50 {
        let belegt = arbeit().ziehen.is_some_and(|a| a.anfang || a.ende) && z.anfang;
        if !belegt { break; }
        FERN_WECKER.notify_one();
        std::thread::sleep(std::time::Duration::from_millis(4));
    }
    arbeit().ziehen_ablegen(z);
    FERN_WECKER.notify_one();
    arbeiter_sicherstellen(app);
}

/// Apps whose scroll views take a window-addressed process wheel (measured:
/// GoodNotes). Cached per pid (the reader thread sees 60-120 packets/s).
fn katalyst_rad_app(pid: i32) -> bool { bundle_gemerkt(pid).to_lowercase().contains("goodnotes") }

/// Bundle id per pid, cached (per-packet checks on the reader thread).
fn bundle_gemerkt(pid: i32) -> String {
    static C: Mutex<Vec<(i32, String)>> = Mutex::new(Vec::new());
    if let Some(v) = C.lock().ok().and_then(|c| c.iter().find(|e| e.0 == pid).map(|e| e.1.clone())) { return v; }
    let b = crate::lesezeichen::app_fuer_pid(pid).map(|(_, b, _)| b).unwrap_or_default();
    if let Ok(mut c) = C.lock() { c.push((pid, b.clone())); if c.len() > 64 { c.remove(0); } }
    b
}

/// Chromium/Electron windows accept a CGEvent addressed to their exact
/// window even while the app is inactive. Keep this framework-based: VS Code
/// is one consumer, not a coordinate special case.
fn chromium_ereignis_app(pid: i32) -> bool {
    static C: Mutex<Vec<(i32, bool)>> = Mutex::new(Vec::new());
    if let Some(v) = C.lock().ok().and_then(|c| c.iter().find(|e| e.0 == pid).map(|e| e.1)) { return v; }
    let v = crate::lesezeichen::app_fuer_pid(pid).is_some_and(|(_, _, pfad)| {
        let fw = std::path::Path::new(&pfad).join("Contents/Frameworks");
        ["Electron Framework.framework", "Chromium Embedded Framework.framework", "Google Chrome Framework.framework"]
            .iter().any(|n| fw.join(n).exists())
    });
    if let Ok(mut c) = C.lock() { c.push((pid, v)); if c.len() > 64 { c.remove(0); } }
    v
}

/// The installed YouTube PWA is rendered by the user's Chrome and freezes on
/// the hidden Noki Space (stale frames, jumpy scroll). Say once where the
/// smooth YouTube is instead of silently degrading.
fn youtube_pwa_hinweis(pid: i32) {
    if !bundle_gemerkt(pid).starts_with("com.google.Chrome.app.agimnkijcaahngcdmfeangaknmldooml") { return; }
    static ZULETZT: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    let faellig = ZULETZT.lock().is_ok_and(|mut z| {
        let f = z.is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(600));
        if f { *z = Some(std::time::Instant::now()); }
        f
    });
    if faellig {
        crate::virtual_workspace::trace(&format!("[YOUTUBE] legacy_pwa_scroll pid={pid} class=LIMITED hint=noki_youtube"));
        hinweis("Diese YouTube-App friert im Hintergrund ein. Flüssig: „YouTube“ in Noki öffnen (Noki YouTube).");
    }
}

/// Whole wheel units for the AX / key routes (they need discrete steps):
/// the float remainder is carried per window; a finger lift flushes a
/// leftover fraction as one unit (as the helper did before).
fn rad_quantisieren(wid: i64, fdx: f64, fdy: f64, phase: &str) -> Option<(i32, i32)> {
    static REST: Mutex<(i64, f64, f64)> = Mutex::new((0, 0.0, 0.0));
    let mut g = REST.lock().ok()?;
    if g.0 != wid { *g = (wid, 0.0, 0.0); }
    g.1 += fdx; g.2 += fdy;
    let (mut dx, mut dy) = (g.1.trunc() as i32, g.2.trunc() as i32);
    if dx == 0 && dy == 0 && phase == "e" {
        dx = if g.1 == 0.0 { 0 } else { g.1.signum() as i32 };
        dy = if g.2 == 0.0 { 0 } else { g.2.signum() as i32 };
    }
    if dx == 0 && dy == 0 { return None; }
    g.1 -= dx as f64; g.2 -= dy as f64;
    Some((dx, dy))
}

fn fern_rad_ablegen(app: &tauri::AppHandle, r: FernRad) {
    rad_arbeit().rad_ablegen(r);
    RAD_WECKER.notify_one();
    rad_arbeiter_sicherstellen(app);
}

#[cfg(debug_assertions)]
pub fn debug_fern_klick(
    app: &tauri::AppHandle, wid: i64, x: f64, y: f64, seq: u64, klicks: i64,
) -> bool {
    let Some(pid) = ziel_erlaubt(wid) else { return false; };
    fern_klick_ablegen(app, FernKlick { pid, wid, x, y, klicks: if klicks < 0 { -1 } else { klicks.max(1) }, seq });
    true
}

/// Debug: hover over window content - the same route as ZEIGER bewege.
#[cfg(debug_assertions)]
pub fn debug_fern_schweben(wid: i64, x: f64, y: f64) -> bool {
    let Some(pid) = ziel_erlaubt(wid) else { return false; };
    if crate::noki_browser::ist_noki_browser(pid) { crate::noki_browser::zeiger(wid, x, y, 0); }
    true
}

/// Debug: eine Zieh-Geste (Anfang, 10 Schritte, Ende) - derselbe Weg wie
/// ein Ziehen in der Vorschau.
#[cfg(debug_assertions)]
pub fn debug_fern_ziehen(app: &tauri::AppHandle, wid: i64, x1: f64, y1: f64, x2: f64, y2: f64, art: i64) -> bool {
    let Some(pid) = ziel_erlaubt(wid) else { return false; };
    // Same routing as the helper's ZEIGER ziehen_* under REAL_SPACE.
    if crate::virtual_workspace::backend() == crate::virtual_workspace::Backend::RealSpace
        && crate::noki_browser::ist_noki_browser(pid)
    {
        crate::noki_browser::zeiger(wid, x1, y1, 1);
        for i in 1..=10 {
            std::thread::sleep(std::time::Duration::from_millis(30));
            let t = i as f64 / 10.0;
            crate::noki_browser::zeiger(wid, x1 + (x2 - x1) * t, y1 + (y2 - y1) * t, if i == 10 { 3 } else { 2 });
        }
        return true;
    }
    fern_ziehen_ablegen(app, FernZiehen { pid, wid, x: x1, y: y1, anfang: true, ende: false, art });
    for i in 1..=10 {
        std::thread::sleep(std::time::Duration::from_millis(30));
        let t = i as f64 / 10.0;
        fern_ziehen_ablegen(app, FernZiehen { pid, wid, x: x1 + (x2 - x1) * t, y: y1 + (y2 - y1) * t,
                                              anfang: false, ende: i == 10, art });
    }
    true
}

#[cfg(debug_assertions)]
pub fn debug_fern_rad(
    app: &tauri::AppHandle, wid: i64, x: f64, y: f64, dx: i32, dy: i32,
) -> bool {
    let Some(pid) = ziel_erlaubt(wid) else { return false; };
    fern_rad_ablegen(app, FernRad { pid, wid, x, y, dx, dy });
    true
}

fn arbeiter_sicherstellen(app: &tauri::AppHandle) {
    FERN_ARBEITER.get_or_init(|| {
        let a = app.clone();
        let _ = std::thread::Builder::new()
            .name("noki-fern".into())
            .spawn(move || fern_arbeiter(a, 0));
        let a = app.clone();
        let _ = std::thread::Builder::new().name("noki-fern-watch".into()).spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let aktiv = FERN_PHASE.load(std::sync::atomic::Ordering::SeqCst) != FernPhase::Leer as u8;
            let hb = FERN_LETZTER_HEARTBEAT_MS.load(std::sync::atomic::Ordering::Relaxed);
            if !aktiv || hb == 0 || jetzt_ms().saturating_sub(hb) <= 6_000 { continue; }
            let alt = FERN_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let neu = alt + 1;
            FERN_TIMEOUTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            phase(FernPhase::Leer);
            crate::virtual_workspace::trace(&format!(
                "[RECOVERY] subsystem=CLICK_WORKER generation={alt}->{neu} stuck_ms={}", jetzt_ms().saturating_sub(hb)));
            FERN_LETZTER_HEARTBEAT_MS.store(jetzt_ms(), std::sync::atomic::Ordering::Relaxed);
            let h = a.clone();
            let _ = std::thread::Builder::new().name(format!("noki-fern-{neu}"))
                .spawn(move || fern_arbeiter(h, neu));
        });
    });
}

fn rad_arbeiter_sicherstellen(app: &tauri::AppHandle) {
    RAD_ARBEITER.get_or_init(|| {
        let a = app.clone();
        let generation = RAD_GENERATION.load(std::sync::atomic::Ordering::SeqCst);
        let _ = std::thread::Builder::new()
            .name("noki-scroll".into())
            .spawn(move || rad_arbeiter(a, generation));
    });
    RAD_WATCHDOG.get_or_init(|| {
        let a = app.clone();
        let _ = std::thread::Builder::new().name("noki-scroll-watch".into()).spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let aktiv = RAD_PHASE.load(std::sync::atomic::Ordering::SeqCst) != FernPhase::Leer as u8;
            let hb = RAD_LETZTER_HEARTBEAT_MS.load(std::sync::atomic::Ordering::Relaxed);
            if !aktiv || hb == 0 || jetzt_ms().saturating_sub(hb) <= 2_000 { continue; }
            let alt = RAD_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let neu = alt + 1;
            let wid = RAD_AKTIV_WID.load(std::sync::atomic::Ordering::Relaxed);
            RAD_ISOLIERT_WID.store(wid, std::sync::atomic::Ordering::Relaxed);
            RAD_ISOLIERT_BIS_MS.store(jetzt_ms() + 30_000, std::sync::atomic::Ordering::Relaxed);
            RAD_TIMEOUTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            RAD_AKTIV_WID.store(0, std::sync::atomic::Ordering::Relaxed);
            RAD_PHASE.store(FernPhase::Leer as u8, std::sync::atomic::Ordering::SeqCst);
            crate::virtual_workspace::trace(&format!(
                "[RECOVERY] subsystem=SCROLL_WORKER generation={alt}->{neu} wid={wid} isolated_seconds=30"));
            let h = a.clone();
            let _ = std::thread::Builder::new().name(format!("noki-scroll-{neu}"))
                .spawn(move || rad_arbeiter(h, neu));
            RAD_LETZTER_HEARTBEAT_MS.store(jetzt_ms(), std::sync::atomic::Ordering::Relaxed);
        });
    });
}

fn rad_arbeiter(app: tauri::AppHandle, generation: u64) {
    use std::time::{Duration, Instant};
    loop {
        if RAD_GENERATION.load(std::sync::atomic::Ordering::SeqCst) != generation { return; }
        RAD_LETZTER_HEARTBEAT_MS.store(jetzt_ms(), std::sync::atomic::Ordering::Relaxed);
        let rad = {
            let mut g = rad_arbeit();
            loop {
                let Some(seit) = g.rad_seit else {
                    g = RAD_WECKER.wait_timeout(g, Duration::from_secs(5))
                        .map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0);
                    RAD_LETZTER_HEARTBEAT_MS.store(jetzt_ms(), std::sync::atomic::Ordering::Relaxed);
                    continue;
                };
                let sammeln = if crate::virtual_workspace::backend()
                    == crate::virtual_workspace::Backend::LegacyVirtualDisplay { 12 } else { RAD_SAMMELN_MS };
                let reif = seit + Duration::from_millis(sammeln);
                let jetzt = Instant::now();
                if jetzt >= reif { break; }
                g = RAD_WECKER.wait_timeout(g, reif - jetzt)
                    .map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0);
            }
            g.entnehmen().1
        };
        let Some(rad) = rad else { continue };
        RAD_LETZTER_HEARTBEAT_MS.store(jetzt_ms(), std::sync::atomic::Ordering::Relaxed);
        RAD_AKTIV_WID.store(rad.wid, std::sync::atomic::Ordering::Relaxed);
        let t0 = Instant::now();
        RAD_PHASE.store(FernPhase::Vorbereiten as u8, std::sync::atomic::Ordering::SeqCst);
        let _fern = FernAktivGuard::neu();
        RAD_PHASE.store(FernPhase::Aktiv as u8, std::sync::atomic::Ordering::SeqCst);
        let ergebnis = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            fern_vorgang(&app, None, Some(rad))
        }));
        if RAD_GENERATION.load(std::sync::atomic::Ordering::SeqCst) == generation {
            RAD_PHASE.store(FernPhase::Nachlauf as u8, std::sync::atomic::Ordering::SeqCst);
        }
        if ergebnis.is_err() { eprintln!("[SCROLL] worker panic caught; lane remains alive"); }
        let ms = t0.elapsed().as_millis();
        if ms > 700 { RAD_TIMEOUTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed); }
        RAD_ABGESCHLOSSEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        RAD_LETZTER_HEARTBEAT_MS.store(jetzt_ms(), std::sync::atomic::Ordering::Relaxed);
        if RAD_GENERATION.load(std::sync::atomic::Ordering::SeqCst) == generation {
            RAD_AKTIV_WID.store(0, std::sync::atomic::Ordering::Relaxed);
            RAD_PHASE.store(FernPhase::Leer as u8, std::sync::atomic::Ordering::SeqCst);
        } else {
            return;
        }
    }
}

fn fern_arbeiter(app: tauri::AppHandle, generation: u64) {
    use std::time::{Duration, Instant};
    let veraltet = || FERN_GENERATION.load(std::sync::atomic::Ordering::SeqCst) != generation;
    loop {
        if veraltet() { return; }
        FERN_LETZTER_HEARTBEAT_MS.store(jetzt_ms(), std::sync::atomic::Ordering::Relaxed);
        // This lane handles click/drag only. Scroll is isolated in
        // `noki-scroll`, so a slow application cannot starve typing setup.
        let klick = {
            let mut g = arbeit();
            loop {
                if g.klick.is_some() {
                    break;
                }
                if let Some(z) = g.ziehen.take() {
                    drop(g);
                    let a = app.clone();
                    FERN_LETZTER_HEARTBEAT_MS.store(jetzt_ms(), std::sync::atomic::Ordering::Relaxed);
                    phase(FernPhase::Aktiv);
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ziehen_vorgang(&a, z)));
                    if veraltet() { return; }
                    phase(FernPhase::Leer);
                    g = arbeit();
                    continue;
                }
                g = FERN_WECKER
                    .wait_timeout(g, Duration::from_secs(5))
                    .map(|(g, _)| g)
                    .unwrap_or_else(|e| e.into_inner().0);
            }
            g.entnehmen().0
        };
        if klick.is_none() {
            continue;
        }
        let t0 = Instant::now();
        // The supervisor measures the OPERATION: stamp its start (the loop-
        // top stamp can be minutes old after an idle wait -> false "stuck").
        FERN_LETZTER_HEARTBEAT_MS.store(jetzt_ms(), std::sync::atomic::Ordering::Relaxed);
        phase(FernPhase::Vorbereiten);
        if let Some(v) = klick.as_ref() {
            for k in v {
                eprintln!("[CLICK] userSeq={} started wid={}", k.seq, k.wid);
            }
        }
        if abnahme() {
            let g = arbeit();
            eprintln!(
                "[ACCEPT] REMOTE start busy=true pendingClick={} accumulatedScroll={},{}",
                g.klick.is_some(),
                g.rad.map(|r| r.dx).unwrap_or(0),
                g.rad.map(|r| r.dy).unwrap_or(0)
            );
        }
        let _fern = FernAktivGuard::neu();
        let a = app.clone();
        let klick_ausgabe = klick.clone();
        let ergebnis = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            fern_vorgang(&a, klick, None)
        }));
        // Abandoned by the supervisor while stuck: the new lane owns phase
        // and heartbeat now.
        if veraltet() { return; }
        phase(FernPhase::Nachlauf);
        let (nachgewiesen, weg) = match ergebnis {
            Ok(v) => v,
            Err(_) => {
                eprintln!("[FERN] Fehler im Vorgang abgefangen - aufgeraeumt");
                (false, "fehler")
            }
        };
        // Der Helfer gibt den Hover wieder frei und gleicht ihn SOFORT gegen
        // die echte Zeigerlage ab (der Zeiger stand kurz woanders, und macOS
        // verliert dabei gern ein mouseExited).
        phase(FernPhase::Leer);
        FERN_ABGESCHLOSSEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        FERN_LETZTER_HEARTBEAT_MS.store(jetzt_ms(), std::sync::atomic::Ordering::Relaxed);
        crate::leiste_anstossen(&app);
        let (wartet, pending_click, pending_dx, pending_dy) = {
            let g = arbeit();
            (
                !g.leer(), g.klick.is_some(),
                g.rad.map(|r| r.dx).unwrap_or(0),
                g.rad.map(|r| r.dy).unwrap_or(0),
            )
        };
        if abnahme() {
            eprintln!(
                "[ACCEPT] REMOTE end busy=false pendingClick={} accumulatedScroll={},{}",
                pending_click, pending_dx, pending_dy
            );
        }
        eprintln!(
            "[FERN] Vorgang klick={} rad={} weg={weg} in {}ms (wartet: {wartet})",
            klick_ausgabe.as_ref().map(|v| format!("{} Klicks", v.len())).unwrap_or_else(|| "-".into()),
            "-",
            t0.elapsed().as_millis()
        );
        if let Some(v) = klick_ausgabe {
            for k in v {
                eprintln!(
                    "[CLICK] userSeq={} completed delivered={} method={}",
                    k.seq, nachgewiesen, weg
                );
                let _ = app.emit(
                    "noki://vorschau_fern",
                    serde_json::json!({
                        "was": "klick", "fenster": k.wid, "seq": k.seq,
                        "nachgewiesen": nachgewiesen,
                        "bedienhilfen": fernbedienung::vertraut()
                    }),
                );
            }
        }
    }
}

/// Neueste Schwebe-Frage (pid, wid, x, y); aeltere sind ueberholt.
static SCHWEBE: Mutex<Option<(i32, i64, f64, f64)>> = Mutex::new(None);
static SCHWEBE_WECKER: std::sync::Condvar = std::sync::Condvar::new();
static SCHWEBE_FADEN: std::sync::OnceLock<()> = std::sync::OnceLock::new();

fn schwebe_ablegen(pid: i32, wid: i64, x: f64, y: f64) {
    if let Ok(mut g) = SCHWEBE.lock() { *g = Some((pid, wid, x, y)); }
    SCHWEBE_WECKER.notify_one();
    SCHWEBE_FADEN.get_or_init(|| {
        let _ = std::thread::Builder::new().name("noki-schwebe".into()).spawn(|| loop {
            let frage = {
                let mut g = SCHWEBE.lock().unwrap_or_else(|e| e.into_inner());
                while g.is_none() {
                    g = SCHWEBE_WECKER.wait(g).unwrap_or_else(|e| e.into_inner());
                }
                g.take()
            };
            if let Some((pid, wid, x, y)) = frage {
                let ok = fernbedienung::fenster_ziehbereich(pid, wid, x, y);
                befehl(&format!("ziehbar {wid} {x:.0} {y:.0} {}", ok as i32));
            }
        });
    });
}

static ZIEH_SITZUNG: Mutex<Option<fernbedienung::AuswahlZiel>> = Mutex::new(None);
/// Laufende Schieberegler-Geste (Fortschritt, Lautstaerke ...).
static SCHIEBER: Mutex<Option<(i64, fernbedienung::Schieber)>> = Mutex::new(None);

/// Ist das ein Browser? (Dort bleibt der fluessige Pfeil-Weg fuers Rollen.)
fn ist_browser(pid: i32) -> bool {
    static CACHE: Mutex<Vec<(i32, bool)>> = Mutex::new(Vec::new());
    if let Some(v) = CACHE.lock().ok().and_then(|c| c.iter().find(|e| e.0 == pid).map(|e| e.1)) { return v; }
    let b = crate::lesezeichen::app_fuer_pid(pid).map(|(_, b, _)| b).unwrap_or_default();
    let ja = matches!(b.as_str(), "com.google.Chrome" | "com.apple.Safari" | "org.mozilla.firefox"
        | "company.thebrowser.Browser" | "com.brave.Browser" | "com.microsoft.edgemac");
    if let Ok(mut c) = CACHE.lock() { c.push((pid, ja)); }
    ja
}
static KEYFREI_REST: Mutex<i32> = Mutex::new(0);
/// Accumulates wheel units per web window; true when one step is due.
fn web_roll_akku(wid: i64, dy: i32) -> bool {
    static AKKU: Mutex<(i64, i32, Option<std::time::Instant>)> = Mutex::new((0, 0, None));
    let Ok(mut g) = AKKU.lock() else { return true };
    let neu = g.0 != wid || g.2.is_none_or(|t| t.elapsed() > std::time::Duration::from_millis(600))
        || (g.1 != 0 && g.1.signum() != dy.signum());
    if neu { *g = (wid, 0, None); }
    g.1 += dy;
    g.2 = Some(std::time::Instant::now());
    if g.1.abs() >= 45 { g.1 = 0; true } else { false }
}

fn keyfrei_schritte(schritte: i32) -> i32 {
    let Ok(mut g) = KEYFREI_REST.lock() else { return schritte.signum() };
    if *g != 0 && g.signum() != schritte.signum() { *g = 0; }
    *g += schritte;
    // One animated ~370 px jump per ~240 px of finger travel.
    let n = (*g / 6).clamp(-1, 1);
    *g -= n * 6;
    n
}

/// Chrome PWA shell (YouTube.app, Motion): its window is a web document.
fn ist_pwa(pid: i32) -> bool {
    crate::lesezeichen::app_fuer_pid(pid).is_some_and(|(_, b, _)| b.starts_with("com.google.Chrome.app."))
}

/// Accumulated finger travel per window -> whole 40 px page steps.
/// Helper units are trackpad points / 4 (precise deltas), i.e. 4 px each.
/// The gesture also locks its classification ("media under the pointer")
/// at its first packet: the page moving under a still pointer must not
/// flip a running scroll gesture into another route (measured flip on
/// YouTube after the first step).
static ROLL_REST: Mutex<(i64, i32, Option<std::time::Instant>, Option<bool>)> = Mutex::new((0, 0, None, None));
fn roll_schritte(wid: i64, dy: i32, medien: impl FnOnce() -> bool) -> (i32, bool) {
    const SCHRITT_PX: i32 = 40;
    let Ok(mut g) = ROLL_REST.lock() else { return (dy.signum(), true) };
    let neu = g.0 != wid || g.2.is_none_or(|t| t.elapsed() > std::time::Duration::from_millis(600));
    if neu { *g = (wid, 0, None, None); }
    if g.1 != 0 && g.1.signum() != dy.signum() { g.1 = 0; }
    if g.3.is_none() { g.3 = Some(medien()); }
    g.1 += dy * 4;
    g.2 = Some(std::time::Instant::now());
    let n = g.1 / SCHRITT_PX;
    g.1 -= n * SCHRITT_PX;
    (n, g.3.unwrap_or(true))
}

#[derive(Clone, Copy)]
struct FensterZug {
    pid: i32, wid: i64,
    start_x: f64, start_y: f64,
    frame: [f64; 4],
    /// 0 = move; otherwise resize edge mask (left/right/top/bottom).
    art: i64,
}
static FENSTER_ZUG: Mutex<Option<FensterZug>> = Mutex::new(None);

#[derive(Clone)]
struct TabZug {
    pid: i32,
    wid: i64,
    start_x: f64,
    start_y: f64,
    windows_before: Vec<i64>,
}
static TAB_ZUG: Mutex<Option<TabZug>> = Mutex::new(None);

fn fensterzug_rahmen(m: FensterZug, x: f64, y: f64, bounds: [f64; 4]) -> [f64; 4] {
    let [left, top, right, bottom] = bounds;
    let (dx, dy) = (x - m.start_x, y - m.start_y);
    let mut frame = m.frame;
    if m.art == 0 {
        let max_x = right - m.frame[2];
        let max_y = bottom - 28.0;
        frame[0] = (m.frame[0] + dx).clamp(left, max_x.max(left));
        frame[1] = (m.frame[1] + dy).clamp(top, max_y.max(top));
        return frame;
    }
    // AX does not expose a portable minimum-size attribute for all apps.
    // Keep a conservative fallback and never let an edge pass its opposite
    // edge or escape the virtual usable bounds.
    const MIN_W: f64 = 240.0;
    const MIN_H: f64 = 160.0;
    if m.art & 1 != 0 {
        let new_left = (m.frame[0] + dx)
            .clamp(left, (m.frame[0] + m.frame[2] - MIN_W).max(left));
        frame[0] = new_left; frame[2] = m.frame[2] + (m.frame[0] - new_left);
    }
    if m.art & 2 != 0 {
        frame[2] = (m.frame[2] + dx).clamp(MIN_W, (right - m.frame[0]).max(MIN_W));
    }
    if m.art & 4 != 0 {
        let new_top = (m.frame[1] + dy)
            .clamp(top, (m.frame[1] + m.frame[3] - MIN_H).max(top));
        frame[1] = new_top; frame[3] = m.frame[3] + (m.frame[1] - new_top);
    }
    if m.art & 8 != 0 {
        frame[3] = (m.frame[3] + dy).clamp(MIN_H, (bottom - m.frame[1]).max(MIN_H));
    }
    frame
}

/// Lage, die die feste(n) Kante(n) wiederherstellt, falls das Programm eine
/// andere Groesse als verlangt angenommen hat. None = alles richtig.
fn resize_anker(m: FensterZug, soll: [f64; 4], ist: [f64; 4]) -> Option<[f64; 2]> {
    if m.art == 0 { return None; }
    let rechts = m.frame[0] + m.frame[2];
    let unten = m.frame[1] + m.frame[3];
    let mut lage = [ist[0], ist[1]];
    // Linke Kante gezogen: rechte Kante fest. Rechte gezogen: linke fest.
    if m.art & 1 != 0 { lage[0] = rechts - ist[2]; } else { lage[0] = m.frame[0]; }
    if m.art & 4 != 0 { lage[1] = unten - ist[3]; } else { lage[1] = m.frame[1]; }
    let _ = soll;
    ((lage[0] - ist[0]).abs() > 1.0 || (lage[1] - ist[1]).abs() > 1.0).then_some(lage)
}

#[cfg(test)]
mod fensterzug_tests {
    use super::{fensterzug_rahmen, resize_anker, FensterZug};

    #[test]
    fn feste_kante_bleibt_bei_begrenzter_groesse() {
        let z = |art| FensterZug { pid: 1, wid: 2, start_x: 0.0, start_y: 0.0,
                                   frame: [100.0, 100.0, 500.0, 400.0], art };
        // Links gezogen, Programm nimmt nur 450 statt 520 Breite: rechts (600) bleibt.
        assert_eq!(resize_anker(z(1), [80.0, 100.0, 520.0, 400.0], [80.0, 100.0, 450.0, 400.0]), Some([150.0, 100.0]));
        // Oben gezogen, Hoehe begrenzt: unten (500) bleibt.
        assert_eq!(resize_anker(z(4), [100.0, 50.0, 500.0, 450.0], [100.0, 50.0, 500.0, 420.0]), Some([100.0, 80.0]));
        // Rechts/unten gezogen: links/oben bleiben - auch wenn die App verschiebt.
        assert_eq!(resize_anker(z(2 | 8), [100.0, 100.0, 600.0, 500.0], [103.0, 100.0, 590.0, 490.0]), Some([100.0, 100.0]));
        // Alles richtig: nichts tun.
        assert_eq!(resize_anker(z(1), [80.0, 100.0, 520.0, 400.0], [80.0, 100.0, 520.0, 400.0]), None);
    }

    fn z(art: i64) -> FensterZug {
        FensterZug { pid: 1, wid: 2, start_x: 100.0, start_y: 100.0,
                     frame: [100.0, 100.0, 500.0, 400.0], art }
    }

    #[test]
    fn alle_kanten_und_ecken_resizen_kontinuierlich() {
        let b = [0.0, 0.0, 1200.0, 900.0];
        assert_eq!(fensterzug_rahmen(z(1), 150.0, 100.0, b), [150.0, 100.0, 450.0, 400.0]);
        assert_eq!(fensterzug_rahmen(z(2), 150.0, 100.0, b), [100.0, 100.0, 550.0, 400.0]);
        assert_eq!(fensterzug_rahmen(z(4), 100.0, 150.0, b), [100.0, 150.0, 500.0, 350.0]);
        assert_eq!(fensterzug_rahmen(z(8), 100.0, 150.0, b), [100.0, 100.0, 500.0, 450.0]);
        assert_eq!(fensterzug_rahmen(z(1|4), 150.0, 150.0, b), [150.0, 150.0, 450.0, 350.0]);
        assert_eq!(fensterzug_rahmen(z(2|4), 150.0, 150.0, b), [100.0, 150.0, 550.0, 350.0]);
        assert_eq!(fensterzug_rahmen(z(1|8), 150.0, 150.0, b), [150.0, 100.0, 450.0, 450.0]);
        assert_eq!(fensterzug_rahmen(z(2|8), 150.0, 150.0, b), [100.0, 100.0, 550.0, 450.0]);
    }

    #[test]
    fn resize_achtet_minimum_und_workspace_grenzen() {
        let b = [0.0, 0.0, 700.0, 600.0];
        assert_eq!(fensterzug_rahmen(z(1|4), 1000.0, 1000.0, b), [360.0, 340.0, 240.0, 160.0]);
        assert_eq!(fensterzug_rahmen(z(2|8), 1000.0, 1000.0, b), [100.0, 100.0, 600.0, 500.0]);
    }
}

/// Zieh-Auswahl. Editierbarer Text bekommt Chromes ECHTE Auswahl (laufend
/// nachgefuehrt) und die Tastatur (Fern-Tippen). Gewoehnlicher Seitentext:
/// Chrome nimmt dafuer gemessen keinerlei Hintergrund-Auswahl an (weder
/// AXSelectedTextMarkerRange noch Zeigerereignisse) - dort zeichnet Noki die
/// Markierung aus Chromes echten Zeichenrahmen, und Cmd+C kopiert den echten Text.
fn ziehen_vorgang(app: &tauri::AppHandle, z: FernZiehen) {
    if crate::virtual_workspace::backend() != crate::virtual_workspace::Backend::LegacyVirtualDisplay
        || !crate::virtual_workspace::ready() { return; }
    if menue_offen() {
        menues_schliessen();
        return;
    }
    let t0 = std::time::Instant::now();
    fernbedienung::hauptfenster_leihen(z.pid);
    if z.anfang {
        crate::fern_tippen::beenden("drag_start");
        crate::fern_tippen::warten_beenden("drag_start");
        real_freilegen(z.pid, z.wid, "drag", false);
        if let Ok(mut g) = FENSTER_ZUG.lock() { *g = None; }
        if let Ok(mut g) = SCHIEBER.lock() { *g = None; }
        if let Ok(mut g) = TAB_ZUG.lock() { *g = None; }
        let window_gesture = z.art != 0
            || fernbedienung::fenster_ziehbereich(z.pid, z.wid, z.x, z.y);
        let mut neu = None;
        if z.art == 0 && ist_browser(z.pid)
            && fernbedienung::tab_am_punkt(z.pid, z.wid, z.x, z.y)
        {
            // Chrome accepts a background click but will not promote that
            // into its native tab-drag controller unless the exact source
            // AXWindow is main/focused.  This is window-scoped AX state: it
            // does not activate the application, move Spaces, or touch a
            // different (user-owned) Chrome window.
            let exact_focused = fernbedienung::fenster_vorn(z.pid, z.wid);
            // Chromium's native drag controller additionally requires its
            // process to be active. This gesture came from an explicit Noki
            // click, so a short, exact-window activation is permitted by the
            // interaction contract. Noki is restored on mouse-up below.
            let app_activated = exact_focused && crate::blende::ziel_vorn(app, z.pid);
            let before = fenster_liste(false).into_iter()
                .filter(|f| f.pid == z.pid).map(|f| f.wid).collect();
            if let Ok(mut g) = TAB_ZUG.lock() {
                *g = Some(TabZug { pid: z.pid, wid: z.wid, start_x: z.x,
                                   start_y: z.y, windows_before: before });
            }
            befehl(&format!("zug {} -1", z.wid));
            fernbedienung::tab_ziehen_event(z.pid, z.x, z.y, 0);
            crate::virtual_workspace::trace(&format!(
                "[TAB_DRAG] start wid={} x={:.0} y={:.0} exact_focused={} app_activated={} target_locked=true cursor_warp=false",
                z.wid, z.x, z.y, exact_focused, app_activated));
        } else if window_gesture {
            if let Some(f) = fenster_liste(false).into_iter().find(|f| f.wid == z.wid && f.pid == z.pid) {
                if let Ok(mut g) = FENSTER_ZUG.lock() {
                    // 16 = explicit move (⌘-drag in the helper) = art 0 here.
                    *g = Some(FensterZug { pid: z.pid, wid: z.wid, start_x: z.x, start_y: z.y,
                                          frame: [f.x, f.y, f.w, f.h], art: if z.art == 16 { 0 } else { z.art } });
                }
                // Der Helfer fuehrt ab jetzt die Ebene selbst mit dem Zeiger
                // (Startrahmen + Zeigerversatz) - sichtbar ohne AX-Latenz.
                befehl(&format!("zug {} {} {:.1} {:.1} {:.1} {:.1} {:.1} {:.1}",
                                z.wid, if z.art == 0 { 16 } else { z.art }, z.x, z.y, f.x, f.y, f.w, f.h));
                crate::virtual_workspace::trace(&format!(
                    "[WINDOW_{}] start wid={} mask={} source={}",
                    if z.art == 0 { "MOVE" } else { "RESIZE" }, z.wid, z.art,
                    if z.art == 0 { "header_band" } else { "preview_edge" }));
            }
        } else if let Some(s) = fernbedienung::schieber_beginnen(z.pid, z.wid, z.x, z.y) {
            // Schieberegler: bis zum Loslassen gehoert die Geste ihm.
            befehl(&format!("zug {} -1", z.wid));
            fernbedienung::schieber_setzen(&s, z.x);
            if let Ok(mut g) = SCHIEBER.lock() { *g = Some((z.wid, s)); }
            if let Ok(mut g) = ZIEH_SITZUNG.lock() { *g = None; }
            return;
        } else {
            befehl(&format!("zug {} -1", z.wid));
            // Only non-window chrome reaches content selection. A Chrome tab
            // is an AX descendant rather than AXWindow, so its native drag
            // path remains distinct from whole-window movement.
            neu = fernbedienung::auswahl_beginnen(z.pid, z.wid, z.x, z.y);
        }
        let editierbar = neu.as_ref().map(|(a, _)| a.editierbar);
        crate::virtual_workspace::trace(&format!(
            "[AUSWAHL] start wid={} target={} space_trip=false",
            z.wid, match editierbar { Some(true) => "editable", Some(false) => "page_text", None => "none" }
        ));
        let sitzung = neu.map(|(a, tippen)| {
            if let Some((feld, vorher)) = tippen { crate::fern_tippen::beginnen(z.pid, z.wid, feld, vorher); }
            a
        });
        if let Ok(mut g) = ZIEH_SITZUNG.lock() { *g = sitzung; }
    }
    let tab = TAB_ZUG.lock().ok().and_then(|g| g.clone());
    if let Some(tab) = tab.filter(|t| t.wid == z.wid && t.pid == z.pid) {
        fernbedienung::tab_ziehen_event(tab.pid, z.x, z.y, if z.ende { 2 } else { 1 });
        if z.ende {
            if let Ok(mut g) = TAB_ZUG.lock() { *g = None; }
            let mut child = None;
            for _ in 0..30 {
                child = fenster_liste(false).into_iter()
                    .find(|f| f.pid == tab.pid && !tab.windows_before.contains(&f.wid))
                    .map(|f| f.wid);
                if child.is_some() { break; }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            let claimed = child.is_some_and(|wid|
                crate::workspace_child_window_registrieren(app, tab.wid, wid, tab.pid));
            if claimed { crate::leiste_aktualisieren(app); }
            crate::noki_interaction_aktivieren(app);
            let distance = ((z.x - tab.start_x).powi(2) + (z.y - tab.start_y).powi(2)).sqrt();
            crate::virtual_workspace::trace(&format!(
                "[TAB_DRAG] end source={} child={:?} child_claimed={claimed} distance={distance:.0} cursor_warp=false space_trip=false",
                tab.wid, child));
        }
        return;
    }
    // Laufender Schieberegler: Wert folgt dem Zeiger.
    let schieber_aktiv = SCHIEBER.lock().ok().is_some_and(|g| g.as_ref().is_some_and(|(w, _)| *w == z.wid));
    if schieber_aktiv {
        if let Ok(mut g) = SCHIEBER.lock() {
            if let Some((_, s)) = g.as_ref() { fernbedienung::schieber_setzen(s, z.x); }
            if z.ende { *g = None; }
        }
        return;
    }
    let zug = FENSTER_ZUG.lock().ok().and_then(|g| *g);
    if let Some(m) = zug.filter(|m| m.wid == z.wid) {
        let Some(display) = crate::virtual_workspace::info() else { return };
        let frame = fensterzug_rahmen(m, z.x, z.y, [
            display.x as f64, display.y as f64,
            (display.x + display.width) as f64,
            (display.y + display.height) as f64,
        ]);
        let ok = if m.art == 0 {
            fernbedienung::ax_lage(m.pid, m.wid, frame[0], frame[1])
        } else {
            let ok = fernbedienung::ax_verschieben(m.pid, m.wid, frame[0], frame[1], frame[2], frame[3]);
            // Die GEGENUEBERLIEGENDE Kante bleibt stehen - auch wenn das
            // Programm die Groesse begrenzt oder rundet (gemessen: sonst
            // wanderte beim Ziehen der linken Kante die rechte mit).
            // Die AX-Lage des Programms (sofort gueltig) - NICHT die
            // WindowServer-Liste: die hinkt Dutzende ms hinterher, und eine
            // "Korrektur" aus veralteten Werten schob das Fenster falsch.
            if let Some(ist) = fernbedienung::ax_fenster_rahmen_von(m.pid, m.wid) {
                if let Some(lage) = resize_anker(m, frame, ist) {
                    fernbedienung::ax_lage(m.pid, m.wid, lage[0], lage[1]);
                }
            }
            ok
        };
        if ok {
            befehl(&format!("geometrie {} {:.1} {:.1} {:.1} {:.1}",
                            m.wid, frame[0], frame[1], frame[2], frame[3]));
        }
        if z.ende {
            if let Ok(mut g) = FENSTER_ZUG.lock() { *g = None; }
            // Massgeblich ist der WIRKLICHE Rahmen (Programme duerfen eine
            // Groesse runden oder ablehnen) - er wird gemeldet und gesichert.
            let frame = fernbedienung::ax_fenster_rahmen_von(m.pid, m.wid).unwrap_or(frame);
            befehl(&format!("zug {} 0", m.wid));
            befehl(&format!("geometrie {} {:.1} {:.1} {:.1} {:.1}",
                            m.wid, frame[0], frame[1], frame[2], frame[3]));
            crate::workspace_fenster_geometrie(app, m.wid, frame);
            crate::virtual_workspace::trace(&format!(
                "[WINDOW_{}] end wid={} mask={} ok={ok} frame={:?} cursor_warp=false space_trip=false",
                if m.art == 0 { "MOVE" } else { "RESIZE" }, m.wid, m.art, frame));
        }
        return;
    }
    let Ok(g) = ZIEH_SITZUNG.lock() else { return };
    let Some(sitzung) = g.as_ref() else { return };
    if sitzung.wid != z.wid { return; }
    let erg = fernbedienung::auswahl_bis(sitzung, z.x, z.y);
    let editierbar = sitzung.editierbar;
    drop(g);
    if let Some((text, rechtecke)) = erg {
        if !editierbar {
            markierung(&rechtecke);
            if z.ende {
                if text.is_empty() { markierung(&[]); }
                else { crate::fern_tippen::auswahl_halten(z.pid, z.wid, text); }
            }
        }
    }
    if z.ende {
        crate::virtual_workspace::trace(&format!(
            "[AUSWAHL] end wid={} editable={editierbar} last_step_ms={}", z.wid, t0.elapsed().as_millis()
        ));
        if let Ok(mut g) = ZIEH_SITZUNG.lock() { *g = None; }
    }
}

/// Nokis Markierung fuer nicht editierbaren Seitentext (virtuelle Koordinaten).
pub fn markierung(rechtecke: &[[f64; 4]]) {
    let teile: Vec<String> = rechtecke.iter()
        .map(|r| format!("{:.0} {:.0} {:.0} {:.0}", r[0], r[1], r[2], r[3])).collect();
    befehl(&format!("markierung {}", teile.join(";")));
}

// ---------------------------------------------------------------------
//  Echte Kontextmenues als voruebergehende Ebenen der Komposition
// ---------------------------------------------------------------------

/// Offene Menue-Fenster (wid, pid) der Noki-Programme auf Noki Schreibtisch.
static MENUES: Mutex<Vec<(i64, i32)>> = Mutex::new(Vec::new());
static APP: std::sync::OnceLock<tauri::AppHandle> = std::sync::OnceLock::new();

pub fn menues() -> Vec<i64> {
    MENUES.lock().map(|g| g.iter().map(|m| m.0).collect()).unwrap_or_default()
}

pub fn menue_offen_oeffentlich() -> bool { menue_offen() }

pub fn menues_schliessen_oeffentlich() {
    menues_schliessen();
    MENUE_ERWARTET.store(10, std::sync::atomic::Ordering::SeqCst);
}

/// Druck/Loslassen aus Nokis Abgriff (globale Punkte, oben links) an die Ansicht.
pub fn tap_druck(runter: bool, x: f64, y: f64) {
    befehl(&format!("tap {} {x:.1} {y:.1}", if runter { "ab" } else { "auf" }));
}

fn menue_offen() -> bool {
    MENUES.lock().map(|g| !g.is_empty()).unwrap_or(false)
}

fn menue_pid(wid: i64) -> Option<i32> {
    MENUES.lock().ok().and_then(|g| g.iter().find(|m| m.0 == wid).map(|m| m.1))
}

/// Alle offenen Menues ohne Wirkung schliessen (Klick daneben).
fn menues_schliessen() {
    let offen: Vec<(i64, i32)> = MENUES.lock().map(|g| g.clone()).unwrap_or_default();
    for (wid, pid) in offen {
        if let Some((x, y, w, _h)) = popup_rahmen(wid) {
            let _ = fernbedienung::menue_abbrechen(pid, x + w / 2.0, y + 8.0);
        }
    }
    crate::virtual_workspace::trace("[MENU] dismissed reason=click_outside");
}

/// Popup-Menues (Ebene 101) der gegebenen Prozesse, vollstaendig auf Noki
/// Schreibtisch. Nur diese werden Teil der Komposition.
fn popup_fenster(pids: &[i32]) -> Vec<(i64, i32)> {
    let Some(d) = crate::virtual_workspace::info() else { return vec![] };
    fenster_liste(true).into_iter().filter(|f| {
        f.layer == 101 && pids.contains(&f.pid) && f.w >= 20.0 && f.h >= 10.0
            && f.x >= d.x as f64 - 1.0 && f.y >= d.y as f64 - 1.0
            && f.x + f.w <= (d.x + d.width) as f64 + 1.0 && f.y + f.h <= (d.y + d.height) as f64 + 1.0
    }).map(|f| (f.wid, f.pid)).collect()
}

fn popup_rahmen(wid: i64) -> Option<(f64, f64, f64, f64)> {
    fenster_liste(true).into_iter().find(|f| f.wid == wid).map(|f| (f.x, f.y, f.w, f.h))
}

/// Offene Menues mit der Komposition abgleichen. Guenstig (eine Fensterliste).
pub fn menues_abgleichen() {
    if crate::virtual_workspace::backend() != crate::virtual_workspace::Backend::LegacyVirtualDisplay
        || !crate::virtual_workspace::ready() || retarget_aktiv() { return; }
    let Some(app) = APP.get() else { return };
    let alt = menues();
    let registriert: Vec<i64> = aufgenommene().into_iter().filter(|w| !alt.contains(w)).collect();
    let mut pids: Vec<i32> = registriert.iter().filter_map(|w| fenster_prozess(*w)).collect();
    pids.sort_unstable(); pids.dedup();
    let neu = popup_fenster(&pids);
    let neu_ids: Vec<i64> = neu.iter().map(|m| m.0).collect();
    if neu_ids == alt { return; }
    if let Ok(mut g) = MENUES.lock() { *g = neu; }
    crate::virtual_workspace::trace(&format!("[MENU] open={:?} space_trip=false", neu_ids));
    let mut ids = registriert;
    ids.extend(neu_ids);
    let _ = setzen(app, ids, 520, 30, "virtual-display".into(), String::new());
}

fn menue_waechter(app: &tauri::AppHandle) {
    let _ = APP.set(app.clone());
    static LAEUFT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    LAEUFT.get_or_init(|| {
        let _ = std::thread::Builder::new().name("noki-menues".into()).spawn(|| loop {
            // Schnell nur, solange ein Menue offen ist oder gerade eines entstehen kann.
            let schnell = menue_offen() || MENUE_ERWARTET.load(std::sync::atomic::Ordering::SeqCst) > 0;
            // Ruhig: ein Abgleich je 1,2 s (gemessen: jeder kostet ~18 ms CPU
            // fuer die Fensterliste). Nach Klick/Rechtsklick sofort 60 ms.
            // In 60-ms-Schritten warten: ein Klick (MENUE_ERWARTET) weckt sofort.
            let schritte = if schnell { 1 } else { 50 };
            for _ in 0..schritte {
                std::thread::sleep(std::time::Duration::from_millis(60));
                if !schnell && MENUE_ERWARTET.load(std::sync::atomic::Ordering::SeqCst) > 0 { break; }
            }
            if MENUE_ERWARTET.load(std::sync::atomic::Ordering::SeqCst) > 0 {
                MENUE_ERWARTET.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            }
            if sichtbar() { menues_abgleichen(); }
        });
    });
}

/// Nach einem Rechts-/Klick kurz schneller nach neuen Menues sehen.
static MENUE_ERWARTET: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

struct FensterInfo { wid: i64, pid: i32, layer: i64, x: f64, y: f64, w: f64, h: f64 }

fn fenster_liste(nur_sichtbar: bool) -> Vec<FensterInfo> {
    use std::ffi::c_void;
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" { fn CGWindowListCopyWindowInfo(o: u32, w: u32) -> *const c_void; }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFArrayGetCount(a: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(a: *const c_void, i: isize) -> *const c_void;
        fn CFDictionaryGetValue(d: *const c_void, k: *const c_void) -> *const c_void;
        fn CFStringCreateWithCString(a: *const c_void, s: *const i8, e: u32) -> *const c_void;
        fn CFNumberGetValue(n: *const c_void, t: i32, v: *mut f64) -> bool;
        fn CFRelease(o: *const c_void);
    }
    let mut out = vec![];
    unsafe {
        let schl = |n: &str| { let c = std::ffi::CString::new(n).unwrap(); CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), 0x0800_0100) };
        let zahl = |v: *const c_void| -> f64 { let mut o = 0f64; if !v.is_null() && CFNumberGetValue(v, 13, &mut o) { o } else { -1.0 } };
        let liste = CGWindowListCopyWindowInfo(if nur_sichtbar { 1 } else { 0 }, 0);
        if liste.is_null() { return out; }
        let k = [schl("kCGWindowNumber"), schl("kCGWindowOwnerPID"), schl("kCGWindowLayer"), schl("kCGWindowBounds"),
                 schl("X"), schl("Y"), schl("Width"), schl("Height")];
        for i in 0..CFArrayGetCount(liste) {
            let w = CFArrayGetValueAtIndex(liste, i);
            let b = CFDictionaryGetValue(w, k[3]);
            if b.is_null() { continue; }
            out.push(FensterInfo {
                wid: zahl(CFDictionaryGetValue(w, k[0])) as i64,
                pid: zahl(CFDictionaryGetValue(w, k[1])) as i32,
                layer: zahl(CFDictionaryGetValue(w, k[2])) as i64,
                x: zahl(CFDictionaryGetValue(b, k[4])), y: zahl(CFDictionaryGetValue(b, k[5])),
                w: zahl(CFDictionaryGetValue(b, k[6])), h: zahl(CFDictionaryGetValue(b, k[7])),
            });
        }
        for x in k { CFRelease(x); }
        CFRelease(liste);
    }
    out
}

/// Real occlusion on the virtual display. Noki composes windows in its own
/// order, but the window server stacks them for real: a window fully covered
/// there counts as occluded, and Chromium/Electron apps then stop painting
/// AND stop updating their accessibility tree (measured: Claude under Figma
/// kept a 900x700 AX layout in a 720x520 window, its "+" menu was invisible
/// to AX, clicks and resizes had no visible effect). Sampled coverage by the
/// on-screen windows above it.
fn real_verdeckt(wid: i64) -> bool {
    let liste = fenster_liste(true);
    let Some(i) = liste.iter().position(|f| f.wid == wid) else { return false };
    let z = &liste[i];
    if z.w < 2.0 || z.h < 2.0 { return false; }
    let oben: Vec<&FensterInfo> = liste[..i].iter()
        .filter(|f| f.layer == 0 && f.w >= 2.0 && f.h >= 2.0
            && f.x < z.x + z.w && f.x + f.w > z.x && f.y < z.y + z.h && f.y + f.h > z.y)
        .collect();
    if oben.is_empty() { return false; }
    const N: usize = 16;
    let mut frei = 0;
    for a in 0..N {
        for b in 0..N {
            let px = z.x + 2.0 + (z.w - 4.0) * a as f64 / (N - 1) as f64;
            let py = z.y + 2.0 + (z.h - 4.0) * b as f64 / (N - 1) as f64;
            if !oben.iter().any(|f| px >= f.x && px < f.x + f.w && py >= f.y && py < f.y + f.h) { frei += 1; }
        }
    }
    // Chromium throttles once practically nothing is visible.
    frei * 100 <= N * N * 3
}

/// The window the user operates in Noki must be live for real: raise it in
/// the window-server stack when it is occluded, or whenever REAL_SPACE is
/// addressing a hidden-Space target. AXRaise only: no app activation, no
/// main/focus change and no Space change. Calls are limited to once per
/// target/second. Returns true when a raise happened.
fn real_freilegen(pid: i32, wid: i64, grund: &str, hidden_space_ziel: bool) -> bool {
    // Scroll bursts: one window-list read per window and second.
    static FREI: Mutex<Option<(i64, std::time::Instant)>> = Mutex::new(None);
    if FREI.lock().is_ok_and(|g| g.is_some_and(|(w, t)| w == wid && t.elapsed() < std::time::Duration::from_secs(1))) {
        return false;
    }
    if !hidden_space_ziel && !real_verdeckt(wid) {
        if let Ok(mut g) = FREI.lock() { *g = Some((wid, std::time::Instant::now())); }
        return false;
    }
    if let Ok(mut g) = FREI.lock() { *g = Some((wid, std::time::Instant::now())); }
    let ok = fernbedienung::fenster_heben(pid, wid);
    crate::virtual_workspace::trace(&format!(
        "[ZORDER] targeted_raise wid={wid} pid={pid} reason={grund} hiddenSpaceTarget={hidden_space_ziel} raised={ok} activation=false space_navigation=false"));
    if ok { std::thread::sleep(std::time::Duration::from_millis(250)); }
    ok
}

/// REAL_SPACE bring-to-front of EXACTLY this window inside its (hidden)
/// Space: AXRaise of an inactive app's window reorders WindowServer's
/// per-Space list (measured) - no activation, no Space switch, no cursor.
/// Then the compositor/hit-test/app bar re-read that native order at once.
/// Callers must have excluded the frontmost app (raising its hidden window
/// makes macOS follow it to that Space).
pub(crate) fn real_nach_vorn(pid: i32, wid: i64) -> bool {
    let sid = crate::cgs::fenster_auf_space_id(wid);
    let reihe = |s: u64| crate::cgs::fenster_reihe(s).into_iter()
        .filter(|w| aufgenommene().contains(w)).collect::<Vec<_>>();
    let vorher = sid.map(reihe).unwrap_or_default();
    let schon_vorn = vorher.first() == Some(&wid);
    let ok = schon_vorn
        || fernbedienung::fenster_heben_und_main(pid, wid)
        || fernbedienung::fenster_heben(pid, wid);
    // The native order settles a moment after AXRaise (measured: first
    // check could still show the old order).
    let mut nativ_vorn = schon_vorn;
    for _ in 0..8 {
        if nativ_vorn { break; }
        nativ_vorn = sid.map(reihe).unwrap_or_default().first() == Some(&wid);
        if !nativ_vorn { std::thread::sleep(std::time::Duration::from_millis(30)); }
    }
    // REAL_SPACE must never keep a compositor-only front override.  The
    // preview is a view of the real Space, not an alternate stacking model.
    // Keeping `wid` here made the Miniatur continue to draw the last clicked
    // window on top even when WindowServer selected another front window as
    // the user entered the Space.  Clear the legacy/virtual-display fallback
    // and let `stapel()` mirror the native per-Space order exactly.
    vorn_setzen(0);
    stapel();
    if let Some(a) = APP.get() { crate::leiste_anstossen(a); }
    crate::virtual_workspace::trace(&format!(
        "[ZORDER] front wid={wid} pid={pid} space={} was_front={schon_vorn} raised={ok} native_top={nativ_vorn} activation=false",
        sid.unwrap_or(0)));
    if !schon_vorn && ok { std::thread::sleep(std::time::Duration::from_millis(80)); }
    nativ_vorn || ok
}

/// EIN Vorgang: Klick und/oder gesammeltes Rollen. Rueckgabe: ob ein Weg
/// wirklich ausgefuehrt wurde, und welcher.
fn fern_vorgang(
    _app: &tauri::AppHandle,
    klick: Option<Vec<FernKlick>>,
    rad: Option<FernRad>,
) -> (bool, &'static str) {
    // Das Fenster kann seit dem Ablegen verschwunden sein.
    let klick = klick.map(|v| {
        v.into_iter().filter(|k| ziel_erlaubt(k.wid) == Some(k.pid)).collect::<Vec<_>>()
    }).filter(|v| !v.is_empty());
    let rad = rad.filter(|r| ziel_erlaubt(r.wid) == Some(r.pid));
    let target_wid = klick.as_ref().and_then(|v| v.first()).map(|k| k.wid)
        .or_else(|| rad.as_ref().map(|r| r.wid)).unwrap_or(0);
    if klick.is_none() && rad.is_none() {
        // Sichtbar machen, warum eine Eingabe nichts bewirkt (z. B. kurz
        // nach dem Start, bevor die Aufnahme das Fenster kennt).
        static ZULETZT: Mutex<Option<std::time::Instant>> = Mutex::new(None);
        if ZULETZT.lock().is_ok_and(|mut z| {
            let neu = z.is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(2));
            if neu { *z = Some(std::time::Instant::now()); }
            neu
        }) {
            crate::virtual_workspace::trace(&format!("[FERN] dropped reason=target_not_allowed retarget={} captured={}",
                RETARGET_AKTIV.load(std::sync::atomic::Ordering::SeqCst), aufgenommene().len()));
        }
        return (false, "verworfen");
    }
    // The owning lane publishes its own phase. Keeping it out of this
    // shared routine prevents click and scroll workers from clearing each
    // other's busy state.
    // Jeder Klick kann ein Menue/Aufklappfeld oeffnen: kurz schnell nachsehen
    // (im Ruhezustand prueft der Menue-Faden nur alle 3 s).
    if klick.is_some() { MENUE_ERWARTET.fetch_max(8, std::sync::atomic::Ordering::SeqCst); }
    for pid in klick.iter().flatten().map(|k| k.pid).chain(rad.iter().map(|r| r.pid)) {
        fernbedienung::hauptfenster_leihen(pid);
    }
    let t0 = std::time::Instant::now();
    let vorher = fernbedienung::zeigerstand();

    // The virtual backend is a separate interaction architecture.  It must
    // never enter `auf_arbeitsplatz_handeln`, raise `blende`, navigate a
    // Space, or run an origin restore loop.  While its startup is incomplete
    // (or failed), the already-consumed miniature event simply has no remote
    // effect: the physical Desktop remains byte-for-byte untouched.
    if crate::virtual_workspace::backend()
        == crate::virtual_workspace::Backend::LegacyVirtualDisplay
    {
        if !crate::virtual_workspace::ready() {
            crate::virtual_workspace::trace(&format!(
                "[FERN] backend=virtual state={:?} input=safely_consumed action=none",
                crate::virtual_workspace::readiness()
            ));
            return (false, "virtual-not-ready");
        }
        let mut delivered = false;
        if let Some(clicks) = klick.as_ref() {
            for k in clicks {
                // Offenes Menue: ein Klick darauf waehlt einen ECHTEN Eintrag,
                // ein Klick daneben schliesst es nur (wie auf dem Mac).
                if menue_pid(k.wid).is_some() {
                    let ok = k.klicks > 0 && fernbedienung::menue_klick(k.pid, k.x, k.y);
                    crate::virtual_workspace::trace(&format!("[MENU] item_press ok={ok}"));
                    MENUE_ERWARTET.store(10, std::sync::atomic::Ordering::SeqCst);
                    delivered |= ok;
                    continue;
                }
                // Occluded for real (frozen renderer, stale AX): make it live
                // before resolving the target.
                real_freilegen(k.pid, k.wid, "click", false);
                if menue_offen() {
                    menues_schliessen();
                    MENUE_ERWARTET.store(10, std::sync::atomic::Ordering::SeqCst);
                    delivered = true;
                    continue;
                }
                if k.klicks < 0 {
                    // Rechtsklick: nie ein normaler Klick. Das echte Menue.
                    crate::fern_tippen::warten_beenden("right_click");
                    let pid = k.pid;
                    let ok = fernbedienung::kontextmenue(k.pid, k.wid, k.x, k.y, &|| !popup_fenster(&[pid]).is_empty());
                    crate::virtual_workspace::trace(&format!("[MENU] right_click wid={} show_menu={ok} space_trip=false", k.wid));
                    MENUE_ERWARTET.store(12, std::sync::atomic::Ordering::SeqCst);
                    if ok { menues_abgleichen(); }
                    delivered |= ok;
                    continue;
                }
                // Ein Klick auf ein Eingabefeld fokussiert es und gibt die
                // Tastatur ausdruecklich an genau dieses Feld (fern_tippen).
                if k.klicks <= 1 {
                    if let Some((feld, vorher)) = fernbedienung::eingabe_fokussieren(k.pid, k.wid, k.x, k.y) {
                        crate::fern_tippen::beginnen(k.pid, k.wid, feld, vorher);
                        delivered = true;
                        continue;
                    }
                }
                // Ein Klick in ein anderes Fenster macht DIESES zum vorderen
                // (wie auf dem Mac) - auch auf Ebene der Komposition.
                if vorn() != 0 && vorn() != k.wid { vorn_setzen(k.wid); }
                // Ein Klick auf etwas anderes als ein Eingabefeld beendet
                // das Tippen - wie ein Klick daneben auf dem echten Desktop.
                crate::fern_tippen::beenden("remote_click_elsewhere");
                crate::fern_tippen::warten_beenden("remote_click_elsewhere");
                let semantisch = fernbedienung::ax_klick_fenster(k.pid, k.wid, k.x, k.y, k.klicks)
                    || fernbedienung::schieber_beginnen(k.pid, k.wid, k.x, k.y)
                        .is_some_and(|s| fernbedienung::schieber_setzen(&s, k.x));
                if semantisch {
                    delivered = true;
                } else {
                    // Some Electron/Chromium editors expose only an
                    // AXGroup/AXImage before the native click. Deliver the
                    // exact-window PID event, then re-read the focused AX
                    // element: contenteditable often becomes visible only
                    // now. This is still cursor-free and window-scoped.
                    let pointer = fernbedienung::zeiger_klick(k.pid, k.wid, k.x, k.y, k.klicks);
                    if pointer && k.klicks <= 1 {
                        std::thread::sleep(std::time::Duration::from_millis(20));
                        if let Some(feld) = fernbedienung::fokus_eingabe(k.pid, k.wid) {
                            crate::fern_tippen::beginnen(k.pid, k.wid, feld, None);
                            crate::virtual_workspace::trace(
                                "[TIPPEN] capture=post_targeted_mouse focused_editable=true"
                            );
                        }
                    }
                    delivered |= pointer;
                }
            }
        }
        if rad.is_some() && menue_offen() {
            menues_schliessen();
        } else if let Some(r) = rad {
            real_freilegen(r.pid, r.wid, "scroll", false);
            // Nokis Markierung auf Seitentext folgt dem Rollen nicht.
            crate::fern_tippen::auswahl_verwerfen("scroll");
            // Arrow scrolling is used only after the AX focus has been
            // moved to, and re-read as, the exact target document.  There
            // is deliberately no wheel/page fallback here: a video player,
            // text field or slider must fail closed rather than seek, edit,
            // or change volume.
            // Browser-Dokument: der gemessene, fluessige Pfeil-Weg. Sonst -
            // und wenn der Pfeil-Weg ablehnt - der allgemeine Roll-Container
            // unter dem Zeiger (jedes Programm mit Accessibility-Baum).
            // SCROLL IS SCROLL: a wheel/trackpad gesture moves the page or
            // list under the pointer - never a slider, never media keys.
            //   1. web documents (browsers AND Chrome PWAs such as YouTube):
            //      pointer over media/value control -> key-free document
            //      steps (AXScrollToVisible); otherwise verified document
            //      focus + vertical arrows (never while a player/field has
            //      focus - ax_pfeil_scroll fails closed then, and we fall
            //      back to the key-free route);
            //   2. everything else: the generic container route, which
            //      itself refuses focus+arrows over media.
            // Magnitude: finger pixels accumulate per window; one 40 px step
            // per 40 px of travel (was >=1 arrow for every 4 px packet).
            let web = ist_browser(r.pid) || ist_pwa(r.pid);
            if web && r.dx.abs() <= r.dy.abs() {
                let (schritte, medien) = roll_schritte(r.wid, r.dy,
                    || fernbedienung::medien_unter_zeiger(r.pid, r.wid, r.x, r.y));
                if schritte == 0 {
                    delivered = true; // accumulating a small movement
                } else {
                    // Key-free steps move ~half a viewport each: one per
                    // 6 finger steps (~240 px), never more per batch.
                    delivered |= (!medien && fernbedienung::ax_pfeil_scroll_fenster(r.pid, r.wid, 0, schritte))
                        || { let k = keyfrei_schritte(schritte); k == 0 || fernbedienung::seite_ohne_tasten(r.pid, r.wid, k) };
                    crate::virtual_workspace::trace(&format!(
                        "[SCROLL] wid={} steps={schritte} media_under_pointer={medien} route={} keys_over_media=false",
                        r.wid, if medien { "keyfree_document" } else { "document" }));
                }
            } else {
                delivered |= fernbedienung::rollen_am_punkt(r.pid, r.wid, r.x, r.y, r.dx, r.dy);
            }
        }
        let after = fernbedienung::zeigerstand();
        crate::virtual_workspace::trace(&format!(
            "[FERN] backend=virtual direct={} shield=false space_trip=false cursor_delta={:.1},{:.1}",
            delivered, after.x - vorher.x, after.y - vorher.y
        ));
        return (delivered, if delivered { "virtual-direct" } else { "sicher-verworfen" });
    }

    // REAL_SPACE remote interaction is forbidden from visiting the target
    // Space, showing a shield, or activating an off-Space application.  It
    // uses only exact-window AX/PID delivery. Unsupported controls fail
    // closed; the current physical Space remains the invariant.
    let space_vorher = crate::cgs::aktiver_space().map(|s| s.0).unwrap_or(0);
    fern_markieren();
    let mut getan = false;
    // Measured twice (2026-09-26, SPACE_AUDIT): raising / making main a
    // hidden window of the FRONTMOST app makes macOS follow it to that
    // Space (Terminal was active -> 1->2796 167 ms after the click). An
    // inactive app is not followed. So the frontmost app's hidden window is
    // never touched from the Miniatur: fail closed, say why.
    let vorne = vorderstes_programm();
    let vorne_gesperrt = |pid: i32, wid: i64, was: &str| -> bool {
        if pid != vorne || crate::cgs::fenster_auf_space(space_vorher).iter().any(|f| f.0 == wid) { return false; }
        crate::virtual_workspace::trace(&format!(
            "[SAFETY] {was} target_app_frontmost pid={pid} wid={wid} action=fail_closed reason=raise_would_switch_space"));
        let name = crate::lesezeichen::app_fuer_pid(pid).map(|a| a.0).unwrap_or_default();
        hinweis(&format!("{name} ist gerade vorne aktiv – in der Miniatur erst nach einem Wechsel zu einem anderen Programm bedienbar."));
        true
    };
    if let Some(clicks) = klick.as_ref() {
        for k in clicks {
            let _frist = fernbedienung::FristGuard::neu(1200);
            if vorne_gesperrt(k.pid, k.wid, "click") { continue; }
            // Right click: the Noki Browser page gets its own contextmenu
            // event (page menus). Native context menus of a hidden app are a
            // modal NSMenu outside the Noki Space - never opened from here.
            if k.klicks < 0 {
                let seite = crate::noki_browser::ist_noki_browser(k.pid)
                    && crate::noki_browser::inhalt_oben(k.wid).is_some_and(|o| k.y >= o);
                if seite {
                    real_nach_vorn(k.pid, k.wid);
                    crate::fern_tippen::beenden("remote_click_elsewhere");
                    crate::noki_browser::klicken(k.pid, k.wid, k.x, k.y, -1);
                    befehl(&format!("lebhaft {}", k.wid));
                    getan = true;
                } else {
                    crate::virtual_workspace::trace(&format!(
                        "[CAPABILITY] right_click wid={} class=NATIVE_ONLY reason=native_context_menu", k.wid));
                    hinweis("Kontextmenüs dieses Programms sind nur auf dem Noki Schreibtisch verfügbar.");
                }
                continue;
            }
            // Noki Browser: page-area clicks go to the SAME page as trusted
            // page-level mouse events on the browser's own queue (no OS
            // event, no app activation, no shared-worker stall). Tab strip and
            // toolbar stay on the AX route below.
            if crate::noki_browser::ist_noki_browser(k.pid) {
                let rahmen = crate::noki_browser::rahmen(k.wid).unwrap_or_default();
                let oben = BROWSER_OBEN.lock().ok().and_then(|g| g.iter().find(|e| e.0 == k.wid && e.2 == rahmen).map(|e| e.1));
                let oben = oben.or_else(|| {
                    let o = crate::noki_browser::inhalt_oben(k.wid)?;
                    if let Ok(mut g) = BROWSER_OBEN.lock() { g.retain(|e| e.0 != k.wid); g.push((k.wid, o, rahmen)); }
                    Some(o)
                });
                if oben.is_some_and(|o| k.y < o) && k.klicks <= 1 {
                    // Browser UI: the address field gets the browser's own
                    // address mode (navigate via DevTools on Enter).
                    let (rolle, _) = fernbedienung::rolle_am_punkt(k.pid, k.wid, k.x, k.y);
                    if rolle == "AXTextField" {
                        real_nach_vorn(k.pid, k.wid);
                        crate::fern_tippen::beginnen_browser_adresse(k.pid, k.wid);
                        getan = true;
                        continue;
                    }
                }
                if oben.is_some_and(|o| k.y >= o) {
                    real_nach_vorn(k.pid, k.wid);
                    crate::fern_tippen::beenden("remote_click_elsewhere");
                    crate::noki_browser::klicken(k.pid, k.wid, k.x, k.y, k.klicks);
                    befehl(&format!("lebhaft {}", k.wid));
                    getan = true;
                    continue;
                }
            }
            // An open popover of the same app above the clicked window: a
            // real outside click only dismisses it (measured GoodNotes: the
            // AX press on the tile below left the popover stuck open).
            if k.klicks >= 1 && popover_ausserhalb_schliessen(k.pid, k.wid, k.x, k.y) {
                getan = true;
                continue;
            }
            // Like a real Desktop: the clicked window comes to the front
            // (natively, on its hidden Space) and the SAME click is then
            // executed at that point - never a second click.
            real_nach_vorn(k.pid, k.wid);
            // A double click INSIDE the text target being typed into is a
            // normal text gesture (word selection) - it must not end typing.
            // Measured 2026-09-26: the user's double click into the Noki
            // Terminal ended the session (`remote_click_elsewhere`).
            let im_tippziel = k.klicks == 2 && crate::fern_tippen::ziel_wid() == Some(k.wid);
            if k.klicks <= 1 || im_tippziel {
                if let Some((feld, vorher_text)) =
                    fernbedienung::eingabe_fokussieren(k.pid, k.wid, k.x, k.y)
                {
                    // VS Code with its Companion bridge: the verified field
                    // (editor vs. terminal) is the target; keys go through
                    // the VS Code API. No caret is guessed from pixels - the
                    // window's real selection is kept.
                    if crate::vscode_bruecke::ist_vscode(k.pid)
                        && crate::vscode_bruecke::fuer_fenster(k.wid).is_some()
                    {
                        crate::fern_tippen::vscode_art_vorgeben(
                            k.wid, fernbedienung::vscode_terminal_am_punkt(k.pid, k.wid, k.x, k.y));
                        crate::fern_tippen::beginnen(k.pid, k.wid, feld, vorher_text);
                        getan = true;
                        continue;
                    }
                    // Focus-only is enough for Terminal; Electron also needs
                    // the real click to place its caret/selection. Address it
                    // to this exact CGWindow, never the global cursor/Space.
                    if chromium_ereignis_app(k.pid) {
                        fernbedienung::zeiger_klick(k.pid, k.wid, k.x, k.y, k.klicks);
                        std::thread::sleep(std::time::Duration::from_millis(25));
                    }
                    if fernbedienung::fokus_ist(k.pid, k.wid, &feld) {
                        crate::fern_tippen::beginnen(k.pid, k.wid, feld, vorher_text);
                    } else if !chromium_ereignis_app(k.pid) {
                        if let Some(neu) = fernbedienung::fokus_eingabe(k.pid, k.wid) {
                            crate::fern_tippen::beginnen(k.pid, k.wid, neu, vorher_text);
                        } else {
                            crate::virtual_workspace::trace(&format!(
                                "[TIPPEN] wid={} route=exact_window_pointer verified=false ownership=none", k.wid));
                        }
                    } else {
                        crate::virtual_workspace::trace(&format!(
                            "[TIPPEN] wid={} route=exact_window_pointer verified=false ownership=none", k.wid));
                    }
                    befehl(&format!("lebhaft {}", k.wid));
                    getan = true;
                    continue;
                }
            }
            crate::fern_tippen::beenden("remote_click_elsewhere");
            crate::fern_tippen::warten_beenden("remote_click_elsewhere");
            // Window-scoped only. The app-wide generic press walked the
            // app's AX tree - which off-Space contains ONLY the user's own
            // windows on the current Desktop (AXWindows omits hidden ones),
            // so it could press a button in the user's window at the same
            // coordinates. The exact hidden window is now resolved via its
            // remote token (`ax_fenster_element`).
            let semantisch = fernbedienung::ax_klick_fenster(k.pid, k.wid, k.x, k.y, k.klicks);
            // A native menu (NSMenu, layer 101) opened by that press lives on
            // the hidden Space: invisible in the Miniatur while the app sits
            // in modal menu tracking (measured 2026-09-27, Notes "Weitere").
            // Close it at once (Escape to exactly that process) and say why.
            if semantisch {
                for _ in 0..4 {
                    std::thread::sleep(std::time::Duration::from_millis(40));
                    if fernbedienung::menue_offen_von(k.pid) {
                        fernbedienung::taste_mit_text(k.pid, 53, "");
                        crate::virtual_workspace::trace(&format!(
                            "[CAPABILITY] menu wid={} class=NATIVE_ONLY action=closed_hidden_menu", k.wid));
                        hinweis("Menüs dieses Programms sind nur auf dem Noki Schreibtisch verfügbar.");
                        break;
                    }
                }
            }
            // No pointer-event fallback under REAL_SPACE: a process-targeted
            // mouse event was measured to RESIZE a Catalyst window (GoodNotes)
            // and cannot be verified. Unsupported stays unsupported.
            if !semantisch && chromium_ereignis_app(k.pid) {
                // Monaco/xterm custom surfaces often expose no press action
                // until a native mouse-down/up selects their editable node.
                // Grant keyboard ownership only after exact-window AX proof.
                let pointer = fernbedienung::zeiger_klick(k.pid, k.wid, k.x, k.y, k.klicks);
                // Input controls never arrive here: they are resolved to a
                // point-specific candidate above. Do not adopt any old
                // focused field elsewhere in the window after this generic
                // click (editor click -> old terminal was the regression).
                getan |= pointer;
            } else if !semantisch {
                crate::virtual_workspace::trace(&format!(
                    "[CAPABILITY] click wid={} clicks={} result={}", k.wid, k.klicks,
                    if k.klicks >= 2 { "NATIVE_ONLY" } else { "NO_REMOTE_TARGET" }));
                if k.klicks >= 2 {
                    hinweis("Diese Aktion ist nur auf dem Noki Schreibtisch verfügbar.");
                }
            }
            befehl(&format!("lebhaft {}", k.wid));
            getan |= semantisch;
        }
    }
    if let Some(r) = rad.filter(|r| !vorne_gesperrt(r.pid, r.wid, "scroll")) {
        // SCROLL IS SCROLL: no raise (a real Desktop does not raise a window
        // you scroll), no focus, no keys. Measured 2026-09-26: AXFocused on
        // web content in the hidden Safari window made macOS switch to the
        // Noki Space - the "scroll enters the Desktop" report.
        crate::fern_tippen::auswahl_verwerfen("scroll");
        // Web content (Safari/Chromium/PWA): each semantic step is a
        // Chromium/WebKit "scroll into view" of ~half a viewport. One step
        // per ~45 wheel units (was: every packet) - less teleport-like and
        // not over-sensitive. Direction reversal resets at once.
        let _frist = fernbedienung::FristGuard::neu(700);
        if crate::noki_browser::ist_noki_browser(r.pid) {
            // Exact incremental page scroll on the browser's own queue:
            // helper units are finger points * 0.25; natural direction.
            crate::noki_browser::rollen(r.wid, r.x as f64, r.y as f64, -(r.dx as f64) * 6.4, -(r.dy as f64) * 6.4);
            befehl(&format!("lebhaft {}", r.wid));
            getan = true;
        } else {
        let web = ist_browser(r.pid) || ist_pwa(r.pid);
        let schritt_frei = !web || web_roll_akku(r.wid, r.dy);
        // A window whose AX scroll route missed once scrolls through the
        // process-addressed wheel from then on (the boundary cache would
        // otherwise swallow every following packet as "handled").
        static PID_RAD: Mutex<Vec<i64>> = Mutex::new(Vec::new());
        let pid_rad = !web && PID_RAD.lock().is_ok_and(|g| g.contains(&r.wid));
        // Per-window circuit breaker. Two over-budget semantic attempts
        // quarantine only this window's AX route for 30 seconds; browser,
        // click, typing, shortcuts and every other app remain independent.
        static AX_KREIS: Mutex<Vec<(i64, u8, Option<std::time::Instant>)>> = Mutex::new(Vec::new());
        let watchdog_offen = RAD_ISOLIERT_WID.load(std::sync::atomic::Ordering::Relaxed) == r.wid
            && RAD_ISOLIERT_BIS_MS.load(std::sync::atomic::Ordering::Relaxed) > jetzt_ms();
        let kreis_offen = watchdog_offen || AX_KREIS.lock().is_ok_and(|mut g| {
            g.retain(|(_, _, bis)| bis.is_none_or(|t| t > std::time::Instant::now()));
            g.iter().any(|(w, _, bis)| *w == r.wid && bis.is_some())
        });
        let ax_start = std::time::Instant::now();
        getan |= if pid_rad || kreis_offen {
            fernbedienung::pfeil_rollen(r.pid, r.wid, r.x, r.y, r.dy)
                || fernbedienung::rad_an_prozess(r.pid, r.wid, r.x, r.y, r.dx, r.dy)
        } else {
            let ax = !schritt_frei || fernbedienung::ax_rad_fenster(r.pid, r.wid, r.dx, r.dy)
                || fernbedienung::rollen_am_punkt(r.pid, r.wid, r.x, r.y, r.dx, r.dy);
            if !ax {
                if let Ok(mut g) = PID_RAD.lock() { if !g.contains(&r.wid) { g.push(r.wid); if g.len() > 64 { g.remove(0); } } }
                fernbedienung::pfeil_rollen(r.pid, r.wid, r.x, r.y, r.dy)
                    || fernbedienung::rad_an_prozess(r.pid, r.wid, r.x, r.y, r.dx, r.dy)
            } else { true }
        };
        if !pid_rad && !kreis_offen {
            let langsam = ax_start.elapsed() > std::time::Duration::from_millis(700);
            if let Ok(mut g) = AX_KREIS.lock() {
                if let Some(e) = g.iter_mut().find(|e| e.0 == r.wid) {
                    e.1 = if langsam { e.1.saturating_add(1) } else { 0 };
                    if e.1 >= 2 {
                        e.2 = Some(std::time::Instant::now() + std::time::Duration::from_secs(30));
                        crate::virtual_workspace::trace(&format!(
                            "[CIRCUIT] subsystem=AX_SCROLL wid={} state=open seconds=30 isolated=true", r.wid));
                    }
                } else {
                    g.push((r.wid, u8::from(langsam), None));
                }
                if g.len() > 64 { g.remove(0); }
            }
        }
        }
        befehl(&format!("lebhaft {}", r.wid));
    }
    fernbedienung::frist_loeschen();
    let space_nachher = crate::cgs::aktiver_space().map(|s| s.0).unwrap_or(0);
    let weg = if getan { "real-space-direct" } else { "sicher-verworfen" };
    crate::virtual_workspace::trace(&format!(
        "[PREVIEW_INPUT] class=MINIATURE_CONTENT_INTERACTION currentPhysicalSpaceID={space_vorher} afterPhysicalSpaceID={space_nachher} spaceTrip={} method={weg}",
        space_vorher != space_nachher
    ));
    fern_markieren();
    if space_vorher != 0 && space_nachher != space_vorher {
        // Diagnostic only. A watchdog return was itself an unrequested Space
        // switch and could keep pulling the user back to one fixed Desktop.
        // The triggering operation must fail closed at its source instead.
        crate::space_action_log("miniature_interaction_violation_no_recovery", "target_app", target_wid, space_vorher, space_nachher);
    }
    eprintln!(
        "[FERNZEIT] semantic={}ms klick={} rad={}",
        t0.elapsed().as_millis(), getan && klick.is_some(), getan && rad.is_some()
    );
    if t0.elapsed() > std::time::Duration::from_millis(1500) {
        crate::virtual_workspace::trace(&format!(
            "[HEALTH] interaction_worker_slow ms={} click={} scroll={}", t0.elapsed().as_millis(), klick.is_some(), rad.is_some()));
    }
    let nachher = fernbedienung::zeigerstand();
    eprintln!(
        "[FERNCURSOR] before={:.1},{:.1} during=unveraendert after={:.1},{:.1} delta={:.1},{:.1}",
        vorher.x, vorher.y, nachher.x, nachher.y,
        nachher.x - vorher.x, nachher.y - vorher.y
    );
    (getan, weg)
}

#[cfg(test)]
mod fern_tests {
    use super::*;

    fn k(x: f64) -> FernKlick {
        FernKlick { pid: 1, wid: 2, x, y: 0.0, klicks: 1, seq: x as u64 }
    }
    fn r(dy: i32) -> FernRad {
        FernRad { pid: 1, wid: 2, x: 5.0, y: 5.0, dx: 0, dy }
    }

    #[test]
    fn hoechstens_ein_wartender_klick() {
        let mut a = FernArbeit::default();
        assert!(!a.klick_ablegen(k(1.0)));
        assert!(a.klick_ablegen(k(2.0)));
        assert!(a.klick_ablegen(k(3.0)));
        let (kl, _) = a.entnehmen();
        assert_eq!(kl.unwrap().iter().map(|k| k.x).collect::<Vec<_>>(), vec![1.0, 2.0, 3.0]);
        assert!(a.leer());
    }

    #[test]
    fn rollen_wird_zu_einer_summe() {
        let mut a = FernArbeit::default();
        for _ in 0..40 {
            a.rad_ablegen(r(-3));
        }
        let (_, rd) = a.entnehmen();
        assert_eq!(rd.map(|r| r.dy), Some(-120));
        assert!(a.leer() && a.rad_seit.is_none());
    }

    #[test]
    fn nullsumme_ist_keine_arbeit() {
        let mut a = FernArbeit::default();
        a.rad_ablegen(r(4));
        a.rad_ablegen(r(-4));
        let (kl, rd) = a.entnehmen();
        assert!(kl.is_none() && rd.is_none());
    }

    #[test]
    fn klick_und_rollen_in_einem_vorgang() {
        let mut a = FernArbeit::default();
        a.rad_ablegen(r(2));
        a.klick_ablegen(k(1.0));
        let (kl, rd) = a.entnehmen();
        assert!(kl.is_some() && rd.is_some());
    }

    #[test]
    fn real_space_fernweg_hat_keinen_space_besuch_oder_blende() {
        let quelle = include_str!("vorschau.rs");
        let start = quelle.find("// REAL_SPACE remote interaction").unwrap();
        let end = quelle[start..].find("\n#[cfg(test)]").map(|i| start + i).unwrap();
        let real = &quelle[start..end];
        assert!(!real.contains("auf_arbeitsplatz_handeln("));
        assert!(!real.contains("blende::"));
        assert!(!real.contains("noki_interaction_aktivieren("));
        assert!(real.contains("space_vorher"));
        assert!(real.contains("space_nachher"));
    }
}

fn leser(app: tauri::AppHandle, aus: std::process::ChildStdout, helper_pid: u32) {
    menue_waechter(&app);
    let mut r = BufReader::new(aus);
    let mut kopf = Vec::new();
    loop {
        kopf.clear();
        match r.read_until(b'\n', &mut kopf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let zeile = String::from_utf8_lossy(&kopf).trim_end().to_owned();
        HELFER_LETZTE_NACHRICHT_MS.store(jetzt_ms(), std::sync::atomic::Ordering::Release);
        let teile: Vec<&str> = zeile.split(' ').collect();
        match teile.first().copied() {
            Some("PULSE") => {
                PRAESENTATION_HEARTBEAT_MS.store(jetzt_ms(), std::sync::atomic::Ordering::Release);
                let wert = |name: &str| teile.windows(2)
                    .find(|p| p[0] == name).and_then(|p| p[1].parse::<u64>().ok());
                if wert("capture") == Some(1) {
                    CAPTURE_HEARTBEAT_MS.store(jetzt_ms(), std::sync::atomic::Ordering::Release);
                }
                if let Some(generation) = wert("generation") {
                    COMPOSITOR_GENERATION.store(generation, std::sync::atomic::Ordering::Release);
                }
            }
            Some("FRAME") if teile.len() == 3 => {
                let id: i64 = teile[1].parse().unwrap_or(0);
                let n: usize = teile[2].parse().unwrap_or(0);
                if n == 0 || n > 12_000_000 {
                    break;
                }
                let mut buf = vec![0u8; n];
                if r.read_exact(&mut buf).is_err() {
                    break;
                }
                let _ = app.emit(
                    "noki://vorschau_bild",
                    serde_json::json!({ "fenster": id, "jpg": b64(&buf) }),
                );
            }
            // Der Schreibtisch selbst: Groesse und Hintergrundbild. Beides
            // kommt einmal je Lauf und macht aus Fensterbildern eine Szene.
            Some("DESK") if teile.len() == 5 => {
                let z = |i: usize| teile[i].parse::<i64>().unwrap_or(0);
                let _ = app.emit(
                    "noki://vorschau_schreibtisch",
                    serde_json::json!({ "x": z(1), "y": z(2), "w": z(3), "h": z(4) }),
                );
            }
            Some("WALL") if teile.len() == 2 => {
                let n: usize = teile[1].parse().unwrap_or(0);
                if n == 0 || n > 24_000_000 {
                    break;
                }
                let mut buf = vec![0u8; n];
                if r.read_exact(&mut buf).is_err() {
                    break;
                }
                let _ = app.emit(
                    "noki://vorschau_wand",
                    serde_json::json!({ "jpg": b64(&buf) }),
                );
            }
            Some("WALLNONE") => {
                let _ = app.emit("noki://vorschau_wand", serde_json::json!({ "jpg": "" }));
            }
            // Wo das Fenster auf dem echten Schreibtisch liegt.
            Some("META") if teile.len() == 7 => {
                let z = |i: usize| teile[i].parse::<i64>().unwrap_or(0);
                let _ = app.emit(
                    "noki://vorschau_ort",
                    serde_json::json!({
                        "fenster": z(1), "x": z(2), "y": z(3),
                        "w": z(4), "h": z(5), "app": teile[6].replace('_', " ")
                    }),
                );
            }
            Some("GONE") if teile.len() == 2 => {
                let _ = app.emit(
                    "noki://vorschau_weg",
                    serde_json::json!({ "fenster": teile[1].parse::<i64>().unwrap_or(0) }),
                );
            }
            // Geschlossen - vom Helfer gegen WindowServer geprueft. Anders als
            // GONE (kein Bild) heisst das: das Fenster gibt es nicht mehr.
            Some("DEAD") if teile.len() == 2 => {
                eprintln!("[VORSCHAU] Fenster {} geschlossen", teile[1]);
                let _ = app.emit(
                    "noki://vorschau_weg",
                    serde_json::json!({ "fenster": teile[1].parse::<i64>().unwrap_or(0), "tot": true }),
                );
            }
            Some("FRESH") => {
                CAPTURE_HEARTBEAT_MS.store(jetzt_ms(), std::sync::atomic::Ordering::Release);
                static ZULETZT: Mutex<Option<std::time::Instant>> = Mutex::new(None);
                if let Ok(mut z) = ZULETZT.lock() {
                    if z.is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(5)) {
                        *z = Some(std::time::Instant::now());
                        let _ = app.emit("noki://vorschau_nativ", serde_json::json!({ "an": true }));
                    }
                }
                // Frische-Zeilen kommen jede Sekunde fuer jedes Fenster:
                // im Protokoll nur alle 30 s ein Satz - und jede Stockung.
                static SATZ: Mutex<Option<std::time::Instant>> = Mutex::new(None);
                let stockt = zeile.split_whitespace()
                    .find_map(|t| t.strip_prefix("lastSourceFrameAge="))
                    .and_then(|v| v.parse::<f64>().ok())
                    .is_some_and(|a| a > 1.5);
                let im_satz = SATZ.lock().is_ok_and(|mut z| {
                    match *z {
                        Some(t) if t.elapsed() < std::time::Duration::from_millis(100) => true,
                        Some(t) if t.elapsed() < std::time::Duration::from_secs(30) => false,
                        _ => { *z = Some(std::time::Instant::now()); true }
                    }
                });
                if stockt || im_satz || zeile.starts_with("FRESH WIN") {
                    crate::virtual_workspace::trace(&format!("[VORSCHAU] {zeile}"));
                }
            }
            // FERNBEDIENUNG. Der Helfer hat bereits umgerechnet und
            // getroffen - er weiss als einziger, welche Ebene der Nutzer
            // WIRKLICH gesehen hat. Hier wird nur noch geprueft, ob dieses
            // Fenster bedient werden darf, und zugestellt.
            // Zeiger schwebt ueber dem Kopfband eines Noki-Fensters: nur die
            // Frage "ziehbar?" fuer den Mauszeiger. Keine Eingabe, kein Ereignis.
            // Ask Noki (Noki's own window): its own exact-window route,
            // never the generic PID/AX/activation path (ask_fern).
            Some("ZEIGER") if teile.len() >= 5
                && crate::ask_fern::ist_ask(teile[2].parse::<i64>().unwrap_or(0)) => {
                let z = |i: usize| teile[i].parse::<f64>().unwrap_or(0.0);
                crate::ask_fern::zeiger(&app, teile[1], teile[2].parse::<i64>().unwrap_or(0), z(3), z(4), &teile);
            }
            Some("ZEIGER") if teile.len() >= 5 && teile[1] == "schwebe" => {
                let z = |i: usize| teile[i].parse::<i64>().unwrap_or(0);
                if let Some(pid) = ziel_erlaubt(z(2)) {
                    schwebe_ablegen(pid, z(2), z(3) as f64, z(4) as f64);
                }
            }
            Some("ZEIGER") if teile.len() >= 6 => {
                let z = |i: usize| teile[i].parse::<i64>().unwrap_or(0);
                let wid = z(2);
                let (x, y) = (z(3) as f64, z(4) as f64);
                // Scroll packets (60-120/s) have no listener in the UI.
                if teile[1] != "radf" { let _ = app.emit(
                    "noki://vorschau_zeiger",
                    serde_json::json!({
                        "was": teile[1], "fenster": wid, "x": z(3), "y": z(4),
                        "a": z(5), "b": if teile.len() > 6 { z(6) } else { 0 }
                    }),
                ); }
                // NUR ablegen - dieser Faden holt die Bilder ab und darf nie
                // auf einen Schreibtischwechsel warten.
                match ziel_erlaubt(wid) {
                    Some(pid) => match teile[1] {
                        "klick" => fern_klick_ablegen(
                            &app,
                            FernKlick {
                                pid, wid, x, y, klicks: z(5).clamp(1, 3),
                                seq: if teile.len() > 6 { z(6).max(0) as u64 } else { 0 },
                            },
                        ),
                        "rechts" => fern_klick_ablegen(
                            &app,
                            FernKlick { pid, wid, x, y, klicks: -1, seq: 0 },
                        ),
                        // Noki Browser page: hover and drags as page mouse
                        // events on the browser's own queue (sliders, seek
                        // bars, hover-only controls). Other apps: no route.
                        "bewege" if crate::noki_browser::ist_noki_browser(pid) =>
                            crate::noki_browser::zeiger(wid, x, y, 0),
                        w @ ("ziehen_an" | "ziehen" | "ziehen_aus") if crate::virtual_workspace::backend()
                            == crate::virtual_workspace::Backend::RealSpace
                            && crate::noki_browser::ist_noki_browser(pid) =>
                            crate::noki_browser::zeiger(wid, x, y, match w { "ziehen_an" => 1, "ziehen" => 2, _ => 3 }),
                        w @ ("ziehen_an" | "ziehen" | "ziehen_aus") if crate::virtual_workspace::backend()
                            == crate::virtual_workspace::Backend::LegacyVirtualDisplay => fern_ziehen_ablegen(
                            &app,
                            FernZiehen { pid, wid, x, y, anfang: w == "ziehen_an",
                                         ende: w == "ziehen_aus", art: z(5) },
                        ),
                        "rad" if teile.len() == 7 => fern_rad_ablegen(
                            &app,
                            FernRad { pid, wid, x, y, dx: z(5) as i32, dy: z(6) as i32 },
                        ),
                        // Float deltas (finger points * 0.25) + gesture phase.
                        "radf" if teile.len() >= 8 => {
                            let f = |i: usize| teile[i].parse::<f64>().unwrap_or(0.0);
                            let (fdx, fdy, phase) = (f(5), f(6), teile[7]);
                            let t_ein = teile.get(8).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
                            if crate::noki_browser::ist_noki_browser(pid) {
                                // Noki Browser: straight onto the browser's own
                                // queue - NOT the shared worker, whose 140 ms
                                // gesture collection turned a smooth gesture
                                // into ~7 visible jumps per second.
                                crate::fern_tippen::auswahl_verwerfen("scroll");
                                crate::noki_browser::rollen_fein(wid, x, y, fdx, fdy, phase, t_ein);
                            } else if katalyst_rad_app(pid) {
                                // GoodNotes: direct, per packet, never the
                                // 140 ms collector (continuous visual pan).
                                crate::fern_tippen::auswahl_verwerfen("scroll");
                                fernbedienung::katalyst_rad(pid, wid, x, y, fdx, fdy, phase);
                                static SPUR: Mutex<Option<std::time::Instant>> = Mutex::new(None);
                                if SPUR.lock().is_ok_and(|mut t| { let alt = t.is_none_or(|t| t.elapsed() > std::time::Duration::from_millis(500)); if alt { *t = Some(std::time::Instant::now()); } alt }) {
                                    crate::virtual_workspace::trace(&format!("[SCROLL] wid={wid} route=catalyst_window_wheel phase={phase} dy={fdy:.2}"));
                                }
                            } else if let Some((dx, dy)) = rad_quantisieren(wid, fdx, fdy, phase) {
                                youtube_pwa_hinweis(pid);
                                fern_rad_ablegen(&app, FernRad { pid, wid, x, y, dx, dy });
                            }
                        }
                        _ => {}
                    },
                    None => {
                        eprintln!(
                            "[VORSCHAU] fern abgelehnt: Fenster {wid} gehoert nicht zu Nokis Schreibtisch"
                        );
                        let _ = app.emit(
                            "noki://vorschau_fern",
                            serde_json::json!({ "was": "abgelehnt", "fenster": wid }),
                        );
                    }
                }
            }
            // App-Leiste: Besitz und Wirkung prueft Noki, nicht der Helfer.
            Some("APP") if teile.len() >= 2 => {
                let was = teile[1].to_string();
                let arg = zeile.splitn(3, ' ').nth(2).unwrap_or("").to_string();
                let a = app.clone();
                std::thread::spawn(move || crate::leiste_befehl(&a, &was, &arg));
            }
            // Helper: an app launched/activated (NSWorkspace). Wake the
            // user-desktop guard so a normal launch restored onto the
            // virtual display is handed back within ~100 ms.
            Some("APPAKTIV") => crate::schreibtisch_wache_wecken(),
            // Dock gesture running / ended: Shortcut 9 pauses live captures.
            Some("WISCH") if teile.len() >= 2 => crate::window_overview::geste(teile[1] == "1"),
            // Black-frame gate in the helper: the app stopped painting (e.g.
            // ChatGPT while inactive). The last valid frame stays visible;
            // say the real reason once instead of showing a black window.
            Some("SCHWARZ") if teile.len() >= 3 => {
                let wid: i64 = teile[1].parse().unwrap_or(0);
                let an = teile[2] == "1";
                crate::virtual_workspace::trace(&format!("[CAPTURE] black_source wid={wid} black={an} action={}",
                    if an { "keep_last_valid_frame" } else { "live_again" }));
                if an {
                    let pid = crate::fenster_pid(wid);
                    if pid > 0 { if let Ok(mut g) = AKTIV_ZEICHNER.lock() { if !g.contains(&pid) { g.push(pid); } } }
                    let name = crate::lesezeichen::app_fuer_pid(pid).map(|a| a.0).unwrap_or_default();
                    if !name.is_empty() { hinweis(&format!("{name} zeichnet nur, solange es aktiv ist – Noki zeigt das letzte echte Bild")); }
                }
            }
            Some("LEER") => {
                // Nie auf dem Lesefaden (er holt die Bilder ab).
                std::thread::spawn(|| {
                    if menue_offen() {
                        menues_schliessen();
                        crate::virtual_workspace::trace("[MENU] cancel reason=click_on_empty_workspace space_trip=false");
                    }
                });
            }
            Some("KLICK") if teile.len() >= 2 => {
                eprintln!("[VORSCHAU] KLICK {}", teile[1]);
                if teile[1] == "besuchen" {
                    if teile.get(2) == Some(&"footer_primary") {
                        crate::fuss_freigabe_erteilen("EXPLICIT_FOOTER_CLICK");
                    } else {
                        crate::virtual_workspace::trace("[NAVIGATION] footer message without source - no authorization");
                    }
                }
                let _ = app.emit("noki://vorschau_klick", serde_json::json!({ "was": teile[1] }));
            }
            // The helper saw scroll input but no new frame (app not painting
            // on the hidden Space). GoodNotes: 1-pt resize wake, throttled.
            Some("STARR") if teile.len() == 2 => {
                let wid = teile[1].parse::<i64>().unwrap_or(0);
                // Catalyst (GoodNotes) only: an occluded Chromium/Electron
                // window does not paint after a resize either (measured VS
                // Code 2026-10-01) - the resize would only risk size drift.
                if let Some(pid) = ziel_erlaubt(wid).filter(|p| katalyst_rad_app(*p)) {
                    static ZULETZT: Mutex<Vec<(i64, std::time::Instant)>> = Mutex::new(Vec::new());
                    let frei = ZULETZT.lock().is_ok_and(|mut z| {
                        z.retain(|e| e.1.elapsed() < std::time::Duration::from_millis(1500));
                        let f = !z.iter().any(|e| e.0 == wid);
                        if f { z.push((wid, std::time::Instant::now())); }
                        f
                    });
                    if frei {
                        std::thread::spawn(move || {
                            let t0 = std::time::Instant::now();
                            let ok = fernbedienung::render_wecken(pid, wid);
                            crate::virtual_workspace::trace(&format!("[RENDER] wake wid={wid} pid={pid} route=resize_1pt ok={ok} ms={}", t0.elapsed().as_millis()));
                        });
                    }
                }
            }
            Some("KLICKART") => {
                crate::virtual_workspace::trace(&format!("[PREVIEW_HIT] {zeile}"));
            }
            Some("OBEN") if teile.len() == 2 => {
                COMPOSITOR_FRONT_WID.store(
                    teile[1].parse::<i64>().unwrap_or(0),
                    std::sync::atomic::Ordering::Release,
                );
                crate::virtual_workspace::trace(&format!("[VORSCHAU] {zeile}"));
            }
            Some("GIANT") | Some("KOMPAKT_NEU") | Some("SWAP") | Some("ZUSTAND") | Some("FENSTERVORN")
                | Some("ROLLMESS") | Some("ROLLMESS_ENDE") | Some("TESTRAD") | Some("ROLLQUELLE") | Some("ROLLHAENGER") => {
                crate::virtual_workspace::trace(&format!("[VORSCHAU] {zeile}"));
            }
            Some("MINIATUR_POSITION_DRIFT") | Some("MINIATUR_MESSUNG") => {
                crate::virtual_workspace::trace(&format!("[MINIATUR_POSITION_DRIFT] {zeile}"));
            }
            Some("GEOMETRY_LOCK") => {
                crate::virtual_workspace::trace(&format!("[VORSCHAU] {zeile}"));
            }
            Some("ANIM") => {
                crate::virtual_workspace::trace(&format!("[VORSCHAU] anim {zeile}"));
            }
            Some("TRANSITION") if teile.len() == 2 => {
                // The helper moved its own place state; Rust's last-sent
                // `ort` no longer describes it - never dedupe the next one.
                if let Ok(mut l) = ORT_LETZT.lock() { l.clear(); }
                if let Ok(mut z) = HELFER_UEBERGANG.lock() { *z = teile[1].to_string(); }
            }
            Some("HOVER") if teile.len() == 2 => {
                // The helper repeats HOVER in bursts (measured: hundreds per
                // ms, 360k log lines). Each one woke the character page twice;
                // under that backlog a Space transfer could not finish its
                // reveal and Noki stayed invisible. Act on a CHANGE, plus one
                // keep-alive every 2 s for a page that missed the first.
                static HOVER_LETZT: std::sync::Mutex<Option<(String, std::time::Instant)>> = std::sync::Mutex::new(None);
                let neu = HOVER_LETZT.lock().map(|mut g| {
                    let gleich = g.as_ref().is_some_and(|(m, t)| m == teile[1] && t.elapsed() < std::time::Duration::from_secs(2));
                    if !gleich { *g = Some((teile[1].to_string(), std::time::Instant::now())); }
                    !gleich
                }).unwrap_or(true);
                if !neu { continue; }
                crate::virtual_workspace::trace(&format!("[VORSCHAU] hover {}", teile[1]));
                // Die Oberflaeche kann beim Start den ersten Hinweis verpasst
                // haben (WebView noch nicht geladen) - dann laege ihre alte
                // DOM-Miniatur ueber der echten. Deshalb bei jedem Lebenszeichen.
                let _ = app.emit("noki://vorschau_nativ", serde_json::json!({ "an": true }));
                if abnahme() {
                    eprintln!("[ACCEPT] PREVIEW {} pid={}", teile[1], std::process::id());
                }
                let _ = app.emit("noki://vorschau_hover", serde_json::json!({ "modus": teile[1] }));
                // LARGE Miniatur and the window overview coexist: while the
                // Miniatur is enlarged it lies above the overview panel.
                crate::window_overview::miniatur_gross(&app, teile[1] == "gross");
            }
            Some("LEISTENMENUE") if teile.len() == 2 => {
                LEISTENMENUE.store(teile[1] == "an", std::sync::atomic::Ordering::SeqCst);
            }
            Some("INTERACTION") if teile.len() == 2 && teile[1] == "an" => {
                crate::fern_tippen::klick_in_ansicht();
                // The process-wide event tap owns and isolates the keyboard;
                // REAL_SPACE does not need to activate Noki. In particular,
                // activating an accessory app while the user is in a native
                // fullscreen Space can ask macOS to follow that app to one of
                // its other Spaces. Legacy keeps its established focus path.
                if crate::virtual_workspace::backend()
                    == crate::virtual_workspace::Backend::LegacyVirtualDisplay
                {
                    crate::noki_interaction_aktivieren(&app);
                } else {
                    crate::virtual_workspace::trace(
                        "[INTERACTION] owner=event_tap app_activation=false space_navigation=false",
                    );
                }
            }
            Some("RAHMEN") if teile.len() == 5 => {
                crate::virtual_workspace::trace(&format!("[VORSCHAU] {zeile}"));
                let z = |i: usize| teile[i].parse::<f64>().unwrap_or(0.0);
                if let Ok(mut r) = IST_RAHMEN.lock() { *r = [z(1), z(2), z(3), z(4)]; }
            }
            Some("VOLL") if teile.len() == 2 => {
                let an = teile[1] == "an";
                VOLL.store(an, std::sync::atomic::Ordering::SeqCst);
                crate::virtual_workspace::trace(&format!("[ANSICHT] voll={an} source=compositor space_trip=false"));
                if !an { crate::fern_tippen::beenden("vollansicht_zu"); }
                let _ = app.emit("noki://vorschau_voll", serde_json::json!({ "an": an }));
            }
            Some("ABGEDECKT") => {
                ABGEDECKT.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            Some("FPS") => {
                if std::env::var("NOKI_VORSCHAU_FPS").is_ok() {
                    eprintln!("[VORSCHAU] {zeile}");
                }
            }
            Some("PAUSED") => {
                if let Ok(mut g) = LAUF.lock() {
                    if let Some(l) = g.as_mut() {
                        l.pausiert = true;
                        l.fenster.clear();
                    }
                }
            }
            Some("READY") => {
                let _ = app.emit("noki://vorschau_nativ", serde_json::json!({ "an": true }));
                // Erst die atomar dargestellte Komposition darf auch das
                // Fernbedienungsziel werden.
                if let Ok(mut g) = LAUF.lock() {
                    if let Some(l) = g.as_mut() {
                        l.pausiert = false;
                        if let Some(neu) = l.wartend.take() { l.fenster = neu; }
                        if let Some(neu_uuid) = l.wartend_uuid.take() {
                            let alt = std::mem::replace(&mut l.veroeffentlicht_uuid, neu_uuid.clone());
                            if abnahme() {
                                eprintln!(
                                    "[ACCEPT] RETARGET publish oldPublishedUUID={} requestedTargetUUID={} currentUserUUID={} newPublishedUUID={}",
                                    if alt.is_empty() { "-" } else { &alt }, neu_uuid,
                                    l.wartend_nutzer_uuid, l.veroeffentlicht_uuid
                                );
                            }
                        }
                    }
                }
                RETARGET_AKTIV.store(false, std::sync::atomic::Ordering::SeqCst);
                let _ = app.emit(
                    "noki://vorschau_stand",
                    serde_json::json!({ "text": zeile }),
                );
            }
            Some("ERR") => {
                // Helfer behaelt bei Retarget-Fehlern die alte vollstaendige
                // Komposition; also bleibt auch deren Identitaet gueltig.
                if let Ok(mut g) = LAUF.lock() {
                    if let Some(l) = g.as_mut() {
                        l.wartend = None;
                        l.wartend_uuid = None;
                        if abnahme() {
                            eprintln!(
                                "[ACCEPT] RETARGET hold publishedUUID={} error={}",
                                l.veroeffentlicht_uuid, zeile
                            );
                        }
                    }
                }
                RETARGET_AKTIV.store(false, std::sync::atomic::Ordering::SeqCst);
                let _ = app.emit(
                    "noki://vorschau_stand",
                    serde_json::json!({ "text": zeile }),
                );
            }
            _ => {}
        }
    }
    let _ = app.app_handle();
    // Helfer beendet (oder abgestuerzt): der naechste Aufruf startet ihn neu.
    if let Ok(mut g) = LAUF.lock() {
        // A replaced helper's old reader can reach EOF after the new helper
        // is already installed.  Never let that stale reader erase the new
        // live process (the former race produced intermittent permanent
        // Miniatur disappearance after recovery).
        if g.as_ref().is_some_and(|l| l.kind.id() == helper_pid) {
            *g = None;
        } else {
            return;
        }
    }
    VOLL.store(false, std::sync::atomic::Ordering::SeqCst);
    if let Ok(mut r) = IST_RAHMEN.lock() { *r = [0.0; 4]; }
    crate::fern_tippen::beenden("helfer_beendet");
    RETARGET_AKTIV.store(false, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::b64;

    #[test]
    fn base64_entspricht_der_referenz() {
        assert_eq!(b64(b""), "");
        assert_eq!(b64(b"f"), "Zg==");
        assert_eq!(b64(b"fo"), "Zm8=");
        assert_eq!(b64(b"foo"), "Zm9v");
        assert_eq!(b64(b"foob"), "Zm9vYg==");
        assert_eq!(b64(b"fooba"), "Zm9vYmE=");
        assert_eq!(b64(b"foobar"), "Zm9vYmFy");
        // Bytes ueber 0x7F muessen unveraendert durchlaufen (JPEG ist binaer).
        assert_eq!(b64(&[0xFF, 0xD8, 0xFF]), "/9j/");
    }

    #[test]
    fn compositor_veroeffentlicht_keine_unvollstaendige_fenstermenge() {
        let quelle = include_str!("../../schirm/main.swift");
        assert!(!quelle.contains("SWAP_PARTIAL"));
        assert!(!quelle.contains("TEILWEISE"));
        assert!(quelle.contains("ERR Retarget nicht vollstaendig"));
        assert!(quelle.contains("last valid composition retained"));
    }
}

/// ---------------------------------------------------------------------
///  FERNBEDIENUNG: ein Klick IN der Miniatur wirkt auf dem echten Fenster.
///
///  Der Nutzer bleibt dabei auf seinem Schreibtisch. Klicks gehen direkt an
///  den Zielprozess (`CGEventPostToPid`). Auch Rollen wird erst nach der
///  verdeckten Ziel-Space-Aktivierung direkt an diesen Prozess geliefert.
///  Kein Fernpfad schreibt mehr an den globalen HID-Zeigerstrom.
///
///  Zuerst wird der SEMANTISCHE Weg versucht: das Bedienungshilfen-Element
///  an dieser Stelle und seine eigene Betaetigung (AXPress). Das ist der
///  einzige Weg, der auch dann richtig trifft, wenn das Programm seine
///  Treffer aus der Zeigerlage ableitet. Erst wenn es dort kein
///  betaetigbares Element gibt, geht der Zeigerweg an den Prozess.
#[cfg(target_os = "macos")]
pub mod fernbedienung {
    #[repr(C)]
    pub struct CfOpak { _p: [u8; 0] }
    use std::ffi::c_void;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXUIElementCreateApplication(pid: i32) -> *mut c_void;
        fn AXUIElementGetPid(el: *mut c_void, pid: *mut i32) -> i32;
        fn AXUIElementCopyElementAtPosition(
            app: *mut c_void, x: f32, y: f32, el: *mut *mut c_void,
        ) -> i32;
        fn AXUIElementPerformAction(el: *mut c_void, action: *const c_void) -> i32;
        fn AXUIElementCopyActionNames(el: *mut c_void, names: *mut *const c_void) -> i32;
        fn AXUIElementCopyAttributeNames(el: *mut c_void, names: *mut *const c_void) -> i32;
        fn AXUIElementCopyAttributeValue(
            el: *mut c_void, attribute: *const c_void, value: *mut *const c_void,
        ) -> i32;
        fn AXUIElementSetAttributeValue(
            el: *mut c_void, attribute: *const c_void, value: *const c_void,
        ) -> i32;
        fn AXIsProcessTrusted() -> bool;
        fn AXUIElementCopyParameterizedAttributeValue(
            el: *mut c_void, attribute: *const c_void, param: *const c_void, value: *mut *const c_void,
        ) -> i32;
        fn AXUIElementIsAttributeSettable(
            el: *mut c_void, attribute: *const c_void, settable: *mut u8,
        ) -> i32;
    }
    #[link(name = "System", kind = "dylib")]
    extern "C" {
        fn dlsym(handle: *mut c_void, symbol: *const i8) -> *mut c_void;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFBooleanTrue: *const c_void;
        static kCFBooleanFalse: *const c_void;
        static kCFTypeArrayCallBacks: CfOpak;
        fn CFArrayCreate(a: *const c_void, v: *const *const c_void, n: isize, cb: *const CfOpak) -> *const c_void;
        fn CFStringCreateWithCString(a: *const c_void, s: *const i8, enc: u32) -> *const c_void;
        fn CFArrayGetCount(a: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(a: *const c_void, i: isize) -> *const c_void;
        fn CFStringCompare(a: *const c_void, b: *const c_void, o: usize) -> i32;
        fn CFStringGetCString(s: *const c_void, buf: *mut i8, n: isize, enc: u32) -> bool;
        fn CFNumberGetValue(n: *const c_void, t: i32, value: *mut c_void) -> bool;
        fn CFNumberCreate(a: *const c_void, t: i32, value: *const c_void) -> *const c_void;
        fn CFEqual(a: *const c_void, b: *const c_void) -> bool;
        fn CFStringGetLength(s: *const c_void) -> isize;
        fn CFStringGetCharacters(s: *const c_void, r: CfRangeRoh, buf: *mut u16);
        fn CFRetain(o: *const c_void) -> *const c_void;
        fn CFRelease(o: *const c_void);
    }
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXValueGetValue(value: *const c_void, typ: i32, out: *mut c_void) -> bool;
        fn AXValueCreate(typ: i32, value: *const c_void) -> *const c_void;
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventCreate(src: *const c_void) -> *mut c_void;
        fn CGEventCreateMouseEvent(
            src: *const c_void, typ: u32, pos: Punkt, taste: u32,
        ) -> *mut c_void;
        fn CGEventCreateKeyboardEvent(
            src: *const c_void, keycode: u16, key_down: bool,
        ) -> *mut c_void;
        fn CGEventSetLocation(ev: *mut c_void, pos: Punkt);
        fn CGEventGetLocation(ev: *mut c_void) -> Punkt;
        fn CGEventSetIntegerValueField(ev: *mut c_void, feld: u32, wert: i64);
        fn CGEventSetDoubleValueField(ev: *mut c_void, feld: u32, wert: f64);
        fn CGEventPostToPid(pid: i32, ev: *mut c_void);
        fn CGEventCreateCopy(ev: *mut c_void) -> *mut c_void;
        fn CGEventKeyboardSetUnicodeString(ev: *mut c_void, len: usize, text: *const u16);
        fn CGEventSetFlags(ev: *mut c_void, flags: u64);
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct Punkt { pub x: f64, pub y: f64 }
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Groesse { w: f64, h: f64 }
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Rechteck { o: Punkt, g: Groesse }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CfRangeRoh { loc: isize, len: isize }

    /// AXEnhancedUserInterface for exactly ONE action: reads the app's
    /// original value, switches it on, and restores that value on drop -
    /// success, failure or early return alike.
    struct EnhancedGuard { app: *mut c_void, vorher: bool }
    impl EnhancedGuard {
        unsafe fn an(app: *mut c_void) -> Self {
            let name = cfstr("AXEnhancedUserInterface");
            let vorher = ax_element_attr(app, "AXEnhancedUserInterface").map(|v| {
                let ja = CFEqual(v as *const c_void, kCFBooleanTrue);
                CFRelease(v as *const c_void);
                ja
            }).unwrap_or(false);
            if !vorher { let _ = AXUIElementSetAttributeValue(app, name, kCFBooleanTrue); }
            CFRelease(name);
            CFRetain(app as *const c_void);
            EnhancedGuard { app, vorher }
        }
    }
    impl Drop for EnhancedGuard {
        fn drop(&mut self) {
            unsafe {
                if !self.vorher {
                    let name = cfstr("AXEnhancedUserInterface");
                    let _ = AXUIElementSetAttributeValue(self.app, name, kCFBooleanFalse);
                    CFRelease(name);
                }
                CFRelease(self.app as *const c_void);
            }
        }
    }

    /// Electron/Chromium liefern ihren Baum erst mit AXManualAccessibility.
    /// VS Code (Standard "accessibilitySupport: auto") schaltet dadurch fuer
    /// ALLE Fenster des Prozesses in den Bildschirmleser-Modus (gemessen) -
    /// also merken und nach 5 s ohne Noki-Bedienung wieder abschalten, wenn
    /// der Nutzer in diesem Prozess eigene Fenster hat.
    static AX_MANUELL: std::sync::Mutex<Vec<(i32, std::time::Instant)>> = std::sync::Mutex::new(Vec::new());

    /// Noki quits: every Electron/Chromium tree Noki switched on goes off
    /// again (VS Code must never stay in "Screen Reader Optimized").
    pub fn ax_manuell_alle_aus() {
        let pids: Vec<i32> = AX_MANUELL.lock().map(|mut g| g.drain(..).map(|e| e.0).collect()).unwrap_or_default();
        for pid in pids {
            unsafe {
                let app = AXUIElementCreateApplication(pid);
                if app.is_null() { continue; }
                for n in ["AXManualAccessibility", "AXEnhancedUserInterface"] {
                    let name = cfstr(n);
                    let _ = AXUIElementSetAttributeValue(app, name, kCFBooleanFalse);
                    CFRelease(name);
                }
                CFRelease(app as *const c_void);
            }
        }
    }
    static AX_MANUELL_FADEN: std::sync::OnceLock<()> = std::sync::OnceLock::new();

    unsafe fn ax_manuell_an(app: *mut c_void, pid: i32) {
        if zeit_um() { return; }
        let manual = cfstr("AXManualAccessibility");
        let _ = AXUIElementSetAttributeValue(app, manual, kCFBooleanTrue);
        CFRelease(manual);
        let neu = AX_MANUELL.lock().map(|mut g| {
            if let Some(e) = g.iter_mut().find(|e| e.0 == pid) { e.1 = std::time::Instant::now(); false }
            else { g.push((pid, std::time::Instant::now())); true }
        }).unwrap_or(false);
        // War der Baum abgeschaltet, baut Chromium ihn erst auf Anfrage neu.
        // Gemessen: 120 ms reichten Chrome nicht (Scrollen/Rechtsklick/Tippen
        // scheiterten). Warten, bis die Knotenzahl stabil ist (max. 1,5 s).
        if neu && crate::prozess_hat_noki_fenster(pid) {
            unsafe fn zaehlen(el: *mut c_void, tiefe: usize, n: &mut usize) {
                if zeit_um() || tiefe > 10 || *n > 600 { return; }
                *n += 1;
                let Some(k) = ax_element_attr(el, "AXChildren") else { return };
                for i in 0..CFArrayGetCount(k) { zaehlen(CFArrayGetValueAtIndex(k, i) as *mut c_void, tiefe + 1, n); }
                CFRelease(k as *const c_void);
            }
            let t0 = std::time::Instant::now();
            let mut vorher = 0usize;
            while !zeit_um() && t0.elapsed() < std::time::Duration::from_millis(1500) {
                std::thread::sleep(std::time::Duration::from_millis(100));
                if zeit_um() { break; }
                let mut n = 0usize;
                if let Some(w) = ax_element_attr(app, "AXWindows") {
                    for i in 0..CFArrayGetCount(w) { zaehlen(CFArrayGetValueAtIndex(w, i) as *mut c_void, 0, &mut n); }
                    CFRelease(w as *const c_void);
                }
                if n > 20 && n == vorher { break; }
                vorher = n;
            }
            crate::virtual_workspace::trace(&format!("[INTERAKTION] ax_tree_ready pid={pid} ms={} nodes={vorher}", t0.elapsed().as_millis()));
        }
        AX_MANUELL_FADEN.get_or_init(|| {
            let _ = std::thread::Builder::new().name("noki-ax-manuell".into()).spawn(|| loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                // 60 s ohne Noki-Bedienung (vorher 5 s: danach musste Chromium
                // den Baum bei jeder Pause neu aufbauen).
                // Processes where the user has OWN windows (VS Code):
                // off 1.5 s after the last Noki action - but never while a
                // typing session into that process runs. Others: 60 s.
                //
                // NEVER evaluate `nutzer_hat_fenster` while holding AX_MANUELL:
                // it reaches `ax_fenster_element -> ax_manuell_an`, which locks
                // AX_MANUELL again - a self-deadlock of this thread that then
                // blocked every AX path (menu bar, Shortcut 9 tabs, arrival
                // order) for good (measured: 4 threads parked on this lock).
                let stand: Vec<(i32, std::time::Instant)> =
                    AX_MANUELL.lock().map(|g| g.clone()).unwrap_or_default();
                let kandidaten: Vec<i32> = stand.iter().filter(|e| {
                    if e.1.elapsed() <= std::time::Duration::from_millis(1500) { return false; }
                    if e.1.elapsed() > std::time::Duration::from_millis(60_000) { return true; }
                    crate::nutzer_hat_fenster(e.0) && crate::fern_tippen::ziel_pid() != Some(e.0)
                }).map(|e| e.0).collect();
                // Re-check the timestamp under the lock: a Noki action in the
                // meantime (ax_manuell_an refreshed it) keeps the entry.
                let faellig: Vec<i32> = AX_MANUELL.lock().map(|mut g| {
                    let f: Vec<i32> = g.iter()
                        .filter(|e| kandidaten.contains(&e.0)
                            && stand.iter().any(|s| s.0 == e.0 && s.1 == e.1))
                        .map(|e| e.0).collect();
                    g.retain(|e| !f.contains(&e.0));
                    f
                }).unwrap_or_default();
                for pid in faellig {
                    // Hat der Nutzer in diesem Prozess KEINE eigenen Fenster, bleibt
                    // der Baum an, solange Noki dort ein Fenster zeigt (die naechste
                    // Bedienung braucht ihn sofort). Hat er eigene Fenster, wird er
                    // 5 s nach der letzten Noki-Bedienung abgeschaltet - auch wenn
                    // Noki dort ein Fenster zeigt: gemessen schaltete VS Code sonst
                    // DAUERHAFT in "Screen Reader Optimized" (Nutzereinstellungen
                    // bleiben unberuehrt; die naechste Bedienung schaltet ihn neu an).
                    if !crate::nutzer_hat_fenster(pid) {
                        if crate::prozess_hat_noki_fenster(pid) {
                            if let Ok(mut g) = AX_MANUELL.lock() { g.push((pid, std::time::Instant::now())); }
                        }
                        continue;
                    }
                    unsafe {
                        let app = AXUIElementCreateApplication(pid);
                        if app.is_null() { continue; }
                        let manual = cfstr("AXManualAccessibility");
                        let _ = AXUIElementSetAttributeValue(app, manual, kCFBooleanFalse);
                        CFRelease(manual);
                        CFRelease(app as *const c_void);
                    }
                    crate::virtual_workspace::trace(&format!("[INTERAKTION] ax_manual_off pid={pid} reason=user_windows_in_process"));
                }
            });
        });
    }

    /// Hauptfenster-Leihe. Bedient Noki ein Fenster eines Programms, in dem
    /// der Nutzer ein EIGENES Fenster als Haupt-/Tastaturfenster hat (z. B.
    /// VS Code, in dem er selbst arbeitet), wird dessen Hauptfenster nur
    /// geliehen: gemessen machte ein Klick das Noki-Fenster zu VS Codes
    /// Hauptfenster - die naechsten Tasten des Nutzers landeten dort.
    /// Nach der Bedienung (1,2 s Ruhe, kein Tippmodus) bekommt das
    /// Nutzerfenster sein Hauptfenster zurueck.
    static LEIHE: std::sync::Mutex<Vec<(i32, Feld, std::time::Instant)>> = std::sync::Mutex::new(Vec::new());
    static LEIHE_FADEN: std::sync::OnceLock<()> = std::sync::OnceLock::new();

    unsafe fn auf_noki(el: *mut c_void) -> Option<bool> {
        let r = ax_rahmen(el)?;
        let d = crate::virtual_workspace::info()?;
        Some(r.o.x + 1.0 >= d.x as f64 && r.o.y + 1.0 >= d.y as f64)
    }

    unsafe fn fokusfenster(pid: i32) -> Option<*mut c_void> {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() { return None; }
        let w = ax_element_attr(app, "AXFocusedWindow");
        CFRelease(app as *const c_void);
        w
    }

    pub fn hauptfenster_leihen(pid: i32) {
        if pid <= 0 || pid == std::process::id() as i32 { return; }
        unsafe {
            if !AXIsProcessTrusted() { return; }
            let Ok(mut g) = LEIHE.lock() else { return };
            if let Some(e) = g.iter_mut().find(|e| e.0 == pid) {
                e.2 = std::time::Instant::now();
            } else if let Some(w) = fokusfenster(pid) {
                // Nur ein Fenster beim Nutzer wird gemerkt (nie ein Noki-Fenster).
                if auf_noki(w) == Some(false) && ax_rahmen(w).is_some_and(|r| r.g.w >= 200.0 && r.g.h >= 150.0) {
                    g.push((pid, Feld(w as usize), std::time::Instant::now()));
                } else {
                    CFRelease(w as *const c_void);
                }
            }
        }
        LEIHE_FADEN.get_or_init(|| {
            let _ = std::thread::Builder::new().name("noki-hauptfenster-leihe".into()).spawn(|| loop {
                std::thread::sleep(std::time::Duration::from_millis(250));
                let tippt = crate::fern_tippen::ziel_pid();
                // Nur fuer das VORDERE Programm zurueckgeben (nur dort koennen die
                // Tasten des Nutzers im falschen Fenster landen). Gemessen: das
                // Setzen von AXMain bei einem Hintergrundprogramm verschluckte
                // das Loslassen laufender Klicks in der Miniatur (3 von 12). Wird
                // das Programm spaeter aktiv, greift dieselbe Pruefung dann.
                let vorn = crate::blende::vorn_pid();
                let faellig: Vec<(i32, Feld)> = LEIHE.lock().map(|mut g| {
                    let mut f = vec![];
                    let mut i = 0;
                    while i < g.len() {
                        if Some(g[i].0) != tippt && vorn == g[i].0 && g[i].2.elapsed() > std::time::Duration::from_millis(1200) {
                            let e = g.remove(i);
                            f.push((e.0, e.1));
                        } else { i += 1; }
                    }
                    f
                }).unwrap_or_default();
                for (pid, feld) in faellig {
                    unsafe {
                        // Nur zurueckgeben, wenn gerade ein Noki-Fenster Hauptfenster
                        // ist - hat der Nutzer selbst gewechselt, entscheidet er.
                        let jetzt = fokusfenster(pid);
                        let noki_vorn = jetzt.is_some_and(|w| auf_noki(w) == Some(true));
                        if let Some(w) = jetzt { CFRelease(w as *const c_void); }
                        if !noki_vorn { continue; }
                        let main_attr = cfstr("AXMain");
                        let ok = AXUIElementSetAttributeValue(feld.0 as *mut c_void, main_attr, kCFBooleanTrue) == 0;
                        CFRelease(main_attr);
                        crate::virtual_workspace::trace(&format!("[FOKUS] user_main_window_restored pid={pid} ok={ok}"));
                    }
                }
            });
        });
    }

    unsafe fn cfstr(s: &str) -> *const c_void {
        let c = std::ffi::CString::new(s).unwrap();
        CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), 0x0800_0100) // UTF8
    }

    unsafe fn cf_string(s: *const c_void) -> String {
        if s.is_null() { return String::new(); }
        let mut b = [0i8; 256];
        if !CFStringGetCString(s, b.as_mut_ptr(), b.len() as isize, 0x0800_0100) {
            return String::new();
        }
        std::ffi::CStr::from_ptr(b.as_ptr()).to_string_lossy().into_owned()
    }

    unsafe fn ax_beschreibung(el: *mut c_void) -> String {
        let role_attr = cfstr("AXRole");
        let mut role: *const c_void = std::ptr::null();
        let _ = AXUIElementCopyAttributeValue(el, role_attr, &mut role);
        CFRelease(role_attr);
        let rolle = cf_string(role);
        if !role.is_null() { CFRelease(role); }
        let mut actions: *const c_void = std::ptr::null();
        let mut namen = vec![];
        if AXUIElementCopyActionNames(el, &mut actions) == 0 && !actions.is_null() {
            for i in 0..CFArrayGetCount(actions) {
                namen.push(cf_string(CFArrayGetValueAtIndex(actions, i)));
            }
            CFRelease(actions);
        }
        let mut attrs: *const c_void = std::ptr::null();
        let mut anamen = vec![];
        if AXUIElementCopyAttributeNames(el, &mut attrs) == 0 && !attrs.is_null() {
            for i in 0..CFArrayGetCount(attrs) {
                let n = cf_string(CFArrayGetValueAtIndex(attrs, i));
                if n.contains("Scroll") || n.contains("Value") || n.contains("Position")
                    || n.contains("DOM") || n == "AXRole" || n == "AXChildren" {
                    anamen.push(n);
                }
            }
            CFRelease(attrs);
        }
        format!("{rolle} actions={} attrs={}", namen.join("|"), anamen.join("|"))
    }

    unsafe fn ax_rolle_erlaubt(el: *mut c_void) -> bool {
        let attr = cfstr("AXRole");
        let mut role: *const c_void = std::ptr::null();
        let ok = AXUIElementCopyAttributeValue(el, attr, &mut role) == 0 && !role.is_null();
        CFRelease(attr);
        if !ok { return false; }
        let erlaubt = ["AXButton", "AXLink", "AXCheckBox", "AXRadioButton", "AXMenuItem",
            "AXMenuButton", "AXPopUpButton", "AXDisclosureTriangle", "AXIncrementor"]
            .iter().any(|r| {
                let s = cfstr(r);
                let gleich = CFStringCompare(role, s, 0) == 0;
                CFRelease(s);
                gleich
            });
        CFRelease(role);
        erlaubt
    }

    unsafe fn ax_rahmen(el: *mut c_void) -> Option<Rechteck> {
        let attr = cfstr("AXFrame");
        let mut value: *const c_void = std::ptr::null();
        let ok = AXUIElementCopyAttributeValue(el, attr, &mut value) == 0 && !value.is_null();
        CFRelease(attr);
        if !ok { return None; }
        let mut r = Rechteck::default();
        let gelesen = AXValueGetValue(value, 3, &mut r as *mut Rechteck as *mut c_void);
        CFRelease(value);
        gelesen.then_some(r)
    }

    unsafe fn ax_press_baum(
        el: *mut c_void, x: f64, y: f64, tiefe: usize, besucht: &mut usize,
    ) -> bool {
        ax_press_baum_mit(el, x, y, tiefe, besucht, false)
    }

    unsafe fn ax_press_baum_mit(
        el: *mut c_void, x: f64, y: f64, tiefe: usize, besucht: &mut usize, generisch: bool,
    ) -> bool {
        if tiefe > 32 || *besucht >= 12_000 || zeit_um() { return false; }
        *besucht += 1;
        if std::env::var("NOKI_AX_DIAG").ok().as_deref() == Some("1") {
            eprintln!("[FERNAXNODE] depth={tiefe} {}", ax_beschreibung(el));
        }
        if let Some(r) = ax_rahmen(el) {
            if x < r.o.x || y < r.o.y || x > r.o.x + r.g.w || y > r.o.y + r.g.h {
                return false;
            }
        }
        let attr = cfstr("AXChildren");
        let mut kinder: *const c_void = std::ptr::null();
        let hat = AXUIElementCopyAttributeValue(el, attr, &mut kinder) == 0 && !kinder.is_null();
        CFRelease(attr);
        if hat {
            for i in (0..CFArrayGetCount(kinder)).rev() {
                let kind = CFArrayGetValueAtIndex(kinder, i) as *mut c_void;
                if !kind.is_null() && ax_press_baum_mit(kind, x, y, tiefe + 1, besucht, generisch) {
                    CFRelease(kinder);
                    return true;
                }
            }
            CFRelease(kinder);
        }
        let darf = ax_rolle_erlaubt(el) || (generisch && !matches!(
            ax_rolle(el).as_str(),
            "AXWebArea" | "AXWindow" | "AXApplication" | "AXScrollArea" | ""
        ));
        darf && ax_aktion(el, "AXPress")
    }

    /// Betaetigt das Element an dieser Stelle, wenn es sich betaetigen
    /// laesst. `true` heisst: das Programm hat eine echte Handlung
    /// ausgefuehrt - nicht nur "ein Ereignis wurde abgeschickt".
    unsafe fn ax_druecken(pid: i32, x: f64, y: f64) -> bool {
        if !AXIsProcessTrusted() {
            return false;
        }
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return false;
        }
        // Chromium builds its web accessibility subtree lazily.  Windows on
        // another Space otherwise expose only the application and browser
        // window, so neither element-at-position nor a tree walk can ever
        // reach the DOM control.  This is Chromium's supported accessibility
        // opt-in and is process-scoped; it neither activates the app nor
        // changes Spaces or the global pointer.
        ax_manuell_an(app, pid);
        // Scoped: original value restored when this action returns (any
        // path). A lasting AXEnhancedUserInterface kept VS Code in "Screen
        // Reader Optimized" (measured 2026-09-29).
        let _enhanced_guard = EnhancedGuard::an(app);
        let mut el: *mut c_void = std::ptr::null_mut();
        let ok = AXUIElementCopyElementAtPosition(app, x as f32, y as f32, &mut el) == 0
            && !el.is_null();
        if !ok {
            CFRelease(app as *const c_void);
            return false;
        }
        // A generic Chrome web-area can advertise AXPress even though the
        // DOM control at the visual point is not pressed.  Accept semantic
        // success only for an actual control role, walking through a static
        // text child to its nearest actionable parent when necessary.
        let mut cur = el;
        let mut getan = false;
        for _ in 0..6 {
            let mut role: *const c_void = std::ptr::null();
            let role_attr = cfstr("AXRole");
            let hat_role = AXUIElementCopyAttributeValue(cur, role_attr, &mut role) == 0
                && !role.is_null();
            CFRelease(role_attr);
            let erlaubt = if hat_role {
                ["AXButton", "AXLink", "AXCheckBox", "AXRadioButton", "AXMenuItem"]
                    .iter().any(|r| {
                        let s = cfstr(r);
                        let gleich = CFStringCompare(role, s, 0) == 0;
                        CFRelease(s);
                        gleich
                    })
            } else { false };
            if !role.is_null() { CFRelease(role); }

            if erlaubt {
                let mut namen: *const c_void = std::ptr::null();
                if AXUIElementCopyActionNames(cur, &mut namen) == 0 && !namen.is_null() {
                    let press = cfstr("AXPress");
                    for i in 0..CFArrayGetCount(namen) {
                        let n = CFArrayGetValueAtIndex(namen, i);
                        if !n.is_null() && CFStringCompare(n, press, 0) == 0 {
                            getan = AXUIElementPerformAction(cur, press) == 0;
                            break;
                        }
                    }
                    CFRelease(press);
                    CFRelease(namen);
                }
            }
            if getan { break; }
            let parent_attr = cfstr("AXParent");
            let mut parent: *const c_void = std::ptr::null();
            let hat_parent = AXUIElementCopyAttributeValue(cur, parent_attr, &mut parent) == 0
                && !parent.is_null();
            CFRelease(parent_attr);
            if cur != el { CFRelease(cur as *const c_void); }
            if !hat_parent { cur = std::ptr::null_mut(); break; }
            cur = parent as *mut c_void;
        }
        if !cur.is_null() && cur != el { CFRelease(cur as *const c_void); }
        CFRelease(el as *const c_void);
        if !getan {
            let mut besucht = 0usize;
            getan = ax_press_baum(app, x, y, 0, &mut besucht);
            eprintln!("[FERNAX] click tree nodes={} success={}", besucht, getan);
        }
        CFRelease(app as *const c_void);
        getan
    }

    /// Zweite Stufe, NUR im verdeckten Vorgang: ein Web-Element ohne
    /// Knopf-Rolle, das nur einen Klick-Hoerer hat (Videobild, Kachel).
    /// Chromium fuehrt AXPress darauf als echten DOM-Klick aus. Gedrueckt
    /// wird das TIEFSTE Element unter dem Punkt, das AXPress anbietet - nie
    /// das Web-Dokument oder Fenster selbst (deren AXPress meldet Erfolg
    /// ohne Wirkung).
    unsafe fn ax_druecken_generisch(pid: i32, x: f64, y: f64) -> bool {
        if !AXIsProcessTrusted() { return false; }
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() { return false; }
        let mut besucht = 0usize;
        let getan = ax_press_baum_mit(app, x, y, 0, &mut besucht, true);
        CFRelease(app as *const c_void);
        eprintln!("[FERNAX] generic press nodes={besucht} success={getan}");
        getan
    }

    unsafe fn ax_rolle(el: *mut c_void) -> String {
        let attr = cfstr("AXRole");
        let mut role: *const c_void = std::ptr::null();
        let _ = AXUIElementCopyAttributeValue(el, attr, &mut role);
        CFRelease(attr);
        let r = cf_string(role);
        if !role.is_null() { CFRelease(role); }
        r
    }

    unsafe fn ax_aktion(el: *mut c_void, name: &str) -> bool {
        let mut namen: *const c_void = std::ptr::null();
        if AXUIElementCopyActionNames(el, &mut namen) != 0 || namen.is_null() { return false; }
        let gesucht = cfstr(name);
        let mut ok = false;
        for i in 0..CFArrayGetCount(namen) {
            let n = CFArrayGetValueAtIndex(namen, i);
            if !n.is_null() && CFStringCompare(n, gesucht, 0) == 0 {
                ok = AXUIElementPerformAction(el, gesucht) == 0;
                break;
            }
        }
        CFRelease(gesucht);
        CFRelease(namen);
        ok
    }

    unsafe fn ax_roll_element(el: *mut c_void, dx: i32, dy: i32) -> bool {
        let page = if dy < 0 { "AXScrollDownByPage" } else { "AXScrollUpByPage" };
        if dy != 0 && ax_aktion(el, page) { return true; }

        let attr = cfstr(if dx.abs() > dy.abs() {
            "AXHorizontalScrollBar"
        } else {
            "AXVerticalScrollBar"
        });
        let mut bar: *const c_void = std::ptr::null();
        let hat_bar = AXUIElementCopyAttributeValue(el, attr, &mut bar) == 0 && !bar.is_null();
        CFRelease(attr);
        if !hat_bar { return false; }

        let wert_attr = cfstr("AXValue");
        let mut wert_obj: *const c_void = std::ptr::null();
        let mut wert = 0.0f64;
        let hat_wert = AXUIElementCopyAttributeValue(
            bar as *mut c_void, wert_attr, &mut wert_obj,
        ) == 0 && !wert_obj.is_null()
            && CFNumberGetValue(wert_obj, 13, &mut wert as *mut f64 as *mut c_void);
        if !wert_obj.is_null() { CFRelease(wert_obj); }
        let delta = if dx.abs() > dy.abs() { dx } else { dy };
        let mut getan = false;
        if hat_wert {
            let neu = (wert + (-(delta as f64) / 80.0)).clamp(0.0, 1.0);
            let num = CFNumberCreate(std::ptr::null(), 13, &neu as *const f64 as *const c_void);
            if !num.is_null() {
                getan = AXUIElementSetAttributeValue(bar as *mut c_void, wert_attr, num) == 0;
                CFRelease(num);
            }
            if getan { eprintln!("[FERNAX] scroll value={wert:.3}->{neu:.3}"); }
        }
        CFRelease(wert_attr);
        if !getan {
            let richtung = if delta < 0 { "AXIncrement" } else { "AXDecrement" };
            let schritte = ((dx.abs().max(dy.abs()) + 3) / 4).clamp(1, 8);
            for _ in 0..schritte { getan |= ax_aktion(bar as *mut c_void, richtung); }
        }
        CFRelease(bar);
        getan
    }

    unsafe fn ax_roll_baum(
        el: *mut c_void, x: f64, y: f64, dx: i32, dy: i32,
        tiefe: usize, besucht: &mut usize,
    ) -> bool {
        if tiefe > 32 || *besucht >= 12_000 || zeit_um() { return false; }
        *besucht += 1;
        if std::env::var_os("NOKI_AX_DIAG").is_some() {
            eprintln!("[FERNAXNODE] depth={tiefe} {}", ax_beschreibung(el));
        }
        if let Some(r) = ax_rahmen(el) {
            if x < r.o.x || y < r.o.y || x > r.o.x + r.g.w || y > r.o.y + r.g.h {
                return false;
            }
        }
        let attr = cfstr("AXChildren");
        let mut kinder: *const c_void = std::ptr::null();
        let hat = AXUIElementCopyAttributeValue(el, attr, &mut kinder) == 0 && !kinder.is_null();
        CFRelease(attr);
        if hat {
            for i in (0..CFArrayGetCount(kinder)).rev() {
                let kind = CFArrayGetValueAtIndex(kinder, i) as *mut c_void;
                if !kind.is_null() && ax_roll_baum(kind, x, y, dx, dy, tiefe + 1, besucht) {
                    CFRelease(kinder);
                    return true;
                }
            }
            CFRelease(kinder);
        }
        ax_roll_element(el, dx, dy)
    }

    /// Scroll an the accessibility scroll area/scrollbar under the visual
    /// point.  This is the first choice because it is semantic, process
    /// scoped, and cannot mutate the global pointer or the user's Desktop.
    unsafe fn ax_rollen(pid: i32, x: f64, y: f64, dx: i32, dy: i32) -> bool {
        if !AXIsProcessTrusted() || (dx == 0 && dy == 0) { return false; }
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() { return false; }
        ax_manuell_an(app, pid);
        // Scoped: original value restored when this action returns (any
        // path). A lasting AXEnhancedUserInterface kept VS Code in "Screen
        // Reader Optimized" (measured 2026-09-29).
        let _enhanced_guard = EnhancedGuard::an(app);
        let mut el: *mut c_void = std::ptr::null_mut();
        let gefunden = AXUIElementCopyElementAtPosition(app, x as f32, y as f32, &mut el) == 0
            && !el.is_null();
        if !gefunden { CFRelease(app as *const c_void); return false; }
        let mut cur = el;
        let mut getan = false;
        for _ in 0..10 {
            if ax_roll_element(cur, dx, dy) { getan = true; break; }
            let parent_attr = cfstr("AXParent");
            let mut parent: *const c_void = std::ptr::null();
            let hat_parent = AXUIElementCopyAttributeValue(cur, parent_attr, &mut parent) == 0
                && !parent.is_null();
            CFRelease(parent_attr);
            if cur != el { CFRelease(cur as *const c_void); }
            if !hat_parent { cur = std::ptr::null_mut(); break; }
            cur = parent as *mut c_void;
        }
        if !cur.is_null() && cur != el { CFRelease(cur as *const c_void); }
        CFRelease(el as *const c_void);
        if !getan {
            let mut besucht = 0usize;
            getan = ax_roll_baum(app, x, y, dx, dy, 0, &mut besucht);
            eprintln!("[FERNAX] scroll tree nodes={} success={}", besucht, getan);
        }
        CFRelease(app as *const c_void);
        getan
    }

    unsafe fn ax_fenster_nummer(window: *mut c_void, number_attr: *const c_void) -> Option<i64> {
        let mut value: *const c_void = std::ptr::null();
        let mut number = 0i64;
        let mut has_number = AXUIElementCopyAttributeValue(window, number_attr, &mut value) == 0
            && !value.is_null()
            && CFNumberGetValue(value, 4, &mut number as *mut i64 as *mut c_void);
        if !value.is_null() { CFRelease(value); }
        // AXWindowNumber is not advertised by Chromium on current macOS.
        if !has_number {
            let symbol = dlsym(
                (-2isize) as *mut c_void,
                b"_AXUIElementGetWindow\0".as_ptr() as *const i8,
            );
            if !symbol.is_null() {
                let get_window: unsafe extern "C" fn(*mut c_void, *mut u32) -> i32 =
                    std::mem::transmute(symbol);
                let mut n = 0u32;
                has_number = get_window(window, &mut n) == 0;
                number = n as i64;
            }
        }
        has_number.then_some(number)
    }

    unsafe fn ax_text_attr(el: *mut c_void, name: &str) -> String {
        let attr = cfstr(name);
        let mut value: *const c_void = std::ptr::null();
        let ok = AXUIElementCopyAttributeValue(el, attr, &mut value) == 0 && !value.is_null();
        CFRelease(attr);
        if !ok { return String::new(); }
        let text = cf_string(value);
        CFRelease(value);
        text
    }

    unsafe fn ax_has_attr_value(el: *mut c_void, name: &str) -> bool {
        let attr = cfstr(name);
        let mut value: *const c_void = std::ptr::null();
        let ok = AXUIElementCopyAttributeValue(el, attr, &mut value) == 0 && !value.is_null();
        CFRelease(attr);
        if !value.is_null() { CFRelease(value); }
        ok
    }

    unsafe fn ax_focus_class(el: *mut c_void) -> &'static str {
        let role = ax_text_attr(el, "AXRole");
        let sub = ax_text_attr(el, "AXSubrole");
        let desc = format!("{} {}", ax_text_attr(el, "AXDescription"),
            ax_text_attr(el, "AXRoleDescription")).to_lowercase();
        if matches!(role.as_str(), "AXTextField" | "AXTextArea" | "AXComboBox" | "AXSearchField")
            || sub == "AXSearchField" { return "TEXT_INPUT"; }
        if ax_has_attr_value(el, "AXEditableAncestor") { return "CONTENTEDITABLE"; }
        if matches!(role.as_str(), "AXSlider" | "AXIncrementor") { return "SLIDER"; }
        if matches!(role.as_str(), "AXPopUpButton" | "AXMenuButton" | "AXList") {
            return "SELECT_CONTROL";
        }
        if role == "AXVideo" || desc.contains("video") || desc.contains("player") {
            return "VIDEO_PLAYER";
        }
        if role == "AXWebArea" { return "PAGE_SCROLLABLE"; }
        "OTHER"
    }

    unsafe fn ax_focused(app: *mut c_void, number_attr: *const c_void)
        -> Option<(*mut c_void, i64, &'static str)>
    {
        let focused_attr = cfstr("AXFocusedUIElement");
        let mut focused: *const c_void = std::ptr::null();
        let ok = AXUIElementCopyAttributeValue(app, focused_attr, &mut focused) == 0
            && !focused.is_null();
        CFRelease(focused_attr);
        if !ok { return None; }
        let window_attr = cfstr("AXWindow");
        let mut window: *const c_void = std::ptr::null();
        let has_window = AXUIElementCopyAttributeValue(
            focused as *mut c_void, window_attr, &mut window,
        ) == 0 && !window.is_null();
        CFRelease(window_attr);
        let wid = if has_window {
            let n = ax_fenster_nummer(window as *mut c_void, number_attr).unwrap_or(0);
            CFRelease(window);
            n
        } else {
            // Chromium does not expose AXWindow on its focused AXWebArea,
            // but macOS can still resolve that exact accessibility object
            // to the owning CGWindowID through _AXUIElementGetWindow.
            ax_fenster_nummer(focused as *mut c_void, number_attr).unwrap_or(0)
        };
        let class = ax_focus_class(focused as *mut c_void);
        Some((focused as *mut c_void, wid, class))
    }

    /// Focus-safe continuous document scrolling for the virtual backend.
    /// Key events are PID-scoped and are emitted only after a second AX read
    /// proves that focus is the target window's neutral AXWebArea.
    unsafe fn ax_pfeil_scroll(pid: i32, wid: i64, dx: i32, dy: i32) -> bool {
        if !AXIsProcessTrusted() || dy == 0 || dx.abs() > dy.abs() { return false; }
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() { return false; }
        ax_manuell_an(app, pid);
        let windows_attr = cfstr("AXWindows");
        let number_attr = cfstr("AXWindowNumber");
        let mut windows: *const c_void = std::ptr::null();
        let mut web = None;
        if AXUIElementCopyAttributeValue(app, windows_attr, &mut windows) == 0 && !windows.is_null() {
            for i in 0..CFArrayGetCount(windows) {
                let window = CFArrayGetValueAtIndex(windows, i) as *mut c_void;
                if ax_fenster_nummer(window, number_attr) == Some(wid) {
                    web = ax_web_bereich(window, 0);
                    break;
                }
            }
            CFRelease(windows);
        }
        CFRelease(windows_attr);
        let Some(web) = web else {
            CFRelease(number_attr); CFRelease(app as *const c_void);
            eprintln!("[FERNAX] focus-safe scroll wid={wid} no_web fail_closed");
            return false;
        };
        let before = ax_focused(app, number_attr);
        let already_safe = before.as_ref().is_some_and(|(_, w, c)| *w == wid && *c == "PAGE_SCROLLABLE");
        if !already_safe {
            let focus_attr = cfstr("AXFocused");
            let _ = AXUIElementSetAttributeValue(web, focus_attr, kCFBooleanTrue);
            CFRelease(focus_attr);
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
        if let Some((f, _, _)) = before { CFRelease(f as *const c_void); }
        let after = ax_focused(app, number_attr);
        // Chromium currently omits AXWindow on a focused AXWebArea, even
        // though the same element was obtained from the exact target
        // CGWindowID above.  Keep the check fail-closed: accept either the
        // advertised matching window id, or CoreFoundation identity with
        // that retained target AXWebArea.  Merely being another WebArea in
        // the same application is deliberately not sufficient.
        let (safe, proof) = after.as_ref().map_or((false, "none"), |(f, w, c)| {
            if *c != "PAGE_SCROLLABLE" {
                (false, "class")
            } else if *w == wid {
                (true, "window-id")
            } else if CFEqual(*f as *const c_void, web as *const c_void) {
                (true, "ax-identity")
            } else {
                (false, "mismatch")
            }
        });
        let class = after.as_ref().map(|(_, _, c)| *c).unwrap_or("NONE");
        if let Some((f, _, _)) = after { CFRelease(f as *const c_void); }
        // Chrome PWA shells: the app-wide focused element is Chrome's (it
        // lives in another window), so identity can't prove anything. Proof
        // here: THIS window's document reports AXFocused (a focused player
        // or field inside it would take that flag) AND the shell's focused
        // window is exactly this one - keys cannot reach a user window.
        let (safe, proof) = if safe { (safe, proof) } else {
            let doc_fokus = {
                let a = cfstr("AXFocused");
                let mut v: *const c_void = std::ptr::null();
                let ok = AXUIElementCopyAttributeValue(web, a, &mut v) == 0 && !v.is_null()
                    && CFEqual(v, kCFBooleanTrue);
                if !v.is_null() { CFRelease(v); }
                CFRelease(a);
                ok
            };
            let fenster_ok = fokus_fenster(pid) == Some(wid);
            if doc_fokus && fenster_ok { (true, "document-focused+focused-window") } else { (false, proof) }
        };
        CFRelease(web as *const c_void);
        CFRelease(number_attr);
        if !safe {
            CFRelease(app as *const c_void);
            eprintln!("[FERNAX] focus-safe scroll wid={wid} class={class} proof={proof} fail_closed");
            return false;
        }
        let keycode: u16 = if dy < 0 { 125 } else { 126 };
        // `dy` is already the number of 40 px steps (see `roll_schritte`):
        // 1:1 with the finger instead of >=1 arrow per tiny packet.
        let steps = (dy.unsigned_abs() as usize).clamp(1, 4);
        let mut sent = 0usize;
        for i in 0..steps {
            let down = CGEventCreateKeyboardEvent(std::ptr::null(), keycode, true);
            let up = CGEventCreateKeyboardEvent(std::ptr::null(), keycode, false);
            if down.is_null() || up.is_null() {
                if !down.is_null() { CFRelease(down); }
                if !up.is_null() { CFRelease(up); }
                break;
            }
            // Nokis eigene Pfeile: markiert (Nokis Kuerzel-Abgriff laesst sie
            // durch - sonst wurde Rollen nach ^ zu "naechstes Fenster") und
            // ohne geerbte Modifikatoren (Ctrl+Pfeil waere ein Space-Wechsel).
            for ev in [down, up] {
                CGEventSetFlags(ev, 0);
                CGEventSetIntegerValueField(ev, 42, crate::fern_tippen::MARKE);
            }
            CGEventPostToPid(pid, down); CGEventPostToPid(pid, up);
            CFRelease(down); CFRelease(up); sent += 1;
            if i + 1 < steps { std::thread::sleep(std::time::Duration::from_millis(25)); }
        }
        CFRelease(app as *const c_void);
        eprintln!(
            "[FERNAX] focus-safe scroll wid={wid} class=PAGE_SCROLLABLE proof={proof} arrows={sent}"
        );
        sent > 0
    }

    /// Bind the bridge transaction to one exact CGWindowID.  Activating an
    /// application is not enough when it owns several overlapping windows:
    /// Chrome otherwise sends wheel input to whichever window was key last.
    unsafe fn ax_fenster_vorn(pid: i32, wid: i64) -> bool {
        if !AXIsProcessTrusted() || pid <= 0 || wid <= 0 { return false; }
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() { return false; }
        let windows_attr = cfstr("AXWindows");
        let number_attr = cfstr("AXWindowNumber");
        let mut windows: *const c_void = std::ptr::null();
        let mut found = false;
        if AXUIElementCopyAttributeValue(app, windows_attr, &mut windows) == 0
            && !windows.is_null()
        {
            for i in 0..CFArrayGetCount(windows) {
                let window = CFArrayGetValueAtIndex(windows, i) as *mut c_void;
                if ax_fenster_nummer(window, number_attr) == Some(wid) {
                    let raise = cfstr("AXRaise");
                    let raised = AXUIElementPerformAction(window, raise) == 0;
                    CFRelease(raise);
                    let main_attr = cfstr("AXMain");
                    let focused_attr = cfstr("AXFocused");
                    let app_focus_attr = cfstr("AXFocusedWindow");
                    let main = AXUIElementSetAttributeValue(window, main_attr, kCFBooleanTrue) == 0;
                    let focused = AXUIElementSetAttributeValue(window, focused_attr, kCFBooleanTrue) == 0;
                    let app_focused = AXUIElementSetAttributeValue(
                        app, app_focus_attr, window as *const c_void,
                    ) == 0;
                    CFRelease(main_attr);
                    CFRelease(focused_attr);
                    CFRelease(app_focus_attr);
                    found = raised || main || focused || app_focused;
                    eprintln!(
                        "[FERNAX] focus wid={wid} raised={raised} main={main} focused={focused} app={app_focused}"
                    );
                    break;
                }
            }
            CFRelease(windows);
        }
        CFRelease(windows_attr);
        CFRelease(number_attr);
        CFRelease(app as *const c_void);
        found
    }

    unsafe fn ax_fenster_verschieben(
        pid: i32, wid: i64, x: f64, y: f64, width: f64, height: f64,
    ) -> bool {
        if !AXIsProcessTrusted() || pid <= 0 || wid <= 0 { return false; }
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() { return false; }
        let windows_attr = cfstr("AXWindows");
        let number_attr = cfstr("AXWindowNumber");
        let mut windows: *const c_void = std::ptr::null();
        let mut moved = false;
        if AXUIElementCopyAttributeValue(app, windows_attr, &mut windows) == 0 && !windows.is_null() {
            for i in 0..CFArrayGetCount(windows) {
                let window = CFArrayGetValueAtIndex(windows, i) as *mut c_void;
                if ax_fenster_nummer(window, number_attr) != Some(wid) { continue; }
                let point = Punkt { x, y };
                let size = Groesse { w: width, h: height };
                let point_value = AXValueCreate(1, &point as *const Punkt as *const c_void);
                let size_value = AXValueCreate(2, &size as *const Groesse as *const c_void);
                let position_attr = cfstr("AXPosition");
                let size_attr = cfstr("AXSize");
                let positioned = !point_value.is_null()
                    && AXUIElementSetAttributeValue(window, position_attr, point_value) == 0;
                let sized = !size_value.is_null()
                    && AXUIElementSetAttributeValue(window, size_attr, size_value) == 0;
                if !point_value.is_null() { CFRelease(point_value); }
                if !size_value.is_null() { CFRelease(size_value); }
                CFRelease(position_attr); CFRelease(size_attr);
                moved = positioned && sized;
                break;
            }
            CFRelease(windows);
        }
        CFRelease(windows_attr); CFRelease(number_attr); CFRelease(app as *const c_void);
        eprintln!("[VIRTUAL_DISPLAY] move wid={wid} pid={pid} target={x:.0},{y:.0},{width:.0}x{height:.0} ok={moved}");
        moved
    }

    /// Das Web-Dokument (AXWebArea) im Fenster, gehalten (CFRetain).
    unsafe fn ax_web_bereich(el: *mut c_void, tiefe: usize) -> Option<*mut c_void> {
        if tiefe > 32 { return None; }
        let role_attr = cfstr("AXRole");
        let mut role: *const c_void = std::ptr::null();
        let has_role = AXUIElementCopyAttributeValue(el, role_attr, &mut role) == 0
            && !role.is_null();
        CFRelease(role_attr);
        let web = cfstr("AXWebArea");
        let is_web = has_role && CFStringCompare(role, web, 0) == 0;
        CFRelease(web);
        if !role.is_null() { CFRelease(role); }
        if is_web { return Some(CFRetain(el as *const c_void) as *mut c_void); }
        let children_attr = cfstr("AXChildren");
        let mut children: *const c_void = std::ptr::null();
        let mut found = None;
        if AXUIElementCopyAttributeValue(el, children_attr, &mut children) == 0
            && !children.is_null()
        {
            for i in 0..CFArrayGetCount(children) {
                let child = CFArrayGetValueAtIndex(children, i) as *mut c_void;
                if !child.is_null() {
                    found = ax_web_bereich(child, tiefe + 1);
                    if found.is_some() { break; }
                }
            }
            CFRelease(children);
        }
        CFRelease(children_attr);
        found
    }

    /// Der naechste Knoten JENSEITS der Sichtkante, in Dokumentreihenfolge.
    ///
    /// Chromium meldet Knoten ausserhalb des Sichtbereichs mit einem auf die
    /// Kante geklemmten Rahmen der Hoehe 0 - Abstaende sind darum nicht
    /// messbar, die Dokumentreihenfolge aber schon: nach unten der ERSTE
    /// Knoten an/unter der Unterkante, nach oben der LETZTE an/ueber der
    /// Oberkante. AXScrollToVisible darauf rollt minimal - ein echter,
    /// begrenzter Schritt statt eines Sprungs ans Dokumentende.
    /// Teilbaeume ganz auf der falschen Seite werden nicht betreten.
    unsafe fn ax_nachbar_jenseits(
        el: *mut c_void, oben: f64, unten: f64, runter: bool,
        tiefe: usize, besucht: &mut usize,
    ) -> Option<*mut c_void> {
        if tiefe > 40 || *besucht >= 6_000 || zeit_um() { return None; }
        *besucht += 1;
        let r = ax_rahmen(el);
        if let Some(r) = r {
            if runter && r.o.y + r.g.h < unten - 1.0 { return None; }
            if !runter && r.o.y > oben + 1.0 { return None; }
        }
        let children_attr = cfstr("AXChildren");
        let mut children: *const c_void = std::ptr::null();
        let mut kinder = 0isize;
        let mut found = None;
        if AXUIElementCopyAttributeValue(el, children_attr, &mut children) == 0
            && !children.is_null()
        {
            kinder = CFArrayGetCount(children);
            for j in 0..kinder {
                let i = if runter { j } else { kinder - 1 - j };
                let child = CFArrayGetValueAtIndex(children, i) as *mut c_void;
                if child.is_null() { continue; }
                found = ax_nachbar_jenseits(child, oben, unten, runter, tiefe + 1, besucht);
                if found.is_some() { break; }
            }
            CFRelease(children);
        }
        CFRelease(children_attr);
        if found.is_some() || tiefe == 0 { return found; }
        let r = r?;
        let jenseits = if runter { r.o.y >= unten - 1.0 } else { r.o.y + r.g.h <= oben + 1.0 };
        // Ein Blatt, das ueber die Kante ragt, zeigt beim Sichtbarmachen
        // genau seinen Rest - der kleinste moegliche Schritt.
        let ragt = kinder == 0 && if runter {
            r.o.y < unten - 1.0 && r.o.y + r.g.h > unten + 1.0
        } else {
            r.o.y + r.g.h > oben + 1.0 && r.o.y < oben - 1.0
        };
        if !(jenseits || ragt) { return None; }
        let mut actions: *const c_void = std::ptr::null();
        let scroll = cfstr("AXScrollToVisible");
        let mut kann = false;
        if AXUIElementCopyActionNames(el, &mut actions) == 0 && !actions.is_null() {
            for i in 0..CFArrayGetCount(actions) {
                let action = CFArrayGetValueAtIndex(actions, i);
                if !action.is_null() && CFStringCompare(action, scroll, 0) == 0 {
                    kann = true;
                    break;
                }
            }
            CFRelease(actions);
        }
        CFRelease(scroll);
        kann.then(|| CFRetain(el as *const c_void) as *mut c_void)
    }

    /// Rollt das Dokument im EXAKTEN Fenster `wid` in `schritte`
    /// Sichtbarmachungs-Schritten. Wirkt nur, solange Chromium den
    /// Web-Baum liefert - also im verdeckten Vorgang, wenn das Fenster auf
    /// dem aktiven Schreibtisch liegt. Kein Zeiger, kein globales Ereignis.
    unsafe fn ax_rollen_fenster(pid: i32, wid: i64, dx: i32, dy: i32) -> bool {
        if !AXIsProcessTrusted() || dy == 0 || dx.abs() > dy.abs() { return false; }
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() { return false; }
        ax_manuell_an(app, pid);
        let windows_attr = cfstr("AXWindows");
        let number_attr = cfstr("AXWindowNumber");
        let mut windows: *const c_void = std::ptr::null();
        let mut web: Option<*mut c_void> = None;
        if AXUIElementCopyAttributeValue(app, windows_attr, &mut windows) == 0
            && !windows.is_null()
        {
            for i in 0..CFArrayGetCount(windows) {
                let window = CFArrayGetValueAtIndex(windows, i) as *mut c_void;
                if ax_fenster_nummer(window, number_attr) == Some(wid) {
                    web = ax_web_bereich(window, 0);
                    break;
                }
            }
            CFRelease(windows);
        }
        CFRelease(windows_attr);
        CFRelease(number_attr);
        CFRelease(app as *const c_void);
        let Some(web) = web else {
            eprintln!("[FERNAX] scroll wid={wid} kein Web-Dokument");
            return false;
        };
        let runter = dy < 0;
        // Ein Schritt je ~12 gesammelten Einheiten (ein Mausrad-Raster oder
        // ein kurzer Trackpad-Zug); eine kraeftige Geste ergibt hoechstens
        // drei Schritte, nie einen Sprung ans Ende.
        let schritte = (dy.unsigned_abs() as usize).div_ceil(12).clamp(1, 3);
        let mut getan = 0usize;
        let mut besucht_summe = 0usize;
        for _ in 0..schritte {
            let Some(rahmen) = ax_rahmen(web) else { break; };
            let mut besucht = 0usize;
            let ziel = ax_nachbar_jenseits(
                web, rahmen.o.y, rahmen.o.y + rahmen.g.h, runter, 0, &mut besucht,
            );
            besucht_summe += besucht;
            let Some(ziel) = ziel else { break; };
            let scroll = cfstr("AXScrollToVisible");
            let ok = AXUIElementPerformAction(ziel, scroll) == 0;
            CFRelease(scroll);
            CFRelease(ziel as *const c_void);
            if !ok { break; }
            getan += 1;
            // Chromium aktualisiert die Rahmen asynchron; ohne diese Pause
            // faende der naechste Schritt denselben Knoten noch einmal.
            std::thread::sleep(std::time::Duration::from_millis(45));
        }
        CFRelease(web as *const c_void);
        eprintln!(
            "[FERNAX] scroll wid={wid} dy={dy} steps={getan}/{schritte} nodes={besucht_summe}"
        );
        getan > 0
    }

    /// Zeigerereignisse DIREKT an den Prozess. Der Systemzeiger bleibt, wo
    /// er ist; der Nutzer wechselt keinen Schreibtisch.
    /// Does this process have an open native menu window (layer 101)?
    pub fn menue_offen_von(pid: i32) -> bool {
        #[link(name = "CoreGraphics", kind = "framework")]
        extern "C" { fn CGWindowListCopyWindowInfo(option: u32, relative: u32) -> *const c_void; }
        #[link(name = "CoreFoundation", kind = "framework")]
        extern "C" {
            fn CFArrayGetCount(a: *const c_void) -> isize;
            fn CFArrayGetValueAtIndex(a: *const c_void, i: isize) -> *const c_void;
            fn CFDictionaryGetValue(d: *const c_void, k: *const c_void) -> *const c_void;
            fn CFNumberGetValue(n: *const c_void, typ: i32, out: *mut c_void) -> bool;
        }
        unsafe {
            let l = CGWindowListCopyWindowInfo(0, 0);
            if l.is_null() { return false; }
            let zahl = |d: *const c_void, k: &str| -> i64 {
                let key = cfstr(k); let v = CFDictionaryGetValue(d, key); CFRelease(key);
                let mut n: i64 = 0; if !v.is_null() { CFNumberGetValue(v, 4, &mut n as *mut i64 as *mut c_void); } n
            };
            let mut offen = false;
            for i in 0..CFArrayGetCount(l) {
                let d = CFArrayGetValueAtIndex(l, i);
                if zahl(d, "kCGWindowOwnerPID") == pid as i64 && zahl(d, "kCGWindowLayer") == 101 { offen = true; break; }
            }
            CFRelease(l);
            offen
        }
    }

    /// Did the last AX scroll attempt on this window just miss (no route)?
    pub fn roll_verfehlt(wid: i64) -> bool {
        ROLL_GRENZE.lock().is_ok_and(|g| g.as_ref().is_some_and(|(w, _, t)| *w == wid
            && t.elapsed() < std::time::Duration::from_millis(450)))
    }

    /// Native app without an AX scroll route (e.g. Goodnotes' document
    /// canvas): a pixel wheel event posted ONLY to that process and addressed
    /// to exactly this window (window fields 91/92). No global event, no
    /// cursor move, no activation (measured 2026-09-27: Goodnotes document
    /// scrolled and returned exactly, 0 Space switches).
    pub fn rad_an_prozess(pid: i32, wid: i64, x: f64, y: f64, dx: i32, dy: i32) -> bool {
        #[link(name = "CoreGraphics", kind = "framework")]
        extern "C" {
            fn CGEventCreateScrollWheelEvent2(src: *const c_void, units: u32, count: u32, w1: i32, w2: i32, w3: i32) -> *mut c_void;
        }
        if pid <= 0 || (dx == 0 && dy == 0) { return false; }
        // Trackpad-style gesture phases (UIKit/Catalyst scroll views need
        // began/changed/ended): began after a pause, "ended" 180 ms after
        // the last packet of this window.
        static GESTE: std::sync::Mutex<Option<(i64, std::time::Instant, u64)>> = std::sync::Mutex::new(None);
        unsafe fn senden(pid: i32, wid: i64, x: f64, y: f64, w1: i32, w2: i32, phase: i64) {
            let ev = CGEventCreateScrollWheelEvent2(std::ptr::null(), 0, 2, w1, w2, 0);
            if ev.is_null() { return; }
            CGEventSetIntegerValueField(ev, 88, 1); // continuous (pixel) scrolling
            CGEventSetIntegerValueField(ev, 99, phase); // kCGScrollWheelEventScrollPhase
            CGEventSetIntegerValueField(ev, 91, wid);
            CGEventSetIntegerValueField(ev, 92, wid);
            CGEventSetIntegerValueField(ev, 42, crate::fern_tippen::MARKE);
            CGEventSetLocation(ev, Punkt { x, y });
            CGEventPostToPid(pid, ev);
            CFRelease(ev as *const c_void);
        }
        let (phase, generation) = {
            let mut g = GESTE.lock().unwrap_or_else(|e| e.into_inner());
            let laeuft = g.as_ref().is_some_and(|(w, t, _)| *w == wid && t.elapsed() < std::time::Duration::from_millis(180));
            let gen = g.as_ref().map(|e| e.2).unwrap_or(0) + 1;
            *g = Some((wid, std::time::Instant::now(), gen));
            (if laeuft { 2 } else { 1 }, gen)
        };
        // Helper units are finger points * 0.25; same scale as the Noki Browser.
        unsafe { senden(pid, wid, x, y, (dy as f64 * 6.4) as i32, (dx as f64 * 6.4) as i32, phase); }
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(180));
            let ende = GESTE.lock().is_ok_and(|g| g.as_ref().is_some_and(|e| e.2 == generation));
            if ende { unsafe { senden(pid, wid, x, y, 0, 0, 4); } }
        });
        crate::virtual_workspace::trace(&format!("[INTERAKTION] scroll route=pid_wheel wid={wid} dx={dx} dy={dy}"));
        true
    }

    /// GoodNotes (Catalyst/UIKit) scroll: the trackpad gesture itself, packet
    /// by packet, as a continuous wheel event posted to exactly that process
    /// AND addressed to exactly that window (field 51 = window under the
    /// pointer). Measured 2026-09-27: GoodNotes ignores process wheels that
    /// carry only fields 91/92 (what `rad_an_prozess` sends), but with field
    /// 51 its UIScrollView under the point pans 1:1-ish and continuously
    /// (30 pt slow input -> 40 pt, momentum phases honored), window frame
    /// unchanged, cursor unchanged, no activation. The UIKit hit test at the
    /// point picks the region (document, sidebar, lists) like a real wheel.
    /// Arrow keys (the old route) move 89 pt per press - the "big jumps".
    /// `phase`: b/c/e finger, M/m/E momentum, w classic wheel notch.
    pub fn katalyst_rad(pid: i32, wid: i64, x: f64, y: f64, fdx: f64, fdy: f64, phase: &str) -> bool {
        #[link(name = "CoreGraphics", kind = "framework")]
        extern "C" {
            fn CGEventCreateScrollWheelEvent2(src: *const c_void, units: u32, count: u32, w1: i32, w2: i32, w3: i32) -> *mut c_void;
        }
        // (wid, carried remainder x/y, finger gesture open, generation, last packet)
        static G: std::sync::Mutex<(i64, f64, f64, bool, u64)> = std::sync::Mutex::new((0, 0.0, 0.0, false, 0));
        unsafe fn senden(pid: i32, wid: i64, x: f64, y: f64, px: i32, py: i32, phase: i64, momentum: i64, kontinuierlich: bool) {
            let ev = CGEventCreateScrollWheelEvent2(std::ptr::null(), if kontinuierlich { 0 } else { 1 }, 2, py, px, 0);
            if ev.is_null() { return; }
            if kontinuierlich {
                CGEventSetIntegerValueField(ev, 88, 1); // kCGScrollWheelEventIsContinuous
                CGEventSetIntegerValueField(ev, 96, py as i64); // point delta axis 1
                CGEventSetIntegerValueField(ev, 97, px as i64);
                CGEventSetDoubleValueField(ev, 93, py as f64); // fixed-point delta axis 1
                CGEventSetDoubleValueField(ev, 94, px as f64);
                CGEventSetIntegerValueField(ev, 99, phase); // scroll phase
                CGEventSetIntegerValueField(ev, 123, momentum); // momentum phase
            }
            CGEventSetIntegerValueField(ev, 51, wid);
            CGEventSetIntegerValueField(ev, 42, crate::fern_tippen::MARKE);
            CGEventSetLocation(ev, Punkt { x, y });
            CGEventPostToPid(pid, ev);
            CFRelease(ev as *const c_void);
        }
        let (ptx, pty) = if phase == "w" { (fdx, fdy) } else { (fdx / 0.25, fdy / 0.25) };
        let g = if phase == "w" { 1.0 } else { 1.4 + 0.4 * (ptx.hypot(pty) / 24.0).min(1.0) };
        let mut st = G.lock().unwrap_or_else(|e| e.into_inner());
        if st.0 != wid { *st = (wid, 0.0, 0.0, false, st.4); }
        st.1 += ptx * g; st.2 += pty * g;
        let (px, py) = (st.1.trunc() as i32, st.2.trunc() as i32);
        st.1 -= px as f64; st.2 -= py as f64;
        st.4 = st.4.wrapping_add(1);
        let generation = st.4;
        let offen = st.3;
        let (ph, mo) = match phase {
            "b" => (1, 0),
            "c" => (if offen { 2 } else { 1 }, 0),
            "e" => (4, 0),
            "M" => (0, 1),
            "m" => (0, 2),
            "E" => (0, 3),
            _ => (0, 0),
        };
        if phase == "w" {
            if px == 0 && py == 0 { return true; }
            unsafe { senden(pid, wid, x, y, px, py, 0, 0, false); }
            return true;
        }
        // A finger packet arriving without its "began" (gesture started
        // elsewhere): open it; an ended/momentum phase closes it.
        st.3 = matches!(ph, 1 | 2);
        if px == 0 && py == 0 && !matches!(phase, "b" | "e" | "E") { return true; }
        drop(st);
        unsafe { senden(pid, wid, x, y, px, py, ph, mo, true); }
        // Watchdog: a finger gesture whose "ended" never arrives (helper
        // retargeted mid-gesture) must not leave UIKit panning. One watcher
        // per open gesture (not one thread per packet).
        static WACHE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if matches!(ph, 1 | 2) && !WACHE.swap(true, std::sync::atomic::Ordering::SeqCst) {
            std::thread::spawn(move || {
                let mut gesehen = generation;
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(350));
                    let mut st = G.lock().unwrap_or_else(|e| e.into_inner());
                    if !st.3 { break; }
                    if st.4 != gesehen { gesehen = st.4; continue; }
                    st.3 = false;
                    let w = st.0;
                    drop(st);
                    unsafe { senden(pid, w, x, y, 0, 0, 4, 0, true); }
                    break;
                }
                WACHE.store(false, std::sync::atomic::Ordering::SeqCst);
            });
        }
        true
    }

    /// Canvas-like content (Goodnotes' document) ignores process wheels but
    /// scrolls with arrow keys. Only when this window IS the app's key window
    /// (keys go there) and the element under the point has no selection
    /// semantics (lists/tables/text would change selection). One arrow per
    /// ~100 px of gesture. Measured 2026-09-27: Goodnotes scrolled, 0 Space
    /// switches, front app unchanged.
    pub fn pfeil_rollen(pid: i32, wid: i64, x: f64, y: f64, dy: i32) -> bool {
        static AKKU: std::sync::Mutex<(i64, f64)> = std::sync::Mutex::new((0, 0.0));
        if dy == 0 { return false; }
        let (rolle, _) = rolle_am_punkt(pid, wid, x, y);
        if !matches!(rolle.as_str(), "AXGenericElement" | "AXGroup" | "AXStaticText" | "AXImage" | "AXScrollArea" | "AXLayoutArea") {
            return false;
        }
        let schluessel = unsafe {
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return false; }
            let number_attr = cfstr("AXWindowNumber");
            let n = ax_element_attr(app, "AXFocusedWindow").and_then(|w| {
                let n = ax_fenster_nummer(w, number_attr); CFRelease(w as *const c_void); n
            });
            CFRelease(number_attr);
            CFRelease(app as *const c_void);
            n == Some(wid)
        };
        if !schluessel { return false; }
        let schritte = {
            let mut a = AKKU.lock().unwrap_or_else(|e| e.into_inner());
            if a.0 != wid || a.1.signum() != (-(dy as f64)).signum() { *a = (wid, 0.0); }
            a.1 += -(dy as f64) * 6.4; // > 0: page moves down
            let n = (a.1 / 100.0).trunc();
            a.1 -= n * 100.0;
            n as i32
        };
        unsafe {
            for _ in 0..schritte.unsigned_abs().min(6) {
                let code: u16 = if schritte > 0 { 125 } else { 126 };
                for runter in [true, false] {
                    let ev = CGEventCreateKeyboardEvent(std::ptr::null(), code, runter);
                    if ev.is_null() { continue; }
                    CGEventSetFlags(ev, 0);
                    CGEventSetIntegerValueField(ev, 42, crate::fern_tippen::MARKE);
                    CGEventPostToPid(pid, ev);
                    CFRelease(ev as *const c_void);
                }
            }
        }
        crate::virtual_workspace::trace(&format!("[INTERAKTION] scroll route=key_window_arrows wid={wid} role={rolle} arrows={schritte}"));
        true
    }

    unsafe fn zeiger_an_prozess(pid: i32, wid: i64, x: f64, y: f64, klicks: i64) {
        let p = Punkt { x, y };
        // kCGEventLeftMouseDown = 1, Up = 2; kCGMouseEventClickState = 1
        for (typ, _) in [(1u32, 0), (2u32, 0)] {
            let ev = CGEventCreateMouseEvent(std::ptr::null(), typ, p, 0);
            if ev.is_null() {
                continue;
            }
            CGEventSetIntegerValueField(ev, 1, klicks.clamp(1, 3));
            CGEventSetIntegerValueField(ev, 91, wid);
            CGEventSetIntegerValueField(ev, 92, wid);
            CGEventSetLocation(ev, p);
            CGEventPostToPid(pid, ev);
            CFRelease(ev as *const c_void);
        }
    }


    // -----------------------------------------------------------------
    //  Fensterscharfe Bedienung (virtueller Noki Schreibtisch)
    // -----------------------------------------------------------------

    /// Retained AX element of exactly this window - also for a window on
    /// another Space / in native fullscreen (remote-token route; AXWindows
    /// lists only the current Space). Caller releases it (as usize ptr).
    pub fn ax_fenster_fuer(pid: i32, wid: i64) -> Option<usize> {
        unsafe {
            if !AXIsProcessTrusted() || pid <= 0 || wid <= 0 { return None; }
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return None; }
            let r = ax_fenster_element(app, wid);
            CFRelease(app as *const c_void);
            r.map(|p| p as usize)
        }
    }

    /// Das AX-Fenster mit genau dieser CGWindowID (retained) oder None.
    unsafe fn ax_fenster_element(app: *mut c_void, wid: i64) -> Option<*mut c_void> {
        let mut app_pid: i32 = 0;
        if AXUIElementGetPid(app, &mut app_pid) == 0 && simuliert(app_pid, wid) { return None; }
        let windows_attr = cfstr("AXWindows");
        let number_attr = cfstr("AXWindowNumber");
        let mut windows: *const c_void = std::ptr::null();
        let mut found = None;
        if AXUIElementCopyAttributeValue(app, windows_attr, &mut windows) == 0 && !windows.is_null() {
            for i in 0..CFArrayGetCount(windows) {
                let window = CFArrayGetValueAtIndex(windows, i) as *mut c_void;
                if ax_fenster_nummer(window, number_attr) == Some(wid) {
                    found = Some(CFRetain(window as *const c_void) as *mut c_void);
                    break;
                }
            }
            // Sheets (Chrome's file chooser, save sheets) are AXSheet
            // children of their parent window, not AXWindows entries.
            if found.is_none() {
                'aussen: for i in 0..CFArrayGetCount(windows) {
                    let window = CFArrayGetValueAtIndex(windows, i) as *mut c_void;
                    let Some(kinder) = ax_element_attr(window, "AXChildren") else { continue };
                    for j in 0..CFArrayGetCount(kinder as *const c_void) {
                        let k = CFArrayGetValueAtIndex(kinder as *const c_void, j) as *mut c_void;
                        if ax_rolle(k) == "AXSheet" && ax_fenster_nummer(k, number_attr) == Some(wid) {
                            found = Some(CFRetain(k as *const c_void) as *mut c_void);
                            CFRelease(kinder as *const c_void);
                            break 'aussen;
                        }
                    }
                    CFRelease(kinder as *const c_void);
                }
            }
            CFRelease(windows);
        }
        // Modal dialogs of some apps (VS Code "Open") are missing from
        // AXWindows but are the app's focused window.
        if found.is_none() {
            let fokus = cfstr("AXFocusedWindow");
            let mut f: *const c_void = std::ptr::null();
            if AXUIElementCopyAttributeValue(app, fokus, &mut f) == 0 && !f.is_null() {
                if ax_fenster_nummer(f as *mut c_void, number_attr) == Some(wid) { found = Some(f as *mut c_void); }
                else { CFRelease(f); }
            }
            CFRelease(fokus);
        }
        // Window on ANOTHER Space (REAL_SPACE: every Noki window while the
        // user stays on their Desktop). Measured macOS 26.6: AXWindows is
        // EMPTY for off-Space windows (Goodnotes: []), so a hidden Terminal
        // next to the user's own Terminal window was never found and every
        // editable lookup failed. The element exists and is fully usable;
        // it is only reachable by its remote token (same approach as
        // AltTab). Ids seen up to ~15800 (Safari); one scan caches every window of the app.
        if found.is_none() {
            found = ax_fenster_ueber_token(app, wid, number_attr);
        }
        CFRelease(windows_attr);
        CFRelease(number_attr);
        found
    }

    /// Screen locked? Then every hidden-window AX lookup fails (measured
    /// 2026-09-27: remote tokens return nothing for ALL apps) - such misses
    /// say nothing about the window and are never remembered.
    pub fn bildschirm_gesperrt() -> bool {
        #[link(name = "CoreGraphics", kind = "framework")]
        extern "C" { fn CGSessionCopyCurrentDictionary() -> *const c_void; }
        #[link(name = "CoreFoundation", kind = "framework")]
        extern "C" { fn CFDictionaryGetValue(d: *const c_void, k: *const c_void) -> *const c_void; }
        unsafe {
            let d = CGSessionCopyCurrentDictionary();
            if d.is_null() { return false; }
            let k = cfstr("CGSSessionScreenIsLocked");
            let v = CFDictionaryGetValue(d, k);
            CFRelease(k);
            let an = !v.is_null() && v == kCFBooleanTrue;
            CFRelease(d);
            an
        }
    }

    /// Recent token-scan misses (pid, wid, when); expire after 20 s.
    static FEHL: std::sync::Mutex<Vec<(i32, i64, std::time::Instant)>> = std::sync::Mutex::new(Vec::new());

    /// Test hook: for `sekunden` every lookup of this window fails like a
    /// transient AX outage (locked screen, busy app) and records a miss;
    /// the cached token is dropped. Recovery must need no restart.
    static SIMULIERT: std::sync::Mutex<Vec<(i32, i64, std::time::Instant)>> = std::sync::Mutex::new(Vec::new());
    pub fn fehlschlag_simulieren(pid: i32, wid: i64, sekunden: u64) {
        if let Ok(mut c) = TOKEN_CACHE.lock() { c.retain(|(p, w, _)| !(*p == pid && *w == wid)); }
        if let Ok(mut g) = SIMULIERT.lock() { g.push((pid, wid, std::time::Instant::now() + std::time::Duration::from_secs(sekunden))); }
        crate::virtual_workspace::trace(&format!("[FERNAX] simulated_outage wid={wid} pid={pid} s={sekunden}"));
    }
    fn simuliert(pid: i32, wid: i64) -> bool {
        let an = SIMULIERT.lock().map(|mut g| {
            g.retain(|e| e.2 > std::time::Instant::now());
            g.iter().any(|e| e.0 == pid && e.1 == wid)
        }).unwrap_or(false);
        if an { if let Ok(mut f) = FEHL.lock() { f.push((pid, wid, std::time::Instant::now())); } }
        an
    }

    /// (pid, wid) -> remote-token element id of that AXWindow.
    static TOKEN_CACHE: std::sync::Mutex<Vec<(i32, i64, u64)>> = std::sync::Mutex::new(Vec::new());

    thread_local! {
        /// Only popover dismissal may accept a non-AXWindow root; geometry
        /// and every other caller must get the real window or nothing.
        static NICHT_FENSTER_ERLAUBT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    unsafe fn ax_fenster_ueber_token(app: *mut c_void, wid: i64, number_attr: *const c_void) -> Option<*mut c_void> {
        #[link(name = "CoreFoundation", kind = "framework")]
        extern "C" {
            fn CFDataCreate(a: *const c_void, b: *const u8, n: isize) -> *const c_void;
        }
        let symbol = dlsym((-2isize) as *mut c_void, b"_AXUIElementCreateWithRemoteToken\0".as_ptr() as *const i8);
        if symbol.is_null() { return None; }
        let erzeugen: unsafe extern "C" fn(*const c_void) -> *mut c_void = std::mem::transmute(symbol);
        let symbol = dlsym((-2isize) as *mut c_void, b"_AXUIElementGetWindow\0".as_ptr() as *const i8);
        if symbol.is_null() { return None; }
        let hole_fenster: unsafe extern "C" fn(*mut c_void, *mut u32) -> i32 = std::mem::transmute(symbol);
        let _ = number_attr;
        let mut pid: i32 = 0;
        if AXUIElementGetPid(app, &mut pid) != 0 || pid <= 0 { return None; }
        // Popover/transient windows (GoodNotes) have no AXWindow root: the
        // root-most element (lowest id) carrying the window number stands in.
        let ersatz: std::cell::Cell<Option<u64>> = std::cell::Cell::new(None);
        let roh_element = |id: u64| -> Option<*mut c_void> {
            let mut roh = [0u8; 20];
            roh[0..4].copy_from_slice(&pid.to_ne_bytes());
            roh[8..12].copy_from_slice(&0x636f_636fi32.to_ne_bytes()); // 'coco'
            roh[12..20].copy_from_slice(&id.to_ne_bytes());
            let daten = CFDataCreate(std::ptr::null(), roh.as_ptr(), 20);
            if daten.is_null() { return None; }
            let e = erzeugen(daten);
            CFRelease(daten);
            (!e.is_null()).then_some(e)
        };
        let element = |id: u64| -> Option<*mut c_void> {
            let mut roh = [0u8; 20];
            roh[0..4].copy_from_slice(&pid.to_ne_bytes());
            roh[8..12].copy_from_slice(&0x636f_636fi32.to_ne_bytes()); // 'coco'
            roh[12..20].copy_from_slice(&id.to_ne_bytes());
            let daten = CFDataCreate(std::ptr::null(), roh.as_ptr(), 20);
            if daten.is_null() { return None; }
            let e = erzeugen(daten);
            CFRelease(daten);
            if e.is_null() { return None; }
            // One IPC per candidate: the window number first, role only on a match.
            let mut w: u32 = 0;
            if hole_fenster(e, &mut w) == 0 && w != 0 && ax_rolle(e) == "AXWindow" {
                // Every window met on the way is cached (one scan serves all
                // of this app's hidden windows).
                if let Ok(mut c) = TOKEN_CACHE.lock() {
                    if !c.iter().any(|(p, x, _)| *p == pid && *x == w as i64) { c.push((pid, w as i64, id)); }
                    if c.len() > 128 { c.remove(0); }
                }
                if w as i64 == wid { return Some(e); }
            } else if w as i64 == wid && ersatz.get().is_none_or(|x| id < x) {
                ersatz.set(Some(id));
            }
            CFRelease(e as *const c_void);
            None
        };
        let gemerkt = TOKEN_CACHE.lock().ok()
            .and_then(|c| c.iter().find(|(p, w, _)| *p == pid && *w == wid).map(|(_, _, id)| *id));
        if let Some(id) = gemerkt {
            if let Some(e) = element(id) { return Some(e); }
        }
        // A window without an AX element (closed but listed, or an invisible
        // helper window - measured Safari 822557: ordered in, no image, no
        // token up to 65536) must never cost another full scan: window ids
        // are not reused, so a miss is remembered for good (bounded).
        // Misses expire after 20 s: a PERMANENT miss (previous version)
        // made a Chrome window unreachable forever after one lookup ran
        // before Chromium had built its accessibility tree (measured: Noki
        // Browser window_found=false for every later click).
        if let Ok(mut f) = FEHL.lock() {
            f.retain(|e| e.2.elapsed() < std::time::Duration::from_secs(20));
            if f.iter().any(|(p, w, _)| *p == pid && *w == wid) { return None; }
        }
        // Locked screen: every remote token fails for every app (measured),
        // and each full scan cost ~3 s of the calling lane - repeated for
        // every window on every lookup until unlock.
        if bildschirm_gesperrt() { return None; }
        // Chromium exposes its windows/trees only with manual accessibility on.
        ax_manuell_an(app, pid);
        // A busy/hung app answers nothing: 65536 IPCs x messaging timeout
        // would block this lane for hours ("Miniatur frozen, only close and
        // front work"). Ask once first; a scan never outlives its budget.
        {
            let rolle = cfstr("AXRole");
            let mut v: *const c_void = std::ptr::null();
            let antwortet = AXUIElementCopyAttributeValue(app, rolle, &mut v) == 0;
            if !v.is_null() { CFRelease(v); }
            CFRelease(rolle);
            if !antwortet {
                crate::virtual_workspace::trace(&format!(
                    "[FERNAX] offspace_window wid={wid} pid={pid} route=remote_token skipped=app_not_answering"));
                return None;
            }
        }
        // Resumable per app: an aborted scan continues where it stopped on
        // the next lookup (found windows are cached on the way anyway).
        static WEITER: std::sync::Mutex<Vec<(i32, u64, std::time::Instant)>> = std::sync::Mutex::new(Vec::new());
        let start = WEITER.lock().ok()
            .and_then(|g| g.iter().find(|e| e.0 == pid && e.2.elapsed() < std::time::Duration::from_secs(30)).map(|e| e.1))
            .unwrap_or(0);
        let t0 = std::time::Instant::now();
        let budget = std::time::Duration::from_millis(1200);
        for k in 0..65536u64 {
            let id = (start + k) % 65536;
            if k % 128 == 127 && (zeit_um() || t0.elapsed() > budget) {
                crate::virtual_workspace::trace(&format!(
                    "[FERNAX] offspace_window wid={wid} pid={pid} route=remote_token aborted_at={id} ms={}", t0.elapsed().as_millis()));
                if let Ok(mut g) = WEITER.lock() { g.retain(|e| e.0 != pid); g.push((pid, id, std::time::Instant::now())); if g.len() > 64 { g.remove(0); } }
                // Short miss (5 s of the 20 s window): callers polling every
                // tick must not pay the budget each time.
                let t = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(15)).unwrap_or_else(std::time::Instant::now);
                if let Ok(mut f) = FEHL.lock() { f.push((pid, wid, t)); if f.len() > 256 { f.remove(0); } }
                return None;
            }
            if let Some(e) = element(id) {
                if let Ok(mut c) = TOKEN_CACHE.lock() {
                    c.retain(|(p, w, _)| !(*p == pid && *w == wid));
                    c.push((pid, wid, id));
                    if c.len() > 64 { c.remove(0); }
                }
                crate::virtual_workspace::trace(&format!(
                    "[FERNAX] offspace_window wid={wid} pid={pid} route=remote_token ms={}", t0.elapsed().as_millis()));
                return Some(e);
            }
        }
        if let Ok(mut g) = WEITER.lock() { g.retain(|e| e.0 != pid); }
        if let Some(e) = ersatz.get().filter(|_| NICHT_FENSTER_ERLAUBT.with(|f| f.get())).and_then(roh_element) {
            crate::virtual_workspace::trace(&format!(
                "[FERNAX] offspace_window wid={wid} pid={pid} route=remote_token root=non_window ms={}", t0.elapsed().as_millis()));
            return Some(e);
        }
        crate::virtual_workspace::trace(&format!(
            "[FERNAX] offspace_window wid={wid} pid={pid} route=remote_token found=false ms={}", t0.elapsed().as_millis()));
        // Only when the app demonstrably answered (a busy app times out:
        // that miss must not become permanent).
        let rolle = cfstr("AXRole");
        let mut v: *const c_void = std::ptr::null();
        let antwortet = AXUIElementCopyAttributeValue(app, rolle, &mut v) == 0;
        if !v.is_null() { CFRelease(v); }
        CFRelease(rolle);
        if antwortet && !bildschirm_gesperrt() { if let Ok(mut f) = FEHL.lock() { f.push((pid, wid, std::time::Instant::now())); if f.len() > 256 { f.remove(0); } } }
        None
    }

    unsafe fn ax_element_attr(el: *mut c_void, name: &str) -> Option<*mut c_void> {
        let attr = cfstr(name);
        let mut value: *const c_void = std::ptr::null();
        let ok = AXUIElementCopyAttributeValue(el, attr, &mut value) == 0 && !value.is_null();
        CFRelease(attr);
        ok.then_some(value as *mut c_void)
    }

    unsafe fn tab_im_baum(
        el: *mut c_void, x: f64, y: f64, tiefe: usize, besucht: &mut usize,
    ) -> bool {
        if tiefe > 24 || *besucht > 8_000 || zeit_um() { return false; }
        *besucht += 1;
        if let Some(r) = ax_rahmen(el) {
            if x < r.o.x || y < r.o.y || x > r.o.x + r.g.w || y > r.o.y + r.g.h {
                return false;
            }
        }
        if ax_text_attr(el, "AXSubrole") == "AXTabButton" { return true; }
        let Some(children) = ax_element_attr(el, "AXChildren") else { return false };
        let mut found = false;
        for i in (0..CFArrayGetCount(children)).rev() {
            let child = CFArrayGetValueAtIndex(children, i) as *mut c_void;
            if !child.is_null() && tab_im_baum(child, x, y, tiefe + 1, besucht) {
                found = true;
                break;
            }
        }
        CFRelease(children as *const c_void);
        found
    }

    /// Small failure-only diagnostic: Chrome's AX coordinates can differ
    /// from the captured CG frame while a virtual display is being retargeted.
    /// Report only semantic tab frames, never the whole accessibility tree.
    unsafe fn tab_rahmen_im_baum(
        el: *mut c_void, tiefe: usize, besucht: &mut usize, out: &mut Vec<String>,
    ) {
        if tiefe > 24 || *besucht > 8_000 || out.len() >= 20 || zeit_um() { return; }
        *besucht += 1;
        if ax_text_attr(el, "AXSubrole") == "AXTabButton" {
            if let Some(r) = ax_rahmen(el) {
                out.push(format!("{:.0},{:.0},{:.0}x{:.0}", r.o.x, r.o.y, r.g.w, r.g.h));
            }
        }
        let Some(children) = ax_element_attr(el, "AXChildren") else { return };
        for i in 0..CFArrayGetCount(children) {
            let child = CFArrayGetValueAtIndex(children, i) as *mut c_void;
            if !child.is_null() { tab_rahmen_im_baum(child, tiefe + 1, besucht, out); }
        }
        CFRelease(children as *const c_void);
    }

    /// Exact-window semantic tab hit test. No coordinate heuristics: the
    /// point must resolve to an AXTabButton inside this CGWindowID.
    pub fn tab_am_punkt(pid: i32, wid: i64, x: f64, y: f64) -> bool {
        unsafe {
            if !AXIsProcessTrusted() || pid <= 0 || wid <= 0 { return false; }
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return false; }
            ax_manuell_an(app, pid);
            let found = ax_fenster_element(app, wid).is_some_and(|window| {
                let mut besucht = 0;
                let yes = tab_im_baum(window, x, y, 0, &mut besucht);
                if !yes {
                    let mut scan = 0;
                    let mut frames = Vec::new();
                    tab_rahmen_im_baum(window, 0, &mut scan, &mut frames);
                    crate::virtual_workspace::trace(&format!(
                        "[TAB_DRAG] candidates wid={wid} frames=[{}]", frames.join(";")
                    ));
                }
                CFRelease(window as *const c_void);
                crate::virtual_workspace::trace(&format!(
                    "[TAB_DRAG] hit_test wid={wid} nodes={besucht} semantic={yes}"
                ));
                yes
            });
            CFRelease(app as *const c_void);
            found
        }
    }

    /// Window-targeted native drag packet. `phase`: 0 down, 1 drag, 2 up.
    /// CGEventPostToPid leaves the global cursor and the user's Space alone.
    pub fn tab_ziehen_event(pid: i32, x: f64, y: f64, phase: u8) {
        unsafe {
            let typ = match phase { 0 => 1, 1 => 6, _ => 2 }; // leftDown/dragged/up
            let ev = CGEventCreateMouseEvent(std::ptr::null(), typ, Punkt { x, y }, 0);
            if ev.is_null() { return; }
            CGEventSetIntegerValueField(ev, 42, crate::fern_tippen::MARKE);
            CGEventPostToPid(pid, ev);
            CFRelease(ev as *const c_void);
        }
    }

    unsafe fn ax_zahl_attr(el: *mut c_void, name: &str) -> Option<i64> {
        let v = ax_element_attr(el, name)?;
        let mut n = 0i64;
        let ok = CFNumberGetValue(v, 4, &mut n as *mut i64 as *mut c_void);
        CFRelease(v);
        ok.then_some(n)
    }

    /// Element am Punkt, aber nur, wenn es zu genau diesem Fenster gehoert.
    unsafe fn ax_element_im_fenster(app: *mut c_void, wid: i64, x: f64, y: f64) -> Option<*mut c_void> {
        let mut el: *mut c_void = std::ptr::null_mut();
        if AXUIElementCopyElementAtPosition(app, x as f32, y as f32, &mut el) != 0 || el.is_null() {
            return None;
        }
        let number_attr = cfstr("AXWindowNumber");
        let eigen = ax_fenster_nummer(el, number_attr) == Some(wid);
        CFRelease(number_attr);
        if eigen { Some(el) } else { CFRelease(el as *const c_void); None }
    }

    /// Chrome-Tabs: AXPress ist erst ein Erfolg, wenn der Tab danach wirklich
    /// ausgewaehlt ist (bzw. beim Schliessknopf: der Tab wirklich weg ist).
    unsafe fn tab_nachweis(el: *mut c_void) -> Option<bool> {
        let sub = ax_text_attr(el, "AXSubrole");
        if sub == "AXTabButton" {
            for _ in 0..15 {
                if ax_zahl_attr(el, "AXValue") == Some(1) { return Some(true); }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            return Some(false);
        }
        let parent = ax_element_attr(el, "AXParent")?;
        let ist_tab = ax_text_attr(parent, "AXSubrole") == "AXTabButton";
        if !ist_tab || ax_rolle(el) != "AXButton" { CFRelease(parent); return None; }
        // Schliessknopf eines Tabs: der Tab verschwindet aus seiner Gruppe.
        let mut weg = false;
        for _ in 0..25 {
            if ax_rolle(parent).is_empty() { weg = true; break; }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        CFRelease(parent);
        Some(weg)
    }

    unsafe fn druecken_mit_nachweis(el: *mut c_void, wo: &str) -> bool {
        let titel = ax_text_attr(el, "AXTitle");
        let sub = ax_text_attr(el, "AXSubrole");
        let rolle = ax_rolle(el);
        let press = cfstr("AXPress");
        let ok = AXUIElementPerformAction(el, press) == 0;
        CFRelease(press);
        if !ok { return false; }
        match tab_nachweis(el) {
            Some(nachweis) => {
                crate::virtual_workspace::trace(&format!(
                    "[TAB] {} via={wo} role={rolle}/{sub} title_len={} verified={nachweis}",
                    if sub == "AXTabButton" { "activate" } else { "close" }, titel.chars().count()
                ));
                nachweis
            }
            None => true,
        }
    }

    unsafe fn ax_press_fenster_baum(
        el: *mut c_void, x: f64, y: f64, tiefe: usize, besucht: &mut usize, generisch: bool,
    ) -> bool {
        if tiefe > 40 || *besucht >= 20_000 || zeit_um() { return false; }
        *besucht += 1;
        if let Some(r) = ax_rahmen(el) {
            if x < r.o.x || y < r.o.y || x > r.o.x + r.g.w || y > r.o.y + r.g.h {
                return false;
            }
        }
        let attr = cfstr("AXChildren");
        let mut kinder: *const c_void = std::ptr::null();
        let hat = AXUIElementCopyAttributeValue(el, attr, &mut kinder) == 0 && !kinder.is_null();
        CFRelease(attr);
        if hat {
            for i in (0..CFArrayGetCount(kinder)).rev() {
                let kind = CFArrayGetValueAtIndex(kinder, i) as *mut c_void;
                if !kind.is_null() && ax_press_fenster_baum(kind, x, y, tiefe + 1, besucht, generisch) {
                    CFRelease(kinder);
                    return true;
                }
            }
            CFRelease(kinder);
        }
        let darf = ax_rolle_erlaubt(el) || (generisch && !matches!(
            ax_rolle(el).as_str(),
            "AXWebArea" | "AXWindow" | "AXApplication" | "AXScrollArea" | ""
        ));
        if !darf { return false; }
        let mut namen: *const c_void = std::ptr::null();
        if AXUIElementCopyActionNames(el, &mut namen) != 0 || namen.is_null() { return false; }
        let press = cfstr("AXPress");
        let mut hat_press = false;
        for i in 0..CFArrayGetCount(namen) {
            let n = CFArrayGetValueAtIndex(namen, i);
            if !n.is_null() && CFStringCompare(n, press, 0) == 0 { hat_press = true; break; }
        }
        CFRelease(press);
        CFRelease(namen);
        hat_press && druecken_mit_nachweis(el, if generisch { "tree-generic" } else { "tree" })
    }

    /// Derselbe semantische Klick wie `ax_druecken` + generischer Rueckfall,
    /// aber ausschliesslich im Zielfenster. Gemessen: Chrome liefert am Punkt
    /// eines Tabs nur die AXGroup der Tableiste; der fruehere Rueckfall lief
    /// dann durch den Baum der GANZEN App (auch fremde und dahinterliegende
    /// Fenster) und traf je nach Reihenfolge das falsche Fenster oder lief in
    /// die Knotengrenze. Jetzt: Tab gedrueckt, Auswahl nachgewiesen.
    unsafe fn ax_druecken_fenster(pid: i32, wid: i64, x: f64, y: f64) -> bool {
        if !AXIsProcessTrusted() || pid <= 0 || wid <= 0 { return false; }
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() { return false; }
        ax_manuell_an(app, pid);
        let Some(fenster) = ax_fenster_element(app, wid) else {
            CFRelease(app as *const c_void);
            return false;
        };
        let mut getan = false;
        // 1. Element am Punkt und seine Vorfahren (nur im Zielfenster).
        if let Some(el) = ax_element_im_fenster(app, wid, x, y) {
            let mut cur = el;
            for _ in 0..6 {
                if ax_rolle_erlaubt(cur) && ax_aktion_vorhanden(cur, "AXPress") {
                    getan = druecken_mit_nachweis(cur, "point");
                    break;
                }
                let Some(parent) = ax_element_attr(cur, "AXParent") else { break };
                if cur != el { CFRelease(cur as *const c_void); }
                cur = parent;
            }
            if cur != el { CFRelease(cur as *const c_void); }
            CFRelease(el as *const c_void);
        }
        // 2. Semantic controls in the exact target window (Chrome tabs,
        // buttons, links). Do not accept a generic AXGroup/AXImage merely
        // because it advertises AXPress: Electron/Chromium surfaces (Claude
        // composer and Safari start-page tiles measured) return success
        // without focus or click. The caller then uses the exact-window,
        // PID-targeted native mouse packet; the global cursor never moves.
        if !getan {
            let mut besucht = 0usize;
            getan = ax_press_fenster_baum(fenster, x, y, 0, &mut besucht, false);
            eprintln!("[FERNAX] window click wid={wid} nodes={besucht} success={getan}");
        }
        CFRelease(fenster as *const c_void);
        CFRelease(app as *const c_void);
        getan
    }

    unsafe fn ax_aktion_vorhanden(el: *mut c_void, name: &str) -> bool {
        let mut namen: *const c_void = std::ptr::null();
        if AXUIElementCopyActionNames(el, &mut namen) != 0 || namen.is_null() { return false; }
        let gesucht = cfstr(name);
        let mut ok = false;
        for i in 0..CFArrayGetCount(namen) {
            let n = CFArrayGetValueAtIndex(namen, i);
            if !n.is_null() && CFStringCompare(n, gesucht, 0) == 0 { ok = true; break; }
        }
        CFRelease(gesucht);
        CFRelease(namen);
        ok
    }

    /// Das editierbare Element fuer ein Element: es selbst (Textfeld) oder
    /// die contenteditable-Wurzel. Retained.
    unsafe fn ax_editierbar(el: *mut c_void) -> Option<*mut c_void> {
        let rolle = ax_rolle(el);
        let sub = ax_text_attr(el, "AXSubrole");
        if matches!(rolle.as_str(), "AXTextField" | "AXTextArea" | "AXComboBox" | "AXSearchField")
            || sub == "AXSearchField" {
            // Ein Textfeld, dessen Wert sich nicht setzen laesst, ist
            // schreibgeschuetzt (z. B. readonly) - kein Tippziel.
            let attr = cfstr("AXValue");
            let mut setzbar: u8 = 0;
            let ok = AXUIElementIsAttributeSettable(el, attr, &mut setzbar) == 0;
            CFRelease(attr);
            if ok && setzbar != 0 { return Some(CFRetain(el as *const c_void) as *mut c_void); }
            // Terminal-artige Textflaeche: Wert nur lesbar, aber fokussierbar
            // mit setzbarer Schreibmarke - sie nimmt Tasten an (gemessen:
            // Terminal "Shell"). Schreibgeschuetzte Anzeigen haben keine
            // setzbare Auswahl.
            if rolle == "AXTextArea" && ax_setzbar(el, "AXFocused") && ax_setzbar(el, "AXSelectedTextRange") {
                return Some(CFRetain(el as *const c_void) as *mut c_void);
            }
            return None;
        }
        ax_element_attr(el, "AXEditableAncestor")
    }

    /// Klick auf ein Eingabefeld: Feld im Zielfenster finden, per AX
    /// fokussieren und nachweisen, dass Chrome genau dieses Feld in genau
    /// diesem Fenster als Fokus meldet. Rueckgabe: das Feld (retained).
    unsafe fn ax_eingabe_fokussieren(pid: i32, wid: i64, x: f64, y: f64)
        -> Option<(*mut c_void, Option<*mut c_void>)>
    {
        if !AXIsProcessTrusted() || pid <= 0 || wid <= 0 { return None; }
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() { return None; }
        ax_manuell_an(app, pid);
        let mut ziel = None;
        if let Some(el) = ax_element_im_fenster(app, wid, x, y) {
            let mut cur = el;
            for _ in 0..4 {
                if let Some(e) = ax_editierbar(cur) { ziel = Some(e); break; }
                // Ueber einen Knopf/Link hinaus wird nicht gesucht: wer auf
                // einen Knopf klickt, will ihn druecken, nicht tippen.
                if ax_rolle_erlaubt(cur) { break; }
                let Some(parent) = ax_element_attr(cur, "AXParent") else { break };
                if cur != el { CFRelease(cur as *const c_void); }
                cur = parent;
            }
            if cur != el { CFRelease(cur as *const c_void); }
            CFRelease(el as *const c_void);
        }
        // Chromium-Treffertest liefert fuer Nicht-Hauptfenster nur die
        // AXWebArea (gemessen: Motion-Web-App). Dann die Kette im Baum GENAU
        // dieses Fensters: tiefstes Element zuerst, hoechstens 4 Ebenen.
        if ziel.is_none() {
            // Platzhalter-Texte liegen oft UEBER dem eigentlichen Feld
            // (Geschwister): in jeder Ebene auch den Teilbaum nach einem
            // bearbeitbaren Element unter dem Punkt absuchen.
            unsafe fn feld_darunter(el: *mut c_void, x: f64, y: f64, tiefe: usize) -> Option<*mut c_void> {
                if tiefe > 3 { return None; }
                if ax_rahmen(el).is_some_and(|r| x >= r.o.x && y >= r.o.y && x <= r.o.x + r.g.w && y <= r.o.y + r.g.h) {
                    if let Some(f) = ax_editierbar(el) { return Some(f); }
                }
                let k = ax_element_attr(el, "AXChildren")?;
                let mut r = None;
                for i in 0..CFArrayGetCount(k) {
                    if let Some(f) = feld_darunter(CFArrayGetValueAtIndex(k, i) as *mut c_void, x, y, tiefe + 1) { r = Some(f); break; }
                }
                CFRelease(k as *const c_void);
                r
            }
            let kette = kette_im_fenster(pid, wid, x, y);
            for &e in kette.iter().take(5) {
                if let Some(f) = ax_editierbar(e).or_else(|| feld_darunter(e, x, y, 0)) { ziel = Some(f); break; }
                if ax_rolle_erlaubt(e) { break; }
            }
            kette_freigeben(kette);
        }
        // Code-Editoren (Monaco, CodeMirror, Ace): das Tippziel ist ein
        // winziges Eingabefeld an der Schreibmarke, nicht unter dem Zeiger.
        // Gemessen VS Code: AXTextArea 203x18 in der Editorflaeche 853x529.
        // Gilt nur, wenn der Bereich um den Punkt (kleiner als das Fenster,
        // kein Knopf/Link dazwischen) GENAU ein mehrzeiliges Feld enthaelt.
        if ziel.is_none() {
            unsafe fn felder(el: *mut c_void, tiefe: usize, aus: &mut Vec<*mut c_void>) {
                if tiefe > 4 || aus.len() > 1 { return; }
                if ax_rolle(el) == "AXTextArea" {
                    if let Some(f) = ax_editierbar(el) { aus.push(f); }
                    return;
                }
                let Some(k) = ax_element_attr(el, "AXChildren") else { return };
                for i in 0..CFArrayGetCount(k) { felder(CFArrayGetValueAtIndex(k, i) as *mut c_void, tiefe + 1, aus); }
                CFRelease(k as *const c_void);
            }
            let fenster = ax_fenster_element(app, wid).and_then(|w| {
                let r = ax_rahmen(w);
                CFRelease(w as *const c_void);
                r
            });
            let kette = kette_im_fenster(pid, wid, x, y);
            for &e in kette.iter().take(10) {
                if ax_rolle_erlaubt(e) { break; }
                let Some(r) = ax_rahmen(e) else { continue };
                if fenster.is_some_and(|f| r.g.w >= f.g.w - 4.0 && r.g.h >= f.g.h - 40.0) { break; }
                let mut gefunden = vec![];
                felder(e, 0, &mut gefunden);
                if gefunden.len() == 1 {
                    ziel = gefunden.pop();
                    crate::virtual_workspace::trace("[TIPPEN] route=editor_hidden_input");
                    break;
                }
                let viele = !gefunden.is_empty();
                kette_freigeben(gefunden);
                if viele { break; }
            }
            kette_freigeben(kette);
        }
        let Some(feld) = ziel else {
            let k = kette_im_fenster(pid, wid, x, y);
            // VS Code's integrated terminal draws into a canvas (AXImage);
            // its input textarea is not under the point. With this window's
            // Companion bridge, the element counts as the terminal target
            // ONLY if it lies inside VS Code's integrated-terminal DOM.
            if let Some(&e0) = k.first() {
                if crate::virtual_workspace::backend() == crate::virtual_workspace::Backend::RealSpace
                    && crate::vscode_bruecke::ist_vscode(pid)
                    && crate::vscode_bruecke::ist_terminal_kontext(&Feld(CFRetain(e0 as *const c_void) as usize).pipe_kontext())
                    && crate::vscode_bruecke::fuer_fenster(wid).is_some()
                {
                    let e = CFRetain(e0 as *const c_void) as *mut c_void;
                    crate::virtual_workspace::trace(&format!(
                        "[TIPPEN] field role={} wid={wid} route=vscode_bridge kind=terminal_canvas", ax_rolle(e)));
                    kette_freigeben(k);
                    CFRelease(app as *const c_void);
                    return Some((e, None));
                }
            }
            crate::virtual_workspace::trace(&format!("[TIPPEN] no_field wid={wid} point={x:.0},{y:.0} chain={:?} window_found={}",
                k.iter().take(8).map(|e| ax_rolle(*e)).collect::<Vec<_>>(), ax_fenster_element(app, wid).map(|w| { CFRelease(w as *const c_void); true }).unwrap_or(false)));
            kette_freigeben(k);
            CFRelease(app as *const c_void); return None;
        };
        // REAL_SPACE: AXFocused on WEB content (WebKit/Chromium) of a hidden
        // window activates it and macOS switches to the Noki Space
        // (measured). Native fields (Safari address bar, Terminal, TextEdit)
        // are fine. Web fields fail closed - never a Space switch.
        // REAL_SPACE: a text field INSIDE a collection (Finder icon/list
        // name label, table cell) is an in-place RENAME/edit field - a real
        // single click there only selects the item. Measured 2026-09-27: a
        // click on a Finder file name started a typing session on it.
        if crate::virtual_workspace::backend() == crate::virtual_workspace::Backend::RealSpace && in_sammlung(feld) {
            crate::virtual_workspace::trace(&format!("[TIPPEN] refused wid={wid} reason=item_label_in_collection"));
            CFRelease(feld as *const c_void);
            CFRelease(app as *const c_void);
            return None;
        }
        // Already THE focused field of exactly this window (e.g. VS Code's
        // editor input of its key window): nothing has to be focused, so no
        // Raise/Main/AXFocused is sent at all - no Space switch is possible.
        // Every key is still re-verified against this field (fern_tippen).
        if crate::virtual_workspace::backend() == crate::virtual_workspace::Backend::RealSpace
            && in_web_inhalt(feld) && ax_fokus_ist(app, wid, feld)
        {
            crate::virtual_workspace::trace(&format!(
                "[TIPPEN] field role={} already_focused_in_window wid={wid} focus_set=false verified=true", ax_rolle(feld)));
            CFRelease(app as *const c_void);
            return Some((feld, None));
        }
        // VS Code with its Noki Companion bridge: the keys go through the VS
        // Code API (exact window, its active editor/terminal), so the AX
        // focus of a hidden Electron window - which cannot be verified - is
        // not needed. The field only tells editor from terminal.
        if crate::virtual_workspace::backend() == crate::virtual_workspace::Backend::RealSpace
            && in_web_inhalt(feld) && crate::vscode_bruecke::ist_vscode(pid)
            && crate::vscode_bruecke::fuer_fenster(wid).is_some()
        {
            crate::virtual_workspace::trace(&format!(
                "[TIPPEN] field role={} wid={wid} route=vscode_bridge ax_focus_needed=false", ax_rolle(feld)));
            CFRelease(app as *const c_void);
            return Some((feld, None));
        }
        if crate::virtual_workspace::backend() == crate::virtual_workspace::Backend::RealSpace
            && in_web_inhalt(feld) && super::chromium_ereignis_app(pid)
        {
            // Electron does not consume a process-addressed mouse packet
            // while its window lives on another Space.  Focus only the
            // exact editable AX node first: no AXRaise, no AXMain and no app
            // activation.  This keeps the physical Space untouched while
            // making the following exact-window packet place the caret.
            let focus_attr = cfstr("AXFocused");
            let setzbar = ax_setzbar(feld, "AXFocused");
            let gesetzt = setzbar
                && AXUIElementSetAttributeValue(feld, focus_attr, kCFBooleanTrue) == 0;
            CFRelease(focus_attr);
            let mut bewiesen = false;
            if gesetzt {
                for _ in 0..12 {
                    if ax_fokus_ist(app, wid, feld) { bewiesen = true; break; }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
            crate::virtual_workspace::trace(&format!(
                "[TIPPEN] electron_exact_focus wid={wid} settable={setzbar} set={gesetzt} verified={bewiesen}"));
            CFRelease(app as *const c_void);
            if bewiesen { return Some((feld, None)); }
            CFRelease(feld as *const c_void);
            return None;
        }
        if crate::virtual_workspace::backend() == crate::virtual_workspace::Backend::RealSpace && in_web_inhalt(feld) {
            crate::virtual_workspace::trace(&format!("[TIPPEN] refused wid={wid} reason=web_content_focus_would_switch_space"));
            crate::vorschau::hinweis("Web-Inhalt ist im Hintergrund pausiert – auf dem Noki Schreibtisch vollständig verfügbar.");
            CFRelease(feld as *const c_void);
            CFRelease(app as *const c_void);
            return None;
        }
        // Gemessen: AXFocused auf ein Feld wirkt NICHT, solange ein anderes
        // Chrome-Fenster (z. B. eines des Nutzers) Chromes Hauptfenster ist.
        // Wie ein echter Klick: Zielfenster nach vorn und zum Hauptfenster
        // (nur innerhalb von Chrome - das vordere Programm bleibt vorn).
        // Das bisherige Fokusfenster wird gemerkt und am Ende zurueckgegeben.
        let vorher = ax_element_attr(app, "AXFocusedWindow");
        let number_attr = cfstr("AXWindowNumber");
        let vorher = vorher.and_then(|w| {
            if ax_fenster_nummer(w, number_attr) == Some(wid) { CFRelease(w as *const c_void); None }
            else { Some(w) }
        });
        CFRelease(number_attr);
        if let Some(fenster) = ax_fenster_element(app, wid) {
            let raise = cfstr("AXRaise");
            let _ = AXUIElementPerformAction(fenster, raise);
            CFRelease(raise);
            let main_attr = cfstr("AXMain");
            let _ = AXUIElementSetAttributeValue(fenster, main_attr, kCFBooleanTrue);
            CFRelease(main_attr);
            CFRelease(fenster as *const c_void);
        }
        let focus_attr = cfstr("AXFocused");
        let gesetzt = AXUIElementSetAttributeValue(feld, focus_attr, kCFBooleanTrue) == 0;
        CFRelease(focus_attr);
        let mut bewiesen = false;
        if gesetzt {
            for _ in 0..20 {
                if ax_fokus_ist(app, wid, feld) { bewiesen = true; break; }
                std::thread::sleep(std::time::Duration::from_millis(15));
            }
        }
        // Measured (macOS 26.6): for an INACTIVE app, AXMain/AXFocused never
        // move its key window (also not via SkyLight key-window records) -
        // keys then belong to its OTHER window. Fail closed, say why.
        let anderes = if bewiesen { None } else {
            let number_attr = cfstr("AXWindowNumber");
            let n = ax_element_attr(app, "AXFocusedWindow").and_then(|w| {
                let n = ax_fenster_nummer(w, number_attr); CFRelease(w as *const c_void); n
            }).filter(|n| *n != wid);
            CFRelease(number_attr);
            n
        };
        CFRelease(app as *const c_void);
        crate::virtual_workspace::trace(&format!(
            "[TIPPEN] field role={} focus_set={gesetzt} verified={bewiesen}", ax_rolle(feld)
        ));
        if bewiesen { return Some((feld, vorher)); }
        {
            if let Some(n) = anderes {
                crate::virtual_workspace::trace(&format!("[CAPABILITY] typing wid={wid} class=REMOTE_PARTIAL reason=other_window_is_key key_window={n}"));
                crate::vorschau::hinweis("Tippen geht hier nur ins zuletzt aktive Fenster dieser App – auf dem Noki Schreibtisch jedes.");
            }
        }
        CFRelease(feld as *const c_void);
        if let Some(v) = vorher {
            // REAL_SPACE: the clicked window stays in front like after a real
            // click. Handing AXMain back to the app's previous window raised
            // THAT window over the clicked one on the real Noki Space while
            // the Miniatur showed the clicked one on top (measured 2026-09-27:
            // Terminal 830482 clicked, 821504 back on top).
            if crate::virtual_workspace::backend() != crate::virtual_workspace::Backend::RealSpace {
                let main_attr = cfstr("AXMain");
                let _ = AXUIElementSetAttributeValue(v, main_attr, kCFBooleanTrue);
                CFRelease(main_attr);
            }
            CFRelease(v as *const c_void);
        }
        None
    }

    /// Liegt das Element in einer Sammlung (Liste/Tabelle/Gliederung)?
    unsafe fn in_sammlung(el: *mut c_void) -> bool {
        let mut e = CFRetain(el as *const c_void) as *mut c_void;
        let mut ja = false;
        for _ in 0..12 {
            let Some(p) = ax_element_attr(e, "AXParent") else { break };
            CFRelease(e as *const c_void);
            e = p;
            let r = ax_rolle(e);
            if matches!(r.as_str(), "AXList" | "AXOutline" | "AXTable" | "AXBrowser" | "AXGrid") { ja = true; break; }
            if r == "AXWindow" { break; }
        }
        CFRelease(e as *const c_void);
        ja
    }

    /// Liegt das Element in Web-Inhalt (ein AXWebArea-Vorfahr)?
    unsafe fn in_web_inhalt(el: *mut c_void) -> bool {
        let mut cur = CFRetain(el as *const c_void) as *mut c_void;
        let mut web = false;
        for _ in 0..40 {
            if ax_rolle(cur) == "AXWebArea" { web = true; break; }
            let Some(p) = ax_element_attr(cur, "AXParent") else { break };
            CFRelease(cur as *const c_void);
            cur = p;
        }
        CFRelease(cur as *const c_void);
        web
    }

    /// Meldet Chrome GENAU dieses Feld (oder etwas in seiner editierbaren
    /// Wurzel) als Fokus, und liegt der in genau diesem Fenster?
    unsafe fn ax_fokus_ist(app: *mut c_void, wid: i64, feld: *mut c_void) -> bool {
        let number_attr = cfstr("AXWindowNumber");
        let feld_fokus = ax_element_attr(feld, "AXFocused").map(|v| {
            let ja = CFEqual(v as *const c_void, kCFBooleanTrue);
            CFRelease(v as *const c_void);
            ja
        }).unwrap_or(false);
        // Inactive Electron can omit AXFocusedWindow. Accept its field only
        // when `_AXUIElementGetWindow` proves this exact CGWindow.
        let feld_fenster = ax_fenster_nummer(feld, number_attr) == Some(wid);
        let fenster_ok = ax_element_attr(app, "AXFocusedWindow").map(|win| {
            let ok = ax_fenster_nummer(win, number_attr) == Some(wid);
            CFRelease(win as *const c_void);
            ok
        }).unwrap_or(false);
        if !fenster_ok && !(feld_fokus && feld_fenster) { CFRelease(number_attr); return false; }
        let Some(fokus) = ax_element_attr(app, "AXFocusedUIElement") else {
            CFRelease(number_attr); return feld_fokus && feld_fenster;
        };
        let mut gleich = CFEqual(fokus as *const c_void, feld as *const c_void);
        if !gleich {
            if let Some(wurzel) = ax_element_attr(fokus, "AXEditableAncestor") {
                gleich = CFEqual(wurzel as *const c_void, feld as *const c_void);
                CFRelease(wurzel as *const c_void);
            }
        }
        // Chromium meldet oft ein INNERES Element als Fokus (z. B. das
        // contenteditable in einer AXComboBox): Vorfahr/Nachfahr gilt auch.
        if !gleich {
            for (von, zu) in [(fokus, feld), (feld, fokus)] {
                let mut cur = CFRetain(von as *const c_void) as *mut c_void;
                for _ in 0..6 {
                    let Some(p) = ax_element_attr(cur, "AXParent") else { break };
                    CFRelease(cur as *const c_void);
                    cur = p;
                    if CFEqual(cur as *const c_void, zu as *const c_void) { gleich = true; break; }
                }
                CFRelease(cur as *const c_void);
                if gleich { break; }
            }
        }
        let im_fenster = !gleich || ax_fenster_nummer(fokus, number_attr).is_none_or(|n| n == wid);
        CFRelease(fokus as *const c_void);
        CFRelease(number_attr);
        (gleich && im_fenster) || (feld_fokus && feld_fenster)
    }

    /// Fenster auf Noki Schreibtisch nach vorn - NUR innerhalb der App
    /// (AXRaise). Gemessen: das vorderste Programm des Nutzers bleibt vorn,
    /// kein Schreibtischwechsel, kein Zeiger.
    unsafe fn ax_fenster_heben(pid: i32, wid: i64) -> bool {
        if !AXIsProcessTrusted() || pid <= 0 || wid <= 0 { return false; }
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() { return false; }
        let mut ok = false;
        if let Some(w) = ax_fenster_element(app, wid) {
            let raise = cfstr("AXRaise");
            ok = AXUIElementPerformAction(w, raise) == 0;
            CFRelease(raise);
            CFRelease(w as *const c_void);
        }
        CFRelease(app as *const c_void);
        ok
    }


    // -----------------------------------------------------------------
    //  Auswahl per Ziehen, Kontextmenue (virtueller Noki Schreibtisch)
    // -----------------------------------------------------------------

    fn ax_bereich(i: isize, l: isize) -> *const c_void {
        #[repr(C)] struct CfRange { loc: isize, len: isize }
        let r = CfRange { loc: i, len: l };
        unsafe { AXValueCreate(4, &r as *const CfRange as *const c_void) } // kAXValueCFRangeType
    }

    unsafe fn ax_zeichen_rahmen(el: *mut c_void, i: isize) -> Option<Rechteck> {
        let attr = cfstr("AXBoundsForRange");
        let bereich = ax_bereich(i, 1);
        let mut v: *const c_void = std::ptr::null();
        let ok = AXUIElementCopyParameterizedAttributeValue(el, attr, bereich, &mut v) == 0 && !v.is_null();
        CFRelease(attr); CFRelease(bereich);
        if !ok { return None; }
        let mut r = Rechteck::default();
        let g = AXValueGetValue(v, 3, &mut r as *mut Rechteck as *mut c_void);
        CFRelease(v);
        (g && r.g.w > 0.0 && r.g.h > 0.0).then_some(r)
    }

    unsafe fn ax_text_laenge(el: *mut c_void) -> isize {
        // Chrome meldet an Textblaettern AXNumberOfCharacters = 0 (gemessen);
        // massgeblich ist dann die Laenge des Werts.
        let n = ax_zahl_attr(el, "AXNumberOfCharacters").unwrap_or(0) as isize;
        n.max(ax_text_attr_lang(el, "AXValue").encode_utf16().count() as isize)
    }

    unsafe fn ax_text_attr_lang(el: *mut c_void, name: &str) -> String {
        let Some(v) = ax_element_attr(el, name) else { return String::new() };
        let n = CFStringGetLength(v as *const c_void);
        let mut buf = vec![0u16; n.max(0) as usize];
        CFStringGetCharacters(v as *const c_void, CfRangeRoh { loc: 0, len: n }, buf.as_mut_ptr());
        CFRelease(v as *const c_void);
        String::from_utf16_lossy(&buf)
    }

    /// Zeichen unter dem Punkt: binaere Suche ueber AXBoundsForRange (Chrome
    /// liefert fuer AXTextMarkerForPosition gemessen immer "kein Wert").
    unsafe fn ax_index_am_punkt(el: *mut c_void, x: f64, y: f64) -> Option<isize> {
        let n = ax_text_laenge(el);
        if n <= 0 { return Some(0); }
        ax_zeichen_rahmen(el, 0)?;
        let (mut lo, mut hi) = (0isize, n);
        while lo < hi {
            let mid = (lo + hi) / 2;
            let Some(b) = ax_zeichen_rahmen(el, mid) else { return Some(lo) };
            let vor = y < b.o.y || (y <= b.o.y + b.g.h && x < b.o.x + b.g.w / 2.0);
            if vor { hi = mid } else { lo = mid + 1 }
        }
        Some(lo)
    }

    /// Das Ziel einer Auswahl am Punkt (retained) und ob es editierbar ist.
    /// Eingabefeld/Textbereich: das Feld selbst. contenteditable: das
    /// Textblatt darin (dort setzt Chrome die Auswahl nachweislich). Sonst ein
    /// gewoehnliches Textblatt der Seite.
    unsafe fn ax_auswahl_ziel(pid: i32, wid: i64, x: f64, y: f64) -> Option<(*mut c_void, bool)> {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() { return None; }
        let el = ax_element_im_fenster(app, wid, x, y);
        CFRelease(app as *const c_void);
        let el = el?;
        let rolle = ax_rolle(el);
        let r = match rolle.as_str() {
            "AXTextField" | "AXTextArea" | "AXComboBox" | "AXSearchField" => {
                if ax_zeichen_rahmen(el, 0).is_some() { Some((el, true)) }
                // contenteditable-Wurzel: keine Zeichenrahmen - das Textblatt
                // unter dem Punkt (bzw. das erste) ist das Ziel.
                else if let Some(blatt) = ax_textblatt_unter(el, x, y) {
                    CFRelease(el as *const c_void);
                    Some((blatt, true))
                } else if ax_text_laenge(el) == 0 { Some((el, true)) }
                else { None }
            }
            "AXStaticText" => {
                if let Some(p) = ax_element_attr(el, "AXParent") {
                    // Blatt eines Eingabefelds: das Feld ist das Ziel.
                    let pr = ax_rolle(p);
                    if matches!(pr.as_str(), "AXTextField" | "AXTextArea") && ax_zeichen_rahmen(p, 0).is_some() {
                        CFRelease(el as *const c_void);
                        return Some((p, true));
                    }
                    CFRelease(p as *const c_void);
                }
                let editierbar = ax_has_attr_value(el, "AXEditableAncestor");
                Some((el, editierbar))
            }
            _ => None,
        };
        if r.is_none() { CFRelease(el as *const c_void); }
        r
    }

    /// Erstes Textblatt (AXStaticText) unter `el`, bevorzugt das am Punkt. Retained.
    unsafe fn ax_textblatt_unter(el: *mut c_void, x: f64, y: f64) -> Option<*mut c_void> {
        let mut erstes: Option<*mut c_void> = None;
        let mut stapel = vec![(CFRetain(el as *const c_void) as *mut c_void, 0usize)];
        let mut treffer = None;
        while let Some((e, tiefe)) = stapel.pop() {
            if treffer.is_none() && ax_rolle(e) == "AXStaticText" {
                let drin = ax_rahmen(e).is_some_and(|r| x >= r.o.x && y >= r.o.y && x <= r.o.x + r.g.w && y <= r.o.y + r.g.h);
                if drin { treffer = Some(CFRetain(e as *const c_void) as *mut c_void); }
                else if erstes.is_none() { erstes = Some(CFRetain(e as *const c_void) as *mut c_void); }
            }
            if tiefe < 6 {
                if let Some(k) = ax_element_attr(e, "AXChildren") {
                    for i in (0..CFArrayGetCount(k)).rev() {
                        let c = CFArrayGetValueAtIndex(k, i);
                        stapel.push((CFRetain(c) as *mut c_void, tiefe + 1));
                    }
                    CFRelease(k as *const c_void);
                }
            }
            CFRelease(e as *const c_void);
        }
        match (treffer, erstes) {
            (Some(t), e) => { if let Some(e) = e { CFRelease(e as *const c_void); } Some(t) }
            (None, e) => e,
        }
    }

    unsafe fn ax_auswahl_setzen(el: *mut c_void, von: isize, bis: isize) -> bool {
        let attr = cfstr("AXSelectedTextRange");
        let b = ax_bereich(von.min(bis), (bis - von).abs());
        let ok = AXUIElementSetAttributeValue(el, attr, b) == 0;
        CFRelease(attr); CFRelease(b);
        ok
    }

    pub struct AuswahlZiel {
        pub pid: i32,
        pub wid: i64,
        feld: Feld,
        pub editierbar: bool,
        pub start: isize,
    }

    /// Beginn einer Zieh-Auswahl. Editierbar: das Feld wird fokussiert und
    /// nachgewiesen (Rueckgabe zusaetzlich fuer das Fern-Tippen).
    pub fn auswahl_beginnen(pid: i32, wid: i64, x: f64, y: f64)
        -> Option<(AuswahlZiel, Option<(Feld, Option<Feld>)>)>
    {
        unsafe {
            if !AXIsProcessTrusted() { return None; }
            let (el, editierbar) = ax_auswahl_ziel(pid, wid, x, y)?;
            let start = ax_index_am_punkt(el, x, y).unwrap_or(0);
            let tippen = if editierbar { ax_eingabe_fokussieren(pid, wid, x, y)
                .map(|(f, v)| (Feld(f as usize), v.map(|v| Feld(v as usize)))) } else { None };
            if editierbar && tippen.is_none() {
                CFRelease(el as *const c_void);
                return None; // Fokus nicht nachweisbar: fail closed
            }
            if editierbar {
                // contenteditable: Chrome setzt die Auswahl nur, wenn auch das
                // Textblatt selbst den Fokus bekommen hat (gemessen).
                if ax_rolle(el) == "AXStaticText" {
                    let a = cfstr("AXFocused");
                    let _ = AXUIElementSetAttributeValue(el, a, kCFBooleanTrue);
                    CFRelease(a);
                }
                let _ = ax_auswahl_setzen(el, start, start);
            }
            Some((AuswahlZiel { pid, wid, feld: Feld(el as usize), editierbar, start }, tippen))
        }
    }

    /// Auswahl bis zum Punkt. Editierbar: Chromes echte Auswahl. Sonst:
    /// (Text, Zeilenrechtecke) fuer Nokis Markierung.
    pub fn auswahl_bis(z: &AuswahlZiel, x: f64, y: f64) -> Option<(String, Vec<[f64; 4]>)> {
        unsafe {
            let el = z.feld.0 as *mut c_void;
            let bis = ax_index_am_punkt(el, x, y)?;
            if z.editierbar {
                ax_auswahl_setzen(el, z.start, bis);
                return Some((String::new(), vec![]));
            }
            let (von, bis) = (z.start.min(bis), z.start.max(bis));
            let wert: Vec<u16> = ax_text_attr_lang(el, "AXValue").encode_utf16().collect();
            let text = if bis > von && (bis as usize) <= wert.len() {
                String::from_utf16_lossy(&wert[von as usize..bis as usize])
            } else if bis > von {
                let attr = cfstr("AXStringForRange");
                let b = ax_bereich(von, bis - von);
                let mut v: *const c_void = std::ptr::null();
                let ok = AXUIElementCopyParameterizedAttributeValue(el, attr, b, &mut v) == 0 && !v.is_null();
                CFRelease(attr); CFRelease(b);
                let t = if ok { let n = CFStringGetLength(v); let mut buf = vec![0u16; n.max(0) as usize];
                    CFStringGetCharacters(v, CfRangeRoh { loc: 0, len: n }, buf.as_mut_ptr()); CFRelease(v);
                    String::from_utf16_lossy(&buf) } else { String::new() };
                t
            } else { String::new() };
            // Zeilenweise Rechtecke aus den echten Zeichenrahmen.
            let mut zeilen: Vec<[f64; 4]> = vec![];
            for i in von..bis.min(von + 400) {
                let Some(r) = ax_zeichen_rahmen(el, i) else { continue };
                match zeilen.last_mut() {
                    Some(l) if (l[1] - r.o.y).abs() < r.g.h / 2.0 => {
                        let rechts = (l[0] + l[2]).max(r.o.x + r.g.w);
                        l[0] = l[0].min(r.o.x); l[2] = rechts - l[0]; l[3] = l[3].max(r.g.h);
                    }
                    _ => zeilen.push([r.o.x, r.o.y, r.g.w, r.g.h]),
                }
            }
            Some((text, zeilen))
        }
    }

    /// Kette der Elemente unter dem Punkt im Zielfenster, tiefstes zuerst
    /// (retained). Gemessen: Chromes Treffertest am Punkt liefert fuer ein
    /// nicht-Hauptfenster oft nur die AXWebArea - der Baum des Fensters nicht.
    unsafe fn ax_kette_am_punkt(el: *mut c_void, x: f64, y: f64, tiefe: usize, besucht: &mut usize) -> Vec<*mut c_void> {
        ax_kette_gewichtet(el, x, y, tiefe, besucht).0
    }

    /// Leeres Blatt: eine Gruppe ohne Kinder und ohne Namen (oder eine
    /// AXEmptyGroup) - eine durchsichtige Ablage-/Abdeckschicht.
    unsafe fn leeres_blatt(el: *mut c_void) -> bool {
        if ax_text_attr(el, "AXSubrole") == "AXEmptyGroup" { return true; }
        // Tabellenspalten ueberdecken alle Zeilen (gemessen: Mail, 323516 pt
        // hoch) - die Zeile unter dem Punkt ist der echte Inhalt.
        if ax_rolle(el) == "AXColumn" { return true; }
        if ax_rolle(el) != "AXGroup" { return false; }
        let kinder = ax_element_attr(el, "AXChildren").map(|k| {
            let n = CFArrayGetCount(k);
            CFRelease(k as *const c_void);
            n
        }).unwrap_or(0);
        kinder == 0 && ax_text_attr(el, "AXDescription").is_empty() && ax_text_attr(el, "AXTitle").is_empty()
    }

    /// Kette vom tiefsten Element am Punkt bis hierher. Zweiter Wert: endet
    /// sie an echtem Inhalt? Gemessen lagen in Spotify, VS Code und Claude
    /// fensterweite, leere Schichten ueber dem Inhalt (in VS Code sechs
    /// Gruppen tief) - die echte Kette zu Editor/Liste gewinnt immer; eine
    /// leere nur, wenn es nichts anderes gibt.
    unsafe fn ax_kette_gewichtet(el: *mut c_void, x: f64, y: f64, tiefe: usize, besucht: &mut usize) -> (Vec<*mut c_void>, bool) {
        if tiefe > 40 || *besucht > 20_000 || zeit_um() { return (vec![], false); }
        *besucht += 1;
        let drin = ax_rahmen(el).map(|r| x >= r.o.x && y >= r.o.y && x <= r.o.x + r.g.w && y <= r.o.y + r.g.h);
        if drin == Some(false) { return (vec![], false); }
        let mut ersatz: Vec<*mut c_void> = vec![];
        if let Some(k) = ax_element_attr(el, "AXChildren") {
            for i in (0..CFArrayGetCount(k)).rev() {
                let c = CFArrayGetValueAtIndex(k, i) as *mut c_void;
                let (mut kette, echt) = ax_kette_gewichtet(c, x, y, tiefe + 1, besucht);
                if kette.is_empty() { continue; }
                // Alles unter einer AXEmptyGroup ist Abdeckschicht.
                let echt = echt && ax_text_attr(c, "AXSubrole") != "AXEmptyGroup";
                if echt {
                    kette_freigeben(std::mem::take(&mut ersatz));
                    CFRelease(k as *const c_void);
                    kette.push(CFRetain(el as *const c_void) as *mut c_void);
                    return (kette, true);
                }
                if ersatz.is_empty() { ersatz = kette; } else { kette_freigeben(kette); }
            }
            CFRelease(k as *const c_void);
        }
        if !ersatz.is_empty() {
            ersatz.push(CFRetain(el as *const c_void) as *mut c_void);
            return (ersatz, false);
        }
        if drin == Some(true) { (vec![CFRetain(el as *const c_void) as *mut c_void], !leeres_blatt(el)) } else { (vec![], false) }
    }

    // -----------------------------------------------------------------
    //  Allgemeine Bedienung (jedes Programm mit Accessibility-Baum)
    // -----------------------------------------------------------------

    unsafe fn ax_zahl(el: *mut c_void, name: &str) -> Option<f64> {
        let v = ax_element_attr(el, name)?;
        let mut d = 0f64;
        let ok = CFNumberGetValue(v, 13, &mut d as *mut f64 as *mut c_void); // kCFNumberDoubleType
        CFRelease(v as *const c_void);
        ok.then_some(d)
    }

    unsafe fn ax_setzbar(el: *mut c_void, name: &str) -> bool {
        let a = cfstr(name);
        let mut ja: u8 = 0;
        let ok = AXUIElementIsAttributeSettable(el, a, &mut ja) == 0 && ja != 0;
        CFRelease(a);
        ok
    }

    unsafe fn ax_dom_klassen(el: *mut c_void) -> String {
        let Some(arr) = ax_element_attr(el, "AXDOMClassList") else { return String::new() };
        let mut s = String::new();
        for i in 0..CFArrayGetCount(arr as *const c_void) {
            s.push_str(&cf_string(CFArrayGetValueAtIndex(arr as *const c_void, i)));
            s.push(' ');
        }
        CFRelease(arr as *const c_void);
        s.to_lowercase()
    }

    /// Value/media controls. A wheel/trackpad scroll over them is PAGE
    /// scroll - never keys (YouTube maps arrows to volume/seek) and never a
    /// value change. Measured root cause: over a YouTube video the generic
    /// route focused the focusable player `AXGroup` (html5-video-player) and
    /// posted arrow keys (`route=focus_arrows`).
    unsafe fn medien_element(el: *mut c_void) -> bool {
        if matches!(ax_rolle(el).as_str(), "AXSlider" | "AXProgressIndicator" | "AXIncrementor"
            | "AXScrollBar" | "AXValueIndicator" | "AXLevelIndicator") { return true; }
        if ax_text_attr(el, "AXSubrole") == "AXApplicationGroup" { return true; }
        let k = ax_dom_klassen(el);
        ["video", "player", "ytp-", "slider", "progress", "volume", "seek", "scrubber", "playback", "media"]
            .iter().any(|w| k.contains(w))
    }

    unsafe fn kette_hat_medien(kette: &[*mut c_void]) -> bool {
        kette.iter().take_while(|e| !matches!(ax_rolle(**e).as_str(), "AXWebArea" | "AXWindow"))
            .any(|e| medien_element(*e))
    }

    /// Is the pointer over a media/value control (see `medien_element`)?
    pub fn medien_unter_zeiger(pid: i32, wid: i64, x: f64, y: f64) -> bool {
        unsafe {
            let k = kette_im_fenster(pid, wid, x, y);
            let m = kette_hat_medien(&k);
            kette_freigeben(k);
            if m { return true; }
            // Second opinion: the system hit test. Measured on the YouTube
            // PWA: the window-tree walk missed the video at its centre while
            // AXUIElementCopyElementAtPosition returned video -> player.
            // Either one saying "media" wins (false positives only cost the
            // smoother key route, never correctness).
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return false; }
            let mut el: *mut c_void = std::ptr::null_mut();
            let mut treffer = false;
            if AXUIElementCopyElementAtPosition(app, x as f32, y as f32, &mut el) == 0 && !el.is_null() {
                let mut e = CFRetain(el as *const c_void) as *mut c_void;
                for _ in 0..30 {
                    let r = ax_rolle(e);
                    if matches!(r.as_str(), "AXWebArea" | "AXWindow" | "AXApplication" | "") { break; }
                    if medien_element(e) { treffer = true; break; }
                    let p = ax_element_attr(e, "AXParent");
                    CFRelease(e as *const c_void);
                    match p { Some(p) => e = p, None => { e = std::ptr::null_mut(); break; } }
                }
                if !e.is_null() { CFRelease(e as *const c_void); }
                CFRelease(el as *const c_void);
            }
            CFRelease(app as *const c_void);
            treffer
        }
    }

    /// Key-free page scroll for web content: bounded AXScrollToVisible
    /// steps on the exact window's document (no focus change, no keys).
    /// `schritte` < 0 = down.
    pub fn seite_ohne_tasten(pid: i32, wid: i64, schritte: i32) -> bool {
        unsafe {
            if !AXIsProcessTrusted() || schritte == 0 { return false; }
            // A no-op at the document edge used to repeat the complete AX
            // tree walk (and its animation timeout) for every queued packet.
            // Remember the edge only for this live gesture. Reversing,
            // changing targets, or pausing immediately invalidates it.
            static GRENZE: std::sync::Mutex<Option<(i32, i64, i32, std::time::Instant)>> =
                std::sync::Mutex::new(None);
            let richtung = schritte.signum();
            if let Ok(mut g) = GRENZE.lock() {
                if g.as_ref().is_some_and(|(p, w, d, t)| *p == pid && *w == wid
                    && *d == richtung && t.elapsed() < std::time::Duration::from_millis(450))
                {
                    *g = Some((pid, wid, richtung, std::time::Instant::now()));
                    crate::virtual_workspace::trace(&format!(
                        "[SCROLL] wid={wid} boundary_cached direction={richtung} route=keyfree_document"
                    ));
                    return true;
                }
                // Opposite direction, another target, or a new gesture.
                *g = None;
            }
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return false; }
            ax_manuell_an(app, pid);
            let web = ax_fenster_element(app, wid).and_then(|w| {
                let web = ax_web_bereich(w, 0);
                CFRelease(w as *const c_void);
                web
            });
            CFRelease(app as *const c_void);
            let Some(web) = web else { return false };
            let mut getan = 0;
            for _ in 0..schritte.unsigned_abs().min(4) {
                if !container_einzelschritt(web, schritte) { break; }
                getan += 1;
            }
            CFRelease(web as *const c_void);
            if let Ok(mut g) = GRENZE.lock() {
                *g = (getan == 0).then(|| (pid, wid, richtung, std::time::Instant::now()));
            }
            eprintln!("[FERNAX] keyfree page scroll wid={wid} steps={getan}/{}", schritte.abs());
            // At a proven edge the packet was handled successfully: do not
            // make the caller enter a slower fallback route.
            true
        }
    }

    /// Kette der Elemente unter dem Punkt im EXAKTEN Fenster, tiefstes zuerst
    /// (jedes gehalten). Leer, wenn es das Fenster nicht gibt.
    unsafe fn kette_im_fenster(pid: i32, wid: i64, x: f64, y: f64) -> Vec<*mut c_void> {
        if !AXIsProcessTrusted() || pid <= 0 { return vec![]; }
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() { return vec![]; }
        ax_manuell_an(app, pid);
        let mut kette = vec![];
        if let Some(w) = ax_fenster_element(app, wid) {
            let mut besucht = 0usize;
            kette = ax_kette_am_punkt(w, x, y, 0, &mut besucht);
            CFRelease(w as *const c_void);
        }
        CFRelease(app as *const c_void);
        kette
    }

    fn kette_freigeben(k: Vec<*mut c_void>) {
        for e in k { unsafe { CFRelease(e as *const c_void) } }
    }

    /// Ein Schieberegler, den eine Zieh-Geste fuehrt: Wert ~ Zeigerlage auf
    /// der sichtbaren Spur.
    /// weg: 0 = AXValue, 1 = AXIncrement/Decrement, 2 = Fokus + Pfeiltasten.
    pub struct Schieber { feld: Feld, links: f64, breite: f64, min: f64, max: f64,
                          weg: std::sync::atomic::AtomicU8, pid: i32, wid: i64,
                          tastenschritt: std::sync::Mutex<f64> }

    unsafe fn schieber_darunter(el: *mut c_void, tiefe: usize) -> Option<*mut c_void> {
        if tiefe > 4 { return None; }
        if ax_rolle(el) == "AXSlider" && ax_setzbar(el, "AXValue")
            && ax_zahl(el, "AXMaxValue").zip(ax_zahl(el, "AXMinValue")).is_some_and(|(a, b)| a > b) {
            return Some(CFRetain(el as *const c_void) as *mut c_void);
        }
        let k = ax_element_attr(el, "AXChildren")?;
        let mut r = None;
        for i in 0..CFArrayGetCount(k) {
            if let Some(s) = schieber_darunter(CFArrayGetValueAtIndex(k, i) as *mut c_void, tiefe + 1) { r = Some(s); break; }
        }
        CFRelease(k as *const c_void);
        r
    }

    /// Schieberegler am Punkt: ein AXSlider selbst, oder die kleinste flache,
    /// breite Flaeche (die sichtbare Spur), die einen setzbaren AXSlider
    /// enthaelt (Chromium: der Regler selbst ist oft 1x1 gross).
    pub fn schieber_beginnen(pid: i32, wid: i64, x: f64, y: f64) -> Option<Schieber> {
        unsafe {
            let kette = kette_im_fenster(pid, wid, x, y);
            let mut ergebnis = None;
            for &el in kette.iter().take(8) {
                let Some(r) = ax_rahmen(el) else { continue };
                if r.g.h > 64.0 { break; }
                let s = if ax_rolle(el) == "AXSlider" && r.g.w >= 20.0 { Some(CFRetain(el as *const c_void) as *mut c_void) }
                        else if r.g.w >= 30.0 { schieber_darunter(el, 0) } else { None };
                if let Some(s) = s {
                    let (min, max) = (ax_zahl(s, "AXMinValue").unwrap_or(0.0), ax_zahl(s, "AXMaxValue").unwrap_or(1.0));
                    ergebnis = Some(Schieber { feld: Feld(s as usize), links: r.o.x, breite: r.g.w.max(1.0), min, max,
                                               weg: std::sync::atomic::AtomicU8::new(0), pid, wid,
                                               tastenschritt: std::sync::Mutex::new(0.0) });
                    break;
                }
            }
            kette_freigeben(kette);
            if let Some(s) = ergebnis.as_ref() {
                crate::virtual_workspace::trace(&format!(
                    "[INTERAKTION] slider wid={wid} track={:.0}+{:.0} range={}..{} route=AXValue", s.links, s.breite, s.min, s.max));
            }
            ergebnis
        }
    }

    /// Wert ~ Zeigerlage, ueber die erste Stufe, die das Programm WIRKLICH
    /// umsetzt: AXValue -> AXIncrement/AXDecrement -> Fokus + Pfeiltasten an
    /// genau diesen Prozess. Gemessen (Spotify/Chromium): AXValue wird
    /// angenommen und ignoriert; Lautstaerke folgt den Schritten, der
    /// Fortschritt nur den Pfeiltasten (5 s je Druck).
    pub fn schieber_setzen(s: &Schieber, x: f64) -> bool {
        use std::sync::atomic::Ordering;
        unsafe {
            let el = s.feld.0 as *mut c_void;
            let anteil = ((x - s.links) / s.breite).clamp(0.0, 1.0);
            let wert = s.min + anteil * (s.max - s.min);
            let spanne = (s.max - s.min).abs().max(1e-9);
            let nah = |v: f64| (v - wert).abs() <= spanne * 0.02;
            if s.weg.load(Ordering::Relaxed) == 0 {
                let vorher = ax_zahl(el, "AXValue");
                let n = CFNumberCreate(std::ptr::null(), 13, &wert as *const f64 as *const c_void);
                let a = cfstr("AXValue");
                let _ = !n.is_null() && AXUIElementSetAttributeValue(el, a, n) == 0;
                CFRelease(a);
                if !n.is_null() { CFRelease(n); }
                std::thread::sleep(std::time::Duration::from_millis(15));
                let nach = ax_zahl(el, "AXValue");
                if nach.is_some_and(nah) { return true; }
                if nach.zip(vorher).is_some_and(|(n, v)| (n - v).abs() < spanne * 0.005) { s.weg.store(1, Ordering::Relaxed); }
            }
            if s.weg.load(Ordering::Relaxed) == 1 {
                let mut v = ax_zahl(el, "AXValue").unwrap_or(wert);
                let mut bewegt = false;
                for _ in 0..60 {
                    if nah(v) { break; }
                    let hoch = wert > v;
                    if !ax_aktion(el, if hoch { "AXIncrement" } else { "AXDecrement" }) { break; }
                    let neu = ax_zahl(el, "AXValue").unwrap_or(v);
                    if (neu - v).abs() < spanne * 0.001 { break; }
                    bewegt = true;
                    if (hoch && neu >= wert) || (!hoch && neu <= wert) { break; }
                    v = neu;
                }
                if bewegt { return true; }
                s.weg.store(2, Ordering::Relaxed);
            }
            // Stufe 3: Fokus auf den Regler (AX weist Fenster + Element nach),
            // dann Pfeiltasten nur an diesen Prozess.
            let app = AXUIElementCreateApplication(s.pid);
            if app.is_null() { return false; }
            let mut fokus = ax_fokus_ist(app, s.wid, el);
            if !fokus {
                let a = cfstr("AXFocused");
                let _ = AXUIElementSetAttributeValue(el, a, kCFBooleanTrue);
                CFRelease(a);
                for _ in 0..5 {
                    std::thread::sleep(std::time::Duration::from_millis(12));
                    if ax_fokus_ist(app, s.wid, el) { fokus = true; break; }
                }
            }
            CFRelease(app as *const c_void);
            if !fokus { return false; }
            let taste = |hoch: bool| {
                let code: u16 = if hoch { 124 } else { 123 };
                for runter in [true, false] {
                    let ev = CGEventCreateKeyboardEvent(std::ptr::null(), code, runter);
                    if ev.is_null() { continue; }
                    CGEventSetFlags(ev, 0);
                    CGEventSetIntegerValueField(ev, 42, crate::fern_tippen::MARKE);
                    CGEventPostToPid(s.pid, ev);
                    CFRelease(ev as *const c_void);
                }
            };
            let Some(v0) = ax_zahl(el, "AXValue") else { return false };
            if nah(v0) { return true; }
            let hoch = wert > v0;
            let mut schritt = s.tastenschritt.lock().map(|g| *g).unwrap_or(0.0);
            if schritt <= 0.0 {
                taste(hoch);
                std::thread::sleep(std::time::Duration::from_millis(60));
                let v1 = ax_zahl(el, "AXValue").unwrap_or(v0);
                schritt = (v1 - v0).abs();
                if schritt < spanne * 0.0005 { return false; }
                if let Ok(mut g) = s.tastenschritt.lock() { *g = schritt; }
            }
            let v = ax_zahl(el, "AXValue").unwrap_or(v0);
            let n = (((wert - v).abs() / schritt).round() as usize).min(80);
            for _ in 0..n { taste(wert > v); }
            true
        }
    }

    /// Letzter Roll-Container je Fenster: eine Geste bleibt bei ihrem Ziel.
    static ROLL_SITZUNG: std::sync::Mutex<Option<(i64, usize, std::time::Instant)>> = std::sync::Mutex::new(None);
    /// Proven no-op edge of the current generic scroll gesture.
    /// (wid, vertical direction, last packet). It intentionally expires with
    /// the same 450 ms gesture boundary as ROLL_SITZUNG.
    static ROLL_GRENZE: std::sync::Mutex<Option<(i64, i32, std::time::Instant)>> =
        std::sync::Mutex::new(None);

    /// Steht der Rollbalken schon am Rand in Richtung der Geste? Dann ist
    /// der Bereich "fertig" - kein Rueckfall auf Pfeiltasten (die bewegten
    /// gemessen in TextEdit die Schreibmarke).
    unsafe fn rollbalken_am_rand(bereich: *mut c_void, dx: i32, dy: i32) -> bool {
        let senkrecht = dy.abs() >= dx.abs();
        let Some(balken) = ax_element_attr(bereich, if senkrecht { "AXVerticalScrollBar" } else { "AXHorizontalScrollBar" }) else { return false };
        let d = if senkrecht { dy } else { dx };
        let min = ax_zahl(balken, "AXMinValue").unwrap_or(0.0);
        let max = ax_zahl(balken, "AXMaxValue").unwrap_or(1.0);
        let toleranz = ((max - min).abs() * 0.002).max(1e-6);
        let rand = ax_setzbar(balken, "AXValue") && ax_zahl(balken, "AXValue")
            .is_some_and(|v| (d > 0 && v <= min + toleranz)
                || (d < 0 && v >= max - toleranz));
        CFRelease(balken as *const c_void);
        rand
    }

    /// Rollbereich ohne setzbaren Rollbalken, aber mit den Standardaktionen
    /// AXScroll{Up,Down}ByPage (gemessen: Finder-Symbolansicht). Eine Seite
    /// erst nach genug Radweg - sonst blaettert jede Radraste eine Seite.
    static SEITEN_WEG: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
    unsafe fn seiten_schritt(bereich: *mut c_void, dy: i32) -> bool {
        if dy == 0 { return false; }
        let aktion = if dy < 0 { "AXScrollDownByPage" } else { "AXScrollUpByPage" };
        if !ax_aktion_vorhanden(bereich, aktion) { return false; }
        let alt = SEITEN_WEG.load(std::sync::atomic::Ordering::SeqCst);
        let weg = if alt.signum() == dy.signum() { alt + dy } else { dy };
        if weg.abs() >= 18 {
            SEITEN_WEG.store(0, std::sync::atomic::Ordering::SeqCst);
            let _ = ax_aktion(bereich, aktion);
        } else {
            SEITEN_WEG.store(weg, std::sync::atomic::Ordering::SeqCst);
        }
        true
    }

    static BALKEN_REST: std::sync::Mutex<f64> = std::sync::Mutex::new(0.0);

    unsafe fn rollbalken_schritt(bereich: *mut c_void, dx: i32, dy: i32) -> bool {
        let senkrecht = dy.abs() >= dx.abs();
        let Some(balken) = ax_element_attr(bereich, if senkrecht { "AXVerticalScrollBar" } else { "AXHorizontalScrollBar" }) else { return false };
        let mut ok = false;
        if let (Some(v), true) = (ax_zahl(balken, "AXValue"), ax_setzbar(balken, "AXValue")) {
            // Sichtbarer Anteil aus dem Schieber-Knopf (AXValueIndicator).
            let spur = ax_rahmen(balken).map(|r| if senkrecht { r.g.h } else { r.g.w }).unwrap_or(1.0).max(1.0);
            let mut anteil = 0.25;
            if let Some(k) = ax_element_attr(balken, "AXChildren") {
                for i in 0..CFArrayGetCount(k) {
                    let c = CFArrayGetValueAtIndex(k, i) as *mut c_void;
                    if ax_rolle(c) == "AXValueIndicator" {
                        if let Some(r) = ax_rahmen(c) { anteil = ((if senkrecht { r.g.h } else { r.g.w }) / spur).clamp(0.01, 0.99); }
                    }
                }
                CFRelease(k as *const c_void);
            }
            let sicht = ax_rahmen(bereich).map(|r| if senkrecht { r.g.h } else { r.g.w }).unwrap_or(400.0);
            let bereich_px = (sicht / anteil - sicht).max(1.0);
            let d = if senkrecht { dy } else { dx } as f64;
            // 1:1 with the finger (helper units are 4 px). Tiny deltas are
            // accumulated up to one text line (TextEdit redraws only then) -
            // before, every packet jumped >= 40 px ("too sensitive", stepwise).
            let px = {
                let mut g = BALKEN_REST.lock().unwrap_or_else(|e| e.into_inner());
                if g.signum() != 0.0 && g.signum() != d.signum() { *g = 0.0; }
                *g += d * 4.0;
                if g.abs() < 14.0 { return true; }
                let p = *g; *g = 0.0; p
            };
            let neu = (v - px / bereich_px).clamp(0.0, 1.0);
            if (neu - v).abs() > 1e-6 {
                let n = CFNumberCreate(std::ptr::null(), 13, &neu as *const f64 as *const c_void);
                let a = cfstr("AXValue");
                ok = AXUIElementSetAttributeValue(balken, a, n) == 0;
                CFRelease(a); CFRelease(n);
                // Nur ein Schritt, der den Wert wirklich veraendert hat, zaehlt.
                ok = ok && ax_zahl(balken, "AXValue").is_some_and(|w| (w - v).abs() > 1e-6);
            }
        }
        CFRelease(balken as *const c_void);
        ok
    }

    /// Erstes (Dokumentreihenfolge; nach oben: letztes) sichtbar HOHES Blatt
    /// jenseits der Containerkante oder ueber sie hinausragend. Gemessen:
    /// Chromium drueckt Knoten ausserhalb des Sichtbereichs auf Hoehe 0 an
    /// die Kante - auf ihnen bewirkt AXScrollToVisible nichts; grosse Bloecke
    /// dagegen springen 400-670 px. Ein Textblatt ergibt kleine Schritte.
    unsafe fn blatt_jenseits(el: *mut c_void, oben: f64, unten: f64, runter: bool, tiefe: usize, besucht: &mut usize, ueberspringen: &mut usize) -> Option<*mut c_void> {
        if tiefe > 40 || *besucht > 6_000 || zeit_um() { return None; }
        *besucht += 1;
        let r = ax_rahmen(el);
        if let Some(r) = r {
            // Chromium clamps OFF-SCREEN nodes to height 0 on the viewport
            // edge - exactly those are the ones whose AXScrollToVisible moves
            // the page (measured YouTube: visible edge nodes are no-ops, the
            // clamped node at 1822 h=0 moved to 1449). Only zero-height nodes
            // away from the scrolling edge are pruned.
            let am_rand = if runter { r.o.y >= unten - 1.0 } else { r.o.y <= oben + 1.0 };
            if tiefe > 0 && r.g.h < 1.0 && !am_rand { return None; }
            if runter && r.o.y + r.g.h < unten - 1.0 { return None; }
            if !runter && r.o.y > oben + 1.0 { return None; }
        }
        let mut gefunden = None;
        if let Some(k) = ax_element_attr(el, "AXChildren") {
            let n = CFArrayGetCount(k);
            for j in 0..n {
                let i = if runter { j } else { n - 1 - j };
                if let Some(x) = blatt_jenseits(CFArrayGetValueAtIndex(k, i) as *mut c_void, oben, unten, runter, tiefe + 1, besucht, ueberspringen) {
                    gefunden = Some(x); break;
                }
            }
            CFRelease(k as *const c_void);
        }
        if gefunden.is_some() || tiefe == 0 { return gefunden; }
        let r = r?;
        let jenseits = if runter { r.o.y >= unten - 1.0 } else { r.o.y + r.g.h <= oben + 1.0 };
        let ragt = if runter { r.o.y < unten - 1.0 && r.o.y + r.g.h > unten + 1.0 }
                   else { r.o.y + r.g.h > oben + 1.0 && r.o.y < oben - 1.0 };
        let passt = (jenseits || ragt) && r.g.h <= 160.0 && ax_aktion_vorhanden(el, "AXScrollToVisible")
            // a visible node that merely touches the edge never scrolls
            && !(r.g.h >= 1.0 && (if runter { r.o.y + r.g.h <= unten + 1.0 } else { r.o.y >= oben - 1.0 }));
        // A candidate that did not move the content (sticky header, inner
        // scroller) is skipped on the next attempt.
        if passt && *ueberspringen > 0 { *ueberspringen -= 1; return None; }
        passt.then(|| CFRetain(el as *const c_void) as *mut c_void)
    }

    /// Off-screen node clamped (height 0) onto the scrolling edge, first in
    /// document order (last for upwards). Bounded full walk, no pruning.
    unsafe fn randknoten(el: *mut c_void, oben: f64, unten: f64, runter: bool, tiefe: usize, n: &mut usize, skip: &mut usize) -> Option<*mut c_void> {
        if zeit_um() || tiefe > 45 || *n > 8_000 { return None; }
        *n += 1;
        if tiefe > 0 {
            if let Some(r) = ax_rahmen(el) {
                let am_rand = if runter { r.o.y >= unten - 1.0 } else { r.o.y + r.g.h <= oben + 1.0 };
                // Clamped (h=0) nodes at the edge, or real nodes lying fully
                // beyond it (measured Figma: content above the viewport keeps
                // its real height - only "down" found candidates before).
                if am_rand && r.g.h <= 160.0 && ax_aktion_vorhanden(el, "AXScrollToVisible") {
                    if *skip > 0 { *skip -= 1; } else { return Some(CFRetain(el as *const c_void) as *mut c_void); }
                }
            }
        }
        let k = ax_element_attr(el, "AXChildren")?;
        let cnt = CFArrayGetCount(k);
        let mut out = None;
        for j in 0..cnt {
            let i = if runter { j } else { cnt - 1 - j };
            if let Some(x) = randknoten(CFArrayGetValueAtIndex(k, i) as *mut c_void, oben, unten, runter, tiefe + 1, n, skip) { out = Some(x); break; }
        }
        CFRelease(k as *const c_void);
        out
    }

    /// Upwards: the off-screen node (clamped onto / lying above the top
    /// edge) that DIRECTLY precedes visible content in document order. Walking
    /// backwards from the end picked empty full-width overlay groups clamped
    /// at the top (measured Figma "Aktuelles": 60 such nodes, AXScrollToVisible
    /// a no-op) - scroll up never moved. Transitions are collected in
    /// document order; the last one is the scroller's content, earlier ones
    /// are tried via `skip`.
    unsafe fn uebergang_oben(el: *mut c_void, oben: f64, unten: f64, skip: usize) -> Option<*mut c_void> {
        unsafe fn gehe(el: *mut c_void, oben: f64, unten: f64, tiefe: usize, n: &mut usize,
                       offen: &mut Option<*mut c_void>, fertig: &mut Vec<*mut c_void>) {
            if zeit_um() || tiefe > 45 || *n > 8_000 { return; }
            *n += 1;
            let r = ax_rahmen(el);
            let kinder = ax_element_attr(el, "AXChildren");
            let blatt = kinder.is_none_or(|k| CFArrayGetCount(k as *const c_void) == 0);
            if let (Some(r), true) = (r, tiefe > 0) {
                let oberhalb = r.o.y + r.g.h <= oben + 1.0 && r.o.y >= oben - 4000.0;
                if oberhalb && r.g.h <= 160.0 && r.g.w >= 1.0 && ax_aktion_vorhanden(el, "AXScrollToVisible") {
                    if let Some(alt) = offen.take() { CFRelease(alt as *const c_void); }
                    *offen = Some(CFRetain(el as *const c_void) as *mut c_void);
                } else if blatt && r.g.h >= 1.0 && r.o.y > oben + 1.0 && r.o.y < unten {
                    if let Some(k) = offen.take() { fertig.push(k); }
                }
            }
            if let Some(k) = kinder {
                for i in 0..CFArrayGetCount(k as *const c_void) {
                    gehe(CFArrayGetValueAtIndex(k as *const c_void, i) as *mut c_void, oben, unten, tiefe + 1, n, offen, fertig);
                }
                CFRelease(k as *const c_void);
            }
        }
        let mut n = 0usize;
        let mut offen = None;
        let mut fertig = Vec::new();
        gehe(el, oben, unten, 0, &mut n, &mut offen, &mut fertig);
        if let Some(o) = offen { CFRelease(o as *const c_void); }
        let wahl = fertig.len().checked_sub(1 + skip).map(|i| fertig[i]);
        for &k in &fertig { if Some(k) != wahl { CFRelease(k as *const c_void); } }
        wahl
    }

    unsafe fn container_schritt(el: *mut c_void, dy: i32) -> bool {
        let schritte = (dy.unsigned_abs() as usize).div_ceil(8).clamp(1, 3);
        let mut getan = 0;
        for _ in 0..schritte {
            if zeit_um() { break; }
            if !container_einzelschritt(el, dy) { break; }
            getan += 1;
        }
        getan > 0
    }

    unsafe fn container_einzelschritt(el: *mut c_void, dy: i32) -> bool {
        (0..3).any(|k| !zeit_um() && container_versuch(el, dy, k))
    }

    unsafe fn container_versuch(el: *mut c_void, dy: i32, ueberspringen: usize) -> bool {
        if zeit_um() { return false; }
        let Some(r) = ax_rahmen(el) else { return false };
        // AX exposes document/list backing groups whose frame is the entire
        // content (Mail measured 323,782 px high). They are not viewports;
        // walking thousands of descendants at the boundary can block the
        // scroll worker for tens of seconds. Real Figma/web viewports stay
        // window-sized and continue through the generic resolver.
        if r.g.w < 40.0 || r.g.h < 40.0 || r.g.w > 5_000.0 || r.g.h > 5_000.0 {
            return false;
        }
        // Nur echte Sichtfenster: nicht groesser als das Fenster selbst ist
        // durch die Rahmenpruefung unten ohnehin gegeben.
        let runter = dy < 0;
        let mut besucht = 0usize;
        let mut skip = ueberspringen;
        let mut gefunden = if runter || zeit_um() { None }
            else { uebergang_oben(el, r.o.y, r.o.y + r.g.h, ueberspringen) };
        if gefunden.is_none() && !zeit_um() {
            gefunden = blatt_jenseits(el, r.o.y, r.o.y + r.g.h, runter, 0, &mut besucht, &mut skip);
        }
        if gefunden.is_none() && !zeit_um() {
            let mut n = 0usize;
            let mut skip2 = ueberspringen;
            gefunden = randknoten(el, r.o.y, r.o.y + r.g.h, runter, 0, &mut n, &mut skip2);
        }
        let Some(ziel) = gefunden else {
            eprintln!("[FERNAX] keyfree candidate none viewport={:.0},{:.0} h={:.0} visited={besucht} skip={ueberspringen}", r.o.x, r.o.y, r.g.h);
            return false
        };
        if zeit_um() { CFRelease(ziel as *const c_void); return false; }
        let vorher = ax_rahmen(ziel).map(|f| f.o.y);
        eprintln!("[FERNAX] keyfree candidate role={} y={:?} h={:?} viewport_y={:.0}..{:.0} skip={ueberspringen}",
            ax_rolle(ziel), vorher, ax_rahmen(ziel).map(|f| f.g.h), r.o.y, r.o.y + r.g.h);
        let a = cfstr("AXScrollToVisible");
        let mut ok = AXUIElementPerformAction(ziel, a) == 0;
        CFRelease(a);
        // Nur ein Schritt, der den Inhalt WIRKLICH bewegt, zaehlt (gemessen:
        // ein Inhaltsblock, dessen Kinder ohnehin ueber ihn hinausragen,
        // meldete "Erfolg", ohne dass sich etwas bewegte).
        if ok {
            // Chromium animates AXScrollToVisible (measured ~340 px in
            // <400 ms). Judging after 72 ms called real steps "failed" and
            // fired further candidates - several stacked jumps.
            let mut bewegt = false;
            for _ in 0..12 {
                if zeit_um() { break; }
                std::thread::sleep(std::time::Duration::from_millis(10));
                let nach = ax_rahmen(ziel).map(|f| f.o.y);
                if nach.zip(vorher).is_some_and(|(n, v)| (n - v).abs() > 0.5) { bewegt = true; break; }
            }
            ok = bewegt;
        }
        CFRelease(ziel as *const c_void);
        ok
    }

    /// Chromium/Electron-Rollbereich per Tastatur: der Container selbst
    /// bekommt den AX-Fokus (ohne das Programm zu aktivieren), AX weist nach,
    /// dass Fenster UND Element stimmen, dann Pfeiltasten nur an diesen
    /// Prozess (markiert, ohne Modifikatoren). Gemessen: AXScrollToVisible
    /// wirkt in inneren Rollbereichen nicht (Chromium klemmt unsichtbare
    /// Knoten mit Hoehe ~0 an die FENSTER-Kante); Pfeile rollen genau den
    /// fokussierten Bereich - wie der bewaehrte Browser-Weg.
    unsafe fn fokus_rollbar(el: *mut c_void) -> bool {
        !matches!(ax_rolle(el).as_str(),
            "AXTextField" | "AXTextArea" | "AXComboBox" | "AXSlider" | "AXList" | "AXTable" | "AXOutline"
            | "AXRadioGroup" | "AXTabGroup" | "AXButton" | "AXPopUpButton" | "AXMenuButton" | "AXCheckBox"
            | "AXRadioButton" | "AXIncrementor" | "AXRow" | "AXCell" | "AXMenu" | "AXMenuItem" | "AXWindow")
            && ax_setzbar(el, "AXFocused")
            && ax_rahmen(el).is_some_and(|r| r.g.h >= 60.0 && r.g.w >= 60.0)
    }

    unsafe fn fokus_pfeil_schritt(pid: i32, wid: i64, el: *mut c_void, dx: i32, dy: i32) -> bool {
        // Must PROVE movement: a sample descendant's frame changes. Before,
        // "focus set + keys sent" counted as scrolled even when the focused
        // element was not the scroller (Figma up: nothing moved, fallbacks
        // never ran).
        let probe = ax_element_attr(el, "AXChildren").and_then(|k| {
            let n = CFArrayGetCount(k as *const c_void);
            let c = (n > 0).then(|| CFRetain(CFArrayGetValueAtIndex(k as *const c_void, n / 2)) as *mut c_void);
            CFRelease(k as *const c_void);
            c
        });
        let vorher = probe.and_then(|p| ax_rahmen(p)).map(|r| (r.o.x, r.o.y));
        let gesendet = fokus_pfeil_senden(pid, wid, el, dx, dy);
        let mut bewegt = false;
        if gesendet {
            for _ in 0..12 {
                std::thread::sleep(std::time::Duration::from_millis(15));
                let nach = probe.and_then(|p| ax_rahmen(p)).map(|r| (r.o.x, r.o.y));
                if vorher.is_none() || nach != vorher { bewegt = true; break; }
            }
        }
        if let Some(p) = probe { CFRelease(p as *const c_void); }
        gesendet && bewegt
    }

    unsafe fn fokus_pfeil_senden(pid: i32, wid: i64, el: *mut c_void, dx: i32, dy: i32) -> bool {
        // REAL_SPACE: focusing an element of a hidden window can activate its
        // app and make macOS follow it to the Noki Space (measured, Safari
        // web content). Scroll never focuses there.
        if crate::virtual_workspace::backend() == crate::virtual_workspace::Backend::RealSpace { return false; }
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() { return false; }
        let mut ok = ax_fokus_ist(app, wid, el);
        if !ok {
            let a = cfstr("AXFocused");
            let _ = AXUIElementSetAttributeValue(el, a, kCFBooleanTrue);
            CFRelease(a);
            for _ in 0..5 {
                std::thread::sleep(std::time::Duration::from_millis(12));
                if ax_fokus_ist(app, wid, el) { ok = true; break; }
            }
        }
        CFRelease(app as *const c_void);
        if !ok { return false; }
        let senkrecht = dy.abs() >= dx.abs();
        let d = if senkrecht { dy } else { dx };
        let code: u16 = match (senkrecht, d < 0) { (true, true) => 125, (true, false) => 126, (false, true) => 124, (false, false) => 123 };
        let schritte = (d.unsigned_abs() as usize).div_ceil(4).clamp(1, 4);
        for _ in 0..schritte {
            for runter in [true, false] {
                let ev = CGEventCreateKeyboardEvent(std::ptr::null(), code, runter);
                if ev.is_null() { continue; }
                CGEventSetFlags(ev, 0);
                CGEventSetIntegerValueField(ev, 42, crate::fern_tippen::MARKE);
                CGEventPostToPid(pid, ev);
                CFRelease(ev as *const c_void);
            }
        }
        true
    }

    /// Rollen am Punkt - fuer jedes Programm: naechster Roll-Container ueber
    /// dem Element unter dem Zeiger im EXAKTEN Fenster. AXScrollArea mit
    /// Rollbalken -> dessen Wert; sonst (Chromium/Electron) der naechste
    /// Knoten jenseits der Containerkante wird sichtbar gemacht (begrenzter
    /// Schritt). Innerhalb einer Geste bleibt der Container derselbe.
    pub fn rollen_am_punkt(pid: i32, wid: i64, x: f64, y: f64, dx: i32, dy: i32) -> bool {
        unsafe {
            if (dx == 0 && dy == 0) || zeit_um() { return false; }
            let richtung = if dy != 0 { dy.signum() } else { dx.signum() };
            if let Ok(mut g) = ROLL_GRENZE.lock() {
                if g.as_ref().is_some_and(|(w, d, t)| *w == wid && *d == richtung
                    && t.elapsed() < std::time::Duration::from_millis(450))
                {
                    crate::virtual_workspace::trace(&format!(
                        "[INTERAKTION] scroll_boundary_cached wid={wid} direction={richtung}"
                    ));
                    return false;
                }
                *g = None;
            }
            // Laufende Geste: derselbe Container.
            let alt = ROLL_SITZUNG.lock().ok().and_then(|g| *g);
            if let Some((w, el, t)) = alt {
                if !zeit_um() && w == wid && t.elapsed() < std::time::Duration::from_millis(450) {
                    let el = el as *mut c_void;
                    let ok = rollbalken_schritt(el, dx, dy) || rollbalken_am_rand(el, dx, dy)
                        || (ax_rolle(el) == "AXScrollArea" && seiten_schritt(el, dy))
                        || (fokus_rollbar(el) && !medien_element(el) && fokus_pfeil_schritt(pid, wid, el, dx, dy))
                        || (dy != 0 && container_schritt(el, dy));
                    if ok {
                        if let Ok(mut g) = ROLL_GRENZE.lock() { *g = None; }
                        if let Ok(mut g) = ROLL_SITZUNG.lock() { *g = Some((w, el as usize, std::time::Instant::now())); }
                        return true;
                    }
                }
            }
            if zeit_um() { return false; }
            let kette = kette_im_fenster(pid, wid, x, y);
            let medien = kette_hat_medien(&kette);
            let mut gewaehlt: Option<*mut c_void> = None;
            let mut weg = "none";
            // Pass 1: the nearest real scroll area moves by its scroll bar
            // value - exact, both directions, no focus change, no keys.
            // Measured Mail: focus+arrows on a row group MOVED THE SELECTION
            // (another mail opened) instead of scrolling.
            for &el in kette.iter() {
                if zeit_um() { break; }
                let rolle = ax_rolle(el);
                if rolle == "AXWindow" { break; }
                if rolle == "AXScrollArea" && rollbalken_schritt(el, dx, dy) { gewaehlt = Some(el); weg = "scrollbar"; break; }
            }
            // Native lists/tables: arrow keys mean SELECT - never use them.
            let liste = kette.iter().any(|e| matches!(ax_rolle(*e).as_str(), "AXTable" | "AXOutline" | "AXList" | "AXBrowser"))
                && !kette.iter().any(|e| ax_rolle(*e) == "AXWebArea");
            for &el in kette.iter() {
                if gewaehlt.is_some() || zeit_um() { break; }
                let rolle = ax_rolle(el);
                if rolle == "AXWindow" { break; }
                if rolle == "AXScrollArea" && rollbalken_schritt(el, dx, dy) { gewaehlt = Some(el); weg = "scrollbar"; break; }
                if rolle == "AXScrollArea" && rollbalken_am_rand(el, dx, dy) { gewaehlt = Some(el); weg = "scrollbar_edge"; break; }
                if rolle == "AXScrollArea" && seiten_schritt(el, dy) { gewaehlt = Some(el); weg = "scroll_by_page"; break; }
                // Catalyst/SwiftUI content (measured GoodNotes) advertises
                // AXScroll{Up,Down}ByPage on groups/tiles instead of an
                // AXScrollArea. That IS the app's scroll semantic; when only
                // the other direction is offered we are at the edge - never
                // fall through to arrow keys, which move the focused item.
                if dy != 0 && rolle != "AXWebArea"
                    && (ax_aktion_vorhanden(el, "AXScrollDownByPage") || ax_aktion_vorhanden(el, "AXScrollUpByPage")) {
                    if seiten_schritt(el, dy) { gewaehlt = Some(el); weg = "scroll_by_page"; } else { weg = "page_edge"; }
                    break;
                }
                // Never keys over media/value controls (scroll means scroll).
                if !medien && !liste && fokus_rollbar(el) && !medien_element(el) && fokus_pfeil_schritt(pid, wid, el, dx, dy) { gewaehlt = Some(el); weg = "focus_arrows"; break; }
                // Native list/table rows must use their scroll area's
                // scrollbar/page actions. Descendant AXScrollToVisible is
                // both semantically risky (it can change selection) and was
                // the unbounded bottom-of-list fallback in Mail.
                if !liste && dy != 0 && container_schritt(el, dy) {
                    gewaehlt = Some(el); weg = "scroll_to_visible"; break;
                }
            }
            let ok = gewaehlt.is_some();
            if let Ok(mut g) = ROLL_GRENZE.lock() {
                *g = (!ok).then(|| (wid, richtung, std::time::Instant::now()));
            }
            if let Ok(mut g) = ROLL_SITZUNG.lock() {
                if let Some((_, alt_el, _)) = g.take() { CFRelease(alt_el as *const c_void); }
                if let Some(el) = gewaehlt { *g = Some((wid, CFRetain(el as *const c_void) as usize, std::time::Instant::now())); }
            }
            let rolle = kette.first().map(|e| ax_rolle(*e)).unwrap_or_default();
            kette_freigeben(kette);
            crate::virtual_workspace::trace(&format!(
                "[INTERAKTION] scroll_{} wid={wid} point={x:.0},{y:.0} dy={dy} role={rolle} route={weg} media_under_pointer={medien}",
                if ok { "session" } else { "miss" }));
            ok
        }
    }

    /// Rechtsklick: das echte Kontextmenue des Programms am Ziel oeffnen.
    /// Erfolg erst, wenn wirklich ein Menue-Fenster erscheint. Nie ueber die
    /// Webseite hinaus (sonst kaeme Chromes Fenster-/Tableisten-Menue).
    pub fn kontextmenue(pid: i32, wid: i64, x: f64, y: f64, menue_da: &dyn Fn() -> bool) -> bool {
        unsafe {
            if !AXIsProcessTrusted() { return false; }
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return false; }
            let Some(fenster) = ax_fenster_element(app, wid) else { CFRelease(app as *const c_void); return false };
            let mut besucht = 0usize;
            let kette = ax_kette_am_punkt(fenster, x, y, 0, &mut besucht);
            let mut ok = false;
            let mut seite_erreicht = false;
            for &e in &kette {
                if ok || seite_erreicht { break; }
                let rolle = ax_rolle(e);
                // Innerhalb einer Webseite endet die Suche an der Seite selbst.
                if rolle == "AXWebArea" { seite_erreicht = true; }
                if !ax_aktion_vorhanden(e, "AXShowMenu") { continue; }
                let a = cfstr("AXShowMenu");
                let _ = AXUIElementPerformAction(e, a);
                CFRelease(a);
                for _ in 0..8 {
                    if menue_da() { ok = true; break; }
                    std::thread::sleep(std::time::Duration::from_millis(30));
                }
            }
            let web = kette.iter().any(|&e| ax_rolle(e) == "AXWebArea");
            for e in kette { CFRelease(e as *const c_void); }
            // Ausserhalb von Webinhalt (z. B. Tableiste) gilt das erste
            // Element mit AXShowMenu - dort IST das Fenstermenue das echte.
            let _ = web;
            CFRelease(fenster as *const c_void);
            CFRelease(app as *const c_void);
            ok
        }
    }

    /// Klick in ein offenes Menue: nur ein echter Menueeintrag wird gedrueckt.
    pub fn menue_klick(pid: i32, x: f64, y: f64) -> bool {
        unsafe {
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return false; }
            let mut el: *mut c_void = std::ptr::null_mut();
            let ok = AXUIElementCopyElementAtPosition(app, x as f32, y as f32, &mut el) == 0 && !el.is_null();
            CFRelease(app as *const c_void);
            if !ok { return false; }
            let getan = ax_rolle(el) == "AXMenuItem" && ax_aktion(el, "AXPress");
            CFRelease(el as *const c_void);
            getan
        }
    }

    /// Dismiss a popover window (Catalyst/AppKit transient) the way an
    /// outside click does: AXCancel on its first element that offers it.
    pub fn popover_abbrechen(pid: i32, wid: i64) -> bool {
        unsafe {
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return false; }
            NICHT_FENSTER_ERLAUBT.with(|f| f.set(true));
            let gefunden = ax_fenster_element(app, wid);
            NICHT_FENSTER_ERLAUBT.with(|f| f.set(false));
            let Some(fenster) = gefunden else {
                crate::virtual_workspace::trace(&format!("[CLICK] popover_dismiss popover={wid} ax_window=missing"));
                CFRelease(app as *const c_void); return false
            };
            let mut ebene: Vec<*mut c_void> = vec![CFRetain(fenster as *const c_void) as *mut c_void];
            let mut ok = false;
            for _ in 0..5 {
                if ok || ebene.is_empty() || zeit_um() { break; }
                let mut naechste = vec![];
                for &el in &ebene {
                    if !ok && ax_aktion_vorhanden(el, "AXCancel") { ok = ax_aktion(el, "AXCancel"); }
                    if ok { continue; }
                    if let Some(k) = ax_element_attr(el, "AXChildren") {
                        for i in 0..CFArrayGetCount(k).min(24) {
                            let c = CFArrayGetValueAtIndex(k, i);
                            if !c.is_null() { naechste.push(CFRetain(c) as *mut c_void); }
                        }
                        CFRelease(k as *const c_void);
                    }
                }
                for el in std::mem::replace(&mut ebene, naechste) { CFRelease(el as *const c_void); }
            }
            for el in ebene { CFRelease(el as *const c_void); }
            CFRelease(fenster as *const c_void);
            CFRelease(app as *const c_void);
            ok
        }
    }

    /// Offenes Menue an dieser Stelle schliessen (AXCancel), ohne Wirkung.
    pub fn menue_abbrechen(pid: i32, x: f64, y: f64) -> bool {
        unsafe {
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return false; }
            let mut el: *mut c_void = std::ptr::null_mut();
            let ok = AXUIElementCopyElementAtPosition(app, x as f32, y as f32, &mut el) == 0 && !el.is_null();
            CFRelease(app as *const c_void);
            if !ok { return false; }
            let rolle = ax_rolle(el);
            let getan = matches!(rolle.as_str(), "AXMenuItem" | "AXMenu") && ax_aktion(el, "AXCancel");
            CFRelease(el as *const c_void);
            getan
        }
    }

    /// Fensterscharfer semantischer Klick (virtueller Schreibtisch).
    pub fn ax_klick_fenster(pid: i32, wid: i64, x: f64, y: f64, klicks: i64) -> bool {
        unsafe {
            if klicks >= 2 { return ax_oeffnen_fenster(pid, wid, x, y); }
            match ax_klick_aufloesen(pid, wid, x, y) {
                Some(ok) => ok,
                // REAL_SPACE + web content: the generic press/select
                // fallbacks can move focus inside a hidden page, and web
                // focus activates the app -> macOS switches Space (measured:
                // YouTube video-area clicks, guard activation). Not provably
                // safe -> removed: no real control = safe no-op + hint.
                None if crate::virtual_workspace::backend() == crate::virtual_workspace::Backend::RealSpace
                    && web_fenster(pid, wid, x, y) => {
                    crate::virtual_workspace::trace(&format!(
                        "[CAPABILITY] click wid={wid} web=true fallback=blocked reason=no_exact_control"));
                    crate::vorschau::hinweis("Hier gibt es in der Miniatur nichts sicher Anklickbares.");
                    false
                }
                None => ax_druecken_fenster(pid, wid, x, y) || ax_auswaehlen_fenster(pid, wid, x, y),
            }
        }
    }

    /// Is the point in WEB content (browser, web app, AXWebArea in chain)?
    unsafe fn web_fenster(pid: i32, wid: i64, x: f64, y: f64) -> bool {
        if super::ist_browser(pid) || super::ist_pwa(pid) { return true; }
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() { return true; }
        let kette = kette_am_punkt(app, wid, x, y);
        CFRelease(app as *const c_void);
        let web = kette.iter().any(|e| ax_rolle(*e) == "AXWebArea");
        for e in kette { CFRelease(e as *const c_void); }
        web
    }

    /// Generic actionable-descendant resolver (one truth for visual point,
    /// hit target and action target). Some(result) when it decided,
    /// None to fall back to the older routes.
    ///   * chain = geometric DFS in the EXACT window (sheets, focused
    ///     dialogs included), deepest element first;
    ///   * control roles are pressed with AXPress even when the app does not
    ///     LIST it (measured Safari "Neuer Tab": no actions listed, AXPress
    ///     works);
    ///   * rows/cells/items are selected; text inputs are left to typing.
    unsafe fn ax_klick_aufloesen(pid: i32, wid: i64, x: f64, y: f64) -> Option<bool> {
        if !AXIsProcessTrusted() || pid <= 0 { return None; }
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() { return None; }
        ax_manuell_an(app, pid);
        let kette = kette_am_punkt(app, wid, x, y);
        CFRelease(app as *const c_void);
        const DRUECKEN: [&str; 12] = ["AXButton", "AXLink", "AXCheckBox", "AXRadioButton", "AXMenuItem",
            "AXMenuButton", "AXPopUpButton", "AXDisclosureTriangle", "AXIncrementor", "AXTab",
            "AXMenuBarItem", "AXToolbarButton"];
        let rollen: Vec<String> = kette.iter().take(8).map(|e| ax_rolle(*e)).collect();
        let mut ergebnis: Option<bool> = None;
        let mut gewaehlt = String::new();
        for &e in &kette {
            let r = ax_rolle(e);
            if matches!(r.as_str(), "AXTextField" | "AXTextArea" | "AXSearchField" | "AXComboBox") {
                gewaehlt = format!("{r}:typing_path"); break;
            }
            if DRUECKEN.contains(&r.as_str()) {
                // Apps that do not LIST AXPress still perform it but return
                // an error code (Safari "Neuer Tab": -25205, tab added). The
                // attempt counts as delivered - a second fallback click
                // would double the action.
                let ok = druecken_mit_nachweis(e, "generic") || !ax_aktion_vorhanden(e, "AXPress");
                gewaehlt = format!("{r}/{}:{}:AXPress", ax_text_attr(e, "AXSubrole"),
                    { let t = ax_text_attr(e, "AXDescription"); if t.is_empty() { ax_text_attr(e, "AXTitle") } else { t } });
                ergebnis = Some(ok);
                break;
            }
            if matches!(r.as_str(), "AXRow" | "AXCell" | "AXOutlineRow") { gewaehlt = format!("{r}:select"); break; }
            if matches!(r.as_str(), "AXWebArea" | "AXWindow" | "AXScrollArea") { break; }
        }
        if ergebnis.is_none() && gewaehlt.is_empty() {
            // Before a weak target: an open web menu overflowing its parents
            // (Claude "+" menu) - press the menu item at the point.
            let app2 = AXUIElementCreateApplication(pid);
            if !app2.is_null() {
                if let Some(f) = ax_fenster_element(app2, wid) {
                    let mut b = 0usize;
                    let gefunden = offenes_menue_am_punkt(f, x, y, 0, &mut b);
                    crate::virtual_workspace::trace(&format!("[KLICK] open_menu_probe wid={wid} visited={b} found={}", gefunden.is_some()));
                    if let Some(menue) = gefunden {
                        let mut k = vec![CFRetain(menue as *const c_void) as *mut c_void];
                        let mut b2 = 0usize;
                        geometrische_kette(menue, x, y, 0, &mut b2, &mut k);
                        crate::virtual_workspace::trace(&format!("[KLICK] open_menu_chain {:?}", k.iter().map(|e| ax_rolle(*e)).collect::<Vec<_>>()));
                        if let Some(&item) = k.iter().rev().find(|e| ax_rolle(**e) == "AXMenuItem") {
                            ergebnis = Some(ax_aktion(item, "AXPress"));
                            gewaehlt = format!("AXMenuItem:{}:AXPress(open_menu)", ax_text_attr(item, "AXTitle"));
                        }
                        for e in k { CFRelease(e as *const c_void); }
                        CFRelease(menue as *const c_void);
                    }
                    CFRelease(f as *const c_void);
                }
                CFRelease(app2 as *const c_void);
            }
        }
        if ergebnis.is_none() && gewaehlt.is_empty() {
            // Nothing typed as a control: the deepest element that lists
            // AXPress (web click listeners, custom views). Outside web
            // content, a pressable AXStaticText IS the control (GoodNotes
            // folders and sidebar entries are exactly that).
            let web = kette.iter().any(|e| ax_rolle(*e) == "AXWebArea");
            for &e in &kette {
                let r = ax_rolle(e);
                if matches!(r.as_str(), "AXWebArea" | "AXWindow" | "AXApplication" | "AXScrollArea") { break; }
                let text_steuerung = r == "AXStaticText" && !web;
                // A LABELLED region with its own press (YouTube "YouTube Video
                // Player" = click on the video toggles play/pause). Unlabelled
                // click-listener groups stay excluded. Measured: AXPress on a
                // hidden Chromium control does not switch Space (focus does).
                let benannt = r == "AXGroup" && !ax_text_attr(e, "AXDescription").is_empty();
                if (text_steuerung || benannt || !matches!(r.as_str(), "AXGroup" | "AXImage" | "AXStaticText"))
                    && ax_aktion_vorhanden(e, "AXPress")
                {
                    ergebnis = Some(ax_aktion(e, "AXPress"));
                    gewaehlt = format!("{r}:listed_AXPress");
                    break;
                }
            }
        }
        for e in kette { CFRelease(e as *const c_void); }
        crate::virtual_workspace::trace(&format!(
            "[KLICK] resolve wid={wid} pid={pid} point={x:.0},{y:.0} chain={:?} chosen={gewaehlt} result={:?}",
            rollen, ergebnis));
        anmeldung_hinweis(pid, &gewaehlt, ergebnis);
        ergebnis
    }

    /// Sign-in buttons of the ChatGPT app: the press arrives (AXPress
    /// succeeds) but starting the sign-in needs a real user activation -
    /// measured 2026-09-27: 3 presses of "Weiter zur Anmeldung", the app
    /// logged nothing and opened nothing. The OAuth hand-off (browser) must
    /// be done by the user anyway. Say so instead of a silent button.
    fn anmeldung_hinweis(pid: i32, gewaehlt: &str, ergebnis: Option<bool>) {
        if ergebnis != Some(true) || super::bundle_gemerkt(pid) != "com.openai.codex" { return; }
        let l = gewaehlt.to_lowercase();
        if ["anmeld", "log in", "login", "sign in", "sign up", "registrier", "weiter mit", "continue with"].iter().any(|w| l.contains(w)) {
            crate::virtual_workspace::trace(&format!("[CAPABILITY] sign_in pid={pid} control={gewaehlt} class=NATIVE_ONLY reason=needs_user_activation_and_browser_auth"));
            crate::vorschau::hinweis("Anmeldung bitte auf dem Noki Schreibtisch abschließen.");
        }
    }

    /// Chain of the element at the point in the exact window (deepest
    /// first, max 10 levels), each retained.
    unsafe fn kette_am_punkt(app: *mut c_void, wid: i64, x: f64, y: f64) -> Vec<*mut c_void> {
        // Geometric walk of the EXACT window first: the system hit test
        // stops at an NSOpenPanel's AXSplitGroup (remote browser view) and
        // never reaches its rows (measured TextEdit "Öffnen"). Dialogs that
        // are not in AXWindows (VS Code "Open") are reached via the app's
        // focused window when it carries this window number.
        let fenster = ax_fenster_element(app, wid);
        if let Some(f) = fenster {
            // Open menus/popovers of web apps overflow their ancestors'
            // frames (measured Claude "+" menu: parents 1547-1655, item at
            // 1484) - geometric pruning cannot reach them. An open AXMenu
            // containing the point is found by a bounded unpruned walk and
            // always wins (it is on top).
            let mut besucht = 0usize;
            let mut k = Vec::new();
            let rang = geometrische_kette_suche(f, x, y, 0, &mut besucht, &mut k);
            // Only when nothing actionable was found (costly walk).
            if rang < 2 {
                let mut b2 = 0usize;
                if let Some(menue) = offenes_menue_am_punkt(f, x, y, 0, &mut b2) {
                    for e in k.drain(..) { CFRelease(e as *const c_void); }
                    k.push(menue);
                    let mut b3 = 0usize;
                    geometrische_kette(menue, x, y, 0, &mut b3, &mut k);
                }
            }
            CFRelease(f as *const c_void);
            if !k.is_empty() { k.reverse(); return k; }
        }
        let mut k = vec![];
        let Some(el) = ax_element_im_fenster(app, wid, x, y) else { return k };
        let mut cur = el;
        for _ in 0..10 {
            if matches!(ax_rolle(cur).as_str(), "AXWindow" | "AXApplication" | "") { CFRelease(cur as *const c_void); return k; }
            k.push(cur);
            let Some(p) = ax_element_attr(cur, "AXParent") else { return k };
            cur = p;
        }
        CFRelease(cur as *const c_void);
        k
    }

    /// Open AXMenu whose frame contains the point, anywhere in the tree
    /// (no geometric pruning; bounded). Retained.
    unsafe fn offenes_menue_am_punkt(el: *mut c_void, x: f64, y: f64, tiefe: usize, besucht: &mut usize) -> Option<*mut c_void> {
        if tiefe > 60 || *besucht > 5_000 || zeit_um() { return None; }
        *besucht += 1;
        if tiefe > 0 && ax_rolle(el) == "AXMenu" {
            if ax_rahmen(el).is_some_and(|r| r.g.w >= 1.0 && r.g.h >= 1.0 && x >= r.o.x && x < r.o.x + r.g.w && y >= r.o.y && y < r.o.y + r.g.h) {
                return Some(CFRetain(el as *const c_void) as *mut c_void);
            }
            return None;
        }
        let kinder = ax_element_attr(el, "AXChildren")?;
        let mut treffer = None;
        let n = CFArrayGetCount(kinder as *const c_void);
        for j in 0..n {
            let c = CFArrayGetValueAtIndex(kinder as *const c_void, n - 1 - j) as *mut c_void;
            if let Some(m) = offenes_menue_am_punkt(c, x, y, tiefe + 1, besucht) { treffer = Some(m); break; }
        }
        CFRelease(kinder as *const c_void);
        treffer
    }

    /// 3 = inside an open menu (menus are always on top), 2 = real control,
    /// 0 = nothing actionable.
    unsafe fn aktions_rang(e: *mut c_void) -> u8 {
        let r = ax_rolle(e);
        if matches!(r.as_str(), "AXMenu" | "AXMenuItem") { return 3; }
        if matches!(r.as_str(), "AXButton" | "AXLink" | "AXCheckBox" | "AXRadioButton"
            | "AXMenuButton" | "AXPopUpButton" | "AXDisclosureTriangle" | "AXIncrementor" | "AXTab"
            | "AXRow" | "AXCell" | "AXTextField" | "AXTextArea" | "AXSearchField" | "AXComboBox" | "AXSlider") { return 2; }
        if r == "AXImage" && ax_aktion_vorhanden(e, "AXOpen") { return 2; }
        // Controls and TILES: Catalyst/SwiftUI apps expose items as
        // AXStaticText/AXGroup with AXPress (measured GoodNotes folder tile
        // 138x207 - the old 120 pt height limit ranked it 0, so clicks on
        // folders resolved to nothing). Area-bounded, so full-page Chromium
        // click-listener groups still do not win.
        if ax_aktion_vorhanden(e, "AXPress") && ax_rahmen(e).is_some_and(|f|
            (f.g.h <= 120.0 && f.g.w <= 700.0) || f.g.w * f.g.h <= 90_000.0) { return 2; }
        0
    }

    /// Geometric search through the EXACT window: every child containing the
    /// point is explored; the branch with the best rank wins (open menu >
    /// real control > deepest), ties go to the FRONT-MOST sibling. Measured:
    /// "longest chain" picked a deep dashboard under Claude's open "+" menu,
    /// the click closed the menu instead of choosing the item.
    /// Pushes window-side first; retained elements.
    unsafe fn geometrische_kette(el: *mut c_void, x: f64, y: f64, tiefe: usize, besucht: &mut usize, k: &mut Vec<*mut c_void>) {
        let _ = geometrische_kette_suche(el, x, y, tiefe, besucht, k);
    }

    /// Returns the best rank inside the pushed chain.
    unsafe fn geometrische_kette_suche(el: *mut c_void, x: f64, y: f64, tiefe: usize, besucht: &mut usize, k: &mut Vec<*mut c_void>) -> u8 {
        if tiefe > 40 || *besucht > 4_000 || zeit_um() { return 0; }
        let Some(kinder) = ax_element_attr(el, "AXChildren") else { return 0 };
        let mut beste: Vec<*mut c_void> = Vec::new();
        let mut beste_rang = 0u8;
        let n = CFArrayGetCount(kinder as *const c_void);
        for j in 0..n {
            let i = n - 1 - j; // front-most first
            *besucht += 1;
            let c = CFArrayGetValueAtIndex(kinder as *const c_void, i) as *mut c_void;
            let Some(r) = ax_rahmen(c) else { continue };
            if !(r.g.w >= 1.0 && r.g.h >= 1.0 && x >= r.o.x && x < r.o.x + r.g.w && y >= r.o.y && y < r.o.y + r.g.h) { continue; }
            let mut kette = vec![CFRetain(c as *const c_void) as *mut c_void];
            let rang = geometrische_kette_suche(c, x, y, tiefe + 1, besucht, &mut kette).max(aktions_rang(c));
            let besser = rang > beste_rang || (rang == beste_rang && beste_rang == 0 && kette.len() > beste.len())
                || beste.is_empty();
            if besser {
                for e in beste.drain(..) { CFRelease(e as *const c_void); }
                beste = kette; beste_rang = rang;
            } else {
                for e in kette { CFRelease(e as *const c_void); }
            }
        }
        CFRelease(kinder as *const c_void);
        k.extend(beste);
        beste_rang
    }

    /// Single click on list/outline/table/browser content (Finder list,
    /// sidebar, open/save panels, icon views): the standard AX meaning is
    /// SELECT - the nearest row/cell/item whose AXSelected is settable.
    /// Finder and NSOpenPanel navigate their sidebar on selection.
    unsafe fn ax_auswaehlen_fenster(pid: i32, wid: i64, x: f64, y: f64) -> bool {
        if !AXIsProcessTrusted() || pid <= 0 { return false; }
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() { return false; }
        let kette = kette_am_punkt(app, wid, x, y);
        let mut ok = false;
        let mut rolle = String::new();
        // Measured Finder: AXSelected on a sidebar row changes nothing;
        // AXSelectedRows=[row] on the outline fires the selection delegate
        // and navigates. Collections (icon view) use AXSelectedChildren.
        let mut zeile: Option<*mut c_void> = None;
        let mut element: Option<*mut c_void> = None;
        for &e in &kette {
            let r = ax_rolle(e);
            if r == "AXRow" && zeile.is_none() { zeile = Some(e); }
            if matches!(r.as_str(), "AXOutline" | "AXTable" | "AXBrowser") {
                if let Some(z) = zeile {
                    if ax_setzbar(e, "AXSelectedRows") {
                        let arr = CFArrayCreate(std::ptr::null(), [z as *const c_void].as_ptr(), 1, &kCFTypeArrayCallBacks);
                        let a = cfstr("AXSelectedRows");
                        ok = AXUIElementSetAttributeValue(e, a, arr) == 0;
                        CFRelease(a); CFRelease(arr);
                        rolle = format!("{r}/AXSelectedRows");
                    }
                }
                break;
            }
            if r == "AXList" {
                if let Some(item) = element {
                    if ax_setzbar(e, "AXSelectedChildren") {
                        let arr = CFArrayCreate(std::ptr::null(), [item as *const c_void].as_ptr(), 1, &kCFTypeArrayCallBacks);
                        let a = cfstr("AXSelectedChildren");
                        ok = AXUIElementSetAttributeValue(e, a, arr) == 0;
                        CFRelease(a); CFRelease(arr);
                        rolle = "AXList/AXSelectedChildren".into();
                    }
                }
                if ok { break; }
            }
            if element.is_none() && matches!(r.as_str(), "AXGroup" | "AXImage" | "AXCell" | "AXRow") { element = Some(e); }
        }
        if !ok {
            for &e in &kette {
                let r = ax_rolle(e);
                if matches!(r.as_str(), "AXRow" | "AXCell" | "AXImage" | "AXGroup" | "AXStaticText")
                    && ax_setzbar(e, "AXSelected")
                {
                    let a = cfstr("AXSelected");
                    ok = AXUIElementSetAttributeValue(e, a, kCFBooleanTrue) == 0;
                    CFRelease(a);
                    rolle = r;
                    break;
                }
                if matches!(r.as_str(), "AXOutline" | "AXTable" | "AXList" | "AXBrowser" | "AXScrollArea") { break; }
            }
        }
        for e in kette { CFRelease(e as *const c_void); }
        CFRelease(app as *const c_void);
        crate::virtual_workspace::trace(&format!("[KLICK] select wid={wid} role={rolle} ok={ok}"));
        ok
    }

    /// Double click = OPEN: AXOpen on the nearest element offering it
    /// (Finder items, panel cells), else select + AXPress.
    unsafe fn ax_oeffnen_fenster(pid: i32, wid: i64, x: f64, y: f64) -> bool {
        if !AXIsProcessTrusted() || pid <= 0 { return false; }
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() { return false; }
        // Select first: open/save panels confirm the SELECTION (measured:
        // AXConfirm opened the previously selected item, not the clicked one).
        CFRelease(app as *const c_void);
        let _ = ax_auswaehlen_fenster(pid, wid, x, y);
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() { return false; }
        let kette = kette_am_punkt(app, wid, x, y);
        let mut ok = false;
        // REAL_SPACE: "open" on an ordinary item launches the document in
        // its app - that window is born on the user's ACTIVE Desktop and the
        // app activates (measured 2026-09-27: Finder double click opened
        // TextEdit on Desktop 1 although AXOpen reported an error). Only
        // inside an open/save panel or dialog does "open" stay in the hidden
        // window (it confirms the panel's choice).
        let echt = crate::virtual_workspace::backend() == crate::virtual_workspace::Backend::RealSpace;
        let im_panel = kette.iter().any(|&e| ax_rolle(e) == "AXSheet" || ax_text_attr(e, "AXSubrole") == "AXDialog")
            || ax_fenster_element(app, wid).is_some_and(|w| {
                let d = matches!(ax_text_attr(w, "AXSubrole").as_str(), "AXDialog" | "AXSystemDialog");
                CFRelease(w as *const c_void); d
            });
        for &e in kette.iter().filter(|_| !echt || im_panel) {
            if ax_aktion_vorhanden(e, "AXOpen") {
                // Remote panel/Finder elements list AXOpen but reject it
                // (-25205); AXConfirm is their working "open".
                ok = ax_aktion(e, "AXOpen") || (ax_aktion_vorhanden(e, "AXConfirm") && ax_aktion(e, "AXConfirm"));
                break;
            }
            if ax_aktion_vorhanden(e, "AXConfirm") { ok = ax_aktion(e, "AXConfirm"); break; }
        }
        CFRelease(app as *const c_void);
        for e in kette { CFRelease(e as *const c_void); }
        crate::virtual_workspace::trace(&format!("[KLICK] open wid={wid} ok={ok}"));
        if ok { return true; }
        // REAL_SPACE: no real "open" exists here. A double click on a real
        // control is still a press; on an item (Catalyst cell, text, group)
        // a press only SELECTS - never report that as "opened".
        if crate::virtual_workspace::backend() == crate::virtual_workspace::Backend::RealSpace {
            const STEUERUNG: [&str; 9] = ["AXButton", "AXLink", "AXCheckBox", "AXRadioButton", "AXMenuItem",
                "AXMenuButton", "AXPopUpButton", "AXTab", "AXToolbarButton"];
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return false; }
            let kette = kette_am_punkt(app, wid, x, y);
            CFRelease(app as *const c_void);
            let steuerung = kette.first().is_some_and(|e| STEUERUNG.contains(&ax_rolle(*e).as_str()));
            for e in kette { CFRelease(e as *const c_void); }
            crate::virtual_workspace::trace(&format!("[CAPABILITY] double_click wid={wid} control={steuerung} class={}",
                if steuerung { "REMOTE_SUPPORTED(press)" } else { "NATIVE_ONLY(open)" }));
            return steuerung && ax_druecken_fenster(pid, wid, x, y);
        }
        ax_druecken_fenster(pid, wid, x, y)
    }

    /// Ein retained AX-Element als Zahl, damit es Faeden wechseln kann.
    pub struct Feld(usize);
    unsafe impl Send for Feld {}
    unsafe impl Sync for Feld {}
    impl Drop for Feld {
        fn drop(&mut self) { unsafe { CFRelease(self.0 as *const c_void) } }
    }

    /// Feld (fokussiert und nachgewiesen) und Chromes vorheriges
    /// Fokusfenster, falls es ein anderes war.
    pub fn eingabe_fokussieren(pid: i32, wid: i64, x: f64, y: f64) -> Option<(Feld, Option<Feld>)> {
        unsafe {
            ax_eingabe_fokussieren(pid, wid, x, y)
                .map(|(f, v)| (Feld(f as usize), v.map(|v| Feld(v as usize))))
        }
    }

    /// Chromes vorheriges Hauptfenster zurueckgeben - nur solange der Nutzer
    /// Chrome nicht selbst nach vorn geholt hat (dann entscheidet er).
    pub fn hauptfenster_zurueck(fenster: &Feld) -> bool {
        unsafe {
            let main_attr = cfstr("AXMain");
            let ok = AXUIElementSetAttributeValue(fenster.0 as *mut c_void, main_attr, kCFBooleanTrue) == 0;
            CFRelease(main_attr);
            ok
        }
    }

    pub fn fokus_ist(pid: i32, wid: i64, feld: &Feld) -> bool {
        unsafe {
            if !AXIsProcessTrusted() { return false; }
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return false; }
            let ok = ax_fokus_ist(app, wid, feld.0 as *mut c_void);
            CFRelease(app as *const c_void);
            ok
        }
    }

    /// Nach Tab/Enter kann der Fokus im selben Fenster auf ein anderes
    /// Eingabefeld gewandert sein: dieses (retained), sonst None.
    pub fn fokus_eingabe(pid: i32, wid: i64) -> Option<Feld> {
        unsafe {
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return None; }
            let fokus = ax_element_attr(app, "AXFocusedUIElement");
            let mut feld = fokus.and_then(|f| {
                let e = ax_editierbar(f);
                CFRelease(f as *const c_void);
                e
            });
            // Inactive Electron may omit AXFocusedUIElement although the
            // exact Monaco/xterm node has AXFocused=true. Search only the
            // retained target-window token, with strict node/time bounds.
            if feld.is_none() {
                unsafe fn fokussiert_suchen(el: *mut c_void, tiefe: usize, besucht: &mut usize, start: std::time::Instant) -> Option<*mut c_void> {
                    if tiefe > 40 || *besucht > 4_000 || start.elapsed() > std::time::Duration::from_millis(120) { return None; }
                    *besucht += 1;
                    if let Some(e) = ax_editierbar(el) {
                        let an = ax_element_attr(e, "AXFocused").map(|v| {
                            let ja = CFEqual(v as *const c_void, kCFBooleanTrue);
                            CFRelease(v as *const c_void); ja
                        }).unwrap_or(false);
                        if an { return Some(e); }
                        CFRelease(e as *const c_void);
                    }
                    let kinder = ax_element_attr(el, "AXChildren")?;
                    let mut aus = None;
                    for i in 0..CFArrayGetCount(kinder as *const c_void) {
                        let c = CFArrayGetValueAtIndex(kinder as *const c_void, i) as *mut c_void;
                        if let Some(e) = fokussiert_suchen(c, tiefe + 1, besucht, start) { aus = Some(e); break; }
                    }
                    CFRelease(kinder as *const c_void);
                    aus
                }
                if let Some(w) = ax_fenster_element(app, wid) {
                    let mut besucht = 0usize;
                    feld = fokussiert_suchen(w, 0, &mut besucht, std::time::Instant::now());
                    CFRelease(w as *const c_void);
                }
            }
            let r = feld.and_then(|f| {
                if ax_fokus_ist(app, wid, f) { Some(Feld(f as usize)) }
                else { CFRelease(f as *const c_void); None }
            });
            CFRelease(app as *const c_void);
            r
        }
    }

    /// Eine Kopie des physischen Tastenereignisses (Keycode, Modifikatoren
    /// und Zeichen bleiben erhalten) GENAU an diesen Prozess. Markiert, damit
    /// Nokis eigener Abgriff sie nie ein zweites Mal verarbeitet.
    pub fn taste_an_prozess(pid: i32, ereignis: usize, marke: i64) -> bool {
        unsafe {
            let kopie = CGEventCreateCopy(ereignis as *mut c_void);
            if kopie.is_null() { return false; }
            CGEventSetIntegerValueField(kopie, 42, marke);
            CGEventPostToPid(pid, kopie);
            CFRelease(kopie as *const c_void);
            true
        }
    }

    /// (AXTitle, AXDocument, hat Tab-Leiste) GENAU dieses Fensters - auch
    /// auf einem anderen Space (remote token).
    pub fn fenster_titel_dokument(pid: i32, wid: i64) -> Option<(String, String, bool)> {
        unsafe {
            if !AXIsProcessTrusted() || pid <= 0 || wid <= 0 { return None; }
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return None; }
            let r = ax_fenster_element(app, wid).map(|w| {
                let titel = ax_text_attr(w, "AXTitle");
                let dok = ax_text_attr(w, "AXDocument");
                let mut tabs = false;
                if let Some(k) = ax_element_attr(w, "AXChildren") {
                    for i in 0..CFArrayGetCount(k as *const c_void) {
                        if ax_rolle(CFArrayGetValueAtIndex(k as *const c_void, i) as *mut c_void) == "AXTabGroup" { tabs = true; }
                    }
                    CFRelease(k as *const c_void);
                }
                CFRelease(w as *const c_void);
                (titel, dok, tabs)
            });
            CFRelease(app as *const c_void);
            r
        }
    }

    /// GENAU dieses Fenster schliessen: sein eigener Schliessknopf (AXCloseButton).
    /// Beendet nie das Programm.
    pub fn fenster_schliessen(pid: i32, wid: i64) -> bool {
        unsafe {
            if !AXIsProcessTrusted() { return false; }
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return false; }
            let mut ok = false;
            if let Some(w) = ax_fenster_element(app, wid) {
                // AXPress on an AXCloseButton may report success but be a
                // no-op while the exact window is minimized (observed with a
                // Chrome PWA). Restore only this owned window first; never
                // activate or quit its application.
                // Only when it really is minimized: an unconditional
                // AXMinimized=false on a normal SwiftUI window (Rechner)
                // swallowed the following close press (measured).
                let minimized = cfstr("AXMinimized");
                let mut wert: *const c_void = std::ptr::null();
                let ist_min = AXUIElementCopyAttributeValue(w, minimized, &mut wert) == 0
                    && !wert.is_null() && CFEqual(wert, kCFBooleanTrue);
                if !wert.is_null() { CFRelease(wert); }
                if ist_min {
                    let _ = AXUIElementSetAttributeValue(w, minimized, kCFBooleanFalse);
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
                CFRelease(minimized);
                // Native fullscreen: AXPress on the close button is accepted
                // but does nothing (measured, Finder). Leave fullscreen with
                // the exact window's own attribute first, then close it.
                let voll = cfstr("AXFullScreen");
                let ist_voll = |w| {
                    let mut v: *const c_void = std::ptr::null();
                    let ja = AXUIElementCopyAttributeValue(w, voll, &mut v) == 0
                        && !v.is_null() && CFEqual(v, kCFBooleanTrue);
                    if !v.is_null() { CFRelease(v); }
                    ja
                };
                if ist_voll(w) {
                    let _ = AXUIElementSetAttributeValue(w, voll, kCFBooleanFalse);
                    let bis = std::time::Instant::now() + std::time::Duration::from_millis(2000);
                    while ist_voll(w) && std::time::Instant::now() < bis {
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                    // The exit animation still runs after the flag flips.
                    std::thread::sleep(std::time::Duration::from_millis(700));
                }
                CFRelease(voll);
                if let Some(knopf) = ax_element_attr(w, "AXCloseButton") {
                    let a = cfstr("AXPress");
                    ok = AXUIElementPerformAction(knopf, a) == 0;
                    CFRelease(a);
                    CFRelease(knopf as *const c_void);
                }
                // Some document-style windows expose no close button but
                // advertise the exact window's AXClose action. This remains
                // window-scoped; never synthesize Cmd-W unless an adapter can
                // prove which one of an app's windows is focused.
                if !ok {
                    let a = cfstr("AXClose");
                    ok = AXUIElementPerformAction(w, a) == 0;
                    CFRelease(a);
                }
                CFRelease(w as *const c_void);
            }
            CFRelease(app as *const c_void);
            ok
        }
    }

    /// Safe secondary close route for apps whose AXCloseButton reports
    /// success but is a no-op. Cmd-W is posted only after AX proves that the
    /// exact registered CGWindowID is the app's focused window.
    pub fn fenster_schliessen_kurzbefehl(pid: i32, wid: i64) -> bool {
        unsafe {
            if !AXIsProcessTrusted() || pid <= 0 || wid <= 0 { return false; }
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return false; }
            let Some(w) = ax_fenster_element(app, wid) else {
                CFRelease(app as *const c_void); return false;
            };
            let raise = cfstr("AXRaise");
            let _ = AXUIElementPerformAction(w, raise);
            CFRelease(raise);
            let focused_attr = cfstr("AXFocusedWindow");
            let _ = AXUIElementSetAttributeValue(app, focused_attr, w as *const c_void);
            let focused = ax_element_attr(app, "AXFocusedWindow");
            let number_attr = cfstr("AXWindowNumber");
            let exact = focused.is_some_and(|f| ax_fenster_nummer(f, number_attr) == Some(wid));
            if let Some(f) = focused { CFRelease(f as *const c_void); }
            CFRelease(number_attr); CFRelease(focused_attr);
            let mut sent = false;
            if exact {
                let down = CGEventCreateKeyboardEvent(std::ptr::null(), 13, true); // W
                let up = CGEventCreateKeyboardEvent(std::ptr::null(), 13, false);
                if !down.is_null() && !up.is_null() {
                    const COMMAND: u64 = 1 << 20;
                    CGEventSetFlags(down, COMMAND); CGEventSetFlags(up, COMMAND);
                    CGEventPostToPid(pid, down); CGEventPostToPid(pid, up);
                    sent = true;
                }
                if !down.is_null() { CFRelease(down as *const c_void); }
                if !up.is_null() { CFRelease(up as *const c_void); }
            }
            CFRelease(w as *const c_void); CFRelease(app as *const c_void);
            eprintln!("[FERNAX] close-shortcut wid={wid} exact-focused={exact} sent={sent}");
            sent
        }
    }

    /// AX is authoritative for an application window. WindowServer can keep
    /// a closed window number in its list briefly, which previously made a
    /// successful exact close look like a failure and left registry UI stale.
    pub fn fenster_vorhanden(pid: i32, wid: i64) -> bool {
        unsafe {
            // Uncertainty must not be interpreted as successful closure.
            // The caller then keeps the real layer/registry and reports a
            // clear failure instead of visually deleting a live window.
            if !AXIsProcessTrusted() { return true; }
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return true; }
            let window = ax_fenster_element(app, wid);
            if let Some(w) = window { CFRelease(w as *const c_void); }
            CFRelease(app as *const c_void);
            window.is_some()
        }
    }

    /// Kann an dieser Stelle das Fenster gezogen werden (Titel-/Werkzeugband)?
    ///
    /// Gemessen (Chrome 153, ColorSync): der Treffer "genau AXWindow" kam
    /// praktisch nie vor - Chromes leere Tab-Leiste ist eine AXGroup, die
    /// Luecken einer Werkzeugleiste sind die AXToolbar selbst. Deshalb:
    ///   * nur im Kopfband des Fensters (Titelzeile bzw. vereinte
    ///     Werkzeugleiste; bei Chrome die Tab-Leiste ueber der Toolbar);
    ///   * nie auf den Ampeln;
    ///   * nie auf einem bedienbaren Element (Tab, Knopf, Feld, Link,
    ///     Webinhalt, alles mit AXPress) - gesucht im Baum GENAU dieses
    ///     Fensters, nicht per App-Treffertest (ueberlappende Fenster).
    pub fn fenster_ziehbereich(pid: i32, wid: i64, x: f64, y: f64) -> bool {
        unsafe fn kopf_unterkante(el: *mut c_void, oben: f64, tiefe: usize, beste: &mut Option<f64>) {
            if tiefe > 7 { return; }
            let Some(k) = ax_element_attr(el, "AXChildren") else { return };
            for i in 0..CFArrayGetCount(k) {
                let c = CFArrayGetValueAtIndex(k, i) as *mut c_void;
                let r = ax_rolle(c);
                if r == "AXWebArea" { continue; }
                let Some(f) = ax_rahmen(c) else { continue };
                if f.o.y > oben + 60.0 { continue; }
                if r == "AXToolbar" {
                    // Vereinte Werkzeugleiste (oben am Fenster) gehoert zum
                    // Kopf; eine Toolbar UNTER einer Tab-Leiste nicht.
                    let unten = if f.o.y <= oben + 32.0 { f.o.y + f.g.h } else { f.o.y };
                    *beste = Some(beste.map_or(unten, |b: f64| b.max(unten)));
                    continue;
                }
                kopf_unterkante(c, oben, tiefe + 1, beste);
            }
            CFRelease(k as *const c_void);
        }
        unsafe fn bedienbar(el: *mut c_void) -> bool {
            matches!(ax_rolle(el).as_str(),
                "AXButton" | "AXRadioButton" | "AXCheckBox" | "AXPopUpButton" | "AXMenuButton"
                | "AXTextField" | "AXTextArea" | "AXComboBox" | "AXSlider" | "AXLink"
                // AXWebArea/AXScrollArea are CONTAINERS: in Electron apps
                // (Spotify, ChatGPT, Claude) the whole title band is web
                // content - treating the web area as a control made those
                // windows impossible to move.
                | "AXTable" | "AXOutline" | "AXList"
                | "AXSegmentedControl" | "AXIncrementor" | "AXMenuBar" | "AXDisclosureTriangle")
                // A listed AXPress makes a CONTROL only at control size:
                // Spotify's whole-window web root lists AXPress (click
                // listener) and blocked every header drag.
                || (ax_aktion_vorhanden(el, "AXPress")
                    && ax_rahmen(el).is_some_and(|r| r.g.h <= 90.0 && r.g.w <= 600.0))
        }
        unsafe fn trifft_bedienbares(el: *mut c_void, x: f64, y: f64, tiefe: usize) -> bool {
            if tiefe > 12 { return false; }
            let Some(k) = ax_element_attr(el, "AXChildren") else { return false };
            let mut treffer = false;
            for i in 0..CFArrayGetCount(k) {
                let c = CFArrayGetValueAtIndex(k, i) as *mut c_void;
                let Some(f) = ax_rahmen(c) else { continue };
                if x < f.o.x || y < f.o.y || x > f.o.x + f.g.w || y > f.o.y + f.g.h { continue; }
                if bedienbar(c) || trifft_bedienbares(c, x, y, tiefe + 1) { treffer = true; break; }
            }
            CFRelease(k as *const c_void);
            treffer
        }
        unsafe {
            if !AXIsProcessTrusted() { return false; }
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return false; }
            let Some(window) = ax_fenster_element(app, wid) else { CFRelease(app); return false };
            let mut ok = false;
            if let Some(r) = ax_rahmen(window) {
                let mut unten = None;
                kopf_unterkante(window, r.o.y, 0, &mut unten);
                let kopf = unten.unwrap_or(r.o.y + 32.0).clamp(r.o.y + 24.0, r.o.y + 100.0);
                let ampeln = x < r.o.x + 76.0 && y < r.o.y + 34.0;
                let im_kopf = y >= r.o.y && y <= kopf && x >= r.o.x && x <= r.o.x + r.g.w;
                ok = im_kopf && !ampeln && !trifft_bedienbares(window, x, y, 0);
            }
            CFRelease(window); CFRelease(app);
            ok
        }
    }

    /// Minimize exactly one registered window. The application and all of
    /// its other (possibly user-owned) windows remain untouched.
    pub fn fenster_minimieren(pid: i32, wid: i64) -> bool {
        unsafe {
            if !AXIsProcessTrusted() { return false; }
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return false; }
            let mut ok = false;
            if let Some(w) = ax_fenster_element(app, wid) {
                if let Some(btn) = ax_element_attr(w, "AXMinimizeButton") {
                    let a = cfstr("AXPress");
                    ok = AXUIElementPerformAction(btn, a) == 0;
                    CFRelease(a);
                    CFRelease(btn as *const c_void);
                }
                if !ok {
                    let attr = cfstr("AXMinimized");
                    ok = AXUIElementSetAttributeValue(w, attr, kCFBooleanTrue) == 0;
                    CFRelease(attr);
                }
                CFRelease(w as *const c_void);
            }
            CFRelease(app as *const c_void);
            ok
        }
    }

    /// Toggle the native zoomed/fullscreen state of exactly one window.
    /// Tries the native AXFullScreenButton or AXZoomButton first, then falls back
    /// to the settable AXZoomed attribute.
    pub fn fenster_vergroessern(pid: i32, wid: i64) -> bool {
        unsafe {
            if !AXIsProcessTrusted() { return false; }
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return false; }
            let mut ok = false;
            if let Some(w) = ax_fenster_element(app, wid) {
                if let Some(btn) = ax_element_attr(w, "AXFullScreenButton") {
                    let a = cfstr("AXPress");
                    ok = AXUIElementPerformAction(btn, a) == 0;
                    CFRelease(a);
                    CFRelease(btn as *const c_void);
                }
                if !ok {
                    if let Some(btn) = ax_element_attr(w, "AXZoomButton") {
                        let a = cfstr("AXPress");
                        ok = AXUIElementPerformAction(btn, a) == 0;
                        CFRelease(a);
                        CFRelease(btn as *const c_void);
                    }
                }
                if !ok {
                    let attr = cfstr("AXZoomed");
                    let mut settable = 0u8;
                    let mut value: *const c_void = std::ptr::null();
                    let can_set = AXUIElementIsAttributeSettable(w, attr, &mut settable) == 0 && settable != 0;
                    let current = AXUIElementCopyAttributeValue(w, attr, &mut value) == 0
                        && !value.is_null() && CFEqual(value, kCFBooleanTrue);
                    if !value.is_null() { CFRelease(value); }
                    if can_set {
                        ok = AXUIElementSetAttributeValue(
                            w, attr, if current { kCFBooleanFalse } else { kCFBooleanTrue }
                        ) == 0;
                    }
                    CFRelease(attr);
                }
                CFRelease(w as *const c_void);
            }
            CFRelease(app as *const c_void);
            ok
        }
    }

    /// Neuer Tab in GENAU diesem Browserfenster (Knopf "Neuer Tab"/"New Tab").
    pub fn neuer_tab(pid: i32, wid: i64) -> bool {
        unsafe fn suchen(el: *mut c_void, tiefe: usize) -> Option<*mut c_void> {
            if tiefe > 10 { return None; }
            if ax_rolle(el) == "AXButton" {
                let t = format!("{} {}", ax_text_attr(el, "AXTitle"), ax_text_attr(el, "AXDescription")).to_lowercase();
                if t.contains("neuer tab") || t.contains("new tab") {
                    return Some(CFRetain(el as *const c_void) as *mut c_void);
                }
            }
            let k = ax_element_attr(el, "AXChildren")?;
            let mut r = None;
            for i in 0..CFArrayGetCount(k) {
                if let Some(t) = suchen(CFArrayGetValueAtIndex(k, i) as *mut c_void, tiefe + 1) { r = Some(t); break; }
            }
            CFRelease(k as *const c_void);
            r
        }
        unsafe {
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return false; }
            let mut ok = false;
            if let Some(w) = ax_fenster_element(app, wid) {
                if let Some(knopf) = suchen(w, 0) {
                    ok = ax_aktion(knopf, "AXPress");
                    CFRelease(knopf as *const c_void);
                }
                CFRelease(w as *const c_void);
            }
            CFRelease(app as *const c_void);
            ok
        }
    }

    /// Anzahl der Fenster, die AX fuer dieses Programm kennt (aktueller
    /// Schreibtisch, einschliesslich minimierter).
    /// Rahmen [x, y, w, h] genau dieses Fensters laut AX (sofort gueltig).
    pub fn ax_fenster_rahmen_von(pid: i32, wid: i64) -> Option<[f64; 4]> {
        unsafe {
            if !AXIsProcessTrusted() || pid <= 0 || wid <= 0 { return None; }
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return None; }
            let r = ax_fenster_element(app, wid).and_then(|w| {
                let r = ax_rahmen(w);
                CFRelease(w as *const c_void);
                r
            });
            CFRelease(app as *const c_void);
            r.map(|r| [r.o.x, r.o.y, r.g.w, r.g.h])
        }
    }

    /// Rahmen [x, y, w, h] aller Fenster, die AX fuer dieses Programm kennt.
    pub fn ax_fenster_rahmen(pid: i32) -> Vec<[f64; 4]> {
        unsafe {
            if !AXIsProcessTrusted() || pid <= 0 { return vec![]; }
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return vec![]; }
            let mut out = vec![];
            if let Some(w) = ax_element_attr(app, "AXWindows") {
                for i in 0..CFArrayGetCount(w as *const c_void) {
                    let f = CFArrayGetValueAtIndex(w as *const c_void, i) as *mut c_void;
                    if let Some(r) = ax_rahmen(f) { out.push([r.o.x, r.o.y, r.g.w, r.g.h]); }
                }
                CFRelease(w as *const c_void);
            }
            CFRelease(app as *const c_void);
            out
        }
    }

    /// Hat der Nutzer das Programm ausgeblendet (Cmd-H)?
    pub fn programm_ausgeblendet(pid: i32) -> bool {
        unsafe {
            if !AXIsProcessTrusted() || pid <= 0 { return false; }
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return false; }
            let v = ax_element_attr(app, "AXHidden");
            CFRelease(app as *const c_void);
            v.map(|v| { let ja = CFEqual(v as *const c_void, kCFBooleanTrue); CFRelease(v as *const c_void); ja })
                .unwrap_or(false)
        }
    }

    /// Wake an app that stopped committing frames on the hidden Space: a
    /// 1-pt height change and back through AX (measured 2026-09-27 on the
    /// GoodNotes Marketplace: model scrolled, pixels frozen; after this the
    /// next pans painted again). Only this window; frame verified restored.
    pub fn render_wecken(pid: i32, wid: i64) -> bool {
        // ONE wake at a time, process-wide. Two overlapping wakes (scroll
        // stall + input lane) let the second read the first one's -1 pt
        // height as "original" and restore to it: measured VS Code
        // 883 -> 882 for good. Serialized, every wake restores the exact
        // frame it measured before touching anything.
        static EINER: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _einer = EINER.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            if !AXIsProcessTrusted() || pid <= 0 || wid <= 0 { return false; }
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return false; }
            let w = ax_fenster_element(app, wid);
            CFRelease(app as *const c_void);
            let Some(w) = w else { return false };
            let Some(r) = ax_rahmen(w) else { CFRelease(w as *const c_void); return false };
            let attr = cfstr("AXSize");
            let setzen = |h: f64| {
                let g = Groesse { w: r.g.w, h };
                let v = AXValueCreate(2, &g as *const Groesse as *const c_void);
                let ok = !v.is_null() && AXUIElementSetAttributeValue(w, attr, v) == 0;
                if !v.is_null() { CFRelease(v); }
                ok
            };
            let a = setzen(r.g.h - 1.0);
            std::thread::sleep(std::time::Duration::from_millis(50));
            let mut b = setzen(r.g.h);
            let mut zurueck = false;
            for _ in 0..6 {
                if ax_rahmen(w).is_some_and(|n| (n.g.h - r.g.h).abs() < 0.5 && (n.g.w - r.g.w).abs() < 0.5) { zurueck = true; break; }
                std::thread::sleep(std::time::Duration::from_millis(60));
                b = setzen(r.g.h);
            }
            if !zurueck {
                crate::virtual_workspace::trace(&format!(
                    "[RENDER] wake wid={wid} restore_failed want={:.1}x{:.1}", r.g.w, r.g.h));
            }
            let b = b && zurueck;
            CFRelease(attr);
            CFRelease(w as *const c_void);
            a && b
        }
    }

    /// Laesst das Fenster seine Groesse aendern (AXSize setzbar)?
    pub fn skalierbar(pid: i32, wid: i64) -> Option<bool> {
        unsafe {
            if !AXIsProcessTrusted() || pid <= 0 || wid <= 0 { return None; }
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return None; }
            let w = ax_fenster_element(app, wid);
            CFRelease(app as *const c_void);
            let w = w?;
            let attr = cfstr("AXSize");
            let mut ja: u8 = 0;
            let ok = AXUIElementIsAttributeSettable(w, attr, &mut ja) == 0;
            CFRelease(attr);
            CFRelease(w as *const c_void);
            ok.then_some(ja != 0)
        }
    }

    /// Die echten Ampel-Knoepfe (Schliessen, Minimieren, Zoom/Vollbild)
    /// dieses Fensters, relativ zum Fenster: [x, y, w, h] je Knopf.
    pub fn ampel_rahmen(pid: i32, wid: i64) -> Option<[[f64; 4]; 3]> {
        unsafe {
            if !AXIsProcessTrusted() || pid <= 0 || wid <= 0 { return None; }
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return None; }
            let Some(w) = ax_fenster_element(app, wid) else { CFRelease(app as *const c_void); return None };
            let mut out = None;
            if let Some(f) = ax_rahmen(w) {
                let mut r = [[0.0; 4]; 3];
                let mut alle = true;
                for (i, namen) in [&["AXCloseButton"][..], &["AXMinimizeButton"][..],
                                   &["AXZoomButton", "AXFullScreenButton"][..]].iter().enumerate() {
                    let k = namen.iter().find_map(|n| ax_element_attr(w, n));
                    match k.and_then(|k| { let q = ax_rahmen(k); CFRelease(k as *const c_void); q }) {
                        Some(q) if q.g.w > 2.0 => r[i] = [q.o.x - f.o.x, q.o.y - f.o.y, q.g.w, q.g.h],
                        _ => alle = false,
                    }
                }
                if alle { out = Some(r); }
            }
            CFRelease(w as *const c_void);
            CFRelease(app as *const c_void);
            out
        }
    }

    /// AXPress auf genau einen Menuepunkt der Menueleiste dieses Programms
    /// (Titel exakt, z. B. "Datei" > "Neues Fenster"). Kein Tastenkuerzel,
    /// keine Aktivierung durch Noki.
    /// Is there such a menu item (without pressing it)?
    /// (AXRole, AXSubrole) of an exact window (incl. sheets / focused
    /// dialogs), empty strings when AX does not know it.
    pub fn fenster_art(pid: i32, wid: i64) -> (String, String) {
        unsafe {
            if !AXIsProcessTrusted() || pid <= 0 { return (String::new(), String::new()); }
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return (String::new(), String::new()); }
            let r = match ax_fenster_element(app, wid) {
                Some(w) => { let r = (ax_rolle(w), ax_text_attr(w, "AXSubrole")); CFRelease(w as *const c_void); r }
                None => (String::new(), String::new()),
            };
            CFRelease(app as *const c_void);
            r
        }
    }

    /// Labels of the field and its ancestors (AXDescription/AXTitle/
    /// AXRoleDescription, lowercase) - only UI labels, never the value. Used
    /// to tell VS Code's editor input from its integrated terminal input.
    pub fn feld_kontext(f: &Feld) -> String {
        #[link(name = "CoreFoundation", kind = "framework")]
        extern "C" {
            fn CFGetTypeID(o: *const c_void) -> usize;
            fn CFArrayGetTypeID() -> usize;
        }
        unsafe {
            let mut out = String::new();
            let mut el = f.0 as *mut c_void;
            let mut eigen = false;
            for _ in 0..24 {
                for a in ["AXDescription", "AXTitle", "AXHelp"] {
                    let t = ax_text_attr(el, a);
                    if !t.is_empty() { out.push_str(&t.to_lowercase()); out.push('|'); }
                }
                // Chromium: the element's DOM classes (".xterm", ".terminal",
                // ".monaco-editor") - structural, never content.
                if let Some(k) = ax_element_attr(el, "AXDOMClassList") {
                    if CFGetTypeID(k as *const c_void) == CFArrayGetTypeID() {
                        for i in 0..CFArrayGetCount(k as *const c_void) {
                            let c = cf_string(CFArrayGetValueAtIndex(k as *const c_void, i));
                            if !c.is_empty() { out.push('.'); out.push_str(&c.to_lowercase()); out.push('|'); }
                        }
                    }
                    CFRelease(k as *const c_void);
                }
                let Some(p) = ax_element_attr(el, "AXParent") else { break };
                if eigen { CFRelease(el as *const c_void); }
                el = p; eigen = true;
            }
            if eigen { CFRelease(el as *const c_void); }
            out
        }
    }

    /// VS Code: what lies DIRECTLY under the click point - the integrated
    /// terminal (Some(true)) or a Monaco editor (Some(false)); None if
    /// neither. The nearby-field search can return the terminal's textarea
    /// for a click into a small editor, so the point decides.
    pub fn vscode_terminal_am_punkt(pid: i32, wid: i64, x: f64, y: f64) -> Option<bool> {
        unsafe {
            let k = kette_im_fenster(pid, wid, x, y);
            let ctx = k.first().map(|e| Feld(CFRetain(*e as *const c_void) as usize).pipe_kontext()).unwrap_or_default();
            kette_freigeben(k);
            if crate::vscode_bruecke::ist_terminal_kontext(&ctx) { Some(true) }
            else if ctx.contains(".monaco-editor|") { Some(false) }
            else { None }
        }
    }

    impl Feld {
        /// `feld_kontext` of this (owned) element; the element is released.
        fn pipe_kontext(self) -> String { feld_kontext(&self) }
    }

    pub fn feld_rahmen(f: &Feld) -> Option<[f64; 4]> {
        unsafe { ax_rahmen(f.0 as *mut c_void).map(|r| [r.o.x, r.o.y, r.g.w, r.g.h]) }
    }

    /// Real text insertion point of the field (AXSelectedTextRange start ->
    /// AXBoundsForRange of that one cell), virtual coords. None when the
    /// app does not expose it or the point lies outside the field.
    pub fn einfuege_rahmen(f: &Feld) -> Option<[f64; 4]> {
        unsafe {
            let el = f.0 as *mut c_void;
            let v = ax_element_attr(el, "AXSelectedTextRange")?;
            #[repr(C)] struct CfRange { loc: isize, len: isize }
            let mut r = CfRange { loc: -1, len: 0 };
            let ok = AXValueGetValue(v as *const c_void, 4, &mut r as *mut CfRange as *mut c_void);
            CFRelease(v as *const c_void);
            if !ok || r.loc < 0 { return None; }
            // First ask AX for the zero-length selection itself. Terminal's
            // one-character AXBoundsForRange is a *whole visual line* (the
            // measured 637 pt white block); the zero-length range is the
            // actual insertion x/y.
            let insertion = {
                let attr = cfstr("AXBoundsForRange");
                let range = ax_bereich(r.loc, 0);
                let mut value: *const c_void = std::ptr::null();
                let got = AXUIElementCopyParameterizedAttributeValue(
                    el, attr, range, &mut value,
                ) == 0 && !value.is_null();
                CFRelease(attr); CFRelease(range);
                let mut rect = Rechteck::default();
                let valid = got && AXValueGetValue(
                    value, 3, &mut rect as *mut Rechteck as *mut c_void,
                );
                if !value.is_null() { CFRelease(value); }
                (valid && rect.g.h >= 1.0).then_some(rect)
            };
            // At end-of-buffer Terminal may not expose a range *after* the
            // final character. Derive that exact insertion cell from the
            // preceding glyph instead of dropping the caret.
            let (z, nach_letztem) = match ax_zeichen_rahmen(el, r.loc) {
                Some(z) => (z, false),
                None if r.loc > 0 => (ax_zeichen_rahmen(el, r.loc - 1)?, true),
                None => return None,
            };
            let feld = ax_rahmen(el)?;
            let x = insertion.map(|i| i.o.x)
                .unwrap_or_else(|| if nach_letztem { z.o.x + z.g.w.min(18.0) } else { z.o.x });
            // A Terminal cell is never a whole line. Preserve a genuine
            // narrow AX cell, otherwise use a neutral block width.
            // Some Terminal AX versions answer the zero-length parameterized
            // query with the remainder of the line (or the whole line), not a
            // caret cell.  Never turn that wide range into a fat fake cursor:
            // keep an honest single Terminal cell unless AX returned a small
            // insertion rectangle itself.
            let gemeldet = insertion.map(|i| i.g.w).unwrap_or(z.g.w);
            let breite = if (1.0..=18.0).contains(&gemeldet) {
                gemeldet.max(6.0)
            } else {
                7.0
            };
            let y = insertion.map(|i| i.o.y).unwrap_or(z.o.y);
            let hoehe = insertion.map(|i| i.g.h).unwrap_or(z.g.h).clamp(8.0, 40.0);
            let innen = x >= feld.o.x - 1.0 && x + breite <= feld.o.x + feld.g.w + 1.0
                && y >= feld.o.y - 1.0 && y + hoehe <= feld.o.y + feld.g.h + 1.0;
            innen.then(|| [x, y, breite, hoehe])
        }
    }

    pub fn menue_punkt_vorhanden(pid: i32, oben: &[&str], punkt: &[&str]) -> bool {
        unsafe {
            if !AXIsProcessTrusted() || pid <= 0 { return false; }
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return false; }
            let mut ok = false;
            if let Some(leiste) = ax_element_attr(app, "AXMenuBar") {
                if let Some(k) = ax_element_attr(leiste, "AXChildren") {
                    'suche: for i in 0..CFArrayGetCount(k) {
                        let top = CFArrayGetValueAtIndex(k, i) as *mut c_void;
                        if !oben.contains(&ax_text_attr(top, "AXTitle").as_str()) { continue; }
                        let Some(menues) = ax_element_attr(top, "AXChildren") else { continue };
                        for j in 0..CFArrayGetCount(menues) {
                            let m = CFArrayGetValueAtIndex(menues, j) as *mut c_void;
                            let Some(items) = ax_element_attr(m, "AXChildren") else { continue };
                            for n in 0..CFArrayGetCount(items) {
                                if punkt.contains(&ax_text_attr(CFArrayGetValueAtIndex(items, n) as *mut c_void, "AXTitle").as_str()) { ok = true; }
                            }
                            CFRelease(items as *const c_void);
                            if ok { CFRelease(menues as *const c_void); break 'suche; }
                        }
                        CFRelease(menues as *const c_void);
                    }
                    CFRelease(k as *const c_void);
                }
                CFRelease(leiste as *const c_void);
            }
            CFRelease(app as *const c_void);
            ok
        }
    }

    pub fn menue_punkt(pid: i32, oben: &[&str], punkt: &[&str]) -> bool {
        unsafe {
            if !AXIsProcessTrusted() || pid <= 0 { return false; }
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return false; }
            let mut ok = false;
            if let Some(leiste) = ax_element_attr(app, "AXMenuBar") {
                if let Some(k) = ax_element_attr(leiste, "AXChildren") {
                    'suche: for i in 0..CFArrayGetCount(k) {
                        let top = CFArrayGetValueAtIndex(k, i) as *mut c_void;
                        if !oben.contains(&ax_text_attr(top, "AXTitle").as_str()) { continue; }
                        let Some(menues) = ax_element_attr(top, "AXChildren") else { continue };
                        for j in 0..CFArrayGetCount(menues) {
                            let m = CFArrayGetValueAtIndex(menues, j) as *mut c_void;
                            let Some(items) = ax_element_attr(m, "AXChildren") else { continue };
                            for n in 0..CFArrayGetCount(items) {
                                let it = CFArrayGetValueAtIndex(items, n) as *mut c_void;
                                if punkt.contains(&ax_text_attr(it, "AXTitle").as_str()) {
                                    // Untermenue (z. B. Terminal: Neues Fenster > mit Profil):
                                    // dessen erster Eintrag ist der Standard.
                                    let unter = ax_element_attr(it, "AXChildren").and_then(|u| {
                                        let erster = (CFArrayGetCount(u) > 0).then(|| CFArrayGetValueAtIndex(u, 0) as *mut c_void)
                                            .and_then(|um| ax_element_attr(um, "AXChildren"))
                                            .and_then(|ei| {
                                                let e = (CFArrayGetCount(ei) > 0)
                                                    .then(|| CFRetain(CFArrayGetValueAtIndex(ei, 0)) as *mut c_void);
                                                CFRelease(ei as *const c_void);
                                                e
                                            });
                                        CFRelease(u as *const c_void);
                                        erster
                                    });
                                    ok = match unter {
                                        Some(e) => { let r = ax_aktion(e, "AXPress"); CFRelease(e as *const c_void); r }
                                        None => ax_aktion(it, "AXPress"),
                                    };
                                    CFRelease(items as *const c_void);
                                    CFRelease(menues as *const c_void);
                                    break 'suche;
                                }
                            }
                            CFRelease(items as *const c_void);
                        }
                        CFRelease(menues as *const c_void);
                    }
                    CFRelease(k as *const c_void);
                }
                CFRelease(leiste as *const c_void);
            }
            CFRelease(app as *const c_void);
            ok
        }
    }

    /// Das fokussierte Fenster des Programms (CGWindowID), falls AX es kennt.
    pub fn fokus_fenster(pid: i32) -> Option<i64> {
        unsafe {
            if !AXIsProcessTrusted() || pid <= 0 { return None; }
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return None; }
            let w = ax_element_attr(app, "AXFocusedWindow");
            CFRelease(app as *const c_void);
            let w = w?;
            let number_attr = cfstr("AXWindowNumber");
            let n = ax_fenster_nummer(w, number_attr);
            CFRelease(number_attr);
            CFRelease(w as *const c_void);
            n
        }
    }

    /// Titel genau dieses Fensters.
    pub fn fenster_titel(pid: i32, wid: i64) -> String {
        unsafe {
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return String::new(); }
            let t = ax_fenster_element(app, wid).map(|w| {
                let t = ax_text_attr(w, "AXTitle");
                CFRelease(w as *const c_void);
                t
            }).unwrap_or_default();
            CFRelease(app as *const c_void);
            t
        }
    }

    /// Adresse im AKTIVEN Tab GENAU dieses Browserfensters oeffnen.
    ///
    /// Gemessen (Chrome 153): Adressleiste = AXTextField ausserhalb der
    /// AXWebArea; AXValue setzen wirkt, AXConfirm nicht. Ein Leerzeichen am
    /// Ende unterdrueckt die Inline-Vervollstaendigung (sonst landete
    /// "youtube.com/" auf einem Video aus dem Verlauf). Return geht nur als
    /// PID-Ereignis und nur, nachdem AX nachweist: fokussiertes Fenster ist
    /// wid und Fokus liegt in genau diesem Feld - Chrome ist EIN Prozess fuer
    /// alle Fenster, ein ungeprueftes Return landete beim Nutzer.
    pub fn adresse_oeffnen(pid: i32, wid: i64, url: &str, marke: i64) -> bool {
        unsafe fn feld(el: *mut c_void, tiefe: usize) -> Option<*mut c_void> {
            if tiefe > 14 { return None; }
            let r = ax_rolle(el);
            if r == "AXWebArea" { return None; }
            if r == "AXTextField" || r == "AXComboBox" {
                return Some(CFRetain(el as *const c_void) as *mut c_void);
            }
            let k = ax_element_attr(el, "AXChildren")?;
            let mut t = None;
            for i in 0..CFArrayGetCount(k) {
                if let Some(f) = feld(CFArrayGetValueAtIndex(k, i) as *mut c_void, tiefe + 1) { t = Some(f); break; }
            }
            CFRelease(k as *const c_void);
            t
        }
        unsafe {
            if !AXIsProcessTrusted() || pid <= 0 || wid <= 0 { return false; }
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return false; }
            let Some(w) = ax_fenster_element(app, wid) else { CFRelease(app as *const c_void); return false };
            let mut ok = false;
            if let Some(f) = feld(w, 0) {
                let fokus = cfstr("AXFocused");
                let wert_attr = cfstr("AXValue");
                let wert = cfstr(&format!("{} ", url.trim()));
                let _ = AXUIElementSetAttributeValue(f, fokus, kCFBooleanTrue);
                let gesetzt = AXUIElementSetAttributeValue(f, wert_attr, wert) == 0;
                CFRelease(fokus); CFRelease(wert_attr); CFRelease(wert);
                if gesetzt {
                    let _ = ax_fenster_vorn(pid, wid);
                    for _ in 0..10 {
                        if ax_fokus_ist(app, wid, f) { ok = true; break; }
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                    if ok { taste_senden(pid, 36, 13, marke); }
                }
                CFRelease(f as *const c_void);
            }
            CFRelease(w as *const c_void);
            CFRelease(app as *const c_void);
            eprintln!("[FERNAX] address wid={wid} gated={ok}");
            ok
        }
    }

    /// Minimiertes Fenster zurueckholen (AXMinimized = false); ein
    /// ausgeblendetes Programm wieder einblenden (AXHidden = false) - beides
    /// ohne das Programm zu aktivieren (gemessen: vorderes Programm bleibt).
    pub fn fenster_zeigen(pid: i32, wid: i64) -> bool {
        unsafe {
            if !AXIsProcessTrusted() { return false; }
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return false; }
            let versteckt = cfstr("AXHidden");
            let _ = AXUIElementSetAttributeValue(app, versteckt, kCFBooleanFalse);
            CFRelease(versteckt);
            let mut ok = false;
            if let Some(w) = ax_fenster_element(app, wid) {
                let attr = cfstr("AXMinimized");
                ok = AXUIElementSetAttributeValue(w, attr, kCFBooleanFalse) == 0;
                CFRelease(attr);
                CFRelease(w as *const c_void);
            }
            CFRelease(app as *const c_void);
            ok
        }
    }

    /// Semantisches Kopieren/Einsetzen am nachgewiesen fokussierten Feld.
    /// Gemessen: an den Prozess gesendetes Cmd+C/V erreicht Chromes
    /// Webinhalt im Hintergrund nicht (Tastaturaequivalente brauchen die
    /// aktive App); AXSelectedText lesen/setzen wirkt dagegen verlaesslich.
    pub fn fokus_auswahl_text(pid: i32) -> Option<String> {
        unsafe {
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return None; }
            let f = ax_element_attr(app, "AXFocusedUIElement");
            CFRelease(app as *const c_void);
            let f = f?;
            let t = ax_text_attr_lang(f, "AXSelectedText");
            CFRelease(f as *const c_void);
            Some(t)
        }
    }

    /// Cmd-A for an inactive app: select the focused field's whole text via
    /// AX (a PID Cmd-A is dropped by an inactive app - measured, Finder search).
    pub fn fokus_alles_markieren(pid: i32) -> bool {
        unsafe {
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return false; }
            let f = ax_element_attr(app, "AXFocusedUIElement");
            CFRelease(app as *const c_void);
            let Some(f) = f else { return false };
            let n = ax_text_attr_lang(f, "AXValue").encode_utf16().count() as isize;
            #[repr(C)] struct CfRange { loc: isize, len: isize }
            let r = CfRange { loc: 0, len: n };
            let wert = AXValueCreate(4, &r as *const CfRange as *const c_void);
            let mut ok = false;
            if !wert.is_null() {
                let a = cfstr("AXSelectedTextRange");
                ok = AXUIElementSetAttributeValue(f, a, wert as *const c_void) == 0;
                CFRelease(a); CFRelease(wert as *const c_void);
            }
            CFRelease(f as *const c_void);
            ok
        }
    }

    /// Rolle des fokussierten Elements (fuer Einsetzen: mehrzeilig oder nicht).
    pub fn fokus_rolle(pid: i32) -> String {
        unsafe {
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return String::new(); }
            let f = ax_element_attr(app, "AXFocusedUIElement");
            CFRelease(app as *const c_void);
            let Some(f) = f else { return String::new() };
            let mut r = ax_rolle(f);
            if r == "AXStaticText" || ax_has_attr_value(f, "AXEditableAncestor") { r = "AXTextArea".into(); }
            CFRelease(f as *const c_void);
            r
        }
    }

    /// Text als Tastaturzeichen GENAU an diesen Prozess (wie getippt).
    /// Chrome ignoriert AXSelectedText-Schreibzugriffe im Webinhalt (gemessen:
    /// Erfolg gemeldet, keine Wirkung); getippte Zeichen wirken verlaesslich.
    pub fn text_an_prozess(pid: i32, text: &str, mehrzeilig: bool, marke: i64) -> bool {
        unsafe {
            let mut puffer: Vec<u16> = vec![];
            let senden = |zeichen: &[u16], code: u16| {
                for runter in [true, false] {
                    let ev = CGEventCreateKeyboardEvent(std::ptr::null(), code, runter);
                    if ev.is_null() { continue; }
                    if !zeichen.is_empty() { CGEventKeyboardSetUnicodeString(ev, zeichen.len(), zeichen.as_ptr()); }
                    // Ohne Quelle erbt das Ereignis die GEHALTENEN Modifikatoren
                    // (beim Einsetzen: Cmd) - aus Text wuerden Tastenkuerzel.
                    CGEventSetFlags(ev, 0);
                    CGEventSetIntegerValueField(ev, 42, marke);
                    CGEventPostToPid(pid, ev);
                    CFRelease(ev as *const c_void);
                }
                std::thread::sleep(std::time::Duration::from_millis(4));
            };
            for c in text.replace("\r\n", "\n").chars() {
                if c == '\n' || c == '\r' {
                    if !puffer.is_empty() { senden(&puffer, 0); puffer.clear(); }
                    if mehrzeilig { senden(&[13], 36); } else { senden(&[32], 49); }
                    continue;
                }
                let mut b = [0u16; 2];
                puffer.extend_from_slice(c.encode_utf16(&mut b));
                if puffer.len() >= 18 { senden(&puffer, 0); puffer.clear(); }
            }
            if !puffer.is_empty() { senden(&puffer, 0); }
            true
        }
    }

    /// Eine einzelne Taste (Keycode + Zeichen) an den Prozess.
    pub fn taste_senden(pid: i32, code: u16, zeichen: u16, marke: i64) {
        unsafe {
            for runter in [true, false] {
                let ev = CGEventCreateKeyboardEvent(std::ptr::null(), code, runter);
                if ev.is_null() { continue; }
                CGEventKeyboardSetUnicodeString(ev, 1, &zeichen);
                CGEventSetFlags(ev, 0);
                CGEventSetIntegerValueField(ev, 42, marke);
                CGEventPostToPid(pid, ev);
                CFRelease(ev as *const c_void);
            }
        }
    }

    pub fn fokus_auswahl_ersetzen(pid: i32, text: &str) -> bool {
        unsafe {
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return false; }
            let f = ax_element_attr(app, "AXFocusedUIElement");
            CFRelease(app as *const c_void);
            let Some(f) = f else { return false };
            let attr = cfstr("AXSelectedText");
            let wert = cfstr(text);
            let ok = AXUIElementSetAttributeValue(f, attr, wert) == 0;
            CFRelease(attr); CFRelease(wert); CFRelease(f as *const c_void);
            ok
        }
    }

    /// AXRaise + AXMain on exactly this window (only for windows on the
    /// Space the user stands on - e.g. arrival after an explicit visit).
    thread_local! {
        static FRIST: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
    }
    /// Time budget for ONE interaction operation: every AX tree walk stops
    /// when it is used up (measured: YouTube scroll walks 2.2-3.3 s blocked
    /// the single interaction worker - clicks/typing behind it looked dead).
    pub fn frist_setzen(ms: u64) {
        FRIST.with(|f| f.set(Some(std::time::Instant::now() + std::time::Duration::from_millis(ms))));
    }
    pub fn frist_loeschen() { FRIST.with(|f| f.set(None)); }
    fn zeit_um() -> bool { FRIST.with(|f| f.get().is_some_and(|t| std::time::Instant::now() >= t)) }

    /// Deadline cleanup is unconditional, including early returns and panic.
    pub struct FristGuard;
    impl FristGuard {
        pub fn neu(ms: u64) -> Self { frist_setzen(ms); Self }
    }
    impl Drop for FristGuard {
        fn drop(&mut self) { frist_loeschen(); }
    }

    /// Role + description of the first element at the point in EXACTLY this
    /// window (for routing browser-UI clicks).
    pub fn rolle_am_punkt(pid: i32, wid: i64, x: f64, y: f64) -> (String, String) {
        unsafe {
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return (String::new(), String::new()); }
            let kette = kette_am_punkt(app, wid, x, y);
            CFRelease(app as *const c_void);
            let r = kette.first().map(|e| (ax_rolle(*e), ax_text_attr(*e, "AXDescription"))).unwrap_or_default();
            for e in kette { CFRelease(e as *const c_void); }
            r
        }
    }

    /// Show `text` in the browser's address field (display only - no focus,
    /// so no app activation). Finds the first AXTextField outside the page.
    /// Chrome's omnibox of exactly this window (first text field outside the web area).
    unsafe fn omnibox(pid: i32, wid: i64) -> Option<*mut c_void> {
        unsafe fn suchen(el: *mut c_void, tiefe: usize) -> Option<*mut c_void> {
            if tiefe > 14 { return None; }
            let r = ax_rolle(el);
            if r == "AXWebArea" { return None; }
            if r == "AXTextField" { return Some(CFRetain(el as *const c_void) as *mut c_void); }
            let k = ax_element_attr(el, "AXChildren")?;
            let mut out = None;
            for i in 0..CFArrayGetCount(k as *const c_void) {
                if let Some(f) = suchen(CFArrayGetValueAtIndex(k as *const c_void, i) as *mut c_void, tiefe + 1) { out = Some(f); break; }
            }
            CFRelease(k as *const c_void);
            out
        }
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() { return None; }
        let w = ax_fenster_element(app, wid);
        CFRelease(app as *const c_void);
        let w = w?;
        let f = suchen(w, 0);
        CFRelease(w as *const c_void);
        f
    }

    /// (value, focused inside Chrome, selection) of the omnibox.
    pub fn adressfeld_lesen(pid: i32, wid: i64) -> Option<(String, bool, (isize, isize))> {
        unsafe {
            let f = omnibox(pid, wid)?;
            let wert = ax_text_attr(f, "AXValue");
            let fokus = ax_element_attr(f, "AXFocused").map(|v| {
                let b = v as *const c_void == kCFBooleanTrue; CFRelease(v as *const c_void); b
            }).unwrap_or(false);
            #[repr(C)] #[derive(Default)] struct CfRange { loc: isize, len: isize }
            let mut r = CfRange::default();
            if let Some(v) = ax_element_attr(f, "AXSelectedTextRange") {
                AXValueGetValue(v as *const c_void, 4, &mut r as *mut CfRange as *mut c_void);
                CFRelease(v as *const c_void);
            }
            CFRelease(f as *const c_void);
            Some((wert, fokus, (r.loc, r.len)))
        }
    }

    /// Press the browser-UI control (outside the web page) at this point -
    /// e.g. the OK/Cancel of a JS dialog, which blocks the page itself.
    pub fn browser_ui_druecken(pid: i32, wid: i64, x: f64, y: f64) -> Option<String> {
        unsafe {
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return None; }
            let kette = kette_am_punkt(app, wid, x, y);
            CFRelease(app as *const c_void);
            let im_web = kette.iter().any(|&e| ax_rolle(e) == "AXWebArea");
            let mut out = None;
            if !im_web {
                for &e in &kette {
                    let r = ax_rolle(e);
                    if matches!(r.as_str(), "AXButton" | "AXCheckBox" | "AXRadioButton") && ax_aktion(e, "AXPress") {
                        out = Some(format!("{r}:{}", ax_text_attr(e, "AXTitle")));
                        break;
                    }
                }
            }
            for e in kette { CFRelease(e as *const c_void); }
            out
        }
    }

    /// Select the Chrome tab titled `titel` in exactly this window via its
    /// AX tab button (AXPress on an inactive app's hidden window: no
    /// activation). DevTools' Target.activateTarget activates Chrome and
    /// switches the Space (measured 2026-09-27) - never used.
    pub fn browser_tab_waehlen(pid: i32, wid: i64, titel: &str) -> bool {
        unsafe fn sammeln(el: *mut c_void, tiefe: usize, out: &mut Vec<*mut c_void>) {
            if tiefe > 14 || out.len() > 200 { return; }
            let r = ax_rolle(el);
            if r == "AXWebArea" { return; }
            if r == "AXRadioButton" { out.push(CFRetain(el as *const c_void) as *mut c_void); return; }
            let Some(k) = ax_element_attr(el, "AXChildren") else { return };
            for i in 0..CFArrayGetCount(k as *const c_void) { sammeln(CFArrayGetValueAtIndex(k as *const c_void, i) as *mut c_void, tiefe + 1, out); }
            CFRelease(k as *const c_void);
        }
        unsafe {
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return false; }
            let w = ax_fenster_element(app, wid);
            CFRelease(app as *const c_void);
            let Some(w) = w else { return false };
            let mut tabs = Vec::new();
            sammeln(w, 0, &mut tabs);
            CFRelease(w as *const c_void);
            let mut ok = false;
            for &t in &tabs {
                if !ok {
                    let name = ax_text_attr(t, "AXTitle");
                    let name = if name.is_empty() { ax_text_attr(t, "AXDescription") } else { name };
                    if !titel.is_empty() && (name == titel || name.starts_with(titel)) { ok = ax_aktion(t, "AXPress"); }
                }
                CFRelease(t as *const c_void);
            }
            ok
        }
    }

    /// Focus the omnibox INSIDE Chrome. Measured 2026-09-27: a native
    /// views field (not web content) - no app activation, no Space switch.
    pub fn adressfeld_fokussieren(pid: i32, wid: i64) -> bool {
        unsafe {
            let Some(f) = omnibox(pid, wid) else { return false };
            let a = cfstr("AXFocused");
            let _ = AXUIElementSetAttributeValue(f, a, kCFBooleanTrue);
            CFRelease(a);
            CFRelease(f as *const c_void);
        }
        for _ in 0..6 {
            if adressfeld_lesen(pid, wid).is_some_and(|f| f.1) { return true; }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        false
    }

    pub fn adressfeld_auswahl(pid: i32, wid: i64, loc: isize, len: isize) -> bool {
        unsafe {
            let Some(f) = omnibox(pid, wid) else { return false };
            #[repr(C)] struct CfRange { loc: isize, len: isize }
            let r = CfRange { loc, len };
            let wert = AXValueCreate(4, &r as *const CfRange as *const c_void);
            let mut ok = false;
            if !wert.is_null() {
                let a = cfstr("AXSelectedTextRange");
                ok = AXUIElementSetAttributeValue(f, a, wert as *const c_void) == 0;
                CFRelease(a); CFRelease(wert as *const c_void);
            }
            CFRelease(f as *const c_void);
            ok
        }
    }

    /// A key GENAU an diesen Prozess: frisch erzeugt, ohne Modifikatoren,
    /// mit Zeichen (falls vorhanden), markiert.
    pub fn taste_mit_text(pid: i32, code: i64, text: &str) {
        unsafe {
            let utf: Vec<u16> = text.encode_utf16().collect();
            let stuecke: Vec<&[u16]> = if utf.is_empty() { vec![&[][..]] } else { utf.chunks(20).collect() };
            for stueck in stuecke {
                for runter in [true, false] {
                    let ev = CGEventCreateKeyboardEvent(std::ptr::null(), code as u16, runter);
                    if ev.is_null() { continue; }
                    CGEventSetFlags(ev, 0);
                    if !stueck.is_empty() { CGEventKeyboardSetUnicodeString(ev, stueck.len(), stueck.as_ptr()); }
                    CGEventSetIntegerValueField(ev, 42, crate::fern_tippen::MARKE);
                    CGEventPostToPid(pid, ev);
                    CFRelease(ev as *const c_void);
                }
            }
        }
    }

    pub fn adressfeld_setzen(pid: i32, wid: i64, text: &str, markiert: bool) -> bool {
        unsafe fn suchen(el: *mut c_void, tiefe: usize) -> Option<*mut c_void> {
            if tiefe > 14 { return None; }
            let r = ax_rolle(el);
            if r == "AXWebArea" { return None; }
            if r == "AXTextField" { return Some(CFRetain(el as *const c_void) as *mut c_void); }
            let k = ax_element_attr(el, "AXChildren")?;
            let mut out = None;
            for i in 0..CFArrayGetCount(k as *const c_void) {
                if let Some(f) = suchen(CFArrayGetValueAtIndex(k as *const c_void, i) as *mut c_void, tiefe + 1) { out = Some(f); break; }
            }
            CFRelease(k as *const c_void);
            out
        }
        unsafe {
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return false; }
            let mut ok = false;
            if let Some(w) = ax_fenster_element(app, wid) {
                if let Some(f) = suchen(w, 0) {
                    let a = cfstr("AXValue");
                    let v = cfstr(text);
                    ok = AXUIElementSetAttributeValue(f, a, v) == 0;
                    CFRelease(a); CFRelease(v);
                    // Display only: whole text selected, else caret at the end.
                    let n = text.encode_utf16().count() as isize;
                    #[repr(C)] struct CfRange { loc: isize, len: isize }
                    let bereich = if markiert { CfRange { loc: 0, len: n } } else { CfRange { loc: n, len: 0 } };
                    let wert = AXValueCreate(4, &bereich as *const CfRange as *const c_void);
                    if !wert.is_null() {
                        let a = cfstr("AXSelectedTextRange");
                        let _ = AXUIElementSetAttributeValue(f, a, wert as *const c_void);
                        CFRelease(a); CFRelease(wert as *const c_void);
                    }
                    CFRelease(f as *const c_void);
                }
                CFRelease(w as *const c_void);
            }
            CFRelease(app as *const c_void);
            ok
        }
    }

    /// One global AX messaging timeout for Noki (default is ~6 s per call).
    /// Measured: a click walk over a frozen hidden Chromium page (1950
    /// nodes) could block the single interaction worker for a long time -
    /// "only bring-to-front still works" (that runs on its own thread).
    pub fn ax_zeitlimit_setzen() {
        #[link(name = "ApplicationServices", kind = "framework")]
        extern "C" {
            fn AXUIElementCreateSystemWide() -> *mut c_void;
            fn AXUIElementSetMessagingTimeout(el: *mut c_void, t: f32) -> i32;
        }
        unsafe {
            let sys = AXUIElementCreateSystemWide();
            if sys.is_null() { return; }
            let r = AXUIElementSetMessagingTimeout(sys, 0.2);
            CFRelease(sys as *const c_void);
            crate::virtual_workspace::trace(&format!("[HEALTH] ax_messaging_timeout=0.2s set={}", r == 0));
        }
    }

    pub fn fenster_heben_und_main(pid: i32, wid: i64) -> bool {
        unsafe {
            if !AXIsProcessTrusted() || pid <= 0 || wid <= 0 { return false; }
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return false; }
            let mut ok = false;
            if let Some(w) = ax_fenster_element(app, wid) {
                let raise = cfstr("AXRaise");
                ok = AXUIElementPerformAction(w, raise) == 0;
                CFRelease(raise);
                let main_attr = cfstr("AXMain");
                let _ = AXUIElementSetAttributeValue(w, main_attr, kCFBooleanTrue);
                CFRelease(main_attr);
                CFRelease(w as *const c_void);
            }
            CFRelease(app as *const c_void);
            ok
        }
    }

    pub fn fenster_heben(pid: i32, wid: i64) -> bool {
        unsafe { ax_fenster_heben(pid, wid) }
    }

    /// Der SEMANTISCHE Weg: das Element an dieser Stelle betaetigt sich
    /// selbst. Wirkt ohne Schreibtischwechsel und ist deshalb der erste.
    pub fn ax_klick(pid: i32, x: f64, y: f64, klicks: i64) -> bool {
        unsafe { klicks <= 1 && ax_druecken(pid, x, y) }
    }

    pub fn ax_klick_generisch(pid: i32, x: f64, y: f64, klicks: i64) -> bool {
        unsafe { klicks <= 1 && ax_druecken_generisch(pid, x, y) }
    }

    /// Der ZEIGER-Weg: an den Prozess zugestellt, ohne den Systemzeiger zu
    /// bewegen. Wirkt gemessen nur, wenn das Fenster in diesem Moment
    /// wirklich sichtbar ist - der Aufrufer sorgt dafuer.
    pub fn zeiger_klick(pid: i32, wid: i64, x: f64, y: f64, klicks: i64) -> bool {
        unsafe { zeiger_an_prozess(pid, wid, x, y, klicks) };
        true
    }

    /// Hat Noki die Freigabe "Bedienungshilfen"? Ohne sie darf dieser
    /// Prozess weder Bedienungshilfen lesen noch Ereignisse erzeugen - dann
    /// ist Fernbedienung nicht moeglich, und das gehoert gesagt.
    pub fn vertraut() -> bool { unsafe { AXIsProcessTrusted() } }

    pub fn ax_rad(pid: i32, x: f64, y: f64, dx: i32, dy: i32) -> bool {
        unsafe { ax_rollen(pid, x, y, dx, dy) }
    }

    pub fn ax_rad_fenster(pid: i32, wid: i64, dx: i32, dy: i32) -> bool {
        unsafe { ax_rollen_fenster(pid, wid, dx, dy) }
    }

    pub fn ax_pfeil_scroll_fenster(pid: i32, wid: i64, dx: i32, dy: i32) -> bool {
        unsafe { ax_pfeil_scroll(pid, wid, dx, dy) }
    }

    pub fn ax_verschieben(pid: i32, wid: i64, x: f64, y: f64, w: f64, h: f64) -> bool {
        unsafe { ax_fenster_verschieben(pid, wid, x, y, w, h) }
    }

    /// Nur die Lage (Fenster ziehen): kein AXSize je Schritt - das kostete
    /// Zeit und schlug bei nicht skalierbaren Fenstern fehl, obwohl sich das
    /// Fenster bewegt hatte.
    pub fn ax_lage(pid: i32, wid: i64, x: f64, y: f64) -> bool {
        unsafe {
            if !AXIsProcessTrusted() || pid <= 0 || wid <= 0 { return false; }
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() { return false; }
            let mut ok = false;
            if let Some(w) = ax_fenster_element(app, wid) {
                let punkt = Punkt { x, y };
                let wert = AXValueCreate(1, &punkt as *const Punkt as *const c_void);
                let attr = cfstr("AXPosition");
                ok = !wert.is_null() && AXUIElementSetAttributeValue(w, attr, wert) == 0;
                if !wert.is_null() { CFRelease(wert); }
                CFRelease(attr);
                CFRelease(w as *const c_void);
            }
            CFRelease(app as *const c_void);
            ok
        }
    }

    pub fn fenster_vorn(pid: i32, wid: i64) -> bool {
        unsafe { ax_fenster_vorn(pid, wid) }
    }

    /// Reine Messung fuer die harte Fernbedienungs-Invariante. Kein
    /// Fernpfad darf diese globale Lage veraendern.
    pub fn zeigerstand() -> Punkt {
        unsafe {
            let ev = CGEventCreate(std::ptr::null());
            if ev.is_null() {
                return Punkt { x: 0.0, y: 0.0 };
            }
            // Ein neutrales CGEvent wird mit der aktuellen HID-Zeigerlage
            // erzeugt. Ein MouseEvent dagegen uebernimmt nur den beim
            // Erzeugen uebergebenen Punkt und waere keine Messung.
            let p = CGEventGetLocation(ev);
            CFRelease(ev as *const c_void);
            p
        }
    }
}
