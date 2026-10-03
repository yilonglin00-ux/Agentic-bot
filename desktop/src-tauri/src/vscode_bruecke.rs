//! Bridge to the "Noki Companion" VS Code extension (desktop/vscode-bridge).
//!
//! macOS/Electron drops key events posted to an INACTIVE VS Code whose
//! window sits on another Space (measured with CGEventPostToPid: no effect),
//! and activating VS Code would move the user to that Space. The extension
//! applies the keys through the VS Code API instead - no focus needed.
//!
//! Transport: one Unix socket per VS Code window in
//! ~/Library/Application Support/com.noki.desktop/vsc (0700; socket 0600)
//! plus a per-session random token in a 0600 file. Local only; the token and
//! anything typed are never logged.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bruecke {
    sock: String,
    token: String,
}

/// What the remote keyboard drives inside VS Code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Art { Editor, Terminal }

impl Art {
    pub fn name(self) -> &'static str {
        match self { Art::Editor => "VS_CODE_EDITOR", Art::Terminal => "VS_CODE_TERMINAL" }
    }
}

fn ordner() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    std::path::Path::new(&home).join("Library/Application Support/com.noki.desktop/vsc")
}

pub fn ist_vscode(pid: i32) -> bool {
    crate::lesezeichen::app_fuer_pid(pid).is_some_and(|(_, b, _)| {
        matches!(b.as_str(), "com.microsoft.VSCode" | "com.microsoft.VSCodeInsiders" | "com.vscodium")
    })
}

/// All live bridges (stale files of ended extension hosts are skipped).
fn bruecken() -> Vec<Bruecke> {
    let Ok(dir) = std::fs::read_dir(ordner()) else { return vec![] };
    let mut out = vec![];
    for e in dir.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") { continue; }
        let Ok(t) = std::fs::read_to_string(&p) else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) else { continue };
        let pid = v["pid"].as_i64().unwrap_or(0) as i32;
        if pid <= 0 || unsafe { libc::kill(pid, 0) } != 0 { continue; }
        let (Some(sock), Some(token)) = (v["sock"].as_str(), v["token"].as_str()) else { continue };
        out.push(Bruecke { sock: sock.into(), token: token.into() });
    }
    out
}

fn anfrage(b: &Bruecke, mut m: serde_json::Value) -> Option<serde_json::Value> {
    let s = UnixStream::connect(&b.sock).ok()?;
    s.set_read_timeout(Some(Duration::from_millis(1500))).ok()?;
    s.set_write_timeout(Some(Duration::from_millis(500))).ok()?;
    m["token"] = serde_json::Value::String(b.token.clone());
    (&s).write_all(format!("{m}\n").as_bytes()).ok()?;
    let mut zeile = String::new();
    BufReader::new(&s).read_line(&mut zeile).ok()?;
    serde_json::from_str(&zeile).ok()
}

/// The bridge of exactly the VS Code window `wid`: its title ("file — workspace"
/// by VS Code's default) must name the bridge's workspace (or, without one,
/// its active tab). Ambiguous or unknown = None (fail closed).
pub fn fuer_fenster(wid: i64) -> Option<Bruecke> {
    let titel = crate::cgs::alle_fenster().into_iter().find(|f| f.0 == wid).map(|f| f.3)?;
    let teile: Vec<String> = titel.split(" — ").map(|t| t.trim().trim_start_matches('●').trim().to_string()).collect();
    let mut passend = vec![];
    for b in bruecken() {
        let Some(id) = anfrage(&b, serde_json::json!({ "op": "ident" })) else { continue };
        let ws = id["workspace"].as_str().unwrap_or("").to_string();
        let tab = id["tab"].as_str().unwrap_or("").to_string();
        let treffer = if !ws.is_empty() { teile.iter().any(|t| *t == ws) } else { !tab.is_empty() && teile.iter().any(|t| *t == tab) };
        if treffer { passend.push((b, tab)); }
    }
    if passend.len() > 1 {
        // Same workspace in two windows: the active tab decides, else none.
        passend.retain(|(_, tab)| !tab.is_empty() && teile.iter().any(|t| t == tab));
    }
    let n = passend.len();
    crate::virtual_workspace::trace(&format!("[VSCODE_BRIDGE] resolve wid={wid} candidates={n}"));
    (n == 1).then(|| passend.pop().map(|p| p.0)).flatten()
}

/// Is a field (by `feld_kontext`) inside VS Code's integrated terminal?
/// Decided by the DOM class only (`.integrated-terminal`, measured on the
/// xterm canvas' ancestors) - never by labels, so an editor showing a file
/// named "terminal.rs" stays the editor.
pub fn ist_terminal_kontext(kontext: &str) -> bool {
    kontext.contains(".integrated-terminal|")
}

/// Verification read (acceptance tests): cursor, dirty flag, cursor line.
pub fn zustand(b: &Bruecke) -> Option<serde_json::Value> {
    anfrage(b, serde_json::json!({ "op": "zustand" }))
}

/// One ordered delivery lane: the key tap only enqueues (never blocks on a
/// socket); keys reach VS Code in order. After each delivery the window's
/// real frame is brought into the Miniatur (it paints nothing on its own
/// while on a hidden Space).
struct Auftrag { b: Bruecke, art: Art, key: String, text: String, shift: bool, pid: i32, wid: i64 }
static LANE: OnceLock<Mutex<Sender<Auftrag>>> = OnceLock::new();

pub fn senden(b: &Bruecke, art: Art, key: &str, text: &str, shift: bool, pid: i32, wid: i64) {
    let lane = LANE.get_or_init(|| {
        let (tx, rx) = channel::<Auftrag>();
        let _ = std::thread::Builder::new().name("noki-vscode-bridge".into()).spawn(move || {
            for a in rx {
                let op = if a.art == Art::Terminal { "terminal" } else { "editor" };
                let r = anfrage(&a.b, serde_json::json!({ "op": op, "key": a.key, "text": a.text, "shift": a.shift }));
                let ok = r.as_ref().is_some_and(|v| v["ok"].as_bool() == Some(true));
                if !ok {
                    crate::virtual_workspace::trace(&format!(
                        "[VSCODE_BRIDGE] deliver target={} key={} ok=false reason={}", a.art.name(),
                        if a.key == "text" { "text" } else { a.key.as_str() },
                        r.as_ref().and_then(|v| v["grund"].as_str()).unwrap_or("no_answer")));
                }
                crate::vorschau::lebhaft(a.wid);
                crate::vorschau::eingabe_zeichnen(a.pid, a.wid);
            }
        });
        Mutex::new(tx)
    });
    if let Ok(tx) = lane.lock() {
        let _ = tx.send(Auftrag { b: b.clone(), art, key: key.into(), text: text.into(), shift, pid, wid });
    }
}

// ---------------------------------------------------------------------
//  Live picture on another Space
// ---------------------------------------------------------------------
//  An occluded VS Code (Electron) window paints NOTHING - measured: input
//  arrives, the Miniatur keeps the old frame; a resize does not help.
//  Started with these Chromium switches it keeps painting (measured live,
//  editor + integrated terminal). Nothing is written to VS Code's settings:
//  the switches only live in the running process (reversible by a normal
//  restart of VS Code).
pub const LIVE_SCHALTER: [&str; 2] = ["--disable-backgrounding-occluded-windows", "--disable-renderer-backgrounding"];
const BUNDLE: &str = "com.microsoft.VSCode";

fn ist_vscode_app(app_oder_pfad: &str) -> bool {
    let a = app_oder_pfad.trim_end_matches('/');
    a.ends_with("Visual Studio Code.app") || a == "Visual Studio Code" || a == BUNDLE
}

/// PID of the running VS Code main process (not a helper), if any.
fn haupt_pid() -> Option<i32> {
    let out = std::process::Command::new("/usr/bin/pgrep")
        .args(["-f", "Visual Studio Code.app/Contents/MacOS/Code"]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.trim().parse::<i32>().ok())
        .find(|pid| befehlszeile(*pid).is_some_and(|c| !c.contains("--type=") && !c.contains("--user-data-dir")))
}

fn befehlszeile(pid: i32) -> Option<String> {
    let out = std::process::Command::new("/bin/ps").args(["-o", "command=", "-p", &pid.to_string()]).output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Is the running VS Code painting while occluded? None = not running.
pub fn live_faehig() -> Option<bool> {
    let pid = haupt_pid()?;
    Some(befehlszeile(pid).is_some_and(|c| c.contains(LIVE_SCHALTER[0])))
}

/// Extra `open` arguments whenever Noki itself starts VS Code: if it is not
/// running yet, it starts in live mode. (A running instance ignores them.)
pub fn open_zusatz(app_oder_pfad: &str) -> Vec<&'static str> {
    if ist_vscode_app(app_oder_pfad) && haupt_pid().is_none() {
        let mut v = vec!["--args"];
        v.extend(LIVE_SCHALTER);
        v
    } else {
        vec![]
    }
}

/// Is a restart lossless right now? Every window must have its bridge (to
/// verify), no dirty document, and no program running in any integrated
/// terminal (a restart would end it, e.g. btop).
fn neustart_sicher() -> Result<(), String> {
    let pid = haupt_pid().ok_or("VS Code läuft nicht")?;
    let fenster = crate::cgs::alle_fenster().into_iter()
        .filter(|f| f.1 == pid && f.6 >= 120 && f.7 >= 80 && crate::cgs::fenster_eingeordnet(f.0)).count();
    let bs = bruecken();
    if bs.is_empty() || bs.len() < fenster {
        return Err("nicht jedes VS-Code-Fenster hat Noki Companion".into());
    }
    for b in &bs {
        let s = anfrage(b, serde_json::json!({ "op": "status" })).ok_or("VS Code antwortet nicht")?;
        if s["ok"].as_bool() != Some(true) {
            // A window still runs an older Companion (no lossless check).
            return Err("Noki Companion in VS Code ist veraltet – „Developer: Reload Window“".into());
        }
        if s["dirty"].as_u64().unwrap_or(1) > 0 {
            return Err("ungespeicherte Änderungen".into());
        }
        for shell in s["shells"].as_array().into_iter().flatten().filter_map(|v| v.as_i64()) {
            let kinder = std::process::Command::new("/usr/bin/pgrep").args(["-P", &shell.to_string()]).output()
                .map(|o| !o.stdout.is_empty()).unwrap_or(true);
            if kinder { return Err("im Terminal läuft ein Programm".into()); }
        }
    }
    Ok(())
}

static LIVE_ABLAUF: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static LIVE_ABGELEHNT: Mutex<Option<std::time::Instant>> = Mutex::new(None);

/// Called on an explicit Miniatur action on a VS Code window. If VS Code is
/// not live-capable: restart it ONCE in live mode - only when lossless (VS
/// Code restores its windows/workspaces itself); otherwise say why, once
/// per minute, and change nothing.
pub fn live_sicherstellen() {
    // Never blocks the caller (the helper's reader thread): one check at a
    // time, at most every 5 s.
    static ZULETZT: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    let faellig = ZULETZT.lock().is_ok_and(|mut t| {
        let f = t.is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(5));
        if f { *t = Some(std::time::Instant::now()); }
        f
    });
    if !faellig || LIVE_ABLAUF.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        if live_faehig() != Some(false) {
            LIVE_ABLAUF.store(false, std::sync::atomic::Ordering::SeqCst);
            return;
        }
        let ergebnis = neustart_sicher().and_then(|_| live_neustart());
        match &ergebnis {
            Ok(()) => crate::vorschau::hinweis("VS Code läuft jetzt mit Live-Ansicht – Fenster und Dateien sind wiederhergestellt."),
            Err(grund) => {
                let melden = LIVE_ABGELEHNT.lock().is_ok_and(|mut t| {
                    let f = t.is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(60));
                    if f { *t = Some(std::time::Instant::now()); }
                    f
                });
                if melden {
                    crate::vorschau::hinweis(&format!("VS Code Live-Ansicht benötigt Neustart – nicht automatisch: {grund}."));
                }
            }
        }
        crate::virtual_workspace::trace(&format!("[VSCODE_LIVE] restart result={ergebnis:?}"));
        LIVE_ABLAUF.store(false, std::sync::atomic::Ordering::SeqCst);
    });
}

/// Graceful quit (VS Code saves its session), then start the same app with
/// the live switches; VS Code restores its windows and workspaces.
fn live_neustart() -> Result<(), String> {
    let alt = haupt_pid().ok_or("VS Code läuft nicht")?;
    crate::virtual_workspace::trace(&format!("[VSCODE_LIVE] restart begin pid={alt}"));
    let _ = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", &format!("tell application id \"{BUNDLE}\" to quit")]).output();
    let mut weg = false;
    for _ in 0..150 {
        if unsafe { libc::kill(alt, 0) } != 0 { weg = true; break; }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    if !weg { return Err("VS Code hat sich nicht beendet (Rückfrage offen?)".into()); }
    let mut c = std::process::Command::new("/usr/bin/open");
    c.args(["-b", BUNDLE, "--args"]).args(LIVE_SCHALTER);
    if !c.status().map(|s| s.success()).unwrap_or(false) { return Err("VS Code ließ sich nicht starten".into()); }
    for _ in 0..200 {
        if live_faehig() == Some(true) && !bruecken().is_empty() { return Ok(()); }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err("VS Code läuft, aber ohne Live-Modus".into())
}
