//! Adaptive runtime router: task -> privacy gate -> task class -> best available
//! model -> quality check -> optional next candidate -> always a local floor.
//!
//! Noki is the harness. Models are replaceable specialists. Local is the
//! guaranteed floor; cloud is an opportunistic accelerator.
//!
//! Principles enforced here, all of them load-bearing:
//! 1. Only `ProviderState::Available` is ever selected.
//! 2. One task -> normally exactly one model call. Never parallel, never a retry
//!    of the same provider, never a benchmark at runtime.
//! 3. Every chain ends on a local model.
//! 4. The privacy gate runs BEFORE routing. External content can never escalate.
//! 5. Free-only: an entry whose $0 status is not provable is `CostUncertain` and
//!    is therefore never selected.
//!
//! The chain order is not a guess: it comes from the measured benchmark in
//! `FINAL_BENCHMARK_DECISION.md` / `final_benchmark_decision.json`.
//! Work: Ministral 8B 87.5 @100% availability @1.6s, DeepSeek V4 Flash 74.5 @1.0s,
//! Qwen 3.5 9B 87.5 local, Qwen 3.5 4B 83.8 local, Gemini 3.8 Flash 88.0 quality
//! but 50% availability. Coding: DeepSeek 80.0 @1.07s, JackOD 9B + Noki Native
//! 78.8 local, Codestral 70.0, Gemini 100.0 on only 3 answered tasks.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::cloud_engine::{
    execute_provider_request_multimodal, resolve_provider_secret,
    CloudProviderConfig, EngineMode, MultimodalAttachment, ProviderExecError, TargetSuite,
};
use crate::model_registry::{self, CanonicalModelDefinition, RoutingRole};
pub use crate::model_registry::{
    BenchmarkProfile as Measured, CostSafety, ExecutionLane, LocalModel,
};
use crate::runtime_registry::{
    self, AuthState, BreakerState, MetadataConfidence, OutcomeScope, QuotaState,
    RuntimeAvailability, RuntimeObservation, RuntimeOutcome,
};

// ---------------------------------------------------------------------------
//  1. Provider states
// ---------------------------------------------------------------------------

/// Compatibility view of the canonical runtime state. Only `Available` may be
/// selected. Model lifecycle remains owned by `model_registry`.
///
/// The states carry their own cooldown, so "skip this one for now" needs no
/// separate bookkeeping and no waiting: the router moves to the next candidate
/// immediately and re-evaluates the state on the next task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderState {
    Available,
    /// 429 from a provider whose limit is per-minute/per-request.
    RateLimited {
        until: Instant,
    },
    /// 429 from a provider whose free budget is the actual constraint (Gemini).
    /// `reset_hint` is only ever filled from a header the provider really sent.
    QuotaExhausted {
        until: Instant,
        reset_hint: Option<String>,
    },
    /// 5xx, network failure or timeout. Short cooldown, then retryable.
    TemporarilyUnavailable {
        until: Instant,
    },
    /// 401/403 or no secret at all. Never probed again this session.
    CredentialsRequired,
    /// $0 status not provable -> unusable while free-only is in force.
    CostUncertain,
    /// Switched off by the user.
    Disabled,
    /// 404: The exact registered model was removed by the provider.
    ModelRemoved,
}

impl ProviderState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::RateLimited { .. } => "rate_limited",
            Self::QuotaExhausted { .. } => "quota_exhausted",
            Self::TemporarilyUnavailable { .. } => "temporarily_unavailable",
            Self::CredentialsRequired => "credentials_required",
            Self::CostUncertain => "cost_uncertain",
            Self::Disabled => "disabled",
            Self::ModelRemoved => "model_removed",
        }
    }

    /// The single gate: everything that is not `Available` is skipped.
    pub fn is_selectable(&self) -> bool {
        matches!(self, Self::Available)
    }

    fn cooldown_remaining(&self) -> Option<u64> {
        let now = Instant::now();
        let until = match self {
            Self::RateLimited { until }
            | Self::QuotaExhausted { until, .. }
            | Self::TemporarilyUnavailable { until } => *until,
            _ => return None,
        };
        if until > now {
            Some(until.duration_since(now).as_secs())
        } else {
            Some(0)
        }
    }
}

/// Cooldown policy. These are router policy, not invented quota numbers: the
/// real reset is used whenever a provider actually sends `Retry-After`.
const RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(60);
const UNAVAILABLE_COOLDOWN: Duration = Duration::from_secs(30);
const COST_BLOCK_COOLDOWN: Duration = Duration::from_secs(15 * 60);
const NOT_OFFERED_COOLDOWN: Duration = Duration::from_secs(6 * 3600);
const FAILURE_COOLDOWN: Duration = Duration::from_secs(15);
/// Gemini free tier reports no reset time with its 429. Measured behaviour:
/// 6x 429 + 2x 503 inside a single 16-request run. A conservative skip window is
/// the only honest response — the alternative would be inventing a quota.
const QUOTA_COOLDOWN: Duration = Duration::from_secs(3600);

/// Hard ceiling on cloud attempts per task. Prevents retry spirals: one primary
/// plus at most two fallbacks, then the local floor.
const MAX_CLOUD_ATTEMPTS: usize = 3;
/// A cloud answer may be rejected by the quality gate exactly once before the
/// router stops spending tokens and goes local.
const MAX_QUALITY_SWITCHES: usize = 1;

// ---------------------------------------------------------------------------
//  2. Task classes and tiers
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskClass {
    Work,
    Coding,
}

impl TaskClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Work => "work",
            Self::Coding => "coding",
        }
    }
}

/// Routing tier. `Deep` covers Work DEEP and Coding COMPLEX — the two names the
/// benchmark report uses for the same escalation level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Fast,
    Normal,
    Deep,
}

/// Coarse local resource state supplied by the caller. It contains no prompt
/// data and only affects ordering inside an already-authorised FREE_CLOUD lane.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourcePressure {
    #[default]
    Normal,
    High,
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "FAST",
            Self::Normal => "NORMAL",
            Self::Deep => "DEEP",
        }
    }

    pub fn from_reasoning(tier: crate::reasoning::ReasoningTier) -> Self {
        match tier {
            crate::reasoning::ReasoningTier::Fast => Self::Fast,
            crate::reasoning::ReasoningTier::Normal => Self::Normal,
            crate::reasoning::ReasoningTier::Deep => Self::Deep,
        }
    }
}

/// Explicit production routing switch. Dynamic is the default after the
/// validated shadow period; Static preserves the previous chain verbatim as
/// an immediate, configuration-only rollback path.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RoutingMode {
    Static,
    #[default]
    Dynamic,
}

impl RoutingMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Static => "STATIC",
            Self::Dynamic => "DYNAMIC",
        }
    }

    fn from_config(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some(value) if value.eq_ignore_ascii_case("STATIC") => Self::Static,
            _ => Self::Dynamic,
        }
    }

    /// Process-level kill switch for immediate rollback. Missing or invalid
    /// values fail to the activated DYNAMIC default.
    pub fn configured() -> Self {
        let value = std::env::var("NOKI_ROUTING_MODE").ok();
        Self::from_config(value.as_deref())
    }
}

// ---------------------------------------------------------------------------
//  3. Model catalog (empirical, from the final benchmark decision)
// ---------------------------------------------------------------------------

/// One routable entry. Cloud entries carry the exact request configuration that
/// was measured — notably DeepSeek's closed reasoning channel.
#[derive(Clone, Debug)]
pub struct ModelEntry {
    pub id: &'static str,
    pub provider: &'static str,
    pub display_name: &'static str,
    pub model: &'static str,
    pub env_var: &'static str,
    pub endpoint: &'static str,
    pub local: Option<LocalModel>,
    pub status_role: &'static str,
    /// $0 proven by a completed benchmark run against this exact workspace.
    /// Kept as a compatibility field until the registry migration; routing uses
    /// `cost_safety` as the canonical policy decision.
    pub free_verified: bool,
    pub cost_safety: CostSafety,
    /// Only reachable under the specialist rules (single shot, DEEP only).
    pub specialist_only: bool,
    /// What the benchmark actually measured for this entry. Information only:
    /// nothing here is ever re-measured at runtime, and an unmeasured figure
    /// stays `None` rather than being estimated.
    pub measured: Measured,
}

#[cfg(test)]
const NOT_MEASURED: Measured = model_registry::NOT_MEASURED;

impl ModelEntry {
    pub fn is_local(&self) -> bool {
        self.local.is_some()
    }

    /// Builds the cloud request configuration. DeepSeek gets `reasoning: none`
    /// here and nowhere else: with the channel open it measured 48.8 on Work
    /// instead of 74.5, at a cost of 2871 reasoning tokens.
    pub fn cloud_config(&self) -> Option<CloudProviderConfig> {
        if self.is_local() {
            return None;
        }
        let definition = model_registry::model(self.id)?;
        let extra_body = if definition.provider_id == "cloudflare_workers_ai" {
            Some(serde_json::json!({
                "chat_template_kwargs": { "enable_thinking": false }
            }))
        } else if definition.canonical_model_id == model_registry::GROQ_GPT_OSS.canonical_model_id {
            // Productive execution must use the same visible-answer configuration
            // that passed the canonical benchmark. Otherwise GPT-OSS can spend a
            // large completion budget on hidden reasoning and return only a few
            // visible words, which breaks bounded long-form continuation.
            Some(serde_json::json!({
                "reasoning_effort": "low",
                "include_reasoning": false
            }))
        } else if definition.connection.disable_reasoning {
            Some(serde_json::json!({ "reasoning": { "effort": "none" } }))
        } else {
            None
        };
        // Workers AI endpoints are account-scoped. This is provider transport
        // resolution, not a routing preference; the canonical registry owns
        // the endpoint template and model identity.
        let endpoint = if definition.provider_id == "cloudflare_workers_ai" {
            let account_id = resolve_provider_secret("CLOUDFLARE_ACCOUNT_ID")?;
            definition.connection.endpoint.replace("{account_id}", &account_id)
        } else {
            self.endpoint.to_string()
        };
        Some(CloudProviderConfig {
            id: self.id.to_string(),
            display_name: self.display_name.to_string(),
            model: self.model.to_string(),
            env_var: self.env_var.to_string(),
            endpoint,
            supports_files: definition.capabilities.files,
            supports_vision: definition.capabilities.vision,
            supports_tools: definition.capabilities.tools,
            context: definition.capabilities.context_tokens,
            free_only: self.cost_safety == CostSafety::VerifiedFreeHardStop,
            target_suite: TargetSuite::Both,
            extra_body,
        })
    }
}

const fn router_entry(definition: CanonicalModelDefinition) -> ModelEntry {
    ModelEntry {
        id: definition.canonical_model_id,
        provider: definition.provider_id,
        display_name: definition.display_name,
        model: definition.exact_model_version,
        env_var: definition.connection.env_var,
        endpoint: definition.connection.endpoint,
        local: definition.local_model,
        status_role: if definition.specialist_only {
            "specialist"
        } else {
            definition.lifecycle.legacy_status()
        },
        free_verified: matches!(
            definition.cost_safety,
            CostSafety::Local | CostSafety::VerifiedFreeHardStop
        ),
        cost_safety: definition.cost_safety,
        specialist_only: definition.specialist_only,
        measured: definition.benchmark_profile,
    }
}

pub const NEX_N2_5_PRO: ModelEntry = router_entry(model_registry::NEX_N2_5_PRO);
pub const NEMOTRON_3_ULTRA: ModelEntry = router_entry(model_registry::NEMOTRON_3_ULTRA);
pub const DEEPSEEK: ModelEntry = router_entry(model_registry::DEEPSEEK);
pub const MINISTRAL: ModelEntry = router_entry(model_registry::MINISTRAL);
pub const CODESTRAL: ModelEntry = router_entry(model_registry::CODESTRAL);
pub const GEMINI: ModelEntry = router_entry(model_registry::GEMINI);
pub const QWEN_9B: ModelEntry = router_entry(model_registry::QWEN_9B);
pub const QWEN_4B: ModelEntry = router_entry(model_registry::QWEN_4B);
pub const JACKOD: ModelEntry = router_entry(model_registry::JACKOD);
pub const GROQ_GPT_OSS: ModelEntry = router_entry(model_registry::GROQ_GPT_OSS);
pub const CLOUDFLARE_GLM_4_7_FLASH: ModelEntry =
    router_entry(model_registry::CLOUDFLARE_GLM_4_7_FLASH);

pub fn catalog() -> Vec<ModelEntry> {
    vec![
        NEX_N2_5_PRO,
        NEMOTRON_3_ULTRA,
        DEEPSEEK,
        MINISTRAL,
        CODESTRAL,
        GEMINI,
        GROQ_GPT_OSS,
        CLOUDFLARE_GLM_4_7_FLASH,
        QWEN_9B,
        QWEN_4B,
        JACKOD,
    ]
}

fn role_for(class: TaskClass, tier: Tier) -> RoutingRole {
    match (class, tier) {
        (TaskClass::Work, Tier::Fast) => RoutingRole::WorkFast,
        (TaskClass::Work, Tier::Normal) => RoutingRole::WorkBalanced,
        (TaskClass::Work, Tier::Deep) => RoutingRole::WorkDeep,
        (TaskClass::Coding, Tier::Fast) => RoutingRole::CodingFast,
        (TaskClass::Coding, Tier::Normal) => RoutingRole::CodingNormal,
        (TaskClass::Coding, Tier::Deep) => RoutingRole::CodingComplex,
    }
}

fn entry_by_registry_id(id: &str) -> ModelEntry {
    let definition = model_registry::model(id)
        .unwrap_or_else(|| panic!("routing role references unknown canonical model: {id}"));
    router_entry(*definition)
}

/// The routing chain for one class, tier and engine mode.
///
/// Every chain ends on a local entry. A local entry that appears in the middle
/// is terminal in normal operation; it is skipped only when the local model is
/// genuinely unavailable, which is what makes the third fallback reachable.
///
/// The Gemini specialist is never part of a chain. It is offered ahead of the
/// chain, once, and only when every specialist condition holds.
pub fn chain(class: TaskClass, tier: Tier, mode: EngineMode) -> Vec<ModelEntry> {
    let role = if mode == EngineMode::OnlyLocal {
        match class {
            TaskClass::Work => RoutingRole::WorkLocalFloor,
            TaskClass::Coding => RoutingRole::CodingLocalFloor,
        }
    } else {
        role_for(class, tier)
    };
    model_registry::role_model_ids(role)
        .into_iter()
        .map(entry_by_registry_id)
        .collect()
}

// ---------------------------------------------------------------------------
//  3a. Phase 7 shadow ranking
// ---------------------------------------------------------------------------

/// Shadow ranking is deliberately separate from `chain`: it is an explainable
/// recommendation/reporting path and is never consulted by productive routing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShadowFilter {
    Role,
    Privacy,
    Lane,
    CostSafety,
    Disabled,
    Lifecycle,
    Specialist,
    Benchmark,
    Runtime,
    Credentials,
    Quota,
    Auth,
    Capability,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowRejection {
    pub model_id: String,
    pub filter: ShadowFilter,
    pub reason: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowWeights {
    pub quality: u32,
    pub availability: u32,
    pub completion: u32,
    pub reliability: u32,
    pub quality_stability: u32,
    pub health: u32,
    pub p95_latency: u32,
    pub quota_freshness: u32,
    pub capability: u32,
    pub cost: u32,
}

impl ShadowWeights {
    pub const fn for_tier(tier: Tier) -> Self {
        match tier {
            Tier::Fast => Self {
                quality: 20,
                availability: 10,
                completion: 15,
                reliability: 20,
                quality_stability: 5,
                health: 10,
                p95_latency: 15,
                quota_freshness: 5,
                capability: 0,
                cost: 1,
            },
            Tier::Normal => Self {
                quality: 35,
                availability: 10,
                completion: 15,
                reliability: 20,
                quality_stability: 10,
                health: 5,
                p95_latency: 3,
                quota_freshness: 5,
                capability: 2,
                cost: 1,
            },
            Tier::Deep => Self {
                quality: 43,
                availability: 8,
                completion: 12,
                reliability: 15,
                quality_stability: 12,
                health: 5,
                p95_latency: 1,
                quota_freshness: 2,
                capability: 4,
                cost: 1,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowComponents {
    pub quality: u32,
    pub availability: u32,
    pub completion: u32,
    pub reliability: u32,
    pub quality_stability: u32,
    pub health: u32,
    pub p95_latency: u32,
    pub quota_freshness: u32,
    pub capability: u32,
    pub cost: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShadowBenchmarkConfidence {
    Unverified,
    Legacy,
    CanonicalV2,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowRankedCandidate {
    pub final_rank: usize,
    pub model_id: String,
    pub provider: String,
    pub role: RoutingRole,
    /// Weighted score in basis points (0..=10_000). Integer arithmetic keeps
    /// reports byte-for-byte deterministic across platforms.
    pub score: u32,
    pub components: ShadowComponents,
    pub weights: ShadowWeights,
    pub benchmark_score: u32,
    pub benchmark_confidence: ShadowBenchmarkConfidence,
    pub benchmark_suite: String,
    pub benchmark_version: String,
    pub benchmark_fingerprint: String,
    pub completion: u32,
    pub reliability: u32,
    pub p95_latency_ms: Option<u64>,
    pub quota: String,
    pub is_role_champion: bool,
    pub is_local_floor: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowRankReport {
    pub task_class: TaskClass,
    pub role: RoutingRole,
    pub tier: Tier,
    pub execution_lane: ExecutionLane,
    pub privacy_gate: PrivacyGate,
    pub static_chain: Vec<String>,
    pub candidates: Vec<ShadowRankedCandidate>,
    pub rejected: Vec<ShadowRejection>,
    pub selected_candidate: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowValidationDivergence {
    pub static_choice: String,
    pub dynamic_choice: Option<String>,
    pub same: bool,
    pub divergence_reason: String,
    pub oracle_correct: bool,
}

impl ShadowValidationDivergence {
    pub fn from_oracle(report: &ShadowRankReport, expected_dynamic_choice: Option<&str>) -> Self {
        let static_choice = report.static_chain.first().cloned().unwrap_or_default();
        let dynamic_choice = report.selected_candidate.clone();
        let same = dynamic_choice.as_deref() == Some(static_choice.as_str());
        Self {
            static_choice,
            dynamic_choice: dynamic_choice.clone(),
            same,
            divergence_reason: if same {
                "same_choice".to_string()
            } else if dynamic_choice.is_none() {
                "all_dynamic_candidates_filtered".to_string()
            } else {
                "hard_filter_or_runtime_rank".to_string()
            },
            oracle_correct: dynamic_choice.as_deref() == expected_dynamic_choice,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowActivationGate {
    pub security_lane_privacy_passed: bool,
    pub benchmark_identity_passed: bool,
    pub scenario_oracles_passed: bool,
    pub deterministic_ranking_passed: bool,
    pub local_floor_guaranteed: bool,
    pub no_unverified_promotion: bool,
    pub zero_external_calls: bool,
}

impl ShadowActivationGate {
    pub const fn ready_for_controlled_activation(self) -> bool {
        self.security_lane_privacy_passed
            && self.benchmark_identity_passed
            && self.scenario_oracles_passed
            && self.deterministic_ranking_passed
            && self.local_floor_guaranteed
            && self.no_unverified_promotion
            && self.zero_external_calls
    }
}

/// Optional benchmark identity expected by an audit consumer. The fingerprint
/// is derived from canonical model/version/config metadata; no benchmark or
/// network call is performed here. A mismatch fails closed for that candidate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShadowBenchmarkExpectation<'a> {
    pub suite_id: Option<&'a str>,
    pub suite_version: Option<&'a str>,
    pub fingerprint: Option<&'a str>,
}

fn shadow_role_pool(role: RoutingRole) -> Vec<&'static str> {
    model_registry::role_model_ids(role)
}

fn shadow_benchmark_fingerprint(definition: &CanonicalModelDefinition, class: TaskClass) -> String {
    let mut hasher = Sha256::new();
    let (suite_id, cases) = match class {
        TaskClass::Work => (
            crate::cloud_engine::WORK_SUITE_ID,
            crate::cloud_engine::work_benchmark_suite(),
        ),
        TaskClass::Coding => (
            crate::cloud_engine::CODING_SUITE_ID,
            crate::cloud_engine::coding_benchmark_suite(),
        ),
    };
    hasher.update(suite_id.as_bytes());
    hasher.update([0]);
    hasher.update(crate::cloud_engine::BENCHMARK_SUITE_VERSION.as_bytes());
    hasher.update([0]);
    hasher.update(crate::cloud_engine::BENCHMARK_HARNESS_VERSION.as_bytes());
    hasher.update([0]);
    hasher.update(definition.canonical_model_id.as_bytes());
    hasher.update([0]);
    hasher.update(definition.exact_model_version.as_bytes());
    hasher.update([0]);
    hasher.update(definition.connection.id.as_bytes());
    hasher.update([0]);
    hasher.update(definition.connection.endpoint.as_bytes());
    for case in cases {
        hasher.update(case.id.as_bytes());
        hasher.update([0]);
        hasher.update(
            crate::cloud_engine::benchmark_case_fingerprint(
                &case,
                match class {
                    TaskClass::Work => crate::cloud_engine::BenchmarkTaskClass::Work,
                    TaskClass::Coding => crate::cloud_engine::BenchmarkTaskClass::Coding,
                },
            )
            .as_bytes(),
        );
        hasher.update([0]);
    }
    hex::encode(hasher.finalize())
}

pub fn shadow_benchmark_fingerprint_for_model(id: &str, class: TaskClass) -> Option<String> {
    model_registry::model(id).map(|definition| shadow_benchmark_fingerprint(definition, class))
}

fn shadow_benchmark_valid(
    definition: &CanonicalModelDefinition,
    class: TaskClass,
    expected: ShadowBenchmarkExpectation<'_>,
) -> Result<(String, ShadowBenchmarkConfidence), &'static str> {
    let profile = definition.benchmark_profile;
    let measured = match class {
        TaskClass::Work => profile.work_quality.is_some() && profile.work_latency_ms.is_some(),
        TaskClass::Coding => profile.code_quality.is_some() && profile.code_latency_ms.is_some(),
    };
    if !measured {
        return Err("benchmark_missing_or_not_canonical");
    }
    let confidence = match definition.benchmark_status {
        model_registry::BenchmarkStatus::CanonicalV2 => ShadowBenchmarkConfidence::CanonicalV2,
        model_registry::BenchmarkStatus::LegacyProductive => ShadowBenchmarkConfidence::Legacy,
        model_registry::BenchmarkStatus::CandidatePendingCanonical
        | model_registry::BenchmarkStatus::Missing => {
            return Err("benchmark_missing_or_not_canonical")
        }
    };
    let suite_id = match class {
        TaskClass::Work => crate::cloud_engine::WORK_SUITE_ID,
        TaskClass::Coding => crate::cloud_engine::CODING_SUITE_ID,
    };
    if expected.suite_id.is_some_and(|value| value != suite_id) {
        return Err("benchmark_suite_task_class_mismatch");
    }
    if expected
        .suite_version
        .is_some_and(|value| value != crate::cloud_engine::BENCHMARK_SUITE_VERSION)
    {
        return Err("benchmark_suite_version_mismatch");
    }
    let fingerprint = shadow_benchmark_fingerprint(definition, class);
    if expected
        .fingerprint
        .is_some_and(|value| value != fingerprint)
    {
        return Err("benchmark_fingerprint_mismatch");
    }
    Ok((
        if confidence == ShadowBenchmarkConfidence::CanonicalV2 {
            fingerprint
        } else {
            String::new()
        },
        confidence,
    ))
}

fn shadow_component_quality(definition: &CanonicalModelDefinition, class: TaskClass) -> u32 {
    let score = match class {
        TaskClass::Work => definition.benchmark_profile.work_quality,
        TaskClass::Coding => definition.benchmark_profile.code_quality,
    }
    .unwrap_or(0.0);
    (score.clamp(0.0, 100.0) * 100.0).round() as u32
}

fn shadow_components(
    definition: &CanonicalModelDefinition,
    class: TaskClass,
    tier: Tier,
    req: &RouteRequest<'_>,
) -> ShadowComponents {
    let profile = definition.benchmark_profile;
    let availability =
        (profile.availability.unwrap_or(0.0).clamp(0.0, 100.0) * 100.0).round() as u32;
    let completion = (profile.completion.unwrap_or(0.0).clamp(0.0, 100.0) * 100.0).round() as u32;
    let caps = definition.capabilities;
    let context_headroom = if req.required_context_tokens == 0 {
        10_000
    } else {
        (caps.context_tokens as u64)
            .saturating_mul(10_000)
            .checked_div(req.required_context_tokens as u64)
            .unwrap_or(10_000)
            .min(10_000) as u32
    };
    let capability = if tier == Tier::Deep {
        context_headroom
    } else {
        10_000
    };
    let (reliability, quality_stability, health, p95_latency, quota_freshness) = if let Some(
        runtime,
    ) =
        runtime_registry::model_runtime_view(definition.canonical_model_id)
    {
        let telemetry = runtime.telemetry();
        let reliability = if telemetry.requests == 0 {
            5_000
        } else {
            telemetry.successes.saturating_mul(10_000) / telemetry.requests
        };
        let quality_failures = telemetry
            .empty_responses
            .saturating_add(telemetry.quality_failures);
        let quality_stability = if telemetry.requests == 0 {
            10_000
        } else {
            10_000u32.saturating_sub(quality_failures.saturating_mul(10_000) / telemetry.requests)
        };
        let health = match (runtime.effective_availability(), runtime.breaker()) {
            (RuntimeAvailability::Available, BreakerState::Closed) => 10_000,
            (RuntimeAvailability::Degraded, _) | (_, BreakerState::Degraded) => 7_000,
            _ => 0,
        };
        let p95_latency = runtime
            .latency_p95_ms()
            .map(|latency| 10_000u32.saturating_sub(latency.min(10_000) as u32))
            .unwrap_or(5_000);
        let quota_freshness = match (runtime.quota_confidence(), runtime.quota_age_ms()) {
            (MetadataConfidence::ExplicitReset, Some(age)) if age <= 300_000 => 10_000,
            (MetadataConfidence::Header, Some(age)) if age <= 300_000 => 8_000,
            (MetadataConfidence::ExplicitReset | MetadataConfidence::Header, Some(_)) => 2_500,
            (MetadataConfidence::Unknown, _) => 5_000,
            (_, None) => 5_000,
        };
        (
            reliability,
            quality_stability,
            health,
            p95_latency,
            quota_freshness,
        )
    } else {
        (5_000, 10_000, 10_000, 5_000, 5_000)
    };
    ShadowComponents {
        quality: shadow_component_quality(definition, class),
        availability,
        completion,
        reliability,
        quality_stability,
        health,
        p95_latency,
        quota_freshness,
        capability,
        cost: match definition.cost_safety {
            CostSafety::Local | CostSafety::VerifiedFreeHardStop => 10_000,
            CostSafety::MeteredPaid => 5_000,
            CostSafety::FreeButBillingUncertain => 0,
        },
    }
}

fn shadow_score(components: ShadowComponents, weights: ShadowWeights) -> u32 {
    let total = weights.quality
        + weights.availability
        + weights.completion
        + weights.reliability
        + weights.quality_stability
        + weights.health
        + weights.p95_latency
        + weights.quota_freshness
        + weights.capability
        + weights.cost;
    if total == 0 {
        return 0;
    }
    (components.quality * weights.quality
        + components.availability * weights.availability
        + components.completion * weights.completion
        + components.reliability * weights.reliability
        + components.quality_stability * weights.quality_stability
        + components.health * weights.health
        + components.p95_latency * weights.p95_latency
        + components.quota_freshness * weights.quota_freshness
        + components.capability * weights.capability
        + components.cost * weights.cost)
        / total
}

fn shadow_capabilities_match(
    definition: &CanonicalModelDefinition,
    req: &RouteRequest<'_>,
) -> bool {
    let caps = definition.capabilities;
    caps.context_tokens >= req.required_context_tokens
        && (!req.requires_reasoning || caps.reasoning)
        && (!req.requires_tools || caps.tools)
        && (!req.requires_structured_output || caps.structured_output)
        && (!req.requires_files || caps.files)
        && (!req.requires_vision || caps.vision)
}

fn shadow_lane_matches(definition: &CanonicalModelDefinition, lane: ExecutionLane) -> bool {
    matches!(
        (lane, definition.execution_lane),
        (ExecutionLane::Local, ExecutionLane::Local)
            | (
                ExecutionLane::FreeCloud,
                ExecutionLane::Local | ExecutionLane::FreeCloud
            )
            | (
                ExecutionLane::PaidCloud,
                ExecutionLane::Local | ExecutionLane::PaidCloud
            )
    )
}

fn shadow_candidate_order(
    a: &ShadowRankedCandidate,
    b: &ShadowRankedCandidate,
    prefer_free_cloud: bool,
    prefer_resident_local: bool,
) -> Ordering {
    let lane_order = if prefer_free_cloud {
        (a.provider == "local").cmp(&(b.provider == "local"))
    } else if prefer_resident_local {
        (b.provider == "local").cmp(&(a.provider == "local"))
    } else {
        Ordering::Equal
    };
    if lane_order != Ordering::Equal {
        return lane_order;
    }
    // The local floor is a guaranteed terminal fallback, never a cloud
    // challenger. This is a routing boundary rather than a score bonus.
    a.is_local_floor
        .cmp(&b.is_local_floor)
        // A fully verified canonical challenger may beat the current champion.
        // A legacy/incomplete challenger may not.
        .then_with(|| b.benchmark_confidence.cmp(&a.benchmark_confidence))
        .then_with(|| {
            if a.benchmark_confidence == ShadowBenchmarkConfidence::Legacy
                && b.benchmark_confidence == ShadowBenchmarkConfidence::Legacy
            {
                b.is_role_champion.cmp(&a.is_role_champion)
            } else {
                Ordering::Equal
            }
        })
        .then_with(|| b.score.cmp(&a.score))
        .then_with(|| b.benchmark_confidence.cmp(&a.benchmark_confidence))
        .then_with(|| b.completion.cmp(&a.completion))
        .then_with(|| {
            a.p95_latency_ms
                .unwrap_or(u64::MAX)
                .cmp(&b.p95_latency_ms.unwrap_or(u64::MAX))
        })
        .then_with(|| a.model_id.cmp(&b.model_id))
}

fn shadow_rank_internal(
    req: &RouteRequest<'_>,
    expected: ShadowBenchmarkExpectation<'_>,
    credentials: &dyn CredentialProbe,
) -> ShadowRankReport {
    // Build a deterministic, metadata-only ranking report. Hard policy filters
    // run before scoring; candidates are sorted by score and canonical ID.
    let gate = privacy_gate(req);
    let lane = resolve_execution_lane(req, gate);
    let role = if req.engine_mode == EngineMode::OnlyLocal {
        match req.class {
            TaskClass::Work => RoutingRole::WorkLocalFloor,
            TaskClass::Coding => RoutingRole::CodingLocalFloor,
        }
    } else {
        role_for(req.class, req.tier)
    };
    let weights = ShadowWeights::for_tier(req.tier);
    let plan = model_registry::role_plan(role);
    let static_chain = model_registry::role_model_ids(role)
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut candidates = Vec::new();
    let mut rejected = Vec::new();

    for id in shadow_role_pool(role) {
        let Some(definition) = model_registry::model(id) else {
            continue;
        };
        let mut reject = |filter, reason: &'static str| {
            rejected.push(ShadowRejection {
                model_id: id.to_string(),
                filter,
                reason: reason.to_string(),
            });
        };
        if !gate.allows_cloud() && definition.execution_lane != ExecutionLane::Local {
            reject(ShadowFilter::Privacy, "privacy_gate_requires_local");
            continue;
        }
        if !shadow_lane_matches(definition, lane) {
            reject(ShadowFilter::Lane, "execution_lane_mismatch");
            continue;
        }
        if !lane.allows(definition.cost_safety) {
            reject(ShadowFilter::CostSafety, "cost_safety_not_allowed_in_lane");
            continue;
        }
        if !definition.enabled {
            reject(ShadowFilter::Disabled, "model_disabled");
            continue;
        }
        if !matches!(
            definition.lifecycle,
            model_registry::ModelLifecycle::Active | model_registry::ModelLifecycle::Candidate
        ) {
            reject(ShadowFilter::Lifecycle, "lifecycle_not_rankable");
            continue;
        }
        if definition.specialist_only
            && !(req.allow_specialist && req.tier == Tier::Deep && !req.in_agent_loop)
        {
            reject(ShadowFilter::Specialist, "specialist_policy_not_satisfied");
            continue;
        }
        let (fingerprint, benchmark_confidence) =
            match shadow_benchmark_valid(definition, req.class, expected) {
                Ok(value) => value,
                Err(reason) => {
                    reject(ShadowFilter::Benchmark, reason);
                    continue;
                }
            };
        if definition.execution_lane != ExecutionLane::Local {
            if !credentials.has_secret(definition.connection.env_var) {
                reject(ShadowFilter::Credentials, "credentials_missing");
                continue;
            }
            let Some(runtime) = runtime_registry::model_runtime_view(id) else {
                reject(ShadowFilter::Runtime, "runtime_view_missing");
                continue;
            };
            if matches!(runtime.auth_state(), AuthState::Missing | AuthState::Failed) {
                reject(ShadowFilter::Auth, "auth_failed");
                continue;
            }
            if runtime.quota_state() == QuotaState::Exhausted {
                reject(ShadowFilter::Quota, "quota_exhausted");
                continue;
            }
            if !runtime.is_selectable() {
                let reason = match (runtime.effective_availability(), runtime.breaker()) {
                    (RuntimeAvailability::HalfOpen, _) | (_, BreakerState::HalfOpen) => {
                        "half_open_probe_only"
                    }
                    (RuntimeAvailability::Cooldown, _) => "runtime_cooldown",
                    (_, BreakerState::Open) => "breaker_open",
                    _ => "runtime_not_selectable",
                };
                reject(ShadowFilter::Runtime, reason);
                continue;
            }
        } else if !req.local_available {
            reject(ShadowFilter::Runtime, "local_runtime_unavailable");
            continue;
        }
        if !shadow_capabilities_match(definition, req) {
            reject(ShadowFilter::Capability, "required_capability_missing");
            continue;
        }
        let components = shadow_components(definition, req.class, req.tier, req);
        let runtime = runtime_registry::model_runtime_view(id);
        let p95_latency_ms = runtime.as_ref().and_then(|view| view.latency_p95_ms());
        let quota = runtime
            .as_ref()
            .map(|view| format!("{:?}", view.quota_state()).to_lowercase())
            .unwrap_or_else(|| "unknown".to_string());
        candidates.push(ShadowRankedCandidate {
            final_rank: 0,
            model_id: id.to_string(),
            provider: definition.provider_id.to_string(),
            role,
            score: shadow_score(components, weights),
            components,
            weights,
            benchmark_score: components.quality,
            benchmark_confidence,
            benchmark_suite: match req.class {
                TaskClass::Work => crate::cloud_engine::WORK_SUITE_ID,
                TaskClass::Coding => crate::cloud_engine::CODING_SUITE_ID,
            }
            .to_string(),
            benchmark_version: if benchmark_confidence == ShadowBenchmarkConfidence::CanonicalV2 {
                crate::cloud_engine::BENCHMARK_SUITE_VERSION.to_string()
            } else {
                "legacy".to_string()
            },
            benchmark_fingerprint: fingerprint,
            completion: components.completion,
            reliability: components.reliability,
            p95_latency_ms,
            quota,
            is_role_champion: id == plan.champion,
            is_local_floor: plan.local_floor.contains(&id),
        });
    }
    let cloud_policy_allows = gate.allows_cloud() && lane == ExecutionLane::FreeCloud;
    candidates.sort_by(|a, b| {
        shadow_candidate_order(
            a,
            b,
            cloud_policy_allows && req.resource_pressure == ResourcePressure::High,
            cloud_policy_allows
                && req.resource_pressure == ResourcePressure::Normal
                && req.local_model_resident,
        )
    });
    for (index, candidate) in candidates.iter_mut().enumerate() {
        candidate.final_rank = index + 1;
    }
    rejected.sort_by(|a, b| {
        a.model_id
            .cmp(&b.model_id)
            .then_with(|| a.reason.cmp(&b.reason))
    });
    ShadowRankReport {
        task_class: req.class,
        role,
        tier: req.tier,
        execution_lane: lane,
        privacy_gate: gate,
        static_chain,
        selected_candidate: candidates
            .first()
            .map(|candidate| candidate.model_id.clone()),
        candidates,
        rejected,
    }
}

pub fn shadow_rank_with_expectation(
    req: &RouteRequest<'_>,
    expected: ShadowBenchmarkExpectation<'_>,
) -> ShadowRankReport {
    shadow_rank_internal(req, expected, &LiveCredentials)
}

pub fn shadow_rank_with_credentials(
    req: &RouteRequest<'_>,
    credentials: &dyn CredentialProbe,
) -> ShadowRankReport {
    shadow_rank_internal(req, ShadowBenchmarkExpectation::default(), credentials)
}

pub fn shadow_rank(req: &RouteRequest<'_>) -> ShadowRankReport {
    shadow_rank_with_credentials(req, &LiveCredentials)
}

// ---------------------------------------------------------------------------
//  4. Privacy gate — runs before any routing decision
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrivacyGate {
    CloudAllowed,
    /// Credentials, secrets or explicitly confidential content.
    LocalRequiredSensitive,
    /// The escalation came from content Noki read, not from the user.
    LocalRequiredSource,
    /// Engine mode is local-only.
    LocalRequiredMode,
}

impl PrivacyGate {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CloudAllowed => "cloud_allowed",
            Self::LocalRequiredSensitive => "local_required_sensitive",
            Self::LocalRequiredSource => "local_required_source",
            Self::LocalRequiredMode => "local_required_mode",
        }
    }

    pub fn allows_cloud(self) -> bool {
        self == Self::CloudAllowed
    }
}

/// Credential-shaped content that must never leave the machine.
///
/// This is deliberately not `memory::sensitive`: that gate also rejects any text
/// containing an '@' or a long digit run, which is right for what gets written
/// into long-term memory but would make ordinary work tasks local-only for the
/// wrong reason. Here the question is narrower — does this text carry secrets?
pub fn credential_like(text: &str) -> bool {
    let lower = text.to_lowercase();

    const MARKERS: &[&str] = &[
        "passwort",
        "password",
        "kennwort",
        "passphrase",
        "api key",
        "api-key",
        "apikey",
        "api_key",
        "secret",
        "geheimnis",
        "zugangsdaten",
        "credentials",
        "private key",
        "privater schlüssel",
        "begin rsa private key",
        "begin openssh private key",
        "begin private key",
        "keychain",
        "schlüsselbund",
        "seed phrase",
        "recovery phrase",
        "access token",
        "bearer ",
        "client_secret",
        "vertraulich",
        "streng geheim",
        "confidential",
        "kreditkarte",
        "credit card",
        "iban",
        "cvv",
    ];
    if MARKERS.iter().any(|m| lower.contains(m)) {
        return true;
    }

    // Token-shaped literals: a known secret prefix, or a long mixed-case string
    // with digits that does not read like prose.
    // Split on every non-token character: a secret is one contiguous run of
    // [A-Za-z0-9_-]. Code such as `THREE.AmbientLight(0x404040` is several
    // short runs, not one "mixed-case token with digits" (that misfire sent
    // every code follow-up to the local 9B).
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .any(|word| {
            let w = word;
            if w.len() < 20 {
                return false;
            }
            let lw = w.to_lowercase();
            if lw.starts_with("sk-")
                || lw.starts_with("sk_")
                || lw.starts_with("ghp_")
                || lw.starts_with("gsk_")
                || lw.starts_with("xox")
                || lw.starts_with("aws_")
                || lw.starts_with("akia")
            {
                return true;
            }
            let digits = w.chars().filter(|c| c.is_ascii_digit()).count();
            let uppers = w.chars().filter(|c| c.is_ascii_uppercase()).count();
            let lowers = w.chars().filter(|c| c.is_ascii_lowercase()).count();
            digits >= 3 && uppers >= 3 && lowers >= 3 && !w.contains(' ')
        })
}

/// The gate itself. Order is the policy: mode, then source, then content.
///
/// External content (documents, MCP results, web pages) can never raise the
/// escalation source, so it can never force a cloud call.
pub fn privacy_gate(req: &RouteRequest) -> PrivacyGate {
    if req.engine_mode == EngineMode::OnlyLocal {
        return PrivacyGate::LocalRequiredMode;
    }
    if !req.escalation_source.may_escalate() {
        return PrivacyGate::LocalRequiredSource;
    }
    if req.is_sensitive()
        || (!req.user_sensitivity_prechecked && credential_like(req.prompt))
    {
        return PrivacyGate::LocalRequiredSensitive;
    }
    PrivacyGate::CloudAllowed
}

/// Finalize the request lane before a candidate list is built. Privacy can only
/// narrow a request to local; no runtime error can call this function again or
/// widen the returned value.
fn resolve_execution_lane(req: &RouteRequest, gate: PrivacyGate) -> ExecutionLane {
    if !gate.allows_cloud() || req.engine_mode == EngineMode::OnlyLocal {
        return ExecutionLane::Local;
    }
    match req.requested_lane {
        // Paid Cloud ist entfernt: eine (alte) Paid-Anfrage laeuft als Free
        // Cloud — nie als bezahlter Aufruf, auch nicht mit Freigabe.
        ExecutionLane::PaidCloud => ExecutionLane::FreeCloud,
        lane => lane,
    }
}

/// Compatibility adapter for the pre-registry-migration `free_verified` flag.
/// A legacy contradiction always fails closed.
fn effective_cost_safety(entry: &ModelEntry) -> CostSafety {
    if entry.is_local() {
        CostSafety::Local
    } else if !entry.free_verified && entry.cost_safety == CostSafety::VerifiedFreeHardStop {
        CostSafety::FreeButBillingUncertain
    } else {
        entry.cost_safety
    }
}

fn candidate_allowed_in_lane(entry: &ModelEntry, lane: ExecutionLane) -> bool {
    // Bezahlte Modelle sind fuer neue Aufgaben unerreichbar, egal welche Spur.
    if effective_cost_safety(entry) == CostSafety::MeteredPaid {
        return false;
    }
    lane.allows(effective_cost_safety(entry))
}

// ---------------------------------------------------------------------------
//  5. Health registry
// ---------------------------------------------------------------------------

/// Compatibility view over the canonical provider/connection/model runtime
/// state. Existing router and UI APIs keep their old shape during migration.
///
/// Mapping is one-way only:
/// `RuntimeAvailability`/runtime metadata -> `ProviderState`.
/// `ProviderState` is never persisted and cannot update the registry.
pub fn provider_state(id: &str) -> ProviderState {
    let Some(runtime) = runtime_registry::model_runtime_view(id) else {
        return ProviderState::CredentialsRequired;
    };
    if runtime
        .last_outcomes()
        .into_iter()
        .flatten()
        .any(|outcome| outcome == RuntimeOutcome::CapabilityMismatch)
    {
        return ProviderState::ModelRemoved;
    }
    if runtime
        .last_outcomes()
        .into_iter()
        .flatten()
        .any(|outcome| outcome == RuntimeOutcome::CostBlocked)
    {
        return ProviderState::CostUncertain;
    }
    let remaining = runtime_registry::remaining_duration(match runtime.effective_availability() {
        RuntimeAvailability::Disabled => None,
        RuntimeAvailability::AuthFailed => None,
        _ => runtime.cooldown_until(),
    });
    let until = || Instant::now() + remaining.unwrap_or_default();
    if runtime.effective_availability() == RuntimeAvailability::HalfOpen {
        return if runtime_registry::admit_probe(id) {
            ProviderState::Available
        } else {
            ProviderState::TemporarilyUnavailable {
                until: Instant::now(),
            }
        };
    }
    match runtime.effective_availability() {
        RuntimeAvailability::Available | RuntimeAvailability::Degraded => ProviderState::Available,
        RuntimeAvailability::RateLimited | RuntimeAvailability::Cooldown => {
            ProviderState::RateLimited { until: until() }
        }
        RuntimeAvailability::QuotaExhausted => ProviderState::QuotaExhausted {
            until: until(),
            reset_hint: runtime.quota_reset(),
        },
        RuntimeAvailability::TemporarilyUnavailable => {
            ProviderState::TemporarilyUnavailable { until: until() }
        }
        RuntimeAvailability::HalfOpen => ProviderState::TemporarilyUnavailable {
            until: Instant::now(),
        },
        RuntimeAvailability::AuthFailed => ProviderState::CredentialsRequired,
        RuntimeAvailability::Disabled => ProviderState::Disabled,
    }
}

#[cfg(test)]
fn set_state(id: &str, state: ProviderState, error_kind: Option<&'static str>) {
    match state {
        ProviderState::Available => {
            let _ = runtime_registry::set_model_enabled(id, true);
        }
        ProviderState::RateLimited { until } => {
            let mut observation =
                RuntimeObservation::outcome(RuntimeOutcome::RateLimited, OutcomeScope::Model);
            observation.cooldown_until = Some(runtime_registry::deadline_after(
                until.saturating_duration_since(Instant::now()),
            ));
            let _ = runtime_registry::observe(id, observation);
        }
        ProviderState::QuotaExhausted { until, reset_hint } => {
            let mut observation = RuntimeObservation::outcome(
                RuntimeOutcome::QuotaExhausted,
                OutcomeScope::Connection,
            );
            observation.cooldown_until = Some(runtime_registry::deadline_after(
                until.saturating_duration_since(Instant::now()),
            ));
            observation.quota_reset = reset_hint;
            let _ = runtime_registry::observe(id, observation);
        }
        ProviderState::TemporarilyUnavailable { until } => {
            let outcome = if error_kind == Some("timeout") {
                RuntimeOutcome::Timeout
            } else {
                RuntimeOutcome::ServiceUnavailable
            };
            let scope = if outcome == RuntimeOutcome::Timeout {
                OutcomeScope::Connection
            } else {
                OutcomeScope::Provider
            };
            let mut observation = RuntimeObservation::outcome(outcome, scope);
            observation.cooldown_until = Some(runtime_registry::deadline_after(
                until.saturating_duration_since(Instant::now()),
            ));
            let _ = runtime_registry::observe(id, observation);
        }
        ProviderState::CredentialsRequired => {
            let auth_state = if error_kind == Some("no_secret") {
                AuthState::Missing
            } else {
                AuthState::Failed
            };
            let _ = runtime_registry::set_auth_state(id, auth_state);
            let _ = runtime_registry::observe(
                id,
                RuntimeObservation::outcome(
                    RuntimeOutcome::AuthenticationFailed,
                    OutcomeScope::Connection,
                ),
            );
        }
        ProviderState::CostUncertain => {
            let _ = runtime_registry::observe(
                id,
                RuntimeObservation::outcome(RuntimeOutcome::CostBlocked, OutcomeScope::Model),
            );
        }
        ProviderState::ModelRemoved => {
            let _ = runtime_registry::observe(
                id,
                RuntimeObservation::outcome(RuntimeOutcome::CapabilityMismatch, OutcomeScope::Model),
            );
        }
        ProviderState::Disabled => {
            let _ = runtime_registry::set_model_enabled(id, false);
        }
    }
}

fn note_request(id: &str) {
    let _ = runtime_registry::note_request(id);
}

#[cfg(test)]
fn note_usage(id: &str, remaining: Option<String>, reset: Option<String>) {
    let _ = runtime_registry::note_quota(id, remaining, reset);
}

/// Authentication belongs to the concrete account/key/endpoint connection.
/// Other models on the same connection see it; unrelated connections do not.
fn mark_connection_credentials_required(entry: &ModelEntry) {
    let _ = runtime_registry::set_auth_state(entry.id, AuthState::Failed);
    let _ = runtime_registry::observe(
        entry.id,
        RuntimeObservation::outcome(
            RuntimeOutcome::AuthenticationFailed,
            OutcomeScope::Connection,
        ),
    );
}

/// Switches a whole cloud provider. The UI offers one toggle per provider, not
/// per model, and it writes the same `disabled` state the router already reads —
/// there is no second, UI-only kind of "off".
pub fn set_provider_enabled(provider: &str, enabled: bool) -> usize {
    let touched = catalog()
        .into_iter()
        .filter(|entry| entry.provider == provider && !entry.is_local())
        .count();
    let _ = runtime_registry::set_provider_enabled(provider, enabled);
    touched
}

/// Reconciles the router with the user's persisted provider switches.
///
/// Only the `disabled` bit is touched: a provider the user disabled becomes
/// `Disabled`, and one they re-enabled leaves `Disabled`. Cooldown states
/// (rate limited, quota exhausted, down) are left completely alone, so calling
/// this on every status refresh can never resurrect a provider that is actually
/// unavailable.
pub fn apply_disabled_providers(disabled: &[String]) {
    let providers = catalog()
        .into_iter()
        .filter(|entry| !entry.is_local())
        .map(|entry| entry.provider)
        .collect::<std::collections::HashSet<_>>();
    for provider in providers {
        let wanted_off = disabled.iter().any(|item| item == provider);
        let _ = runtime_registry::set_provider_enabled(provider, !wanted_off);
    }
    for id in disabled {
        if model_registry::model(id).is_some() {
            let _ = runtime_registry::set_model_enabled(id, false);
        }
    }
}

/// Whether a provider currently has a usable credential. The answer is cached
/// so opening settings never turns into a Keychain storm, and it is a boolean:
/// the secret itself never leaves the backend, not even masked.
pub fn credentials_present(provider: &str, probe: &dyn CredentialProbe) -> bool {
    const TTL: Duration = Duration::from_secs(300);
    static CACHE: OnceLock<Mutex<HashMap<String, (bool, Instant)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = cache.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((val, at)) = map.get(provider) {
        if at.elapsed() < TTL {
            return *val;
        }
    }
    let env_var = catalog()
        .into_iter()
        .find(|e| e.provider == provider && !e.is_local())
        .map(|e| e.env_var.to_string())
        .unwrap_or_default();
    let present = !env_var.is_empty() && probe.has_secret(&env_var);
    map.insert(provider.to_string(), (present, Instant::now()));
    present
}

/// Explicit user switch. `disabled` never expires on its own.
pub fn set_enabled(id: &str, enabled: bool) {
    let _ = runtime_registry::set_model_enabled(id, enabled);
}

/// Test and maintenance hook: forget all learned states.
pub fn reset_states() {
    runtime_registry::clear();
}

/// Providers that were evaluated but are not routable right now. They carry one
/// of the existing states — "parked" is a grouping in the UI, never a new state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ParkedProvider {
    pub id: &'static str,
    pub display_name: &'static str,
    pub state: &'static str,
    pub reason: &'static str,
    pub note: &'static str,
}

pub fn parked_providers() -> Vec<ParkedProvider> {
    model_registry::PARKED_CATALOG
        .iter()
        .map(|entry| ParkedProvider {
            id: entry.id,
            display_name: entry.display_name,
            state: entry.reason.legacy_runtime_label(),
            reason: entry.reason.as_str(),
            note: entry.note,
        })
        .collect()
}

/// Serializable snapshot for status output. No secrets, no prompts.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderStateSnapshot {
    pub id: String,
    pub provider: String,
    pub display_name: String,
    pub local: bool,
    pub state: String,
    pub status_role: &'static str,
    pub cost_safety: &'static str,
    pub cooldown_remaining_s: Option<u64>,
    pub reset_hint: Option<String>,
    pub last_error_kind: Option<String>,
    pub requests: u32,
    pub failures: u32,
    pub last_429_unix: Option<u64>,
    /// "connected" / "credentials_required"; local entries need none.
    pub credentials: &'static str,
    /// Real header allowance. `None` means there is no quota data to render.
    pub remaining_usage: Option<String>,
    /// Whether the reported quota is model-specific or shared by its
    /// provider/account. Never inferred by the UI.
    pub quota_scope: Option<&'static str>,
    /// Structured quota evidence; omitted for local models and unknown cloud limits.
    pub quota_telemetry: Option<runtime_registry::QuotaTelemetry>,
    /// Display-only local packaging metadata. Cloud entries leave this empty.
    pub local_quantization: Option<&'static str>,
    pub specialist_only: bool,
    pub measured: Measured,
}

pub fn state_snapshot(probe: &dyn CredentialProbe) -> Vec<ProviderStateSnapshot> {
    catalog()
        .into_iter()
        .map(|e| {
            let state = provider_state(e.id);
            let mut runtime =
                runtime_registry::model_runtime_view(e.id).expect("catalog entry is registered");
            // Workers AI exposes no per-response neuron balance. Its documented
            // free cap is still useful, shared account metadata and has no
            // routing effect. Seed it lazily when the status view is requested.
            let needs_cloudflare_baseline = runtime
                .quota_telemetry()
                .is_none_or(|quota| quota.source == "static_limit" && quota.local_reset_window != Some(runtime_registry::current_utc_day()));
            if e.provider == "cloudflare_workers_ai" && needs_cloudflare_baseline {
                let _ = runtime_registry::note_quota_telemetry(
                    e.id,
                    runtime_registry::QuotaTelemetry {
                        source: "static_limit".into(),
                        scope: "account".into(),
                        observed_at: 0,
                        reset_at: Some("00:00 UTC".into()),
                        neuron_limit: Some(10_000),
                        ..runtime_registry::QuotaTelemetry::default()
                    },
                );
                runtime = runtime_registry::model_runtime_view(e.id)
                    .expect("catalog entry is registered");
            }
            let credentials = if e.is_local() {
                "not_required"
            } else if matches!(runtime.auth_state(), AuthState::Missing | AuthState::Failed) {
                "credentials_required"
            } else if credentials_present(e.provider, probe) {
                "connected"
            } else {
                "credentials_required"
            };
            let last_error = runtime
                .last_outcomes()
                .into_iter()
                .flatten()
                .find(|outcome| outcome.is_failure());
            ProviderStateSnapshot {
                id: e.id.to_string(),
                provider: e.provider.to_string(),
                display_name: e.display_name.to_string(),
                local: e.is_local(),
                state: state.as_str().to_string(),
                status_role: e.status_role,
                cost_safety: effective_cost_safety(&e).as_str(),
                cooldown_remaining_s: state.cooldown_remaining(),
                reset_hint: runtime.quota_reset(),
                last_error_kind: last_error.map(|outcome| outcome.as_str().to_string()),
                requests: runtime.request_count(),
                failures: runtime.failure_count(),
                last_429_unix: runtime
                    .last_rate_limit()
                    .map(|milliseconds| milliseconds / 1000),
                credentials,
                remaining_usage: runtime.quota_remaining(),
                quota_scope: runtime.quota_scope(),
                quota_telemetry: (!e.is_local()).then(|| runtime.quota_telemetry()).flatten(),
                local_quantization: e.local.map(|model| model.quantization()),
                specialist_only: e.specialist_only,
                measured: e.measured,
            }
        })
        .collect()
}

/// The model the router would pick right now for a NORMAL task, given the live
/// provider states. Derived from the same chain the router walks, so the UI can
/// never drift from the actual decision.
pub fn current_default(class: TaskClass, mode: EngineMode) -> &'static str {
    for entry in chain(class, Tier::Normal, mode) {
        if entry.is_local() {
            return entry.display_name;
        }
        if provider_state(entry.id).is_selectable() {
            return entry.display_name;
        }
    }
    match class {
        TaskClass::Work => QWEN_9B.display_name,
        TaskClass::Coding => JACKOD.display_name,
    }
}

// ---------------------------------------------------------------------------
//  6. Execution seam (injectable, so tests never touch the network)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct ExecOk {
    pub text: String,
    pub latency_ms: u64,
    pub tokens: Option<u32>,
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    pub reasoning_tokens: Option<u32>,
    pub http_status: Option<u16>,
    pub rate_limit_headers: crate::cloud_engine::ProviderRateLimitHeaders,
    pub finish_reason: Option<String>,
    pub reset_hint: Option<String>,
    /// Remaining allowance EXACTLY as the provider's rate-limit headers stated
    /// it. Never derived from a response body's `usage` field: OpenRouter's
    /// `usage: 0` means "this call cost 0 credits", not "0 requests left".
    pub remaining_usage: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecErr {
    RateLimited {
        cooldown: Option<Duration>,
        reset_hint: Option<String>,
    },
    QuotaExhausted {
        cooldown: Option<Duration>,
        reset_hint: Option<String>,
    },
    Unavailable,
    Timeout,
    Auth,
    EmptyResponse,
    Failed,
    /// Local $0 proof missing/expired: nothing left the machine.
    CostBlocked,
    /// The provider no longer offers this model id (HTTP 404).
    NotOffered,
}

impl ExecErr {
    fn outcome(&self) -> RuntimeOutcome {
        match self {
            Self::RateLimited { .. } => RuntimeOutcome::RateLimited,
            Self::QuotaExhausted { .. } => RuntimeOutcome::QuotaExhausted,
            Self::Unavailable | Self::Failed => RuntimeOutcome::ServiceUnavailable,
            Self::Timeout => RuntimeOutcome::Timeout,
            Self::Auth => RuntimeOutcome::AuthenticationFailed,
            Self::EmptyResponse => RuntimeOutcome::EmptyResponse,
            Self::CostBlocked => RuntimeOutcome::CostBlocked,
            Self::NotOffered => RuntimeOutcome::ServiceUnavailable,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::CostBlocked => "cost_attestation_expired",
            Self::NotOffered => "model_not_offered",
            _ => self.outcome().as_str(),
        }
    }
}

pub trait ProviderExecutor {
    fn execute(
        &self,
        entry: &ModelEntry,
        prompt: &str,
        images: &[MultimodalAttachment],
        max_tokens: u32,
        timeout: Duration,
    ) -> Result<ExecOk, ExecErr>;
}

/// Answers "is there a usable secret" without ever revealing it.
pub trait CredentialProbe {
    fn has_secret(&self, env_var: &str) -> bool;
}

/// "A model starts producing this answer" (model id, execution lane) - called
/// the moment a cloud attempt starts and when the local floor is chosen, so
/// the chat can name the answering model immediately instead of only after
/// the answer arrived. Set once by the app; no prompt content passes here.
pub static MODELL_START: Mutex<Option<Box<dyn Fn(&str, &str) + Send + Sync>>> = Mutex::new(None);
pub fn modell_start_melden(model_id: &str, lane: &str) {
    if let Ok(g) = MODELL_START.lock() {
        if let Some(f) = g.as_ref() {
            f(model_id, lane);
        }
    }
}

/// The real executor: one curl request through the existing cloud engine.
pub struct LiveExecutor;

impl ProviderExecutor for LiveExecutor {
    fn execute(
        &self,
        entry: &ModelEntry,
        prompt: &str,
        images: &[MultimodalAttachment],
        max_tokens: u32,
        timeout: Duration,
    ) -> Result<ExecOk, ExecErr> {
        let cfg = match entry.cloud_config() {
            Some(c) => c,
            None => return Err(ExecErr::Failed),
        };
        modell_start_melden(
            entry.id,
            model_registry::model(entry.id).map(|d| d.execution_lane.as_str()).unwrap_or("FREE_CLOUD"),
        );
        match execute_provider_request_multimodal(&cfg, prompt, images, max_tokens, timeout) {
            Ok(resp) => {
                if let Some(telemetry) = resp.quota_telemetry.clone() {
                    let _ = runtime_registry::note_quota_telemetry(entry.id, telemetry);
                }
                // Workers AI does not disclose a neuron cost in ordinary
                // inference responses. A successful call therefore invalidates
                // the display-only zero baseline rather than inventing usage.
                if entry.provider == "cloudflare_workers_ai" {
                    let _ = runtime_registry::mark_static_quota_usage_unknown(entry.id);
                }
                Ok(ExecOk {
                    text: resp.text,
                    latency_ms: resp.latency_ms,
                    tokens: resp.tokens,
                    input_tokens: resp.input_tokens,
                    output_tokens: resp.output_tokens,
                    reasoning_tokens: resp.reasoning_tokens,
                    http_status: resp.http_status,
                    rate_limit_headers: resp.rate_limit_headers,
                    finish_reason: resp.finish_reason,
                    reset_hint: resp.reset_at,
                    remaining_usage: resp.remaining_usage,
                })
            }
            Err(ProviderExecError::RateLimited { cooldown, .. }) => Err(ExecErr::RateLimited {
                cooldown: Some(cooldown),
                reset_hint: None,
            }),
            Err(ProviderExecError::QuotaExhausted(_)) => Err(ExecErr::QuotaExhausted {
                cooldown: None,
                reset_hint: None,
            }),
            Err(ProviderExecError::Auth(_)) => Err(ExecErr::Auth),
            Err(ProviderExecError::Unavailable(_)) => Err(ExecErr::Unavailable),
            Err(ProviderExecError::Timeout(_)) => Err(ExecErr::Timeout),
            Err(ProviderExecError::EmptyResponse(_)) => Err(ExecErr::EmptyResponse),
            Err(ProviderExecError::CostBlocked(_)) => Err(ExecErr::CostBlocked),
            Err(ProviderExecError::Failed(msg)) if msg.contains(" 404") => Err(ExecErr::NotOffered),
            Err(ProviderExecError::Failed(_)) => Err(ExecErr::Failed),
        }
    }
}

/// The real credential probe: environment, then macOS Keychain.
pub struct LiveCredentials;

impl CredentialProbe for LiveCredentials {
    fn has_secret(&self, env_var: &str) -> bool {
        resolve_provider_secret(env_var).is_some()
    }
}

pub struct RouterDeps<'a> {
    pub executor: &'a dyn ProviderExecutor,
    pub credentials: &'a dyn CredentialProbe,
}

impl Default for RouterDeps<'static> {
    fn default() -> Self {
        Self {
            executor: &LiveExecutor,
            credentials: &LiveCredentials,
        }
    }
}

// ---------------------------------------------------------------------------
//  7. Request, outcome, audit
// ---------------------------------------------------------------------------

pub struct RouteRequest<'a> {
    pub task_id: u64,
    pub class: TaskClass,
    pub tier: Tier,
    pub prompt: &'a str,
    pub max_tokens: u32,
    pub engine_mode: EngineMode,
    /// Productive selection policy. STATIC remains a one-field rollback.
    pub routing_mode: RoutingMode,
    /// The caller's own sensitivity verdict (protected path, user marking, ...).
    /// Private on purpose: later layers may add sensitivity, never clear it.
    sensitive: bool,
    /// The caller separated user-authored input from attached/external context
    /// and applied the credential gate to that user input explicitly.
    user_sensitivity_prechecked: bool,
    /// Requested before candidate selection. The finalized lane is recorded on
    /// the outcome and cannot change during fallback handling.
    requested_lane: ExecutionLane,
    /// Structural placeholder for the future paid policy. No current caller
    /// authorizes it and no paid model exists in the productive catalog.
    paid_authorized: bool,
    /// Where the escalation came from. Only user intent or user policy may leave
    /// the machine — content never can.
    pub escalation_source: crate::specialist::EscalationSource,
    /// The task is genuinely high-value and the user asked for depth. Required
    /// before the Gemini specialist is even considered.
    pub allow_specialist: bool,
    /// Inside an agent loop. Disables the specialist outright.
    pub in_agent_loop: bool,
    /// Whether the local models can actually run right now.
    pub local_available: bool,
    /// A warm local model avoids a cold-load penalty under normal conditions;
    /// high pressure reverses that preference when FREE_CLOUD is eligible.
    pub local_model_resident: bool,
    pub resource_pressure: ResourcePressure,
    /// Hard capability requirements. These are metadata derived by the caller;
    /// prompt text is never copied into the shadow audit.
    pub required_context_tokens: u32,
    pub requires_reasoning: bool,
    pub requires_tools: bool,
    pub requires_structured_output: bool,
    pub requires_files: bool,
    pub requires_vision: bool,
    pub multimodal_attachments: Vec<MultimodalAttachment>,
    /// Optional gate over a cloud answer: `false` means clearly unusable.
    pub quality_gate: Option<&'a dyn Fn(&str) -> bool>,
}

impl<'a> RouteRequest<'a> {
    pub fn new(task_id: u64, class: TaskClass, tier: Tier, prompt: &'a str) -> Self {
        Self {
            task_id,
            class,
            tier,
            prompt,
            max_tokens: 500,
            engine_mode: EngineMode::LocalAndCloud,
            routing_mode: RoutingMode::configured(),
            sensitive: false,
            user_sensitivity_prechecked: false,
            requested_lane: ExecutionLane::FreeCloud,
            paid_authorized: false,
            escalation_source: crate::specialist::EscalationSource::UserIntent,
            allow_specialist: false,
            in_agent_loop: false,
            local_available: true,
            local_model_resident: false,
            resource_pressure: ResourcePressure::Normal,
            required_context_tokens: 1,
            requires_reasoning: tier == Tier::Deep,
            requires_tools: false,
            requires_structured_output: false,
            requires_files: false,
            requires_vision: false,
            multimodal_attachments: Vec::new(),
            quality_gate: None,
        }
    }

    /// Monotonic privacy propagation: `false` is deliberately a no-op.
    pub fn mark_sensitive(&mut self, sensitive: bool) {
        self.sensitive |= sensitive;
    }

    pub fn mark_user_input_sensitive(&mut self, sensitive: bool) {
        self.user_sensitivity_prechecked = true;
        self.sensitive |= sensitive;
    }

    pub fn is_sensitive(&self) -> bool {
        self.sensitive
    }

    pub fn request_lane(&mut self, lane: ExecutionLane) {
        self.requested_lane = lane;
        if lane != ExecutionLane::PaidCloud {
            self.paid_authorized = false;
        }
    }

    pub fn set_routing_mode(&mut self, mode: RoutingMode) {
        self.routing_mode = mode;
    }

    /// Kept internal to the routing foundation until paid policy and budgets
    /// are implemented. Merely selecting `PaidCloud` is not authorization.
    #[cfg(test)]
    fn authorize_paid_for_test(&mut self) {
        self.requested_lane = ExecutionLane::PaidCloud;
        self.paid_authorized = true;
    }
}

/// Final metadata-only comparison between the productive static result and the
/// recommendation computed before execution. It never contains prompt or
/// response content and never triggers a provider request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowModeAudit {
    pub report: ShadowRankReport,
    pub static_primary: Option<String>,
    pub static_selected_candidate: String,
    pub dynamic_selected_candidate: Option<String>,
    pub selection_differs: bool,
    pub difference_reason: String,
    pub fallback_count: usize,
}

/// Audit record: metadata only. No prompts, no documents, no secrets.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RouteAudit {
    pub task_id: u64,
    pub task_type: &'static str,
    pub tier: &'static str,
    pub engine_mode: &'static str,
    pub routing_mode: &'static str,
    pub execution_lane: &'static str,
    pub privacy_gate: &'static str,
    pub selected_provider: String,
    pub selected_model: String,
    pub reason: &'static str,
    pub provider_state: String,
    pub fallback_count: usize,
    pub result: &'static str,
    pub latency_ms: u64,
    pub quality_decision: &'static str,
    pub cloud_requests: usize,
    pub ranked_candidates: Vec<String>,
    pub filter_fallback_reasons: Vec<String>,
    pub rank_factors: Vec<ShadowRankedCandidate>,
    pub runtime_outcome: String,
    pub local_floor_used: bool,
    pub shadow: ShadowModeAudit,
}

pub fn format_route_audit(a: &RouteAudit) -> String {
    format!(
        "noki-router task={} type={} tier={} engine={} routing_mode={} lane={} privacy={} provider={} model={} reason={} state={} fallbacks={} result={} runtime_outcome={} local_floor={} ms={} quality={} cloud_requests={} ranked_candidates={} filter_fallback_reasons={} shadow_static={} shadow_dynamic={} shadow_differs={} shadow_candidates={} shadow_rejected={}",
        a.task_id,
        a.task_type,
        a.tier,
        a.engine_mode,
        a.routing_mode,
        a.execution_lane,
        a.privacy_gate,
        a.selected_provider,
        a.selected_model,
        a.reason,
        a.provider_state,
        a.fallback_count,
        a.result,
        a.runtime_outcome,
        a.local_floor_used,
        a.latency_ms,
        a.quality_decision,
        a.cloud_requests,
        a.ranked_candidates.len(),
        a.filter_fallback_reasons.len(),
        a.shadow.static_selected_candidate,
        a.shadow.dynamic_selected_candidate.as_deref().unwrap_or("none"),
        a.shadow.selection_differs,
        a.shadow.report.candidates.len(),
        a.shadow.report.rejected.len(),
    )
}

fn finish_shadow_audit(
    report: &ShadowRankReport,
    static_selected_candidate: &str,
    fallback_count: usize,
) -> ShadowModeAudit {
    let dynamic = report.selected_candidate.clone();
    let differs = dynamic.as_deref() != Some(static_selected_candidate);
    let difference_reason = if dynamic.is_none() {
        "no_dynamic_candidate"
    } else if differs {
        "different_rank_or_hard_filter"
    } else {
        "same_selection"
    };
    ShadowModeAudit {
        report: report.clone(),
        static_primary: report.static_chain.first().cloned(),
        static_selected_candidate: static_selected_candidate.to_string(),
        dynamic_selected_candidate: dynamic,
        selection_differs: differs,
        difference_reason: difference_reason.to_string(),
        fallback_count,
    }
}

pub fn log_route(a: &RouteAudit) {
    log::info!("{}", format_route_audit(a));
}

#[derive(Clone, Debug)]
pub struct RouteOutcome {
    /// `Some` only when a cloud model actually produced an answer.
    pub answer: Option<String>,
    /// Set when the caller must run a local model — the normal terminal case.
    pub local_model: Option<LocalModel>,
    pub selected_provider: String,
    pub selected_model: String,
    pub reason: &'static str,
    pub fallback_count: usize,
    pub cloud_requests: usize,
    pub latency_ms: u64,
    pub tokens: Option<u32>,
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    pub reasoning_tokens: Option<u32>,
    pub http_status: Option<u16>,
    pub rate_limit_headers: Option<crate::cloud_engine::ProviderRateLimitHeaders>,
    pub finish_reason: Option<String>,
    pub privacy_gate: PrivacyGate,
    pub execution_lane: ExecutionLane,
    pub audit: RouteAudit,
}

impl RouteOutcome {
    pub fn is_local(&self) -> bool {
        self.local_model.is_some()
    }
}

// ---------------------------------------------------------------------------
//  8. The router
// ---------------------------------------------------------------------------

/// Is the Gemini specialist permitted for this task? Every condition must hold.
fn specialist_permitted(req: &RouteRequest, gate: PrivacyGate, lane: ExecutionLane) -> bool {
    gate.allows_cloud()
        && candidate_allowed_in_lane(&GEMINI, lane)
        && (req.allow_specialist || req.requires_vision)
        && (req.tier == Tier::Deep || req.requires_vision)
        && !req.in_agent_loop
        && provider_state(GEMINI.id).is_selectable()
}

/// Routes one task. Performs at most one model call in the normal case, never a
/// parallel call, and never a retry of a provider that just failed.
pub fn route(req: &RouteRequest, deps: &RouterDeps) -> RouteOutcome {
    let out = route_inner(req, deps);
    // Release builds have no log backend: one line per routing decision so the
    // real lane/model/fallback is visible (no prompt, no secret).
    eprintln!(
        "[ROUTE] class={:?} tier={:?} engine={:?} lane={:?} gate={:?} provider={} model={} local={:?} reason={} fallbacks={} cloud_requests={} http={:?} finish={:?} {}ms",
        req.class, req.tier, req.engine_mode, out.execution_lane, out.privacy_gate,
        out.selected_provider, out.selected_model, out.local_model, out.reason,
        out.fallback_count, out.cloud_requests, out.http_status, out.finish_reason, out.latency_ms
    );
    if out.fallback_count > 0 {
        eprintln!("[ROUTE] why={}", out.audit.filter_fallback_reasons.join(","));
    }
    out
}

fn route_inner(req: &RouteRequest, deps: &RouterDeps) -> RouteOutcome {
    let gate = privacy_gate(req);
    // Immutable for the remainder of this function. Failures and quality
    // decisions can only advance within this lane or end at the local floor.
    let lane = resolve_execution_lane(req, gate);
    // One metadata snapshot feeds both the audit and productive DYNAMIC mode.
    // The candidate order is frozen before execution: failures only advance
    // through this already-ranked same-lane list.
    let shadow_report = shadow_rank_with_credentials(req, deps.credentials);
    let mut fallback_count = 0usize;
    let mut cloud_requests = 0usize;
    let mut quality_switches = 0usize;
    let mut cloud_attempts = 0usize;
    // "not_consulted" is the honest default: under local-only or a closed
    // privacy gate no provider state was ever looked at.
    let mut last_state = if gate.allows_cloud() && lane != ExecutionLane::Local {
        "available".to_string()
    } else {
        "not_consulted".to_string()
    };
    let mut quality_decision = "not_run";
    let mut last_runtime_outcome = "not_attempted".to_string();
    let mut filter_fallback_reasons = shadow_report
        .rejected
        .iter()
        .map(|rejection| format!("{}:{}", rejection.model_id, rejection.reason))
        .collect::<Vec<_>>();

    // In productive dynamic mode, a credential hard-filter is also reconciled
    // into the canonical runtime registry just as the previous static walk did.
    // This is metadata-only and happens after the candidate order is frozen.
    if req.routing_mode == RoutingMode::Dynamic {
        for rejection in shadow_report
            .rejected
            .iter()
            .filter(|rejection| rejection.filter == ShadowFilter::Credentials)
        {
            let _ = runtime_registry::set_auth_state(&rejection.model_id, AuthState::Missing);
            let _ = runtime_registry::observe(
                &rejection.model_id,
                RuntimeObservation::outcome(
                    RuntimeOutcome::AuthenticationFailed,
                    OutcomeScope::Connection,
                ),
            );
            last_runtime_outcome = RuntimeOutcome::AuthenticationFailed.as_str().to_string();
        }
    }

    let candidates: Vec<ModelEntry> = match req.routing_mode {
        RoutingMode::Static => {
            // Exact pre-activation behavior, including the single-shot
            // specialist ahead of the unchanged static chain.
            let mut static_candidates = Vec::new();
            if specialist_permitted(req, gate, lane) {
                static_candidates.push(GEMINI);
            }
            if req.requires_vision {
                if candidate_allowed_in_lane(&NEX_N2_5_PRO, lane) && provider_state(NEX_N2_5_PRO.id).is_selectable() {
                    static_candidates.push(NEX_N2_5_PRO);
                }
            }
            static_candidates.extend(chain(req.class, req.tier, req.engine_mode));
            if req.requires_vision {
                static_candidates.retain(|e| model_registry::model(e.id).map(|m| m.capabilities.vision).unwrap_or(false));
            }
            static_candidates
        }
        RoutingMode::Dynamic => {
            let mut dynamic_candidates: Vec<ModelEntry> = shadow_report
                .candidates
                .iter()
                .map(|candidate| entry_by_registry_id(&candidate.model_id))
                .collect();
            if req.requires_vision && dynamic_candidates.is_empty() {
                if specialist_permitted(req, gate, lane) {
                    dynamic_candidates.push(GEMINI);
                }
                if candidate_allowed_in_lane(&NEX_N2_5_PRO, lane) && provider_state(NEX_N2_5_PRO.id).is_selectable() {
                    dynamic_candidates.push(NEX_N2_5_PRO);
                }
            }
            dynamic_candidates
        }
    };
    let ranked_candidates = candidates
        .iter()
        .map(|candidate| candidate.id.to_string())
        .collect::<Vec<_>>();

    for entry in candidates {
        if entry.is_local() {
            if !req.local_available {
                filter_fallback_reasons.push(format!("{}:local_runtime_unavailable", entry.id));
                fallback_count += 1;
                continue;
            }
            let reason = local_reason(gate, lane, fallback_count);
            return finish_local(
                req,
                entry,
                gate,
                lane,
                reason,
                fallback_count,
                cloud_requests,
                quality_decision,
                &last_state,
                &shadow_report,
                &ranked_candidates,
                &filter_fallback_reasons,
                &last_runtime_outcome,
            );
        }

        // --- cloud candidate: every gate before a single byte leaves ---
        if !gate.allows_cloud() {
            filter_fallback_reasons.push(format!("{}:privacy_gate_requires_local", entry.id));
            fallback_count += 1;
            continue;
        }
        if !candidate_allowed_in_lane(&entry, lane) {
            if effective_cost_safety(&entry) == CostSafety::FreeButBillingUncertain {
                let _ = runtime_registry::observe(
                    entry.id,
                    RuntimeObservation::outcome(RuntimeOutcome::CostBlocked, OutcomeScope::Model),
                );
            }
            filter_fallback_reasons.push(format!("{}:execution_lane_or_cost_blocked", entry.id));
            fallback_count += 1;
            continue;
        }
        if entry.specialist_only && !specialist_permitted(req, gate, lane) {
            filter_fallback_reasons.push(format!("{}:specialist_policy_not_satisfied", entry.id));
            fallback_count += 1;
            continue;
        }
        if cloud_attempts >= MAX_CLOUD_ATTEMPTS {
            filter_fallback_reasons.push(format!("{}:max_cloud_attempts_reached", entry.id));
            fallback_count += 1;
            continue;
        }

        let state = provider_state(entry.id);
        last_state = state.as_str().to_string();
        if !state.is_selectable() {
            filter_fallback_reasons.push(format!("{}:runtime_not_selectable", entry.id));
            fallback_count += 1;
            continue;
        }
        if !deps.credentials.has_secret(entry.env_var) {
            let _ = runtime_registry::set_auth_state(entry.id, AuthState::Missing);
            let _ = runtime_registry::observe(
                entry.id,
                RuntimeObservation::outcome(
                    RuntimeOutcome::AuthenticationFailed,
                    OutcomeScope::Connection,
                ),
            );
            last_runtime_outcome = RuntimeOutcome::AuthenticationFailed.as_str().to_string();
            filter_fallback_reasons.push(format!("{}:auth_failed", entry.id));
            fallback_count += 1;
            continue;
        }

        cloud_attempts += 1;
        cloud_requests += 1;
        note_request(entry.id);
        let timeout = cloud_call_timeout(req.tier, req.max_tokens);

        match deps
            .executor
            .execute(&entry, req.prompt, &req.multimodal_attachments, req.max_tokens, timeout)
        {
            Ok(resp) => {
                if resp.text.trim().is_empty() {
                    let _ = runtime_registry::observe(
                        entry.id,
                        RuntimeObservation::outcome(
                            RuntimeOutcome::EmptyResponse,
                            OutcomeScope::Model,
                        ),
                    );
                    last_state = RuntimeAvailability::Degraded.as_str().to_string();
                    last_runtime_outcome = RuntimeOutcome::EmptyResponse.as_str().to_string();
                    filter_fallback_reasons.push(format!("{}:empty_response", entry.id));
                    fallback_count += 1;
                    continue;
                }

                // A healthy answer clears stale scoped cooldown memory and
                // stores only provider-reported quota metadata.
                let mut observation =
                    RuntimeObservation::outcome(RuntimeOutcome::Success, OutcomeScope::Model);
                observation.last_latency_ms = Some(resp.latency_ms);
                observation.quota_remaining = resp.remaining_usage.clone();
                observation.quota_reset = resp.reset_hint.clone();
                let _ = runtime_registry::observe(entry.id, observation);

                if let Some(gate_fn) = req.quality_gate {
                    if !gate_fn(&resp.text) {
                        let _ = runtime_registry::observe(
                            entry.id,
                            RuntimeObservation::outcome(
                                RuntimeOutcome::QualityFailure,
                                OutcomeScope::Model,
                            ),
                        );
                        quality_decision = "cloud_answer_rejected";
                        last_runtime_outcome = RuntimeOutcome::QualityFailure.as_str().to_string();
                        filter_fallback_reasons.push(format!("{}:quality_failure", entry.id));
                        quality_switches += 1;
                        fallback_count += 1;
                        if quality_switches > MAX_QUALITY_SWITCHES {
                            // Bounded chain: stop spending tokens, go local.
                            break;
                        }
                        continue;
                    }
                    quality_decision = "cloud_answer_accepted";
                }
                last_runtime_outcome = RuntimeOutcome::Success.as_str().to_string();

                let audit = RouteAudit {
                    task_id: req.task_id,
                    task_type: req.class.as_str(),
                    tier: req.tier.as_str(),
                    engine_mode: req.engine_mode.as_str(),
                    routing_mode: req.routing_mode.as_str(),
                    execution_lane: lane.as_str(),
                    privacy_gate: gate.as_str(),
                    selected_provider: entry.provider.to_string(),
                    selected_model: entry.id.to_string(),
                    reason: if entry.specialist_only {
                        "specialist_single_shot"
                    } else if fallback_count == 0 {
                        "primary_available"
                    } else {
                        "fallback_after_provider_failure"
                    },
                    provider_state: "available".to_string(),
                    fallback_count,
                    result: "cloud_ok",
                    latency_ms: resp.latency_ms,
                    quality_decision,
                    cloud_requests,
                    ranked_candidates: ranked_candidates.clone(),
                    filter_fallback_reasons: filter_fallback_reasons.clone(),
                    rank_factors: shadow_report.candidates.clone(),
                    runtime_outcome: last_runtime_outcome.clone(),
                    local_floor_used: false,
                    shadow: finish_shadow_audit(&shadow_report, entry.id, fallback_count),
                };
                log_route(&audit);
                return RouteOutcome {
                    answer: Some(resp.text),
                    local_model: None,
                    selected_provider: entry.provider.to_string(),
                    selected_model: entry.id.to_string(),
                    reason: audit.reason,
                    fallback_count,
                    cloud_requests,
                    latency_ms: resp.latency_ms,
                    tokens: resp.tokens,
                    input_tokens: resp.input_tokens,
                    output_tokens: resp.output_tokens,
                    reasoning_tokens: resp.reasoning_tokens,
                    http_status: resp.http_status,
                    rate_limit_headers: Some(resp.rate_limit_headers),
                    finish_reason: resp.finish_reason,
                    privacy_gate: gate,
                    execution_lane: lane,
                    audit,
                };
            }
            Err(err) => {
                if matches!(err, ExecErr::CostBlocked) {
                    // Refused before any byte left: it neither counts as a
                    // cloud request nor uses up one of the bounded attempts,
                    // so the next healthy candidate is still reached.
                    cloud_attempts -= 1;
                    cloud_requests -= 1;
                }
                apply_failure(&entry, &err);
                last_runtime_outcome = err.kind().to_string();
                filter_fallback_reasons.push(format!("{}:{}", entry.id, err.kind()));
                last_state = provider_state(entry.id).as_str().to_string();
                fallback_count += 1;
                // No retry of this provider. Straight on to the next candidate.
                continue;
            }
        }
    }

    // Chain exhausted: the local floor. This is reached when every cloud
    // candidate was skipped or failed — never by waiting.
    let floor = local_floor(req.class, req.tier);
    finish_local(
        req,
        floor,
        gate,
        lane,
        if gate.allows_cloud() && lane != ExecutionLane::Local {
            "all_cloud_candidates_unavailable"
        } else {
            local_reason(gate, lane, fallback_count)
        },
        fallback_count,
        cloud_requests,
        quality_decision,
        &last_state,
        &shadow_report,
        &ranked_candidates,
        &filter_fallback_reasons,
        &last_runtime_outcome,
    )
}

fn local_reason(gate: PrivacyGate, lane: ExecutionLane, fallback_count: usize) -> &'static str {
    match gate {
        PrivacyGate::LocalRequiredMode => "engine_mode_local_only",
        PrivacyGate::LocalRequiredSensitive => "privacy_gate_sensitive_content",
        PrivacyGate::LocalRequiredSource => "escalation_source_not_user",
        PrivacyGate::CloudAllowed => {
            if lane == ExecutionLane::Local {
                "execution_lane_local"
            } else if fallback_count == 0 {
                "local_primary"
            } else {
                "local_fallback_after_cloud_failure"
            }
        }
    }
}

fn finish_local(
    req: &RouteRequest,
    entry: ModelEntry,
    gate: PrivacyGate,
    lane: ExecutionLane,
    reason: &'static str,
    fallback_count: usize,
    cloud_requests: usize,
    quality_decision: &'static str,
    // State of the last cloud candidate that was considered: the audit's answer
    // to "why local?", so it must not be flattened to "available" when a
    // provider was actually rate limited or down.
    last_cloud_state: &str,
    shadow_report: &ShadowRankReport,
    ranked_candidates: &[String],
    filter_fallback_reasons: &[String],
    last_runtime_outcome: &str,
) -> RouteOutcome {
    let local = entry.local.unwrap_or(LocalModel::Qwen9B);
    let audit = RouteAudit {
        task_id: req.task_id,
        task_type: req.class.as_str(),
        tier: req.tier.as_str(),
        engine_mode: req.engine_mode.as_str(),
        routing_mode: req.routing_mode.as_str(),
        execution_lane: lane.as_str(),
        privacy_gate: gate.as_str(),
        selected_provider: "local".to_string(),
        selected_model: local.as_str().to_string(),
        reason,
        provider_state: last_cloud_state.to_string(),
        fallback_count,
        result: "local_selected",
        latency_ms: 0,
        quality_decision,
        cloud_requests,
        ranked_candidates: ranked_candidates.to_vec(),
        filter_fallback_reasons: filter_fallback_reasons.to_vec(),
        rank_factors: shadow_report.candidates.clone(),
        runtime_outcome: last_runtime_outcome.to_string(),
        local_floor_used: true,
        shadow: finish_shadow_audit(shadow_report, entry.id, fallback_count),
    };
    log_route(&audit);
    modell_start_melden(local.as_str(), "LOCAL");
    RouteOutcome {
        answer: None,
        local_model: Some(local),
        selected_provider: "local".to_string(),
        selected_model: local.as_str().to_string(),
        reason,
        fallback_count,
        cloud_requests,
        latency_ms: 0,
        tokens: None,
        input_tokens: None,
        output_tokens: None,
        reasoning_tokens: None,
        http_status: None,
        rate_limit_headers: None,
        finish_reason: None,
        privacy_gate: gate,
        execution_lane: lane,
        audit,
    }
}

/// The guaranteed floor per class. Coding stays on the Noki Native harness.
fn local_floor(class: TaskClass, tier: Tier) -> ModelEntry {
    match (class, tier) {
        // 9B is the local Work default for every tier; the 4B is only the
        // runtime fallback when the 9B fails (NokiLocalModel::generate).
        (TaskClass::Work, _) => QWEN_9B,
        (TaskClass::Coding, _) => JACKOD,
    }
}

/// Maps a failure onto a provider state. 429 on Gemini means the free budget is
/// gone, not that the minute window is full — that distinction is what protects
/// the quota.
fn apply_failure(entry: &ModelEntry, err: &ExecErr) {
    let observe = |outcome: RuntimeOutcome,
                   scope: OutcomeScope,
                   cooldown: Option<Duration>,
                   reset_hint: Option<String>| {
        let mut observation = RuntimeObservation::outcome(outcome, scope);
        observation.cooldown_until = cooldown.map(runtime_registry::deadline_after);
        observation.quota_reset = reset_hint;
        let _ = runtime_registry::observe(entry.id, observation);
    };
    match err {
        ExecErr::QuotaExhausted {
            cooldown,
            reset_hint,
        } => {
            observe(
                RuntimeOutcome::QuotaExhausted,
                OutcomeScope::Connection,
                Some(cooldown.unwrap_or(QUOTA_COOLDOWN)),
                reset_hint.clone(),
            );
        }
        ExecErr::RateLimited {
            cooldown,
            reset_hint,
        } => {
            if entry.provider == "google" {
                observe(
                    RuntimeOutcome::QuotaExhausted,
                    OutcomeScope::Connection,
                    Some(cooldown.unwrap_or(QUOTA_COOLDOWN)),
                    reset_hint.clone(),
                );
            } else {
                observe(
                    RuntimeOutcome::RateLimited,
                    OutcomeScope::Model,
                    Some(cooldown.unwrap_or(RATE_LIMIT_COOLDOWN)),
                    reset_hint.clone(),
                );
            }
        }
        ExecErr::Unavailable | ExecErr::Timeout => {
            let (outcome, scope) = if matches!(err, ExecErr::Timeout) {
                (RuntimeOutcome::Timeout, OutcomeScope::Connection)
            } else {
                (RuntimeOutcome::ServiceUnavailable, OutcomeScope::Provider)
            };
            observe(outcome, scope, Some(UNAVAILABLE_COOLDOWN), None);
        }
        ExecErr::Auth => mark_connection_credentials_required(entry),
        ExecErr::EmptyResponse => {
            let _ = runtime_registry::observe(
                entry.id,
                RuntimeObservation::outcome(RuntimeOutcome::EmptyResponse, OutcomeScope::Model),
            );
        }
        ExecErr::Failed => {
            observe(
                RuntimeOutcome::ServiceUnavailable,
                OutcomeScope::Provider,
                Some(FAILURE_COOLDOWN),
                None,
            );
        }
        // CostBlocked: The user's $0 attestation has expired. This is an
        // approval state (Permission), NOT an Unhealthy/Broken server (Health).
        // No circuit-breaker trip, no multi-hour cooldown.
        ExecErr::CostBlocked => {
            observe(RuntimeOutcome::CostBlocked, OutcomeScope::Model, None, None);
        }
        // NotOffered: 404 returned because the provider retired/removed this exact
        // model id (e.g. OpenRouter removed the free DeepSeek variant).
        ExecErr::NotOffered => {
            observe(RuntimeOutcome::CapabilityMismatch, OutcomeScope::Model, None, None);
        }
    }
}

/// Per-call cloud timeout: the tier's latency budget, but never shorter than
/// the time a free provider needs to WRITE the requested output (~40 tok/s,
/// capped at 90 s). With a flat 15 s, every 700-2600-token research section
/// timed out (no HTTP status), the router fell back to the local 9B and the
/// 1000-word Sushi article ran into the delivery deadline.
fn cloud_call_timeout(tier: Tier, max_tokens: u32) -> Duration {
    let schreiben = Duration::from_secs(8 + u64::from(max_tokens) / 40);
    // 150 s cap: a code step writes whole files (up to ~6000 tokens).
    tier_timeout(tier).max(schreiben).min(Duration::from_secs(150))
}

fn tier_timeout(tier: Tier) -> Duration {
    match tier {
        Tier::Fast => Duration::from_secs(8),
        Tier::Normal => Duration::from_secs(15),
        Tier::Deep => Duration::from_secs(25),
    }
}

// ---------------------------------------------------------------------------
//  9. Engine mode
// ---------------------------------------------------------------------------
//
// The engine mode is NOT stored here. It already lives in the app's own
// `settings.json` (`Settings::engine_mode`), written whenever the user changes
// it and read back at startup. A second store would only create a second truth,
// so the router takes the mode as an input on every request, and
// `Intelligence::new` applies the persisted value to the shared engine.

/// Status for the existing Intelligence settings view. Metadata only: no
/// prompts, no answers, and never a key — `credentials` is a word, not a value.
///
/// The chains come from `chain()`, the same function the router walks, so the
/// view follows automatically when the routing changes.
pub fn router_status_json(mode: EngineMode, probe: &dyn CredentialProbe) -> serde_json::Value {
    let names = |class: TaskClass, tier: Tier, m: EngineMode| {
        chain(class, tier, m)
            .iter()
            .map(|e| e.display_name)
            .collect::<Vec<_>>()
    };
    serde_json::json!({
        "engine_mode": mode.as_str(),
        "local_only": mode == EngineMode::OnlyLocal,
        "work_chains": {
            "FAST": names(TaskClass::Work, Tier::Fast, mode),
            "NORMAL": names(TaskClass::Work, Tier::Normal, mode),
            "DEEP": names(TaskClass::Work, Tier::Deep, mode),
            "LOCAL": names(TaskClass::Work, Tier::Normal, EngineMode::OnlyLocal),
        },
        "coding_chains": {
            "FAST": names(TaskClass::Coding, Tier::Fast, mode),
            "NORMAL": names(TaskClass::Coding, Tier::Normal, mode),
            "COMPLEX": names(TaskClass::Coding, Tier::Deep, mode),
            "LOCAL": names(TaskClass::Coding, Tier::Normal, EngineMode::OnlyLocal),
        },
        "current_defaults": {
            "work": current_default(TaskClass::Work, mode),
            "coding": current_default(TaskClass::Coding, mode),
        },
        "specialist": {
            "id": GEMINI.id,
            "model": GEMINI.display_name,
            "label": "Limited quota",
            "hint": "Used only for selected high-value tasks",
            "rules": ["deep_only", "single_shot", "no_agent_loop", "no_retry", "requires_user_intent"],
        },
        "providers": state_snapshot(probe),
        "parked": parked_providers(),
    })
}

// ---------------------------------------------------------------------------
//  Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// The health registry is global on purpose — provider state is a property
    /// of the machine, not of a call. Tests therefore run one at a time.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Records every call so a test can assert "exactly one request" or "none".
    struct MockExecutor {
        calls: RefCell<Vec<String>>,
        responses: RefCell<HashMap<String, Result<ExecOk, ExecErr>>>,
        default: Result<ExecOk, ExecErr>,
    }

    impl MockExecutor {
        fn new() -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                responses: RefCell::new(HashMap::new()),
                default: Ok(ExecOk {
                    text: "Antwort".into(),
                    latency_ms: 900,
                    tokens: Some(120),
                    input_tokens: Some(40),
                    output_tokens: Some(80),
                    reasoning_tokens: Some(0),
                    http_status: Some(200),
                    rate_limit_headers: crate::cloud_engine::ProviderRateLimitHeaders::default(),
                    finish_reason: Some("stop".into()),
                    reset_hint: None,
                    remaining_usage: None,
                }),
            }
        }
        fn with(self, id: &str, res: Result<ExecOk, ExecErr>) -> Self {
            self.responses.borrow_mut().insert(id.to_string(), res);
            self
        }
        fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
    }

    impl ProviderExecutor for MockExecutor {
        fn execute(
            &self,
            entry: &ModelEntry,
            _prompt: &str,
            _images: &[MultimodalAttachment],
            _max_tokens: u32,
            _timeout: Duration,
        ) -> Result<ExecOk, ExecErr> {
            self.calls.borrow_mut().push(entry.id.to_string());
            self.responses
                .borrow()
                .get(entry.id)
                .cloned()
                .unwrap_or_else(|| self.default.clone())
        }
    }

    struct AllSecrets;
    impl CredentialProbe for AllSecrets {
        fn has_secret(&self, _env_var: &str) -> bool {
            true
        }
    }

    /// Counts probes, so local-only can be proven not to touch credentials.
    struct CountingSecrets {
        probes: RefCell<usize>,
        present: bool,
    }
    impl CredentialProbe for CountingSecrets {
        fn has_secret(&self, _env_var: &str) -> bool {
            *self.probes.borrow_mut() += 1;
            self.present
        }
    }

    fn req<'a>(class: TaskClass, tier: Tier, prompt: &'a str) -> RouteRequest<'a> {
        let mut request = RouteRequest::new(1, class, tier, prompt);
        request.set_routing_mode(RoutingMode::Dynamic);
        request
    }

    fn run<'a>(r: &RouteRequest<'a>, ex: &MockExecutor) -> RouteOutcome {
        reset_states();
        let deps = RouterDeps {
            executor: ex,
            credentials: &AllSecrets,
        };
        route(r, &deps)
    }

    struct ShadowScenarioOracle<'a> {
        eligible: &'a [&'a str],
        filtered: &'a [(&'a str, ShadowFilter, &'a str)],
        ranking: &'a [&'a str],
        winner: Option<&'a str>,
        fallback: &'a [&'a str],
    }

    fn assert_shadow_oracle(report: &ShadowRankReport, oracle: ShadowScenarioOracle<'_>) {
        let eligible = report
            .candidates
            .iter()
            .map(|candidate| candidate.model_id.as_str())
            .collect::<Vec<_>>();
        let ranking = eligible.clone();
        let fallback = eligible.iter().skip(1).copied().collect::<Vec<_>>();
        let mut filtered = report
            .rejected
            .iter()
            .map(|rejection| {
                (
                    rejection.model_id.as_str(),
                    rejection.filter,
                    rejection.reason.as_str(),
                )
            })
            .collect::<Vec<_>>();
        let mut expected_filtered = oracle.filtered.to_vec();
        filtered.sort_by(|a, b| a.0.cmp(b.0).then_with(|| a.2.cmp(b.2)));
        expected_filtered.sort_by(|a, b| a.0.cmp(b.0).then_with(|| a.2.cmp(b.2)));

        assert_eq!(eligible, oracle.eligible, "eligible candidates");
        assert_eq!(filtered, expected_filtered, "filtered candidates");
        assert_eq!(ranking, oracle.ranking, "ranking");
        assert_eq!(
            report.selected_candidate.as_deref(),
            oracle.winner,
            "winner"
        );
        assert_eq!(fallback, oracle.fallback, "fallback order");
        if report.execution_lane != ExecutionLane::Local && report.privacy_gate.allows_cloud() {
            assert!(
                report
                    .candidates
                    .last()
                    .is_some_and(|candidate| candidate.is_local_floor),
                "local floor must be the terminal eligible fallback"
            );
        }
    }

    fn synthetic_observation(model_id: &str, outcome: RuntimeOutcome, latency_ms: Option<u64>) {
        runtime_registry::note_request(model_id).unwrap();
        let mut observation = RuntimeObservation::outcome(outcome, OutcomeScope::Model);
        observation.last_latency_ms = latency_ms;
        runtime_registry::observe(model_id, observation).unwrap();
    }

    fn synthetic_candidate(
        id: &str,
        confidence: ShadowBenchmarkConfidence,
        score: u32,
        champion: bool,
    ) -> ShadowRankedCandidate {
        let components = ShadowComponents {
            quality: score,
            availability: 10_000,
            completion: 10_000,
            reliability: 10_000,
            quality_stability: 10_000,
            health: 10_000,
            p95_latency: 10_000,
            quota_freshness: 10_000,
            capability: 10_000,
            cost: 10_000,
        };
        ShadowRankedCandidate {
            final_rank: 0,
            model_id: id.to_string(),
            provider: "synthetic".to_string(),
            role: RoutingRole::WorkBalanced,
            score,
            components,
            weights: ShadowWeights::for_tier(Tier::Normal),
            benchmark_score: score,
            benchmark_confidence: confidence,
            benchmark_suite: crate::cloud_engine::WORK_SUITE_ID.to_string(),
            benchmark_version: crate::cloud_engine::BENCHMARK_SUITE_VERSION.to_string(),
            benchmark_fingerprint: "synthetic-valid-v2".to_string(),
            completion: 10_000,
            reliability: 10_000,
            p95_latency_ms: Some(100),
            quota: "available".to_string(),
            is_role_champion: champion,
            is_local_floor: false,
        }
    }

    // --- Work routing -----------------------------------------------------

    #[test]
    fn generated_code_is_not_a_credential_but_real_tokens_are() {
        assert!(!credential_like("const light = new THREE.AmbientLight(0x404040, 0.6); scene.add(light);"));
        assert!(!credential_like("renderer.setPixelRatio(window.devicePixelRatio); camera.position.set(4.5, 2.2, 6.0);"));
        assert!(credential_like("export KEY=sk-proj-AbCdEf1234567890XyZ"));
        assert!(credential_like("token ghp_1234567890abcdefGHIJKLmnop"));
        assert!(credential_like("Mein Token ist Xy7Ab9Kq2Lm4Np8Rs5Tu"));
    }

    /// Measured 2026-09-30: DeepSeek free retired (404), GLM/GPT-OSS cost
    /// attestations expired. Those refusals used up all three cloud attempts,
    /// Codestral was never asked and the build ran on the local 9B.
    #[test]
    fn cost_blocked_and_retired_models_do_not_starve_the_coding_chain() {
        let _guard = lock();
        let ex = MockExecutor::new()
            .with(DEEPSEEK.id, Err(ExecErr::NotOffered))
            .with(CLOUDFLARE_GLM_4_7_FLASH.id, Err(ExecErr::CostBlocked))
            .with(GROQ_GPT_OSS.id, Err(ExecErr::CostBlocked));
        let out = run(&req(TaskClass::Coding, Tier::Normal, "Baue eine Seite"), &ex);
        assert_eq!(out.selected_model, CODESTRAL.id, "{:?}", ex.calls());
        assert_eq!(out.cloud_requests, 2, "cost-blocked refusals send nothing");
        // Not transient: the next request goes straight to Codestral.
        let ex2 = MockExecutor::new();
        let deps = RouterDeps { executor: &ex2, credentials: &AllSecrets };
        let out2 = route(&req(TaskClass::Coding, Tier::Normal, "Noch eine"), &deps);
        assert_eq!(out2.selected_model, CODESTRAL.id, "{:?}", ex2.calls());
        assert_eq!(ex2.calls(), vec![CODESTRAL.id]);
    }

    #[test]
    fn coding_uses_glm_when_deepseek_is_retired() {
        let _guard = lock();
        let ex = MockExecutor::new()
            .with(DEEPSEEK.id, Err(ExecErr::NotOffered));
        let out = run(&req(TaskClass::Coding, Tier::Normal, "Baue eine Seite"), &ex);
        assert_eq!(out.selected_model, CLOUDFLARE_GLM_4_7_FLASH.id, "{:?}", ex.calls());
        assert_eq!(ex.calls(), vec![DEEPSEEK.id, CLOUDFLARE_GLM_4_7_FLASH.id]);

        // Once DeepSeek is marked ModelRemoved, subsequent calls go straight to GLM:
        let ex2 = MockExecutor::new();
        let deps = RouterDeps { executor: &ex2, credentials: &AllSecrets };
        let out2 = route(&req(TaskClass::Coding, Tier::Normal, "Zweite Aufgabe"), &deps);
        assert_eq!(out2.selected_model, CLOUDFLARE_GLM_4_7_FLASH.id, "{:?}", ex2.calls());
        assert_eq!(ex2.calls(), vec![CLOUDFLARE_GLM_4_7_FLASH.id]);
    }

    #[test]
    fn coding_uses_gpt_oss_when_glm_is_down() {
        let _guard = lock();
        let ex = MockExecutor::new()
            .with(DEEPSEEK.id, Err(ExecErr::NotOffered))
            .with(CLOUDFLARE_GLM_4_7_FLASH.id, Err(ExecErr::Unavailable));
        let out = run(&req(TaskClass::Coding, Tier::Normal, "Baue eine Seite"), &ex);
        assert_eq!(out.selected_model, GROQ_GPT_OSS.id, "{:?}", ex.calls());
        assert_eq!(ex.calls(), vec![DEEPSEEK.id, CLOUDFLARE_GLM_4_7_FLASH.id, GROQ_GPT_OSS.id]);
    }

    #[test]
    fn work_fast_uses_gpt_oss_with_local_fallback() {
        let _guard = lock();
        let ex = MockExecutor::new();
        let out = run(&req(TaskClass::Work, Tier::Fast, "Kurze Frage"), &ex);
        assert_eq!(out.selected_model, GROQ_GPT_OSS.id);
        assert_eq!(ex.calls(), vec![GROQ_GPT_OSS.id]);
        assert_eq!(out.cloud_requests, 1, "exactly one model call");

        // When the clouds fail, the local floor is the 9B (the 4B is only the
        // runtime fallback when the 9B itself fails):
        let ex_fb = MockExecutor::new()
            .with(GROQ_GPT_OSS.id, Err(ExecErr::Unavailable))
            .with(CLOUDFLARE_GLM_4_7_FLASH.id, Err(ExecErr::Unavailable))
            .with(MINISTRAL.id, Err(ExecErr::Unavailable));
        let out_fb = run(&req(TaskClass::Work, Tier::Fast, "Kurze Frage"), &ex_fb);
        assert_eq!(out_fb.local_model, Some(LocalModel::Qwen9B));

        // The measured configuration: the reasoning channel stays shut.
        let cfg = NEMOTRON_3_ULTRA.cloud_config().unwrap();
        let extra = cfg.extra_body.expect("Nemotron must pin reasoning off");
        assert_eq!(extra["reasoning"]["effort"], "none");

        let groq = GROQ_GPT_OSS.cloud_config().unwrap();
        let groq_extra = groq.extra_body.expect("GPT-OSS productive routing must match its benchmark reasoning mode");
        assert_eq!(groq_extra["reasoning_effort"], "low");
        assert_eq!(groq_extra["include_reasoning"], false);
    }

    #[test]
    fn work_normal_uses_gpt_oss() {
        let _guard = lock();
        let ex = MockExecutor::new();
        let out = run(
            &req(TaskClass::Work, Tier::Normal, "Analysiere den Vertrag"),
            &ex,
        );
        assert_eq!(out.selected_model, GROQ_GPT_OSS.id);
        assert_eq!(ex.calls(), vec![GROQ_GPT_OSS.id]);
        assert_eq!(out.fallback_count, 0);
    }

    #[test]
    fn work_normal_with_gpt_oss_429_falls_back_to_glm_once() {
        let _guard = lock();
        let ex = MockExecutor::new().with(
            GROQ_GPT_OSS.id,
            Err(ExecErr::RateLimited {
                cooldown: None,
                reset_hint: None,
            }),
        );
        let out = run(
            &req(TaskClass::Work, Tier::Normal, "Analysiere den Vertrag"),
            &ex,
        );
        assert_eq!(out.selected_model, CLOUDFLARE_GLM_4_7_FLASH.id);
        // Exactly one fallback, and GPT-OSS is not called a second time.
        assert_eq!(ex.calls(), vec![GROQ_GPT_OSS.id, CLOUDFLARE_GLM_4_7_FLASH.id]);
        assert_eq!(out.fallback_count, 1);
        assert_eq!(provider_state(GROQ_GPT_OSS.id).as_str(), "rate_limited");
        assert_eq!(out.execution_lane, ExecutionLane::FreeCloud);
    }

    #[test]
    fn provider_state_is_read_only_compatibility_view() {
        let _guard = lock();
        reset_states();
        let mut observation =
            RuntimeObservation::outcome(RuntimeOutcome::RateLimited, OutcomeScope::Model);
        observation.cooldown_until =
            Some(runtime_registry::deadline_after(Duration::from_secs(60)));
        runtime_registry::observe(MINISTRAL.id, observation).unwrap();
        let before = runtime_registry::snapshot(MINISTRAL.id).unwrap();
        assert_eq!(provider_state(MINISTRAL.id).as_str(), "rate_limited");
        let after = runtime_registry::snapshot(MINISTRAL.id).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn work_normal_with_all_clouds_down_lands_on_qwen_9b() {
        let _guard = lock();
        let ex = MockExecutor::new()
            .with(GROQ_GPT_OSS.id, Err(ExecErr::Unavailable))
            .with(CLOUDFLARE_GLM_4_7_FLASH.id, Err(ExecErr::Unavailable))
            .with(MINISTRAL.id, Err(ExecErr::Unavailable))
            .with(DEEPSEEK.id, Err(ExecErr::Unavailable));
        let out = run(
            &req(TaskClass::Work, Tier::Normal, "Analysiere den Vertrag"),
            &ex,
        );
        assert_eq!(out.local_model, Some(LocalModel::Qwen9B));
        assert!(out.answer.is_none(), "local work is executed by the caller");
        assert_eq!(out.reason, "local_fallback_after_cloud_failure");
        assert_eq!(
            provider_state(MINISTRAL.id).as_str(),
            "temporarily_unavailable"
        );
    }

    #[test]
    fn shadow_rank_is_deterministic_and_does_not_change_productive_chain() {
        let _guard = lock();
        reset_states();
        let request = req(TaskClass::Work, Tier::Normal, "Analysiere den Vertrag");
        let first = shadow_rank_with_credentials(&request, &AllSecrets);
        let second = shadow_rank_with_credentials(&request, &AllSecrets);
        assert_eq!(first, second);
        assert_eq!(
            chain(TaskClass::Work, Tier::Normal, EngineMode::LocalAndCloud)
                .into_iter()
                .map(|entry| entry.id)
                .collect::<Vec<_>>(),
            vec![
                GROQ_GPT_OSS.id,
                CLOUDFLARE_GLM_4_7_FLASH.id,
                MINISTRAL.id,
                DEEPSEEK.id,
                QWEN_9B.id,
            ]
        );
        assert!(first
            .candidates
            .iter()
            .enumerate()
            .all(|(index, candidate)| candidate.final_rank == index + 1));
        assert_eq!(first.selected_candidate.as_deref(), Some(GROQ_GPT_OSS.id));
    }

    #[test]
    fn healthy_shadow_matrix_matches_all_six_static_role_primaries() {
        let _guard = lock();
        reset_states();
        for (class, tier) in [
            (TaskClass::Work, Tier::Fast),
            (TaskClass::Work, Tier::Normal),
            (TaskClass::Work, Tier::Deep),
            (TaskClass::Coding, Tier::Fast),
            (TaskClass::Coding, Tier::Normal),
            (TaskClass::Coding, Tier::Deep),
        ] {
            let request = req(class, tier, "metadata-only matrix");
            let report = shadow_rank_with_credentials(&request, &AllSecrets);
            let static_primary = chain(class, tier, EngineMode::LocalAndCloud)
                .first()
                .unwrap()
                .id;
            assert_eq!(
                report.selected_candidate.as_deref(),
                Some(static_primary),
                "healthy shadow result must preserve {class:?}/{tier:?}"
            );
            assert!(!report.candidates.iter().any(|candidate| {
                candidate.model_id == NEX_N2_5_PRO.id || candidate.model_id == NEMOTRON_3_ULTRA.id
            }));
        }
    }

    #[test]
    fn shadow_rank_uses_role_specific_quality_components() {
        let _guard = lock();
        reset_states();
        let work = shadow_rank_with_credentials(
            &req(
                TaskClass::Work,
                Tier::Normal,
                "Schreibe eine Zusammenfassung",
            ),
            &AllSecrets,
        );
        let coding = shadow_rank_with_credentials(
            &req(
                TaskClass::Coding,
                Tier::Normal,
                "Implementiere einen Parser",
            ),
            &AllSecrets,
        );
        assert_eq!(work.role, RoutingRole::WorkBalanced);
        assert_eq!(coding.role, RoutingRole::CodingNormal);
        let work_deepseek = work
            .candidates
            .iter()
            .find(|candidate| candidate.model_id == DEEPSEEK.id)
            .unwrap();
        let coding_deepseek = coding
            .candidates
            .iter()
            .find(|candidate| candidate.model_id == DEEPSEEK.id)
            .unwrap();
        assert_ne!(
            work_deepseek.components.quality, coding_deepseek.components.quality,
            "work and coding must use separate benchmark dimensions"
        );
    }

    #[test]
    fn shadow_rank_filters_privacy_and_paid_cost_violations() {
        let _guard = lock();
        reset_states();
        let mut sensitive = req(
            TaskClass::Work,
            Tier::Normal,
            "The password is confidential",
        );
        sensitive.mark_sensitive(true);
        let report = shadow_rank_with_credentials(&sensitive, &AllSecrets);
        assert_eq!(report.privacy_gate, PrivacyGate::LocalRequiredSensitive);
        assert!(report.candidates.iter().all(|candidate| {
            model_registry::model(&candidate.model_id)
                .is_some_and(|definition| definition.execution_lane == ExecutionLane::Local)
        }));
        assert!(report
            .rejected
            .iter()
            .any(|rejection| rejection.filter == ShadowFilter::Privacy));

        let mut paid = req(TaskClass::Work, Tier::Normal, "Paid policy audit");
        paid.authorize_paid_for_test();
        let paid_report = shadow_rank_with_credentials(&paid, &AllSecrets);
        // Paid Cloud ist entfernt: auch eine "freigegebene" Paid-Anfrage
        // bekommt nie ein bezahltes Modell.
        assert!(paid_report.candidates.iter().all(|candidate| {
            model_registry::model(&candidate.model_id)
                .is_some_and(|definition| definition.cost_safety != CostSafety::MeteredPaid)
        }));
    }

    #[test]
    fn shadow_rank_rejects_benchmark_fingerprint_mismatch() {
        let _guard = lock();
        reset_states();
        let request = req(TaskClass::Coding, Tier::Deep, "Fix the failing test");
        let report = shadow_rank_with_expectation(
            &request,
            ShadowBenchmarkExpectation {
                fingerprint: Some("not-the-canonical-fingerprint"),
                ..ShadowBenchmarkExpectation::default()
            },
        );
        assert!(report.candidates.is_empty());
        assert!(report.rejected.iter().any(|rejection| {
            rejection.filter == ShadowFilter::Benchmark
                && rejection.reason == "benchmark_fingerprint_mismatch"
        }));
    }

    #[test]
    fn shadow_rank_rejects_wrong_benchmark_suite() {
        let _guard = lock();
        reset_states();
        let request = req(TaskClass::Work, Tier::Normal, "Fasse den Text zusammen");
        let report = shadow_rank_internal(
            &request,
            ShadowBenchmarkExpectation {
                suite_id: Some(crate::cloud_engine::CODING_SUITE_ID),
                ..ShadowBenchmarkExpectation::default()
            },
            &AllSecrets,
        );
        assert!(report.candidates.is_empty());
        assert!(report.rejected.iter().all(|rejection| {
            rejection.filter == ShadowFilter::Benchmark
                && rejection.reason == "benchmark_suite_task_class_mismatch"
        }));
    }

    #[test]
    fn shadow_hard_filter_beats_score_and_excludes_runtime_failures() {
        let _guard = lock();
        let request = req(TaskClass::Work, Tier::Normal, "Analysiere den Vertrag");

        reset_states();
        set_state(
            GROQ_GPT_OSS.id,
            ProviderState::RateLimited {
                until: Instant::now() + Duration::from_secs(60),
            },
            Some("rate_limited"),
        );
        let cooldown = shadow_rank_with_credentials(&request, &AllSecrets);
        assert!(!cooldown
            .candidates
            .iter()
            .any(|candidate| candidate.model_id == GROQ_GPT_OSS.id));
        assert!(cooldown.rejected.iter().any(|rejection| {
            rejection.model_id == GROQ_GPT_OSS.id && rejection.filter == ShadowFilter::Runtime
        }));

        reset_states();
        set_state(
            MINISTRAL.id,
            ProviderState::QuotaExhausted {
                until: Instant::now() + Duration::from_secs(60),
                reset_hint: None,
            },
            Some("quota_exhausted"),
        );
        let quota = shadow_rank_with_credentials(&request, &AllSecrets);
        assert!(quota.rejected.iter().any(|rejection| {
            rejection.model_id == MINISTRAL.id && rejection.filter == ShadowFilter::Quota
        }));

        reset_states();
        runtime_registry::set_auth_state(MINISTRAL.id, AuthState::Failed).unwrap();
        let auth = shadow_rank_with_credentials(&request, &AllSecrets);
        assert!(auth.rejected.iter().any(|rejection| {
            rejection.model_id == MINISTRAL.id && rejection.filter == ShadowFilter::Auth
        }));

        reset_states();
        for _ in 0..3 {
            let mut observation = RuntimeObservation::outcome(
                RuntimeOutcome::ServiceUnavailable,
                OutcomeScope::Model,
            );
            observation.cooldown_until =
                Some(runtime_registry::deadline_after(Duration::from_secs(60)));
            runtime_registry::observe(MINISTRAL.id, observation).unwrap();
        }
        let open = shadow_rank_with_credentials(&request, &AllSecrets);
        assert!(open.rejected.iter().any(|rejection| {
            rejection.model_id == MINISTRAL.id
                && rejection.filter == ShadowFilter::Runtime
                && rejection.reason == "breaker_open"
        }));
    }

    #[test]
    fn shadow_credentials_and_capabilities_are_hard_filters() {
        let _guard = lock();
        reset_states();
        let request = req(TaskClass::Work, Tier::Normal, "Analysiere den Vertrag");
        let no_secrets = CountingSecrets {
            probes: RefCell::new(0),
            present: false,
        };
        let credentials = shadow_rank_with_credentials(&request, &no_secrets);
        assert!(credentials.rejected.iter().any(|rejection| {
            rejection.filter == ShadowFilter::Credentials
                && rejection.reason == "credentials_missing"
        }));

        let mut vision = req(TaskClass::Work, Tier::Normal, "Prüfe das Bild");
        vision.requires_vision = true;
        let capabilities = shadow_rank_with_credentials(&vision, &AllSecrets);
        assert!(capabilities.candidates.is_empty());
        assert!(capabilities
            .rejected
            .iter()
            .all(|rejection| rejection.filter == ShadowFilter::Capability));
    }

    #[test]
    fn validated_v2_challenger_does_not_displace_role_champion() {
        let _guard = lock();
        reset_states();
        let report = shadow_rank_with_credentials(
            &req(TaskClass::Work, Tier::Normal, "Analysiere den Vertrag"),
            &AllSecrets,
        );
        assert_eq!(report.selected_candidate.as_deref(), Some(GROQ_GPT_OSS.id));
        let challenger = report
            .candidates
            .iter()
            .find(|candidate| candidate.model_id == DEEPSEEK.id)
            .unwrap();
        assert_eq!(
            challenger.benchmark_confidence,
            ShadowBenchmarkConfidence::CanonicalV2
        );
        assert!(!challenger.is_role_champion);
        assert!(challenger.final_rank > 1);
    }

    #[test]
    fn fast_normal_and_complex_use_distinct_weights() {
        let fast = ShadowWeights::for_tier(Tier::Fast);
        let normal = ShadowWeights::for_tier(Tier::Normal);
        let complex = ShadowWeights::for_tier(Tier::Deep);
        assert_ne!(fast, normal);
        assert_ne!(normal, complex);
        assert!(fast.p95_latency > normal.p95_latency);
        assert!(normal.reliability >= complex.reliability);
        assert!(complex.quality > normal.quality);
        assert!(complex.capability > normal.capability);
        assert!(complex.quality_stability > normal.quality_stability);
    }

    #[test]
    fn shadow_mode_never_calls_an_executor() {
        let _guard = lock();
        reset_states();
        let executor = MockExecutor::new();
        let report = shadow_rank_with_credentials(
            &req(
                TaskClass::Coding,
                Tier::Normal,
                "Implementiere einen Parser",
            ),
            &AllSecrets,
        );
        assert!(!report.candidates.is_empty());
        assert!(
            executor.calls().is_empty(),
            "shadow ranking has no execution seam"
        );
    }

    #[test]
    fn shadow_audit_compares_static_and_dynamic_without_prompt_content() {
        let _guard = lock();
        let secret_prompt = "PROMPT_CONTENT_MUST_NOT_ENTER_SHADOW_AUDIT";
        let executor = MockExecutor::new();
        let out = run(
            &req(TaskClass::Work, Tier::Normal, secret_prompt),
            &executor,
        );
        assert_eq!(out.audit.shadow.static_selected_candidate, GROQ_GPT_OSS.id);
        assert_eq!(
            out.audit.shadow.dynamic_selected_candidate.as_deref(),
            Some(GROQ_GPT_OSS.id)
        );
        assert!(!out.audit.shadow.selection_differs);
        assert_eq!(out.audit.shadow.fallback_count, 0);
        let serialized = serde_json::to_string(&out.audit.shadow).unwrap();
        assert!(!serialized.contains(secret_prompt));
        assert_eq!(executor.calls(), vec![GROQ_GPT_OSS.id]);
    }

    #[test]
    fn phase_7_5_runtime_state_scenario_oracles() {
        let _guard = lock();
        let request = req(TaskClass::Work, Tier::Normal, "synthetic metadata only");
        let healthy = ShadowScenarioOracle {
            eligible: &[GROQ_GPT_OSS.id, CLOUDFLARE_GLM_4_7_FLASH.id, MINISTRAL.id, DEEPSEEK.id, QWEN_9B.id],
            filtered: &[],
            ranking: &[GROQ_GPT_OSS.id, CLOUDFLARE_GLM_4_7_FLASH.id, MINISTRAL.id, DEEPSEEK.id, QWEN_9B.id],
            winner: Some(GROQ_GPT_OSS.id),
            fallback: &[CLOUDFLARE_GLM_4_7_FLASH.id, MINISTRAL.id, DEEPSEEK.id, QWEN_9B.id],
        };

        reset_states();
        assert_shadow_oracle(
            &shadow_rank_with_credentials(&request, &AllSecrets),
            healthy,
        );

        reset_states();
        synthetic_observation(MINISTRAL.id, RuntimeOutcome::EmptyResponse, None);
        let degraded = shadow_rank_with_credentials(&request, &AllSecrets);
        assert_shadow_oracle(
            &degraded,
            ShadowScenarioOracle {
                eligible: &[GROQ_GPT_OSS.id, CLOUDFLARE_GLM_4_7_FLASH.id, DEEPSEEK.id, MINISTRAL.id, QWEN_9B.id],
                filtered: &[],
                ranking: &[GROQ_GPT_OSS.id, CLOUDFLARE_GLM_4_7_FLASH.id, DEEPSEEK.id, MINISTRAL.id, QWEN_9B.id],
                winner: Some(GROQ_GPT_OSS.id),
                fallback: &[CLOUDFLARE_GLM_4_7_FLASH.id, DEEPSEEK.id, MINISTRAL.id, QWEN_9B.id],
            },
        );
        assert_eq!(
            degraded.candidates.iter().find(|candidate| candidate.model_id == MINISTRAL.id).unwrap().components.health, 7_000,
            "DEGRADED stays eligible with an explicit health penalty"
        );

        reset_states();
        for _ in 0..2 {
            runtime_registry::note_request(MINISTRAL.id).unwrap();
            let mut observation = RuntimeObservation::outcome(
                RuntimeOutcome::ServiceUnavailable,
                OutcomeScope::Model,
            );
            observation.cooldown_until =
                Some(runtime_registry::deadline_after(Duration::from_secs(60)));
            runtime_registry::observe(MINISTRAL.id, observation).unwrap();
        }
        let open = shadow_rank_with_credentials(&request, &AllSecrets);
        assert_shadow_oracle(
            &open,
            ShadowScenarioOracle {
                eligible: &[GROQ_GPT_OSS.id, CLOUDFLARE_GLM_4_7_FLASH.id, DEEPSEEK.id, QWEN_9B.id],
                filtered: &[(MINISTRAL.id, ShadowFilter::Runtime, "breaker_open")],
                ranking: &[GROQ_GPT_OSS.id, CLOUDFLARE_GLM_4_7_FLASH.id, DEEPSEEK.id, QWEN_9B.id],
                winner: Some(GROQ_GPT_OSS.id),
                fallback: &[CLOUDFLARE_GLM_4_7_FLASH.id, DEEPSEEK.id, QWEN_9B.id],
            },
        );
        let divergence = ShadowValidationDivergence::from_oracle(&open, Some(GROQ_GPT_OSS.id));
        assert!(divergence.same);
        assert!(divergence.oracle_correct);

        reset_states();
        for _ in 0..2 {
            runtime_registry::note_request(MINISTRAL.id).unwrap();
            let mut observation = RuntimeObservation::outcome(
                RuntimeOutcome::ServiceUnavailable,
                OutcomeScope::Model,
            );
            observation.cooldown_until = Some(runtime_registry::deadline_after(Duration::ZERO));
            runtime_registry::observe(MINISTRAL.id, observation).unwrap();
        }
        assert_shadow_oracle(
            &shadow_rank_with_credentials(&request, &AllSecrets),
            ShadowScenarioOracle {
                eligible: &[GROQ_GPT_OSS.id, CLOUDFLARE_GLM_4_7_FLASH.id, DEEPSEEK.id, QWEN_9B.id],
                filtered: &[(MINISTRAL.id, ShadowFilter::Runtime, "half_open_probe_only")],
                ranking: &[GROQ_GPT_OSS.id, CLOUDFLARE_GLM_4_7_FLASH.id, DEEPSEEK.id, QWEN_9B.id],
                winner: Some(GROQ_GPT_OSS.id),
                fallback: &[CLOUDFLARE_GLM_4_7_FLASH.id, DEEPSEEK.id, QWEN_9B.id],
            },
        );
    }

    #[test]
    fn phase_7_5_rate_quota_timeout_and_stale_quota_oracles() {
        let _guard = lock();
        let request = req(TaskClass::Work, Tier::Normal, "synthetic metadata only");

        reset_states();
        runtime_registry::note_request(MINISTRAL.id).unwrap();
        let mut limited =
            RuntimeObservation::outcome(RuntimeOutcome::RateLimited, OutcomeScope::Model);
        limited.cooldown_until = Some(runtime_registry::deadline_after(Duration::from_secs(60)));
        runtime_registry::observe(MINISTRAL.id, limited).unwrap();
        assert_shadow_oracle(
            &shadow_rank_with_credentials(&request, &AllSecrets),
            ShadowScenarioOracle {
                eligible: &[GROQ_GPT_OSS.id, CLOUDFLARE_GLM_4_7_FLASH.id, DEEPSEEK.id, QWEN_9B.id],
                filtered: &[(
                    MINISTRAL.id,
                    ShadowFilter::Runtime,
                    "runtime_not_selectable",
                )],
                ranking: &[GROQ_GPT_OSS.id, CLOUDFLARE_GLM_4_7_FLASH.id, DEEPSEEK.id, QWEN_9B.id],
                winner: Some(GROQ_GPT_OSS.id),
                fallback: &[CLOUDFLARE_GLM_4_7_FLASH.id, DEEPSEEK.id, QWEN_9B.id],
            },
        );

        reset_states();
        runtime_registry::note_request(MINISTRAL.id).unwrap();
        let mut exhausted =
            RuntimeObservation::outcome(RuntimeOutcome::QuotaExhausted, OutcomeScope::Model);
        exhausted.cooldown_until = Some(runtime_registry::deadline_after(Duration::from_secs(60)));
        runtime_registry::observe(MINISTRAL.id, exhausted).unwrap();
        assert_shadow_oracle(
            &shadow_rank_with_credentials(&request, &AllSecrets),
            ShadowScenarioOracle {
                eligible: &[GROQ_GPT_OSS.id, CLOUDFLARE_GLM_4_7_FLASH.id, DEEPSEEK.id, QWEN_9B.id],
                filtered: &[(MINISTRAL.id, ShadowFilter::Quota, "quota_exhausted")],
                ranking: &[GROQ_GPT_OSS.id, CLOUDFLARE_GLM_4_7_FLASH.id, DEEPSEEK.id, QWEN_9B.id],
                winner: Some(GROQ_GPT_OSS.id),
                fallback: &[CLOUDFLARE_GLM_4_7_FLASH.id, DEEPSEEK.id, QWEN_9B.id],
            },
        );

        reset_states();
        synthetic_observation(MINISTRAL.id, RuntimeOutcome::Timeout, None);
        assert_shadow_oracle(
            &shadow_rank_with_credentials(&request, &AllSecrets),
            ShadowScenarioOracle {
                eligible: &[GROQ_GPT_OSS.id, CLOUDFLARE_GLM_4_7_FLASH.id, DEEPSEEK.id, QWEN_9B.id],
                filtered: &[(
                    MINISTRAL.id,
                    ShadowFilter::Runtime,
                    "runtime_not_selectable",
                )],
                ranking: &[GROQ_GPT_OSS.id, CLOUDFLARE_GLM_4_7_FLASH.id, DEEPSEEK.id, QWEN_9B.id],
                winner: Some(GROQ_GPT_OSS.id),
                fallback: &[CLOUDFLARE_GLM_4_7_FLASH.id, DEEPSEEK.id, QWEN_9B.id],
            },
        );

        reset_states();
        runtime_registry::note_request(MINISTRAL.id).unwrap();
        let mut stale = RuntimeObservation::outcome(RuntimeOutcome::Success, OutcomeScope::Model);
        stale.observed_at = Some(1);
        stale.quota_remaining = Some("synthetic".to_string());
        stale.quota_confidence = MetadataConfidence::Header;
        runtime_registry::observe(MINISTRAL.id, stale).unwrap();
        let stale_report = shadow_rank_with_credentials(&request, &AllSecrets);
        assert_shadow_oracle(
            &stale_report,
            ShadowScenarioOracle {
                eligible: &[MINISTRAL.id, GROQ_GPT_OSS.id, CLOUDFLARE_GLM_4_7_FLASH.id, DEEPSEEK.id, QWEN_9B.id],
                filtered: &[],
                ranking: &[MINISTRAL.id, GROQ_GPT_OSS.id, CLOUDFLARE_GLM_4_7_FLASH.id, DEEPSEEK.id, QWEN_9B.id],
                winner: Some(MINISTRAL.id),
                fallback: &[GROQ_GPT_OSS.id, CLOUDFLARE_GLM_4_7_FLASH.id, DEEPSEEK.id, QWEN_9B.id],
            },
        );
        assert_eq!(stale_report.candidates.iter().find(|candidate| candidate.model_id == MINISTRAL.id).unwrap().components.quota_freshness, 2_500);
    }

    #[test]
    fn phase_7_5_failure_rates_and_p95_oracles() {
        let _guard = lock();
        let request = req(TaskClass::Work, Tier::Normal, "synthetic metadata only");

        reset_states();
        for _ in 0..3 {
            synthetic_observation(MINISTRAL.id, RuntimeOutcome::EmptyResponse, None);
        }
        for _ in 0..2 {
            synthetic_observation(DEEPSEEK.id, RuntimeOutcome::QualityFailure, None);
        }
        let failures = shadow_rank_with_credentials(&request, &AllSecrets);
        assert_shadow_oracle(
            &failures,
            ShadowScenarioOracle {
                eligible: &[GROQ_GPT_OSS.id, CLOUDFLARE_GLM_4_7_FLASH.id, MINISTRAL.id, DEEPSEEK.id, QWEN_9B.id],
                filtered: &[],
                ranking: &[GROQ_GPT_OSS.id, CLOUDFLARE_GLM_4_7_FLASH.id, MINISTRAL.id, DEEPSEEK.id, QWEN_9B.id],
                winner: Some(GROQ_GPT_OSS.id),
                fallback: &[CLOUDFLARE_GLM_4_7_FLASH.id, MINISTRAL.id, DEEPSEEK.id, QWEN_9B.id],
            },
        );
        assert_eq!(failures.candidates.iter().find(|candidate| candidate.model_id == MINISTRAL.id).unwrap().components.quality_stability, 0);
        assert_eq!(failures.candidates.iter().find(|candidate| candidate.model_id == DEEPSEEK.id).unwrap().components.quality_stability, 0);

        reset_states();
        for latency in [4_000, 4_500, 5_000] {
            synthetic_observation(MINISTRAL.id, RuntimeOutcome::Success, Some(latency));
        }
        for latency in [300, 400, 500] {
            synthetic_observation(DEEPSEEK.id, RuntimeOutcome::Success, Some(latency));
        }
        let latency = shadow_rank_with_credentials(&request, &AllSecrets);
        assert_shadow_oracle(
            &latency,
            ShadowScenarioOracle {
                eligible: &[DEEPSEEK.id, MINISTRAL.id, GROQ_GPT_OSS.id, CLOUDFLARE_GLM_4_7_FLASH.id, QWEN_9B.id],
                filtered: &[],
                ranking: &[DEEPSEEK.id, MINISTRAL.id, GROQ_GPT_OSS.id, CLOUDFLARE_GLM_4_7_FLASH.id, QWEN_9B.id],
                winner: Some(DEEPSEEK.id),
                fallback: &[MINISTRAL.id, GROQ_GPT_OSS.id, CLOUDFLARE_GLM_4_7_FLASH.id, QWEN_9B.id],
            },
        );
        assert_eq!(latency.candidates.iter().find(|candidate| candidate.model_id == MINISTRAL.id).unwrap().p95_latency_ms, Some(4_500));
        assert_eq!(latency.candidates.iter().find(|candidate| candidate.model_id == DEEPSEEK.id).unwrap().p95_latency_ms, Some(400));
        assert!(
            latency.candidates.iter().find(|candidate| candidate.model_id == DEEPSEEK.id).unwrap().components.p95_latency
                > latency.candidates.iter().find(|candidate| candidate.model_id == MINISTRAL.id).unwrap().components.p95_latency
        );
    }

    #[test]
    fn phase_7_5_tier_weighting_oracles() {
        let enough_fast = ShadowComponents {
            quality: 8_200,
            availability: 10_000,
            completion: 10_000,
            reliability: 9_500,
            quality_stability: 9_500,
            health: 10_000,
            p95_latency: 9_500,
            quota_freshness: 8_000,
            capability: 8_000,
            cost: 10_000,
        };
        let quality_but_slow = ShadowComponents {
            quality: 9_000,
            reliability: 8_000,
            p95_latency: 8_000,
            ..enough_fast
        };
        assert!(
            shadow_score(enough_fast, ShadowWeights::for_tier(Tier::Fast))
                > shadow_score(quality_but_slow, ShadowWeights::for_tier(Tier::Fast))
        );

        let normal_quality = ShadowComponents {
            quality: 9_000,
            reliability: 9_000,
            p95_latency: 9_000,
            ..enough_fast
        };
        assert!(
            shadow_score(normal_quality, ShadowWeights::for_tier(Tier::Normal))
                > shadow_score(enough_fast, ShadowWeights::for_tier(Tier::Normal))
        );

        let deep_quality = ShadowComponents {
            quality: 9_300,
            quality_stability: 9_900,
            capability: 10_000,
            p95_latency: 5_000,
            ..enough_fast
        };
        assert!(
            shadow_score(deep_quality, ShadowWeights::for_tier(Tier::Deep))
                > shadow_score(enough_fast, ShadowWeights::for_tier(Tier::Deep))
        );
    }

    #[test]
    fn phase_7_5_benchmark_identity_and_confidence_oracles() {
        let _guard = lock();
        reset_states();
        let request = req(TaskClass::Work, Tier::Normal, "synthetic metadata only");
        for bad_fingerprint in ["wrong", ""] {
            let report = shadow_rank_internal(
                &request,
                ShadowBenchmarkExpectation {
                    fingerprint: Some(bad_fingerprint),
                    ..ShadowBenchmarkExpectation::default()
                },
                &AllSecrets,
            );
            assert!(report.candidates.is_empty());
            assert!(report.rejected.iter().all(|rejection| {
                rejection.filter == ShadowFilter::Benchmark
                    && rejection.reason == "benchmark_fingerprint_mismatch"
            }));
        }

        let mut candidates = vec![
            synthetic_candidate(
                "legacy_challenger",
                ShadowBenchmarkConfidence::Legacy,
                9_900,
                false,
            ),
            synthetic_candidate(
                "legacy_champion",
                ShadowBenchmarkConfidence::Legacy,
                8_000,
                true,
            ),
            synthetic_candidate(
                "canonical_challenger",
                ShadowBenchmarkConfidence::CanonicalV2,
                7_000,
                false,
            ),
        ];
        candidates.sort_by(|a, b| shadow_candidate_order(a, b, false, false));
        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.model_id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "canonical_challenger",
                "legacy_champion",
                "legacy_challenger"
            ]
        );
    }

    #[test]
    fn phase_7_5_security_lane_local_and_sensitive_oracles() {
        let _guard = lock();
        reset_states();

        let mut paid = model_registry::DEEPSEEK;
        paid.execution_lane = ExecutionLane::PaidCloud;
        paid.cost_safety = CostSafety::MeteredPaid;
        assert!(!shadow_lane_matches(&paid, ExecutionLane::FreeCloud));
        assert!(!ExecutionLane::FreeCloud.allows(paid.cost_safety));
        assert!(!shadow_lane_matches(
            &model_registry::DEEPSEEK,
            ExecutionLane::Local
        ));

        let mut local = req(TaskClass::Work, Tier::Normal, "local only");
        local.engine_mode = EngineMode::OnlyLocal;
        assert_shadow_oracle(
            &shadow_rank_with_credentials(&local, &AllSecrets),
            ShadowScenarioOracle {
                eligible: &[QWEN_9B.id, QWEN_4B.id],
                filtered: &[],
                ranking: &[QWEN_9B.id, QWEN_4B.id],
                winner: Some(QWEN_9B.id),
                fallback: &[QWEN_4B.id],
            },
        );

        let mut sensitive = req(TaskClass::Work, Tier::Normal, "confidential");
        sensitive.mark_sensitive(true);
        assert_shadow_oracle(
            &shadow_rank_with_credentials(&sensitive, &AllSecrets),
            ShadowScenarioOracle {
                eligible: &[QWEN_9B.id],
                filtered: &[
                    (
                        GROQ_GPT_OSS.id,
                        ShadowFilter::Privacy,
                        "privacy_gate_requires_local",
                    ),
                    (
                        CLOUDFLARE_GLM_4_7_FLASH.id,
                        ShadowFilter::Privacy,
                        "privacy_gate_requires_local",
                    ),
                    (
                        MINISTRAL.id,
                        ShadowFilter::Privacy,
                        "privacy_gate_requires_local",
                    ),
                    (
                        DEEPSEEK.id,
                        ShadowFilter::Privacy,
                        "privacy_gate_requires_local",
                    ),
                ],
                ranking: &[QWEN_9B.id],
                winner: Some(QWEN_9B.id),
                fallback: &[],
            },
        );

        reset_states();
        runtime_registry::set_auth_state(GROQ_GPT_OSS.id, AuthState::Failed).unwrap();
        runtime_registry::set_auth_state(CLOUDFLARE_GLM_4_7_FLASH.id, AuthState::Failed)
            .unwrap();
        runtime_registry::set_auth_state(MINISTRAL.id, AuthState::Failed).unwrap();
        runtime_registry::set_auth_state(DEEPSEEK.id, AuthState::Failed).unwrap();
        assert_shadow_oracle(
            &shadow_rank_with_credentials(
                &req(TaskClass::Work, Tier::Normal, "auth failure"),
                &AllSecrets,
            ),
            ShadowScenarioOracle {
                eligible: &[QWEN_9B.id],
                filtered: &[
                    (GROQ_GPT_OSS.id, ShadowFilter::Auth, "auth_failed"),
                    (
                        CLOUDFLARE_GLM_4_7_FLASH.id,
                        ShadowFilter::Auth,
                        "auth_failed",
                    ),
                    (MINISTRAL.id, ShadowFilter::Auth, "auth_failed"),
                    (DEEPSEEK.id, ShadowFilter::Auth, "auth_failed"),
                ],
                ranking: &[QWEN_9B.id],
                winner: Some(QWEN_9B.id),
                fallback: &[],
            },
        );
    }

    #[test]
    fn phase_7_5_activation_gate_oracle() {
        let _guard = lock();
        reset_states();
        let executor = MockExecutor::new();
        let request = req(TaskClass::Coding, Tier::Normal, "metadata only");
        let first = shadow_rank_with_credentials(&request, &AllSecrets);
        let second = shadow_rank_with_credentials(&request, &AllSecrets);
        assert_eq!(first, second);
        assert!(executor.calls().is_empty());
        assert!(first
            .candidates
            .last()
            .is_some_and(|candidate| candidate.is_local_floor));
        assert!(!first.candidates.iter().any(|candidate| {
            candidate.benchmark_confidence == ShadowBenchmarkConfidence::Unverified
        }));

        let gate = ShadowActivationGate {
            security_lane_privacy_passed: true,
            benchmark_identity_passed: true,
            scenario_oracles_passed: true,
            deterministic_ranking_passed: true,
            local_floor_guaranteed: true,
            no_unverified_promotion: true,
            zero_external_calls: true,
        };
        assert!(gate.ready_for_controlled_activation());
        assert!(!ShadowActivationGate {
            zero_external_calls: false,
            ..gate
        }
        .ready_for_controlled_activation());
    }

    #[test]
    fn phase_8_dynamic_default_executes_frozen_ranker_order() {
        let _guard = lock();
        reset_states();
        assert_eq!(RoutingMode::from_config(None), RoutingMode::Dynamic);
        assert_eq!(
            RoutingMode::from_config(Some("STATIC")),
            RoutingMode::Static
        );
        let mut request = req(TaskClass::Work, Tier::Normal, "phase 8 metadata audit");
        request.set_routing_mode(RoutingMode::Dynamic);
        let expected = shadow_rank_with_credentials(&request, &AllSecrets);
        let executor = MockExecutor::new();
        let deps = RouterDeps {
            executor: &executor,
            credentials: &AllSecrets,
        };
        let out = route(&request, &deps);
        let expected_order = expected
            .candidates
            .iter()
            .map(|candidate| candidate.model_id.clone())
            .collect::<Vec<_>>();

        assert_eq!(out.audit.routing_mode, "DYNAMIC");
        assert_eq!(out.audit.ranked_candidates, expected_order);
        assert_eq!(
            out.selected_model,
            expected.selected_candidate.as_deref().unwrap()
        );
        assert_eq!(executor.calls(), vec![out.selected_model.clone()]);
        assert_eq!(out.audit.runtime_outcome, RuntimeOutcome::Success.as_str());
        assert!(!out.audit.local_floor_used);
        assert_eq!(out.audit.rank_factors, expected.candidates);
    }

    #[test]
    fn resource_pressure_prefers_free_cloud_only_when_policy_allows() {
        let _guard = lock();
        reset_states();
        let mut normal = req(TaskClass::Work, Tier::Normal, "resource-aware routing");
        normal.local_model_resident = true;
        normal.resource_pressure = ResourcePressure::Normal;
        let normal_report = shadow_rank_with_credentials(&normal, &AllSecrets);
        assert_eq!(normal_report.selected_candidate.as_deref(), Some(QWEN_9B.id));

        let mut pressured = normal;
        pressured.resource_pressure = ResourcePressure::High;
        let pressured_report = shadow_rank_with_credentials(&pressured, &AllSecrets);
        assert_ne!(pressured_report.selected_candidate.as_deref(), Some(QWEN_9B.id));
        let selected = model_registry::model(
            pressured_report.selected_candidate.as_deref().unwrap(),
        )
        .unwrap();
        assert_eq!(selected.execution_lane, ExecutionLane::FreeCloud);
        assert_eq!(selected.cost_safety, CostSafety::VerifiedFreeHardStop);

        let mut only_local = pressured;
        only_local.engine_mode = EngineMode::OnlyLocal;
        let executor = MockExecutor::new();
        let out = run(&only_local, &executor);
        assert!(out.is_local());
        assert!(executor.calls().is_empty());
        assert_eq!(out.execution_lane, ExecutionLane::Local);
    }

    #[test]
    fn research_synthesis_executes_the_dynamic_ranker_winner() {
        let _guard = lock();
        reset_states();
        let request = req(
            TaskClass::Work,
            Tier::Deep,
            "Synthetisiere den verifizierten Evidence Pack",
        );
        let expected = shadow_rank_with_credentials(&request, &AllSecrets)
            .selected_candidate
            .expect("research must have an eligible responder");
        let executor = MockExecutor::new();
        let deps = RouterDeps {
            executor: &executor,
            credentials: &AllSecrets,
        };
        let outcome = route(&request, &deps);

        assert_eq!(outcome.selected_model, expected);
        assert_ne!(outcome.selected_model, QWEN_4B.id);
        assert_eq!(executor.calls(), vec![expected]);
    }

    #[test]
    fn unavailable_research_cloud_falls_back_to_stronger_local_work_model() {
        let _guard = lock();
        reset_states();
        let executor = MockExecutor::new();
        let credentials = CountingSecrets {
            probes: RefCell::new(0),
            present: false,
        };
        let request = req(
            TaskClass::Work,
            Tier::Deep,
            "Nichttriviale Recherche mit mehreren Quellen",
        );
        let deps = RouterDeps {
            executor: &executor,
            credentials: &credentials,
        };
        let outcome = route(&request, &deps);

        assert_eq!(outcome.local_model, Some(LocalModel::Qwen9B));
        assert_ne!(outcome.selected_model, QWEN_4B.id);
        assert!(executor.calls().is_empty());
        assert_eq!(outcome.execution_lane, ExecutionLane::FreeCloud);
    }

    #[test]
    fn phase_8_static_is_exact_specialist_and_chain_rollback() {
        let _guard = lock();
        let executor = MockExecutor::new().with(
            GEMINI.id,
            Err(ExecErr::RateLimited {
                cooldown: None,
                reset_hint: None,
            }),
        );
        let mut request = req(TaskClass::Work, Tier::Deep, "static rollback");
        request.set_routing_mode(RoutingMode::Static);
        request.allow_specialist = true;
        let out = run(&request, &executor);

        assert_eq!(out.audit.routing_mode, "STATIC");
        assert_eq!(executor.calls(), vec![GEMINI.id, GROQ_GPT_OSS.id]);
        assert_eq!(out.selected_model, GROQ_GPT_OSS.id);
        assert_eq!(
            out.audit.ranked_candidates,
            vec![
                GEMINI.id,
                GROQ_GPT_OSS.id,
                CLOUDFLARE_GLM_4_7_FLASH.id,
                MINISTRAL.id,
                DEEPSEEK.id,
                QWEN_9B.id,
            ]
        );
    }

    #[test]
    fn phase_8_dynamic_failure_advances_same_lane_and_changes_next_route() {
        let _guard = lock();
        reset_states();
        let first_executor = MockExecutor::new().with(
            GROQ_GPT_OSS.id,
            Err(ExecErr::RateLimited {
                cooldown: None,
                reset_hint: None,
            }),
        );
        let request = req(TaskClass::Work, Tier::Normal, "same lane fallback");
        let deps = RouterDeps {
            executor: &first_executor,
            credentials: &AllSecrets,
        };
        let first = route(&request, &deps);
        assert_eq!(first.execution_lane, ExecutionLane::FreeCloud);
        assert_eq!(first_executor.calls(), vec![GROQ_GPT_OSS.id, CLOUDFLARE_GLM_4_7_FLASH.id]);
        assert_eq!(first.selected_model, CLOUDFLARE_GLM_4_7_FLASH.id);
        assert_eq!(first.cloud_requests, 2);
        assert!(first.audit.filter_fallback_reasons.iter().any(|reason| {
            reason == &format!("{}:{}", GROQ_GPT_OSS.id, RuntimeOutcome::RateLimited.as_str())
        }));

        let second_executor = MockExecutor::new();
        let second_deps = RouterDeps {
            executor: &second_executor,
            credentials: &AllSecrets,
        };
        let second = route(&request, &second_deps);
        assert_eq!(second_executor.calls(), vec![CLOUDFLARE_GLM_4_7_FLASH.id]);
        assert_eq!(second.selected_model, CLOUDFLARE_GLM_4_7_FLASH.id);
        assert!(!second
            .audit
            .ranked_candidates
            .contains(&GROQ_GPT_OSS.id.to_string()));
    }

    #[test]
    fn phase_8_dynamic_local_floor_and_privacy_lane_invariants_hold() {
        let _guard = lock();
        let failing = MockExecutor::new()
            .with(GROQ_GPT_OSS.id, Err(ExecErr::Unavailable))
            .with(CLOUDFLARE_GLM_4_7_FLASH.id, Err(ExecErr::Unavailable))
            .with(MINISTRAL.id, Err(ExecErr::Unavailable))
            .with(DEEPSEEK.id, Err(ExecErr::Timeout));
        let out = run(
            &req(TaskClass::Work, Tier::Normal, "cloud failures"),
            &failing,
        );
        assert_eq!(
            failing.calls(),
            vec![
                GROQ_GPT_OSS.id,
                CLOUDFLARE_GLM_4_7_FLASH.id,
                MINISTRAL.id,
            ]
        );
        assert_eq!(out.local_model, Some(LocalModel::Qwen9B));
        assert!(out.audit.local_floor_used);
        assert_eq!(out.execution_lane, ExecutionLane::FreeCloud);

        let sensitive_executor = MockExecutor::new();
        let mut sensitive = req(TaskClass::Coding, Tier::Deep, "private source");
        sensitive.mark_sensitive(true);
        let sensitive_out = run(&sensitive, &sensitive_executor);
        assert_eq!(sensitive_out.execution_lane, ExecutionLane::Local);
        assert_eq!(sensitive_out.local_model, Some(LocalModel::JackOd9BNative));
        assert!(sensitive_executor.calls().is_empty());

        let local_executor = MockExecutor::new();
        let mut local = req(TaskClass::Work, Tier::Normal, "local lane");
        local.request_lane(ExecutionLane::Local);
        let local_out = run(&local, &local_executor);
        assert_eq!(local_out.execution_lane, ExecutionLane::Local);
        assert!(local_out.is_local());
        assert!(local_executor.calls().is_empty());
    }

    #[test]
    fn phase_8_mode_switch_is_deterministic_and_tests_use_no_live_executor() {
        let _guard = lock();
        let mut static_request = req(TaskClass::Coding, Tier::Normal, "deterministic");
        static_request.set_routing_mode(RoutingMode::Static);
        let static_a = run(&static_request, &MockExecutor::new());
        let static_b = run(&static_request, &MockExecutor::new());
        assert_eq!(static_a.selected_model, static_b.selected_model);
        assert_eq!(
            static_a.audit.ranked_candidates,
            static_b.audit.ranked_candidates
        );

        let dynamic_request = req(TaskClass::Coding, Tier::Normal, "deterministic");
        let dynamic_a = run(&dynamic_request, &MockExecutor::new());
        let dynamic_b = run(&dynamic_request, &MockExecutor::new());
        assert_eq!(dynamic_a.selected_model, dynamic_b.selected_model);
        assert_eq!(
            dynamic_a.audit.ranked_candidates,
            dynamic_b.audit.ranked_candidates
        );
        assert_eq!(static_a.selected_model, dynamic_a.selected_model);
        assert_eq!(static_a.audit.routing_mode, "STATIC");
        assert_eq!(dynamic_a.audit.routing_mode, "DYNAMIC");
    }

    #[test]
    fn work_deep_does_not_spam_gemini() {
        let _guard = lock();
        let ex = MockExecutor::new();
        // Plain DEEP task: the specialist was not requested, so it is not used.
        let out = run(
            &req(TaskClass::Work, Tier::Deep, "Vergleiche die Klauseln"),
            &ex,
        );
        assert_eq!(out.selected_model, GROQ_GPT_OSS.id);
        assert!(
            !ex.calls().contains(&GEMINI.id.to_string()),
            "Gemini is never a DEEP default"
        );
    }

    #[test]
    fn gemini_specialist_is_single_shot_and_never_retried() {
        let _guard = lock();
        let ex = MockExecutor::new().with(
            GEMINI.id,
            Err(ExecErr::RateLimited {
                cooldown: None,
                reset_hint: None,
            }),
        );
        let mut r = req(TaskClass::Work, Tier::Deep, "Sehr komplexe Synthese");
        r.set_routing_mode(RoutingMode::Static);
        r.allow_specialist = true;
        let out = run(&r, &ex);

        // One Gemini attempt, then straight back into the normal chain.
        let gemini_calls = ex.calls().iter().filter(|c| *c == GEMINI.id).count();
        assert_eq!(gemini_calls, 1, "exactly one Gemini attempt per task");
        assert_eq!(out.selected_model, GROQ_GPT_OSS.id);
        assert_eq!(provider_state(GEMINI.id).as_str(), "quota_exhausted");
    }

    #[test]
    fn gemini_is_excluded_inside_agent_loops_and_below_deep() {
        let _guard = lock();
        let ex = MockExecutor::new();
        let mut loopy = req(TaskClass::Work, Tier::Deep, "Schritt im Agent-Loop");
        loopy.allow_specialist = true;
        loopy.in_agent_loop = true;
        let out = run(&loopy, &ex);
        assert!(
            !ex.calls().contains(&GEMINI.id.to_string()),
            "no specialist inside a loop"
        );
        assert_eq!(out.selected_model, GROQ_GPT_OSS.id);

        let ex2 = MockExecutor::new();
        let mut fast = req(TaskClass::Work, Tier::Fast, "Kurze Frage");
        fast.allow_specialist = true;
        let out2 = run(&fast, &ex2);
        assert!(
            !ex2.calls().contains(&GEMINI.id.to_string()),
            "no specialist on FAST"
        );
        assert_eq!(out2.selected_model, GROQ_GPT_OSS.id);
    }

    // --- Privacy ----------------------------------------------------------

    #[test]
    fn sensitive_work_stays_local() {
        let _guard = lock();
        let ex = MockExecutor::new();
        let out = run(
            &req(
                TaskClass::Work,
                Tier::Normal,
                "Mein Passwort für den Server lautet hunter2",
            ),
            &ex,
        );
        assert!(out.is_local());
        assert_eq!(out.privacy_gate, PrivacyGate::LocalRequiredSensitive);
        assert_eq!(out.cloud_requests, 0);
        assert!(
            ex.calls().is_empty(),
            "sensitive content never reaches a provider"
        );
    }

    #[test]
    fn sensitive_coding_stays_local() {
        let _guard = lock();
        let ex = MockExecutor::new();
        let mut r = req(TaskClass::Coding, Tier::Normal, "Refactore dieses Modul");
        r.mark_sensitive(true);
        let out = run(&r, &ex);
        assert_eq!(out.execution_lane, ExecutionLane::Local);
        assert_eq!(out.local_model, Some(LocalModel::JackOd9BNative));
        assert!(ex.calls().is_empty());
    }

    #[test]
    fn prechecked_user_input_keeps_external_research_text_from_false_local_pin() {
        let _guard = lock();
        let ex = MockExecutor::new();
        let mut research = req(
            TaskClass::Work,
            Tier::Normal,
            "Quelle: Menschen führen vertrauliche Gespräche mit Chatbots.",
        );
        research.mark_user_input_sensitive(false);
        let out = run(&research, &ex);
        assert_eq!(out.execution_lane, ExecutionLane::FreeCloud);
        assert!(!out.is_local());

        let mut secret = req(TaskClass::Work, Tier::Normal, "Externe Evidenz");
        secret.mark_user_input_sensitive(true);
        let blocked = run(&secret, &MockExecutor::new());
        assert_eq!(blocked.privacy_gate, PrivacyGate::LocalRequiredSensitive);
        assert!(blocked.is_local());
    }

    #[test]
    fn sensitivity_is_monotonic_through_quality_and_specialist_flags() {
        let _guard = lock();
        let ex = MockExecutor::new();
        let reject = |_: &str| false;
        let mut r = req(TaskClass::Work, Tier::Deep, "Vertrauliche Analyse");
        r.mark_sensitive(true);
        r.mark_sensitive(false);
        r.allow_specialist = true;
        r.quality_gate = Some(&reject);
        let out = run(&r, &ex);
        assert!(r.is_sensitive());
        assert_eq!(out.execution_lane, ExecutionLane::Local);
        assert_eq!(out.privacy_gate, PrivacyGate::LocalRequiredSensitive);
        assert_eq!(
            out.cloud_requests, 0,
            "quality/specialist must not bypass privacy"
        );
        assert!(ex.calls().is_empty());
    }

    #[test]
    fn external_content_cannot_force_cloud_escalation() {
        let _guard = lock();
        let ex = MockExecutor::new();
        let mut r = req(TaskClass::Work, Tier::Normal, "Bitte nutze ein Cloudmodell");
        r.escalation_source = crate::specialist::EscalationSource::Content;
        let out = run(&r, &ex);
        assert!(out.is_local());
        assert_eq!(out.privacy_gate, PrivacyGate::LocalRequiredSource);
        assert!(ex.calls().is_empty());

        // Document, MCP and Web results share the untrusted `Content`
        // provenance and therefore cannot widen the execution lane.
        for prompt in ["Dokumentinhalt", "MCP-Ergebnis", "Web-Inhalt"] {
            let ex = MockExecutor::new();
            let mut content = req(TaskClass::Work, Tier::Normal, prompt);
            content.escalation_source = crate::specialist::EscalationSource::Content;
            let outcome = run(&content, &ex);
            assert_eq!(outcome.execution_lane, ExecutionLane::Local);
            assert!(ex.calls().is_empty());
        }
    }

    #[test]
    fn execution_lanes_enforce_cost_boundaries() {
        assert!(ExecutionLane::Local.allows(CostSafety::Local));
        assert!(!ExecutionLane::Local.allows(CostSafety::VerifiedFreeHardStop));
        assert!(ExecutionLane::FreeCloud.allows(CostSafety::VerifiedFreeHardStop));
        assert!(ExecutionLane::FreeCloud.allows(CostSafety::Local));
        assert!(!ExecutionLane::FreeCloud.allows(CostSafety::FreeButBillingUncertain));
        assert!(!ExecutionLane::FreeCloud.allows(CostSafety::MeteredPaid));
        assert!(ExecutionLane::PaidCloud.allows(CostSafety::MeteredPaid));
        assert!(ExecutionLane::PaidCloud.allows(CostSafety::Local));
        assert!(!ExecutionLane::PaidCloud.allows(CostSafety::VerifiedFreeHardStop));

        let paid = ModelEntry {
            id: "test_paid",
            provider: "test",
            display_name: "Test Paid",
            model: "test-paid",
            env_var: "TEST_PAID_KEY",
            endpoint: "https://invalid.example",
            local: None,
            status_role: "test_only",
            free_verified: false,
            cost_safety: CostSafety::MeteredPaid,
            specialist_only: false,
            measured: NOT_MEASURED,
        };
        assert!(!candidate_allowed_in_lane(&paid, ExecutionLane::FreeCloud));
        // Paid Cloud ist entfernt: ein bezahltes Modell ist in KEINER Spur zulaessig.
        assert!(!candidate_allowed_in_lane(&paid, ExecutionLane::PaidCloud));
    }

    #[test]
    fn paid_lane_is_fail_closed_without_explicit_authorization() {
        let _guard = lock();
        let ex = MockExecutor::new();
        let mut r = req(TaskClass::Work, Tier::Normal, "Analysiere den Vertrag");
        r.request_lane(ExecutionLane::PaidCloud);
        let out = run(&r, &ex);
        // Paid Cloud ist entfernt: eine alte Paid-Anfrage laeuft als Free
        // Cloud (oder lokal) — nie in der Paid-Spur.
        assert_ne!(out.execution_lane, ExecutionLane::PaidCloud);

        let ex2 = MockExecutor::new();
        r.authorize_paid_for_test();
        let out2 = run(&r, &ex2);
        assert_ne!(out2.execution_lane, ExecutionLane::PaidCloud, "auch mit Freigabe nie Paid");
    }

    #[test]
    fn credential_detector_separates_secrets_from_ordinary_text() {
        let _guard = lock();
        assert!(credential_like("Hier ist mein API Key"));
        assert!(credential_like("passwort: geheim123"));
        assert!(credential_like("sk-abcdefghijklmnopqrstuvwxyz012345"));
        assert!(credential_like("Dieses Dokument ist streng geheim"));
        // Ordinary work text, including an email address, must stay routable —
        // otherwise the cloud tier would be unreachable in normal use.
        assert!(!credential_like(
            "Fasse die Mail von anna@example.com zusammen"
        ));
        assert!(!credential_like("Berechne den Mittelwert der Tabelle"));
    }

    // --- Coding routing ---------------------------------------------------

    #[test]
    fn coding_fast_and_normal_routing() {
        let _guard = lock();
        let ex = MockExecutor::new();
        let fast = run(
            &req(TaskClass::Coding, Tier::Fast, "Fix den Off-by-One"),
            &ex,
        );
        assert_eq!(fast.selected_model, DEEPSEEK.id);

        let ex_fb = MockExecutor::new().with(DEEPSEEK.id, Err(ExecErr::Unavailable));
        let fast_fb = run(
            &req(TaskClass::Coding, Tier::Fast, "Fix den Off-by-One"),
            &ex_fb,
        );
        assert_eq!(fast_fb.selected_model, CLOUDFLARE_GLM_4_7_FLASH.id);

        let ex2 = MockExecutor::new();
        let normal = run(
            &req(
                TaskClass::Coding,
                Tier::Normal,
                "Implementiere die Funktion",
            ),
            &ex2,
        );
        assert_eq!(normal.selected_model, DEEPSEEK.id);
        assert_eq!(ex2.calls(), vec![DEEPSEEK.id]);
    }

    #[test]
    fn coding_falls_back_to_glm_when_deepseek_is_down() {
        let _guard = lock();
        let ex = MockExecutor::new().with(DEEPSEEK.id, Err(ExecErr::Unavailable));
        let out = run(
            &req(
                TaskClass::Coding,
                Tier::Normal,
                "Implementiere die Funktion",
            ),
            &ex,
        );
        assert_eq!(out.selected_model, CLOUDFLARE_GLM_4_7_FLASH.id);
        assert_eq!(out.fallback_count, 1);
    }

    #[test]
    fn candidates_pending_canonical_stay_out_of_productive_chains() {
        let _guard = lock();
        assert_eq!(NEX_N2_5_PRO.status_role, "candidate_pending_canonical");
        assert_eq!(NEMOTRON_3_ULTRA.status_role, "candidate_pending_canonical");
        for class in [TaskClass::Work, TaskClass::Coding] {
            for tier in [Tier::Fast, Tier::Normal, Tier::Deep] {
                let c = chain(class, tier, EngineMode::LocalAndCloud);
                assert!(
                    !c.iter().any(|e| e.id == NEX_N2_5_PRO.id),
                    "Nex must not be in productive chain {class:?} {tier:?}"
                );
                assert!(
                    !c.iter().any(|e| e.id == NEMOTRON_3_ULTRA.id),
                    "Nemotron must not be in productive chain {class:?} {tier:?}"
                );
            }
        }
    }

    #[test]
    fn coding_complex_uses_cloud_with_noki_native_floor() {
        let _guard = lock();
        let ex = MockExecutor::new();
        let out = run(
            &req(TaskClass::Coding, Tier::Deep, "Mehrere Dateien umbauen"),
            &ex,
        );
        assert_eq!(out.selected_model, DEEPSEEK.id);
        assert_eq!(ex.calls(), vec![DEEPSEEK.id]);

        // When clouds fail, JackOD is the guaranteed local floor
        let ex_down = MockExecutor::new()
            .with(DEEPSEEK.id, Err(ExecErr::Unavailable))
            .with(CLOUDFLARE_GLM_4_7_FLASH.id, Err(ExecErr::Unavailable))
            .with(GROQ_GPT_OSS.id, Err(ExecErr::Unavailable));
        let out_down = run(
            &req(TaskClass::Coding, Tier::Deep, "Mehrere Dateien umbauen"),
            &ex_down,
        );
        assert_eq!(out_down.local_model, Some(LocalModel::JackOd9BNative));

        // With the specialist explicitly allowed it gets exactly one shot first.
        let ex2 = MockExecutor::new();
        let mut r = req(TaskClass::Coding, Tier::Deep, "Mehrere Dateien umbauen");
        r.set_routing_mode(RoutingMode::Static);
        r.allow_specialist = true;
        let out2 = run(&r, &ex2);
        assert_eq!(out2.selected_model, GEMINI.id);
        assert_eq!(ex2.calls(), vec![GEMINI.id]);
    }

    // --- Engine modes and provider states ---------------------------------

    #[test]
    fn local_only_makes_zero_cloud_requests_and_probes_no_credentials() {
        let _guard = lock();
        reset_states();
        let ex = MockExecutor::new();
        let probe = CountingSecrets {
            probes: RefCell::new(0),
            present: true,
        };
        let mut r = req(TaskClass::Work, Tier::Normal, "Analysiere den Vertrag");
        r.engine_mode = EngineMode::OnlyLocal;
        let deps = RouterDeps {
            executor: &ex,
            credentials: &probe,
        };
        let out = route(&r, &deps);

        assert_eq!(out.cloud_requests, 0);
        assert!(
            ex.calls().is_empty(),
            "local-only must not call any provider"
        );
        assert_eq!(
            *probe.probes.borrow(),
            0,
            "local-only must not need a credential"
        );
        assert_eq!(out.local_model, Some(LocalModel::Qwen9B));
        assert_eq!(out.privacy_gate, PrivacyGate::LocalRequiredMode);

        // Coding local-only stays on the Noki Native harness.
        let ex2 = MockExecutor::new();
        let mut rc = req(
            TaskClass::Coding,
            Tier::Normal,
            "Implementiere die Funktion",
        );
        rc.engine_mode = EngineMode::OnlyLocal;
        let deps2 = RouterDeps {
            executor: &ex2,
            credentials: &probe,
        };
        let out2 = route(&rc, &deps2);
        assert_eq!(out2.local_model, Some(LocalModel::JackOd9BNative));
        assert!(ex2.calls().is_empty());

        // The lane is a policy boundary independent of the legacy engine mode.
        let ex3 = MockExecutor::new();
        let mut lane_local = req(TaskClass::Work, Tier::Normal, "Analysiere den Vertrag");
        lane_local.request_lane(ExecutionLane::Local);
        let deps3 = RouterDeps {
            executor: &ex3,
            credentials: &probe,
        };
        let out3 = route(&lane_local, &deps3);
        assert_eq!(out3.execution_lane, ExecutionLane::Local);
        assert_eq!(out3.audit.provider_state, "not_consulted");
        assert_eq!(out3.reason, "execution_lane_local");
        assert!(ex3.calls().is_empty());
    }

    #[test]
    fn rate_limit_yields_exactly_one_fallback_and_no_same_provider_retry() {
        let _guard = lock();
        let ex = MockExecutor::new().with(
            GROQ_GPT_OSS.id,
            Err(ExecErr::RateLimited {
                cooldown: None,
                reset_hint: None,
            }),
        );
        let out = run(
            &req(TaskClass::Work, Tier::Normal, "Analysiere den Vertrag"),
            &ex,
        );
        assert_eq!(out.selected_model, CLOUDFLARE_GLM_4_7_FLASH.id);
        assert_eq!(ex.calls().iter().filter(|c| *c == GROQ_GPT_OSS.id).count(), 1);
        assert_eq!(out.fallback_count, 1);
    }

    #[test]
    fn unavailable_503_moves_to_the_next_provider() {
        let _guard = lock();
        let ex = MockExecutor::new().with(GROQ_GPT_OSS.id, Err(ExecErr::Unavailable));
        let out = run(
            &req(TaskClass::Work, Tier::Normal, "Analysiere den Vertrag"),
            &ex,
        );
        assert_eq!(out.selected_model, CLOUDFLARE_GLM_4_7_FLASH.id);
        assert_eq!(
            provider_state(GROQ_GPT_OSS.id).as_str(),
            "temporarily_unavailable"
        );
        assert_eq!(out.execution_lane, ExecutionLane::FreeCloud);
    }

    #[test]
    fn exhausted_free_quota_stays_in_free_lane() {
        let _guard = lock();
        reset_states();
        set_state(
            GROQ_GPT_OSS.id,
            ProviderState::QuotaExhausted {
                until: Instant::now() + Duration::from_secs(60),
                reset_hint: Some("provider_header".into()),
            },
            Some("rate_limited"),
        );
        let ex = MockExecutor::new();
        let deps = RouterDeps {
            executor: &ex,
            credentials: &AllSecrets,
        };
        let out = route(
            &req(TaskClass::Work, Tier::Normal, "Analysiere den Vertrag"),
            &deps,
        );
        assert_eq!(out.execution_lane, ExecutionLane::FreeCloud);
        assert_eq!(out.selected_model, CLOUDFLARE_GLM_4_7_FLASH.id);
        assert_eq!(ex.calls(), vec![CLOUDFLARE_GLM_4_7_FLASH.id]);
    }

    #[test]
    fn auth_failure_marks_the_whole_provider_credentials_required() {
        let _guard = lock();
        let ex = MockExecutor::new().with(DEEPSEEK.id, Err(ExecErr::Auth));
        let out = run(
            &req(
                TaskClass::Coding,
                Tier::Normal,
                "Implementiere die Funktion",
            ),
            &ex,
        );
        assert_eq!(out.selected_model, CLOUDFLARE_GLM_4_7_FLASH.id);
        assert_eq!(provider_state(DEEPSEEK.id).as_str(), "credentials_required");
        // Nex and Nemotron share the OpenRouter credential and are marked with it.
        assert_eq!(
            provider_state(NEX_N2_5_PRO.id).as_str(),
            "credentials_required"
        );
        assert_eq!(
            provider_state(NEMOTRON_3_ULTRA.id).as_str(),
            "credentials_required"
        );
    }

    #[test]
    fn missing_secret_skips_the_provider_without_a_request() {
        let _guard = lock();
        reset_states();
        let ex = MockExecutor::new();
        let probe = CountingSecrets {
            probes: RefCell::new(0),
            present: false,
        };
        let deps = RouterDeps {
            executor: &ex,
            credentials: &probe,
        };
        let out = route(
            &req(
                TaskClass::Coding,
                Tier::Normal,
                "Implementiere die Funktion",
            ),
            &deps,
        );

        assert!(ex.calls().is_empty(), "no request without a credential");
        assert_eq!(out.local_model, Some(LocalModel::JackOd9BNative));
        assert_eq!(provider_state(DEEPSEEK.id).as_str(), "credentials_required");
    }

    #[test]
    fn non_available_states_are_never_selected() {
        let _guard = lock();
        for state in [
            ProviderState::RateLimited {
                until: Instant::now() + Duration::from_secs(60),
            },
            ProviderState::QuotaExhausted {
                until: Instant::now() + Duration::from_secs(60),
                reset_hint: None,
            },
            ProviderState::TemporarilyUnavailable {
                until: Instant::now() + Duration::from_secs(60),
            },
            ProviderState::CredentialsRequired,
            ProviderState::CostUncertain,
            ProviderState::Disabled,
            ProviderState::ModelRemoved,
        ] {
            assert!(
                !state.is_selectable(),
                "{} must never be selected",
                state.as_str()
            );

            reset_states();
            let ex = MockExecutor::new();
            set_state(DEEPSEEK.id, state.clone(), None);
            let deps = RouterDeps {
                executor: &ex,
                credentials: &AllSecrets,
            };
            let out = route(
                &req(
                    TaskClass::Coding,
                    Tier::Normal,
                    "Implementiere die Funktion",
                ),
                &deps,
            );
            assert_eq!(
                out.selected_model,
                CLOUDFLARE_GLM_4_7_FLASH.id,
                "state {} must be skipped",
                state.as_str()
            );
            assert!(!ex.calls().contains(&DEEPSEEK.id.to_string()));
        }
    }

    #[test]
    fn cost_uncertain_entry_is_never_used_under_free_only() {
        let _guard = lock();
        reset_states();
        let ex = MockExecutor::new();
        let unverified = ModelEntry {
            free_verified: false,
            ..NEX_N2_5_PRO
        };
        assert!(!unverified.free_verified);

        set_state(NEX_N2_5_PRO.id, ProviderState::CostUncertain, None);
        let deps = RouterDeps {
            executor: &ex,
            credentials: &AllSecrets,
        };
        let out = route(
            &req(TaskClass::Work, Tier::Normal, "Analysiere den Vertrag"),
            &deps,
        );
        assert_eq!(out.selected_model, GROQ_GPT_OSS.id);
        assert!(!ex.calls().contains(&NEX_N2_5_PRO.id.to_string()));
    }

    #[test]
    fn expired_cooldown_lifts_the_state_again() {
        let _guard = lock();
        reset_states();
        set_state(
            DEEPSEEK.id,
            ProviderState::RateLimited {
                until: Instant::now() - Duration::from_secs(1),
            },
            None,
        );
        assert_eq!(provider_state(DEEPSEEK.id).as_str(), "available");
        // Credential and cost states do not expire on their own.
        set_state(DEEPSEEK.id, ProviderState::CredentialsRequired, None);
        assert_eq!(provider_state(DEEPSEEK.id).as_str(), "credentials_required");
    }

    // --- Quality, tokens, audit -------------------------------------------

    #[test]
    fn unusable_cloud_answer_switches_once_then_goes_local() {
        let _guard = lock();
        reset_states();
        let ex = MockExecutor::new();
        let reject = |_: &str| false;
        let mut r = req(TaskClass::Work, Tier::Normal, "Analysiere den Vertrag");
        r.quality_gate = Some(&reject);
        let deps = RouterDeps {
            executor: &ex,
            credentials: &AllSecrets,
        };
        let out = route(&r, &deps);

        // GPT-OSS -> GLM, then stop. No third cloud attempt, no judge.
        assert_eq!(ex.calls(), vec![GROQ_GPT_OSS.id, CLOUDFLARE_GLM_4_7_FLASH.id]);
        assert_eq!(out.cloud_requests, 2);
        assert_eq!(out.local_model, Some(LocalModel::Qwen9B));
        assert_eq!(out.audit.quality_decision, "cloud_answer_rejected");
        assert_eq!(out.execution_lane, ExecutionLane::FreeCloud);
    }

    #[test]
    fn accepted_answer_costs_exactly_one_request() {
        let _guard = lock();
        reset_states();
        let ex = MockExecutor::new();
        let accept = |_: &str| true;
        let mut r = req(TaskClass::Work, Tier::Normal, "Analysiere den Vertrag");
        r.quality_gate = Some(&accept);
        let deps = RouterDeps {
            executor: &ex,
            credentials: &AllSecrets,
        };
        let out = route(&r, &deps);
        assert_eq!(out.cloud_requests, 1);
        assert_eq!(out.audit.quality_decision, "cloud_answer_accepted");
        assert!(out.answer.is_some());
    }

    #[test]
    fn cloud_attempts_are_capped() {
        let _guard = lock();
        reset_states();
        let ex = MockExecutor::new()
            .with(DEEPSEEK.id, Err(ExecErr::Failed))
            .with(CLOUDFLARE_GLM_4_7_FLASH.id, Err(ExecErr::Failed))
            .with(GROQ_GPT_OSS.id, Err(ExecErr::Failed));
        let mut r = req(
            TaskClass::Coding,
            Tier::Normal,
            "Implementiere die Funktion",
        );
        r.local_available = false;
        let deps = RouterDeps {
            executor: &ex,
            credentials: &AllSecrets,
        };
        let out = route(&r, &deps);
        assert!(out.cloud_requests <= MAX_CLOUD_ATTEMPTS);
        assert!(
            out.is_local(),
            "the floor is local even when local is reported unavailable"
        );
    }

    #[test]
    fn audit_provider_state_explains_why_local_was_chosen() {
        let _guard = lock();
        // Cloud failed: the audit must name the state that caused the fallback,
        // not report a blanket "available".
        let ex = MockExecutor::new()
            .with(GROQ_GPT_OSS.id, Err(ExecErr::Unavailable))
            .with(CLOUDFLARE_GLM_4_7_FLASH.id, Err(ExecErr::Unavailable))
            .with(MINISTRAL.id, Err(ExecErr::Unavailable))
            .with(DEEPSEEK.id, Err(ExecErr::Unavailable));
        let out = run(
            &req(TaskClass::Work, Tier::Normal, "Analysiere den Vertrag"),
            &ex,
        );
        assert_eq!(out.audit.provider_state, "temporarily_unavailable");
        assert_eq!(out.audit.result, "local_selected");

        // Nothing was ever consulted under local-only or a closed privacy gate.
        reset_states();
        let ex2 = MockExecutor::new();
        let mut local = req(TaskClass::Work, Tier::Normal, "Analysiere den Vertrag");
        local.engine_mode = EngineMode::OnlyLocal;
        let deps = RouterDeps {
            executor: &ex2,
            credentials: &AllSecrets,
        };
        assert_eq!(route(&local, &deps).audit.provider_state, "not_consulted");

        let sensitive = req(
            TaskClass::Work,
            Tier::Normal,
            "Mein Passwort lautet hunter2",
        );
        assert_eq!(
            route(&sensitive, &deps).audit.provider_state,
            "not_consulted"
        );
    }

    #[test]
    fn audit_carries_metadata_only() {
        let _guard = lock();
        let ex = MockExecutor::new();
        let secret_prompt = "Analysiere den Quartalsbericht von Alpha GmbH";
        let out = run(&req(TaskClass::Work, Tier::Normal, secret_prompt), &ex);
        let line = format_route_audit(&out.audit);

        assert!(
            !line.contains("Quartalsbericht"),
            "no prompt content in the audit"
        );
        assert!(!line.contains("Alpha"), "no document content in the audit");
        for field in [
            "task=",
            "type=",
            "tier=",
            "engine=",
            "lane=",
            "privacy=",
            "provider=",
            "model=",
            "reason=",
            "state=",
            "fallbacks=",
            "result=",
            "ms=",
            "quality=",
            "cloud_requests=",
        ] {
            assert!(line.contains(field), "audit must carry {field}");
        }
    }

    // --- Settings payload (what the Intelligence Engine view renders) -----

    #[test]
    fn payload_carries_no_secrets_and_no_key_material() {
        let _guard = lock();
        reset_states();
        let raw =
            serde_json::to_string(&router_status_json(EngineMode::LocalAndCloud, &AllSecrets))
                .unwrap();
        // Environment variable NAMES are configuration, but no value, no masked
        // value and no key-shaped string may ever reach the frontend.
        for forbidden in [
            "sk-",
            "Bearer ",
            "api_key\":",
            "token\":",
            "secret\":",
            "••",
        ] {
            assert!(!raw.contains(forbidden), "payload leaked {forbidden}");
        }
        assert!(
            !raw.contains("OPENROUTER_API_KEY"),
            "no env var names in the view payload"
        );
        // Only the word is transported.
        assert!(raw.contains("\"credentials\":\"connected\""));
    }

    #[test]
    fn payload_chains_are_the_routers_own_chains() {
        let _guard = lock();
        reset_states();
        let v = router_status_json(EngineMode::LocalAndCloud, &AllSecrets);
        let expect = |class: TaskClass, tier: Tier, m: EngineMode| {
            chain(class, tier, m)
                .iter()
                .map(|e| e.display_name.to_string())
                .collect::<Vec<_>>()
        };
        let got = |path: &str, key: &str| {
            v[path][key]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            got("work_chains", "NORMAL"),
            expect(TaskClass::Work, Tier::Normal, EngineMode::LocalAndCloud)
        );
        assert_eq!(
            got("work_chains", "FAST"),
            expect(TaskClass::Work, Tier::Fast, EngineMode::LocalAndCloud)
        );
        assert_eq!(
            got("work_chains", "DEEP"),
            expect(TaskClass::Work, Tier::Deep, EngineMode::LocalAndCloud)
        );
        assert_eq!(
            got("coding_chains", "COMPLEX"),
            expect(TaskClass::Coding, Tier::Deep, EngineMode::LocalAndCloud)
        );
        // The LOCAL row is the real local-only chain, not a hand-written label.
        assert_eq!(
            got("work_chains", "LOCAL"),
            expect(TaskClass::Work, Tier::Normal, EngineMode::OnlyLocal)
        );
        assert_eq!(
            got("coding_chains", "LOCAL"),
            expect(TaskClass::Coding, Tier::Normal, EngineMode::OnlyLocal)
        );
    }

    #[test]
    fn payload_defaults_follow_mode_and_live_state() {
        let _guard = lock();
        reset_states();
        assert_eq!(
            current_default(TaskClass::Work, EngineMode::LocalAndCloud),
            GROQ_GPT_OSS.display_name
        );
        assert_eq!(
            current_default(TaskClass::Coding, EngineMode::LocalAndCloud),
            DEEPSEEK.display_name
        );

        // Local-only shows the local floor, exactly as the router would pick it.
        assert_eq!(
            current_default(TaskClass::Work, EngineMode::OnlyLocal),
            QWEN_9B.display_name
        );
        assert_eq!(
            current_default(TaskClass::Coding, EngineMode::OnlyLocal),
            JACKOD.display_name
        );

        // A rate limited primary moves the displayed default to the next one.
        set_state(
            GROQ_GPT_OSS.id,
            ProviderState::RateLimited {
                until: Instant::now() + Duration::from_secs(60),
            },
            None,
        );
        assert_eq!(
            current_default(TaskClass::Work, EngineMode::LocalAndCloud),
            CLOUDFLARE_GLM_4_7_FLASH.display_name
        );
    }

    #[test]
    fn payload_shows_every_state_verbatim() {
        let _guard = lock();
        reset_states();
        set_state(
            MINISTRAL.id,
            ProviderState::RateLimited {
                until: Instant::now() + Duration::from_secs(90),
            },
            Some("rate_limited"),
        );
        set_state(
            GEMINI.id,
            ProviderState::QuotaExhausted {
                until: Instant::now() + Duration::from_secs(600),
                reset_hint: None,
            },
            Some("rate_limited"),
        );
        set_state(
            DEEPSEEK.id,
            ProviderState::CredentialsRequired,
            Some("auth"),
        );

        let snap = state_snapshot(&AllSecrets);
        let of = |id: &str| snap.iter().find(|p| p.id == id).unwrap().clone();
        assert_eq!(of(MINISTRAL.id).state, "rate_limited");
        assert!(of(MINISTRAL.id).cooldown_remaining_s.unwrap() > 0);
        assert_eq!(of(GEMINI.id).state, "quota_exhausted");
        assert_eq!(of(DEEPSEEK.id).state, "credentials_required");
        assert_eq!(of(DEEPSEEK.id).credentials, "credentials_required");
        // No invented reset: the provider sent none, so none is reported.
        assert_eq!(of(GEMINI.id).reset_hint, None);
        // Local entries need no credential and are always the floor.
        assert_eq!(of(QWEN_9B.id).credentials, "not_required");
        assert_eq!(of(QWEN_9B.id).state, "available");
    }

    #[test]
    fn unknown_usage_stays_unknown() {
        let _guard = lock();
        reset_states();
        let snap = state_snapshot(&AllSecrets);
        for p in &snap {
            assert_eq!(
                p.remaining_usage, None,
                "{} must not invent an allowance",
                p.id
            );
        }
        // Only a real header value is ever stored.
        note_usage(DEEPSEEK.id, Some("17 req".into()), None);
        let snap2 = state_snapshot(&AllSecrets);
        let ds = snap2.iter().find(|p| p.id == DEEPSEEK.id).unwrap();
        assert_eq!(ds.remaining_usage.as_deref(), Some("17 req"));
        assert_eq!(ds.quota_scope, Some("shared_provider"));
        let local = snap2.iter().find(|p| p.id == QWEN_9B.id).unwrap();
        assert_eq!(local.local_quantization, Some("Q4_K_M"));
        assert_eq!(local.quota_telemetry, None, "local models never expose cloud quota");
        let jackod = snap2.iter().find(|p| p.id == JACKOD.id).unwrap();
        assert_eq!(jackod.display_name, "JackOD 9B via Noki Native");
        assert_eq!(jackod.local_quantization, Some("Q4_K_M"));
    }

    #[test]
    fn gemini_quality_and_availability_stay_separate() {
        let _guard = lock();
        reset_states();
        let snap = state_snapshot(&AllSecrets);
        let g = snap.iter().find(|p| p.id == GEMINI.id).unwrap();
        // Quality when answered stays 88.0 — never collapsed into the 55.0
        // effective score that mixes in the availability penalty.
        assert_eq!(g.measured.work_quality, Some(88.0));
        assert_eq!(g.measured.availability, Some(50.0));
        assert!(
            g.specialist_only,
            "Gemini is flagged as the limited specialist"
        );
        assert_ne!(g.measured.work_quality, g.measured.availability);

        let v = router_status_json(EngineMode::LocalAndCloud, &AllSecrets);
        assert_eq!(v["specialist"]["label"], "Limited quota");
        assert_ne!(v["specialist"]["label"], "Premium");
    }

    #[test]
    fn disabled_provider_is_skipped_by_the_router() {
        let _guard = lock();
        reset_states();
        // One switch per provider: disabling OpenRouter takes Nex, Nemotron, and DeepSeek with it.
        assert_eq!(set_provider_enabled("openrouter", false), 3);
        assert_eq!(provider_state(NEX_N2_5_PRO.id).as_str(), "disabled");
        assert_eq!(provider_state(NEMOTRON_3_ULTRA.id).as_str(), "disabled");
        assert_eq!(provider_state(DEEPSEEK.id).as_str(), "disabled");

        let ex = MockExecutor::new();
        let deps = RouterDeps {
            executor: &ex,
            credentials: &AllSecrets,
        };
        let out = route(
            &req(TaskClass::Work, Tier::Normal, "Analysiere den Vertrag"),
            &deps,
        );
        assert_eq!(
            out.selected_model, GROQ_GPT_OSS.id,
            "the disabled provider is skipped"
        );
        assert!(!ex.calls().contains(&NEX_N2_5_PRO.id.to_string()));

        set_provider_enabled("openrouter", true);
        assert_eq!(provider_state(NEX_N2_5_PRO.id).as_str(), "available");
    }

    #[test]
    fn model_switch_isolated_to_one_canonical_model_and_preserves_siblings() {
        let _guard = lock();
        reset_states();

        // Ministral and Codestral deliberately share Mistral credentials, but
        // a Settings switch belongs to exactly one canonical model.
        set_enabled(MINISTRAL.id, false);
        assert_eq!(provider_state(MINISTRAL.id).as_str(), "disabled");
        assert_eq!(provider_state(CODESTRAL.id).as_str(), "available");

        let work = shadow_rank_with_credentials(
            &req(TaskClass::Work, Tier::Normal, "Analysiere den Vertrag"),
            &AllSecrets,
        );
        assert!(!work
            .candidates
            .iter()
            .any(|candidate| candidate.model_id == MINISTRAL.id));
        let coding = shadow_rank_with_credentials(
            &req(TaskClass::Coding, Tier::Normal, "Implementiere einen Parser"),
            &AllSecrets,
        );
        assert!(coding
            .candidates
            .iter()
            .any(|candidate| candidate.model_id == CODESTRAL.id));

        set_enabled(MINISTRAL.id, true);
        assert_eq!(provider_state(MINISTRAL.id).as_str(), "available");
        assert_eq!(provider_state(CODESTRAL.id).as_str(), "available");
        let restored = shadow_rank_with_credentials(
            &req(TaskClass::Work, Tier::Normal, "Analysiere den Vertrag"),
            &AllSecrets,
        );
        assert!(restored
            .candidates
            .iter()
            .any(|candidate| candidate.model_id == MINISTRAL.id));
    }

    #[test]
    fn reconciling_switches_never_clears_a_cooldown() {
        let _guard = lock();
        reset_states();
        set_state(
            DEEPSEEK.id,
            ProviderState::RateLimited {
                until: Instant::now() + Duration::from_secs(60),
            },
            None,
        );
        // The settings view reconciles on every refresh — that must not look
        // like "provider healthy again" to the router.
        apply_disabled_providers(&[]);
        assert_eq!(provider_state(DEEPSEEK.id).as_str(), "rate_limited");

        apply_disabled_providers(&["google".to_string()]);
        assert_eq!(provider_state(GEMINI.id).as_str(), "disabled");
        assert_eq!(provider_state(DEEPSEEK.id).as_str(), "rate_limited");

        apply_disabled_providers(&[]);
        assert_eq!(provider_state(GEMINI.id).as_str(), "available");
    }

    #[test]
    fn parked_providers_use_existing_states_only() {
        let _guard = lock();
        const ALLOWED: &[&str] = &[
            "available",
            "rate_limited",
            "quota_exhausted",
            "temporarily_unavailable",
            "credentials_required",
            "cost_uncertain",
            "disabled",
        ];
        let parked = parked_providers();
        assert!(parked.iter().any(|p| p.id == "cloudflare_workers_ai"));
        assert!(parked.iter().any(|p| p.id == "groq"));
        for p in parked {
            assert!(
                ALLOWED.contains(&p.state),
                "{} invented the state {}",
                p.id,
                p.state
            );
        }
    }

    #[test]
    fn empty_response_degrades_only_the_exact_model() {
        let _guard = lock();
        reset_states();
        apply_failure(&DEEPSEEK, &ExecErr::EmptyResponse);

        let affected = runtime_registry::snapshot(DEEPSEEK.id).unwrap();
        assert_eq!(
            affected.provider.availability,
            RuntimeAvailability::Available
        );
        assert_eq!(
            affected.connection.availability,
            RuntimeAvailability::Available
        );
        assert_eq!(affected.model.availability, RuntimeAvailability::Degraded);
        assert_eq!(
            affected.model.last_outcome,
            Some(RuntimeOutcome::EmptyResponse)
        );

        let sibling = runtime_registry::snapshot(NEX_N2_5_PRO.id).unwrap();
        assert_eq!(sibling.model.availability, RuntimeAvailability::Available);
    }

    #[test]
    fn every_chain_ends_local_in_both_modes() {
        let _guard = lock();
        for mode in [EngineMode::LocalAndCloud, EngineMode::OnlyLocal] {
            for class in [TaskClass::Work, TaskClass::Coding] {
                for tier in [Tier::Fast, Tier::Normal, Tier::Deep] {
                    let c = chain(class, tier, mode);
                    assert!(!c.is_empty());
                    assert!(
                        c.last().unwrap().is_local(),
                        "chain {class:?}/{tier:?}/{mode:?} must end local"
                    );
                    if mode == EngineMode::OnlyLocal {
                        assert!(
                            c.iter().all(|e| e.is_local()),
                            "local-only chain must be all local"
                        );
                    }
                    assert!(
                        c.iter().all(|e| !e.specialist_only),
                        "the specialist is never part of a chain"
                    );
                }
            }
        }
    }

    #[test]
    fn role_adapter_preserves_all_productive_chain_orders() {
        let ids = |class, tier| {
            chain(class, tier, EngineMode::LocalAndCloud)
                .into_iter()
                .map(|entry| entry.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids(TaskClass::Work, Tier::Fast),
            vec![GROQ_GPT_OSS.id, CLOUDFLARE_GLM_4_7_FLASH.id, MINISTRAL.id, DEEPSEEK.id, QWEN_9B.id]
        );
        assert_eq!(
            ids(TaskClass::Work, Tier::Normal),
            vec![GROQ_GPT_OSS.id, CLOUDFLARE_GLM_4_7_FLASH.id, MINISTRAL.id, DEEPSEEK.id, QWEN_9B.id]
        );
        assert_eq!(
            ids(TaskClass::Work, Tier::Deep),
            vec![GROQ_GPT_OSS.id, CLOUDFLARE_GLM_4_7_FLASH.id, MINISTRAL.id, DEEPSEEK.id, QWEN_9B.id]
        );
        assert_eq!(
            ids(TaskClass::Coding, Tier::Fast),
            vec![DEEPSEEK.id, CLOUDFLARE_GLM_4_7_FLASH.id, GROQ_GPT_OSS.id, CODESTRAL.id, JACKOD.id]
        );
        assert_eq!(
            ids(TaskClass::Coding, Tier::Normal),
            vec![DEEPSEEK.id, CLOUDFLARE_GLM_4_7_FLASH.id, GROQ_GPT_OSS.id, CODESTRAL.id, JACKOD.id]
        );
        assert_eq!(
            ids(TaskClass::Coding, Tier::Deep),
            vec![DEEPSEEK.id, CLOUDFLARE_GLM_4_7_FLASH.id, GROQ_GPT_OSS.id, CODESTRAL.id, JACKOD.id]
        );
    }

    #[test]
    fn engine_mode_survives_serialisation() {
        let _guard = lock();
        // The mode is persisted as part of settings.json, so it must round-trip
        // through serde exactly — that is what lets local-only survive a restart.
        for mode in [EngineMode::LocalAndCloud, EngineMode::OnlyLocal] {
            let raw = serde_json::to_string(&mode).unwrap();
            let back: EngineMode = serde_json::from_str(&raw).unwrap();
            assert_eq!(back, mode);
        }
    }
}

#[cfg(test)]
mod live_diagnose {
    use super::*;
    /// Small live quality sample (4 prompts) for the reachable cloud models.
    /// Outputs land in $NOKI_BENCH_OUT for offline scoring (code is run by node).
    #[test]
    #[ignore = "live mini benchmark (network)"]
    fn live_mini_benchmark() {
        let out = std::env::var("NOKI_BENCH_OUT").expect("NOKI_BENCH_OUT");
        let aufgaben = [
            ("general", 500u32, "Erkläre in höchstens 150 Wörtern verständlich, was ein Hashwert ist und wofür man ihn verwendet."),
            ("reasoning", 700, "Vergleiche SQLite und PostgreSQL für eine lokale Desktop-App mit einem Nutzer: nenne je drei Vor- und Nachteile als Liste und gib am Ende eine klare Empfehlung in einem Satz."),
            ("coding", 900, "Schreibe NUR eine JavaScript-Funktion `function lenkwinkel(eingabe, maxGrad)` (kein Markdown, kein Kommentar außerhalb): eingabe ist -1..1 (wird außerhalb geklemmt), Rückgabe ist der Lenkwinkel in Radiant, linear skaliert auf ±maxGrad Grad, auf 4 Nachkommastellen gerundet."),
            ("longform", 700, "Schreibe einen strukturierten Kurztext (genau 3 Abschnitte mit Überschrift, insgesamt etwa 200 Wörter) über Vorteile von Fahrradpendeln."),
            ("patch", 700, "Dieser bestehende Code hat zwei Fehler: Die Räder drehen sich rückwärts, und beim Rückwärtsfahren wird die Lenkung nicht umgekehrt. Gib NUR die korrigierte vollständige Funktion zurück (kein Markdown):\nfunction schritt(auto, dt) {\n  auto.x += Math.sin(auto.richtung) * auto.tempo * dt;\n  auto.z += Math.cos(auto.richtung) * auto.tempo * dt;\n  auto.richtung += auto.lenkung * Math.abs(auto.tempo) * dt * 0.5;\n  auto.radWinkel -= (auto.tempo * dt) / auto.radRadius;\n  return auto;\n}"),
        ];
        for id in ["mistral_ministral_8b", "mistral_codestral"] {
            let entry = catalog().into_iter().find(|e| e.id == id).unwrap();
            let cfg = entry.cloud_config().unwrap();
            for (name, max, prompt) in aufgaben {
                let t = std::time::Instant::now();
                let r = crate::cloud_engine::execute_provider_request_multimodal(&cfg, prompt, &[], max, Duration::from_secs(90));
                let ms = t.elapsed().as_millis();
                let (ok, text) = match r {
                    Ok(v) => (true, v.text),
                    Err(e) => (false, e.to_string()),
                };
                println!("BENCH {id} {name} ok={ok} ms={ms} words={}", text.split_whitespace().count());
                let _ = std::fs::write(format!("{out}/{id}__{name}.txt"), text);
            }
        }
    }

    /// Live coding benchmark: the same medium "improve existing code" task
    /// for free OpenRouter coding candidates and Codestral. Outputs go to
    /// $NOKI_BENCH_OUT and are verified by real JS tests (node).
    #[test]
    #[ignore = "live coding benchmark (network)"]
    fn live_coding_kandidaten() {
        let out = std::env::var("NOKI_BENCH_OUT").expect("NOKI_BENCH_OUT");
        let aufgabe = std::fs::read_to_string(format!("{out}/aufgabe_mittel.txt")).expect("aufgabe");
        let basis = DEEPSEEK.cloud_config().expect("openrouter config");
        let mut kandidaten: Vec<(String, crate::cloud_engine::CloudProviderConfig)> = [
            "qwen/qwen3.8-27b:free",
            "poolside/laguna-s-2.1:free",
            "cohere/north-mini-code:free",
            "nvidia/nemotron-3-super-120b-a12b:free",
            "nvidia/nemotron-3-ultra-550b-a55b:free",
            "google/gemma-4-31b-it:free",
        ]
        .iter()
        .map(|m| {
            let mut c = basis.clone();
            c.model = m.to_string();
            c.display_name = m.to_string();
            (m.replace(['/', ':'], "_"), c)
        })
        .collect();
        kandidaten.push(("codestral".into(), CODESTRAL.cloud_config().unwrap()));
        kandidaten.push(("ministral".into(), MINISTRAL.cloud_config().unwrap()));
        for (name, cfg) in kandidaten {
            let t = std::time::Instant::now();
            let r = crate::cloud_engine::execute_provider_request_multimodal(&cfg, &aufgabe, &[], 3000, Duration::from_secs(150));
            let ms = t.elapsed().as_millis();
            match r {
                Ok(v) => {
                    println!("KAND {name} ok ms={ms} finish={:?} chars={}", v.finish_reason, v.text.len());
                    let _ = std::fs::write(format!("{out}/mittel__{name}.txt"), v.text);
                }
                Err(e) => println!("KAND {name} FAIL ms={ms} err={}", e.to_string().chars().take(200).collect::<String>()),
            }
        }
    }

    /// Builder-protocol compliance: first step of a real build prompt must be
    /// exactly one JSON tool call (fs.write with full content).
    #[test]
    #[ignore = "live builder protocol check (network)"]
    fn live_builder_protokoll() {
        let out = std::env::var("NOKI_BENCH_OUT").expect("NOKI_BENCH_OUT");
        let prompt = crate::code_agent::builder_testprompt("Erstelle eine kleine 3D-Szene mit einem drehenden Würfel auf einer Fläche (Three.js).");
        let basis = DEEPSEEK.cloud_config().expect("openrouter config");
        let mut kandidaten: Vec<(String, crate::cloud_engine::CloudProviderConfig)> = ["nvidia/nemotron-3-super-120b-a12b:free", "qwen/qwen3.8-27b:free"]
            .iter()
            .map(|m| { let mut c = basis.clone(); c.model = m.to_string(); (m.replace(['/', ':'], "_"), c) })
            .collect();
        kandidaten.push(("codestral".into(), CODESTRAL.cloud_config().unwrap()));
        for (name, cfg) in kandidaten {
            let t = std::time::Instant::now();
            match crate::cloud_engine::execute_provider_request_multimodal(&cfg, &prompt, &[], 6000, Duration::from_secs(150)) {
                Ok(v) => {
                    let ok = crate::code_agent::builder_antwort_gueltig(&v.text);
                    println!("PROTO {name} ms={} gueltig={ok} chars={} finish={:?}", t.elapsed().as_millis(), v.text.len(), v.finish_reason);
                    let _ = std::fs::write(format!("{out}/proto__{name}.txt"), v.text);
                }
                Err(e) => println!("PROTO {name} FAIL {}", e.to_string().chars().take(160).collect::<String>()),
            }
        }
    }

    /// Live dry run of the real builder loop: full brief, real Codestral
    /// calls, on a scratch copy (NOKI_BUILDER_ROOT). Preview = `node --check`
    /// of every JS file. Shows format compliance and edit success per step.
    #[test]
    #[ignore = "live builder dry run (network, scratch copy only)"]
    fn live_builder_trockenlauf() {
        let root = std::path::PathBuf::from(std::env::var("NOKI_BUILDER_ROOT").expect("NOKI_BUILDER_ROOT"));
        let schritte: usize = std::env::var("NOKI_BUILDER_SCHRITTE").ok().and_then(|x| x.parse().ok()).unwrap_or(8);
        let brief = include_str!("../tests/fixtures/gt3_rs_funktional_auftrag.txt");
        let verlauf = std::env::var("NOKI_BUILDER_VERLAUF").unwrap_or_default();
        let cfg = CODESTRAL.cloud_config().expect("codestral");
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let r = crate::code_agent::run_builder(
            &root,
            brief,
            &verlauf,
            crate::code_agent::CodeStyle::Functional,
            crate::code_agent::BauBudget { schritte, pruefrunden: 1 },
            |prompt, max, _| {
                let t = std::time::Instant::now();
                let v = crate::cloud_engine::execute_provider_request_multimodal(&cfg, prompt, &[], max as u32, Duration::from_secs(150))
                    .map_err(|e| e.to_string())?;
                println!("GEN prompt={} out={} ms={} finish={:?}", prompt.chars().count(), v.text.chars().count(), t.elapsed().as_millis(), v.finish_reason);
                if let Ok(d) = std::env::var("NOKI_BUILDER_RAW") {
                    let n = std::fs::read_dir(&d).map(|x| x.count()).unwrap_or(0);
                    let _ = std::fs::write(format!("{d}/gen{n:02}.txt"), &v.text);
                }
                std::thread::sleep(Duration::from_millis(1200));
                Ok(v.text)
            },
            |a| println!("STEP {} ok={} {} +{} -{} | {}", a.label, a.ok, a.datei, a.plus, a.minus, a.detail.chars().take(160).collect::<String>().replace('\n', " ")),
            || {
                let mut fehler = Vec::new();
                for f in crate::code_agent::projekt_dateien(&root) {
                    let name = f.split(" (").next().unwrap_or("").to_string();
                    if name.ends_with(".js") {
                        let o = std::process::Command::new("node").arg("--check").arg(root.join(&name)).output().map_err(|e| e.to_string())?;
                        if !o.status.success() {
                            let err = String::from_utf8_lossy(&o.stderr).to_string();
                            let zeile: String = err.lines().next().and_then(|l| l.rsplit(':').next()).unwrap_or("").chars().filter(|c| c.is_ascii_digit()).collect();
                            let msg = err.lines().find(|l| l.contains("Error")).unwrap_or("SyntaxError").to_string();
                            fehler.push(format!("{msg} ({name}:{zeile})"));
                        }
                    }
                }
                if fehler.is_empty() { Ok("Trockenlauf: Syntax OK (keine echte Vorschau).".into()) } else { Ok(format!("FEHLER ({}): - {}", fehler.len(), fehler.join("\n- "))) }
            },
            &cancel,
        );
        match r {
            Ok(run) => println!("ENDE ok iterationen={} antwort={}", run.iterations, run.answer.chars().take(300).collect::<String>()),
            Err(e) => println!("ENDE fehler {e}"),
        }
    }

    /// Live health: ONE tiny real request per configured cloud model
    /// (max 16 output tokens). Prints provider, HTTP, latency, error class,
    /// rate-limit headers and the router's current runtime state.
    #[test]
    #[ignore = "live provider health (network, tiny requests)"]
    fn live_provider_health() {
        for entry in catalog() {
            if entry.is_local() {
                continue;
            }
            let key = LiveCredentials.has_secret(entry.env_var);
            let state = provider_state(entry.id);
            if !key {
                println!("HEALTH {} provider={} model={} key=NO state={:?}", entry.id, entry.provider, entry.model, state);
                continue;
            }
            let Some(cfg) = entry.cloud_config() else {
                println!("HEALTH {} no cloud config", entry.id);
                continue;
            };
            let t = std::time::Instant::now();
            let r = crate::cloud_engine::execute_provider_request_multimodal(
                &cfg,
                "Antworte nur mit dem Wort OK.",
                &[],
                16,
                Duration::from_secs(40),
            );
            let ms = t.elapsed().as_millis();
            match r {
                Ok(resp) => println!(
                    "HEALTH {} provider={} model={} key=yes state={:?} OK http={:?} ms={} finish={:?} text={:?} ratelimit={:?}",
                    entry.id, entry.provider, entry.model, state, resp.http_status, ms, resp.finish_reason,
                    resp.text.chars().take(40).collect::<String>(),
                    resp.rate_limit_headers
                ),
                Err(e) => println!(
                    "HEALTH {} provider={} model={} key=yes state={:?} FAIL ms={} err={}",
                    entry.id, entry.provider, entry.model, state, ms, e.to_string().chars().take(300).collect::<String>()
                ),
            }
        }
    }
    /// Read-only: provider states and the ranking the router WOULD choose.
    /// No model call, no network. Keychain presence only (no secret printed).
    #[test]
    #[ignore = "live router diagnosis (keychain presence)"]
    fn live_router_status() {
        println!("STATUS {}", serde_json::to_string_pretty(&router_status_json(EngineMode::LocalAndCloud, &LiveCredentials)).unwrap());
        for (class, tier, label) in [
            (TaskClass::Work, Tier::Fast, "work-fast"),
            (TaskClass::Work, Tier::Normal, "work-normal"),
            (TaskClass::Work, Tier::Deep, "work-deep"),
            (TaskClass::Coding, Tier::Normal, "coding-normal"),
            (TaskClass::Coding, Tier::Deep, "coding-deep"),
        ] {
            let prompt = "Schreibe einen Text über Sushi.";
            let mut r = RouteRequest::new(1, class, tier, prompt);
            r.max_tokens = 1800;
            r.engine_mode = EngineMode::LocalAndCloud;
            let rep = shadow_rank(&r);
            println!("RANK {label}: lane={:?} gate={:?} selected={:?}", rep.execution_lane, rep.privacy_gate, rep.selected_candidate);
            println!("  chain={:?}", rep.static_chain);
            for c in rep.rejected.iter().take(12) { println!("  rejected {:?}", c); }
        }
    }
}

