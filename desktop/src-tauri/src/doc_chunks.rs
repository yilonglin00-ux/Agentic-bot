//! Turning a large document into the few passages that answer a question.
//!
//! THE PROBLEM. A 200-page PDF does not fit in 8k tokens, and truncating it to
//! the first 12,000 characters answers questions about the table of contents.
//! Both failures are silent, which is the worst property a document pipeline
//! can have: the answer looks fine and is about the wrong part of the file.
//!
//! THE PIPELINE. extract -> segment -> score against the question -> take what
//! fits -> (for a whole-document task) map/reduce. A question gets the passages
//! that mention what it asks about; a summary gets an even spread across the
//! whole document, because "summarise this" has no keywords to match on and
//! taking the top-scoring passages would just return the introduction.
//!
//! SCORING. Term frequency with an inverse-document-frequency weight, so a word
//! that appears in every passage ("Vertrag" in a contract) counts for little
//! and a rare, specific word counts for a lot. This is deliberately not an
//! embedding model: it needs no weights, no load time and no GPU, it is
//! deterministic, and for keyword-shaped questions over one document it is
//! competitive. Where semantic matching is needed, `semantic.rs` already
//! exists and this module's output feeds it rather than replacing it.

use serde::{Deserialize, Serialize};

/// One passage of a document, with where it came from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Chunk {
    /// Position in the document, 0-based. Used to restore reading order.
    pub index: usize,
    pub text: String,
    /// Page number when the source had page breaks, else `None`.
    pub page: Option<usize>,
}

/// Target size of one passage, in characters. Large enough to hold a whole
/// argument, small enough that several fit in a context window.
const TARGET_CHARS: usize = 1_400;
/// Overlap between neighbours, so a sentence spanning a boundary is not lost
/// to both sides.
const OVERLAP_CHARS: usize = 180;

/// Splits text into overlapping passages, preferring paragraph boundaries.
///
/// The form-feed character is how the extractors mark a page break, so page
/// numbers survive into the chunks and an answer can say where it read something.
pub fn segment(text: &str) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let pages: Vec<&str> = text.split('\u{000c}').collect();
    let paged = pages.len() > 1;

    for (page_idx, page) in pages.iter().enumerate() {
        let page_no = paged.then_some(page_idx + 1);
        // Paragraphs first; a paragraph longer than the target is split on
        // sentence ends, and only then hard-split.
        let mut buffer = String::new();
        let mut flush = |buffer: &mut String, chunks: &mut Vec<Chunk>| {
            let t = buffer.trim();
            if !t.is_empty() {
                chunks.push(Chunk {
                    index: chunks.len(),
                    text: t.to_owned(),
                    page: page_no,
                });
            }
            buffer.clear();
        };
        for para in page.split("\n\n") {
            let para = para.trim();
            if para.is_empty() {
                continue;
            }
            if buffer.chars().count() + para.chars().count() <= TARGET_CHARS {
                if !buffer.is_empty() {
                    buffer.push_str("\n\n");
                }
                buffer.push_str(para);
                continue;
            }
            flush(&mut buffer, &mut chunks);
            if para.chars().count() <= TARGET_CHARS {
                buffer.push_str(para);
                continue;
            }
            // An over-long paragraph: break it at sentence ends.
            let mut current = String::new();
            for sentence in split_sentences(para) {
                if current.chars().count() + sentence.chars().count() > TARGET_CHARS
                    && !current.is_empty()
                {
                    // Carry the tail forward so the boundary is not a cliff.
                    let tail: String = {
                        let c: Vec<char> = current.chars().collect();
                        c[c.len().saturating_sub(OVERLAP_CHARS)..].iter().collect()
                    };
                    chunks.push(Chunk {
                        index: chunks.len(),
                        text: current.trim().to_owned(),
                        page: page_no,
                    });
                    current = tail;
                }
                if !current.is_empty() && !current.ends_with(' ') {
                    current.push(' ');
                }
                current.push_str(&sentence);
            }
            if !current.trim().is_empty() {
                buffer.push_str(current.trim());
            }
        }
        flush(&mut buffer, &mut chunks);
    }
    // Re-index so indices are contiguous across pages.
    for (i, c) in chunks.iter_mut().enumerate() {
        c.index = i;
    }
    chunks
}

/// Splits on sentence ends, keeping the terminator. Hard-splits a "sentence"
/// that is really a wall of text with no punctuation.
fn split_sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for c in text.chars() {
        current.push(c);
        if matches!(c, '.' | '!' | '?' | '\n') && current.chars().count() > 20 {
            out.push(std::mem::take(&mut current));
        } else if current.chars().count() >= TARGET_CHARS {
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.trim().is_empty() {
        out.push(current);
    }
    out
}

fn terms(q: &str) -> Vec<String> {
    const STOP: &[&str] = &[
        "der", "die", "das", "und", "oder", "ist", "sind", "war", "waren", "ein", "eine", "einen",
        "einem", "eines", "den", "dem", "des", "mit", "von", "für", "auf", "aus", "bei", "nach",
        "über", "unter", "was", "wie", "wer", "wo", "welche", "welcher", "welches", "dieser",
        "diese", "dieses", "hier", "dort", "nicht", "auch", "aber", "dass", "sich", "kann", "soll",
        "the", "and", "for", "with", "this", "that",
    ];
    q.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3 && !STOP.contains(w))
        .map(str::to_owned)
        .collect()
}

/// Scores each chunk against the question. Higher is more relevant.
///
/// The idf weight is what makes this work on a single document: without it,
/// the most common word in the file dominates and every chunk scores the same.
pub fn score(chunks: &[Chunk], question: &str) -> Vec<f64> {
    let terms = terms(question);
    if terms.is_empty() || chunks.is_empty() {
        return vec![0.0; chunks.len()];
    }
    let lowered: Vec<String> = chunks.iter().map(|c| c.text.to_lowercase()).collect();
    let n = chunks.len() as f64;
    let mut scores = vec![0.0; chunks.len()];
    for term in &terms {
        let containing = lowered.iter().filter(|t| t.contains(term.as_str())).count();
        if containing == 0 {
            continue;
        }
        // Standard smoothed idf: a term in every chunk contributes ~0.
        let idf = ((n + 1.0) / (containing as f64 + 0.5)).ln().max(0.0);
        for (i, text) in lowered.iter().enumerate() {
            let tf = text.matches(term.as_str()).count() as f64;
            if tf > 0.0 {
                // Saturating term frequency: ten mentions are not ten times as
                // relevant as one.
                scores[i] += idf * (1.0 + tf.ln());
            }
        }
    }
    scores
}

/// The passages to put in front of the model for a QUESTION.
///
/// Selected by score, then restored to document order - a model reading
/// passages out of order loses the thread, and page 9 before page 2 reads as a
/// contradiction.
pub fn select_for_question(chunks: &[Chunk], question: &str, max_chars: usize) -> Vec<Chunk> {
    let scores = score(chunks, question);
    let mut ranked: Vec<(usize, f64)> = scores.iter().copied().enumerate().collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut picked: Vec<usize> = Vec::new();
    let mut used = 0usize;
    for (i, s) in ranked {
        // A zero score means the chunk shares nothing with the question. Taking
        // it would spend context on noise - unless nothing matched at all, in
        // which case the document opening is the honest default.
        if s <= 0.0 && !picked.is_empty() {
            break;
        }
        let len = chunks[i].text.chars().count();
        if used + len > max_chars {
            continue;
        }
        used += len;
        picked.push(i);
        if used >= max_chars {
            break;
        }
    }
    if picked.is_empty() && !chunks.is_empty() {
        return spread(chunks, max_chars);
    }
    picked.sort_unstable();
    picked.into_iter().map(|i| chunks[i].clone()).collect()
}

/// An even spread across the whole document, for a task with no keywords to
/// match on - a summary, a report, "what is in here".
///
/// This is the part that stops a summary from being a summary of page one.
pub fn spread(chunks: &[Chunk], max_chars: usize) -> Vec<Chunk> {
    if chunks.is_empty() {
        return Vec::new();
    }
    let total: usize = chunks.iter().map(|c| c.text.chars().count()).sum();
    if total <= max_chars {
        return chunks.to_vec();
    }
    // How many chunks fit, on average.
    let avg = (total / chunks.len()).max(1);
    let room = (max_chars / avg).max(1);
    if room >= chunks.len() {
        return chunks.to_vec();
    }
    // Sample at even intervals ACROSS THE WHOLE RANGE, so the first and the
    // last passage are both included. Stepping by `len / room` instead would
    // stop at `(room-1) * step`, which is short of the end - that is the
    // truncation bug this function exists to avoid, in a subtler form.
    let last = chunks.len() - 1;
    let mut out: Vec<Chunk> = Vec::new();
    let mut used = 0usize;
    for k in 0..room {
        let i = if room == 1 {
            0
        } else {
            ((k as f64) * last as f64 / (room - 1) as f64).round() as usize
        };
        let Some(c) = chunks.get(i.min(last)) else {
            continue;
        };
        let len = c.text.chars().count();
        if used + len > max_chars {
            break;
        }
        if out.last().map(|l: &Chunk| l.index) == Some(c.index) {
            continue;
        }
        used += len;
        out.push(c.clone());
    }
    out
}

/// One unit of work for a map/reduce pass over a document too large to read at
/// once: a group of chunks that fits one model call.
#[derive(Clone, Debug, PartialEq)]
pub struct Batch {
    pub chunks: Vec<Chunk>,
    pub chars: usize,
}

/// Groups every chunk into passes that each fit `budget_chars`.
///
/// Used for a whole-document task where sampling is not good enough - "compare
/// these two contracts in full", "find every mention of X". Each batch is
/// summarised on its own (map), then the summaries are combined (reduce). This
/// costs one model call per batch, which is why the router only reaches for it
/// when the document genuinely does not fit.
pub fn batches(chunks: &[Chunk], budget_chars: usize) -> Vec<Batch> {
    let mut out: Vec<Batch> = Vec::new();
    let mut current: Vec<Chunk> = Vec::new();
    let mut chars = 0usize;
    for c in chunks {
        let len = c.text.chars().count();
        if chars + len > budget_chars && !current.is_empty() {
            out.push(Batch {
                chunks: std::mem::take(&mut current),
                chars,
            });
            chars = 0;
        }
        chars += len;
        current.push(c.clone());
    }
    if !current.is_empty() {
        out.push(Batch {
            chunks: current,
            chars,
        });
    }
    out
}

/// Whether a document needs the map/reduce route at all.
pub fn needs_map_reduce(chunks: &[Chunk], budget_chars: usize) -> bool {
    chunks.iter().map(|c| c.text.chars().count()).sum::<usize>() > budget_chars
}

/// Renders selected chunks as evidence, with their provenance.
///
/// Page markers are included because they are what makes an answer checkable:
/// "auf Seite 4" can be verified by the user, "im Dokument" cannot.
pub fn render(chunks: &[Chunk]) -> String {
    let mut out = String::new();
    for c in chunks {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        match c.page {
            Some(p) => out.push_str(&format!("[Abschnitt {} · Seite {p}]\n", c.index + 1)),
            None => out.push_str(&format!("[Abschnitt {}]\n", c.index + 1)),
        }
        out.push_str(&c.text);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document() -> String {
        // A document whose answer is deliberately in the MIDDLE, so a pipeline
        // that truncates from the front cannot pass.
        let filler = "Allgemeine Bestimmungen und weitere Hinweise zum Vertrag. ".repeat(40);
        format!(
            "{filler}\n\n{}\n\n{filler}",
            "Die Kündigungsfrist beträgt drei Monate zum Quartalsende."
        )
    }

    #[test]
    fn segmentation_covers_everything_without_losing_text() {
        let text = document();
        let chunks = segment(&text);
        assert!(chunks.len() > 1, "a long document must be split");
        // Nothing vanished: the key sentence is in exactly one chunk.
        let hits = chunks
            .iter()
            .filter(|c| c.text.contains("Kündigungsfrist"))
            .count();
        assert!(hits >= 1, "the key sentence was lost");
        // Indices are contiguous.
        for (i, c) in chunks.iter().enumerate() {
            assert_eq!(c.index, i);
        }
    }

    #[test]
    fn the_answer_in_the_middle_is_the_one_selected() {
        let chunks = segment(&document());
        let picked = select_for_question(&chunks, "Wie lang ist die Kündigungsfrist?", 2_000);
        let joined = render(&picked);
        assert!(
            joined.contains("Kündigungsfrist beträgt drei Monate"),
            "retrieval missed the answer:\n{joined}"
        );
        assert!(joined.chars().count() <= 2_400, "budget overrun");
    }

    #[test]
    fn a_common_word_does_not_dominate_the_ranking() {
        // "Vertrag" is in every chunk; "Kündigungsfrist" in one. The rare term
        // must win, which is exactly what idf is for.
        let chunks = segment(&document());
        let s = score(&chunks, "Vertrag Kündigungsfrist");
        let best = s
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        assert!(
            chunks[best].text.contains("Kündigungsfrist"),
            "the common word won"
        );
    }

    #[test]
    fn a_summary_gets_the_whole_document_not_just_the_start() {
        // Distinct markers throughout, so coverage is measurable.
        let mut text = String::new();
        for i in 0..60 {
            text.push_str(&format!(
                "Abschnitt MARKER{i}. {}\n\n",
                "Inhalt ".repeat(40)
            ));
        }
        let chunks = segment(&text);
        let picked = spread(&chunks, 4_000);
        let joined = render(&picked);
        assert!(joined.chars().count() <= 4_500);
        // It must reach the end of the document, not only the beginning.
        let first_half = (0..10)
            .filter(|i| joined.contains(&format!("MARKER{i}")))
            .count();
        let last_half = (50..60)
            .filter(|i| joined.contains(&format!("MARKER{i}")))
            .count();
        assert!(first_half > 0, "the start is missing");
        assert!(
            last_half > 0,
            "the end is missing - this is the truncation bug"
        );
    }

    #[test]
    fn page_numbers_survive_into_the_evidence() {
        let text = "Seite eins Inhalt.\u{000c}Seite zwei Inhalt mit Zahl 42.\u{000c}Seite drei.";
        let chunks = segment(text);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[1].page, Some(2));
        let picked = select_for_question(&chunks, "Welche Zahl steht dort?", 1_000);
        let rendered = render(&picked);
        assert!(
            rendered.contains("Seite 2"),
            "page provenance lost:\n{rendered}"
        );
    }

    #[test]
    fn a_question_matching_nothing_still_returns_readable_context() {
        let chunks = segment(&document());
        // Nothing in the document is about this.
        let picked = select_for_question(&chunks, "Quantenphysik Zebrastreifen", 1_500);
        assert!(
            !picked.is_empty(),
            "an unmatched question must not return nothing"
        );
        assert!(render(&picked).chars().count() <= 1_900);
    }

    #[test]
    fn selected_passages_stay_in_document_order() {
        let mut text = String::new();
        for i in 0..30 {
            text.push_str(&format!(
                "Teil {i} mit Stichwort{i}. {}\n\n",
                "Text ".repeat(60)
            ));
        }
        let chunks = segment(&text);
        // Ask for two things, the later one first.
        let picked = select_for_question(&chunks, "Stichwort20 Stichwort3", 6_000);
        let indices: Vec<usize> = picked.iter().map(|c| c.index).collect();
        let mut sorted = indices.clone();
        sorted.sort_unstable();
        assert_eq!(indices, sorted, "passages were handed over out of order");
    }

    #[test]
    fn map_reduce_batches_cover_every_chunk_exactly_once() {
        let chunks = segment(&document());
        assert!(needs_map_reduce(&chunks, 500));
        let batches = batches(&chunks, 2_000);
        assert!(batches.len() > 1);
        let covered: Vec<usize> = batches
            .iter()
            .flat_map(|b| b.chunks.iter().map(|c| c.index))
            .collect();
        let mut expected: Vec<usize> = chunks.iter().map(|c| c.index).collect();
        expected.sort_unstable();
        let mut got = covered.clone();
        got.sort_unstable();
        assert_eq!(
            got, expected,
            "map/reduce must not drop or duplicate a chunk"
        );
        // A small document needs no map/reduce at all.
        assert!(!needs_map_reduce(&segment("Kurzer Text."), 10_000));
    }

    #[test]
    fn an_unpunctuated_wall_of_text_is_still_split() {
        let wall = "wort ".repeat(5_000);
        let chunks = segment(&wall);
        assert!(chunks.len() > 1, "a wall of text must be split");
        for c in &chunks {
            assert!(
                c.text.chars().count() <= TARGET_CHARS * 2,
                "chunk too large: {}",
                c.text.chars().count()
            );
        }
    }

    #[test]
    fn empty_and_tiny_inputs_are_handled() {
        assert!(segment("").is_empty());
        assert!(segment("   \n\n  ").is_empty());
        assert!(select_for_question(&[], "frage", 100).is_empty());
        assert!(spread(&[], 100).is_empty());
        let one = segment("Ein Satz.");
        assert_eq!(one.len(), 1);
        assert_eq!(spread(&one, 10_000).len(), 1);
    }
}
