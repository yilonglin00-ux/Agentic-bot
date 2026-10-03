//! A minimal, real PDF writer.
//!
//! WHY NOT SHELL OUT. macOS can turn text into PDF (`cupsfilter`), and that was
//! the first thing tried - it works, but it produces a typewriter page with no
//! headings, no wrapping control and no table alignment, and it depends on the
//! printing subsystem being healthy. Writing the file directly is less code
//! than it looks, has no external dependency, and makes the output testable:
//! the tests below assert on actual PDF structure rather than on "a file
//! appeared".
//!
//! WHY NOT A CRATE. The project carries almost no dependencies on purpose (HTTP
//! here is raw `TcpStream`), and a PDF generator would be one of the larger
//! ones. What is needed is a subset: the base-14 fonts, one page size, text,
//! and rules. That subset is small enough to own.
//!
//! WHAT THIS PRODUCES. A PDF 1.4 document: a real header, one object per page,
//! a real cross-reference table, and a trailer. Text is laid out with Helvetica
//! and Helvetica-Bold, which every reader has built in, so nothing needs
//! embedding. Encoding is WinAnsi, which covers German text - the whole point,
//! since a report that mangles "Ausreißer" is not a finished report.

const PAGE_W: f64 = 595.28; // A4 at 72 dpi
const PAGE_H: f64 = 841.89;
const MARGIN: f64 = 56.0;
const LEADING: f64 = 14.0;

/// One piece of a document, already structured. The caller decides the
/// structure; this module only knows how to put it on paper.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Title(String),
    Heading(String),
    Paragraph(String),
    Bullet(String),
    /// A table with a header row. Columns are sized evenly.
    Table {
        header: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    /// Preformatted text, kept as-is and not re-wrapped.
    Code(String),
    Spacer,
    /// A horizontal rule.
    Rule,
}

#[derive(Clone, Copy, PartialEq)]
enum Font {
    Regular,
    Bold,
    Mono,
}

impl Font {
    fn resource(self) -> &'static str {
        match self {
            Font::Regular => "/F1",
            Font::Bold => "/F2",
            Font::Mono => "/F3",
        }
    }
    /// Average glyph width as a fraction of the font size. Helvetica's real
    /// widths vary per glyph; this approximation is what the wrapper uses to
    /// decide line breaks, and it is deliberately slightly generous so a line
    /// breaks early rather than running into the margin.
    fn width_factor(self) -> f64 {
        match self {
            Font::Regular => 0.50,
            Font::Bold => 0.54,
            Font::Mono => 0.60,
        }
    }
    fn char_width(self, size: f64) -> f64 {
        size * self.width_factor()
    }
}

/// Unicode -> WinAnsi (CP1252). Characters outside it become '?', which is
/// honest: a glyph that cannot be encoded must not silently vanish.
fn winansi(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| match c {
            '\u{20ac}' => 0x80, // €
            '\u{201a}' => 0x82,
            '\u{201e}' => 0x84, // „
            '\u{2026}' => 0x85, // …
            '\u{2018}' => 0x91,
            '\u{2019}' => 0x92, // ’
            '\u{201c}' => 0x93,
            '\u{201d}' => 0x94, // ”
            '\u{2022}' => 0x95, // •
            '\u{2013}' => 0x96, // –
            '\u{2014}' => 0x97, // —
            '\u{00a0}' => 0x20,
            c if (c as u32) <= 0xFF => c as u8,
            _ => b'?',
        })
        .collect()
}

/// Escapes a byte string for a PDF literal string.
fn escape(bytes: &[u8]) -> String {
    let mut out = String::new();
    for b in bytes {
        match b {
            b'(' => out.push_str("\\("),
            b')' => out.push_str("\\)"),
            b'\\' => out.push_str("\\\\"),
            0x20..=0x7e => out.push(*b as char),
            // Everything else goes out as an octal escape, which keeps the
            // file 7-bit clean and avoids any encoding guesswork by a reader.
            b => out.push_str(&format!("\\{b:03o}")),
        }
    }
    out
}

/// Breaks text into lines that fit `width`, splitting on spaces. A single word
/// longer than the line is hard-split rather than allowed to overflow.
fn wrap(text: &str, font: Font, size: f64, width: f64) -> Vec<String> {
    let max_chars = (width / font.char_width(size)).floor().max(8.0) as usize;
    let mut lines = Vec::new();
    for raw in text.split('\n') {
        let mut current = String::new();
        for word in raw.split_whitespace() {
            let mut word = word;
            // Hard-split an over-long token (a URL, a hash).
            while word.chars().count() > max_chars {
                if !current.is_empty() {
                    lines.push(std::mem::take(&mut current));
                }
                let head: String = word.chars().take(max_chars).collect();
                let consumed = head.chars().count();
                lines.push(head);
                word = &word[word
                    .char_indices()
                    .nth(consumed)
                    .map(|(i, _)| i)
                    .unwrap_or(word.len())..];
            }
            if word.is_empty() {
                continue;
            }
            let candidate = if current.is_empty() {
                word.to_owned()
            } else {
                format!("{current} {word}")
            };
            if candidate.chars().count() > max_chars {
                lines.push(std::mem::take(&mut current));
                current = word.to_owned();
            } else {
                current = candidate;
            }
        }
        lines.push(current);
    }
    // Trailing empties from consecutive newlines are kept as blank lines, but a
    // single trailing empty from a clean split is not.
    while lines.last().is_some_and(|l| l.is_empty()) && lines.len() > 1 {
        lines.pop();
    }
    lines
}

/// One drawing instruction in page space.
enum Op {
    Text {
        x: f64,
        y: f64,
        font: Font,
        size: f64,
        text: String,
    },
    Rule {
        y: f64,
    },
}

/// Lays blocks out into pages, then serialises them.
pub fn render(blocks: &[Block]) -> Vec<u8> {
    let usable = PAGE_W - 2.0 * MARGIN;
    let mut pages: Vec<Vec<Op>> = Vec::new();
    let mut page: Vec<Op> = Vec::new();
    let mut y = PAGE_H - MARGIN;

    // `need` reserves vertical space, starting a new page when the current one
    // cannot hold it. Keeping this in one closure is what stops a heading from
    // being orphaned at the foot of a page.
    let mut ensure = |need: f64, page: &mut Vec<Op>, y: &mut f64, pages: &mut Vec<Vec<Op>>| {
        if *y - need < MARGIN {
            pages.push(std::mem::take(page));
            *y = PAGE_H - MARGIN;
        }
    };

    for block in blocks {
        match block {
            Block::Title(t) => {
                let size = 20.0;
                for line in wrap(t, Font::Bold, size, usable) {
                    ensure(size + 8.0, &mut page, &mut y, &mut pages);
                    y -= size + 4.0;
                    page.push(Op::Text {
                        x: MARGIN,
                        y,
                        font: Font::Bold,
                        size,
                        text: line,
                    });
                }
                y -= 10.0;
            }
            Block::Heading(h) => {
                let size = 13.5;
                // A heading plus at least one line of its text, or it moves on.
                ensure(size + LEADING + 12.0, &mut page, &mut y, &mut pages);
                y -= size + 10.0;
                for line in wrap(h, Font::Bold, size, usable) {
                    page.push(Op::Text {
                        x: MARGIN,
                        y,
                        font: Font::Bold,
                        size,
                        text: line,
                    });
                    y -= size + 2.0;
                }
                y -= 2.0;
            }
            Block::Paragraph(p) => {
                let size = 10.5;
                for line in wrap(p, Font::Regular, size, usable) {
                    ensure(LEADING, &mut page, &mut y, &mut pages);
                    y -= LEADING;
                    page.push(Op::Text {
                        x: MARGIN,
                        y,
                        font: Font::Regular,
                        size,
                        text: line,
                    });
                }
                y -= 4.0;
            }
            Block::Bullet(b) => {
                let size = 10.5;
                let indent = 16.0;
                let lines = wrap(b, Font::Regular, size, usable - indent);
                for (i, line) in lines.iter().enumerate() {
                    ensure(LEADING, &mut page, &mut y, &mut pages);
                    y -= LEADING;
                    if i == 0 {
                        page.push(Op::Text {
                            x: MARGIN,
                            y,
                            font: Font::Regular,
                            size,
                            text: "\u{2022}".into(),
                        });
                    }
                    page.push(Op::Text {
                        x: MARGIN + indent,
                        y,
                        font: Font::Regular,
                        size,
                        text: line.clone(),
                    });
                }
            }
            Block::Code(c) => {
                let size = 9.0;
                for line in c.split('\n') {
                    for piece in wrap(line, Font::Mono, size, usable - 8.0) {
                        ensure(12.0, &mut page, &mut y, &mut pages);
                        y -= 12.0;
                        page.push(Op::Text {
                            x: MARGIN + 8.0,
                            y,
                            font: Font::Mono,
                            size,
                            text: piece,
                        });
                    }
                }
                y -= 4.0;
            }
            Block::Table { header, rows } => {
                let size = 9.5;
                let cols = header.len().max(1);
                let col_w = usable / cols as f64;
                let mut draw_row = |cells: &[String],
                                    font: Font,
                                    page: &mut Vec<Op>,
                                    y: &mut f64,
                                    pages: &mut Vec<Vec<Op>>| {
                    // A row is as tall as its tallest wrapped cell.
                    let wrapped: Vec<Vec<String>> = cells
                        .iter()
                        .map(|c| wrap(c, font, size, col_w - 6.0))
                        .collect();
                    let height = wrapped.iter().map(|w| w.len()).max().unwrap_or(1) as f64 * 11.5;
                    if *y - height < MARGIN {
                        pages.push(std::mem::take(page));
                        *y = PAGE_H - MARGIN;
                    }
                    let top = *y;
                    for (i, lines) in wrapped.iter().enumerate() {
                        for (j, line) in lines.iter().enumerate() {
                            page.push(Op::Text {
                                x: MARGIN + i as f64 * col_w + 2.0,
                                y: top - 11.5 * (j as f64 + 1.0),
                                font,
                                size,
                                text: line.clone(),
                            });
                        }
                    }
                    *y = top - height;
                };
                ensure(34.0, &mut page, &mut y, &mut pages);
                draw_row(header, Font::Bold, &mut page, &mut y, &mut pages);
                page.push(Op::Rule { y: y - 2.0 });
                y -= 6.0;
                for row in rows {
                    draw_row(row, Font::Regular, &mut page, &mut y, &mut pages);
                }
                y -= 6.0;
            }
            Block::Rule => {
                ensure(10.0, &mut page, &mut y, &mut pages);
                y -= 8.0;
                page.push(Op::Rule { y });
            }
            Block::Spacer => {
                y -= LEADING;
            }
        }
    }
    pages.push(page);
    // A document is never zero pages, even from no blocks at all.
    if pages.is_empty() {
        pages.push(Vec::new());
    }
    serialise(&pages)
}

fn content_stream(ops: &[Op]) -> Vec<u8> {
    let mut s = String::new();
    for op in ops {
        match op {
            Op::Text {
                x,
                y,
                font,
                size,
                text,
            } => {
                if text.is_empty() {
                    continue;
                }
                s.push_str(&format!(
                    "BT {} {size:.1} Tf 1 0 0 1 {x:.2} {y:.2} Tm ({}) Tj ET\n",
                    font.resource(),
                    escape(&winansi(text))
                ));
            }
            Op::Rule { y } => {
                s.push_str(&format!(
                    "0.75 w 0.6 G {MARGIN:.2} {y:.2} m {:.2} {y:.2} l S\n",
                    PAGE_W - MARGIN
                ));
            }
        }
    }
    s.into_bytes()
}

/// Assembles the object graph and the cross-reference table.
///
/// Object numbering: 1 = catalog, 2 = page tree, 3..5 = fonts, then for each
/// page a page object followed by its content stream.
fn serialise(pages: &[Vec<Op>]) -> Vec<u8> {
    let n = pages.len();
    let first_page_obj = 6;
    let page_ids: Vec<usize> = (0..n).map(|i| first_page_obj + i * 2).collect();

    let mut objects: Vec<(usize, Vec<u8>)> = Vec::new();
    objects.push((1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()));
    let kids = page_ids
        .iter()
        .map(|id| format!("{id} 0 R"))
        .collect::<Vec<_>>()
        .join(" ");
    objects.push((
        2,
        format!("<< /Type /Pages /Count {n} /Kids [{kids}] >>").into_bytes(),
    ));
    for (id, base) in [(3usize, "Helvetica"), (4, "Helvetica-Bold"), (5, "Courier")] {
        objects.push((
            id,
            format!(
                "<< /Type /Font /Subtype /Type1 /BaseFont /{base} /Encoding /WinAnsiEncoding >>"
            )
            .into_bytes(),
        ));
    }
    for (i, ops) in pages.iter().enumerate() {
        let page_id = page_ids[i];
        let content_id = page_id + 1;
        objects.push((
            page_id,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_W:.2} {PAGE_H:.2}] \
                 /Resources << /Font << /F1 3 0 R /F2 4 0 R /F3 5 0 R >> >> /Contents {content_id} 0 R >>"
            )
            .into_bytes(),
        ));
        let stream = content_stream(ops);
        let mut obj = format!("<< /Length {} >>\nstream\n", stream.len()).into_bytes();
        obj.extend_from_slice(&stream);
        obj.extend_from_slice(b"\nendstream");
        objects.push((content_id, obj));
    }
    objects.sort_by_key(|(id, _)| *id);

    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"%PDF-1.4\n");
    // A binary comment marks the file as binary for transfer tools.
    out.extend_from_slice(&[b'%', 0xE2, 0xE3, 0xCF, 0xD3, b'\n']);
    let mut offsets: Vec<(usize, usize)> = Vec::new();
    for (id, body) in &objects {
        offsets.push((*id, out.len()));
        out.extend_from_slice(format!("{id} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref_at = out.len();
    let max_id = objects.last().map(|(id, _)| *id).unwrap_or(0);
    out.extend_from_slice(format!("xref\n0 {}\n", max_id + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for id in 1..=max_id {
        let off = offsets
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, o)| *o)
            .unwrap_or(0);
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            max_id + 1
        )
        .as_bytes(),
    );
    out
}

/// Turns a Markdown-ish text into blocks.
///
/// This is the bridge from what a model writes to what can be typeset. It is
/// deliberately forgiving: a local model does not emit perfectly clean
/// Markdown, so anything unrecognised becomes a paragraph rather than being
/// dropped or shown with its syntax.
pub fn blocks_from_markdown(md: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut paragraph = String::new();
    let mut code: Option<String> = None;
    let mut table: Vec<Vec<String>> = Vec::new();

    fn flush_paragraph(paragraph: &mut String, blocks: &mut Vec<Block>) {
        let t = paragraph.trim();
        if !t.is_empty() {
            blocks.push(Block::Paragraph(t.to_owned()));
        }
        paragraph.clear();
    }
    fn flush_table(table: &mut Vec<Vec<String>>, blocks: &mut Vec<Block>) {
        if table.is_empty() {
            return;
        }
        let header = table.remove(0);
        blocks.push(Block::Table {
            header,
            rows: std::mem::take(table),
        });
    }
    let cells = |line: &str| -> Vec<String> {
        line.trim()
            .trim_matches('|')
            .split('|')
            .map(|c| strip_inline(c.trim()))
            .collect()
    };

    for line in md.lines() {
        let t = line.trim();
        // Fenced code.
        if t.starts_with("```") {
            match code.take() {
                Some(body) => blocks.push(Block::Code(body.trim_end().to_owned())),
                None => {
                    flush_paragraph(&mut paragraph, &mut blocks);
                    flush_table(&mut table, &mut blocks);
                    code = Some(String::new());
                }
            }
            continue;
        }
        if let Some(body) = code.as_mut() {
            body.push_str(line);
            body.push('\n');
            continue;
        }
        // Tables: a row of pipes. The |---|---| separator is skipped.
        if t.starts_with('|') && t.matches('|').count() >= 2 {
            let row = cells(t);
            let is_separator = row
                .iter()
                .all(|c| !c.is_empty() && c.chars().all(|x| x == '-' || x == ':' || x == ' '));
            if !is_separator {
                flush_paragraph(&mut paragraph, &mut blocks);
                table.push(row);
            }
            continue;
        }
        flush_table(&mut table, &mut blocks);

        if t.is_empty() {
            flush_paragraph(&mut paragraph, &mut blocks);
            continue;
        }
        if let Some(rest) = t.strip_prefix("# ") {
            flush_paragraph(&mut paragraph, &mut blocks);
            blocks.push(Block::Title(strip_inline(rest)));
            continue;
        }
        if let Some(rest) = t
            .strip_prefix("## ")
            .or_else(|| t.strip_prefix("### "))
            .or_else(|| t.strip_prefix("#### "))
        {
            flush_paragraph(&mut paragraph, &mut blocks);
            blocks.push(Block::Heading(strip_inline(rest)));
            continue;
        }
        if t == "---" || t == "***" || t == "___" {
            flush_paragraph(&mut paragraph, &mut blocks);
            blocks.push(Block::Rule);
            continue;
        }
        if let Some(rest) = t
            .strip_prefix("- ")
            .or_else(|| t.strip_prefix("* "))
            .or_else(|| t.strip_prefix("• "))
        {
            flush_paragraph(&mut paragraph, &mut blocks);
            blocks.push(Block::Bullet(strip_inline(rest)));
            continue;
        }
        // "1. " style lists keep their number, which is information.
        if t.len() > 3
            && t.chars().next().is_some_and(|c| c.is_ascii_digit())
            && t.split_once(". ").is_some()
        {
            flush_paragraph(&mut paragraph, &mut blocks);
            blocks.push(Block::Bullet(strip_inline(t)));
            continue;
        }
        if !paragraph.is_empty() {
            paragraph.push(' ');
        }
        paragraph.push_str(&strip_inline(t));
    }
    if let Some(body) = code {
        blocks.push(Block::Code(body.trim_end().to_owned()));
    }
    flush_table(&mut table, &mut blocks);
    flush_paragraph(&mut paragraph, &mut blocks);
    blocks
}

/// Removes inline emphasis markers. Bold-inside-a-paragraph is not worth a
/// rich-text model here; showing `**` in a finished PDF is worse than losing
/// the emphasis.
fn strip_inline(s: &str) -> String {
    let mut out = s.replace("**", "").replace("__", "");
    if out.contains('`') {
        out = out.replace('`', "");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Block> {
        vec![
            Block::Title("Quartalsbericht".into()),
            Block::Paragraph(
                "Ausreißer in Zeile 6, Größe über dem Grenzwert. Preis: 1.200 €.".into(),
            ),
            Block::Heading("Kennzahlen".into()),
            Block::Table {
                header: vec!["Monat".into(), "Umsatz".into()],
                rows: vec![
                    vec!["Januar".into(), "1000".into()],
                    vec!["Februar".into(), "1100".into()],
                ],
            },
            Block::Bullet("Erster Punkt".into()),
            Block::Rule,
            Block::Code("let x = 1;".into()),
        ]
    }

    #[test]
    fn the_output_is_a_structurally_valid_pdf() {
        let pdf = render(&sample());
        assert!(pdf.starts_with(b"%PDF-1.4"), "no PDF header");
        let s = String::from_utf8_lossy(&pdf);
        assert!(s.contains("/Type /Catalog"));
        assert!(s.contains("/Type /Pages"));
        assert!(s.contains("/Type /Page "));
        assert!(s.contains("/BaseFont /Helvetica"));
        assert!(s.contains("stream") && s.contains("endstream"));
        assert!(s.contains("xref"));
        assert!(s.trim_end().ends_with("%%EOF"));
        // startxref must point at the real xref table.
        let at: usize = s
            .rsplit("startxref\n")
            .next()
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(
            &pdf[at..at + 4],
            b"xref",
            "startxref does not point at xref"
        );
    }

    #[test]
    fn every_object_offset_in_the_xref_is_correct() {
        // A wrong offset is the classic way a hand-written PDF opens in one
        // reader and fails in another, so it is checked directly.
        let pdf = render(&sample());
        // The binary marker after the header is not UTF-8, so a lossy string
        // would shift every index. Offsets are byte offsets, so this works on
        // the bytes - which is also exactly how a reader parses the file.
        let find = |needle: &[u8]| -> usize {
            pdf.windows(needle.len())
                .rposition(|w| w == needle)
                .expect("needle not found")
        };
        // "\nxref\n", not "xref\n": the latter also matches inside "startxref".
        let xref_at = find(b"\nxref\n") + 1;
        let table = String::from_utf8_lossy(&pdf[xref_at..]).into_owned();
        let mut lines = table.lines();
        assert_eq!(lines.next().unwrap(), "xref");
        let count: usize = lines
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let entries: Vec<String> = lines.take(count).map(str::to_owned).collect();
        assert_eq!(entries.len(), count);
        for (id, entry) in entries.iter().enumerate().skip(1) {
            let off: usize = entry.split_whitespace().next().unwrap().parse().unwrap();
            let expect = format!("{id} 0 obj");
            assert_eq!(
                &pdf[off..off + expect.len()],
                expect.as_bytes(),
                "object {id} points at the wrong byte offset"
            );
        }
        // And the trailer's /Size covers every object.
        assert!(
            String::from_utf8_lossy(&pdf[find(b"trailer")..]).contains(&format!("/Size {count}"))
        );
    }

    #[test]
    fn german_text_survives_as_winansi() {
        let pdf = render(&[Block::Paragraph("Größe Ausreißer Übermaß".into())]);
        let s = String::from_utf8_lossy(&pdf);
        // ö = 0xF6 = \366, ß = 0xDF = \337, Ü = 0xDC = \334
        assert!(s.contains("\\366"), "o-umlaut not encoded");
        assert!(s.contains("\\337"), "sharp s not encoded");
        assert!(s.contains("\\334"), "U-umlaut not encoded");
        assert!(s.contains("/WinAnsiEncoding"));
        // And a euro sign, which is CP1252-specific.
        let e = render(&[Block::Paragraph("1200 €".into())]);
        assert!(String::from_utf8_lossy(&e).contains("\\200"));
    }

    #[test]
    fn parentheses_and_backslashes_cannot_break_out_of_a_string() {
        // An unescaped ")" would end the string early and corrupt the file.
        let pdf = render(&[Block::Paragraph(r"Ein (Test) mit \ Zeichen".into())]);
        let s = String::from_utf8_lossy(&pdf);
        assert!(s.contains("\\(Test\\)"), "parentheses not escaped");
        assert!(s.contains("\\\\"), "backslash not escaped");
    }

    #[test]
    fn long_documents_paginate() {
        let blocks: Vec<Block> = (0..200)
            .map(|i| Block::Paragraph(format!("Absatz Nummer {i} mit etwas Text darin.")))
            .collect();
        let pdf = render(&blocks);
        let s = String::from_utf8_lossy(&pdf);
        let pages = s.matches("/Type /Page ").count();
        assert!(pages >= 4, "expected several pages, got {pages}");
        let declared: usize = s
            .split("/Count ")
            .nth(1)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(declared, pages, "/Count disagrees with the page objects");
    }

    #[test]
    fn an_empty_document_is_still_a_valid_one_page_pdf() {
        let pdf = render(&[]);
        assert!(pdf.starts_with(b"%PDF"));
        let s = String::from_utf8_lossy(&pdf);
        assert!(s.contains("/Count 1"));
        assert!(s.trim_end().ends_with("%%EOF"));
    }

    #[test]
    fn an_unbreakable_token_is_split_instead_of_overflowing() {
        let long = "x".repeat(400);
        let lines = wrap(&long, Font::Regular, 10.5, PAGE_W - 2.0 * MARGIN);
        assert!(lines.len() > 1, "a long token must be split");
        assert!(lines.iter().all(|l| l.chars().count() <= 100));
    }

    #[test]
    fn markdown_becomes_the_right_blocks() {
        let md = "\
# Bericht

Ein Absatz mit **Betonung**.

## Zahlen

| Monat | Umsatz |
|-------|--------|
| Jan   | 1000   |

- Punkt eins
- Punkt zwei

1. Erster Schritt

---

```
code hier
```
";
        let b = blocks_from_markdown(md);
        assert_eq!(b[0], Block::Title("Bericht".into()));
        assert_eq!(b[1], Block::Paragraph("Ein Absatz mit Betonung.".into()));
        assert_eq!(b[2], Block::Heading("Zahlen".into()));
        match &b[3] {
            Block::Table { header, rows } => {
                assert_eq!(header, &vec!["Monat".to_string(), "Umsatz".to_string()]);
                assert_eq!(rows.len(), 1, "the |---| separator must not be a row");
                assert_eq!(rows[0], vec!["Jan".to_string(), "1000".to_string()]);
            }
            other => panic!("expected a table, got {other:?}"),
        }
        assert_eq!(b[4], Block::Bullet("Punkt eins".into()));
        assert_eq!(b[6], Block::Bullet("1. Erster Schritt".into()));
        assert_eq!(b[7], Block::Rule);
        assert_eq!(b[8], Block::Code("code hier".into()));
    }

    #[test]
    fn markdown_without_any_structure_still_produces_content() {
        let b = blocks_from_markdown("Nur ein Satz ohne Struktur.");
        assert_eq!(b.len(), 1);
        assert!(matches!(b[0], Block::Paragraph(_)));
        // And it renders.
        assert!(render(&b).starts_with(b"%PDF"));
    }
}
