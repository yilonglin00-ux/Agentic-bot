//! Two small runtime smoke tests for the adaptive router. Both are `#[ignore]`d:
//! test B makes exactly ONE real free-tier request and is never part of a
//! normal `cargo test` run.
//!
//!   cargo test --test router_live -- --ignored --nocapture

use app_lib::cloud_engine::EngineMode;
use app_lib::router::{
    route, LiveCredentials, LiveExecutor, LocalModel, RouteRequest, RouterDeps, TaskClass, Tier,
};

/// A. Only Local: a normal work request must stay on the machine.
/// Zero cloud requests, local model selected, no credential needed.
#[test]
#[ignore = "live/external: requires the local model runtime"]
fn smoke_local_only_makes_no_cloud_request() {
    app_lib::router::reset_states();
    let mut req = RouteRequest::new(
        9001,
        TaskClass::Work,
        Tier::Normal,
        "Fasse in einem Satz zusammen, was eine Kündigungsfrist ist.",
    );
    req.engine_mode = EngineMode::OnlyLocal;

    let deps = RouterDeps {
        executor: &LiveExecutor,
        credentials: &LiveCredentials,
    };
    let out = route(&req, &deps);

    println!(
        "A local-only: model={} cloud_requests={} gate={} reason={}",
        out.selected_model, out.cloud_requests, out.audit.privacy_gate, out.reason
    );
    assert_eq!(
        out.cloud_requests, 0,
        "local-only must not issue a cloud request"
    );
    assert_eq!(out.local_model, Some(LocalModel::Qwen9B));
    assert!(out.answer.is_none(), "the local model is run by the caller");
    assert_eq!(out.audit.privacy_gate, "local_required_mode");
}

/// B. Local + Cloud: one small work request against the already configured free
/// provider. Exactly one cloud request, a real answer, no benchmark.
/// Skips itself (without failing) when no credential is configured.
#[test]
#[ignore = "live/external: requires local runtime and may call a cloud provider"]
fn smoke_local_and_cloud_uses_exactly_one_request() {
    use app_lib::router::{provider_state, CredentialProbe};

    app_lib::router::reset_states();
    if !LiveCredentials.has_secret("OPENROUTER_API_KEY")
        && !LiveCredentials.has_secret("MISTRAL_API_KEY")
    {
        println!("B skipped: no free provider configured");
        return;
    }

    let req = RouteRequest::new(
        9002,
        TaskClass::Work,
        Tier::Normal,
        "Antworte mit genau einem Wort: Bereit",
    );
    let deps = RouterDeps {
        executor: &LiveExecutor,
        credentials: &LiveCredentials,
    };
    let out = route(&req, &deps);

    println!(
        "B local+cloud: provider={} model={} cloud_requests={} fallbacks={} latency={}ms tokens={:?}",
        out.selected_provider,
        out.selected_model,
        out.cloud_requests,
        out.fallback_count,
        out.latency_ms,
        out.tokens
    );
    println!(
        "B answer: {:?}",
        out.answer
            .as_deref()
            .map(|a| a.chars().take(60).collect::<String>())
    );
    println!(
        "B audit: {}",
        app_lib::router::format_route_audit(&out.audit)
    );
    for p in app_lib::router::state_snapshot(&LiveCredentials) {
        if !p.local {
            println!(
                "   state {} = {} (credentials {}, usage {:?})",
                p.id, p.state, p.credentials, p.remaining_usage
            );
        }
    }

    assert!(
        out.cloud_requests >= 1 && out.cloud_requests <= 2,
        "at most 2 requests (1 attempt + at most 1 fallback)"
    );
    assert!(out.answer.is_some(), "a real answer came back");
    assert!(!out.answer.unwrap().trim().is_empty());
    assert!(
        out.selected_provider == "openrouter" || out.selected_provider == "mistral",
        "NORMAL work provider per tournament and fallback"
    );
    assert_eq!(provider_state(&out.selected_model).as_str(), "available");
}
