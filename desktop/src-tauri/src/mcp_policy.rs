//! What an MCP server is ALLOWED to be, and how its tools map onto Noki's
//! capability model.
//!
//! THE RULE THIS MODULE ENFORCES. A server does not describe its own
//! privileges. Discovery tells Noki what a server OFFERS; this table decides
//! what Noki will USE. A tool that is not matched by a rule in the server's own
//! configuration is not available - not "available with a warning", not
//! "available after a confirmation". Absent.
//!
//! WHY DENY BY DEFAULT IS THE ONLY SAFE DIRECTION. A server can rename a tool,
//! add one on the next version bump, or describe a delete as a read. If an
//! unknown tool defaulted to anything usable, installing a server would mean
//! granting whatever it later decides to offer. So the mapping is keyed on the
//! tool names the USER's configuration lists, and `readOnlyHint` from the
//! server can only ever LOWER what a rule already allows.
//!
//! SCOPE IS NOT DECORATION. `filesystem.read_text_file` is not "READ". It is
//! "READ, within these roots, for this argument". The scope check runs against
//! the actual arguments before the call leaves Noki, which is what keeps
//! "read the file I attached" from becoming "read the file system".

use crate::capability::{self, Mode};
use crate::mcp_client::{RemoteTool, Transport};
use crate::permissions::RiskLevel;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, path::Path};

/// When the user must be asked before a tool runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmPolicy {
    /// R0 reads inside an allowed scope run without interrupting the user.
    #[default]
    Never,
    /// Anything that writes, sends or deletes.
    Always,
}

/// What kind of thing a tool operates on.
///
/// WHY THIS IS CONFIGURATION AND NOT INFERENCE. The selector needs to know
/// whether `read_text_file` wants a file or a directory. The obvious source is
/// the server's own tool name and description - and that was the first
/// implementation, which was wrong: those strings are written by whoever wrote
/// the server, so a server could describe a directory lister as a file reader
/// and steer Noki's choice by editing its own metadata. It could never gain a
/// capability that way, but it could misdirect a call inside the allowlist.
///
/// So the mapping moves here, into the file the USER controls. Server metadata
/// is then descriptive only, which is the correct role for untrusted data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    File,
    Directory,
    /// Operates on anything addressable - a tool that takes a query rather than
    /// a path, for instance. Still bound by `path_args` and `roots`.
    Any,
}

impl TargetKind {
    /// Whether this rule accepts a target that is (or is not) a directory.
    pub fn accepts(self, target_is_dir: bool) -> bool {
        match self {
            TargetKind::File => !target_is_dir,
            TargetKind::Directory => target_is_dir,
            TargetKind::Any => true,
        }
    }
}

/// One rule: this tool, in this phase, at this risk, restricted to this scope.
///
/// `path_args` names the arguments that carry a filesystem path. They are the
/// ones checked against `roots` - naming them explicitly is deliberate, because
/// guessing which argument is a path is how a scope check gets silently skipped.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolRule {
    /// Tool name exactly as the server reports it.
    pub tool: String,
    /// READ or ACT. Anything not explicitly READ is ACT.
    #[serde(default)]
    pub mode: Option<String>,
    pub risk: RiskLevel,
    #[serde(default)]
    pub confirm: ConfirmPolicy,
    /// Arguments carrying a path that must lie inside `roots`.
    #[serde(default)]
    pub path_args: Vec<String>,
    /// The Noki capability this tool is exposed as.
    pub capability: String,
    /// What the tool operates on. `None` means the configuration has not said,
    /// and an unstated kind is NOT a permissive one: such a tool stays
    /// reachable by an explicit request but can never be picked autonomously,
    /// because picking it would require guessing exactly what this field exists
    /// to stop being guessed.
    #[serde(default)]
    pub target_kind: Option<TargetKind>,
}

/// A configured server. This is what a user edits; nothing here is a secret.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServerConfig {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(flatten)]
    pub transport: Transport,
    /// Off unless the user turned it on. A shipped default is never live.
    #[serde(default)]
    pub enabled: bool,
    /// Directories any path argument must stay inside. Empty = no path tool
    /// may run, because an unbounded root is the same as no scope at all.
    #[serde(default)]
    pub roots: Vec<String>,
    /// The allowlist. A tool absent from here is absent, period.
    #[serde(default)]
    pub tools: Vec<ToolRule>,
    /// Per-call budget in milliseconds.
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    /// Free-form notes about provenance, shown to the user. Never policy.
    #[serde(default)]
    pub trusted_metadata: HashMap<String, String>,
}

fn default_timeout() -> u64 {
    15_000
}

/// The whole configuration file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: Vec<ServerConfig>,
}

/// Why a call was refused, before any process was contacted.
#[derive(Clone, Debug, PartialEq)]
pub enum Refusal {
    /// No rule lists this tool.
    NotAllowed(String),
    /// A path argument pointed outside the configured roots.
    OutOfScope(String),
    /// The tool needs a confirmation that was not given.
    NeedsConfirmation(String),
    /// A path argument named something Noki never reads.
    Sensitive(String),
    /// The server is configured but switched off.
    Disabled(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::NotAllowed(t) => write!(f, "MCP-Werkzeug '{t}' ist nicht freigegeben."),
            Refusal::OutOfScope(p) => {
                write!(f, "'{p}' liegt außerhalb der freigegebenen Verzeichnisse.")
            }
            Refusal::NeedsConfirmation(t) => {
                write!(f, "'{t}' verändert etwas und braucht eine Bestätigung.")
            }
            Refusal::Sensitive(p) => write!(f, "'{p}' ist ein geschützter Pfad."),
            Refusal::Disabled(s) => write!(f, "MCP-Server '{s}' ist deaktiviert."),
        }
    }
}

impl ToolRule {
    /// READ only when the rule says so AND the capability is classified as a
    /// read by the same table the rest of Noki uses. Two independent sources
    /// must agree before a tool counts as a read.
    pub fn resolved_mode(&self) -> Mode {
        let declared_read = self.mode.as_deref() == Some("READ");
        if declared_read && capability::mode_for(&self.capability) == Mode::Read {
            Mode::Read
        } else {
            Mode::Act
        }
    }

    /// A read that the server itself calls read-only, at R0, inside scope, is
    /// the only thing that runs unattended.
    pub fn needs_confirmation(&self) -> bool {
        self.confirm == ConfirmPolicy::Always
            || self.risk >= RiskLevel::R2
            || self.resolved_mode() == Mode::Act
    }
}

impl ServerConfig {
    pub fn rule_for(&self, tool: &str) -> Option<&ToolRule> {
        self.tools.iter().find(|r| r.tool == tool)
    }

    /// Discovery result -> the tools Noki will actually expose.
    ///
    /// Two things happen here, and both matter. Tools with no rule are dropped.
    /// Tools with a rule that the server contradicts - a rule saying READ for a
    /// tool the server does NOT mark read-only - are dropped as well, rather
    /// than quietly downgraded: a disagreement about what a tool does is
    /// exactly the case where guessing is wrong.
    pub fn admit(&self, discovered: &[RemoteTool]) -> Vec<(ToolRule, RemoteTool)> {
        discovered
            .iter()
            .filter_map(|remote| {
                let rule = self.rule_for(&remote.name)?;
                if rule.resolved_mode() == Mode::Read && !remote.read_only_hint {
                    log::warn!(
                        "noki-mcp tool '{}' is configured READ but the server does not mark it read-only - dropped",
                        remote.name
                    );
                    return None;
                }
                // Admitted, but not autonomously selectable. Said out loud
                // because a configuration written before `target_kind` existed
                // loads with `None`, and a silently non-autonomous tool is the
                // kind of thing that gets diagnosed as "MCP stopped working".
                if rule.target_kind.is_none() {
                    log::info!(
                        "noki-mcp tool '{}' has no target_kind in the configuration - reachable explicitly, never chosen automatically",
                        remote.name
                    );
                }
                Some((rule.clone(), remote.clone()))
            })
            .collect()
    }

    /// The gate every call passes before a byte reaches the server.
    pub fn authorize(
        &self,
        tool: &str,
        arguments: &Value,
        confirmed: bool,
    ) -> Result<ToolRule, Refusal> {
        if !self.enabled {
            return Err(Refusal::Disabled(self.id.clone()));
        }
        let rule = self
            .rule_for(tool)
            .ok_or_else(|| Refusal::NotAllowed(tool.to_owned()))?
            .clone();
        // Scope BEFORE confirmation, on purpose. An out-of-scope target is
        // refused outright; it must never become a question the user could
        // answer with "yes". A confirmation widens nothing.
        for arg in &rule.path_args {
            let Some(raw) = arguments.get(arg).and_then(Value::as_str) else {
                // A rule that names a path argument requires it: a missing path
                // must not turn into "no scope to check".
                return Err(Refusal::OutOfScope(format!("{arg} fehlt")));
            };
            self.check_path(raw)?;
        }
        if rule.needs_confirmation() && !confirmed {
            return Err(Refusal::NeedsConfirmation(tool.to_owned()));
        }
        Ok(rule)
    }

    /// A path is in scope only if it resolves inside a configured root.
    ///
    /// Resolution happens first, so `root/../../etc/passwd` is judged as what
    /// it actually points at. When a path does not exist yet its parent is
    /// resolved instead, which is what makes a create-file scope checkable.
    fn check_path(&self, raw: &str) -> Result<(), Refusal> {
        if self.roots.is_empty() {
            return Err(Refusal::OutOfScope(raw.to_owned()));
        }
        let path = Path::new(raw);
        let resolved = dunce::canonicalize(path).or_else(|_| {
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("/"));
            dunce::canonicalize(parent).map(|p| path.file_name().map(|n| p.join(n)).unwrap_or(p))
        });
        let Ok(resolved) = resolved else {
            return Err(Refusal::OutOfScope(raw.to_owned()));
        };
        // Noki's own protected-path rule applies to MCP exactly as it does to
        // local reads; a server is not a way around it.
        if crate::code_agent::is_sensitive_path(&resolved) {
            return Err(Refusal::Sensitive(raw.to_owned()));
        }
        let inside = self.roots.iter().any(|root| {
            dunce::canonicalize(root)
                .map(|r| resolved.starts_with(&r))
                .unwrap_or(false)
        });
        if inside {
            Ok(())
        } else {
            Err(Refusal::OutOfScope(raw.to_owned()))
        }
    }

    /// The scope string recorded on the lease and in the audit log. For a path
    /// tool that is the concrete target, not the server name.
    pub fn scope_for(&self, rule: &ToolRule, arguments: &Value) -> String {
        rule.path_args
            .iter()
            .find_map(|a| arguments.get(a).and_then(Value::as_str))
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{}:{}", self.id, rule.tool))
    }
}

/// The shipped default: the reference filesystem server, switched OFF, scoped
/// to nothing until the user names a directory.
///
/// It is written out so the file explains itself, and it is deliberately inert:
/// enabling it is a decision, and it still has no roots to read until one is
/// given. Only the two read tools are listed - the same package also offers
/// write, move and delete tools, and they are absent on purpose.
pub fn default_config() -> McpConfig {
    let read_rule = |tool: &str| ToolRule {
        tool: tool.to_owned(),
        mode: Some("READ".into()),
        risk: RiskLevel::R0,
        confirm: ConfirmPolicy::Never,
        path_args: vec!["path".into()],
        capability: "mcp.read".into(),
        // Stated here, by Noki, and not read off the server's description.
        target_kind: Some(TargetKind::File),
    };
    McpConfig {
        servers: vec![ServerConfig {
            id: "filesystem".into(),
            name: "Dateien (MCP)".into(),
            description: "Liest Dateien in ausdrücklich freigegebenen Ordnern.".into(),
            transport: Transport::Stdio {
                command: "npx".into(),
                args: vec![
                    "-y".into(),
                    "@modelcontextprotocol/server-filesystem".into(),
                ],
                env_from: HashMap::new(),
                cwd: None,
            },
            enabled: false,
            roots: Vec::new(),
            tools: vec![
                read_rule("read_text_file"),
                ToolRule {
                    tool: "list_directory".into(),
                    mode: Some("READ".into()),
                    risk: RiskLevel::R0,
                    confirm: ConfirmPolicy::Never,
                    path_args: vec!["path".into()],
                    capability: "mcp.list".into(),
                    target_kind: Some(TargetKind::Directory),
                },
            ],
            timeout_ms: 15_000,
            trusted_metadata: HashMap::from([(
                "package".into(),
                "@modelcontextprotocol/server-filesystem".into(),
            )]),
        }],
    }
}

/// Fills in `target_kind` on rules that predate the field.
///
/// THE SOURCE OF TRUTH IS `default_config`, not the server. A rule is only
/// filled in when Noki's OWN shipped table states a kind for that server id and
/// tool name; anything else is left as `None`, which keeps it explicitly
/// non-autonomous. Nothing is inferred from a tool's name shape, and nothing at
/// all is read from the server - a migration that consulted server metadata
/// would reintroduce exactly the trust problem `target_kind` exists to remove.
///
/// Only `target_kind` is ever written. `enabled`, `roots`, `risk`, `confirm`,
/// `capability`, `path_args` and the transport are not touched, so migrating
/// cannot widen anything.
///
/// Returns the tool names that were filled in, for the log.
pub fn migrate_target_kinds(config: &mut McpConfig) -> Vec<String> {
    let known = default_config();
    let mut filled = Vec::new();
    for server in &mut config.servers {
        let Some(reference) = known.servers.iter().find(|s| s.id == server.id) else {
            // An unknown server id: Noki has no table for it, so it keeps
            // whatever the file said.
            continue;
        };
        for rule in &mut server.tools {
            if rule.target_kind.is_some() {
                continue;
            }
            // Matched on server id AND tool name. A tool Noki does not ship a
            // kind for stays unset rather than being guessed from its name.
            let Some(kind) = reference
                .tools
                .iter()
                .find(|r| r.tool == rule.tool)
                .and_then(|r| r.target_kind)
            else {
                continue;
            };
            rule.target_kind = Some(kind);
            filled.push(format!("{}/{}={:?}", server.id, rule.tool, kind));
        }
    }
    filled
}

/// The server's roots are also its command-line arguments: the reference
/// filesystem server takes allowed directories as positional arguments. Passing
/// the SAME list to both means the server's own sandbox and Noki's scope check
/// cannot drift apart.
pub fn transport_with_roots(cfg: &ServerConfig) -> Transport {
    let Transport::Stdio {
        command,
        args,
        env_from,
        cwd,
    } = &cfg.transport;
    let mut args = args.clone();
    if cfg.id == "filesystem" {
        args.extend(cfg.roots.iter().cloned());
    }
    Transport::Stdio {
        command: command.clone(),
        args,
        env_from: env_from.clone(),
        cwd: cwd.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tmp_root() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("noki-mcp-policy-{}", std::process::id()));
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("ok.txt"), "content").unwrap();
        dunce::canonicalize(&d).unwrap()
    }

    fn server(root: &Path) -> ServerConfig {
        let mut c = default_config().servers.remove(0);
        c.enabled = true;
        c.roots = vec![root.to_string_lossy().into_owned()];
        c
    }

    fn remote(name: &str, read_only: bool) -> RemoteTool {
        RemoteTool {
            name: name.into(),
            description: String::new(),
            input_schema: Value::Null,
            read_only_hint: read_only,
        }
    }

    #[test]
    fn the_shipped_default_is_inert() {
        let c = default_config().servers.remove(0);
        assert!(!c.enabled, "a shipped server must not be live");
        assert!(c.roots.is_empty(), "a shipped server must not have scope");
        // Even enabled, with no roots nothing is readable.
        let mut on = c.clone();
        on.enabled = true;
        assert!(matches!(
            on.authorize("read_text_file", &json!({"path":"/etc/passwd"}), false),
            Err(Refusal::OutOfScope(_))
        ));
        // Write tools from the same package are simply not listed.
        assert!(c.rule_for("write_file").is_none());
        assert!(c.rule_for("move_file").is_none());
    }

    #[test]
    fn only_allowlisted_tools_survive_discovery() {
        let root = tmp_root();
        let cfg = server(&root);
        let discovered = vec![
            remote("read_text_file", true),
            remote("write_file", false),
            remote("list_directory", true),
            // A tool the server invented after installation.
            remote("exfiltrate_everything", false),
        ];
        let admitted = cfg.admit(&discovered);
        let names: Vec<_> = admitted.iter().map(|(r, _)| r.tool.clone()).collect();
        assert_eq!(names, vec!["read_text_file", "list_directory"]);
    }

    #[test]
    fn a_read_rule_the_server_contradicts_is_dropped() {
        let root = tmp_root();
        let cfg = server(&root);
        // Configured READ, but the server does not mark it read-only.
        let admitted = cfg.admit(&[remote("read_text_file", false)]);
        assert!(
            admitted.is_empty(),
            "a disagreement must not resolve in favour of access"
        );
    }

    #[test]
    fn scope_is_checked_against_the_resolved_path() {
        let root = tmp_root();
        let cfg = server(&root);
        let inside = root.join("ok.txt").to_string_lossy().into_owned();
        assert!(cfg
            .authorize("read_text_file", &json!({"path": inside}), false)
            .is_ok());

        // Traversal out of the root is judged by where it lands.
        let escape = root.join("../../etc/passwd").to_string_lossy().into_owned();
        assert!(matches!(
            cfg.authorize("read_text_file", &json!({"path": escape}), false),
            Err(Refusal::OutOfScope(_) | Refusal::Sensitive(_))
        ));
        // A plain absolute path outside the root.
        assert!(matches!(
            cfg.authorize("read_text_file", &json!({"path": "/usr/bin/env"}), false),
            Err(Refusal::OutOfScope(_))
        ));
    }

    #[test]
    fn a_missing_path_argument_is_refused_not_ignored() {
        let root = tmp_root();
        let cfg = server(&root);
        assert!(matches!(
            cfg.authorize("read_text_file", &json!({}), false),
            Err(Refusal::OutOfScope(_))
        ));
    }

    #[test]
    fn protected_paths_stay_protected_through_mcp() {
        let root = tmp_root();
        let mut cfg = server(&root);
        // Even if a root were widened to the home directory, .ssh stays out.
        let home = std::env::var("HOME").unwrap();
        cfg.roots = vec![home.clone()];
        let key = format!("{home}/.ssh/id_rsa");
        assert!(matches!(
            cfg.authorize("read_text_file", &json!({"path": key}), false),
            Err(Refusal::Sensitive(_) | Refusal::OutOfScope(_))
        ));
    }

    #[test]
    fn an_act_tool_needs_confirmation_even_at_low_risk() {
        let rule = ToolRule {
            tool: "send".into(),
            mode: Some("ACT".into()),
            risk: RiskLevel::R1,
            confirm: ConfirmPolicy::Never,
            path_args: vec![],
            capability: "mcp.send".into(),
            target_kind: None,
        };
        assert_eq!(rule.resolved_mode(), Mode::Act);
        assert!(rule.needs_confirmation(), "an ACT tool is never unattended");
    }

    #[test]
    fn a_read_claim_on_an_act_capability_is_not_a_read() {
        // The rule says READ, but the capability is classified ACT elsewhere.
        let rule = ToolRule {
            tool: "x".into(),
            mode: Some("READ".into()),
            risk: RiskLevel::R0,
            confirm: ConfirmPolicy::Never,
            path_args: vec![],
            capability: "file.delete".into(),
            target_kind: None,
        };
        assert_eq!(rule.resolved_mode(), Mode::Act);
        assert!(rule.needs_confirmation());
    }

    #[test]
    fn a_disabled_server_authorizes_nothing() {
        let root = tmp_root();
        let mut cfg = server(&root);
        cfg.enabled = false;
        let inside = root.join("ok.txt").to_string_lossy().into_owned();
        assert!(matches!(
            cfg.authorize("read_text_file", &json!({"path": inside}), true),
            Err(Refusal::Disabled(_))
        ));
    }

    #[test]
    fn roots_reach_the_server_as_its_own_sandbox() {
        let root = tmp_root();
        let cfg = server(&root);
        let Transport::Stdio { args, .. } = transport_with_roots(&cfg);
        assert!(args.contains(&root.to_string_lossy().into_owned()));
    }

    #[test]
    fn the_audit_scope_is_the_target_not_the_server() {
        let root = tmp_root();
        let cfg = server(&root);
        let rule = cfg.rule_for("read_text_file").unwrap();
        let p = root.join("ok.txt").to_string_lossy().into_owned();
        assert_eq!(cfg.scope_for(rule, &json!({"path": p.clone()})), p);
    }

    /// The file as it was written by the version before `target_kind` existed.
    const OLD_SCHEMA: &str = r#"{
      "servers": [
        {
          "id": "filesystem",
          "name": "Dateien (MCP)",
          "description": "Liest Dateien in ausdrücklich freigegebenen Ordnern.",
          "transport": "stdio",
          "command": "npx",
          "args": ["-y", "@modelcontextprotocol/server-filesystem"],
          "env_from": {},
          "enabled": false,
          "roots": [],
          "tools": [
            {"tool":"read_text_file","mode":"READ","risk":"R0","confirm":"never","path_args":["path"],"capability":"mcp.read"},
            {"tool":"list_directory","mode":"READ","risk":"R0","confirm":"never","path_args":["path"],"capability":"mcp.list"}
          ],
          "timeout_ms": 15000,
          "trusted_metadata": {"package":"@modelcontextprotocol/server-filesystem"}
        }
      ]
    }"#;

    #[test]
    fn an_old_config_parses_and_its_tools_start_non_autonomous() {
        let cfg: McpConfig = serde_json::from_str(OLD_SCHEMA).expect("old schema must still parse");
        assert_eq!(cfg.servers.len(), 1);
        // Before migration: no kind, so not autonomously selectable.
        for rule in &cfg.servers[0].tools {
            assert_eq!(rule.target_kind, None, "{} should start unset", rule.tool);
        }
    }

    #[test]
    fn migration_sets_the_two_known_kinds_and_nothing_else() {
        let mut cfg: McpConfig = serde_json::from_str(OLD_SCHEMA).unwrap();
        let before = cfg.clone();
        let filled = migrate_target_kinds(&mut cfg);
        assert_eq!(filled.len(), 2, "both tools should be filled: {filled:?}");

        let tools = &cfg.servers[0].tools;
        assert_eq!(
            tools
                .iter()
                .find(|r| r.tool == "read_text_file")
                .unwrap()
                .target_kind,
            Some(TargetKind::File)
        );
        assert_eq!(
            tools
                .iter()
                .find(|r| r.tool == "list_directory")
                .unwrap()
                .target_kind,
            Some(TargetKind::Directory)
        );

        // NOTHING ELSE MOVED. Compared field by field against the parsed
        // original, because "only target_kind changed" is the whole promise.
        let s = &cfg.servers[0];
        let b = &before.servers[0];
        assert_eq!(s.enabled, b.enabled, "enabled changed");
        assert!(!s.enabled, "a disabled server must stay disabled");
        assert_eq!(s.roots, b.roots, "roots changed");
        assert!(s.roots.is_empty(), "roots must not be widened");
        assert_eq!(s.transport, b.transport, "transport changed");
        assert_eq!(s.timeout_ms, b.timeout_ms);
        assert_eq!(s.trusted_metadata, b.trusted_metadata);
        assert_eq!(s.tools.len(), b.tools.len(), "a tool was added or removed");
        for (now, was) in s.tools.iter().zip(&b.tools) {
            assert_eq!(now.tool, was.tool);
            assert_eq!(now.mode, was.mode);
            assert_eq!(now.risk, was.risk);
            assert_eq!(now.confirm, was.confirm);
            assert_eq!(now.path_args, was.path_args);
            assert_eq!(now.capability, was.capability);
        }
        // Idempotent: running it again changes nothing.
        assert!(migrate_target_kinds(&mut cfg).is_empty());
    }

    #[test]
    fn migration_never_guesses_for_a_tool_noki_has_no_table_for() {
        let mut cfg: McpConfig = serde_json::from_str(OLD_SCHEMA).unwrap();
        // A tool the user added by hand, with a name that LOOKS like a reader
        // and a capability that looks like a read. Noki ships no kind for it,
        // so it must stay unset rather than be inferred from its name.
        cfg.servers[0].tools.push(ToolRule {
            tool: "read_media_file".into(),
            mode: Some("READ".into()),
            risk: RiskLevel::R0,
            confirm: ConfirmPolicy::Never,
            path_args: vec!["path".into()],
            capability: "mcp.read".into(),
            target_kind: None,
        });
        let filled = migrate_target_kinds(&mut cfg);
        assert_eq!(filled.len(), 2, "only the two known tools: {filled:?}");
        assert_eq!(
            cfg.servers[0]
                .tools
                .iter()
                .find(|r| r.tool == "read_media_file")
                .unwrap()
                .target_kind,
            None,
            "an unknown tool was guessed"
        );

        // And an unknown SERVER id is left entirely alone.
        let mut renamed: McpConfig = serde_json::from_str(OLD_SCHEMA).unwrap();
        renamed.servers[0].id = "my-own-fs".into();
        assert!(migrate_target_kinds(&mut renamed).is_empty());
        assert!(renamed.servers[0]
            .tools
            .iter()
            .all(|r| r.target_kind.is_none()));
    }

    #[test]
    fn migration_does_not_overwrite_a_kind_the_user_already_set() {
        let mut cfg: McpConfig = serde_json::from_str(OLD_SCHEMA).unwrap();
        // The user deliberately narrowed one tool to Any.
        cfg.servers[0].tools[0].target_kind = Some(TargetKind::Any);
        let filled = migrate_target_kinds(&mut cfg);
        assert_eq!(filled.len(), 1, "only the unset one: {filled:?}");
        assert_eq!(cfg.servers[0].tools[0].target_kind, Some(TargetKind::Any));
    }

    #[test]
    fn a_migrated_config_becomes_autonomously_usable_once_enabled_and_scoped() {
        let root = tmp_root();
        let mut cfg: McpConfig = serde_json::from_str(OLD_SCHEMA).unwrap();
        migrate_target_kinds(&mut cfg);
        // The user's two separate decisions, which migration never makes:
        cfg.servers[0].enabled = true;
        cfg.servers[0].roots = vec![root.to_string_lossy().into_owned()];

        // The rules now carry everything autonomous selection requires.
        let server = &cfg.servers[0];
        let admitted = server.admit(&[
            remote("read_text_file", true),
            remote("list_directory", true),
        ]);
        assert_eq!(admitted.len(), 2);
        for (rule, _) in &admitted {
            assert!(
                rule.target_kind.is_some(),
                "{} still unclassified",
                rule.tool
            );
            assert_eq!(rule.resolved_mode(), Mode::Read);
            assert!(!rule.needs_confirmation());
        }
        // And the scope gate is unchanged by the migration.
        let inside = root.join("ok.txt").to_string_lossy().into_owned();
        assert!(server
            .authorize("read_text_file", &json!({"path": inside}), false)
            .is_ok());
        assert!(matches!(
            server.authorize("read_text_file", &json!({"path": "/etc/passwd"}), false),
            Err(Refusal::OutOfScope(_) | Refusal::Sensitive(_))
        ));
    }

    #[test]
    fn migration_writes_the_expected_json_shape() {
        let mut cfg: McpConfig = serde_json::from_str(OLD_SCHEMA).unwrap();
        migrate_target_kinds(&mut cfg);
        let j = serde_json::to_string_pretty(&cfg).unwrap();
        assert!(j.contains("\"target_kind\": \"file\""), "{j}");
        assert!(j.contains("\"target_kind\": \"directory\""), "{j}");
        // Still disabled, still unscoped.
        assert!(j.contains("\"enabled\": false"));
        assert!(j.contains("\"roots\": []"));
        // Round trips.
        let back: McpConfig = serde_json::from_str(&j).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn config_round_trips_without_holding_a_secret() {
        let mut c = default_config();
        if let Transport::Stdio { env_from, .. } = &mut c.servers[0].transport {
            env_from.insert("API_TOKEN".into(), "NOKI_SOME_TOKEN_NAME".into());
        }
        let j = serde_json::to_string_pretty(&c).unwrap();
        let back: McpConfig = serde_json::from_str(&j).unwrap();
        assert_eq!(back, c);
        assert!(j.contains("NOKI_SOME_TOKEN_NAME"));
        assert!(j.contains("\"transport\": \"stdio\""));
    }
}
