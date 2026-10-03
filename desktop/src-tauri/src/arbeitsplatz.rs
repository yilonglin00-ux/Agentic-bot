//! Nokis reservierter Arbeits-Schreibtisch.
//!
//! Noki bekommt EINEN echten macOS-Schreibtisch als Arbeitsplatz. Nicht eine
//! Kulisse, nicht ein Vollbildfenster: einen Schreibtisch, zu dem der Nutzer
//! auch mit dem Trackpad wischen kann.
//!
//! Warum hier nur Logik und kein macOS-Aufruf steht: die Auswahl ist die
//! Stelle, an der man sich irren kann (einen belegten Schreibtisch kapern,
//! zwei Arbeitsplaetze anlegen, eine tote Kennung weiterbenutzen). Genau die
//! ist deshalb rein und pruefbar; die privaten CGS-Aufrufe liegen in lib.rs.
//!
//! Zwei Kennungen, zwei Lebensdauern:
//!   * `id`   — die fluechtige ManagedSpaceID. Gilt NUR in dieser Sitzung.
//!   * `uuid` — die stabile Kennung des Schreibtischs. Nur sie wird gemerkt.
//! Eine gemerkte `id` ohne Abgleich waere genau der Fehler, vor dem die
//! Vorgabe warnt: nach Neustart zeigt sie irgendwohin.

use serde::{Deserialize, Serialize};

/// Die Rolle, die Noki diesem Schreibtisch intern gibt. macOS nennt ihn
/// weiter "Schreibtisch 4" — das ist Darstellung, nicht Identitaet.
pub const ROLLE: &str = "NOKI_WORKSPACE";

/// Ein echter Schreibtisch: `typ` 0. `typ` 4 ist ein Vollbild-Space einer
/// App und gehoert dieser App, nie Noki.
pub const TYP_SCHREIBTISCH: i32 = 0;

/// A Desktop identity may be persisted only when it is independent from the
/// session-local ManagedSpaceID.  macOS sometimes omits the UUID of the
/// primary Desktop; the discovery layer gives that one the stable
/// `primary-desktop:<display>` identity.
pub fn persistente_identitaet(uuid: &str) -> bool {
    !uuid.is_empty() && !uuid.starts_with("managed-space:")
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Schreibtisch {
    pub id: u64,
    pub uuid: String,
    pub typ: i32,
    /// Fenster, die weder Noki gehoeren noch auf allen Schreibtischen
    /// mitschwimmen. Nur die zaehlen als "hier arbeitet jemand".
    pub fremde_fenster: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Reservierung {
    pub uuid: String,
    pub display: String,
    /// Fluechtig: bei jedem Start neu aufgeloest, nie blind uebernommen.
    pub id: u64,
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Lage {
    /// Reserviert und in der aktuellen Liste wiedergefunden.
    Bereit(Reservierung),
    /// Es gibt eine Reservierung, aber der Schreibtisch existiert nicht mehr.
    Verschwunden,
    /// Noch nichts reserviert.
    Offen,
}

/// Ein Schreibtisch ist frei, wenn kein fremdes Fenster darauf liegt.
///
/// Bewusst NICHT geprueft: Nokis eigene Fenster und Fenster, die auf allen
/// Schreibtischen mitschwimmen (Hilfsprogramme mit CanJoinAllSpaces). Die
/// liegen ueberall und wuerden sonst jeden Schreibtisch als belegt melden —
/// dann faende Noki nie einen freien. Das Aussortieren passiert beim Zaehlen
/// in lib.rs, hier zaehlt nur das Ergebnis.
pub fn ist_frei(s: &Schreibtisch) -> bool {
    s.typ == TYP_SCHREIBTISCH && persistente_identitaet(&s.uuid) && s.fremde_fenster == 0
}

/// Sichtbare Mission-Control-Nummer eines normalen Schreibtischs.
/// Die Nummer ist nur Darstellung und wird aus der aktuellen Reihenfolge
/// berechnet; Vollbild- und Spezial-Spaces zaehlen nicht mit.
pub fn sichtbare_nummer(liste: &[Schreibtisch], id: u64) -> Option<usize> {
    liste
        .iter()
        .filter(|s| s.typ == TYP_SCHREIBTISCH)
        .position(|s| s.id == id)
        .map(|i| i + 1)
}

/// Waehlt den Schreibtisch, den Noki beanspruchen darf.
///
/// Regeln, jede aus einer konkreten Gefahr:
///   * nur echte Schreibtische — ein Vollbild-Space gehoert seiner App;
///   * nur mit `uuid` — ohne sie liesse sich die Wahl nie wiederfinden
///     (der erste Schreibtisch hat gemessen keine bzw. managed-space:);
///   * nur wirklich leere — die Arbeit des Nutzers wird nie verdraengt;
///   * nie der gerade aktive — sonst naehme Noki dem Nutzer den Boden weg,
///     auf dem er in diesem Moment steht;
///   * der hinterste zuerst — er liegt am weitesten weg von dem, womit der
///     Nutzer gerade arbeitet.
pub fn waehle_freien(liste: &[Schreibtisch], aktiv: u64) -> Option<&Schreibtisch> {
    liste
        .iter()
        .filter(|s| ist_frei(s) && s.id != aktiv)
        .next_back()
}

/// Findet eine bestehende Reservierung in der AKTUELLEN Liste wieder — ueber
/// die `uuid`, nie ueber die gemerkte `id` und nie ueber die Position.
/// Ergebnis ist die frisch aufgeloeste Reservierung.
pub fn wiederfinden(res: &Reservierung, liste: &[Schreibtisch]) -> Lage {
    if !persistente_identitaet(&res.uuid) {
        return Lage::Offen;
    }
    match liste
        .iter()
        .find(|s| s.uuid == res.uuid && s.typ == TYP_SCHREIBTISCH)
    {
        Some(s) => Lage::Bereit(Reservierung {
            uuid: s.uuid.clone(),
            display: res.display.clone(),
            id: s.id, // frisch, nicht die gemerkte
            generation: res.generation,
        }),
        // Der Nutzer hat den Schreibtisch entfernt oder die Sitzung ist neu.
        // Die alte Kennung ist ab hier tot und wird nie wieder benutzt.
        None => Lage::Verschwunden,
    }
}

/// Der ganze Entscheidungsweg an einer Stelle: erst wiederfinden, dann — und
/// nur dann — neu waehlen. Ohne diese Reihenfolge entstuenden mit jeder
/// Aufgabe neue Arbeitsplaetze.
pub fn aufloesen(
    vorhanden: Option<&Reservierung>,
    liste: &[Schreibtisch],
    aktiv: u64,
    display: &str,
    generation: u64,
) -> (Lage, bool) {
    if let Some(r) = vorhanden {
        match wiederfinden(r, liste) {
            Lage::Bereit(neu) => return (Lage::Bereit(neu), false),
            Lage::Verschwunden => return (Lage::Verschwunden, false),
            Lage::Offen => return (Lage::Offen, false),
        }
    }
    match waehle_freien(liste, aktiv) {
        Some(s) => (
            Lage::Bereit(Reservierung {
                uuid: s.uuid.clone(),
                display: display.to_owned(),
                id: s.id,
                generation,
            }),
            true,
        ),
        // Kein freier Schreibtisch: NICHT einen belegten nehmen. Der Aufrufer
        // meldet das und bittet einmalig um einen neuen.
        None => (
            if vorhanden.is_some() {
                Lage::Verschwunden
            } else {
                Lage::Offen
            },
            false,
        ),
    }
}

/// Zaehlt je Schreibtisch die FREMDEN Fenster.
///
/// Eingabe ist je Fenster (Programmname, Schreibtisch). Programme, deren
/// Fenster auf mindestens `ueberall` verschiedenen Schreibtischen liegen,
/// zaehlen nicht mit: das sind Hilfsprogramme, die ueberall mitschwimmen
/// (gemessen etwa ein Treiber-Fenster auf jedem einzelnen Schreibtisch).
/// Wuerde man sie mitzaehlen, waere KEIN Schreibtisch je leer und Noki
/// koennte nie einen beanspruchen. Ein echtes Nutzerfenster - Code, Chrome,
/// ChatGPT - liegt dagegen auf einem Schreibtisch und zaehlt sehr wohl.
pub fn belegung(fenster: &[(String, u64)], ueberall: usize) -> Vec<(u64, u32)> {
    let mut breite: Vec<(&str, Vec<u64>)> = vec![];
    for (owner, space) in fenster {
        match breite.iter_mut().find(|(o, _)| *o == owner.as_str()) {
            Some((_, v)) => {
                if !v.contains(space) {
                    v.push(*space);
                }
            }
            None => breite.push((owner.as_str(), vec![*space])),
        }
    }
    let mut out: Vec<(u64, u32)> = vec![];
    for (owner, space) in fenster {
        let ubiquitaer = breite
            .iter()
            .find(|(o, _)| *o == owner.as_str())
            .is_some_and(|(_, v)| v.len() >= ueberall);
        if ubiquitaer {
            continue;
        }
        match out.iter_mut().find(|(s, _)| s == space) {
            Some((_, n)) => *n += 1,
            None => out.push((*space, 1)),
        }
    }
    out
}

/// Wohin Kuerzel 4 vom Arbeitsplatz aus zurueckfuehrt.
///
/// Normalerweise dorthin, wo der Nutzer herkam. Ist nichts gemerkt - etwa
/// weil Noki gestartet wurde, waehrend der Nutzer schon auf dem Arbeitsplatz
/// stand -, waere das Kuerzel sonst wirkungslos und der Nutzer saesse auf
/// Nokis Schreibtisch fest. Dann fuehrt der Weg zum Hauptschreibtisch
/// (der ohne uuid), sonst zum ersten anderen echten Schreibtisch.
pub fn rueckweg(liste: &[Schreibtisch], arbeitsplatz: u64, gemerkt: u64) -> Option<u64> {
    let gueltig = |id: u64| {
        liste
            .iter()
            .any(|s| s.id == id && s.typ == TYP_SCHREIBTISCH)
    };
    if gemerkt != 0 && gemerkt != arbeitsplatz && gueltig(gemerkt) {
        return Some(gemerkt);
    }
    liste
        .iter()
        .find(|s| s.typ == TYP_SCHREIBTISCH && s.id != arbeitsplatz && (s.uuid.is_empty() || s.uuid.starts_with("managed-space:")))
        .or_else(|| {
            liste
                .iter()
                .find(|s| s.typ == TYP_SCHREIBTISCH && s.id != arbeitsplatz)
        })
        .map(|s| s.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(id: u64, uuid: &str, typ: i32, fremd: u32) -> Schreibtisch {
        Schreibtisch {
            id,
            uuid: uuid.into(),
            typ,
            fremde_fenster: fremd,
        }
    }

    /// Die gemessene Lage der Testmaschine: vier echte Schreibtische, zwei
    /// Vollbild-Spaces, der erste Schreibtisch ohne uuid.
    fn echte_liste() -> Vec<Schreibtisch> {
        vec![
            st(2796, "377A68E9", 0, 8),  // belegt: Terminal, Claude, ...
            st(2864, "CF7C2FE8", 4, 3),  // Vollbild ChatGPT
            st(2576, "A45D6A2E", 0, 0),  // frei
            st(1, "", 0, 10),            // Hauptschreibtisch, ohne uuid
            st(2803, "89FE20F9", 4, 4),  // Vollbild Chrome
            st(2730, "4086311E", 0, 0),  // frei
        ]
    }

    #[test]
    fn nur_echte_leere_schreibtische_kommen_in_frage() {
        let l = echte_liste();
        assert!(!ist_frei(&l[0]), "belegter Schreibtisch ist nicht frei");
        assert!(!ist_frei(&l[1]), "Vollbild-Space gehoert seiner App");
        assert!(ist_frei(&l[2]));
        assert!(!ist_frei(&l[3]), "ohne uuid nicht wiederauffindbar");
        assert!(ist_frei(&l[5]));
    }

    #[test]
    fn sichtbare_nummer_zaehlt_nur_normale_schreibtische() {
        let l = vec![
            st(10, "A", 0, 1),
            st(11, "VOLL", 4, 0),
            st(12, "B", 0, 0),
            st(13, "SYSTEM", 7, 0),
            st(14, "C", 0, 0),
        ];
        assert_eq!(sichtbare_nummer(&l, 10), Some(1));
        assert_eq!(sichtbare_nummer(&l, 12), Some(2));
        assert_eq!(sichtbare_nummer(&l, 14), Some(3));
        assert_eq!(sichtbare_nummer(&l, 11), None);
    }

    #[test]
    fn der_hinterste_freie_wird_gewaehlt_und_nie_der_aktive() {
        let l = echte_liste();
        assert_eq!(waehle_freien(&l, 1).unwrap().id, 2730);
        // Steht der Nutzer gerade auf 2730, bekommt er ihn nicht weggenommen.
        assert_eq!(waehle_freien(&l, 2730).unwrap().id, 2576);
    }

    #[test]
    fn ein_belegter_schreibtisch_wird_niemals_gekapert() {
        let belegt = vec![
            st(2796, "A", 0, 8),
            st(1, "", 0, 10),
            st(2864, "B", 4, 1),
        ];
        assert!(waehle_freien(&belegt, 1).is_none());
        let (lage, neu) = aufloesen(None, &belegt, 1, "D", 1);
        assert_eq!(lage, Lage::Offen);
        assert!(!neu, "ohne freien Schreibtisch wird nichts beansprucht");
    }

    #[test]
    fn eine_bestehende_reservierung_wird_wiederverwendet_statt_verdoppelt() {
        let l = echte_liste();
        let res = Reservierung {
            uuid: "4086311E".into(),
            display: "D".into(),
            id: 999, // absichtlich veraltet
            generation: 1,
        };
        let (lage, neu) = aufloesen(Some(&res), &l, 1, "D", 2);
        assert!(!neu, "kein zweiter Arbeitsplatz");
        match lage {
            Lage::Bereit(r) => {
                assert_eq!(r.uuid, "4086311E");
                assert_eq!(r.id, 2730, "die fluechtige Kennung wird neu aufgeloest");
            }
            other => panic!("Bereit erwartet, nicht {other:?}"),
        }
    }

    /// Der Nutzer hat den Schreibtisch in Mission Control entfernt.
    /// P0: Niemals automatisch auf einen anderen Schreibtisch ausweichen!
    #[test]
    fn eine_tote_kennung_wird_nie_automatisch_durch_anderen_desktop_ersetzt() {
        let l = echte_liste();
        let res = Reservierung {
            uuid: "WEG".into(),
            display: "D".into(),
            id: 2730,
            generation: 1,
        };
        assert_eq!(wiederfinden(&res, &l), Lage::Verschwunden);
        let (lage, neu) = aufloesen(Some(&res), &l, 1, "D", 2);
        assert!(!neu);
        assert_eq!(lage, Lage::Verschwunden);
    }

    /// Ein Hilfsprogramm auf JEDEM Schreibtisch darf keinen davon belegen -
    /// sonst faende Noki nie einen freien. Ein echtes Nutzerfenster schon.
    #[test]
    fn allgegenwaertige_helfer_belegen_keinen_schreibtisch() {
        let f = vec![
            ("Cua Driver".to_string(), 2796),
            ("Cua Driver".to_string(), 2576),
            ("Cua Driver".to_string(), 1),
            ("Cua Driver".to_string(), 2730),
            ("Code".to_string(), 1),       // echtes Nutzerfenster
            ("Terminal".to_string(), 2796),
        ];
        let b = belegung(&f, 3);
        assert_eq!(b.iter().find(|(s, _)| *s == 2730), None, "leer bleibt leer");
        assert_eq!(b.iter().find(|(s, _)| *s == 2576), None);
        assert_eq!(b.iter().find(|(s, _)| *s == 1).map(|(_, n)| *n), Some(1), "Code belegt");
        assert_eq!(b.iter().find(|(s, _)| *s == 2796).map(|(_, n)| *n), Some(1), "Terminal belegt");
    }

    /// Ein Programm auf zwei Schreibtischen ist noch kein Helfer.
    #[test]
    fn zwei_schreibtische_machen_noch_keinen_helfer() {
        let f = vec![("Chrome".to_string(), 1), ("Chrome".to_string(), 2730)];
        let b = belegung(&f, 3);
        assert_eq!(b.iter().find(|(s, _)| *s == 2730).map(|(_, n)| *n), Some(1));
    }

    /// Ein Vollbild-Space gehoert der App, die ihn aufgespannt hat - auch
    /// wenn dort sonst nichts liegt. Waehlte Noki ihn, landete der Nutzer
    /// beim Sprung mitten in einem fremden Vollbildprogramm.
    #[test]
    fn ein_leerer_vollbild_space_wird_trotzdem_nie_gewaehlt() {
        let nur_vollbild = vec![
            st(1, "", 0, 9),              // Hauptschreibtisch, belegt
            st(2864, "CF7C2FE8", 4, 0),   // Vollbild, leer gezaehlt
            st(2803, "89FE20F9", 4, 0),   // Vollbild, leer gezaehlt
        ];
        assert!(waehle_freien(&nur_vollbild, 1).is_none());
        assert!(!ist_frei(&nur_vollbild[1]));
        assert!(!ist_frei(&nur_vollbild[2]));
        // Auch als Rueckweg kommt ein Vollbild-Space nie in Frage.
        assert_eq!(rueckweg(&nur_vollbild, 2730, 2864), Some(1));
    }

    /// Ohne gemerkte Herkunft muss das Kuerzel trotzdem zurueckfuehren -
    /// sonst sitzt der Nutzer auf Nokis Schreibtisch fest.
    #[test]
    fn der_rueckweg_existiert_auch_ohne_gemerkte_herkunft() {
        let l = echte_liste();
        assert_eq!(rueckweg(&l, 2730, 2796), Some(2796), "gemerkte Herkunft gilt");
        assert_eq!(rueckweg(&l, 2730, 0), Some(1), "sonst der Hauptschreibtisch");
        assert_eq!(rueckweg(&l, 2730, 2730), Some(1), "nie auf sich selbst");
        assert_eq!(rueckweg(&l, 2730, 99999), Some(1), "unbekannte Herkunft faellt zurueck");
        // Ein Vollbild-Space ist nie ein Rueckweg.
        assert_eq!(rueckweg(&l, 2730, 2864), Some(1));
    }

    /// Die Reihenfolge in Mission Control darf die Identitaet nicht bestimmen.
    #[test]
    fn die_position_ist_keine_identitaet() {
        let mut l = echte_liste();
        let res = Reservierung {
            uuid: "4086311E".into(),
            display: "D".into(),
            id: 2730,
            generation: 1,
        };
        l.reverse(); // Nutzer hat die Schreibtische umsortiert
        match wiederfinden(&res, &l) {
            Lage::Bereit(r) => assert_eq!(r.id, 2730),
            other => panic!("trotz neuer Reihenfolge wiederfinden, nicht {other:?}"),
        }
    }
}

/// WELCHER Schreibtisch soll Nokis sein? Der Nutzer waehlt ihn selbst.
///
/// `LINKS-VON-1 + Pfeil` blaettert durch die normalen Schreibtische in der
/// LEBENDEN Mission-Control-Ordnung. Drei Regeln, und nur diese drei:
///
///   * Nur echte Schreibtische (`typ` 0) mit gueltiger uuid. Vollbild-Spaces
///     gehoeren ihrer App, tote Eintraege gehoeren niemandem.
///   * Auch der Schreibtisch, auf dem der Nutzer GERADE steht, bleibt in der
///     Liste. Die Auswahl ist eine Rollenwahl, keine Navigation; ein normaler
///     Desktop darf nicht nur wegen der aktuellen Position verschwinden.
///   * Rundherum: hinter dem letzten kommt wieder der erste.
///
/// Gewaehlt wird ueber die uuid, nicht ueber die Nummer. Die Nummer ist
/// Darstellung und wandert, sobald macOS die Reihenfolge aendert.
///
/// `None` heisst: es gibt keinen normalen Schreibtisch, zwischen dem man
/// waehlen koennte. Die Funktion selbst navigiert niemals zu einem Space.
pub fn naechster_arbeitsplatz(
    liste: &[Schreibtisch],
    jetzt: &str,
    nutzer_space: u64,
    vor: bool,
) -> Option<String> {
    // Candidates are filtered BEFORE choosing: normal Desktops with a
    // stable identity, and NEVER the Desktop the user physically stands on
    // (it would be hidden at once - the Miniatur "vanished", or showed the
    // current Desktop for a moment before the next press moved on).
    let normal: Vec<&Schreibtisch> = liste
        .iter()
        .filter(|s| s.typ == TYP_SCHREIBTISCH && persistente_identitaet(&s.uuid))
        .collect();
    let n = normal.len();
    if n == 0 {
        return None;
    }
    let waehlbar = |s: &&Schreibtisch| s.id != nutzer_space && s.uuid != jetzt;
    match normal.iter().position(|s| s.uuid == jetzt) {
        // Walk the real Mission-Control order from the current target
        // (also when that target is the physical Desktop itself).
        Some(i) => (1..n)
            .map(|k| if vor { normal[(i + k) % n] } else { normal[(i + n - k) % n] })
            .find(waehlbar)
            .map(|s| s.uuid.clone()),
        // Unknown current target: start at the edge.
        None => {
            let mut it: Box<dyn Iterator<Item = &&Schreibtisch>> =
                if vor { Box::new(normal.iter()) } else { Box::new(normal.iter().rev()) };
            it.find(|s| s.id != nutzer_space).map(|s| s.uuid.clone())
        }
    }
}

/// Explicit SHOW request (Shortcut 4) while the user physically stands on
/// the preview target: the next normal Desktop in Mission-Control order that
/// is NOT the physical one. Deterministic, never the current Desktop.
pub fn naechster_ausser_physisch(liste: &[Schreibtisch], jetzt: &str, nutzer_space: u64) -> Option<String> {
    let waehlbar: Vec<&Schreibtisch> = liste
        .iter()
        .filter(|s| s.typ == TYP_SCHREIBTISCH && persistente_identitaet(&s.uuid))
        .collect();
    let n = waehlbar.len();
    let start = waehlbar.iter().position(|s| s.uuid == jetzt).unwrap_or(n.saturating_sub(1));
    (1..=n)
        .map(|k| waehlbar[(start + k) % n])
        .find(|s| s.id != nutzer_space && s.uuid != jetzt)
        .map(|s| s.uuid.clone())
}

#[cfg(test)]
mod wahl_tests {
    use super::*;

    fn tisch(id: u64, uuid: &str) -> Schreibtisch {
        Schreibtisch { id, uuid: uuid.into(), typ: TYP_SCHREIBTISCH, fremde_fenster: 0 }
    }

    /// Der aktuelle Schreibtisch bleibt wie jeder andere normale Desktop in
    /// der stabilen Reihenfolge; die Funktion waehlt nur und navigiert nie.
    #[test]
    fn show_request_never_picks_the_physical_desktop() {
        let l = vec![tisch(1, "a"), tisch(2, "b"), tisch(3, "c")];
        // target == physical (b): next non-physical after b is c
        assert_eq!(naechster_ausser_physisch(&l, "b", 2).as_deref(), Some("c"));
        // wraps, skips physical
        assert_eq!(naechster_ausser_physisch(&l, "c", 3).as_deref(), Some("a"));
        // only one Desktop: nothing to show
        assert_eq!(naechster_ausser_physisch(&l[..1], "a", 1), None);
        // unknown current target: first non-physical in order
        assert_eq!(naechster_ausser_physisch(&l, "zz", 1).as_deref(), Some("b"));
    }

    #[test]
    fn der_physische_schreibtisch_ist_nie_kandidat() {
        let l = vec![tisch(1, "A"), tisch(2, "B"), tisch(3, "C"), tisch(4, "D")];
        // Nutzer steht auf 3 (C): C ist in keiner Richtung waehlbar.
        assert_eq!(naechster_arbeitsplatz(&l, "D", 3, false).as_deref(), Some("B"));
        assert_eq!(naechster_arbeitsplatz(&l, "B", 3, false).as_deref(), Some("A"));
        assert_eq!(naechster_arbeitsplatz(&l, "A", 3, false).as_deref(), Some("D"));
        assert_eq!(naechster_arbeitsplatz(&l, "A", 3, true).as_deref(), Some("B"));
        assert_eq!(naechster_arbeitsplatz(&l, "B", 3, true).as_deref(), Some("D"));
        assert_eq!(naechster_arbeitsplatz(&l, "D", 3, true).as_deref(), Some("A"));
        // Target == physical (user swiped onto it): next in order, never C.
        assert_eq!(naechster_arbeitsplatz(&l, "C", 3, true).as_deref(), Some("D"));
        assert_eq!(naechster_arbeitsplatz(&l, "C", 3, false).as_deref(), Some("B"));
        // 200 presses from every physical Desktop: never the physical one,
        // and every other normal Desktop is reached.
        for phys in 1..=4u64 {
            let mut jetzt = "A".to_string();
            let mut gesehen = std::collections::HashSet::new();
            for k in 0..50 {
                jetzt = naechster_arbeitsplatz(&l, &jetzt, phys, k % 7 != 3).unwrap();
                let id = l.iter().find(|s| s.uuid == jetzt).unwrap().id;
                assert_ne!(id, phys);
                gesehen.insert(id);
            }
            assert_eq!(gesehen.len(), 3, "phys={phys}");
        }
    }

    /// Vollbild-Spaces und tote Eintraege gehoeren nicht zur Wahl.
    #[test]
    fn nur_echte_schreibtische_stehen_zur_wahl() {
        let l = vec![
            tisch(1, "A"),
            Schreibtisch { id: 9, uuid: "V".into(), typ: 4, fremde_fenster: 0 }, // Vollbild
            Schreibtisch { id: 7, uuid: String::new(), typ: 0, fremde_fenster: 0 }, // ohne uuid
            tisch(2, "B"),
        ];
        assert_eq!(naechster_arbeitsplatz(&l, "A", 99, true).as_deref(), Some("B"));
        assert_eq!(naechster_arbeitsplatz(&l, "B", 99, true).as_deref(), Some("A"));
    }

    #[test]
    fn primaerer_schreibtisch_bleibt_waehlbar_ohne_session_id() {
        let primary = "primary-desktop:DISPLAY-A";
        assert!(persistente_identitaet(primary));
        assert!(!persistente_identitaet("managed-space:DISPLAY-A:9182"));
        let l = vec![tisch(9182, primary), tisch(42, "DESKTOP-B")];
        assert_eq!(naechster_arbeitsplatz(&l, "DESKTOP-B", 99, true).as_deref(), Some(primary));
    }

    #[test]
    fn hundert_runden_enthalten_alle_normalen_aber_keinen_vollbild_space() {
        let l = vec![
            tisch(1, "A"),
            tisch(2, "B"),
            Schreibtisch { id: 20, uuid: "FS1".into(), typ: 4, fremde_fenster: 0 },
            tisch(3, "AKTIV"),
            tisch(4, "D"),
            Schreibtisch { id: 21, uuid: "FS2".into(), typ: 4, fremde_fenster: 0 },
        ];
        let mut jetzt = "A".to_owned();
        for _ in 0..100 {
            jetzt = naechster_arbeitsplatz(&l, &jetzt, 3, true)
                .expect("mindestens ein normaler Desktop");
            // never fullscreen, never the physical Desktop (id 3 = AKTIV)
            assert!(matches!(jetzt.as_str(), "A" | "B" | "D"));
        }
        let mut gesehen = std::collections::BTreeSet::new();
        let mut jetzt = "A".to_owned();
        for _ in 0..3 {
            jetzt = naechster_arbeitsplatz(&l, &jetzt, 3, true).unwrap();
            gesehen.insert(jetzt.clone());
        }
        assert_eq!(gesehen, ["A", "B", "D"].into_iter().map(str::to_owned).collect());
    }

    /// Gibt es nichts zu waehlen, geschieht nichts. Auch dann navigiert die
    /// Auswahl nie physisch zu einem Space.
    #[test]
    fn ohne_zweiten_schreibtisch_bleibt_alles_stehen() {
        let l = vec![tisch(1, "A"), tisch(2, "B")];
        // A is the physical Desktop: no other candidate - nothing changes.
        assert_eq!(naechster_arbeitsplatz(&l, "B", 1, true), None);
        assert_eq!(naechster_arbeitsplatz(&l, "B", 7, true).as_deref(), Some("A"));
        assert_eq!(naechster_arbeitsplatz(&[], "B", 1, true), None);
    }

    /// Ist der gemerkte Arbeitsplatz weg, beginnt das Blaettern am Rand -
    /// nicht im Nichts.
    #[test]
    fn ein_verschwundener_arbeitsplatz_faengt_am_rand_an() {
        let l = vec![tisch(1, "A"), tisch(2, "B"), tisch(3, "C")];
        assert_eq!(naechster_arbeitsplatz(&l, "WEG", 3, true).as_deref(), Some("A"));
        // from the end, skipping the physical Desktop (3 = C)
        assert_eq!(naechster_arbeitsplatz(&l, "WEG", 3, false).as_deref(), Some("B"));
        // Der Nutzer steht auf Nokis bisherigem Schreibtisch: der naechste
        // in der stabilen Reihenfolge ist A (C selbst ist nie Kandidat).
        assert_eq!(naechster_arbeitsplatz(&l, "C", 3, true).as_deref(), Some("A"));
    }
}
