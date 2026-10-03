//! Writing a file Noki produced: PDF first, then the plain formats.
//!
//! THE CAPABILITY QUESTION COMES FIRST. Producing a document is an ACT, not a
//! read, and it is the one ACT where the target is a path the user never typed.
//! So the scope on the lease is the RESOLVED, FINAL path - not a directory, not
//! a pattern. One lease, one file.
//!
//! OVERWRITING IS A DIFFERENT DECISION FROM WRITING. An existing file is never
//! replaced unless the request said so. The default is to pick a free name
//! beside it (`Bericht-2.pdf`), because losing a file the user already had is
//! not recoverable by "undo" and a report is rarely worth it.
//!
//! WHERE FILES GO. A configured output directory, defaulting to the user's
//! Downloads folder - somewhere the user can find without being told, and
//! nowhere near Noki's own data. The target is confined to it: a filename the
//! model produced cannot escape via `../`, which matters because the filename
//! can be influenced by document content.

use crate::capability::{self, Mode};
use crate::permissions::RiskLevel;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    Pdf,
    Txt,
    Markdown,
    Csv,
    Json,
    /// Not implemented. Listed so the router can say so instead of writing a
    /// file with the wrong contents under a `.docx` name.
    Docx,
}

impl OutputFormat {
    pub fn extension(self) -> &'static str {
        match self {
            OutputFormat::Pdf => "pdf",
            OutputFormat::Txt => "txt",
            OutputFormat::Markdown => "md",
            OutputFormat::Csv => "csv",
            OutputFormat::Json => "json",
            OutputFormat::Docx => "docx",
        }
    }

    pub fn available(self) -> bool {
        self != OutputFormat::Docx
    }

    /// What the user asked for, from the request text.
    pub fn from_request(q: &str) -> Option<Self> {
        let l = q.to_lowercase();
        // Checked most specific first: "als pdf" beats a stray "datei".
        for (needles, f) in [
            (&["pdf"][..], OutputFormat::Pdf),
            (
                &["docx", "word-datei", "word datei", "worddokument"][..],
                OutputFormat::Docx,
            ),
            (&["markdown", ".md", "als md"][..], OutputFormat::Markdown),
            (&["csv", "tabelle als datei"][..], OutputFormat::Csv),
            (&["json"][..], OutputFormat::Json),
            (&["textdatei", ".txt", "als txt"][..], OutputFormat::Txt),
        ] {
            if needles.iter().any(|n| l.contains(n)) {
                return Some(f);
            }
        }
        None
    }
}

/// True when the request is actually asking for a file to be produced.
///
/// A format word alone is not enough: "fasse diese PDF zusammen" names a PDF
/// but asks for a summary. A creation verb has to be present too.
pub fn wants_file(q: &str) -> Option<OutputFormat> {
    let l = q.to_lowercase();
    const CREATE: &[&str] = &[
        "erstelle",
        "erstell",
        "speicher",
        "exportier",
        "generier",
        "schreib mir",
        "schreibe mir",
        "leg mir",
        "lege mir",
        "als datei",
        "gib mir eine",
        "wandle",
        "konvertier",
        // Bare "mach/mache" needs no object here: a format word is required
        // separately, so "mache aus diesen Daten einen Bericht als PDF" counts
        // while "mach mal weiter" never reaches this point.
        "mache ",
        "mach ",
    ];
    let creating = CREATE.iter().any(|v| l.contains(v));
    if !creating {
        return None;
    }
    let format = OutputFormat::from_request(q)?;
    // "erstelle eine Zusammenfassung dieser PDF" is still a summary request:
    // the format word has to be the OBJECT of the creation, which in practice
    // means it is not preceded by a reading verb aimed at it.
    const READING: &[&str] = &[
        "fasse",
        "fass ",
        "zusammenfassung dieser",
        "zusammenfassung der",
        "analysiere die pdf",
        "lies",
    ];
    if READING.iter().any(|r| l.contains(r))
        && !l.contains("als pdf")
        && !l.contains("als datei")
        && !l.contains("zu einer pdf")
        && !l.contains("in eine pdf")
        && !l.contains("als markdown")
        && !l.contains("als txt")
        && !l.contains("als csv")
        && !l.contains("als json")
    {
        return None;
    }
    Some(format)
}

/// Where Noki writes. Overridable for tests and for a user who wants elsewhere.
pub fn output_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("NOKI_OUTPUT_DIR") {
        return PathBuf::from(p);
    }
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join("Downloads"))
        .unwrap_or_else(std::env::temp_dir)
}

/// Makes a filename safe. Everything outside a conservative set is dropped,
/// which removes path separators, `..`, and control characters in one step -
/// so a name suggested by document content cannot become a path.
pub fn safe_stem(raw: &str) -> String {
    // Transliterated rather than stripped: a filename stays readable, and it
    // stays plain ASCII so it survives being copied to any other system.
    let cleaned: String = raw
        .replace('ß', "ss")
        .replace('ä', "ae")
        .replace('ö', "oe")
        .replace('ü', "ue")
        .replace('Ä', "Ae")
        .replace('Ö', "Oe")
        .replace('Ü', "Ue")
        .chars()
        .map(|c| match c {
            c if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' => c,
            _ => '-',
        })
        .collect();
    let collapsed: String = cleaned.split_whitespace().collect::<Vec<_>>().join("-");
    let trimmed: String = collapsed.trim_matches('-').chars().take(64).collect();
    if trimmed.is_empty() {
        "Noki-Dokument".into()
    } else {
        trimmed
    }
}

/// Resolves the final target inside `output_dir`, never outside it.
///
/// Returns the path plus whether an existing file would be replaced.
pub fn resolve_target(
    stem: &str,
    format: OutputFormat,
    allow_overwrite: bool,
) -> Result<(PathBuf, bool), String> {
    let dir = output_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Zielordner nicht verfügbar: {e}"))?;
    let canonical_dir = dunce::canonicalize(&dir).map_err(|e| e.to_string())?;
    let base = safe_stem(stem);
    let ext = format.extension();

    let first = canonical_dir.join(format!("{base}.{ext}"));
    // The confinement check, done on the built path rather than trusted from
    // `safe_stem` alone - two independent guards for the same property.
    if !first.starts_with(&canonical_dir) {
        return Err("Zieldatei läge außerhalb des Ausgabeordners.".into());
    }
    if !first.exists() {
        return Ok((first, false));
    }
    if allow_overwrite {
        return Ok((first, true));
    }
    for n in 2..=99 {
        let candidate = canonical_dir.join(format!("{base}-{n}.{ext}"));
        if !candidate.exists() {
            return Ok((candidate, false));
        }
    }
    Err("Im Ausgabeordner liegen zu viele Dateien mit diesem Namen.".into())
}

/// Whether the request explicitly authorises replacing an existing file.
pub fn overwrite_requested(q: &str) -> bool {
    let l = q.to_lowercase();
    [
        "überschreib",
        "ueberschreib",
        "ersetze die datei",
        "ersetzen",
        "gleiche datei",
    ]
    .iter()
    .any(|n| l.contains(n))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WrittenFile {
    pub path: String,
    pub name: String,
    pub format: OutputFormat,
    pub bytes: usize,
    pub replaced: bool,
}

/// Serialises content into the requested format's bytes.
fn encode(format: OutputFormat, title: &str, body: &str) -> Result<Vec<u8>, String> {
    match format {
        OutputFormat::Pdf => {
            let mut blocks = crate::pdf::blocks_from_markdown(body);
            // A title is prepended only when the content does not already open
            // with one, so a model that wrote "# Bericht" is not titled twice.
            if !matches!(blocks.first(), Some(crate::pdf::Block::Title(_))) && !title.is_empty() {
                blocks.insert(0, crate::pdf::Block::Title(title.to_owned()));
            }
            Ok(crate::pdf::render(&blocks))
        }
        OutputFormat::Markdown => {
            let mut s = String::new();
            if !title.is_empty() && !body.trim_start().starts_with("# ") {
                s.push_str(&format!("# {title}\n\n"));
            }
            s.push_str(body);
            if !s.ends_with('\n') {
                s.push('\n');
            }
            Ok(s.into_bytes())
        }
        OutputFormat::Txt => {
            // Markdown markers are noise in a plain text file.
            let mut s = if title.is_empty() { String::new() } else { format!("{title}\n\n") };
            for line in body.lines() {
                let t = line.trim_start_matches('#').trim_start();
                let t = t.strip_prefix("- ").map(|r| format!("• {r}")).unwrap_or_else(|| t.to_owned());
                s.push_str(t.replace("**", "").trim_end());
                s.push('\n');
            }
            Ok(s.into_bytes())
        }
        OutputFormat::Csv => {
            // Only a real table can become a CSV; anything else would be a
            // text file with a misleading extension.
            let rows: Vec<Vec<String>> = body
                .lines()
                .map(str::trim)
                .filter(|l| l.starts_with('|') && l.matches('|').count() >= 2)
                .map(|l| {
                    l.trim_matches('|')
                        .split('|')
                        .map(|c| c.trim().to_owned())
                        .collect::<Vec<_>>()
                })
                .filter(|r: &Vec<String>| {
                    !r.iter().all(|c| c.chars().all(|x| x == '-' || x == ':' || x.is_whitespace()))
                })
                .collect();
            if rows.is_empty() {
                return Err("Für eine CSV-Datei ist keine Tabelle im Ergebnis enthalten.".into());
            }
            let mut s = String::new();
            for row in rows {
                let cells: Vec<String> = row
                    .iter()
                    .map(|c| {
                        if c.contains(',') || c.contains('"') || c.contains('\n') {
                            format!("\"{}\"", c.replace('"', "\"\""))
                        } else {
                            c.clone()
                        }
                    })
                    .collect();
                s.push_str(&cells.join(","));
                s.push('\n');
            }
            Ok(s.into_bytes())
        }
        OutputFormat::Json => {
            let v = serde_json::json!({"title": title, "content": body});
            serde_json::to_vec_pretty(&v).map_err(|e| e.to_string())
        }
        OutputFormat::Docx => Err(
            "Eine DOCX-Datei kann Noki lokal noch nicht erzeugen. Möglich sind PDF, Markdown, TXT, CSV und JSON.".into(),
        ),
    }
}

/// Writes one file under one single-use ACT lease.
///
/// The lease is issued by the CALLER from user intent and handed in, so this
/// function cannot manufacture its own permission. It redeems it against the
/// exact resolved path; a lease for another path or another capability fails
/// here, which is the point of passing it rather than the path alone.
#[allow(clippy::too_many_arguments)]
pub fn write_file(
    leases: &capability::LeaseStore,
    lease_id: u64,
    task_id: u64,
    intent: &str,
    target: &Path,
    format: OutputFormat,
    title: &str,
    body: &str,
    replaced: bool,
) -> Result<WrittenFile, String> {
    let started = std::time::Instant::now();
    let scope = target.to_string_lossy().into_owned();

    let mut finish = |result: &'static str, error: Option<String>| {
        capability::log_action(capability::ActionAudit {
            timestamp: capability::now_ms(),
            task_id,
            intent: capability::compact_intent(intent),
            phase: Mode::Act.as_str(),
            capability: "file.create".into(),
            scope: scope.clone(),
            lease_id,
            risk: if replaced {
                RiskLevel::R2
            } else {
                RiskLevel::R1
            },
            confirmed: true,
            tool: "document.output".into(),
            result,
            duration_ms: started.elapsed().as_millis() as u64,
            error,
        });
    };

    // The gate. Capability AND scope must match, and an ACT lease is spent.
    if let Err(e) = leases.redeem(lease_id, "file.create", &scope) {
        finish("denied", Some(e.clone()));
        return Err(e);
    }
    let bytes = match encode(format, title, body) {
        Ok(b) => b,
        Err(e) => {
            finish("unsupported", Some(e.clone()));
            return Err(e);
        }
    };
    // Written via a temporary file in the same directory, then renamed: a
    // crash halfway through leaves either the old file or the new one, never a
    // truncated report.
    let tmp = target.with_extension(format!("{}.part", format.extension()));
    if let Err(e) = std::fs::write(&tmp, &bytes) {
        let msg = format!("Datei konnte nicht geschrieben werden: {e}");
        finish("error", Some(msg.clone()));
        return Err(msg);
    }
    if let Err(e) = std::fs::rename(&tmp, target) {
        let _ = std::fs::remove_file(&tmp);
        let msg = format!("Datei konnte nicht abgelegt werden: {e}");
        finish("error", Some(msg.clone()));
        return Err(msg);
    }
    finish("ok", None);
    Ok(WrittenFile {
        path: scope,
        name: target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        format,
        bytes: bytes.len(),
        replaced,
    })
}

/// A short title for a produced document, from the request.
pub fn title_from_request(q: &str) -> String {
    let l = q.to_lowercase();
    for (needle, title) in [
        ("bericht", "Bericht"),
        ("zusammenfassung", "Zusammenfassung"),
        ("analyse", "Analyse"),
        ("übersicht", "Übersicht"),
        ("uebersicht", "Übersicht"),
        ("vergleich", "Vergleich"),
        ("protokoll", "Protokoll"),
        ("auswertung", "Auswertung"),
    ] {
        if l.contains(needle) {
            return title.to_owned();
        }
    }
    "Noki-Dokument".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `NOKI_OUTPUT_DIR` is process-global, so these tests cannot run beside
    /// each other: one would redirect another's output mid-test. The lock makes
    /// that explicit instead of leaving it to the scheduler.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn sandbox() -> (PathBuf, std::sync::MutexGuard<'static, ()>) {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let d = std::env::temp_dir().join(format!(
            "noki-out-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::env::set_var("NOKI_OUTPUT_DIR", &d);
        (dunce::canonicalize(&d).unwrap(), guard)
    }

    #[test]
    fn only_a_creation_request_produces_a_file() {
        // Asks for a file.
        assert_eq!(
            wants_file("Erstelle mir daraus eine PDF."),
            Some(OutputFormat::Pdf)
        );
        assert_eq!(
            wants_file("Mache aus diesen Daten einen Bericht als PDF"),
            Some(OutputFormat::Pdf)
        );
        assert_eq!(
            wants_file("Speichere das als Markdown"),
            Some(OutputFormat::Markdown)
        );
        assert_eq!(
            wants_file("Exportiere die Tabelle als CSV"),
            Some(OutputFormat::Csv)
        );
        // Asks about a file. These must NOT write anything.
        assert_eq!(wants_file("Fasse diese PDF zusammen."), None);
        assert_eq!(wants_file("Was steht in der PDF?"), None);
        assert_eq!(wants_file("Analysiere die Daten"), None);
        assert_eq!(wants_file("Erstelle eine Zusammenfassung dieser PDF"), None);
        // But an explicit output format overrides the reading verb.
        assert_eq!(
            wants_file("Fasse das zusammen und speichere es als PDF"),
            Some(OutputFormat::Pdf)
        );
    }

    #[test]
    fn a_filename_from_content_cannot_escape_the_output_folder() {
        let (dir, _env) = sandbox();
        // The nastiest realistic case: the stem comes from text Noki read.
        for hostile in [
            "../../../../etc/passwd",
            "/etc/passwd",
            "..\\..\\windows",
            "report/../../../secret",
            "....//....//x",
        ] {
            let (p, _) = resolve_target(hostile, OutputFormat::Pdf, false).unwrap();
            assert!(p.starts_with(&dir), "{hostile} escaped to {p:?}");
            assert!(!p.to_string_lossy().contains(".."));
        }
        // Control characters and separators are gone.
        assert!(!safe_stem("a/b\0c\nd").contains('/'));
        assert_eq!(safe_stem(""), "Noki-Dokument");
        assert_eq!(safe_stem("   "), "Noki-Dokument");
        assert_eq!(safe_stem("Quartalsbericht Q1"), "Quartalsbericht-Q1");
        assert_eq!(safe_stem("Größe"), "Groesse");
    }

    #[test]
    fn an_existing_file_is_kept_and_a_free_name_is_chosen() {
        let (dir, _env) = sandbox();
        std::fs::write(dir.join("Bericht.pdf"), b"original").unwrap();
        let (p, replaced) = resolve_target("Bericht", OutputFormat::Pdf, false).unwrap();
        assert!(!replaced);
        assert_eq!(p.file_name().unwrap(), "Bericht-2.pdf");
        // The original is untouched.
        assert_eq!(std::fs::read(dir.join("Bericht.pdf")).unwrap(), b"original");
        // Only an explicit request replaces it.
        let (p2, replaced2) = resolve_target("Bericht", OutputFormat::Pdf, true).unwrap();
        assert!(replaced2);
        assert_eq!(p2.file_name().unwrap(), "Bericht.pdf");
        assert!(overwrite_requested("überschreib die Datei"));
        assert!(!overwrite_requested("Erstelle eine PDF"));
    }

    #[test]
    fn writing_needs_a_matching_lease_and_spends_it() {
        let (_dir, _env) = sandbox();
        let leases = capability::LeaseStore::default();
        let (target, _) = resolve_target("Bericht", OutputFormat::Pdf, false).unwrap();
        let scope = target.to_string_lossy().into_owned();

        // A lease for a DIFFERENT file does not authorise this one.
        let wrong = leases.issue("file.create", "/tmp/other.pdf", 1, RiskLevel::R1);
        assert!(write_file(
            &leases,
            wrong.id,
            1,
            "i",
            &target,
            OutputFormat::Pdf,
            "T",
            "Text",
            false
        )
        .is_err());
        assert!(!target.exists(), "nothing may be written without a lease");

        // The right lease works exactly once.
        let ok = leases.issue("file.create", &scope, 1, RiskLevel::R1);
        let written = write_file(
            &leases,
            ok.id,
            1,
            "i",
            &target,
            OutputFormat::Pdf,
            "Bericht",
            "Inhalt",
            false,
        )
        .unwrap();
        assert!(written.bytes > 500);
        assert!(target.exists());
        // Replay is refused.
        assert!(write_file(
            &leases,
            ok.id,
            1,
            "i",
            &target,
            OutputFormat::Pdf,
            "T",
            "Text",
            false
        )
        .is_err());
    }

    #[test]
    fn a_created_pdf_is_a_real_pdf() {
        let (_dir, _env) = sandbox();
        let leases = capability::LeaseStore::default();
        let (target, _) = resolve_target("Analyse", OutputFormat::Pdf, false).unwrap();
        let lease = leases.issue("file.create", &target.to_string_lossy(), 2, RiskLevel::R1);
        write_file(
            &leases,
            lease.id,
            2,
            "erstelle pdf",
            &target,
            OutputFormat::Pdf,
            "Analyse",
            "## Ergebnis\n\nDer Umsatz stieg.\n\n- Größe geprüft\n",
            false,
        )
        .unwrap();
        let bytes = std::fs::read(&target).unwrap();
        assert!(bytes.starts_with(b"%PDF-1.4"), "not a PDF");
        assert!(bytes.ends_with(b"%%EOF\n"));
        assert!(bytes.len() > 700, "suspiciously small: {}", bytes.len());
        // Crucially NOT markdown with a .pdf name.
        assert!(!bytes.starts_with(b"##"));
        assert!(!String::from_utf8_lossy(&bytes).contains("## Ergebnis"));
        // No leftover temporary file.
        assert!(!target.with_extension("pdf.part").exists());
    }

    #[test]
    fn the_plain_formats_each_produce_their_own_shape() {
        let md = encode(OutputFormat::Markdown, "Titel", "## Teil\n\nText\n").unwrap();
        assert!(String::from_utf8_lossy(&md).starts_with("# Titel"));

        let txt = encode(OutputFormat::Txt, "Titel", "## Teil\n- Punkt\n").unwrap();
        let t = String::from_utf8_lossy(&txt);
        assert!(!t.contains('#'), "markdown markers leaked into txt: {t}");
        assert!(t.contains("• Punkt"));

        let json = encode(OutputFormat::Json, "Titel", "Inhalt").unwrap();
        let v: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(v["title"], "Titel");

        let csv = encode(OutputFormat::Csv, "T", "| a | b |\n|---|---|\n| 1 | 2 |\n").unwrap();
        assert_eq!(String::from_utf8_lossy(&csv), "a,b\n1,2\n");
        // A CSV request with no table is refused rather than mislabelled.
        assert!(encode(OutputFormat::Csv, "T", "nur Prosa").is_err());
    }

    #[test]
    fn docx_is_reported_as_unavailable_not_faked() {
        assert!(!OutputFormat::Docx.available());
        let e = encode(OutputFormat::Docx, "T", "Text").unwrap_err();
        assert!(e.contains("noch nicht"), "{e}");
        // And the request is recognised, so the router can say so.
        assert_eq!(
            wants_file("Erstelle daraus eine Word-Datei"),
            Some(OutputFormat::Docx)
        );
    }

    #[test]
    fn a_csv_field_with_a_comma_is_quoted() {
        let csv = encode(
            OutputFormat::Csv,
            "T",
            "| name | x |\n|---|---|\n| Meier, Anna | 1 |\n",
        )
        .unwrap();
        assert_eq!(String::from_utf8_lossy(&csv), "name,x\n\"Meier, Anna\",1\n");
    }

    #[test]
    fn titles_come_from_the_request() {
        assert_eq!(
            title_from_request("Erstelle einen Bericht als PDF"),
            "Bericht"
        );
        assert_eq!(
            title_from_request("Mach mir eine Zusammenfassung als PDF"),
            "Zusammenfassung"
        );
        assert_eq!(title_from_request("Speicher das als PDF"), "Noki-Dokument");
    }
}
