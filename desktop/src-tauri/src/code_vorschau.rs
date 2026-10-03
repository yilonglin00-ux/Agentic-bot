//! Live preview of a Noki code project (`nokicode://localhost/…`).
//!
//! The Ask Noki code workflow builds a project in its own folder. This module
//! makes the result visible and checkable:
//!
//! * `nokicode://<projekt-id>/` serves ONLY files of that code project, read-only,
//!   with the same path rules as the code agent (no absolute paths, no `..`,
//!   no symlink escape). There is no dev server and no process to leave
//!   behind.
//! * The preview window has NO Tauri IPC (its label is in no capability): the
//!   generated page is untrusted code.
//! * An injected script reports what really happened in the page - runtime
//!   errors, unhandled rejections, console errors, whether a canvas / WebGL
//!   context exists - by POSTing to `nokicode://localhost/__noki_report`.
//!   `pruefen` turns that into the `preview.check` tool result.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::Manager;

/// The chat's "current" project (follow-ups continue it). Serving and
/// preview state are per project below - two projects never share them.
static PROJEKT: Mutex<Option<PathBuf>> = Mutex::new(None);
/// Preview host id -> project root (`nokicode://<id>/...`).
static PROJEKTE: Mutex<Option<HashMap<String, PathBuf>>> = Mutex::new(None);
/// Last page report per project id: (run marker, JSON).
static BERICHTE: Mutex<Option<HashMap<String, (u64, String)>>> = Mutex::new(None);
static LAUF: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn projekt_setzen(p: PathBuf) {
    if let Ok(mut g) = PROJEKT.lock() {
        *g = Some(p);
    }
}

pub fn projekt() -> Option<PathBuf> {
    PROJEKT.lock().ok().and_then(|g| g.clone())
}

/// Stable short id of a project (its preview host and window label).
pub fn id_fuer(root: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    root.to_string_lossy().hash(&mut h);
    let id = format!("p{:012x}", h.finish() & 0xffff_ffff_ffff);
    if let Ok(mut g) = PROJEKTE.lock() {
        g.get_or_insert_with(HashMap::new).insert(id.clone(), root.to_path_buf());
    }
    id
}

fn wurzel_fuer(id: &str) -> Option<PathBuf> {
    PROJEKTE.lock().ok().and_then(|g| g.as_ref().and_then(|m| m.get(id).cloned()))
}

fn label(id: &str) -> String {
    format!("code-preview-{id}")
}

/// Injected into every preview page before its own scripts run.
const MELDER: &str = r#"(function(){
  // Power: full frame rate only while the user is looking at / driving in
  // the preview. Unfocused it renders ~10 fps; closed it renders nothing.
  var raf = window.requestAnimationFrame.bind(window);
  window.requestAnimationFrame = function(cb){
    if (document.hasFocus() || /[?&]pruefen=1/.test(location.search)) return raf(cb);
    return setTimeout(function(){ raf(cb); }, 100);
  };
  var fehler = [];
  function merk(t){ try { t = String(t); if (fehler.length < 20 && fehler.indexOf(t) < 0) fehler.push(t.slice(0, 400)); } catch(_){} }
  window.addEventListener('error', function(e){
    if (e && e.target && e.target !== window && (e.target.src || e.target.href)) { merk('Ressource nicht geladen: ' + (e.target.src || e.target.href)); return; }
    merk((e && e.message ? e.message : 'Fehler') + (e && e.filename ? ' (' + String(e.filename).split('/').pop() + ':' + e.lineno + ')' : ''));
  }, true);
  window.addEventListener('unhandledrejection', function(e){ merk('Unbehandelte Promise-Ablehnung: ' + (e && e.reason && (e.reason.message || e.reason))); });
  var ce = console.error; console.error = function(){ try { merk(Array.prototype.map.call(arguments, String).join(' ')); } catch(_){} return ce.apply(console, arguments); };
  // Interactive projects expose their real state; drive it with DOM key
  // events INSIDE this page (no OS input) and measure what changed.
  function steuertest(fertig){
    var z = window.__nokiZustand;
    if (typeof z !== 'function' || !/[?&]pruefen=1/.test(location.search)) { fertig(null); return; }
    function lies(){ try { return JSON.parse(JSON.stringify(z() || {})); } catch(e){ return { fehler: String(e) }; } }
    function taste(typ, key, code){ var ev = new KeyboardEvent(typ, { key: key, code: code, bubbles: true }); window.dispatchEvent(ev); }
    function num(o, ks){ for (var i=0;i<ks.length;i++){ var v=o && o[ks[i]]; if (typeof v==='number' && isFinite(v)) return v; } return null; }
    var X=['x','posX'], Z=['z','posZ'], V=['tempo','speed','geschwindigkeit','v'], L=['lenkung','steer','steering','lenkwinkel'],
        R=['richtung','heading','yaw','winkel'], RAD=['radDrehung','radWinkel','wheelRotation','wheelSpin'], VR=['vorderradWinkel','frontWheelAngle','radEinschlag'];
    function kam(o){ var k = o && (o.kamera || o.camera); return k && typeof k.x==='number' ? k : null; }
    var a = lies();
    taste('keydown','w','KeyW'); taste('keydown','ArrowUp','ArrowUp');
    setTimeout(function(){
      var b = lies();
      taste('keydown','a','KeyA'); taste('keydown','ArrowLeft','ArrowLeft');
      setTimeout(function(){
        var c = lies();
        taste('keyup','a','KeyA'); taste('keyup','ArrowLeft','ArrowLeft');
        taste('keyup','w','KeyW'); taste('keyup','ArrowUp','ArrowUp');
        taste('keydown','s','KeyS'); taste('keydown','ArrowDown','ArrowDown');
        setTimeout(function(){
          var d = lies();
          taste('keyup','s','KeyS'); taste('keyup','ArrowDown','ArrowDown');
          var ka = kam(a), kc = kam(c);
          fertig({
            weg: (num(a,X)!==null && num(c,X)!==null) ? Math.hypot(num(c,X)-num(a,X), (num(c,Z)||0)-(num(a,Z)||0)) : null,
            tempo: num(b,V), tempoNachBremsen: num(d,V), lenkung: num(c,L),
            drehung: (num(c,R)!==null && num(a,R)!==null) ? num(c,R)-num(a,R) : null,
            rad: (num(c,RAD)!==null && num(a,RAD)!==null) ? num(c,RAD)-num(a,RAD) : null,
            vorderrad: num(c,VR),
            kamera: (ka && kc) ? Math.hypot(kc.x-ka.x, (kc.y||0)-(ka.y||0), (kc.z||0)-(ka.z||0)) : null
          });
        }, 1800);
      }, 900);
    }, 1200);
  }
  // Structure of the main object (window.__nokiSzene = {szene, objekt}):
  // parts, share of plain boxes, share of near-black materials, and whether
  // all parts are connected (no floating pieces). Pure measurement.
  function szenencheck(){
    var s = window.__nokiSzene;
    if (!s || !s.objekt || typeof s.objekt.traverse !== 'function') return null;
    try {
      s.objekt.updateMatrixWorld && s.objekt.updateMatrixWorld(true);
      var teile = [], box = 0, schwarz = 0, arten = {};
      s.objekt.traverse(function(o){
        if (!o.isMesh || !o.geometry) return;
        var t = o.geometry.type || ''; arten[t] = (arten[t] || 0) + 1;
        if (/^Box/.test(t)) box++;
        var m = Array.isArray(o.material) ? o.material[0] : o.material;
        if (m && m.color) { var c = m.color; if (0.2126*c.r + 0.7152*c.g + 0.0722*c.b < 0.06) schwarz++; }
        if (!o.geometry.boundingBox && o.geometry.computeBoundingBox) o.geometry.computeBoundingBox();
        var bb = o.geometry.boundingBox; if (!bb) return;
        var e = o.matrixWorld.elements, lo = [1e9,1e9,1e9], hi = [-1e9,-1e9,-1e9];
        [bb.min.x, bb.max.x].forEach(function(x){ [bb.min.y, bb.max.y].forEach(function(y){ [bb.min.z, bb.max.z].forEach(function(z){
          var p = [e[0]*x+e[4]*y+e[8]*z+e[12], e[1]*x+e[5]*y+e[9]*z+e[13], e[2]*x+e[6]*y+e[10]*z+e[14]];
          for (var k=0;k<3;k++){ lo[k]=Math.min(lo[k],p[k]); hi[k]=Math.max(hi[k],p[k]); }
        }); }); });
        teile.push({ lo: lo, hi: hi, typ: t.replace(/Geometry$/, ''), name: o.name || (o.parent && o.parent.name) || '' });
      });
      var n = teile.length; if (!n) return { teile: 0 };
      var glo = [1e9,1e9,1e9], ghi = [-1e9,-1e9,-1e9];
      teile.forEach(function(t){ for (var k=0;k<3;k++){ glo[k]=Math.min(glo[k],t.lo[k]); ghi[k]=Math.max(ghi[k],t.hi[k]); } });
      var tol = 0.02 * Math.max(ghi[0]-glo[0], ghi[1]-glo[1], ghi[2]-glo[2]);
      var eltern = teile.map(function(_, i){ return i; });
      function wurzel(i){ while (eltern[i] !== i) i = eltern[i] = eltern[eltern[i]]; return i; }
      for (var i=0;i<n;i++) for (var j=i+1;j<n;j++) {
        var a = teile[i], b = teile[j], ber = true;
        for (var k=0;k<3;k++) if (a.lo[k] > b.hi[k] + tol || b.lo[k] > a.hi[k] + tol) ber = false;
        if (ber) eltern[wurzel(i)] = wurzel(j);
      }
      var komp = {}; for (var q=0;q<n;q++) komp[wurzel(q)] = 1;
      // Which parts form each group (to find the floating one).
      var gr = {};
      teile.forEach(function(t, q){
        var w = wurzel(q), g = gr[w] || (gr[w] = { teile: 0, typen: {}, namen: {}, lo: [1e9,1e9,1e9], hi: [-1e9,-1e9,-1e9] });
        g.teile++; g.typen[t.typ] = (g.typen[t.typ] || 0) + 1; if (t.name) g.namen[t.name] = 1;
        for (var k=0;k<3;k++){ g.lo[k]=Math.min(g.lo[k],t.lo[k]); g.hi[k]=Math.max(g.hi[k],t.hi[k]); }
      });
      var r2 = function(x){ return Math.round(x * 100) / 100; };
      var gruppen = Object.keys(gr).map(function(w){ var g = gr[w];
        return { teile: g.teile, typen: g.typen, namen: Object.keys(g.namen).slice(0, 6),
          mitte: [0,1,2].map(function(k){ return r2((g.lo[k]+g.hi[k])/2); }), groesse: [0,1,2].map(function(k){ return r2(g.hi[k]-g.lo[k]); }) }; })
        .sort(function(a, b){ return b.teile - a.teile; }).slice(0, 4);
      return { teile: n, boxAnteil: box / n, schwarzAnteil: schwarz / n, komponenten: Object.keys(komp).length, arten: arten, gruppen: gruppen,
        groesse: [ghi[0]-glo[0], ghi[1]-glo[1], ghi[2]-glo[2]] };
    } catch (e) { return { fehler: String(e) }; }
  }
  function bericht(){
    var cv = document.querySelectorAll('canvas'), gl = false, groesse = '';
    for (var i = 0; i < cv.length; i++) {
      if (cv[i].width > 0 && cv[i].height > 0) groesse = cv[i].width + 'x' + cv[i].height;
      try { if (cv[i].getContext('webgl2') || cv[i].getContext('webgl')) gl = true; } catch(_){}
    }
    var body = document.body ? (document.body.innerText || '').trim().slice(0, 200) : '';
    var r = { lauf: +((/[?&]lauf=(\d+)/.exec(location.search) || [0, 0])[1]) || window.__nokiLauf || 0, fehler: fehler, canvas: cv.length, webgl: gl, groesse: groesse,
      titel: document.title || '', text: body, elemente: document.querySelectorAll('*').length };
    r.szene = szenencheck();
    r.szenePflicht = gl && typeof window.__nokiZustand === 'function';
    steuertest(function(st){
      r.steuerung = st;
      r.fehler = fehler;
      try { fetch('nokicode://' + location.host + '/__noki_report', { method: 'POST', body: JSON.stringify(r) }); } catch(_){}
    });
  }
  window.addEventListener('load', function(){ setTimeout(bericht, 2500); });
})();"#;

fn mime(p: &std::path::Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "glb" => "model/gltf-binary",
        "gltf" => "model/gltf+json",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// `nokicode://` handler: project files (read-only) + the page's report.
pub fn protokoll(anfrage: tauri::http::Request<Vec<u8>>) -> tauri::http::Response<Vec<u8>> {
    let antwort = tauri::http::Response::builder();
    let id = anfrage.uri().host().unwrap_or("").to_string();
    let pfad = anfrage.uri().path().to_string();
    if pfad == "/__noki_report" {
        let text = String::from_utf8_lossy(anfrage.body()).chars().take(20_000).collect::<String>();
        let lauf = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v["lauf"].as_u64())
            .unwrap_or(0);
        if let Ok(mut g) = BERICHTE.lock() {
            g.get_or_insert_with(HashMap::new).insert(id, (lauf, text));
        }
        return antwort.status(204).body(Vec::new()).unwrap_or_default();
    }
    let Some(root) = wurzel_fuer(&id) else {
        return antwort.status(404).body(Vec::new()).unwrap_or_default();
    };
    let rel = pfad.trim_start_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };
    let rel = urlencoding_decode(rel);
    let Ok(datei) = crate::code_agent::target(&root, &rel) else {
        return antwort.status(403).body(Vec::new()).unwrap_or_default();
    };
    match std::fs::read(&datei) {
        Ok(inhalt) => antwort
            .header("Content-Type", mime(&datei))
            .header("Cache-Control", "no-store")
            .body(inhalt)
            .unwrap_or_default(),
        Err(_) => antwort.status(404).body(Vec::new()).unwrap_or_default(),
    }
}

fn urlencoding_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn titel(root: &Path) -> String {
    root.file_name()
        .map(|n| format!("Noki Vorschau – {}", n.to_string_lossy()))
        .unwrap_or_else(|| "Noki Vorschau".into())
}

/// Opens (or reloads) THIS project's preview window. `lauf` marks the load
/// so an old report cannot be mistaken for the new one. Checks show the
/// window without taking focus (two builds must not steal the keyboard).
fn laden(app: &tauri::AppHandle, root: &Path, fokus: bool, pruefung: bool) -> Result<u64, String> {
    let id = id_fuer(root);
    eprintln!(
        "[CODE] vorschau laden projekt={} fokus={fokus} pruefung={pruefung}",
        root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    );
    // One preview at a time: every other project's page (and its render
    // loop) closes - hidden previews on other Spaces still cost GPU.
    let eigenes = label(&id);
    for (lbl, w) in app.webview_windows() {
        if lbl.starts_with("code-preview-") && lbl != eigenes {
            let _ = w.close();
        }
    }
    let lauf = LAUF.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let zusatz = if pruefung { "&pruefen=1" } else { "" };
    let url_text = format!("nokicode://{id}/index.html?lauf={lauf}{zusatz}");
    let h = app.clone();
    let titel = titel(root);
    let lbl = label(&id);
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    app.run_on_main_thread(move || {
        let init = format!("window.__nokiLauf = {lauf};\n{MELDER}");
        let r = if let Some(w) = h.get_webview_window(&lbl) {
            let _ = w.set_title(&titel);
            let _ = w.show();
            if fokus {
                let _ = w.set_focus();
            }
            w.eval(&format!("window.location.replace('{url_text}');")).map_err(|e| e.to_string())
        } else {
            match url_text.parse() {
                Ok(u) => tauri::WebviewWindowBuilder::new(&h, &lbl, tauri::WebviewUrl::CustomProtocol(u))
                    .title(titel)
                    .inner_size(960.0, 640.0)
                    .visible(true)
                    .focused(fokus)
                    .initialization_script(&init)
                    .build()
                    .map(|w| {
                        // An accessory app's new window is not ordered in on
                        // the current Space by `visible` alone.
                        let _ = w.show();
                        if fokus {
                            let _ = w.set_focus();
                        }
                    })
                    .map_err(|e| e.to_string()),
                Err(_) => Err("Ungültige Vorschau-Adresse.".into()),
            }
        };
        let _ = tx.send(r);
    })
    .map_err(|e| e.to_string())?;
    rx.recv_timeout(Duration::from_secs(10))
        .map_err(|_| "Vorschaufenster reagiert nicht.".to_string())??;
    Ok(lauf)
}

/// `preview.check` for one project: load the page for real and report what
/// happened. "FEHLER" in the result marks a failed check for the agent.
pub fn pruefen_projekt(app: &tauri::AppHandle, root: &Path) -> Result<String, String> {
    if !root.join("index.html").exists() {
        return Ok("FEHLER: index.html fehlt im Projekt - die Vorschau kann nichts laden.".into());
    }
    let id = id_fuer(root);
    let lauf = laden(app, root, false, true)?;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(15) {
        std::thread::sleep(Duration::from_millis(250));
        let fertig = BERICHTE
            .lock()
            .ok()
            .and_then(|g| g.as_ref().and_then(|m| m.get(&id).cloned()))
            .filter(|(l, _)| *l >= lauf);
        if let Some((_, text)) = fertig {
            return Ok(bericht_text(&text));
        }
    }
    Ok("FEHLER: Die Seite hat nach 15 s keinen Ladebericht geliefert (Skript hängt oder lädt nicht).".into())
}

/// The chat's current project (compatibility for the chat workflow).
pub fn pruefen(app: &tauri::AppHandle) -> Result<String, String> {
    let root = projekt().ok_or("Kein Code-Projekt aktiv.")?;
    pruefen_projekt(app, &root)
}

fn bericht_text(text: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
    let fehler: Vec<String> = v["fehler"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| x.as_str().map(str::to_owned))
        .collect();
    let canvas = v["canvas"].as_u64().unwrap_or(0);
    let webgl = v["webgl"].as_bool().unwrap_or(false);
    let mut out = format!(
        "Seite geladen: {} Elemente, {} Canvas{}{}, Titel „{}“.",
        v["elemente"].as_u64().unwrap_or(0),
        canvas,
        if webgl { ", WebGL aktiv" } else { "" },
        v["groesse"].as_str().filter(|s| !s.is_empty()).map(|s| format!(" ({s})")).unwrap_or_default(),
        v["titel"].as_str().unwrap_or("")
    );
    if let Some(st) = v.get("steuerung").filter(|x| !x.is_null()) {
        let f = |k: &str| st[k].as_f64();
        let (weg, tempo, lenkung, drehung) = (f("weg"), f("tempo"), f("lenkung"), f("drehung"));
        let (tempo_b, rad, vorderrad, kamera) = (f("tempoNachBremsen"), f("rad"), f("vorderrad"), f("kamera"));
        let bewegt = weg.is_some_and(|w| w > 0.05) || tempo.is_some_and(|t| t.abs() > 0.01);
        let lenkt = lenkung.is_some_and(|l| l.abs() > 0.001) || drehung.is_some_and(|d| d.abs() > 0.001);
        let wendet = drehung.is_some_and(|d| d.abs() > 0.001);
        let bremst = matches!((tempo, tempo_b), (Some(a), Some(b)) if b < a - 0.01);
        let raeder = rad.is_some_and(|r| r.abs() > 0.01);
        let einschlag = vorderrad.is_some_and(|v| v.abs() > 0.001);
        let kam = kamera.is_some_and(|k| k > 0.01);
        let zahl = |x: Option<f64>, n: usize| x.map(|x| format!("{x:.n$}")).unwrap_or_else(|| "–".into());
        out.push_str(&format!(
            " Steuertest (Tasten nur in der Vorschau: W/↑ 1,2 s, +A/← 0,9 s, dann S/↓ 1,8 s): Weg {} · Tempo {} → nach Bremsen {} · Lenkung {} · Richtungsänderung {} · Radrotation {} · Vorderrad-Einschlag {} · Kamerabewegung {}.",
            zahl(weg, 2), zahl(tempo, 2), zahl(tempo_b, 2), zahl(lenkung, 3), zahl(drehung, 3), zahl(rad, 2), zahl(vorderrad, 3), zahl(kamera, 2)
        ));
        let mut fehlt = Vec::new();
        if !bewegt { fehlt.push("Gas bewegt das Auto nicht"); }
        if !lenkt || !wendet { fehlt.push("Lenken ändert die Fahrtrichtung nicht"); }
        if !bremst { fehlt.push("S/↓ bremst nicht (tempo sinkt nicht)"); }
        if !raeder { fehlt.push("Räder drehen sich nicht (radDrehung)"); }
        if !einschlag { fehlt.push("Vorderräder schlagen nicht ein (vorderradWinkel)"); }
        let kam_text = format!(
            "Kamera folgt nicht: das Auto fuhr {} Einheiten, kamera {{x,y,z}} bewegte sich {} - pro Frame die Kamera hinter dem Auto nachführen (Ziel = Autoposition + Versatz entgegen der Fahrtrichtung, weich per lerp), lookAt bzw. controls.target = Autoposition",
            zahl(weg, 2),
            zahl(kamera, 2)
        );
        if !kam { fehlt.push(&kam_text); }
        if !fehlt.is_empty() {
            out.push_str(&format!(" FEHLER Steuerung: {} - laut __nokiZustand().", fehlt.join("; ")));
        }
    }
    if let Some(sz) = v.get("szene").filter(|x| !x.is_null()) {
        let teile = sz["teile"].as_u64().unwrap_or(0);
        let box_a = sz["boxAnteil"].as_f64().unwrap_or(0.0);
        let schwarz = sz["schwarzAnteil"].as_f64().unwrap_or(0.0);
        let komp = sz["komponenten"].as_u64().unwrap_or(0);
        out.push_str(&format!(
            " Objekt: {teile} Teile · {:.0} % einfache Boxen · {:.0} % fast schwarz · {komp} zusammenhängende Gruppe(n).",
            box_a * 100.0,
            schwarz * 100.0
        ));
        let mut optik = Vec::new();
        if teile < 12 { optik.push(format!("nur {teile} Teile - zu grob modelliert")); }
        // A vehicle (it reports wheel rotation) is flat: height well below
        // its length. A tall block usually means an ExtrudeGeometry whose
        // depth became the height after rotation.x = -PI/2.
        let fahrzeug = v.get("steuerung").is_some_and(|st| st["rad"].as_f64().is_some());
        if let Some(g) = sz["groesse"].as_array().filter(|g| g.len() == 3 && fahrzeug) {
            let (x, y, z) = (g[0].as_f64().unwrap_or(0.0), g[1].as_f64().unwrap_or(0.0), g[2].as_f64().unwrap_or(0.0));
            let laenge = x.max(z);
            if laenge > 0.0 && y > 0.5 * laenge {
                optik.push(format!(
                    "Proportion: das Fahrzeug ist {y:.2} hoch bei {laenge:.2} Länge und {:.2} Breite - ein Sportwagen ist flach (Höhe etwa 0,25-0,35 × Länge); prüfe die Ausrichtung der Karosserie-Geometrie (bei ExtrudeGeometry mit rotation.x = -PI/2 wird depth zur Höhe)",
                    x.min(z)
                ));
            }
        }
        if box_a > 0.4 { optik.push("überwiegend einfache Boxen statt geformter Geometrie".to_string()); }
        if schwarz > 0.5 { optik.push("überwiegend fast schwarze Materialien - wirkt wie schwarze Blöcke".to_string()); }
        if komp > 1 {
            let gruppen: Vec<String> = sz["gruppen"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
                .map(|(i, g)| {
                    let typen = g["typen"].as_object().map(|o| o.iter().map(|(k, v)| format!("{k}×{v}")).collect::<Vec<_>>().join(", ")).unwrap_or_default();
                    let namen = g["namen"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
                    let v = |k: &str| g[k].as_array().map(|a| a.iter().map(|x| format!("{:.2}", x.as_f64().unwrap_or(0.0))).collect::<Vec<_>>().join("/")).unwrap_or_default();
                    format!(
                        "Gruppe {} ({} Teile: {typen}{}; Mitte x/y/z {}; Größe {})",
                        i + 1,
                        g["teile"].as_u64().unwrap_or(0),
                        if namen.is_empty() { String::new() } else { format!("; Namen {namen}") },
                        v("mitte"),
                        v("groesse")
                    )
                })
                .collect();
            optik.push(format!(
                "{komp} getrennte Teilgruppen - Teile schweben statt verbunden zu sein{}",
                if gruppen.is_empty() { String::new() } else { format!(": {}", gruppen.join(" | ")) }
            ));
        }
        if !optik.is_empty() {
            out.push_str(&format!(" FEHLER Optik: {}.", optik.join("; ")));
        }
    } else if v["szenePflicht"] == true {
        out.push_str(" FEHLER Optik: window.__nokiSzene = { szene, objekt } fehlt - das Hauptobjekt ist nicht prüfbar.");
    }
    if fehler.is_empty() {
        out.push_str(" Keine Laufzeitfehler.");
    } else {
        out.push_str(&format!(" FEHLER ({}):\n- {}", fehler.len(), fehler.join("\n- ")));
    }
    out
}

/// Show a project's preview to the user (buttons in chat / Code Space).
pub fn oeffnen_projekt(app: &tauri::AppHandle, root: &Path, fokus: bool) -> Result<(), String> {
    if !root.join("index.html").exists() {
        return Err("Dieses Projekt hat keine Vorschau (index.html fehlt).".into());
    }
    laden(app, root, fokus, false).map(|_| ())
}

pub fn oeffnen(app: &tauri::AppHandle) -> Result<(), String> {
    let root = projekt().ok_or("Kein Code-Projekt aktiv.")?;
    oeffnen_projekt(app, &root, true)
}

/// Close one project's preview (closing ends its render loop).
pub fn schliessen_projekt(app: &tauri::AppHandle, root: &Path) {
    if let Some(w) = app.get_webview_window(&label(&id_fuer(root))) {
        let _ = w.close();
    }
}

pub fn schliessen(app: &tauri::AppHandle) {
    if let Some(root) = projekt() {
        schliessen_projekt(app, &root);
    }
}

#[cfg(test)]
mod bericht_tests {
    use super::*;

    #[test]
    fn failures_name_what_to_fix() {
        let r = serde_json::json!({
            "elemente": 11, "canvas": 1, "webgl": true, "titel": "T", "fehler": [],
            "steuerung": { "weg": 53.88, "tempo": 1.2, "tempoNachBremsen": -0.6, "lenkung": 0.5, "drehung": 24.1, "rad": 57.9, "vorderrad": 0.5, "kamera": 0.0 },
            "szene": { "teile": 12, "boxAnteil": 0.0, "schwarzAnteil": 0.33, "komponenten": 2,
                "gruppen": [
                    { "teile": 10, "typen": { "Extrude": 4, "Cylinder": 6 }, "namen": [], "mitte": [0.0, 1.2, 0.0], "groesse": [3.0, 2.0, 5.0] },
                    { "teile": 2, "typen": { "Extrude": 2 }, "namen": ["Dach"], "mitte": [0.0, 4.1, -0.5], "groesse": [2.0, 0.3, 2.0] }
                ] }
        });
        let t = bericht_text(&r.to_string());
        assert!(t.contains("das Auto fuhr 53.88 Einheiten, kamera {x,y,z} bewegte sich 0.00"), "{t}");
        assert!(t.contains("Gruppe 2 (2 Teile: Extrude×2; Namen Dach; Mitte x/y/z 0.00/4.10/-0.50"), "{t}");
        assert!(!t.contains("Proportion"), "no size reported, no verdict");
        let mut r2 = r.clone();
        r2["szene"]["groesse"] = serde_json::json!([4.6, 4.6, 5.0]);
        assert!(bericht_text(&r2.to_string()).contains("das Fahrzeug ist 4.60 hoch bei 5.00 Länge"));
        r2["szene"]["groesse"] = serde_json::json!([2.0, 1.3, 4.5]);
        assert!(!bericht_text(&r2.to_string()).contains("Proportion"));
    }
}
