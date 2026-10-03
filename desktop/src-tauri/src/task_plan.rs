//! Task understanding: ONE structured reading of a request before any route
//! is chosen.
//!
//! Ask Noki used to decide with scattered single-word tests ("contains
//! 'Text' -> document", "contains 'fass' -> summary", a short message ->
//! small talk). Each fix added another word. This layer replaces those
//! decisions with a plan built from the WHOLE request:
//!
//! * meaning cues are grouped into families (asking/checking, evidence,
//!   currency, external channel, file reference, creation, analysis). A
//!   single cue never decides; the combination does, and counter-evidence
//!   (a creative task, a plain explanation) lowers the score;
//! * the conversation is part of the input: a correction ("Nein, du sollst
//!   recherchieren", "mach es ausführlicher", "nur lokal", "nimm die Datei
//!   dazu") modifies the PREVIOUS task instead of being read as a new one;
//! * a request can need several things at once (file + web + long form).
//!
//! What stays uncertain is marked as such (`web_score` in the middle band)
//! and left to the existing model-based classification downstream.
//! This module only CLASSIFIES; it grants nothing. Web, cloud and file
//! access are still decided by the permission gate and the router.

use crate::intelligence::ChatMessage;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaskPlan {
    /// The request to execute. For a correction: the previous request with
    /// the user's modification applied (constraints such as "1000 Wörter"
    /// are carried over).
    pub effective_request: String,
    /// The message modified the previous turn instead of starting a new task.
    pub correction: bool,
    /// Current/external information is required (research).
    pub needs_web: bool,
    /// 0.0..1.0 - how strongly the whole request asks for external evidence.
    pub web_score: f32,
    /// The task is about a file / attachment the user has or means.
    pub needs_files: bool,
    /// The object is a folder's contents ("Welche Dateien liegen in meinem
    /// Ordner …?"), not one document to read.
    pub lists_folder: bool,
    /// A local action (open, play, ...) is requested.
    pub needs_action: bool,
    /// A long text is requested (explicit word count or long-form genre).
    pub needs_long_form: bool,
    pub requested_words: Option<usize>,
    /// Comparison, evaluation, argument - benefits from a stronger model.
    pub needs_reasoning: bool,
    /// Invented content (story, poem) - never a research task.
    pub creative: bool,
    /// The user asked to stay local for this task (no web, no cloud).
    pub local_only: bool,
    /// Short social message without any task.
    pub small_talk: bool,
    /// The user explicitly ruled research out ("nicht recherchieren").
    pub research_negated: bool,
    /// Write/build code (program, script, web page, 3D scene, function).
    pub coding: bool,
    /// Continues the running conversation without a topic of its own
    /// ("Gib mir ein Beispiel", "Und warum?", "Zeig mir mehr").
    pub follow_up: bool,
    /// Which cues fired - for the log, never shown to the user.
    pub signals: Vec<&'static str>,
}

impl TaskPlan {
    /// Anything beyond a social reply: never answered by the small-talk prompt.
    pub fn is_task(&self) -> bool {
        self.needs_web || self.needs_files || self.needs_action || self.needs_long_form
            || self.needs_reasoning || self.creative || self.correction || self.follow_up
            || self.coding
    }
    /// Worth the router's stronger (possibly cloud) models, not just the
    /// fast local draft.
    /// Writing work (long text, analysis): the routed writing path, not the
    /// short-answer/claim-check path. The router picks the model - with
    /// "nur lokal" it can only pick a local one (turn_local_only).
    pub fn wants_strong_model(&self) -> bool {
        self.needs_long_form || self.needs_reasoning
    }
    pub fn log_line(&self) -> String {
        format!(
            "web={} ({:.2}) files={} action={} coding={} long={}{} reasoning={} creative={} local_only={} correction={} follow_up={} small_talk={} signals={:?}",
            self.needs_web, self.web_score, self.needs_files, self.needs_action, self.coding,
            self.needs_long_form,
            self.requested_words.map(|w| format!("/{w}w")).unwrap_or_default(),
            self.needs_reasoning, self.creative, self.local_only, self.correction,
            self.follow_up, self.small_talk, self.signals
        )
    }
}

pub(crate) fn words(q: &str) -> Vec<String> {
    q.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

fn any_word(w: &[String], list: &[&str]) -> bool {
    w.iter().any(|x| list.contains(&x.as_str()))
}
fn any_prefix(w: &[String], stems: &[&str]) -> bool {
    w.iter().any(|x| stems.iter().any(|s| x.starts_with(s)))
}
fn phrase(lower: &str, list: &[&str]) -> bool {
    list.iter().any(|p| lower.contains(p))
}

// --- negation scope ---------------------------------------------------------

const NEGATION: &[&str] = &["nicht", "kein", "keine", "keinen", "ohne", "nie", "niemals", "not", "no", "without", "dont", "don", "never"];
const RECHERCHE_STAEMME: &[&str] = &[
    "recherch", "research", "nachschau", "nachseh", "googl", "such", "prüf", "pruef",
    "überprüf", "ueberpruef", "verifizier", "nachgeschaut", "herausfind",
    "websuch", "webrecherch", "internet", "online", "web",
];

/// "Du musst dafür nicht recherchieren", "ohne Recherche", "keine Websuche":
/// the research request is NEGATED - a negation within the four words before
/// the research word. Measured failure: the negated sentence was read as a
/// research task and the 1200-word text went through web research.
fn research_negated(w: &[String]) -> bool {
    w.iter().enumerate().any(|(i, x)| {
        RECHERCHE_STAEMME.iter().any(|s| x.starts_with(s))
            && w[i.saturating_sub(4)..i].iter().any(|v| NEGATION.contains(&v.as_str()))
    })
}

// --- cue families -----------------------------------------------------------

/// Asking to find out / look up / check something outside own knowledge.
fn inquiry(w: &[String], lower: &str) -> bool {
    any_prefix(w, &[
        "recherch", "research", "nachschau", "nachseh", "nachgeschaut", "herausfind",
        "verifizier", "googl", "informier", "fact",
    ]) || phrase(lower, &[
        "schau nach", "schau mal nach", "schau im", "schau mal", "guck nach", "guck im",
        "sieh nach", "finde heraus", "find heraus", "finde raus", "find raus", "such nach",
        "suche nach", "such im", "suche im", "prüf im", "prüfe im", "pruef im", "pruefe im",
        "look up", "check online", "fact-check", "fact check", "ob das stimmt",
        "ob diese behauptung", "stimmt es, dass", "stimmt es dass", "ob es stimmt",
        "nachschauen", "nachsehen",
    ]) || (any_prefix(w, &["prüf", "pruef", "überprüf", "ueberpruef", "check", "schau", "guck", "nachseh", "kontrollier"])
        // "check / look WHETHER ..." - the structure, not a fixed phrase.
        && (any_word(w, &["ob", "whether", "if"]) || phrase(lower, &["stimmt", "behauptung", "zahlen", "angaben", "fakten"])))
        || (any_prefix(w, &["find", "such", "sammel", "nenn"])
            && any_prefix(w, &["quelle", "studie", "beleg", "source", "paper", "artikel"]))
}

/// Evidence / scientific / source vocabulary.
fn evidence(w: &[String]) -> bool {
    any_prefix(w, &[
        "quelle", "studie", "studien", "forschung", "wissenschaft", "evidenz", "beleg",
        "statistik", "datenlage", "metaanalyse", "leitlinie", "empfehlung", "source",
        "studies", "study", "evidence", "paper",
    ])
}

/// Current state of the world.
fn currency(w: &[String], lower: &str) -> bool {
    any_word(w, &[
        "aktuell", "aktuelle", "aktuellen", "aktueller", "aktuelles", "derzeit", "momentan",
        "zurzeit", "neueste", "neuesten", "neuste", "jüngste", "juengste", "heute", "heutige",
        "latest", "current", "currently", "recent", "today", "2025", "2026", "2027",
    ]) || phrase(lower, &["stand der", "forschungsstand", "gerade jetzt", "diese woche", "dieses jahr", "im moment"])
}

/// Explicit external channel.
fn channel(w: &[String], lower: &str) -> bool {
    any_word(w, &["internet", "web", "online", "google", "netz", "websuche", "webrecherche"])
        || phrase(lower, &["im netz"])
}

/// Invented content: the model writes, nothing is looked up.
fn creative(w: &[String]) -> bool {
    any_prefix(w, &[
        "geschichte", "märchen", "maerchen", "gedicht", "reim", "witz", "erfinde", "erfind",
        "fantasie", "kurzgeschichte", "songtext", "liedtext", "story", "poem", "fiktiv",
    ])
}

/// A file the user has or means. Only real file nouns - "Text", "das",
/// "hier" are not files (the former substring test read "umfasst" as the
/// verb "fass" and the topic word "Text" as a document).
pub fn file_reference(w: &[String]) -> bool {
    any_word(w, &[
        "dokument", "dokuments", "dokumente", "datei", "dateien", "pdf", "pdfs", "anhang",
        "anhänge", "anhaenge", "bild", "bilder", "foto", "fotos", "screenshot", "screenshots",
        "tabelle", "tabellen", "csv", "unterlage", "unterlagen", "file", "files", "docx",
        "xlsx", "pptx", "attachment", "anlage",
    ])
}

/// A question about the user's OWN files/folders ("Welche Dateien liegen in
/// meinem Ordner Dokumente/Noki?", "Was ist in ~/Downloads?"): local file
/// system, never the web. Measured: this went to web research.
pub fn own_files(q: &str, w: &[String]) -> bool {
    const ORT: &[&str] = &[
        "ordner", "ordners", "verzeichnis", "folder", "directory", "dateien", "datei", "dokumente",
        "downloads", "files", "unterordner",
    ];
    const BEZUG: &[&str] = &["mein", "meine", "meinem", "meinen", "meiner", "meines", "my", "dieser", "diesem", "dieses", "im", "in"];
    let pfad = q.split_whitespace().any(|t| {
        let t = t.trim_matches(|c: char| !c.is_alphanumeric() && c != '/' && c != '~' && c != '.' && c != '_' && c != '-');
        // "W/A/S/D", "an/aus", "links/rechts" are not paths. A relative path
        // names a file (extension) or has at least three name segments;
        // "Dokumente/Noki" counts through the folder words below.
        (t.starts_with('/') || t.starts_with("~/") || {
            let seg: Vec<&str> = t.split('/').filter(|x| !x.is_empty()).collect();
            seg.len() >= 2
                && seg.iter().all(|x| x.chars().count() >= 2)
                && (seg.len() >= 3 || seg.last().is_some_and(|x| x.contains('.') && !x.ends_with('.')))
        })
            && !t.contains("://")
    });
    let ort = |x: &str| ORT.contains(&x) || x.ends_with("ordner") || x.ends_with("verzeichnis") || x.ends_with("folder");
    pfad || w.iter().enumerate().any(|(i, x)| {
        ort(x) && w[i.saturating_sub(3)..(i + 4).min(w.len())].iter().any(|v| BEZUG.contains(&v.as_str()))
            && w.iter().any(|v| v.starts_with("mein") || v == "my" || v.starts_with("dies"))
    })
}

fn reads_or_analyzes(w: &[String]) -> bool {
    any_prefix(w, &[
        "lies", "lese", "analys", "zusammenfass", "summar", "vergleich", "auswert", "prüf",
        "pruef", "nutze", "verwende", "benutze", "inhalt", "überflieg", "ueberflieg", "erklär",
        "erklaer", "beschreib", "zeig", "read", "compare", "use",
    ]) || any_word(w, &["fass", "fasse", "steht"])
        // "Was steht in dieser Datei?" reads a file; "Was ist eine Datei?" asks a definition.
        || (any_word(w, &["was"]) && any_prefix(w, &["dies", "mein", "enthält", "enthaelt", "drin", "anhang"]))
}

fn reasoning(w: &[String]) -> bool {
    any_prefix(w, &[
        "vergleich", "bewert", "beurteil", "analys", "abwäg", "abwaeg", "argument", "vor-",
        "vorteil", "nachteil", "begründ", "begruend", "einschätz", "einschaetz", "kritisch",
        "strategie", "compare", "evaluat", "assess",
    ])
}

/// A code/build task: a code object plus a building verb, or "lauffähiger Code".
/// Measured failure: "Schreib mir Code für eine animierte Figur …" had no task
/// signal at all and landed in the desktop path ("keine Browserseite aktiv").
fn coding(w: &[String], lower: &str) -> bool {
    let objekt = any_word(w, &[
        "code", "quellcode", "programm", "script", "skript", "html", "css", "javascript", "js",
        "typescript", "python", "rust", "swift", "kotlin", "java", "sql", "webgl", "threejs",
        "three", "canvas", "shader", "funktion", "klasse", "komponente", "api", "regex",
        "algorithmus", "webseite", "website", "spiel", "game", "cli", "bash", "json",
    ]) || any_prefix(w, &["programmier", "quellcode", "komponent"]);
    let bauen = write_verb(w)
        || any_word(w, &["coden", "codiere", "codier"])
        || any_prefix(w, &["bau", "implementier", "entwickl", "programmier", "generier", "refaktor", "debug", "fix", "build", "implement", "create", "gib"]);
    (objekt && bauen)
        || phrase(lower, &["lauffähigen code", "lauffaehigen code", "code für", "code fuer", "code dafür", "code dafuer", "im code", "als code"])
        || interaktives_artefakt(w)
}

/// "Erstelle mir eine animierte Figur, die sich bewegt …": a creation verb, a
/// buildable artifact and a behaviour that only software has (animated,
/// interactive, rotatable, clickable). Measured failure: without the word
/// "Code" this landed in the desktop path ("Der Bildschirm ist aktiv").
/// Stories/poems are text artifacts and stay `creative`.
fn interaktives_artefakt(w: &[String]) -> bool {
    let erschaffen = any_prefix(w, &[
        "erstell", "bau", "entwickl", "gestalt", "programmier", "generier", "baue", "create",
        "build", "make", "design", "implementier",
    ]) || (any_word(w, &["mach", "mache"]) && any_word(w, &["mir", "uns"]));
    let artefakt = any_prefix(w, &[
        "figur", "charakter", "character", "avatar", "3d", "szene", "spiel", "game", "webseite",
        "website", "landingpage", "app", "tool", "werkzeug", "simulation", "visualisierung",
        "dashboard", "oberfläche", "oberflaeche", "animation", "roboter", "raum", "welt", "level",
        "rechner", "uhr", "diagramm", "editor",
    ]);
    let verhalten = any_prefix(w, &[
        "animiert", "animation", "beweg", "interaktiv", "klick", "dreh", "rotier", "steuer",
        "blickwinkel", "perspektiv", "sichtbar", "spielbar", "zoom", "reagier", "läuft", "laeuft",
        "lauffähig", "lauffaehig", "3d",
    ]);
    erschaffen && artefakt && verhalten
}

fn long_form_genre(w: &[String]) -> bool {
    any_prefix(w, &["aufsatz", "essay", "artikel", "bericht", "hausarbeit", "abhandlung", "ausführlich", "ausfuehrlich", "detailliert"])
}

fn write_verb(w: &[String]) -> bool {
    any_prefix(w, &["schreib", "verfass", "formulier", "erstell", "write", "draft"])
}

/// "1000 Wörter", "ca. 800 words" - the number directly before the unit.
pub fn requested_words(q: &str) -> Option<usize> {
    let w = words(q);
    for i in 1..w.len() {
        if any_prefix(&w[i..=i], &["wort", "wörter", "woerter", "word"]) {
            if let Ok(n) = w[i - 1].parse::<usize>() {
                if (20..=20_000).contains(&n) {
                    return Some(n);
                }
            }
        }
    }
    None
}

/// Does the request name WHERE an action happens - an app or a place
/// ("in Safari", "im Finder", "mit Spotify", "auf dem Desktop")? Checked on
/// the original text: an app name is written with a capital letter.
fn app_target(q: &str) -> bool {
    const ORTE: &[&str] = &[
        "safari", "chrome", "firefox", "browser", "finder", "spotify", "textedit", "notizen",
        "notes", "mail", "rechner", "terminal", "vscode", "code", "word", "pages", "excel",
        "numbers", "kalender", "calendar", "musik", "music", "youtube", "fenster", "tab",
        "desktop", "schreibtisch", "feld", "suchfeld", "eingabefeld", "dock",
    ];
    let toks: Vec<&str> = q.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).collect();
    toks.windows(2).any(|p| {
        let prae = p[0].to_lowercase();
        ["in", "im", "ins", "auf", "mit", "bei", "über", "ueber"].contains(&prae.as_str())
            && (ORTE.contains(&p[1].to_lowercase().as_str())
                || (p[1].chars().next().is_some_and(char::is_uppercase)
                    && !["Deutsch", "Englisch", "English", "German", "Sushi"].contains(&p[1])))
    }) || toks.first().is_some_and(|t| ORTE.contains(&t.to_lowercase().as_str()))
}

/// A real app/UI action. "Schreib mir einen Text", "suche Quellen", "lies
/// nach" are part of an ANSWER or RESEARCH task unless they name where to
/// act ("tippe das in TextEdit", "suche in Spotify"). Measured failure: the
/// speech act of "…schreibe mir einen Text mit 1000 Wörtern" was `Type`, and
/// the research request was offered as "„…“ auf diesem Mac suchen".
fn is_action(q: &str, w: &[String], answer_task: bool) -> bool {
    use crate::working_context::SpeechAct;
    let act = crate::working_context::speech_act(q);
    if !act.is_explicit_action() {
        return false;
    }
    let absorbierbar = matches!(act, SpeechAct::Type | SpeechAct::Find | SpeechAct::Search | SpeechAct::Read | SpeechAct::Show | SpeechAct::Select);
    !(absorbierbar && !app_target(q) && (answer_task || write_verb(w)))
}

fn small_talk_shape(w: &[String]) -> bool {
    w.len() <= 5
        && any_word(w, &[
            "hi", "hallo", "hey", "servus", "moin", "danke", "dankeschön", "tschüss", "tschuess",
            "geht", "gehts", "thanks", "hello", "ok", "okay", "super", "cool", "gut",
        ])
}

/// Does the request point at the user's OWN current screen ("mein Desktop",
/// "dieses Fenster", "was ist gerade offen", "auf meinem Bildschirm")? A
/// screen word alone is a topic ("Architekturen für einen Desktop-
/// Assistenten") - measured: that sentence went into the desktop-context path
/// and hung for 120 s instead of being answered.
pub fn refers_to_own_screen(q: &str) -> bool {
    let w = words(q);
    const SCREEN: &[&str] = &[
        "desktop", "bildschirm", "screen", "fenster", "window", "app", "apps", "programm",
        "programme", "tab", "tabs", "schreibtisch", "ablage", "timer", "fokus", "auswahl",
        "markiert", "markierte", "markierten", "offen", "offene", "offenen", "geöffnet", "geoeffnet",
    ];
    const BEZUG: &[&str] = &[
        "mein", "meine", "meinem", "meinen", "meiner", "meines", "my", "dieses", "diese",
        "dieser", "diesem", "this", "hier", "gerade", "jetzt", "aktuell", "aktuellen",
        "aktuelle", "current", "offen", "offene", "geöffnet", "geoeffnet", "sehe", "siehst",
    ];
    w.iter().enumerate().any(|(i, x)| {
        SCREEN.iter().any(|s| x == s || (x.starts_with(s) && s.len() >= 6))
            && w[i.saturating_sub(3)..(i + 3).min(w.len())].iter().any(|v| BEZUG.contains(&v.as_str()))
    })
}

// --- corrections ------------------------------------------------------------

/// Words of a short message that carry a topic of their own.
fn topic_words(w: &[String]) -> usize {
    const FUELL: &[&str] = &[
        "nein", "ja", "doch", "du", "sollst", "sollt", "soll", "sollen", "bitte", "dann", "das",
        "es", "dies", "dazu", "darüber", "darueber", "dafür", "dafuer", "mal", "einfach", "jetzt",
        "im", "internet", "web", "online", "nach", "noch", "aber", "ich", "will", "möchte",
        "moechte", "dass", "und", "wirklich", "richtig", "selbst", "google", "mach", "machs",
        "mache", "etwas", "viel", "bisschen", "nur", "lokal", "local", "nimm", "benutze",
        "nutze", "verwende", "die", "der", "den", "datei", "anhang", "pdf", "quellen", "aktuelle",
        "aktuellen", "informationen", "meinte", "meine", "gemeint", "öffne", "oeffne", "open",
        "please", "no", "you", "should", "it", "that", "use", "the", "file", "sources", "mit",
        "ohne", "cloud", "ausführlicher", "ausfuehrlicher", "länger", "laenger", "kürzer",
        "kuerzer", "genauer", "detaillierter", "schau", "guck", "finde", "heraus", "sondern",
        "stattdessen", "lieber", "eigentlich", "so", "nicht", "kein", "keine", "wieder",
    ];
    w.iter()
        .filter(|x| !FUELL.contains(&x.as_str()))
        .filter(|x| !x.starts_with("recherch") && !x.starts_with("such") && !x.starts_with("research")
            && !x.starts_with("prüf") && !x.starts_with("pruef"))
        .count()
}

/// Does this short message modify the previous task (rather than start one)?
fn correction_shape(w: &[String], lower: &str) -> bool {
    if w.is_empty() || w.len() > 14 || topic_words(w) > 1 {
        return false;
    }
    let opener = any_word(&w[..1], &["nein", "doch", "nicht", "no", "eigentlich", "lieber", "stattdessen"])
        || phrase(lower, &["ich meinte", "ich meine", "gemeint war", "i meant"]);
    let modifier = any_prefix(w, &[
        "ausführlich", "ausfuehrlich", "länger", "laenger", "kürzer", "kuerzer", "genauer",
        "detaillierter", "einfacher", "recherch", "research",
    ]) || phrase(lower, &[
        "nur lokal", "nur local", "ohne cloud", "ohne internet", "benutze dafür", "benutze dafuer",
        "nutze dafür", "nutze dafuer", "nimm die datei", "nimm den anhang", "mit quellen",
        "aktuelle quellen", "aktuelle informationen", "im internet", "öffne es", "oeffne es",
        "mach es", "mach das", "machs",
    ]);
    opener || modifier
}

fn last_user_request<'a>(history: &'a [ChatMessage], exclude: &str) -> Option<&'a str> {
    history
        .iter()
        .rev()
        .filter(|m| m.role == "user")
        .map(|m| m.text.trim())
        .find(|t| {
            let w = words(t);
            !t.is_empty() && t != &exclude.trim() && !correction_shape(&w, &t.to_lowercase())
        })
}

// --- understanding ----------------------------------------------------------

/// Plan for a single message without conversation context.
fn plan_of(q: &str, has_attachments: bool) -> TaskPlan {
    let lower = q.to_lowercase();
    let w = words(q);
    let mut p = TaskPlan { effective_request: q.trim().to_string(), ..Default::default() };

    let f_negiert = research_negated(&w);
    let f_inquiry = inquiry(&w, &lower) && !f_negiert;
    let f_evidence = evidence(&w);
    let f_currency = currency(&w, &lower);
    let f_channel = channel(&w, &lower) && !f_negiert;
    let f_creative = creative(&w);
    let f_file = file_reference(&w);
    if f_inquiry { p.signals.push("inquiry"); }
    if f_negiert { p.signals.push("no_research"); p.research_negated = true; }
    if f_evidence { p.signals.push("evidence"); }
    if f_currency { p.signals.push("currency"); }
    if f_channel { p.signals.push("channel"); }
    if f_creative { p.signals.push("creative"); }
    if f_file { p.signals.push("file"); }

    // Research: combinations, not single words.
    let mut s: f32 = 0.0;
    if f_inquiry { s += 0.6; }
    if f_evidence { s += 0.3; }
    if f_currency { s += 0.3; }
    if f_channel { s += 0.4; }
    if f_evidence && f_currency { s += 0.2; }
    if f_creative && !f_inquiry && !f_channel { s -= 0.6; }
    // "Was weiß man derzeit über X?" - knowledge question about the current state.
    if f_currency && any_prefix(&w, &["weiß", "weiss", "sagt", "sagen", "gibt", "steht", "know"]) { s += 0.3; }
    // The user explicitly ruled research out: no web, whatever else fired.
    if f_negiert { s = 0.0; }
    p.web_score = s.clamp(0.0, 1.0);
    p.needs_web = p.web_score >= 0.6;

    p.creative = f_creative && !p.needs_web;
    let f_eigene = own_files(q, &w);
    if f_eigene { p.signals.push("own_files"); }
    let sammlung = any_prefix(&w, &["ordner", "verzeichnis", "folder", "director", "unterordner"])
        || w.iter().any(|x| x.ends_with("ordner") || x.ends_with("verzeichnis"))
        || any_word(&w, &["dateien", "files"]);
    p.lists_folder = f_eigene && sammlung && !reads_or_analyzes(&w) && !has_attachments;
    if p.lists_folder { p.signals.push("folder_listing"); }
    p.needs_files = f_eigene
        || (f_file && reads_or_analyzes(&w)) || (has_attachments && (f_file || reads_or_analyzes(&w)
        || any_word(&w, &["dies", "diese", "dieser", "dieses", "hier", "anbei", "das"])));
    p.requested_words = requested_words(q);
    p.needs_long_form = p.requested_words.is_some_and(|n| n >= 300)
        || (long_form_genre(&w) && (write_verb(&w) || any_prefix(&w, &["ausführlich", "ausfuehrlich"])));
    p.needs_reasoning = reasoning(&w);
    p.coding = coding(&w, &lower);
    if p.coding { p.signals.push("coding"); }
    // Building code: "welche Dateien erstellt wurden" are the task's OUTPUT,
    // not files of the user to read. Only a named path/own folder or an
    // attachment makes it a file task too.
    if p.coding && !f_eigene && !has_attachments && p.needs_files {
        p.needs_files = false;
        p.signals.retain(|s| *s != "file");
        p.signals.push("files_are_output");
    }
    let answer_task = p.needs_web || p.needs_long_form || p.needs_files || p.needs_reasoning || f_creative || p.coding;
    p.needs_action = is_action(q, &w, answer_task);
    // Building something is the task itself ("Bau mir ein Spiel" is not
    // "play"), unless an app is named where to act.
    if p.coding && !app_target(q) {
        p.needs_action = false;
    }
    p.local_only = phrase(&lower, &["nur lokal", "nur local", "ohne cloud", "ohne internet", "offline"]);
    if p.local_only {
        p.needs_web = false;
    }
    p.small_talk = !p.is_task() && small_talk_shape(&w);
    p
}

/// The one reading of a request, with the conversation.
/// A short request that only makes sense against the conversation: no topic
/// of its own, just asking for more of the same (an example, a reason, more).
fn follow_up_shape(w: &[String]) -> bool {
    const ANSCHLUSS: &[&str] = &[
        "gib", "zeig", "zeige", "nenn", "nenne", "beispiel", "beispiele", "mehr", "weiter",
        "noch", "eins", "einen", "eine", "ein", "und", "warum", "wieso", "weshalb", "wie", "was",
        "genau", "bitte", "mir", "uns", "davon", "dazu", "konkret", "konkretes", "praktisch",
        "anderes", "anders", "another", "example", "more", "why", "how", "give", "me", "an",
        "a", "jetzt", "mal", "kannst", "du", "erkläre", "erklär", "erklaer", "zusammen",
    ];
    !w.is_empty() && w.len() <= 8 && w.iter().all(|x| ANSCHLUSS.contains(&x.as_str()) || topic_words(std::slice::from_ref(x)) == 0)
}

pub fn understand(message: &str, history: &[ChatMessage], has_attachments: bool) -> TaskPlan {
    let mut own = plan_of(message, has_attachments);
    let w = words(message);
    let lower = message.to_lowercase();
    if correction_shape(&w, &lower) {
        if let Some(prev) = last_user_request(history, message) {
            let mut p = plan_of(prev, has_attachments);
            p.correction = true;
            p.signals.push("correction");
            // Apply the modification on top of the previous task.
            if own.needs_web || inquiry(&w, &lower) || channel(&w, &lower) || own.web_score >= 0.3 {
                p.needs_web = true;
                p.web_score = p.web_score.max(0.8);
                p.local_only = false;
            }
            if file_reference(&w) {
                p.needs_files = true;
            }
            if own.local_only {
                p.local_only = true;
                p.needs_web = false;
            }
            if any_prefix(&w, &["ausführlich", "ausfuehrlich", "länger", "laenger", "detaillierter", "genauer"]) {
                p.needs_long_form = true;
            }
            if own.needs_action && !own.needs_web {
                p.needs_action = true;
            }
            p.creative = p.creative && !p.needs_web;
            // Downstream still reads text: a research correction becomes an
            // explicit research request that carries all original constraints.
            p.effective_request = if p.needs_web && !inquiry(&words(prev), &prev.to_lowercase()) {
                format!("Recherchiere: {prev}")
            } else {
                prev.to_string()
            };
            let hinweis = message.trim();
            if !hinweis.is_empty() {
                p.effective_request = format!("{}\n\n(Präzisierung des Nutzers: {hinweis})", p.effective_request);
            }
            p.small_talk = false;
            return p;
        }
    }
    // "Zeig mir noch ein Beispiel" has a Show act but no target app: inside a
    // conversation it asks for content, not for an app action.
    let nur_absorbierbare_aktion = own.needs_action && !app_target(message) && {
        use crate::working_context::SpeechAct;
        matches!(crate::working_context::speech_act(message),
            SpeechAct::Show | SpeechAct::Find | SpeechAct::Search | SpeechAct::Read | SpeechAct::Type | SpeechAct::Select)
    };
    if nur_absorbierbare_aktion && follow_up_shape(&w) && last_user_request(history, message).is_some() {
        own.needs_action = false;
    }
    if !own.is_task() && follow_up_shape(&w) && last_user_request(history, message).is_some() {
        own.follow_up = true;
        own.small_talk = false;
        own.signals.push("follow_up");
    }
    own
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(q: &str) -> TaskPlan { understand(q, &[], false) }
    fn research(q: &str) -> bool { plan(q).needs_web }
    fn h(prev: &str) -> Vec<ChatMessage> {
        vec![
            ChatMessage { role: "user".into(), text: prev.into() },
            ChatMessage { role: "assistant".into(), text: "…".into() },
        ]
    }

    #[test]
    fn research_by_meaning_not_by_one_word() {
        for q in [
            "Recherchiere, ob Rohfisch ungesund ist.",
            "Recherchiere, ob Rohfisch ungesund ist und beziehe dich dabei auf Sushi und schreibe einen Text, der 1000 Wörter umfasst.",
            "Kannst du nachschauen, was aktuelle Studien zu Sushi sagen?",
            "Wie ist der aktuelle Forschungsstand zu Mikroplastik?",
            "Prüf im Internet, ob diese Behauptung stimmt.",
            "Finde seriöse Quellen dafür.",
            "Was weiß man derzeit über Long Covid?",
            "Kannst du mal schauen, ob Sushi gesundheitlich problematisch sein kann?",
            "Was sagt die aktuelle Forschung über gesundheitliche Risiken von rohem Fisch?",
            "Fass mir zusammen, was die aktuelle Forschung über rohen Fisch sagt.",
        ] {
            assert!(research(q), "research expected: {q} -> {}", plan(q).log_line());
            assert!(!plan(q).needs_files, "no file: {q}");
        }
    }

    #[test]
    fn no_research_for_knowledge_creative_and_small_talk() {
        for q in [
            "Was ist Sushi?",
            "Erklär mir Photosynthese.",
            "Hi.",
            "Schreib mir eine Geschichte über einen Fisch.",
            "Schreib mir einen Text über Sushi.",
            "Wie geht's?",
        ] {
            let p = plan(q);
            assert!(!p.needs_web && !p.needs_files, "{q} -> {}", p.log_line());
        }
        assert!(plan("Hi.").small_talk);
        assert!(plan("Schreib mir eine Geschichte über einen Fisch.").creative);
    }

    #[test]
    fn files_hybrids_actions() {
        for q in ["Fass diese PDF zusammen.", "Analysiere den Anhang.", "Was steht in dieser Datei?"] {
            let p = plan(q);
            assert!(p.needs_files && !p.needs_web, "{q} -> {}", p.log_line());
        }
        for q in [
            "Vergleiche diese PDF mit dem aktuellen Forschungsstand.",
            "Nutze den Anhang, aber prüfe die Zahlen im Internet.",
        ] {
            let p = plan(q);
            assert!(p.needs_files && p.needs_web, "hybrid: {q} -> {}", p.log_line());
        }
        let p = plan("Recherchiere drei gute Quellen dazu, schreib daraus 1000 Wörter und speichere das Ergebnis.");
        assert!(p.needs_web && p.needs_long_form && p.requested_words == Some(1000), "{}", p.log_line());
        for q in ["Öffne Spotify.", "Öffne Safari und suche nach Daft Punk.", "Öffne Rechner.", "Suche in Spotify nach Daft Punk."] {
            assert!(plan(q).needs_action, "{q} -> {}", plan(q).log_line());
        }
        // A screen word as topic is not a reference to the own screen.
        assert!(!refers_to_own_screen("Vergleiche drei unterschiedliche Softwarearchitekturen für einen lokalen Desktop-Assistenten, nenne Vor- und Nachteile und gib mir eine ausführliche technische Empfehlung."));
        for q in ["Was ist gerade auf meinem Bildschirm?", "Fass dieses Fenster zusammen.", "Welche Apps sind offen?"] {
            assert!(refers_to_own_screen(q), "{q}");
        }
        // Questions about the own files are local file tasks, never web.
        for q in ["Welche Dateien liegen in meinem Ordner Dokumente/Noki?", "Was ist in ~/Downloads?", "Liste die Dateien in meinem Projektordner auf."] {
            let p = plan(q);
            assert!(p.needs_files && !p.needs_web, "{q} -> {}", p.log_line());
        }
        assert!(!plan("Was ist eine Datei?").needs_files);
        // Code / build tasks.
        for q in [
            "Schreib mir Code für eine schöne animierte Figur, die ähnlich aussieht wie Noki, aber mit anderen Farben und Details, die du passend findest. Die Figur steht in einem geschlossenen Raum mit einem Fenster, bewegt sich animiert und soll von allen Seiten gut sichtbar sein (Kamera drehbar). Gib mir einen Umsetzungsplan und lauffähigen Code.",
            "Schreib eine Python-Funktion, die Primzahlen findet.",
            "Bau mir eine kleine Webseite mit HTML und CSS.",
            "Erstelle mir eine schöne animierte Figur, die etwas an Noki erinnert, aber ein eigenständiges Design und eine andere Farbe hat. Sie soll sich in einem geschlossenen Raum mit einem Fenster befinden, sich bewegen können und aus verschiedenen Blickwinkeln sichtbar sein.",
            "Bau mir ein kleines Spiel, bei dem man mit der Maus Bälle fangen kann.",
        ] {
            let p = plan(q);
            assert!(p.coding && !p.needs_action && !p.needs_web, "code: {q} -> {}", p.log_line());
        }
        assert!(!plan("Was ist Code?").coding);
        assert!(!plan("Schreib mir eine Geschichte über einen Fisch.").coding);
        assert!(!plan("Erklär mir, wie sich ein Roboter bewegt.").coding);
        // A negated research request is not research.
        for q in [
            "Schreib mir einen gut strukturierten Text mit ungefähr 1200 Wörtern darüber, wie neuronale Netze grundsätzlich funktionieren. Du musst dafür nicht recherchieren.",
            "Erklär mir Git ohne Recherche.",
            "Keine Websuche bitte, erklär mir einfach, wie Photosynthese funktioniert.",
        ] {
            let p = plan(q);
            assert!(!p.needs_web && p.web_score == 0.0, "negated: {q} -> {}", p.log_line());
        }
        assert!(plan("Schreib mir einen gut strukturierten Text mit ungefähr 1200 Wörtern darüber, wie neuronale Netze grundsätzlich funktionieren. Du musst dafür nicht recherchieren.").needs_long_form);
        // Writing / researching is the answer, not an app action.
        for q in [
            "Recherchiere, ob roher Fisch gesundheitlich ungesund sein kann. Beziehe dich besonders auf Sushi, nutze aktuelle und seriöse Quellen und schreibe mir dazu einen gut strukturierten Text mit ungefähr 1000 Wörtern.",
            "Schreib mir einen strukturierten Text mit ungefähr 1200 Wörtern darüber, wie neuronale Netze grundsätzlich funktionieren. Du musst dafür nicht recherchieren.",
            "Was sagt die aktuelle Forschung zu gesundheitlichen Risiken von Sushi? Prüfe seriöse Quellen und fasse den Forschungsstand ausführlich zusammen.",
            "Schreib mir eine Geschichte über einen Fisch.",
            "Recherchiere aktuelle Informationen zu ScreenCaptureKit, fass die wichtigsten Punkte zusammen und speichere das Ergebnis als Textdatei auf dem Desktop.",
        ] {
            assert!(!plan(q).needs_action, "answer task, not an action: {q} -> {}", plan(q).log_line());
        }
    }

    #[test]
    fn corrections_modify_the_previous_task() {
        let prev = "Schreibe einen Text über rohen Fisch und Sushi mit 1000 Wörtern.";
        let p = understand("Nein, du sollst recherchieren.", &h(prev), false);
        assert!(p.correction && p.needs_web, "{}", p.log_line());
        assert!(p.effective_request.contains("1000 Wörtern") && p.effective_request.starts_with("Recherchiere"), "{}", p.effective_request);
        assert_eq!(p.requested_words, Some(1000));

        let p = understand("Nein, recherchier das.", &h("Ist Rohfisch ungesund?"), false);
        assert!(p.correction && p.needs_web);

        let p = understand("Benutze dafür aktuelle Quellen.", &h("Erklär mir die Risiken von Sushi."), false);
        assert!(p.correction && p.needs_web, "{}", p.log_line());

        let p = understand("Mach es ausführlicher.", &h("Erklär mir Photosynthese."), false);
        assert!(p.correction && p.needs_long_form && !p.needs_web, "{}", p.log_line());

        let p = understand("Nein, ich meinte die Datei.", &h("Fasse das zusammen."), true);
        assert!(p.correction && p.needs_files, "{}", p.log_line());

        let p = understand("Nur lokal bitte.", &h("Recherchiere Sushi-Risiken."), false);
        assert!(p.correction && p.local_only && !p.needs_web, "{}", p.log_line());

        // Follow-ups without own topic continue the conversation.
        for q in ["Gib mir jetzt ein Beispiel.", "Und warum?", "Zeig mir noch ein anderes Beispiel."] {
            let p = understand(q, &h("Erklär mir Quantencomputer einfach."), false);
            assert!(p.follow_up && !p.needs_web, "{q} -> {}", p.log_line());
        }
        assert!(!understand("Gib mir ein Beispiel.", &[], false).follow_up, "no conversation, no follow-up");
        // A new request with its own topic is not a correction.
        assert!(!understand("Recherchiere Thunfisch-Quecksilber", &h(prev), false).correction);
        assert!(!understand("Hi", &h(prev), false).correction);
    }
}


#[cfg(test)]
mod benchmark_prompt_tests {
    use super::*;
    /// The Functional/Creative benchmark prompt (verbatim): a CODE task, not
    /// research/files/action ("Öffne die Vorschau" is part of building).
    #[test]
    fn gt3_benchmark_prompt_is_a_code_task() {
        let prompt = include_str!("../tests/fixtures/gt3_benchmark_prompt.txt");
        let p = understand(prompt, &[], false);
        assert!(p.coding, "{:?}", p.signals);
        assert!(!p.needs_web && !p.needs_files && !p.needs_action, "{:?}", p.signals);
    }
}

#[cfg(test)]
mod folder_tests {
    use super::*;

    #[test]
    fn folder_contents_are_a_listing_not_a_document() {
        let p = understand("Welche Dateien liegen in meinem Ordner Dokumente/Noki?", &[], false);
        assert!(p.needs_files && p.lists_folder, "{:?}", p.signals);
        let p = understand("Fass die Datei /tmp/bericht.txt zusammen.", &[], false);
        assert!(p.needs_files && !p.lists_folder);
        let p = understand("Fasse die Dateien in meinem Ordner Projekte zusammen.", &[], false);
        assert!(!p.lists_folder);
        let p = understand("Baue ein Auto, das man mit W/A/S/D steuert, in einem Raum mit Fenstern.", &[], false);
        assert!(!p.needs_files && p.coding, "{:?}", p.signals);
    }
}
