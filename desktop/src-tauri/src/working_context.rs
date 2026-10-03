//! Task-scoped context and resource resolution for the local-first work agent.
//! This module contains no product or app name catalogue. Callers supply metadata
//! discovered from the current Mac, recent conversation and connected services.

use crate::permissions::RiskLevel;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    InstalledApp,
    RunningApp,
    Window,
    BrowserTab,
    File,
    Folder,
    Project,
    Service,
    KnownEntity,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Resource {
    pub id: String,
    pub name: String,
    pub kind: ResourceKind,
    pub locator: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub running: bool,
    #[serde(default)]
    pub recency: f32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WorkingContext {
    pub installed_apps: Vec<Resource>,
    pub running_apps: Vec<Resource>,
    pub active_app: Option<Resource>,
    pub open_windows: Vec<Resource>,
    pub active_window: Option<Resource>,
    pub browser_context: Vec<Resource>,
    pub selected_text: Option<String>,
    pub recent_files: Vec<Resource>,
    pub project_context: Vec<Resource>,
    pub connected_services: Vec<Resource>,
    pub available_mcp_tools: Vec<String>,
    pub available_local_capabilities: Vec<String>,
    pub recent_entities: Vec<Resource>,
}

impl WorkingContext {
    pub fn resources(&self) -> Vec<Resource> {
        let mut out = Vec::new();
        out.extend(self.installed_apps.clone());
        out.extend(self.running_apps.clone());
        out.extend(self.active_app.clone());
        out.extend(self.open_windows.clone());
        out.extend(self.active_window.clone());
        out.extend(self.browser_context.clone());
        out.extend(self.recent_files.clone());
        out.extend(self.project_context.clone());
        out.extend(self.connected_services.clone());
        out.extend(self.recent_entities.clone());
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out.dedup_by(|a, b| a.id == b.id);
        out
    }

    /// Only the metadata groups required by the current task are retained.
    pub fn scoped(&self, need: &ContextNeed) -> Self {
        Self {
            installed_apps: need
                .apps
                .then(|| self.installed_apps.clone())
                .unwrap_or_default(),
            running_apps: need
                .apps
                .then(|| self.running_apps.clone())
                .unwrap_or_default(),
            active_app: need.active.then(|| self.active_app.clone()).flatten(),
            open_windows: need
                .windows
                .then(|| self.open_windows.clone())
                .unwrap_or_default(),
            active_window: need.active.then(|| self.active_window.clone()).flatten(),
            browser_context: need
                .browser
                .then(|| self.browser_context.clone())
                .unwrap_or_default(),
            selected_text: need
                .selected_text
                .then(|| self.selected_text.clone())
                .flatten(),
            recent_files: need
                .files
                .then(|| self.recent_files.clone())
                .unwrap_or_default(),
            project_context: need
                .projects
                .then(|| self.project_context.clone())
                .unwrap_or_default(),
            connected_services: need
                .services
                .then(|| self.connected_services.clone())
                .unwrap_or_default(),
            available_mcp_tools: need
                .services
                .then(|| self.available_mcp_tools.clone())
                .unwrap_or_default(),
            available_local_capabilities: self.available_local_capabilities.clone(),
            recent_entities: need
                .conversation
                .then(|| self.recent_entities.clone())
                .unwrap_or_default(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContextNeed {
    pub apps: bool,
    pub active: bool,
    pub windows: bool,
    pub browser: bool,
    pub selected_text: bool,
    pub files: bool,
    pub projects: bool,
    pub services: bool,
    pub conversation: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SpeechAct {
    Open,
    Close,
    Show,
    Find,
    Read,
    Search,
    Type,
    Select,
    Play,
    Pause,
    Analyze,
    Summarize,
    Answer,
}

impl SpeechAct {
    pub fn is_explicit_action(self) -> bool {
        !matches!(self, Self::Analyze | Self::Summarize | Self::Answer)
    }
}

pub fn speech_act(input: &str) -> SpeechAct {
    let n = normalize(input);
    let words: Vec<_> = n.split_whitespace().collect();
    // Trennbares "aufmachen": "Mach Spotify auf." / "Mach das Dokument auf."
    // Das Partikel muss am Satzende stehen - sonst waere "Mach mir eine
    // Zusammenfassung auf Deutsch" ein Oeffnen-Auftrag.
    if words.last() == Some(&"auf")
        && words
            .iter()
            .any(|w| matches!(*w, "mach" | "mache" | "machst" | "aufmachen"))
    {
        return SpeechAct::Open;
    }
    let first = words
        .iter()
        .copied()
        .find(|w| !matches!(*w, "bitte" | "kannst" | "konntest" | "du" | "mal" | "kurz"))
        .unwrap_or("");
    let verb = if matches!(
        first,
        "offne"
            | "offnen"
            | "starte"
            | "starten"
            | "launch"
            | "schliesse"
            | "schliessen"
            | "beende"
            | "beenden"
            | "zeig"
            | "zeige"
            | "anzeigen"
            | "finde"
            | "finden"
            | "lies"
            | "lese"
            | "lesen"
            | "suche"
            | "such"
            | "suchen"
            | "tippe"
            | "schreibe"
            | "eingeben"
            | "wahle"
            | "markiere"
            | "selektiere"
            | "spiel"
            | "spiele"
            | "abspielen"
            | "pause"
            | "pausiere"
            | "stoppe"
            | "analysiere"
            | "erklare"
            | "fass"
            | "fasse"
            | "zusammenfassen"
    ) {
        first
    } else {
        words
            .iter()
            .copied()
            .find(|w| {
                matches!(
                    *w,
                    "offne"
                        | "offnen"
                        | "starte"
                        | "starten"
                        | "schliesse"
                        | "schliessen"
                        | "beende"
                        | "beenden"
                        | "zeig"
                        | "zeige"
                        | "anzeigen"
                        | "finde"
                        | "finden"
                        | "lies"
                        | "lese"
                        | "lesen"
                        | "suche"
                        | "such"
                        | "suchen"
                        | "tippe"
                        | "schreibe"
                        | "eingeben"
                        | "wahle"
                        | "markiere"
                        | "selektiere"
                        | "spiel"
                        | "spiele"
                        | "abspielen"
                        | "pause"
                        | "pausiere"
                        | "stoppe"
                        | "analysiere"
                        | "erklare"
                        | "fass"
                        | "fasse"
                        | "zusammenfassen"
                )
            })
            .unwrap_or(first)
    };
    match verb {
        "offne" | "offnen" | "starte" | "starten" | "launch" => SpeechAct::Open,
        "schliesse" | "schliessen" | "beende" | "beenden" => SpeechAct::Close,
        "zeig" | "zeige" | "anzeigen" => SpeechAct::Show,
        "finde" | "finden" => SpeechAct::Find,
        "lies" | "lese" | "lesen" => SpeechAct::Read,
        "suche" | "such" | "suchen" => SpeechAct::Search,
        "tippe" | "schreibe" | "eingeben" => SpeechAct::Type,
        "wahle" | "markiere" | "selektiere" => SpeechAct::Select,
        "spiel" | "spiele" | "abspielen" => SpeechAct::Play,
        "pause" | "pausiere" | "stoppe" => SpeechAct::Pause,
        "analysiere" | "erklare" => SpeechAct::Analyze,
        "fass" | "fasse" | "zusammenfassen" => SpeechAct::Summarize,
        _ => SpeechAct::Answer,
    }
}

pub fn context_need(input: &str, act: SpeechAct) -> ContextNeed {
    let n = normalize(input);
    let deictic = ["dies", "dieses", "diese", "hier", "ausgewahlt", "offen"]
        .iter()
        .any(|x| n.contains(x));
    ContextNeed {
        apps: matches!(
            act,
            SpeechAct::Open | SpeechAct::Close | SpeechAct::Play | SpeechAct::Pause
        ),
        active: deictic
            || matches!(
                act,
                SpeechAct::Read | SpeechAct::Analyze | SpeechAct::Summarize
            ),
        windows: deictic || matches!(act, SpeechAct::Show | SpeechAct::Close),
        browser: n.contains("browser") || matches!(act, SpeechAct::Search),
        selected_text: deictic
            && matches!(
                act,
                SpeechAct::Read | SpeechAct::Analyze | SpeechAct::Summarize
            ),
        files: n.contains("datei")
            || n.contains("dokument")
            || n.contains("pdf")
            || matches!(act, SpeechAct::Find),
        projects: n.contains("projekt") || n.contains("repo"),
        services: matches!(act, SpeechAct::Play | SpeechAct::Pause),
        conversation: true,
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CandidateScore {
    pub resource: Resource,
    pub phonetic_similarity: f32,
    pub spelling_similarity: f32,
    pub semantic_fit: f32,
    pub action_fit: f32,
    pub local_availability: f32,
    pub conversation_context: f32,
    pub recency: f32,
    pub active_context: f32,
    pub total: f32,
}

#[derive(Clone, Debug)]
pub enum Resolution {
    Resolved(CandidateScore),
    Ambiguous(Vec<CandidateScore>),
    Missing,
}

pub fn rank_resources(
    target: &str,
    act: SpeechAct,
    context: &WorkingContext,
) -> Vec<CandidateScore> {
    let target = normalize(target);
    let mut out: Vec<_> = context
        .resources()
        .into_iter()
        .map(|resource| {
            let names = std::iter::once(resource.name.as_str())
                .chain(resource.aliases.iter().map(String::as_str));
            let (spelling, phonetic) = names.fold((0.0_f32, 0.0_f32), |best, name| {
                (
                    best.0.max(similarity(&target, &normalize(name))),
                    best.1
                        .max(similarity(&phonetic_key(&target), &phonetic_key(name))),
                )
            });
            let semantic = semantic_fit(act, resource.kind);
            let action = if action_supports(act, resource.kind) {
                1.0
            } else {
                0.15
            };
            let local = if resource.locator.is_some()
                || matches!(
                    resource.kind,
                    ResourceKind::Window | ResourceKind::BrowserTab
                ) {
                1.0
            } else {
                0.55
            };
            let conversation = matches!(resource.kind, ResourceKind::KnownEntity) as u8 as f32;
            let active = if resource.active {
                1.0
            } else if resource.running {
                0.55
            } else {
                0.0
            };
            let total = spelling * 0.28
                + phonetic * 0.20
                + semantic * 0.12
                + action * 0.12
                + local * 0.10
                + conversation * 0.05
                + resource.recency.clamp(0.0, 1.0) * 0.05
                + active * 0.08;
            CandidateScore {
                resource,
                phonetic_similarity: phonetic,
                spelling_similarity: spelling,
                semantic_fit: semantic,
                action_fit: action,
                local_availability: local,
                conversation_context: conversation,
                recency: active.max(0.0),
                active_context: active,
                total,
            }
        })
        .filter(|s| s.spelling_similarity >= 0.28 || s.phonetic_similarity >= 0.42)
        .collect();
    out.sort_by(|a, b| b.total.partial_cmp(&a.total).unwrap_or(Ordering::Equal));
    out
}

pub fn resolve_resource(target: &str, act: SpeechAct, context: &WorkingContext) -> Resolution {
    let ranked = rank_resources(target, act, context);
    let Some(best) = ranked.first() else {
        return Resolution::Missing;
    };
    // The same local entity may appear as installed app, running app and active
    // window/tab metadata. Those are supporting signals, not competing choices.
    let second_distinct = ranked
        .iter()
        .skip(1)
        .find(|x| !x.resource.name.eq_ignore_ascii_case(&best.resource.name));
    let margin = best.total - second_distinct.map(|x| x.total).unwrap_or(0.0);
    let single_phonetic =
        ranked.len() == 1 && best.total >= 0.54 && best.phonetic_similarity >= 0.5;
    if (best.total >= 0.68 && (margin >= 0.08 || best.spelling_similarity >= 0.98))
        || single_phonetic
    {
        Resolution::Resolved(best.clone())
    } else if best.total >= 0.50 {
        Resolution::Ambiguous(ranked.into_iter().take(3).collect())
    } else {
        Resolution::Missing
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskStep {
    pub ordinal: usize,
    pub action: String,
    pub capability: String,
    pub target: Option<String>,
    pub risk: RiskLevel,
    pub verify: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskPlan {
    pub speech_act: SpeechAct,
    pub model_tier: String,
    pub steps: Vec<TaskStep>,
}

pub fn plan_task(input: &str, target: Option<&Resource>) -> TaskPlan {
    let act = speech_act(input);
    let n = normalize(input);
    let mut capabilities: Vec<(&str, RiskLevel, &str)> = Vec::new();
    if matches!(act, SpeechAct::Find)
        || n.contains("von gestern")
        || (n.contains("offne") && (n.contains("datei") || n.contains("pdf")))
    {
        capabilities.push(("file.find", RiskLevel::R0, "target_exists"));
    }
    if matches!(act, SpeechAct::Open)
        && n.contains("browser")
        && (n.contains(" suche ") || n.contains(" suchen "))
    {
        capabilities.push(("browser.open", RiskLevel::R1, "browser_ready"));
        capabilities.push(("browser.search", RiskLevel::R1, "results_page"));
    } else {
        match act {
            SpeechAct::Open => capabilities.push((
                match target.map(|r| r.kind) {
                    Some(ResourceKind::File) => "file.open",
                    Some(ResourceKind::Folder) => "directory.open",
                    _ => "app.open",
                },
                RiskLevel::R1,
                "resource_open",
            )),
            SpeechAct::Search => {
                capabilities.push(("browser.open", RiskLevel::R1, "browser_ready"));
                capabilities.push(("browser.search", RiskLevel::R1, "results_page"));
            }
            SpeechAct::Read => capabilities.push(("file.read", RiskLevel::R0, "content_extracted")),
            SpeechAct::Analyze | SpeechAct::Summarize => {
                capabilities.push(("document.extract", RiskLevel::R0, "content_extracted"));
                capabilities.push(("document.analyze", RiskLevel::R0, "answer_grounded"));
            }
            SpeechAct::Close => capabilities.push(("app.close", RiskLevel::R1, "resource_closed")),
            SpeechAct::Show => capabilities.push(("ui.focus", RiskLevel::R1, "resource_frontmost")),
            SpeechAct::Type => capabilities.push(("ui.type", RiskLevel::R2, "value_present")),
            SpeechAct::Select => {
                capabilities.push(("ui.select", RiskLevel::R1, "selection_changed"))
            }
            SpeechAct::Play => capabilities.push(("media.play", RiskLevel::R1, "playing")),
            SpeechAct::Pause => capabilities.push(("media.pause", RiskLevel::R1, "paused")),
            SpeechAct::Find | SpeechAct::Answer => {}
        }
    }
    if n.contains("erste seite") && !capabilities.iter().any(|x| x.0 == "file.read") {
        capabilities.push(("file.read", RiskLevel::R0, "page_1_extracted"));
    }
    if (n.contains("wichtig") || n.contains("zusammen"))
        && !capabilities.iter().any(|x| x.0 == "document.analyze")
    {
        capabilities.push(("document.analyze", RiskLevel::R0, "answer_grounded"));
    }
    let multi = capabilities.len() > 2 || n.contains("vergleich");
    TaskPlan {
        speech_act: act,
        model_tier: if multi { "9b" } else { "4b" }.into(),
        steps: capabilities
            .into_iter()
            .enumerate()
            .map(|(i, (capability, risk, verify))| TaskStep {
                ordinal: i + 1,
                action: capability
                    .rsplit('.')
                    .next()
                    .unwrap_or(capability)
                    .to_uppercase(),
                capability: capability.into(),
                target: target.map(|r| r.name.clone()),
                risk,
                verify: verify.into(),
            })
            .collect(),
    }
}

/// Keeps the last explicit self-correction, but never guesses between differing numbers.
pub fn semantic_repair(input: &str) -> (String, bool) {
    let lower = input.to_lowercase();
    for marker in [" nein,", " nein ", " ich meine ", " besser gesagt "] {
        if let Some(pos) = lower.rfind(marker) {
            let tail = input[pos + marker.len()..].trim();
            if !tail.is_empty() {
                let nums = numbers(input);
                let tail_nums = numbers(tail);
                if nums.len() > 1 && !tail_nums.is_empty() {
                    return (input.to_owned(), true);
                }
                return (tail.to_owned(), false);
            }
        }
    }
    (input.to_owned(), false)
}

fn numbers(s: &str) -> Vec<&str> {
    s.split(|c: char| !(c.is_ascii_digit() || c == ',' || c == '.'))
        .filter(|x| x.chars().any(|c| c.is_ascii_digit()))
        .collect()
}

fn action_supports(act: SpeechAct, kind: ResourceKind) -> bool {
    match act {
        SpeechAct::Open | SpeechAct::Close | SpeechAct::Show => {
            !matches!(kind, ResourceKind::KnownEntity)
        }
        SpeechAct::Read | SpeechAct::Analyze | SpeechAct::Summarize => matches!(
            kind,
            ResourceKind::File | ResourceKind::Window | ResourceKind::BrowserTab
        ),
        SpeechAct::Search => matches!(
            kind,
            ResourceKind::InstalledApp
                | ResourceKind::RunningApp
                | ResourceKind::BrowserTab
                | ResourceKind::Service
        ),
        SpeechAct::Play | SpeechAct::Pause => matches!(
            kind,
            ResourceKind::InstalledApp | ResourceKind::RunningApp | ResourceKind::Service
        ),
        _ => true,
    }
}

fn semantic_fit(act: SpeechAct, kind: ResourceKind) -> f32 {
    if action_supports(act, kind) {
        1.0
    } else {
        0.2
    }
}

fn normalize(s: &str) -> String {
    s.to_lowercase()
        .replace('ä', "a")
        .replace('ö', "o")
        .replace('ü', "u")
        .replace('ß', "ss")
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn compact(s: &str) -> String {
    normalize(s)
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn phonetic_key(s: &str) -> String {
    let mut out = String::new();
    let mut prev = '\0';
    for c in compact(s).chars() {
        let p = match c {
            'a' | 'e' | 'i' | 'o' | 'u' | 'y' => 'a',
            'v' | 'f' | 'w' => 'f',
            'd' | 't' => 't',
            'g' | 'k' | 'q' => 'k',
            'c' | 's' | 'z' | 'x' => 's',
            'b' | 'p' => 'p',
            other => other,
        };
        if p != prev {
            out.push(p);
            prev = p;
        }
    }
    out
}

fn similarity(a: &str, b: &str) -> f32 {
    let (a, b) = (compact(a), compact(b));
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b {
        return 1.0;
    }
    if a.contains(&b) || b.contains(&a) {
        return 0.94;
    }
    let max = a.chars().count().max(b.chars().count()) as f32;
    (1.0 - edit_distance(&a, &b) as f32 / max).max(0.0)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let mut row: Vec<usize> = (0..=b.chars().count()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut next = vec![i + 1; row.len()];
        for (j, cb) in b.chars().enumerate() {
            next[j + 1] = (row[j + 1] + 1)
                .min(next[j] + 1)
                .min(row[j] + usize::from(ca != cb));
        }
        row = next;
    }
    *row.last().unwrap_or(&usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn resource(id: &str, name: &str, kind: ResourceKind) -> Resource {
        Resource {
            id: id.into(),
            name: name.into(),
            kind,
            locator: Some(format!("/{id}")),
            aliases: vec![],
            active: false,
            running: false,
            recency: 0.0,
        }
    }
    #[test]
    fn explicit_action_precedes_domain_meaning() {
        assert_eq!(speech_act("GitHub öffnen"), SpeechAct::Open);
        assert_eq!(speech_act("Öffne GitHub"), SpeechAct::Open);
    }
    #[test]
    fn asr_recovery_uses_actual_inventory() {
        let mut c = WorkingContext::default();
        c.installed_apps
            .push(resource("app:1", "GitHub", ResourceKind::InstalledApp));
        match resolve_resource("Gitarre", SpeechAct::Open, &c) {
            Resolution::Resolved(x) => assert_eq!(x.resource.name, "GitHub"),
            x => panic!("unexpected resolution: {x:?}"),
        }
    }
    #[test]
    fn close_candidates_prefer_running_and_active() {
        let mut c = WorkingContext::default();
        let mut running = resource("run:1", "Visual Studio Code", ResourceKind::RunningApp);
        running.running = true;
        running.active = true;
        c.running_apps.push(running);
        c.installed_apps.push(resource(
            "app:2",
            "Visual Studio",
            ResourceKind::InstalledApp,
        ));
        match resolve_resource("Visual Studio Code", SpeechAct::Close, &c) {
            Resolution::Resolved(x) => assert_eq!(x.resource.id, "run:1"),
            _ => panic!(),
        }
    }
    #[test]
    fn material_number_correction_is_not_silent() {
        let (same, ambiguous) = semantic_repair("Nimm 15, nein, 50 Stück");
        assert!(ambiguous);
        assert!(same.contains("15"));
        let (fixed, ambiguous) = semantic_repair("Öffne Alfa, nein, Beta");
        assert!(!ambiguous);
        assert_eq!(fixed, "Beta");
    }
    #[test]
    fn plans_browser_and_document_workflows_generically() {
        let browser = plan_task(
            "Öffne meinen Browser und suche nach verteilten Systemen",
            None,
        );
        assert_eq!(
            browser
                .steps
                .iter()
                .map(|x| x.capability.as_str())
                .collect::<Vec<_>>(),
            vec!["browser.open", "browser.search"]
        );
        let doc = plan_task("Öffne die Statistik-PDF von gestern, lies die erste Seite und sag mir die wichtigsten Punkte", None);
        let caps: Vec<_> = doc.steps.iter().map(|x| x.capability.as_str()).collect();
        assert!(
            caps.contains(&"file.find")
                && caps.contains(&"file.read")
                && caps.contains(&"document.analyze")
        );
        assert_eq!(doc.model_tier, "9b");
    }
    #[test]
    fn context_is_minimally_scoped() {
        let need = context_need("Fass dieses Dokument zusammen", SpeechAct::Summarize);
        assert!(need.active && need.selected_text && need.files);
        assert!(!need.apps && !need.services);
    }
    #[test]
    fn diverse_dynamic_app_names_rank_without_catalogue_rules() {
        let mut c = WorkingContext::default();
        for (id, name) in [
            ("1", "Freeform"),
            ("2", "Visual Studio Code"),
            ("3", "DaVinci Resolve"),
            ("4", "System Settings"),
        ] {
            c.installed_apps
                .push(resource(id, name, ResourceKind::InstalledApp));
        }
        for target in [
            "Freeform",
            "Visual Studio Code",
            "DaVinci Resolve",
            "System Settings",
        ] {
            match resolve_resource(target, SpeechAct::Open, &c) {
                Resolution::Resolved(x) => assert_eq!(x.resource.name, target),
                x => panic!("{target}: {x:?}"),
            }
        }
    }
}
