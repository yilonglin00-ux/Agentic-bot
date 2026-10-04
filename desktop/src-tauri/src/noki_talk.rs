//! NOKI TALK - systemwide LOCAL dictation.
//!
//!   Option twice (the existing "Noki hoert zu" trigger) -> microphone at once
//!   -> live preview while speaking -> Option twice / Space -> final text is
//!   inserted into the text field that was focused when dictation started.
//!
//! * Capture: the persistent helper NokiTalk.app (talk/noki-talk.swift),
//!   started once, then only "start"/"stop" - no process per dictation; the
//!   microphone is open only while dictating.
//! * Recognition: whisper.cpp with Metal as a local `whisper-server` on
//!   127.0.0.1 (same pattern as Noki's llama.cpp router): started lazily,
//!   model kept warm between dictations, stopped after 20 min of real
//!   non-use. Audio never leaves this Mac; no cloud, no keys.
//! * Streaming: talk_kern::Segmentierer (energy VAD). Segments close at real
//!   pauses and are transcribed ONCE (stable); only the open tail is re-run
//!   for the preview.
//! * Insertion: Accessibility (AXSelectedText) when the field reports its
//!   value, else the clipboard (saved and restored) + Cmd+V. Never into a
//!   secure/password field.
//! * History: last 30 dictations (SQLite + one AAC file each).
use crate::talk_kern::{self as kern, Segment, Segmentierer, Verlauf};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};

const PORT: u16 = 8178;
const SERVER_LEERLAUF: Duration = Duration::from_secs(20 * 60);
const EREIGNIS: &str = "noki-talk";

fn home() -> PathBuf { PathBuf::from(std::env::var("HOME").unwrap_or_default()) }
fn noki_lokal() -> PathBuf { home().join("NOKI/.local") }
fn daten_dir() -> PathBuf { home().join("Library/Application Support/com.noki.desktop/NokiTalk") }
pub fn modell_dir() -> PathBuf { noki_lokal().join("whisper-models") }
fn trace(m: &str) { crate::virtual_workspace::trace(m); }
fn jetzt_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}
fn melden(app: &tauri::AppHandle, v: serde_json::Value) { let _ = app.emit(EREIGNIS, v); }

// ---- settings (einstellungen.json, key "noki_talk") --------------------------
#[derive(Clone)]
struct Einst { sprache: String, sprachen: Vec<String>, auto: bool, modell: String }
fn einst(app: &tauri::AppHandle) -> Einst {
    let v = crate::einstellungen_datei(app).and_then(|d| std::fs::read_to_string(d).ok())
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("noki_talk").cloned()).unwrap_or_default();
    // Quick selection (persisted; default DE/EN/FR/ZH). The active language
    // must be one of them - otherwise Auto (also after removing it).
    let sprachen = match v.get("sprachen").and_then(|s| s.as_array()) {
        Some(a) => kern::sprachen_bereinigen(&a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect::<Vec<_>>()),
        None => kern::STANDARD_SPRACHEN.iter().map(|s| s.to_string()).collect(),
    };
    let sprache = kern::aktive_sprache(v.get("sprache").and_then(|s| s.as_str()).unwrap_or("auto"), &sprachen);
    Einst {
        sprache,
        sprachen,
        auto: v.get("auto_einfuegen").and_then(|b| b.as_bool()).unwrap_or(true),
        modell: v.get("modell").and_then(|s| s.as_str()).unwrap_or("").to_string(),
    }
}

// ---- local model + whisper-server --------------------------------------------
/// Installed ggml models, best first (large-v3-turbo is the default).
pub fn modelle() -> Vec<PathBuf> {
    const RANG: [&str; 7] = ["large-v3-turbo-q5", "large-v3-turbo-q8", "large-v3-turbo", "medium", "small", "base", "tiny"];
    let mut v: Vec<PathBuf> = std::fs::read_dir(modell_dir()).into_iter().flatten().flatten().map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "bin")
            && p.file_name().is_some_and(|n| { let n = n.to_string_lossy().to_lowercase(); n.starts_with("ggml-") && !n.contains("vad") && !n.contains("silero") }))
        .collect();
    let rang = |p: &PathBuf| { let n = p.to_string_lossy().to_lowercase(); RANG.iter().position(|r| n.contains(r)).unwrap_or(RANG.len()) };
    v.sort_by_key(|p| (rang(p), p.clone()));
    v
}
fn modell_wahl(e: &Einst) -> Option<PathBuf> {
    let alle = modelle();
    alle.iter().find(|p| !e.modell.is_empty() && p.file_name().is_some_and(|n| n.to_string_lossy() == e.modell)).cloned()
        .or_else(|| alle.into_iter().next())
}
pub fn server_programm() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("NOKI_WHISPER_SERVER") { let p = PathBuf::from(p); if p.is_file() { return Some(p); } }
    [noki_lokal().join("whisper-src/build/bin/whisper-server"),
     PathBuf::from("/opt/homebrew/opt/whisper-cpp/bin/whisper-server"),
     PathBuf::from("/opt/homebrew/bin/whisper-server"),
     PathBuf::from("/usr/local/opt/whisper-cpp/bin/whisper-server"),
     PathBuf::from("/usr/local/bin/whisper-server")]
        .into_iter().find(|p| p.is_file())
}
struct Server { kind: std::process::Child, modell: PathBuf }
static SERVER: Mutex<Option<Server>> = Mutex::new(None);
static SERVER_GEN: AtomicU64 = AtomicU64::new(0);

/// Starts the server if it is not running (returns at once; loading the
/// model continues in the server process).
fn server_starten(modell: &Path) -> Result<(), String> {
    let mut g = SERVER.lock().map_err(|_| "Server-Zustand")?;
    if let Some(s) = g.as_mut() {
        if s.kind.try_wait().ok().flatten().is_none() && s.modell == modell { return Ok(()); }
        let _ = s.kind.kill(); let _ = s.kind.wait();
        *g = None;
    }
    if kern::gesund(PORT).is_some() {
        // A server from a previous Noki run (crash) still listens: reuse it.
        return Ok(());
    }
    let prog = server_programm().ok_or("whisper.cpp ist nicht installiert (Einstellungen › Noki Talk zeigt die Einrichtung).")?;
    let log = std::fs::OpenOptions::new().create(true).append(true).open(noki_lokal().join("whisper-server.log")).ok();
    let kerne = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8);
    let mut c = std::process::Command::new(&prog);
    c.arg("-m").arg(modell).args(["--host", "127.0.0.1", "--port", &PORT.to_string(), "-t", &kerne.to_string()])
        .stdin(std::process::Stdio::null());
    if let Some(l) = log { if let Ok(l2) = l.try_clone() { c.stdout(l).stderr(l2); } }
    let kind = c.spawn().map_err(|e| format!("whisper-server startet nicht: {e}"))?;
    trace(&format!("NOKI_TALK server_start model={} pid={}", modell.display(), kind.id()));
    *g = Some(Server { kind, modell: modell.to_path_buf() });
    Ok(())
}
/// Waits until the model is loaded (only while a dictation needs it).
fn server_bereit(frist: Duration, mut melde_laden: impl FnMut()) -> Result<(), String> {
    let t = Instant::now();
    let mut gemeldet = false;
    loop {
        match kern::gesund(PORT) {
            Some(true) => return Ok(()),
            _ if t.elapsed() > frist => return Err("Das lokale Whisper-Modell antwortet nicht.".into()),
            _ => {}
        }
        let tot = SERVER.lock().ok().and_then(|mut g| g.as_mut().map(|s| s.kind.try_wait().ok().flatten().is_some())).unwrap_or(true);
        if tot && kern::gesund(PORT).is_none() {
            return Err("whisper-server ist beendet (Modell defekt? Protokoll: ~/NOKI/.local/whisper-server.log)".into());
        }
        if !gemeldet { gemeldet = true; melde_laden(); }
        std::thread::sleep(Duration::from_millis(120));
    }
}
/// After real non-use the model is unloaded (one sleeper per dictation end;
/// a newer dictation invalidates it).
fn leerlauf_planen() {
    let gen = SERVER_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        std::thread::sleep(SERVER_LEERLAUF);
        if SERVER_GEN.load(Ordering::SeqCst) == gen && SITZUNG.lock().map(|g| g.is_none()).unwrap_or(false) {
            server_stoppen("idle");
        }
    });
}
fn server_stoppen(grund: &str) {
    if let Ok(mut g) = SERVER.lock() {
        if let Some(mut s) = g.take() {
            let _ = s.kind.kill(); let _ = s.kind.wait();
            trace(&format!("NOKI_TALK server_stop reason={grund}"));
        }
    }
}

// ---- capture helper (persistent) ---------------------------------------------
struct Helfer { eingang: std::fs::File, aus_pfad: PathBuf }
static HELFER: Mutex<Option<Helfer>> = Mutex::new(None);
/// Set by the thread that waits (blocking, no polling) for the helper to end.
static HELFER_LEBT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Last "ready"/"status" of the helper: (microphone status 0..3, device).
static MIKRO: Mutex<Option<(i64, String)>> = Mutex::new(None);

fn helfer_bundle() -> Option<PathBuf> {
    let b = std::env::current_exe().ok()?.parent()?.join("../Helpers/NokiTalk.app");
    if b.join("Contents/MacOS/noki-talk").is_file() { return Some(b); }
    let d = noki_lokal().join("talk/NokiTalk.app");
    d.join("Contents/MacOS/noki-talk").is_file().then_some(d)
}
fn fifo(p: &Path) -> Result<(), String> {
    let _ = std::fs::remove_file(p);
    let ok = std::process::Command::new("/usr/bin/mkfifo").arg(p).status().map(|s| s.success()).unwrap_or(false);
    if ok { Ok(()) } else { Err("Noki Talk: FIFO".into()) }
}
fn helfer_senden(zeile: &str) -> Result<(), String> {
    let mut g = HELFER.lock().map_err(|_| "Helfer")?;
    let h = g.as_mut().ok_or("Aufnahme-Helfer laeuft nicht")?;
    h.eingang.write_all(format!("{zeile}\n").as_bytes()).map_err(|e| e.to_string())
}
/// Launches NokiTalk.app once (LaunchServices: its own microphone permission).
/// Output comes through a FIFO: the reader blocks, no polling while idle.
fn helfer_sicherstellen(app: &tauri::AppHandle) -> Result<(), String> {
    let mut g = HELFER.lock().map_err(|_| "Helfer")?;
    if g.is_some() && HELFER_LEBT.load(Ordering::SeqCst) { return Ok(()); }
    *g = None;
    let bundle = helfer_bundle().ok_or("Noki Talk ist nicht gebaut (bauen.sh baut NokiTalk.app).")?;
    let dir = daten_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let ein = dir.join(format!("helfer-{}.in", std::process::id()));
    let aus = dir.join(format!("helfer-{}.out", std::process::id()));
    fifo(&ein)?; fifo(&aus)?;
    let mut launcher = std::process::Command::new("/usr/bin/open")
        .args(["-n", "-W", "-g", "-i"]).arg(&ein).arg("-o").arg(&aus).arg(&bundle)
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .spawn().map_err(|e| format!("Noki Talk startet nicht: {e}"))?;
    // O_RDWR: no FIFO-open deadlock while LaunchServices connects the ends.
    let eingang = std::fs::OpenOptions::new().read(true).write(true).open(&ein).map_err(|e| e.to_string())?;
    let lesen = std::fs::OpenOptions::new().read(true).write(true).open(&aus).map_err(|e| e.to_string())?;
    let weck = lesen.try_clone().map_err(|e| e.to_string())?;
    *g = Some(Helfer { eingang, aus_pfad: aus });
    HELFER_LEBT.store(true, Ordering::SeqCst);
    drop(g);
    let h = app.clone();
    std::thread::Builder::new().name("noki-talk-helper".into()).spawn(move || helfer_lesen(h, lesen)).map_err(|e| e.to_string())?;
    // `open -W` returns when the helper ends (crash, quit): then the blocked
    // reader is woken once. A blocking wait - nothing runs while it lives.
    std::thread::spawn(move || {
        let _ = launcher.wait();
        HELFER_LEBT.store(false, Ordering::SeqCst);
        let _ = (&weck).write_all(b"{\"state\":\"exited\"}\n");
    });
    Ok(())
}
fn helfer_lesen(app: tauri::AppHandle, f: std::fs::File) {
    let mut r = std::io::BufReader::new(f);
    let mut pegel_t = Instant::now() - Duration::from_secs(1);
    loop {
        let mut z = String::new();
        if r.read_line(&mut z).unwrap_or(0) == 0 { break; }
        let Ok(m) = serde_json::from_str::<serde_json::Value>(&z) else { continue };
        if let Some(b64) = m.get("pcm").and_then(|p| p.as_str()) {
            sitzung_senden(Msg::Pcm(kern::base64_pcm(b64)));
            if let Some(l) = m.get("level").and_then(|l| l.as_f64()) {
                // UI only on real level changes, ~16/s.
                if pegel_t.elapsed() >= Duration::from_millis(60) {
                    pegel_t = Instant::now();
                    melden(&app, serde_json::json!({ "pegel": (l * 100.0).round() / 100.0 }));
                }
            }
            continue;
        }
        if m.get("ready").is_some() {
            if let Ok(mut g) = MIKRO.lock() {
                *g = Some((m.get("mic").and_then(|v| v.as_i64()).unwrap_or(0), m.get("device").and_then(|v| v.as_str()).unwrap_or("").to_string()));
            }
        }
        match m.get("state").and_then(|s| s.as_str()) {
            Some("recording") => sitzung_senden(Msg::Aufnahme),
            Some("stopped") => sitzung_senden(Msg::Gestoppt),
            Some("exited") => { sitzung_senden(Msg::Fehler("Der Aufnahme-Helfer wurde beendet.".into())); break; }
            _ => {}
        }
        if let Some(e) = m.get("error").and_then(|e| e.as_str()) {
            if e == "mic_denied" { if let Ok(mut g) = MIKRO.lock() { *g = Some((2, String::new())); } }
            sitzung_senden(Msg::Fehler(match e {
                "mic_denied" => "mic_denied".to_string(),
                _ => format!("Aufnahme nicht möglich ({e})."),
            }));
        }
    }
    if let Ok(mut g) = HELFER.lock() {
        if let Some(h) = g.take() { let _ = std::fs::remove_file(&h.aus_pfad); }
    }
}

// ---- start / stop sound ----------------------------------------------------------
/// Short Noki chime (generated, see talk_kern::ton) - NSSound, async, quiet.
/// Prepared once per kind; never the system alert sound.
#[cfg(target_os = "macos")]
pub fn ton(start: bool) {
    use ax::*;
    static TOENE: Mutex<[usize; 2]> = Mutex::new([0, 0]);
    let i = if start { 0 } else { 1 };
    let Ok(mut g) = TOENE.lock() else { return };
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let m0: unsafe extern "C" fn(Id, Id) -> Id = std::mem::transmute(msg());
        let m1: unsafe extern "C" fn(Id, Id, Id) -> Id = std::mem::transmute(msg());
        let md: unsafe extern "C" fn(Id, Id, *const u8, usize) -> Id = std::mem::transmute(msg());
        let mv: unsafe extern "C" fn(Id, Id, f32) = std::mem::transmute(msg());
        if g[i] == 0 {
            let wav = kern::ton(start);
            let daten = md(klasse(b"NSData\0"), sel(b"dataWithBytes:length:\0"), wav.as_ptr(), wav.len());
            let s = m1(m0(klasse(b"NSSound\0"), sel(b"alloc\0")), sel(b"initWithData:\0"), daten);
            if !s.is_null() { mv(s, sel(b"setVolume:\0"), 0.55); g[i] = s as usize; }
        }
        if g[i] != 0 {
            let s = g[i] as Id;
            m0(s, sel(b"stop\0"));
            m0(s, sel(b"play\0"));
        }
        objc_autoreleasePoolPop(pool);
    }
}
#[cfg(not(target_os = "macos"))]
pub fn ton(_start: bool) {}

// ---- one dictation -------------------------------------------------------------
enum Msg { Pcm(Vec<i16>), Aufnahme, Stop, Abbruch, Gestoppt, Fehler(String) }
static SITZUNG: Mutex<Option<mpsc::Sender<Msg>>> = Mutex::new(None);
fn sitzung_senden(m: Msg) {
    if let Ok(g) = SITZUNG.lock() { if let Some(tx) = g.as_ref() { let _ = tx.send(m); } }
}
static VERLAUF: Mutex<Option<Verlauf>> = Mutex::new(None);
fn mit_verlauf<T>(f: impl FnOnce(&Verlauf) -> T) -> Option<T> {
    let mut g = VERLAUF.lock().ok()?;
    if g.is_none() { *g = Verlauf::oeffnen(&daten_dir()).map_err(|e| trace(&format!("NOKI_TALK history_open_failed {e}"))).ok(); }
    g.as_ref().map(f)
}

/// Option twice (via stimme_starten). Returns at once; all work runs on the
/// dictation thread, never on the main thread.
pub fn starten(app: &tauri::AppHandle) {
    let (tx, rx) = mpsc::channel();
    {
        let Ok(mut g) = SITZUNG.lock() else { return };
        if g.is_some() {
            // The previous dictation is still being written: no second
            // recording, and the panel must not claim "Noki hoert zu".
            drop(g);
            crate::stimme_extern_beendet();
            melden(app, serde_json::json!({ "zustand": "schreibt", "meldung": "Noki schreibt noch …" }));
            return;
        }
        *g = Some(tx);
    }
    let h = app.clone();
    let t0 = Instant::now();
    let _ = std::thread::Builder::new().name("noki-talk".into()).spawn(move || {
        diktat(&h, rx, t0);
        if let Ok(mut g) = SITZUNG.lock() { *g = None; }
        leerlauf_planen();
    });
}
/// Option twice again / Space (stop) or Esc (cancel).
pub fn stoppen(abbruch: bool) { sitzung_senden(if abbruch { Msg::Abbruch } else { Msg::Stop }); }
/// App exit: microphone closed, model unloaded.
pub fn alles_beenden() {
    let _ = helfer_senden("quit");
    server_stoppen("app_exit");
}

fn diktat(app: &tauri::AppHandle, rx: mpsc::Receiver<Msg>, t0: Instant) {
    let e = einst(app);
    let ziel = ziel_erfassen();
    melden(app, serde_json::json!({ "zustand": "hoert", "text": "" }));
    let audio = daten_dir().join("audio").join(format!("{}.m4a", jetzt_ms()));
    if let Err(f) = helfer_sicherstellen(app).and_then(|_| { let _ = std::fs::create_dir_all(audio.parent().unwrap()); helfer_senden(&format!("start {}", audio.display())) }) {
        crate::stimme_extern_beendet();
        return melden(app, serde_json::json!({ "zustand": "fehler", "meldung": f }));
    }
    // Model warm-up starts NOW, in parallel to speaking.
    let modell = modell_wahl(&e);
    let mut server_fehler = match &modell {
        Some(m) => server_starten(m).err(),
        None => Some(format!("Kein lokales Whisper-Modell in {}.", modell_dir().display())),
    };
    let mut seg = Segmentierer::default();
    let mut text = String::new();
    let mut offen_seit: Vec<Segment> = Vec::new();
    let mut sprache = e.sprache.clone();
    let mut erkannt = String::new();
    let (mut bereit, mut bereit_t) = (false, Instant::now() - Duration::from_secs(1));
    let (mut vorschau_t, mut vorschau_bis) = (Instant::now(), 0usize);
    let mut aufnahme_ms: Option<u64> = None;
    let mut stop_t: Option<Instant> = None;
    let mut abbruch = false;
    let mut fehler: Option<String> = None;
    let transkr = |pcm: &[i16], spr: &str, prompt: &str| kern::transkribieren(PORT, pcm, spr, prompt, Duration::from_secs(30));
    loop {
        let m = match rx.recv_timeout(Duration::from_secs(120)) {
            Ok(m) => m,
            Err(mpsc::RecvTimeoutError::Timeout) => { fehler = Some("Keine Audiodaten vom Mikrofon.".into()); let _ = helfer_senden("cancel"); break; }
            Err(_) => break,
        };
        match m {
            Msg::Aufnahme => {
                let ms = t0.elapsed().as_millis() as u64;
                aufnahme_ms = Some(ms);
                trace(&format!("NOKI_TALK_TIMING shortcut_to_recording_ms={ms}"));
            }
            Msg::Pcm(p) => {
                for s in seg.push(&p) { offen_seit.push(s); }
                if stop_t.is_some() { continue; }
                if !bereit && server_fehler.is_none() && bereit_t.elapsed() > Duration::from_millis(400) {
                    bereit_t = Instant::now();
                    bereit = kern::gesund(PORT) == Some(true);
                }
                if !bereit { continue; }
                // Confirmed segments: transcribed exactly once, stable.
                for s in std::mem::take(&mut offen_seit) {
                    if let Ok(a) = transkr(&seg.audio[s.von..s.bis], &sprache, &prompt(&text)) {
                        if sprache == "auto" && !a.sprache.is_empty() { erkannt = a.sprache.clone(); sprache = a.sprache.clone(); }
                        text = kern::anfuegen(&text, &kern::segment_bereinigen(&a.text), s.ueberlapp);
                    }
                }
                // Preview of the open tail (may still change).
                if let Some(o) = seg.offen() {
                    if vorschau_t.elapsed() >= Duration::from_millis(650) && seg.audio.len() >= vorschau_bis + kern::RATE / 2 {
                        vorschau_t = Instant::now(); vorschau_bis = seg.audio.len();
                        if let Ok(a) = transkr(&seg.audio[o.von..o.bis], &sprache, &prompt(&text)) {
                            let v = kern::anfuegen(&text, &kern::segment_bereinigen(&a.text), o.ueberlapp);
                            melden(app, serde_json::json!({ "zustand": "hoert", "text": kern::abschliessen(&v) }));
                            continue;
                        }
                    }
                }
                melden(app, serde_json::json!({ "zustand": "hoert", "text": kern::abschliessen(&text) }));
            }
            Msg::Stop => {
                if stop_t.is_none() { stop_t = Some(Instant::now()); let _ = helfer_senden("stop"); }
                melden(app, serde_json::json!({ "zustand": "schreibt" }));
            }
            Msg::Abbruch => { abbruch = true; let _ = helfer_senden("cancel"); }
            Msg::Gestoppt => break,
            Msg::Fehler(f) => { fehler = Some(f); let _ = helfer_senden("cancel"); break; }
        }
    }
    crate::stimme_extern_beendet();
    if abbruch || fehler.is_some() {
        let _ = std::fs::remove_file(&audio);
        let meldung = match fehler.as_deref() {
            Some("mic_denied") => serde_json::json!({ "zustand": "fehler", "meldung": "Noki Talk braucht die Mikrofon-Freigabe.", "aktion": "mikrofon" }),
            Some(f) => serde_json::json!({ "zustand": "fehler", "meldung": f }),
            None => serde_json::json!({ "zustand": "aus" }),
        };
        return melden(app, meldung);
    }
    let stop_t = stop_t.unwrap_or_else(Instant::now);
    // ---- final transcript: pending segments + the open tail
    if seg.sprache_gesamt_ms < 200 {
        let _ = std::fs::remove_file(&audio);
        return melden(app, serde_json::json!({ "zustand": "leer", "meldung": "Nichts verstanden" }));
    }
    if server_fehler.is_none() && !bereit {
        server_fehler = server_bereit(Duration::from_secs(90), || melden(app, serde_json::json!({ "zustand": "schreibt", "meldung": "Lokales Modell lädt …" }))).err();
    }
    if let Some(f) = server_fehler {
        let _ = std::fs::remove_file(&audio);
        return melden(app, serde_json::json!({ "zustand": "fehler", "meldung": f, "aktion": "einrichten" }));
    }
    let mut rest = std::mem::take(&mut offen_seit);
    rest.extend(seg.offen());
    for s in rest {
        match transkr(&seg.audio[s.von..s.bis.min(seg.audio.len())], &sprache, &prompt(&text)) {
            Ok(a) => {
                if sprache == "auto" && !a.sprache.is_empty() { erkannt = a.sprache.clone(); sprache = a.sprache.clone(); }
                text = kern::anfuegen(&text, &kern::segment_bereinigen(&a.text), s.ueberlapp);
            }
            Err(f) => trace(&format!("NOKI_TALK final_segment_failed {f}")),
        }
    }
    let text = kern::abschliessen(&text);
    let final_ms = stop_t.elapsed().as_millis() as u64;
    if text.is_empty() {
        let _ = std::fs::remove_file(&audio);
        return melden(app, serde_json::json!({ "zustand": "leer", "meldung": "Nichts verstanden" }));
    }
    // ---- insert, then history (the text is never lost)
    let t_ein = Instant::now();
    let ergebnis = if e.auto { einsetzen(app, &ziel, &text) } else { kopieren(app, &text); Ergebnis::Kopiert };
    let ein_ms = t_ein.elapsed().as_millis() as u64;
    trace(&format!("NOKI_TALK_TIMING recording_ms={} stop_to_final_ms={final_ms} final_to_inserted_ms={ein_ms} result={} app={} chars={}",
        aufnahme_ms.unwrap_or(0), ergebnis.wort(), ziel.bundle, text.chars().count()));
    let dauer = seg.dauer_ms() as i64;
    let gespeichert = mit_verlauf(|v| v.speichern(jetzt_ms(), dauer, &text, &audio, if erkannt.is_empty() { &sprache } else { &erkannt }, &ziel.name));
    if !matches!(gespeichert, Some(Ok(_))) {
        trace(&format!("NOKI_TALK history_save_failed {:?}", gespeichert.map(|r| r.err())));
        let _ = std::fs::remove_file(&audio);
    }
    melden(app, serde_json::json!({ "zustand": "fertig", "text": text, "meldung": ergebnis.meldung(), "verlauf": true }));
}
fn prompt(t: &str) -> String {
    let n = t.chars().count();
    t.chars().skip(n.saturating_sub(200)).collect()
}

// ---- target field + insertion -------------------------------------------------
#[derive(Default)]
struct Ziel { pid: i32, bundle: String, name: String, element: usize, sicher: bool, editierbar: Option<bool> }
unsafe impl Send for Ziel {}
impl Drop for Ziel {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        if self.element != 0 { unsafe { ax::CFRelease(self.element as ax::Id) } }
    }
}
enum Ergebnis { Ax, Paste, Kopiert, Passwort }
impl Ergebnis {
    fn wort(&self) -> &'static str { match self { Ergebnis::Ax => "ax", Ergebnis::Paste => "paste", Ergebnis::Kopiert => "copied", Ergebnis::Passwort => "secure_field" } }
    fn meldung(&self) -> &'static str {
        match self { Ergebnis::Ax | Ergebnis::Paste => "", Ergebnis::Kopiert => "Text kopiert", Ergebnis::Passwort => "Passwortfeld – Text nicht eingesetzt (im Verlauf)" }
    }
}
fn kopieren(app: &tauri::AppHandle, text: &str) {
    let _ = crate::lesezeichen::pb_text_setzen(text);
    // A user-visible copy belongs into the clipboard history like any copy.
    let _ = app;
}

#[cfg(not(target_os = "macos"))]
fn ziel_erfassen() -> Ziel { Ziel::default() }
#[cfg(not(target_os = "macos"))]
fn einsetzen(app: &tauri::AppHandle, _: &Ziel, text: &str) -> Ergebnis { kopieren(app, text); Ergebnis::Kopiert }

#[cfg(target_os = "macos")]
mod ax {
    #![allow(clashing_extern_declarations)]
    pub type Id = *mut std::ffi::c_void;
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        pub fn AXUIElementCreateSystemWide() -> Id;
        pub fn AXUIElementCopyAttributeValue(e: Id, a: Id, v: *mut Id) -> i32;
        pub fn AXUIElementSetAttributeValue(e: Id, a: Id, v: Id) -> i32;
        pub fn AXUIElementIsAttributeSettable(e: Id, a: Id, s: *mut u8) -> i32;
        pub fn AXUIElementSetMessagingTimeout(e: Id, t: f32) -> i32;
        pub fn AXUIElementGetPid(e: Id, pid: *mut i32) -> i32;
        pub fn CGEventCreateKeyboardEvent(src: *const std::ffi::c_void, key: u16, down: bool) -> *const std::ffi::c_void;
        pub fn CGEventSetFlags(ev: *const std::ffi::c_void, flags: u64);
        pub fn CGEventPost(tap: u32, ev: *const std::ffi::c_void);
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        pub fn CFRelease(x: Id);
        pub fn CFStringCreateWithCString(a: Id, s: *const i8, e: u32) -> Id;
        pub fn CFStringGetCString(s: Id, b: *mut i8, n: isize, e: u32) -> bool;
        pub fn CFStringGetLength(s: Id) -> isize;
        pub fn CFGetTypeID(x: Id) -> usize;
        pub fn CFStringGetTypeID() -> usize;
        pub static kCFBooleanTrue: Id;
    }
    #[link(name = "Carbon", kind = "framework")]
    extern "C" { pub fn IsSecureEventInputEnabled() -> u8; }
    #[link(name = "objc")]
    extern "C" {
        pub fn objc_getClass(n: *const i8) -> Id;
        pub fn sel_registerName(n: *const i8) -> Id;
        pub fn objc_msgSend();
        pub fn objc_autoreleasePoolPush() -> Id;
        pub fn objc_autoreleasePoolPop(p: Id);
    }
    pub const UTF8: u32 = 0x0800_0100;
    pub struct Cf(pub Id);
    impl Drop for Cf { fn drop(&mut self) { if !self.0.is_null() { unsafe { CFRelease(self.0) } } } }
    pub fn cfs(s: &str) -> Cf {
        let c = std::ffi::CString::new(s.replace('\0', "")).unwrap_or_default();
        Cf(unsafe { CFStringCreateWithCString(std::ptr::null_mut(), c.as_ptr(), UTF8) })
    }
    pub unsafe fn attr(e: Id, a: &str) -> Cf {
        let k = cfs(a); let mut v: Id = std::ptr::null_mut();
        if AXUIElementCopyAttributeValue(e, k.0, &mut v) != 0 { v = std::ptr::null_mut(); }
        Cf(v)
    }
    pub unsafe fn text(v: Id) -> Option<String> {
        if v.is_null() || CFGetTypeID(v) != CFStringGetTypeID() { return None; }
        let n = (CFStringGetLength(v) * 4 + 16) as usize;
        let mut b = vec![0i8; n];
        CFStringGetCString(v, b.as_mut_ptr(), n as isize, UTF8).then(|| std::ffi::CStr::from_ptr(b.as_ptr()).to_string_lossy().into_owned())
    }
    pub unsafe fn settable(e: Id, a: &str) -> bool { let k = cfs(a); let mut s = 0u8; AXUIElementIsAttributeSettable(e, k.0, &mut s) == 0 && s != 0 }
    pub fn sicher_eingabe() -> bool { unsafe { IsSecureEventInputEnabled() != 0 } }
    pub fn msg() -> unsafe extern "C" fn() { objc_msgSend }
    pub fn sel(n: &[u8]) -> Id { unsafe { sel_registerName(n.as_ptr() as *const i8) } }
    pub fn klasse(n: &[u8]) -> Id { unsafe { objc_getClass(n.as_ptr() as *const i8) } }
}

/// The focused field of the frontmost app, captured BEFORE anything else
/// (Noki never takes the keyboard focus).
#[cfg(target_os = "macos")]
fn ziel_erfassen() -> Ziel {
    use ax::*;
    let pid = crate::blende::vorn_pid();
    let eigen = std::process::id() as i32;
    let (name, bundle, _) = crate::lesezeichen::app_fuer_pid(pid).unwrap_or_default();
    let mut z = Ziel { pid, bundle, name, sicher: sicher_eingabe(), ..Default::default() };
    if pid <= 0 || pid == eigen { z.pid = 0; return z; }
    unsafe {
        let sys = Cf(AXUIElementCreateSystemWide());
        AXUIElementSetMessagingTimeout(sys.0, 0.5);
        let fokus = attr(sys.0, "AXFocusedUIElement");
        if fokus.0.is_null() { return z; } // Electron & co.: unknown -> paste
        let mut fp = 0;
        if AXUIElementGetPid(fokus.0, &mut fp) == 0 && fp != pid { return z; }
        let rolle = text(attr(fokus.0, "AXRole").0).unwrap_or_default();
        let sub = text(attr(fokus.0, "AXSubrole").0).unwrap_or_default();
        z.sicher |= rolle == "AXSecureTextField" || sub == "AXSecureTextField";
        z.editierbar = if ["AXTextField", "AXTextArea", "AXComboBox", "AXSearchField"].contains(&rolle.as_str())
            || !attr(fokus.0, "AXSelectedTextRange").0.is_null() { Some(true) }
            else if ["AXButton", "AXList", "AXOutline", "AXTable", "AXImage", "AXMenuItem", "AXCheckBox", "AXRadioButton", "AXSlider", "AXMenuBar", "AXDockItem"].contains(&rolle.as_str()) { Some(false) }
            else { None };
        let e = fokus.0; std::mem::forget(fokus);
        z.element = e as usize;
    }
    trace(&format!("NOKI_TALK target app={} pid={} editable={:?} secure={}", z.bundle, z.pid, z.editierbar, z.sicher));
    z
}

#[cfg(target_os = "macos")]
fn einsetzen(app: &tauri::AppHandle, z: &Ziel, inhalt: &str) -> Ergebnis {
    use ax::*;
    // Never into a password field - checked at start AND now.
    if z.sicher || sicher_eingabe() { return Ergebnis::Passwort; }
    if z.pid == 0 || z.editierbar == Some(false) { kopieren(app, inhalt); return Ergebnis::Kopiert; }
    unsafe {
        let el = z.element as Id;
        // 1. Accessibility: replace the selection at the caret - only where
        //    the field reports its value, so success is verifiable (no
        //    double insert by a following paste).
        if !el.is_null() && crate::blende::vorn_pid() == z.pid && settable(el, "AXSelectedText") {
            if let Some(vorher) = text_von(el) {
                let t = cfs(inhalt);
                if AXUIElementSetAttributeValue(el, cfs("AXSelectedText").0, t.0) == 0 {
                    if let Some(nachher) = text_von(el) {
                        if nachher != vorher && nachher.contains(inhalt.trim()) { return Ergebnis::Ax; }
                    }
                }
                if text_von(el).is_some_and(|n| n != vorher) { return Ergebnis::Ax; } // changed, though normalised
            }
        }
        // 2. Clipboard + Cmd+V into the original app (brought back if the
        //    user switched away meanwhile).
        if crate::blende::vorn_pid() != z.pid {
            aktivieren(z.pid);
            let bis = Instant::now() + Duration::from_millis(700);
            while crate::blende::vorn_pid() != z.pid && Instant::now() < bis { std::thread::sleep(Duration::from_millis(20)); }
            if crate::blende::vorn_pid() != z.pid { kopieren(app, inhalt); return Ergebnis::Kopiert; }
            if !el.is_null() { let _ = AXUIElementSetAttributeValue(el, cfs("AXFocused").0, kCFBooleanTrue); }
        }
        if sicher_eingabe() { return Ergebnis::Passwort; }
        let sicherung = pb_sichern();
        if !crate::lesezeichen::pb_text_setzen(inhalt) { return Ergebnis::Kopiert; }
        let unser = crate::lesezeichen::pb_zaehler();
        app.state::<crate::Clip>().eigen.store(unser, Ordering::Relaxed);
        for (taste, runter) in [(9u16, true), (9, false)] {
            let ev = CGEventCreateKeyboardEvent(std::ptr::null(), taste, runter);
            if !ev.is_null() { CGEventSetFlags(ev, 1 << 20); CGEventPost(0, ev); CFRelease(ev as Id); }
        }
        // The user's clipboard comes back once the app has read the paste
        // (unless the user copied something new in between).
        let h = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(450));
            if crate::lesezeichen::pb_zaehler() == unser {
                pb_wiederherstellen(&sicherung);
                h.state::<crate::Clip>().eigen.store(crate::lesezeichen::pb_zaehler(), Ordering::Relaxed);
            }
        });
        Ergebnis::Paste
    }
}
#[cfg(target_os = "macos")]
unsafe fn text_von(el: ax::Id) -> Option<String> { ax::text(ax::attr(el, "AXValue").0) }
#[cfg(target_os = "macos")]
unsafe fn aktivieren(pid: i32) {
    use ax::*;
    let pool = objc_autoreleasePoolPush();
    let mp: unsafe extern "C" fn(Id, Id, i32) -> Id = std::mem::transmute(msg());
    let mu: unsafe extern "C" fn(Id, Id, u64) -> bool = std::mem::transmute(msg());
    let a = mp(klasse(b"NSRunningApplication\0"), sel(b"runningApplicationWithProcessIdentifier:\0"), pid);
    if !a.is_null() { mu(a, sel(b"activateWithOptions:\0"), 0); }
    objc_autoreleasePoolPop(pool);
}
/// Every item and type of the general pasteboard, as data.
#[cfg(target_os = "macos")]
fn pb_sichern() -> Vec<Vec<(String, Vec<u8>)>> {
    use ax::*;
    let mut out = Vec::new();
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let m0: unsafe extern "C" fn(Id, Id) -> Id = std::mem::transmute(msg());
        let m1: unsafe extern "C" fn(Id, Id, Id) -> Id = std::mem::transmute(msg());
        let mi: unsafe extern "C" fn(Id, Id) -> usize = std::mem::transmute(msg());
        let mu: unsafe extern "C" fn(Id, Id, usize) -> Id = std::mem::transmute(msg());
        let mc: unsafe extern "C" fn(Id, Id) -> *const i8 = std::mem::transmute(msg());
        let mb: unsafe extern "C" fn(Id, Id) -> *const u8 = std::mem::transmute(msg());
        let pb = m0(klasse(b"NSPasteboard\0"), sel(b"generalPasteboard\0"));
        let items = m0(pb, sel(b"pasteboardItems\0"));
        let n = if items.is_null() { 0 } else { mi(items, sel(b"count\0")) };
        for i in 0..n {
            let it = mu(items, sel(b"objectAtIndex:\0"), i);
            let typen = m0(it, sel(b"types\0"));
            let mut eintrag = Vec::new();
            for j in 0..if typen.is_null() { 0 } else { mi(typen, sel(b"count\0")) } {
                let t = mu(typen, sel(b"objectAtIndex:\0"), j);
                let d = m1(it, sel(b"dataForType:\0"), t);
                if d.is_null() { continue; }
                let len = mi(d, sel(b"length\0"));
                let p = mb(d, sel(b"bytes\0"));
                let c = mc(t, sel(b"UTF8String\0"));
                if c.is_null() { continue; }
                let bytes = if p.is_null() || len == 0 { Vec::new() } else { std::slice::from_raw_parts(p, len).to_vec() };
                eintrag.push((std::ffi::CStr::from_ptr(c).to_string_lossy().into_owned(), bytes));
            }
            out.push(eintrag);
        }
        objc_autoreleasePoolPop(pool);
    }
    out
}
#[cfg(target_os = "macos")]
fn pb_wiederherstellen(sicherung: &[Vec<(String, Vec<u8>)>]) {
    use ax::*;
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let m0: unsafe extern "C" fn(Id, Id) -> Id = std::mem::transmute(msg());
        let m1: unsafe extern "C" fn(Id, Id, Id) -> Id = std::mem::transmute(msg());
        let m2: unsafe extern "C" fn(Id, Id, Id, Id) -> bool = std::mem::transmute(msg());
        let md: unsafe extern "C" fn(Id, Id, *const u8, usize) -> Id = std::mem::transmute(msg());
        let ms: unsafe extern "C" fn(Id, Id, *const i8) -> Id = std::mem::transmute(msg());
        let pb = m0(klasse(b"NSPasteboard\0"), sel(b"generalPasteboard\0"));
        m0(pb, sel(b"clearContents\0"));
        let arr = m0(klasse(b"NSMutableArray\0"), sel(b"array\0"));
        for eintrag in sicherung {
            let it = m0(m0(klasse(b"NSPasteboardItem\0"), sel(b"alloc\0")), sel(b"init\0"));
            for (typ, daten) in eintrag {
                let c = std::ffi::CString::new(typ.as_str()).unwrap_or_default();
                let t = ms(klasse(b"NSString\0"), sel(b"stringWithUTF8String:\0"), c.as_ptr());
                let d = md(klasse(b"NSData\0"), sel(b"dataWithBytes:length:\0"), daten.as_ptr(), daten.len());
                m2(it, sel(b"setData:forType:\0"), d, t);
            }
            m1(arr, sel(b"addObject:\0"), it);
            m0(it, sel(b"release\0"));
        }
        if !sicherung.is_empty() { m1(pb, sel(b"writeObjects:\0"), arr); }
        objc_autoreleasePoolPop(pool);
    }
}
#[cfg(not(target_os = "macos"))]
#[allow(dead_code)]
fn pb_sichern() -> Vec<Vec<(String, Vec<u8>)>> { Vec::new() }

// ---- Tauri commands (Settings › Noki Talk) -------------------------------------
#[tauri::command]
pub async fn noki_talk_status(app: tauri::AppHandle) -> serde_json::Value {
    tauri::async_runtime::spawn_blocking(move || {
        let e = einst(&app);
        let modell = modell_wahl(&e);
        let laeuft = SERVER.lock().ok().is_some_and(|g| g.is_some());
        let server = match kern::gesund(PORT) { Some(true) => "bereit", Some(false) => "laedt", None if laeuft => "startet", None => "aus" };
        let mikro = MIKRO.lock().ok().and_then(|g| g.clone());
        serde_json::json!({
            "server": server,
            "programm": server_programm().map(|p| p.display().to_string()),
            "modell": modell.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()),
            "modelle": modelle().iter().filter_map(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).collect::<Vec<_>>(),
            "modell_dir": modell_dir().display().to_string(),
            "helfer": helfer_bundle().is_some(),
            "mikrofon": mikro.as_ref().map(|m| m.0),
            "geraet": mikro.map(|m| m.1).unwrap_or_default(),
            "sprache": e.sprache, "sprachen": e.sprachen, "katalog": &kern::WHISPER_SPRACHEN[..],
            "auto_einfuegen": e.auto, "modell_wahl": e.modell,
            "laeuft": SITZUNG.lock().ok().is_some_and(|g| g.is_some()),
        })
    }).await.unwrap_or_default()
}
#[tauri::command]
pub fn noki_talk_einstellung(app: tauri::AppHandle, schluessel: String, wert: serde_json::Value) {
    let alt = crate::einstellungen_datei(&app).and_then(|d| std::fs::read_to_string(d).ok())
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("noki_talk").cloned()).unwrap_or_else(|| serde_json::json!({}));
    let mut neu = if alt.is_object() { alt } else { serde_json::json!({}) };
    if ["sprache", "auto_einfuegen", "modell", "sprachen"].contains(&schluessel.as_str()) { neu[schluessel.as_str()] = wert; }
    // Removing the active language from the quick selection -> Auto.
    if let Some(a) = neu.get("sprachen").and_then(|s| s.as_array()) {
        let schnell = kern::sprachen_bereinigen(&a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect::<Vec<_>>());
        let aktiv = kern::aktive_sprache(neu.get("sprache").and_then(|s| s.as_str()).unwrap_or("auto"), &schnell);
        neu["sprachen"] = serde_json::json!(schnell);
        neu["sprache"] = serde_json::json!(aktiv);
    }
    crate::einstellungen_setzen(&app, "noki_talk", neu);
}
/// Explicit warm-up / repair from Settings (starts the server now).
#[tauri::command]
pub async fn noki_talk_engine_starten(app: tauri::AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let m = modell_wahl(&einst(&app)).ok_or_else(|| format!("Kein lokales Whisper-Modell in {}.", modell_dir().display()))?;
        server_starten(&m)?;
        leerlauf_planen();
        Ok(())
    }).await.map_err(|e| e.to_string())?
}
#[tauri::command]
pub async fn noki_talk_verlauf() -> Vec<kern::Eintrag> {
    tauri::async_runtime::spawn_blocking(|| mit_verlauf(|v| v.liste()).unwrap_or_default()).await.unwrap_or_default()
}
/// The local recording of one entry as a data URL (AAC, a few hundred KB).
#[tauri::command]
pub async fn noki_talk_audio(id: i64) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let e = mit_verlauf(|v| v.holen(id)).flatten().ok_or("Eintrag nicht gefunden")?;
        let daten = std::fs::read(&e.audio_path).map_err(|_| "Audio nicht gefunden".to_string())?;
        Ok(format!("data:audio/mp4;base64,{}", kern::base64_kodieren(&daten)))
    }).await.map_err(|e| e.to_string())?
}
#[tauri::command]
pub fn noki_talk_kopieren(app: tauri::AppHandle, id: i64) -> bool {
    match mit_verlauf(|v| v.holen(id)).flatten() { Some(e) => { kopieren(&app, &e.transcript); true } None => false }
}
#[tauri::command]
pub fn noki_talk_loeschen(id: i64) -> bool { mit_verlauf(|v| v.loeschen(id)).unwrap_or(false) }
#[tauri::command]
pub fn noki_talk_mikrofon_freigabe() {
    let _ = std::process::Command::new("/usr/bin/open").arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone").spawn();
}
