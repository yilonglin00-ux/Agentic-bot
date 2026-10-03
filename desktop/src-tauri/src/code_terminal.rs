//! Noki Code: one persistent session per project, several views.
//!
//! A small host process (`noki code --serve`) owns the JackOD run, permission
//! prompts and the compact persistent history. The macOS terminal (`noki code`)
//! and the embedded Noki view are thin clients on a private Unix socket: output
//! is broadcast to every view, input goes through a single run lease. The coding
//! agent itself stays in `code_agent`.
use crate::{
    code_agent::{self, Action, CodeStyle},
    model_manager::{AssistantMode, ModelManager},
    web_gateway::WebGateway,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    hash::{Hash, Hasher},
    io::{self, BufRead, BufReader, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Condvar, Mutex, MutexGuard, OnceLock,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
    fn signal(sig: i32, handler: extern "C" fn(i32)) -> usize;
}

pub const MODEL_LABEL: &str = "JackOD 9B Coder";
/// A host without views and without a running task exits after this grace period
/// (history stays on disk; the next view starts a new host).
const HOST_IDLE_EXIT: Duration = Duration::from_secs(30);

const HISTORY_LIMIT: usize = 500;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CodeEvent {
    pub timestamp: u64,
    pub kind: String,
    pub summary: String,
    #[serde(default)]
    pub affected_files: Vec<String>,
    #[serde(default)]
    pub ok: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<CodeStyle>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CodeSession {
    pub project_root: String,
    pub session_id: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub style: CodeStyle,
    #[serde(default)]
    pub history: Vec<CodeEvent>,
    #[serde(default)]
    pub running: bool,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub last_result: Option<String>,
    /// True only while JackOD works on a task (shared by all views).
    #[serde(default)]
    pub busy: bool,
    #[serde(default)]
    pub terminal_views: u32,
    #[serde(default)]
    pub embedded_views: u32,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct CodeTerminalStatus {
    pub available: bool,
    /// A persistent session exists for this project.
    pub active: bool,
    /// The session host process is alive.
    pub host: bool,
    /// Kept for older frontends: same as `host`.
    pub running: bool,
    /// An external macOS terminal view is attached.
    pub terminal: bool,
    /// The embedded Noki view is attached.
    pub embedded: bool,
    pub busy: bool,
    pub project: String,
    pub session_id: String,
    pub style: String,
    pub last_result: String,
    pub model: String,
    pub runtime: String,
    pub history_len: usize,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn app_project() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

fn store_dir() -> PathBuf {
    app_project().join(".local/intelligence/code-sessions")
}

fn canonical_project(path: &Path) -> Result<PathBuf, String> {
    let p = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    p.canonicalize()
        .map_err(|e| format!("Projekt nicht gefunden: {e}"))
}

fn project_id(root: &Path) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    root.to_string_lossy().hash(&mut h);
    format!("{:016x}", h.finish())
}

fn session_path(root: &Path) -> PathBuf {
    store_dir().join(format!("{}.json", project_id(root)))
}

fn read_session(root: &Path) -> Option<CodeSession> {
    serde_json::from_slice(&fs::read(session_path(root)).ok()?).ok()
}

fn save_session(session: &CodeSession) -> Result<(), String> {
    let path = session_path(Path::new(&session.project_root));
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(
        &tmp,
        serde_json::to_vec_pretty(session).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::rename(tmp, path).map_err(|e| e.to_string())
}

fn new_session(root: &Path) -> CodeSession {
    let stamp = now();
    CodeSession {
        project_root: root.to_string_lossy().into_owned(),
        session_id: format!("{}-{}", project_id(root), stamp),
        created_at: stamp,
        updated_at: stamp,
        style: CodeStyle::Functional,
        history: Vec::new(),
        running: false,
        pid: None,
        last_result: None,
        busy: false,
        terminal_views: 0,
        embedded_views: 0,
    }
}

fn process_alive(pid: Option<u32>) -> bool {
    pid.is_some_and(|p| p > 0 && unsafe { kill(p as i32, 0) } == 0)
}

fn sock_path(root: &Path) -> PathBuf {
    store_dir().join(format!("{}.sock", project_id(root)))
}

fn lock_path(root: &Path) -> PathBuf {
    store_dir().join(format!("{}.lock", project_id(root)))
}

fn runtime_label() -> &'static str {
    if std::env::var("NOKI_LLM_RUNTIME").ok().as_deref() == Some("ollama") {
        "Ollama"
    } else {
        "llama.cpp · Metal"
    }
}

fn redact(raw: &str) -> String {
    let mut out = Vec::new();
    let mut private = false;
    for original in raw.lines() {
        let lower = original.to_lowercase();
        if lower.contains("-----begin ") && lower.contains("private key-----") {
            private = true;
            out.push("[REDACTED PRIVATE KEY]".to_string());
            continue;
        }
        if private {
            if lower.contains("-----end ") && lower.contains("private key-----") {
                private = false;
            }
            continue;
        }
        let sensitive_name = [
            "password",
            "passwd",
            "api_key",
            "apikey",
            "oauth",
            "authorization",
            "token",
            "ssh_key",
        ]
        .iter()
        .any(|key| lower.contains(key));
        if sensitive_name {
            if let Some(pos) = original.find('=').or_else(|| original.find(':')) {
                out.push(format!("{}=[REDACTED]", original[..pos].trim()));
                continue;
            }
        }
        let words = original
            .split_whitespace()
            .map(|word| {
                if word.starts_with("ghp_")
                    || word.starts_with("github_pat_")
                    || word.starts_with("sk-")
                    || word.starts_with("xoxb-")
                    || word.starts_with("xoxp-")
                {
                    "[REDACTED]"
                } else {
                    word
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        out.push(words);
    }
    out.join("\n").chars().take(1200).collect()
}

fn append(
    session: &mut CodeSession,
    kind: &str,
    summary: &str,
    files: Vec<String>,
    ok: Option<bool>,
) {
    session.history.push(CodeEvent {
        timestamp: now(),
        kind: kind.to_owned(),
        summary: redact(summary),
        affected_files: files.into_iter().map(|p| redact(&p)).collect(),
        ok,
        style: (kind == "User").then_some(session.style),
    });
    if session.history.len() > HISTORY_LIMIT {
        session
            .history
            .drain(..session.history.len() - HISTORY_LIMIT);
    }
    session.updated_at = now();
    let _ = save_session(session);
}

/// Maps real agent actions to compact log lines. A patch that follows a failed
/// test/patch is shown as REPAIR; tests always stay TEST with PASS/FAIL.
#[derive(Default)]
struct WorkLog {
    pending_failures: usize,
    last_test: Option<bool>,
}

impl WorkLog {
    fn kind(action: &Action) -> &'static str {
        let label = action.label.to_lowercase();
        if label.contains("analysiere") || label.contains("lese") {
            "INSPECT"
        } else if label.contains("suche") || label.starts_with("referenz") {
            "SEARCH"
        } else if label.contains("ändere") || label.contains("patch") {
            "PATCH"
        } else if label.starts_with('$')
            && (label.contains("test") || label.contains("check") || label.contains("build"))
        {
            "TEST"
        } else {
            "VERIFY"
        }
    }

    fn line(&mut self, action: &Action) -> (&'static str, String) {
        let label = action.label.trim().to_owned();
        match Self::kind(action) {
            "TEST" => {
                self.last_test = Some(action.ok);
                if action.ok {
                    self.pending_failures = 0;
                } else {
                    self.pending_failures += 1;
                }
                (
                    "TEST",
                    format!("{label} · {}", if action.ok { "PASS" } else { "FAIL" }),
                )
            }
            "PATCH" if !action.ok => {
                self.pending_failures += 1;
                ("REPAIR", format!("{label} · Patch abgelehnt"))
            }
            "PATCH" if self.pending_failures > 0 => {
                let n = std::mem::take(&mut self.pending_failures);
                ("REPAIR", format!("{label} · {n} Fehler"))
            }
            kind if !action.ok => (kind, format!("{label} · FAIL")),
            kind => (kind, label),
        }
    }

    fn result(&self, actions: &[Action]) -> &'static str {
        match self.last_test {
            Some(true) => "PASS",
            Some(false) => "FAIL",
            None if actions.iter().any(|a| !a.ok) => "WARN",
            None => "PASS",
        }
    }
}

fn diff_state(root: &Path) -> BTreeMap<String, String> {
    let output = Command::new("git")
        .args(["diff", "--numstat"])
        .current_dir(root)
        .output();
    let Ok(output) = output else {
        return BTreeMap::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let a = parts.next()?;
            let d = parts.next()?;
            let p = parts.next()?;
            Some((p.to_owned(), format!("{a}/{d}")))
        })
        .collect()
}

fn changed_since(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> Vec<String> {
    after
        .iter()
        .filter(|(path, stat)| before.get(*path) != Some(*stat))
        .map(|(path, _)| path.clone())
        .collect()
}

fn history_context(session: &CodeSession) -> String {
    session
        .history
        .iter()
        .rev()
        .take(24)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|e| format!("[{}] {}", e.kind, e.summary))
        .collect::<Vec<_>>()
        .join("\n")
}

fn home_display(path: &str) -> String {
    std::env::var("HOME")
        .ok()
        .and_then(|home| path.strip_prefix(&home).map(|rest| format!("~{rest}")))
        .unwrap_or_else(|| path.to_owned())
}

fn style_label(style: CodeStyle) -> &'static str {
    match style {
        CodeStyle::Functional => "Funktional",
        CodeStyle::Creative => "Kreativ",
    }
}

fn terminal_title(root: &Path) -> String {
    let name = root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("Projekt");
    format!("Noki Code — {name} — {}", project_id(root))
}

fn local_offset() -> i64 {
    static OFFSET: OnceLock<i64> = OnceLock::new();
    *OFFSET.get_or_init(|| {
        let raw = Command::new("/bin/date")
            .arg("+%z")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .unwrap_or_default();
        let (sign, digits) = match raw.strip_prefix('-') {
            Some(d) => (-1, d),
            None => (1, raw.trim_start_matches('+')),
        };
        let h = digits
            .get(0..2)
            .and_then(|x| x.parse::<i64>().ok())
            .unwrap_or(0);
        let m = digits
            .get(2..4)
            .and_then(|x| x.parse::<i64>().ok())
            .unwrap_or(0);
        sign * (h * 3600 + m * 60)
    })
}

fn clock(ts: u64) -> String {
    let local = ts as i64 + local_offset();
    format!(
        "{:02}:{:02}",
        local.rem_euclid(86_400) / 3600,
        local.rem_euclid(3600) / 60
    )
}

fn day_label(ts: u64) -> String {
    let day = (ts as i64 + local_offset()).div_euclid(86_400);
    let today = (now() as i64 + local_offset()).div_euclid(86_400);
    match today - day {
        0 => "Heute".into(),
        1 => "Gestern".into(),
        n => format!("vor {n} Tagen"),
    }
}

fn confirm_label(tool: &str) -> &'static str {
    match tool {
        "fs.patch" => "Repository-Dateien ändern",
        "shell.test" => "Tests ausführen",
        "shell.build" => "Build ausführen",
        _ => "eine geschützte Aktion ausführen",
    }
}

fn confirm(reader: &mut impl BufRead, question: &str) -> Result<bool, String> {
    println!("{question}");
    print!("› ");
    io::stdout().flush().map_err(|e| e.to_string())?;
    let mut answer = String::new();
    reader.read_line(&mut answer).map_err(|e| e.to_string())?;
    Ok(matches!(
        answer.trim().to_lowercase().as_str(),
        "y" | "yes" | "j" | "ja"
    ))
}

fn web_enabled() -> bool {
    let path = app_project().join(".local/intelligence/settings.json");
    fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v.get("web").and_then(|x| x.as_bool()))
        .unwrap_or(false)
}

fn history_lines(session: &CodeSession) -> Vec<String> {
    if session.history.is_empty() {
        return vec!["Noch kein gespeicherter Code-Verlauf.".into()];
    }
    let mut out = Vec::new();
    let mut day = String::new();
    for event in &session.history {
        let label = day_label(event.timestamp);
        if label != day {
            out.push(format!("── {label}"));
            day = label;
        }
        let (glyph, name, _) = glyph(&event.kind, event.ok);
        let first = event.summary.lines().next().unwrap_or("");
        out.push(format!(
            "{}  {glyph} {name:<8} {first}",
            clock(event.timestamp)
        ));
        if !event.affected_files.is_empty() {
            out.push(format!(
                "                  {}",
                event.affected_files.join(", ")
            ));
        }
    }
    out
}

fn session_lines() -> Vec<String> {
    let Ok(rows) = fs::read_dir(store_dir()) else {
        return vec!["Keine Code-Sessions vorhanden.".into()];
    };
    let mut sessions = rows
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| fs::read(e.path()).ok())
        .filter_map(|b| serde_json::from_slice::<CodeSession>(&b).ok())
        .collect::<Vec<_>>();
    sessions.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
    sessions
        .iter()
        .map(|s| {
            format!(
                "{}  {}  {}  {} Einträge  {}",
                short_id(&s.session_id),
                day_label(s.updated_at),
                style_label(s.style),
                s.history.len(),
                home_display(&s.project_root)
            )
        })
        .collect()
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

const HELP: &str =
    "/functional  /creative  /status  /history  /sessions  /clear  /clear-history  /help  /exit";

/// Glyph, label and ANSI colour per work-log event type (shared by CLI and history).
fn glyph(kind: &str, ok: Option<bool>) -> (&'static str, &'static str, &'static str) {
    match kind {
        "User" => ("›", "Du", "1"),
        "INSPECT" => ("◇", "Inspect", "36"),
        "SEARCH" => ("⌕", "Search", "36"),
        "PLAN" => ("◆", "Plan", "35"),
        "PATCH" => ("✎", "Patch", "33"),
        "TEST" if ok == Some(false) => ("✗", "Test", "31"),
        "TEST" if ok == Some(true) => ("✓", "Test", "32"),
        "TEST" => ("▶", "Test", "34"),
        "REPAIR" => ("↻", "Repair", "33"),
        "VERIFY" => ("✓", "Verify", "32"),
        "DONE" => ("●", "Done", "32"),
        "CONFIRM" => ("⚿", "Confirm", "33"),
        "WARNING" => ("!", "Warning", "33"),
        "ERROR" => ("×", "Error", "31"),
        _ => ("·", "Event", "37"),
    }
}

fn render_event(event: &CodeEvent, dim: bool) -> String {
    let (g, label, color) = glyph(&event.kind, event.ok);
    let mut lines = event.summary.lines();
    let first = lines.next().unwrap_or("");
    let body = if dim { "\x1b[2m" } else { "" };
    let mut out = if event.kind == "User" {
        format!("\n\x1b[1m› {first}\x1b[0m")
    } else {
        format!("  \x1b[{color}m{g} {label:<8}\x1b[0m {body}{first}\x1b[0m")
    };
    for line in lines {
        out.push_str(&format!("\n             {body}{line}\x1b[0m"));
    }
    if !event.affected_files.is_empty() {
        out.push_str(&format!(
            "\n             \x1b[2m{}\x1b[0m",
            event.affected_files.join("  ")
        ));
    }
    out
}

// ───────────────────────────── session host ─────────────────────────────

struct Client {
    id: u64,
    view: String,
    out: Mutex<UnixStream>,
}

impl Client {
    fn send(&self, msg: &Value) -> bool {
        let mut line = msg.to_string();
        line.push('\n');
        self.out
            .lock()
            .map(|mut s| s.write_all(line.as_bytes()).is_ok())
            .unwrap_or(false)
    }
}

struct Pending {
    id: u64,
    items: Vec<String>,
    answer: Option<bool>,
}

struct HubState {
    session: CodeSession,
    clients: Vec<Arc<Client>>,
    /// Run lease: the view whose input started the current task owns its prompts.
    owner: Option<u64>,
    confirm: Option<Pending>,
    seq: u64,
    model_state: String,
    idle_since: Instant,
}

impl HubState {
    fn view_of(&self, id: u64) -> Option<String> {
        self.clients
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.view.clone())
    }

    fn meta(&self) -> Value {
        let s = &self.session;
        json!({
            "t": "session",
            "project": home_display(&s.project_root),
            "project_root": s.project_root,
            "session_id": s.session_id,
            "style": s.style.as_str(),
            "busy": s.busy,
            "last_result": s.last_result,
            "model": MODEL_LABEL,
            "runtime": runtime_label(),
            "model_state": self.model_state,
            "owner_view": self.owner.and_then(|o| self.view_of(o)),
            "terminal_views": s.terminal_views,
            "embedded_views": s.embedded_views,
            "history_len": s.history.len(),
            "confirm_pending": self.confirm.as_ref().is_some_and(|p| p.answer.is_none()),
        })
    }

    fn broadcast(&mut self, msg: &Value) {
        let before = self.clients.len();
        self.clients.retain(|c| c.send(msg));
        if self.clients.len() != before {
            self.count_views();
        }
    }

    fn publish(&mut self) {
        let meta = self.meta();
        self.broadcast(&meta);
    }

    fn to_client(&mut self, id: u64, msg: &Value) {
        if let Some(c) = self.clients.iter().find(|c| c.id == id).cloned() {
            if !c.send(msg) {
                self.clients.retain(|x| x.id != id);
                self.count_views();
            }
        }
    }

    fn count_views(&mut self) {
        let count = |v: &str| self.clients.iter().filter(|c| c.view == v).count() as u32;
        let (t, e) = (count("terminal"), count("embedded"));
        self.session.terminal_views = t;
        self.session.embedded_views = e;
        if self.clients.is_empty() {
            self.idle_since = Instant::now();
        }
        let _ = save_session(&self.session);
    }

    fn event(
        &mut self,
        kind: &str,
        summary: &str,
        files: Vec<String>,
        ok: Option<bool>,
        origin: Option<String>,
    ) {
        append(&mut self.session, kind, summary, files, ok);
        if let Some(event) = self.session.history.last().cloned() {
            self.broadcast(&json!({"t": "event", "event": event, "origin": origin}));
        }
    }

    /// Sends an open prompt to its audience: the lease owner, or every view when
    /// the owner has left. Other views only see that a confirmation is waiting.
    fn offer_confirm(&mut self) {
        let Some(p) = self.confirm.as_ref().filter(|p| p.answer.is_none()) else {
            return;
        };
        if self
            .owner
            .is_some_and(|o| !self.clients.iter().any(|c| c.id == o))
        {
            self.owner = None;
        }
        let msg = |mine: bool| json!({"t": "confirm", "id": p.id, "items": p.items, "mine": mine});
        let (own, wait) = (msg(true), msg(false));
        let owner = self.owner;
        let before = self.clients.len();
        self.clients.retain(|c| {
            c.send(if owner.is_none() || owner == Some(c.id) {
                &own
            } else {
                &wait
            })
        });
        if self.clients.len() != before {
            self.count_views();
        }
    }
}

struct Hub {
    state: Mutex<HubState>,
    wake: Condvar,
    cancel: AtomicBool,
    tasks: mpsc::Sender<String>,
}

impl Hub {
    fn new(session: CodeSession, tasks: mpsc::Sender<String>) -> Self {
        Hub {
            state: Mutex::new(HubState {
                session,
                clients: Vec::new(),
                owner: None,
                confirm: None,
                seq: 0,
                model_state: "loading".into(),
                idle_since: Instant::now(),
            }),
            wake: Condvar::new(),
            cancel: AtomicBool::new(false),
            tasks,
        }
    }

    fn lock(&self) -> MutexGuard<'_, HubState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn snapshot(&self, id: u64) {
        let mut s = self.lock();
        let msg = json!({"t": "snapshot", "session": s.meta(), "history": s.session.history});
        s.to_client(id, &msg);
    }

    fn attach(&self, client: Arc<Client>) {
        let id = client.id;
        {
            let mut s = self.lock();
            s.clients.push(client);
            s.count_views();
        }
        self.snapshot(id);
        let mut s = self.lock();
        s.publish();
        s.offer_confirm();
    }

    fn detach(&self, id: u64) {
        let mut s = self.lock();
        s.clients.retain(|c| c.id != id);
        s.count_views();
        if s.owner == Some(id) {
            s.owner = None;
            s.offer_confirm();
        }
        s.publish();
    }

    fn notice(&self, id: Option<u64>, level: &str, text: &str) {
        let msg = json!({"t": "notice", "level": level, "text": text});
        let mut s = self.lock();
        match id {
            Some(id) => s.to_client(id, &msg),
            None => s.broadcast(&msg),
        }
    }

    fn info(&self, id: u64, lines: Vec<String>) {
        self.lock()
            .to_client(id, &json!({"t": "info", "lines": lines}));
    }

    fn status_lines(&self) -> Vec<String> {
        let loaded = ModelManager::default()
            .loaded_large_models()
            .map(|m| m.len());
        let s = self.lock();
        vec![
            "Mode: Code".into(),
            format!("Model: {MODEL_LABEL}"),
            format!("Style: {}", style_label(s.session.style)),
            format!("Project: {}", home_display(&s.session.project_root)),
            format!("Session: {}", s.session.session_id),
            format!("Runtime: {}", runtime_label()),
            format!(
                "Loaded models: {}",
                loaded.map(|n| n.to_string()).unwrap_or_else(|e| e)
            ),
            format!(
                "Views: Terminal {} · Noki {}",
                s.session.terminal_views, s.session.embedded_views
            ),
            format!(
                "Status: {}",
                if s.session.busy {
                    "JackOD arbeitet"
                } else {
                    "Bereit"
                }
            ),
        ]
    }

    fn set_style(&self, style: CodeStyle) {
        let mut s = self.lock();
        s.session.style = style;
        s.session.updated_at = now();
        let _ = save_session(&s.session);
        s.publish();
        s.broadcast(&json!({"t": "notice", "level": "info", "text": format!("Style: {}", style_label(style))}));
    }

    fn input(&self, id: u64, raw: &str) {
        let text: String = raw.trim().chars().take(4000).collect();
        match text.as_str() {
            "" | "/clear" | "/exit" | "/quit" => {}
            "/functional" => self.set_style(CodeStyle::Functional),
            "/creative" => self.set_style(CodeStyle::Creative),
            "/status" => {
                let lines = self.status_lines();
                self.info(id, lines);
            }
            "/history" => {
                let lines = history_lines(&self.lock().session);
                self.info(id, lines);
            }
            "/sessions" | "/session" => self.info(id, session_lines()),
            "/help" => self.info(id, vec![HELP.into()]),
            "/clear-history" => self.info(
                id,
                vec!["Verlauf löschen braucht eine Bestätigung in der Ansicht.".into()],
            ),
            command if command.starts_with('/') => self.info(
                id,
                vec!["Unbekannter Befehl. /help zeigt alle Befehle.".into()],
            ),
            task => {
                let mut s = self.lock();
                if s.session.busy {
                    let who = match s.owner.and_then(|o| s.view_of(o)).as_deref() {
                        Some("embedded") => "der Noki-Ansicht",
                        Some(_) => "dem Terminal",
                        None => "einer geschlossenen Ansicht",
                    };
                    let msg = json!({"t": "notice", "level": "warn",
                        "text": format!("JackOD arbeitet bereits an einer Aufgabe aus {who}. Eingabe nicht gesendet.")});
                    s.to_client(id, &msg);
                    return;
                }
                s.session.busy = true;
                s.owner = Some(id);
                self.cancel.store(false, Ordering::SeqCst);
                let origin = s.view_of(id);
                s.event("User", task, Vec::new(), None, origin);
                s.publish();
                drop(s);
                let _ = self.tasks.send(task.to_owned());
            }
        }
    }

    fn answer(&self, from: u64, id: u64, yes: bool) {
        let mut s = self.lock();
        let allowed = s.owner.is_none() || s.owner == Some(from);
        let open = s
            .confirm
            .as_ref()
            .is_some_and(|p| p.id == id && p.answer.is_none());
        if !open {
            return;
        }
        if !allowed {
            let msg = json!({"t": "notice", "level": "warn", "text": "Diese Bestätigung gehört zur anderen Ansicht."});
            s.to_client(from, &msg);
            return;
        }
        if let Some(p) = s.confirm.as_mut() {
            p.answer = Some(yes);
        }
        self.wake.notify_all();
    }

    fn cancel(&self) {
        let mut s = self.lock();
        if s.session.busy && !self.cancel.swap(true, Ordering::SeqCst) {
            self.wake.notify_all();
            s.broadcast(&json!({"t": "notice", "level": "warn", "text": "Abbruch angefordert."}));
        }
    }

    fn clear_history(&self, id: u64) {
        let mut s = self.lock();
        if s.session.busy {
            let msg = json!({"t": "notice", "level": "warn", "text": "Während JackOD arbeitet, bleibt der Verlauf unverändert."});
            s.to_client(id, &msg);
            return;
        }
        s.session.history.clear();
        s.session.last_result = None;
        s.session.updated_at = now();
        let _ = save_session(&s.session);
        s.broadcast(&json!({"t": "history_cleared"}));
        s.publish();
    }

    /// R2/R3 prompt for the unchanged agent permission callback. Blocks the run
    /// until the owning view answers (default: no on cancel).
    fn authorize(&self, tool: &str) -> bool {
        if self.cancel.load(Ordering::SeqCst) {
            return false;
        }
        let item = confirm_label(tool).to_owned();
        let mut s = self.lock();
        s.seq += 1;
        let id = s.seq;
        s.confirm = Some(Pending {
            id,
            items: vec![item.clone()],
            answer: None,
        });
        s.offer_confirm();
        s.publish();
        let yes = loop {
            if let Some(answer) = s.confirm.as_ref().and_then(|p| p.answer) {
                break answer;
            }
            if self.cancel.load(Ordering::SeqCst) {
                break false;
            }
            s = self
                .wake
                .wait_timeout(s, Duration::from_millis(500))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        };
        s.confirm = None;
        s.broadcast(&json!({"t": "confirm_done", "id": id, "yes": yes}));
        let summary = format!("{item} · {}", if yes { "bestätigt" } else { "abgelehnt" });
        s.event("CONFIRM", &summary, Vec::new(), Some(yes), None);
        s.publish();
        yes
    }
}

fn numstat_label(stat: &str) -> String {
    let (a, d) = stat.split_once('/').unwrap_or((stat, "0"));
    format!("+{a} −{d}")
}

fn run_task(hub: &Hub, manager: &mut ModelManager, question: &str) {
    let (root, history, style) = {
        let mut s = hub.lock();
        s.event(
            "PLAN",
            "Aufgabe wird lokal geplant und schrittweise geprüft.",
            Vec::new(),
            None,
            None,
        );
        (
            PathBuf::from(&s.session.project_root),
            history_context(&s.session),
            s.session.style,
        )
    };
    let before = diff_state(&root);
    let mut seen = before.clone();
    let gateway = Arc::new(WebGateway::new());
    let mut approved = HashSet::<String>::new();
    let mut log = WorkLog::default();
    let result = code_agent::run_authorized(
        &root,
        question,
        &history,
        style,
        Some(&gateway),
        web_enabled(),
        |tool| {
            if approved.contains(tool) {
                return Ok(true);
            }
            let yes = hub.authorize(tool);
            if yes {
                approved.insert(tool.to_owned());
            }
            Ok(yes)
        },
        |prompt, max, constrained| {
            if hub.cancel.load(Ordering::SeqCst) {
                return Err("Abgebrochen.".into());
            }
            if constrained {
                manager
                    .generate_json_schema(
                        AssistantMode::Code,
                        prompt,
                        max,
                        code_agent::action_schema(),
                        &hub.cancel,
                    )
                    .or_else(|_| manager.generate(AssistantMode::Code, prompt, max, &hub.cancel))
            } else {
                manager.generate(AssistantMode::Code, prompt, max, &hub.cancel)
            }
        },
        |action| {
            let (kind, mut summary) = log.line(action);
            let mut files = Vec::new();
            if matches!(kind, "PATCH" | "REPAIR") && action.ok {
                let now_state = diff_state(&root);
                for path in changed_since(&seen, &now_state) {
                    files.push(format!("{path} {}", numstat_label(&now_state[&path])));
                }
                if !files.is_empty() {
                    summary = format!("{summary} · {} Datei(en)", files.len());
                }
                seen = now_state;
            }
            hub.lock()
                .event(kind, &summary, files, Some(action.ok), None);
        },
    );
    let files = changed_since(&before, &diff_state(&root));
    let cancelled = hub.cancel.load(Ordering::SeqCst);
    let mut s = hub.lock();
    match result {
        Ok(run) => {
            let verify = if files.is_empty() {
                "Arbeitsstand geprüft".to_owned()
            } else {
                format!("Diff geprüft · {} Datei(en)", files.len())
            };
            s.event("VERIFY", &verify, files, Some(true), None);
            s.event("DONE", &run.answer, Vec::new(), Some(true), None);
            s.session.last_result = Some(log.result(&run.actions).into());
        }
        Err(_) if cancelled => {
            s.event(
                "WARNING",
                "Abgebrochen. Keine weitere Aktion wurde ausgeführt.",
                Vec::new(),
                Some(false),
                None,
            );
            s.session.last_result = Some("CANCELLED".into());
        }
        Err(error) => {
            let invalid = error.contains("gültige, vollständige Aktion")
                || error.contains("strukturierte Aktion");
            let message = if invalid {
                "Ungültige Agent-Aktion. Keine weitere Aktion wurde ausgeführt.".to_owned()
            } else {
                redact(&error)
            };
            s.event("ERROR", &message, Vec::new(), Some(false), None);
            s.session.last_result = Some("ERROR".into());
        }
    }
    s.session.busy = false;
    s.owner = None;
    s.session.updated_at = now();
    let _ = save_session(&s.session);
    s.publish();
    hub.cancel.store(false, Ordering::SeqCst);
}

fn client_loop(hub: Arc<Hub>, id: u64, stream: UnixStream) {
    let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));
    let Ok(out) = stream.try_clone() else { return };
    let mut reader = BufReader::new(stream);
    let mut first = String::new();
    if reader.read_line(&mut first).unwrap_or(0) == 0 {
        return;
    }
    let hello: Value = serde_json::from_str(&first).unwrap_or_default();
    if hello["t"] != "hello" {
        return;
    }
    let view = if hello["view"] == "embedded" {
        "embedded"
    } else {
        "terminal"
    };
    hub.attach(Arc::new(Client {
        id,
        view: view.into(),
        out: Mutex::new(out),
    }));
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match msg["t"].as_str().unwrap_or("") {
            "input" => hub.input(id, msg["text"].as_str().unwrap_or("")),
            "confirm" => hub.answer(id, msg["id"].as_u64().unwrap_or(0), msg["yes"] == true),
            "cancel" => hub.cancel(),
            "style" => match msg["style"].as_str() {
                Some("creative") => hub.set_style(CodeStyle::Creative),
                Some("functional") => hub.set_style(CodeStyle::Functional),
                _ => {}
            },
            "clear_history" => hub.clear_history(id),
            "snapshot" => hub.snapshot(id),
            _ => {}
        }
    }
    hub.detach(id);
}

/// `noki code --serve`: the single session owner for one project.
pub fn serve(project: PathBuf) -> Result<(), String> {
    let root = canonical_project(&project)?;
    fs::create_dir_all(store_dir()).map_err(|e| e.to_string())?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path(&root))
        .map_err(|e| e.to_string())?;
    if lock.try_lock().is_err() {
        return Ok(()); // another host already owns this project session
    }
    let sock = sock_path(&root);
    let _ = fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).map_err(|e| e.to_string())?;
    let _ = fs::set_permissions(&sock, fs::Permissions::from_mode(0o600));

    let mut session = read_session(&root).unwrap_or_else(|| new_session(&root));
    session.running = true;
    session.pid = Some(std::process::id());
    session.busy = false;
    session.terminal_views = 0;
    session.embedded_views = 0;
    save_session(&session)?;

    let (tx, rx) = mpsc::channel::<String>();
    let hub = Arc::new(Hub::new(session, tx));
    {
        let hub = hub.clone();
        thread::spawn(move || {
            let mut manager = ModelManager::default();
            let state = match manager.switch(AssistantMode::Code) {
                Ok(_) => "ready".to_owned(),
                Err(e) => {
                    hub.notice(None, "error", &redact(&e));
                    "error".to_owned()
                }
            };
            {
                let mut s = hub.lock();
                s.model_state = state;
                s.publish();
            }
            for question in rx {
                run_task(&hub, &mut manager, &question);
            }
        });
    }
    {
        let hub = hub.clone();
        let sock = sock.clone();
        thread::spawn(move || loop {
            thread::sleep(Duration::from_secs(1));
            let mut s = hub.lock();
            if s.clients.is_empty() && !s.session.busy && s.idle_since.elapsed() > HOST_IDLE_EXIT {
                let _ = fs::remove_file(&sock);
                s.session.running = false;
                s.session.pid = None;
                let _ = save_session(&s.session);
                std::process::exit(0);
            }
        });
    }
    let ids = AtomicU64::new(1);
    for stream in listener.incoming().flatten() {
        let hub = hub.clone();
        let id = ids.fetch_add(1, Ordering::SeqCst);
        thread::spawn(move || client_loop(hub, id, stream));
    }
    drop(lock);
    Ok(())
}

// ───────────────────────────── view clients ─────────────────────────────

fn write_msg(stream: &UnixStream, msg: &Value) -> Result<(), String> {
    let mut line = msg.to_string();
    line.push('\n');
    (&*stream)
        .write_all(line.as_bytes())
        .map_err(|e| e.to_string())
}

fn spawn_host(binary: &Path, root: &Path) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let log = fs::File::create(store_dir().join(format!("{}.log", project_id(root))))
        .map(Stdio::from)
        .unwrap_or_else(|_| Stdio::null());
    let mut child = Command::new("/usr/bin/nohup")
        .arg(binary)
        .args(["code", "--serve", "--project"])
        .arg(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log)
        .process_group(0)
        .spawn()
        .map_err(|e| format!("Noki-Code-Session konnte nicht starten: {e}"))?;
    thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Connects to the project's session host, starting it when none is running.
fn connect(root: &Path) -> Result<UnixStream, String> {
    let sock = sock_path(root);
    if let Ok(stream) = UnixStream::connect(&sock) {
        return Ok(stream);
    }
    fs::create_dir_all(store_dir()).map_err(|e| e.to_string())?;
    let binary = cli_binary().ok_or("Noki-Code-CLI fehlt. Bitte Noki neu bauen.")?;
    let started = Instant::now();
    let mut spawned: Option<Instant> = None;
    loop {
        if spawned.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) {
            spawn_host(&binary, root)?;
            spawned = Some(Instant::now());
        }
        thread::sleep(Duration::from_millis(100));
        if let Ok(stream) = UnixStream::connect(&sock) {
            return Ok(stream);
        }
        if started.elapsed() > Duration::from_secs(12) {
            return Err("Noki-Code-Session antwortet nicht.".into());
        }
    }
}

static VIEW: Mutex<Option<UnixStream>> = Mutex::new(None);
static VIEW_GEN: AtomicU64 = AtomicU64::new(0);

/// Embedded Noki view: attaches to the same host and forwards every host
/// message to the frontend.
pub fn view_attach(project: &Path, emit: impl Fn(Value) + Send + 'static) -> Result<(), String> {
    let root = canonical_project(project)?;
    let stream = connect(&root)?;
    write_msg(&stream, &json!({"t": "hello", "view": "embedded"}))?;
    let reader = stream.try_clone().map_err(|e| e.to_string())?;
    let generation = VIEW_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    if let Some(old) = VIEW.lock().map_err(|e| e.to_string())?.replace(stream) {
        let _ = old.shutdown(std::net::Shutdown::Both);
    }
    thread::spawn(move || {
        for line in BufReader::new(reader).lines() {
            let Ok(line) = line else { break };
            if VIEW_GEN.load(Ordering::SeqCst) != generation {
                return;
            }
            if let Ok(msg) = serde_json::from_str::<Value>(&line) {
                emit(msg);
            }
        }
        if VIEW_GEN.load(Ordering::SeqCst) == generation {
            if let Ok(mut view) = VIEW.lock() {
                *view = None;
            }
            emit(json!({"t": "detached"}));
        }
    });
    Ok(())
}

pub fn view_send(msg: Value) -> Result<(), String> {
    let kind = msg["t"].as_str().unwrap_or("");
    if ![
        "input",
        "confirm",
        "cancel",
        "style",
        "clear_history",
        "snapshot",
    ]
    .contains(&kind)
    {
        return Err("Unbekannte Code-Nachricht.".into());
    }
    let view = VIEW.lock().map_err(|e| e.to_string())?;
    let stream = view.as_ref().ok_or("Code-Ansicht ist nicht verbunden.")?;
    write_msg(stream, &msg)
}

pub fn view_detach() {
    VIEW_GEN.fetch_add(1, Ordering::SeqCst);
    if let Some(stream) = VIEW.lock().ok().and_then(|mut v| v.take()) {
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }
}

// ─────────────────────────── macOS terminal view ───────────────────────────

pub const BANNER: &str = "\
███╗   ██╗ ██████╗ ██╗  ██╗██╗
████╗  ██║██╔═══██╗██║ ██╔╝██║
██╔██╗ ██║██║   ██║█████╔╝ ██║
██║╚██╗██║██║   ██║██╔═██╗ ██║
██║ ╚████║╚██████╔╝██║  ██╗██║
╚═╝  ╚═══╝ ╚═════╝ ╚═╝  ╚═╝╚═╝";

#[derive(Default)]
struct CliState {
    out: Mutex<Option<UnixStream>>,
    pending: Mutex<Option<u64>>,
    meta: Mutex<Value>,
}

impl CliState {
    fn send(&self, msg: Value) {
        if let Some(stream) = self.out.lock().ok().as_ref().and_then(|o| o.as_ref()) {
            let _ = write_msg(stream, &msg);
        }
    }
    fn busy(&self) -> bool {
        self.meta.lock().map(|m| m["busy"] == true).unwrap_or(false)
    }
}

fn prompt() {
    print!("\x1b[38;5;180m›\x1b[0m ");
    let _ = io::stdout().flush();
}

fn print_header(meta: &Value) {
    let style = if meta["style"] == "creative" {
        "Kreativ"
    } else {
        "Funktional"
    };
    println!("\x1b[38;5;180m{BANNER}\x1b[0m");
    println!("\x1b[1mNOKI CODE\x1b[0m  \x1b[2mPowered by {MODEL_LABEL}\x1b[0m");
    println!();
    println!(
        "\x1b[38;5;180m◉\x1b[0m NOKI CODE   ◆ JackOD 9B   ⚡ {}   ◇ {style}",
        meta["runtime"].as_str().unwrap_or("llama.cpp · Metal")
    );
    println!(
        "\x1b[2m{} · Session #{}\x1b[0m",
        meta["project"].as_str().unwrap_or(""),
        short_id(meta["session_id"].as_str().unwrap_or(""))
    );
    println!("\x1b[2m{}\x1b[0m", "─".repeat(56));
}

fn render_host_message(state: &CliState, msg: &Value) {
    match msg["t"].as_str().unwrap_or("") {
        "snapshot" => {
            let meta = msg["session"].clone();
            print_header(&meta);
            let history: Vec<CodeEvent> =
                serde_json::from_value(msg["history"].clone()).unwrap_or_default();
            if !history.is_empty() {
                println!(
                    "\x1b[2mZuletzt ({} Einträge gespeichert · /history zeigt alles)\x1b[0m",
                    history.len()
                );
                for event in history.iter().rev().take(8).rev() {
                    println!("{}", render_event(event, true));
                }
                println!("\x1b[2m{}\x1b[0m", "─".repeat(56));
            }
            if meta["busy"] == true {
                println!("\x1b[38;5;180m◉ JackOD arbeitet …\x1b[0m");
            }
            *state.meta.lock().unwrap() = meta;
            println!();
            prompt();
        }
        "session" => {
            let was_busy = state.busy();
            let busy = msg["busy"] == true;
            *state.meta.lock().unwrap() = msg.clone();
            if was_busy && !busy {
                println!();
                prompt();
            }
        }
        "event" => {
            let Ok(event) = serde_json::from_value::<CodeEvent>(msg["event"].clone()) else {
                return;
            };
            if event.kind == "User" {
                let from = if msg["origin"] == "embedded" {
                    "  \x1b[2m(aus Noki)\x1b[0m"
                } else {
                    ""
                };
                println!("\r{}{from}", render_event(&event, false));
                println!("\x1b[38;5;180m◉ JackOD arbeitet …\x1b[0m");
            } else {
                println!("\r{}", render_event(&event, false));
            }
        }
        "confirm" => {
            let items: Vec<String> =
                serde_json::from_value(msg["items"].clone()).unwrap_or_default();
            if msg["mine"] == true {
                let mut pending = state.pending.lock().unwrap();
                if *pending == msg["id"].as_u64() {
                    return; // already shown (host re-offers open prompts on attach)
                }
                *pending = msg["id"].as_u64();
                drop(pending);
                println!(
                    "\n\x1b[33m┌ Bestätigung erforderlich {}\x1b[0m",
                    "─".repeat(28)
                );
                println!("\x1b[33m│\x1b[0m Noki möchte:");
                for item in &items {
                    println!("\x1b[33m│\x1b[0m • {item}");
                }
                println!("\x1b[33m└{}\x1b[0m", "─".repeat(54));
                println!("Ausführen? [y/N]");
                prompt();
            } else {
                println!(
                    "  \x1b[33m⧗ Bestätigung wartet in der Noki-Ansicht:\x1b[0m {}",
                    items.join(", ")
                );
            }
        }
        "confirm_done" => {
            let mut pending = state.pending.lock().unwrap();
            if *pending == msg["id"].as_u64() {
                *pending = None;
            }
        }
        "info" => {
            println!();
            for line in msg["lines"].as_array().into_iter().flatten() {
                println!("{}", line.as_str().unwrap_or(""));
            }
            println!();
            prompt();
        }
        "notice" => {
            let color = match msg["level"].as_str() {
                Some("error") => "31",
                Some("warn") => "33",
                _ => "2",
            };
            println!(
                "\r  \x1b[{color}m{}\x1b[0m",
                msg["text"].as_str().unwrap_or("")
            );
            if !state.busy() {
                prompt();
            }
        }
        "history_cleared" => {
            println!("\r  \x1b[2mCode-Verlauf gelöscht.\x1b[0m");
            prompt();
        }
        _ => {}
    }
}

static SIGINT: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sigint(_: i32) {
    SIGINT.store(true, Ordering::SeqCst);
}

fn terminal_hello(root: &Path, state: &CliState) -> Result<UnixStream, String> {
    let stream = connect(root)?;
    write_msg(&stream, &json!({"t": "hello", "view": "terminal"}))?;
    let reader = stream.try_clone().map_err(|e| e.to_string())?;
    *state.out.lock().unwrap() = Some(stream);
    Ok(reader)
}

pub fn run_cli(project: PathBuf) -> Result<(), String> {
    let root = canonical_project(&project)?;
    if let Some(s) = read_session(&root) {
        if s.running
            && process_alive(s.pid)
            && s.terminal_views > 0
            && focus_existing(&root).unwrap_or(false)
        {
            println!(
                "Noki Code läuft bereits für {} – Terminal fokussiert.",
                home_display(&s.project_root)
            );
            return Ok(());
        }
    }
    print!("\x1b]0;{}\x07", terminal_title(&root));
    let state = Arc::new(CliState::default());
    let mut reader = terminal_hello(&root, &state)?;
    {
        let state = state.clone();
        let root = root.clone();
        thread::spawn(move || loop {
            for line in BufReader::new(&reader).lines() {
                let Ok(line) = line else { break };
                if let Ok(msg) = serde_json::from_str::<Value>(&line) {
                    render_host_message(&state, &msg);
                }
            }
            println!("\r  \x1b[33mVerbindung zur Session getrennt – verbinde neu …\x1b[0m");
            thread::sleep(Duration::from_millis(300));
            match terminal_hello(&root, &state) {
                Ok(next) => reader = next,
                Err(e) => {
                    eprintln!("[ERROR] {e}");
                    std::process::exit(1);
                }
            }
        });
    }
    unsafe {
        signal(2, on_sigint);
    }
    {
        let state = state.clone();
        thread::spawn(move || {
            let mut last: Option<Instant> = None;
            loop {
                thread::sleep(Duration::from_millis(100));
                if !SIGINT.swap(false, Ordering::SeqCst) {
                    continue;
                }
                if state.busy() {
                    state.send(json!({"t": "cancel"}));
                    println!("\n  \x1b[33m⌃C Abbruch angefordert\x1b[0m");
                } else if last.is_some_and(|t| t.elapsed() < Duration::from_secs(2)) {
                    println!("\nSession bleibt erhalten.");
                    let _ = io::stdout().flush();
                    std::process::exit(0);
                } else {
                    last = Some(Instant::now());
                    println!("\n  \x1b[2m⌃C erneut oder /exit beendet die Ansicht (Session bleibt).\x1b[0m");
                    prompt();
                }
            }
        });
    }

    let stdin = io::stdin();
    let mut input = stdin.lock();
    loop {
        let mut line = String::new();
        if input.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            break;
        }
        let text = line.trim();
        if let Some(id) = state.pending.lock().unwrap().take() {
            let yes = matches!(text.to_lowercase().as_str(), "y" | "yes" | "j" | "ja");
            state.send(json!({"t": "confirm", "id": id, "yes": yes}));
            continue;
        }
        match text {
            "" => prompt(),
            "/exit" | "/quit" => break,
            "/clear" => {
                print!("\x1b[2J\x1b[3J\x1b[H");
                let meta = state.meta.lock().unwrap().clone();
                print_header(&meta);
                println!("\x1b[2mAnsicht geleert – Verlauf bleibt gespeichert.\x1b[0m\n");
                prompt();
            }
            "/clear-history" => {
                if confirm(&mut input, "Code-Verlauf wirklich löschen? [y/N]")? {
                    state.send(json!({"t": "clear_history"}));
                } else {
                    println!("Code-Verlauf bleibt erhalten.");
                    prompt();
                }
            }
            other => state.send(json!({"t": "input", "text": other})),
        }
    }
    println!("Session bleibt erhalten.");
    Ok(())
}

pub fn status(project: &Path) -> CodeTerminalStatus {
    let root = canonical_project(project).unwrap_or_else(|_| project.to_path_buf());
    let base = CodeTerminalStatus {
        available: cli_binary().is_some(),
        project: home_display(&root.to_string_lossy()),
        style: "functional".into(),
        model: MODEL_LABEL.into(),
        runtime: runtime_label().into(),
        ..Default::default()
    };
    let Some(session) = read_session(&root) else {
        return base;
    };
    let host = session.running && process_alive(session.pid);
    CodeTerminalStatus {
        active: true,
        host,
        running: host,
        terminal: host && session.terminal_views > 0,
        embedded: host && session.embedded_views > 0,
        busy: host && session.busy,
        project: home_display(&session.project_root),
        session_id: session.session_id,
        style: session.style.as_str().into(),
        last_result: session.last_result.unwrap_or_default(),
        history_len: session.history.len(),
        ..base
    }
}

fn cli_binary() -> Option<PathBuf> {
    let current = std::env::current_exe().ok()?;
    let bundled = current.parent()?.join("../Helpers/noki");
    if bundled.is_file() {
        return bundled.canonicalize().ok();
    }
    let sibling = current.parent()?.join("noki");
    if sibling.is_file() {
        return Some(sibling);
    }
    let dev = app_project().join("desktop/src-tauri/target/debug/noki");
    dev.is_file().then_some(dev)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Finds the Noki Code tab by its fixed title (custom title or OSC title) and
/// focuses it. With `start` (no live CLI) it relaunches in that tab or opens a new one.
#[cfg(target_os = "macos")]
fn terminal_script(title: &str, command: &str, start: bool) -> Result<String, String> {
    let script = r#"on run argv
set wantedTitle to item 1 of argv
set launchCommand to item 2 of argv
set mayStart to item 3 of argv
tell application "Terminal"
  repeat with w in windows
    repeat with t in tabs of w
      set tabTitle to ""
      try
        set tabTitle to (custom title of t) & " " & (name of w)
      end try
      if tabTitle contains wantedTitle then
        set selected of t to true
        set index of w to 1
        activate
        if mayStart is "start" then
          do script launchCommand in t
          return "restarted"
        end if
        return "focused"
      end if
    end repeat
  end repeat
  if mayStart is not "start" then return "missing"
  activate
  set t to do script launchCommand
  set custom title of t to wantedTitle
  return "started"
end tell
end run"#;
    let output = Command::new("/usr/bin/osascript")
        .args([
            "-e",
            script,
            "--",
            title,
            command,
            if start { "start" } else { "focus" },
        ])
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(target_os = "macos")]
pub fn open_or_focus(project: &Path) -> Result<String, String> {
    let root = canonical_project(project)?;
    let binary = cli_binary().ok_or("Noki-Code-CLI fehlt. Bitte Noki neu bauen.")?;
    let command = format!(
        "{} code --project {}",
        shell_quote(&binary.to_string_lossy()),
        shell_quote(&root.to_string_lossy())
    );
    // An attached terminal view is only focused; otherwise a terminal view is
    // (re)started and attaches to the same session host.
    let attached = read_session(&root)
        .is_some_and(|s| s.running && process_alive(s.pid) && s.terminal_views > 0);
    terminal_script(&terminal_title(&root), &command, !attached)
}

#[cfg(target_os = "macos")]
fn focus_existing(root: &Path) -> Result<bool, String> {
    Ok(terminal_script(&terminal_title(root), "", false)? == "focused")
}

#[cfg(not(target_os = "macos"))]
pub fn open_or_focus(_project: &Path) -> Result<String, String> {
    Err("Noki Code Terminal ist nur unter macOS verfügbar.".into())
}

#[cfg(not(target_os = "macos"))]
fn focus_existing(_root: &Path) -> Result<bool, String> {
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_secrets_without_hiding_normal_code_requests() {
        assert_eq!(redact("API_KEY=secret-value"), "API_KEY=[REDACTED]");
        assert_eq!(redact("Fix API key handling"), "Fix API key handling");
        assert!(!redact("token: abc\nghp_123456").contains("abc"));
        assert!(!redact("token: abc\nghp_123456").contains("ghp_"));
    }

    fn action(label: &str, ok: bool) -> Action {
        Action {
            label: label.into(),
            ok,
            detail: String::new(),
            ms: 0,
            ..Default::default()
        }
    }

    #[test]
    fn work_log_shows_real_events_with_test_repair_cycle() {
        let mut log = WorkLog::default();
        let lines: Vec<_> = [
            action("Analysiere Datei src/main.rs", true),
            action("Suche nach model_router", true),
            action("Ändere Dateien", true),
            action("$ cargo test --offline", false),
            action("Ändere Dateien", true),
            action("$ cargo test --offline", true),
            action("$ git diff --stat", true),
        ]
        .iter()
        .map(|a| log.line(a))
        .collect();
        let kinds: Vec<_> = lines.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            ["INSPECT", "SEARCH", "PATCH", "TEST", "REPAIR", "TEST", "VERIFY"]
        );
        assert!(lines[3].1.ends_with("FAIL"));
        assert!(lines[4].1.ends_with("1 Fehler"));
        assert!(lines[5].1.ends_with("PASS"));
        assert_eq!(log.result(&[action("$ cargo test", false)]), "PASS");
    }

    #[test]
    fn result_without_tests_reflects_failed_actions() {
        let log = WorkLog::default();
        assert_eq!(log.result(&[action("Analysiere Datei x", true)]), "PASS");
        assert_eq!(log.result(&[action("$ git status", false)]), "WARN");
        let mut failed = WorkLog::default();
        failed.line(&action("$ cargo test", false));
        assert_eq!(failed.result(&[]), "FAIL");
    }

    #[test]
    fn confirmation_defaults_to_no() {
        for (input, expected) in [
            ("\n", false),
            ("n\n", false),
            ("", false),
            ("maybe\n", false),
            ("y\n", true),
            ("JA\n", true),
        ] {
            assert_eq!(
                confirm(&mut io::Cursor::new(input), "?").unwrap(),
                expected,
                "{input:?}"
            );
        }
    }

    #[test]
    fn sessions_persist_per_project_without_mixing() {
        let base = app_project().join(format!(
            ".local/qa/code-terminal-projects-{}",
            std::process::id()
        ));
        let (a, b) = (base.join("a"), base.join("b"));
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        let (a, b) = (
            canonical_project(&a).unwrap(),
            canonical_project(&b).unwrap(),
        );
        let mut sa = new_session(&a);
        sa.style = CodeStyle::Creative;
        append(
            &mut sa,
            "User",
            "Aufgabe A token: geheim",
            vec!["src/a.rs".into()],
            None,
        );
        let mut sb = new_session(&b);
        append(&mut sb, "User", "Aufgabe B", Vec::new(), None);

        // Reload from disk as after an app/Mac restart.
        let ra = read_session(&a).unwrap();
        let rb = read_session(&b).unwrap();
        assert_ne!(ra.session_id, rb.session_id);
        assert_eq!(ra.history.len(), 1);
        assert_eq!(ra.style, CodeStyle::Creative);
        assert_eq!(ra.history[0].style, Some(CodeStyle::Creative));
        assert!(!ra.history[0].summary.contains("geheim"));
        assert!(rb.history.iter().all(|e| e.summary == "Aufgabe B"));
        assert_eq!(
            read_session(&base.join("a/../a")).map(|s| s.session_id),
            None,
            "non-canonical path is not a key"
        );
        assert_eq!(
            read_session(&canonical_project(&base.join("a/../a")).unwrap())
                .unwrap()
                .session_id,
            ra.session_id
        );

        for root in [&a, &b] {
            let _ = fs::remove_file(session_path(root));
        }
        let _ = fs::remove_dir_all(base);
    }

    struct Peer(BufReader<UnixStream>);

    impl Peer {
        /// Next host message matching `pred` (fails the test after a short timeout).
        fn until(&mut self, pred: impl Fn(&Value) -> bool) -> Value {
            loop {
                let mut line = String::new();
                self.0.read_line(&mut line).expect("host message");
                let msg: Value = serde_json::from_str(&line).unwrap();
                if pred(&msg) {
                    return msg;
                }
            }
        }
    }

    fn test_hub(name: &str) -> (Arc<Hub>, mpsc::Receiver<String>, PathBuf) {
        let root = app_project().join(format!(".local/qa/code-hub-{name}-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let root = canonical_project(&root).unwrap();
        let (tx, rx) = mpsc::channel();
        (Arc::new(Hub::new(new_session(&root), tx)), rx, root)
    }

    fn peer(hub: &Hub, id: u64, view: &str) -> Peer {
        let (host_side, view_side) = UnixStream::pair().unwrap();
        view_side
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        hub.attach(Arc::new(Client {
            id,
            view: view.into(),
            out: Mutex::new(host_side),
        }));
        let mut p = Peer(BufReader::new(view_side));
        p.until(|m| m["t"] == "snapshot");
        p
    }

    fn cleanup(root: &Path) {
        let _ = fs::remove_file(session_path(root));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn one_run_lease_and_events_reach_every_view() {
        let (hub, rx, root) = test_hub("lease");
        let mut term = peer(&hub, 1, "terminal");
        let mut noki = peer(&hub, 2, "embedded");
        assert_eq!(hub.lock().session.terminal_views, 1);
        assert_eq!(hub.lock().session.embedded_views, 1);

        hub.input(2, "Aufgabe aus Noki");
        hub.input(1, "Gleichzeitige Aufgabe");
        assert_eq!(rx.try_recv().unwrap(), "Aufgabe aus Noki");
        assert!(
            rx.try_recv().is_err(),
            "second input must not start a second run"
        );
        let user = term.until(|m| m["t"] == "event");
        assert_eq!(user["event"]["summary"], "Aufgabe aus Noki");
        assert_eq!(user["origin"], "embedded");
        noki.until(|m| m["t"] == "event");
        let busy = term.until(|m| m["t"] == "notice");
        assert!(busy["text"].as_str().unwrap().contains("arbeitet bereits"));

        let worker = {
            let hub = hub.clone();
            thread::spawn(move || hub.authorize("shell.test"))
        };
        let own = noki.until(|m| m["t"] == "confirm");
        let other = term.until(|m| m["t"] == "confirm");
        assert_eq!(own["mine"], true);
        assert_eq!(other["mine"], false);
        let id = own["id"].as_u64().unwrap();
        hub.answer(1, id, true); // not the owner: ignored
        term.until(|m| m["t"] == "notice");
        hub.answer(2, id, false);
        assert!(!worker.join().unwrap());
        assert_eq!(term.until(|m| m["t"] == "confirm_done")["yes"], false);
        assert_eq!(hub.lock().session.history.last().unwrap().kind, "CONFIRM");
        cleanup(&root);
    }

    #[test]
    fn prompt_moves_to_remaining_view_when_owner_leaves() {
        let (hub, _rx, root) = test_hub("handover");
        let _term = peer(&hub, 1, "terminal");
        let mut noki = peer(&hub, 2, "embedded");
        hub.input(1, "Tests bitte");
        let worker = {
            let hub = hub.clone();
            thread::spawn(move || hub.authorize("fs.patch"))
        };
        assert_eq!(noki.until(|m| m["t"] == "confirm")["mine"], false);
        hub.detach(1);
        let mine = noki.until(|m| m["t"] == "confirm" && m["mine"] == true);
        assert_eq!(mine["items"][0], "Repository-Dateien ändern");
        hub.answer(2, mine["id"].as_u64().unwrap(), true);
        assert!(worker.join().unwrap());
        cleanup(&root);
    }

    #[test]
    fn style_cancel_and_clear_history_are_shared_session_state() {
        let (hub, _rx, root) = test_hub("style");
        let mut term = peer(&hub, 1, "terminal");
        let mut noki = peer(&hub, 2, "embedded");
        hub.input(2, "/creative");
        term.until(|m| m["t"] == "session" && m["style"] == "creative");
        assert_eq!(read_session(&root).unwrap().style, CodeStyle::Creative);

        hub.input(1, "Aufgabe");
        hub.clear_history(2);
        noki.until(|m| m["t"] == "notice" && m["text"].as_str().unwrap().contains("unverändert"));
        assert!(!hub.lock().session.history.is_empty());
        hub.cancel();
        assert!(
            !hub.authorize("shell.test"),
            "cancel answers open prompts with no"
        );

        {
            let mut s = hub.lock();
            s.session.busy = false;
            s.owner = None;
        }
        hub.clear_history(2);
        term.until(|m| m["t"] == "history_cleared");
        assert!(read_session(&root).unwrap().history.is_empty());
        cleanup(&root);
    }

    #[test]
    fn clear_history_data_is_separate_from_display_clear() {
        let root = app_project().join(format!(
            ".local/qa/code-terminal-unit-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let mut session = new_session(&root);
        append(&mut session, "User", "test", Vec::new(), None);
        assert_eq!(session.history.len(), 1);
        // `/clear` deliberately has no persistence operation; deletion is explicit.
        session.history.clear();
        assert!(session.history.is_empty());
        let _ = fs::remove_file(session_path(Path::new(&session.project_root)));
        let _ = fs::remove_dir(root);
    }
}
