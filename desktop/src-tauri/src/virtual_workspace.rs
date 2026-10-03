//! Lifecycle and routing state for Noki's virtual workspace backend.
//!
//! The display itself lives in a tiny helper process.  This makes teardown
//! crash-safe: WindowServer removes an app-scoped `CGVirtualDisplay` when
//! the owning process exits, even after SIGKILL (verified by the probe).

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// The workspace implementation selected for this launch.
///
/// RealSpace is deliberately the product default.  The virtual display is
/// retained as an explicit comparison/fallback while the real-Space capture
/// gate is being measured; it must not silently become the primary model
/// again when the environment variable is absent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Backend { RealSpace, LegacyVirtualDisplay }

impl Backend {
    pub const fn label(self) -> &'static str {
        match self {
            Self::RealSpace => "REAL_SPACE",
            Self::LegacyVirtualDisplay => "LEGACY_VIRTUAL_DISPLAY",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Readiness { Initializing, Ready, Failed }

#[derive(Clone, Copy, Debug, Default)]
pub struct DisplayInfo {
    pub id: u32,
    pub main_id: u32,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

struct Runtime { child: Child, info: DisplayInfo }

static STATUS: AtomicU8 = AtomicU8::new(0);
static WORKSPACE_STATUS: AtomicU8 = AtomicU8::new(0);
static WORKSPACE_WINDOWS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static WORKSPACE_FAILURES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static RUNTIME: Mutex<Option<Runtime>> = Mutex::new(None);

/// Runtime tracing is OFF by default. Enabled by NOKI_DEBUG=1 or the flag
/// file ~/Library/Application Support/com.noki.desktop/debug-log (checked at
/// most every 5 s, so it can be switched on/off without a restart).
pub fn debug_an() -> bool {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    static AN: AtomicBool = AtomicBool::new(false);
    static GEPRUEFT: AtomicU64 = AtomicU64::new(0);
    let jetzt = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if jetzt.saturating_sub(GEPRUEFT.load(Ordering::Relaxed)) >= 5 {
        GEPRUEFT.store(jetzt, Ordering::Relaxed);
        let env = std::env::var("NOKI_DEBUG").is_ok_and(|v| v == "1");
        let datei = std::env::var("HOME").is_ok_and(|h| std::path::Path::new(&h)
            .join("Library/Application Support/com.noki.desktop/debug-log").exists());
        AN.store(env || datei, Ordering::Relaxed);
    }
    AN.load(Ordering::Relaxed)
}

fn log(message: &str) {
    if !debug_an() { return; }
    eprintln!("{message}");
    use std::io::Write;
    static FILE: std::sync::OnceLock<Mutex<Option<std::fs::File>>> = std::sync::OnceLock::new();
    let file = FILE.get_or_init(|| {
        let path = std::path::Path::new("/Users/yilonglin/NOKI/.local/workspace-runtime.log");
        if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
        Mutex::new(std::fs::OpenOptions::new().create(true).append(true).open(path).ok())
    });
    if let Ok(mut guard) = file.lock() {
        let Some(file) = guard.as_mut() else { return };
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis()).unwrap_or(0);
        let _ = writeln!(file, "{now} pid={} {message}", std::process::id());
    }
}

pub fn trace(message: &str) { log(message); }

fn parse_backend(value: Option<&str>) -> Backend {
    match value {
        // Keep the historical spelling for developer scripts, but make the
        // legacy backend an affirmative opt-in.
        Some("virtual") | Some("virtual-display") | Some("legacy-virtual-display") => {
            Backend::LegacyVirtualDisplay
        }
        Some("space") | Some("real-space") | None => Backend::RealSpace,
        Some(other) => {
            log(&format!("[WORKSPACE] unknown backend={other:?}; using REAL_SPACE"));
            Backend::RealSpace
        }
    }
}

pub fn backend() -> Backend {
    parse_backend(std::env::var("NOKI_WORKSPACE_BACKEND").ok().as_deref())
}

#[cfg(test)]
mod backend_tests {
    use super::*;

    #[test]
    fn real_space_is_the_default_and_legacy_is_explicit() {
        assert_eq!(parse_backend(None), Backend::RealSpace);
        assert_eq!(parse_backend(Some("real-space")), Backend::RealSpace);
        assert_eq!(parse_backend(Some("space")), Backend::RealSpace);
        assert_eq!(parse_backend(Some("virtual")), Backend::LegacyVirtualDisplay);
        assert_eq!(parse_backend(Some("legacy-virtual-display")), Backend::LegacyVirtualDisplay);
    }
}

pub fn readiness() -> Readiness {
    match STATUS.load(Ordering::SeqCst) {
        1 => Readiness::Ready,
        2 => Readiness::Failed,
        _ => Readiness::Initializing,
    }
}

pub fn backend_ready() -> bool {
    backend() == Backend::LegacyVirtualDisplay && readiness() == Readiness::Ready
}

pub fn workspace_readiness() -> Readiness {
    match WORKSPACE_STATUS.load(Ordering::SeqCst) {
        1 => Readiness::Ready,
        2 => Readiness::Failed,
        _ => Readiness::Initializing,
    }
}

pub fn ready() -> bool {
    backend_ready() && workspace_readiness() == Readiness::Ready
}

pub fn finish_workspace(ok: bool, windows: usize, failures: usize) {
    WORKSPACE_WINDOWS.store(windows, Ordering::SeqCst);
    WORKSPACE_FAILURES.store(failures, Ordering::SeqCst);
    WORKSPACE_STATUS.store(if ok { 1 } else { 2 }, Ordering::SeqCst);
    log(&format!(
        "[WORKSPACE] content_state={} windows={} restore_failures={} interaction_ready={}",
        if ok { "READY" } else { "FAILED" }, windows, failures, ok
    ));
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
struct CgRect { x: f64, y: f64, w: f64, h: f64 }

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGDisplayBounds(display: u32) -> CgRect;
}

/// Identity plus the display's CURRENT bounds.  WindowServer may re-arrange
/// the virtual display after start (measured: READY reported 1470,956, later
/// it sat at 1470,0).  A cached origin then misplaced every miniature
/// coordinate and disabled the pointer guard, so bounds are always read live.
pub fn info() -> Option<DisplayInfo> {
    let mut i = RUNTIME.lock().ok().and_then(|runtime| runtime.as_ref().map(|r| r.info))?;
    #[cfg(target_os = "macos")]
    {
        let b = unsafe { CGDisplayBounds(i.id) };
        if b.w >= 1.0 && b.h >= 1.0 {
            i.x = b.x as i32; i.y = b.y as i32; i.width = b.w as i32; i.height = b.h as i32;
        }
    }
    Some(i)
}

/// Re-publish the live bounds to the preview compositor.
fn bounds_publizieren(grund: &str) {
    let Some(i) = info() else { return };
    let changed = RUNTIME.lock().ok().and_then(|mut g| g.as_mut().map(|r| {
        let alt = r.info;
        r.info = i;
        (alt.x, alt.y, alt.width, alt.height) != (i.x, i.y, i.width, i.height)
    })).unwrap_or(false);
    log(&format!(
        "[WORKSPACE] bounds reason={grund} display={} bounds={},{},{}x{} changed={changed}",
        i.id, i.x, i.y, i.width, i.height
    ));
    crate::vorschau::desktop_von(i.x, i.y, i.width, i.height, i.id);
}

/// WindowServer insists that a virtual display touches the physical layout.
/// The helper reduces that contact to one diagonal point; this final guard
/// rejects pointer coordinates inside the virtual rectangle.
pub fn guard_pointer(x: f64, y: f64) -> Option<(f64, f64)> {
    if backend() != Backend::LegacyVirtualDisplay { return None; }
    let i = info()?;
    let inside = x >= i.x as f64 && x < (i.x + i.width) as f64
        && y >= i.y as f64 && y < (i.y + i.height) as f64;
    inside.then_some((i.x as f64 - 1.0, i.y as f64 - 1.0))
}

fn helper_path() -> Option<std::path::PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let mut candidates = Vec::new();
    if let Some(macos) = executable.parent() {
        if let Some(contents) = macos.parent() {
            candidates.push(contents.join("Helpers/noki-virtual-display"));
        }
        candidates.push(macos.join("noki-virtual-display"));
    }
    candidates.push(std::path::PathBuf::from(
        "/Users/yilonglin/NOKI/desktop/virtual-display/noki-virtual-display",
    ));
    candidates.into_iter().find(|path| path.is_file())
}

fn value(line: &str, name: &str) -> Option<i64> {
    line.split_whitespace()
        .find_map(|part| part.strip_prefix(&format!("{name}=")).and_then(|v| v.parse().ok()))
}

fn parse_ready(line: &str) -> Option<DisplayInfo> {
    if !line.starts_with("READY ") { return None; }
    Some(DisplayInfo {
        id: value(line, "display")? as u32,
        main_id: value(line, "main")? as u32,
        x: value(line, "x")? as i32,
        y: value(line, "y")? as i32,
        width: value(line, "w")? as i32,
        height: value(line, "h")? as i32,
    })
}

/// Starts once.  Consumers observe INITIALIZING and fail closed until the
/// helper has published a complete display identity and bounds tuple.
pub fn start(app: tauri::AppHandle) {
    let selected = backend();
    log(&format!("NOKI_WORKSPACE_BACKEND={}", selected.label()));
    if selected == Backend::RealSpace {
        STATUS.store(1, Ordering::SeqCst);
        WORKSPACE_STATUS.store(1, Ordering::SeqCst);
        log("[WORKSPACE] backend=space state=READY");
        return;
    }
    STATUS.store(0, Ordering::SeqCst);
    WORKSPACE_STATUS.store(0, Ordering::SeqCst);
    std::thread::Builder::new().name("noki-virtual-workspace".into()).spawn(move || {
        let Some(path) = helper_path() else {
            STATUS.store(2, Ordering::SeqCst);
            log("[WORKSPACE] backend=virtual state=FAILED reason=helper_missing");
            return;
        };
        let mut child = match Command::new(&path)
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn() {
            Ok(child) => child,
            Err(error) => {
                STATUS.store(2, Ordering::SeqCst);
                log(&format!("[WORKSPACE] backend=virtual state=FAILED reason=spawn error={error}"));
                return;
            }
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            STATUS.store(2, Ordering::SeqCst);
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            let result = reader.read_line(&mut line).map(|_| line);
            let _ = tx.send((result, reader));
        });
        // Der Helfer prueft 2 s lang, ob ein echter Bildschirm da ist, bevor er
        // die Anzeige anlegt (Deckel-Schutz) - 5 s reichten gemessen nicht immer.
        let (first, mut reader) = match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(pair) => pair,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                STATUS.store(2, Ordering::SeqCst);
                log(&format!("[WORKSPACE] backend=virtual state=FAILED reason=ready_timeout error={error}"));
                wiederaufnahme_planen(app);
                return;
            }
        };
        let first = first.ok().unwrap_or_default();
        if first.starts_with("SUSPENDED") {
            let _ = child.wait();
            STATUS.store(2, Ordering::SeqCst);
            log(&format!("[WORKSPACE] backend=virtual state=SUSPENDED helper={}", first.trim()));
            wiederaufnahme_planen(app);
            return;
        }
        let Some(info) = parse_ready(first.trim()) else {
            let _ = child.kill();
            STATUS.store(2, Ordering::SeqCst);
            log(&format!("[WORKSPACE] backend=virtual state=FAILED helper={}", first.trim()));
            // Nach einem Aussetzen (Deckel zu) nicht aufgeben: spaeter erneut -
            // der Helfer prueft selbst 2 s lang, ob wieder ein echter Bildschirm da ist.
            if WAR_AUSGESETZT.load(Ordering::SeqCst) { wiederaufnahme_planen(app); }
            return;
        };
        if info.id == info.main_id || info.width < 1 || info.height < 1 {
            let _ = child.kill();
            STATUS.store(2, Ordering::SeqCst);
            log("[WORKSPACE] backend=virtual state=FAILED reason=unsafe_display_identity");
            return;
        }
        if let Ok(mut runtime) = RUNTIME.lock() {
            *runtime = Some(Runtime { child, info });
        }
        STATUS.store(1, Ordering::SeqCst);
        WAR_AUSGESETZT.store(false, Ordering::SeqCst);
        log(&format!(
            "[WORKSPACE] backend=virtual state=READY display={} main={} bounds={},{},{}x{}",
            info.id, info.main_id, info.x, info.y, info.width, info.height
        ));
        // The preview compositor must use the virtual desktop's coordinates;
        // inheriting CGMainDisplayID here stretches/offsets its surfaces onto
        // the user's real display.
        crate::vorschau::desktop_von(info.x, info.y, info.width, info.height, info.id);
        crate::vorschau::navi("Noki Schreibtisch öffnen");
        crate::virtuelle_bereitschaft(&app);
        let mut line = String::new();
        let mut ausgesetzt = false;
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    eprintln!("[VIRTUAL_DISPLAY] {}", line.trim());
                    if line.starts_with("SUSPENDED") {
                        ausgesetzt = true;
                        log(&format!("[WORKSPACE] backend=virtual state=SUSPENDED helper={}", line.trim()));
                    }
                    // The helper re-places the display after a WindowServer
                    // re-arrangement; the preview must follow the same truth.
                    if line.starts_with("MOVED ") { bounds_publizieren("helper_moved"); }
                }
            }
        }
        STATUS.store(2, Ordering::SeqCst);
        WORKSPACE_STATUS.store(0, Ordering::SeqCst);
        if let Ok(mut runtime) = RUNTIME.lock() {
            if let Some(mut r) = runtime.take() { let _ = r.child.wait(); }
        }
        if ausgesetzt || !BEENDET.load(Ordering::SeqCst) {
            // Deckel zu / Schlaf / Helfer weg: die Anzeige ist freigegeben
            // (der Mac kann schlafen). Sobald wieder ein echter Bildschirm da
            // ist, entsteht Noki Schreibtisch neu und holt seine Fenster zurueck.
            if !ausgesetzt { log("[WORKSPACE] backend=virtual state=FAILED reason=helper_exit"); }
            wiederaufnahme_planen(app);
        }
    }).ok();
}

/// Noki wird beendet: keine Wiederaufnahme mehr.
static BEENDET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static WIEDER_GEPLANT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Alle 3 s neu versuchen. Ohne echten Bildschirm (Deckel zu) beendet sich
/// der Helfer, BEVOR er eine Anzeige anlegt - es entsteht nichts.
static WAR_AUSGESETZT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn wiederaufnahme_planen(app: tauri::AppHandle) {
    if BEENDET.load(Ordering::SeqCst) || WIEDER_GEPLANT.swap(true, Ordering::SeqCst) { return; }
    WAR_AUSGESETZT.store(true, Ordering::SeqCst);
    std::thread::spawn(move || {
        // Gemessen: 4 s nach dem Aussetzen flackerte der Deckelzustand noch.
        std::thread::sleep(Duration::from_secs(10));
        WIEDER_GEPLANT.store(false, Ordering::SeqCst);
        if BEENDET.load(Ordering::SeqCst) { return; }
        log("[WORKSPACE] backend=virtual resume_attempt");
        start(app);
    });
}

pub fn stop() {
    BEENDET.store(true, Ordering::SeqCst);
    STATUS.store(2, Ordering::SeqCst);
    WORKSPACE_STATUS.store(2, Ordering::SeqCst);
    if let Ok(mut runtime) = RUNTIME.lock() {
        if let Some(mut runtime) = runtime.take() {
            let _ = runtime.child.kill();
            let _ = runtime.child.wait();
        }
    }
}

pub fn diagnostic() -> serde_json::Value {
    let i = info();
    serde_json::json!({
        "backend": backend().label(),
        "state": match readiness() { Readiness::Initializing => "INITIALIZING", Readiness::Ready => "READY", Readiness::Failed => "FAILED" },
        "workspace_state": match workspace_readiness() { Readiness::Initializing => "INITIALIZING", Readiness::Ready => "READY", Readiness::Failed => "FAILED" },
        "window_count": WORKSPACE_WINDOWS.load(Ordering::SeqCst),
        "restore_failures": WORKSPACE_FAILURES.load(Ordering::SeqCst),
        "display": i.map(|x| x.id), "main_display": i.map(|x| x.main_id),
        "bounds": i.map(|x| [x.x, x.y, x.width, x.height]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_complete_ready_record() {
        let info = parse_ready("READY display=8 main=1 x=1470 y=956 w=1470 h=956").unwrap();
        assert_eq!((info.id, info.main_id, info.x, info.y, info.width, info.height),
                   (8, 1, 1470, 956, 1470, 956));
    }

    #[test]
    fn refuses_partial_or_failed_records() {
        assert!(parse_ready("FAILED reason=create_failed").is_none());
        assert!(parse_ready("READY display=8").is_none());
    }
}
