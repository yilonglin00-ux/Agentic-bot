//! Live tests for the deterministic-computation and file-output paths.
//!
//! The CSV test talks to the local llama.cpp server, so it is `#[ignore]`d;
//! the PDF test needs nothing but the filesystem and is also kept here because
//! it belongs to the same story: compute, interpret, then produce a document.
//!
//!     cargo test --test pipeline_live -- --ignored --nocapture

use app_lib::{data_analysis, doc_chunks, doc_output};

const CSV: &str = "\
Monat,Umsatz,Region,Kunden
2026-01,10000,Nord,120
2026-02,10800,Sued,131
2026-03,11500,Nord,128
2026-04,12200,Sued,140
2026-05,13100,Nord,155
2026-06,48000,Sued,141
2026-07,13900,Nord,160
2026-08,14500,Sued,158
";

fn ask_local(prompt: &str, max_tokens: u32) -> Result<String, String> {
    // Minimal HTTP/1.1 over TCP, the same way model_manager does it - so this
    // test needs no dependency the crate does not already have.
    use std::io::{Read, Write};
    let body = serde_json::json!({
        "model": "qwen3.5-9b",
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": max_tokens,
        "temperature": 0.0,
        "cache_prompt": true,
        // NORMAL tier: no thinking channel.
        "chat_template_kwargs": {"enable_thinking": false}
    })
    .to_string();
    let mut s = std::net::TcpStream::connect("127.0.0.1:8080").map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(std::time::Duration::from_secs(240)))
        .ok();
    let req = format!(
        "POST /v1/chat/completions HTTP/1.1\r\nHost: 127.0.0.1:8080\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&raw);
    let json_start = text.find("\r\n\r\n").ok_or("keine Antwort")? + 4;
    // Chunked or not, the JSON object is the last balanced `{...}` in the body.
    let payload = &text[json_start..];
    let start = payload.find('{').ok_or("kein JSON")?;
    let end = payload.rfind('}').ok_or("kein JSON")? + 1;
    let v: serde_json::Value =
        serde_json::from_str(&payload[start..end]).map_err(|e| e.to_string())?;
    Ok(v.pointer("/choices/0/message/content")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_owned())
}

#[test]
#[ignore = "needs the local llama.cpp server on 127.0.0.1:8080"]
fn csv_figures_are_computed_in_rust_and_only_interpreted_by_the_model() {
    // 1. DETERMINISTIC. Every number here comes from Rust, not from a model.
    let a = data_analysis::analyze_table(CSV).expect("CSV should parse");
    let umsatz = a.columns[1].numeric.as_ref().expect("Umsatz is numeric");

    println!("rows={} columns={}", a.rows, a.columns.len());
    println!(
        "Umsatz: sum={} mean={:.1} median={} outlier_rows={:?}",
        umsatz.sum, umsatz.mean, umsatz.median, umsatz.outlier_rows
    );

    // The planted anomaly is row 6 (48000). This is an exact assertion, which
    // is the entire point: an LLM-derived figure could not be asserted on.
    assert_eq!(umsatz.outlier_rows, vec![6]);
    assert_eq!(umsatz.outlier_values, vec![48000.0]);
    assert_eq!(umsatz.sum, 134_000.0);
    assert_eq!(umsatz.count, 8);
    assert!(umsatz.trend_pct_per_row > 0.0, "the series rises");

    let facts = data_analysis::facts_block(&a);
    println!("--- facts block ({} bytes) ---\n{facts}", facts.len());
    assert!(
        facts.len() < 4_000,
        "the facts block must stay cheap context"
    );

    // 2. INTERPRETATION. The model is given the finished figures and asked to
    //    explain them. It is told not to recalculate, and the numbers it needs
    //    are already present, so it has no reason to.
    let prompt = format!(
        "Du bist ein Analyst. Die folgenden Kennzahlen wurden bereits exakt berechnet. \
         Verwende sie unveraendert und rechne nichts neu.\n\n{facts}\n\
         Frage: Analysiere die Daten und nenne die drei wichtigsten Auffaelligkeiten. \
         Antworte knapp in drei Punkten."
    );
    let started = std::time::Instant::now();
    let answer = match ask_local(&prompt, 600) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("local model unreachable, skipping interpretation: {e}");
            return;
        }
    };
    println!("--- model answer ({:?}) ---\n{answer}", started.elapsed());
    assert!(
        !answer.is_empty(),
        "NORMAL tier must produce a visible answer"
    );

    // The model should be talking about the anomaly the ANALYSER found. It is
    // free to phrase it how it likes, so the assertion is on the figure.
    assert!(
        answer.contains("48000")
            || answer.contains("48.000")
            || answer.to_lowercase().contains("juni")
            || answer.contains("2026-06")
            || answer.to_lowercase().contains("ausrei"),
        "the answer ignored the computed anomaly:\n{answer}"
    );
}

#[test]
fn a_report_becomes_a_real_pdf_under_a_scoped_lease() {
    let dir = std::env::temp_dir().join(format!("noki-pipe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("NOKI_OUTPUT_DIR", &dir);

    // The content a document task would produce.
    let a = data_analysis::analyze_table(CSV).unwrap();
    let facts = data_analysis::facts_block(&a);
    let body = format!(
        "## Ergebnis\n\nDer Umsatz steigt, mit einem Ausreißer im Juni.\n\n\
         | Monat | Umsatz |\n|---|---|\n| 2026-05 | 13100 |\n| 2026-06 | 48000 |\n\n\
         - Größe des Ausreißers: 48000\n- Gesamtsumme: {}\n",
        a.columns[1].numeric.as_ref().unwrap().sum
    );
    assert!(facts.contains("Zeile 6"));

    // The ACT lease: one capability, one exact path, single use.
    let leases = app_lib::capability::LeaseStore::default();
    let (target, replaced) =
        doc_output::resolve_target("Bericht", doc_output::OutputFormat::Pdf, false).unwrap();
    let lease = leases.issue(
        "file.create",
        &target.to_string_lossy(),
        1,
        app_lib::capability::RiskLevel::R1,
    );
    let written = doc_output::write_file(
        &leases,
        lease.id,
        1,
        "erstelle daraus eine pdf",
        &target,
        doc_output::OutputFormat::Pdf,
        "Bericht",
        &body,
        replaced,
    )
    .unwrap();

    println!("wrote {} ({} bytes)", written.path, written.bytes);

    // It is a REAL PDF, not markdown wearing a .pdf extension.
    let bytes = std::fs::read(&target).unwrap();
    assert!(bytes.starts_with(b"%PDF-1.4"));
    assert!(bytes.ends_with(b"%%EOF\n"));
    assert!(!String::from_utf8_lossy(&bytes).contains("## Ergebnis"));

    // macOS agrees it is a PDF and can read the text back out - the strongest
    // available check short of opening it by hand.
    let file = std::process::Command::new("/usr/bin/file")
        .arg(&target)
        .output()
        .unwrap();
    let kind = String::from_utf8_lossy(&file.stdout);
    println!("file(1) says: {}", kind.trim());
    assert!(kind.contains("PDF document"), "{kind}");

    let swift = format!(
        "import PDFKit\nlet d = PDFDocument(url: URL(fileURLWithPath: \"{}\"))!\nprint(d.string ?? \"\")",
        target.to_string_lossy()
    );
    if let Ok(out) = std::process::Command::new("/usr/bin/swift")
        .args(["-e", &swift])
        .output()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        println!("--- text extracted back from the PDF ---\n{}", text.trim());
        assert!(
            text.contains("Bericht"),
            "title missing from the rendered PDF"
        );
        assert!(text.contains("48000"), "table content missing");
        // The umlaut round trip, which is what WinAnsi encoding is for.
        assert!(text.contains("Ausreißer"), "umlauts were mangled: {text}");
        assert!(text.contains("Größe"), "umlauts were mangled: {text}");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_large_document_is_retrieved_not_truncated() {
    // 400 KB of text with the answer buried at 70% depth.
    let mut text = String::new();
    for i in 0..600 {
        if i == 420 {
            text.push_str("Die Vertragsstrafe betraegt 4.500 EUR pro Verstoss.\n\n");
        }
        text.push_str(&format!(
            "Abschnitt {i}. Allgemeine Regelungen und Verwaltungshinweise ohne besonderen Inhalt. {}\n\n",
            "Fuelltext ".repeat(50)
        ));
    }
    println!("document is {} KB", text.len() / 1024);
    assert!(text.len() > 300_000);

    let chunks = doc_chunks::segment(&text);
    println!("segmented into {} chunks", chunks.len());
    assert!(doc_chunks::needs_map_reduce(&chunks, 12_000));

    // A question finds the buried fact inside a small budget.
    let picked =
        doc_chunks::select_for_question(&chunks, "Wie hoch ist die Vertragsstrafe?", 6_000);
    let evidence = doc_chunks::render(&picked);
    println!(
        "retrieved {} chars from {} KB",
        evidence.len(),
        text.len() / 1024
    );
    assert!(
        evidence.contains("4.500 EUR"),
        "retrieval missed the buried fact"
    );
    assert!(evidence.chars().count() <= 7_000, "budget overrun");

    // And map/reduce covers the whole thing when the task demands it.
    let batches = doc_chunks::batches(&chunks, 12_000);
    println!("map/reduce would need {} passes", batches.len());
    let covered: usize = batches.iter().map(|b| b.chunks.len()).sum();
    assert_eq!(covered, chunks.len(), "map/reduce must cover every chunk");
}
