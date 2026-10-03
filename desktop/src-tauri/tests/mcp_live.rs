//! Live end-to-end test against a REAL MCP server.
//!
//! This is the test that proves the client actually speaks the protocol, rather
//! than speaking to a fixture that agrees with it. It runs the official
//! reference server (`@modelcontextprotocol/server-filesystem`) as a child
//! process over stdio and drives the whole Noki path: configure -> discover ->
//! policy -> lease-scoped call -> isolated result.
//!
//! It is `#[ignore]`d because it needs `npx` and, on a cold cache, the network.
//! CI without either would fail for a reason that has nothing to do with Noki.
//!
//!     cargo test --test mcp_live -- --ignored --nocapture

use app_lib::capability::RiskLevel;
use app_lib::mcp::{CallContext, ConnectionState, McpRegistry};
use app_lib::mcp_client::Transport;
use app_lib::mcp_policy::{ConfirmPolicy, McpConfig, ServerConfig, TargetKind, ToolRule};
use std::collections::HashMap;

/// A scoped sandbox with one readable file, plus one file OUTSIDE the scope
/// that must stay unreachable.
fn sandbox() -> (std::path::PathBuf, std::path::PathBuf) {
    let base = std::env::temp_dir().join(format!("noki-mcp-live-{}", std::process::id()));
    let allowed = base.join("allowed");
    let secret = base.join("secret");
    std::fs::create_dir_all(&allowed).unwrap();
    std::fs::create_dir_all(&secret).unwrap();
    std::fs::write(
        allowed.join("bericht.txt"),
        "Quartalsbericht 2026\nUmsatz: 1200 EUR\nAusreißer in Zeile 6.\n",
    )
    .unwrap();
    // A file the server is not allowed to reach, to prove the scope is real.
    std::fs::write(secret.join("geheim.txt"), "Vertraulich: nicht lesen.\n").unwrap();
    (
        dunce::canonicalize(&allowed).unwrap(),
        dunce::canonicalize(&secret).unwrap(),
    )
}

fn config(root: &std::path::Path) -> McpConfig {
    McpConfig {
        servers: vec![ServerConfig {
            id: "filesystem".into(),
            name: "Dateien (MCP)".into(),
            description: "Reference filesystem server, read-only".into(),
            transport: Transport::Stdio {
                command: "npx".into(),
                args: vec![
                    "-y".into(),
                    "@modelcontextprotocol/server-filesystem".into(),
                ],
                env_from: HashMap::new(),
                cwd: None,
            },
            enabled: true,
            roots: vec![root.to_string_lossy().into_owned()],
            // Only the two read tools. The same package also ships
            // write_file, edit_file, move_file and create_directory - they are
            // omitted, and the test below proves they stay unreachable.
            tools: vec![
                ToolRule {
                    tool: "read_text_file".into(),
                    mode: Some("READ".into()),
                    risk: RiskLevel::R0,
                    confirm: ConfirmPolicy::Never,
                    path_args: vec!["path".into()],
                    capability: "mcp.read".into(),
                    target_kind: Some(TargetKind::File),
                },
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
            // Generous: a cold `npx` may have to fetch the package.
            timeout_ms: 90_000,
            trusted_metadata: HashMap::from([(
                "package".into(),
                "@modelcontextprotocol/server-filesystem".into(),
            )]),
        }],
    }
}

#[test]
#[ignore = "needs npx and possibly network access"]
fn the_whole_mcp_path_works_against_the_reference_server() {
    let (allowed, secret) = sandbox();
    let registry = McpRegistry::new();
    registry.configure(config(&allowed));

    // 1. DISCOVERY. Spawn, handshake, tools/list, resources/list, admit.
    let t0 = std::time::Instant::now();
    registry.refresh();
    let connectors = registry.list_connectors(true);
    println!("discovery took {:?}", t0.elapsed());

    assert_eq!(connectors.len(), 1);
    let c = &connectors[0];
    println!(
        "server={:?} version={:?} state={:?} tools={:?} resources={}",
        c.server_name,
        c.server_version,
        c.connection_state,
        c.tools.iter().map(|t| &t.name).collect::<Vec<_>>(),
        c.resources.len()
    );
    assert_eq!(
        c.connection_state,
        ConnectionState::Connected,
        "server did not connect: {:?}",
        c.last_error
    );
    // The real server identifies itself.
    assert!(
        c.server_name.contains("filesystem"),
        "unexpected server: {:?}",
        c.server_name
    );

    // 2. CAPABILITY MAPPING. Exactly the allowlisted tools, mapped to READ.
    let names: Vec<&str> = c.tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"read_text_file"), "got {names:?}");
    for t in &c.tools {
        assert_eq!(t.mode, "READ", "{} should be READ", t.name);
        assert_eq!(t.risk_level, RiskLevel::R0);
        assert!(!t.needs_confirmation, "{} should run unattended", t.name);
    }
    // The server OFFERS write tools; policy makes them unreachable.
    for forbidden in ["write_file", "edit_file", "move_file", "create_directory"] {
        assert!(
            registry.find_tool(true, forbidden).is_none(),
            "{forbidden} must not be reachable"
        );
        let err = registry
            .execute_checked(
                true,
                forbidden,
                serde_json::json!({"path": allowed.join("x.txt"), "content": "x"}),
                &CallContext::unattended(1, "test"),
            )
            .unwrap_err();
        println!("{forbidden} refused: {err}");
    }

    // 3. THE REAL CALL, inside scope.
    let target = allowed.join("bericht.txt").to_string_lossy().into_owned();
    let t1 = std::time::Instant::now();
    let result = registry
        .execute_checked(
            true,
            "read_text_file",
            serde_json::json!({"path": target}),
            &CallContext::unattended(42, "lies den bericht"),
        )
        .expect("the in-scope read should succeed");
    println!("tool call took {:?}", t1.elapsed());
    assert!(result.success);

    // 4. ISOLATION. The content came back as data, wrapped.
    assert!(
        result.isolated_text.contains("UNTRUSTED_EXTERNAL_CONTENT"),
        "result was not isolated:\n{}",
        result.isolated_text
    );
    assert!(result.isolated_text.contains("Quartalsbericht 2026"));
    assert!(result.isolated_text.contains("Umsatz: 1200 EUR"));
    // Umlauts survive the round trip.
    assert!(result.isolated_text.contains("Ausreißer"));
    println!(
        "--- isolated result ---\n{}\n-----------------------",
        result.isolated_text
    );

    // 5. SCOPE. A file outside the configured root is refused by Noki BEFORE
    //    the server is contacted, and would be refused by the server too.
    let outside = secret.join("geheim.txt").to_string_lossy().into_owned();
    let err = registry
        .execute_checked(
            true,
            "read_text_file",
            serde_json::json!({"path": outside}),
            // even WITH a confirmation
            &CallContext {
                task_id: 43,
                lease_id: 0,
                intent: "lies die geheime datei".into(),
                confirmed: true,
            },
        )
        .unwrap_err();
    println!("out-of-scope refused: {err}");
    assert!(
        err.contains("außerhalb") || err.contains("geschützter"),
        "unexpected refusal: {err}"
    );

    // 6. Traversal out of the root lands on the same refusal.
    let traversal = allowed
        .join("../secret/geheim.txt")
        .to_string_lossy()
        .into_owned();
    assert!(registry
        .execute_checked(
            true,
            "read_text_file",
            serde_json::json!({"path": traversal}),
            &CallContext::unattended(44, "traversal")
        )
        .is_err());

    // 7. The master gate still overrides everything.
    assert!(registry
        .execute_checked(
            false,
            "read_text_file",
            serde_json::json!({"path": target}),
            &CallContext::unattended(45, "x")
        )
        .unwrap_err()
        .contains("Master Gate"));

    // 8. AUDIT. Every attempt is recorded, with the concrete scope and no
    //    document content.
    let log = app_lib::capability::action_log();
    let mcp_entries: Vec<_> = log.iter().filter(|e| e.tool.starts_with("mcp:")).collect();
    assert!(!mcp_entries.is_empty(), "nothing was audited");
    for e in &mcp_entries {
        println!(
            "audit task={} phase={} cap={} scope={} result={} ms={}",
            e.task_id, e.phase, e.capability, e.scope, e.result, e.duration_ms
        );
        assert!(
            !e.scope.contains("Quartalsbericht"),
            "document content leaked into the audit log"
        );
    }
    let ok = mcp_entries
        .iter()
        .find(|e| e.result == "ok")
        .expect("no successful call audited");
    assert_eq!(ok.phase, "READ");
    assert_eq!(ok.capability, "mcp.read");
    assert_eq!(
        ok.scope, target,
        "the audit scope must be the concrete file"
    );

    let _ = std::fs::remove_dir_all(secret.parent().unwrap());
}

#[test]
#[ignore = "needs npx and possibly network access"]
fn a_reconnect_after_the_server_dies_succeeds() {
    let (allowed, _secret) = sandbox();
    let registry = McpRegistry::new();
    registry.configure(config(&allowed));
    registry.refresh();
    let target = allowed.join("bericht.txt").to_string_lossy().into_owned();

    let first = registry.execute_checked(
        true,
        "read_text_file",
        serde_json::json!({"path": target.clone()}),
        &CallContext::unattended(1, "erste lesung"),
    );
    assert!(first.is_ok(), "first read failed: {first:?}");

    // Kill every server process, simulating a crash.
    let _ = std::process::Command::new("/usr/bin/pkill")
        .args(["-f", "server-filesystem"])
        .status();
    std::thread::sleep(std::time::Duration::from_millis(500));

    // The next call must reconnect rather than fail permanently. Discovery is
    // re-run because the cached connection is gone.
    registry.refresh();
    let second = registry.execute_checked(
        true,
        "read_text_file",
        serde_json::json!({"path": target}),
        &CallContext::unattended(2, "zweite lesung"),
    );
    println!("after reconnect: {:?}", second.as_ref().map(|r| r.success));
    assert!(second.is_ok(), "reconnect failed: {second:?}");
    assert!(second.unwrap().isolated_text.contains("Quartalsbericht"));
}
