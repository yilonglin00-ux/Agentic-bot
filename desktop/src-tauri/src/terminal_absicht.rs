//! Noki Terminal: what a message ASKS FOR (conversation vs. coding) and WHICH
//! project it targets - two separate questions. A bound project never turns a
//! chat into a build; a coding request never picks "the last project".

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Absicht {
    /// Talk, questions, explanations: answered in the terminal, nothing is built.
    Gespraech,
    /// Build something new ("Baue mir ein Snake-Spiel").
    NeuesProjekt,
    /// Change existing code - needs an explicit target project.
    Aendern,
    /// "Führe fort", "Mach weiter": continue the bound project where it stands.
    Fortsetzen,
}

/// "Führe fort.", "Weiter.", "Mach weiter", "Arbeite daran weiter",
/// "Führe das Projekt fort", "continue" - a request to go on, nothing more.
pub fn ist_fortsetzung(text: &str) -> bool {
    let w = woerter(text);
    if w.is_empty() || w.len() > 7 {
        return false;
    }
    // A concrete change ("… und verbessere die Karosserie") is a change.
    const NUR_WEITER: &[&str] = &["mach", "mache", "machen", "arbeite", "weiterarbeiten", "continue"];
    if w.iter().any(|y| (AENDERN.contains(&y.as_str()) && !NUR_WEITER.contains(&y.as_str())) || ERSTELLEN.contains(&y.as_str())) {
        return false;
    }
    let hat = |x: &str| w.iter().any(|y| y == x);
    (hat("fort") && (hat("führe") || hat("fuehre") || hat("setze") || hat("fahre")))
        || hat("fortsetzen")
        || hat("fortführen")
        || hat("weitermachen")
        || (hat("weiter") && w.iter().all(|y| ["weiter", "mach", "mache", "arbeite", "daran", "bitte", "einfach", "am", "projekt", "dem", "hier", "jetzt", "ok", "gerne", "geht", "es"].contains(&y.as_str())))
        || (w.len() <= 3 && (hat("continue") || (hat("go") && hat("on")) || (hat("keep") && hat("going"))))
}

fn woerter(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '-'))
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

// Verbs that ask to CREATE something (imperative, infinitive, English).
const ERSTELLEN: &[&str] = &[
    "baue", "bau", "bauen", "erstelle", "erstell", "erstellen", "programmiere", "programmier", "programmieren",
    "entwickle", "entwickeln", "implementiere", "implementieren", "schreibe", "schreib", "schreiben", "generiere",
    "generieren", "build", "create", "make", "write", "develop", "implement",
];
// Verbs that ask to CHANGE existing work.
const AENDERN: &[&str] = &[
    "ändere", "änder", "ändern", "aendere", "verbessere", "verbesser", "verbessern", "fixe", "fix", "fixen", "behebe",
    "beheben", "repariere", "reparieren", "korrigiere", "korrigieren", "füge", "fuege", "hinzufügen", "ergänze",
    "ergaenze", "ergänzen", "entferne", "entfernen", "passe", "anpassen", "refaktoriere", "optimiere", "optimieren",
    "mach", "mache", "machen", "arbeite", "weiterarbeiten", "setze", "ersetze", "verschiebe", "improve", "change",
    "update", "refactor", "add", "remove", "tweak", "continue",
];
// What code work is about (objects of a build/change request).
const CODE_OBJEKT: &[&str] = &[
    "spiel", "game", "app", "webseite", "website", "seite", "programm", "skript", "script", "funktion", "feature",
    "code", "bug", "fehler", "projekt", "project", "szene", "animation", "rechner", "tool", "html", "css",
    "javascript", "python", "api", "auto", "figur", "button", "menü", "ui", "oberfläche", "datei", "lenkung",
    "kamera", "karosserie", "türkonturen", "front", "heck", "räder", "farbe", "farben", "level", "snake",
];
// Openers of questions/explanations (read-only, conversation).
const FRAGE: &[&str] = &[
    "was", "wie", "warum", "wieso", "weshalb", "wer", "wo", "wann", "welche", "welcher", "welches", "erklär",
    "erkläre", "erklaere", "erklären", "zeig", "zeige", "beschreibe", "what", "how", "why", "explain", "who",
    "where", "when", "which", "describe", "hältst", "findest", "meinst",
];

/// Conversation unless the message really asks to build or change code.
pub fn absicht(text: &str, plan: &crate::task_plan::TaskPlan) -> Absicht {
    let w = woerter(text);
    if ist_fortsetzung(text) {
        return Absicht::Fortsetzen;
    }
    if w.is_empty() || plan.small_talk {
        return Absicht::Gespraech;
    }
    let hat = |liste: &[&str]| w.iter().any(|x| liste.contains(&x.as_str()));
    let objekt = hat(CODE_OBJEKT) || plan.coding;
    let erstellen = hat(ERSTELLEN);
    let aendern = hat(AENDERN);
    // "Erklär mir den Code", "Was macht dieses Projekt?": read, don't write -
    // unless the question itself asks for a build ("Kannst du mir ... bauen?").
    let frage_start = w.first().is_some_and(|x| FRAGE.contains(&x.as_str()));
    if frage_start && !(erstellen && objekt) {
        return Absicht::Gespraech;
    }
    if erstellen && objekt && !verweist_auf_projekt(text) {
        return Absicht::NeuesProjekt;
    }
    if (aendern || erstellen) && objekt {
        return Absicht::Aendern;
    }
    Absicht::Gespraech
}

/// "in diesem Projekt", "im aktuellen Projekt", "this project".
pub fn verweist_auf_projekt(text: &str) -> bool {
    let t = text.to_lowercase();
    ["dieses projekt", "diesem projekt", "dieses projekts", "aktuellen projekt", "aktuelle projekt", "im projekt", "this project", "current project", "bestehenden projekt", "bestehende projekt"]
        .iter()
        .any(|p| t.contains(p))
        || woerter(text).iter().any(|x| x.ends_with("-projekt") || x.ends_with("projekt") && x.len() > 7)
}

/// A project the message names explicitly ("im GT3-Projekt", the folder
/// name). Ok(None) = none named, Err = several equally good matches.
pub fn genanntes_projekt(text: &str, projekte: &[(PathBuf, String, String)]) -> Result<Option<PathBuf>, Vec<String>> {
    if !verweist_auf_projekt(text) && !projekte.iter().any(|(_, n, _)| text.contains(n.as_str())) {
        return Ok(None);
    }
    const LEER: &[&str] = &["projekt", "projekts", "project", "im", "in", "dem", "diesem", "dieses", "weiter", "arbeite", "und", "die", "der", "das", "den", "mit", "für", "bitte", "aktuellen", "bestehenden"];
    let suche: Vec<String> = woerter(text)
        .into_iter()
        .flat_map(|x| x.split('-').map(str::to_owned).collect::<Vec<_>>())
        .filter(|x| x.chars().count() >= 3 && !LEER.contains(&x.as_str()))
        .collect();
    let mut wertung: Vec<(usize, &PathBuf, &String)> = projekte
        .iter()
        .map(|(p, name, aufgabe)| {
            let heu = format!("{} {}", name.to_lowercase(), aufgabe.to_lowercase());
            let mut n = suche.iter().filter(|s| heu.contains(s.as_str())).count();
            if text.contains(name.as_str()) {
                n += 100;
            }
            (n, p, name)
        })
        .filter(|x| x.0 > 0)
        .collect();
    wertung.sort_by(|a, b| b.0.cmp(&a.0));
    match wertung.as_slice() {
        [] => Ok(None),
        [eins] => Ok(Some(eins.1.clone())),
        [a, b, ..] if a.0 > b.0 => Ok(Some(a.1.clone())),
        mehrere => Err(mehrere.iter().take_while(|x| x.0 == mehrere[0].0).map(|x| x.2.clone()).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(t: &str) -> Absicht {
        absicht(t, &crate::task_plan::understand(t, &[], false))
    }

    #[test]
    fn conversation_is_never_a_build() {
        for t in ["Hey", "hes", "Hallo", "Wie geht's?", "Was kannst du?", "Erklär mir, was ein Array ist.", "Was hältst du von Rust?",
                  "Warum funktioniert mein Code nicht?", "Erklär mir den Code", "Was macht dieses Projekt?", "danke", "ok cool"] {
            assert_eq!(a(t), Absicht::Gespraech, "{t}");
        }
    }

    #[test]
    fn build_and_change_requests() {
        assert_eq!(a("Baue mir ein kleines Snake-Spiel."), Absicht::NeuesProjekt);
        assert_eq!(a("Kannst du mir eine Webseite bauen?"), Absicht::NeuesProjekt);
        assert_eq!(a("Mach die Türkonturen besser."), Absicht::Aendern);
        assert_eq!(a("Verbessere in diesem Projekt die Türkonturen."), Absicht::Aendern);
        assert_eq!(a("Arbeite im GT3-Projekt weiter und verbessere die Front"), Absicht::Aendern);
        assert_eq!(a("Ändere die Lenkung im GT3-Projekt"), Absicht::Aendern);
    }

    #[test]
    fn continue_requests_are_their_own_intent() {
        for t in ["Führe fort.", "Weiter.", "Mach weiter", "Arbeite daran weiter.", "Führe das Projekt fort.", "Bitte weiter", "continue"] {
            assert_eq!(a(t), Absicht::Fortsetzen, "{t}");
        }
        // A concrete change stays a change; questions stay conversation.
        assert_eq!(a("Führe fort und verbessere die Karosserie."), Absicht::Aendern);
        assert_eq!(a("Was wurde zuletzt gemacht?"), Absicht::Gespraech);
        assert_eq!(a("Zeig mir die Dateien."), Absicht::Gespraech);
        assert_eq!(a("Welche Dateien hat dieses Projekt?"), Absicht::Gespraech);
    }

    #[test]
    fn named_project_is_found_or_ambiguous() {
        let p = |n: &str, a: &str| (PathBuf::from(format!("/x/{n}")), n.to_string(), a.to_string());
        let liste = vec![p("hochwertiges-interaktives-auto-funktional-1", "GT3 Auto Porsche"), p("snake-spiel-2", "Snake")];
        assert_eq!(genanntes_projekt("Arbeite im GT3-Projekt weiter", &liste).unwrap().unwrap(), PathBuf::from("/x/hochwertiges-interaktives-auto-funktional-1"));
        assert_eq!(genanntes_projekt("Mach die Türkonturen besser.", &liste).unwrap(), None);
        let zwei = vec![p("gt3-funktional", "GT3"), p("gt3-kreativ", "GT3")];
        assert!(genanntes_projekt("Arbeite im GT3-Projekt weiter", &zwei).is_err());
    }
}
