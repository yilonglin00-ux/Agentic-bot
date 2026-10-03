//! Noki Browser: a Noki-owned Chrome instance (own persistent profile) for
//! web content on the Noki Desktop.
//!
//! Why (measured 2026-09-26): web pages in the user's own browser freeze as
//! soon as the Noki Space is hidden (Safari and the Chrome YouTube PWA: 0
//! backing-store changes in 10 s). The Miniatur then shows old frames and a
//! scroll looks like a teleport. A Chrome started with occlusion
//! backgrounding disabled keeps painting while hidden (clock: 20/20 s, video
//! ~1 fps sampled, Miniatur frame age 0.7 s), and its loopback DevTools
//! endpoint gives exact, activation-free control of the SAME page the real
//! window shows: scrollBy 200/200 exact (p50 0.9 ms), YouTube play/pause
//! 20/20 verified.
//!
//! Boundaries:
//!   * only the Chrome process started with THIS profile dir is ever touched
//!     (never the user's normal Chrome);
//!   * DevTools binds 127.0.0.1 on a random port (DevToolsActivePort file);
//!   * all commands run on this module's own thread - the shared Miniatur
//!     interaction worker never waits for the browser.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub fn profil_pfad() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    std::path::PathBuf::from(home).join("Library/Application Support/com.noki.desktop/NokiBrowser")
}

/// Chrome flags that keep the Noki Browser alive on a hidden Space.
pub const FLAGS: [&str; 8] = [
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-backgrounding-occluded-windows",
    "--disable-renderer-backgrounding",
    "--disable-background-timer-throttling",
    // Frames from a synthetic clock, not the display link: on another Space
    // the display link never ticks, so the hidden page rendered NO frames
    // (measured rAF 0/s -> 61/s with this flag; ~21% CPU for 1080p video).
    "--disable-gpu-vsync",
    "--remote-debugging-port=0",
    "--remote-debugging-address=127.0.0.1",
];

/// Start (or reuse) the Noki Browser. The FIRST window is born on the
/// currently active Space - callers launch it only while the Noki Space is
/// active. Later pages are opened as tabs via DevTools (no new window).
pub fn starten(url: &str) -> Result<(), String> {
    if let Ok(mut g) = PID_FEHLT.lock() { *g = None; }
    if let Some(port) = port() {
        return neuer_tab(port, url);
    }
    let profil = profil_pfad();
    let _ = std::fs::create_dir_all(&profil);
    let mut args: Vec<String> = vec!["-n".into(), "-g".into(), "-a".into(), "Google Chrome".into(), "--args".into(),
        format!("--user-data-dir={}", profil.display())];
    args.extend(FLAGS.iter().map(|s| s.to_string()));
    args.push("--new-window".into());
    args.push(url.into());
    std::process::Command::new("/usr/bin/open").args(&args).status().map_err(|e| e.to_string())?;
    if let Ok(mut g) = PID_FEHLT.lock() { *g = None; }
    crate::virtual_workspace::trace(&format!("[BROWSER] launch profile={} url={url}", profil.display()));
    Ok(())
}

// ---------------------------------------------------------------------
//  Noki YouTube: a dedicated app-style window of the Noki Browser
// ---------------------------------------------------------------------
//
// Why (measured 2026-09-26/27): the installed YouTube PWA is rendered by the
// USER's Chrome, which stops painting on the hidden Noki Space (0 backing
// store changes in 10 s) - stale Miniatur, half-screen scroll teleports,
// Play without effect. The Noki YouTube window is the SAME Noki Browser
// (own profile, occlusion backgrounding off, loopback DevTools) in Chrome's
// app mode: its own window without tab strip/omnibox, so it looks and
// behaves like an app, while scroll/click/keys/freshness use exactly the
// Noki Browser engine. The installed PWA is never touched or removed.

pub const YOUTUBE_URL: &str = "https://www.youtube.com/";

fn youtube_merker() -> std::path::PathBuf { profil_pfad().join("noki-youtube-window") }

fn fenster_von(p: i32) -> Vec<(i64, i64, i64)> {
    crate::cgs::alle_fenster().into_iter().filter(|f| f.1 == p && f.6 >= 300 && f.7 >= 200)
        .map(|f| (f.0, f.6, f.7)).collect()
}

/// The Noki YouTube window, if it is open.
pub fn youtube_fenster() -> Option<i64> {
    // Called from the 1 s inventory tick and once per app-bar chip.  The full
    // page check costs a CDP scan, so its verdict is kept for 3 s; the cheap
    // native-window check still runs on every call.
    static PRUEFUNG: Mutex<Option<(i64, bool, Instant)>> = Mutex::new(None);
    let p = pid()?;
    let w: i64 = std::fs::read_to_string(youtube_merker()).ok()?.trim().parse().ok()?;
    // Chrome leaves closed native windows in WindowServer for a while.  A
    // remembered id alone therefore turns into a permanent false positive:
    // the launcher says "already open", but there is no page to show or
    // scroll.  The page target is the second authority and must still map to
    // this exact window.
    fenster_von(p).iter().any(|f| f.0 == w).then_some(())?;
    if let Some((cw, ok, t)) = PRUEFUNG.lock().ok().and_then(|g| *g) {
        if cw == w && t.elapsed() < Duration::from_secs(3) { return ok.then_some(w); }
    }
    // Never purge the scroll path's page cache here: that forced the next
    // scroll packet into a slow CDP rescan every inventory tick.
    let ok = port()
        .zip(rahmen(w))
        .and_then(|(port, r)| seite_suchen(port, r))
        .is_some_and(|s| s.url.starts_with("https://www.youtube.com/"));
    if let Ok(mut g) = PRUEFUNG.lock() { *g = Some((w, ok, Instant::now())); }
    ok.then_some(w)
}

/// Close the Noki YouTube window through its own page target (CDP
/// Target.closeTarget: measured safe, no activation) and drop the marker.
/// Used for a stray window that was born on the user's Desktop.
pub fn youtube_schliessen(w: i64) -> bool {
    let ok = port().zip(rahmen(w)).and_then(|(port, r)| {
        let seite = seite_suchen(port, r)?;
        let mut b = browser_ws(port)?;
        b.rufen("Target.closeTarget", serde_json::json!({ "targetId": seite.id }))
    }).is_some();
    vergessen(w);
    let _ = std::fs::remove_file(youtube_merker());
    crate::virtual_workspace::trace(&format!("[YOUTUBE] stray_window_closed wid={w} ok={ok}"));
    ok
}

pub fn ist_youtube_fenster(wid: i64) -> bool { wid > 0 && youtube_fenster() == Some(wid) }

/// Open (or return) the Noki YouTube window. macOS creates a new window on
/// the ACTIVE Space: callers run this only while the Noki Space is active.
pub fn youtube_starten() -> Result<i64, String> {
    if let Some(w) = youtube_fenster() { return Ok(w); }
    let vorher: Vec<i64> = pid().map(|p| fenster_von(p).into_iter().map(|f| f.0).collect()).unwrap_or_default();
    let ziele_vorher: Vec<String> = port().map(|p| seiten(p).into_iter().map(|s| s.id).collect()).unwrap_or_default();
    let profil = profil_pfad();
    let _ = std::fs::create_dir_all(&profil);
    // A running Noki Browser receives this as a new app window (Chrome's
    // process singleton of THIS profile); otherwise it starts with FLAGS.
    let mut args: Vec<String> = vec!["-n".into(), "-g".into(), "-a".into(), "Google Chrome".into(), "--args".into(),
        format!("--user-data-dir={}", profil.display())];
    args.extend(FLAGS.iter().map(|s| s.to_string()));
    args.push(format!("--app={YOUTUBE_URL}"));
    std::process::Command::new("/usr/bin/open").args(&args).status().map_err(|e| e.to_string())?;
    if let Ok(mut g) = PID_FEHLT.lock() { *g = None; }
    let bis = Instant::now() + Duration::from_secs(10);
    while Instant::now() < bis {
        std::thread::sleep(Duration::from_millis(150));
        let Some(p) = pid() else { continue };
        if let Some(f) = fenster_von(p).into_iter().filter(|f| !vorher.contains(&f.0)).max_by_key(|f| f.1 * f.2) {
            let _ = std::fs::write(youtube_merker(), f.0.to_string());
            // Chrome opens the app window at the frame of the last browser
            // window: identical bounds would make the window->page mapping
            // (by bounds) ambiguous and hide one window behind the other.
            if let (Some(port), Some(r)) = (port(), rahmen(f.0)) {
                let belegt = fenster_von(p).iter().any(|g| g.0 != f.0 && rahmen(g.0) == Some(r));
                let neu = seiten(port).into_iter().find(|s| !ziele_vorher.contains(&s.id));
                if let (true, Some(neu), Some(mut b)) = (belegt, neu, browser_ws(port)) {
                    if let Some(w) = b.rufen("Browser.getWindowForTarget", serde_json::json!({ "targetId": neu.id })) {
                        let _ = b.rufen("Browser.setWindowBounds", serde_json::json!({ "windowId": w["windowId"],
                            "bounds": { "left": r[0] as i64 + 60, "top": r[1] as i64 + 30, "width": (r[2] as i64 - 40).max(640), "height": (r[3] as i64 - 60).max(480) } }));
                    }
                }
            }
            crate::virtual_workspace::trace(&format!("[YOUTUBE] noki_window_created wid={} size={}x{} route=noki_browser_app_mode", f.0, f.1, f.2));
            return Ok(f.0);
        }
    }
    Err("Noki YouTube: kein Fenster erschienen.".into())
}

/// "Open YouTube" asked while the user is NOT on the Noki Space: a new
/// window would be born on the user's Desktop. Remember it and open it the
/// moment the user arrives on the Noki Space (explicit footer visit).
pub fn youtube_vormerken() {
    static WARTET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if WARTET.swap(true, std::sync::atomic::Ordering::SeqCst) { return; }
    let _ = std::thread::Builder::new().name("noki-youtube-wartet".into()).spawn(|| {
        let bis = Instant::now() + Duration::from_secs(600);
        while Instant::now() < bis {
            std::thread::sleep(Duration::from_millis(300));
            let nk = crate::vorschau::noki_space_id();
            if nk != 0 && crate::cgs::aktiver_space().map(|a| a.0) == Some(nk) {
                std::thread::sleep(Duration::from_millis(400));
                let r = youtube_starten();
                crate::virtual_workspace::trace(&format!("[YOUTUBE] deferred_open on_noki_space result={r:?}"));
                break;
            }
        }
        WARTET.store(false, std::sync::atomic::Ordering::SeqCst);
    });
}

/// PID of the Noki Browser main process (the Chrome started with our profile).
static PID_FEHLT: Mutex<Option<Instant>> = Mutex::new(None);

pub fn pid() -> Option<i32> {
    static CACHE: Mutex<Option<(i32, Instant)>> = Mutex::new(None);
    // "Not running" is remembered too (10 s): without it every call - the
    // 1 s inventory tick plus each app-bar chip - spawned a full `ps -axww`
    // whenever the Noki Browser was not running (measured in idle profile).
    if PID_FEHLT.lock().ok().and_then(|g| *g).is_some_and(|t| t.elapsed() < Duration::from_secs(10)) {
        return None;
    }
    if let Some((p, t)) = CACHE.lock().ok().and_then(|g| *g) {
        // Alive = still ours (a restarted browser gets a new pid); no `ps`
        // spawn every 5 s on the Miniatur's input thread mid-gesture.
        if unsafe { libc::kill(p, 0) } == 0 && (t.elapsed() < Duration::from_secs(60) || {
            if let Ok(mut g) = CACHE.lock() { *g = Some((p, Instant::now())); } true }) { return Some(p); }
    }
    let profil = format!("--user-data-dir={}", profil_pfad().display());
    let out = std::process::Command::new("/bin/ps").args(["-axwwo", "pid=,command="]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let p = text.lines().find(|l| l.contains(&profil) && l.contains("/MacOS/Google Chrome ") && !l.contains("--type="))
        .and_then(|l| l.split_whitespace().next()?.parse::<i32>().ok());
    if let Ok(mut g) = CACHE.lock() { *g = p.map(|p| (p, Instant::now())); }
    if let Ok(mut g) = PID_FEHLT.lock() { *g = p.is_none().then(Instant::now); }
    p
}

pub fn ist_noki_browser(pid_: i32) -> bool { pid_ > 0 && pid() == Some(pid_) }

/// Real WindowServer frame (global points) of a window - also for windows on
/// a hidden Space. (The legacy registry frame is empty under REAL_SPACE.)
pub fn rahmen(wid: i64) -> Option<[f64; 4]> {
    crate::cgs::alle_fenster().into_iter().find(|f| f.0 == wid)
        .map(|(_, _, _, _, x, y, w, h, _)| [x as f64, y as f64, w as f64, h as f64])
}

/// DevTools port of the running Noki Browser (loopback).
pub fn port() -> Option<u16> {
    pid()?;
    let text = std::fs::read_to_string(profil_pfad().join("DevToolsActivePort")).ok()?;
    text.lines().next()?.trim().parse().ok()
}

// ---------------------------------------------------------------------
//  Minimal loopback HTTP + WebSocket (RFC 6455 client, text frames)
// ---------------------------------------------------------------------

fn http_get(port: u16, pfad: &str) -> Option<String> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).ok()?;
    s.set_read_timeout(Some(Duration::from_millis(800))).ok()?;
    write!(s, "GET {pfad} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").ok()?;
    // Read exactly the announced body: Chrome keeps the connection open, so
    // "read until close" waited for the full read timeout (measured: every
    // tab re-mapping stalled the browser queue ~800 ms).
    let mut buf = Vec::new();
    let mut tmp = [0u8; 16384];
    loop {
        let n = s.read(&mut tmp).ok()?;
        if n == 0 { break; }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let kopf = String::from_utf8_lossy(&buf[..i]).to_lowercase();
            let laenge = kopf.lines().find_map(|l| l.strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok()));
            if let Some(n) = laenge { if buf.len() >= i + 4 + n { return Some(String::from_utf8_lossy(&buf[i + 4..i + 4 + n]).into_owned()); } }
        }
    }
    let text = String::from_utf8_lossy(&buf).into_owned();
    text.split_once("\r\n\r\n").map(|(_, b)| b.to_string())
}

pub struct Ws { s: TcpStream, id: u64 }

impl Ws {
    pub fn verbinden(port: u16, pfad: &str) -> Option<Ws> {
        let mut s = TcpStream::connect(("127.0.0.1", port)).ok()?;
        s.set_read_timeout(Some(Duration::from_millis(1500))).ok()?;
        s.set_nodelay(true).ok()?;
        write!(s, "GET {pfad} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
            Sec-WebSocket-Key: bm9raS1icm93c2VyLWtleQ==\r\nSec-WebSocket-Version: 13\r\n\r\n").ok()?;
        let mut kopf = Vec::new();
        let mut b = [0u8; 1];
        while !kopf.ends_with(b"\r\n\r\n") {
            if s.read(&mut b).ok()? == 0 { return None; }
            kopf.push(b[0]);
            if kopf.len() > 4096 { return None; }
        }
        if !String::from_utf8_lossy(&kopf).contains(" 101 ") { return None; }
        Some(Ws { s, id: 0 })
    }

    fn senden(&mut self, text: &str) -> Option<()> {
        let daten = text.as_bytes();
        let mut f = vec![0x81u8];
        let n = daten.len();
        if n < 126 { f.push(0x80 | n as u8); }
        else if n < 65536 { f.push(0x80 | 126); f.extend_from_slice(&(n as u16).to_be_bytes()); }
        else { f.push(0x80 | 127); f.extend_from_slice(&(n as u64).to_be_bytes()); }
        let maske = [0x4e, 0x6f, 0x6b, 0x69];
        f.extend_from_slice(&maske);
        f.extend(daten.iter().enumerate().map(|(i, c)| c ^ maske[i % 4]));
        self.s.write_all(&f).ok()
    }

    fn lesen(&mut self) -> Option<String> {
        let mut nachricht = Vec::new();
        loop {
            let mut k = [0u8; 2];
            self.s.read_exact(&mut k).ok()?;
            let fin = k[0] & 0x80 != 0;
            let op = k[0] & 0x0f;
            let mut n = (k[1] & 0x7f) as u64;
            if n == 126 { let mut e = [0u8; 2]; self.s.read_exact(&mut e).ok()?; n = u16::from_be_bytes(e) as u64; }
            else if n == 127 { let mut e = [0u8; 8]; self.s.read_exact(&mut e).ok()?; n = u64::from_be_bytes(e); }
            let mut d = vec![0u8; n as usize];
            self.s.read_exact(&mut d).ok()?;
            if op == 0x9 { continue; } // ping (server->client never needs a pong for our short sessions)
            if op == 0x8 { return None; }
            nachricht.extend_from_slice(&d);
            if fin { return String::from_utf8(nachricht).ok(); }
        }
    }

    /// One CDP call; returns `result` (or None on error/timeout).
    pub fn rufen(&mut self, methode: &str, params: serde_json::Value) -> Option<serde_json::Value> {
        self.id += 1;
        let id = self.id;
        self.senden(&serde_json::json!({ "id": id, "method": methode, "params": params }).to_string())?;
        let bis = Instant::now() + Duration::from_millis(1500);
        while Instant::now() < bis {
            let text = self.lesen()?;
            let v: serde_json::Value = serde_json::from_str(&text).ok()?;
            if v["id"].as_u64() == Some(id) {
                return if v.get("error").is_some() { None } else { Some(v["result"].clone()) };
            }
        }
        None
    }

    pub fn js(&mut self, ausdruck: &str) -> Option<serde_json::Value> {
        let r = self.rufen("Runtime.evaluate", serde_json::json!({
            "expression": ausdruck, "returnByValue": true, "awaitPromise": true, "userGesture": true }))?;
        Some(r["result"]["value"].clone())
    }
}

// ---------------------------------------------------------------------
//  Dialog guard: page dialogs must never leave the Noki Space
// ---------------------------------------------------------------------

/// Measured 2026-09-27: alert/confirm/prompt of a page in the hidden Noki
/// Browser open as a separate Chrome window ON THE USER'S CURRENT DESKTOP and
/// activate Chrome. Unless Noki says the user is on the Noki Space
/// (`__nokiHier`, set by the guard session), dialogs become an in-page
/// notice; confirm/prompt answer "Cancel" (the safe answer - nothing is
/// confirmed on the user's behalf). document.hasFocus() is NOT usable: it is
/// true in the hidden page after a DevTools click (measured). Versioned: a
/// newer guard replaces an older one in a live page; the true natives are
/// captured once (`__nokiOrig`).
const DIALOG_JS: &str = "(()=>{if(window.__nokiDialog===5)return;window.__nokiDialog=5;\
const o=window.__nokiOrig||(window.__nokiOrig={a:window.alert,c:window.confirm,p:window.prompt,o:window.open,pr:window.print,ic:HTMLInputElement.prototype.click,sp:HTMLInputElement.prototype.showPicker});\
const zeige=(art,m)=>{try{const d=document.createElement('div');\
d.style.cssText='position:fixed;left:50%;top:14px;transform:translateX(-50%);z-index:2147483647;max-width:520px;background:#fff;color:#111;border:1px solid #999;border-radius:10px;box-shadow:0 8px 28px rgba(0,0,0,.3);font:13px -apple-system,system-ui;padding:12px 14px;color-scheme:light';\
const t=document.createElement('div');t.textContent=String(m??'');t.style.cssText='margin-bottom:8px;white-space:pre-wrap';\
const h=document.createElement('div');h.textContent=art==='alert'?'Seitenhinweis':art==='native'?'Nur auf dem Noki Schreibtisch verf\\u00fcgbar.':'Seitendialog beantwortet mit \\u201eAbbrechen\\u201c \\u2013 auf dem Noki Schreibtisch erneut ausf\\u00fchren, um zu best\\u00e4tigen.';h.style.cssText='opacity:.65;font-size:12px;margin-bottom:8px';\
const b=document.createElement('button');b.textContent='OK';b.onclick=()=>d.remove();\
d.append(t,h,b);(document.body||document.documentElement).appendChild(d);setTimeout(()=>d.remove(),15000);}catch(_){}};\
const hier=()=>window.__nokiHier===true;\
const neu=u=>{try{const x=new URL(u,location.href);if(!/^(https?|file):$/.test(x.protocol))return false;(window.__nokiNeu=window.__nokiNeu||[]).push(x.href);return true;}catch(_){return false}};\
window.alert=function(m){if(hier())return o.a.apply(this,arguments);zeige('alert',m);};\
window.confirm=function(m){if(hier())return o.c.apply(this,arguments);zeige('confirm',m);return false;};\
window.prompt=function(m,v){if(hier())return o.p.apply(this,arguments);zeige('prompt',m);return null;};\
window.open=function(u){if(hier())return o.o.apply(this,arguments);if(!(u&&neu(u)))zeige('native','Neues Fenster');return null;};\
window.print=function(){if(hier())return o.pr.apply(this,arguments);zeige('native','Drucken');};\
HTMLInputElement.prototype.click=function(){if(!hier()&&this.type==='file'){zeige('native','Dateiauswahl');return;}return o.ic.apply(this,arguments);};\
if(o.sp)HTMLInputElement.prototype.showPicker=function(){if(!hier()){zeige('native','Auswahl');return;}return o.sp.apply(this,arguments);};\
for(const n of ['showOpenFilePicker','showSaveFilePicker','showDirectoryPicker']){const f=window[n];if(f)window[n]=function(){if(hier())return f.apply(this,arguments);zeige('native','Dateiauswahl');return Promise.reject(new DOMException('Noki','AbortError'));};}\
try{const rp=Notification.requestPermission.bind(Notification);Notification.requestPermission=function(){if(hier())return rp.apply(this,arguments);zeige('native','Benachrichtigungen erlauben');return Promise.resolve(Notification.permission);};}catch(_){}\
try{const md=navigator.mediaDevices,gu=md.getUserMedia.bind(md);md.getUserMedia=function(){if(hier())return gu.apply(this,arguments);zeige('native','Kamera/Mikrofon');return Promise.reject(new DOMException('Noki','NotAllowedError'));};}catch(_){}\
try{const g=navigator.geolocation,gp=g.getCurrentPosition.bind(g),wp=g.watchPosition.bind(g);g.getCurrentPosition=function(a,e){if(hier())return gp.apply(this,arguments);zeige('native','Standort');e&&e({code:1,message:'Noki'});};g.watchPosition=function(a,e){if(hier())return wp.apply(this,arguments);zeige('native','Standort');e&&e({code:1,message:'Noki'});return 0;};}catch(_){}\
addEventListener('click',ev=>{if(hier())return;const a=ev.target&&ev.target.closest&&ev.target.closest('a[href]');\
if(a&&((a.target&&!/^_(self|top|parent)$/i.test(a.target))||ev.metaKey||ev.ctrlKey||ev.button===1)){ev.preventDefault();ev.stopImmediatePropagation();if(!neu(a.href))zeige('native','Neuer Tab');}\
else if(ev.target&&ev.target.closest&&ev.target.closest('input[type=file],label')&&(ev.target.closest('input[type=file]')||ev.target.closest('label')?.control?.type==='file')){ev.preventDefault();zeige('native','Dateiauswahl');}},true);\
for(const [k,n] of [[Element.prototype,'requestFullscreen'],[Element.prototype,'webkitRequestFullscreen'],[Element.prototype,'webkitRequestFullScreen'],[HTMLVideoElement.prototype,'webkitEnterFullscreen']]){const f=k[n];if(f)k[n]=function(){if(hier())return f.apply(this,arguments);zeige('native','Vollbild');return Promise.reject(new DOMException('Noki','NotAllowedError'));};}\
addEventListener('submit',ev=>{if(!hier()&&ev.target&&ev.target.target&&!/^_(self|top|parent)$/i.test(ev.target.target)){ev.preventDefault();zeige('native','Formular in neuem Fenster');}},true);})()";

/// One persistent browser-level DevTools session attaches to every page and
/// frame of the Noki Browser and installs the dialog guard (scripts added by
/// a session live as long as that session - hence persistent).
pub fn waechter_starten() {
    static GESTARTET: OnceLock<()> = OnceLock::new();
    if GESTARTET.set(()).is_err() { return; }
    let _ = std::thread::Builder::new().name("noki-browser-guard".into()).spawn(|| loop {
        let Some(port) = port() else { std::thread::sleep(Duration::from_secs(3)); continue };
        let Some(mut b) = browser_ws(port) else { std::thread::sleep(Duration::from_secs(3)); continue };
        let _ = b.s.set_read_timeout(None);
        let mut n = 1u64;
        if b.senden(&serde_json::json!({ "id": n, "method": "Target.setDiscoverTargets", "params": { "discover": true } }).to_string()).is_none() {
            std::thread::sleep(Duration::from_secs(3)); continue;
        }
        let mut bewacht: Vec<String> = Vec::new();
        let mut sitzungen: Vec<String> = Vec::new();
        let mut hier_alt = false;
        let mut gesendet = Instant::now();
        loop {
            // Poll: new DevTools message, or a tick that tells every page
            // whether the user is on the Noki Space right now.
            let _ = b.s.set_read_timeout(Some(Duration::from_millis(300)));
            let mut kopf = [0u8; 1];
            match b.s.peek(&mut kopf) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                    let nk = crate::vorschau::noki_space_id();
                    let hier = nk != 0 && crate::cgs::aktiver_space().map(|a| a.0) == Some(nk);
                    if hier != hier_alt || (hier && gesendet.elapsed() > Duration::from_secs(1)) {
                        for sz in &sitzungen {
                            n += 1;
                            let _ = b.senden(&serde_json::json!({ "id": n, "sessionId": sz, "method": "Runtime.evaluate",
                                "params": { "expression": format!("window.__nokiHier={hier}") } }).to_string());
                        }
                        hier_alt = hier;
                        gesendet = Instant::now();
                    }
                    continue;
                }
                Err(_) => break,
            }
            let _ = b.s.set_read_timeout(Some(Duration::from_secs(3)));
            let Some(text) = b.lesen() else { break };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
            match v["method"].as_str().unwrap_or("") {
                "Target.targetCreated" | "Target.targetInfoChanged" => {
                    let i = &v["params"]["targetInfo"];
                    let id = i["targetId"].as_str().unwrap_or("").to_string();
                    let typ = i["type"].as_str().unwrap_or("");
                    if (typ == "page" || typ == "iframe") && !id.is_empty() && !bewacht.contains(&id) {
                        bewacht.push(id.clone());
                        n += 1;
                        let _ = b.senden(&serde_json::json!({ "id": n, "method": "Target.attachToTarget",
                            "params": { "targetId": id, "flatten": true } }).to_string());
                    }
                }
                "Target.attachedToTarget" => {
                    let sitzung = v["params"]["sessionId"].as_str().unwrap_or("").to_string();
                    if !sitzungen.contains(&sitzung) { sitzungen.push(sitzung.clone()); }
                    let url = v["params"]["targetInfo"]["url"].as_str().unwrap_or("").to_string();
                    // Page.enable: without it the new-document script is never
                    // applied (measured: guard missing after a reload).
                    for (methode, params) in [
                        ("Page.enable", serde_json::json!({})),
                        ("Page.addScriptToEvaluateOnNewDocument", serde_json::json!({ "source": DIALOG_JS })),
                        ("Runtime.evaluate", serde_json::json!({ "expression": DIALOG_JS })),
                        ("Runtime.runIfWaitingForDebugger", serde_json::json!({})),
                    ] {
                        n += 1;
                        let _ = b.senden(&serde_json::json!({ "id": n, "sessionId": sitzung, "method": methode, "params": params }).to_string());
                    }
                    crate::virtual_workspace::trace(&format!("[BROWSER] dialog_guard attached url={}", url.chars().take(80).collect::<String>()));
                }
                "Target.targetDestroyed" => {
                    let id = v["params"]["targetId"].as_str().unwrap_or("");
                    bewacht.retain(|x| x != id);
                }
                "Target.detachedFromTarget" => {
                    let sz = v["params"]["sessionId"].as_str().unwrap_or("");
                    sitzungen.retain(|x| x != sz);
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    });
}

// ---------------------------------------------------------------------
//  Window <-> page mapping
// ---------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Seite { pub id: String, pub url: String, pub titel: String, pub ws: String }

pub fn seiten(port: u16) -> Vec<Seite> {
    let Some(text) = http_get(port, "/json") else { return vec![] };
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    v.as_array().into_iter().flatten().filter(|t| t["type"] == "page").map(|t| Seite {
        id: t["id"].as_str().unwrap_or("").into(),
        url: t["url"].as_str().unwrap_or("").into(),
        titel: t["title"].as_str().unwrap_or("").into(),
        ws: t["webSocketDebuggerUrl"].as_str().unwrap_or("")
            .trim_start_matches(&format!("ws://127.0.0.1:{port}")).into(),
    }).collect()
}

fn browser_ws(port: u16) -> Option<Ws> {
    let text = std::fs::read_to_string(profil_pfad().join("DevToolsActivePort")).ok()?;
    let pfad = text.lines().nth(1)?.trim().to_string();
    Ws::verbinden(port, &pfad)
}

/// The VISIBLE page (active tab) of the native window `wid`, matched by the
/// window bounds DevTools reports for each tab. Cached for 2 s.
pub fn seite_fuer_fenster(port: u16, wid: i64) -> Option<Seite> {
    // Cached while the window frame is unchanged; the worker re-validates
    // "still the visible tab" on its open connection (1 call) and drops the
    // entry on a tab switch (`vergessen`).
    let rahmen = rahmen(wid)?; // [x, y, w, h]
    if let Some(s) = CACHE.lock().ok().and_then(|c| c.iter().find(|e| e.0 == wid && e.2 == rahmen).map(|e| e.1.clone())) {
        return Some(s);
    }
    let treffer = seite_suchen(port, rahmen);
    if let (Ok(mut c), Some(s)) = (CACHE.lock(), treffer.clone()) {
        c.retain(|e| e.0 != wid);
        c.push((wid, s, rahmen));
    }
    treffer
}

/// Uncached: the visible page whose Chrome window has exactly this frame.
fn seite_suchen(port: u16, rahmen: [f64; 4]) -> Option<Seite> {
    let mut b = browser_ws(port)?;
    let mut treffer: Option<Seite> = None;
    for s in seiten(port) {
        let Some(r) = b.rufen("Browser.getWindowForTarget", serde_json::json!({ "targetId": s.id })) else { continue };
        let bb = &r["bounds"];
        let (l, t, w, h) = (bb["left"].as_f64().unwrap_or(-1.0), bb["top"].as_f64().unwrap_or(-1.0),
            bb["width"].as_f64().unwrap_or(0.0), bb["height"].as_f64().unwrap_or(0.0));
        if (l - rahmen[0]).abs() > 3.0 || (t - rahmen[1]).abs() > 3.0 || (w - rahmen[2]).abs() > 3.0 || (h - rahmen[3]).abs() > 3.0 { continue; }
        // Several tabs share a window: the active one is the visible one.
        let Some(mut p) = Ws::verbinden(port, &s.ws) else { continue };
        if p.js("document.visibilityState") == Some(serde_json::json!("visible")) { treffer = Some(s); break; }
    }
    treffer
}

static CACHE: Mutex<Vec<(i64, Seite, [f64; 4])>> = Mutex::new(Vec::new());
/// Last full window->page validation of the scroll fast path.
static GEPRUEFT: Mutex<Option<(i64, Instant)>> = Mutex::new(None);
/// Forget the window->tab mapping (tab switch, navigation to a new target).
pub fn vergessen(wid: i64) {
    if let Ok(mut c) = CACHE.lock() { c.retain(|e| e.0 != wid); }
    if let Ok(mut g) = GEPRUEFT.lock() { if g.is_some_and(|g| g.0 == wid) { *g = None; } }
}

pub fn neuer_tab(port: u16, url: &str) -> Result<(), String> {
    let mut b = browser_ws(port).ok_or("Noki Browser nicht erreichbar")?;
    // newWindow=false: a TAB in the existing Noki Browser window on the Noki
    // Space - never a new window on the user's Desktop. background=true:
    // a foreground createTarget ACTIVATES Chrome and macOS follows it to the
    // Noki Space (measured 2026-09-27: 1 -> 2796). The tab is then selected
    // through its AX tab button (no activation).
    let r = b.rufen("Target.createTarget", serde_json::json!({ "url": url, "newWindow": false, "background": true }))
        .ok_or("Tab konnte nicht geöffnet werden")?;
    let ziel = r["targetId"].as_str().unwrap_or_default().to_string();
    let Some(p) = pid() else { return Ok(()) };
    let Some(wid) = crate::cgs::alle_fenster().into_iter().filter(|f| f.1 == p).max_by_key(|f| f.6 * f.7).map(|f| f.0)
        else { return Ok(()) };
    let bis = Instant::now() + Duration::from_secs(5);
    let mut titel = String::new();
    while Instant::now() < bis {
        if let Some(i) = b.rufen("Target.getTargetInfo", serde_json::json!({ "targetId": ziel })) {
            let t = i["targetInfo"]["title"].as_str().unwrap_or_default().to_string();
            let u = i["targetInfo"]["url"].as_str().unwrap_or_default().to_string();
            if !t.is_empty() && t != u && !u.contains(&t) { titel = t; break; }
            titel = t;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let gewaehlt = crate::vorschau::fernbedienung::browser_tab_waehlen(p, wid, &titel);
    if gewaehlt { vergessen(wid); }
    crate::virtual_workspace::trace(&format!("[BROWSER] new_tab background=true title={titel:?} selected={gewaehlt}"));
    Ok(())
}

// ---------------------------------------------------------------------
//  Own interaction queue (never the shared Miniatur worker)
// ---------------------------------------------------------------------

/// Scroll target under a global point: innermost scrollable ancestor that can
/// still move in that direction (open shadow roots included), else the page.
const ROLLEN_JS: &str = "((gx,gy,dx,dy)=>{const vx=gx-screenX-(outerWidth-innerWidth)/2, vy=gy-screenY-(outerHeight-innerHeight);\
let e=document.elementFromPoint(vx,vy);\
while(e&&e.shadowRoot){const i=e.shadowRoot.elementFromPoint(vx,vy);if(!i||i===e)break;e=i;}\
if(e&&(e.tagName==='IFRAME'||e.tagName==='FRAME')){try{if(!e.contentDocument)return 'iframe';}catch(_){return 'iframe';}}\
const root=document.scrollingElement;\
while(e&&e!==root&&e!==document.body&&e!==document.documentElement){\
const s=getComputedStyle(e),rx=/(auto|scroll|overlay)/;\
const y=dy&&rx.test(s.overflowY)&&e.scrollHeight>e.clientHeight+1&&(dy<0?e.scrollTop>0:e.scrollTop+e.clientHeight<e.scrollHeight-1);\
const x=dx&&rx.test(s.overflowX)&&e.scrollWidth>e.clientWidth+1&&(dx<0?e.scrollLeft>0:e.scrollLeft+e.clientWidth<e.scrollWidth-1);\
if(y||x){e.scrollBy(x?dx:0,y?dy:0);return 'el:'+(e.id||e.tagName)+':'+Math.round(e.scrollTop);}\
e=e.parentElement||(e.getRootNode&&e.getRootNode().host)||null;}\
window.scrollBy(dx,dy);return 'page:'+Math.round(scrollY);})";

/// Smooth scroll engine inside the page (v6). Input packets only ADD to a
/// pending delta; the page's own frame clock (rAF, 60/s also on the hidden
/// Space thanks to --disable-gpu-vsync) moves a share of it every frame.
/// Uneven packet arrival (threads, DevTools round trips) therefore becomes
/// even per-frame motion instead of 0-2 packets per frame. The target is
/// latched per gesture like a real wheel (innermost scroller under the
/// point that can move that way, else the page) - on the first packet
/// that HAS a delta: a trackpad's "began" often carries 0,0, and latching
/// on it picked the page instead of the inner list (measured: nested list
/// did not move for slow gestures). Remainders that cannot
/// move (edge) are dropped. After a main-thread stall (measured: YouTube
/// loading a video blocks it; neither JS nor DevTools wheel input moves the
/// hidden page meanwhile) the backlog drains at 30%/frame instead of one
/// jump; only a page whose frame clock never resumes (background tab) gets
/// the rest applied at once after 250 ms.
/// Horizontal two-finger swipe where nothing can scroll sideways = Back /
/// Forward, like Chrome's own trackpad gesture (app-mode windows such as
/// Noki YouTube have no back button). Once per gesture, after 150 px (~95 pt of finger travel).
const ROLL_ENGINE_JS: &str = "(()=>{if(window.__nokiRollV===7)return;window.__nokiRollV=7;\
const S={el:null,px:0,py:0,run:false,t:-1e9,f:0,ox:0,nav:false,neu:true};\
const pick=(gx,gy,dx,dy)=>{const vx=gx-screenX-(outerWidth-innerWidth)/2,vy=gy-screenY-(outerHeight-innerHeight);\
let e=document.elementFromPoint(vx,vy);while(e&&e.shadowRoot){const i=e.shadowRoot.elementFromPoint(vx,vy);if(!i||i===e)break;e=i;}\
if(e&&(e.tagName==='IFRAME'||e.tagName==='FRAME')){try{if(!e.contentDocument)return 'iframe';}catch(_){return 'iframe';}}\
const root=document.scrollingElement;\
while(e&&e!==root&&e!==document.body&&e!==document.documentElement){const s=getComputedStyle(e),rx=/(auto|scroll|overlay)/;\
const y=dy&&rx.test(s.overflowY)&&e.scrollHeight>e.clientHeight+1&&(dy<0?e.scrollTop>0:e.scrollTop+e.clientHeight<e.scrollHeight-1);\
const x=dx&&rx.test(s.overflowX)&&e.scrollWidth>e.clientWidth+1&&(dx<0?e.scrollLeft>0:e.scrollLeft+e.clientWidth<e.scrollWidth-1);\
if(y||x)return e;e=e.parentElement||(e.getRootNode&&e.getRootNode().host)||null;}return null;};\
const pos=el=>el?[el.scrollLeft,el.scrollTop]:[scrollX,scrollY];\
const by=(el,x,y)=>{const o={left:x,top:y,behavior:'instant'};if(el)el.scrollBy(o);else window.scrollBy(o);};\
const step=all=>{const bl=Math.hypot(S.px,S.py)>3*(S.v||0)+60;const k=all?1:(bl?0.3:0.55);let sx=Math.abs(S.px)<1.5?S.px:S.px*k,sy=Math.abs(S.py)<1.5?S.py:S.py*k;\
const a=pos(S.el);by(S.el,sx,sy);const b=pos(S.el),mx=b[0]-a[0],my=b[1]-a[1];\
S.px=(Math.abs(mx)<0.01&&(Math.abs(sx)>=1||Math.abs(S.px)<0.6))?0:S.px-mx;\
S.py=(Math.abs(my)<0.01&&(Math.abs(sy)>=1||Math.abs(S.py)<0.6))?0:S.py-my;};\
const frame=()=>{S.f=performance.now();step(false);if(Math.abs(S.px)>0.05||Math.abs(S.py)>0.05)requestAnimationFrame(frame);else{S.px=S.py=0;S.run=false;}};\
window.__nokiRoll=(gx,gy,dx,dy,neu)=>{const now=performance.now();\
if(neu||now-S.t>300||(S.el&&!S.el.isConnected)){S.neu=true;S.ox=0;S.nav=false;}\
if(S.neu&&(dx||dy)){const p=pick(gx,gy,dx,dy);if(p==='iframe')return 'iframe';if(p!==S.el){S.px=S.py=0;}S.el=p;S.neu=false;}\
if(dx&&Math.abs(dx)>2*Math.abs(dy)){const e=S.el||document.scrollingElement;\
const cx=e.scrollWidth>e.clientWidth+1&&(dx<0?e.scrollLeft>0:e.scrollLeft+e.clientWidth<e.scrollWidth-1);\
if(!cx){S.ox=(S.ox||0)+dx;dx=0;if(!S.nav&&Math.abs(S.ox)>150){S.nav=true;S.t=now;if(S.ox<0)history.back();else history.forward();return 'v'+'nav:'+(S.ox<0?'back':'forward');}}}\
S.t=now;S.px+=dx;S.py+=dy;S.v=0.8*(S.v||0)+0.2*Math.hypot(dx,dy);\
if(!S.run){S.run=true;S.f=now;requestAnimationFrame(frame);}\
clearTimeout(S.w);S.w=setTimeout(()=>{if(S.run&&performance.now()-S.f>200)step(true);},250);\
const q=pos(S.el);return document.visibilityState[0]+(S.el?'el:'+(S.el.id||S.el.tagName):'page')+':'+Math.round(q[1]);};})()";

/// Right click on the page: pointer/mouse down+up (button 2) and the
/// contextmenu event at that point. "page_menu" = the page handled it.
const KONTEXT_JS: &str = "((x,y)=>{let e=document.elementFromPoint(x,y);while(e&&e.shadowRoot){const i=e.shadowRoot.elementFromPoint(x,y);if(!i||i===e)break;e=i;}\
if(!e)return 'none';const o={bubbles:true,cancelable:true,composed:true,clientX:x,clientY:y,button:2,buttons:2,view:window};\
e.dispatchEvent(new PointerEvent('pointerdown',o));e.dispatchEvent(new MouseEvent('mousedown',o));\
e.dispatchEvent(new PointerEvent('pointerup',{...o,buttons:0}));e.dispatchEvent(new MouseEvent('mouseup',{...o,buttons:0}));\
const ok=!e.dispatchEvent(new MouseEvent('contextmenu',o));return ok?'page_menu':'native_menu';})";

/// A click on a <select>: Chrome's own popup is a native NSMenu of a hidden
/// app (modal, outside the Noki Space). Instead the options open as an
/// in-page list at the control - visible in the Miniatur, chosen with a
/// normal click; the real input/change events fire. Outside click closes.
const AUSWAHL_JS: &str = "((x,y)=>{let e=document.elementFromPoint(x,y);while(e&&e.shadowRoot){const i=e.shadowRoot.elementFromPoint(x,y);if(!i||i===e)break;e=i;}\
if(!e||!e.closest)return 'none';if(e.closest('#__noki_sel'))return 'none';\
const s=e.closest('select');if(!s||s.multiple||s.size>1||s.disabled)return 'none';\
document.getElementById('__noki_sel')?.remove();const r=s.getBoundingClientRect(),h=document.createElement('div');h.id='__noki_sel';\
const unten=innerHeight-r.bottom-8,oben=r.top-8,nachOben=unten<160&&oben>unten;\
h.style.cssText='position:fixed;left:'+r.left+'px;'+(nachOben?'bottom:'+(innerHeight-r.top)+'px;':'top:'+r.bottom+'px;')+'min-width:'+r.width+'px;max-height:'+Math.max(120,nachOben?oben:unten)+'px;overflow:auto;z-index:2147483647;background:#fff;color:#111;border:1px solid #999;border-radius:6px;box-shadow:0 6px 18px rgba(0,0,0,.25);font:13px -apple-system,system-ui;padding:4px 0;color-scheme:light';\
[...s.options].forEach((o,i)=>{const d=document.createElement('div');d.textContent=o.text;d.dataset.i=i;\
d.style.cssText='padding:5px 14px;white-space:nowrap;'+(o.disabled?'opacity:.4;':'')+(i===s.selectedIndex?'background:#0a64d8;color:#fff;':'');\
d.addEventListener('click',ev=>{ev.stopPropagation();ev.preventDefault();if(o.disabled)return;if(s.selectedIndex!==i){s.selectedIndex=i;s.dispatchEvent(new Event('input',{bubbles:true}));s.dispatchEvent(new Event('change',{bubbles:true}));}h.remove();});h.appendChild(d);});\
const weg=ev=>{if(!h.contains(ev.target)){h.remove();removeEventListener('pointerdown',weg,true);removeEventListener('mousedown',weg,true);}};\
setTimeout(()=>{addEventListener('pointerdown',weg,true);addEventListener('mousedown',weg,true);},0);\
document.documentElement.appendChild(h);s.focus({preventScroll:true});h.querySelector('[data-i=\"'+s.selectedIndex+'\"]')?.scrollIntoView({block:'nearest'});return 'select_opened';})";

enum Auftrag {
    Rollen { wid: i64, x: f64, y: f64, dx: f64, dy: f64 },
    /// Page pixels for the engine; `neu` = a new gesture starts (re-latch);
    /// `t_ein` = input time (epoch ms) of the packet, for latency tracing.
    RollenFein { wid: i64, x: f64, y: f64, dx: f64, dy: f64, neu: bool, t_ein: f64 },
    Klick { pid: i32, wid: i64, x: f64, y: f64, klicks: i64 },
    Taste { wid: i64, code: i64, text: String, befehl: Option<char> },
    /// art: 0 hover move, 1 press, 2 drag move, 3 release.
    Zeiger { wid: i64, x: f64, y: f64, art: u8 },
}

static QUEUE_DEPTH: AtomicUsize = AtomicUsize::new(0);
static COMPLETED: AtomicU64 = AtomicU64::new(0);
static DROPPED: AtomicU64 = AtomicU64::new(0);
static HEARTBEAT_MS: AtomicU64 = AtomicU64::new(0);

fn jetzt_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn kanal() -> &'static SyncSender<Auftrag> {
    static K: OnceLock<SyncSender<Auftrag>> = OnceLock::new();
    K.get_or_init(|| {
        // Bounded independently from generic AX. The worker drains and
        // coalesces every batch; an unresponsive page can no longer grow an
        // unbounded process-wide queue.
        let (tx, rx) = sync_channel::<Auftrag>(256);
        let _ = std::thread::Builder::new().name("noki-browser".into()).spawn(move || arbeiter(rx));
        tx
    })
}

fn einreihen(a: Auftrag) {
    match kanal().try_send(a) {
        Ok(()) => { QUEUE_DEPTH.fetch_add(1, Ordering::Relaxed); }
        Err(TrySendError::Full(_)) => {
            DROPPED.fetch_add(1, Ordering::Relaxed);
            crate::virtual_workspace::trace("[HEALTH] browser_queue full depth=256 stale_operation_dropped=true");
        }
        Err(TrySendError::Disconnected(_)) => {
            DROPPED.fetch_add(1, Ordering::Relaxed);
            crate::virtual_workspace::trace("[HEALTH] browser_queue disconnected=true");
        }
    }
}

pub fn health() -> serde_json::Value {
    serde_json::json!({
        "lastHeartbeat": HEARTBEAT_MS.load(Ordering::Relaxed),
        "queueDepth": QUEUE_DEPTH.load(Ordering::Relaxed),
        "completedCount": COMPLETED.load(Ordering::Relaxed),
        "errorCount": DROPPED.load(Ordering::Relaxed),
    })
}

/// Scroll by page pixels at GLOBAL point x,y (positive dy = page scrolls
/// down). The scroller under the point moves, like a real wheel.
pub fn rollen(wid: i64, x: f64, y: f64, dx: f64, dy: f64) { einreihen(Auftrag::Rollen { wid, x, y, dx, dy }); }
/// Trackpad/wheel packet from the Miniatur: `fdx/fdy` = finger points * 0.25
/// (precise) or wheel lines (phase "w"); natural direction like `rollen`.
/// Gain curve on the per-packet speed: a slow finger moves the page 1.4x
/// its points, fast packets up to 1.8x - continuous, no fixed step, no
/// steep acceleration (macOS already accelerates the deltas it reports).
pub fn rollen_fein(wid: i64, x: f64, y: f64, fdx: f64, fdy: f64, phase: &str, t_ein: f64) {
    let (px, py) = if phase == "w" { (fdx * 32.0, fdy * 32.0) } else {
        let (ptx, pty) = (fdx / 0.25, fdy / 0.25);
        let g = |p: f64| 1.4 + 0.4 * (p.abs() / 24.0).min(1.0);
        (ptx * g(ptx.hypot(pty)), pty * g(ptx.hypot(pty)))
    };
    let neu = matches!(phase, "b" | "w");
    if px == 0.0 && py == 0.0 && !neu { return; }
    einreihen(Auftrag::RollenFein { wid, x, y, dx: -px, dy: -py, neu, t_ein });
}
/// Click at GLOBAL screen point (inside the page viewport).
pub fn klicken(pid: i32, wid: i64, x: f64, y: f64, klicks: i64) { einreihen(Auftrag::Klick { pid, wid, x, y, klicks }); }
/// Pointer at GLOBAL point: hover (0), drag press (1) / move (2) / release (3).
pub fn zeiger(wid: i64, x: f64, y: f64, art: u8) { einreihen(Auftrag::Zeiger { wid, x, y, art }); }
/// One key for the page's focused element (browser typing session).
/// `befehl` = Some('a'|'c'|'x'|'v'|'z') for the Cmd editing commands.
pub fn taste(wid: i64, code: i64, text: String, befehl: Option<char>) { einreihen(Auftrag::Taste { wid, code, text, befehl }); }

fn arbeiter(rx: Receiver<Auftrag>) {
    // One open page connection per window, reused.
    let mut verbindung: Option<(i64, String, Ws)> = None;
    loop {
        HEARTBEAT_MS.store(jetzt_ms(), Ordering::Relaxed);
        let erster = match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(a) => a,
            Err(RecvTimeoutError::Timeout) => { verbindung = None; continue; }
            Err(_) => return,
        };
        // Coalesce: everything already queued is merged - old scroll deltas
        // are summed, never replayed one by one.
        let mut auftraege = vec![erster];
        while let Ok(a) = rx.try_recv() { auftraege.push(a); }
        QUEUE_DEPTH.fetch_sub(auftraege.len().min(QUEUE_DEPTH.load(Ordering::Relaxed)), Ordering::Relaxed);
        let mut rollen_summe: Vec<(i64, f64, f64, f64, f64)> = Vec::new();
        // (wid, dx, dy, x, y, neu, oldest input time, packets)
        let mut fein: Vec<(i64, f64, f64, f64, f64, bool, f64, u32)> = Vec::new();
        let mut klicks = Vec::new();
        let mut tasten = Vec::new();
        let mut zeiger_liste: Vec<(i64, f64, f64, u8)> = Vec::new();
        for a in auftraege {
            match a {
                Auftrag::Rollen { wid, x, y, dx, dy } => {
                    if let Some(e) = rollen_summe.iter_mut().find(|e| e.0 == wid) { e.1 += dx; e.2 += dy; e.3 = x; e.4 = y; }
                    else { rollen_summe.push((wid, dx, dy, x, y)); }
                }
                Auftrag::RollenFein { wid, x, y, dx, dy, neu, t_ein } => {
                    match fein.iter_mut().find(|e| e.0 == wid) {
                        Some(e) => { e.1 += dx; e.2 += dy; e.3 = x; e.4 = y; e.5 |= neu; e.7 += 1;
                                     if e.6 == 0.0 || (t_ein > 0.0 && t_ein < e.6) { e.6 = t_ein; } }
                        None => fein.push((wid, dx, dy, x, y, neu, t_ein, 1)),
                    }
                }
                Auftrag::Klick { pid, wid, x, y, klicks: n } => klicks.push((pid, wid, x, y, n)),
                // Keys are never coalesced or reordered.
                Auftrag::Taste { wid, code, text, befehl } => tasten.push((wid, code, text, befehl)),
                // Pointer: order kept; a move replaces the move before it.
                Auftrag::Zeiger { wid, x, y, art } => {
                    if matches!(art, 0 | 2) && zeiger_liste.last().is_some_and(|z: &(i64, f64, f64, u8)| z.0 == wid && z.3 == art) {
                        zeiger_liste.pop();
                    }
                    zeiger_liste.push((wid, x, y, art));
                }
            }
        }
        HEARTBEAT_MS.store(jetzt_ms(), Ordering::Relaxed);
        let Some(port) = port() else { continue };
        let mut seite_ws = |wid: i64, verbindung: &mut Option<(i64, String, Ws)>| -> bool {
            let Some(s) = seite_fuer_fenster(port, wid) else { return false };
            if let Some(v) = verbindung.as_mut().filter(|v| v.0 == wid && v.1 == s.id) {
                if v.2.js("document.visibilityState") == Some(serde_json::json!("visible")) { return true; }
                // The user switched tabs: re-map once.
                vergessen(wid);
                *verbindung = None;
                let Some(s2) = seite_fuer_fenster(port, wid) else { return false };
                *verbindung = Ws::verbinden(port, &s2.ws).map(|w| (wid, s2.id, w));
                return verbindung.is_some();
            }
            *verbindung = Ws::verbinden(port, &s.ws).map(|w| (wid, s.id, w));
            verbindung.is_some()
        };
        for (wid, dx, dy, gx, gy, neu, t_ein, pakete) in fein {
            let t0 = Instant::now();
            // Fast path: the open connection of this window, re-validated by
            // the engine's own answer (visibility) instead of an extra
            // round trip + window-list query per packet.
            let frisch = verbindung.as_ref().is_some_and(|v| v.0 == wid) && GEPRUEFT.lock().is_ok_and(|g| g.is_some_and(|g| g.0 == wid && g.1.elapsed() < Duration::from_secs(2)));
            if !frisch {
                if !seite_ws(wid, &mut verbindung) { continue; }
                if let Ok(mut g) = GEPRUEFT.lock() { *g = Some((wid, Instant::now())); }
            }
            let Some((_, _, w)) = verbindung.as_mut() else { continue };
            let aufruf = format!("window.__nokiRollV===7?__nokiRoll({gx:.1},{gy:.1},{dx:.2},{dy:.2},{neu}):'x'");
            let mut r = w.js(&aufruf);
            if r == Some(serde_json::json!("x")) {
                let _ = w.js(ROLL_ENGINE_JS);
                r = w.js(&aufruf);
            }
            if r == Some(serde_json::json!("iframe")) {
                let vp = w.js(&format!("[{gx:.1}-screenX-(outerWidth-innerWidth)/2, {gy:.1}-screenY-(outerHeight-innerHeight)]"));
                if let Some([vx, vy]) = vp.and_then(|v| v.as_array().map(|a| a.iter().filter_map(|v| v.as_f64()).collect::<Vec<_>>())).and_then(|v| <[f64; 2]>::try_from(v).ok()) {
                    let _ = w.rufen("Input.dispatchMouseEvent", serde_json::json!({
                        "type": "mouseWheel", "x": vx, "y": vy, "deltaX": dx, "deltaY": dy }));
                }
            }
            let txt = r.as_ref().and_then(|v| v.as_str()).unwrap_or("").to_string();
            if r.is_none() { verbindung = None; }
            // The tab went to the background (user switched tabs): re-map.
            if txt.starts_with('h') { vergessen(wid); verbindung = None; if let Ok(mut g) = GEPRUEFT.lock() { *g = None; } }
            let jetzt_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0);
            crate::virtual_workspace::trace(&format!(
                "[BROWSER] scroll wid={wid} dx={dx:.1} dy={dy:.1} packets={pakete} new={neu} pos={txt} ms={} input_age_ms={:.0}",
                t0.elapsed().as_millis(), if t_ein > 0.0 { jetzt_ms - t_ein } else { -1.0 }));
        }
        for (wid, dx, dy, gx, gy) in rollen_summe {
            let t0 = Instant::now();
            if !seite_ws(wid, &mut verbindung) { continue; }
            let Some((_, _, w)) = verbindung.as_mut() else { continue };
            // Exact delta on the scroller UNDER THE POINT (nested containers,
            // app-style pages with an inner main scroller), else the page:
            // continuous, no "scroll into view" jumps, never a wheel event
            // (so a video player under the point never sees seek/volume).
            let r = w.js(&format!("{ROLLEN_JS}({gx:.1},{gy:.1},{dx:.1},{dy:.1})"));
            if r == Some(serde_json::json!("iframe")) {
                // Cross-origin frame: the browser's own wheel routing.
                let vp = w.js(&format!("[{gx:.1}-screenX-(outerWidth-innerWidth)/2, {gy:.1}-screenY-(outerHeight-innerHeight)]"));
                if let Some([vx, vy]) = vp.and_then(|v| v.as_array().map(|a| a.iter().filter_map(|v| v.as_f64()).collect::<Vec<_>>())).and_then(|v| <[f64; 2]>::try_from(v).ok()) {
                    let _ = w.rufen("Input.dispatchMouseEvent", serde_json::json!({
                        "type": "mouseWheel", "x": vx, "y": vy, "deltaX": dx, "deltaY": dy }));
                }
            }
            if r.is_none() { verbindung = None; }
            crate::virtual_workspace::trace(&format!(
                "[BROWSER] scroll wid={wid} dx={dx:.0} dy={dy:.0} pos={} ms={}", r.unwrap_or_default(), t0.elapsed().as_millis()));
        }
        for (wid, gx, gy, art) in zeiger_liste {
            let t0 = Instant::now();
            if !seite_ws(wid, &mut verbindung) { continue; }
            let Some((_, _, w)) = verbindung.as_mut() else { continue };
            let Some(geo) = w.js("[screenX, screenY, outerWidth - innerWidth, outerHeight - innerHeight, innerWidth, innerHeight]") else { verbindung = None; continue };
            let g: Vec<f64> = geo.as_array().map(|a| a.iter().filter_map(|v| v.as_f64()).collect()).unwrap_or_default();
            if g.len() != 6 { continue; }
            let (vx, vy) = (gx - g[0] - g[2] / 2.0, gy - g[1] - g[3]);
            // Outside the page viewport (tab strip, toolbar): not a page pointer.
            if art == 0 && (vy < 0.0 || vx < 0.0 || vx > g[4] || vy > g[5]) { continue; }
            let (typ, buttons, knopf) = match art { 1 => ("mousePressed", 1, "left"), 3 => ("mouseReleased", 0, "left"), 2 => ("mouseMoved", 1, "left"), _ => ("mouseMoved", 0, "none") };
            if art == 1 {
                let _ = w.rufen("Input.dispatchMouseEvent", serde_json::json!({ "type": "mouseMoved", "x": vx, "y": vy, "button": "none", "buttons": 0 }));
            }
            if art == 3 {
                let _ = w.rufen("Input.dispatchMouseEvent", serde_json::json!({ "type": "mouseMoved", "x": vx, "y": vy, "button": "left", "buttons": 1 }));
            }
            let ok = w.rufen("Input.dispatchMouseEvent", serde_json::json!({
                "type": typ, "x": vx, "y": vy, "button": knopf, "buttons": buttons, "clickCount": if art == 0 || art == 2 { 0 } else { 1 } })).is_some();
            if art != 0 {
                crate::virtual_workspace::trace(&format!(
                    "[BROWSER] drag wid={wid} phase={art} viewport={vx:.0},{vy:.0} ok={ok} ms={}", t0.elapsed().as_millis()));
            }
        }
        for (wid, code, text, befehl) in tasten {
            if !seite_ws(wid, &mut verbindung) { continue; }
            let Some((_, _, w)) = verbindung.as_mut() else { continue };
            let ok = taste_senden(w, code, &text, befehl);
            if !ok { verbindung = None; }
        }
        for (pid_, wid, x, y, n) in klicks {
            let t0 = Instant::now();
            // A page that does not answer is blocked (JS alert/confirm/prompt
            // is open): its buttons are browser UI - pressed via AX.
            let blockiert = |grund: &str| {
                let r = crate::vorschau::fernbedienung::browser_ui_druecken(pid_, wid, x, y);
                crate::virtual_workspace::trace(&format!(
                    "[BROWSER] page_unresponsive wid={wid} reason={grund} ui_press={r:?} ms={}", t0.elapsed().as_millis()));
            };
            if !seite_ws(wid, &mut verbindung) { blockiert("no_page"); continue; }
            let Some((_, _, w)) = verbindung.as_mut() else { continue };
            // Global point -> viewport point: the page reports its own window
            // position and chrome insets.
            let Some(geo) = w.js("[screenX, screenY, outerWidth - innerWidth, outerHeight - innerHeight]") else { verbindung = None; blockiert("js_timeout"); continue };
            let g: Vec<f64> = geo.as_array().map(|a| a.iter().filter_map(|v| v.as_f64()).collect()).unwrap_or_default();
            if g.len() != 4 { continue; }
            let (vx, vy) = (x - g[0] - g[2] / 2.0, y - g[1] - g[3]);
            if n < 0 {
                // Right click: the page's own contextmenu event only. A real
                // right button would open Chrome's NATIVE menu (modal NSMenu
                // of a hidden app) - that stays on the Noki Desktop.
                let r = w.js(&format!("{KONTEXT_JS}({vx:.1},{vy:.1})"));
                let seitenmenue = r == Some(serde_json::json!("page_menu"));
                if !seitenmenue {
                    crate::vorschau::hinweis("Das Browser-Kontextmenü ist nur auf dem Noki Schreibtisch verfügbar.");
                }
                crate::virtual_workspace::trace(&format!(
                    "[BROWSER] right_click wid={wid} viewport={vx:.0},{vy:.0} result={} ms={}",
                    r.unwrap_or_default(), t0.elapsed().as_millis()));
                continue;
            }
            if n == 1 && w.js(&format!("{AUSWAHL_JS}({vx:.1},{vy:.1})")) == Some(serde_json::json!("select_opened")) {
                crate::virtual_workspace::trace(&format!(
                    "[BROWSER] select_open wid={wid} viewport={vx:.0},{vy:.0} route=in_page_list ms={}", t0.elapsed().as_millis()));
                continue;
            }
            // A real mouse arrives before it presses: pages with hover-revealed
            // controls (YouTube's auto-hidden bar) ignore a press without it.
            let _ = w.rufen("Input.dispatchMouseEvent", serde_json::json!({
                "type": "mouseMoved", "x": vx, "y": vy, "button": "none", "buttons": 0 }));
            let mut ok = true;
            for (typ, count) in [("mousePressed", n.max(1)), ("mouseReleased", n.max(1))] {
                ok &= w.rufen("Input.dispatchMouseEvent", serde_json::json!({
                    "type": typ, "x": vx, "y": vy, "button": "left", "clickCount": count })).is_some();
            }
            // Did the click focus an editable element of THIS page? Then the
            // keyboard belongs to it (browser typing session) - verified, not
            // assumed.
            let editierbar = w.js("(() => { const e = document.activeElement; return !!e && !e.disabled && !e.readOnly \
                && (e.isContentEditable || e.tagName === 'TEXTAREA' || (e.tagName === 'INPUT' && \
                !/^(button|submit|reset|checkbox|radio|range|color|file|image|hidden)$/i.test(e.type))); })()")
                == Some(serde_json::json!(true));
            // A click that leaves a page text selection (double/triple click)
            // also gets the keyboard, so Cmd+C copies it - like Chrome.
            let auswahl = !editierbar && n >= 2
                && w.js("getSelection().toString().length > 0") == Some(serde_json::json!(true));
            if editierbar || auswahl { crate::fern_tippen::beginnen_browser(pid_, wid); }
            // Links/windows the page wanted in a NEW tab (target=_blank,
            // window.open): a foreground tab from the page activates Chrome
            // and switches the Space (measured) - the guard queues them and
            // they open here as a background tab, selected via AX.
            let neue = w.js("(()=>{const q=window.__nokiNeu||[];window.__nokiNeu=[];return q})()");
            for u in neue.as_ref().and_then(|v| v.as_array()).into_iter().flatten().filter_map(|v| v.as_str()).take(3) {
                let r = neuer_tab(port, u);
                crate::virtual_workspace::trace(&format!("[BROWSER] page_new_tab wid={wid} url={} ok={}", u.chars().take(80).collect::<String>(), r.is_ok()));
                vergessen(wid);
                verbindung = None;
            }
            crate::virtual_workspace::trace(&format!(
                "[BROWSER] click wid={wid} viewport={vx:.0},{vy:.0} clicks={n} ok={ok} editable={editierbar} ms={}", t0.elapsed().as_millis()));
        }
        COMPLETED.fetch_add(1, Ordering::Relaxed);
        HEARTBEAT_MS.store(jetzt_ms(), Ordering::Relaxed);
    }
}

/// Deliver one key to the page's focused element. Text goes in as text
/// (layout-correct, IME-free); editing keys as page key events.
fn taste_senden(w: &mut Ws, code: i64, text: &str, befehl: Option<char>) -> bool {
    if let Some(c) = befehl {
        return match c {
            'a' => w.js("document.execCommand('selectAll'), 1").is_some(),
            'z' => w.js("document.execCommand('undo'), 1").is_some(),
            'c' | 'x' => {
                let t = w.js("String(getSelection() || (document.activeElement && document.activeElement.value \
                    ? document.activeElement.value.substring(document.activeElement.selectionStart, document.activeElement.selectionEnd) : ''))");
                let t = t.and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
                let ok = !t.is_empty() && crate::lesezeichen::pb_text_setzen(&t);
                if ok && c == 'x' { let _ = w.js("document.execCommand('delete'), 1"); }
                crate::virtual_workspace::trace(&format!("[BROWSER] {} chars={} ok={ok}", if c == 'c' { "copy" } else { "cut" }, t.chars().count()));
                ok
            }
            'v' => {
                let t = match crate::lesezeichen::pb_lesen() { Some(crate::lesezeichen::PbInhalt::Text(t, _)) => t, _ => String::new() };
                !t.is_empty() && w.rufen("Input.insertText", serde_json::json!({ "text": t })).is_some()
            }
            _ => false,
        };
    }
    let spezial = match code {
        51 => Some(("Backspace", 8)), 117 => Some(("Delete", 46)), 36 | 76 => Some(("Enter", 13)),
        48 => Some(("Tab", 9)), 123 => Some(("ArrowLeft", 37)), 124 => Some(("ArrowRight", 39)),
        125 => Some(("ArrowDown", 40)), 126 => Some(("ArrowUp", 38)), 115 => Some(("Home", 36)), 119 => Some(("End", 35)),
        _ => None,
    };
    if let Some((key, vk)) = spezial {
        let mut down = serde_json::json!({ "type": "rawKeyDown", "key": key, "code": key, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk });
        if key == "Enter" { down = serde_json::json!({ "type": "keyDown", "key": "Enter", "code": "Enter", "text": "\r", "windowsVirtualKeyCode": 13, "nativeVirtualKeyCode": 13 }); }
        let ok = w.rufen("Input.dispatchKeyEvent", down).is_some();
        let _ = w.rufen("Input.dispatchKeyEvent", serde_json::json!({ "type": "keyUp", "key": key, "code": key, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk }));
        return ok;
    }
    if text.is_empty() || text.chars().any(|c| c.is_control()) { return true; }
    w.rufen("Input.insertText", serde_json::json!({ "text": text })).is_some()
}

/// Top edge of the page viewport in GLOBAL coordinates (below tabs/toolbar).
pub fn inhalt_oben(wid: i64) -> Option<f64> {
    let port = port()?;
    let s = seite_fuer_fenster(port, wid)?;
    let mut w = Ws::verbinden(port, &s.ws)?;
    let g = w.js("[screenY, outerHeight - innerHeight]")?;
    let a = g.as_array()?;
    Some(a.first()?.as_f64()? + a.get(1)?.as_f64()?)
}

/// Navigate the visible tab of `wid`: a URL (scheme added) or a search.
pub fn navigieren(wid: i64, eingabe: &str) {
    let e = eingabe.trim();
    let url = if e.contains("://") { e.to_string() }
        else if !e.contains(' ') && e.contains('.') { format!("https://{e}") }
        else { format!("https://www.google.com/search?q={}", e.split_whitespace().collect::<Vec<_>>().join("+")) };
    let Some(port) = port() else { return };
    let Some(s) = seite_fuer_fenster(port, wid) else { return };
    let ok = Ws::verbinden(port, &s.ws).and_then(|mut w| w.rufen("Page.navigate", serde_json::json!({ "url": url }))).is_some();
    crate::virtual_workspace::trace(&format!("[BROWSER] navigate wid={wid} url={url} ok={ok}"));
}

/// Verification / state read-back (URL, title, scroll, media) of a window.
pub fn zustand(wid: i64) -> Option<serde_json::Value> {
    let port = port()?;
    let s = seite_fuer_fenster(port, wid)?;
    let mut w = Ws::verbinden(port, &s.ws)?;
    w.js("(() => { const v = document.querySelector('video'); const p = document.querySelector('#movie_player'); \
        return { url: location.href, title: document.title, scrollY: Math.round(scrollY), \
        video: v ? { paused: v.paused, t: Math.round(v.currentTime*10)/10, muted: v.muted, volume: v.volume } : null, \
        ytState: p && p.getPlayerState ? p.getPlayerState() : null }; })()")
}

// ---------------------------------------------------------------------
//  Tabs of one exact native window (Shortcut 9 tab paging)
// ---------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Tab { pub id: String, pub titel: String, pub url: String, pub aktiv: bool }

/// All tabs of exactly the native window `wid`: its CDP windowId is the one
/// whose tab bounds equal the window's CG frame; every page target of that
/// windowId belongs to it (never tabs of another Chrome window). Order:
/// target creation order (the order the tabs were opened in).
pub fn tabs_fuer_fenster(wid: i64) -> Option<Vec<Tab>> {
    let port = port()?;
    let r = rahmen(wid)?;
    let mut b = browser_ws(port)?;
    let alle = b.rufen("Target.getTargets", serde_json::json!({}))?;
    let seiten_ws: std::collections::HashMap<String, String> =
        seiten(port).into_iter().map(|s| (s.id.clone(), s.ws)).collect();
    let mut eintraege = Vec::new();
    let mut fenster = None;
    for t in alle["targetInfos"].as_array().into_iter().flatten() {
        if t["type"] != "page" { continue; }
        let id = t["targetId"].as_str().unwrap_or("").to_string();
        let Some(w) = b.rufen("Browser.getWindowForTarget", serde_json::json!({ "targetId": id })) else { continue };
        let fid = w["windowId"].as_i64();
        let bb = &w["bounds"];
        let passt = [bb["left"].as_f64(), bb["top"].as_f64(), bb["width"].as_f64(), bb["height"].as_f64()]
            .iter().zip(r.iter()).all(|(a, c)| a.is_some_and(|a| (a - c).abs() <= 3.0));
        if passt && fenster.is_none() { fenster = fid; }
        eintraege.push((id, t["title"].as_str().unwrap_or("").to_string(), t["url"].as_str().unwrap_or("").to_string(), fid));
    }
    let fid = fenster?;
    let tabs = eintraege.into_iter().filter(|e| e.3 == Some(fid)).map(|(id, titel, url, _)| {
        let aktiv = seiten_ws.get(&id).and_then(|pfad| Ws::verbinden(port, pfad))
            .and_then(|mut p| p.js("document.visibilityState"))
            == Some(serde_json::json!("visible"));
        Tab { id, titel, url, aktiv }
    }).collect();
    Some(tabs)
}

/// Screenshot of one tab's page (also a background tab of this browser,
/// which runs without renderer backgrounding), scaled once inside the page
/// renderer to fit `max_w` x `max_h` DEVICE pixels - sharp, no upscale.
pub fn tab_bild(target_id: &str, max_w: usize, max_h: usize) -> Option<String> {
    let port = port()?;
    let pfad = seiten(port).into_iter().find(|s| s.id == target_id)?.ws;
    let mut p = Ws::verbinden(port, &pfad)?;
    let m = p.js("JSON.stringify([innerWidth, innerHeight, devicePixelRatio])")?;
    let v: Vec<f64> = serde_json::from_str(m.as_str()?).ok()?;
    let (w, h, dpr) = (v.first().copied()?, v.get(1).copied()?, v.get(2).copied().unwrap_or(2.0));
    if w < 2.0 || h < 2.0 { return None; }
    let skala = (max_w as f64 / (w * dpr)).min(max_h as f64 / (h * dpr)).min(1.0);
    let r = p.rufen("Page.captureScreenshot", serde_json::json!({
        "format": "png", "captureBeyondViewport": false,
        "clip": { "x": 0, "y": 0, "width": w, "height": h, "scale": skala }
    }))?;
    Some(format!("data:image/png;base64,{}", r["data"].as_str()?))
}

/// Select exactly this tab in its window (explicit user click).
pub fn tab_aktivieren(target_id: &str) -> bool {
    let Some(port) = port() else { return false };
    let Some(mut b) = browser_ws(port) else { return false };
    b.rufen("Target.activateTarget", serde_json::json!({ "targetId": target_id })).is_some()
}
