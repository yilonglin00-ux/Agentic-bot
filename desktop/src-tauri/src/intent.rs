//! What kind of message is this, before deciding whether any tool is needed.
//!
//! Work used to have no conversational route at all: everything that was not a
//! recognised action fell into the factual research pipeline, whose failure mode
//! is "Das weiß ich nicht sicher. Sag Recherchiere ...". So a greeting was
//! answered like a failed fact lookup.
//!
//! This layer only CLASSIFIES. It grants nothing: capability leases are still
//! issued from the action path alone, so better understanding never widens
//! access.
//!
//! Deliberately not a greeting list. The cheap signals here are structural and
//! language independent (length, punctuation, verb presence); anything the
//! structure cannot settle is handed to the local model as one short question.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum IntentFamily {
    /// Greeting, smalltalk, thanks, social reply - answer, do not search.
    Conversation,
    /// The user wants something known explained. Local knowledge first.
    Question,
    /// Current or external information is actually required.
    Research,
    /// A short reply that only makes sense against the previous turn.
    FollowUp,
    /// Structure could not settle it; ask the model, then decide.
    Undecided,
}

/// Stable, user-facing request taxonomy.  This is deliberately independent of
/// execution lanes: deciding that something is fresh knowledge does not grant
/// web access, and deciding that it is a document request does not grant an MCP
/// capability.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RequestIntent {
    Conversation,
    StableKnowledge,
    FreshKnowledge,
    WebResearch,
    Document,
    Coding,
    Action,
    Mixed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrchestrationDecision {
    pub intent: RequestIntent,
    /// A routing requirement, not a permission.  The permission gate still has
    /// the final say before a network request is made.
    pub need_web: bool,
    pub consider_mcp: bool,
}

fn contains_any(text: &str, terms: &[&str]) -> bool {
    terms.iter().any(|term| text.contains(term))
}

/// Cheap first-stage orchestration.  It uses combinations of speech act,
/// object and temporal signals instead of a single magic keyword.  Model
/// uncertainty is handled later by `research_after_uncertainty`.
pub fn orchestrate(text: &str, has_history: bool) -> OrchestrationDecision {
    let lower = text.trim().to_lowercase();
    let family = structural(text, has_history);
    let document = contains_any(
        &lower,
        &["datei", "dokument", "vertrag", "pdf", "anhang", "file", "document", "contract"],
    ) && contains_any(
        &lower,
        &["was steht", "lies", "lese", "fass", "analys", "inhalt", "summar", "read", "in my"],
    );
    let action = contains_any(
        &lower,
        &["öffne ", "oeffne ", "starte ", "spiele ", "suche in spotify", "open ", "launch ", "play "],
    );
    let coding = contains_any(
        &lower,
        &["code", "programmier", "funktion", "compiler", "typescript", "javascript", "python", "rust"],
    ) && contains_any(
        &lower,
        &["schreib", "implement", "fix", "debug", "fehler", "write", "create", "why"],
    );
    let explicit_web = contains_any(
        &lower,
        &[
            "recherchier",
            "recherche",
            "suche im internet",
            "such im internet",
            "finde heraus",
            "find heraus",
            "prüfe aktuell",
            "pruefe aktuell",
            "schau nach",
            "guck nach",
            "research",
            "look up",
            "im web",
            "online nach",
            "verify",
            "überprüf",
            "ueberpruef",
        ],
    );
    // Holders of offices and live conditions are volatile even when the user
    // omits words such as "aktuell".
    let volatile_role = contains_any(
        &lower,
        &["bundeskanzler", "präsident", "praesident", "prime minister", "president", "ceo von"],
    );
    let fresh = needs_current_info(text) || volatile_role;

    let intent = if explicit_web {
        RequestIntent::WebResearch
    } else if action && (document || coding) {
        RequestIntent::Mixed
    } else if action {
        RequestIntent::Action
    } else if document {
        RequestIntent::Document
    } else if coding {
        RequestIntent::Coding
    } else if fresh {
        RequestIntent::FreshKnowledge
    } else if matches!(family, IntentFamily::Conversation) {
        RequestIntent::Conversation
    } else {
        RequestIntent::StableKnowledge
    };
    OrchestrationDecision {
        intent,
        need_web: explicit_web || fresh,
        consider_mcp: document,
    }
}

/// Lightweight, deterministic, conservative input normalization for intent classification and routing.
/// Preserves code blocks, inline code, URLs, file paths, and proper names while repairing common typos.
pub fn normalize_query(input: &str) -> String {
    if input.trim().is_empty() || input.contains("```") {
        return input.to_string();
    }

    let mut words_out: Vec<String> = Vec::new();
    let tokens: Vec<&str> = input.split_whitespace().collect();
    let n = tokens.len();

    for i in 0..n {
        let raw = tokens[i];

        // Protect URLs, file paths, and inline code
        if raw.starts_with("http://")
            || raw.starts_with("https://")
            || raw.starts_with('/')
            || raw.starts_with("./")
            || raw.starts_with("~/")
            || raw.starts_with("C:\\")
            || (raw.starts_with('`') && raw.ends_with('`'))
        {
            words_out.push(raw.to_string());
            continue;
        }

        // Punctuation prefix/suffix separation
        // Byte offsets (not char counts): "„Wort“" has multi-byte quotes.
        let p_start = raw.char_indices().find(|(_, c)| c.is_alphanumeric()).map(|(i, _)| i).unwrap_or(raw.len());
        let p_end = raw.char_indices().rev().find(|(_, c)| c.is_alphanumeric()).map(|(i, c)| i + c.len_utf8()).unwrap_or(0);
        let (prefix, suffix, core) = if p_start < p_end {
            (&raw[..p_start], &raw[p_end..], &raw[p_start..p_end])
        } else {
            ("", "", raw)
        };

        let low = core.to_lowercase();
        let next_low = tokens.get(i + 1).map(|t| t.to_lowercase());
        let prev_low = if i > 0 { tokens.get(i - 1).map(|t| t.to_lowercase()) } else { None };

        let corrected: Option<&'static str> = match low.as_str() {
            "schle" => {
                let next_is_copula = next_low.as_deref().map_or(false, |next| {
                    let n_clean = next.trim_matches(|c: char| !c.is_alphanumeric());
                    matches!(n_clean, "ist" | "war" | "sind" | "sei" | "wäre" | "bleibt" | "wird")
                });
                let prev_is_eval = prev_low.as_deref().map_or(false, |prev| {
                    let p_clean = prev.trim_matches(|c: char| !c.is_alphanumeric());
                    matches!(p_clean, "so" | "zu" | "wie" | "warum" | "ziemlich" | "echt")
                });
                if next_is_copula || prev_is_eval || i + 1 == n {
                    Some("schlecht")
                } else {
                    None
                }
            }
            "rauchn" => Some("rauchen"),
            "gesunheit" | "gesundheitt" | "gesuntheit" => Some("gesundheit"),
            "rechechiere" | "rechechier" | "recharche" | "recharchier" => Some("recherchiere"),
            "wissn" => Some("wissen"),
            "woertern" => Some("wörtern"),
            "woerter" => Some("wörter"),
            "spotfy" => Some("spotify"),
            "labubu" => Some("Labubu"),
            _ => None,
        };

        if let Some(c) = corrected {
            let is_cap = core.chars().next().map_or(false, char::is_uppercase);
            let final_word = if is_cap && !c.chars().next().map_or(false, char::is_uppercase) {
                let mut s = c.to_string();
                if let Some(first) = s.get_mut(0..1) {
                    first.make_ascii_uppercase();
                }
                s
            } else {
                c.to_string()
            };
            words_out.push(format!("{}{}{}", prefix, final_word, suffix));
        } else {
            words_out.push(raw.to_string());
        }
    }

    words_out.join(" ")
}

/// A low-confidence/unknown answer is an internal recovery event.  For a
/// knowledge request it may add a web candidate; it never does so for talk,
/// local actions, code, or private document retrieval.
pub fn research_after_uncertainty(decision: OrchestrationDecision) -> bool {
    decision.need_web
        || matches!(
            decision.intent,
            RequestIntent::StableKnowledge | RequestIntent::FreshKnowledge | RequestIntent::WebResearch
        )
}

/// Languages we can answer in without guessing. Used only to tell the model
/// which language to reply in - never to decide what the user wants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    De,
    En,
    Fr,
    Other,
}

impl Lang {
    pub fn instruction(self) -> &'static str {
        match self {
            Lang::De => "Antworte auf Deutsch.",
            Lang::En => "Reply in English.",
            Lang::Fr => "Réponds en français.",
            Lang::Other => "Reply in the same language the user wrote in.",
        }
    }
}

/// Function-word scoring. These are grammar words, not topic words: they say
/// which language a sentence is in, and nothing about what it asks for.
pub fn detect_lang(text: &str) -> Lang {
    let t = format!(" {} ", text.to_lowercase());
    let score = |words: &[&str]| {
        words
            .iter()
            .filter(|w| t.contains(&format!(" {w} ")))
            .count()
    };
    let de = score(&[
        "ich", "du", "ist", "nicht", "und", "der", "die", "das", "wie", "was", "mir", "mich",
        "bitte", "kannst", "geht", "ein", "eine", "auf", "mit", "für", "wer", "warum",
    ]);
    let en = score(&[
        "i", "you", "is", "are", "the", "and", "what", "how", "please", "can", "do", "does", "me",
        "my", "a", "of", "to", "why", "who", "hey", "hi",
    ]);
    let fr = score(&[
        "je", "tu", "vous", "est", "et", "le", "la", "les", "que", "quoi", "comment", "pourquoi",
        "merci", "bonjour", "ça", "va", "un", "une", "des", "moi", "s'il",
    ]);
    // Accented letters that German does not use are a strong French signal.
    let fr_marks = text
        .chars()
        .filter(|c| "àâçéèêëîïôùûœ".contains(*c))
        .count();
    let fr = fr + fr_marks.min(3);
    if de == 0 && en == 0 && fr == 0 {
        return Lang::Other;
    }
    if fr > de && fr >= en {
        Lang::Fr
    } else if en > de {
        Lang::En
    } else if de > 0 {
        Lang::De
    } else {
        Lang::Other
    }
}

/// True when the message clearly asks for something outside Noki's knowledge:
/// today's events, prices, news. Only these justify going to the web on their
/// own; everything else needs the user to ask for research explicitly.
pub fn needs_current_info(text: &str) -> bool {
    let t = text.to_lowercase();
    const RECENCY: &[&str] = &[
        "heute",
        "gestern",
        "aktuell",
        "gerade",
        "momentan",
        "neueste",
        "letzte woche",
        "diese woche",
        "dieses jahr",
        "news",
        "nachrichten",
        "kurs",
        "preis",
        "wetter",
        "today",
        "yesterday",
        "current",
        "currently",
        "latest",
        "recent",
        "now",
        "price",
        "weather",
        "aujourd'hui",
        "actuel",
        "actuellement",
        "dernier",
        "météo",
        "prix",
    ];
    RECENCY.iter().any(|k| t.contains(k))
}

/// A first, free pass. Returns `Undecided` whenever structure is not enough -
/// that is the signal to spend one short model call, and nothing more.
pub fn structural(text: &str, has_history: bool) -> IntentFamily {
    let t = text.trim();
    if t.is_empty() {
        return IntentFamily::Conversation;
    }
    let lower = t.to_lowercase();
    let words: Vec<&str> = t.split_whitespace().collect();
    let n = words.len();

    // Interrogatives across the three supported languages. These are grammar,
    // not subject matter: they mark a question, they do not pick a route.
    const WH: &[&str] = &[
        "was", "wie", "wer", "warum", "wann", "wo", "welche", "welcher", "welches", "wieso",
        "what", "how", "who", "why", "when", "where", "which", "whose", "quoi", "comment", "qui",
        "pourquoi", "quand", "où", "quel", "quelle", "combien",
    ];
    let first = words
        .first()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .unwrap_or_default();
    let is_wh = WH.contains(&first.as_str());
    let has_q = t.ends_with('?');

    const IMPERATIVE_TASK: &[&str] = &[
        "analysiere", "analysier", "analyze", "analyse",
        "vergleiche", "vergleich", "compare",
        "untersuche", "untersuch", "examine", "evaluate",
        "fasse", "fass", "summarize", "summarise",
        "schreibe", "schreib", "write",
        "erstelle", "erstell", "create",
        "programmiere", "programmier", "implementiere", "implementier", "implement",
        "erkläre", "erklaer", "explain", "beschreibe", "beschreib", "describe",
        "berechne", "calculate", "compute",
    ];
    let second = if first == "bitte" || first == "please" || first == "svp" {
        words.get(1).map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase()).unwrap_or_default()
    } else {
        String::new()
    };
    if IMPERATIVE_TASK.contains(&first.as_str()) || (!second.is_empty() && IMPERATIVE_TASK.contains(&second.as_str())) {
        return IntentFamily::Question;
    }

    // A short message with no question and no interrogative is social talk.
    // Length is the signal, not the wording - "Hallo", "Hey there", "ça va"
    // and "danke dir" all land here without any of them being listed.
    if n <= 4 && !has_q && !is_wh {
        return IntentFamily::Conversation;
    }
    // "How are you" style: an interrogative, but about the addressee, not a
    // topic. Very short and mentioning the assistant or the speaker.
    if n <= 6 && (is_wh || has_q) {
        const SELF_REF: &[&str] = &[
            "dir", "dich", "du", "you", "your", "yourself", "tu", "vous", "toi", "ça", "ca",
        ];
        if SELF_REF.iter().any(|w| {
            lower
                .split_whitespace()
                .any(|x| x.trim_matches(|c: char| !c.is_alphanumeric()) == *w)
        }) {
            return IntentFamily::Conversation;
        }
    }
    // A short reply that points back continues the previous turn. An explicit
    // back-reference ("daraus", "davon", "that") outranks the interrogative:
    // "Was ist der wichtigste Punkt daraus?" is a follow-up, not a fresh topic.
    if has_history && n <= 8 {
        const BACKREF: &[&str] = &[
            "daraus", "davon", "dazu", "damit", "darüber", "das", "es", "dieser", "diese", "that",
            "it", "this", "those", "cela", "ça", "en",
        ];
        if BACKREF.iter().any(|w| {
            lower
                .split_whitespace()
                .any(|x| x.trim_matches(|c: char| !c.is_alphanumeric()) == *w)
        }) {
            return IntentFamily::FollowUp;
        }
    }
    if is_wh || has_q {
        return if needs_current_info(t) {
            IntentFamily::Research
        } else {
            IntentFamily::Question
        };
    }
    IntentFamily::Undecided
}

/// The one short prompt used when structure was not enough. Kept tiny on
/// purpose: classifying "Hallo" must never cost a reasoning pass.
pub fn classifier_prompt(text: &str) -> String {
    format!(
        "Classify the user's message into exactly one word:\n\
         CONVERSATION - greeting, smalltalk, thanks, social remark, opinion about you\n\
         QUESTION - asks for an explanation or knowledge\n\
         RESEARCH - needs current, external or news information\n\
         Answer with the single word only.\n\nMessage: {}\nAnswer:",
        text.chars().take(400).collect::<String>()
    )
}

/// Where the residual goes when neither structure nor the classifier decided.
///
/// It must not be the factual pipeline: that pipeline's only exit for an
/// unverifiable input is "Sag Recherchiere ...", which is exactly the catch-all
/// we are removing. A statement with no question mark and no interrogative is
/// ordinary talk, so it is answered as talk. Anything that DOES look like a
/// question keeps its old path, where the fallback is still appropriate.
pub fn resolve_residual(text: &str, family: IntentFamily) -> IntentFamily {
    if family != IntentFamily::Undecided {
        return family;
    }
    let t = text.trim();
    if t.ends_with('?') || needs_current_info(t) {
        return IntentFamily::Question;
    }
    let first = t
        .split_whitespace()
        .next()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .unwrap_or_default();
    const WH: &[&str] = &[
        "was", "wie", "wer", "warum", "wann", "wo", "welche", "welcher", "welches", "wieso",
        "what", "how", "who", "why", "when", "where", "which", "whose", "quoi", "comment", "qui",
        "pourquoi", "quand", "où", "quel", "quelle", "combien",
    ];
    if WH.contains(&first.as_str()) {
        IntentFamily::Question
    } else {
        IntentFamily::Conversation
    }
}

pub fn parse_family(raw: &str) -> IntentFamily {
    let t = raw.trim().to_uppercase();
    if t.starts_with("CONVERSATION") {
        IntentFamily::Conversation
    } else if t.starts_with("RESEARCH") {
        IntentFamily::Research
    } else if t.starts_with("QUESTION") {
        IntentFamily::Question
    } else {
        IntentFamily::Undecided
    }
}

/// System prompt for a normal reply. No fact-verification framing - a greeting
/// answered by a verifier looks like a failed lookup, which was the bug.
pub fn conversation_prompt(lang: Lang) -> String {
    format!(
        "Du bist Noki, ein lokaler Desktop-Begleiter. Dies ist normale Unterhaltung, \
         keine Recherche. Antworte kurz, freundlich und natürlich - ein bis zwei Sätze. \
         Biete keine Websuche an, frage nicht nach einer Datei oder einem Programm, \
         und erwähne keine Werkzeuge. {}",
        lang.instruction()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multibyte_punctuation_does_not_crash() {
        // „ “ « » are 2-3 bytes: offsets must be byte positions.
        let n = normalize_query("Im Noki Terminal ist das Projekt „gt3-auto“ geöffnet: «Hey» …");
        assert!(n.contains("gt3-auto") || n.contains("gt3"));
        let _ = normalize_query("„“ «» …");
    }

    #[test]
    fn greetings_are_conversation_in_every_language() {
        // None of these words appear in any list in this module.
        for s in [
            "Hallo",
            "Hi",
            "Servus",
            "Moin",
            "Hey",
            "Bonjour",
            "Salut",
            "Buongiorno",
            "Hola",
            "Guten Morgen",
            "Na du",
            "Yo",
        ] {
            assert_eq!(
                structural(s, false),
                IntentFamily::Conversation,
                "{s} should be conversation"
            );
        }
    }

    #[test]
    fn asking_how_the_assistant_is_doing_is_conversation() {
        for s in [
            "Wie geht's dir?",
            "Wie geht es dir?",
            "Hey, how are you?",
            "How are you doing?",
            "Bonjour, comment ça va ?",
            "ça va ?",
            // People type without accents; the pronoun still identifies it.
            "Bonjour, comment ca va ?",
        ] {
            assert_eq!(
                structural(s, false),
                IntentFamily::Conversation,
                "{s} should be conversation"
            );
        }
    }

    #[test]
    fn imperative_tasks_are_classified_as_questions() {
        for s in [
            "Analysiere ausführlich die Vor- und Nachteile von drei Geschäftsmodellen.",
            "Fasse diesen Vertrag zusammen.",
            "Schreibe eine Python-Funktion für Primzahlen.",
            "Bitte vergleiche Ansatz A und B.",
            "Erstelle einen Marketingplan.",
        ] {
            assert_eq!(structural(s, false), IntentFamily::Question, "{s}");
        }
    }

    #[test]
    fn a_knowledge_question_is_not_research() {
        for s in [
            "Was ist ein neuronales Netz?",
            "What is inflation?",
            "Comment fonctionne un moteur ?",
        ] {
            assert_eq!(structural(s, false), IntentFamily::Question, "{s}");
        }
    }

    #[test]
    fn current_information_is_research() {
        for s in [
            "Was ist heute bei Nvidia passiert?",
            "What is the current price of gold?",
            "Quelles sont les nouvelles d'aujourd'hui ?",
        ] {
            assert_eq!(structural(s, false), IntentFamily::Research, "{s}");
        }
    }

    #[test]
    fn a_short_backreference_is_a_follow_up() {
        assert_eq!(
            structural("Was ist der wichtigste Punkt daraus?", true),
            IntentFamily::FollowUp
        );
        assert_eq!(structural("Und das genauer?", true), IntentFamily::FollowUp);
        // Without a previous turn the same words are just a question.
        assert_ne!(
            structural("Was ist der wichtigste Punkt daraus?", false),
            IntentFamily::FollowUp
        );
    }

    #[test]
    fn language_is_detected_for_the_reply() {
        assert_eq!(detect_lang("Wie geht es dir heute?"), Lang::De);
        assert_eq!(detect_lang("Hey, how are you doing?"), Lang::En);
        assert_eq!(detect_lang("Bonjour, comment ça va ?"), Lang::Fr);
    }

    #[test]
    fn an_undecided_statement_is_talk_not_a_failed_lookup() {
        // The case seen live: a longer greeting the classifier did not settle.
        for s in [
            "Hallo Nummer 3Hallo Nummer 4",
            "Alles klar bei dir soweit",
            "Danke dir, das war hilfreich",
        ] {
            assert_eq!(
                resolve_residual(s, IntentFamily::Undecided),
                IntentFamily::Conversation,
                "{s}"
            );
        }
        // A real question keeps the knowledge path.
        assert_eq!(
            resolve_residual("Was ist Entropie", IntentFamily::Undecided),
            IntentFamily::Question
        );
        assert_eq!(
            resolve_residual("Erklär mir das genauer?", IntentFamily::Undecided),
            IntentFamily::Question
        );
        // A decided family is never overridden.
        assert_eq!(
            resolve_residual("irgendwas", IntentFamily::Research),
            IntentFamily::Research
        );
    }

    #[test]
    fn the_classifier_answer_is_read_leniently() {
        assert_eq!(parse_family("CONVERSATION"), IntentFamily::Conversation);
        assert_eq!(parse_family(" research \n"), IntentFamily::Research);
        assert_eq!(parse_family("Question."), IntentFamily::Question);
        assert_eq!(parse_family("banana"), IntentFamily::Undecided);
    }

    #[test]
    fn autonomous_orchestration_acceptance_matrix() {
        let hello = orchestrate("Hallo", false);
        assert_eq!(hello.intent, RequestIntent::Conversation);
        assert!(!hello.need_web && !hello.consider_mcp);

        for q in ["Was ist ein Hund?", "Was ist die Hauptstadt von Saarbrücken?"] {
            let d = orchestrate(q, false);
            assert_eq!(d.intent, RequestIntent::StableKnowledge, "{q}");
            assert!(!d.need_web && !d.consider_mcp, "{q}");
        }

        for q in ["Wer ist Bundeskanzler?", "Wie ist das Wetter morgen?", "Was geschah heute bei X?"] {
            let d = orchestrate(q, false);
            assert_eq!(d.intent, RequestIntent::FreshKnowledge, "{q}");
            assert!(d.need_web, "{q}");
        }

        let unknown = orchestrate("Was ist Flumbrax?", false);
        assert!(!unknown.need_web);
        assert!(research_after_uncertainty(unknown));

        let doc = orchestrate("Was steht in meiner Datei Vertrag.pdf?", false);
        assert_eq!(doc.intent, RequestIntent::Document);
        assert!(doc.consider_mcp && !doc.need_web);

        let basic = orchestrate("Was ist ein Hund?", false);
        assert!(!basic.consider_mcp);
    }
}
