//! Canonical model metadata and stable routing roles.
//!
//! This module is deliberately execution-free: it never calls a provider,
//! probes credentials, runs a benchmark, or changes the productive router.
//! Existing subsystems consume its definitions through small adapters while
//! their runtime state and static fallback behaviour remain where they are.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Immutable request/cost boundaries
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExecutionLane {
    Local,
    FreeCloud,
    PaidCloud,
}

impl ExecutionLane {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "LOCAL",
            Self::FreeCloud => "FREE_CLOUD",
            Self::PaidCloud => "PAID_CLOUD",
        }
    }

    pub fn allows(self, cost: CostSafety) -> bool {
        matches!(
            (self, cost),
            (Self::Local, CostSafety::Local)
                | (Self::FreeCloud, CostSafety::Local)
                | (Self::FreeCloud, CostSafety::VerifiedFreeHardStop)
                | (Self::PaidCloud, CostSafety::Local)
                | (Self::PaidCloud, CostSafety::MeteredPaid)
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostSafety {
    Local,
    VerifiedFreeHardStop,
    FreeButBillingUncertain,
    MeteredPaid,
}

impl CostSafety {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::VerifiedFreeHardStop => "verified_free_hard_stop",
            Self::FreeButBillingUncertain => "free_but_billing_uncertain",
            Self::MeteredPaid => "metered_paid",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalModel {
    Qwen9B,
    Qwen4B,
    JackOd9BNative,
}

impl LocalModel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Qwen9B => "qwen_3.5_9b_local",
            Self::Qwen4B => "qwen_3.5_4b_local",
            Self::JackOd9BNative => "jackod_9b_noki_native",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Qwen9B => "Qwen 3.5 9B (lokal)",
            Self::Qwen4B => "Qwen 3.5 4B (lokal)",
            Self::JackOd9BNative => "JackOD 9B via Noki Native",
        }
    }

    /// Packaging metadata for the local artifacts; it is display-only and
    /// deliberately has no effect on routing or model selection.
    pub const fn quantization(self) -> &'static str {
        match self {
            Self::Qwen9B | Self::Qwen4B | Self::JackOd9BNative => "Q4_K_M",
        }
    }
}

// ---------------------------------------------------------------------------
// Canonical metadata
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelLifecycle {
    Active,
    Candidate,
    Parked,
    Unavailable,
    Retired,
}

impl ModelLifecycle {
    pub const fn legacy_status(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Candidate => "candidate_pending_canonical",
            Self::Parked => "parked",
            Self::Unavailable => "unavailable",
            Self::Retired => "retired",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkStatus {
    /// Productive bootstrap evidence predating benchmark-v2 metadata.
    LegacyProductive,
    CandidatePendingCanonical,
    CanonicalV2,
    Missing,
}

impl BenchmarkStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LegacyProductive => "legacy_productive",
            Self::CandidatePendingCanonical => "candidate_pending_canonical",
            Self::CanonicalV2 => "canonical_v2",
            Self::Missing => "missing",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub context_tokens: u32,
    pub reasoning: bool,
    pub tools: bool,
    pub structured_output: bool,
    pub files: bool,
    pub vision: bool,
    pub streaming: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct BenchmarkProfile {
    pub work_quality: Option<f32>,
    pub work_latency_ms: Option<u64>,
    pub code_quality: Option<f32>,
    pub code_latency_ms: Option<u64>,
    pub code_tests_passed: Option<&'static str>,
    pub availability: Option<f32>,
    pub completion: Option<f32>,
    pub note: Option<&'static str>,
}

pub const NOT_MEASURED: BenchmarkProfile = BenchmarkProfile {
    work_quality: None,
    work_latency_ms: None,
    code_quality: None,
    code_latency_ms: None,
    code_tests_passed: None,
    availability: None,
    completion: None,
    note: None,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestProtocol {
    OpenAiCompatible,
    GeminiGenerateContent,
    AnthropicMessages,
    OfficialCli,
    LocalRuntime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthRequirement {
    None,
    EnvironmentOrKeychainReference,
    ExistingCliSession,
}

#[derive(Clone, Copy, Debug)]
pub struct ConnectionIdentity {
    pub id: &'static str,
    pub env_var: &'static str,
    pub endpoint: &'static str,
    pub protocol: RequestProtocol,
    pub disable_reasoning: bool,
    /// Compatibility key used by cloud_engine.rs during migration.
    pub cloud_engine_id: Option<&'static str>,
}

impl ConnectionIdentity {
    pub const fn auth_requirement(self) -> AuthRequirement {
        match self.protocol {
            RequestProtocol::LocalRuntime => AuthRequirement::None,
            RequestProtocol::OfficialCli => AuthRequirement::ExistingCliSession,
            _ => AuthRequirement::EnvironmentOrKeychainReference,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CanonicalModelDefinition {
    pub provider_id: &'static str,
    pub canonical_model_id: &'static str,
    pub display_name: &'static str,
    /// Exact provider model slug, including a free/version suffix where used.
    pub exact_model_version: &'static str,
    pub connection: ConnectionIdentity,
    pub execution_lane: ExecutionLane,
    pub cost_safety: CostSafety,
    pub enabled: bool,
    pub capabilities: ModelCapabilities,
    pub benchmark_status: BenchmarkStatus,
    pub benchmark_profile: BenchmarkProfile,
    pub lifecycle: ModelLifecycle,
    pub local_model: Option<LocalModel>,
    pub specialist_only: bool,
}

const OPENROUTER_ENDPOINT: &str = "https://openrouter.ai/api/v1/chat/completions";
const MISTRAL_ENDPOINT: &str = "https://api.mistral.ai/v1/chat/completions";
const GROQ_ENDPOINT: &str = "https://api.groq.com/openai/v1/chat/completions";
const CLOUDFLARE_GLM_ENDPOINT_TEMPLATE: &str =
    "https://api.cloudflare.com/client/v4/accounts/{account_id}/ai/v1/chat/completions";
const GEMINI_ENDPOINT: &str =
    "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.8-flash:generateContent";

const LOCAL_CAPS: ModelCapabilities = ModelCapabilities {
    context_tokens: 32_768,
    reasoning: true,
    tools: true,
    structured_output: true,
    files: true,
    vision: false,
    streaming: true,
};

const OPENAI_FREE_CAPS: ModelCapabilities = ModelCapabilities {
    context_tokens: 32_768,
    reasoning: true,
    tools: false,
    structured_output: true,
    files: false,
    vision: false,
    streaming: true,
};

pub const NEX_N2_5_PRO: CanonicalModelDefinition = CanonicalModelDefinition {
    provider_id: "openrouter",
    canonical_model_id: "openrouter_nex_n2_5_pro",
    display_name: "Nex-N2.5 Pro Free (OpenRouter)",
    exact_model_version: "nex-agi/nex-n2.5-pro:free",
    connection: ConnectionIdentity {
        id: "openrouter_free_no_reasoning",
        env_var: "OPENROUTER_API_KEY",
        endpoint: OPENROUTER_ENDPOINT,
        protocol: RequestProtocol::OpenAiCompatible,
        disable_reasoning: true,
        cloud_engine_id: Some("openrouter_free"),
    },
    execution_lane: ExecutionLane::FreeCloud,
    cost_safety: CostSafety::VerifiedFreeHardStop,
    enabled: true,
    capabilities: ModelCapabilities {
        context_tokens: 262_144,
        reasoning: true,
        tools: true,
        structured_output: true,
        files: true,
        vision: true,
        streaming: true,
    },
    benchmark_status: BenchmarkStatus::CandidatePendingCanonical,
    benchmark_profile: BenchmarkProfile {
        work_quality: Some(99.8),
        work_latency_ms: Some(694),
        code_quality: Some(85.0),
        code_latency_ms: Some(553),
        code_tests_passed: Some("6/8"),
        availability: Some(100.0),
        completion: Some(100.0),
        note: Some("candidate_pending_canonical; compact_no_reasoning_benchmark"),
    },
    lifecycle: ModelLifecycle::Candidate,
    local_model: None,
    specialist_only: false,
};

pub const NEMOTRON_3_ULTRA: CanonicalModelDefinition = CanonicalModelDefinition {
    provider_id: "openrouter",
    canonical_model_id: "openrouter_nemotron_3_ultra",
    display_name: "Nemotron 3 Ultra Free (OpenRouter)",
    exact_model_version: "nvidia/nemotron-3-ultra-550b-a55b:free",
    connection: ConnectionIdentity {
        id: "openrouter_free_no_reasoning",
        env_var: "OPENROUTER_API_KEY",
        endpoint: OPENROUTER_ENDPOINT,
        protocol: RequestProtocol::OpenAiCompatible,
        disable_reasoning: true,
        cloud_engine_id: Some("openrouter_nemotron"),
    },
    execution_lane: ExecutionLane::FreeCloud,
    cost_safety: CostSafety::VerifiedFreeHardStop,
    enabled: true,
    capabilities: ModelCapabilities {
        context_tokens: 1_000_000,
        reasoning: true,
        tools: true,
        structured_output: true,
        files: true,
        vision: false,
        streaming: true,
    },
    benchmark_status: BenchmarkStatus::CandidatePendingCanonical,
    benchmark_profile: BenchmarkProfile {
        work_quality: Some(100.0),
        work_latency_ms: Some(384),
        code_quality: Some(82.9),
        code_latency_ms: Some(408),
        code_tests_passed: Some("5/7"),
        availability: Some(87.5),
        completion: Some(87.5),
        note: Some(
            "candidate_pending_canonical; compact_no_reasoning_benchmark; ultra_fast_candidate",
        ),
    },
    lifecycle: ModelLifecycle::Candidate,
    local_model: None,
    specialist_only: false,
};

pub const DEEPSEEK: CanonicalModelDefinition = CanonicalModelDefinition {
    provider_id: "openrouter",
    canonical_model_id: "openrouter_deepseek_v4_flash",
    display_name: "DeepSeek V4 Flash Free (OpenRouter)",
    exact_model_version: "deepseek/deepseek-v4-flash-0731:free",
    connection: ConnectionIdentity {
        id: "openrouter_free_no_reasoning",
        env_var: "OPENROUTER_API_KEY",
        endpoint: OPENROUTER_ENDPOINT,
        protocol: RequestProtocol::OpenAiCompatible,
        disable_reasoning: true,
        cloud_engine_id: Some("openrouter_deepseek"),
    },
    execution_lane: ExecutionLane::FreeCloud,
    cost_safety: CostSafety::VerifiedFreeHardStop,
    enabled: true,
    capabilities: ModelCapabilities {
        context_tokens: 1_048_576,
        reasoning: true,
        tools: true,
        structured_output: true,
        files: false,
        vision: false,
        streaming: true,
    },
    benchmark_status: BenchmarkStatus::CanonicalV2,
    benchmark_profile: BenchmarkProfile {
        work_quality: Some(91.9),
        work_latency_ms: Some(988),
        code_quality: Some(83.2),
        code_latency_ms: Some(1066),
        code_tests_passed: Some("6/8"),
        availability: Some(100.0),
        completion: Some(100.0),
        note: Some("benchmark-v2; reasoning=none required; legacy measurements retained"),
    },
    lifecycle: ModelLifecycle::Active,
    local_model: None,
    specialist_only: false,
};

pub const MINISTRAL: CanonicalModelDefinition = CanonicalModelDefinition {
    provider_id: "mistral",
    canonical_model_id: "mistral_ministral_8b",
    display_name: "Ministral 8B Free (Mistral)",
    exact_model_version: "ministral-8b-latest",
    connection: ConnectionIdentity {
        id: "mistral_free",
        env_var: "MISTRAL_API_KEY",
        endpoint: MISTRAL_ENDPOINT,
        protocol: RequestProtocol::OpenAiCompatible,
        disable_reasoning: false,
        cloud_engine_id: None,
    },
    execution_lane: ExecutionLane::FreeCloud,
    cost_safety: CostSafety::VerifiedFreeHardStop,
    enabled: true,
    capabilities: OPENAI_FREE_CAPS,
    benchmark_status: BenchmarkStatus::CanonicalV2,
    benchmark_profile: BenchmarkProfile {
        work_quality: Some(92.4),
        work_latency_ms: Some(1589),
        code_quality: None,
        code_latency_ms: None,
        code_tests_passed: None,
        availability: Some(100.0),
        completion: Some(100.0),
        note: Some("benchmark-v2 Work; legacy latency retained"),
    },
    lifecycle: ModelLifecycle::Active,
    local_model: None,
    specialist_only: false,
};

pub const CODESTRAL: CanonicalModelDefinition = CanonicalModelDefinition {
    provider_id: "mistral",
    canonical_model_id: "mistral_codestral",
    display_name: "Codestral Free (Mistral)",
    exact_model_version: "codestral-latest",
    connection: ConnectionIdentity {
        id: "mistral_free",
        env_var: "MISTRAL_API_KEY",
        endpoint: MISTRAL_ENDPOINT,
        protocol: RequestProtocol::OpenAiCompatible,
        disable_reasoning: false,
        cloud_engine_id: None,
    },
    execution_lane: ExecutionLane::FreeCloud,
    cost_safety: CostSafety::VerifiedFreeHardStop,
    enabled: true,
    capabilities: OPENAI_FREE_CAPS,
    benchmark_status: BenchmarkStatus::CanonicalV2,
    benchmark_profile: BenchmarkProfile {
        work_quality: None,
        work_latency_ms: None,
        code_quality: Some(77.2),
        code_latency_ms: Some(2545),
        code_tests_passed: Some("5/8"),
        availability: Some(100.0),
        completion: Some(100.0),
        note: Some("benchmark-v2 Coding; legacy latency retained"),
    },
    lifecycle: ModelLifecycle::Active,
    local_model: None,
    specialist_only: false,
};

pub const GEMINI: CanonicalModelDefinition = CanonicalModelDefinition {
    provider_id: "google",
    canonical_model_id: "google_gemini_3_8_flash",
    display_name: "Gemini 3.8 Flash Free (Google)",
    exact_model_version: "gemini-3.8-flash",
    connection: ConnectionIdentity {
        id: "gemini_free",
        env_var: "GEMINI_API_KEY",
        endpoint: GEMINI_ENDPOINT,
        protocol: RequestProtocol::GeminiGenerateContent,
        disable_reasoning: false,
        cloud_engine_id: Some("gemini_free"),
    },
    execution_lane: ExecutionLane::FreeCloud,
    cost_safety: CostSafety::VerifiedFreeHardStop,
    enabled: true,
    capabilities: ModelCapabilities {
        context_tokens: 1_000_000,
        reasoning: true,
        tools: true,
        structured_output: true,
        files: true,
        vision: true,
        streaming: true,
    },
    benchmark_status: BenchmarkStatus::LegacyProductive,
    benchmark_profile: BenchmarkProfile {
        work_quality: Some(88.0),
        work_latency_ms: Some(4167),
        code_quality: Some(100.0),
        code_latency_ms: Some(4864),
        code_tests_passed: Some("3/8"),
        availability: Some(50.0),
        completion: Some(50.0),
        note: Some("quality when answered; coding sample only 3 answered tasks"),
    },
    lifecycle: ModelLifecycle::Active,
    local_model: None,
    specialist_only: true,
};

pub const QWEN_9B: CanonicalModelDefinition = CanonicalModelDefinition {
    provider_id: "local",
    canonical_model_id: "local_qwen_9b",
    display_name: "Qwen 3.5 9B (lokal)",
    exact_model_version: "qwen3.5:9b",
    connection: ConnectionIdentity {
        id: "local_runtime",
        env_var: "",
        endpoint: "",
        protocol: RequestProtocol::LocalRuntime,
        disable_reasoning: false,
        cloud_engine_id: None,
    },
    execution_lane: ExecutionLane::Local,
    cost_safety: CostSafety::Local,
    enabled: true,
    capabilities: LOCAL_CAPS,
    benchmark_status: BenchmarkStatus::CanonicalV2,
    benchmark_profile: BenchmarkProfile {
        work_quality: Some(97.8),
        work_latency_ms: Some(7056),
        code_quality: None,
        code_latency_ms: None,
        code_tests_passed: None,
        availability: Some(100.0),
        completion: Some(100.0),
        note: Some("benchmark-v2 Work; legacy latency retained; offline"),
    },
    lifecycle: ModelLifecycle::Active,
    local_model: Some(LocalModel::Qwen9B),
    specialist_only: false,
};

pub const QWEN_4B: CanonicalModelDefinition = CanonicalModelDefinition {
    provider_id: "local",
    canonical_model_id: "local_qwen_4b",
    display_name: "Qwen 3.5 4B (lokal)",
    exact_model_version: "qwen3.5:4b",
    connection: ConnectionIdentity {
        id: "local_runtime",
        env_var: "",
        endpoint: "",
        protocol: RequestProtocol::LocalRuntime,
        disable_reasoning: false,
        cloud_engine_id: None,
    },
    execution_lane: ExecutionLane::Local,
    cost_safety: CostSafety::Local,
    enabled: true,
    capabilities: LOCAL_CAPS,
    benchmark_status: BenchmarkStatus::LegacyProductive,
    benchmark_profile: BenchmarkProfile {
        work_quality: Some(83.8),
        work_latency_ms: Some(4364),
        code_quality: None,
        code_latency_ms: None,
        code_tests_passed: None,
        availability: Some(100.0),
        completion: Some(100.0),
        note: Some("W1-W4 latency measured; offline"),
    },
    lifecycle: ModelLifecycle::Active,
    local_model: Some(LocalModel::Qwen4B),
    specialist_only: false,
};

pub const JACKOD: CanonicalModelDefinition = CanonicalModelDefinition {
    provider_id: "local",
    canonical_model_id: "local_jackod_9b",
    display_name: "JackOD 9B via Noki Native",
    exact_model_version: "mannix/JackOD-9B-Coder:Q4_K_M",
    connection: ConnectionIdentity {
        id: "local_runtime",
        env_var: "",
        endpoint: "",
        protocol: RequestProtocol::LocalRuntime,
        disable_reasoning: false,
        cloud_engine_id: None,
    },
    execution_lane: ExecutionLane::Local,
    cost_safety: CostSafety::Local,
    enabled: true,
    capabilities: LOCAL_CAPS,
    benchmark_status: BenchmarkStatus::CanonicalV2,
    benchmark_profile: BenchmarkProfile {
        work_quality: None,
        work_latency_ms: None,
        code_quality: Some(68.6),
        code_latency_ms: Some(11026),
        code_tests_passed: Some("6/8"),
        availability: Some(100.0),
        completion: Some(100.0),
        note: Some("benchmark-v2 Coding; legacy latency retained; offline end-to-end (11.0s avg)"),
    },
    lifecycle: ModelLifecycle::Active,
    local_model: Some(LocalModel::JackOd9BNative),
    specialist_only: false,
};

/// Compatibility-only records still surfaced by cloud_engine.rs. They are not
/// role champions and cannot become productive without canonical promotion.
pub const GROQ_QWEN: CanonicalModelDefinition = CanonicalModelDefinition {
    provider_id: "groq",
    canonical_model_id: "groq_qwen_3_8_27b",
    display_name: "Qwen 3.8 27B (Groq)",
    exact_model_version: "qwen/qwen3.8-27b",
    connection: ConnectionIdentity {
        id: "groq_free",
        env_var: "GROQ_API_KEY",
        endpoint: GROQ_ENDPOINT,
        protocol: RequestProtocol::OpenAiCompatible,
        disable_reasoning: false,
        cloud_engine_id: Some("groq_qwen"),
    },
    execution_lane: ExecutionLane::FreeCloud,
    cost_safety: CostSafety::VerifiedFreeHardStop,
    enabled: false,
    capabilities: ModelCapabilities {
        context_tokens: 262_144,
        reasoning: true,
        tools: true,
        structured_output: true,
        files: true,
        vision: true,
        streaming: true,
    },
    benchmark_status: BenchmarkStatus::Missing,
    benchmark_profile: NOT_MEASURED,
    lifecycle: ModelLifecycle::Parked,
    local_model: None,
    specialist_only: false,
};

pub const GROQ_GPT_OSS: CanonicalModelDefinition = CanonicalModelDefinition {
    provider_id: "groq",
    canonical_model_id: "groq_gpt_oss_120b",
    display_name: "GPT-OSS 120B (Groq)",
    exact_model_version: "openai/gpt-oss-120b",
    connection: ConnectionIdentity {
        id: "groq_free",
        env_var: "GROQ_API_KEY",
        endpoint: GROQ_ENDPOINT,
        protocol: RequestProtocol::OpenAiCompatible,
        disable_reasoning: false,
        cloud_engine_id: Some("groq_gpt_oss"),
    },
    execution_lane: ExecutionLane::FreeCloud,
    cost_safety: CostSafety::VerifiedFreeHardStop,
    enabled: true,
    capabilities: ModelCapabilities {
        context_tokens: 128_000,
        reasoning: true,
        tools: true,
        structured_output: true,
        files: true,
        vision: false,
        streaming: true,
    },
    benchmark_status: BenchmarkStatus::CanonicalV2,
    benchmark_profile: BenchmarkProfile {
        work_quality: Some(100.0),
        // No v2 latency is claimed here. The dynamic FAST score uses runtime
        // p95 telemetry once available; this retained value is only the
        // benchmark-identity completeness marker.
        work_latency_ms: Some(0),
        code_quality: Some(77.5),
        code_latency_ms: Some(0),
        code_tests_passed: None,
        availability: Some(100.0),
        completion: Some(100.0),
        note: Some("benchmark-v2; Groq free-tier attestation must be fresh"),
    },
    lifecycle: ModelLifecycle::Active,
    local_model: None,
    specialist_only: false,
};

pub const CLOUDFLARE_GLM_4_7_FLASH: CanonicalModelDefinition = CanonicalModelDefinition {
    provider_id: "cloudflare_workers_ai",
    canonical_model_id: "cloudflare_glm_4_7_flash",
    display_name: "GLM-4.7-Flash Free (Cloudflare Workers AI)",
    exact_model_version: "@cf/zai-org/glm-4.7-flash",
    connection: ConnectionIdentity {
        id: "cloudflare_glm_4_7_flash",
        env_var: "CLOUDFLARE_API_TOKEN",
        endpoint: CLOUDFLARE_GLM_ENDPOINT_TEMPLATE,
        protocol: RequestProtocol::OpenAiCompatible,
        disable_reasoning: true,
        cloud_engine_id: None,
    },
    execution_lane: ExecutionLane::FreeCloud,
    cost_safety: CostSafety::VerifiedFreeHardStop,
    enabled: true,
    capabilities: ModelCapabilities {
        context_tokens: 131_072,
        reasoning: true,
        tools: true,
        structured_output: true,
        files: false,
        vision: false,
        streaming: true,
    },
    benchmark_status: BenchmarkStatus::CanonicalV2,
    benchmark_profile: BenchmarkProfile {
        work_quality: Some(96.0),
        work_latency_ms: Some(0),
        code_quality: Some(82.1),
        code_latency_ms: Some(0),
        code_tests_passed: None,
        availability: Some(100.0),
        completion: Some(100.0),
        note: Some("benchmark-v2; Workers Free hard-stop remains required"),
    },
    lifecycle: ModelLifecycle::Active,
    local_model: None,
    specialist_only: false,
};

pub const MISTRAL_SMALL_LEGACY: CanonicalModelDefinition = CanonicalModelDefinition {
    provider_id: "mistral",
    canonical_model_id: "mistral_small_legacy_cloud_engine",
    display_name: "Mistral Small (Mistral)",
    exact_model_version: "mistral-small-latest",
    connection: ConnectionIdentity {
        id: "mistral_free",
        env_var: "MISTRAL_API_KEY",
        endpoint: MISTRAL_ENDPOINT,
        protocol: RequestProtocol::OpenAiCompatible,
        disable_reasoning: false,
        cloud_engine_id: Some("mistral_free"),
    },
    execution_lane: ExecutionLane::FreeCloud,
    cost_safety: CostSafety::VerifiedFreeHardStop,
    enabled: true,
    capabilities: ModelCapabilities {
        context_tokens: 32_768,
        reasoning: false,
        tools: true,
        structured_output: true,
        files: true,
        vision: false,
        streaming: true,
    },
    benchmark_status: BenchmarkStatus::LegacyProductive,
    benchmark_profile: NOT_MEASURED,
    lifecycle: ModelLifecycle::Active,
    local_model: None,
    specialist_only: false,
};

pub const MODELS: &[CanonicalModelDefinition] = &[
    NEX_N2_5_PRO,
    NEMOTRON_3_ULTRA,
    DEEPSEEK,
    MINISTRAL,
    CODESTRAL,
    GEMINI,
    QWEN_9B,
    QWEN_4B,
    JACKOD,
    GROQ_QWEN,
    GROQ_GPT_OSS,
    CLOUDFLARE_GLM_4_7_FLASH,
    MISTRAL_SMALL_LEGACY,
];

const CLOUD_ENGINE_MODELS: &[CanonicalModelDefinition] = &[
    GEMINI,
    GROQ_QWEN,
    GROQ_GPT_OSS,
    NEX_N2_5_PRO,
    NEMOTRON_3_ULTRA,
    DEEPSEEK,
    MISTRAL_SMALL_LEGACY,
];

pub fn model(id: &str) -> Option<&'static CanonicalModelDefinition> {
    MODELS.iter().find(|entry| entry.canonical_model_id == id)
}

pub fn cloud_engine_models() -> impl Iterator<Item = &'static CanonicalModelDefinition> {
    CLOUD_ENGINE_MODELS.iter()
}

pub fn model_by_cloud_engine_id(id: &str) -> Option<&'static CanonicalModelDefinition> {
    CLOUD_ENGINE_MODELS
        .iter()
        .find(|model| model.connection.cloud_engine_id == Some(id))
}

pub fn local_models() -> impl Iterator<Item = &'static CanonicalModelDefinition> {
    MODELS
        .iter()
        .filter(|model| model.execution_lane == ExecutionLane::Local)
}

/// Existing specialist connections are provider-level and do not pin an exact
/// model version. They therefore live beside, not inside, the model catalog.
/// Their routes remain in specialist.rs until exact models are configured.
#[derive(Clone, Copy, Debug)]
pub struct SpecialistConnectionDefinition {
    pub provider_id: &'static str,
    pub connection_id: &'static str,
    pub display_name: &'static str,
    pub env_var: &'static str,
    pub endpoint: &'static str,
    pub protocol: RequestProtocol,
    pub execution_lane: ExecutionLane,
    pub cost_safety: CostSafety,
    pub enabled: bool,
    pub capabilities: ModelCapabilities,
    pub lifecycle: ModelLifecycle,
}

pub const OPENAI_SPECIALIST: SpecialistConnectionDefinition = SpecialistConnectionDefinition {
    provider_id: "openai",
    connection_id: "openai_official_api",
    display_name: "OpenAI",
    env_var: "OPENAI_API_KEY",
    endpoint: "https://api.openai.com/v1/chat/completions",
    protocol: RequestProtocol::OpenAiCompatible,
    execution_lane: ExecutionLane::PaidCloud,
    cost_safety: CostSafety::MeteredPaid,
    enabled: false,
    capabilities: ModelCapabilities {
        context_tokens: 128_000,
        reasoning: true,
        tools: true,
        structured_output: true,
        files: true,
        vision: true,
        streaming: true,
    },
    lifecycle: ModelLifecycle::Unavailable,
};

pub const ANTHROPIC_SPECIALIST: SpecialistConnectionDefinition = SpecialistConnectionDefinition {
    provider_id: "anthropic",
    connection_id: "anthropic_official_api",
    display_name: "Claude",
    env_var: "ANTHROPIC_API_KEY",
    endpoint: "https://api.anthropic.com/v1/messages",
    protocol: RequestProtocol::AnthropicMessages,
    execution_lane: ExecutionLane::PaidCloud,
    cost_safety: CostSafety::MeteredPaid,
    enabled: false,
    capabilities: ModelCapabilities {
        context_tokens: 200_000,
        reasoning: true,
        tools: true,
        structured_output: true,
        files: true,
        vision: true,
        streaming: true,
    },
    lifecycle: ModelLifecycle::Unavailable,
};

pub const CLAUDE_CLI_SPECIALIST: SpecialistConnectionDefinition = SpecialistConnectionDefinition {
    provider_id: "claude_cli",
    connection_id: "claude_official_cli",
    display_name: "Claude CLI",
    env_var: "",
    endpoint: "",
    protocol: RequestProtocol::OfficialCli,
    execution_lane: ExecutionLane::PaidCloud,
    // The user's external subscription is not a verified free hard stop.
    cost_safety: CostSafety::FreeButBillingUncertain,
    enabled: false,
    capabilities: ModelCapabilities {
        context_tokens: 200_000,
        reasoning: true,
        tools: false,
        structured_output: true,
        files: true,
        vision: false,
        streaming: false,
    },
    lifecycle: ModelLifecycle::Unavailable,
};

pub fn specialist_connection(id: &str) -> Option<&'static SpecialistConnectionDefinition> {
    [
        &OPENAI_SPECIALIST,
        &ANTHROPIC_SPECIALIST,
        &CLAUDE_CLI_SPECIALIST,
    ]
    .into_iter()
    .find(|connection| connection.provider_id == id)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParkedReason {
    BenchmarkPending,
    RateLimited,
    FreeEndpointMissing,
    CostUncertain,
    CredentialsRequired,
    ManuallyDisabled,
}

impl ParkedReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BenchmarkPending => "benchmark_pending",
            Self::RateLimited => "rate_limited",
            Self::FreeEndpointMissing => "free_endpoint_missing",
            Self::CostUncertain => "cost_uncertain",
            Self::CredentialsRequired => "credentials_required",
            Self::ManuallyDisabled => "manually_disabled",
        }
    }

    pub const fn legacy_runtime_label(self) -> &'static str {
        match self {
            Self::RateLimited => "rate_limited",
            Self::CostUncertain | Self::FreeEndpointMissing => "cost_uncertain",
            Self::CredentialsRequired | Self::BenchmarkPending => "credentials_required",
            Self::ManuallyDisabled => "disabled",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ParkedCatalogEntry {
    pub id: &'static str,
    pub display_name: &'static str,
    pub canonical_model_id: Option<&'static str>,
    pub connection_id: Option<&'static str>,
    pub reason: ParkedReason,
    pub note: &'static str,
}

pub const PARKED_CATALOG: &[ParkedCatalogEntry] = &[
    ParkedCatalogEntry {
        id: "groq",
        display_name: "Groq",
        canonical_model_id: Some(GROQ_QWEN.canonical_model_id),
        connection_id: Some(GROQ_QWEN.connection.id),
        reason: ParkedReason::CredentialsRequired,
        note: "Nie eingerichtet – kein Benchmark möglich.",
    },
    ParkedCatalogEntry {
        id: "cloudflare_workers_ai",
        display_name: "Cloudflare Workers AI",
        canonical_model_id: None,
        connection_id: Some("cloudflare_workers_ai_legacy"),
        reason: ParkedReason::CredentialsRequired,
        note: "Token wird von der API abgelehnt (401) – ungemessen.",
    },
    ParkedCatalogEntry {
        id: "openrouter_qwen_3_8_free",
        display_name: "Qwen 3.8 Free (OpenRouter)",
        canonical_model_id: None,
        connection_id: Some("openrouter_free_no_reasoning"),
        reason: ParkedReason::RateLimited,
        note: "Beim Tournament-Benchmark upstream 429 rate-limited – geparkt.",
    },
    ParkedCatalogEntry {
        id: "openrouter_glm_5_2_free",
        display_name: "GLM 5.2 Free (OpenRouter)",
        canonical_model_id: None,
        connection_id: Some("openrouter_free_no_reasoning"),
        reason: ParkedReason::RateLimited,
        note: "Beim Tournament-Benchmark upstream 429 rate-limited – geparkt.",
    },
    ParkedCatalogEntry {
        id: "openrouter_laguna_s_free",
        display_name: "Laguna S 2.1 Free (OpenRouter)",
        canonical_model_id: None,
        connection_id: Some("openrouter_free_no_reasoning"),
        reason: ParkedReason::RateLimited,
        note: "Beim Tournament-Benchmark upstream 429 rate-limited (40% Screening) – geparkt.",
    },
    ParkedCatalogEntry {
        id: "openrouter_kimi_k2_6",
        display_name: "Kimi K2.6 (OpenRouter)",
        canonical_model_id: None,
        connection_id: Some("openrouter_paid_unconfigured"),
        reason: ParkedReason::CostUncertain,
        note: "Nur als Paid-Modell verfügbar – $0-Anforderung nicht erfüllt, nicht aufgerufen.",
    },
];

// ---------------------------------------------------------------------------
// Stable roles and incumbent-neutral assignments
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingRole {
    WorkFast,
    WorkBalanced,
    WorkDeep,
    CodingFast,
    CodingNormal,
    CodingComplex,
    WorkLocalFloor,
    CodingLocalFloor,
}

#[derive(Clone, Copy, Debug)]
pub struct RolePlan {
    pub role: RoutingRole,
    pub champion: &'static str,
    pub challengers: &'static [&'static str],
    pub local_floor: &'static [&'static str],
}

const NONE: &[&str] = &[];
const WORK_FAST_CHALLENGERS: &[&str] = &[
    CLOUDFLARE_GLM_4_7_FLASH.canonical_model_id,
    MINISTRAL.canonical_model_id,
    DEEPSEEK.canonical_model_id,
];
const WORK_BALANCED_CHALLENGERS: &[&str] = &[
    CLOUDFLARE_GLM_4_7_FLASH.canonical_model_id,
    MINISTRAL.canonical_model_id,
    DEEPSEEK.canonical_model_id,
];
const WORK_DEEP_CHALLENGERS: &[&str] = WORK_BALANCED_CHALLENGERS;
const CODING_FAST_CHALLENGERS: &[&str] = &[
    CLOUDFLARE_GLM_4_7_FLASH.canonical_model_id,
    GROQ_GPT_OSS.canonical_model_id,
    CODESTRAL.canonical_model_id,
];
const CODING_CHALLENGERS: &[&str] = CODING_FAST_CHALLENGERS;
// Local Work default is the 9B for every tier; the 4B is only the runtime
// fallback inside NokiLocalModel::generate (9B unavailable/failed).
const WORK_FAST_FLOOR: &[&str] = &[QWEN_9B.canonical_model_id];
const WORK_FLOOR: &[&str] = &[QWEN_9B.canonical_model_id];
const WORK_LOCAL_FLOOR: &[&str] = &[QWEN_4B.canonical_model_id];
const CODING_FLOOR: &[&str] = &[JACKOD.canonical_model_id];
const CODING_LOCAL_FLOOR: &[&str] = &[JACKOD.canonical_model_id];

pub const ROLE_PLANS: &[RolePlan] = &[
    RolePlan {
        role: RoutingRole::WorkFast,
        // Dynamic FAST ranks this pool using task-specific quality,
        // reliability, and runtime p95; this field is not a FAST override.
        champion: GROQ_GPT_OSS.canonical_model_id,
        challengers: WORK_FAST_CHALLENGERS,
        local_floor: WORK_FAST_FLOOR,
    },
    RolePlan {
        role: RoutingRole::WorkBalanced,
        champion: GROQ_GPT_OSS.canonical_model_id,
        challengers: WORK_BALANCED_CHALLENGERS,
        local_floor: WORK_FLOOR,
    },
    RolePlan {
        role: RoutingRole::WorkDeep,
        champion: GROQ_GPT_OSS.canonical_model_id,
        challengers: WORK_DEEP_CHALLENGERS,
        local_floor: WORK_FLOOR,
    },
    RolePlan {
        role: RoutingRole::CodingFast,
        champion: DEEPSEEK.canonical_model_id,
        challengers: CODING_FAST_CHALLENGERS,
        local_floor: CODING_FLOOR,
    },
    RolePlan {
        role: RoutingRole::CodingNormal,
        champion: DEEPSEEK.canonical_model_id,
        challengers: CODING_CHALLENGERS,
        local_floor: CODING_FLOOR,
    },
    RolePlan {
        role: RoutingRole::CodingComplex,
        champion: DEEPSEEK.canonical_model_id,
        challengers: CODING_CHALLENGERS,
        local_floor: CODING_FLOOR,
    },
    RolePlan {
        role: RoutingRole::WorkLocalFloor,
        champion: QWEN_9B.canonical_model_id,
        challengers: NONE,
        local_floor: WORK_LOCAL_FLOOR,
    },
    RolePlan {
        role: RoutingRole::CodingLocalFloor,
        champion: JACKOD.canonical_model_id,
        challengers: NONE,
        local_floor: CODING_LOCAL_FLOOR,
    },
];

pub fn role_plan(role: RoutingRole) -> &'static RolePlan {
    ROLE_PLANS
        .iter()
        .find(|plan| plan.role == role)
        .expect("every stable role has a plan")
}

pub fn role_model_ids(role: RoutingRole) -> Vec<&'static str> {
    let plan = role_plan(role);
    std::iter::once(plan.champion)
        .chain(plan.challengers.iter().copied())
        .chain(plan.local_floor.iter().copied())
        .collect()
}

// ---------------------------------------------------------------------------
// Discovery and promotion policy (not connected to productive routing yet)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DiscoveryMetadata {
    pub external_quality_rank: Option<f32>,
    pub coding_rank: Option<f32>,
    pub general_work_rank: Option<f32>,
    pub task_fit: Vec<String>,
    pub context_tokens: Option<u32>,
    pub tools: Option<bool>,
    pub reasoning: Option<bool>,
    pub advertised_cost: Option<String>,
    pub last_verified: Option<u64>,
    pub discovery_source: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BenchmarkEvidence {
    pub suite_id: String,
    pub suite_version: String,
    pub model_config_fingerprint: String,
    pub case_fingerprints: Vec<(String, String)>,
    pub quality_score: f32,
    pub availability_score: f32,
    pub latency_ms: u64,
}

/// Only benchmark-v2 validation may construct this wrapper. Discovery scores
/// intentionally have no conversion path into promotion evidence.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedBenchmarkEvidence(BenchmarkEvidence);

impl ValidatedBenchmarkEvidence {
    pub(crate) fn from_validated_v2(evidence: BenchmarkEvidence) -> Self {
        Self(evidence)
    }

    pub fn evidence(&self) -> &BenchmarkEvidence {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromotionBenefit {
    BeatsCurrentChampion,
    FillsNewSpecialistRole,
}

#[derive(Clone, Debug)]
pub struct PromotionContext<'a> {
    pub role: RoutingRole,
    pub lane: ExecutionLane,
    pub privacy_allows_lane: bool,
    pub provider_usable: bool,
    pub required_context_tokens: u32,
    pub needs_reasoning: bool,
    pub needs_tools: bool,
    pub needs_structured_output: bool,
    pub expected_suite_id: &'a str,
    pub expected_suite_version: &'a str,
    pub expected_model_config_fingerprint: &'a str,
    pub expected_case_fingerprints: &'a [(String, String)],
    pub benefit: Option<PromotionBenefit>,
}

pub fn evaluate_promotion(
    candidate: &CanonicalModelDefinition,
    evidence: Option<&ValidatedBenchmarkEvidence>,
    context: &PromotionContext<'_>,
) -> Result<(), &'static str> {
    if candidate.lifecycle != ModelLifecycle::Candidate {
        return Err("promotion_requires_candidate_lifecycle");
    }
    if !candidate.enabled || candidate.execution_lane != context.lane {
        return Err("promotion_lane_or_enabled_mismatch");
    }
    if !context.lane.allows(candidate.cost_safety) {
        return Err("promotion_cost_safety_rejected");
    }
    if !context.privacy_allows_lane {
        return Err("promotion_privacy_rejected");
    }
    if !context.provider_usable {
        return Err("promotion_provider_unusable");
    }
    let role_suite = match context.role {
        RoutingRole::WorkFast | RoutingRole::WorkBalanced | RoutingRole::WorkDeep => "noki-work",
        RoutingRole::CodingFast | RoutingRole::CodingNormal | RoutingRole::CodingComplex => {
            "noki-coding"
        }
        RoutingRole::WorkLocalFloor | RoutingRole::CodingLocalFloor => {
            return Err("promotion_not_applicable_to_local_floor")
        }
    };
    if context.expected_suite_id != role_suite || context.expected_case_fingerprints.is_empty() {
        return Err("promotion_suite_role_mismatch");
    }
    let caps = candidate.capabilities;
    if caps.context_tokens < context.required_context_tokens
        || (context.needs_reasoning && !caps.reasoning)
        || (context.needs_tools && !caps.tools)
        || (context.needs_structured_output && !caps.structured_output)
    {
        return Err("promotion_capability_mismatch");
    }
    let evidence = evidence
        .ok_or("promotion_requires_complete_benchmark_v2")?
        .evidence();
    if evidence.suite_id != context.expected_suite_id
        || evidence.suite_version != context.expected_suite_version
        || evidence.model_config_fingerprint != context.expected_model_config_fingerprint
        || evidence.case_fingerprints != context.expected_case_fingerprints
    {
        return Err("promotion_benchmark_identity_mismatch");
    }
    if context.benefit.is_none() {
        return Err("promotion_requires_material_benefit");
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct RegistryModelState {
    pub canonical_model_id: String,
    pub lifecycle: ModelLifecycle,
    pub benchmark_history: Vec<BenchmarkEvidence>,
    pub discovery: Vec<DiscoveryMetadata>,
}

impl RegistryModelState {
    pub fn from_definition(definition: &CanonicalModelDefinition) -> Self {
        Self {
            canonical_model_id: definition.canonical_model_id.to_string(),
            lifecycle: definition.lifecycle,
            benchmark_history: Vec::new(),
            discovery: Vec::new(),
        }
    }

    /// Lifecycle changes never mutate evidence. Demotion is operational state,
    /// not a rewrite of history.
    pub fn transition_to(&mut self, lifecycle: ModelLifecycle) -> Result<(), &'static str> {
        if lifecycle == ModelLifecycle::Active && self.lifecycle != ModelLifecycle::Active {
            return Err("active_lifecycle_requires_promotion");
        }
        self.lifecycle = lifecycle;
        Ok(())
    }

    pub fn promote(
        &mut self,
        definition: &CanonicalModelDefinition,
        evidence: Option<&ValidatedBenchmarkEvidence>,
        context: &PromotionContext<'_>,
    ) -> Result<(), &'static str> {
        if self.canonical_model_id != definition.canonical_model_id {
            return Err("promotion_state_identity_mismatch");
        }
        evaluate_promotion(definition, evidence, context)?;
        self.benchmark_history
            .push(evidence.expect("validated above").evidence().clone());
        self.lifecycle = ModelLifecycle::Active;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn promotion_context<'a>(fingerprints: &'a [(String, String)]) -> PromotionContext<'a> {
        PromotionContext {
            role: RoutingRole::CodingNormal,
            lane: ExecutionLane::FreeCloud,
            privacy_allows_lane: true,
            provider_usable: true,
            required_context_tokens: 32_768,
            needs_reasoning: false,
            needs_tools: true,
            needs_structured_output: true,
            expected_suite_id: "noki-coding",
            expected_suite_version: "2.0.0",
            expected_model_config_fingerprint: "config-sha256",
            expected_case_fingerprints: fingerprints,
            benefit: Some(PromotionBenefit::BeatsCurrentChampion),
        }
    }

    #[test]
    fn stable_roles_resolve_only_through_registry_ids() {
        for plan in ROLE_PLANS {
            for id in role_model_ids(plan.role) {
                assert!(
                    model(id).is_some(),
                    "role {:?} references missing registry id",
                    plan.role
                );
            }
        }
        assert_eq!(RoutingRole::WorkBalanced, RoutingRole::WorkBalanced);
    }

    #[test]
    fn benchmark_v2_role_plans_keep_work_and_coding_orders_separate() {
        let work = role_model_ids(RoutingRole::WorkBalanced);
        assert_eq!(
            work,
            vec![
                GROQ_GPT_OSS.canonical_model_id,
                CLOUDFLARE_GLM_4_7_FLASH.canonical_model_id,
                MINISTRAL.canonical_model_id,
                DEEPSEEK.canonical_model_id,
                QWEN_9B.canonical_model_id,
            ]
        );
        let coding = role_model_ids(RoutingRole::CodingNormal);
        assert_eq!(
            coding,
            vec![
                DEEPSEEK.canonical_model_id,
                CLOUDFLARE_GLM_4_7_FLASH.canonical_model_id,
                GROQ_GPT_OSS.canonical_model_id,
                CODESTRAL.canonical_model_id,
                JACKOD.canonical_model_id,
            ]
        );
        assert_eq!(GROQ_GPT_OSS.benchmark_profile.work_quality, Some(100.0));
        assert_eq!(QWEN_9B.benchmark_profile.work_quality, Some(97.8));
        assert_eq!(
            CLOUDFLARE_GLM_4_7_FLASH.benchmark_profile.work_quality,
            Some(96.0)
        );
        assert_eq!(DEEPSEEK.benchmark_profile.code_quality, Some(83.2));
        assert_eq!(
            CLOUDFLARE_GLM_4_7_FLASH.benchmark_profile.code_quality,
            Some(82.1)
        );
    }

    #[test]
    fn validated_v2_models_are_active_and_stay_hard_free() {
        for definition in [
            GROQ_GPT_OSS,
            CLOUDFLARE_GLM_4_7_FLASH,
            QWEN_9B,
            MINISTRAL,
            DEEPSEEK,
            CODESTRAL,
            JACKOD,
        ] {
            assert_eq!(definition.lifecycle, ModelLifecycle::Active);
            assert_eq!(definition.benchmark_status, BenchmarkStatus::CanonicalV2);
        }
        assert_eq!(GROQ_GPT_OSS.cost_safety, CostSafety::VerifiedFreeHardStop);
        assert_eq!(
            CLOUDFLARE_GLM_4_7_FLASH.cost_safety,
            CostSafety::VerifiedFreeHardStop
        );
    }

    #[test]
    fn candidate_cannot_promote_without_validated_v2_benchmark() {
        let fingerprints = vec![("C1".into(), "sha256".into())];
        let context = promotion_context(&fingerprints);
        assert_eq!(
            evaluate_promotion(&NEX_N2_5_PRO, None, &context),
            Err("promotion_requires_complete_benchmark_v2")
        );
        let mut state = RegistryModelState::from_definition(&NEX_N2_5_PRO);
        assert_eq!(
            state.transition_to(ModelLifecycle::Active),
            Err("active_lifecycle_requires_promotion")
        );
        assert_eq!(state.lifecycle, ModelLifecycle::Candidate);
    }

    #[test]
    fn discovery_rank_cannot_promote_candidate() {
        let discovery = DiscoveryMetadata {
            external_quality_rank: Some(100.0),
            coding_rank: Some(1.0),
            general_work_rank: Some(1.0),
            task_fit: vec!["coding".into()],
            context_tokens: Some(1_000_000),
            tools: Some(true),
            reasoning: Some(true),
            advertised_cost: Some("free".into()),
            last_verified: Some(1),
            discovery_source: "external-ranking".into(),
        };
        let mut state = RegistryModelState::from_definition(&NEX_N2_5_PRO);
        state.discovery.push(discovery);
        let fingerprints = vec![("C1".into(), "sha256".into())];
        assert!(
            evaluate_promotion(&NEX_N2_5_PRO, None, &promotion_context(&fingerprints)).is_err()
        );
        assert_eq!(state.lifecycle, ModelLifecycle::Candidate);
    }

    #[test]
    fn demotion_and_retirement_preserve_benchmark_history() {
        let mut state = RegistryModelState::from_definition(&DEEPSEEK);
        state.benchmark_history.push(BenchmarkEvidence {
            suite_id: "noki-coding".into(),
            suite_version: "2.0.0".into(),
            model_config_fingerprint: "config".into(),
            case_fingerprints: vec![("C1".into(), "fingerprint".into())],
            quality_score: 80.0,
            availability_score: 100.0,
            latency_ms: 1066,
        });
        state.transition_to(ModelLifecycle::Parked).unwrap();
        state.transition_to(ModelLifecycle::Retired).unwrap();
        assert_eq!(state.benchmark_history.len(), 1);
        assert_eq!(state.lifecycle, ModelLifecycle::Retired);
    }
}
