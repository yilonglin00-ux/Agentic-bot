# Noki routing architecture contract

This contract establishes policy boundaries and the incremental canonical
model registry without replacing the productive router or its static chains.

## Request contract

```text
Task
  -> Privacy
  -> immutable ExecutionLane
  -> Shadow hard filters + deterministic rank (metadata only)
  -> Existing static router (productive)
  -> Quality
  -> Same-Lane Fallback
  -> Local Floor
```

Privacy is monotonic: a layer may mark a request sensitive, but no later layer
may clear that decision. Document, MCP, web and model-generated content cannot
authorize a wider lane.

## Execution lanes

- `LOCAL`: local candidates only.
- `FREE_CLOUD`: `VerifiedFreeHardStop` candidates, then other allowed free
  candidates, then the local floor.
- `PAID_CLOUD`: `MeteredPaid` candidates only after explicit authorization,
  then the local floor. It is present structurally but is not authorized by
  default and has no productive provider in this phase.

There is no `FREE_CLOUD -> PAID_CLOUD` transition. A 429, 503, timeout, exhausted
quota or quality failure can only select another candidate in the already fixed
lane or the local floor.

`FreeButBillingUncertain` always fails closed in `FREE_CLOUD`.

## Canonical model registry and stable roles

`model_registry.rs` is the canonical source for provider/model identity, exact
model version, connection identity, lane, cost class, capabilities, context,
benchmark status/profile and lifecycle. Lifecycle is one of
`active`, `candidate`, `parked`, `unavailable` or `retired`.

Routing asks for a stable role, never for a vendor name:

- Work: `Fast`, `Balanced`, `Deep`
- Coding: `Fast`, `Normal`, `Complex`
- Work and Coding each have an explicit local-floor role

Each role plan contains a current champion, zero or more challengers/fallbacks,
and a local floor. The current assignments preserve the pre-registry chain
order exactly. They are bootstrap assignments, not an incumbency bonus.

| Source | Current responsibility | Registry adapter |
|---|---|---|
| `model_registry.rs` | Canonical metadata, role assignments, lifecycle and promotion policy | New source of truth; performs no execution or discovery |
| `runtime_registry.rs` | Canonical provider, connection and exact-model runtime state plus the shared outcome taxonomy | Accepts observations only; never changes lifecycle or benchmark evidence |
| `router.rs` | Privacy, execution and existing fallback semantics | Builds chains from role plans and exposes its legacy state payload as a runtime-registry adapter |
| `cloud_engine.rs` | Provider execution, legacy rankings and benchmark harness | Builds configs from registry and mirrors execution observations to canonical runtime state; only validated Benchmark-v2 evidence may support promotion |
| `providers.rs` | Existing UI/provider contract | Builds local `ModelInfo` capabilities and identities from registry definitions |
| `specialist.rs` | Existing concrete API/CLI routes and specialist policy | Reads connection IDs, endpoint/auth references, labels, capabilities and lifecycle from registry connection definitions; uses the shared outcome taxonomy |

## Champion, challenger and discovery contract

External rankings and later OmniRoute discovery may populate discovery metadata
(quality/work/coding ranks, task fit, context, tool/reasoning claims, advertised
cost, source and verification time). Discovery metadata has no conversion path
to an active assignment.

Promotion is fail-closed and requires candidate lifecycle, matching immutable
lane, allowed cost, privacy permission, usable provider, sufficient capability
and context, a complete validated Benchmark-v2 result for the role's exact
suite/version/model configuration/case fingerprints, and an explicit material
benefit (champion improvement or a genuinely new specialist role).

Parking, unavailability and retirement only change lifecycle. Benchmark history
is append-only and is never deleted by demotion.

## Runtime-state contract

Runtime state is keyed by `provider_id + connection_id + canonical_model_id +
exact_model_version` and split into three scopes:

- Provider: provider-wide availability, failures and cooldown.
- Connection: one account/key/endpoint, including auth and quota state. Registry
  metadata stores only env-var/keychain reference names, never secret values.
- Model: the exact version on that connection, including model availability,
  last outcome and last latency.

The effective candidate state is the most restrictive of these three scopes.
An observation cannot change model lifecycle or benchmark history. In
particular, an active model may be temporarily rate-limited, a candidate stays
a candidate after a successful request, and a retired model cannot reactivate
itself.

The shared outcomes are `success`, `empty_response`, `rate_limited`,
`quota_exhausted`, `timeout`, `service_unavailable`, `authentication_failed`,
`cost_blocked`, `privacy_blocked`, `capability_mismatch`, `quality_failure` and
`cancelled`. A model-specific 429 is model-scoped; account quota and auth are
connection-scoped; a provider-wide 503 is provider-scoped. These outcomes are
runtime facts, not benchmark scores.

Parked catalog entries use an explicit reason: `benchmark_pending`,
`rate_limited`, `free_endpoint_missing`, `cost_uncertain`,
`credentials_required` or `manually_disabled`. Parked lifecycle and temporary
runtime health remain separate.

## Remaining compatibility state

- `router.rs` retains the legacy `ProviderState` and serialized status shape as
  adapters for existing commands/UI, but their state is derived from the
  canonical runtime registry.
- `cloud_engine.rs` retains its `CloudProvider` health counters and static
  rank/score maps for compatibility. Requests now mirror canonical observations;
  removing those duplicate counters is a later, UI-compatible migration.
- `specialist.rs` retains concrete request construction, process execution and
  its legacy policy enums. These are execution concerns, not registry metadata.
- `model_manager.rs` still owns the local runtime/load constants
  `CHAT_MODEL_FAST`, `CHAT_MODEL_NORMAL`, `CHAT_MODEL_FALLBACK`,
  `CHAT_MODEL_CANDIDATE`, `CODE_MODEL_DEFAULT`, `CODE_MODEL_7B` and
  `CODE_MODEL_14B`. GGUF, llama.cpp and load/unload behavior are deliberately
  not migrated in this phase.
- Parked catalog entries that predate an exact canonical model keep an optional
  model ID rather than inventing an identity.

Normal tests must not launch real provider or external CLI probes. Such probes
are explicitly `ignored` and manually opt-in. Provider discovery, paid
providers, automatic promotion, exploration, runtime learning and UI redesign
remain outside this phase.

## Phase 7: dynamic ranker in shadow mode

The dynamic ranker now runs beside the productive static router. It reads only
canonical registry, runtime and credential-presence metadata. It never executes
a provider request and its recommendation cannot change productive selection or
fallback order.

Hard filters run before scoring: task role, immutable lane, privacy, CostSafety,
enabled/lifecycle state, credential presence, capability/context requirements,
benchmark status/identity, effective runtime availability, breaker/cooldown,
quota and auth. `LOCAL` admits only local definitions; `FREE_CLOUD` never admits
paid definitions. `OPEN` and `HALF_OPEN` are not shadow candidates; HALF_OPEN
remains reserved for the runtime registry's controlled single probe.

Ranking keeps benchmark suitability separate from runtime executability. Work
roles read only Work evidence and Coding roles only Coding evidence. Canonical
v2 confidence requires the exact suite/version/harness/model/case fingerprint.
Legacy bootstrap evidence remains visible but carries no canonical fingerprint;
a legacy challenger cannot displace its role champion automatically. There is
no numeric incumbency bonus.

The deterministic weighted components are benchmark quality, benchmark
availability/completion, runtime success rate, empty/quality stability,
health/breaker, runtime p95 latency, quota freshness, capability/context and,
last, cost within the already selected lane. FAST emphasizes sufficient quality,
reliability and p95; NORMAL emphasizes benchmark quality and reliability;
DEEP/COMPLEX emphasizes quality, capability/context and stability. Ties resolve
by benchmark confidence, completion, lower p95, then canonical model ID.

Every route audit includes metadata-only static/dynamic selection, candidates,
hard-filter reasons, benchmark score/confidence, reliability, p95, quota, final
rank and fallback count. Prompt and response content are excluded. Nex and
Nemotron remain candidate-pending-canonical and are neither promoted nor added
to productive chains.

## Phase 7.5: shadow validation

Synthetic runtime oracles cover healthy, degraded, open and half-open breakers;
429, exhausted and stale quota; empty, timeout and quality-failure observations;
runtime p95 differences; canonical versus legacy confidence; invalid benchmark
identity; FREE/PAID and LOCAL lane boundaries; and sensitive requests. Each
oracle fixes the eligible set, filtered set and reason, complete ranking,
winner, and fallback order. The local floor is always terminal within a cloud
lane.

Validation divergence records contain only `static_choice`, `dynamic_choice`,
`same`, `divergence_reason`, and `oracle_correct`. No prompt, response, document,
credential, or provider call is involved.

`ready_for_controlled_activation` is true only when security/lane/privacy,
benchmark identity, every scenario oracle, deterministic ranking, terminal
Local Floor, no-unverified-promotion, and zero-external-call checks all pass.
This gate is validation metadata only and does not activate productive dynamic
routing.

The five known unrelated failures are classified as
`pre_existing_environment_dependent`: Cloud Projection, Photos, and the three
Web/DNS tests. None is classified as `routing_regression`, and Phase 7.5 does
not modify or repair them.

## Phase 8: controlled production activation

`RouteRequest::routing_mode` is the explicit production switch. Its default is
`DYNAMIC`; setting `NOKI_ROUTING_MODE=STATIC` (or explicitly setting the request
mode) immediately restores the complete previous chain behavior without a code
rollback or removal of those chains. Missing and invalid environment values
resolve to `DYNAMIC`.

Dynamic routing consumes the single validated Phase 7 rank report directly:
privacy and the immutable execution lane are resolved first, hard filters and
ranking produce one frozen candidate list, and execution walks that list once
and sequentially. A runtime failure updates the canonical Runtime Registry and
advances to the next already-ranked candidate in the same lane, subject to the
existing cloud-attempt and quality-switch limits. The class-specific Local
Floor remains terminal. No re-ranking, parallel call, FREE-to-PAID escalation,
or automatic lifecycle promotion occurs during a route.

The production audit remains metadata-only and adds routing mode, frozen ranked
candidate IDs, filter/fallback reasons, rank factors, canonical runtime outcome,
fallback count, and Local Floor usage. Prompts and responses are not recorded.
Nex and Nemotron remain non-productive pending canonical benchmark evidence.
