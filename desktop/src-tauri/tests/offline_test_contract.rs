//! Static guard for tests that can launch provider requests or external model
//! CLIs. It performs no process or network access itself.

fn assert_live_probe_is_ignored(source: &str, function: &str) {
    let declaration = format!("fn {function}(");
    let declaration_at = source
        .find(&declaration)
        .unwrap_or_else(|| panic!("missing live probe {function}"));
    let attributes = &source[..declaration_at];
    let attributes = &attributes[attributes.rfind("#[test]").unwrap_or(0)..];
    assert!(
        attributes.contains("#[ignore") && attributes.contains("live/external:"),
        "{function} must be explicitly ignored and labelled live/external"
    );
}

#[test]
fn real_provider_and_cli_probes_are_excluded_from_normal_tests() {
    let specialist = include_str!("specialist_live.rs");
    for function in [
        "test_claude_cli_detection",
        "test_claude_cli_error_falls_back_cleanly",
        "test_work_quality_check_claude_success_or_rate_limited",
    ] {
        assert_live_probe_is_ignored(specialist, function);
    }

    assert_live_probe_is_ignored(
        include_str!("cloud_engine_live.rs"),
        "test_11_live_benchmark_run_report",
    );
    assert_live_probe_is_ignored(
        include_str!("router_live.rs"),
        "smoke_local_and_cloud_uses_exactly_one_request",
    );
}
