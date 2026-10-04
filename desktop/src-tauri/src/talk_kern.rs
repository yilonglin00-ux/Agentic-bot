//! Noki Talk - the platform independent core: voice activity detection and
//! segmenting, WAV encoding, the client for the LOCAL whisper.cpp server
//! (127.0.0.1 only), transcript clean-up and the local history (SQLite via
//! the existing memory.rs wrapper). No Tauri, no macOS: everything here is
//! unit tested on any machine. Audio never leaves this Mac - the only
//! network peer is the whisper-server on the loopback interface.
use crate::memory::{Db, Val};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Whisper input: mono, 16 kHz, PCM16.
pub const RATE: usize = 16_000;
/// 20 ms analysis frame.
const FRAME: usize = RATE / 50;
/// A pause this long after speech closes a segment (confirmed text).
const STILLE_MS: usize = 650;
/// Less speech than this in a segment is noise, not a word.
const MIN_SPRACHE_MS: usize = 160;
/// Continuous speech without a pause is cut at the quietest point.
const MAX_SEGMENT_MS: usize = 20_000;
/// Forced cuts keep this much audio on both sides (word at the seam).
const UEBERLAPP_MS: usize = 300;
/// Leading silence kept in front of the first speech of a segment.
const VORLAUF_MS: usize = 250;
/// Trailing audio kept after the last speech frame of a segment.
const NACHLAUF_MS: usize = 200;

fn ms(n: usize) -> usize { n * RATE / 1000 }

/// dBFS of a frame of PCM16 samples.
pub fn dbfs(s: &[i16]) -> f32 {
    if s.is_empty() { return -100.0; }
    let e: f64 = s.iter().map(|x| { let v = *x as f64 / 32768.0; v * v }).sum::<f64>() / s.len() as f64;
    (20.0 * (e.sqrt() + 1e-9).log10()) as f32
}

/// Energy VAD with a running noise floor: falls fast, rises slowly while
/// there is no speech, so a fan or room tone does not count as speech.
pub struct Vad { boden: f32 }
impl Default for Vad { fn default() -> Self { Vad { boden: -58.0 } } }
impl Vad {
    pub fn sprache(&mut self, db: f32) -> bool {
        let schwelle = (self.boden + 11.0).max(-50.0);
        let ja = db > schwelle;
        if db < self.boden { self.boden = self.boden * 0.6 + db * 0.4; }
        else if !ja { self.boden += (db - self.boden) * 0.03; }
        ja
    }
}

/// A finished segment [von, bis) in samples; `ueberlapp`: it starts inside
/// the previous one (forced cut) - the joiner removes the doubled words.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment { pub von: usize, pub bis: usize, pub ueberlapp: bool }

/// Streams PCM in, emits closed segments at real pauses. Confirmed segments
/// are never transcribed again; only the open tail is (for the preview).
pub struct Segmentierer {
    pub audio: Vec<i16>,
    vad: Vad,
    frame_db: Vec<f32>,
    seg_start: usize,
    seg_ueberlapp: bool,
    sprache_frames: usize,
    erste_sprache: Option<usize>,
    letzte_sprache_ende: usize,
    stille_frames: usize,
    pub sprache_gesamt_ms: usize,
}
impl Default for Segmentierer {
    fn default() -> Self {
        Segmentierer { audio: Vec::new(), vad: Vad::default(), frame_db: Vec::new(), seg_start: 0, seg_ueberlapp: false,
            sprache_frames: 0, erste_sprache: None, letzte_sprache_ende: 0, stille_frames: 0, sprache_gesamt_ms: 0 }
    }
}
impl Segmentierer {
    fn zuruecksetzen(&mut self, start: usize, ueberlapp: bool) {
        self.seg_start = start; self.seg_ueberlapp = ueberlapp;
        self.sprache_frames = 0; self.erste_sprache = None; self.stille_frames = 0;
    }
    fn hat_sprache(&self) -> bool { self.sprache_frames * 20 >= MIN_SPRACHE_MS }
    pub fn push(&mut self, neu: &[i16]) -> Vec<Segment> {
        self.audio.extend_from_slice(neu);
        let mut fertig = Vec::new();
        while self.frame_db.len() * FRAME + FRAME <= self.audio.len() {
            let a = self.frame_db.len() * FRAME;
            let db = dbfs(&self.audio[a..a + FRAME]);
            self.frame_db.push(db);
            let ende = a + FRAME;
            if self.vad.sprache(db) {
                self.sprache_frames += 1;
                self.sprache_gesamt_ms += 20;
                self.erste_sprache.get_or_insert(a);
                self.letzte_sprache_ende = ende;
                self.stille_frames = 0;
            } else {
                self.stille_frames += 1;
            }
            if self.hat_sprache() && self.stille_frames * 20 >= STILLE_MS {
                // Pause after speech: the segment is confirmed.
                let von = self.erste_sprache.map_or(self.seg_start, |e| e.saturating_sub(ms(VORLAUF_MS)).max(self.seg_start));
                let bis = (self.letzte_sprache_ende + ms(NACHLAUF_MS)).min(ende);
                fertig.push(Segment { von, bis, ueberlapp: self.seg_ueberlapp });
                self.zuruecksetzen(bis, false);
            } else if !self.hat_sprache() && self.stille_frames * 20 >= STILLE_MS {
                // Only silence/noise so far: nothing to keep in front.
                self.zuruecksetzen(ende.saturating_sub(ms(VORLAUF_MS)), false);
            } else if ende - self.seg_start >= ms(MAX_SEGMENT_MS) && self.hat_sprache() {
                // Long speech without a pause: cut at the quietest frame of
                // the last 3 s, keep an overlap so no word is split.
                let von_f = (ende - ms(3000)) / FRAME;
                let bis_f = ende / FRAME;
                let leise = (von_f..bis_f).min_by(|a, b| self.frame_db[*a].total_cmp(&self.frame_db[*b])).unwrap_or(bis_f - 1);
                let schnitt = (leise * FRAME + FRAME / 2).min(ende);
                fertig.push(Segment { von: self.seg_start, bis: schnitt, ueberlapp: self.seg_ueberlapp });
                self.zuruecksetzen(schnitt.saturating_sub(ms(UEBERLAPP_MS)), true);
                self.sprache_frames = MIN_SPRACHE_MS / 20; // it is still speech
                self.erste_sprache = Some(self.seg_start);
            }
        }
        fertig
    }
    /// The open segment (for the live preview / the final tail), if it
    /// contains real speech.
    pub fn offen(&self) -> Option<Segment> {
        if !self.hat_sprache() { return None; }
        let von = self.erste_sprache.map_or(self.seg_start, |e| e.saturating_sub(ms(VORLAUF_MS)).max(self.seg_start));
        Some(Segment { von, bis: self.audio.len(), ueberlapp: self.seg_ueberlapp })
    }
    pub fn dauer_ms(&self) -> u64 { (self.audio.len() * 1000 / RATE) as u64 }
}

/// 16 kHz mono PCM16 WAV.
pub fn wav(pcm: &[i16]) -> Vec<u8> {
    let daten = (pcm.len() * 2) as u32;
    let mut v = Vec::with_capacity(44 + daten as usize);
    v.extend_from_slice(b"RIFF"); v.extend_from_slice(&(36 + daten).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt "); v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes()); v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&(RATE as u32).to_le_bytes()); v.extend_from_slice(&(RATE as u32 * 2).to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes()); v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data"); v.extend_from_slice(&daten.to_le_bytes());
    for s in pcm { v.extend_from_slice(&s.to_le_bytes()); }
    v
}

/// PCM16 LE from the capture helper's base64 lines.
pub fn base64_pcm(b64: &str) -> Vec<i16> {
    let b = base64_dekodieren(b64);
    b.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect()
}
pub fn base64_dekodieren(s: &str) -> Vec<u8> {
    let wert = |c: u8| -> Option<u32> { Some(match c { b'A'..=b'Z' => c - b'A', b'a'..=b'z' => c - b'a' + 26, b'0'..=b'9' => c - b'0' + 52, b'+' => 62, b'/' => 63, _ => return None } as u32) };
    let (mut out, mut acc, mut n) = (Vec::with_capacity(s.len() * 3 / 4), 0u32, 0);
    for c in s.bytes() {
        let Some(v) = wert(c) else { continue };
        acc = (acc << 6) | v; n += 6;
        if n >= 8 { n -= 8; out.push((acc >> n) as u8); acc &= (1 << n) - 1; }
    }
    out
}
pub fn base64_kodieren(b: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let v = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 { s.push(if i <= c.len() { A[(v >> (18 - 6 * i) & 63) as usize] as char } else { '=' }); }
    }
    s
}

// ---- local whisper.cpp server (examples/server: POST /inference) ----------
pub struct Antwort { pub text: String, pub sprache: String }

fn http(port: u16, anfrage: &[u8], frist: Duration) -> Result<(u16, Vec<u8>), String> {
    let adr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut s = TcpStream::connect_timeout(&adr, Duration::from_millis(300)).map_err(|e| format!("whisper-server nicht erreichbar: {e}"))?;
    let _ = s.set_read_timeout(Some(frist));
    let _ = s.set_write_timeout(Some(Duration::from_secs(5)));
    s.write_all(anfrage).map_err(|e| e.to_string())?;
    let mut roh = Vec::new();
    s.read_to_end(&mut roh).map_err(|e| format!("whisper-server antwortet nicht: {e}"))?;
    let kopf_ende = roh.windows(4).position(|w| w == b"\r\n\r\n").ok_or("unvollstaendige Antwort")?;
    let kopf = String::from_utf8_lossy(&roh[..kopf_ende]).to_string();
    let status = kopf.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let mut koerper = roh[kopf_ende + 4..].to_vec();
    if kopf.to_ascii_lowercase().contains("transfer-encoding: chunked") { koerper = entstuecken(&koerper); }
    Ok((status, koerper))
}
fn entstuecken(b: &[u8]) -> Vec<u8> {
    let (mut out, mut i) = (Vec::new(), 0);
    while let Some(z) = b[i..].windows(2).position(|w| w == b"\r\n") {
        let n = usize::from_str_radix(String::from_utf8_lossy(&b[i..i + z]).trim(), 16).unwrap_or(0);
        i += z + 2;
        if n == 0 || i + n > b.len() { break; }
        out.extend_from_slice(&b[i..i + n]);
        i += n + 2;
    }
    out
}
/// Some(true) ready, Some(false) loading the model, None no server.
pub fn gesund(port: u16) -> Option<bool> {
    let a = format!("GET /health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    match http(port, a.as_bytes(), Duration::from_millis(800)) {
        Ok((200, _)) => Some(true),
        Ok(_) => Some(false),
        Err(_) => None,
    }
}
/// Language name in the server's verbose_json -> our code.
pub fn sprachcode(name: &str) -> String {
    match name.to_ascii_lowercase().as_str() {
        "german" | "de" => "de", "english" | "en" => "en", "french" | "fr" => "fr",
        "chinese" | "zh" => "zh", other => return other.to_string(),
    }.to_string()
}
/// One request on the warm server. `sprache`: "auto" | "de" | "en" | "fr" | "zh".
pub fn transkribieren(port: u16, pcm: &[i16], sprache: &str, prompt: &str, frist: Duration) -> Result<Antwort, String> {
    let grenze = format!("noki-talk-{:x}", pcm.len() ^ 0x5eed_u64 as usize);
    let mut k: Vec<u8> = Vec::new();
    let mut feld = |name: &str, wert: &str| {
        k.extend_from_slice(format!("--{grenze}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{wert}\r\n").as_bytes());
    };
    feld("response_format", "verbose_json");
    feld("language", if sprache.is_empty() { "auto" } else { sprache });
    feld("temperature", "0.0");
    feld("no_timestamps", "true");
    if !prompt.trim().is_empty() { feld("prompt", prompt.trim()); }
    k.extend_from_slice(format!("--{grenze}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"talk.wav\"\r\nContent-Type: audio/wav\r\n\r\n").as_bytes());
    k.extend_from_slice(&wav(pcm));
    k.extend_from_slice(format!("\r\n--{grenze}--\r\n").as_bytes());
    let mut a = format!("POST /inference HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: multipart/form-data; boundary={grenze}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", k.len()).into_bytes();
    a.extend_from_slice(&k);
    let (status, koerper) = http(port, &a, frist)?;
    let j: serde_json::Value = serde_json::from_slice(&koerper).map_err(|_| format!("whisper-server: unerwartete Antwort ({status})"))?;
    if status != 200 || j.get("error").is_some() {
        return Err(format!("whisper-server: {}", j.get("error").and_then(|e| e.as_str()).unwrap_or("Fehler")));
    }
    let text = match j.get("segments").and_then(|s| s.as_array()) {
        Some(seg) if !seg.is_empty() => seg.iter().filter_map(|s| s.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join(" "),
        _ => j.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string(),
    };
    let sprache = j.get("language").or_else(|| j.get("detected_language")).and_then(|l| l.as_str()).map(sprachcode).unwrap_or_default();
    Ok(Antwort { text, sprache })
}

// ---- transcript clean-up (local, instant) ---------------------------------
fn cjk(c: char) -> bool { matches!(c as u32, 0x3000..=0x9FFF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF) }
fn norm(w: &str) -> String { w.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect() }
/// Known Whisper hallucinations on silence/noise (only when they are the
/// WHOLE segment - real speech containing these words stays).
const HALLUZINATIONEN: [&str; 10] = [
    "vielen dank fürs zuschauen", "danke fürs zuschauen", "vielen dank", "bis zum nächsten mal",
    "thank you", "thanks for watching", "thank you for watching", "soustitrage st 501", "字幕由amaraorg社区提供", "you",
];
/// Subtitle credits Whisper invents on silence - recognised by their start.
const HALLUZINATION_ANFANG: [&str; 4] = ["untertitel im auftrag", "untertitel der amara", "untertitelung des", "untertitel von"];
/// lower case, letters/digits and single spaces only
fn phrase(t: &str) -> String {
    t.chars().filter(|c| c.is_alphanumeric() || c.is_whitespace()).flat_map(|c| c.to_lowercase())
        .collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}
const GERAEUSCHE: [&str; 12] = ["musik", "music", "applaus", "applause", "lachen", "laughter", "gelächter", "stille", "silence", "geräusche", "noise", "blank_audio"];

/// One raw Whisper segment -> clean text (may become empty).
pub fn segment_bereinigen(roh: &str) -> String {
    let mut t = String::new();
    let mut tiefe = 0;
    for c in roh.chars() {
        match c { '[' => tiefe += 1, ']' if tiefe > 0 => tiefe -= 1, '♪' | '♫' => {}, _ if tiefe == 0 => t.push(c), _ => {} }
    }
    // "(Musik)" / "*Applaus*" style noise labels
    for g in GERAEUSCHE {
        for (a, b) in [("(", ")"), ("*", "*")] {
            loop {
                let low = t.to_lowercase();
                let Some(i) = low.find(&format!("{a}{g}")) else { break };
                let Some(j) = low[i + 1..].find(b).map(|j| i + 1 + j) else { break };
                if j - i > g.len() + 3 { break; }
                t.replace_range(i..=j, " ");
            }
        }
    }
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    let n = phrase(&t);
    if n.is_empty() || HALLUZINATIONEN.iter().any(|h| n == *h) || HALLUZINATION_ANFANG.iter().any(|h| n.starts_with(h)) {
        return String::new();
    }
    wiederholungen_entfernen(&t)
}
/// Whisper loops ("und dann und dann und dann ...") collapse to one.
pub fn wiederholungen_entfernen(t: &str) -> String {
    let mut w: Vec<&str> = t.split_whitespace().collect();
    // Smallest period first: "und dann" x4 collapses as a 2-gram.
    for n in 1..=8 {
        let mindestens = if n == 1 { 3 } else if n == 2 { 3 } else { 2 };
        let mut i = 0;
        while i + n * mindestens <= w.len() {
            let gleich = |a: usize, b: usize| (0..n).all(|k| norm(w[a + k]) == norm(w[b + k]) && !norm(w[a + k]).is_empty());
            let mut r = 1;
            while i + (r + 1) * n <= w.len() && gleich(i, i + r * n) { r += 1; }
            if r >= mindestens { w.drain(i + n..i + r * n); }
            i += 1;
        }
    }
    w.join(" ")
}
/// Appends a confirmed segment. `ueberlapp`: the audio started inside the
/// previous segment - the doubled words at the seam are removed.
pub fn anfuegen(bisher: &str, neu: &str, ueberlapp: bool) -> String {
    let neu = neu.trim();
    if neu.is_empty() { return bisher.to_string(); }
    if bisher.trim().is_empty() { return neu.to_string(); }
    let mut neu_w: Vec<&str> = neu.split_whitespace().collect();
    if ueberlapp {
        let alt_w: Vec<&str> = bisher.split_whitespace().collect();
        for k in (1..=alt_w.len().min(neu_w.len()).min(6)).rev() {
            let a: Vec<String> = alt_w[alt_w.len() - k..].iter().map(|x| norm(x)).collect();
            let b: Vec<String> = neu_w[..k].iter().map(|x| norm(x)).collect();
            if a == b && (k > 1 || a[0].chars().count() >= 3) { neu_w.drain(..k); break; }
        }
        if neu_w.is_empty() { return bisher.to_string(); }
    }
    let neu = neu_w.join(" ");
    let ohne_raum = bisher.chars().last().is_some_and(cjk) || neu.chars().next().is_some_and(cjk);
    format!("{}{}{}", bisher.trim_end(), if ohne_raum { "" } else { " " }, neu)
}
/// Final polish: spacing around punctuation, capital first letter.
pub fn abschliessen(t: &str) -> String {
    let mut s = t.split_whitespace().collect::<Vec<_>>().join(" ");
    for p in [" .", " ,", " !", " ?", " :", " ;"] {
        while s.contains(p) { s = s.replace(p, &p[1..]); }
    }
    let mut c = s.chars();
    match c.next() {
        Some(f) if f.is_lowercase() => f.to_uppercase().chain(c).collect(),
        Some(f) => std::iter::once(f).chain(c).collect(),
        None => String::new(),
    }
}

// ---- history: rolling buffer of the last 30 dictations ---------------------
pub const VERLAUF_MAX: usize = 30;
#[derive(serde::Serialize, Clone, Debug, PartialEq)]
pub struct Eintrag {
    pub id: i64,
    pub created_at: i64,
    pub duration_ms: i64,
    pub transcript: String,
    pub audio_path: String,
    pub language: String,
    pub source_app: String,
}
pub struct Verlauf { db: Db, audio_dir: PathBuf }
impl Verlauf {
    /// `dir`/talk.sqlite and `dir`/audio/. Orphaned audio files (crash
    /// between recording and saving) are removed on open.
    pub fn oeffnen(dir: &Path) -> Result<Self, String> {
        let audio_dir = dir.join("audio");
        std::fs::create_dir_all(&audio_dir).map_err(|e| e.to_string())?;
        let db = Db::open(&dir.join("talk.sqlite"))?;
        db.exec("CREATE TABLE IF NOT EXISTS diktate(id INTEGER PRIMARY KEY AUTOINCREMENT, created_at INTEGER NOT NULL,
                 duration_ms INTEGER NOT NULL, transcript TEXT NOT NULL, audio_path TEXT NOT NULL,
                 language TEXT NOT NULL DEFAULT '', source_app TEXT NOT NULL DEFAULT '');")?;
        let v = Verlauf { db, audio_dir };
        v.waisen_entfernen(&[]);
        Ok(v)
    }
    #[cfg(test)]
    pub fn audio_dir(&self) -> &Path { &self.audio_dir }
    /// Removes audio files no entry references (except `behalten`, e.g. a
    /// recording that is running right now).
    pub fn waisen_entfernen(&self, behalten: &[PathBuf]) -> usize {
        let bekannt: Vec<String> = self.db.query("SELECT audio_path FROM diktate", &[], 1).unwrap_or_default().into_iter().map(|r| r[0].clone()).collect();
        let mut n = 0;
        if let Ok(rd) = std::fs::read_dir(&self.audio_dir) {
            for e in rd.flatten() {
                let p = e.path();
                let s = p.to_string_lossy().to_string();
                if !bekannt.contains(&s) && !behalten.iter().any(|b| *b == p) && std::fs::remove_file(&p).is_ok() { n += 1; }
            }
        }
        n
    }
    fn zeile(r: &[String]) -> Eintrag {
        Eintrag { id: r[0].parse().unwrap_or(0), created_at: r[1].parse().unwrap_or(0), duration_ms: r[2].parse().unwrap_or(0),
            transcript: r[3].clone(), audio_path: r[4].clone(), language: r[5].clone(), source_app: r[6].clone() }
    }
    /// Saves one dictation; entry 31 removes the oldest entry AND its audio.
    /// Returns (new id, removed audio files).
    pub fn speichern(&self, created_at: i64, duration_ms: i64, transcript: &str, audio: &Path, language: &str, source_app: &str) -> Result<(i64, Vec<String>), String> {
        if transcript.trim().is_empty() { return Err("leeres Diktat".into()); }
        self.db.exec("BEGIN IMMEDIATE")?;
        let r = (|| {
            self.db.query("INSERT INTO diktate(created_at, duration_ms, transcript, audio_path, language, source_app) VALUES(?,?,?,?,?,?)",
                &[Val::I(created_at), Val::I(duration_ms), Val::T(transcript), Val::T(&audio.to_string_lossy()), Val::T(language), Val::T(source_app)], 0)?;
            let id: i64 = self.db.query("SELECT last_insert_rowid()", &[], 1)?.first().and_then(|r| r[0].parse().ok()).unwrap_or(0);
            let alt = self.db.query("SELECT id, audio_path FROM diktate ORDER BY id DESC LIMIT -1 OFFSET ?", &[Val::I(VERLAUF_MAX as i64)], 2)?;
            for r in &alt { self.db.query("DELETE FROM diktate WHERE id = ?", &[Val::I(r[0].parse().unwrap_or(-1))], 0)?; }
            Ok::<_, String>((id, alt.into_iter().map(|r| r[1].clone()).collect::<Vec<_>>()))
        })();
        match r {
            Ok((id, weg)) => {
                self.db.exec("COMMIT")?;
                for p in &weg { self.datei_loeschen(p); }
                Ok((id, weg))
            }
            Err(e) => { let _ = self.db.exec("ROLLBACK"); Err(e) }
        }
    }
    fn datei_loeschen(&self, p: &str) {
        let p = Path::new(p);
        if p.starts_with(&self.audio_dir) { let _ = std::fs::remove_file(p); }
    }
    pub fn liste(&self) -> Vec<Eintrag> {
        self.db.query("SELECT id, created_at, duration_ms, transcript, audio_path, language, source_app FROM diktate ORDER BY id DESC LIMIT ?",
            &[Val::I(VERLAUF_MAX as i64)], 7).unwrap_or_default().iter().map(|r| Self::zeile(r)).collect()
    }
    pub fn holen(&self, id: i64) -> Option<Eintrag> {
        self.db.query("SELECT id, created_at, duration_ms, transcript, audio_path, language, source_app FROM diktate WHERE id = ?",
            &[Val::I(id)], 7).ok()?.first().map(|r| Self::zeile(r))
    }
    /// Entry AND its audio file - really gone.
    pub fn loeschen(&self, id: i64) -> bool {
        let Some(e) = self.holen(id) else { return false };
        if self.db.query("DELETE FROM diktate WHERE id = ?", &[Val::I(id)], 0).is_err() { return false; }
        self.datei_loeschen(&e.audio_path);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ton(ms_: usize, amp: f32) -> Vec<i16> {
        (0..ms(ms_)).map(|i| ((i as f32 * 0.07).sin() * amp * 32767.0 * (1.0 + 0.3 * (i as f32 * 0.0013).sin())) as i16).collect()
    }
    fn rauschen(ms_: usize, amp: f32) -> Vec<i16> {
        let mut x: u32 = 12345;
        (0..ms(ms_)).map(|_| { x ^= x << 13; x ^= x >> 17; x ^= x << 5; ((x as f32 / u32::MAX as f32 - 0.5) * 2.0 * amp * 32767.0) as i16 }).collect()
    }
    fn fuettern(s: &mut Segmentierer, a: &[i16]) -> Vec<Segment> {
        a.chunks(1600).flat_map(|c| s.push(c)).collect()
    }

    #[test]
    fn pausen_schliessen_segmente_rauschen_nicht() {
        let mut s = Segmentierer::default();
        let mut a = rauschen(800, 0.004);
        a.extend(ton(1200, 0.3)); a.extend(rauschen(900, 0.004));
        a.extend(ton(700, 0.25)); a.extend(rauschen(900, 0.004));
        let seg = fuettern(&mut s, &a);
        assert_eq!(seg.len(), 2, "{seg:?}");
        assert!(seg[0].von >= ms(500) && seg[0].von <= ms(800), "Vorlauf {:?}", seg[0]);
        assert!(seg[0].bis >= ms(2000) && seg[0].bis <= ms(2300), "Nachlauf {:?}", seg[0]);
        assert!(!seg[0].ueberlapp && !seg[1].ueberlapp);
        assert!(s.offen().is_none(), "nach der Pause ist nichts offen");
        // Pure noise: no segment, nothing open.
        let mut r = Segmentierer::default();
        assert!(fuettern(&mut r, &rauschen(5000, 0.004)).is_empty() && r.offen().is_none());
        // Speech still running: open segment for the preview.
        let mut o = Segmentierer::default();
        fuettern(&mut o, &rauschen(300, 0.004)); fuettern(&mut o, &ton(900, 0.3));
        assert!(o.offen().is_some());
    }

    #[test]
    fn lange_rede_wird_mit_ueberlappung_geschnitten() {
        let mut s = Segmentierer::default();
        let mut a = ton(9000, 0.3); a.extend(ton(400, 0.02)); a.extend(ton(15000, 0.3));
        let seg = fuettern(&mut s, &a);
        assert_eq!(seg.len(), 1, "{seg:?}");
        assert!(seg[0].bis - seg[0].von <= ms(MAX_SEGMENT_MS));
        let offen = s.offen().unwrap();
        assert!(offen.ueberlapp && offen.von < seg[0].bis, "{offen:?}");
    }

    #[test]
    fn base64_rundweg() {
        for daten in [&b""[..], b"a", b"ab", b"abc", b"Noki Talk \x00\xff\x10"] {
            assert_eq!(base64_dekodieren(&base64_kodieren(daten)), daten);
        }
        assert_eq!(base64_kodieren(b"Man"), "TWFu");
        assert_eq!(base64_pcm(&base64_kodieren(&[0x34, 0x12, 0xff, 0xff])), vec![0x1234, -1]);
    }

    #[test]
    fn wav_kopf() {
        let w = wav(&[1, -1, 2]);
        assert_eq!(&w[0..4], b"RIFF"); assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), 16000);
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 6);
        assert_eq!(w.len(), 50);
    }

    #[test]
    fn text_bereinigung() {
        assert_eq!(segment_bereinigen(" Untertitel im Auftrag des ZDF, 2020 "), "");
        assert_eq!(segment_bereinigen("Vielen Dank für die schnelle Hilfe."), "Vielen Dank für die schnelle Hilfe.");
        assert_eq!(segment_bereinigen("Vielen Dank."), "");
        assert_eq!(segment_bereinigen("[Musik]"), "");
        assert_eq!(segment_bereinigen("(Musik) Hallo zusammen"), "Hallo zusammen");
        assert_eq!(segment_bereinigen("und dann und dann und dann und dann gehen wir"), "und dann gehen wir");
        assert_eq!(segment_bereinigen("ja ja ja ja"), "ja");
        assert_eq!(anfuegen("Ich wollte morgen", "noch zur Uni.", false), "Ich wollte morgen noch zur Uni.");
        assert_eq!(anfuegen("Ich fahre morgen zur", "morgen zur Universität.", true), "Ich fahre morgen zur Universität.");
        assert_eq!(anfuegen("Das ist", "ist gut", false), "Das ist ist gut", "Pause: kein Ueberlappungsabzug");
        assert_eq!(anfuegen("我想", "明天去大学", false), "我想明天去大学");
        assert_eq!(abschliessen("  ich wollte   morgen , noch los . "), "Ich wollte morgen, noch los.");
        assert_eq!(sprachcode("german"), "de");
    }

    /// The client against a stand-in that speaks the whisper.cpp server
    /// protocol (examples/server: /health, /inference multipart, verbose_json).
    #[test]
    fn client_spricht_whisper_server_protokoll() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let mut felder = Vec::new();
            for _ in 0..2 {
                let (mut c, _) = l.accept().unwrap();
                let mut buf = Vec::new(); let mut tmp = [0u8; 65536];
                loop {
                    let n = c.read(&mut tmp).unwrap(); buf.extend_from_slice(&tmp[..n]);
                    let s = String::from_utf8_lossy(&buf).to_string();
                    if let Some(k) = s.find("\r\n\r\n") {
                        let len = s[..k].lines().find_map(|z| z.strip_prefix("Content-Length: ").and_then(|v| v.trim().parse::<usize>().ok())).unwrap_or(0);
                        if buf.len() >= k + 4 + len { break; }
                    }
                    if n == 0 { break; }
                }
                let s = String::from_utf8_lossy(&buf).to_string();
                if s.starts_with("GET /health") {
                    c.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 15\r\n\r\n{\"status\":\"ok\"}").unwrap();
                } else {
                    for f in ["response_format", "language", "prompt", "file"] { if s.contains(&format!("name=\"{f}\"")) { felder.push(f.to_string()); } }
                    assert!(s.contains("RIFF") && s.contains("WAVEfmt "));
                    let body = r#"{"task":"transcribe","language":"german","duration":1.0,"text":" Hallo Welt.","segments":[{"id":0,"text":" Hallo Welt."}]}"#;
                    c.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes()).unwrap();
                }
            }
            felder
        });
        assert_eq!(gesund(port), Some(true));
        let a = transkribieren(port, &ton(500, 0.2), "auto", "Vorher gesagt.", Duration::from_secs(5)).unwrap();
        assert_eq!(a.text.trim(), "Hallo Welt."); assert_eq!(a.sprache, "de");
        assert_eq!(h.join().unwrap(), ["response_format", "language", "prompt", "file"]);
        assert_eq!(gesund(1), None, "kein Server -> None");
    }

    #[test]
    fn verlauf_haelt_genau_30_mit_audio() {
        let dir = std::env::temp_dir().join(format!("noki-talk-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let v = Verlauf::oeffnen(&dir).unwrap();
        let mut pfade = Vec::new();
        for i in 0..31 {
            let p = v.audio_dir().join(format!("{i}.m4a"));
            std::fs::write(&p, b"audio").unwrap();
            pfade.push(p.clone());
            let (_, weg) = v.speichern(1_000 + i, 1500, &format!("Diktat {i}"), &p, "de", "com.apple.Notes").unwrap();
            if i < 30 { assert!(weg.is_empty()); } else { assert_eq!(weg, [pfade[0].to_string_lossy().to_string()]); }
        }
        let l = v.liste();
        assert_eq!(l.len(), 30);
        assert_eq!(l[0].transcript, "Diktat 30"); assert_eq!(l.last().unwrap().transcript, "Diktat 1");
        assert!(!pfade[0].exists(), "Audio von Eintrag 1 ist mit entfernt");
        assert!(pfade[1].exists());
        // delete: entry + audio
        let id = l[3].id; let p = PathBuf::from(&l[3].audio_path);
        assert!(v.loeschen(id) && !p.exists() && v.holen(id).is_none());
        assert_eq!(v.liste().len(), 29);
        // empty dictation is never stored
        assert!(v.speichern(1, 1, "   ", &pfade[2], "de", "").is_err());
        // orphan audio removed on next open, referenced files stay
        let waise = v.audio_dir().join("waise.m4a"); std::fs::write(&waise, b"x").unwrap();
        drop(v);
        let v = Verlauf::oeffnen(&dir).unwrap();
        assert!(!waise.exists() && pfade[2].exists() && v.liste().len() == 29);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
