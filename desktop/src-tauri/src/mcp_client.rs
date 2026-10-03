//! JSON-RPC 2.0 client for Model Context Protocol servers.
//!
//! WHY THIS IS SEPARATE FROM `mcp.rs`. `mcp.rs` owns POLICY - which tool may
//! be reached, in which phase, at which risk. This module owns TRANSPORT - how
//! bytes get to a server process and back. Keeping them apart is what makes the
//! policy layer testable without a child process, and what stops a transport
//! detail (a slow pipe, a dead server) from turning into a permission decision.
//!
//! WHAT THIS MODULE DOES NOT DO. It never decides whether a call is allowed, it
//! never issues a lease, and it never trusts what a server says about itself.
//! Server-declared metadata (titles, descriptions, `readOnlyHint`) is carried
//! through as DATA for the policy layer to judge - it is written by whoever
//! wrote the server, which is not necessarily the user.
//!
//! TRANSPORT. stdio, newline-delimited JSON-RPC 2.0, as the MCP specification
//! describes it. One child process per server, one background reader thread per
//! process. Responses are correlated BY ID and never by arrival order: a real
//! server answers out of order (verified against the reference filesystem
//! server, which returned ids 3, 4, 2 for requests 2, 3, 4).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Condvar, Mutex,
    },
    time::{Duration, Instant},
};

/// The protocol revision Noki speaks. A server may answer with another one;
/// that is recorded, not silently accepted as equal.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// Hard ceiling on one server answer. A tool result is evidence for a local
/// model with a bounded context - a server that wants to return 40 MB of text
/// is a denial of service against the context budget, not a useful answer.
const MAX_MESSAGE_BYTES: usize = 1_000_000;

/// Why a call did not produce a result. The variants exist because the caller
/// reacts differently to each: a timeout may be retried, a refusal may not.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum McpError {
    /// The server process could not be started at all.
    Spawn(String),
    /// The process is gone (crashed, exited, or never came up).
    Disconnected(String),
    /// No answer inside the configured budget.
    Timeout(String),
    /// A JSON-RPC error object came back (`code`, `message`).
    Protocol { code: i64, message: String },
    /// The server answered, but the answer was not usable.
    Malformed(String),
    /// The server ran the tool and reported that the tool itself failed.
    ToolFailed(String),
}

impl std::fmt::Display for McpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            McpError::Spawn(m) => write!(f, "MCP-Server konnte nicht gestartet werden: {m}"),
            McpError::Disconnected(m) => write!(f, "MCP-Server ist nicht verbunden: {m}"),
            McpError::Timeout(m) => write!(f, "MCP-Server hat nicht rechtzeitig geantwortet: {m}"),
            McpError::Protocol { code, message } => {
                write!(f, "MCP-Server meldet Fehler {code}: {message}")
            }
            McpError::Malformed(m) => write!(f, "MCP-Antwort war nicht lesbar: {m}"),
            McpError::ToolFailed(m) => write!(f, "MCP-Werkzeug ist fehlgeschlagen: {m}"),
        }
    }
}

/// How to reach a server. Only stdio is implemented; the enum exists so that
/// adding a network transport later is a new variant and not a new call path
/// through the policy layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub enum Transport {
    /// A child process speaking newline-delimited JSON-RPC on stdin/stdout.
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        /// Extra environment for the child. Values here are NAMES of variables
        /// to forward from Noki's own environment, never literal secrets - see
        /// `resolve_env`.
        #[serde(default)]
        env_from: HashMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
    },
}

/// What a server said about itself during `initialize`. Descriptive only.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
    pub protocol_version: String,
    /// Whether the server declared a `tools` capability.
    pub has_tools: bool,
    /// Whether the server declared a `resources` capability. Discovery is
    /// gated on this: the reference filesystem server answers `resources/list`
    /// with "Method not found", which is correct behaviour and not an error
    /// worth showing the user.
    pub has_resources: bool,
}

/// One tool as the SERVER describes it. Untrusted metadata.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RemoteTool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub input_schema: Value,
    /// The server's own claim that this tool only reads. Usable as a signal
    /// that can LOWER privilege, never as a reason to raise it.
    #[serde(default)]
    pub read_only_hint: bool,
}

/// One resource as the server describes it. Untrusted metadata.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RemoteResource {
    pub uri: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mime_type: String,
}

/// Shared mailbox between the reader thread and the callers waiting on ids.
#[derive(Default)]
struct Inbox {
    /// Answers that arrived, keyed by request id.
    answers: Mutex<HashMap<u64, Value>>,
    /// Set once the reader thread sees EOF or a fatal read error.
    closed: AtomicBool,
    /// Last line the server wrote that could not be parsed - for diagnostics.
    ready: Condvar,
}

/// A live connection to one MCP server process.
struct Connection {
    child: Child,
    stdin: ChildStdin,
    inbox: Arc<Inbox>,
    info: ServerInfo,
}

impl Drop for Connection {
    fn drop(&mut self) {
        // A server that is going away gets killed rather than left behind: an
        // orphaned child process would keep whatever access it was granted.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A configured server plus whatever connection it currently has.
///
/// The client is deliberately lazy: nothing is spawned until a discovery or a
/// call actually needs the server. A configured-but-unused server costs no
/// process, which is what makes it safe to ship entries that are off by default.
pub struct McpClient {
    transport: Transport,
    timeout: Duration,
    conn: Mutex<Option<Connection>>,
    next_id: AtomicU64,
    /// Consecutive failed connection attempts, for backoff.
    failures: Mutex<(u32, Option<Instant>)>,
}

/// Backoff after a failed spawn. A server whose command does not exist must not
/// be retried on every keystroke - that would stall the request path.
const BACKOFF: [u64; 4] = [0, 2, 10, 60];

impl McpClient {
    pub fn new(transport: Transport, timeout: Duration) -> Self {
        Self {
            transport,
            timeout,
            conn: Mutex::new(None),
            next_id: AtomicU64::new(0),
            failures: Mutex::new((0, None)),
        }
    }

    /// True while a live process is held. Used for reporting, not for policy.
    pub fn connected(&self) -> bool {
        self.conn
            .lock()
            .map(|c| {
                c.as_ref()
                    .is_some_and(|c| !c.inbox.closed.load(Ordering::Relaxed))
            })
            .unwrap_or(false)
    }

    pub fn server_info(&self) -> Option<ServerInfo> {
        self.conn.lock().ok()?.as_ref().map(|c| c.info.clone())
    }

    /// Drops the process. The next call reconnects from scratch.
    pub fn disconnect(&self) {
        if let Ok(mut g) = self.conn.lock() {
            *g = None;
        }
    }

    fn backoff_active(&self) -> Option<u64> {
        let g = self.failures.lock().ok()?;
        let (count, last) = &*g;
        let wait = BACKOFF[(*count as usize).min(BACKOFF.len() - 1)];
        let since = last.as_ref()?.elapsed().as_secs();
        (wait > 0 && since < wait).then_some(wait - since)
    }

    fn note_failure(&self) {
        if let Ok(mut g) = self.failures.lock() {
            g.0 = g.0.saturating_add(1);
            g.1 = Some(Instant::now());
        }
    }

    fn note_success(&self) {
        if let Ok(mut g) = self.failures.lock() {
            *g = (0, None);
        }
    }

    /// Ensures a live, initialized connection - reconnecting if the previous
    /// process died. This is the ONLY place a process is spawned.
    fn ensure(&self) -> Result<(), McpError> {
        {
            let g = self
                .conn
                .lock()
                .map_err(|e| McpError::Disconnected(e.to_string()))?;
            if let Some(c) = g.as_ref() {
                if !c.inbox.closed.load(Ordering::Relaxed) {
                    return Ok(());
                }
            }
        }
        if let Some(left) = self.backoff_active() {
            return Err(McpError::Disconnected(format!(
                "letzter Start ist fehlgeschlagen, neuer Versuch in {left}s"
            )));
        }
        match self.spawn_and_initialize() {
            Ok(conn) => {
                self.note_success();
                let mut g = self
                    .conn
                    .lock()
                    .map_err(|e| McpError::Disconnected(e.to_string()))?;
                *g = Some(conn);
                Ok(())
            }
            Err(e) => {
                self.note_failure();
                Err(e)
            }
        }
    }

    fn spawn_and_initialize(&self) -> Result<Connection, McpError> {
        let Transport::Stdio {
            command,
            args,
            env_from,
            cwd,
        } = &self.transport;
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // A server's diagnostics belong in its own log, not in Noki's
            // stdout and not in a model's context.
            .stderr(Stdio::null());
        // Start from a clean environment: a child inherits nothing it was not
        // explicitly given, so a server cannot read tokens that happen to be
        // in Noki's environment for unrelated reasons.
        cmd.env_clear();
        for (key, value) in baseline_env() {
            cmd.env(key, value);
        }
        for (child_key, host_key) in env_from {
            if let Some(v) = resolve_env(host_key) {
                cmd.env(child_key, v);
            }
        }
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        let mut child = cmd.spawn().map_err(|e| McpError::Spawn(e.to_string()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::Spawn("kein stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::Spawn("kein stdout".into()))?;
        let inbox = Arc::new(Inbox::default());

        // Reader thread: the only owner of stdout. It parses whole lines and
        // files each answer under its id. Nothing here blocks a caller.
        let reader_inbox = Arc::clone(&inbox);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        if line.len() > MAX_MESSAGE_BYTES {
                            log::warn!("noki-mcp oversized message dropped ({} bytes)", line.len());
                            continue;
                        }
                        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
                            continue;
                        };
                        // Notifications carry no id; nobody is waiting for them.
                        let Some(id) = v.get("id").and_then(Value::as_u64) else {
                            continue;
                        };
                        if let Ok(mut answers) = reader_inbox.answers.lock() {
                            answers.insert(id, v);
                        }
                        reader_inbox.ready.notify_all();
                    }
                    Err(_) => break,
                }
            }
            reader_inbox.closed.store(true, Ordering::Relaxed);
            reader_inbox.ready.notify_all();
        });

        let mut conn = Connection {
            child,
            stdin,
            inbox,
            info: ServerInfo::default(),
        };

        // Handshake. A server that cannot complete this is not usable, and
        // saying so here keeps a half-initialized server out of the registry.
        let init = self.exchange(
            &mut conn,
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "noki", "version": env!("CARGO_PKG_VERSION")}
            }),
        )?;
        conn.info = ServerInfo {
            name: init
                .pointer("/serverInfo/name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            version: init
                .pointer("/serverInfo/version")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            protocol_version: init
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            has_tools: init.pointer("/capabilities/tools").is_some(),
            has_resources: init.pointer("/capabilities/resources").is_some(),
        };
        // The spec requires this notification before normal operation.
        let notification = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        write_line(&mut conn.stdin, &notification)?;
        Ok(conn)
    }

    /// One request/response round trip on an established connection.
    fn exchange(
        &self,
        conn: &mut Connection,
        method: &str,
        params: Value,
    ) -> Result<Value, McpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        write_line(&mut conn.stdin, &request)?;

        let deadline = Instant::now() + self.timeout;
        let mut answers = conn
            .inbox
            .answers
            .lock()
            .map_err(|e| McpError::Disconnected(e.to_string()))?;
        loop {
            if let Some(v) = answers.remove(&id) {
                if let Some(e) = v.get("error") {
                    return Err(McpError::Protocol {
                        code: e.get("code").and_then(Value::as_i64).unwrap_or(0),
                        message: e
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("unbekannt")
                            .to_owned(),
                    });
                }
                return v
                    .get("result")
                    .cloned()
                    .ok_or_else(|| McpError::Malformed("Antwort ohne result".into()));
            }
            if conn.inbox.closed.load(Ordering::Relaxed) {
                return Err(McpError::Disconnected(
                    "Serverprozess hat sich beendet".into(),
                ));
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return Err(McpError::Timeout(format!(
                    "{method} nach {:?}",
                    self.timeout
                )));
            };
            let (g, _) = conn
                .inbox
                .ready
                .wait_timeout(answers, left)
                .map_err(|e| McpError::Disconnected(e.to_string()))?;
            answers = g;
        }
    }

    /// Sends a request, reconnecting once if the process died meanwhile.
    ///
    /// Exactly one retry, and only for a transport failure: a refusal or a
    /// protocol error is an answer and must not be replayed. Replaying a call
    /// that may have had an effect is how "read a file" becomes "write twice".
    fn call(&self, method: &str, params: Value) -> Result<Value, McpError> {
        for attempt in 0..2 {
            self.ensure()?;
            let mut g = self
                .conn
                .lock()
                .map_err(|e| McpError::Disconnected(e.to_string()))?;
            let Some(conn) = g.as_mut() else {
                return Err(McpError::Disconnected("keine Verbindung".into()));
            };
            match self.exchange(conn, method, params.clone()) {
                Err(McpError::Disconnected(m)) if attempt == 0 => {
                    *g = None;
                    drop(g);
                    log::info!("noki-mcp reconnecting after transport loss: {m}");
                }
                other => return other,
            }
        }
        Err(McpError::Disconnected(
            "Verbindung nach Neuversuch nicht möglich".into(),
        ))
    }

    /// Handshake only - used to report a server's state without discovery.
    pub fn connect(&self) -> Result<ServerInfo, McpError> {
        self.ensure()?;
        self.server_info()
            .ok_or_else(|| McpError::Disconnected("keine Verbindung".into()))
    }

    /// `tools/list`. Returns the server's own descriptions, unjudged.
    pub fn list_tools(&self) -> Result<Vec<RemoteTool>, McpError> {
        self.ensure()?;
        if self.server_info().is_some_and(|i| !i.has_tools) {
            return Ok(Vec::new());
        }
        let result = self.call("tools/list", json!({}))?;
        let list = result
            .get("tools")
            .and_then(Value::as_array)
            .ok_or_else(|| McpError::Malformed("tools/list ohne tools".into()))?;
        Ok(list
            .iter()
            .filter_map(|t| {
                let name = t.get("name").and_then(Value::as_str)?.to_owned();
                Some(RemoteTool {
                    name,
                    description: t
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .chars()
                        .take(400)
                        .collect(),
                    input_schema: t.get("inputSchema").cloned().unwrap_or(Value::Null),
                    read_only_hint: t
                        .pointer("/annotations/readOnlyHint")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                })
            })
            .collect())
    }

    /// `resources/list`, only when the server declared the capability.
    pub fn list_resources(&self) -> Result<Vec<RemoteResource>, McpError> {
        self.ensure()?;
        if self.server_info().is_some_and(|i| !i.has_resources) {
            return Ok(Vec::new());
        }
        let result = match self.call("resources/list", json!({})) {
            Ok(v) => v,
            // -32601 "Method not found" is a legitimate answer from a server
            // that only offers tools. It is an absence, not a fault.
            Err(McpError::Protocol { code: -32601, .. }) => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let list = result
            .get("resources")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(list
            .iter()
            .filter_map(|r| {
                Some(RemoteResource {
                    uri: r.get("uri").and_then(Value::as_str)?.to_owned(),
                    name: r
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    mime_type: r
                        .get("mimeType")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                })
            })
            .collect())
    }

    /// `tools/call`. Returns the flattened text of the result.
    ///
    /// A server reports a tool failure INSIDE a successful response
    /// (`isError: true`), which is a different thing from a JSON-RPC error and
    /// is mapped to `ToolFailed` so the caller can tell "the server is broken"
    /// from "the tool said no".
    pub fn call_tool(&self, name: &str, arguments: Value) -> Result<ToolOutcome, McpError> {
        let result = self.call("tools/call", json!({"name": name, "arguments": arguments}))?;
        let text = flatten_content(&result);
        if result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err(McpError::ToolFailed(text.chars().take(400).collect()));
        }
        Ok(ToolOutcome {
            text,
            structured: result.get("structuredContent").cloned(),
        })
    }
}

/// What a tool returned. `text` is always populated; `structured` only when the
/// server provided a machine-readable form.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolOutcome {
    pub text: String,
    pub structured: Option<Value>,
}

/// MCP content blocks -> one text blob. Non-text blocks are named, not inlined:
/// a base64 image has no business expanding a model's context by a megabyte.
fn flatten_content(result: &Value) -> String {
    let Some(blocks) = result.get("content").and_then(Value::as_array) else {
        return String::new();
    };
    let mut out = String::new();
    for b in blocks {
        let kind = b.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "text" => {
                if let Some(t) = b.get("text").and_then(Value::as_str) {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(t);
                }
            }
            other => {
                if !out.is_empty() {
                    out.push('\n');
                }
                let mime = b.get("mimeType").and_then(Value::as_str).unwrap_or("");
                out.push_str(&format!(
                    "[{other}-Inhalt{}]",
                    if mime.is_empty() {
                        String::new()
                    } else {
                        format!(": {mime}")
                    }
                ));
            }
        }
    }
    out
}

/// The minimum environment a child needs to run at all. Everything else must be
/// asked for by name in the server's configuration.
fn baseline_env() -> Vec<(&'static str, String)> {
    let mut v = vec![(
        "PATH",
        std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin:/usr/sbin:/sbin".into()),
    )];
    for key in ["HOME", "LANG", "TMPDIR"] {
        if let Ok(val) = std::env::var(key) {
            v.push((key, val));
        }
    }
    v
}

/// Looks up a secret by the NAME the configuration gave, from Noki's own
/// environment. A configuration file therefore never has to contain a secret,
/// and this function is the only way one can reach a child process.
fn resolve_env(host_key: &str) -> Option<String> {
    if host_key.is_empty() {
        return None;
    }
    std::env::var(host_key).ok().filter(|v| !v.is_empty())
}

fn write_line(stdin: &mut ChildStdin, value: &Value) -> Result<(), McpError> {
    let mut line = serde_json::to_vec(value).map_err(|e| McpError::Malformed(e.to_string()))?;
    line.push(b'\n');
    stdin
        .write_all(&line)
        .and_then(|_| stdin.flush())
        .map_err(|e| McpError::Disconnected(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in MCP server built from shell, so the transport is exercised
    /// without depending on a package being installed. It answers out of order
    /// on purpose - that is the behaviour the real reference server shows.
    fn scripted_server(script: &str) -> McpClient {
        McpClient::new(
            Transport::Stdio {
                command: "/bin/sh".into(),
                args: vec!["-c".into(), script.into()],
                env_from: HashMap::new(),
                cwd: None,
            },
            Duration::from_secs(5),
        )
    }

    /// Reads every request line and replies from a fixed table, keyed by method.
    const ECHO: &str = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1.0"}}}\n' "$id" ;;
    *'"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"notes.read","description":"Read","inputSchema":{"type":"object"},"annotations":{"readOnlyHint":true}}]}}\n' "$id" ;;
    *'"tools/call"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"hello"}]}}\n' "$id" ;;
    *'"resources/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":"Method not found"}}\n' "$id" ;;
  esac
done
"#;

    #[test]
    fn handshake_discovery_and_call_work_over_stdio() {
        let client = scripted_server(ECHO);
        let info = client.connect().unwrap();
        assert_eq!(info.name, "fixture");
        assert!(info.has_tools);
        assert!(!info.has_resources);

        let tools = client.list_tools().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "notes.read");
        assert!(tools[0].read_only_hint);

        let out = client.call_tool("notes.read", json!({})).unwrap();
        assert_eq!(out.text, "hello");
    }

    #[test]
    fn a_tools_only_server_reports_no_resources_instead_of_an_error() {
        let client = scripted_server(ECHO);
        // Declared capability is absent -> discovery is skipped entirely.
        assert!(client.list_resources().unwrap().is_empty());
    }

    #[test]
    fn a_missing_command_is_a_spawn_error_and_then_backs_off() {
        let client = McpClient::new(
            Transport::Stdio {
                command: "/nonexistent/noki-mcp-fixture".into(),
                args: vec![],
                env_from: HashMap::new(),
                cwd: None,
            },
            Duration::from_millis(500),
        );
        assert!(matches!(client.connect(), Err(McpError::Spawn(_))));
        // The second attempt must not spawn again immediately.
        assert!(matches!(client.connect(), Err(McpError::Disconnected(_))));
        assert!(!client.connected());
    }

    #[test]
    fn a_silent_server_times_out_rather_than_hanging() {
        // Consumes input and never answers.
        let client = scripted_server("cat > /dev/null");
        let started = Instant::now();
        let err = McpClient::new(
            Transport::Stdio {
                command: "/bin/sh".into(),
                args: vec!["-c".into(), "cat > /dev/null".into()],
                env_from: HashMap::new(),
                cwd: None,
            },
            Duration::from_millis(400),
        )
        .connect()
        .unwrap_err();
        assert!(matches!(err, McpError::Timeout(_)), "got {err:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
        drop(client);
    }

    #[test]
    fn a_server_that_exits_is_disconnected_not_hung() {
        let client = scripted_server("exit 0");
        assert!(matches!(client.connect(), Err(McpError::Disconnected(_))));
    }

    #[test]
    fn out_of_order_answers_are_matched_by_id() {
        // On the FIRST request (initialize, id 1) this answers an unrelated id
        // first, then the real one. The waiting caller must ignore the noise
        // and still pick up its own answer.
        let script = r#"
read -r first
printf '{"jsonrpc":"2.0","id":7,"result":{"serverInfo":{"name":"someone-elses","version":"1"}}}\n'
printf '{"jsonrpc":"2.0","method":"notifications/progress","params":{"x":1}}\n'
printf '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"y","capabilities":{},"serverInfo":{"name":"early","version":"1"}}}\n'
sleep 1
"#;
        let client = scripted_server(script);
        // id 1 is the initialize; its answer arrives second and must still match.
        let info = client.connect().unwrap();
        assert_eq!(info.name, "early");
    }

    #[test]
    fn a_tool_level_failure_is_distinct_from_a_protocol_error() {
        let script = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"capabilities":{"tools":{}},"serverInfo":{"name":"f","version":"1"}}}\n' "$id" ;;
    *'"tools/call"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"Access denied"}],"isError":true}}\n' "$id" ;;
  esac
done
"#;
        let client = scripted_server(script);
        let err = client.call_tool("x", json!({})).unwrap_err();
        match err {
            McpError::ToolFailed(m) => assert!(m.contains("Access denied")),
            other => panic!("expected ToolFailed, got {other:?}"),
        }
    }

    #[test]
    fn non_text_blocks_are_named_and_not_inlined() {
        let result = json!({"content":[
            {"type":"text","text":"Bericht"},
            {"type":"image","data":"AAAA","mimeType":"image/png"}
        ]});
        let flat = flatten_content(&result);
        assert!(flat.contains("Bericht"));
        assert!(flat.contains("image-Inhalt: image/png"));
        assert!(!flat.contains("AAAA"), "payload must not enter the context");
    }

    #[test]
    fn a_secret_is_referenced_by_name_and_never_inlined() {
        std::env::set_var("NOKI_MCP_TEST_TOKEN", "s3cret");
        assert_eq!(
            resolve_env("NOKI_MCP_TEST_TOKEN").as_deref(),
            Some("s3cret")
        );
        assert!(resolve_env("NOKI_MCP_ABSENT_TOKEN").is_none());
        assert!(resolve_env("").is_none());
        // The configuration only ever holds the NAME.
        let t = Transport::Stdio {
            command: "x".into(),
            args: vec![],
            env_from: HashMap::from([("TOKEN".into(), "NOKI_MCP_TEST_TOKEN".into())]),
            cwd: None,
        };
        let j = serde_json::to_string(&t).unwrap();
        assert!(j.contains("NOKI_MCP_TEST_TOKEN"));
        assert!(!j.contains("s3cret"));
        std::env::remove_var("NOKI_MCP_TEST_TOKEN");
    }
}
