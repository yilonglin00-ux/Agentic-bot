//! Noki Kamera - Galerie in Einstellungen › Kamera.
//!
//! * Liste: alle Fotos/Videos in ~/Desktop/Noki Kamera, neueste zuerst.
//! * `nokimedien://localhost/t/<name>?v=<zeit>`: Vorschaubild (JPEG), erzeugt
//!   bei Bedarf vom Helfer `noki-medien` (ImageIO / AVFoundation), auf der
//!   Platte zwischengespeichert, hoechstens zwei gleichzeitig. Die Webseite
//!   fragt nur sichtbare Bilder an (loading="lazy") - kein Polling.
//! * `nokimedien://localhost/f/<name>`: die Datei selbst, mit HTTP-Range
//!   (Video-Wiedergabe springt, ohne die Datei ganz zu laden).
//! * Bearbeiten: Foto als Kopie (Standard) oder bewusst "Original ersetzen"
//!   (das Original wandert vorher in den Papierkorb); Video schneiden und
//!   zuschneiden als NEUE Datei, mit Fortschritt und Abbruch.
//! * Loeschen: nur Dateien direkt im Ordner, nur in den Papierkorb.
use crate::kamera_kern as kern;
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock};
use tauri::Emitter;

static APP: OnceLock<tauri::AppHandle> = OnceLock::new();

fn home() -> PathBuf { PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())) }
pub fn ordner() -> PathBuf { home().join("Desktop").join("Noki Kamera") }
fn cache_ordner() -> PathBuf { home().join("Library/Caches/Noki/kamera") }

fn helfer() -> Option<PathBuf> {
    let b = std::env::current_exe().ok()?.parent()?.join("../Helpers/noki-medien");
    if b.is_file() { return Some(b); }
    let d = home().join("NOKI/.local/kamera/noki-medien");
    d.is_file().then_some(d)
}
fn json_zeile(s: &str) -> Option<serde_json::Value> {
    s.lines().rev().find_map(|z| serde_json::from_str::<serde_json::Value>(z.trim()).ok())
}

// ---- Vorschaubilder ------------------------------------------------------
/// Hoechstens zwei Vorschau-Prozesse zugleich (CPU bleibt ruhig).
static PLAETZE: (Mutex<usize>, Condvar) = (Mutex::new(0), Condvar::new());
struct Platz;
impl Platz {
    fn nehmen() -> Platz {
        let (m, c) = &PLAETZE;
        let mut n = m.lock().unwrap_or_else(|e| e.into_inner());
        while *n >= 2 { n = c.wait(n).unwrap_or_else(|e| e.into_inner()); }
        *n += 1;
        Platz
    }
}
impl Drop for Platz {
    fn drop(&mut self) {
        let (m, c) = &PLAETZE;
        if let Ok(mut n) = m.lock() { *n = n.saturating_sub(1); }
        c.notify_one();
    }
}
fn schluessel(pfad: &Path, name: &str) -> Option<String> {
    let m = std::fs::metadata(pfad).ok()?;
    let zeit = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64;
    Some(kern::cache_schluessel(name, m.len(), zeit))
}
fn meta_lesen(k: &str) -> Option<serde_json::Value> {
    std::fs::read_to_string(cache_ordner().join(format!("{k}.json"))).ok().and_then(|t| serde_json::from_str(&t).ok())
}
/// JPEG-Vorschau (aus dem Cache oder neu erzeugt) + Content-Type.
fn vorschau(name: &str) -> Option<(Vec<u8>, &'static str)> {
    let pfad = kern::pfad_im_ordner(&ordner(), name)?;
    let k = schluessel(&pfad, name)?;
    let cache = cache_ordner();
    let jpg = cache.join(format!("{k}.jpg"));
    if let Ok(b) = std::fs::read(&jpg) { return Some((b, "image/jpeg")); }
    let _platz = Platz::nehmen();
    if let Ok(b) = std::fs::read(&jpg) { return Some((b, "image/jpeg")); }
    std::fs::create_dir_all(&cache).ok()?;
    let tmp = cache.join(format!(".{k}-{}.jpg", std::process::id()));
    let mut meta = None;
    if let Some(h) = helfer() {
        let aus = std::process::Command::new(h).arg("thumb").arg(&pfad).arg(&tmp).arg("360").output().ok()?;
        if aus.status.success() { meta = json_zeile(&String::from_utf8_lossy(&aus.stdout)); }
    }
    if meta.is_none() && kern::art(name) == Some("foto") {
        // Ohne Helfer: sips (macOS) fuer Fotos.
        let ok = std::process::Command::new("/usr/bin/sips").args(["-Z", "360", "-s", "format", "jpeg"]).arg(&pfad).arg("--out").arg(&tmp)
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
        if ok { meta = Some(serde_json::json!({})); }
    }
    if meta.is_none() || std::fs::rename(&tmp, &jpg).is_err() { let _ = std::fs::remove_file(&tmp); return None; }
    let meta = meta.unwrap_or_default();
    let _ = std::fs::write(cache.join(format!("{k}.json")), meta.to_string());
    // Dauer/Groesse an die Seite (einmal je Datei, kein Nachfragen noetig).
    if let Some(app) = APP.get() {
        let _ = app.emit("kamera-medien", serde_json::json!({ "name": name, "w": meta.get("w"), "h": meta.get("h"), "dauer": meta.get("dauer") }));
    }
    std::fs::read(&jpg).ok().map(|b| (b, "image/jpeg"))
}

// ---- Protokoll -----------------------------------------------------------
fn antwort(status: u16, typ: &str, body: Vec<u8>) -> tauri::http::Response<Vec<u8>> {
    tauri::http::Response::builder().status(status).header("Content-Type", typ)
        .header("Access-Control-Allow-Origin", "*").body(body).unwrap_or_default()
}
fn datei_antwort(name: &str, range: Option<&str>) -> tauri::http::Response<Vec<u8>> {
    let Some(pfad) = kern::pfad_im_ordner(&ordner(), name) else { return antwort(404, "text/plain", Vec::new()) };
    let Ok(mut f) = std::fs::File::open(&pfad) else { return antwort(404, "text/plain", Vec::new()) };
    let gesamt = f.metadata().map(|m| m.len()).unwrap_or(0);
    let typ = kern::mime(name);
    let teil = range.and_then(|r| kern::range(r, gesamt, 4 << 20));
    let (start, ende) = match (range, teil) {
        (Some(_), None) => return tauri::http::Response::builder().status(416).header("Content-Range", format!("bytes */{gesamt}"))
            .body(Vec::new()).unwrap_or_default(),
        (_, Some(t)) => t,
        (None, None) => (0, gesamt.saturating_sub(1)),
    };
    let laenge = if gesamt == 0 { 0 } else { ende - start + 1 };
    let mut buf = vec![0u8; laenge as usize];
    if f.seek(SeekFrom::Start(start)).is_err() || f.read_exact(&mut buf).is_err() { return antwort(500, "text/plain", Vec::new()); }
    let mut b = tauri::http::Response::builder().header("Content-Type", typ).header("Accept-Ranges", "bytes")
        .header("Access-Control-Allow-Origin", "*").header("Cache-Control", "no-cache");
    b = if teil.is_some() { b.status(206).header("Content-Range", format!("bytes {start}-{ende}/{gesamt}")) } else { b.status(200) };
    b.header("Content-Length", laenge.to_string()).body(buf).unwrap_or_default()
}
/// Asynchrones Protokoll: alles laeuft auf einem eigenen Thread, nie auf
/// dem Hauptthread.
pub fn protokoll(app: &tauri::AppHandle, anfrage: tauri::http::Request<Vec<u8>>, antworter: tauri::UriSchemeResponder) {
    let _ = APP.set(app.clone());
    let pfad = anfrage.uri().path().to_string();
    let range = anfrage.headers().get("range").and_then(|v| v.to_str().ok()).map(str::to_string);
    std::thread::spawn(move || {
        let (art, roh) = pfad.trim_start_matches('/').split_once('/').unwrap_or(("", ""));
        let Some(name) = kern::url_dekodieren(roh) else { return antworter.respond(antwort(400, "text/plain", Vec::new())) };
        let r = match art {
            "t" => match vorschau(&name) {
                Some((b, typ)) => tauri::http::Response::builder().header("Content-Type", typ).header("Access-Control-Allow-Origin", "*")
                    // Die URL traegt die Aenderungszeit: lange cachen ist sicher.
                    .header("Cache-Control", "max-age=604800").body(b).unwrap_or_default(),
                None => antwort(404, "text/plain", Vec::new()),
            },
            "f" => datei_antwort(&name, range.as_deref()),
            _ => antwort(404, "text/plain", Vec::new()),
        };
        antworter.respond(r);
    });
}

// ---- Befehle -------------------------------------------------------------
#[tauri::command]
pub async fn kamera_medien(app: tauri::AppHandle) -> Vec<kern::Medium> {
    let _ = APP.set(app);
    tauri::async_runtime::spawn_blocking(|| {
        let o = ordner();
        kern::liste(&o).into_iter().map(|mut m| {
            if let Some(v) = schluessel(&o.join(&m.name), &m.name).and_then(|k| meta_lesen(&k)) {
                m.w = v.get("w").and_then(|x| x.as_u64()).map(|x| x as u32);
                m.h = v.get("h").and_then(|x| x.as_u64()).map(|x| x as u32);
                m.dauer = v.get("dauer").and_then(|x| x.as_f64());
            }
            m
        }).collect()
    }).await.unwrap_or_default()
}

/// Bearbeitetes Foto speichern. `daten` = PNG (base64) aus dem Editor.
/// ersetzen=false: neue Datei "<name>-bearbeitet.<endung>" (Standard).
/// ersetzen=true: das Original geht zuerst in den Papierkorb (wiederher-
/// stellbar), dann liegt das Ergebnis unter dem alten Namen.
#[tauri::command]
pub async fn kamera_bild_speichern(name: String, daten: String, ersetzen: bool) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let o = ordner();
        let original = kern::pfad_im_ordner(&o, &name).ok_or("Datei nicht im Ordner Noki Kamera")?;
        if kern::art(&name) != Some("foto") { return Err("Kein Foto".into()); }
        let png = crate::talk_kern::base64_dekodieren(daten.trim_start_matches("data:image/png;base64,"));
        if png.len() < 8 || &png[..8] != b"\x89PNG\r\n\x1a\n" { return Err("Ungueltiges Bild".into()); }
        let endung = kern::endung(&name);
        let ziel_name = if ersetzen { name.clone() } else { kern::freier_name(&o, &name, "bearbeitet", &endung) };
        let roh = o.join(format!(".noki-roh-{}.png", std::process::id()));
        let neu = o.join(format!(".noki-neu-{}.{endung}", std::process::id()));
        std::fs::write(&roh, &png).map_err(|e| e.to_string())?;
        // Format + Metadaten des Originals (Helfer); ohne Helfer nur PNG.
        let ok = match helfer() {
            Some(h) => std::process::Command::new(h).arg("bild").arg(&roh).arg(&neu).arg(&original).output()
                .map(|a| a.status.success()).unwrap_or(false),
            None if endung == "png" => std::fs::rename(&roh, &neu).is_ok(),
            None => false,
        };
        let _ = std::fs::remove_file(&roh);
        if !ok || !neu.is_file() { let _ = std::fs::remove_file(&neu); return Err("Speichern fehlgeschlagen".into()); }
        if ersetzen && !crate::lesezeichen::in_papierkorb(&original.to_string_lossy()) {
            let _ = std::fs::remove_file(&neu);
            return Err("Original konnte nicht in den Papierkorb - nichts ersetzt".into());
        }
        std::fs::rename(&neu, o.join(&ziel_name)).map_err(|e| { let _ = std::fs::remove_file(&neu); e.to_string() })?;
        Ok(ziel_name)
    }).await.map_err(|e| e.to_string())?
}

/// In den Papierkorb - nur eine Datei direkt im Ordner Noki Kamera.
#[tauri::command]
pub async fn kamera_loeschen(name: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let p = kern::pfad_im_ordner(&ordner(), &name).ok_or("Datei nicht im Ordner Noki Kamera")?;
        if let Some(k) = schluessel(&p, &name) {
            let _ = std::fs::remove_file(cache_ordner().join(format!("{k}.jpg")));
            let _ = std::fs::remove_file(cache_ordner().join(format!("{k}.json")));
        }
        if crate::lesezeichen::in_papierkorb(&p.to_string_lossy()) { Ok(()) } else { Err("Papierkorb nicht erreichbar".into()) }
    }).await.map_err(|e| e.to_string())?
}

/// Laufender Video-Export: (pid, Zieldatei).
static EXPORT: Mutex<Option<(u32, PathBuf)>> = Mutex::new(None);

/// Video schneiden (start/ende in s) und zuschneiden (x/y/w/h: 0..1) als
/// NEUE Datei. Fortschritt als Ereignis "kamera-export"; das Original
/// bleibt unberuehrt.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn kamera_video_export(app: tauri::AppHandle, name: String, start: f64, ende: f64, x: f64, y: f64, w: f64, h: f64) -> Result<String, String> {
    let o = ordner();
    let quelle = kern::pfad_im_ordner(&o, &name).ok_or("Datei nicht im Ordner Noki Kamera")?;
    if kern::art(&name) != Some("video") { return Err("Kein Video".into()); }
    let werte = [start, ende, x, y, w, h];
    if werte.iter().any(|v| !v.is_finite()) || ende <= start || w <= 0.0 || h <= 0.0 { return Err("Ungueltiger Bereich".into()); }
    let mut g = EXPORT.lock().map_err(|_| "Export")?;
    if g.is_some() { return Err("Es laeuft bereits ein Export".into()); }
    let helfer = helfer().ok_or("Medien-Helfer fehlt (bauen.sh baut noki-medien)")?;
    let ziel_name = kern::freier_name(&o, &name, "geschnitten", "mov");
    let ziel = o.join(&ziel_name);
    let c = |v: f64| format!("{:.4}", v.clamp(0.0, 1.0));
    let mut kind = std::process::Command::new(helfer).arg("export").arg(&quelle).arg(&ziel)
        .arg(format!("{start:.3}")).arg(format!("{ende:.3}")).arg(c(x)).arg(c(y)).arg(c(w)).arg(c(h))
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null())
        .spawn().map_err(|e| e.to_string())?;
    *g = Some((kind.id(), ziel.clone()));
    drop(g);
    let aus = kind.stdout.take();
    let n = ziel_name.clone();
    std::thread::spawn(move || {
        let mut ende = serde_json::json!({ "name": n, "fehler": "Export abgebrochen" });
        if let Some(aus) = aus {
            for z in std::io::BufReader::new(aus).lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&z) else { continue };
                if let Some(p) = v.get("p").and_then(|p| p.as_f64()) {
                    let _ = app.emit("kamera-export", serde_json::json!({ "name": n, "p": p }));
                } else if v.get("ok").is_some() {
                    ende = serde_json::json!({ "name": n, "fertig": true });
                } else if v.get("abgebrochen").is_some() {
                    ende = serde_json::json!({ "name": n, "abgebrochen": true });
                } else if let Some(f) = v.get("fehler") {
                    ende = serde_json::json!({ "name": n, "fehler": f });
                }
            }
        }
        let _ = kind.wait();
        if ende.get("fertig").is_none() { let _ = std::fs::remove_file(&ziel); }
        if let Ok(mut g) = EXPORT.lock() { *g = None; }
        let _ = app.emit("kamera-export", ende);
    });
    Ok(ziel_name)
}
#[tauri::command]
pub fn kamera_video_abbrechen() -> bool {
    match EXPORT.lock().ok().and_then(|g| g.clone()) {
        // SIGTERM: der Helfer bricht sauber ab und entfernt die halbe Datei.
        Some((pid, _)) => unsafe { libc::kill(pid as i32, libc::SIGTERM) == 0 },
        None => false,
    }
}
