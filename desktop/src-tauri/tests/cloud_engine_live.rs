//! Integration tests for Cloud Engine, Free-Tier Adapters, Benchmarks and Low-Latency Routing.
//!
//! Run with:
//!     cargo test --test cloud_engine_live -- --nocapture

use app_lib::cloud_engine::{
    apply_benchmark_results, get_cloud_engine, parse_rate_limit_headers, CloudEngine, EngineMode,
};
use app_lib::{model_registry, runtime_registry};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

fn activate_all_runtime_models() {
    runtime_registry::clear();
    for definition in model_registry::cloud_engine_models() {
        runtime_registry::set_auth_state(
            definition.canonical_model_id,
            runtime_registry::AuthState::Ready,
        )
        .unwrap();
        runtime_registry::set_model_enabled(definition.canonical_model_id, true).unwrap();
    }
}

#[test]
fn test_01_only_local_never_sends_cloud_request() {
    let mut ce = CloudEngine::new();
    ce.set_mode(EngineMode::OnlyLocal);

    let called = AtomicUsize::new(0);
    let mock = |_p: &str, _q: &str| {
        called.fetch_add(1, Ordering::SeqCst);
        Ok(("Antwort".into(), 100, None))
    };

    let outcome = ce.route_work("Wie lange gilt der Vertrag?", false, 200, Some(&mock));
    assert!(
        outcome.is_none(),
        "OnlyLocal mode must immediately return None without calling any cloud provider"
    );
    assert_eq!(
        called.load(Ordering::SeqCst),
        0,
        "Zero cloud requests may be made in OnlyLocal mode"
    );
}

#[test]
fn test_02_local_and_cloud_work_takes_work_ranking() {
    let mut ce = CloudEngine::new();
    ce.set_mode(EngineMode::LocalAndCloud);
    activate_all_runtime_models();
    ce.recompute_rankings();

    // Verify Gemini is rank #1 for Work
    let best_work = ce
        .providers
        .iter()
        .find(|p| p.work_rank == Some(1))
        .expect("Must have Work rank 1");
    let best_id = best_work.id.clone();

    let recorded_provider = std::sync::Mutex::new(String::new());
    let mock = |p: &str, _q: &str| {
        *recorded_provider.lock().unwrap() = p.to_string();
        Ok((
            "Work Antwort von Provider".into(),
            120,
            Some("15 req".into()),
        ))
    };

    let outcome = ce
        .route_work("Fasse dieses Dokument zusammen", false, 300, Some(&mock))
        .expect("Should route to work provider");

    assert_eq!(
        outcome.provider_id, best_id,
        "Work task must be routed to Work rank #1 provider ({best_id})"
    );
    assert_eq!(*recorded_provider.lock().unwrap(), best_id);
    assert_eq!(
        outcome.attempts, 1,
        "Should succeed on first attempt without parallel calls"
    );
}

#[test]
fn test_03_coding_takes_coding_ranking() {
    let mut ce = CloudEngine::new();
    ce.set_mode(EngineMode::LocalAndCloud);
    activate_all_runtime_models();
    ce.recompute_rankings();

    // Verify Groq Code is rank #1 for Coding
    let best_code = ce
        .providers
        .iter()
        .find(|p| p.code_rank == Some(1))
        .expect("Must have Code rank 1");
    let best_id = best_code.id.clone();

    let recorded_provider = std::sync::Mutex::new(String::new());
    let mock = |p: &str, _q: &str| {
        *recorded_provider.lock().unwrap() = p.to_string();
        Ok(("Code Patch: diff".into(), 150, None))
    };

    let outcome = ce
        .route_code("Repariere den Rust Compilerfehler", false, 300, Some(&mock))
        .expect("Should route to coding provider");

    assert_eq!(
        outcome.provider_id, best_id,
        "Coding task must be routed to Coding rank #1 provider ({best_id})"
    );
    assert_eq!(*recorded_provider.lock().unwrap(), best_id);
}

#[test]
fn test_04_provider_429_immediately_fails_over_to_next_provider() {
    let mut ce = CloudEngine::new();
    ce.set_mode(EngineMode::LocalAndCloud);
    activate_all_runtime_models();
    ce.recompute_rankings();

    let r1_id = ce
        .providers
        .iter()
        .find(|p| p.work_rank == Some(1))
        .unwrap()
        .id
        .clone();
    let r2_id = ce
        .providers
        .iter()
        .find(|p| p.work_rank == Some(2))
        .unwrap()
        .id
        .clone();

    let calls = std::sync::Mutex::new(Vec::<String>::new());
    let r1_ref = r1_id.as_str();
    let mock = |p: &str, _q: &str| {
        calls.lock().unwrap().push(p.to_string());
        if p == r1_ref {
            // Rank #1 fails with 429
            Err("Rate limit exceeded (429): Resource exhausted".into())
        } else {
            // Rank #2 succeeds
            Ok((
                "Erfolgreiche Antwort von Provider 2".into(),
                210,
                Some("45 req".into()),
            ))
        }
    };

    let outcome = ce
        .route_work("Vergleiche Klauseln", false, 400, Some(&mock))
        .expect("Should failover to provider #2");

    let history = calls.lock().unwrap().clone();
    assert_eq!(
        history.len(),
        2,
        "Should have made exactly 2 attempts in sequence"
    );
    assert_eq!(history[0], r1_id, "First try must be rank #1");
    assert_eq!(history[1], r2_id, "Second try must be rank #2");
    assert_eq!(outcome.provider_id, r2_id);
    assert_eq!(outcome.attempts, 2);

    // Verify Rank #1 is now marked rate limited with cooldown
    let first = ce.providers.iter().find(|p| p.id == r1_id).unwrap();
    assert_eq!(first.health, "rate_limited");
    assert!(first.cooldown_until.is_some());
}

#[test]
fn test_05_provider_timeout_fails_over_to_next_provider() {
    let mut ce = CloudEngine::new();
    ce.set_mode(EngineMode::LocalAndCloud);
    activate_all_runtime_models();
    ce.recompute_rankings();

    let calls = std::sync::Mutex::new(Vec::<String>::new());
    let mock = |p: &str, _q: &str| {
        calls.lock().unwrap().push(p.to_string());
        if calls.lock().unwrap().len() == 1 {
            // Provider #1 times out
            Err("curl timeout nach 15s".into())
        } else {
            // Provider #2 answers
            Ok(("Antwort nach Timeout-Failover".into(), 180, None))
        }
    };

    let outcome = ce
        .route_work("Analysiere Daten", false, 300, Some(&mock))
        .expect("Should succeed after timeout failover");

    assert_eq!(outcome.attempts, 2);
    let history = calls.lock().unwrap().clone();
    assert_eq!(history.len(), 2);
    assert_ne!(
        history[0], history[1],
        "Failover must switch to a different provider"
    );
}

#[test]
fn test_06_all_cloud_providers_unavailable_falls_back_locally() {
    let mut ce = CloudEngine::new();
    ce.set_mode(EngineMode::LocalAndCloud);
    activate_all_runtime_models();
    for definition in model_registry::cloud_engine_models() {
        let mut observation = runtime_registry::RuntimeObservation::outcome(
            runtime_registry::RuntimeOutcome::ServiceUnavailable,
            runtime_registry::OutcomeScope::Provider,
        );
        observation.cooldown_until = Some(runtime_registry::deadline_after(
            std::time::Duration::from_secs(60),
        ));
        runtime_registry::observe(definition.canonical_model_id, observation).unwrap();
    }

    let called = AtomicUsize::new(0);
    let mock = |_p: &str, _q: &str| {
        called.fetch_add(1, Ordering::SeqCst);
        Ok(("Antwort".into(), 100, None))
    };

    let outcome = ce.route_work("Schreibe Zusammenfassung", false, 300, Some(&mock));
    assert!(
        outcome.is_none(),
        "When all cloud providers are unavailable, router must return None for clean local fallback"
    );
    assert_eq!(called.load(Ordering::SeqCst), 0);
}

#[test]
fn test_07_private_sensitive_task_stays_strictly_local() {
    let mut ce = CloudEngine::new();
    ce.set_mode(EngineMode::LocalAndCloud);
    activate_all_runtime_models();

    let called = AtomicUsize::new(0);
    let mock = |_p: &str, _q: &str| {
        called.fetch_add(1, Ordering::SeqCst);
        Ok(("Antwort".into(), 100, None))
    };

    // Sensitive task (sensitive = true)
    let outcome = ce.route_work(
        "Berechne Abfindung für Mitarbeiter X",
        true,
        300,
        Some(&mock),
    );
    assert!(
        outcome.is_none(),
        "Sensitive task must NEVER be sent to cloud providers, even under LocalAndCloud mode"
    );
    assert_eq!(called.load(Ordering::SeqCst), 0);
}

#[test]
fn test_08_usage_headers_parsed_truthfully() {
    let raw = "HTTP/1.1 200 OK\r\nx-ratelimit-remaining-requests: 18\r\nx-ratelimit-remaining-tokens: 240000\r\n";
    let (usage, _, _, _) = parse_rate_limit_headers(raw);
    assert_eq!(usage, Some("18 req · 240000 tokens".into()));
}

#[test]
fn test_09_usage_absent_reports_unavailable_never_guessed() {
    let raw = "HTTP/1.1 200 OK\r\nServer: Google\r\nContent-Type: application/json\r\n";
    let (usage, reset, _, _) = parse_rate_limit_headers(raw);
    assert_eq!(
        usage, None,
        "When provider does not supply remaining quota headers, usage must be None (displayed as 'Usage: unavailable')"
    );
    assert_eq!(reset, None);
}

#[test]
fn test_10_benchmark_changes_work_and_code_rankings_independently() {
    let mut ce = CloudEngine::new();

    // Custom scenario: Provider A is phenomenal at Coding but mediocre at Work,
    // while Provider B is great at Work but mediocre at Coding.
    let mut work_scores = HashMap::new();
    work_scores.insert("mistral_free".to_string(), 100.0f32); // Now #1 in Work
    work_scores.insert("openrouter_free".to_string(), 60.0f32);
    work_scores.insert("gemini_free".to_string(), 70.0f32);

    let mut code_scores = HashMap::new();
    code_scores.insert("openrouter_free".to_string(), 100.0f32); // Now #1 in Coding
    code_scores.insert("mistral_free".to_string(), 65.0f32);

    ce.apply_benchmarks(work_scores, code_scores);

    let mistral = ce
        .providers
        .iter()
        .find(|p| p.id == "mistral_free")
        .unwrap();
    let openrouter_code = ce
        .providers
        .iter()
        .find(|p| p.id == "openrouter_free")
        .unwrap();

    // Mistral must now be Work #1, but a lower Coding rank
    assert_eq!(mistral.work_rank, Some(1));
    assert!(mistral.code_rank.unwrap() > 1);

    // OpenRouter Code must now be Coding #1, but a lower Work rank
    assert_eq!(openrouter_code.code_rank, Some(1));
    assert!(openrouter_code.work_rank.unwrap() > 1);

    // Also verify apply_benchmark_results helper works cleanly
    let mut ws = HashMap::new();
    ws.insert("groq_qwen".to_string(), 100.0f32);
    let mut cs = HashMap::new();
    cs.insert("gemini_free".to_string(), 100.0f32);
    apply_benchmark_results(ws, cs);
    let ge = get_cloud_engine();
    assert_eq!(
        ge.providers
            .iter()
            .find(|p| p.id == "groq_qwen")
            .unwrap()
            .work_rank,
        Some(1)
    );
}

#[test]
#[ignore = "live/external: benchmark makes provider requests and consumes rate-limit quota"]
fn test_11_live_benchmark_run_report() {
    let mut ce = CloudEngine::new();
    let report = ce.run_benchmarks();
    assert_eq!(report.providers_cooldown.len(), 0);
    assert!(!report.work_ranking.is_empty());
    assert!(!report.code_ranking.is_empty());
}

#[test]
fn test_12_status_json_conforms_to_settings_spec() {
    let json = app_lib::cloud_engine::cloud_engine_status_json();
    assert_eq!(json["engine_mode"], "local_and_cloud");
    assert_eq!(json["mode_label"], "Local + Cloud");

    let providers = json["providers"]
        .as_array()
        .expect("providers must be an array");
    assert_eq!(providers.len(), 7, "Must have 7 free-tier cloud providers");

    let ids: Vec<&str> = providers
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"gemini_free"));
    assert!(ids.contains(&"groq_qwen"));
    assert!(ids.contains(&"groq_gpt_oss"));
    assert!(ids.contains(&"openrouter_free"));
    assert!(ids.contains(&"openrouter_nemotron"));
    assert!(ids.contains(&"openrouter_deepseek"));
    assert!(ids.contains(&"mistral_free"));

    for p in providers {
        assert!(
            p["free_only"].as_bool().unwrap(),
            "Must strictly be free_only"
        );
        assert!(p["work_rank"].is_number(), "Must have a work rank");
        assert!(p["code_rank"].is_number(), "Must have a code rank");
        // No fabricated usage:
        if p["remaining_usage"].is_null() {
            // Truthfully null -> UI will render "Usage: unavailable"
        }
    }
}
