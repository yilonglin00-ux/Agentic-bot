//! Rollen der lokalen Modelle (Noki Chat / Noki Code).
//!
//! Noki Chat waehlt intern nach Aufgabe (General / Reasoning / Tools) - ohne
//! sichtbaren Modus. Noki Code waehlt nach der bestehenden Auswahl
//! Funktional / Kreativ. Welches lokale Modell eine Rolle besetzt, steht in
//! `~/NOKI/.local/llama-models/noki-rollen.json`: geschrieben vom lokalen
//! Benchmark (`desktop/models/noki-bench/bench.py uebernehmen`) oder von Hand
//! in Einstellungen › Intelligence. Ohne Datei (oder ohne Eintrag) gilt
//! unveraendert der bisherige Stand (qwen3.5-9b / JackOD).
//!
//! Die Modelle selbst laufen weiter ueber den EINEN llama.cpp-Router
//! (`models.ini`, `--models-max 1`): eine Rolle nennt nur dessen Preset-ID.
//! Ein Rollenwechsel ist damit ein gewoehnlicher Modellwechsel des
//! ModelManagers (entladen -> pruefen -> laden), nie ein zweites Modell im RAM.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::reasoning::ReasoningTier;

/// Interne Aufgabenart von Noki Chat (nie als Modus sichtbar).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRolle {
    #[default]
    General,
    Reasoning,
    Tools,
}

impl ChatRolle {
    pub fn as_str(self) -> &'static str {
        match self {
            ChatRolle::General => "general",
            ChatRolle::Reasoning => "reasoning",
            ChatRolle::Tools => "tools",
        }
    }
}

/// Die Entscheidung von Noki Chat: aus der bereits vorhandenen Einstufung
/// (reasoning::classify) und dem Werkzeugplan - keine zweite Analyse.
/// * mehrere Werkzeugschritte / ausdrueckliche Aktion -> Tools
/// * DEEP (Analyse, Planung, Mathematik, mehrere Quellen) -> Reasoning
/// * sonst -> General (normales Gespraech)
pub fn chat_rolle(tier: ReasoningTier, werkzeug_schritte: usize, ausdrueckliche_aktion: bool) -> ChatRolle {
    if ausdrueckliche_aktion || werkzeug_schritte >= 2 {
        ChatRolle::Tools
    } else if tier == ReasoningTier::Deep {
        ChatRolle::Reasoning
    } else {
        ChatRolle::General
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatRollen {
    #[serde(default)]
    pub general: Option<String>,
    #[serde(default)]
    pub reasoning: Option<String>,
    #[serde(default)]
    pub tools: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CodeRollen {
    #[serde(default)]
    pub funktional: Option<String>,
    #[serde(default)]
    pub kreativ: Option<String>,
}

/// Anzeige-Angaben eines Presets (fuer Chat-Kopf und Einstellungen).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ModellInfo {
    #[serde(default)]
    pub anzeige: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub quant: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RollenDatei {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub chat: ChatRollen,
    #[serde(default)]
    pub code: CodeRollen,
    #[serde(default)]
    pub modelle: BTreeMap<String, ModellInfo>,
    /// "benchmark" | "manuell"
    #[serde(default)]
    pub quelle: String,
    #[serde(default)]
    pub stand: String,
}

impl RollenDatei {
    pub fn chat(&self, r: ChatRolle) -> Option<&str> {
        match r {
            ChatRolle::General => self.chat.general.as_deref(),
            ChatRolle::Reasoning => self.chat.reasoning.as_deref(),
            ChatRolle::Tools => self.chat.tools.as_deref(),
        }
        .filter(|s| !s.trim().is_empty())
    }
    pub fn code(&self, kreativ: bool) -> Option<&str> {
        if kreativ { self.code.kreativ.as_deref() } else { self.code.funktional.as_deref() }.filter(|s| !s.trim().is_empty())
    }
    /// Setzt eine Rolle ("chat.general" … "code.kreativ"); None = Standard.
    pub fn setzen(&mut self, rolle: &str, id: Option<String>) -> Result<(), String> {
        let id = id.filter(|s| !s.trim().is_empty());
        match rolle {
            "chat.general" => self.chat.general = id,
            "chat.reasoning" => self.chat.reasoning = id,
            "chat.tools" => self.chat.tools = id,
            "code.funktional" => self.code.funktional = id,
            "code.kreativ" => self.code.kreativ = id,
            _ => return Err(format!("Unbekannte Rolle: {rolle}")),
        }
        Ok(())
    }
}

fn home() -> PathBuf { PathBuf::from(std::env::var("HOME").unwrap_or_default()) }
pub fn modell_ordner() -> PathBuf {
    std::env::var("NOKI_LLAMA_MODELS").map(PathBuf::from).unwrap_or_else(|_| home().join("NOKI/.local/llama-models"))
}
pub fn datei() -> PathBuf { modell_ordner().join("noki-rollen.json") }

/// Gelesen wird nur, wenn sich die Datei geaendert hat (mtime) - der
/// Modellwahl-Pfad bleibt ohne Plattenzugriff in der Regel.
static CACHE: Mutex<Option<(Option<std::time::SystemTime>, RollenDatei)>> = Mutex::new(None);

pub fn laden() -> RollenDatei {
    let p = datei();
    let mtime = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
    if let Ok(g) = CACHE.lock() {
        if let Some((t, r)) = g.as_ref() {
            if *t == mtime { return r.clone(); }
        }
    }
    let r = lesen(&p);
    if let Ok(mut g) = CACHE.lock() { *g = Some((mtime, r.clone())); }
    r
}
pub fn lesen(p: &Path) -> RollenDatei {
    std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}
pub fn speichern(r: &RollenDatei) -> Result<(), String> {
    let p = datei();
    if let Some(d) = p.parent() { std::fs::create_dir_all(d).map_err(|e| e.to_string())?; }
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(r).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &p).map_err(|e| e.to_string())?;
    if let Ok(mut g) = CACHE.lock() { *g = None; }
    Ok(())
}

// ---- installierte Presets (models.ini des llama.cpp-Routers) ------------------

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Preset {
    pub id: String,
    pub datei: Option<String>,
    pub bytes: u64,
    pub quant: String,
}

/// "Q4_K_M", "Q5_K_S", "Q8_0", "IQ4_XS", "F16" … aus einem Dateinamen.
pub fn quant_aus_name(name: &str) -> String {
    let n = name.to_ascii_uppercase();
    for teil in n.split(['-', '.', '_']).collect::<Vec<_>>().windows(3) {
        let k = format!("{}_{}_{}", teil[0], teil[1], teil[2]);
        if (k.starts_with('Q') || k.starts_with("IQ")) && k.contains("_K_") { return k; }
    }
    for teil in n.split(['-', '.']) {
        let t = teil.trim();
        if (t.starts_with('Q') || t.starts_with("IQ")) && t.chars().nth(1).is_some_and(|c| c.is_ascii_digit() || c == 'Q') { return t.to_string(); }
        if t == "F16" || t == "BF16" || t == "F32" { return t.to_string(); }
    }
    String::new()
}

/// `[id]`-Abschnitte und ihr `model = …` aus models.ini (Pfade relativ zum
/// Modellordner). Kommentare (# ;) werden ignoriert.
pub fn presets_aus_ini(ini: &str, ordner: &Path) -> Vec<Preset> {
    let mut out: Vec<Preset> = Vec::new();
    for zeile in ini.lines() {
        let z = zeile.trim();
        if z.is_empty() || z.starts_with('#') || z.starts_with(';') { continue; }
        if let Some(id) = z.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            let id = id.trim();
            if id != "*" && !id.is_empty() { out.push(Preset { id: id.to_string(), datei: None, bytes: 0, quant: String::new() }); }
            continue;
        }
        let Some((k, v)) = z.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim().trim_matches('"'));
        if !(k == "model" || k == "m") { continue; }
        if let Some(p) = out.last_mut() {
            let pfad = if Path::new(v).is_absolute() { PathBuf::from(v) } else { ordner.join(v) };
            p.bytes = std::fs::metadata(&pfad).map(|m| m.len()).unwrap_or(0);
            p.quant = quant_aus_name(&pfad.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default());
            p.datei = Some(pfad.to_string_lossy().to_string());
        }
    }
    out
}
pub fn presets() -> Vec<Preset> {
    let o = modell_ordner();
    std::fs::read_to_string(o.join("models.ini")).map(|t| presets_aus_ini(&t, &o)).unwrap_or_default()
}

// ---- Denk-Text, der im Inhalt landet ----------------------------------------

/// Manche Fine-Tunes schreiben ihr Denken als `<think>…</think>` in den
/// normalen Inhalt (statt in `reasoning_content`). Dieser Filter trennt das
/// beim Streamen: sichtbar bleibt nur die Antwort; gezaehlt wird, wie viel
/// gedacht wurde. Tags duerfen ueber Chunk-Grenzen gehen.
#[derive(Default, Debug)]
pub struct DenkFilter {
    im_denken: bool,
    rest: String,
    pub denk_zeichen: usize,
}

const AUF: &str = "<think>";
const ZU: &str = "</think>";

impl DenkFilter {
    pub fn push(&mut self, s: &str) -> String {
        self.rest.push_str(s);
        let mut sichtbar = String::new();
        loop {
            let tag = if self.im_denken { ZU } else { AUF };
            if let Some(i) = self.rest.find(tag) {
                let vor: String = self.rest[..i].to_string();
                if self.im_denken { self.denk_zeichen += vor.chars().count(); } else { sichtbar.push_str(&vor); }
                self.rest = self.rest[i + tag.len()..].to_string();
                self.im_denken = !self.im_denken;
                continue;
            }
            // Ein moeglicher Tag-Anfang am Ende bleibt im Puffer.
            let halte = (1..tag.len()).rev().find(|&n| self.rest.len() >= n && self.rest.is_char_boundary(self.rest.len() - n)
                && tag.starts_with(&self.rest[self.rest.len() - n..])).unwrap_or(0);
            let bis = self.rest.len() - halte;
            let teil: String = self.rest[..bis].to_string();
            if self.im_denken { self.denk_zeichen += teil.chars().count(); } else { sichtbar.push_str(&teil); }
            self.rest = self.rest[bis..].to_string();
            return sichtbar;
        }
    }
    pub fn ende(&mut self) -> String {
        let r = std::mem::take(&mut self.rest);
        if self.im_denken { self.denk_zeichen += r.chars().count(); String::new() } else { r }
    }
}

/// Endgueltige Antwort: alle `<think>`-Bloecke weg; steht ein einzelnes
/// `</think>` ohne Anfang im Text (Vorlage hat das Denken implizit begonnen),
/// gilt alles davor als Denken. Gibt (Antwort, Denk-Zeichen) zurueck.
pub fn antwort_bereinigen(text: &str) -> (String, usize) {
    if !text.contains(AUF) && !text.contains(ZU) { return (text.to_string(), 0); }
    let mut t = text.to_string();
    let mut denk = 0;
    if let Some(i) = t.find(ZU) {
        if t.find(AUF).is_none_or(|a| a > i) {
            denk += t[..i].chars().count();
            t = t[i + ZU.len()..].to_string();
        }
    }
    let mut f = DenkFilter::default();
    let mut aus = f.push(&t);
    aus.push_str(&f.ende());
    (aus.trim().to_string(), denk + f.denk_zeichen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_rolle_aus_vorhandener_einstufung() {
        assert_eq!(chat_rolle(ReasoningTier::Fast, 0, false), ChatRolle::General);
        assert_eq!(chat_rolle(ReasoningTier::Normal, 1, false), ChatRolle::General);
        assert_eq!(chat_rolle(ReasoningTier::Deep, 1, false), ChatRolle::Reasoning);
        assert_eq!(chat_rolle(ReasoningTier::Normal, 2, false), ChatRolle::Tools);
        assert_eq!(chat_rolle(ReasoningTier::Fast, 0, true), ChatRolle::Tools);
        assert_eq!(chat_rolle(ReasoningTier::Deep, 3, false), ChatRolle::Tools);
    }

    #[test]
    fn rollen_datei_und_setzen() {
        let mut r: RollenDatei = serde_json::from_str(r#"{"version":1,"chat":{"general":"qwen3.5-9b","reasoning":"r7-9b"},"code":{"funktional":"swe-9b","kreativ":""}}"#).unwrap();
        assert_eq!(r.chat(ChatRolle::General), Some("qwen3.5-9b"));
        assert_eq!(r.chat(ChatRolle::Reasoning), Some("r7-9b"));
        assert_eq!(r.chat(ChatRolle::Tools), None);
        assert_eq!(r.code(false), Some("swe-9b"));
        assert_eq!(r.code(true), None, "leerer Eintrag = Standard");
        r.setzen("code.kreativ", Some("claude-code-9b".into())).unwrap();
        assert_eq!(r.code(true), Some("claude-code-9b"));
        r.setzen("chat.reasoning", None).unwrap();
        assert_eq!(r.chat(ChatRolle::Reasoning), None);
        assert!(r.setzen("chat.kreativ", None).is_err(), "Kreativ ist keine Chat-Rolle");
        assert_eq!(lesen(Path::new("/gibt/es/nicht.json")), RollenDatei::default());
    }

    #[test]
    fn presets_und_quant() {
        let d = std::env::temp_dir().join(format!("noki-rollen-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("Qwen3.5-9B-Q4_K_M.gguf"), vec![0u8; 1234]).unwrap();
        let ini = "# Noki\n[*]\nctx-size = 8192\n\n[qwen3.5-9b]\nmodel = Qwen3.5-9B-Q4_K_M.gguf\n\n[jackod-9b]\nmodel = \"/abs/JackOD-9B-Coder.Q4_K_M.gguf\"\n";
        let p = presets_aus_ini(ini, &d);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].id, "qwen3.5-9b");
        assert_eq!(p[0].bytes, 1234);
        assert_eq!(p[0].quant, "Q4_K_M");
        assert_eq!(p[1].datei.as_deref(), Some("/abs/JackOD-9B-Coder.Q4_K_M.gguf"));
        assert_eq!(p[1].bytes, 0);
        assert_eq!(quant_aus_name("model.Q8_0.gguf"), "Q8_0");
        assert_eq!(quant_aus_name("x-IQ4_XS.gguf"), "IQ4_XS");
        assert_eq!(quant_aus_name("x-f16.gguf"), "F16");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn denkfilter_ueber_chunkgrenzen() {
        let mut f = DenkFilter::default();
        let mut sichtbar = String::new();
        for c in ["Hal", "lo <th", "ink>geheim", "er Plan</thi", "nk> Welt", "<", "b>!"] { sichtbar.push_str(&f.push(c)); }
        sichtbar.push_str(&f.ende());
        assert_eq!(sichtbar, "Hallo  Welt<b>!");
        assert_eq!(f.denk_zeichen, "geheimer Plan".chars().count());
        // ohne Tags: unveraendert, auch Umlaute und '<' am Ende
        let mut g = DenkFilter::default();
        let mut s = g.push("Grüße <");
        s.push_str(&g.ende());
        assert_eq!(s, "Grüße <");
    }

    #[test]
    fn antwort_ohne_denktext() {
        assert_eq!(antwort_bereinigen("Nur Antwort."), ("Nur Antwort.".into(), 0));
        assert_eq!(antwort_bereinigen("<think>a b</think>\n\nJa.").0, "Ja.");
        let (a, n) = antwort_bereinigen("ich überlege lange</think>\nDie Antwort ist 42.");
        assert_eq!(a, "Die Antwort ist 42.");
        assert_eq!(n, "ich überlege lange".chars().count());
        assert!(!antwort_bereinigen("<think>x</think>Ok<think>y</think> gut").0.contains("think"));
    }
}
