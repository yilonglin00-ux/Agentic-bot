//! NokiPermissionManager: one place that decides what Intelligence may read or do.
//! Least privilege: every read is off unless enabled; every write goes through the tool gate.
use super::intelligence::Settings;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Capability {
    ReadAuto,
    ReadSensitive,
    WriteLowRisk,
    WriteConfirm,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RiskLevel {
    /// R0: Read / Auto (reading permitted context, calculation, classification)
    R0,
    /// R1: Reversible local actions (create folder, move file to shelf) with undo / audit-log
    R1,
    /// R2: Meaningful write actions (overwrite file, execute script) -> ActionPlan with confirm
    R2,
    /// R3: Irreversible or external actions (delete, send mail/msg, purchase) -> always confirm gate
    R3,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActionPlan {
    pub title: String,
    pub steps: Vec<ActionStep>,
    pub risk_level: RiskLevel,
    pub requires_confirm: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActionStep {
    pub description: String,
    pub tool: String,
    pub target: Option<String>,
    pub reversible: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UndoRecord {
    pub id: u64,
    pub action: String,
    pub target: String,
    pub timestamp: u64,
    pub undo_supported: bool,
}

impl UndoRecord {
    pub fn new(
        id: u64,
        action: impl Into<String>,
        target: impl Into<String>,
        undo_supported: bool,
    ) -> Self {
        Self {
            id,
            action: action.into(),
            target: target.into(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            undo_supported,
        }
    }
}

impl ActionPlan {
    pub fn for_tool(tool_name: &str, target: Option<String>) -> Self {
        let r_level = tool_risk_level(tool_name);
        let requires_confirm = r_level >= RiskLevel::R2;
        let step = ActionStep {
            description: format!("Führe Aktion {tool_name} aus"),
            tool: tool_name.to_string(),
            target,
            reversible: r_level == RiskLevel::R1,
        };
        Self {
            title: format!("Plan für {tool_name}"),
            steps: vec![step],
            risk_level: r_level,
            requires_confirm,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PipelinePhase {
    #[default]
    Read,
    Plan,
    Act,
    Verify,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CapabilityLifecycle {
    pub phase: PipelinePhase,
    pub read_granted: bool,
    pub write_granted: bool,
    pub log: Vec<String>,
}

impl CapabilityLifecycle {
    pub fn new() -> Self {
        Self {
            phase: PipelinePhase::Read,
            read_granted: true,
            write_granted: false,
            log: vec!["READ granted".into()],
        }
    }
    pub fn enter_plan(&mut self) {
        self.phase = PipelinePhase::Plan;
    }
    pub fn enter_act(&mut self) {
        self.phase = PipelinePhase::Act;
        self.read_granted = false;
        self.log.push("READ revoked".into());
        self.write_granted = true;
        self.log.push("WRITE granted".into());
    }
    pub fn record_action(&mut self, action_name: &str) {
        self.log.push(format!("action: {action_name}"));
    }
    pub fn enter_verify(&mut self) {
        self.phase = PipelinePhase::Verify;
        self.write_granted = false;
        self.log.push("WRITE revoked".into());
        self.read_granted = true;
        self.log.push("READ verify".into());
    }
    pub fn can_read(&self) -> bool {
        self.read_granted
    }
    pub fn can_write(&self) -> bool {
        self.write_granted
    }
    pub fn check_tool(&self, tool_name: &str) -> Result<(), String> {
        let r_level = tool_risk_level(tool_name);
        match r_level {
            RiskLevel::R0 => {
                if !self.read_granted {
                    return Err(format!("Tool '{tool_name}' (Read) verweigert: Kein aktiver Read-Scope während {:?}", self.phase));
                }
            }
            RiskLevel::R1 | RiskLevel::R2 | RiskLevel::R3 => {
                if !self.write_granted {
                    return Err(format!("Tool '{tool_name}' (Write) verweigert: Kein aktiver Write-Scope während {:?}", self.phase));
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Perm {
    NokiState,
    ActiveApp,
    WindowTitle,
    Shelf,
    NokiFolder,
    Web,
    Memory,
    Screen,
    Ocr,
    SelectedText,
    ActivePage,
    ExternalDocs,
}

pub fn capability(p: Perm) -> Capability {
    match p {
        Perm::NokiState
        | Perm::ActiveApp
        | Perm::WindowTitle
        | Perm::Shelf
        | Perm::NokiFolder
        | Perm::Web
        | Perm::Memory => Capability::ReadAuto,
        Perm::Screen | Perm::Ocr | Perm::SelectedText | Perm::ActivePage | Perm::ExternalDocs => {
            Capability::ReadSensitive
        }
    }
}
/// Functional settings as real policies:
/// When switch is OFF: capability is denied and context is neither read nor surfaced.
/// When switch is ON: read-only, task-scoped, minimal scope.
pub fn allowed(s: &Settings, p: Perm) -> bool {
    match p {
        Perm::NokiState => true,
        Perm::ActiveApp => s.active_app,
        Perm::WindowTitle => s.window_title,
        Perm::Shelf => s.shelf,
        Perm::NokiFolder => s.noki_folder,
        Perm::Web => s.web,
        Perm::Memory => s.memory,
        Perm::Screen => s.screen,
        Perm::Ocr => s.ocr,
        Perm::SelectedText => s.selected_text,
        Perm::ActivePage => s.active_page,
        Perm::ExternalDocs => true,
    }
}

pub fn tool_risk_level(name: &str) -> RiskLevel {
    match name {
        "fs.read" | "fs.search" | "shell.readonly" | "web.reference" | "document.extract" => {
            RiskLevel::R0
        }
        "shelf_file" | "shelf" | "folder" | "timer" | "focus" | "workspace" | "camera"
        | "windows" | "freeze" | "app" | "app.open" | "file.open" | "directory.open"
        | "browser.open" | "browser.search" | "app.open_url" | "ui.focus" | "ui.select"
        | "media.play" | "media.pause" => RiskLevel::R1,
        "fs.patch" | "fs.write" | "fs.edit" | "shell.build" | "shell.test" | "edit_file" => RiskLevel::R2,
        "fs.list" | "preview.check" => RiskLevel::R0,
        _ => RiskLevel::R3,
    }
}

/// Fixed tool catalogue: (name, risk, executor exists in Noki). Model text is never executable.
const TOOLS: &[(&str, Capability, bool)] = &[
    ("fs.read", Capability::ReadAuto, true),
    ("fs.search", Capability::ReadAuto, true),
    ("fs.patch", Capability::WriteLowRisk, true),
    ("fs.write", Capability::WriteLowRisk, true),
    ("fs.edit", Capability::WriteLowRisk, true),
    ("fs.list", Capability::ReadAuto, true),
    ("preview.check", Capability::ReadAuto, true),
    ("shell.readonly", Capability::ReadAuto, true),
    ("shell.build", Capability::WriteLowRisk, true),
    ("shell.test", Capability::WriteLowRisk, true),
    ("web.reference", Capability::ReadAuto, true),
    ("shelf_file", Capability::WriteLowRisk, true),
    ("shelf", Capability::WriteLowRisk, true),
    ("folder", Capability::WriteLowRisk, true),
    ("timer", Capability::WriteLowRisk, true),
    ("focus", Capability::WriteLowRisk, true),
    ("workspace", Capability::WriteLowRisk, true),
    ("camera", Capability::WriteLowRisk, true),
    ("windows", Capability::WriteLowRisk, true),
    ("freeze", Capability::WriteLowRisk, true),
    ("app", Capability::WriteLowRisk, true),
    ("app.open", Capability::WriteLowRisk, true),
    ("app.open_url", Capability::WriteLowRisk, true),
    ("document.extract", Capability::ReadAuto, true),
    ("file.open", Capability::WriteLowRisk, true),
    ("directory.open", Capability::WriteLowRisk, true),
    ("browser.open", Capability::WriteLowRisk, true),
    ("browser.search", Capability::WriteLowRisk, true),
    ("delete_file", Capability::WriteConfirm, false),
    ("edit_file", Capability::WriteConfirm, false),
    ("move_file", Capability::WriteConfirm, false),
    ("send_message", Capability::WriteConfirm, false),
    ("submit_form", Capability::WriteConfirm, false),
    ("download", Capability::WriteConfirm, false),
    ("purchase", Capability::WriteConfirm, false),
    ("system_change", Capability::WriteConfirm, false),
];
pub fn tool_risk(name: &str) -> Option<(Capability, bool)> {
    TOOLS.iter().find(|t| t.0 == name).map(|t| (t.1, t.2))
}

/// Irreversible or outward-facing requests (always WRITE_CONFIRM).
/// Frage nach Verfuegbarkeit/Bestand - KEINE Kaufaktion. "ob es das bei Kaufland zu
/// kaufen gibt" enthaelt "kaufe" als Teilkette und wurde bisher als Bestellung
/// abgefangen, sodass die Recherche nie lief.
fn verfuegbarkeitsfrage(q: &str) -> bool {
    const HINWEIS: &[&str] = &[
        "verfügbar",
        "verfuegbar",
        "verfügbarkeit",
        "erhältlich",
        "erhaeltlich",
        "ausverkauft",
        "lieferbar",
        "vorrätig",
        "vorraetig",
        "im angebot",
        "im regal",
        "im sortiment",
        "zu kaufen gibt",
        "zu kaufen ist",
        "kaufen kann",
        "kaufen könnte",
        "noch gibt",
        "es noch",
    ];
    HINWEIS.iter().any(|k| q.contains(k))
}
pub fn confirm_intent(q: &str) -> Option<(&'static str, &'static str)> {
    let q = q.to_lowercase();
    [
        (
            &["lösch", "loesch", "entferne"][..],
            "delete_file",
            "Datei löschen",
        ),
        (
            &[
                "ändere die datei",
                "ändere diese datei",
                "ändere datei",
                "bearbeite die datei",
                "bearbeite diese datei",
                "überschreib",
            ][..],
            "edit_file",
            "Datei ändern",
        ),
        (&["verschieb"][..], "move_file", "Datei verschieben"),
        (
            &["sende ", "schick", "poste", "nachricht an"][..],
            "send_message",
            "Nachricht senden",
        ),
        (
            &["formular", "abschicken", "absenden"][..],
            "submit_form",
            "Formular absenden",
        ),
        (
            &["herunterlad", "download"][..],
            "download",
            "Download starten",
        ),
        (
            &["kaufe", "bestelle", "bezahle"][..],
            "purchase",
            "Kauf/Bestellung",
        ),
        (
            &["systemeinstellung", "passwort ändern", "account", "konto"][..],
            "system_change",
            "Konto-/Systemänderung",
        ),
    ]
    .iter()
    .find(|(keys, _, n)| {
        keys.iter().any(|k| q.contains(k)) && !(*n == "Kauf/Bestellung" && verfuegbarkeitsfrage(&q))
    })
    .map(|(_, n, l)| (*n, *l))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn verfuegbarkeitsfrage_ist_keine_kaufaktion() {
        assert_eq!(
            confirm_intent("ob es das Red Bull Purple bei Kaufland gerade zu kaufen gibt"),
            None
        );
        assert_eq!(confirm_intent("ist das bei Kaufland noch verfügbar"), None);
        // eine echte Kaufabsicht bleibt bestaetigungspflichtig
        assert_eq!(
            confirm_intent("kaufe mir zwei Dosen Red Bull").map(|x| x.0),
            Some("purchase")
        );
        assert_eq!(
            confirm_intent("bestelle das bitte").map(|x| x.0),
            Some("purchase")
        );
        assert_eq!(
            confirm_intent("lösche diese Datei").map(|x| x.0),
            Some("delete_file")
        );
    }
    #[test]
    fn least_privilege_defaults() {
        let s = Settings::default();
        assert!(allowed(&s, Perm::NokiState));
        for p in [
            Perm::ActiveApp,
            Perm::WindowTitle,
            Perm::Shelf,
            Perm::NokiFolder,
            Perm::Web,
            Perm::Memory,
            Perm::Screen,
            Perm::Ocr,
            Perm::SelectedText,
            Perm::ActivePage,
        ] {
            assert!(!allowed(&s, p), "{p:?}");
        }
        let all = Settings {
            active_app: true,
            window_title: true,
            shelf: true,
            noki_folder: true,
            web: true,
            memory: true,
            screen: true,
            ocr: true,
            selected_text: true,
            active_page: true,
            ..Default::default()
        };
        for p in [
            Perm::Screen,
            Perm::Ocr,
            Perm::SelectedText,
            Perm::ActivePage,
            Perm::ExternalDocs,
        ] {
            assert!(allowed(&all, p), "{p:?}");
            assert_eq!(capability(p), Capability::ReadSensitive);
        }
        assert_eq!(capability(Perm::Web), Capability::ReadAuto);
    }
    #[test]
    fn write_actions_are_classified() {
        assert_eq!(tool_risk("timer"), Some((Capability::WriteLowRisk, true)));
        assert_eq!(
            confirm_intent("Lösche die Datei Statistik.pdf").unwrap().0,
            "delete_file"
        );
        assert_eq!(
            tool_risk("delete_file"),
            Some((Capability::WriteConfirm, false))
        );
        assert!(confirm_intent("Was ist RAM?").is_none() && tool_risk("rm -rf").is_none());
        assert_eq!(
            confirm_intent("Ändere diese Datei bitte").unwrap().0,
            "edit_file"
        );
        assert_eq!(
            tool_risk("edit_file"),
            Some((Capability::WriteConfirm, false))
        );
        assert_eq!(tool_risk("fs.read"), Some((Capability::ReadAuto, true)));
        assert_eq!(
            tool_risk("fs.patch"),
            Some((Capability::WriteLowRisk, true))
        );
        assert!(tool_risk("shell.exec").is_none());
    }
    #[test]
    fn risk_levels_match_pipeline() {
        assert_eq!(tool_risk_level("fs.read"), RiskLevel::R0);
        assert_eq!(tool_risk_level("timer"), RiskLevel::R1);
        assert_eq!(tool_risk_level("folder"), RiskLevel::R1);
        assert_eq!(tool_risk_level("fs.patch"), RiskLevel::R2);
        assert_eq!(tool_risk_level("edit_file"), RiskLevel::R2);
        assert_eq!(tool_risk_level("delete_file"), RiskLevel::R3);
        assert_eq!(tool_risk_level("purchase"), RiskLevel::R3);
    }
    #[test]
    fn capability_lifecycle_read_plan_act_verify() {
        let mut lc = CapabilityLifecycle::new();
        assert!(lc.can_read());
        assert!(!lc.can_write());
        assert!(lc.check_tool("fs.read").is_ok());
        assert!(
            lc.check_tool("folder").is_err(),
            "write tool blocked in Read phase"
        );

        lc.enter_plan();
        assert!(lc.check_tool("fs.read").is_ok());
        assert!(
            lc.check_tool("folder").is_err(),
            "write tool blocked in Plan phase"
        );

        lc.enter_act();
        assert!(!lc.can_read());
        assert!(lc.can_write());
        assert!(
            lc.check_tool("folder").is_ok(),
            "write tool allowed in Act phase"
        );
        assert!(
            lc.check_tool("fs.read").is_err(),
            "read tool blocked in Act phase"
        );
        lc.record_action("folder:create");

        lc.enter_verify();
        assert!(lc.can_read());
        assert!(!lc.can_write());
        assert!(
            lc.check_tool("fs.read").is_ok(),
            "read tool allowed in Verify phase"
        );
        assert!(
            lc.check_tool("folder").is_err(),
            "write tool blocked in Verify phase"
        );

        assert_eq!(
            lc.log,
            vec![
                "READ granted",
                "READ revoked",
                "WRITE granted",
                "action: folder:create",
                "WRITE revoked",
                "READ verify"
            ]
        );
    }
    #[test]
    fn least_privilege_action_plan_and_undo() {
        let plan_r0 = ActionPlan::for_tool("fs.read", None);
        assert_eq!(plan_r0.risk_level, RiskLevel::R0);
        assert!(!plan_r0.requires_confirm);

        let plan_r1 = ActionPlan::for_tool("folder", Some("Neuer Ordner".into()));
        assert_eq!(plan_r1.risk_level, RiskLevel::R1);
        assert!(!plan_r1.requires_confirm);
        assert!(plan_r1.steps[0].reversible);

        let undo = UndoRecord::new(101, "folder", "Neuer Ordner", true);
        assert_eq!(undo.action, "folder");
        assert!(undo.undo_supported);

        let plan_r2 = ActionPlan::for_tool("fs.patch", Some("src/main.rs".into()));
        assert_eq!(plan_r2.risk_level, RiskLevel::R2);
        assert!(plan_r2.requires_confirm);

        let plan_r3 = ActionPlan::for_tool("delete_file", Some("wichtig.doc".into()));
        assert_eq!(plan_r3.risk_level, RiskLevel::R3);
        assert!(plan_r3.requires_confirm);
    }
}
