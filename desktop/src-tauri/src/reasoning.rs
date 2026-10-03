//! How hard Noki thinks about one request, and what that is allowed to cost.
//!
//! THE PROBLEM THIS SOLVES. Thinking used to follow a PROFILE: Work plus
//! `ChatProfile::Intensive` meant `enable_thinking: true` for every request in
//! that profile, whatever it was. That is the wrong axis. "Hallo" and "vergleiche
//! diese drei Verträge" are not the same task, and a profile cannot tell them
//! apart - only the request can. So the decision moves here, per request.
//!
//! THREE TIERS, NOT A DIAL. The user is never asked to pick one. A tier is
//! derived from what the request structurally is, and it carries its own budget:
//!
//!   FAST    no thinking channel at all. Greetings, app opens, navigation,
//!           a stable definition. These must stay as quick as they are today,
//!           which is why classification is pure string work - no model call
//!           may happen in order to decide whether to call the model.
//!   NORMAL  no thinking channel, a larger generation budget. Ordinary
//!           document work, combining a few facts, a structured answer.
//!   DEEP    the thinking channel, with a BOUNDED reasoning budget. Multi-step
//!           problems, hard comparisons, contradictory documents, real analysis.
//!
//! WHY NORMAL DOES NOT THINK. A reasoning pass on a 9B model costs seconds and
//! tokens before the first visible word. For "summarise this page" it buys
//! nothing a larger generation budget does not. Reserving the channel for DEEP
//! is what keeps the middle of the range usable.
//!
//! BUDGETS ARE CEILINGS, NOT TARGETS. Each tier fixes a context window, a
//! reasoning allowance, a generation allowance and a wall-clock timeout, so a
//! hard question cannot quietly spend a minute in a thinking loop.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningTier {
    Fast,
    #[default]
    Normal,
    Deep,
}

impl ReasoningTier {
    pub fn as_str(self) -> &'static str {
        match self {
            ReasoningTier::Fast => "FAST",
            ReasoningTier::Normal => "NORMAL",
            ReasoningTier::Deep => "DEEP",
        }
    }

    /// The only place that decides whether the thinking channel is opened.
    pub fn thinking(self) -> bool {
        self == ReasoningTier::Deep
    }

    pub fn budget(self) -> Budget {
        match self {
            // The timeouts are derived from a MEASURED generation rate of about
            // 10 tokens/second for Qwen3.5-9B-Q4 on this hardware, with head
            // room. A ceiling below `generation_tokens / rate` would abort
            // legitimate answers, which is worse than being slow - so the two
            // numbers are kept consistent on purpose.
            //
            // Small context, short answer, tight clock. A greeting or an app
            // open has no evidence to weigh. Measured: "Hallo" answers in 0.7s
            // with thinking off, against 6.7s with it on.
            ReasoningTier::Fast => Budget {
                context_tokens: 4_096,
                reasoning_tokens: 0,
                generation_tokens: 160,
                // One fast candidate gets a short, honest attempt budget.  A
                // second local candidate still fits inside the delivery
                // watchdog instead of one model consuming the whole request.
                timeout_ms: 18_000,
            },
            ReasoningTier::Normal => Budget {
                context_tokens: 8_192,
                reasoning_tokens: 0,
                generation_tokens: 900,
                timeout_ms: 110_000,
            },
            // Thinking and answering share one token budget in llama.cpp, and
            // the reasoning channel cannot be capped per request on this build,
            // so the total is what bounds it. 1_700 tokens is about 170s worst
            // case; the timeout allows for that plus the recovery pass that runs
            // when reasoning consumes everything.
            ReasoningTier::Deep => Budget {
                context_tokens: 8_192,
                reasoning_tokens: 700,
                generation_tokens: 1_000,
                timeout_ms: 210_000,
            },
        }
    }
}

/// What one request may spend. `reasoning_tokens` of 0 means the channel stays
/// shut - it is not "unlimited".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    pub context_tokens: u32,
    pub reasoning_tokens: u32,
    pub generation_tokens: u32,
    pub timeout_ms: u64,
}

/// What the router already knows by the time it asks for a tier. Everything
/// here is cheap to compute; nothing requires a model call or a file read.
#[derive(Clone, Copy, Debug, Default)]
pub struct Signals {
    /// Characters of document/evidence text that will go into the prompt.
    pub context_chars: usize,
    /// How many separate documents are in play.
    pub document_count: usize,
    /// Tools the plan expects to use.
    pub tool_count: usize,
    /// The plan has more than one dependent step.
    pub multi_step: bool,
    /// Keyword-derived depth from `evaluate_task_complexity`.
    pub reasoning_depth: f32,
    /// An explicit local action (open, play, navigate). Never deep.
    pub explicit_action: bool,
    /// Ordinary talk, settled by the intent classifier.
    pub conversation: bool,
    /// A deterministic tool already produced the numbers.
    pub has_computed_facts: bool,
}

/// Phrases that mean the user asked for real work, not a lookup. These are the
/// only textual reason to open the thinking channel.
const DEEP_PHRASES: &[&str] = &[
    "vergleiche",
    "vergleich",
    "analysiere",
    "analyse",
    "warum",
    "wieso",
    "weshalb",
    "begründe",
    "begruende",
    "abwägung",
    "abwaegung",
    "vor- und nachteile",
    "schlussfolgerung",
    "widerspruch",
    "widersprüch",
    "strategie",
    "konzept",
    "ursache",
    "auswertung",
    "schritt für schritt",
    "schritt fuer schritt",
    "leite ab",
    "bewerte",
    "beurteile",
    "priorisier",
    "optimier",
    "trends",
    "trend",
    "zusammenhang",
    "welche auffälligkeiten",
    "welche auffaelligkeiten",
    "ausreißer",
    "ausreisser",
    "strukturierten bericht",
    "bericht",
];

/// Phrases that mean the answer is short by nature. Kept separate from the
/// signals so a greeting cannot be pushed up a tier by a long attachment.
const FAST_PHRASES: &[&str] = &[
    "hallo",
    "hi ",
    "hey",
    "guten morgen",
    "guten tag",
    "guten abend",
    "danke",
    "tschüss",
    "tschuess",
    "öffne",
    "oeffne",
    "starte",
    "spiele",
    "mach auf",
    "zeig mir",
    "wie spät",
    "wie spaet",
    "uhrzeit",
    "datum",
    "was ist",
    "definiere",
    "wer ist",
];

/// The router's decision. Pure, cheap, and deliberately biased toward the
/// cheaper tier: a wrongly-DEEP request costs the user seconds of latency on
/// every single turn, while a wrongly-NORMAL one costs some depth on one answer.
pub fn classify(query: &str, s: Signals) -> ReasoningTier {
    // 1. Things that are never deep, whatever they contain.
    //
    // This runs FIRST so no amount of attached text can turn "öffne Spotify"
    // into a reasoning task. That is the FAST path the rest of Noki relies on.
    if s.explicit_action || s.conversation {
        return ReasoningTier::Fast;
    }
    let q = query.to_lowercase();
    let words = q.split_whitespace().count();

    // 2. A short greeting or a one-line lookup.
    let looks_fast = FAST_PHRASES
        .iter()
        .any(|p| q.starts_with(p) || q.contains(p));
    let asks_deeply = DEEP_PHRASES.iter().any(|p| q.contains(p));
    if looks_fast && !asks_deeply && words <= 8 && s.context_chars == 0 {
        return ReasoningTier::Fast;
    }

    // 3. Real reasons to think. Each of these is a task that a single pass
    //    demonstrably gets wrong: several documents to reconcile, a multi-step
    //    plan, or an explicit request to compare or justify.
    let deep = asks_deeply
        || s.multi_step && s.tool_count >= 2
        || s.document_count >= 3
        || s.reasoning_depth >= 0.8 && s.context_chars > 0
        // A large body of evidence needs structure before it needs words.
        || s.context_chars > 18_000;
    if deep {
        return ReasoningTier::Deep;
    }

    // 4. Everything with something to work from is NORMAL; a bare short
    //    question with nothing attached stays FAST.
    if s.context_chars == 0 && s.document_count == 0 && words <= 6 && !asks_deeply {
        return ReasoningTier::Fast;
    }
    ReasoningTier::Normal
}

/// Numbers that a deterministic tool already produced must not be re-derived by
/// the model, so a task whose figures are settled drops a tier: there is
/// nothing left to reason ABOUT except the interpretation.
pub fn after_computation(tier: ReasoningTier, s: Signals) -> ReasoningTier {
    if !s.has_computed_facts {
        return tier;
    }
    match tier {
        // Interpreting a finished statistics table is a NORMAL job.
        ReasoningTier::Deep if s.document_count <= 1 && !s.multi_step => ReasoningTier::Normal,
        other => other,
    }
}

/// Whether the internal quality check is worth running. It costs a pass, so it
/// is reserved for the tiers where an unchecked answer is actually risky.
pub fn wants_quality_check(tier: ReasoningTier) -> bool {
    tier != ReasoningTier::Fast
}

#[cfg(test)]
mod tests {
    use super::*;

    fn docs(n: usize, chars: usize) -> Signals {
        Signals {
            document_count: n,
            context_chars: chars,
            ..Default::default()
        }
    }

    #[test]
    fn a_greeting_never_thinks() {
        let t = classify(
            "Hallo",
            Signals {
                conversation: true,
                ..Default::default()
            },
        );
        assert_eq!(t, ReasoningTier::Fast);
        assert!(!t.thinking());
        assert_eq!(t.budget().reasoning_tokens, 0);
    }

    #[test]
    fn an_app_open_stays_fast_even_with_a_document_attached() {
        // The FAST path is the one regression that would be felt on every turn.
        let t = classify(
            "Öffne Spotify",
            Signals {
                explicit_action: true,
                context_chars: 40_000,
                document_count: 2,
                ..Default::default()
            },
        );
        assert_eq!(t, ReasoningTier::Fast);
        assert!(!t.thinking());
    }

    #[test]
    fn a_simple_knowledge_question_does_not_think() {
        assert_eq!(
            classify("Was ist Inflation?", Signals::default()),
            ReasoningTier::Fast
        );
        assert_eq!(
            classify("Wie spät ist es?", Signals::default()),
            ReasoningTier::Fast
        );
    }

    #[test]
    fn an_ordinary_summary_is_normal_not_deep() {
        let t = classify("Fasse diese PDF zusammen.", docs(1, 6_000));
        assert_eq!(t, ReasoningTier::Normal);
        assert!(!t.thinking(), "a summary must not pay for a reasoning pass");
        assert!(t.budget().generation_tokens > ReasoningTier::Fast.budget().generation_tokens);
    }

    #[test]
    fn an_explicit_comparison_thinks() {
        let t = classify("Vergleiche beide Dokumente.", docs(2, 9_000));
        assert_eq!(t, ReasoningTier::Deep);
        assert!(t.thinking());
        assert!(t.budget().reasoning_tokens > 0);
    }

    #[test]
    fn three_documents_think_even_without_a_keyword() {
        assert_eq!(
            classify(
                "Beantworte die Aufgabe anhand der Anhänge.",
                docs(3, 20_000)
            ),
            ReasoningTier::Deep
        );
    }

    #[test]
    fn a_csv_question_about_anomalies_thinks() {
        let t = classify(
            "Analysiere die Daten und nenne die drei wichtigsten Auffälligkeiten.",
            docs(1, 4_000),
        );
        assert_eq!(t, ReasoningTier::Deep);
    }

    #[test]
    fn a_multi_step_plan_with_tools_thinks() {
        let t = classify(
            "Erstelle daraus eine Übersicht und speichere sie",
            Signals {
                multi_step: true,
                tool_count: 2,
                context_chars: 3_000,
                ..Default::default()
            },
        );
        assert_eq!(t, ReasoningTier::Deep);
    }

    #[test]
    fn a_huge_body_of_evidence_thinks_without_a_keyword() {
        assert_eq!(
            classify("Was steht da drin?", docs(1, 40_000)),
            ReasoningTier::Deep
        );
    }

    #[test]
    fn budgets_are_bounded_and_ordered() {
        let (f, n, d) = (
            ReasoningTier::Fast.budget(),
            ReasoningTier::Normal.budget(),
            ReasoningTier::Deep.budget(),
        );
        assert!(f.timeout_ms < n.timeout_ms && n.timeout_ms < d.timeout_ms);
        assert!(f.generation_tokens < n.generation_tokens);
        assert_eq!(f.reasoning_tokens, 0);
        assert_eq!(n.reasoning_tokens, 0);
        assert!(d.reasoning_tokens > 0 && d.reasoning_tokens <= 1_200);
        // Nothing may be unbounded.
        for b in [f, n, d] {
            assert!(b.timeout_ms > 0 && b.generation_tokens > 0 && b.context_tokens > 0);
        }
        // A timeout shorter than the token budget needs would abort answers the
        // budget explicitly allows. Measured rate: ~10 tokens/second for
        // Qwen3.5-9B-Q4, so every tier must allow its own tokens to be spent.
        const MEASURED_TOKENS_PER_SEC: u64 = 10;
        for (tier, b) in [
            (ReasoningTier::Fast, f),
            (ReasoningTier::Normal, n),
            (ReasoningTier::Deep, d),
        ] {
            let needed_ms =
                (b.reasoning_tokens + b.generation_tokens) as u64 * 1_000 / MEASURED_TOKENS_PER_SEC;
            assert!(
                b.timeout_ms >= needed_ms,
                "{} allows {} tokens (~{}ms) but times out at {}ms",
                tier.as_str(),
                b.reasoning_tokens + b.generation_tokens,
                needed_ms,
                b.timeout_ms
            );
        }
    }

    #[test]
    fn settled_numbers_drop_a_tier_instead_of_being_re_derived() {
        let s = Signals {
            has_computed_facts: true,
            document_count: 1,
            context_chars: 3_000,
            ..Default::default()
        };
        assert_eq!(
            after_computation(ReasoningTier::Deep, s),
            ReasoningTier::Normal
        );
        // But a genuine multi-document task keeps its depth.
        let multi = Signals {
            has_computed_facts: true,
            document_count: 3,
            ..Default::default()
        };
        assert_eq!(
            after_computation(ReasoningTier::Deep, multi),
            ReasoningTier::Deep
        );
        // And without computed facts nothing changes.
        assert_eq!(
            after_computation(ReasoningTier::Deep, Signals::default()),
            ReasoningTier::Deep
        );
    }

    #[test]
    fn the_quality_check_is_skipped_for_fast_requests_only() {
        assert!(!wants_quality_check(ReasoningTier::Fast));
        assert!(wants_quality_check(ReasoningTier::Normal));
        assert!(wants_quality_check(ReasoningTier::Deep));
    }

    #[test]
    fn classification_costs_nothing_measurable() {
        // It runs on the request path of every single turn.
        let started = std::time::Instant::now();
        for _ in 0..2_000 {
            let _ = classify(
                "Vergleiche diese drei Dokumente und begründe",
                docs(3, 30_000),
            );
        }
        assert!(
            started.elapsed().as_millis() < 250,
            "took {:?}",
            started.elapsed()
        );
    }
}
