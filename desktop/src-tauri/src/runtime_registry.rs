//! Canonical runtime state for provider + connection + exact model.
//!
//! Runtime observations are metadata only: no prompts, response bodies,
//! credentials or secret values are accepted by these types. Lifecycle and
//! benchmark evidence deliberately live in `model_registry` and are never
//! changed here.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::model_registry::{self, CanonicalModelDefinition, ExecutionLane};

/// Display-only quota evidence. It deliberately contains no account identity,
/// key, request content, or inferred allowance.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaTelemetry {
    /// provider_live, provider_account, locally_observed, or static_limit.
    pub source: String,
    /// model, provider, or account. Shared scopes must never be rendered as a
    /// per-model allowance.
    pub scope: String,
    pub observed_at: u64,
    pub reset_at: Option<String>,
    pub request_limit: Option<u64>,
    pub request_remaining: Option<u64>,
    pub token_limit: Option<u64>,
    pub token_remaining: Option<u64>,
    pub neuron_limit: Option<u64>,
    pub neuron_remaining: Option<u64>,
    pub cooldown_seconds: Option<u64>,
    /// Display-only local observation of a known shared cap. It is never a
    /// provider-confirmed remainder and is intentionally kept separate.
    #[serde(default)]
    pub local_usage_observed: bool,
    #[serde(default)]
    pub locally_observed_used: Option<u64>,
    /// UTC day number for daily static allocations such as Workers AI.
    #[serde(default)]
    pub local_reset_window: Option<u64>,
}

/// Minimal restart-safe projection of quota evidence. It has model identity
/// and numeric metadata only; credentials and provider/account identifiers are
/// intentionally absent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedQuotaTelemetry {
    pub canonical_model_id: String,
    pub telemetry: QuotaTelemetry,
}

impl QuotaTelemetry {
    pub fn is_useful(&self) -> bool {
        self.request_limit.is_some()
            || self.request_remaining.is_some()
            || self.token_limit.is_some()
            || self.token_remaining.is_some()
            || self.neuron_limit.is_some()
            || self.neuron_remaining.is_some()
            || self.reset_at.is_some()
            || self.cooldown_seconds.is_some()
    }
}

pub fn current_utc_day() -> u64 {
    unix_millis() / 1_000 / 86_400
}

fn preserve_static_observation(
    telemetry: &mut QuotaTelemetry,
    previous: Option<&QuotaTelemetry>,
) {
    if telemetry.source != "static_limit" {
        return;
    }
    let today = current_utc_day();
    if let Some(old) = previous.filter(|old| old.local_reset_window == Some(today)) {
        telemetry.local_usage_observed = old.local_usage_observed;
        telemetry.locally_observed_used = old.locally_observed_used;
        telemetry.local_reset_window = old.local_reset_window;
    } else {
        telemetry.local_usage_observed = true;
        telemetry.locally_observed_used = Some(0);
        telemetry.local_reset_window = Some(today);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeOutcome {
    Success,
    EmptyResponse,
    RateLimited,
    QuotaExhausted,
    Timeout,
    ServiceUnavailable,
    AuthenticationFailed,
    CostBlocked,
    PrivacyBlocked,
    CapabilityMismatch,
    QualityFailure,
    Cancelled,
}

impl RuntimeOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::EmptyResponse => "empty_response",
            Self::RateLimited => "rate_limited",
            Self::QuotaExhausted => "quota_exhausted",
            Self::Timeout => "timeout",
            Self::ServiceUnavailable => "service_unavailable",
            Self::AuthenticationFailed => "authentication_failed",
            Self::CostBlocked => "cost_blocked",
            Self::PrivacyBlocked => "privacy_blocked",
            Self::CapabilityMismatch => "capability_mismatch",
            Self::QualityFailure => "quality_failure",
            Self::Cancelled => "cancelled",
        }
    }

    pub const fn is_failure(self) -> bool {
        !matches!(self, Self::Success | Self::Cancelled)
    }
}

/// Conservative adapter for legacy integrations that still return only an
/// error string. Typed provider errors should map directly to `RuntimeOutcome`;
/// this function exists so Cloud Engine mocks and specialist CLIs do not grow
/// separate, drifting keyword taxonomies.
pub fn classify_error_message(message: &str) -> RuntimeOutcome {
    let lower = message.to_lowercase();
    if lower.contains("quota exhausted")
        || lower.contains("quota_exhausted")
        || lower.contains("kontingent erschöpft")
        || lower.contains("kontingent erschoepft")
    {
        RuntimeOutcome::QuotaExhausted
    } else if lower.contains("429")
        || lower.contains("rate limit")
        || lower.contains("rate_limit")
        || lower.contains("resource exhausted")
        || lower.contains("resource_exhausted")
        || lower.contains("session limit")
    {
        RuntimeOutcome::RateLimited
    } else if lower.contains("401")
        || lower.contains("403")
        || lower.contains("nicht angemeldet")
        || lower.contains("authentication")
        || lower.contains("unauthorized")
        || lower.contains("forbidden")
    {
        RuntimeOutcome::AuthenticationFailed
    } else if lower.contains("timeout") || lower.contains("zeitüberschreitung") {
        RuntimeOutcome::Timeout
    } else if lower.contains("leer") || lower.contains("empty") {
        RuntimeOutcome::EmptyResponse
    } else if lower.contains("cancelled")
        || lower.contains("canceled")
        || lower.contains("abgebrochen")
    {
        RuntimeOutcome::Cancelled
    } else {
        RuntimeOutcome::ServiceUnavailable
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeAvailability {
    Available,
    Degraded,
    Cooldown,
    RateLimited,
    QuotaExhausted,
    TemporarilyUnavailable,
    HalfOpen,
    AuthFailed,
    Disabled,
}

impl RuntimeAvailability {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Degraded => "degraded",
            Self::Cooldown => "cooldown",
            Self::RateLimited => "rate_limited",
            Self::QuotaExhausted => "quota_exhausted",
            Self::TemporarilyUnavailable => "temporarily_unavailable",
            Self::HalfOpen => "half_open",
            Self::AuthFailed => "auth_failed",
            Self::Disabled => "disabled",
        }
    }

    pub const fn is_selectable(self) -> bool {
        matches!(self, Self::Available | Self::Degraded)
    }
}

/// Deterministic circuit-breaker state.  The breaker is kept independently at
/// provider, connection, and exact-model scope so an isolated model failure
/// cannot poison its siblings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BreakerState {
    Closed,
    Degraded,
    Open,
    HalfOpen,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeTelemetry {
    #[serde(rename = "request_count")]
    pub requests: u32,
    #[serde(rename = "success_count")]
    pub successes: u32,
    #[serde(rename = "failure_count")]
    pub failures: u32,
    #[serde(rename = "recovery_success_count")]
    pub recovery_successes: u32,
    #[serde(rename = "recovery_failure_count")]
    pub recovery_failures: u32,
    #[serde(rename = "timeout_count")]
    pub timeouts: u32,
    #[serde(rename = "service_unavailable_count")]
    pub service_unavailable: u32,
    #[serde(rename = "rate_limit_count")]
    pub rate_limited: u32,
    #[serde(rename = "quota_exhausted_count")]
    pub quota_exhausted: u32,
    #[serde(rename = "auth_failure_count")]
    pub authentication_failures: u32,
    #[serde(rename = "empty_count")]
    pub empty_responses: u32,
    #[serde(rename = "quality_failure_count")]
    pub quality_failures: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataConfidence {
    Unknown,
    Header,
    ExplicitReset,
}

impl Default for MetadataConfidence {
    fn default() -> Self {
        Self::Unknown
    }
}

impl RuntimeTelemetry {
    fn record(&mut self, outcome: RuntimeOutcome) {
        match outcome {
            RuntimeOutcome::Success => self.successes = self.successes.saturating_add(1),
            RuntimeOutcome::Timeout => self.timeouts = self.timeouts.saturating_add(1),
            RuntimeOutcome::ServiceUnavailable => {
                self.service_unavailable = self.service_unavailable.saturating_add(1)
            }
            RuntimeOutcome::RateLimited => self.rate_limited = self.rate_limited.saturating_add(1),
            RuntimeOutcome::QuotaExhausted => {
                self.quota_exhausted = self.quota_exhausted.saturating_add(1)
            }
            RuntimeOutcome::AuthenticationFailed => {
                self.authentication_failures = self.authentication_failures.saturating_add(1)
            }
            RuntimeOutcome::EmptyResponse => {
                self.empty_responses = self.empty_responses.saturating_add(1)
            }
            RuntimeOutcome::QualityFailure => {
                self.quality_failures = self.quality_failures.saturating_add(1)
            }
            _ => {}
        }
        if outcome.is_failure() {
            self.failures = self.failures.saturating_add(1);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaState {
    Unknown,
    Available,
    RateLimited,
    Exhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthState {
    NotRequired,
    Unknown,
    Ready,
    Missing,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutcomeScope {
    Provider,
    Connection,
    Model,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RuntimeKey {
    pub provider_id: String,
    pub connection_id: String,
    pub canonical_model_id: String,
    pub exact_model_version: String,
}

impl RuntimeKey {
    fn from_definition(definition: &CanonicalModelDefinition) -> Self {
        Self {
            provider_id: definition.provider_id.to_string(),
            connection_id: definition.connection.id.to_string(),
            canonical_model_id: definition.canonical_model_id.to_string(),
            exact_model_version: definition.exact_model_version.to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderRuntimeState {
    pub provider_id: String,
    pub availability: RuntimeAvailability,
    pub last_success: Option<u64>,
    pub last_failure: Option<u64>,
    pub consecutive_failures: u32,
    pub last_outcome: Option<RuntimeOutcome>,
    pub cooldown_until: Option<u64>,
    pub breaker: BreakerState,
    pub probe_in_flight: bool,
    pub telemetry: RuntimeTelemetry,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConnectionRuntimeState {
    pub provider_id: String,
    pub connection_id: String,
    pub availability: RuntimeAvailability,
    pub last_success: Option<u64>,
    pub last_failure: Option<u64>,
    pub consecutive_failures: u32,
    pub last_outcome: Option<RuntimeOutcome>,
    pub cooldown_until: Option<u64>,
    pub quota_state: QuotaState,
    pub quota_remaining: Option<String>,
    pub quota_reset: Option<String>,
    /// Unix milliseconds of the last real quota/header observation.
    pub quota_freshness: Option<u64>,
    pub quota_source: Option<String>,
    pub quota_confidence: MetadataConfidence,
    #[serde(default)]
    pub quota_telemetry: Option<QuotaTelemetry>,
    pub auth_state: AuthState,
    pub breaker: BreakerState,
    pub probe_in_flight: bool,
    pub telemetry: RuntimeTelemetry,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelRuntimeState {
    pub key: RuntimeKey,
    pub availability: RuntimeAvailability,
    pub last_success: Option<u64>,
    pub last_failure: Option<u64>,
    pub consecutive_failures: u32,
    pub last_outcome: Option<RuntimeOutcome>,
    pub cooldown_until: Option<u64>,
    pub quota_state: QuotaState,
    pub quota_remaining: Option<String>,
    pub quota_reset: Option<String>,
    pub quota_freshness: Option<u64>,
    pub quota_source: Option<String>,
    pub quota_confidence: MetadataConfidence,
    #[serde(default)]
    pub quota_telemetry: Option<QuotaTelemetry>,
    pub auth_state: AuthState,
    pub last_latency_ms: Option<u64>,
    pub request_count: u32,
    pub failure_count: u32,
    pub last_rate_limit: Option<u64>,
    pub breaker: BreakerState,
    pub probe_in_flight: bool,
    pub telemetry: RuntimeTelemetry,
    /// Recent latency observations only; capped to keep snapshots bounded.
    pub latency_samples_ms: Vec<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CanonicalRuntimeState {
    pub provider: ProviderRuntimeState,
    pub connection: ConnectionRuntimeState,
    pub model: ModelRuntimeState,
    pub effective_availability: RuntimeAvailability,
}

/// Read-only consumer view over the canonical runtime registry.
///
/// The contained state is private on purpose: consumers can inspect runtime
/// health but cannot feed a legacy projection back into the registry.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelRuntimeView {
    state: CanonicalRuntimeState,
}

impl ModelRuntimeView {
    pub fn quota_telemetry(&self) -> Option<QuotaTelemetry> {
        self.state
            .connection
            .quota_telemetry
            .clone()
            .or_else(|| self.state.model.quota_telemetry.clone())
    }
    pub fn effective_availability(&self) -> RuntimeAvailability {
        self.state.effective_availability
    }

    pub fn is_selectable(&self) -> bool {
        self.effective_availability().is_selectable()
    }

    pub fn provider_availability(&self) -> RuntimeAvailability {
        self.state.provider.availability
    }

    pub fn connection_availability(&self) -> RuntimeAvailability {
        self.state.connection.availability
    }

    pub fn model_availability(&self) -> RuntimeAvailability {
        self.state.model.availability
    }

    pub fn cooldown_until(&self) -> Option<u64> {
        self.state
            .model
            .cooldown_until
            .or(self.state.connection.cooldown_until)
            .or(self.state.provider.cooldown_until)
    }

    pub fn quota_remaining(&self) -> Option<String> {
        self.state
            .connection
            .quota_remaining
            .clone()
            .or_else(|| self.state.model.quota_remaining.clone())
    }

    pub fn quota_reset(&self) -> Option<String> {
        self.state
            .connection
            .quota_reset
            .clone()
            .or_else(|| self.state.model.quota_reset.clone())
    }

    /// The narrowest truthful scope for the quota data currently known for
    /// this model. Connection data is shared by all models using that
    /// provider/account; model data may be shown on the individual model.
    pub fn quota_scope(&self) -> Option<&'static str> {
        if self.state.connection.quota_remaining.is_some()
            || self.state.connection.quota_reset.is_some()
        {
            Some("shared_provider")
        } else if self.state.model.quota_remaining.is_some()
            || self.state.model.quota_reset.is_some()
        {
            Some("model")
        } else {
            None
        }
    }

    pub fn quota_source(&self) -> Option<String> {
        self.state
            .connection
            .quota_source
            .clone()
            .or_else(|| self.state.model.quota_source.clone())
    }

    pub fn quota_confidence(&self) -> MetadataConfidence {
        if self.state.connection.quota_freshness.is_some() {
            self.state.connection.quota_confidence
        } else {
            self.state.model.quota_confidence
        }
    }

    pub fn quota_observed_at(&self) -> Option<u64> {
        self.state
            .connection
            .quota_freshness
            .or(self.state.model.quota_freshness)
    }

    pub fn quota_age_ms(&self) -> Option<u64> {
        self.quota_observed_at()
            .map(|observed_at| unix_millis().saturating_sub(observed_at))
    }

    pub fn quota_state(&self) -> QuotaState {
        if self.state.connection.quota_state != QuotaState::Unknown {
            self.state.connection.quota_state
        } else {
            self.state.model.quota_state
        }
    }

    pub fn auth_state(&self) -> AuthState {
        self.state.connection.auth_state
    }

    pub fn last_outcomes(&self) -> [Option<RuntimeOutcome>; 3] {
        [
            self.state.model.last_outcome,
            self.state.connection.last_outcome,
            self.state.provider.last_outcome,
        ]
    }

    pub fn last_latency_ms(&self) -> Option<u64> {
        self.state.model.last_latency_ms
    }

    pub fn request_count(&self) -> u32 {
        self.state.model.request_count
    }

    pub fn failure_count(&self) -> u32 {
        self.state.model.failure_count
    }

    pub fn breaker(&self) -> BreakerState {
        effective_breaker(
            &self.state.provider,
            &self.state.connection,
            &self.state.model,
        )
    }

    pub fn telemetry(&self) -> &RuntimeTelemetry {
        &self.state.model.telemetry
    }

    pub fn latency_p50_ms(&self) -> Option<u64> {
        percentile(&self.state.model.latency_samples_ms, 50)
    }

    pub fn latency_p95_ms(&self) -> Option<u64> {
        percentile(&self.state.model.latency_samples_ms, 95)
    }

    pub fn last_rate_limit(&self) -> Option<u64> {
        self.state.model.last_rate_limit.or_else(|| {
            matches!(
                self.state.connection.last_outcome,
                Some(RuntimeOutcome::RateLimited | RuntimeOutcome::QuotaExhausted)
            )
            .then_some(self.state.connection.last_failure)
            .flatten()
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct RuntimeObservation {
    pub outcome: Option<RuntimeOutcome>,
    pub scope: Option<OutcomeScope>,
    pub observed_at: Option<u64>,
    pub cooldown_until: Option<u64>,
    pub quota_remaining: Option<String>,
    pub quota_reset: Option<String>,
    pub quota_source: Option<String>,
    pub quota_confidence: MetadataConfidence,
    pub quota_telemetry: Option<QuotaTelemetry>,
    pub last_latency_ms: Option<u64>,
}

const MAX_LATENCY_SAMPLES: usize = 64;

impl RuntimeObservation {
    pub fn outcome(outcome: RuntimeOutcome, scope: OutcomeScope) -> Self {
        Self {
            outcome: Some(outcome),
            scope: Some(scope),
            observed_at: Some(unix_millis()),
            ..Self::default()
        }
    }
}

#[derive(Default)]
pub struct RuntimeRegistry {
    providers: HashMap<String, ProviderRuntimeState>,
    connections: HashMap<(String, String), ConnectionRuntimeState>,
    models: HashMap<RuntimeKey, ModelRuntimeState>,
}

impl RuntimeRegistry {
    pub fn clear(&mut self) {
        self.providers.clear();
        self.connections.clear();
        self.models.clear();
    }

    fn ensure(&mut self, definition: &CanonicalModelDefinition) {
        let provider_id = definition.provider_id.to_string();
        let connection_id = definition.connection.id.to_string();
        let model_key = RuntimeKey::from_definition(definition);
        self.providers
            .entry(provider_id.clone())
            .or_insert_with(|| ProviderRuntimeState {
                provider_id: provider_id.clone(),
                availability: RuntimeAvailability::Available,
                last_success: None,
                last_failure: None,
                consecutive_failures: 0,
                last_outcome: None,
                cooldown_until: None,
                breaker: BreakerState::Closed,
                probe_in_flight: false,
                telemetry: RuntimeTelemetry::default(),
            });
        self.connections
            .entry((provider_id.clone(), connection_id.clone()))
            .or_insert_with(|| ConnectionRuntimeState {
                provider_id: provider_id.clone(),
                connection_id: connection_id.clone(),
                availability: RuntimeAvailability::Available,
                last_success: None,
                last_failure: None,
                consecutive_failures: 0,
                last_outcome: None,
                cooldown_until: None,
                quota_state: QuotaState::Unknown,
                quota_remaining: None,
                quota_reset: None,
                quota_freshness: None,
                quota_source: None,
                quota_confidence: MetadataConfidence::Unknown,
                quota_telemetry: None,
                auth_state: if definition.execution_lane == ExecutionLane::Local {
                    AuthState::NotRequired
                } else {
                    AuthState::Unknown
                },
                breaker: BreakerState::Closed,
                probe_in_flight: false,
                telemetry: RuntimeTelemetry::default(),
            });
        self.models
            .entry(model_key.clone())
            .or_insert_with(|| ModelRuntimeState {
                key: model_key,
                availability: RuntimeAvailability::Available,
                last_success: None,
                last_failure: None,
                consecutive_failures: 0,
                last_outcome: None,
                cooldown_until: None,
                quota_state: QuotaState::Unknown,
                quota_remaining: None,
                quota_reset: None,
                quota_freshness: None,
                quota_source: None,
                quota_confidence: MetadataConfidence::Unknown,
                quota_telemetry: None,
                auth_state: if definition.execution_lane == ExecutionLane::Local {
                    AuthState::NotRequired
                } else {
                    AuthState::Unknown
                },
                last_latency_ms: None,
                request_count: 0,
                failure_count: 0,
                last_rate_limit: None,
                breaker: BreakerState::Closed,
                probe_in_flight: false,
                telemetry: RuntimeTelemetry::default(),
                latency_samples_ms: Vec::new(),
            });
    }

    pub fn note_request(&mut self, model_id: &str) -> Result<(), &'static str> {
        let definition = model_registry::model(model_id).ok_or("runtime_model_unknown")?;
        self.ensure(definition);
        let model_key = RuntimeKey::from_definition(definition);
        let model = self.models.get_mut(&model_key).expect("ensured");
        model.request_count = model.request_count.saturating_add(1);
        model.telemetry.requests = model.telemetry.requests.saturating_add(1);
        Ok(())
    }

    pub fn set_auth_state(
        &mut self,
        model_id: &str,
        auth_state: AuthState,
    ) -> Result<(), &'static str> {
        let definition = model_registry::model(model_id).ok_or("runtime_model_unknown")?;
        self.ensure(definition);
        let key = (
            definition.provider_id.to_string(),
            definition.connection.id.to_string(),
        );
        let connection = self.connections.get_mut(&key).expect("ensured");
        connection.auth_state = auth_state;
        connection.availability = match auth_state {
            AuthState::Missing | AuthState::Failed => RuntimeAvailability::AuthFailed,
            _ if connection.availability == RuntimeAvailability::AuthFailed => {
                RuntimeAvailability::Available
            }
            _ => connection.availability,
        };
        Ok(())
    }

    pub fn note_quota(
        &mut self,
        model_id: &str,
        remaining: Option<String>,
        reset: Option<String>,
    ) -> Result<(), &'static str> {
        let definition = model_registry::model(model_id).ok_or("runtime_model_unknown")?;
        self.ensure(definition);
        let key = (
            definition.provider_id.to_string(),
            definition.connection.id.to_string(),
        );
        let connection = self.connections.get_mut(&key).expect("ensured");
        if remaining.is_some() || reset.is_some() {
            connection.quota_state = QuotaState::Available;
            connection.quota_remaining = remaining;
            connection.quota_reset = reset;
            connection.quota_freshness = Some(unix_millis());
            connection.quota_source = Some("runtime_observation".to_string());
            connection.quota_confidence = MetadataConfidence::Header;
        }
        Ok(())
    }

    pub fn note_quota_telemetry(
        &mut self,
        model_id: &str,
        mut telemetry: QuotaTelemetry,
    ) -> Result<(), &'static str> {
        let definition = model_registry::model(model_id).ok_or("runtime_model_unknown")?;
        self.ensure(definition);
        if !telemetry.is_useful() {
            return Ok(());
        }
        if telemetry.observed_at == 0 {
            telemetry.observed_at = unix_millis();
        }
        let key = (definition.provider_id.to_string(), definition.connection.id.to_string());
        if telemetry.scope == "model" {
            let model = self.models.get_mut(&RuntimeKey::from_definition(definition)).expect("ensured");
            preserve_static_observation(&mut telemetry, model.quota_telemetry.as_ref());
            if !matches!(model.quota_telemetry.as_ref().map(|old| old.source.as_str()), Some("provider_live"))
                || telemetry.source == "provider_live"
            {
                model.quota_telemetry = Some(telemetry);
            }
        } else {
            let connection = self.connections.get_mut(&key).expect("ensured");
            preserve_static_observation(&mut telemetry, connection.quota_telemetry.as_ref());
            // Authoritative headers always outrank an informational static cap.
            if !matches!(connection.quota_telemetry.as_ref().map(|old| old.source.as_str()), Some("provider_live"))
                || telemetry.source == "provider_live"
            {
                connection.quota_telemetry = Some(telemetry);
            }
        }
        Ok(())
    }

    /// A successful request may have consumed a static allocation even when
    /// the provider does not reveal a unit cost. Do not retain a false zero.
    pub fn mark_static_quota_usage_unknown(&mut self, model_id: &str) -> Result<(), &'static str> {
        let definition = model_registry::model(model_id).ok_or("runtime_model_unknown")?;
        self.ensure(definition);
        let key = (definition.provider_id.to_string(), definition.connection.id.to_string());
        let telemetry = self
            .connections
            .get_mut(&key)
            .and_then(|connection| connection.quota_telemetry.as_mut());
        if let Some(telemetry) = telemetry.filter(|telemetry| telemetry.source == "static_limit") {
            telemetry.local_usage_observed = true;
            telemetry.locally_observed_used = None;
            telemetry.local_reset_window = Some(current_utc_day());
        }
        Ok(())
    }

    pub fn set_provider_enabled(
        &mut self,
        provider_id: &str,
        enabled: bool,
    ) -> Result<(), &'static str> {
        for definition in model_registry::MODELS
            .iter()
            .filter(|model| model.provider_id == provider_id)
        {
            self.ensure(definition);
        }
        let provider = self
            .providers
            .get_mut(provider_id)
            .ok_or("runtime_provider_unknown")?;
        if enabled {
            if provider.availability == RuntimeAvailability::Disabled {
                provider.availability = RuntimeAvailability::Available;
                provider.cooldown_until = None;
            }
        } else {
            provider.availability = RuntimeAvailability::Disabled;
            provider.cooldown_until = None;
        }
        Ok(())
    }

    pub fn set_model_enabled(&mut self, model_id: &str, enabled: bool) -> Result<(), &'static str> {
        let definition = model_registry::model(model_id).ok_or("runtime_model_unknown")?;
        self.ensure(definition);
        let model_key = RuntimeKey::from_definition(definition);
        let model = self.models.get_mut(&model_key).expect("ensured");
        if enabled {
            if model.availability == RuntimeAvailability::Disabled {
                model.availability = RuntimeAvailability::Available;
                model.cooldown_until = None;
            }
        } else {
            model.availability = RuntimeAvailability::Disabled;
            model.cooldown_until = None;
        }
        Ok(())
    }

    pub fn observe(
        &mut self,
        model_id: &str,
        observation: RuntimeObservation,
    ) -> Result<(), &'static str> {
        let definition = model_registry::model(model_id).ok_or("runtime_model_unknown")?;
        self.ensure(definition);
        let at = observation.observed_at.unwrap_or_else(unix_millis);
        let outcome = observation.outcome.ok_or("runtime_outcome_missing")?;
        let scope = observation.scope.ok_or("runtime_scope_missing")?;
        let provider_key = definition.provider_id.to_string();
        let connection_key = (provider_key.clone(), definition.connection.id.to_string());
        let model_key = RuntimeKey::from_definition(definition);

        // The exact model keeps the latest request outcome for audit, while
        // health counters are updated only at the scope responsible for it.
        let model = self.models.get_mut(&model_key).expect("ensured");
        model.telemetry.record(outcome);
        model.last_outcome = Some(outcome);
        model.last_latency_ms = observation.last_latency_ms.or(model.last_latency_ms);
        if observation.quota_remaining.is_some() || observation.quota_reset.is_some() {
            model.quota_remaining = observation.quota_remaining.clone();
            model.quota_reset = observation.quota_reset.clone();
            model.quota_freshness = Some(at);
            model.quota_source = observation
                .quota_source
                .clone()
                .or_else(|| Some("runtime_observation".to_string()));
            model.quota_confidence = observation.quota_confidence;
        }
        if let Some(telemetry) = observation.quota_telemetry.as_ref().filter(|value| value.is_useful()) {
            model.quota_telemetry = Some(telemetry.clone());
        }
        if let Some(latency) = observation.last_latency_ms {
            if model.latency_samples_ms.len() == MAX_LATENCY_SAMPLES {
                model.latency_samples_ms.remove(0);
            }
            model.latency_samples_ms.push(latency);
        }
        if outcome.is_failure() {
            model.last_failure = Some(at);
            model.failure_count = model.failure_count.saturating_add(1);
        } else if outcome == RuntimeOutcome::Success {
            model.last_success = Some(at);
        }

        if outcome == RuntimeOutcome::Success {
            self.apply_success(&provider_key, &connection_key, &model_key, at, &observation);
            return Ok(());
        }

        if matches!(
            outcome,
            RuntimeOutcome::CostBlocked
                | RuntimeOutcome::PrivacyBlocked
                | RuntimeOutcome::CapabilityMismatch
                | RuntimeOutcome::Cancelled
        ) {
            return Ok(());
        }

        match scope {
            OutcomeScope::Provider => {
                let provider = self.providers.get_mut(&provider_key).expect("ensured");
                let was_probe = provider.breaker == BreakerState::HalfOpen;
                provider.telemetry.record(outcome);
                apply_failure_state(
                    &mut provider.availability,
                    &mut provider.last_failure,
                    &mut provider.consecutive_failures,
                    &mut provider.last_outcome,
                    &mut provider.cooldown_until,
                    outcome,
                    at,
                    observation.cooldown_until,
                );
                provider.breaker =
                    next_breaker(provider.breaker, outcome, provider.consecutive_failures);
                if was_probe {
                    provider.telemetry.recovery_failures =
                        provider.telemetry.recovery_failures.saturating_add(1);
                }
                provider.probe_in_flight = false;
            }
            OutcomeScope::Connection => {
                let connection = self.connections.get_mut(&connection_key).expect("ensured");
                let was_probe = connection.breaker == BreakerState::HalfOpen;
                connection.telemetry.record(outcome);
                apply_failure_state(
                    &mut connection.availability,
                    &mut connection.last_failure,
                    &mut connection.consecutive_failures,
                    &mut connection.last_outcome,
                    &mut connection.cooldown_until,
                    outcome,
                    at,
                    observation.cooldown_until,
                );
                connection.breaker =
                    next_breaker(connection.breaker, outcome, connection.consecutive_failures);
                if was_probe {
                    connection.telemetry.recovery_failures =
                        connection.telemetry.recovery_failures.saturating_add(1);
                }
                connection.probe_in_flight = false;
                apply_connection_metadata(connection, outcome, at, &observation);
            }
            OutcomeScope::Model => {
                let model = self.models.get_mut(&model_key).expect("ensured");
                let was_probe = model.breaker == BreakerState::HalfOpen;
                apply_failure_state(
                    &mut model.availability,
                    &mut model.last_failure,
                    &mut model.consecutive_failures,
                    &mut model.last_outcome,
                    &mut model.cooldown_until,
                    outcome,
                    at,
                    observation.cooldown_until,
                );
                model.breaker = next_breaker(model.breaker, outcome, model.consecutive_failures);
                if was_probe {
                    model.telemetry.recovery_failures =
                        model.telemetry.recovery_failures.saturating_add(1);
                }
                model.probe_in_flight = false;
                apply_model_quota_metadata(model, outcome, at, &observation);
            }
        }
        Ok(())
    }

    fn apply_success(
        &mut self,
        provider_key: &str,
        connection_key: &(String, String),
        model_key: &RuntimeKey,
        at: u64,
        observation: &RuntimeObservation,
    ) {
        let provider = self.providers.get_mut(provider_key).expect("ensured");
        let provider_was_probe = provider.breaker == BreakerState::HalfOpen;
        if provider.availability != RuntimeAvailability::Disabled {
            provider.availability = RuntimeAvailability::Available;
        }
        provider.last_success = Some(at);
        provider.telemetry.successes = provider.telemetry.successes.saturating_add(1);
        provider.consecutive_failures = 0;
        provider.last_outcome = Some(RuntimeOutcome::Success);
        provider.cooldown_until = None;
        provider.breaker = BreakerState::Closed;
        provider.probe_in_flight = false;
        if provider_was_probe {
            provider.telemetry.recovery_successes =
                provider.telemetry.recovery_successes.saturating_add(1);
        }

        let connection = self.connections.get_mut(connection_key).expect("ensured");
        let connection_was_probe = connection.breaker == BreakerState::HalfOpen;
        connection.availability = RuntimeAvailability::Available;
        connection.last_success = Some(at);
        connection.telemetry.successes = connection.telemetry.successes.saturating_add(1);
        connection.consecutive_failures = 0;
        connection.last_outcome = Some(RuntimeOutcome::Success);
        connection.cooldown_until = None;
        connection.breaker = BreakerState::Closed;
        connection.probe_in_flight = false;
        if connection_was_probe {
            connection.telemetry.recovery_successes =
                connection.telemetry.recovery_successes.saturating_add(1);
        }
        if connection.auth_state != AuthState::NotRequired {
            connection.auth_state = AuthState::Ready;
        }
        if observation.quota_remaining.is_some() || observation.quota_reset.is_some() {
            connection.quota_state = QuotaState::Available;
            connection.quota_remaining = observation.quota_remaining.clone();
            connection.quota_reset = observation.quota_reset.clone();
            connection.quota_freshness = Some(at);
            connection.quota_source = observation
                .quota_source
                .clone()
                .or_else(|| Some("runtime_observation".to_string()));
            connection.quota_confidence = observation.quota_confidence;
        }
        let model_quota_telemetry = observation
            .quota_telemetry
            .as_ref()
            .filter(|value| value.is_useful() && value.scope == "model")
            .cloned();
        if let Some(telemetry) = observation
            .quota_telemetry
            .as_ref()
            .filter(|value| value.is_useful() && value.scope != "model")
        {
            connection.quota_telemetry = Some(telemetry.clone());
        }
        let model = self.models.get_mut(model_key).expect("ensured");
        if let Some(telemetry) = model_quota_telemetry {
            model.quota_telemetry = Some(telemetry);
        }
        let was_probe = model.breaker == BreakerState::HalfOpen;
        if model.availability != RuntimeAvailability::Disabled {
            model.availability = RuntimeAvailability::Available;
        }
        model.last_success = Some(at);
        model.consecutive_failures = 0;
        model.last_outcome = Some(RuntimeOutcome::Success);
        model.cooldown_until = None;
        model.breaker = BreakerState::Closed;
        model.probe_in_flight = false;
        if was_probe {
            model.telemetry.recovery_successes =
                model.telemetry.recovery_successes.saturating_add(1);
        }
        model.last_latency_ms = observation.last_latency_ms.or(model.last_latency_ms);
        if model.auth_state != AuthState::NotRequired {
            model.auth_state = AuthState::Ready;
        }
    }

    pub fn snapshot(&mut self, model_id: &str) -> Option<CanonicalRuntimeState> {
        let definition = model_registry::model(model_id)?;
        self.ensure(definition);
        let now = unix_millis();
        let provider_key = definition.provider_id.to_string();
        let connection_key = (provider_key.clone(), definition.connection.id.to_string());
        let model_key = RuntimeKey::from_definition(definition);
        refresh_availability(self.providers.get_mut(&provider_key)?, now);
        refresh_connection(self.connections.get_mut(&connection_key)?, now);
        refresh_model(self.models.get_mut(&model_key)?, now);
        let provider = self.providers.get(&provider_key)?.clone();
        let connection = self.connections.get(&connection_key)?.clone();
        let model = self.models.get(&model_key)?.clone();
        let effective_availability = effective_availability(&provider, &connection, &model);
        Some(CanonicalRuntimeState {
            provider,
            connection,
            model,
            effective_availability,
        })
    }

    /// Admit at most one lazy probe for each expired breaker scope. Normal
    /// traffic remains unaffected; callers invoke this only after seeing
    /// `HalfOpen` in a snapshot.
    pub fn admit_probe(&mut self, model_id: &str) -> bool {
        let definition = match model_registry::model(model_id) {
            Some(definition) => definition,
            None => return false,
        };
        self.ensure(definition);
        let now = unix_millis();
        let provider_key = definition.provider_id.to_string();
        let connection_key = (provider_key.clone(), definition.connection.id.to_string());
        let model_key = RuntimeKey::from_definition(definition);
        refresh_availability(self.providers.get_mut(&provider_key).expect("ensured"), now);
        refresh_connection(
            self.connections.get_mut(&connection_key).expect("ensured"),
            now,
        );
        refresh_model(self.models.get_mut(&model_key).expect("ensured"), now);
        let provider = self.providers.get_mut(&provider_key).expect("ensured");
        if provider.breaker == BreakerState::HalfOpen {
            if provider.probe_in_flight {
                return false;
            }
            provider.probe_in_flight = true;
            return true;
        }
        let connection = self.connections.get_mut(&connection_key).expect("ensured");
        if connection.breaker == BreakerState::HalfOpen {
            if connection.probe_in_flight {
                return false;
            }
            connection.probe_in_flight = true;
            return true;
        }
        let model = self.models.get_mut(&model_key).expect("ensured");
        if model.breaker == BreakerState::HalfOpen {
            if model.probe_in_flight {
                return false;
            }
            model.probe_in_flight = true;
            return true;
        }
        false
    }

    pub fn model_runtime_view(&mut self, model_id: &str) -> Option<ModelRuntimeView> {
        self.snapshot(model_id)
            .map(|state| ModelRuntimeView { state })
    }
}

fn availability_for_outcome(outcome: RuntimeOutcome) -> RuntimeAvailability {
    match outcome {
        RuntimeOutcome::RateLimited => RuntimeAvailability::RateLimited,
        RuntimeOutcome::QuotaExhausted => RuntimeAvailability::QuotaExhausted,
        RuntimeOutcome::Timeout | RuntimeOutcome::ServiceUnavailable => {
            RuntimeAvailability::TemporarilyUnavailable
        }
        RuntimeOutcome::AuthenticationFailed => RuntimeAvailability::AuthFailed,
        RuntimeOutcome::EmptyResponse
        | RuntimeOutcome::QualityFailure
        | RuntimeOutcome::CapabilityMismatch => RuntimeAvailability::Degraded,
        RuntimeOutcome::CostBlocked
        | RuntimeOutcome::PrivacyBlocked
        | RuntimeOutcome::Cancelled
        | RuntimeOutcome::Success => RuntimeAvailability::Available,
    }
}

fn next_breaker(current: BreakerState, outcome: RuntimeOutcome, failures: u32) -> BreakerState {
    match outcome {
        RuntimeOutcome::EmptyResponse | RuntimeOutcome::QualityFailure => BreakerState::Degraded,
        RuntimeOutcome::CostBlocked
        | RuntimeOutcome::PrivacyBlocked
        | RuntimeOutcome::CapabilityMismatch
        | RuntimeOutcome::Cancelled
        | RuntimeOutcome::Success
        | RuntimeOutcome::AuthenticationFailed => BreakerState::Closed,
        _ if current == BreakerState::HalfOpen => BreakerState::Open,
        _ if failures >= 2 => BreakerState::Open,
        _ => BreakerState::Degraded,
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_failure_state(
    availability: &mut RuntimeAvailability,
    last_failure: &mut Option<u64>,
    consecutive_failures: &mut u32,
    last_outcome: &mut Option<RuntimeOutcome>,
    cooldown_until: &mut Option<u64>,
    outcome: RuntimeOutcome,
    at: u64,
    observed_cooldown: Option<u64>,
) {
    *availability = availability_for_outcome(outcome);
    *last_failure = Some(at);
    *consecutive_failures = consecutive_failures.saturating_add(1);
    *last_outcome = Some(outcome);
    *cooldown_until = observed_cooldown;
}

fn apply_connection_metadata(
    connection: &mut ConnectionRuntimeState,
    outcome: RuntimeOutcome,
    at: u64,
    observation: &RuntimeObservation,
) {
    match outcome {
        RuntimeOutcome::AuthenticationFailed => connection.auth_state = AuthState::Failed,
        RuntimeOutcome::QuotaExhausted => connection.quota_state = QuotaState::Exhausted,
        RuntimeOutcome::RateLimited => connection.quota_state = QuotaState::RateLimited,
        _ => {}
    }
    if observation.quota_remaining.is_some() || observation.quota_reset.is_some() {
        connection.quota_remaining = observation.quota_remaining.clone();
        connection.quota_reset = observation.quota_reset.clone();
        connection.quota_freshness = Some(at);
        connection.quota_source = observation
            .quota_source
            .clone()
            .or_else(|| Some("runtime_observation".to_string()));
        connection.quota_confidence = observation.quota_confidence;
    }
}

fn apply_model_quota_metadata(
    model: &mut ModelRuntimeState,
    outcome: RuntimeOutcome,
    at: u64,
    observation: &RuntimeObservation,
) {
    match outcome {
        RuntimeOutcome::RateLimited => {
            model.quota_state = QuotaState::RateLimited;
            model.last_rate_limit = Some(at);
        }
        RuntimeOutcome::QuotaExhausted => {
            model.quota_state = QuotaState::Exhausted;
            model.last_rate_limit = Some(at);
        }
        _ => {}
    }
    if observation.quota_remaining.is_some() || observation.quota_reset.is_some() {
        model.quota_remaining = observation.quota_remaining.clone();
        model.quota_reset = observation.quota_reset.clone();
        model.quota_freshness = Some(at);
        model.quota_source = observation
            .quota_source
            .clone()
            .or_else(|| Some("runtime_observation".to_string()));
        model.quota_confidence = observation.quota_confidence;
    }
}

fn refresh_availability(state: &mut ProviderRuntimeState, now: u64) {
    if state.cooldown_until.is_some_and(|until| now >= until) {
        if state.breaker == BreakerState::Open {
            state.breaker = BreakerState::HalfOpen;
            state.availability = RuntimeAvailability::HalfOpen;
        } else {
            state.availability = RuntimeAvailability::Available;
        }
        state.cooldown_until = None;
    }
}

fn refresh_connection(state: &mut ConnectionRuntimeState, now: u64) {
    if state.cooldown_until.is_some_and(|until| now >= until) {
        state.cooldown_until = None;
        // Time can clear a transient quota/network cooldown, but never proves
        // that a missing or rejected credential became valid. Only an explicit
        // auth update or a successful request may recover `AuthFailed`.
        if matches!(state.auth_state, AuthState::Missing | AuthState::Failed) {
            state.availability = RuntimeAvailability::AuthFailed;
        } else {
            if state.breaker == BreakerState::Open {
                state.breaker = BreakerState::HalfOpen;
                state.availability = RuntimeAvailability::HalfOpen;
            } else {
                state.availability = RuntimeAvailability::Available;
            }
            if matches!(
                state.quota_state,
                QuotaState::RateLimited | QuotaState::Exhausted
            ) {
                state.quota_state = QuotaState::Unknown;
            }
        }
    }
}

fn refresh_model(state: &mut ModelRuntimeState, now: u64) {
    if state.cooldown_until.is_some_and(|until| now >= until) {
        if state.breaker == BreakerState::Open {
            state.breaker = BreakerState::HalfOpen;
            state.availability = RuntimeAvailability::HalfOpen;
        } else {
            state.availability = RuntimeAvailability::Available;
        }
        state.cooldown_until = None;
        if matches!(
            state.quota_state,
            QuotaState::RateLimited | QuotaState::Exhausted
        ) {
            state.quota_state = QuotaState::Unknown;
        }
    }
}

fn effective_availability(
    provider: &ProviderRuntimeState,
    connection: &ConnectionRuntimeState,
    model: &ModelRuntimeState,
) -> RuntimeAvailability {
    for availability in [
        provider.availability,
        connection.availability,
        model.availability,
    ] {
        if !availability.is_selectable() {
            return availability;
        }
    }
    if [
        provider.availability,
        connection.availability,
        model.availability,
    ]
    .contains(&RuntimeAvailability::Degraded)
    {
        RuntimeAvailability::Degraded
    } else {
        RuntimeAvailability::Available
    }
}

fn effective_breaker(
    provider: &ProviderRuntimeState,
    connection: &ConnectionRuntimeState,
    model: &ModelRuntimeState,
) -> BreakerState {
    [provider.breaker, connection.breaker, model.breaker]
        .into_iter()
        .find(|breaker| matches!(breaker, BreakerState::Open | BreakerState::HalfOpen))
        .or_else(|| {
            [provider.breaker, connection.breaker, model.breaker]
                .into_iter()
                .find(|breaker| *breaker == BreakerState::Degraded)
        })
        .unwrap_or(BreakerState::Closed)
}

fn percentile(samples: &[u64], percentile: u64) -> Option<u64> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let index = ((sorted.len() - 1) as u64 * percentile / 100) as usize;
    sorted.get(index).copied()
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

static RUNTIME_REGISTRY: OnceLock<Mutex<RuntimeRegistry>> = OnceLock::new();

fn global() -> std::sync::MutexGuard<'static, RuntimeRegistry> {
    RUNTIME_REGISTRY
        .get_or_init(|| Mutex::new(RuntimeRegistry::default()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn clear() {
    global().clear();
}

pub fn note_request(model_id: &str) -> Result<(), &'static str> {
    global().note_request(model_id)
}

pub fn observe(model_id: &str, observation: RuntimeObservation) -> Result<(), &'static str> {
    global().observe(model_id, observation)
}

pub fn set_auth_state(model_id: &str, state: AuthState) -> Result<(), &'static str> {
    global().set_auth_state(model_id, state)
}

pub fn note_quota(
    model_id: &str,
    remaining: Option<String>,
    reset: Option<String>,
) -> Result<(), &'static str> {
    global().note_quota(model_id, remaining, reset)
}

pub fn note_quota_telemetry(
    model_id: &str,
    telemetry: QuotaTelemetry,
) -> Result<(), &'static str> {
    global().note_quota_telemetry(model_id, telemetry)
}

pub fn mark_static_quota_usage_unknown(model_id: &str) -> Result<(), &'static str> {
    global().mark_static_quota_usage_unknown(model_id)
}

pub fn set_provider_enabled(provider_id: &str, enabled: bool) -> Result<(), &'static str> {
    global().set_provider_enabled(provider_id, enabled)
}

pub fn set_model_enabled(model_id: &str, enabled: bool) -> Result<(), &'static str> {
    global().set_model_enabled(model_id, enabled)
}

pub fn snapshot(model_id: &str) -> Option<CanonicalRuntimeState> {
    global().snapshot(model_id)
}

pub fn model_runtime_view(model_id: &str) -> Option<ModelRuntimeView> {
    global().model_runtime_view(model_id)
}

pub fn quota_persistence_snapshot() -> Vec<PersistedQuotaTelemetry> {
    let mut registry = global();
    model_registry::MODELS
        .iter()
        .filter(|definition| definition.execution_lane != ExecutionLane::Local)
        .filter_map(|definition| {
            registry.model_runtime_view(definition.canonical_model_id)
                .and_then(|view| view.quota_telemetry())
                .filter(|telemetry| telemetry.is_useful())
                .map(|telemetry| PersistedQuotaTelemetry {
                    canonical_model_id: definition.canonical_model_id.to_string(),
                    telemetry,
                })
        })
        .collect()
}

pub fn restore_quota_persistence(entries: Vec<PersistedQuotaTelemetry>) {
    let mut registry = global();
    for entry in entries {
        let _ = registry.note_quota_telemetry(&entry.canonical_model_id, entry.telemetry);
    }
}

pub fn admit_probe(model_id: &str) -> bool {
    global().admit_probe(model_id)
}

pub fn deadline_after(duration: std::time::Duration) -> u64 {
    unix_millis().saturating_add(duration.as_millis() as u64)
}

pub fn remaining_duration(deadline: Option<u64>) -> Option<std::time::Duration> {
    let remaining = deadline?.saturating_sub(unix_millis());
    Some(std::time::Duration::from_millis(remaining))
}

/// Converts a registry deadline to the legacy in-process `Instant` shape.
/// This is projection-only; callers must never persist or write it back.
pub fn instant_from_deadline(deadline: u64) -> Option<std::time::Instant> {
    let remaining = remaining_duration(Some(deadline))?;
    Some(std::time::Instant::now() + remaining)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_registry::{ModelLifecycle, RegistryModelState};

    #[test]
    fn model_rate_limit_does_not_take_down_provider_or_connection() {
        let mut registry = RuntimeRegistry::default();
        let mut observation =
            RuntimeObservation::outcome(RuntimeOutcome::RateLimited, OutcomeScope::Model);
        observation.cooldown_until = Some(deadline_after(std::time::Duration::from_secs(60)));
        registry
            .observe(model_registry::DEEPSEEK.canonical_model_id, observation)
            .unwrap();
        let state = registry
            .snapshot(model_registry::DEEPSEEK.canonical_model_id)
            .unwrap();
        assert_eq!(state.provider.availability, RuntimeAvailability::Available);
        assert_eq!(
            state.connection.availability,
            RuntimeAvailability::Available
        );
        assert_eq!(state.model.availability, RuntimeAvailability::RateLimited);
        let sibling = registry
            .snapshot(model_registry::NEX_N2_5_PRO.canonical_model_id)
            .unwrap();
        assert_eq!(
            sibling.connection.availability,
            RuntimeAvailability::Available
        );
        assert_eq!(sibling.model.availability, RuntimeAvailability::Available);
    }

    #[test]
    fn model_runtime_view_exposes_registry_without_mutable_state() {
        let mut registry = RuntimeRegistry::default();
        let mut observation =
            RuntimeObservation::outcome(RuntimeOutcome::RateLimited, OutcomeScope::Model);
        observation.cooldown_until = Some(deadline_after(std::time::Duration::from_secs(60)));
        registry
            .observe(model_registry::DEEPSEEK.canonical_model_id, observation)
            .unwrap();

        let view = registry
            .model_runtime_view(model_registry::DEEPSEEK.canonical_model_id)
            .unwrap();
        assert_eq!(
            view.effective_availability(),
            RuntimeAvailability::RateLimited
        );
        assert!(!view.is_selectable());
        assert!(view.cooldown_until().is_some());
    }

    #[test]
    fn authentication_failure_is_connection_scoped_and_not_benchmark_state() {
        let mut registry = RuntimeRegistry::default();
        registry
            .observe(
                model_registry::MINISTRAL.canonical_model_id,
                RuntimeObservation::outcome(
                    RuntimeOutcome::AuthenticationFailed,
                    OutcomeScope::Connection,
                ),
            )
            .unwrap();
        let state = registry
            .snapshot(model_registry::CODESTRAL.canonical_model_id)
            .unwrap();
        assert_eq!(
            state.connection.availability,
            RuntimeAvailability::AuthFailed
        );
        assert_eq!(state.model.availability, RuntimeAvailability::Available);
        assert_eq!(
            model_registry::MINISTRAL.benchmark_profile.work_quality,
            Some(92.4)
        );
    }

    #[test]
    fn settings_reconciliation_does_not_clear_runtime_failures() {
        let mut registry = RuntimeRegistry::default();
        let mut observation =
            RuntimeObservation::outcome(RuntimeOutcome::ServiceUnavailable, OutcomeScope::Provider);
        observation.cooldown_until = Some(deadline_after(std::time::Duration::from_secs(60)));
        registry
            .observe(model_registry::DEEPSEEK.canonical_model_id, observation)
            .unwrap();
        registry.set_provider_enabled("openrouter", true).unwrap();
        assert_eq!(
            registry
                .snapshot(model_registry::DEEPSEEK.canonical_model_id)
                .unwrap()
                .provider
                .availability,
            RuntimeAvailability::TemporarilyUnavailable
        );
    }

    #[test]
    fn runtime_observations_never_change_lifecycle() {
        let mut registry = RuntimeRegistry::default();
        let mut candidate = RegistryModelState::from_definition(&model_registry::NEX_N2_5_PRO);
        registry
            .observe(
                model_registry::NEX_N2_5_PRO.canonical_model_id,
                RuntimeObservation::outcome(RuntimeOutcome::Success, OutcomeScope::Model),
            )
            .unwrap();
        assert_eq!(candidate.lifecycle, ModelLifecycle::Candidate);

        let active_lifecycle = model_registry::DEEPSEEK.lifecycle;
        let mut timeout =
            RuntimeObservation::outcome(RuntimeOutcome::Timeout, OutcomeScope::Connection);
        timeout.cooldown_until = Some(deadline_after(std::time::Duration::from_secs(30)));
        registry
            .observe(model_registry::DEEPSEEK.canonical_model_id, timeout)
            .unwrap();
        assert_eq!(model_registry::DEEPSEEK.lifecycle, active_lifecycle);
        assert_eq!(active_lifecycle, ModelLifecycle::Active);
        assert_eq!(
            registry
                .snapshot(model_registry::DEEPSEEK.canonical_model_id)
                .unwrap()
                .connection
                .availability,
            RuntimeAvailability::TemporarilyUnavailable
        );

        candidate.transition_to(ModelLifecycle::Retired).unwrap();
        registry
            .observe(
                model_registry::NEX_N2_5_PRO.canonical_model_id,
                RuntimeObservation::outcome(RuntimeOutcome::Success, OutcomeScope::Model),
            )
            .unwrap();
        assert_eq!(candidate.lifecycle, ModelLifecycle::Retired);
    }

    #[test]
    fn serialized_runtime_state_contains_no_secret_material() {
        let mut registry = RuntimeRegistry::default();
        registry
            .snapshot(model_registry::DEEPSEEK.canonical_model_id)
            .unwrap();
        let serialized = serde_json::to_string(
            &registry
                .snapshot(model_registry::DEEPSEEK.canonical_model_id)
                .unwrap(),
        )
        .unwrap();
        assert!(!serialized.contains("OPENROUTER_API_KEY"));
        assert!(!serialized.contains("Bearer"));
        assert!(!serialized.contains("sk-"));
        assert!(!serialized.to_lowercase().contains("secret"));
    }

    #[test]
    fn outcome_taxonomy_is_complete_and_stable() {
        let outcomes = [
            RuntimeOutcome::Success,
            RuntimeOutcome::EmptyResponse,
            RuntimeOutcome::RateLimited,
            RuntimeOutcome::QuotaExhausted,
            RuntimeOutcome::Timeout,
            RuntimeOutcome::ServiceUnavailable,
            RuntimeOutcome::AuthenticationFailed,
            RuntimeOutcome::CostBlocked,
            RuntimeOutcome::PrivacyBlocked,
            RuntimeOutcome::CapabilityMismatch,
            RuntimeOutcome::QualityFailure,
            RuntimeOutcome::Cancelled,
        ];
        assert_eq!(
            outcomes.map(RuntimeOutcome::as_str),
            [
                "success",
                "empty_response",
                "rate_limited",
                "quota_exhausted",
                "timeout",
                "service_unavailable",
                "authentication_failed",
                "cost_blocked",
                "privacy_blocked",
                "capability_mismatch",
                "quality_failure",
                "cancelled",
            ]
        );
    }

    #[test]
    fn expired_cooldown_never_recovers_failed_authentication() {
        let mut registry = RuntimeRegistry::default();
        let mut observation = RuntimeObservation::outcome(
            RuntimeOutcome::AuthenticationFailed,
            OutcomeScope::Connection,
        );
        observation.cooldown_until = Some(1);
        registry
            .observe(model_registry::MINISTRAL.canonical_model_id, observation)
            .unwrap();

        let state = registry
            .snapshot(model_registry::CODESTRAL.canonical_model_id)
            .unwrap();
        assert_eq!(
            state.connection.availability,
            RuntimeAvailability::AuthFailed
        );
        assert_eq!(state.connection.auth_state, AuthState::Failed);
        assert_eq!(state.model.availability, RuntimeAvailability::Available);
    }

    #[test]
    fn legacy_error_text_uses_one_shared_outcome_taxonomy() {
        assert_eq!(
            classify_error_message("HTTP 429: RESOURCE_EXHAUSTED"),
            RuntimeOutcome::RateLimited
        );
        assert_eq!(
            classify_error_message("free quota exhausted"),
            RuntimeOutcome::QuotaExhausted
        );
        assert_eq!(
            classify_error_message("401 unauthorized"),
            RuntimeOutcome::AuthenticationFailed
        );
        assert_eq!(
            classify_error_message("leere Antwort"),
            RuntimeOutcome::EmptyResponse
        );
    }

    #[test]
    fn breaker_transitions_open_half_open_and_allows_one_probe() {
        let mut registry = RuntimeRegistry::default();
        let id = model_registry::DEEPSEEK.canonical_model_id;
        let mut failure =
            RuntimeObservation::outcome(RuntimeOutcome::ServiceUnavailable, OutcomeScope::Provider);
        failure.cooldown_until = Some(1);
        registry.observe(id, failure).unwrap();
        let mut second_failure =
            RuntimeObservation::outcome(RuntimeOutcome::ServiceUnavailable, OutcomeScope::Provider);
        second_failure.cooldown_until = Some(1);
        registry.observe(id, second_failure).unwrap();

        let state = registry.snapshot(id).unwrap();
        assert_eq!(state.provider.breaker, BreakerState::HalfOpen);
        assert_eq!(state.effective_availability, RuntimeAvailability::HalfOpen);
        assert!(registry.admit_probe(id));
        assert!(!registry.admit_probe(id));

        registry
            .observe(
                id,
                RuntimeObservation::outcome(RuntimeOutcome::Success, OutcomeScope::Provider),
            )
            .unwrap();
        let state = registry.snapshot(id).unwrap();
        assert_eq!(state.provider.breaker, BreakerState::Closed);
        assert_eq!(state.effective_availability, RuntimeAvailability::Available);
    }

    #[test]
    fn telemetry_and_latency_samples_are_bounded_and_percentiled() {
        let mut registry = RuntimeRegistry::default();
        let id = model_registry::DEEPSEEK.canonical_model_id;
        registry.note_request(id).unwrap();
        for latency in 0..(MAX_LATENCY_SAMPLES as u64 + 10) {
            let mut observation =
                RuntimeObservation::outcome(RuntimeOutcome::Success, OutcomeScope::Model);
            observation.last_latency_ms = Some(latency);
            registry.observe(id, observation).unwrap();
        }
        let view = registry.model_runtime_view(id).unwrap();
        assert_eq!(view.request_count(), 1);
        assert_eq!(view.telemetry().successes, MAX_LATENCY_SAMPLES as u32 + 10);
        assert_eq!(
            view.state.model.latency_samples_ms.len(),
            MAX_LATENCY_SAMPLES
        );
        assert!(view.latency_p50_ms().is_some());
        assert!(view.latency_p95_ms().is_some());
    }

    #[test]
    fn quota_freshness_is_recorded_only_with_metadata() {
        let mut registry = RuntimeRegistry::default();
        let id = model_registry::DEEPSEEK.canonical_model_id;
        registry.note_quota(id, None, None).unwrap();
        assert!(registry
            .snapshot(id)
            .unwrap()
            .connection
            .quota_freshness
            .is_none());
        registry
            .note_quota(id, Some("7".to_string()), Some("tomorrow".to_string()))
            .unwrap();
        let state = registry.snapshot(id).unwrap();
        assert!(state.connection.quota_freshness.is_some());
        assert_eq!(state.connection.quota_remaining.as_deref(), Some("7"));
    }

    #[test]
    fn shared_quota_is_not_duplicated_and_live_evidence_wins() {
        let mut registry = RuntimeRegistry::default();
        let static_cap = QuotaTelemetry {
            source: "static_limit".into(), scope: "provider".into(), neuron_limit: Some(10_000),
            reset_at: Some("00:00 UTC".into()), ..QuotaTelemetry::default()
        };
        registry.note_quota_telemetry(model_registry::MINISTRAL.canonical_model_id, static_cap).unwrap();
        let sibling = registry.model_runtime_view(model_registry::CODESTRAL.canonical_model_id).unwrap();
        assert_eq!(sibling.quota_telemetry().unwrap().neuron_limit, Some(10_000));

        let live = QuotaTelemetry {
            source: "provider_live".into(), scope: "provider".into(), request_limit: Some(1000),
            request_remaining: Some(742), ..QuotaTelemetry::default()
        };
        registry.note_quota_telemetry(model_registry::CODESTRAL.canonical_model_id, live).unwrap();
        let first = registry.model_runtime_view(model_registry::MINISTRAL.canonical_model_id).unwrap();
        assert_eq!(first.quota_telemetry().unwrap().request_remaining, Some(742));
        registry.note_quota_telemetry(model_registry::MINISTRAL.canonical_model_id, QuotaTelemetry {
            source: "static_limit".into(), scope: "provider".into(), neuron_limit: Some(1), ..QuotaTelemetry::default()
        }).unwrap();
        assert_eq!(registry.model_runtime_view(model_registry::CODESTRAL.canonical_model_id).unwrap().quota_telemetry().unwrap().request_remaining, Some(742));
    }

    #[test]
    fn static_cap_starts_with_a_local_zero_baseline_and_never_overrides_live() {
        let mut registry = RuntimeRegistry::default();
        let id = model_registry::CLOUDFLARE_GLM_4_7_FLASH.canonical_model_id;
        registry.note_quota_telemetry(id, QuotaTelemetry {
            source: "static_limit".into(), scope: "account".into(), neuron_limit: Some(10_000),
            reset_at: Some("00:00 UTC".into()), ..QuotaTelemetry::default()
        }).unwrap();
        let baseline = registry.model_runtime_view(id).unwrap().quota_telemetry().unwrap();
        assert!(baseline.local_usage_observed);
        assert_eq!(baseline.locally_observed_used, Some(0));
        registry.mark_static_quota_usage_unknown(id).unwrap();
        assert_eq!(registry.model_runtime_view(id).unwrap().quota_telemetry().unwrap().locally_observed_used, None);
        registry.note_quota_telemetry(id, QuotaTelemetry {
            source: "provider_live".into(), scope: "account".into(), neuron_limit: Some(10_000),
            neuron_remaining: Some(7_820), ..QuotaTelemetry::default()
        }).unwrap();
        let live = registry.model_runtime_view(id).unwrap().quota_telemetry().unwrap();
        assert_eq!(live.source, "provider_live");
        assert_eq!(live.neuron_remaining, Some(7_820));
    }

    #[test]
    fn quota_persistence_round_trip_keeps_only_safe_metadata() {
        let entry = PersistedQuotaTelemetry {
            canonical_model_id: model_registry::GROQ_GPT_OSS.canonical_model_id.into(),
            telemetry: QuotaTelemetry {
                source: "provider_live".into(), scope: "account".into(), observed_at: 123,
                request_limit: Some(1000), request_remaining: Some(742),
                reset_at: Some("in 1 h".into()), ..QuotaTelemetry::default()
            },
        };
        let encoded = serde_json::to_string(&vec![entry]).unwrap();
        assert!(!encoded.contains("key") && !encoded.contains("Bearer"));
        let restored: Vec<PersistedQuotaTelemetry> = serde_json::from_str(&encoded).unwrap();
        let mut registry = RuntimeRegistry::default();
        registry.note_quota_telemetry(
            &restored[0].canonical_model_id,
            restored[0].telemetry.clone(),
        ).unwrap();
        assert_eq!(registry.model_runtime_view(model_registry::GROQ_GPT_OSS.canonical_model_id).unwrap().quota_telemetry().unwrap().request_remaining, Some(742));
    }
}
