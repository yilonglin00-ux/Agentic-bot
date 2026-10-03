//! Cloud Provider Abstraction, Free-Tier Adapters, Benchmarking and Low-Latency Routing.
//!
//! Principles:
//! 1. Strictly $0 / Free-tier only: No paid endpoint is ever called automatically.
//! 2. One request -> one best model: offline benchmarking, single-shot runtime routing.
//! 3. Fallback chain on failure: best -> next best -> local fallback. Never parallel calls.
//! 4. Two separate rankings: Work Assistant (`work_rank`) vs Coding (`code_rank`).
//! 5. Truthful usage: only display real usage headers; if absent, display "unavailable".
//! 6. Privacy first: sensitive/private tasks stay strictly local.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Operational mode for Noki Intelligence inference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineMode {
    // Alte Paid-Cloud-Einstellungen werden beim Einlesen zu Free Cloud.
    #[serde(alias = "paid_cloud", alias = "PAID_CLOUD", alias = "paid", alias = "free_cloud", alias = "FREE_CLOUD")]
    LocalAndCloud,
    #[serde(alias = "local", alias = "LOCAL")]
    OnlyLocal,
}

impl Default for EngineMode {
    fn default() -> Self {
        Self::LocalAndCloud
    }
}

impl EngineMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LocalAndCloud => "local_and_cloud",
            Self::OnlyLocal => "only_local",
        }
    }

    pub fn display_label(&self) -> &'static str {
        match self {
            Self::LocalAndCloud => "Local + Cloud",
            Self::OnlyLocal => "Only Local",
        }
    }
}

/// Static configuration for a Cloud Provider adapter.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CloudProviderConfig {
    pub id: String,
    pub display_name: String,
    pub model: String,
    pub env_var: String,
    pub endpoint: String,
    pub supports_files: bool,
    pub supports_vision: bool,
    pub supports_tools: bool,
    pub context: u32,
    pub free_only: bool,
    /// Whether this provider targets work, coding, or both.
    pub target_suite: TargetSuite,
    /// Extra JSON merged into the request body, e.g. `{"reasoning": {"effort": "none"}}`.
    /// Measured: DeepSeek V4 Flash drops from 74.5 to 48.8 on Work when the
    /// reasoning channel is left open, so the router pins it shut here.
    #[serde(default)]
    pub extra_body: Option<serde_json::Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetSuite {
    Work,
    Code,
    Both,
}

/// A Cloud Provider runtime descriptor, fully meeting all Section B requirements.
///
/// Health, quota, cooldown and availability members are serialized compatibility
/// projections. Runtime writes belong exclusively to `runtime_registry`; call
/// sites refresh these fields only after reading a canonical snapshot.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CloudProvider {
    pub id: String,
    pub display_name: String,
    pub available: bool,
    pub free_only: bool,
    pub model: String,
    pub supports_files: bool,
    pub supports_vision: bool,
    pub supports_tools: bool,
    pub context: u32,
    pub rate_limit: Option<String>,
    pub remaining_usage: Option<String>,
    pub reset_at: Option<String>,
    pub latency: Option<u64>,
    pub health: String,
    pub work_rank: Option<usize>,
    pub code_rank: Option<usize>,
    pub work_score: Option<f32>,
    pub code_score: Option<f32>,
    pub enabled: bool,
    pub last_error: Option<String>,
    pub total_tokens_used: Option<u32>,
    #[serde(skip)]
    pub cooldown_until: Option<Instant>,
}

impl CloudProvider {
    pub fn is_available(&self) -> bool {
        if !self.free_only {
            return false;
        }
        canonical_model_id_for_cloud_engine(&self.id)
            .and_then(crate::runtime_registry::model_runtime_view)
            .map(|runtime| runtime.is_selectable())
            .unwrap_or(false)
    }

    /// Refreshes the old serialized shape from the canonical registry.
    /// Nothing in this projection is an input to runtime state.
    fn refresh_legacy_projection(&mut self) {
        let Some(model_id) = canonical_model_id_for_cloud_engine(&self.id) else {
            return;
        };
        let Some(runtime) = crate::runtime_registry::model_runtime_view(model_id) else {
            return;
        };
        self.available = runtime.is_selectable();
        self.enabled = runtime.effective_availability()
            != crate::runtime_registry::RuntimeAvailability::Disabled;
        self.health = match runtime.effective_availability() {
            crate::runtime_registry::RuntimeAvailability::Available
            | crate::runtime_registry::RuntimeAvailability::Degraded => "available",
            crate::runtime_registry::RuntimeAvailability::RateLimited
            | crate::runtime_registry::RuntimeAvailability::Cooldown => "rate_limited",
            crate::runtime_registry::RuntimeAvailability::QuotaExhausted => "quota_exhausted",
            crate::runtime_registry::RuntimeAvailability::TemporarilyUnavailable => "unavailable",
            crate::runtime_registry::RuntimeAvailability::HalfOpen => "unavailable",
            crate::runtime_registry::RuntimeAvailability::AuthFailed => "not_configured",
            crate::runtime_registry::RuntimeAvailability::Disabled => "disabled",
        }
        .to_string();
        self.remaining_usage = runtime.quota_remaining();
        self.reset_at = runtime.quota_reset();
        self.latency = runtime.last_latency_ms();
        self.last_error = runtime
            .last_outcomes()
            .into_iter()
            .flatten()
            .find(|outcome| outcome.is_failure())
            .map(|outcome| outcome.as_str().to_string());
        self.cooldown_until = runtime
            .cooldown_until()
            .and_then(crate::runtime_registry::instant_from_deadline);
    }
}

/// Shipped free cloud provider configurations. Strictly $0 / free-tier only!
pub fn known_cloud_configs() -> Vec<CloudProviderConfig> {
    crate::model_registry::cloud_engine_models()
        .map(|model| CloudProviderConfig {
            id: model
                .connection
                .cloud_engine_id
                .expect("filtered above")
                .to_string(),
            display_name: model.display_name.to_string(),
            model: model.exact_model_version.to_string(),
            env_var: model.connection.env_var.to_string(),
            endpoint: model.connection.endpoint.to_string(),
            supports_files: model.capabilities.files,
            supports_vision: model.capabilities.vision,
            supports_tools: model.capabilities.tools,
            context: model.capabilities.context_tokens,
            free_only: model.cost_safety == crate::model_registry::CostSafety::VerifiedFreeHardStop,
            target_suite: TargetSuite::Both,
            extra_body: model
                .connection
                .disable_reasoning
                .then(|| serde_json::json!({ "reasoning": { "effort": "none" } })),
        })
        .collect()
}

fn canonical_model_id_for_cloud_engine(id: &str) -> Option<&'static str> {
    crate::model_registry::model_by_cloud_engine_id(id).map(|model| model.canonical_model_id)
}

fn note_cloud_request(config: &CloudProviderConfig) {
    if let Some(model_id) = canonical_model_id_for_cloud_engine(&config.id) {
        let _ = crate::runtime_registry::note_request(model_id);
    }
}

fn note_cloud_success(
    config: &CloudProviderConfig,
    latency_ms: u64,
    quota_remaining: Option<String>,
    quota_reset: Option<String>,
) {
    let Some(model_id) = canonical_model_id_for_cloud_engine(&config.id) else {
        return;
    };
    let mut observation = crate::runtime_registry::RuntimeObservation::outcome(
        crate::runtime_registry::RuntimeOutcome::Success,
        crate::runtime_registry::OutcomeScope::Model,
    );
    observation.last_latency_ms = Some(latency_ms);
    observation.quota_remaining = quota_remaining;
    observation.quota_reset = quota_reset;
    let _ = crate::runtime_registry::observe(model_id, observation);
}

fn note_cloud_quota_telemetry(config: &CloudProviderConfig, telemetry: Option<crate::runtime_registry::QuotaTelemetry>) {
    if let (Some(model_id), Some(telemetry)) = (canonical_model_id_for_cloud_engine(&config.id), telemetry) {
        let _ = crate::runtime_registry::note_quota_telemetry(model_id, telemetry);
    }
}

fn note_cloud_failure(
    config: &CloudProviderConfig,
    error: &ProviderExecError,
    cooldown: Option<Duration>,
) {
    use crate::runtime_registry::{OutcomeScope, RuntimeObservation, RuntimeOutcome};
    let Some(model_id) = canonical_model_id_for_cloud_engine(&config.id) else {
        return;
    };
    let mut outcome = error.runtime_outcome();
    let scope = match outcome {
        RuntimeOutcome::AuthenticationFailed | RuntimeOutcome::Timeout => OutcomeScope::Connection,
        RuntimeOutcome::QuotaExhausted => OutcomeScope::Connection,
        RuntimeOutcome::RateLimited if config.id == "gemini_free" => {
            outcome = RuntimeOutcome::QuotaExhausted;
            OutcomeScope::Connection
        }
        RuntimeOutcome::RateLimited => OutcomeScope::Model,
        RuntimeOutcome::ServiceUnavailable => OutcomeScope::Provider,
        _ => OutcomeScope::Model,
    };
    let mut observation = RuntimeObservation::outcome(outcome, scope);
    observation.cooldown_until = matches!(
        outcome,
        RuntimeOutcome::RateLimited
            | RuntimeOutcome::QuotaExhausted
            | RuntimeOutcome::Timeout
            | RuntimeOutcome::ServiceUnavailable
    )
    .then(|| cooldown.map(crate::runtime_registry::deadline_after))
    .flatten();
    let _ = crate::runtime_registry::observe(model_id, observation);
}

fn provider_error_from_message(message: String) -> ProviderExecError {
    use crate::runtime_registry::{classify_error_message, RuntimeOutcome};
    match classify_error_message(&message) {
        RuntimeOutcome::RateLimited => ProviderExecError::RateLimited {
            cooldown: Duration::from_secs(60),
            reason: message,
        },
        RuntimeOutcome::QuotaExhausted => ProviderExecError::QuotaExhausted(message),
        RuntimeOutcome::AuthenticationFailed => ProviderExecError::Auth(message),
        RuntimeOutcome::Timeout => ProviderExecError::Timeout(message),
        RuntimeOutcome::EmptyResponse => ProviderExecError::EmptyResponse(message),
        _ => ProviderExecError::Failed(message),
    }
}

// ---------------------------------------------------------------------------
//  Header & Usage Parsing (Never guessing)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProviderRateLimitHeaders {
    pub limit_requests: Option<u64>,
    /// Groq documents this as the request-per-day window.
    pub remaining_requests: Option<u64>,
    pub limit_tokens: Option<u64>,
    /// Groq documents this as the token-per-minute window.
    pub remaining_tokens: Option<u64>,
    pub reset_requests: Option<Duration>,
    pub reset_tokens: Option<Duration>,
    pub retry_after: Option<Duration>,
}

fn parse_rate_limit_duration(value: &str) -> Option<Duration> {
    let value = value.trim().trim_start_matches("in ");
    if let Ok(seconds) = value.parse::<f64>() {
        return (seconds >= 0.0).then(|| Duration::from_millis((seconds * 1_000.0) as u64));
    }
    let mut rest = value;
    let mut total_ms = 0f64;
    let mut found = false;
    while !rest.is_empty() {
        let number_end = rest
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(rest.len());
        if number_end == 0 {
            return None;
        }
        let number = rest[..number_end].parse::<f64>().ok()?;
        rest = &rest[number_end..];
        let (unit, len) = if rest.starts_with("ms") {
            ("ms", 2)
        } else if rest.starts_with('s') {
            ("s", 1)
        } else if rest.starts_with('m') {
            ("m", 1)
        } else if rest.starts_with('h') {
            ("h", 1)
        } else if rest.starts_with('d') {
            ("d", 1)
        } else {
            return None;
        };
        total_ms += match unit {
            "ms" => number,
            "s" => number * 1_000.0,
            "m" => number * 60_000.0,
            "h" => number * 3_600_000.0,
            "d" => number * 86_400_000.0,
            _ => return None,
        };
        rest = &rest[len..];
        found = true;
    }
    found.then(|| Duration::from_millis(total_ms as u64))
}

pub fn parse_provider_rate_limit_headers(raw_headers: &str) -> ProviderRateLimitHeaders {
    let mut parsed = ProviderRateLimitHeaders::default();
    for line in raw_headers.lines() {
        let l = line.trim();
        let lower = l.to_lowercase();
        let value = || l.split_once(':').map(|(_, value)| value.trim()).unwrap_or("");
        // RFC-style headers may carry a window parameter (`100;w=60`). The
        // numeric allowance is still authoritative; the unit is requests.
        let request_value = || value().split(';').next().unwrap_or("").trim().parse().ok();
        if lower.starts_with("x-ratelimit-remaining-requests:")
            || lower.starts_with("ratelimit-remaining-requests:")
        {
            parsed.remaining_requests = value().parse().ok();
        } else if lower.starts_with("x-ratelimit-limit-requests:")
            || lower.starts_with("ratelimit-limit-requests:")
        {
            parsed.limit_requests = value().parse().ok();
        } else if lower.starts_with("x-ratelimit-remaining:")
            || lower.starts_with("ratelimit-remaining:")
        {
            parsed.remaining_requests = request_value();
        } else if lower.starts_with("x-ratelimit-limit:")
            || lower.starts_with("ratelimit-limit:")
        {
            parsed.limit_requests = request_value();
        } else if lower.starts_with("x-ratelimit-remaining-tokens:")
            || lower.starts_with("ratelimit-remaining-tokens:")
        {
            parsed.remaining_tokens = value().parse().ok();
        } else if lower.starts_with("x-ratelimit-limit-tokens:")
            || lower.starts_with("ratelimit-limit-tokens:")
        {
            parsed.limit_tokens = value().parse().ok();
        } else if lower.starts_with("x-ratelimit-reset-requests:")
            || lower.starts_with("ratelimit-reset-requests:")
        {
            parsed.reset_requests = parse_rate_limit_duration(value());
        } else if lower.starts_with("x-ratelimit-reset:")
            || lower.starts_with("ratelimit-reset:")
        {
            parsed.reset_requests = parse_rate_limit_duration(value());
        } else if lower.starts_with("x-ratelimit-reset-tokens:")
            || lower.starts_with("ratelimit-reset-tokens:")
        {
            parsed.reset_tokens = parse_rate_limit_duration(value());
        } else if lower.starts_with("retry-after:") {
            parsed.retry_after = parse_rate_limit_duration(value());
        }
    }
    parsed
}

fn quota_telemetry_for_response(
    config: &CloudProviderConfig,
    headers: &ProviderRateLimitHeaders,
) -> Option<crate::runtime_registry::QuotaTelemetry> {
    use crate::runtime_registry::QuotaTelemetry;
    let provider = crate::model_registry::model(&config.id)
        .or_else(|| canonical_model_id_for_cloud_engine(&config.id).and_then(crate::model_registry::model))
        .map(|model| model.provider_id);
    // Workers AI's free allowance is documented account-wide. No remaining
    // value is guessed because a chat response does not expose neuron cost.
    if provider == Some("cloudflare_workers_ai") {
        return Some(QuotaTelemetry {
            source: "static_limit".into(),
            scope: "account".into(),
            observed_at: 0,
            reset_at: Some("00:00 UTC".into()),
            neuron_limit: Some(10_000),
            ..QuotaTelemetry::default()
        });
    }
    let has_headers = headers.limit_requests.is_some()
        || headers.remaining_requests.is_some()
        || headers.limit_tokens.is_some()
        || headers.remaining_tokens.is_some()
        || headers.reset_requests.is_some()
        || headers.reset_tokens.is_some()
        || headers.retry_after.is_some();
    if !has_headers {
        return None;
    }
    let scope = match provider {
        Some("groq") | Some("openrouter") => "account",
        Some("mistral") => "provider",
        // Gemini API-key limits belong to the configured project/model tier;
        // no generic public tier number is seeded here.
        Some("google") => "project",
        _ => "provider",
    };
    Some(QuotaTelemetry {
        source: "provider_live".into(),
        scope: scope.into(),
        observed_at: 0,
        reset_at: headers
            .reset_requests
            .or(headers.reset_tokens)
            .or(headers.retry_after)
            .map(format_rate_limit_duration),
        request_limit: headers.limit_requests,
        request_remaining: headers.remaining_requests,
        token_limit: headers.limit_tokens,
        token_remaining: headers.remaining_tokens,
        cooldown_seconds: headers.retry_after.map(|value| value.as_secs()),
        ..QuotaTelemetry::default()
    })
}

fn format_rate_limit_duration(value: Duration) -> String {
    let seconds = value.as_secs();
    if seconds >= 3600 { format!("in {} h", seconds / 3600) }
    else if seconds >= 60 { format!("in {} min", seconds / 60) }
    else { format!("in {} s", seconds) }
}

/// Parses HTTP headers for rate limit and quota information.
/// If headers do not contain usage numbers, returns None (UI displays "Usage: unavailable").
pub fn parse_rate_limit_headers(
    raw_headers: &str,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<Duration>,
) {
    let parsed = parse_provider_rate_limit_headers(raw_headers);
    let remaining_usage = match (parsed.remaining_requests, parsed.remaining_tokens) {
        (Some(requests), Some(tokens)) => Some(format!("{requests} req · {tokens} tokens")),
        (Some(requests), None) => Some(format!("{requests} req")),
        (None, Some(tokens)) => Some(format!("{tokens} tokens")),
        (None, None) => None,
    };
    let reset = raw_headers.lines().find_map(|line| {
        let line = line.trim();
        let lower = line.to_ascii_lowercase();
        (lower.starts_with("x-ratelimit-reset-requests:")
            || lower.starts_with("ratelimit-reset-requests:")
            || lower.starts_with("x-ratelimit-reset-tokens:")
            || lower.starts_with("ratelimit-reset-tokens:"))
        .then(|| line.split(':').nth(1).unwrap_or("").trim().to_string())
        .filter(|value| !value.is_empty())
    });
    (remaining_usage, reset.clone(), reset, parsed.retry_after)
}

// ---------------------------------------------------------------------------
//  HTTP Execution via curl (Strict, lightweight, dependency-free)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct ProviderExecutionResponse {
    pub text: String,
    pub latency_ms: u64,
    pub remaining_usage: Option<String>,
    pub reset_at: Option<String>,
    pub tokens: Option<u32>,
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    pub reasoning_tokens: Option<u32>,
    pub rate_limit_headers: ProviderRateLimitHeaders,
    pub quota_telemetry: Option<crate::runtime_registry::QuotaTelemetry>,
    pub http_status: Option<u16>,
    pub finish_reason: Option<String>,
    pub content_present: bool,
}

/// Redacted facts about one transport attempt. This deliberately has neither
/// request/response bodies nor error text: benchmark evidence must remain
/// useful after a partial run without retaining user content or secrets.
#[derive(Clone, Debug, Default)]
struct ProviderExecutionObservation {
    attempted: bool,
    http_status: Option<u16>,
    finish_reason: Option<String>,
    content_present: Option<bool>,
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
    reasoning_tokens: Option<u32>,
    latency_ms: u64,
    retry_after_ms: Option<u64>,
    reset_requests_ms: Option<u64>,
    reset_tokens_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkFailureCategory {
    ProviderError,
    Timeout,
    RateLimit,
    QuotaExhausted,
    Auth,
    ContentNull,
    ReasoningOnly,
    TokenLimit,
    ParserError,
    UnsupportedParameter,
    HarnessSkip,
    QualityFail,
}

#[derive(Clone, Debug)]
struct ObservedProviderExecError {
    error: ProviderExecError,
    observation: ProviderExecutionObservation,
    failure_category: BenchmarkFailureCategory,
}

impl From<ProviderExecError> for ObservedProviderExecError {
    fn from(error: ProviderExecError) -> Self {
        let failure_category = benchmark_failure_category_for_error(&error);
        Self {
            error,
            observation: ProviderExecutionObservation { attempted: true, ..Default::default() },
            failure_category,
        }
    }
}

fn observed_failure(
    error: ProviderExecError,
    observation: ProviderExecutionObservation,
    failure_category: BenchmarkFailureCategory,
) -> ObservedProviderExecError {
    ObservedProviderExecError {
        error,
        observation,
        failure_category,
    }
}

#[derive(Clone, Debug)]
pub enum ProviderExecError {
    RateLimited {
        cooldown: Duration,
        reason: String,
    },
    /// The account/connection has no usable allowance until its quota resets.
    QuotaExhausted(String),
    /// 401/403: the credential is missing, expired or unauthorized. Never retried.
    Auth(String),
    Unavailable(String),
    Timeout(String),
    EmptyResponse(String),
    Failed(String),
    /// The local $0 proof (cost attestation / billing preflight) is missing or
    /// expired: nothing was sent. Only the user can renew it.
    CostBlocked(String),
}

impl ProviderExecError {
    pub fn runtime_outcome(&self) -> crate::runtime_registry::RuntimeOutcome {
        use crate::runtime_registry::RuntimeOutcome;
        match self {
            Self::RateLimited { .. } => RuntimeOutcome::RateLimited,
            Self::QuotaExhausted(_) => RuntimeOutcome::QuotaExhausted,
            Self::Auth(_) => RuntimeOutcome::AuthenticationFailed,
            Self::Unavailable(_) | Self::Failed(_) => RuntimeOutcome::ServiceUnavailable,
            Self::Timeout(_) => RuntimeOutcome::Timeout,
            Self::EmptyResponse(_) => RuntimeOutcome::EmptyResponse,
            Self::CostBlocked(_) => RuntimeOutcome::CostBlocked,
        }
    }
}

impl std::fmt::Display for ProviderExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RateLimited { reason, .. } => write!(f, "Rate limited: {reason}"),
            Self::QuotaExhausted(msg) => write!(f, "Quota exhausted: {msg}"),
            Self::Auth(msg) => write!(f, "Auth failure: {msg}"),
            Self::Unavailable(msg) => write!(f, "Unavailable: {msg}"),
            Self::Timeout(msg) => write!(f, "Timeout: {msg}"),
            Self::EmptyResponse(msg) => write!(f, "Empty response: {msg}"),
            Self::CostBlocked(msg) => write!(f, "Cost blocked: {msg}"),
            Self::Failed(msg) => write!(f, "Failed: {msg}"),
        }
    }
}

impl std::error::Error for ProviderExecError {}

fn benchmark_failure_category_for_error(error: &ProviderExecError) -> BenchmarkFailureCategory {
    match error {
        ProviderExecError::RateLimited { reason, .. } => {
            if reason.contains("groq_limit_dimension=tpd") || reason.contains("groq_limit_dimension=rpd") {
                BenchmarkFailureCategory::QuotaExhausted
            } else {
                BenchmarkFailureCategory::RateLimit
            }
        }
        ProviderExecError::QuotaExhausted(_) => BenchmarkFailureCategory::QuotaExhausted,
        ProviderExecError::Auth(_) => BenchmarkFailureCategory::Auth,
        ProviderExecError::Timeout(_) => BenchmarkFailureCategory::Timeout,
        ProviderExecError::EmptyResponse(_) => BenchmarkFailureCategory::ContentNull,
        ProviderExecError::Unavailable(_)
        | ProviderExecError::Failed(_)
        | ProviderExecError::CostBlocked(_) => BenchmarkFailureCategory::ProviderError,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderSecretSource {
    Env,
    Keychain,
    Missing,
}

impl ProviderSecretSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Env => "env",
            Self::Keychain => "keychain",
            Self::Missing => "missing",
        }
    }
}

fn nonempty_secret(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn select_provider_secret(
    env_value: Option<String>,
    keychain_value: Option<String>,
) -> (Option<String>, ProviderSecretSource) {
    if let Some(value) = nonempty_secret(env_value) {
        return (Some(value), ProviderSecretSource::Env);
    }
    if let Some(value) = nonempty_secret(keychain_value) {
        return (Some(value), ProviderSecretSource::Keychain);
    }
    (None, ProviderSecretSource::Missing)
}

fn resolve_provider_secret_with_source(env_var: &str) -> (Option<String>, ProviderSecretSource) {
    let env_value = std::env::var(env_var).ok();
    if nonempty_secret(env_value.clone()).is_some() {
        return select_provider_secret(env_value, None);
    }

    let mut keychain_value = None;
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("/usr/bin/security")
            .args(["find-generic-password", "-s", env_var, "-w"])
            .output();
        if let Ok(out) = output {
            if out.status.success() {
                keychain_value = Some(String::from_utf8_lossy(&out.stdout).into_owned());
            }
        }
    }
    select_provider_secret(env_value, keychain_value)
}

/// Nokis canonical environment -> macOS Keychain resolver. Secrets are never
/// logged, serialized to disk, or returned to the frontend.
pub fn resolve_provider_secret(env_var: &str) -> Option<String> {
    resolve_provider_secret_with_source(env_var).0
}

/// Redacted diagnostics only; returns `env`, `keychain`, or `missing`.
pub fn provider_secret_source(env_var: &str) -> ProviderSecretSource {
    resolve_provider_secret_with_source(env_var).1
}

/// Executes a single completion request against a cloud provider.
/// Enforces:
/// - strictly free models (e.g. OpenRouter must end with `:free`)
/// - timeout (hard limit)
/// - 429 detection with cooldown
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultimodalAttachment {
    pub mime_type: String,
    pub data_base64: String,
}

/// - no parallel calls
/// - zero re-entrant mutex locking
pub fn execute_provider_request(
    config: &CloudProviderConfig,
    prompt: &str,
    max_tokens: u32,
    timeout: Duration,
) -> Result<ProviderExecutionResponse, ProviderExecError> {
    execute_provider_request_multimodal(config, prompt, &[], max_tokens, timeout)
}

pub fn execute_provider_request_multimodal(
    config: &CloudProviderConfig,
    prompt: &str,
    images: &[MultimodalAttachment],
    max_tokens: u32,
    timeout: Duration,
) -> Result<ProviderExecutionResponse, ProviderExecError> {
    match execute_provider_request_observed(config, prompt, images, max_tokens, timeout) {
        Ok(response) => Ok(response),
        Err(failure) => {
            log::warn!(
                "noki-provider-attempt provider={} http_status={:?} category={:?} requested_max_tokens={} finish_reason={:?} input_tokens={:?} output_tokens={:?} reasoning_tokens={:?} retry_after_ms={:?} reset_requests_ms={:?} reset_tokens_ms={:?}",
                config.id,
                failure.observation.http_status,
                failure.failure_category,
                max_tokens,
                failure.observation.finish_reason,
                failure.observation.input_tokens,
                failure.observation.output_tokens,
                failure.observation.reasoning_tokens,
                failure.observation.retry_after_ms,
                failure.observation.reset_requests_ms,
                failure.observation.reset_tokens_ms
            );
            Err(failure.error)
        }
    }
}

fn execute_provider_request_observed(
    config: &CloudProviderConfig,
    prompt: &str,
    images: &[MultimodalAttachment],
    max_tokens: u32,
    timeout: Duration,
) -> Result<ProviderExecutionResponse, ObservedProviderExecError> {
    if !config.free_only {
        return Err(ProviderExecError::Unavailable(format!(
            "Provider {} ist nicht als kostenlos zertifiziert.",
            config.display_name
        )).into());
    }
    if config.id.starts_with("openrouter") && !config.model.ends_with(":free") {
        return Err(ProviderExecError::Unavailable(format!(
            "OpenRouter Modell {} ist kein kostenloses Modell (:free).",
            config.model
        )).into());
    }

    let api_key = resolve_provider_secret(&config.env_var).ok_or_else(|| {
        ProviderExecError::Unavailable(format!(
            "Umgebungsvariable/Secret {} ist nicht konfiguriert.",
            config.env_var
        ))
    })?;

    // Productive routing reuses the same hard $0 proof as the explicit v2
    // benchmark adapters. A fresh attestation/preflight is required before a
    // byte can leave for either newly activated provider.
    match config.id.as_str() {
        "groq_gpt_oss_120b" | "benchmark_groq_gpt_oss_120b" => {
            groq_local_cost_attestation(&api_key)
                .map_err(ProviderExecError::CostBlocked)?;
        }
        "cloudflare_glm_4_7_flash" | "benchmark_cloudflare_glm_4_7_flash" => {
            preflight_cloudflare_glm(config).map_err(|e| {
                if e.contains("cost_attestation") || e.contains("paid_plan") {
                    ProviderExecError::CostBlocked(e)
                } else {
                    ProviderExecError::Unavailable(e)
                }
            })?;
        }
        _ => {}
    }

    let start = Instant::now();
    let is_gemini = config.id.starts_with("gemini");

    let (url, body_json, auth_header) = if is_gemini {
        let u = format!("{}?key={}", config.endpoint, api_key);
        let mut parts: Vec<serde_json::Value> = vec![serde_json::json!({ "text": prompt })];
        for img in images {
            parts.push(serde_json::json!({
                "inlineData": {
                    "mimeType": img.mime_type,
                    "data": img.data_base64,
                }
            }));
        }
        let b = serde_json::json!({
            "contents": [{
                "role": "user",
                "parts": parts,
            }],
            "generationConfig": {
                "maxOutputTokens": max_tokens,
                "temperature": 0.2
            }
        });
        (u, b.to_string(), None)
    } else {
        let completion_limit_key = if matches!(
            config.id.as_str(),
            "groq_gpt_oss_120b" | "benchmark_groq_gpt_oss_120b"
        ) {
            "max_completion_tokens"
        } else {
            "max_tokens"
        };
        let user_msg = if images.is_empty() {
            serde_json::json!({ "role": "user", "content": prompt })
        } else {
            let mut content: Vec<serde_json::Value> = vec![serde_json::json!({
                "type": "text",
                "text": prompt,
            })];
            for img in images {
                content.push(serde_json::json!({
                    "type": "image_url",
                    "image_url": {
                        "url": format!("data:{};base64,{}", img.mime_type, img.data_base64),
                    }
                }));
            }
            serde_json::json!({ "role": "user", "content": content })
        };
        let mut b = serde_json::json!({
            "model": config.model,
            "messages": [ user_msg ],
            "temperature": 0.2
        });
        b.as_object_mut()
            .expect("completion request is an object")
            .insert(completion_limit_key.into(), serde_json::json!(max_tokens));
        if let (Some(extra), Some(obj)) = (config.extra_body.as_ref(), b.as_object_mut()) {
            if let Some(extra_obj) = extra.as_object() {
                for (k, v) in extra_obj {
                    // Benchmark control metadata belongs in the fingerprint,
                    // never in the provider request schema.
                    if !k.starts_with("_noki_") {
                        obj.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (
            config.endpoint.clone(),
            b.to_string(),
            Some(format!("Authorization: Bearer {}", api_key)),
        )
    };

    let mut cmd = std::process::Command::new("/usr/bin/curl");
    cmd.args([
        "-s",
        "-i",
        "--compressed",
        "--max-time",
        &timeout.as_secs().to_string(),
        "-X",
        "POST",
        "-H",
        "Content-Type: application/json",
    ]);

    if let Some(auth) = auth_header {
        cmd.arg("-H").arg(auth);
    }
    if config.id.starts_with("openrouter") {
        cmd.arg("-H").arg("HTTP-Referer: https://noki.local");
        cmd.arg("-H").arg("X-Title: Noki Desktop");
    }

    cmd.arg("-d").arg(&body_json);
    cmd.arg(&url);

    let output = cmd.output().map_err(|e| {
        ProviderExecError::Failed(format!("curl konnte nicht ausgeführt werden: {e}"))
    })?;
    let latency_ms = start.elapsed().as_millis() as u64;

    let raw = String::from_utf8_lossy(&output.stdout).to_string();
    if raw.is_empty() {
        return Err(observed_failure(
            ProviderExecError::Timeout(format!(
                "{} hat keine Antwort geliefert (Timeout nach {}s oder Verbindungsfehler).",
                config.display_name,
                timeout.as_secs()
            )),
            ProviderExecutionObservation { attempted: true, latency_ms, ..Default::default() },
            BenchmarkFailureCategory::Timeout,
        ));
    }

    // Split headers and body
    let (headers, body) = if let Some(idx) = raw.find("\r\n\r\n") {
        (&raw[..idx], &raw[idx + 4..])
    } else if let Some(idx) = raw.find("\n\n") {
        (&raw[..idx], &raw[idx + 2..])
    } else {
        ("", raw.as_str())
    };

    let status_line = headers.lines().next().unwrap_or("");
    let http_status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok());
    let is_429 = status_line.contains("429")
        || body.contains("RESOURCE_EXHAUSTED")
        || body.contains("rate_limit");
    let rate_limit_headers = parse_provider_rate_limit_headers(headers);
    let (remaining_usage, reset_at, _, retry_after) = parse_rate_limit_headers(headers);
    let mut observation = ProviderExecutionObservation {
        attempted: true,
        http_status,
        latency_ms,
        retry_after_ms: retry_after.or(rate_limit_headers.retry_after).map(|d| d.as_millis() as u64),
        reset_requests_ms: rate_limit_headers.reset_requests.map(|d| d.as_millis() as u64),
        reset_tokens_ms: rate_limit_headers.reset_tokens.map(|d| d.as_millis() as u64),
        ..Default::default()
    };

    if is_429 {
        let body_lower = body.to_ascii_lowercase();
        let groq_dimension = if matches!(
            config.id.as_str(),
            "groq_gpt_oss_120b" | "benchmark_groq_gpt_oss_120b"
        ) {
            if body_lower.contains("tokens per minute") || body_lower.contains("tpm") {
                "tpm"
            } else if body_lower.contains("requests per minute") || body_lower.contains("rpm") {
                "rpm"
            } else if body_lower.contains("tokens per day") || body_lower.contains("tpd") {
                "tpd"
            } else if body_lower.contains("requests per day")
                || body_lower.contains("rpd")
                || rate_limit_headers.remaining_requests == Some(0)
            {
                "rpd"
            } else if rate_limit_headers.reset_tokens.is_some() {
                "tpm"
            } else {
                "unknown"
            }
        } else {
            "unknown"
        };
        let cooldown = retry_after
            .or(rate_limit_headers.retry_after)
            .or(rate_limit_headers.reset_tokens)
            .or(rate_limit_headers.reset_requests)
            .unwrap_or(Duration::from_secs(60));
        let error = ProviderExecError::RateLimited {
            cooldown,
            reason: format!(
                "HTTP 429 bei {}; groq_limit_dimension={groq_dimension}.",
                config.display_name
            ),
        };
        log::warn!(
            "noki-provider-attempt provider={} http_status={:?} category=rate_limit dimension={} retry_after_ms={:?} reset_requests_ms={:?} reset_tokens_ms={:?} remaining_requests={:?} remaining_tokens={:?}",
            config.id,
            http_status,
            groq_dimension,
            observation.retry_after_ms,
            observation.reset_requests_ms,
            observation.reset_tokens_ms,
            rate_limit_headers.remaining_requests,
            rate_limit_headers.remaining_tokens
        );
        let category = benchmark_failure_category_for_error(&error);
        return Err(observed_failure(error, observation, category));
    }

    if !status_line.contains("200") && !status_line.is_empty() {
        // The status class decides the provider state: an auth failure is never
        // retried, a 5xx is a temporary outage, anything else is a plain failure.
        if status_line.contains(" 401") || status_line.contains(" 403") {
            return Err(observed_failure(
                ProviderExecError::Auth(format!(
                    "{} lehnte die Zugangsdaten ab ({}).",
                    config.display_name, status_line
                )),
                observation,
                BenchmarkFailureCategory::Auth,
            ));
        }
        if status_line.contains(" 500")
            || status_line.contains(" 502")
            || status_line.contains(" 503")
            || status_line.contains(" 504")
        {
            return Err(observed_failure(
                ProviderExecError::Unavailable(format!(
                    "{} ist vorübergehend nicht erreichbar ({}).",
                    config.display_name, status_line
                )),
                observation,
                BenchmarkFailureCategory::ProviderError,
            ));
        }
        let category = if http_status == Some(400)
            && (body.to_ascii_lowercase().contains("unsupported")
                || body.to_ascii_lowercase().contains("not supported")
                || body.to_ascii_lowercase().contains("unknown parameter"))
        {
            BenchmarkFailureCategory::UnsupportedParameter
        } else {
            BenchmarkFailureCategory::ProviderError
        };
        return Err(observed_failure(
            ProviderExecError::Failed(format!(
                "HTTP-Fehler bei {}: {}",
                config.display_name, status_line
            )),
            observation,
            category,
        ));
    }

    let parsed: serde_json::Value = serde_json::from_str(body).map_err(|e| {
        observed_failure(
            ProviderExecError::Failed(format!("Ungültiges JSON von {}: {e}", config.display_name)),
            observation.clone(),
            BenchmarkFailureCategory::ParserError,
        )
    })?;

    let (text, tokens, input_tokens, output_tokens, reasoning_tokens) = if is_gemini {
        let txt = parsed
            .pointer("/candidates/0/content/parts/0/text")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let tok = parsed
            .pointer("/usageMetadata/totalTokenCount")
            .and_then(|t| t.as_u64())
            .map(|t| t as u32);
        let input = parsed
            .pointer("/usageMetadata/promptTokenCount")
            .and_then(|t| t.as_u64())
            .map(|t| t as u32);
        let output = parsed
            .pointer("/usageMetadata/candidatesTokenCount")
            .and_then(|t| t.as_u64())
            .map(|t| t as u32);
        let reasoning = parsed
            .pointer("/usageMetadata/thoughtsTokenCount")
            .and_then(|t| t.as_u64())
            .map(|t| t as u32);
        (txt, tok, input, output, reasoning)
    } else {
        let txt = parsed
            .pointer("/choices/0/message/content")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let tok = parsed
            .pointer("/usage/total_tokens")
            .and_then(|t| t.as_u64())
            .map(|t| t as u32);
        let input = parsed
            .pointer("/usage/prompt_tokens")
            .and_then(|t| t.as_u64())
            .map(|t| t as u32);
        let output = parsed
            .pointer("/usage/completion_tokens")
            .and_then(|t| t.as_u64())
            .map(|t| t as u32);
        let reasoning = parsed
            .pointer("/usage/completion_tokens_details/reasoning_tokens")
            .or_else(|| parsed.pointer("/usage/reasoning_tokens"))
            .and_then(|t| t.as_u64())
            .map(|t| t as u32);
        (txt, tok, input, output, reasoning)
    };

    observation.finish_reason = parsed
        .pointer("/choices/0/finish_reason")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    observation.content_present = Some(
        parsed
            .pointer("/choices/0/message/content")
            .and_then(|value| value.as_str())
            .is_some_and(|value| !value.trim().is_empty()),
    );
    observation.input_tokens = input_tokens;
    observation.output_tokens = output_tokens;
    observation.reasoning_tokens = reasoning_tokens;

    if text.is_empty() {
        let failure_category = if observation.finish_reason.as_deref() == Some("length") {
            BenchmarkFailureCategory::TokenLimit
        } else if observation.reasoning_tokens.unwrap_or(0) > 0 {
            BenchmarkFailureCategory::ReasoningOnly
        } else {
            BenchmarkFailureCategory::ContentNull
        };
        return Err(observed_failure(
            ProviderExecError::EmptyResponse(format!(
                "{} lieferte leeren Antworttext.",
                config.display_name
            )),
            observation,
            failure_category,
        ));
    }

    let quota_telemetry = quota_telemetry_for_response(config, &rate_limit_headers);
    log::info!(
        "noki-provider-attempt provider={} http_status={:?} category=success requested_max_tokens={} total_tokens={:?} input_tokens={:?} output_tokens={:?} reasoning_tokens={:?} finish_reason={:?} response_words={} remaining_requests={:?} remaining_tokens={:?} reset_requests_ms={:?} reset_tokens_ms={:?} retry_after_ms={:?}",
        config.id,
        http_status,
        max_tokens,
        tokens,
        input_tokens,
        output_tokens,
        reasoning_tokens,
        observation.finish_reason,
        text.split_whitespace().count(),
        rate_limit_headers.remaining_requests,
        rate_limit_headers.remaining_tokens,
        rate_limit_headers.reset_requests.map(|d| d.as_millis()),
        rate_limit_headers.reset_tokens.map(|d| d.as_millis()),
        rate_limit_headers.retry_after.map(|d| d.as_millis())
    );
    Ok(ProviderExecutionResponse {
        text,
        latency_ms,
        remaining_usage,
        reset_at,
        tokens,
        input_tokens,
        output_tokens,
        reasoning_tokens,
        rate_limit_headers,
        quota_telemetry,
        http_status,
        finish_reason: observation.finish_reason,
        content_present: observation.content_present.unwrap_or(false),
    })
}

// ---------------------------------------------------------------------------
//  Two Separate Benchmarks: Work Assistant & Coding
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchmarkCase {
    pub id: &'static str,
    pub name: &'static str,
    pub prompt: &'static str,
    pub context: Option<&'static str>,
    pub expected_fact: &'static str,
    pub requires_json: bool,
    pub max_tokens: u32,
}

pub const BENCHMARK_SCHEMA_VERSION: u32 = 2;
pub const BENCHMARK_SUITE_VERSION: &str = "2.0.0";
pub const BENCHMARK_HARNESS_VERSION: &str = "noki-benchmark-v2";
pub const WORK_SUITE_ID: &str = "noki-work";
pub const CODING_SUITE_ID: &str = "noki-coding";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkTaskClass {
    Work,
    Coding,
}

impl BenchmarkTaskClass {
    pub fn suite_id(self) -> &'static str {
        match self {
            Self::Work => WORK_SUITE_ID,
            Self::Coding => CODING_SUITE_ID,
        }
    }

    fn scorer_id(self) -> &'static str {
        match self {
            Self::Work => "noki-work-scorer-v1",
            Self::Coding => "noki-coding-scorer-v1",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Evaluation-harness result only; it is deliberately not a runtime health
/// event and never updates `runtime_registry`.
pub enum BenchmarkOutcome {
    Success,
    Empty,
    RateLimited,
    Unavailable,
    Timeout,
    AuthFailure,
    Failed,
}

/// One observed case. Suite/run identity lives on the containing run so cases
/// from Work and Coding cannot be merged accidentally.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkCaseResultV2 {
    pub case_id: String,
    pub case_fingerprint: String,
    pub quality_score: f32,
    pub availability_score: f32,
    pub latency_ms: u64,
    pub outcome: BenchmarkOutcome,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkSuiteRunV2 {
    pub schema_version: u32,
    pub suite_id: String,
    pub suite_version: String,
    pub harness_version: String,
    pub task_class: BenchmarkTaskClass,
    pub model_config_fingerprint: String,
    pub run_id: String,
    pub timestamp: u64,
    pub cases: Vec<BenchmarkCaseResultV2>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkAggregateV2 {
    pub suite_id: String,
    pub suite_version: String,
    pub model_config_fingerprint: String,
    pub quality_score: f32,
    pub availability_score: f32,
    pub latency_ms: u64,
    pub case_count: usize,
}

fn fingerprint_parts(parts: &[&str]) -> String {
    let mut hash = Sha256::new();
    // Length-prefixing makes the canonical encoding unambiguous.
    for part in parts {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part.as_bytes());
    }
    hex::encode(hash.finalize())
}

/// Fingerprints everything that can change the meaning or scoring of a case:
/// prompt, context, expected result, scorer and output limits.
pub fn benchmark_case_fingerprint(case: &BenchmarkCase, task_class: BenchmarkTaskClass) -> String {
    let max_tokens = case.max_tokens.to_string();
    let output_contract = if case.requires_json { "json" } else { "text" };
    fingerprint_parts(&[
        case.prompt,
        case.context.unwrap_or(""),
        case.expected_fact,
        task_class.scorer_id(),
        &max_tokens,
        output_contract,
    ])
}

pub fn benchmark_model_config_fingerprint(config: &CloudProviderConfig) -> String {
    let context = config.context.to_string();
    let free_only = config.free_only.to_string();
    let target_suite = format!("{:?}", config.target_suite);
    let extra_body = config
        .extra_body
        .as_ref()
        .map(serde_json::Value::to_string)
        .unwrap_or_default();
    fingerprint_parts(&[
        &config.id,
        &config.model,
        &config.endpoint,
        &context,
        &free_only,
        &target_suite,
        &extra_body,
    ])
}

fn expected_suite(task_class: BenchmarkTaskClass) -> Vec<BenchmarkCase> {
    match task_class {
        BenchmarkTaskClass::Work => work_benchmark_suite(),
        BenchmarkTaskClass::Coding => coding_benchmark_suite(),
    }
}

/// Rejects Compact-vs-Canonical mixing, cross-suite mixing, duplicate/missing
/// cases and changed case definitions before any aggregate is calculated.
pub fn validate_benchmark_run_v2(run: &BenchmarkSuiteRunV2) -> Result<(), String> {
    if run.schema_version != BENCHMARK_SCHEMA_VERSION {
        return Err("benchmark_schema_version_mismatch".into());
    }
    if run.suite_id != run.task_class.suite_id() {
        return Err("benchmark_suite_task_class_mismatch".into());
    }
    if run.suite_version != BENCHMARK_SUITE_VERSION {
        return Err("benchmark_suite_version_mismatch".into());
    }
    if run.harness_version != BENCHMARK_HARNESS_VERSION {
        return Err("benchmark_harness_version_mismatch".into());
    }
    if run.model_config_fingerprint.is_empty() || run.run_id.is_empty() || run.timestamp == 0 {
        return Err("benchmark_run_identity_incomplete".into());
    }

    let expected = expected_suite(run.task_class);
    if run.cases.len() != expected.len() {
        return Err("benchmark_suite_incomplete".into());
    }
    let expected_by_id: HashMap<&str, &BenchmarkCase> =
        expected.iter().map(|case| (case.id, case)).collect();
    let mut seen = std::collections::HashSet::new();
    for result in &run.cases {
        if !seen.insert(result.case_id.as_str()) {
            return Err("benchmark_case_duplicate".into());
        }
        let case = expected_by_id
            .get(result.case_id.as_str())
            .ok_or_else(|| "benchmark_case_unexpected".to_string())?;
        let expected_fingerprint = benchmark_case_fingerprint(case, run.task_class);
        if result.case_fingerprint != expected_fingerprint {
            return Err("benchmark_case_fingerprint_mismatch".into());
        }
    }
    Ok(())
}

pub fn aggregate_benchmark_run_v2(
    run: &BenchmarkSuiteRunV2,
) -> Result<BenchmarkAggregateV2, String> {
    validate_benchmark_run_v2(run)?;
    if !run
        .cases
        .iter()
        .any(|case| case.outcome == BenchmarkOutcome::Success)
    {
        return Err("provider_execution_failed".into());
    }
    let count = run.cases.len() as f32;
    let successful = run
        .cases
        .iter()
        .filter(|case| case.outcome == BenchmarkOutcome::Success)
        .collect::<Vec<_>>();
    Ok(BenchmarkAggregateV2 {
        suite_id: run.suite_id.clone(),
        suite_version: run.suite_version.clone(),
        model_config_fingerprint: run.model_config_fingerprint.clone(),
        quality_score: successful
            .iter()
            .map(|case| case.quality_score)
            .sum::<f32>()
            / successful.len() as f32,
        availability_score: run.cases.iter().map(|c| c.availability_score).sum::<f32>() / count,
        latency_ms: (run.cases.iter().map(|c| c.latency_ms as u128).sum::<u128>()
            / run.cases.len() as u128) as u64,
        case_count: run.cases.len(),
    })
}

/// Adapter from the benchmark-v2 source of truth into the registry promotion
/// policy. No other evidence source can create a promotion credential.
pub fn registry_benchmark_evidence(
    run: &BenchmarkSuiteRunV2,
) -> Result<crate::model_registry::ValidatedBenchmarkEvidence, String> {
    validate_benchmark_run_v2(run)?;
    let aggregate = aggregate_benchmark_run_v2(run)?;
    Ok(
        crate::model_registry::ValidatedBenchmarkEvidence::from_validated_v2(
            crate::model_registry::BenchmarkEvidence {
                suite_id: run.suite_id.clone(),
                suite_version: run.suite_version.clone(),
                model_config_fingerprint: run.model_config_fingerprint.clone(),
                case_fingerprints: run
                    .cases
                    .iter()
                    .map(|case| (case.case_id.clone(), case.case_fingerprint.clone()))
                    .collect(),
                quality_score: aggregate.quality_score,
                availability_score: aggregate.availability_score,
                latency_ms: aggregate.latency_ms,
            },
        ),
    )
}

/// Aggregates repeated runs only when suite identity, suite version, task class,
/// model configuration and canonical case fingerprints all match.
pub fn aggregate_benchmark_runs_v2(
    runs: &[BenchmarkSuiteRunV2],
) -> Result<BenchmarkAggregateV2, String> {
    let first = runs
        .first()
        .ok_or_else(|| "benchmark_runs_empty".to_string())?;
    validate_benchmark_run_v2(first)?;
    for run in &runs[1..] {
        validate_benchmark_run_v2(run)?;
        if run.suite_id != first.suite_id
            || run.suite_version != first.suite_version
            || run.task_class != first.task_class
            || run.model_config_fingerprint != first.model_config_fingerprint
        {
            return Err("benchmark_runs_not_compatible".into());
        }
        if run
            .cases
            .iter()
            .map(|case| (&case.case_id, &case.case_fingerprint))
            .ne(first
                .cases
                .iter()
                .map(|case| (&case.case_id, &case.case_fingerprint)))
        {
            return Err("benchmark_run_fingerprints_differ".into());
        }
    }

    if !runs
        .iter()
        .flat_map(|run| &run.cases)
        .any(|case| case.outcome == BenchmarkOutcome::Success)
    {
        return Err("provider_execution_failed".into());
    }

    let case_count = runs.iter().map(|run| run.cases.len()).sum::<usize>();
    let denominator = case_count as f32;
    let successful = runs
        .iter()
        .flat_map(|run| &run.cases)
        .filter(|case| case.outcome == BenchmarkOutcome::Success)
        .collect::<Vec<_>>();
    Ok(BenchmarkAggregateV2 {
        suite_id: first.suite_id.clone(),
        suite_version: first.suite_version.clone(),
        model_config_fingerprint: first.model_config_fingerprint.clone(),
        quality_score: successful
            .iter()
            .map(|case| case.quality_score)
            .sum::<f32>()
            / successful.len() as f32,
        availability_score: runs
            .iter()
            .flat_map(|run| &run.cases)
            .map(|case| case.availability_score)
            .sum::<f32>()
            / denominator,
        latency_ms: (runs
            .iter()
            .flat_map(|run| &run.cases)
            .map(|case| case.latency_ms as u128)
            .sum::<u128>()
            / case_count as u128) as u64,
        case_count,
    })
}

pub fn work_benchmark_suite() -> Vec<BenchmarkCase> {
    vec![
        BenchmarkCase {
            id: "w01_instruction",
            name: "Präzise Instruktionsbefolgung",
            prompt: "Antworte exakt und ausschließlich mit dem Wort: Bereit",
            context: None,
            expected_fact: "bereit",
            requires_json: false,
            max_tokens: 15,
        },
        BenchmarkCase {
            id: "w02_doc_extract",
            name: "Dokumenten-Faktenextraktion",
            prompt: "Wie hoch ist die vereinbarte monatliche Vergütung laut Vertrag?",
            context: Some("Dienstleistungsvertrag: Die vereinbarte monatliche Vergütung beträgt netto 3.500 Euro."),
            expected_fact: "3.500",
            requires_json: false,
            max_tokens: 40,
        },
        BenchmarkCase {
            id: "w03_multi_doc_compare",
            name: "Mehrdokument-Vergleich",
            prompt: "Welcher Vertrag hat die längere Kündigungsfrist?",
            context: Some("Vertrag Alpha: Kündigungsfrist 14 Tage.\nVertrag Beta: Kündigungsfrist 30 Tage."),
            expected_fact: "beta",
            requires_json: false,
            max_tokens: 40,
        },
        BenchmarkCase {
            id: "w04_csv_numbers",
            name: "CSV/Zahlen-Statistik",
            prompt: "Wie hoch war der Mittelwert der Tabelle?",
            context: Some("[BERECHNETE_FAKTEN] Spalte Gewinn: Min 100 · Max 900 · Mittelwert 500 · Summe 2500"),
            expected_fact: "500",
            requires_json: false,
            max_tokens: 40,
        },
        BenchmarkCase {
            id: "w05_json_schema",
            name: "Strukturierte JSON-Ausgabe",
            prompt: "Gib ein valides JSON-Objekt mit den Schlüsseln 'status' (string 'ok') und 'code' (number 200) aus. Kein Markdown.",
            context: None,
            expected_fact: "200",
            requires_json: true,
            max_tokens: 40,
        },
        BenchmarkCase {
            id: "w06_tool_select",
            name: "MCP / Tool-Auswahlverständnis",
            prompt: "Welches Tool liest eine lokale Textdatei: 'fs.read' oder 'web.search'?",
            context: None,
            expected_fact: "fs.read",
            requires_json: false,
            max_tokens: 25,
        },
        BenchmarkCase {
            id: "w07_german_terminology",
            name: "Deutsche Fachterminologie",
            prompt: "Welche Steuer kann ein vorsteuerabzugsberechtigtes Unternehmen vom Finanzamt zurückfordern?",
            context: None,
            expected_fact: "vorsteuer",
            requires_json: false,
            max_tokens: 50,
        },
        BenchmarkCase {
            id: "w08_logic_deduction",
            name: "Logische Deduktion",
            prompt: "Alle Mitglieder von Team Rot sind Ingenieure. Lisa ist im Team Rot. Ist Lisa Ingenieurin? Antworte mit Ja oder Nein.",
            context: None,
            expected_fact: "ja",
            requires_json: false,
            max_tokens: 15,
        },
    ]
}

pub fn coding_benchmark_suite() -> Vec<BenchmarkCase> {
    vec![
        BenchmarkCase {
            id: "c01_off_by_one",
            name: "Off-by-one Bugfix",
            prompt: "Korrigiere den Indexfehler in Rust: `for i in 0..=vec.len() { vec[i]; }`. Wie lautet der korrekte Range?",
            context: None,
            expected_fact: "0..vec.len()",
            requires_json: false,
            max_tokens: 30,
        },
        BenchmarkCase {
            id: "c02_rust_borrow",
            name: "Rust Borrow Checker",
            prompt: "Welches Schlüsselwort fehlt bei `let s = String::new(); s.push_str(\"x\");`?",
            context: None,
            expected_fact: "mut",
            requires_json: false,
            max_tokens: 20,
        },
        BenchmarkCase {
            id: "c03_unified_diff",
            name: "Unified Diff Patch-Format",
            prompt: "Erzeuge einen gültigen Unified Diff Header für file.rs.",
            context: None,
            expected_fact: "---",
            requires_json: false,
            max_tokens: 40,
        },
        BenchmarkCase {
            id: "c04_js_async",
            name: "Async/Await Korrektheit",
            prompt: "Welches Schlüsselwort fehlt vor `fetch('/api')` in einer async function, um das Response-Objekt direkt zu erhalten?",
            context: None,
            expected_fact: "await",
            requires_json: false,
            max_tokens: 20,
        },
        BenchmarkCase {
            id: "c05_tauri_command",
            name: "Tauri IPC Command Signature",
            prompt: "Mit welchem Rust-Makro wird eine Funktion als Tauri-Frontend-Command registriert?",
            context: None,
            expected_fact: "tauri::command",
            requires_json: false,
            max_tokens: 25,
        },
        BenchmarkCase {
            id: "c06_test_repair",
            name: "Test-Assertion Reparatur",
            prompt: "Repariere den Test: `assert_eq!(2 + 2, 5);`. Was muss statt 5 stehen?",
            context: None,
            expected_fact: "4",
            requires_json: false,
            max_tokens: 15,
        },
        BenchmarkCase {
            id: "c07_compiler_diagnostic",
            name: "Compilerfehler-Analyse",
            prompt: "rustc meldet 'mismatched types: expected u64, found i32'. Wie wird x: i32 sicher nach u64 gecastet, wenn x >= 0?",
            context: None,
            expected_fact: "as u64",
            requires_json: false,
            max_tokens: 25,
        },
        BenchmarkCase {
            id: "c08_minimal_patch",
            name: "Minimalität / Keine Redundanz",
            prompt: "Gib nur die eine korrigierte Zeile für `return x * 2` aus, wenn x verdreifacht werden soll. Kein Markdown.",
            context: None,
            expected_fact: "x * 3",
            requires_json: false,
            max_tokens: 20,
        },
    ]
}

/// Computes the structured score for the Work Assistant benchmark.
/// Weights:
/// - 35% correctness
/// - 20% document/evidence fidelity
/// - 15% instruction following
/// - 10% structured/tool reliability
/// - 10% latency
/// - 10% availability/reliability
pub fn score_work_case(
    case: &BenchmarkCase,
    response: &str,
    latency_ms: u64,
    success: bool,
) -> f32 {
    if !success || response.trim().is_empty() {
        return 0.0;
    }
    let lower = response.to_lowercase();
    let expected = case.expected_fact.to_lowercase();

    let correctness = if lower.contains(&expected) { 1.0 } else { 0.0 };
    let fidelity = if case.context.is_some() {
        if correctness > 0.0 {
            1.0
        } else {
            0.2
        }
    } else {
        1.0
    };

    let instruction = if case.prompt.contains("exakt") || case.prompt.contains("ohne") {
        if response.lines().count() <= 2 {
            1.0
        } else {
            0.5
        }
    } else {
        1.0
    };

    let structured = if case.requires_json {
        if serde_json::from_str::<serde_json::Value>(response.trim()).is_ok() {
            1.0
        } else {
            0.0
        }
    } else {
        1.0
    };

    let latency_score = if latency_ms <= 1000 {
        1.0
    } else if latency_ms <= 3000 {
        0.8
    } else if latency_ms <= 6000 {
        0.5
    } else {
        0.2
    };

    let reliability = 1.0;

    let total = (0.35 * correctness)
        + (0.20 * fidelity)
        + (0.15 * instruction)
        + (0.10 * structured)
        + (0.10 * latency_score)
        + (0.10 * reliability);

    ((total * 100.0) as f32).clamp(0.0f32, 100.0f32)
}

/// Computes the structured score for the Coding benchmark.
/// Weights:
/// - 40% tests/build correctness
/// - 20% patch correctness
/// - 15% minimality
/// - 10% instruction following
/// - 10% latency
/// - 5% reliability
pub fn score_coding_case(
    case: &BenchmarkCase,
    response: &str,
    latency_ms: u64,
    success: bool,
) -> f32 {
    if !success || response.trim().is_empty() {
        return 0.0;
    }
    let lower = response.to_lowercase();
    let expected = case.expected_fact.to_lowercase();

    let correctness = if lower.contains(&expected) { 1.0 } else { 0.0 };
    let patch_correctness = if case.id == "c03_unified_diff" {
        if response.contains("---") && response.contains("+++") {
            1.0
        } else {
            0.5
        }
    } else {
        correctness
    };

    let minimality = if response.chars().count() <= 150 {
        1.0
    } else if response.chars().count() <= 400 {
        0.7
    } else {
        0.3
    };

    let instruction = if case.prompt.contains("Kein Markdown") || case.prompt.contains("ohne") {
        if !response.contains("```") && response.lines().count() <= 3 {
            1.0
        } else {
            0.5
        }
    } else {
        1.0
    };

    let latency_score = if latency_ms <= 1000 {
        1.0
    } else if latency_ms <= 3000 {
        0.8
    } else if latency_ms <= 6000 {
        0.5
    } else {
        0.2
    };

    let reliability = 1.0;

    let total = (0.40 * correctness)
        + (0.20 * patch_correctness)
        + (0.15 * minimality)
        + (0.10 * instruction)
        + (0.10 * latency_score)
        + (0.05 * reliability);

    ((total * 100.0) as f32).clamp(0.0f32, 100.0f32)
}

// ---------------------------------------------------------------------------
//  Cloud Engine State & Router
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkMetadataStatus {
    LegacyMissingV2,
    V2Validated,
}

impl Default for BenchmarkMetadataStatus {
    fn default() -> Self {
        Self::LegacyMissingV2
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchmarkResultsFile {
    pub updated_at: u64,
    pub provider_work_scores: HashMap<String, f32>,
    pub provider_code_scores: HashMap<String, f32>,
    /// `None` means the score maps are historical legacy data. They remain
    /// readable but can never pass Benchmark-v2 validation implicitly.
    #[serde(default)]
    pub schema_version: Option<u32>,
    #[serde(default)]
    pub benchmark_v2_runs: Vec<BenchmarkSuiteRunV2>,
    /// Old files deserialize explicitly as `legacy_missing_v2`; their scores
    /// remain available but are never promoted to validated v2 data.
    #[serde(default)]
    pub metadata_status: BenchmarkMetadataStatus,
}

impl BenchmarkResultsFile {
    pub fn legacy_metadata_missing(&self) -> bool {
        self.schema_version != Some(BENCHMARK_SCHEMA_VERSION)
            || self.benchmark_v2_runs.is_empty()
            || matches!(
                self.metadata_status,
                BenchmarkMetadataStatus::LegacyMissingV2
            )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchmarkRunReport {
    pub schema_version: u32,
    pub timestamp: u64,
    pub work_ranking: Vec<String>,
    pub code_ranking: Vec<String>,
    pub requests_made: usize,
    pub total_duration_ms: u64,
    pub total_tokens_used: u32,
    pub providers_cooldown: Vec<String>,
    pub tested_providers: Vec<String>,
    pub benchmark_v2_runs: Vec<BenchmarkSuiteRunV2>,
}

fn empty_benchmark_v2_run(
    config: &CloudProviderConfig,
    task_class: BenchmarkTaskClass,
    timestamp: u64,
    nonce: u128,
) -> BenchmarkSuiteRunV2 {
    BenchmarkSuiteRunV2 {
        schema_version: BENCHMARK_SCHEMA_VERSION,
        suite_id: task_class.suite_id().to_string(),
        suite_version: BENCHMARK_SUITE_VERSION.to_string(),
        harness_version: BENCHMARK_HARNESS_VERSION.to_string(),
        task_class,
        model_config_fingerprint: benchmark_model_config_fingerprint(config),
        run_id: format!("{}-{}-{nonce}", task_class.suite_id(), config.id),
        timestamp,
        cases: Vec::new(),
    }
}

fn benchmark_error_outcome(error: &ProviderExecError) -> BenchmarkOutcome {
    match error {
        ProviderExecError::RateLimited { .. } | ProviderExecError::QuotaExhausted(_) => {
            BenchmarkOutcome::RateLimited
        }
        ProviderExecError::Auth(_) => BenchmarkOutcome::AuthFailure,
        ProviderExecError::Unavailable(_) | ProviderExecError::CostBlocked(_) => {
            BenchmarkOutcome::Unavailable
        }
        ProviderExecError::Timeout(_) => BenchmarkOutcome::Timeout,
        ProviderExecError::EmptyResponse(_) | ProviderExecError::Failed(_) => {
            BenchmarkOutcome::Failed
        }
    }
}

fn benchmark_case_result(
    case: &BenchmarkCase,
    task_class: BenchmarkTaskClass,
    quality_score: f32,
    availability_score: f32,
    latency_ms: u64,
    outcome: BenchmarkOutcome,
) -> BenchmarkCaseResultV2 {
    BenchmarkCaseResultV2 {
        case_id: case.id.to_string(),
        case_fingerprint: benchmark_case_fingerprint(case, task_class),
        quality_score,
        availability_score,
        latency_ms,
        outcome,
    }
}

// ---------------------------------------------------------------------------
//  Benchmark-only Cloudflare Workers AI adapter
// ---------------------------------------------------------------------------

pub const CLOUDFLARE_GLM_BENCHMARK_MODEL: &str = "@cf/zai-org/glm-4.7-flash";
pub const GROQ_GPT_OSS_BENCHMARK_MODEL: &str = "openai/gpt-oss-120b";
// A new deterministic configuration generation. This deliberately differs
// from the incomplete quota-exhausted run and therefore has its own
// model_config_fingerprint; it is not evidence that can be merged with it.
const GROQ_GPT_OSS_BENCHMARK_SEED: u32 = 20_260_920;
/// Bounded provider-only completion allowance for GPT-OSS low-effort
/// reasoning. Canonical case limits remain the visible-answer contract.
const GROQ_GPT_OSS_REASONING_HEADROOM_TOKENS: u32 = 96;
const CLOUDFLARE_GLM_CONTEXT: u32 = 131_072;
const CLOUDFLARE_ACCOUNT_ID_SECRET: &str = "CLOUDFLARE_ACCOUNT_ID";
const CLOUDFLARE_API_TOKEN_SECRET: &str = "CLOUDFLARE_API_TOKEN";
const GROQ_API_KEY_SECRET: &str = "GROQ_API_KEY";

/// Detailed observations live beside, but never replace, canonical v2 data.
/// No prompt or response content is retained.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchmarkCaseTelemetry {
    pub task_class: BenchmarkTaskClass,
    pub case_id: String,
    pub passed: bool,
    pub summary: String,
    pub outcome: BenchmarkOutcome,
    pub latency_ms: u64,
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    pub reasoning_tokens: Option<u32>,
}

/// Persisted per-case execution evidence. It intentionally excludes prompts,
/// completion content, raw response bodies, error strings, and credentials.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchmarkExecutionDiagnostic {
    pub run_id: String,
    pub case_id: String,
    pub attempted: bool,
    pub provider: String,
    pub model: String,
    pub http_status: Option<u16>,
    pub runtime_outcome: crate::runtime_registry::RuntimeOutcome,
    pub finish_reason: Option<String>,
    pub content_present: Option<bool>,
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    pub reasoning_tokens: Option<u32>,
    pub latency_ms: u64,
    pub failure_category: Option<BenchmarkFailureCategory>,
    pub retry_after_ms: Option<u64>,
    pub reset_requests_ms: Option<u64>,
    pub reset_tokens_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CloudflareCanonicalBenchmarkReport {
    pub provider_id: String,
    pub model: String,
    pub cost_safety: String,
    pub billing_verification_method: String,
    pub cost_usd: f32,
    pub model_config_fingerprint: String,
    pub work: BenchmarkSuiteRunV2,
    pub coding: BenchmarkSuiteRunV2,
    pub telemetry: Vec<BenchmarkCaseTelemetry>,
    pub quota_remaining: Option<String>,
    pub quota_reset: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GroqCanonicalBenchmarkReport {
    pub provider_id: String,
    pub model: String,
    pub cost_safety: String,
    pub cost_usd: f32,
    pub model_config_fingerprint: String,
    pub work: BenchmarkSuiteRunV2,
    pub coding: BenchmarkSuiteRunV2,
    pub telemetry: Vec<BenchmarkCaseTelemetry>,
    pub execution_diagnostics: Vec<BenchmarkExecutionDiagnostic>,
    pub quota_remaining: Option<String>,
    pub quota_reset: Option<String>,
    pub throttle_wait_ms: u64,
    pub limiting_dimension: Option<String>,
    pub incomplete_reason: Option<String>,
}

fn persist_groq_execution_diagnostics(report: &GroqCanonicalBenchmarkReport) -> Result<(), String> {
    let directory = std::env::current_dir()
        .map_err(|error| format!("benchmark diagnostic cwd unavailable: {error}"))?
        .join(".local/benchmark-v2/groq");
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("benchmark diagnostic directory unavailable: {error}"))?;
    let run_id = report
        .work
        .run_id
        .replace(|character: char| !character.is_ascii_alphanumeric() && character != '-' && character != '_', "_");
    let destination = directory.join(format!("{run_id}.execution.json"));
    let temporary = directory.join(format!(".{run_id}.execution.tmp"));
    let payload = serde_json::to_vec_pretty(&report.execution_diagnostics)
        .map_err(|error| format!("benchmark diagnostic serialization failed: {error}"))?;
    std::fs::write(&temporary, payload)
        .map_err(|error| format!("benchmark diagnostic write failed: {error}"))?;
    std::fs::rename(&temporary, &destination)
        .map_err(|error| format!("benchmark diagnostic commit failed: {error}"))
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CloudflareCostSafetyAttestation {
    schema_version: u32,
    provider: String,
    connection_id: String,
    account_id_sha256: String,
    workers_plan: String,
    automatic_paid_overage: bool,
    source: String,
    verified_at: u64,
    freshness_seconds: u64,
    expires_at: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct GroqFreeLimitsAttestation {
    rpm: u32,
    rpd: u32,
    tpm: u32,
    tpd: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct GroqCostSafetyAttestation {
    schema_version: u32,
    provider: String,
    connection_id: String,
    account_identity_sha256: String,
    tier: String,
    automatic_paid_overage: bool,
    requires_explicit_upgrade: bool,
    model: String,
    limits: GroqFreeLimitsAttestation,
    source: String,
    verified_at: u64,
    freshness_seconds: u64,
    expires_at: u64,
}

fn validate_cloudflare_cost_attestation(
    attestation: &CloudflareCostSafetyAttestation,
    account_id: &str,
    now: u64,
) -> Result<(), String> {
    const MAX_FRESHNESS_SECONDS: u64 = 7 * 24 * 60 * 60;
    let account_hash = hex::encode(Sha256::digest(account_id.as_bytes()));
    if attestation.schema_version != 1
        || attestation.provider != "cloudflare_workers_ai"
        || attestation.connection_id != "benchmark_cloudflare_glm_4_7_flash"
        || attestation.account_id_sha256 != account_hash
        || attestation.workers_plan != "workers_free"
        || attestation.automatic_paid_overage
        || attestation.source != "user_verified_dashboard"
    {
        return Err("cloudflare_cost_attestation_identity_or_claim_mismatch".into());
    }
    if attestation.freshness_seconds == 0
        || attestation.freshness_seconds > MAX_FRESHNESS_SECONDS
        || attestation.expires_at
            != attestation
                .verified_at
                .saturating_add(attestation.freshness_seconds)
        || attestation.verified_at > now.saturating_add(300)
        || now >= attestation.expires_at
    {
        return Err("cloudflare_cost_attestation_stale_or_invalid".into());
    }
    Ok(())
}

fn cloudflare_local_cost_attestation(account_id: &str) -> Result<(), String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/cloudflare_workers_ai_cost_attestation.json");
    let raw = std::fs::read_to_string(path)
        .map_err(|_| "cloudflare_cost_attestation_missing".to_string())?;
    let attestation: CloudflareCostSafetyAttestation = serde_json::from_str(&raw)
        .map_err(|_| "cloudflare_cost_attestation_invalid_json".to_string())?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    validate_cloudflare_cost_attestation(&attestation, account_id, now)
}

fn validate_groq_cost_attestation(
    attestation: &GroqCostSafetyAttestation,
    api_key: &str,
    now: u64,
) -> Result<(), String> {
    const MAX_FRESHNESS_SECONDS: u64 = 7 * 24 * 60 * 60;
    let account_identity_hash = hex::encode(Sha256::digest(api_key.as_bytes()));
    if attestation.schema_version != 1
        || attestation.provider != "groq"
        || attestation.connection_id != "benchmark_groq_gpt_oss_120b"
        || attestation.account_identity_sha256 != account_identity_hash
        || attestation.tier != "free"
        || attestation.automatic_paid_overage
        || !attestation.requires_explicit_upgrade
        || attestation.model != GROQ_GPT_OSS_BENCHMARK_MODEL
        || attestation.limits.rpm != 30
        || attestation.limits.rpd != 1_000
        || attestation.limits.tpm != 8_000
        || attestation.limits.tpd != 200_000
        || attestation.source != "user_verified_dashboard"
    {
        return Err("groq_cost_attestation_identity_or_claim_mismatch".into());
    }
    if attestation.freshness_seconds == 0
        || attestation.freshness_seconds > MAX_FRESHNESS_SECONDS
        || attestation.expires_at
            != attestation
                .verified_at
                .saturating_add(attestation.freshness_seconds)
        || attestation.verified_at > now.saturating_add(300)
        || now >= attestation.expires_at
    {
        return Err("groq_cost_attestation_stale_or_invalid".into());
    }
    Ok(())
}

fn groq_local_cost_attestation(api_key: &str) -> Result<(), String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/groq_gpt_oss_120b_cost_attestation.json");
    let raw = std::fs::read_to_string(path)
        .map_err(|_| "groq_cost_attestation_missing_free_billing_uncertain".to_string())?;
    let attestation: GroqCostSafetyAttestation = serde_json::from_str(&raw)
        .map_err(|_| "groq_cost_attestation_invalid_free_billing_uncertain".to_string())?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    validate_groq_cost_attestation(&attestation, api_key, now)
}

pub fn renew_cloudflare_cost_attestation() -> Result<(), String> {
    let account_id = resolve_provider_secret(CLOUDFLARE_ACCOUNT_ID_SECRET)
        .ok_or_else(|| "CLOUDFLARE_ACCOUNT_ID fehlt im Keychain".to_string())?;
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/cloudflare_workers_ai_cost_attestation.json");
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| format!("Attestation file read error: {e}"))?;
    let mut attestation: CloudflareCostSafetyAttestation = serde_json::from_str(&raw)
        .map_err(|e| format!("Attestation JSON parse error: {e}"))?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    attestation.verified_at = now;
    attestation.freshness_seconds = 7 * 24 * 60 * 60;
    attestation.expires_at = now + attestation.freshness_seconds;
    attestation.account_id_sha256 = hex::encode(Sha256::digest(account_id.as_bytes()));

    let updated_raw = serde_json::to_string_pretty(&attestation)
        .map_err(|e| format!("Serialization error: {e}"))?;
    std::fs::write(&path, updated_raw)
        .map_err(|e| format!("Attestation write error: {e}"))?;
    validate_cloudflare_cost_attestation(&attestation, &account_id, now)?;
    Ok(())
}

pub fn renew_groq_cost_attestation() -> Result<(), String> {
    let api_key = resolve_provider_secret(GROQ_API_KEY_SECRET)
        .ok_or_else(|| "GROQ_API_KEY fehlt im Keychain".to_string())?;
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/groq_gpt_oss_120b_cost_attestation.json");
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| format!("Attestation file read error: {e}"))?;
    let mut attestation: GroqCostSafetyAttestation = serde_json::from_str(&raw)
        .map_err(|e| format!("Attestation JSON parse error: {e}"))?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    attestation.verified_at = now;
    attestation.freshness_seconds = 7 * 24 * 60 * 60;
    attestation.expires_at = now + attestation.freshness_seconds;
    attestation.account_identity_sha256 = hex::encode(Sha256::digest(api_key.as_bytes()));

    let updated_raw = serde_json::to_string_pretty(&attestation)
        .map_err(|e| format!("Serialization error: {e}"))?;
    std::fs::write(&path, updated_raw)
        .map_err(|e| format!("Attestation write error: {e}"))?;
    validate_groq_cost_attestation(&attestation, &api_key, now)?;
    Ok(())
}

fn groq_gpt_oss_benchmark_config() -> CloudProviderConfig {
    CloudProviderConfig {
        id: "benchmark_groq_gpt_oss_120b".into(),
        display_name: "GPT-OSS 120B on Groq (Benchmark only)".into(),
        model: GROQ_GPT_OSS_BENCHMARK_MODEL.into(),
        env_var: GROQ_API_KEY_SECRET.into(),
        endpoint: "https://api.groq.com/openai/v1/chat/completions".into(),
        supports_files: true,
        supports_vision: false,
        supports_tools: true,
        context: 131_072,
        free_only: true,
        target_suite: TargetSuite::Both,
        extra_body: Some(serde_json::json!({
            "reasoning_effort": "low",
            "include_reasoning": false,
            "seed": GROQ_GPT_OSS_BENCHMARK_SEED,
            "_noki_benchmark_reasoning_headroom_tokens": GROQ_GPT_OSS_REASONING_HEADROOM_TOKENS
        })),
    }
}

fn groq_read_json(url: &str, api_key: &str) -> Result<serde_json::Value, String> {
    let output = std::process::Command::new("/usr/bin/curl")
        .args([
            "-sS",
            "--compressed",
            "--max-time",
            "15",
            "-H",
            &format!("Authorization: Bearer {api_key}"),
            "-H",
            "Content-Type: application/json",
            url,
        ])
        .output()
        .map_err(|_| "groq_preflight_transport_failed".to_string())?;
    if !output.status.success() {
        return Err("groq_preflight_transport_failed".into());
    }
    serde_json::from_slice(&output.stdout).map_err(|_| "groq_preflight_invalid_json".to_string())
}

fn preflight_groq_gpt_oss(config: &CloudProviderConfig) -> Result<(), String> {
    if !config.free_only || config.model != GROQ_GPT_OSS_BENCHMARK_MODEL {
        return Err("groq_benchmark_cost_or_model_uncertain".into());
    }
    let api_key = resolve_provider_secret(GROQ_API_KEY_SECRET)
        .ok_or_else(|| "groq_api_key_missing".to_string())?;
    groq_local_cost_attestation(&api_key)?;
    let models = groq_read_json("https://api.groq.com/openai/v1/models", &api_key)?;
    if models.pointer("/object").and_then(serde_json::Value::as_str) != Some("list") {
        return Err("groq_auth_failed".into());
    }
    if !models
        .pointer("/data")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .any(|model| {
            model.pointer("/id").and_then(serde_json::Value::as_str)
                == Some(GROQ_GPT_OSS_BENCHMARK_MODEL)
                && model
                    .pointer("/active")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true)
        })
    {
        return Err("groq_exact_model_unavailable".into());
    }
    Ok(())
}

fn cloudflare_glm_benchmark_config_for_account(account_id: &str) -> CloudProviderConfig {
    CloudProviderConfig {
        id: "benchmark_cloudflare_glm_4_7_flash".into(),
        display_name: "Cloudflare GLM-4.7-Flash (Benchmark only)".into(),
        model: CLOUDFLARE_GLM_BENCHMARK_MODEL.into(),
        env_var: CLOUDFLARE_API_TOKEN_SECRET.into(),
        endpoint: format!(
            "https://api.cloudflare.com/client/v4/accounts/{account_id}/ai/v1/chat/completions"
        ),
        supports_files: false,
        supports_vision: false,
        supports_tools: true,
        context: CLOUDFLARE_GLM_CONTEXT,
        free_only: true,
        target_suite: TargetSuite::Both,
        // GLM-4.7 spends the canonical short output budgets entirely on
        // reasoning unless turn-level thinking is disabled. The model's own
        // supported template switch preserves the suite's output-token caps.
        extra_body: Some(serde_json::json!({
            "chat_template_kwargs": { "enable_thinking": false }
        })),
    }
}

/// This configuration is deliberately not returned by `known_cloud_configs`.
/// It cannot participate in routing and exists solely for an explicit v2 run.
pub fn cloudflare_glm_benchmark_config() -> Result<CloudProviderConfig, String> {
    let account_id = resolve_provider_secret(CLOUDFLARE_ACCOUNT_ID_SECRET)
        .ok_or_else(|| "cloudflare_account_id_missing".to_string())?;
    resolve_provider_secret(CLOUDFLARE_API_TOKEN_SECRET)
        .ok_or_else(|| "cloudflare_api_token_missing".to_string())?;
    Ok(cloudflare_glm_benchmark_config_for_account(&account_id))
}

fn cloudflare_read_json(url: &str, token: &str) -> Result<serde_json::Value, String> {
    let output = std::process::Command::new("/usr/bin/curl")
        .args([
            "-sS",
            "--compressed",
            "--max-time",
            "15",
            "-H",
            &format!("Authorization: Bearer {token}"),
            url,
        ])
        .output()
        .map_err(|_| "cloudflare_preflight_transport_failed".to_string())?;
    if !output.status.success() {
        return Err("cloudflare_preflight_transport_failed".into());
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|_| "cloudflare_preflight_invalid_json".to_string())
}

fn json_contains_exact_string(value: &serde_json::Value, expected: &str) -> bool {
    match value {
        serde_json::Value::String(value) => value == expected,
        serde_json::Value::Array(values) => values
            .iter()
            .any(|value| json_contains_exact_string(value, expected)),
        serde_json::Value::Object(values) => values
            .values()
            .any(|value| json_contains_exact_string(value, expected)),
        _ => false,
    }
}

fn cloudflare_has_active_paid_workers_subscription(value: &serde_json::Value) -> bool {
    value
        .pointer("/result")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .any(|subscription| {
            let state = subscription
                .pointer("/state")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase();
            let active = matches!(
                state.as_str(),
                "trial" | "provisioned" | "paid" | "awaitingpayment"
            );
            let plan = subscription
                .pointer("/rate_plan")
                .cloned()
                .unwrap_or(serde_json::Value::Null)
                .to_string()
                .to_ascii_lowercase();
            let workers_plan = plan.contains("worker");
            let priced = subscription
                .pointer("/price")
                .and_then(serde_json::Value::as_f64)
                .is_some_and(|price| price > 0.0);
            active && workers_plan && (priced || state != "provisioned")
        })
}

/// Read-only preflight: active token plus an account-scoped model catalog hit.
/// A successful catalog response proves account match and Workers AI Read.
fn preflight_cloudflare_glm(config: &CloudProviderConfig) -> Result<String, String> {
    if !config.free_only || config.model != CLOUDFLARE_GLM_BENCHMARK_MODEL {
        return Err("cloudflare_benchmark_cost_or_model_uncertain".into());
    }
    let token = resolve_provider_secret(CLOUDFLARE_API_TOKEN_SECRET)
        .ok_or_else(|| "cloudflare_api_token_missing".to_string())?;
    let auth = cloudflare_read_json(
        "https://api.cloudflare.com/client/v4/user/tokens/verify",
        &token,
    )?;
    if auth.pointer("/success").and_then(|v| v.as_bool()) != Some(true)
        || auth.pointer("/result/status").and_then(|v| v.as_str()) != Some("active")
    {
        return Err("cloudflare_auth_failed".into());
    }

    let account_id = resolve_provider_secret(CLOUDFLARE_ACCOUNT_ID_SECRET)
        .ok_or_else(|| "cloudflare_account_id_missing".to_string())?;
    // Direct Workers AI calls are a hard $0 stop only on Workers Free. On a
    // Workers Paid subscription, Cloudflare bills usage above the daily free
    // allocation. Fail closed unless Billing Read proves no active paid
    // Workers subscription; absence of permission is not treated as proof.
    let subscriptions_url =
        format!("https://api.cloudflare.com/client/v4/accounts/{account_id}/subscriptions");
    let subscriptions = cloudflare_read_json(&subscriptions_url, &token)?;
    let billing_verification_method =
        if subscriptions.pointer("/success").and_then(|v| v.as_bool()) == Some(true) {
            if cloudflare_has_active_paid_workers_subscription(&subscriptions) {
                return Err("cloudflare_workers_paid_plan_not_allowed".into());
            }
            "cloudflare_subscriptions_api"
        } else {
            cloudflare_local_cost_attestation(&account_id)?;
            "user_verified_dashboard_attestation"
        };
    let catalog_url = format!(
        "https://api.cloudflare.com/client/v4/accounts/{account_id}/ai/models/search?search=glm-4.7-flash&per_page=50"
    );
    let catalog = cloudflare_read_json(&catalog_url, &token)?;
    if catalog.pointer("/success").and_then(|v| v.as_bool()) != Some(true) {
        return Err("cloudflare_account_or_workers_ai_permission_failed".into());
    }
    if !json_contains_exact_string(&catalog, CLOUDFLARE_GLM_BENCHMARK_MODEL) {
        return Err("cloudflare_exact_model_unavailable".into());
    }
    Ok(billing_verification_method.into())
}

fn benchmark_case_passed(case: &BenchmarkCase, response: &str) -> (bool, String) {
    let expected = response
        .to_lowercase()
        .contains(&case.expected_fact.to_lowercase());
    if !expected {
        return (false, "expected fact missing".into());
    }
    if case.requires_json && serde_json::from_str::<serde_json::Value>(response.trim()).is_err() {
        return (false, "invalid JSON".into());
    }
    if case.id == "c03_unified_diff" && !response.contains("+++") {
        return (false, "incomplete unified diff header".into());
    }
    if (case.prompt.contains("Kein Markdown") || case.prompt.contains("ohne"))
        && (response.contains("```") || response.lines().count() > 3)
    {
        return (false, "unnecessary formatting".into());
    }
    (true, "expected result and output contract satisfied".into())
}

fn run_canonical_suite_for_config(
    config: &CloudProviderConfig,
    task_class: BenchmarkTaskClass,
    timestamp: u64,
    nonce: u128,
) -> (
    BenchmarkSuiteRunV2,
    Vec<BenchmarkCaseTelemetry>,
    Option<String>,
    Option<String>,
) {
    let mut run = empty_benchmark_v2_run(config, task_class, timestamp, nonce);
    let mut telemetry = Vec::new();
    let mut quota_remaining = None;
    let mut quota_reset = None;

    for case in expected_suite(task_class) {
        let prompt = match case.context {
            Some(context) => format!("{context}\n\n{}", case.prompt),
            None => case.prompt.to_string(),
        };
        let started = Instant::now();
        match execute_provider_request(config, &prompt, case.max_tokens, Duration::from_secs(15)) {
            Ok(response) => {
                let score = match task_class {
                    BenchmarkTaskClass::Work => {
                        score_work_case(&case, &response.text, response.latency_ms, true)
                    }
                    BenchmarkTaskClass::Coding => {
                        score_coding_case(&case, &response.text, response.latency_ms, true)
                    }
                };
                let (passed, summary) = benchmark_case_passed(&case, &response.text);
                quota_remaining = response.remaining_usage.clone().or(quota_remaining);
                quota_reset = response.reset_at.clone().or(quota_reset);
                run.cases.push(benchmark_case_result(
                    &case,
                    task_class,
                    score,
                    100.0,
                    response.latency_ms,
                    BenchmarkOutcome::Success,
                ));
                telemetry.push(BenchmarkCaseTelemetry {
                    task_class,
                    case_id: case.id.into(),
                    passed,
                    summary,
                    outcome: BenchmarkOutcome::Success,
                    latency_ms: response.latency_ms,
                    input_tokens: response.input_tokens,
                    output_tokens: response.output_tokens,
                    reasoning_tokens: response.reasoning_tokens,
                });
            }
            Err(error) => {
                let outcome = benchmark_error_outcome(&error);
                let stop = matches!(
                    error,
                    ProviderExecError::RateLimited { .. }
                        | ProviderExecError::QuotaExhausted(_)
                        | ProviderExecError::Auth(_)
                );
                let latency_ms = started.elapsed().as_millis() as u64;
                run.cases.push(benchmark_case_result(
                    &case, task_class, 0.0, 0.0, latency_ms, outcome,
                ));
                telemetry.push(BenchmarkCaseTelemetry {
                    task_class,
                    case_id: case.id.into(),
                    passed: false,
                    summary: outcome.as_str().into(),
                    outcome,
                    latency_ms,
                    input_tokens: None,
                    output_tokens: None,
                    reasoning_tokens: None,
                });
                if stop {
                    break;
                }
            }
        }
    }
    (run, telemetry, quota_remaining, quota_reset)
}

const MAX_GROQ_SHORT_WINDOW_WAIT: Duration = Duration::from_secs(90);

fn groq_provider_completion_budget(config: &CloudProviderConfig, canonical_visible_budget: u32) -> u32 {
    if config.id != "benchmark_groq_gpt_oss_120b" {
        return canonical_visible_budget;
    }
    let headroom = config
        .extra_body
        .as_ref()
        .and_then(|body| body.get("_noki_benchmark_reasoning_headroom_tokens"))
        .and_then(|value| value.as_u64())
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value == GROQ_GPT_OSS_REASONING_HEADROOM_TOKENS)
        .unwrap_or(0);
    canonical_visible_budget.saturating_add(headroom)
}

fn enforce_canonical_visible_answer_limit(
    text: &str,
    provider_output_tokens: Option<u32>,
    reasoning_tokens: Option<u32>,
    canonical_visible_budget: u32,
) -> String {
    let visible_tokens = provider_output_tokens
        .unwrap_or(0)
        .saturating_sub(reasoning_tokens.unwrap_or(0));
    if visible_tokens <= canonical_visible_budget || visible_tokens == 0 {
        return text.to_string();
    }
    // Provider usage supplies the exact overage ratio. Keep a deterministic,
    // UTF-8-safe prefix; no reasoning headroom can expand the visible answer.
    let keep_chars = (text.chars().count() as u64)
        .saturating_mul(canonical_visible_budget as u64)
        .checked_div(visible_tokens as u64)
        .unwrap_or(0) as usize;
    text.chars().take(keep_chars.max(1)).collect()
}

fn execute_groq_benchmark_case(
    config: &CloudProviderConfig,
    prompt: &str,
    canonical_visible_budget: u32,
    timeout: Duration,
) -> Result<ProviderExecutionResponse, ObservedProviderExecError> {
    let mut response = execute_provider_request_observed(
        config,
        prompt,
        &[],
        groq_provider_completion_budget(config, canonical_visible_budget),
        timeout,
    )?;
    response.text = enforce_canonical_visible_answer_limit(
        &response.text,
        response.output_tokens,
        response.reasoning_tokens,
        canonical_visible_budget,
    );
    Ok(response)
}

#[derive(Default)]
struct GroqThrottleState {
    remaining_requests: Option<u64>,
    remaining_tokens: Option<u64>,
    reset_tokens: Option<Duration>,
}

enum GroqPacingDecision {
    Proceed,
    Wait(Duration),
    DailyQuotaExhausted,
}

fn groq_estimated_case_tokens(prompt: &str, max_tokens: u32) -> u64 {
    // A deliberately conservative request estimate. It is only used to decide
    // whether the documented TPM window cannot fit the next canonical case.
    (prompt.len() as u64).div_ceil(4) + max_tokens as u64
}

fn groq_pacing_decision(state: &GroqThrottleState, needed_tokens: u64) -> GroqPacingDecision {
    // Groq's request headers represent RPD, not RPM: never sleep for a daily
    // limit. A zero daily balance is terminal for this run.
    if state.remaining_requests == Some(0) {
        return GroqPacingDecision::DailyQuotaExhausted;
    }
    if state.remaining_tokens.is_some_and(|remaining| remaining < needed_tokens) {
        if let Some(reset) = state.reset_tokens.filter(|reset| *reset <= MAX_GROQ_SHORT_WINDOW_WAIT)
        {
            return GroqPacingDecision::Wait(reset);
        }
    }
    GroqPacingDecision::Proceed
}

fn groq_429_dimension(reason: &str) -> &'static str {
    if reason.contains("groq_limit_dimension=tpm") {
        "tpm"
    } else if reason.contains("groq_limit_dimension=rpm") {
        "rpm"
    } else if reason.contains("groq_limit_dimension=tpd") {
        "tpd"
    } else if reason.contains("groq_limit_dimension=rpd") {
        "rpd"
    } else {
        "unknown"
    }
}

struct GroqSuiteRun {
    run: BenchmarkSuiteRunV2,
    telemetry: Vec<BenchmarkCaseTelemetry>,
    diagnostics: Vec<BenchmarkExecutionDiagnostic>,
    quota_remaining: Option<String>,
    quota_reset: Option<String>,
    throttle_wait_ms: u64,
    limiting_dimension: Option<String>,
    incomplete_reason: Option<String>,
}

fn groq_diagnostic(
    run_id: &str,
    case: &BenchmarkCase,
    config: &CloudProviderConfig,
    attempted: bool,
    runtime_outcome: crate::runtime_registry::RuntimeOutcome,
    observation: ProviderExecutionObservation,
    failure_category: Option<BenchmarkFailureCategory>,
) -> BenchmarkExecutionDiagnostic {
    BenchmarkExecutionDiagnostic {
        run_id: run_id.to_string(),
        case_id: case.id.to_string(),
        attempted,
        provider: config.id.clone(),
        model: config.model.clone(),
        http_status: observation.http_status,
        runtime_outcome,
        finish_reason: observation.finish_reason,
        content_present: observation.content_present,
        input_tokens: observation.input_tokens,
        output_tokens: observation.output_tokens,
        reasoning_tokens: observation.reasoning_tokens,
        latency_ms: observation.latency_ms,
        failure_category,
        retry_after_ms: observation.retry_after_ms,
        reset_requests_ms: observation.reset_requests_ms,
        reset_tokens_ms: observation.reset_tokens_ms,
    }
}

fn groq_harness_skip_diagnostics(
    config: &CloudProviderConfig,
    task_class: BenchmarkTaskClass,
    run_id: &str,
    already_recorded: &[BenchmarkExecutionDiagnostic],
) -> Vec<BenchmarkExecutionDiagnostic> {
    expected_suite(task_class)
        .into_iter()
        .filter(|case| !already_recorded.iter().any(|record| record.case_id == case.id))
        .map(|case| groq_diagnostic(
            run_id,
            &case,
            config,
            false,
            crate::runtime_registry::RuntimeOutcome::Cancelled,
            ProviderExecutionObservation::default(),
            Some(BenchmarkFailureCategory::HarnessSkip),
        ))
        .collect()
}

fn run_groq_canonical_suite_for_config(
    config: &CloudProviderConfig,
    task_class: BenchmarkTaskClass,
    timestamp: u64,
    nonce: u128,
) -> GroqSuiteRun {
    let mut run = empty_benchmark_v2_run(config, task_class, timestamp, nonce);
    let mut telemetry = Vec::new();
    let mut diagnostics: Vec<BenchmarkExecutionDiagnostic> = Vec::new();
    let mut quota_remaining = None;
    let mut quota_reset = None;
    let mut throttle = GroqThrottleState::default();
    let mut throttle_wait_ms = 0u64;
    let mut limiting_dimension = None;
    let mut incomplete_reason = None;

    'cases: for case in expected_suite(task_class) {
        let prompt = match case.context {
            Some(context) => format!("{context}\n\n{}", case.prompt),
            None => case.prompt.to_string(),
        };
        match groq_pacing_decision(&throttle, groq_estimated_case_tokens(&prompt, case.max_tokens)) {
            GroqPacingDecision::DailyQuotaExhausted => {
                incomplete_reason = Some("daily_rpd_exhausted".into());
                limiting_dimension = Some("rpd".into());
                break;
            }
            GroqPacingDecision::Wait(wait) => {
                let started = Instant::now();
                std::thread::sleep(wait);
                throttle_wait_ms += started.elapsed().as_millis() as u64;
                // The next response refreshes authoritative values; do not
                // decrement a header estimate locally.
                throttle.remaining_tokens = None;
                throttle.reset_tokens = None;
                limiting_dimension = Some("tpm".into());
            }
            GroqPacingDecision::Proceed => {}
        }

        let mut retried_after_429 = false;
        loop {
            match execute_groq_benchmark_case(config, &prompt, case.max_tokens, Duration::from_secs(15)) {
                Ok(response) => {
                    let score = match task_class {
                        BenchmarkTaskClass::Work => {
                            score_work_case(&case, &response.text, response.latency_ms, true)
                        }
                        BenchmarkTaskClass::Coding => {
                            score_coding_case(&case, &response.text, response.latency_ms, true)
                        }
                    };
                    let (passed, summary) = benchmark_case_passed(&case, &response.text);
                    quota_remaining = response.remaining_usage.clone().or(quota_remaining);
                    quota_reset = response.reset_at.clone().or(quota_reset);
                    throttle.remaining_requests = response.rate_limit_headers.remaining_requests;
                    throttle.remaining_tokens = response.rate_limit_headers.remaining_tokens;
                    throttle.reset_tokens = response.rate_limit_headers.reset_tokens;
                    run.cases.push(benchmark_case_result(
                        &case,
                        task_class,
                        score,
                        100.0,
                        response.latency_ms,
                        BenchmarkOutcome::Success,
                    ));
                    telemetry.push(BenchmarkCaseTelemetry {
                        task_class,
                        case_id: case.id.into(),
                        passed,
                        summary,
                        outcome: BenchmarkOutcome::Success,
                        latency_ms: response.latency_ms,
                        input_tokens: response.input_tokens,
                        output_tokens: response.output_tokens,
                        reasoning_tokens: response.reasoning_tokens,
                    });
                    diagnostics.push(groq_diagnostic(
                        &run.run_id,
                        &case,
                        config,
                        true,
                        if passed {
                            crate::runtime_registry::RuntimeOutcome::Success
                        } else {
                            crate::runtime_registry::RuntimeOutcome::QualityFailure
                        },
                        ProviderExecutionObservation {
                            attempted: true,
                            http_status: response.http_status,
                            finish_reason: response.finish_reason,
                            content_present: Some(response.content_present),
                            input_tokens: response.input_tokens,
                            output_tokens: response.output_tokens,
                            reasoning_tokens: response.reasoning_tokens,
                            latency_ms: response.latency_ms,
                            retry_after_ms: response.rate_limit_headers.retry_after.map(|d| d.as_millis() as u64),
                            reset_requests_ms: response.rate_limit_headers.reset_requests.map(|d| d.as_millis() as u64),
                            reset_tokens_ms: response.rate_limit_headers.reset_tokens.map(|d| d.as_millis() as u64),
                        },
                        (!passed).then_some(BenchmarkFailureCategory::QualityFail),
                    ));
                    break;
                }
                Err(failure) if matches!(failure.error, ProviderExecError::RateLimited { .. }) => {
                    let (cooldown, reason) = match &failure.error {
                        ProviderExecError::RateLimited { cooldown, reason } => (*cooldown, reason.as_str()),
                        _ => unreachable!(),
                    };
                    diagnostics.push(groq_diagnostic(
                        &run.run_id, &case, config, true,
                        failure.error.runtime_outcome(), failure.observation.clone(),
                        Some(failure.failure_category),
                    ));
                    let dimension = groq_429_dimension(reason);
                    limiting_dimension = Some(dimension.into());
                    if matches!(dimension, "rpd" | "tpd") {
                        incomplete_reason = Some(format!("daily_{dimension}_exhausted"));
                        break 'cases;
                    }
                    if !retried_after_429 && cooldown <= MAX_GROQ_SHORT_WINDOW_WAIT {
                        let started = Instant::now();
                        std::thread::sleep(cooldown);
                        throttle_wait_ms += started.elapsed().as_millis() as u64;
                        retried_after_429 = true;
                        continue;
                    }
                    incomplete_reason = Some(format!("short_window_{dimension}_limited"));
                    break 'cases;
                }
                Err(failure) => {
                    let outcome = benchmark_error_outcome(&failure.error);
                    let latency_ms = failure.observation.latency_ms;
                    run.cases.push(benchmark_case_result(
                        &case, task_class, 0.0, 0.0, latency_ms, outcome,
                    ));
                    telemetry.push(BenchmarkCaseTelemetry {
                        task_class,
                        case_id: case.id.into(),
                        passed: false,
                        summary: outcome.as_str().into(),
                        outcome,
                        latency_ms,
                        input_tokens: None,
                        output_tokens: None,
                        reasoning_tokens: None,
                    });
                    diagnostics.push(groq_diagnostic(
                        &run.run_id, &case, config, true,
                        failure.error.runtime_outcome(), failure.observation,
                        Some(failure.failure_category),
                    ));
                    break;
                }
            }
        }
    }
    diagnostics.extend(groq_harness_skip_diagnostics(
        config,
        task_class,
        &run.run_id,
        &diagnostics,
    ));
    GroqSuiteRun {
        run,
        telemetry,
        diagnostics,
        quota_remaining,
        quota_reset,
        throttle_wait_ms,
        limiting_dimension,
        incomplete_reason,
    }
}

impl BenchmarkOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Empty => "empty",
            Self::RateLimited => "rate_limited_or_quota",
            Self::Unavailable => "unavailable",
            Self::Timeout => "timeout",
            Self::AuthFailure => "auth_failure",
            Self::Failed => "failed",
        }
    }
}

/// Explicit live entry point. It is not reachable from routing or the UI.
pub fn run_cloudflare_glm_canonical_benchmark_v2(
) -> Result<CloudflareCanonicalBenchmarkReport, String> {
    let config = cloudflare_glm_benchmark_config()?;
    let billing_verification_method = preflight_cloudflare_glm(&config)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let timestamp = now.as_secs();
    let nonce = now.as_nanos();
    let fingerprint = benchmark_model_config_fingerprint(&config);

    let (work, mut telemetry, mut quota_remaining, mut quota_reset) =
        run_canonical_suite_for_config(&config, BenchmarkTaskClass::Work, timestamp, nonce);
    // Do not touch the shared Cloudflare pool after a safety-critical failure.
    if work.cases.len() != work_benchmark_suite().len()
        || work.cases.iter().any(|case| {
            matches!(
                case.outcome,
                BenchmarkOutcome::AuthFailure | BenchmarkOutcome::RateLimited
            )
        })
    {
        return Ok(CloudflareCanonicalBenchmarkReport {
            provider_id: config.id.clone(),
            model: config.model.clone(),
            cost_safety: "VerifiedFreeHardStop".into(),
            billing_verification_method,
            cost_usd: 0.0,
            model_config_fingerprint: fingerprint,
            coding: empty_benchmark_v2_run(&config, BenchmarkTaskClass::Coding, timestamp, nonce),
            work,
            telemetry,
            quota_remaining,
            quota_reset,
        });
    }

    let (coding, coding_telemetry, coding_quota, coding_reset) =
        run_canonical_suite_for_config(&config, BenchmarkTaskClass::Coding, timestamp, nonce);
    telemetry.extend(coding_telemetry);
    quota_remaining = coding_quota.or(quota_remaining);
    quota_reset = coding_reset.or(quota_reset);
    Ok(CloudflareCanonicalBenchmarkReport {
        provider_id: config.id,
        model: config.model,
        cost_safety: "VerifiedFreeHardStop".into(),
        billing_verification_method,
        cost_usd: 0.0,
        model_config_fingerprint: fingerprint,
        work,
        coding,
        telemetry,
        quota_remaining,
        quota_reset,
    })
}

pub fn run_groq_gpt_oss_canonical_benchmark_v2() -> Result<GroqCanonicalBenchmarkReport, String> {
    let config = groq_gpt_oss_benchmark_config();
    preflight_groq_gpt_oss(&config)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let timestamp = now.as_secs();
    let nonce = now.as_nanos();
    let fingerprint = benchmark_model_config_fingerprint(&config);
    let mut work = run_groq_canonical_suite_for_config(
        &config,
        BenchmarkTaskClass::Work,
        timestamp,
        nonce,
    );
    if work.run.cases.len() != work_benchmark_suite().len()
        || work.incomplete_reason.is_some()
        || work.run.cases.iter().any(|case| case.outcome != BenchmarkOutcome::Success)
    {
        work.incomplete_reason.get_or_insert_with(|| "provider_execution_incomplete".into());
    }
    // Work and Coding are independent canonical suites. A transport failure in
    // either is recorded on that suite but must never turn the other into a
    // harness skip or suppress its execution evidence.
    let mut coding = run_groq_canonical_suite_for_config(
        &config,
        BenchmarkTaskClass::Coding,
        timestamp,
        nonce,
    );
    if coding.incomplete_reason.is_none()
        && coding
            .run
            .cases
            .iter()
            .any(|case| case.outcome != BenchmarkOutcome::Success)
    {
        coding.incomplete_reason = Some("provider_execution_incomplete".into());
    }
    let mut telemetry = work.telemetry;
    telemetry.extend(coding.telemetry);
    let mut execution_diagnostics = work.diagnostics;
    execution_diagnostics.extend(coding.diagnostics);
    let report = GroqCanonicalBenchmarkReport {
        provider_id: config.id,
        model: config.model,
        cost_safety: "VerifiedFreeHardStop".into(),
        cost_usd: 0.0,
        model_config_fingerprint: fingerprint,
        work: work.run,
        coding: coding.run,
        telemetry,
        execution_diagnostics,
        quota_remaining: coding.quota_remaining.or(work.quota_remaining),
        quota_reset: coding.quota_reset.or(work.quota_reset),
        throttle_wait_ms: work.throttle_wait_ms + coding.throttle_wait_ms,
        limiting_dimension: coding.limiting_dimension.or(work.limiting_dimension),
        incomplete_reason: coding.incomplete_reason.or(work.incomplete_reason),
    };
    persist_groq_execution_diagnostics(&report)?;
    Ok(report)
}

pub struct CloudEngine {
    pub mode: EngineMode,
    pub providers: Vec<CloudProvider>,
    pub disabled_ids: Vec<String>,
}

impl CloudEngine {
    fn refresh_provider_projection(&mut self, id: &str) {
        if let Some(provider) = self.providers.iter_mut().find(|provider| provider.id == id) {
            provider.refresh_legacy_projection();
        }
    }

    pub fn new() -> Self {
        let configs = known_cloud_configs();
        let mut providers = Vec::new();

        // Baseline pre-calibrated benchmark scores (empirically tested for $0 models)
        // Gemini 3.8 Flash is Work #1; Qwen 3.8 27B on Groq is Coding #1.
        // OpenRouter tournament winners: Nex-N2.5 Pro (99.8 Work, 85.0 Code),
        // Nemotron 3 Ultra (87.5 Work, 72.5 Code), DeepSeek V4 Flash (74.5 Work, 80.0 Code).
        let default_work_scores: HashMap<&str, f32> = [
            ("gemini_free", 94.0),
            ("groq_qwen", 91.5),
            ("groq_gpt_oss", 89.0),
            ("openrouter_free", 99.8),
            ("openrouter_nemotron", 87.5),
            ("openrouter_deepseek", 74.5),
            ("mistral_free", 82.0),
        ]
        .into_iter()
        .collect();

        let default_code_scores: HashMap<&str, f32> = [
            ("groq_qwen", 95.0),
            ("gemini_free", 92.0),
            ("groq_gpt_oss", 90.5),
            ("openrouter_free", 85.0),
            ("openrouter_nemotron", 72.5),
            ("openrouter_deepseek", 80.0),
            ("mistral_free", 81.0),
        ]
        .into_iter()
        .collect();

        for cfg in configs {
            let has_key = resolve_provider_secret(&cfg.env_var).is_some();
            if let Some(model_id) = canonical_model_id_for_cloud_engine(&cfg.id) {
                let _ = crate::runtime_registry::set_auth_state(
                    model_id,
                    if has_key {
                        crate::runtime_registry::AuthState::Ready
                    } else {
                        crate::runtime_registry::AuthState::Missing
                    },
                );
            }

            let w_score = default_work_scores.get(cfg.id.as_str()).copied();
            let c_score = default_code_scores.get(cfg.id.as_str()).copied();

            providers.push(CloudProvider {
                id: cfg.id.clone(),
                display_name: cfg.display_name.clone(),
                available: has_key,
                free_only: cfg.free_only,
                model: cfg.model.clone(),
                supports_files: cfg.supports_files,
                supports_vision: cfg.supports_vision,
                supports_tools: cfg.supports_tools,
                context: cfg.context,
                rate_limit: None,
                remaining_usage: None,
                reset_at: None,
                latency: None,
                health: if has_key {
                    "available".to_string()
                } else {
                    "not_configured".to_string()
                },
                work_rank: None,
                code_rank: None,
                work_score: w_score,
                code_score: c_score,
                enabled: true,
                last_error: None,
                total_tokens_used: None,
                cooldown_until: None,
            });
            if let Some(provider) = providers.last_mut() {
                provider.refresh_legacy_projection();
            }
        }

        let mut engine = Self {
            mode: EngineMode::LocalAndCloud,
            providers,
            disabled_ids: Vec::new(),
        };
        engine.recompute_rankings();
        engine
    }

    /// Recomputes independent work and coding rankings from scores.
    pub fn recompute_rankings(&mut self) {
        // 1. Work ranking
        let mut work_list: Vec<(usize, f32)> = self
            .providers
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.work_score.map(|s| (i, s)))
            .collect();
        work_list.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        for (rank, (idx, _)) in work_list.into_iter().enumerate() {
            self.providers[idx].work_rank = Some(rank + 1);
        }

        // 2. Coding ranking
        let mut code_list: Vec<(usize, f32)> = self
            .providers
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.code_score.map(|s| (i, s)))
            .collect();
        code_list.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        for (rank, (idx, _)) in code_list.into_iter().enumerate() {
            self.providers[idx].code_rank = Some(rank + 1);
        }
    }

    /// Returns the sorted routing chain for Work tasks.
    pub fn work_routing_chain(&self) -> Vec<String> {
        let mut list: Vec<&CloudProvider> = self
            .providers
            .iter()
            .filter(|p| p.is_available() && p.work_rank.is_some())
            .collect();
        list.sort_by_key(|p| p.work_rank.unwrap_or(999));
        let mut res: Vec<String> = list.into_iter().map(|p| p.display_name.clone()).collect();
        res.push("Qwen 3.5 (lokal)".into());
        res
    }

    /// Returns the sorted routing chain for Coding tasks.
    pub fn code_routing_chain(&self) -> Vec<String> {
        let mut list: Vec<&CloudProvider> = self
            .providers
            .iter()
            .filter(|p| p.is_available() && p.code_rank.is_some())
            .collect();
        list.sort_by_key(|p| p.code_rank.unwrap_or(999));
        let mut res: Vec<String> = list.into_iter().map(|p| p.display_name.clone()).collect();
        res.push("JackOD 9B (lokal)".into());
        res
    }

    /// Toggles enabling a specific cloud provider.
    pub fn set_provider_enabled(&mut self, id: &str, enabled: bool) -> bool {
        if let Some(p) = self.providers.iter_mut().find(|p| p.id == id) {
            if let Some(model_id) = canonical_model_id_for_cloud_engine(id) {
                let _ = crate::runtime_registry::set_provider_enabled(
                    crate::model_registry::model(model_id)
                        .map(|model| model.provider_id)
                        .unwrap_or_default(),
                    enabled,
                );
                p.refresh_legacy_projection();
            }
            if enabled {
                self.disabled_ids.retain(|x| x != id);
            } else if !self.disabled_ids.contains(&id.to_string()) {
                self.disabled_ids.push(id.to_string());
            }
            true
        } else {
            false
        }
    }

    /// Sets the engine mode (LocalAndCloud or OnlyLocal).
    pub fn set_mode(&mut self, mode: EngineMode) {
        self.mode = mode;
    }

    /// Updates scores manually or from saved benchmarks and recomputes the rankings.
    pub fn apply_benchmarks(
        &mut self,
        work_scores: HashMap<String, f32>,
        code_scores: HashMap<String, f32>,
    ) {
        for p in &mut self.providers {
            if let Some(ws) = work_scores.get(&p.id) {
                p.work_score = Some(*ws);
            }
            if let Some(cs) = code_scores.get(&p.id) {
                p.code_score = Some(*cs);
            }
        }
        self.recompute_rankings();
    }

    /// Runs real compact benchmarks sequentially across all configured providers.
    /// Max 8 Work + 8 Coding tests, 1 request per test, strict timeout, no retries.
    pub fn run_benchmarks(&mut self) -> BenchmarkRunReport {
        let start = Instant::now();
        let system_now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let benchmark_timestamp = system_now.as_secs();
        let run_nonce = system_now.as_nanos();
        let mut requests_made = 0;
        let mut total_tokens = 0;
        let mut providers_cooldown = Vec::new();
        let mut tested_providers = Vec::new();
        let mut benchmark_v2_runs = Vec::new();

        let configs = known_cloud_configs();
        let work_cases = work_benchmark_suite();
        let code_cases = coding_benchmark_suite();

        for cfg in configs {
            let is_candidate = self
                .providers
                .iter()
                .any(|p| p.id == cfg.id && p.is_available());
            if !is_candidate {
                continue;
            }

            tested_providers.push(cfg.id.clone());
            let mut w_scores = Vec::new();
            let mut c_scores = Vec::new();
            let mut is_rate_limited = false;
            let mut work_v2 = empty_benchmark_v2_run(
                &cfg,
                BenchmarkTaskClass::Work,
                benchmark_timestamp,
                run_nonce,
            );
            let mut coding_v2 = empty_benchmark_v2_run(
                &cfg,
                BenchmarkTaskClass::Coding,
                benchmark_timestamp,
                run_nonce,
            );

            // 1. Work Suite (8 compact cases)
            for case in &work_cases {
                requests_made += 1;
                let case_started = Instant::now();
                let full_prompt = match case.context {
                    Some(ctx) => format!("{ctx}\n\n{}", case.prompt),
                    None => case.prompt.to_string(),
                };
                match execute_provider_request(
                    &cfg,
                    &full_prompt,
                    case.max_tokens,
                    Duration::from_secs(10),
                ) {
                    Ok(resp) => {
                        let score = score_work_case(case, &resp.text, resp.latency_ms, true);
                        w_scores.push(score);
                        work_v2.cases.push(benchmark_case_result(
                            case,
                            BenchmarkTaskClass::Work,
                            score,
                            100.0,
                            resp.latency_ms,
                            BenchmarkOutcome::Success,
                        ));
                        if let Some(t) = resp.tokens {
                            total_tokens += t;
                        }
                    }
                    Err(ProviderExecError::RateLimited { .. }) => {
                        work_v2.cases.push(benchmark_case_result(
                            case,
                            BenchmarkTaskClass::Work,
                            0.0,
                            0.0,
                            case_started.elapsed().as_millis() as u64,
                            BenchmarkOutcome::RateLimited,
                        ));
                        providers_cooldown.push(cfg.id.clone());
                        is_rate_limited = true;
                        break;
                    }
                    Err(err) => {
                        work_v2.cases.push(benchmark_case_result(
                            case,
                            BenchmarkTaskClass::Work,
                            0.0,
                            0.0,
                            case_started.elapsed().as_millis() as u64,
                            benchmark_error_outcome(&err),
                        ));
                        w_scores.push(0.0);
                    }
                }
            }

            // 2. Coding Suite (8 compact cases)
            if !is_rate_limited {
                for case in &code_cases {
                    requests_made += 1;
                    let case_started = Instant::now();
                    let full_prompt = match case.context {
                        Some(ctx) => format!("{ctx}\n\n{}", case.prompt),
                        None => case.prompt.to_string(),
                    };
                    match execute_provider_request(
                        &cfg,
                        &full_prompt,
                        case.max_tokens,
                        Duration::from_secs(10),
                    ) {
                        Ok(resp) => {
                            let score = score_coding_case(case, &resp.text, resp.latency_ms, true);
                            c_scores.push(score);
                            coding_v2.cases.push(benchmark_case_result(
                                case,
                                BenchmarkTaskClass::Coding,
                                score,
                                100.0,
                                resp.latency_ms,
                                BenchmarkOutcome::Success,
                            ));
                            if let Some(t) = resp.tokens {
                                total_tokens += t;
                            }
                        }
                        Err(ProviderExecError::RateLimited { .. }) => {
                            coding_v2.cases.push(benchmark_case_result(
                                case,
                                BenchmarkTaskClass::Coding,
                                0.0,
                                0.0,
                                case_started.elapsed().as_millis() as u64,
                                BenchmarkOutcome::RateLimited,
                            ));
                            providers_cooldown.push(cfg.id.clone());
                            break;
                        }
                        Err(err) => {
                            coding_v2.cases.push(benchmark_case_result(
                                case,
                                BenchmarkTaskClass::Coding,
                                0.0,
                                0.0,
                                case_started.elapsed().as_millis() as u64,
                                benchmark_error_outcome(&err),
                            ));
                            c_scores.push(0.0);
                        }
                    }
                }
            }

            // Productive rankings may only consume complete canonical suites.
            // Partial data remains in the report for diagnosis but is never
            // averaged into a provider score.
            if validate_benchmark_run_v2(&work_v2).is_ok() {
                let avg_w = w_scores.iter().sum::<f32>() / (w_scores.len() as f32);
                if let Some(p) = self.providers.iter_mut().find(|p| p.id == cfg.id) {
                    p.work_score = Some(avg_w);
                }
            }
            if validate_benchmark_run_v2(&coding_v2).is_ok() {
                let avg_c = c_scores.iter().sum::<f32>() / (c_scores.len() as f32);
                if let Some(p) = self.providers.iter_mut().find(|p| p.id == cfg.id) {
                    p.code_score = Some(avg_c);
                }
            }
            benchmark_v2_runs.push(work_v2);
            if !coding_v2.cases.is_empty() {
                benchmark_v2_runs.push(coding_v2);
            }
        }

        self.recompute_rankings();

        let work_ranking = self.work_routing_chain();
        let code_ranking = self.code_routing_chain();
        let total_duration_ms = start.elapsed().as_millis() as u64;

        BenchmarkRunReport {
            schema_version: BENCHMARK_SCHEMA_VERSION,
            timestamp: benchmark_timestamp,
            work_ranking,
            code_ranking,
            requests_made,
            total_duration_ms,
            total_tokens_used: total_tokens,
            providers_cooldown,
            tested_providers,
            benchmark_v2_runs,
        }
    }

    /// Routes a Work Assistant task through the Work ranking.
    pub fn route_work(
        &mut self,
        prompt: &str,
        sensitive: bool,
        max_tokens: u32,
        mock_invoker: Option<&dyn Fn(&str, &str) -> Result<(String, u64, Option<String>), String>>,
    ) -> Option<CloudRoutingOutcome> {
        if self.mode == EngineMode::OnlyLocal || sensitive {
            return None;
        }
        let mut candidate_ids: Vec<(String, usize)> = self
            .providers
            .iter()
            .filter(|p| p.is_available() && p.work_rank.is_some())
            .map(|p| (p.id.clone(), p.work_rank.unwrap()))
            .collect();
        candidate_ids.sort_by_key(|x| x.1);

        if candidate_ids.is_empty() {
            return None;
        }

        let configs = known_cloud_configs();
        let mut attempts = 0;

        for (id, _) in candidate_ids {
            attempts += 1;
            let cfg = match configs.iter().find(|c| c.id == id) {
                Some(c) => c,
                None => continue,
            };
            note_cloud_request(cfg);

            if let Some(mock) = mock_invoker {
                match mock(&cfg.id, prompt) {
                    Ok((ans, lat, usage)) => {
                        note_cloud_success(cfg, lat, usage.clone(), None);
                        self.refresh_provider_projection(&cfg.id);
                        return Some(CloudRoutingOutcome {
                            provider_id: cfg.id.clone(),
                            model: cfg.model.clone(),
                            answer: ans,
                            latency_ms: lat,
                            attempts,
                        });
                    }
                    Err(err) => {
                        let provider_error = provider_error_from_message(err.clone());
                        let cooldown = match &provider_error {
                            ProviderExecError::RateLimited { cooldown, .. } => Some(*cooldown),
                            ProviderExecError::QuotaExhausted(_) => Some(Duration::from_secs(60)),
                            ProviderExecError::Timeout(_) | ProviderExecError::Unavailable(_) => {
                                Some(Duration::from_secs(15))
                            }
                            _ => None,
                        };
                        note_cloud_failure(cfg, &provider_error, cooldown);
                        self.refresh_provider_projection(&cfg.id);
                        continue;
                    }
                }
            }

            match execute_provider_request(cfg, prompt, max_tokens, Duration::from_secs(10)) {
                Ok(resp) => {
                    note_cloud_success(
                        cfg,
                        resp.latency_ms,
                        resp.remaining_usage.clone(),
                        resp.reset_at.clone(),
                    );
                    note_cloud_quota_telemetry(cfg, resp.quota_telemetry.clone());
                    self.refresh_provider_projection(&cfg.id);
                    return Some(CloudRoutingOutcome {
                        provider_id: cfg.id.clone(),
                        model: cfg.model.clone(),
                        answer: resp.text,
                        latency_ms: resp.latency_ms,
                        attempts,
                    });
                }
                Err(ProviderExecError::RateLimited { cooldown, reason }) => {
                    note_cloud_failure(
                        cfg,
                        &ProviderExecError::RateLimited {
                            cooldown,
                            reason: reason.clone(),
                        },
                        Some(cooldown),
                    );
                    self.refresh_provider_projection(&cfg.id);
                    continue;
                }
                Err(err) => {
                    note_cloud_failure(cfg, &err, Some(Duration::from_secs(30)));
                    self.refresh_provider_projection(&cfg.id);
                    log::warn!(
                        "Cloud Work Provider {} failed: {}. Failing over to next provider.",
                        cfg.id,
                        err
                    );
                    continue;
                }
            }
        }
        None
    }

    /// Routes a Coding task through the Coding ranking.
    pub fn route_code(
        &mut self,
        prompt: &str,
        sensitive: bool,
        max_tokens: u32,
        mock_invoker: Option<&dyn Fn(&str, &str) -> Result<(String, u64, Option<String>), String>>,
    ) -> Option<CloudRoutingOutcome> {
        if self.mode == EngineMode::OnlyLocal || sensitive {
            return None;
        }
        let mut candidate_ids: Vec<(String, usize)> = self
            .providers
            .iter()
            .filter(|p| p.is_available() && p.code_rank.is_some())
            .map(|p| (p.id.clone(), p.code_rank.unwrap()))
            .collect();
        candidate_ids.sort_by_key(|x| x.1);

        if candidate_ids.is_empty() {
            return None;
        }

        let configs = known_cloud_configs();
        let mut attempts = 0;

        for (id, _) in candidate_ids {
            attempts += 1;
            let cfg = match configs.iter().find(|c| c.id == id) {
                Some(c) => c,
                None => continue,
            };
            note_cloud_request(cfg);

            if let Some(mock) = mock_invoker {
                match mock(&cfg.id, prompt) {
                    Ok((ans, lat, usage)) => {
                        note_cloud_success(cfg, lat, usage.clone(), None);
                        self.refresh_provider_projection(&cfg.id);
                        return Some(CloudRoutingOutcome {
                            provider_id: cfg.id.clone(),
                            model: cfg.model.clone(),
                            answer: ans,
                            latency_ms: lat,
                            attempts,
                        });
                    }
                    Err(err) => {
                        let provider_error = provider_error_from_message(err.clone());
                        let cooldown = match &provider_error {
                            ProviderExecError::RateLimited { cooldown, .. } => Some(*cooldown),
                            ProviderExecError::QuotaExhausted(_) => Some(Duration::from_secs(60)),
                            ProviderExecError::Timeout(_) | ProviderExecError::Unavailable(_) => {
                                Some(Duration::from_secs(15))
                            }
                            _ => None,
                        };
                        note_cloud_failure(cfg, &provider_error, cooldown);
                        self.refresh_provider_projection(&cfg.id);
                        continue;
                    }
                }
            }

            match execute_provider_request(cfg, prompt, max_tokens, Duration::from_secs(10)) {
                Ok(resp) => {
                    note_cloud_success(
                        cfg,
                        resp.latency_ms,
                        resp.remaining_usage.clone(),
                        resp.reset_at.clone(),
                    );
                    note_cloud_quota_telemetry(cfg, resp.quota_telemetry.clone());
                    self.refresh_provider_projection(&cfg.id);
                    return Some(CloudRoutingOutcome {
                        provider_id: cfg.id.clone(),
                        model: cfg.model.clone(),
                        answer: resp.text,
                        latency_ms: resp.latency_ms,
                        attempts,
                    });
                }
                Err(ProviderExecError::RateLimited { cooldown, reason }) => {
                    note_cloud_failure(
                        cfg,
                        &ProviderExecError::RateLimited {
                            cooldown,
                            reason: reason.clone(),
                        },
                        Some(cooldown),
                    );
                    self.refresh_provider_projection(&cfg.id);
                    continue;
                }
                Err(err) => {
                    note_cloud_failure(cfg, &err, Some(Duration::from_secs(30)));
                    self.refresh_provider_projection(&cfg.id);
                    log::warn!(
                        "Cloud Code Provider {} failed: {}. Failing over to next provider.",
                        cfg.id,
                        err
                    );
                    continue;
                }
            }
        }
        None
    }
}

static GLOBAL_CLOUD_ENGINE: OnceLock<Arc<Mutex<CloudEngine>>> = OnceLock::new();

pub fn get_cloud_engine() -> std::sync::MutexGuard<'static, CloudEngine> {
    GLOBAL_CLOUD_ENGINE
        .get_or_init(|| Arc::new(Mutex::new(CloudEngine::new())))
        .lock()
        .unwrap()
}

pub fn cloud_engine_status_json() -> serde_json::Value {
    let mut engine = get_cloud_engine();
    for provider in &mut engine.providers {
        provider.refresh_legacy_projection();
    }
    serde_json::json!({
        "engine_mode": engine.mode.as_str(),
        "mode_label": engine.mode.display_label(),
        "work_routing": engine.work_routing_chain(),
        "code_routing": engine.code_routing_chain(),
        "providers": engine.providers,
    })
}

// ---------------------------------------------------------------------------
//  Runtime Router: Work & Coding with Fallback Chains
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct CloudRoutingOutcome {
    pub provider_id: String,
    pub model: String,
    pub answer: String,
    pub latency_ms: u64,
    pub attempts: usize,
}

pub fn route_work_task(
    prompt: &str,
    sensitive: bool,
    max_tokens: u32,
    mock_invoker: Option<&dyn Fn(&str, &str) -> Result<(String, u64, Option<String>), String>>,
) -> Option<CloudRoutingOutcome> {
    let mut engine = get_cloud_engine();
    engine.route_work(prompt, sensitive, max_tokens, mock_invoker)
}

pub fn route_code_task(
    prompt: &str,
    sensitive: bool,
    max_tokens: u32,
    mock_invoker: Option<&dyn Fn(&str, &str) -> Result<(String, u64, Option<String>), String>>,
) -> Option<CloudRoutingOutcome> {
    let mut engine = get_cloud_engine();
    engine.route_code(prompt, sensitive, max_tokens, mock_invoker)
}

/// Runs the Work benchmark suite against a provider evaluation function.
pub fn run_work_benchmark_suite<F>(provider_id: &str, mut invoker: F) -> f32
where
    F: FnMut(&BenchmarkCase) -> Result<(String, u64), String>,
{
    let cases = work_benchmark_suite();
    let count = cases.len() as f32;
    let mut total_score = 0.0f32;

    for case in &cases {
        let (resp, latency) = match invoker(case) {
            Ok((r, l)) => (r, l),
            Err(_) => (String::new(), 10_000),
        };
        let s = score_work_case(case, &resp, latency, !resp.is_empty());
        total_score += s;
    }

    let avg = total_score / count;
    let mut engine = get_cloud_engine();
    if let Some(p) = engine.providers.iter_mut().find(|p| p.id == provider_id) {
        p.work_score = Some(avg);
    }
    engine.recompute_rankings();
    avg
}

/// Runs the Coding benchmark suite against a provider evaluation function.
pub fn run_coding_benchmark_suite<F>(provider_id: &str, mut invoker: F) -> f32
where
    F: FnMut(&BenchmarkCase) -> Result<(String, u64), String>,
{
    let cases = coding_benchmark_suite();
    let count = cases.len() as f32;
    let mut total_score = 0.0f32;

    for case in &cases {
        let (resp, latency) = match invoker(case) {
            Ok((r, l)) => (r, l),
            Err(_) => (String::new(), 10_000),
        };
        let s = score_coding_case(case, &resp, latency, !resp.is_empty());
        total_score += s;
    }

    let avg = total_score / count;
    let mut engine = get_cloud_engine();
    if let Some(p) = engine.providers.iter_mut().find(|p| p.id == provider_id) {
        p.code_score = Some(avg);
    }
    engine.recompute_rankings();
    avg
}

/// Updates scores manually or from saved benchmarks and recomputes the rankings.
pub fn apply_benchmark_results(
    work_scores: HashMap<String, f32>,
    code_scores: HashMap<String, f32>,
) {
    let mut engine = get_cloud_engine();
    engine.apply_benchmarks(work_scores, code_scores);
}

// ---------------------------------------------------------------------------
//  Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn complete_v2_run(task_class: BenchmarkTaskClass) -> BenchmarkSuiteRunV2 {
        let cfg = known_cloud_configs().into_iter().next().unwrap();
        let mut run = empty_benchmark_v2_run(&cfg, task_class, 1, 1);
        run.cases = expected_suite(task_class)
            .iter()
            .map(|case| {
                benchmark_case_result(
                    case,
                    task_class,
                    80.0,
                    100.0,
                    500,
                    BenchmarkOutcome::Success,
                )
            })
            .collect();
        run
    }

    #[test]
    fn test_cloud_providers_initialized_free_only() {
        let engine = CloudEngine::new();
        assert_eq!(engine.mode, EngineMode::LocalAndCloud);
        for p in &engine.providers {
            assert!(p.free_only, "Every CloudProvider must be free_only: true");
            assert!(p.work_rank.is_some() || p.code_rank.is_some());
        }
    }

    #[test]
    fn runtime_update_is_reflected_in_cloud_provider_projection() {
        crate::runtime_registry::clear();
        let mut engine = CloudEngine::new();
        let model_id = canonical_model_id_for_cloud_engine("mistral_free").unwrap();
        let mut observation = crate::runtime_registry::RuntimeObservation::outcome(
            crate::runtime_registry::RuntimeOutcome::RateLimited,
            crate::runtime_registry::OutcomeScope::Model,
        );
        observation.cooldown_until = Some(crate::runtime_registry::deadline_after(
            Duration::from_secs(60),
        ));
        crate::runtime_registry::observe(model_id, observation).unwrap();

        let provider = engine
            .providers
            .iter_mut()
            .find(|p| p.id == "mistral_free")
            .unwrap();
        provider.refresh_legacy_projection();
        assert!(!provider.available);
        assert_eq!(provider.health, "rate_limited");
        assert!(provider.cooldown_until.is_some());
    }

    #[test]
    fn legacy_projection_cannot_override_runtime_selection() {
        crate::runtime_registry::clear();
        let mut engine = CloudEngine::new();
        let model_id = canonical_model_id_for_cloud_engine("gemini_free").unwrap();
        let mut observation = crate::runtime_registry::RuntimeObservation::outcome(
            crate::runtime_registry::RuntimeOutcome::AuthenticationFailed,
            crate::runtime_registry::OutcomeScope::Connection,
        );
        observation.cooldown_until = Some(crate::runtime_registry::deadline_after(
            Duration::from_secs(60),
        ));
        crate::runtime_registry::observe(model_id, observation).unwrap();

        let provider = engine
            .providers
            .iter_mut()
            .find(|p| p.id == "gemini_free")
            .unwrap();
        provider.available = true;
        provider.health = "available".to_string();
        assert!(!provider.is_available());
    }

    #[test]
    fn benchmark_outcome_is_evaluation_only() {
        crate::runtime_registry::clear();
        let cfg = known_cloud_configs().into_iter().next().unwrap();
        let error = ProviderExecError::Auth("benchmark fixture".into());
        assert_eq!(
            benchmark_error_outcome(&error),
            BenchmarkOutcome::AuthFailure
        );
        let model_id = canonical_model_id_for_cloud_engine(&cfg.id).unwrap();
        let runtime = crate::runtime_registry::snapshot(model_id).unwrap();
        assert_eq!(runtime.model.last_outcome, None);
        assert_eq!(
            runtime.effective_availability,
            crate::runtime_registry::RuntimeAvailability::Available
        );
    }

    #[test]
    fn test_separate_rankings_differ() {
        let engine = CloudEngine::new();
        let gemini = engine
            .providers
            .iter()
            .find(|p| p.id == "gemini_free")
            .unwrap();
        let groq_qwen = engine
            .providers
            .iter()
            .find(|p| p.id == "groq_qwen")
            .unwrap();
        let openrouter_free = engine
            .providers
            .iter()
            .find(|p| p.id == "openrouter_free")
            .unwrap();

        // Nex-N2.5 Pro is Work #1 but Coding #4
        assert_eq!(openrouter_free.work_rank, Some(1));
        assert_eq!(openrouter_free.code_rank, Some(4));

        // Gemini is Work #2 and Coding #2
        assert_eq!(gemini.work_rank, Some(2));
        assert_eq!(gemini.code_rank, Some(2));

        // Groq Qwen is Coding #1 but Work #3
        assert_eq!(groq_qwen.code_rank, Some(1));
        assert_eq!(groq_qwen.work_rank, Some(3));
    }

    #[test]
    fn test_rate_limit_header_parsing_with_and_without_usage() {
        // Headers with usage
        let raw_with_usage = "HTTP/1.1 200 OK\r\nx-ratelimit-remaining-requests: 17\r\nx-ratelimit-remaining-tokens: 182000\r\nx-ratelimit-reset-requests: 3h\r\n";
        let (usage, reset, _, _) = parse_rate_limit_headers(raw_with_usage);
        assert_eq!(usage, Some("17 req · 182000 tokens".into()));
        assert_eq!(reset, Some("3h".into()));

        // Headers without usage (must return None, NEVER guess)
        let raw_no_usage = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nDate: Fri, 18 Sep 2026 18:00:00 GMT\r\n";
        let (usage_none, reset_none, _, _) = parse_rate_limit_headers(raw_no_usage);
        assert_eq!(usage_none, None);
        assert_eq!(reset_none, None);
    }

    #[test]
    fn groq_rate_headers_keep_short_tpm_separate_from_daily_rpd() {
        let raw = concat!(
            "HTTP/1.1 200 OK\r\n",
            "x-ratelimit-limit-requests: 1000\r\n",
            "x-ratelimit-remaining-requests: 982\r\n",
            "x-ratelimit-reset-requests: 25m55.199s\r\n",
            "x-ratelimit-limit-tokens: 8000\r\n",
            "x-ratelimit-remaining-tokens: 120\r\n",
            "x-ratelimit-reset-tokens: 1.5s\r\n",
            "retry-after: 2\r\n",
        );
        let headers = parse_provider_rate_limit_headers(raw);
        assert_eq!(headers.limit_requests, Some(1000));
        assert_eq!(headers.remaining_requests, Some(982));
        assert_eq!(headers.limit_tokens, Some(8000));
        assert_eq!(headers.remaining_tokens, Some(120));
        assert_eq!(headers.reset_requests, Some(Duration::from_millis(1_555_199)));
        assert_eq!(headers.reset_tokens, Some(Duration::from_millis(1_500)));
        assert_eq!(headers.retry_after, Some(Duration::from_secs(2)));

        let config = CloudProviderConfig {
            id: "groq_gpt_oss_120b".into(), display_name: "fixture".into(), model: "fixture".into(),
            env_var: "fixture".into(), endpoint: "https://fixture.invalid".into(), supports_files: false,
            supports_vision: false, supports_tools: false, context: 1, free_only: true,
            target_suite: TargetSuite::Both, extra_body: None,
        };
        let telemetry = quota_telemetry_for_response(&config, &headers).unwrap();
        assert_eq!(telemetry.source, "provider_live");
        assert_eq!(telemetry.scope, "account");
        assert_eq!(telemetry.request_remaining, Some(982));
        assert_eq!(telemetry.token_limit, Some(8000));
    }

    #[test]
    fn cloudflare_static_free_cap_is_shared_and_never_claims_remaining() {
        let config = CloudProviderConfig {
            id: "cloudflare_glm_4_7_flash".into(), display_name: "fixture".into(), model: "fixture".into(),
            env_var: "fixture".into(), endpoint: "https://fixture.invalid".into(), supports_files: false,
            supports_vision: false, supports_tools: false, context: 1, free_only: true,
            target_suite: TargetSuite::Both, extra_body: None,
        };
        let telemetry = quota_telemetry_for_response(&config, &ProviderRateLimitHeaders::default()).unwrap();
        assert_eq!(telemetry.source, "static_limit");
        assert_eq!(telemetry.scope, "account");
        assert_eq!(telemetry.neuron_limit, Some(10_000));
        assert_eq!(telemetry.neuron_remaining, None);
    }

    #[test]
    fn mistral_headers_remain_shared_provider_evidence() {
        let headers = parse_provider_rate_limit_headers(concat!(
            "HTTP/1.1 200 OK\r\n",
            "x-ratelimit-limit-requests: 60\r\n",
            "x-ratelimit-remaining-requests: 41\r\n",
            "x-ratelimit-reset-requests: 30s\r\n",
        ));
        let config = CloudProviderConfig {
            id: "mistral_ministral_8b".into(), display_name: "fixture".into(), model: "fixture".into(),
            env_var: "fixture".into(), endpoint: "https://fixture.invalid".into(), supports_files: false,
            supports_vision: false, supports_tools: false, context: 1, free_only: true,
            target_suite: TargetSuite::Both, extra_body: None,
        };
        let telemetry = quota_telemetry_for_response(&config, &headers).unwrap();
        assert_eq!(telemetry.source, "provider_live");
        assert_eq!(telemetry.scope, "provider");
        assert_eq!(telemetry.request_limit, Some(60));
        assert_eq!(telemetry.request_remaining, Some(41));
    }

    #[test]
    fn generic_shared_and_project_rate_headers_reach_canonical_telemetry() {
        let headers = parse_provider_rate_limit_headers(concat!(
            "HTTP/1.1 200 OK\r\n",
            "RateLimit-Limit: 50;w=60\r\n",
            "RateLimit-Remaining: 31\r\n",
            "RateLimit-Reset: 42\r\n",
        ));
        assert_eq!(headers.limit_requests, Some(50));
        assert_eq!(headers.remaining_requests, Some(31));
        assert_eq!(headers.reset_requests, Some(Duration::from_secs(42)));
        let config = |id: &str| CloudProviderConfig {
            id: id.into(), display_name: "fixture".into(), model: "fixture".into(),
            env_var: "fixture".into(), endpoint: "https://fixture.invalid".into(), supports_files: false,
            supports_vision: false, supports_tools: false, context: 1, free_only: true,
            target_suite: TargetSuite::Both, extra_body: None,
        };
        let openrouter = quota_telemetry_for_response(&config("openrouter_deepseek_v4_flash"), &headers).unwrap();
        assert_eq!(openrouter.scope, "account");
        assert_eq!(openrouter.request_remaining, Some(31));
        let gemini = quota_telemetry_for_response(&config("google_gemini_3_8_flash"), &headers).unwrap();
        assert_eq!(gemini.scope, "project");
        assert_eq!(gemini.request_limit, Some(50));
    }

    #[test]
    fn legacy_cloud_engine_id_resolves_to_groq_canonical_quota_scope() {
        let headers = parse_provider_rate_limit_headers("x-ratelimit-limit-requests: 100\r\nx-ratelimit-remaining-requests: 99\r\n");
        let config = CloudProviderConfig {
            id: "groq_gpt_oss".into(), display_name: "fixture".into(), model: "fixture".into(),
            env_var: "fixture".into(), endpoint: "https://fixture.invalid".into(), supports_files: false,
            supports_vision: false, supports_tools: false, context: 1, free_only: true,
            target_suite: TargetSuite::Both, extra_body: None,
        };
        assert_eq!(quota_telemetry_for_response(&config, &headers).unwrap().scope, "account");
    }

    #[test]
    fn groq_pacing_waits_only_for_tpm_and_never_for_rpd() {
        let short = GroqThrottleState {
            remaining_requests: Some(900),
            remaining_tokens: Some(100),
            reset_tokens: Some(Duration::from_secs(2)),
        };
        match groq_pacing_decision(&short, 101) {
            GroqPacingDecision::Wait(wait) => assert_eq!(wait, Duration::from_secs(2)),
            _ => panic!("TPM should request a short-window wait"),
        }
        let daily = GroqThrottleState {
            remaining_requests: Some(0),
            remaining_tokens: Some(8_000),
            reset_tokens: Some(Duration::from_secs(1)),
        };
        assert!(matches!(
            groq_pacing_decision(&daily, 1),
            GroqPacingDecision::DailyQuotaExhausted
        ));
        assert_eq!(groq_429_dimension("groq_limit_dimension=tpm"), "tpm");
        assert_eq!(groq_429_dimension("groq_limit_dimension=rpd"), "rpd");
    }

    #[test]
    fn provider_errors_use_the_canonical_runtime_outcomes() {
        use crate::runtime_registry::RuntimeOutcome;

        assert_eq!(
            ProviderExecError::EmptyResponse("empty".into()).runtime_outcome(),
            RuntimeOutcome::EmptyResponse
        );
        assert_eq!(
            provider_error_from_message("free quota exhausted".into()).runtime_outcome(),
            RuntimeOutcome::QuotaExhausted
        );
        assert_eq!(
            provider_error_from_message("curl timeout nach 15s".into()).runtime_outcome(),
            RuntimeOutcome::Timeout
        );
    }

    #[test]
    fn test_benchmark_scoring_formulas() {
        let work_case = BenchmarkCase {
            id: "w02",
            name: "Doc extract",
            prompt: "Wie hoch ist die Vergütung?",
            context: Some("Vergütung: 3.500 Euro."),
            expected_fact: "3.500",
            requires_json: false,
            max_tokens: 50,
        };
        let score_ok = score_work_case(
            &work_case,
            "Die Vergütung beträgt genau 3.500 Euro.",
            850,
            true,
        );
        assert!(
            score_ok >= 90.0,
            "Score should be high for correct fast answer: {score_ok}"
        );

        let score_bad = score_work_case(&work_case, "Keine Angabe im Text.", 1200, true);
        assert!(
            score_bad < 50.0,
            "Score should be low for missing fact: {score_bad}"
        );

        let code_case = BenchmarkCase {
            id: "c01",
            name: "Off by one",
            prompt: "Fix range",
            context: None,
            expected_fact: "0..vec.len()",
            requires_json: false,
            max_tokens: 40,
        };
        let code_ok = score_coding_case(&code_case, "0..vec.len()", 600, true);
        assert!(code_ok >= 90.0);
    }

    #[test]
    fn benchmark_v2_rejects_changed_case_fingerprint() {
        let mut run = complete_v2_run(BenchmarkTaskClass::Work);
        assert!(validate_benchmark_run_v2(&run).is_ok());
        run.cases[0].case_fingerprint = "compact-or-different-suite".into();
        assert_eq!(
            validate_benchmark_run_v2(&run).unwrap_err(),
            "benchmark_case_fingerprint_mismatch"
        );
        assert!(aggregate_benchmark_run_v2(&run).is_err());
    }

    #[test]
    fn benchmark_v2_rejects_incomplete_suite() {
        let mut run = complete_v2_run(BenchmarkTaskClass::Coding);
        run.cases.pop();
        assert_eq!(
            validate_benchmark_run_v2(&run).unwrap_err(),
            "benchmark_suite_incomplete"
        );
        assert!(aggregate_benchmark_run_v2(&run).is_err());
    }

    #[test]
    fn benchmark_v2_zero_success_is_invalid_not_zero_quality() {
        let mut run = complete_v2_run(BenchmarkTaskClass::Work);
        for case in &mut run.cases {
            case.outcome = BenchmarkOutcome::Failed;
            case.quality_score = 0.0;
            case.availability_score = 0.0;
        }
        assert_eq!(
            aggregate_benchmark_run_v2(&run).unwrap_err(),
            "provider_execution_failed"
        );
        assert!(registry_benchmark_evidence(&run).is_err());
    }

    #[test]
    fn benchmark_v2_availability_failure_does_not_become_quality_zero() {
        let mut run = complete_v2_run(BenchmarkTaskClass::Work);
        run.cases[0].outcome = BenchmarkOutcome::Failed;
        run.cases[0].quality_score = 0.0;
        run.cases[0].availability_score = 0.0;
        for case in &mut run.cases[1..] {
            case.quality_score = 80.0;
        }
        let aggregate = aggregate_benchmark_run_v2(&run).unwrap();
        assert_eq!(aggregate.quality_score, 80.0);
        assert_eq!(aggregate.availability_score, 87.5);
    }

    #[test]
    fn benchmark_v2_keeps_work_and_coding_separate() {
        let mut run = complete_v2_run(BenchmarkTaskClass::Work);
        run.suite_id = CODING_SUITE_ID.into();
        assert_eq!(
            validate_benchmark_run_v2(&run).unwrap_err(),
            "benchmark_suite_task_class_mismatch"
        );

        let work = complete_v2_run(BenchmarkTaskClass::Work);
        let coding = complete_v2_run(BenchmarkTaskClass::Coding);
        assert!(aggregate_benchmark_runs_v2(&[work, coding]).is_err());
    }

    #[test]
    fn only_validated_matching_benchmark_v2_can_reach_promotion_policy() {
        let run = complete_v2_run(BenchmarkTaskClass::Coding);
        let evidence = registry_benchmark_evidence(&run).expect("complete canonical suite");
        let fingerprints = run
            .cases
            .iter()
            .map(|case| (case.case_id.clone(), case.case_fingerprint.clone()))
            .collect::<Vec<_>>();
        let context = crate::model_registry::PromotionContext {
            role: crate::model_registry::RoutingRole::CodingNormal,
            lane: crate::model_registry::ExecutionLane::FreeCloud,
            privacy_allows_lane: true,
            provider_usable: true,
            required_context_tokens: 32_768,
            needs_reasoning: false,
            needs_tools: true,
            needs_structured_output: true,
            expected_suite_id: &run.suite_id,
            expected_suite_version: &run.suite_version,
            expected_model_config_fingerprint: &run.model_config_fingerprint,
            expected_case_fingerprints: &fingerprints,
            benefit: Some(crate::model_registry::PromotionBenefit::BeatsCurrentChampion),
        };
        assert!(crate::model_registry::evaluate_promotion(
            &crate::model_registry::NEX_N2_5_PRO,
            Some(&evidence),
            &context,
        )
        .is_ok());
    }

    #[test]
    fn historical_score_file_is_preserved_and_marked_legacy() {
        let old = r#"{
            "updated_at": 1,
            "provider_work_scores": {"legacy": 87.5},
            "provider_code_scores": {"legacy": 78.8}
        }"#;
        let parsed: BenchmarkResultsFile = serde_json::from_str(old).unwrap();
        assert_eq!(parsed.provider_work_scores["legacy"], 87.5);
        assert_eq!(parsed.provider_code_scores["legacy"], 78.8);
        assert!(matches!(
            parsed.metadata_status,
            BenchmarkMetadataStatus::LegacyMissingV2
        ));
        assert!(parsed.legacy_metadata_missing());
    }

    #[test]
    fn cloudflare_glm_adapter_is_benchmark_only_and_pinned() {
        let account = "0123456789abcdef0123456789abcdef";
        let config = cloudflare_glm_benchmark_config_for_account(account);
        assert_eq!(config.id, "benchmark_cloudflare_glm_4_7_flash");
        assert_eq!(config.model, CLOUDFLARE_GLM_BENCHMARK_MODEL);
        assert_eq!(config.context, 131_072);
        assert!(config.free_only);
        assert_eq!(config.target_suite, TargetSuite::Both);
        assert_eq!(
            config.endpoint,
            format!(
                "https://api.cloudflare.com/client/v4/accounts/{account}/ai/v1/chat/completions"
            )
        );
        assert!(known_cloud_configs()
            .iter()
            .all(|known| known.id != config.id && known.model != config.model));
        assert!(canonical_model_id_for_cloud_engine(&config.id).is_none());
        let mut invalid_old_config = config.clone();
        invalid_old_config.extra_body = None;
        assert_ne!(
            benchmark_model_config_fingerprint(&invalid_old_config),
            benchmark_model_config_fingerprint(&config)
        );
    }

    #[test]
    fn cloudflare_credentials_use_canonical_resolver_wiring() {
        let config = cloudflare_glm_benchmark_config_for_account("account-fixture");
        assert_eq!(CLOUDFLARE_ACCOUNT_ID_SECRET, "CLOUDFLARE_ACCOUNT_ID");
        assert_eq!(CLOUDFLARE_API_TOKEN_SECRET, "CLOUDFLARE_API_TOKEN");
        assert_eq!(config.env_var, CLOUDFLARE_API_TOKEN_SECRET);

        let (value, source) =
            select_provider_secret(Some(" env-value ".into()), Some("keychain-value".into()));
        assert_eq!(value.as_deref(), Some("env-value"));
        assert_eq!(source, ProviderSecretSource::Env);

        let (value, source) =
            select_provider_secret(Some(" ".into()), Some(" keychain-value ".into()));
        assert_eq!(value.as_deref(), Some("keychain-value"));
        assert_eq!(source, ProviderSecretSource::Keychain);

        let (value, source) = select_provider_secret(None, None);
        assert!(value.is_none());
        assert_eq!(source, ProviderSecretSource::Missing);
    }

    #[test]
    fn cloudflare_model_catalog_requires_the_exact_model_id() {
        let exact = serde_json::json!({
            "success": true,
            "result": [{ "name": CLOUDFLARE_GLM_BENCHMARK_MODEL }]
        });
        let lookalike = serde_json::json!({
            "success": true,
            "result": [{ "name": "@cf/zai-org/glm-4.7-flash-preview" }]
        });
        assert!(json_contains_exact_string(
            &exact,
            CLOUDFLARE_GLM_BENCHMARK_MODEL
        ));
        assert!(!json_contains_exact_string(
            &lookalike,
            CLOUDFLARE_GLM_BENCHMARK_MODEL
        ));
    }

    #[test]
    fn cloudflare_cost_guard_rejects_active_paid_workers_plans() {
        let free = serde_json::json!({
            "success": true,
            "result": [{
                "state": "Provisioned",
                "price": 0,
                "rate_plan": { "id": "workers_free", "public_name": "Workers Free" }
            }]
        });
        let paid = serde_json::json!({
            "success": true,
            "result": [{
                "state": "Paid",
                "price": 5,
                "rate_plan": { "id": "workers_paid", "public_name": "Workers Paid" }
            }]
        });
        assert!(!cloudflare_has_active_paid_workers_subscription(&free));
        assert!(cloudflare_has_active_paid_workers_subscription(&paid));
    }

    #[test]
    fn cloudflare_dashboard_attestation_is_scoped_and_expires() {
        let account_id = "account-fixture";
        let verified_at = 1_000_000;
        let mut attestation = CloudflareCostSafetyAttestation {
            schema_version: 1,
            provider: "cloudflare_workers_ai".into(),
            connection_id: "benchmark_cloudflare_glm_4_7_flash".into(),
            account_id_sha256: hex::encode(Sha256::digest(account_id.as_bytes())),
            workers_plan: "workers_free".into(),
            automatic_paid_overage: false,
            source: "user_verified_dashboard".into(),
            verified_at,
            freshness_seconds: 604_800,
            expires_at: verified_at + 604_800,
        };
        assert!(
            validate_cloudflare_cost_attestation(&attestation, account_id, verified_at + 1).is_ok()
        );
        assert!(validate_cloudflare_cost_attestation(
            &attestation,
            account_id,
            attestation.expires_at
        )
        .is_err());
        attestation.account_id_sha256 = "different-account".into();
        assert!(
            validate_cloudflare_cost_attestation(&attestation, account_id, verified_at + 1)
                .is_err()
        );
    }

    #[test]
    fn groq_dashboard_attestation_is_scoped_and_expires() {
        let api_key = "groq-key-fixture";
        let verified_at = 1_000_000;
        let mut attestation = GroqCostSafetyAttestation {
            schema_version: 1,
            provider: "groq".into(),
            connection_id: "benchmark_groq_gpt_oss_120b".into(),
            account_identity_sha256: hex::encode(Sha256::digest(api_key.as_bytes())),
            tier: "free".into(),
            automatic_paid_overage: false,
            requires_explicit_upgrade: true,
            model: GROQ_GPT_OSS_BENCHMARK_MODEL.into(),
            limits: GroqFreeLimitsAttestation {
                rpm: 30,
                rpd: 1_000,
                tpm: 8_000,
                tpd: 200_000,
            },
            source: "user_verified_dashboard".into(),
            verified_at,
            freshness_seconds: 604_800,
            expires_at: verified_at + 604_800,
        };
        assert!(validate_groq_cost_attestation(&attestation, api_key, verified_at + 1).is_ok());
        assert!(validate_groq_cost_attestation(&attestation, api_key, attestation.expires_at)
            .is_err());
        attestation.account_identity_sha256 = "different-account".into();
        assert!(validate_groq_cost_attestation(&attestation, api_key, verified_at + 1).is_err());
    }

    #[test]
    fn groq_benchmark_config_is_pinned_and_isolated() {
        let config = groq_gpt_oss_benchmark_config();
        assert_eq!(config.id, "benchmark_groq_gpt_oss_120b");
        assert_eq!(config.model, GROQ_GPT_OSS_BENCHMARK_MODEL);
        assert_eq!(config.env_var, GROQ_API_KEY_SECRET);
        assert!(config.free_only);
        assert_eq!(config.target_suite, TargetSuite::Both);
        assert!(canonical_model_id_for_cloud_engine(&config.id).is_none());
        assert!(known_cloud_configs().iter().all(|known| known.id != config.id));
    }

    #[test]
    fn groq_failed_cases_retain_redacted_execution_diagnostics() {
        let config = groq_gpt_oss_benchmark_config();
        let case = work_benchmark_suite().remove(0);
        let diagnostic = groq_diagnostic(
            "noki-work-benchmark_groq_gpt_oss_120b-fixture",
            &case,
            &config,
            true,
            crate::runtime_registry::RuntimeOutcome::EmptyResponse,
            ProviderExecutionObservation {
                attempted: true,
                http_status: Some(200),
                finish_reason: Some("length".into()),
                content_present: Some(false),
                input_tokens: Some(12),
                output_tokens: Some(32),
                reasoning_tokens: Some(32),
                latency_ms: 45,
                retry_after_ms: None,
                reset_requests_ms: Some(1_000),
                reset_tokens_ms: Some(2_000),
            },
            Some(BenchmarkFailureCategory::TokenLimit),
        );
        assert!(diagnostic.attempted);
        assert_eq!(diagnostic.http_status, Some(200));
        assert_eq!(diagnostic.content_present, Some(false));
        assert_eq!(diagnostic.failure_category, Some(BenchmarkFailureCategory::TokenLimit));
        assert_eq!(diagnostic.runtime_outcome, crate::runtime_registry::RuntimeOutcome::EmptyResponse);
        let serialized = serde_json::to_string(&diagnostic).unwrap();
        assert!(!serialized.contains(case.prompt));
        assert!(!serialized.contains("response content"));

        let run_id = diagnostic.run_id.clone();
        let skipped = groq_harness_skip_diagnostics(
            &config,
            BenchmarkTaskClass::Work,
            &run_id,
            std::slice::from_ref(&diagnostic),
        );
        assert_eq!(skipped.len(), 7);
        assert!(skipped.iter().all(|record| !record.attempted));
        assert!(skipped.iter().all(|record| {
            record.failure_category == Some(BenchmarkFailureCategory::HarnessSkip)
        }));
    }

    #[test]
    fn groq_reasoning_headroom_preserves_canonical_visible_contract() {
        let config = groq_gpt_oss_benchmark_config();
        let case = work_benchmark_suite().remove(0);
        assert_eq!(case.max_tokens, 15);
        assert_eq!(
            groq_provider_completion_budget(&config, case.max_tokens),
            case.max_tokens + GROQ_GPT_OSS_REASONING_HEADROOM_TOKENS
        );
        assert_eq!(
            benchmark_case_fingerprint(&case, BenchmarkTaskClass::Work),
            benchmark_case_fingerprint(&work_benchmark_suite().remove(0), BenchmarkTaskClass::Work)
        );
        let mut without_headroom = config.clone();
        without_headroom.extra_body.as_mut().unwrap().as_object_mut().unwrap()
            .remove("_noki_benchmark_reasoning_headroom_tokens");
        assert_ne!(
            benchmark_model_config_fingerprint(&config),
            benchmark_model_config_fingerprint(&without_headroom)
        );
        let enforced = enforce_canonical_visible_answer_limit(
            "eins zwei drei vier", Some(10), Some(6), 2,
        );
        assert!(enforced.chars().count() <= 10);
        assert!(enforced.chars().count() < "eins zwei drei vier".chars().count());
    }

    #[test]
    fn groq_work_and_coding_execution_are_independent() {
        let config = groq_gpt_oss_benchmark_config();
        let work_case = work_benchmark_suite().remove(0);
        let work_failure = groq_diagnostic(
            "work-run", &work_case, &config, true,
            crate::runtime_registry::RuntimeOutcome::EmptyResponse,
            ProviderExecutionObservation::default(),
            Some(BenchmarkFailureCategory::ContentNull),
        );
        let coding_case = coding_benchmark_suite().remove(0);
        let coding_success = groq_diagnostic(
            "coding-run", &coding_case, &config, true,
            crate::runtime_registry::RuntimeOutcome::Success,
            ProviderExecutionObservation { attempted: true, http_status: Some(200), content_present: Some(true), ..Default::default() },
            None,
        );
        assert!(work_failure.attempted);
        assert!(coding_success.attempted);
        assert_eq!(coding_success.failure_category, None);
        assert_eq!(coding_success.http_status, Some(200));
    }

    #[test]
    #[ignore = "requires configured Groq API key"]
    fn groq_gpt_oss_live_readiness() {
        println!(
            "credential_source={}",
            provider_secret_source(GROQ_API_KEY_SECRET).as_str()
        );
        let config = groq_gpt_oss_benchmark_config();
        preflight_groq_gpt_oss(&config).expect("Groq benchmark preflight failed");
        println!("CostSafety=VerifiedFreeHardStop");
        println!("READY_FOR_FREE_BENCHMARK=yes");
    }

    #[test]
    #[ignore = "requires configured Groq API key and valid Free-tier attestation"]
    fn groq_gpt_oss_live_w1_c1_headroom_smoke() {
        let config = groq_gpt_oss_benchmark_config();
        preflight_groq_gpt_oss(&config).expect("Groq benchmark preflight failed");
        for (task_class, case) in [
            (BenchmarkTaskClass::Work, work_benchmark_suite().remove(0)),
            (BenchmarkTaskClass::Coding, coding_benchmark_suite().remove(0)),
        ] {
            let prompt = case.context
                .map(|context| format!("{context}\n\n{}", case.prompt))
                .unwrap_or_else(|| case.prompt.to_string());
            let response = execute_groq_benchmark_case(
                &config, &prompt, case.max_tokens, Duration::from_secs(15),
            ).unwrap_or_else(|failure| panic!(
                "{} execution failed category={:?} http_status={:?} finish_reason={:?}",
                case.id, failure.failure_category, failure.observation.http_status, failure.observation.finish_reason
            ));
            assert!(response.content_present, "{} returned no visible content", case.id);
            println!(
                "groq_headroom_smoke case={} class={:?} http_status={:?} finish_reason={:?} input_tokens={:?} output_tokens={:?} reasoning_tokens={:?}",
                case.id, task_class, response.http_status, response.finish_reason,
                response.input_tokens, response.output_tokens, response.reasoning_tokens,
            );
        }
    }

    /// Explicit operator-only entry point for the live benchmark. The report
    /// deliberately contains no prompts, responses, account IDs or secrets.
    #[test]
    #[ignore = "requires configured Cloudflare account ID and API token"]
    fn cloudflare_glm_live_readiness() {
        println!(
            "credential_sources account_id={} api_token={}",
            provider_secret_source(CLOUDFLARE_ACCOUNT_ID_SECRET).as_str(),
            provider_secret_source(CLOUDFLARE_API_TOKEN_SECRET).as_str(),
        );
        let config = cloudflare_glm_benchmark_config().expect("Cloudflare credentials missing");
        let method =
            preflight_cloudflare_glm(&config).expect("Cloudflare benchmark preflight failed");
        println!("billing_verification_method={method}");
        println!("READY_FOR_FREE_BENCHMARK=yes");
    }

    #[test]
    #[ignore = "requires configured Cloudflare account ID and API token"]
    fn cloudflare_glm_live_smoke() {
        let config = cloudflare_glm_benchmark_config().expect("Cloudflare credentials missing");
        preflight_cloudflare_glm(&config).expect("Cloudflare benchmark preflight failed");
        match execute_provider_request(
            &config,
            "Antworte exakt und ausschließlich mit dem Wort: Bereit",
            15,
            Duration::from_secs(15),
        ) {
            Ok(response) => println!(
                "cloudflare_glm_smoke ok latency_ms={} output_chars={} input_tokens={:?} output_tokens={:?} reasoning_tokens={:?}",
                response.latency_ms,
                response.text.chars().count(),
                response.input_tokens,
                response.output_tokens,
                response.reasoning_tokens,
            ),
            Err(error) => panic!("cloudflare_glm_smoke failed: {error}"),
        }
    }

    #[test]
    #[ignore = "requires configured Cloudflare account ID and API token"]
    fn cloudflare_glm_live_two_case_harness_smoke() {
        let config = cloudflare_glm_benchmark_config().expect("Cloudflare credentials missing");
        preflight_cloudflare_glm(&config).expect("Cloudflare benchmark preflight failed");
        for (task_class, case) in [
            (BenchmarkTaskClass::Work, work_benchmark_suite().remove(0)),
            (
                BenchmarkTaskClass::Coding,
                coding_benchmark_suite().remove(0),
            ),
        ] {
            let response = execute_provider_request(
                &config,
                case.prompt,
                case.max_tokens,
                Duration::from_secs(15),
            )
            .expect("two-case smoke provider execution failed");
            let (passed, summary) = benchmark_case_passed(&case, &response.text);
            assert!(passed, "{}: {summary}", case.id);
            let score = match task_class {
                BenchmarkTaskClass::Work => {
                    score_work_case(&case, &response.text, response.latency_ms, true)
                }
                BenchmarkTaskClass::Coding => {
                    score_coding_case(&case, &response.text, response.latency_ms, true)
                }
            };
            println!(
                "two_case_smoke case={} outcome=success score={score:.1} latency_ms={}",
                case.id, response.latency_ms
            );
        }
    }

    #[test]
    #[ignore = "requires configured Cloudflare account ID and API token"]
    fn cloudflare_glm_live_canonical_w1_w8_c1_c8() {
        let report = run_cloudflare_glm_canonical_benchmark_v2()
            .expect("Cloudflare benchmark readiness/preflight failed");
        let mut invalid_old_config = cloudflare_glm_benchmark_config().unwrap();
        invalid_old_config.extra_body = None;
        let fingerprint_is_new = benchmark_model_config_fingerprint(&invalid_old_config)
            != report.model_config_fingerprint;
        validate_benchmark_run_v2(&report.work).expect("incomplete canonical Work suite");
        validate_benchmark_run_v2(&report.coding).expect("incomplete canonical Coding suite");
        let work = aggregate_benchmark_run_v2(&report.work).unwrap();
        let coding = aggregate_benchmark_run_v2(&report.coding).unwrap();
        let passed = |class| {
            report
                .telemetry
                .iter()
                .filter(|case| case.task_class == class && case.passed)
                .count()
        };
        let mut outcomes = std::collections::BTreeMap::<String, usize>::new();
        for case in &report.telemetry {
            *outcomes
                .entry(format!("{}:{}", case.outcome.as_str(), case.summary))
                .or_default() += 1;
        }
        let details = |run: &BenchmarkSuiteRunV2| {
            run.cases
                .iter()
                .map(|case| {
                    format!(
                        "{}:{:.1}/{}/{}ms",
                        case.case_id,
                        case.quality_score,
                        case.outcome.as_str(),
                        case.latency_ms
                    )
                })
                .collect::<Vec<_>>()
                .join(",")
        };
        let input_tokens = report
            .telemetry
            .iter()
            .filter_map(|case| case.input_tokens)
            .sum::<u32>();
        let output_tokens = report
            .telemetry
            .iter()
            .filter_map(|case| case.output_tokens)
            .sum::<u32>();
        let reasoning_tokens = report
            .telemetry
            .iter()
            .filter_map(|case| case.reasoning_tokens)
            .sum::<u32>();
        println!(
            "cloudflare_glm_benchmark model={} fingerprint_new={} work_quality={:.1} work_available={:.1} work_latency_ms={} work_passed={}/8 coding_quality={:.1} coding_available={:.1} coding_latency_ms={} coding_passed={}/8 input_tokens={} output_tokens={} reasoning_tokens={} cost_usd={:.2} quota_remaining={:?} outcomes={:?}",
            report.model,
            fingerprint_is_new,
            work.quality_score,
            work.availability_score,
            work.latency_ms,
            passed(BenchmarkTaskClass::Work),
            coding.quality_score,
            coding.availability_score,
            coding.latency_ms,
            passed(BenchmarkTaskClass::Coding),
            input_tokens,
            output_tokens,
            reasoning_tokens,
            report.cost_usd,
            report.quota_remaining,
            outcomes,
        );
        println!("work_cases={}", details(&report.work));
        println!("coding_cases={}", details(&report.coding));
    }

    #[test]
    #[ignore = "requires configured Groq API key and valid Free-tier attestation"]
    fn groq_gpt_oss_live_canonical_w1_w8_c1_c8() {
        let report = run_groq_gpt_oss_canonical_benchmark_v2()
            .expect("Groq benchmark readiness/preflight failed");
        let complete = report.incomplete_reason.is_none()
            && report.work.cases.len() == work_benchmark_suite().len()
            && report.coding.cases.len() == coding_benchmark_suite().len();
        if !complete {
            println!(
                "groq_gpt_oss_v2 incomplete reason={:?} limiting_dimension={:?} throttle_wait_ms={} work_cases={} coding_cases={} quota_remaining={:?} quota_reset={:?}",
                report.incomplete_reason,
                report.limiting_dimension,
                report.throttle_wait_ms,
                report.work.cases.len(),
                report.coding.cases.len(),
                report.quota_remaining,
                report.quota_reset,
            );
            return;
        }
        validate_benchmark_run_v2(&report.work).expect("canonical Work suite invalid");
        validate_benchmark_run_v2(&report.coding).expect("canonical Coding suite invalid");
        let work = aggregate_benchmark_run_v2(&report.work).expect("invalid Work execution");
        let coding = aggregate_benchmark_run_v2(&report.coding).expect("invalid Coding execution");
        let input_tokens = report
            .telemetry
            .iter()
            .filter_map(|case| case.input_tokens)
            .sum::<u32>();
        let output_tokens = report
            .telemetry
            .iter()
            .filter_map(|case| case.output_tokens)
            .sum::<u32>();
        let reasoning_tokens = report
            .telemetry
            .iter()
            .filter_map(|case| case.reasoning_tokens)
            .sum::<u32>();
        let executed = report
            .telemetry
            .iter()
            .filter(|case| case.outcome == BenchmarkOutcome::Success)
            .count();
        let avg_inference_latency_ms = report
            .telemetry
            .iter()
            .filter(|case| case.outcome == BenchmarkOutcome::Success)
            .map(|case| case.latency_ms as u128)
            .sum::<u128>()
            / executed.max(1) as u128;
        println!(
            "groq_gpt_oss_v2 model={} fingerprint={} work_quality={:.1} coding_quality={:.1} work_available={:.1} coding_available={:.1} executed={}/16 avg_latency_ms={} throttle_wait_ms={} limiting_dimension={:?} input_tokens={} output_tokens={} reasoning_tokens={} quota_remaining={:?} quota_reset={:?} cost_usd={:.2}",
            report.model,
            report.model_config_fingerprint,
            work.quality_score,
            coding.quality_score,
            work.availability_score,
            coding.availability_score,
            executed,
            avg_inference_latency_ms,
            report.throttle_wait_ms,
            report.limiting_dimension,
            input_tokens,
            output_tokens,
            reasoning_tokens,
            report.quota_remaining,
            report.quota_reset,
            report.cost_usd,
        );
    }

    /// Canonical noki-coding suite for a free coding candidate next to the
    /// incumbent Codestral, same harness, same run.
    #[test]
    #[ignore = "live canonical coding benchmark for a candidate (network)"]
    fn kandidat_live_canonical_coding_v2() {
        use crate::model_registry::{CODESTRAL, DEEPSEEK};
        let modelle: Vec<(String, String, String, String)> = vec![
            ("openrouter_nemotron_3_super".into(), "nvidia/nemotron-3-super-120b-a12b:free".into(), DEEPSEEK.connection.env_var.into(), DEEPSEEK.connection.endpoint.into()),
            (CODESTRAL.canonical_model_id.into(), CODESTRAL.exact_model_version.into(), CODESTRAL.connection.env_var.into(), CODESTRAL.connection.endpoint.into()),
        ];
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        for (id, model, env_var, endpoint) in modelle {
            let config = CloudProviderConfig {
                id: id.clone(), display_name: id.clone(), model, env_var, endpoint,
                supports_files: false, supports_vision: false, supports_tools: true, context: 128_000,
                free_only: true, target_suite: TargetSuite::Code,
                extra_body: id.starts_with("openrouter").then(|| serde_json::json!({"reasoning": {"effort": "none"}})),
            };
            let (run, _telemetry, _qr, _qs) = run_canonical_suite_for_config(&config, BenchmarkTaskClass::Coding, ts, 1);
            let n = run.cases.len().max(1) as f32;
            let q: f32 = run.cases.iter().map(|c| c.quality_score).sum::<f32>() / n;
            let a: f32 = run.cases.iter().map(|c| c.availability_score).sum::<f32>() / n;
            let mut lat: Vec<u64> = run.cases.iter().map(|c| c.latency_ms).collect();
            lat.sort();
            let passed = run.cases.iter().filter(|c| c.quality_score >= 70.0).count();
            println!("KANON {id} suite={} v={} quality={q:.1} availability={a:.1} median_latency_ms={} passed={passed}/{} outcomes={:?}",
                run.suite_id, run.suite_version, lat.get(lat.len() / 2).copied().unwrap_or(0), run.cases.len(),
                run.cases.iter().map(|c| format!("{}:{:.0}", c.case_id, c.quality_score)).collect::<Vec<_>>());
        }
    }

    #[test]
    #[ignore = "runs the requested canonical incumbent migration benchmarks"]
    fn incumbent_live_canonical_benchmark_v2() {
        use crate::model_manager::{AssistantMode, ModelManager, WorkTier};
        use crate::model_registry::{CanonicalModelDefinition, CODESTRAL, DEEPSEEK, JACKOD, MINISTRAL, QWEN_9B};
        use std::sync::atomic::AtomicBool;

        fn config_for(
            model: &CanonicalModelDefinition,
            target_suite: TargetSuite,
        ) -> CloudProviderConfig {
            CloudProviderConfig {
                id: model.canonical_model_id.into(),
                display_name: model.display_name.into(),
                model: model.exact_model_version.into(),
                env_var: model.connection.env_var.into(),
                endpoint: model.connection.endpoint.into(),
                supports_files: model.capabilities.files,
                supports_vision: model.capabilities.vision,
                supports_tools: model.capabilities.tools,
                context: model.capabilities.context_tokens,
                free_only: matches!(
                    model.cost_safety,
                    crate::model_registry::CostSafety::VerifiedFreeHardStop
                        | crate::model_registry::CostSafety::Local
                ),
                target_suite,
                extra_body: model
                    .connection
                    .disable_reasoning
                    .then(|| serde_json::json!({"reasoning": {"effort": "none"}})),
            }
        }

        fn local_config(
            model: &CanonicalModelDefinition,
            target_suite: TargetSuite,
        ) -> CloudProviderConfig {
            let mut config = config_for(model, target_suite);
            config.endpoint = "local://noki-llama-cpp".into();
            config.extra_body = Some(serde_json::json!({
                "temperature": 0.0,
                "chat_template_kwargs": {"enable_thinking": false},
                "runtime": "llama.cpp"
            }));
            config
        }

        fn run_local(
            model: &CanonicalModelDefinition,
            task_class: BenchmarkTaskClass,
            timestamp: u64,
            nonce: u128,
        ) -> (BenchmarkSuiteRunV2, Vec<BenchmarkCaseTelemetry>) {
            let target_suite = match task_class {
                BenchmarkTaskClass::Work => TargetSuite::Work,
                BenchmarkTaskClass::Coding => TargetSuite::Code,
            };
            let config = local_config(model, target_suite);
            let mut run = empty_benchmark_v2_run(&config, task_class, timestamp, nonce);
            let mut telemetry = Vec::new();
            let mut manager = ModelManager::default();
            let mode = match task_class {
                BenchmarkTaskClass::Work => {
                    manager.set_work_tier(WorkTier::Tier9B);
                    AssistantMode::Work
                }
                BenchmarkTaskClass::Coding => AssistantMode::Code,
            };
            assert_eq!(manager.model_for(mode), model.exact_model_version);
            let cancel = AtomicBool::new(false);
            for case in expected_suite(task_class) {
                let prompt = match case.context {
                    Some(context) => format!("{context}\n\n{}", case.prompt),
                    None => case.prompt.to_string(),
                };
                let started = Instant::now();
                match manager.generate(mode, &prompt, case.max_tokens as usize, &cancel) {
                    Ok(text) => {
                        let latency_ms = started.elapsed().as_millis() as u64;
                        let score = match task_class {
                            BenchmarkTaskClass::Work => {
                                score_work_case(&case, &text, latency_ms, true)
                            }
                            BenchmarkTaskClass::Coding => {
                                score_coding_case(&case, &text, latency_ms, true)
                            }
                        };
                        let (passed, summary) = benchmark_case_passed(&case, &text);
                        let metrics = manager.last_metrics.as_ref();
                        run.cases.push(benchmark_case_result(
                            &case,
                            task_class,
                            score,
                            100.0,
                            latency_ms,
                            BenchmarkOutcome::Success,
                        ));
                        telemetry.push(BenchmarkCaseTelemetry {
                            task_class,
                            case_id: case.id.into(),
                            passed,
                            summary,
                            outcome: BenchmarkOutcome::Success,
                            latency_ms,
                            input_tokens: metrics.map(|m| m.prompt_tokens as u32),
                            output_tokens: metrics.map(|m| m.output_tokens as u32),
                            reasoning_tokens: Some(0),
                        });
                    }
                    Err(_error) => {
                        let latency_ms = started.elapsed().as_millis() as u64;
                        run.cases.push(benchmark_case_result(
                            &case,
                            task_class,
                            0.0,
                            0.0,
                            latency_ms,
                            BenchmarkOutcome::Failed,
                        ));
                        telemetry.push(BenchmarkCaseTelemetry {
                            task_class,
                            case_id: case.id.into(),
                            passed: false,
                            summary: "provider_execution_failed".into(),
                            outcome: BenchmarkOutcome::Failed,
                            latency_ms,
                            input_tokens: None,
                            output_tokens: None,
                            reasoning_tokens: None,
                        });
                    }
                }
            }
            (run, telemetry)
        }

        fn print_result(
            label: &str,
            run: &BenchmarkSuiteRunV2,
            telemetry: &[BenchmarkCaseTelemetry],
        ) {
            validate_benchmark_run_v2(run).expect("canonical suite identity mismatch");
            match aggregate_benchmark_run_v2(run) {
                Ok(aggregate) => {
                    let passed = telemetry.iter().filter(|case| case.passed).count();
                    let executed = telemetry
                        .iter()
                        .filter(|case| case.outcome == BenchmarkOutcome::Success)
                        .count();
                    let details = run
                        .cases
                        .iter()
                        .map(|case| {
                            format!(
                                "{}:{:.1}/{}/{}ms",
                                case.case_id,
                                case.quality_score,
                                case.outcome.as_str(),
                                case.latency_ms
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(",");
                    println!(
                        "incumbent_v2 label={} suite={} version={} fingerprint={} quality={:.1} availability={:.1} executed={}/8 passed={}/8 latency_ms={} cases={}",
                        label,
                        run.suite_id,
                        run.suite_version,
                        run.model_config_fingerprint,
                        aggregate.quality_score,
                        aggregate.availability_score,
                        executed,
                        passed,
                        aggregate.latency_ms,
                        details
                    );
                }
                Err(error) => println!(
                    "incumbent_v2 label={} suite={} version={} fingerprint={} invalid={}",
                    label,
                    run.suite_id,
                    run.suite_version,
                    run.model_config_fingerprint,
                    error
                ),
            }
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let timestamp = now.as_secs();
        let nonce = now.as_nanos();

        let ministral = config_for(&MINISTRAL, TargetSuite::Work);
        assert!(ministral.free_only);
        assert!(resolve_provider_secret(&ministral.env_var).is_some());
        let (ministral_work, ministral_telemetry, _, _) = run_canonical_suite_for_config(
            &ministral,
            BenchmarkTaskClass::Work,
            timestamp,
            nonce,
        );
        print_result("ministral_work", &ministral_work, &ministral_telemetry);

        let (qwen_work, qwen_telemetry) = run_local(
            &QWEN_9B,
            BenchmarkTaskClass::Work,
            timestamp,
            nonce + 1,
        );
        print_result("qwen_9b_work", &qwen_work, &qwen_telemetry);

        let codestral = config_for(&CODESTRAL, TargetSuite::Code);
        assert!(codestral.free_only);
        assert!(resolve_provider_secret(&codestral.env_var).is_some());
        let (codestral_coding, codestral_telemetry, _, _) = run_canonical_suite_for_config(
            &codestral,
            BenchmarkTaskClass::Coding,
            timestamp,
            nonce + 2,
        );
        print_result("codestral_coding", &codestral_coding, &codestral_telemetry);

        let (jackod_coding, jackod_telemetry) = run_local(
            &JACKOD,
            BenchmarkTaskClass::Coding,
            timestamp,
            nonce + 3,
        );
        print_result("jackod_coding", &jackod_coding, &jackod_telemetry);

        let deepseek = config_for(&DEEPSEEK, TargetSuite::Both);
        assert!(deepseek.free_only);
        assert!(resolve_provider_secret(&deepseek.env_var).is_some());
        let (deepseek_work, mut deepseek_telemetry, _, _) = run_canonical_suite_for_config(
            &deepseek,
            BenchmarkTaskClass::Work,
            timestamp,
            nonce + 4,
        );
        print_result("deepseek_work", &deepseek_work, &deepseek_telemetry);
        let (deepseek_coding, coding_telemetry, _, _) = run_canonical_suite_for_config(
            &deepseek,
            BenchmarkTaskClass::Coding,
            timestamp,
            nonce + 5,
        );
        deepseek_telemetry.extend(coding_telemetry);
        print_result(
            "deepseek_coding",
            &deepseek_coding,
            &deepseek_telemetry
                .into_iter()
                .filter(|case| case.task_class == BenchmarkTaskClass::Coding)
                .collect::<Vec<_>>(),
        );
    }
}
