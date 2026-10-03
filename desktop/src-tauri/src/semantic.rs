//! Cheap, structured interpretation of spontaneous speech. No model rewrite is trusted as input.
use serde::Serialize;

#[derive(Clone, Debug, Default, Serialize)]
pub struct SemanticFrame {
    pub raw_text: String,
    pub normalized_meaning: String,
    pub primary_intent: String,
    pub entities: Vec<String>,
    pub quantities: Vec<Quantity>,
    pub frequencies: Vec<String>,
    pub time_relations: Vec<String>,
    pub conditions: Vec<String>,
    pub requested_outcome: Vec<String>,
    pub important_facts: Vec<String>,
    pub incidental_phrases: Vec<String>,
    /// "fünf, nee sieben" → "fünf → sieben"; the revoked value never reaches `quantities`.
    pub self_corrections: Vec<String>,
    pub ambiguities: Vec<String>,
    pub confidence: f32,
    /// Spontaneous speech signals (asides, corrections, repetition, many clauses). Clear short
    /// questions stay on the fast path; the parse itself is rule-based either way (microseconds).
    pub full_pass: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct Quantity {
    pub value: u32,
    pub unit: String,
    pub frequency: Option<String>,
}

fn number(w: &str) -> Option<u32> {
    match w {
        "ein" | "eine" | "einen" | "eins" => Some(1),
        "zwei" => Some(2),
        "drei" => Some(3),
        "vier" => Some(4),
        "fünf" | "fuenf" => Some(5),
        "sechs" => Some(6),
        "sieben" => Some(7),
        "acht" => Some(8),
        "neun" => Some(9),
        "zehn" => Some(10),
        "elf" => Some(11),
        "zwölf" | "zwoelf" => Some(12),
        _ => w.parse().ok(),
    }
}
/// "sieben", "7", "siebenmal".
fn count(w: &str) -> Option<u32> {
    number(w).or_else(|| w.strip_suffix("mal").and_then(number))
}
fn singular(w: &str) -> String {
    let l = w.to_lowercase();
    if matches!(l.as_str(), "rauchen" | "essen" | "trinken" | "schlafen" | "laufen" | "dampfen") {
        return w.to_owned();
    }
    if l.ends_with("en") && w.chars().count() > 5 && !l.ends_with("een") {
        w[..w.len() - 1].to_owned()
    } else {
        w.to_owned()
    }
}
/// Discourse phrases: noticeable, never the topic. Longest first.
const ASIDES: &[&str] = &[
    "das klingt vielleicht komplett verrückt",
    "klingt vielleicht komplett verrückt",
    "klingt komplett verrückt",
    "klingt verrückt",
    "ist ja komplett krank",
    "ist komplett krank",
    "komplett krank",
    "komplett verrückt",
    "keine ahnung",
    "stell dir vor",
    "stell dir mal vor",
    "sage ich mal",
    "sag ich mal",
    "oder so",
    "wie die hölle",
    "wie hölle",
    "aus der hölle",
    "ehrlich gesagt",
    "weißt du",
    "um ehrlich zu sein",
];
fn is_noise(w: &str) -> bool {
    matches!(
        w,
        "also"
            | "halt"
            | "eben"
            | "irgendwie"
            | "eigentlich"
            | "ähm"
            | "öhm"
            | "äh"
            | "hm"
            | "vielleicht"
            | "bitte"
            | "noki"
            | "ja"
            | "hey"
    )
}
const MARKERS: &[&str] = &[
    "nee",
    "nein",
    "warte",
    "beziehungsweise",
    "bzw",
    "korrigiere",
];
fn is_function(w: &str) -> bool {
    matches!(
        w,
        "ich"
            | "du"
            | "mir"
            | "dir"
            | "man"
            | "wenn"
            | "dann"
            | "und"
            | "oder"
            | "aber"
            | "es"
            | "das"
            | "die"
            | "der"
            | "den"
            | "dem"
            | "des"
            | "eine"
            | "einen"
            | "ein"
            | "am"
            | "an"
            | "im"
            | "in"
            | "pro"
            | "jeden"
            | "später"
            | "noch"
            | "nochmal"
            | "davon"
            | "ist"
            | "sind"
            | "wird"
            | "wäre"
            | "wie"
            | "was"
            | "warum"
            | "wieso"
            | "weshalb"
            | "wann"
            | "lange"
            | "viel"
            | "würde"
            | "würden"
            | "dass"
            | "so"
            | "zu"
            | "für"
            | "mit"
            | "bei"
            | "beim"
            | "mal"
            | "morgens"
            | "abends"
            | "mittags"
            | "täglich"
            | "langfristig"
            | "gesund"
            | "ungesund"
            | "schlecht"
            | "okay"
            | "mein"
            | "meine"
            | "sag"
            | "stell"
            | "soll"
            | "kann"
            | "gibt"
            | "gibts"
            | "nicht"
            | "mehr"
            | "bis"
            | "seit"
            | "sehr"
            | "viele"
            | "gerade"
            | "neues"
            | "neue"
            | "aktuell"
            | "wer"
            | "wo"
    )
}
fn is_directive_verb(w: &str) -> bool {
    matches!(
        w,
        "recherchiere"
            | "recherchieren"
            | "recherche"
            | "suche"
            | "suchen"
            | "finde"
            | "finden"
            | "schreibe"
            | "schreiben"
            | "verfasse"
            | "verfassen"
            | "erstelle"
            | "erstellen"
            | "erkläre"
            | "erklären"
            | "beschreibe"
            | "beschreiben"
    )
}
/// Capitalised nouns that are frame words, not topics.
fn is_frame_noun(w: &str) -> bool {
    matches!(
        w,
        "tag"
            | "tage"
            | "tagen"
            | "tages"
            | "zeit"
            | "preis"
            | "dauer"
            | "ahnung"
            | "update"
            | "woche"
            | "monat"
            | "jahr"
            | "idee"
            | "frage"
            | "internet"
            | "web"
            | "online"
            | "netz"
            | "text"
            | "texte"
            | "texten"
            | "aufsatz"
            | "aufsätze"
            | "aufsatze"
            | "artikel"
            | "bericht"
            | "berichte"
            | "zusammenfassung"
            | "zusammenfassungen"
            | "absatz"
            | "absätze"
            | "absatze"
            | "wort"
            | "worte"
            | "worten"
            | "wörter"
            | "wörtern"
            | "woerter"
            | "woertern"
            | "zeichen"
            | "seite"
            | "seiten"
            | "rechercheergebnis"
            | "rechercheergebnisse"
            | "suchanfrage"
    )
}
/// Containers/units point at the substance next to them ("sechs Tassen Kaffee").
fn is_unit(w: &str) -> bool {
    matches!(
        w,
        "tasse"
            | "tassen"
            | "glas"
            | "gläser"
            | "liter"
            | "stück"
            | "portion"
            | "portionen"
            | "dose"
            | "dosen"
            | "flasche"
            | "flaschen"
            | "gramm"
            | "g"
            | "mg"
            | "ml"
    )
}
const STORES: &[&str] = &[
    "kaufland",
    "rewe",
    "aldi",
    "lidl",
    "edeka",
    "netto",
    "penny",
    "amazon",
    "mediamarkt",
    "saturn",
    "dm",
    "rossmann",
];
fn push_unique(v: &mut Vec<String>, s: String) {
    if !s.is_empty() && !v.iter().any(|x| x.eq_ignore_ascii_case(&s)) {
        v.push(s);
    }
}

/// One spoken token: original casing, lowercase form, and whether punctuation ended it.
#[derive(Clone)]
struct Tok {
    orig: String,
    low: String,
    stop: bool,
}

pub fn understand(raw: &str, previous: Option<&SemanticFrame>) -> SemanticFrame {
    let low = raw.to_lowercase();
    let mut frame = SemanticFrame {
        raw_text: raw.to_owned(),
        confidence: 0.9,
        ..Default::default()
    };
    // 1) Asides out, case preserved. A removed phrase is a boundary for names.
    let mut clean = raw.to_owned();
    for p in ASIDES {
        while let Some(i) = clean.to_lowercase().find(p) {
            if clean.to_lowercase().len() != clean.len() {
                break;
            }
            push_unique(&mut frame.incidental_phrases, (*p).into());
            clean.replace_range(i..i + p.len(), " , ");
        }
    }
    let mut toks: Vec<Tok> = clean
        .split_whitespace()
        .filter_map(|w| {
            let orig = w.trim_matches(|c: char| !c.is_alphanumeric()).to_owned();
            if orig.is_empty() {
                return None;
            }
            Some(Tok {
                low: orig.to_lowercase(),
                stop: w.ends_with([',', '.', '?', '!', ';', ':']),
                orig,
            })
        })
        .collect();
    // Join written thousands groups before semantic quantity extraction
    // ("5 000 Wörter"). The word count is an output constraint, not a
    // quantity of the subject discussed in the question.
    let mut digit_i = 0;
    while digit_i + 1 < toks.len() {
        if toks[digit_i].low.chars().all(|c| c.is_ascii_digit())
            && toks[digit_i + 1].low.len() == 3
            && toks[digit_i + 1].low.chars().all(|c| c.is_ascii_digit())
        {
            let group = toks.remove(digit_i + 1);
            toks[digit_i].orig.push_str(&group.orig);
            toks[digit_i].low.push_str(&group.low);
            toks[digit_i].stop = group.stop;
        } else {
            digit_i += 1;
        }
    }
    let mut i = 0;
    while i < toks.len() {
        if is_noise(&toks[i].low) {
            let t = toks.remove(i);
            push_unique(&mut frame.incidental_phrases, t.low);
            if t.stop && i > 0 {
                toks[i - 1].stop = true;
            }
        } else if toks[i].low == "ich"
            && toks.get(i + 1).is_some_and(|t| t.low == "meine")
            && i > 0
            && toks[i - 1].stop
        {
            // "Wasser, ich meine Liter" → clarification aside; "fünf ich meine sieben" → correction.
            if toks.get(i + 2).is_some_and(|t| count(&t.low).is_some())
                && count(&toks[i - 1].low).is_some()
            {
                toks[i + 1].low = "nee".into();
                toks.remove(i);
            } else {
                toks.drain(i..=i + 1);
                push_unique(&mut frame.incidental_phrases, "ich meine".into());
            }
        } else if i > 0 && toks[i].low == toks[i - 1].low && !toks[i - 1].stop {
            toks.remove(i); // "Kaffee Kaffee" – spoken repetition
        } else {
            i += 1;
        }
    }
    // 2) Self-corrections: <count> <marker> <count> → the later value wins.
    let mut i = 1;
    while i + 1 < toks.len() {
        if MARKERS.contains(&toks[i].low.as_str())
            && count(&toks[i - 1].low).is_some()
            && count(&toks[i + 1].low).is_some()
        {
            frame
                .self_corrections
                .push(format!("{} → {}", toks[i - 1].low, toks[i + 1].low));
            push_unique(
                &mut frame.incidental_phrases,
                format!("{} {}", toks[i - 1].low, toks[i].low),
            );
            toks.drain(i - 1..=i);
        } else {
            i += 1;
        }
    }
    toks.retain(|t| !MARKERS.contains(&t.low.as_str()));
    let has = |xs: &[&str]| xs.iter().any(|x| low.contains(x));
    let word = |w: &str| toks.iter().any(|t| t.low == w);
    // 3) Time and condition relations.
    let daily = word("tag")
        || word("täglich")
        || word("tägl")
        || low.contains("jeden tag")
        || low.contains("am tag");
    if daily {
        frame.time_relations.push("per_day".into());
        frame.frequencies.push("per_day".into());
    }
    if has(&["langfristig", "irgendwann", "auf dauer", "auf lange sicht"]) {
        frame.time_relations.push("long_term".into());
    }
    if word("wenn")
        || frame
            .incidental_phrases
            .iter()
            .any(|p| p.starts_with("stell dir"))
    {
        frame.conditions.push("hypothetical_or_conditional".into());
    }
    // 4) Intent from meaning cues, independent of word order.
    let has_count = toks.iter().any(|t| count(&t.low).is_some());
    let explicit_health = has(&[
        "gesund",
        "gesundheit",
        "schädlich",
        "gefährlich für den körper",
    ]);
    let consumption = has(&[
        "esse",
        "essen",
        "trinke",
        "trinken",
        "verzehr",
        "ernähr",
        "nahrung",
        "laufe",
        "jogge",
        "rauche",
        "rauchen",
        "tabak",
        "nikotin",
        "zigarette",
        "zigaretten",
        "vape",
        "vaping",
        "schlafe",
        "trainiere",
    ]) || (daily && has_count)
        || (low.contains("davon") && previous.is_some_and(|f| f.primary_intent == "health_effect"));
    let evaluative = has(&[
        "schlecht",
        "schle",
        "ungesund",
        "schädlich",
        "schaedlich",
        "giftig",
        "gefährlich",
        "gefaehrlich",
        "risiko",
        "risiken",
        "folge",
        "folgen",
        "auswirkung",
        "auswirkungen",
        "zu viel",
        "okay",
        " ok",
        "geht das gut",
        "problematisch",
        "bedenklich",
    ]);
    let health = explicit_health || (consumption && evaluative);
    let heat = has(&["heiß", "heiss", "temperatur", "überhitz"]);
    let price = has(&[
        "kostet",
        "was kosten",
        "wie viel kosten",
        "wieviel kosten",
        "preis",
    ]);
    let availability = has(&[
        "nicht mehr da",
        "gibts nicht mehr",
        "gibt es nicht mehr",
        "verfüg",
        "sortiment",
        "ausverkauft",
        "nicht mehr im regal",
    ]) || (STORES.iter().any(|s| word(s))
        && has(&["warum", "wieso", "nicht mehr"]));
    frame.primary_intent = if health {
        "health_effect"
    } else if heat {
        "temperature_cause"
    } else if price {
        "price"
    } else if availability {
        "availability"
    } else if has(&["warum", "wieso", "weshalb"]) {
        "cause"
    } else if has(&["wie lange", "wann"]) {
        "time"
    } else {
        "fact"
    }
    .into();
    if health && (low.contains("preis") || low.contains('€')) {
        frame
            .ambiguities
            .push("Gesundheitswirkung oder Preis".into());
        frame.confidence = 0.4;
    }
    let out = &mut frame.requested_outcome;
    match frame.primary_intent.as_str() {
        "health_effect" => {
            out.push("health_risk".into());
            if has(&[
                "wie lange",
                "wie viel zeit",
                "wieviel zeit",
                "nach wie vielen tagen",
                "wann",
                "ab wann",
            ]) {
                out.push("duration".into());
            }
            if has(&["irgendwann", "langfristig", "auf dauer", "auf lange sicht"]) {
                out.push("long_term_effect".into());
            }
            if has(&["sofort", "akut", "direkt danach"]) {
                out.push("acute_effect".into());
            }
            if has(&["ab welcher menge", "wie viele", "zu viel"]) {
                out.push("amount_threshold".into());
            }
        }
        "temperature_cause" => out.push("cause_of_heat".into()),
        "price" => {
            out.push("price".into());
            if has(&[
                "gerade", "aktuell", "derzeit", "neues", "neue ", "jetzt", "momentan",
            ]) {
                out.push("current".into());
            }
        }
        "availability" => out.push("availability_reason".into()),
        "cause" => out.push("cause".into()),
        _ => {}
    }
    if frame.requested_outcome.iter().any(|x| x == "duration") {
        frame.time_relations.push("duration_requested".into());
    }
    if frame.requested_outcome.iter().any(|x| x == "acute_effect") {
        frame.time_relations.push("acute".into());
    }
    // 5) Entities: runs of name-like tokens ("Red Bull Purple", "MacBook Air", "Mac mini", "PlayStation 5").
    let name_like = |t: &Tok| {
        (t.orig.chars().next().is_some_and(char::is_uppercase)
            || t.orig.chars().skip(1).any(char::is_uppercase))
            && !is_function(&t.low)
            && !is_frame_noun(&t.low)
            && !is_directive_verb(&t.low)
            && count(&t.low).is_none()
            && !is_noise(&t.low)
    };
    let continuation = |t: &Tok| {
        t.orig.chars().all(|c| c.is_ascii_digit())
            || matches!(
                t.low.as_str(),
                "mini" | "pro" | "max" | "plus" | "air" | "ultra"
            )
    };
    let mut runs: Vec<(usize, Vec<String>)> = Vec::new();
    let mut j = 0;
    while j < toks.len() {
        if !name_like(&toks[j]) {
            j += 1;
            continue;
        }
        let start = j;
        let mut parts = vec![toks[j].orig.clone()];
        while !toks[j].stop
            && j + 1 < toks.len()
            && (name_like(&toks[j + 1]) || continuation(&toks[j + 1]))
            && !is_unit(&toks[j + 1].low)
            && !is_unit(&toks[j].low)
        {
            j += 1;
            parts.push(toks[j].orig.clone());
        }
        j += 1;
        // Stores are their own entity, wherever they sit in the run.
        let mut rest = Vec::new();
        for p in parts {
            if STORES.contains(&p.to_lowercase().as_str()) {
                if !rest.is_empty() {
                    runs.push((start, std::mem::take(&mut rest)));
                }
                runs.push((start, vec![p]));
            } else {
                rest.push(p);
            }
        }
        if !rest.is_empty() {
            runs.push((start, rest));
        }
    }
    let mut named: Vec<(usize, String)> = Vec::new();
    for (pos, parts) in runs {
        if parts.len() == 1 && is_unit(&parts[0].to_lowercase()) {
            continue;
        }
        let name = if parts.len() == 1 {
            singular(&parts[0])
        } else {
            parts.join(" ")
        };
        if !named.iter().any(|(_, n)| n.eq_ignore_ascii_case(&name)) {
            named.push((pos, name));
        }
    }
    // 6) Quantities: count + the thing counted; additive scenarios are summed, corrections already removed.
    let mut counts: Vec<(u32, bool)> = Vec::new(); // (value, firm)
    let mut counted: Option<String> = None;
    for (i, t) in toks.iter().enumerate() {
        let Some(n) = count(&t.low) else { continue };
        if toks
            .get(i + 1)
            .is_some_and(|next| is_frame_noun(&next.low))
        {
            continue;
        }
        if named
            .iter()
            .any(|(_, e)| e.split_whitespace().skip(1).any(|p| p == t.orig))
        {
            continue;
        }
        let article = matches!(t.low.as_str(), "ein" | "eine" | "einen");
        if article
            && toks[i.saturating_sub(4)..i]
                .iter()
                .any(|x| x.low.ends_with("mal") && count(&x.low).is_some())
        {
            continue;
        }
        let next_noun = toks
            .iter()
            .skip(i + 1)
            .take(4)
            .take_while(|x| count(&x.low).is_none())
            .find(|x| name_like(x) || is_unit(&x.low));
        let noun = match next_noun {
            Some(x) if is_unit(&x.low) => named
                .iter()
                .filter(|(p, _)| {
                    *p != toks
                        .iter()
                        .position(|y| std::ptr::eq(y, x))
                        .unwrap_or(usize::MAX)
                })
                .map(|(_, e)| e.clone())
                .next(),
            Some(x) => Some(singular(&x.orig)),
            None => named
                .iter()
                .rev()
                .find(|(p, _)| *p < i && i - *p <= 3)
                .map(|(_, e)| e.clone()),
        };
        if article && next_noun.is_some_and(|x| !name_like(x)) {
            continue;
        }
        if article && noun.is_none() {
            counts.push((n, false));
            continue;
        }
        if counted.is_none() {
            counted = noun;
        }
        counts.push((n, true));
    }
    let additive = low.contains("noch")
        || low.contains("später")
        || low.contains("plus")
        || low.contains("dazu");
    let firm: Vec<u32> = counts.iter().filter(|(_, f)| *f).map(|(n, _)| *n).collect();
    let value = if counts.len() > 1 && additive {
        Some(counts.iter().map(|(n, _)| n).sum())
    } else {
        firm.last().copied()
    };
    for (_, e) in &named {
        push_unique(&mut frame.entities, e.clone());
    }
    if let Some(c) = &counted {
        if !frame.entities.iter().any(|e| e.eq_ignore_ascii_case(c)) {
            frame.entities.insert(0, c.clone());
        }
    }
    // A count names the topic of a quantified question; other capitalised words are secondary.
    if let (Some(c), true) = (&counted, frame.primary_intent == "health_effect") {
        let c = c.clone();
        frame.entities.retain(|e| e.eq_ignore_ascii_case(&c));
    }
    if let Some(value) = value {
        frame.quantities.push(Quantity {
            value,
            unit: frame
                .entities
                .first()
                .cloned()
                .unwrap_or_else(|| "items".into()),
            frequency: daily.then(|| "per_day".into()),
        });
    }
    // 7) Elliptical follow-up inherits only the last turn's topic and intent.
    if toks.len() <= 28
        && frame.entities.is_empty()
        && (low.starts_with("und ")
            || low.contains("davon")
            || low.contains("nur ")
            || low.contains("drüber")
            || low.contains("darüber")
            || low.contains("dazu")
            || low.contains("daran")
            || low.contains("das")
            || low.contains("dies"))
    {
        if let Some(prev) = previous {
            frame.entities = prev.entities.clone();
            if frame.primary_intent == "fact" {
                frame.primary_intent = prev.primary_intent.clone();
                frame.requested_outcome = prev.requested_outcome.clone();
            }
            if frame.time_relations.is_empty() {
                frame.time_relations = prev.time_relations.clone();
            }
            if frame.frequencies.is_empty() {
                frame.frequencies = prev.frequencies.clone();
            }
            for q in &mut frame.quantities {
                if q.unit == "items" {
                    q.unit = frame
                        .entities
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "items".into());
                }
                if q.frequency.is_none()
                    && prev
                        .quantities
                        .iter()
                        .any(|p| p.frequency.as_deref() == Some("per_day"))
                {
                    q.frequency = Some("per_day".into());
                }
            }
            frame.conditions.push("follow_up".into());
        }
    }
    if frame.entities.len() > 4 {
        frame.entities.truncate(4);
    }
    frame.important_facts.extend(frame.entities.iter().cloned());
    for t in &toks {
        if t.low.contains("compil") || t.low.contains("kompil") {
            push_unique(&mut frame.important_facts, "compiling".into());
        }
    }
    frame
        .important_facts
        .extend(frame.quantities.iter().map(|q| {
            format!(
                "{} {}{}",
                q.value,
                q.unit,
                q.frequency
                    .as_deref()
                    .map(|x| format!("/{x}"))
                    .unwrap_or_default()
            )
        }));
    frame
        .important_facts
        .extend(frame.requested_outcome.iter().cloned());
    frame.normalized_meaning = if !frame.entities.is_empty() {
        format!(
            "{} {} {}",
            frame.entities.join(" "),
            frame
                .quantities
                .iter()
                .map(|q| format!("{} {}", q.value, q.frequency.as_deref().unwrap_or("")))
                .collect::<Vec<_>>()
                .join(" "),
            frame.requested_outcome.join(" ")
        )
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
    } else {
        toks.iter()
            .map(|t| t.low.clone())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let clauses = raw
        .split([',', '.', '?', '!', ';'])
        .filter(|c| c.trim().len() > 2)
        .count()
        + low.matches(" und ").count();
    frame.full_pass = raw.chars().count() > 90
        || !frame.incidental_phrases.is_empty()
        || !frame.self_corrections.is_empty()
        || clauses >= 3
        || raw
            .split_whitespace()
            .collect::<Vec<_>>()
            .windows(2)
            .any(|w| w[0].eq_ignore_ascii_case(w[1]));
    if frame.entities.is_empty() && frame.full_pass {
        frame.confidence = 0.65;
    }
    frame
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn noisy_language_twenty_cases() {
        let previous = understand("Ich esse sieben Bananen pro Tag. Ist das ungesund?", None);
        let cases: [(&str, &str, &str, Option<u32>, bool); 20] = [
            ("Stell dir vor ich esse siebenmal pro Tag eine Banane und wie viel Zeit würde es kosten dass es nicht gesund ist", "health_effect", "banane", Some(7), true),
            ("Wenn sieben Banane jeden Tag wann wird schlecht?", "health_effect", "banane", Some(7), true),
            ("Ich esse fünf nee sieben Bananen am Tag ist das irgendwann zu viel", "health_effect", "banane", Some(7), true),
            ("Morgens eine und später noch sechs davon, ist das langfristig okay?", "health_effect", "banane", Some(7), true),
            ("MacBook wenn ich compile wird wie Hölle heiß warum?", "temperature_cause", "macbook", None, false),
            ("Red Bull Purple Kaufland warum nicht mehr da irgendwie keine Ahnung", "availability", "red bull purple", None, true),
            ("Also wenn ich drei Bananen am Tag esse, ist das gesund?", "health_effect", "banane", Some(3), true),
            ("Banane sieben jeden Tag, irgendwann schlecht?", "health_effect", "banane", Some(7), true),
            ("Ähm siebenmal eine Banane am Tag, zu viel?", "health_effect", "banane", Some(7), true),
            ("Ich esse 5 nee 7 Bananen am Tag, gesund?", "health_effect", "banane", Some(7), true),
            ("Ich esse vier und später noch drei Bananen am Tag, okay?", "health_effect", "banane", Some(7), true),
            ("Und wenn es nur fünf sind?", "health_effect", "banane", Some(5), true),
            ("Ich esse sehr viele Bananen.", "fact", "banane", None, false),
            ("MacBook wird beim Kompilieren heiß, wieso?", "temperature_cause", "macbook", None, false),
            ("Kaufland Red Bull Purple nicht mehr verfügbar warum?", "availability", "red bull purple", None, false),
            ("Sage ich mal, zwei Bananen täglich, ist das schlecht?", "health_effect", "banane", Some(2), true),
            ("Fünf, warte sieben Bananen pro Tag gesund?", "health_effect", "banane", Some(7), true),
            ("Ich esse sieben Bananen pro Tag. Wie lange ist das okay?", "health_effect", "banane", Some(7), true),
            ("Red Bull Purple bei Kaufland, warum ausverkauft?", "availability", "red bull purple", None, false),
            ("Wie viel kosten drei Bananen?", "price", "banane", Some(3), false),
        ];
        for (i, (q, intent, entity, quantity, incidental)) in cases.iter().enumerate() {
            let f = understand(q, (i == 3 || i == 11).then_some(&previous));
            assert_eq!(&f.primary_intent, intent, "case {i}: {f:?}");
            assert!(
                f.entities.iter().any(|e| e.to_lowercase().contains(entity)),
                "case {i}: {f:?}"
            );
            assert_eq!(
                f.quantities.first().map(|x| x.value),
                *quantity,
                "case {i}: {f:?}"
            );
            if *incidental && [0, 5, 8, 15].contains(&i) {
                assert!(!f.incidental_phrases.is_empty(), "case {i}: {f:?}");
            }
        }
        assert_ne!(
            understand("Ist es okay, das MacBook heute zu verkaufen?", None).primary_intent,
            "health_effect"
        );
    }
    /// (text, intent, entity, quantity, per_day, must_be_important, must_be_incidental, forbidden_entity, corrected)
    type Fixture = (
        &'static str,
        &'static str,
        &'static str,
        Option<u32>,
        bool,
        &'static [&'static str],
        &'static [&'static str],
        &'static str,
        bool,
    );
    const NOISY: &[Fixture] = &[
        ("Wie lange sieben Bananen täglich bis ungesund?", "health_effect", "banane", Some(7), true, &["duration"], &[], "", false),
        ("Wenn siebenmal Banane am Tag wann nicht mehr gesund?", "health_effect", "banane", Some(7), true, &["duration"], &[], "", false),
        ("Ich esse jeden Tag sieben Banane wie lange bis das schlecht ist?", "health_effect", "banane", Some(7), true, &["duration"], &[], "", false),
        ("Stell dir vor ich esse siebenmal pro Tag eine Banane und wie viel Zeit würde es kosten dass es nicht gesund ist", "health_effect", "banane", Some(7), true, &["duration"], &["stell dir vor"], "", false),
        ("Ich esse fünf, nee sieben Bananen am Tag. Ist das irgendwann zu viel?", "health_effect", "banane", Some(7), true, &["long_term_effect"], &[], "", true),
        ("Morgens esse ich eine Banane und später am Tag noch sechs davon. Ist das langfristig okay?", "health_effect", "banane", Some(7), true, &["long_term_effect"], &[], "", false),
        ("Also keine Ahnung, stell dir vor, ich esse siebenmal am Tag eine Banane, das klingt vielleicht komplett verrückt, aber wann wäre das eigentlich ungesund?", "health_effect", "banane", Some(7), true, &["duration"], &["keine ahnung", "stell dir vor", "eigentlich"], "verrückt", false),
        ("MacBook wenn ich compile wird wie Hölle heiß warum?", "temperature_cause", "macbook", None, false, &["compiling"], &[], "hölle", false),
        ("Red Bull Purple Kaufland warum nicht mehr da irgendwie keine Ahnung", "availability", "red bull purple", None, false, &["Kaufland"], &["keine ahnung"], "ahnung", false),
        ("Der Preis ist ja komplett krank was kostet eigentlich neues MacBook Air gerade", "price", "macbook air", None, false, &["current"], &["eigentlich"], "krank", false),
        ("Ich trinke drei, ähm nein vier Kaffee am Tag, ist das auf Dauer schlecht?", "health_effect", "kaffee", Some(4), true, &["long_term_effect"], &[], "", true),
        ("Kaffee Kaffee sechs Tassen täglich zu viel oder?", "health_effect", "kaffee", Some(6), true, &[], &[], "", false),
        ("Zwei Eier morgens und abends nochmal zwei, jeden Tag, gesund?", "health_effect", "eier", Some(4), true, &[], &[], "", false),
        ("iPhone Akku warum so schnell leer irgendwie seit Update", "cause", "iphone", None, false, &[], &[], "irgendwie", false),
        ("Sag ich mal so, was kostet gerade eigentlich die PlayStation 5?", "price", "playstation 5", None, false, &["current"], &["sag ich mal"], "", false),
        ("Ich laufe acht, nein zehn Kilometer pro Tag, ist das auf Dauer zu viel?", "health_effect", "kilometer", Some(10), true, &["long_term_effect"], &[], "", true),
        ("Und wenn es nur fünf sind?", "health_effect", "banane", Some(5), true, &[], &[], "", false),
        ("Energy Drink zwei am Tag wie lange geht das gut?", "health_effect", "energy drink", Some(2), true, &["duration"], &[], "", false),
        ("Mein Mac mini wird beim Rendern heiß wie Hölle wieso", "temperature_cause", "mac mini", None, false, &[], &[], "hölle", false),
        ("Lidl Hafermilch warum gibts nicht mehr keine Ahnung", "availability", "hafermilch", None, false, &["Lidl"], &["keine ahnung"], "ahnung", false),
        ("Wie viel Wasser, ich meine Liter, soll man am Tag trinken?", "fact", "wasser", None, false, &[], &["ich meine"], "", false),
        ("Wasser wie viel Liter am Tag trinken normal?", "fact", "wasser", None, false, &[], &[], "", false),
    ];
    #[test]
    fn noisy_language_metrics() {
        let previous = understand("Ich esse sieben Bananen pro Tag. Ist das ungesund?", None);
        let (mut intent, mut entity, mut quantity, mut important, mut correction, mut failures) =
            (0, 0, 0, 0, 0, Vec::new());
        for (text, want_intent, want_entity, want_q, per_day, imp, inc, forbidden, corrected) in
            NOISY
        {
            let f = understand(text, text.starts_with("Und ").then_some(&previous));
            let ok_intent = f.primary_intent == *want_intent;
            let ok_entity = f
                .entities
                .iter()
                .any(|e| e.to_lowercase().contains(want_entity))
                && (forbidden.is_empty()
                    || !f
                        .entities
                        .iter()
                        .chain(&f.important_facts)
                        .any(|e| e.to_lowercase().contains(forbidden)));
            let ok_q = f.quantities.first().map(|q| q.value) == *want_q
                && f.quantities.len() <= 1
                && (!per_day
                    || f.quantities
                        .first()
                        .is_some_and(|q| q.frequency.as_deref() == Some("per_day")));
            let facts = f.important_facts.join(" | ").to_lowercase();
            let incidental = f.incidental_phrases.join(" | ");
            let ok_imp = imp.iter().all(|x| facts.contains(&x.to_lowercase()))
                && inc.iter().all(|x| incidental.contains(x))
                && !f
                    .incidental_phrases
                    .iter()
                    .any(|p| facts.split(" | ").any(|x| x == p));
            let ok_corr = f.self_corrections.is_empty() != *corrected;
            intent += ok_intent as usize;
            entity += ok_entity as usize;
            quantity += ok_q as usize;
            important += ok_imp as usize;
            correction += ok_corr as usize;
            if !(ok_intent && ok_entity && ok_q && ok_imp && ok_corr) {
                failures.push(format!("{text:?} i={ok_intent} e={ok_entity} q={ok_q} imp={ok_imp} c={ok_corr} -> {f:?}"));
            }
        }
        let n = NOISY.len();
        println!("noisy_metrics n={n} intent_accuracy={intent}/{n} entity_accuracy={entity}/{n} quantity_accuracy={quantity}/{n} important_vs_incidental={important}/{n} self_correction_accuracy={correction}/{n}");
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
    #[test]
    fn clear_questions_keep_fast_path() {
        for q in [
            "Was ist eine CPU?",
            "Wie spät ist es in Tokio?",
            "Was kostet ein MacBook Air?",
            "Wer ist der Bundeskanzler?",
        ] {
            let f = understand(q, None);
            assert!(!f.full_pass, "{q}: {f:?}");
            assert!(
                f.self_corrections.is_empty() && f.incidental_phrases.is_empty(),
                "{q}: {f:?}"
            );
        }
        assert!(understand("Stell dir vor ich esse siebenmal pro Tag eine Banane und wie viel Zeit würde es kosten dass es nicht gesund ist", None).full_pass);
    }
}
