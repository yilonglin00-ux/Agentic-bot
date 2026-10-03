//! Bounded local document extraction. It reads one resolved file, never scans a folder.

use serde::{Deserialize, Serialize};
use std::{fs, path::Path, process::Command};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExtractedDocument {
    pub path: String,
    pub file_type: String,
    pub text: String,
    pub truncated: bool,
    pub needs_ocr: bool,
}

const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_EXTRACT_CHARS: usize = 80_000;

pub fn extract(path: &Path, page: Option<usize>) -> Result<ExtractedDocument, String> {
    let canonical = dunce::canonicalize(path)
        .or_else(|_| path.canonicalize())
        .map_err(|_| "Dokument wurde nicht gefunden.".to_string())?;
    if crate::code_agent::is_sensitive_path(&canonical) {
        return Err("Geschütztes Dokument darf nicht gelesen werden.".into());
    }
    let meta = fs::metadata(&canonical).map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return Err("Dokument fehlt oder ist für die lokale Extraktion zu groß.".into());
    }
    let ext = canonical
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or("")
        .to_lowercase();
    let raw = match ext.as_str() {
        "txt" | "md" | "csv" | "tsv" | "json" | "xml" | "html" | "htm" | "log" | "rs" | "js"
        | "ts" | "py" | "sh" | "bash" | "zsh" | "yaml" | "yml" | "toml" | "css" | "sql"
        | "swift" | "c" | "cpp" | "h" | "hpp" => fs::read_to_string(&canonical)
            .map_err(|_| "Dokument ist nicht als Text lesbar.".to_string())?,
        "rtf" | "doc" | "docx" | "odt" => textutil(&canonical)?,
        "pdf" => pdf_text(&canonical)?,
        "png" | "jpg" | "jpeg" | "webp" | "heic" | "heif" | "tiff" | "tif" | "bmp" | "gif"
        | "svg" | "ico" => image_summary(&canonical)?,
        _ => {
            return Err(format!(
                "Für den Dokumenttyp .{ext} ist kein lokaler Parser verfügbar."
            ))
        }
    };
    let raw = if let Some(page) = page {
        page_slice(&raw, page)
    } else {
        raw
    };
    let needs_ocr = ext == "pdf" && raw.trim().is_empty();
    let truncated = raw.chars().count() > MAX_EXTRACT_CHARS;
    let text = raw.chars().take(MAX_EXTRACT_CHARS).collect();
    Ok(ExtractedDocument {
        path: canonical.to_string_lossy().into_owned(),
        file_type: ext,
        text,
        truncated,
        needs_ocr,
    })
}

pub fn image_summary(path: &Path) -> Result<String, String> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let meta = fs::metadata(path).map_err(|e| e.to_string())?;
    let size_kb = meta.len() / 1024;
    let mut out = format!("Bilddatei: {name} ({size_kb} KB)");

    if let Ok(o) = Command::new("/usr/bin/sips")
        .args(["-g", "pixelWidth", "-g", "pixelHeight"])
        .arg(path)
        .output()
    {
        let s = String::from_utf8_lossy(&o.stdout);
        let w = s
            .lines()
            .find(|l| l.contains("pixelWidth"))
            .and_then(|l| l.split(':').nth(1))
            .map(str::trim)
            .unwrap_or("");
        let h = s
            .lines()
            .find(|l| l.contains("pixelHeight"))
            .and_then(|l| l.split(':').nth(1))
            .map(str::trim)
            .unwrap_or("");
        if !w.is_empty() && !h.is_empty() {
            out.push_str(&format!(", Abmessungen: {w}x{h} Pixel"));
        }
    }

    let p_str = path.to_string_lossy();
    let escaped = p_str.replace('\\', "\\\\").replace('"', "\\\"");
    let swift_code = format!(
        "import Vision\nimport AppKit\nlet url = URL(fileURLWithPath: \"{escaped}\")\nif let img = NSImage(contentsOf: url), let cg = img.cgImage(forProposedRect: nil, context: nil, hints: nil) {{\n    let req = VNRecognizeTextRequest {{ r, _ in\n        let obs = r.results as? [VNRecognizedTextObservation] ?? []\n        let txt = obs.compactMap {{ $0.topCandidates(1).first?.string }}.joined(separator: \"\\n\")\n        if !txt.isEmpty {{ print(\"OCR_START\\n\" + txt + \"\\nOCR_END\") }}\n    }}\n    req.recognitionLevel = .accurate\n    let h = VNImageRequestHandler(cgImage: cg, options: [:])\n    try? h.perform([req])\n}}"
    );
    if let Ok(res) = Command::new("/usr/bin/swift")
        .args(["-e", &swift_code])
        .output()
    {
        let s = String::from_utf8_lossy(&res.stdout);
        if let Some(start) = s.find("OCR_START\n") {
            if let Some(end) = s.find("\nOCR_END") {
                let ocr = s[start + 10..end].trim();
                if !ocr.is_empty() {
                    out.push_str(&format!("\nErkannter Text im Bild:\n{ocr}"));
                }
            }
        }
    }
    Ok(out)
}

fn textutil(path: &Path) -> Result<String, String> {
    let out = Command::new("/usr/bin/textutil")
        .args(["-convert", "txt", "-stdout"])
        .arg(path)
        .output()
        .map_err(|e| format!("Dokumentparser nicht verfügbar: {e}"))?;
    if !out.status.success() {
        return Err("Dokument konnte lokal nicht extrahiert werden.".into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn spotlight_text(path: &Path) -> Result<String, String> {
    let out = Command::new("/usr/bin/mdls")
        .args(["-raw", "-name", "kMDItemTextContent"])
        .arg(path)
        .output()
        .map_err(|e| format!("PDF-Metadatenparser nicht verfügbar: {e}"))?;
    if !out.status.success() {
        return Err("PDF konnte lokal nicht extrahiert werden.".into());
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    Ok(if text == "(null)" {
        String::new()
    } else {
        text.trim_matches('"').replace("\\n", "\n")
    })
}

/// Text of a PDF, with page breaks preserved.
///
/// WHY THIS EXISTS. Extraction used to rely on Spotlight alone
/// (`mdls kMDItemTextContent`). Spotlight only answers for files it has
/// INDEXED, and it returns `(null)` rather than an error for everything else -
/// so a PDF in `/tmp`, on an external disk, in an excluded folder, or simply
/// newly written came back as "no extractable text" and was reported to the
/// user as a scanned document needing OCR. Verified against a freshly written
/// PDF: Spotlight said `(null)`, PDFKit read it perfectly.
///
/// So Spotlight is now the FAST PATH and PDFKit is the truth. Spotlight costs
/// milliseconds when it works; PDFKit costs about a second and a half because
/// `swift -e` compiles the snippet first, which is acceptable for a document
/// task and is never on the FAST conversational path.
///
/// `\u{000c}` between pages is what `doc_chunks` reads to attach page numbers.
pub fn pdf_text(path: &Path) -> Result<String, String> {
    if let Ok(t) = spotlight_text(path) {
        if !t.trim().is_empty() {
            return Ok(t);
        }
    }
    let p = path.to_string_lossy();
    let escaped = p.replace('\\', "\\\\").replace('"', "\\\"");
    let code = format!(
        "import PDFKit\n\
         let u = URL(fileURLWithPath: \"{escaped}\")\n\
         guard let d = PDFDocument(url: u) else {{ exit(2) }}\n\
         var parts: [String] = []\n\
         for i in 0..<d.pageCount {{ parts.append(d.page(at: i)?.string ?? \"\") }}\n\
         print(parts.joined(separator: \"\\u{{000c}}\"))"
    );
    let out = Command::new("/usr/bin/swift")
        .args(["-e", &code])
        .output()
        .map_err(|e| format!("PDF-Parser nicht verfügbar: {e}"))?;
    if !out.status.success() {
        return Err("PDF konnte lokal nicht gelesen werden.".into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_owned())
}

fn page_slice(text: &str, page: usize) -> String {
    if page == 0 {
        return String::new();
    }
    let pages: Vec<_> = text.split('\u{000c}').collect();
    if pages.len() > 1 {
        return pages.get(page - 1).copied().unwrap_or("").to_owned();
    }
    if page == 1 {
        text.chars().take(12_000).collect()
    } else {
        String::new()
    }
}

/// The passages of `text` that answer `query`, within `max_chars`.
///
/// This delegates to `doc_chunks`, which fixed three things the previous
/// paragraph-scoring version got wrong on real documents: a word common to the
/// whole document no longer outranks the rare word that actually matters (idf),
/// selected passages are handed over in READING ORDER rather than by score,
/// and a request with no keywords to match on - "fasse das zusammen" - gets an
/// even spread over the whole document instead of whatever happened to sort
/// first, which in practice was always the opening.
pub fn relevant_chunks(text: &str, query: &str, max_chars: usize) -> String {
    let chunks = crate::doc_chunks::segment(text);
    if chunks.is_empty() {
        return String::new();
    }
    let selected = if summary_style(query) {
        crate::doc_chunks::spread(&chunks, max_chars)
    } else {
        crate::doc_chunks::select_for_question(&chunks, query, max_chars)
    };
    selected
        .iter()
        .map(|c| c.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Whether a request is about the document as a whole rather than one fact in
/// it. Those two need different retrieval, and guessing wrong is what makes a
/// summary describe only the first page.
pub fn summary_style(query: &str) -> bool {
    let q = query.to_lowercase();
    [
        "fasse",
        "fass ",
        "zusammenfass",
        "zusammenfassung",
        "überblick",
        "ueberblick",
        "übersicht",
        "uebersicht",
        "worum geht",
        "was steht",
        "inhalt",
        "bericht",
        "gliederung",
        "kernaussagen",
        "wichtigsten punkte",
        "summary",
        "beschreib",
    ]
    .iter()
    .any(|k| q.contains(k))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extracts_only_resolved_text_and_chunks_relevantly() {
        let path = std::env::temp_dir().join(format!("noki-doc-{}.txt", std::process::id()));
        fs::write(
            &path,
            "Einleitung.\n\nUmsatz stieg um zehn Prozent.\n\nAnhang.",
        )
        .unwrap();
        let doc = extract(&path, None).unwrap();
        assert_eq!(doc.file_type, "txt");
        // A short document fits whole, and is handed over in READING ORDER.
        // This is a deliberate change from score-order: passages delivered
        // out of sequence read as contradictions to the model.
        let chunk = relevant_chunks(&doc.text, "wichtigster Umsatz", 200);
        assert!(chunk.contains("Umsatz stieg um zehn Prozent"));
        assert!(
            chunk.find("Einleitung") < chunk.find("Umsatz"),
            "reading order lost"
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn a_long_document_keeps_the_relevant_part_and_drops_the_rest() {
        // The property that matters on real files: the answer is in the middle,
        // and the budget is far smaller than the document.
        let filler = "Allgemeine Hinweise zur Verwaltung. ".repeat(60);
        let text = format!("{filler}\n\nDie Frist endet am 30. September.\n\n{filler}");
        let picked = relevant_chunks(&text, "Wann endet die Frist?", 1_200);
        assert!(
            picked.contains("Frist endet am 30. September"),
            "lost the answer:\n{picked}"
        );
        assert!(picked.chars().count() <= 1_600);
        assert!(text.chars().count() > 4_000);
    }

    #[test]
    fn a_summary_request_is_retrieved_differently_from_a_question() {
        assert!(summary_style("Fasse diese PDF zusammen."));
        assert!(summary_style("Worum geht es in dem Dokument?"));
        assert!(!summary_style("Wie hoch ist die Kündigungsfrist?"));
    }
    #[test]
    fn first_page_is_bounded_without_page_markers() {
        let path = std::env::temp_dir().join(format!("noki-page-{}.txt", std::process::id()));
        fs::write(&path, "a".repeat(20_000)).unwrap();
        assert_eq!(
            extract(&path, Some(1)).unwrap().text.chars().count(),
            12_000
        );
        let _ = fs::remove_file(path);
    }
}
