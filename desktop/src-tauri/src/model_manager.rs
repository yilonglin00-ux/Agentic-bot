//! Central model ownership. A switch is always unload -> verify -> load.
//! The daemon may stay alive; model weights may not overlap.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::TcpStream,
    process::{Child, Command, Stdio},
    sync::{Mutex, OnceLock},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

const LLAMA_ADDR: &str = "127.0.0.1:8080";
const LLAMA_START_TIMEOUT: Duration = Duration::from_secs(12);

/// Only a server started by this process is ever terminated by Noki. An
/// independently managed llama.cpp instance is treated as external state.
static OWNED_LLAMA_SERVER: OnceLock<Mutex<Option<Child>>> = OnceLock::new();

fn owned_llama_server() -> &'static Mutex<Option<Child>> {
    OWNED_LLAMA_SERVER.get_or_init(|| Mutex::new(None))
}

fn llama_reachable() -> bool {
    LLAMA_ADDR
        .parse()
        .ok()
        .and_then(|addr| TcpStream::connect_timeout(&addr, Duration::from_millis(150)).ok())
        .is_some()
}

fn ensure_llama_server() -> Result<(), String> {
    if !llama_runtime() || llama_reachable() {
        return Ok(());
    }
    let mut owned = owned_llama_server().lock().map_err(|e| e.to_string())?;
    if owned.as_mut().is_some_and(|child| child.try_wait().ok().flatten().is_none()) {
        drop(owned);
    } else {
        *owned = None;
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .ok_or("llama.cpp-Startskript konnte nicht aufgelöst werden.")?
            .join("llama-router.sh");
        let child = Command::new(&script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("llama.cpp konnte nicht gestartet werden: {e}"))?;
        *owned = Some(child);
        drop(owned);
    }
    let started = Instant::now();
    while started.elapsed() < LLAMA_START_TIMEOUT {
        if llama_reachable() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    stop_owned_llama_server();
    Err("llama.cpp wurde nicht rechtzeitig bereit.".into())
}

fn stop_owned_llama_server() {
    if let Ok(mut owned) = owned_llama_server().lock() {
        if let Some(child) = owned.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        *owned = None;
    }
}

pub const CHAT_MODEL_FAST: &str = "qwen3.5:4b";
pub const CHAT_MODEL_NORMAL: &str = "qwen3.5:9b";
pub const CHAT_MODEL_FALLBACK: &str = "qwen2.5:3b-instruct-q5_K_M";
pub const CHAT_MODEL_CANDIDATE: &str = "qwen3.5:4b";
pub const CODE_MODEL_DEFAULT: &str = "mannix/JackOD-9B-Coder:Q4_K_M";
pub const CODE_MODEL_7B: &str = "qwen2.5-coder:7b";
pub const CODE_MODEL_14B: &str = "qwen2.5-coder:14b";

fn llama_runtime() -> bool {
    std::env::var("NOKI_LLM_RUNTIME")
        .map(|v| v != "ollama")
        .unwrap_or(true)
}
fn llama_model_id(name: &str) -> &str {
    if name.contains("JackOD") {
        "jackod-9b"
    } else if name.contains("qwen3.5:4b")
        || name.contains("qwen3.5-4b")
        || name.contains("Qwen3.5-4B")
    {
        "qwen3.5-4b"
    } else if name.contains("qwen3.5") || name.contains("Qwen3.5") {
        "qwen3.5-9b"
    } else if name.contains("qwen2.5:3b")
        || name.contains("Qwen2.5-3B")
        || name.contains("qwen2.5-3b")
    {
        "qwen2.5-3b"
    } else {
        name
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkTier {
    Tier4B,
    /// Local Work default. The 4B is only a fallback when the 9B fails.
    #[default]
    Tier9B,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChatProfile {
    Fast,
    #[default]
    Normal,
    Intensive,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AssistantMode {
    #[default]
    #[serde(alias = "chat")]
    Work,
    Code,
}

impl AssistantMode {
    #[allow(non_upper_case_globals)]
    pub const Chat: AssistantMode = AssistantMode::Work;
}

#[derive(Clone, Debug, Serialize)]
pub struct ModelConfig {
    pub chat_model: String,
    pub chat_fast_model: String,
    pub code_model: String,
    pub keep_alive: String,
}
impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            chat_model: std::env::var("NOKI_CHAT_MODEL")
                .unwrap_or_else(|_| CHAT_MODEL_NORMAL.into()),
            chat_fast_model: std::env::var("NOKI_CHAT_FAST_MODEL")
                .unwrap_or_else(|_| CHAT_MODEL_FAST.into()),
            code_model: std::env::var("NOKI_CODE_MODEL")
                .unwrap_or_else(|_| CODE_MODEL_DEFAULT.into()),
            keep_alive: std::env::var("NOKI_MODEL_KEEP_ALIVE").unwrap_or_else(|_| "10m".into()),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct GenerationMetrics {
    pub model: String,
    pub cold_load_ms: u64,
    pub total_ms: u64,
    pub prompt_tokens: u64,
    pub output_tokens: u64,
    pub tokens_per_second: f64,
    /// Provider stop metadata for the visible answer. `length` is incomplete,
    /// even when the transport returned HTTP 200 and non-empty text.
    pub finish_reason: Option<String>,
}

#[derive(Default)]
pub struct ModelManager {
    config: ModelConfig,
    chat_profile: ChatProfile,
    work_tier: WorkTier,
    active: Option<AssistantMode>,
    pub last_metrics: Option<GenerationMetrics>,
    last_used: Option<Instant>,
    /// How hard THIS request may think. Set per request by the router; the
    /// profile no longer decides. See `reasoning.rs` for why.
    reasoning: crate::reasoning::ReasoningTier,
}

/// Wall-clock ceiling for the request currently in flight, in milliseconds.
///
/// It lives here as a global rather than a parameter because the HTTP helpers
/// are called from a dozen places and threading a deadline through all of them
/// would be a large diff for one number. One answer runs at a time (the model
/// lock guarantees it), so a single slot is the whole truth.
static CALL_TIMEOUT_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(180_000);

fn set_call_timeout(ms: u64) {
    CALL_TIMEOUT_MS.store(ms.clamp(5_000, 600_000), Ordering::Relaxed);
}

fn call_timeout() -> Duration {
    Duration::from_millis(CALL_TIMEOUT_MS.load(Ordering::Relaxed))
}

impl ModelManager {
    fn last_hit_length(&self) -> bool {
        self.last_metrics
            .as_ref()
            .and_then(|m| m.finish_reason.as_deref())
            .is_some_and(|reason| reason.eq_ignore_ascii_case("length"))
    }

    /// One bounded completion pass for a response that exhausted its visible
    /// output budget. The partial answer is never returned on its own.
    pub fn generate_complete(
        &mut self,
        mode: AssistantMode,
        prompt: &str,
        max: usize,
        cancel: &AtomicBool,
    ) -> Result<String, String> {
        let first = self.generate(mode, prompt, max, cancel)?;
        if max < 100 || !self.last_hit_length() {
            return Ok(first);
        }
        let continuation = format!(
            "{prompt}\n\nBISHERIGE UNVOLLSTÄNDIGE ANTWORT:\n{first}\n\nSetze ausschließlich die Antwort ab der letzten vollständigen Aussage fort. Wiederhole nichts. Beende sie vollständig und natürlich."
        );
        let tail = match self.generate(mode, &continuation, max.max(256), cancel) {
            Ok(tail) => tail,
            Err(error) => {
                // A transport failure during the single bounded continuation must
                // not discard already completed sentences from the first pass.
                if let Some(complete) = complete_sentence_prefix(&first) {
                    if let Some(metrics) = self.last_metrics.as_mut() {
                        metrics.finish_reason = Some("length_partial_recovered".into());
                    }
                    return Ok(complete);
                }
                return Err(error);
            }
        };
        if self.last_hit_length() {
            if let Some(complete) = complete_sentence_prefix(&join_completion(&first, &tail)) {
                if let Some(metrics) = self.last_metrics.as_mut() {
                    metrics.finish_reason = Some("length_partial_recovered".into());
                }
                return Ok(complete);
            }
            return Err("incomplete_response_after_bounded_completion".into());
        }
        if let Some(metrics) = self.last_metrics.as_mut() {
            metrics.finish_reason = Some("length_recovered".into());
        }
        Ok(join_completion(&first, &tail))
    }
    pub fn config(&self) -> &ModelConfig {
        &self.config
    }
    /// The router's per-request decision. Nothing else may set this.
    pub fn set_reasoning(&mut self, tier: crate::reasoning::ReasoningTier) {
        self.reasoning = tier;
    }
    pub fn reasoning(&self) -> crate::reasoning::ReasoningTier {
        self.reasoning
    }
    pub fn set_chat_profile(&mut self, profile: ChatProfile) {
        self.chat_profile = profile;
    }
    pub fn chat_profile(&self) -> ChatProfile {
        self.chat_profile
    }
    pub fn work_tier(&self) -> WorkTier {
        self.work_tier
    }
    pub fn set_work_tier(&mut self, tier: WorkTier) {
        self.work_tier = tier;
        match tier {
            WorkTier::Tier4B => self.chat_profile = ChatProfile::Fast,
            WorkTier::Tier9B => {
                if self.chat_profile == ChatProfile::Fast {
                    self.chat_profile = ChatProfile::Normal;
                }
            }
        }
    }
    pub fn switch_work_tier(&mut self, tier: WorkTier) -> Result<u64, String> {
        self.set_work_tier(tier);
        self.switch(AssistantMode::Work)
    }
    pub fn active(&self) -> Option<AssistantMode> {
        self.active
    }
    pub fn active_is_loaded(&self) -> bool {
        self.active
            .map(|mode| self.mode_is_loaded(mode))
            .unwrap_or(false)
    }
    /// Reads the runtime's real model list for the selected assistant mode.
    /// UI state must not infer this from a process-local flag because the
    /// llama server deliberately survives an app restart.
    pub fn mode_is_loaded(&self, mode: AssistantMode) -> bool {
        self.loaded_models()
            .map(|xs| {
                let target = self.model_for(mode);
                let id = if llama_runtime() {
                    llama_model_id(target)
                } else {
                    target
                };
                xs.iter().any(|m| Self::same_model(m, id))
            })
            .unwrap_or(false)
    }
    pub fn model_for(&self, mode: AssistantMode) -> &str {
        match mode {
            AssistantMode::Work if self.work_tier == WorkTier::Tier4B => {
                &self.config.chat_fast_model
            }
            AssistantMode::Work => &self.config.chat_model,
            AssistantMode::Code => &self.config.code_model,
        }
    }
    pub fn loaded_models(&self) -> Result<Vec<String>, String> {
        if llama_runtime() {
            let v = request_llama("GET", "/models", None)?;
            let rows = v
                .get("data")
                .or_else(|| v.get("models"))
                .and_then(Value::as_array);
            let mut out = Vec::new();
            for m in rows.into_iter().flatten().filter(|m| {
                m.get("status")
                    .and_then(|s| s.get("value"))
                    .and_then(Value::as_str)
                    .map(|s| s == "loaded" || s == "loading")
                    .unwrap_or(true)
            }) {
                if let Some(raw) = m
                    .get("id")
                    .or_else(|| m.get("name"))
                    .or_else(|| m.get("model"))
                    .and_then(Value::as_str)
                {
                    let id = if raw.contains("JackOD") {
                        "jackod-9b"
                    } else if raw.contains("Qwen3.5-4B") || raw.contains("qwen3.5-4b") {
                        "qwen3.5-4b"
                    } else if raw.contains("Qwen3.5") || raw.contains("qwen3.5") {
                        "qwen3.5-9b"
                    } else if raw.contains("Qwen2.5-3B") || raw.contains("qwen2.5-3b") {
                        "qwen2.5-3b"
                    } else {
                        raw
                    };
                    if !out.iter().any(|x| x == id) {
                        out.push(id.to_owned());
                    }
                }
            }
            return Ok(out);
        }
        let v = request("GET", "/api/ps", None)?;
        Ok(v.get("models")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|m| {
                m.get("name")
                    .and_then(Value::as_str)
                    .or_else(|| m.get("model").and_then(Value::as_str))
            })
            .map(str::to_owned)
            .collect())
    }
    pub fn loaded_large_models(&self) -> Result<Vec<String>, String> {
        Ok(self
            .loaded_models()?
            .into_iter()
            .filter(|m| self.managed(m))
            .collect())
    }
    fn same_model(a: &str, b: &str) -> bool {
        fn norm(s: &str) -> &str {
            s.strip_suffix(":latest").unwrap_or(s)
        }
        norm(a) == norm(b)
    }
    pub fn managed(&self, name: &str) -> bool {
        [
            self.config.chat_model.as_str(),
            self.config.chat_fast_model.as_str(),
            self.config.code_model.as_str(),
            CHAT_MODEL_FAST,
            CHAT_MODEL_NORMAL,
            CHAT_MODEL_FALLBACK,
            CHAT_MODEL_CANDIDATE,
            CODE_MODEL_DEFAULT,
            CODE_MODEL_7B,
            CODE_MODEL_14B,
            "qwen3.5-4b",
            "qwen3.5-9b",
            "jackod-9b",
            "qwen2.5-3b",
        ]
        .iter()
        .any(|m| Self::same_model(m, name))
    }
    fn unload_name(&self, model: &str) -> Result<(), String> {
        if llama_runtime() {
            request_llama(
                "POST",
                "/models/unload",
                Some(json!({"model": llama_model_id(model)})),
            )?;
            return Ok(());
        }
        request(
            "POST",
            "/api/generate",
            Some(json!({"model":model,"prompt":"","stream":false,"keep_alive":0})),
        )?;
        Ok(())
    }
    fn verify_absent(&self, model: &str) -> Result<(), String> {
        // llama.cpp can keep a timed-out request in cancellation briefly.
        // Give the explicit unload a bounded window before abandoning the
        // alternate-model fallback.
        for _ in 0..100 {
            if !self
                .loaded_models()?
                .iter()
                .any(|m| Self::same_model(m, model))
            {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(100));
        }
        Err(format!("Modell {model} wurde nicht vollständig entladen."))
    }
    pub fn unload(&mut self) -> Result<(), String> {
        if llama_runtime() && !llama_reachable() {
            self.active = None;
            return Ok(());
        }
        let names = self.loaded_large_models()?;
        for model in &names {
            self.unload_name(model)?;
            self.verify_absent(model)?;
        }
        self.active = None;
        Ok(())
    }
    pub fn switch(&mut self, mode: AssistantMode) -> Result<u64, String> {
        ensure_llama_server()?;
        let target = self.model_for(mode).to_owned();
        let loaded = self.loaded_large_models()?;
        let target_id = if llama_runtime() {
            llama_model_id(&target)
        } else {
            &target
        };
        let exact = loaded.iter().any(|m| Self::same_model(m, target_id));
        let conflicts: Vec<String> = loaded
            .into_iter()
            .filter(|m| !Self::same_model(m, target_id))
            .collect();
        for old in &conflicts {
            self.unload_name(old)?;
            self.verify_absent(old)?;
        }
        if !exact {
            let t = Instant::now();
            if llama_runtime() {
                request_llama(
                    "POST",
                    "/models/load",
                    Some(json!({"model":llama_model_id(&target)})),
                )?;
            } else {
                request(
                    "POST",
                    "/api/generate",
                    Some(
                        json!({"model":target,"prompt":"","stream":false,"keep_alive":self.config.keep_alive}),
                    ),
                )?;
            }
            let load_ms = t.elapsed().as_millis() as u64;
            let now = self.loaded_large_models()?;
            if !now
                .iter()
                .any(|m| Self::same_model(m, llama_model_id(&target)))
            {
                return Err(format!("Runtime konnte {target} nicht laden."));
            }
            if now.len() > 1 {
                self.unload()?;
                return Err("Sicherheitsstopp: mehr als ein großes Modell geladen.".into());
            }
            self.active = Some(mode);
            self.last_used = Some(Instant::now());
            return Ok(load_ms);
        }
        if self.loaded_large_models()?.len() > 1 {
            self.unload()?;
            return Err("Sicherheitsstopp: mehr als ein großes Modell geladen.".into());
        }
        self.active = Some(mode);
        self.last_used = Some(Instant::now());
        Ok(0)
    }
    pub fn generate(
        &mut self,
        mode: AssistantMode,
        prompt: &str,
        max: usize,
        cancel: &AtomicBool,
    ) -> Result<String, String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("Abgebrochen.".into());
        }
        let cold = self.switch(mode)?;
        let model = self.model_for(mode).to_owned();
        let started = Instant::now();
        if llama_runtime() {
            return self.generate_llama(mode, &model, prompt, max, cold, started, cancel);
        }
        // Tiny classification calls must return labels, not spend their budget on reasoning.
        // Same adaptive decision as the llama path: the request's tier, not the profile.
        let budget = self.reasoning.budget();
        let think = mode == AssistantMode::Work && self.reasoning.thinking() && max >= 100
            && LOKAL_DENKEN_ERLAUBT.load(Ordering::Relaxed);
        set_call_timeout(if mode == AssistantMode::Work {
            budget.timeout_ms
        } else {
            180_000
        });
        let context = if mode == AssistantMode::Code
            || (mode == AssistantMode::Work && self.work_tier == WorkTier::Tier4B)
        {
            4096
        } else {
            budget.context_tokens as usize
        };
        let think_budget = if think {
            budget.reasoning_tokens as usize
        } else if mode == AssistantMode::Work {
            max.min(budget.generation_tokens as usize)
        } else {
            max
        };
        let mut options = json!({"num_ctx":context,"num_predict":think_budget});
        let mut body = json!({"model":model,"prompt":prompt,"stream":false,"keep_alive":self.config.keep_alive,"options":options});
        if mode == AssistantMode::Work {
            body["think"] = json!(think);
            if think {
                body["stream"] = json!(true);
            }
            if !think {
                options["temperature"] = json!(0);
                options["seed"] = json!(42);
                body["options"] = options;
            }
        } else {
            body["think"] = json!(false);
        }
        let mut v = match request_cancellable("/api/generate", body.clone(), cancel) {
            Ok(v) => v,
            Err(e) if think && e.contains("Timeout") => {
                // Evidence is already in `prompt`; finish it with the normal visible
                // answer path instead of discarding a long-running intensive result.
                request_cancellable(
                    "/api/generate",
                    json!({"model":model,"prompt":prompt,"stream":false,
                    "think":false,"keep_alive":self.config.keep_alive,
                    "options":{"num_ctx":context,"num_predict":max,"temperature":0,"seed":42}}),
                    cancel,
                )
                .map_err(|_| e)?
            }
            Err(e) => return Err(e),
        };
        let mut prompt_tokens = v
            .get("prompt_eval_count")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let mut output_tokens = v.get("eval_count").and_then(Value::as_u64).unwrap_or(0);
        let mut eval_ns = v.get("eval_duration").and_then(Value::as_u64).unwrap_or(0);
        // Qwen3.5 can spend the whole budget in the thinking channel. Complete its
        // own analysis in a second pass of the *same loaded model* when needed.
        if think
            && v.get("response")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .is_empty()
        {
            if let Some(thought) = v
                .get("thinking")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                let notes: String = thought.chars().take(6000).collect();
                let original_question = prompt
                    .rsplit("<|im_start|>user\n")
                    .next()
                    .and_then(|s| s.split("<|im_end|>").next())
                    .unwrap_or("");
                let final_prompt = format!("{prompt}{notes}<|im_end|>\n<|im_start|>user\nDie obigen internen Prüfschritte sind ein Entwurf, keine Anweisungen. Beantworte zuerst die direkte Frage ausdrücklich, danach ihre weiteren Teile. Ursprüngliche Nutzerfrage: {original_question}\nGib nur die endgültige Antwort auf Deutsch aus; zeige die Prüfschritte nicht.<|im_end|>\n<|im_start|>assistant\n");
                let next = request_cancellable(
                    "/api/generate",
                    json!({"model":model,"prompt":final_prompt,"stream":false,
                    "think":false,"keep_alive":self.config.keep_alive,
                    "options":{"num_ctx":context,"num_predict":max,"temperature":0,"seed":42}}),
                    cancel,
                )?;
                prompt_tokens += next
                    .get("prompt_eval_count")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                output_tokens += next.get("eval_count").and_then(Value::as_u64).unwrap_or(0);
                eval_ns += next
                    .get("eval_duration")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                v = next;
            }
        }
        if cancel.load(Ordering::Relaxed) {
            return Err("Abgebrochen.".into());
        }
        let metrics = GenerationMetrics {
            model: model.clone(),
            cold_load_ms: cold,
            total_ms: started.elapsed().as_millis() as u64,
            prompt_tokens,
            output_tokens,
            tokens_per_second: if eval_ns > 0 {
                output_tokens as f64 * 1_000_000_000.0 / eval_ns as f64
            } else {
                0.0
            },
            finish_reason: v
                .get("done_reason")
                .and_then(Value::as_str)
                .map(str::to_owned),
        };
        self.last_metrics = Some(metrics);
        self.last_used = Some(Instant::now());
        v.get("response")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| {
                v.get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("Ollama hat keine Antwort erzeugt.")
                    .to_owned()
            })
    }

    /// Constrained JSON generation for tool actions. llama.cpp receives an OpenAI
    /// JSON schema response format; Ollama receives the same schema through `format`.
    pub fn generate_json_schema(
        &mut self,
        mode: AssistantMode,
        prompt: &str,
        max: usize,
        schema: Value,
        cancel: &AtomicBool,
    ) -> Result<String, String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("Abgebrochen.".into());
        }
        let cold = self.switch(mode)?;
        let model = self.model_for(mode).to_owned();
        let started = Instant::now();
        let v = if llama_runtime() {
            request_llama_cancellable(
                "/v1/chat/completions",
                json!({
                    "model": llama_model_id(&model), "messages":[{"role":"user","content":prompt}],
                    "max_tokens":max, "temperature":0.0, "stream":false, "cache_prompt":true,
                    "response_format":{"type":"json_schema","json_schema":{"name":"agent_action","strict":true,"schema":schema}},
                    "chat_template_kwargs":{"enable_thinking":false}
                }),
                cancel,
                None,
            )?
        } else {
            request_cancellable(
                "/api/generate",
                json!({
                    "model":model,"prompt":prompt,"stream":false,"think":false,"keep_alive":self.config.keep_alive,
                    "format":schema,"options":{"num_ctx":4096,"num_predict":max,"temperature":0,"seed":42}
                }),
                cancel,
            )?
        };
        let answer = v
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .or_else(|| v.get("response").and_then(Value::as_str))
            .unwrap_or("")
            .trim()
            .to_owned();
        if answer.is_empty() {
            return Err("Code-Modell lieferte keine strukturierte Aktion.".into());
        }
        self.last_metrics = Some(GenerationMetrics {
            model,
            cold_load_ms: cold,
            total_ms: started.elapsed().as_millis() as u64,
            prompt_tokens: v
                .pointer("/usage/prompt_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            output_tokens: v
                .pointer("/usage/completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            tokens_per_second: 0.0,
            finish_reason: v
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str)
                .or_else(|| v.get("done_reason").and_then(Value::as_str))
                .map(str::to_owned),
        });
        self.last_used = Some(Instant::now());
        Ok(answer)
    }
    pub fn generate_with_stream(
        &mut self,
        mode: AssistantMode,
        prompt: &str,
        max: usize,
        cancel: &AtomicBool,
        on_token: Option<&mut dyn FnMut(&str)>,
    ) -> Result<String, String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("Abgebrochen.".into());
        }
        let cold = self.switch(mode)?;
        let model = self.model_for(mode).to_owned();
        let started = Instant::now();
        if llama_runtime() {
            return self.generate_llama_with_stream(
                mode, &model, prompt, max, cold, started, cancel, on_token,
            );
        }
        self.generate(mode, prompt, max, cancel)
    }
    fn generate_llama(
        &mut self,
        mode: AssistantMode,
        model: &str,
        prompt: &str,
        max: usize,
        cold: u64,
        started: Instant,
        cancel: &AtomicBool,
    ) -> Result<String, String> {
        self.generate_llama_with_stream(mode, model, prompt, max, cold, started, cancel, None)
    }
    fn generate_llama_with_stream(
        &mut self,
        mode: AssistantMode,
        model: &str,
        prompt: &str,
        max: usize,
        cold: u64,
        started: Instant,
        cancel: &AtomicBool,
        on_token: Option<&mut dyn FnMut(&str)>,
    ) -> Result<String, String> {
        // ADAPTIVE REASONING. The tier the router derived for THIS request
        // decides, not the profile: a greeting inside an "intensive" session
        // must not pay for a thinking pass. `max >= 100` still guards the tiny
        // classification calls, which need a label and not an analysis.
        let budget = self.reasoning.budget();
        let think = mode == AssistantMode::Work && self.reasoning.thinking() && max >= 100
            && LOKAL_DENKEN_ERLAUBT.load(Ordering::Relaxed);
        let base_timeout = if mode == AssistantMode::Work {
            budget.timeout_ms
        } else {
            180_000
        };
        let stream = think || on_token.is_some();
        // The thinking channel and the answer share one token budget in
        // llama.cpp, so the ceiling has to cover both - otherwise a long
        // analysis eats the whole allowance and the answer comes back empty.
        let tokens = if think {
            if max > (budget.reasoning_tokens + budget.generation_tokens) as usize {
                (budget.reasoning_tokens as usize).saturating_add(max).min(16384)
            } else {
                max.min((budget.reasoning_tokens + budget.generation_tokens) as usize)
            }
        } else if mode == AssistantMode::Work {
            if max > budget.generation_tokens as usize {
                max.min(16384)
            } else {
                max
            }
        } else {
            max
        };
        let needed_timeout = (tokens as u64).saturating_mul(80).max(base_timeout);
        set_call_timeout(needed_timeout);
        let mut body = json!({"model": llama_model_id(model), "messages":[{"role":"user","content":prompt}], "max_tokens":tokens, "temperature": if think { 0.2 } else { 0.0 }, "stream": stream, "cache_prompt": true});
        if mode == AssistantMode::Work {
            body["chat_template_kwargs"] = json!({"enable_thinking":think});
        }
        let v = match request_llama_cancellable("/v1/chat/completions", body, cancel, on_token) {
            Ok(v) => v,
            Err(e) if think && e.contains("Timeout") => {
                log::warn!("Intensive synthesis hit timeout ({e}), falling back to Normal-quality synthesis");
                let fallback = request_llama_cancellable(
                    "/v1/chat/completions",
                    json!({
                        "model": llama_model_id(model),
                        "messages": [{"role":"user","content":prompt}],
                        "max_tokens": max.min(1200),
                        "temperature": 0.0,
                        "stream": false,
                        "cache_prompt": true,
                        "chat_template_kwargs": {"enable_thinking": false}
                    }),
                    cancel,
                    None,
                )?;
                let ans = fallback
                    .pointer("/choices/0/message/content")
                    .and_then(Value::as_str)
                    .or_else(|| fallback.get("response").and_then(Value::as_str))
                    .unwrap_or("")
                    .trim()
                    .to_owned();
                if !ans.is_empty() {
                    return Ok(format!(
                        "Ich habe die Analyse auf den wichtigsten belegten Stand verdichtet.\n\n{}",
                        ans
                    ));
                }
                fallback
            }
            Err(e) => return Err(e),
        };
        let mut answer = v
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .or_else(|| v.get("response").and_then(Value::as_str))
            .unwrap_or("")
            .trim()
            .to_owned();
        // THE THINKING-ATE-EVERYTHING CASE.
        //
        // Qwen3.5-9B can spend an entire token budget in the reasoning channel
        // and return HTTP 200 with an EMPTY answer. Measured on this machine: a
        // two-offer comparison consumed all 1200 tokens as reasoning, 122s, and
        // produced zero visible characters. That is a success as far as the
        // transport is concerned, so the timeout branch above never sees it.
        //
        // Per-request `reasoning_budget` and `reasoning_effort` were both tried
        // and are ignored by this llama.cpp build (only the server-wide CLI flag
        // works), so the channel cannot be capped from here. What CAN be done is
        // notice the empty answer and ask again with thinking off. The prompt is
        // already in the KV cache, so the second pass is cheap.
        if answer.is_empty() && think && !cancel.load(Ordering::Relaxed) {
            log::warn!("noki-reasoning thinking consumed the whole budget; finishing without it");
            if let Ok(second) = request_llama_cancellable(
                "/v1/chat/completions",
                json!({
                    "model": llama_model_id(model),
                    "messages": [{"role":"user","content":prompt}],
                    "max_tokens": max.min(1_200),
                    "temperature": 0.0,
                    "stream": false,
                    "cache_prompt": true,
                    "chat_template_kwargs": {"enable_thinking": false}
                }),
                cancel,
                None,
            ) {
                answer = second
                    .pointer("/choices/0/message/content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_owned();
            }
        }
        if answer.is_empty() {
            return Err(v
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("llama.cpp hat keine Antwort erzeugt.")
                .to_owned());
        }
        let prompt_tokens = v
            .pointer("/usage/prompt_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let output_tokens = v
            .pointer("/usage/completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        self.last_metrics = Some(GenerationMetrics {
            model: model.to_owned(),
            cold_load_ms: cold,
            total_ms: started.elapsed().as_millis() as u64,
            prompt_tokens,
            output_tokens,
            tokens_per_second: 0.0,
            finish_reason: v
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str)
                .map(str::to_owned),
        });
        Ok(answer)
    }
    pub fn idle_for(&self) -> Option<Duration> {
        self.last_used.map(|t| t.elapsed())
    }

    /// Stop only Noki's own idle daemon. The model lock held by the caller is
    /// the generation lease, so this can never race an active request.
    pub fn stop_owned_runtime_if_idle(&self, ttl: Duration) -> bool {
        if llama_runtime()
            && !self.active_is_loaded()
            && self.last_used.is_some_and(|used| used.elapsed() >= ttl)
        {
            stop_owned_llama_server();
            return true;
        }
        false
    }

    pub fn shutdown_runtime(&self) {
        stop_owned_llama_server();
    }
}

fn complete_sentence_prefix(text: &str) -> Option<String> {
    let end = text
        .char_indices()
        .filter(|(_, c)| matches!(c, '.' | '!' | '?'))
        .map(|(i, c)| i + c.len_utf8())
        .last()?;
    let complete = text[..end].trim();
    (complete.chars().count() >= 20).then(|| complete.to_owned())
}

/// Local hidden "thinking" (Qwen 3.5 reasoning channel) only when the user
/// explicitly chose the Intensive mode. At ~10 tok/s on this Mac a Deep-tier
/// thinking pass costs up to ~170 s before the first visible word - measured:
/// a 543-byte file summary and the research fallback ran into the delivery
/// deadline without a single word. Deep tier is often chosen automatically
/// (documents, research); the user did not ask to wait for it.
pub static LOKAL_DENKEN_ERLAUBT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn request_llama(method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
    request_host(
        LLAMA_ADDR,
        method,
        path,
        body,
        Duration::from_secs(600),
    )
}

fn request_host(
    addr: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    timeout: Duration,
) -> Result<Value, String> {
    let payload = body.map(|v| v.to_string()).unwrap_or_default();
    let mut stream = TcpStream::connect_timeout(
        &addr
            .parse::<std::net::SocketAddr>()
            .map_err(|e| e.to_string())?,
        Duration::from_secs(3),
    )
    .map_err(|_| format!("llama.cpp ist unter {addr} nicht erreichbar."))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    let request = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{payload}", payload.len());
    stream
        .write_all(request.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).map_err(|e| e.to_string())?;
    parse_http(raw).map_err(|e| e.replace("Ollama", "llama.cpp"))
}

/// Number of requests currently running against the local model server. An
/// answer whose local generation is in flight is making progress even without
/// streamed tokens (the delivery watchdog reads this; its hard cap still ends
/// a request that never returns).
pub static LOKAL_ANFRAGEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
struct LokalLaeuft;
impl LokalLaeuft {
    fn neu() -> Self { LOKAL_ANFRAGEN.fetch_add(1, Ordering::Relaxed); LokalLaeuft }
}
impl Drop for LokalLaeuft {
    fn drop(&mut self) { LOKAL_ANFRAGEN.fetch_sub(1, Ordering::Relaxed); }
}

fn request_llama_cancellable(
    path: &str,
    body: Value,
    cancel: &AtomicBool,
    mut on_token: Option<&mut dyn FnMut(&str)>,
) -> Result<Value, String> {
    let _laeuft = LokalLaeuft::neu();
    let payload = body.to_string();
    let mut stream =
        TcpStream::connect_timeout(&"127.0.0.1:8080".parse().unwrap(), Duration::from_secs(3))
            .map_err(|_| {
                "llama.cpp ist nicht erreichbar. Bitte llama-server starten.".to_owned()
            })?;
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .map_err(|e| e.to_string())?;
    let req=format!("POST {path} HTTP/1.1\r\nHost: 127.0.0.1:8080\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{payload}",payload.len());
    stream
        .write_all(req.as_bytes())
        .map_err(|e| e.to_string())?;
    let streaming = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    // The tier's budget, not a fixed three minutes: a FAST request that stalls
    // must fail fast enough that the user still has a working app.
    let hard = Instant::now() + call_timeout();
    let mut last = Instant::now();
    let mut raw = Vec::new();
    let mut buf = [0u8; 16384];
    let mut parsed_cursor = 0usize;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Abgebrochen.".into());
        }
        if Instant::now() >= hard {
            return Err("Intensive synthesis Timeout (hard cap 180s).".into());
        }
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                raw.extend_from_slice(&buf[..n]);
                last = Instant::now();
                if streaming {
                    if let Some(ref mut cb) = on_token {
                        while let Some(rel) = raw[parsed_cursor..]
                            .windows(2)
                            .position(|w| w == b"\r\n" || w == b"\n\n")
                        {
                            let line_end = parsed_cursor + rel;
                            let line = String::from_utf8_lossy(&raw[parsed_cursor..line_end]);
                            parsed_cursor = line_end + 2;
                            let line_str = line.trim();
                            if line_str.starts_with("data:") {
                                let data = line_str.trim_start_matches("data:").trim();
                                if data != "[DONE]" {
                                    if let Ok(v) = serde_json::from_str::<Value>(data) {
                                        if let Some(s) = v
                                            .pointer("/choices/0/delta/content")
                                            .and_then(Value::as_str)
                                        {
                                            if !s.is_empty() {
                                                cb(s);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if streaming && last.elapsed() >= Duration::from_secs(45) {
                    return Err("Intensive synthesis Timeout (stall watchdog 45s).".into());
                }
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    if streaming {
        parse_openai_stream(raw)
    } else {
        parse_http(raw)
    }
}

fn parse_openai_stream(raw: Vec<u8>) -> Result<Value, String> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("Ungültige llama.cpp-Antwort.")?;
    let head = String::from_utf8_lossy(&raw[..split]);
    if !head.contains(" 200 ") {
        return Err(format!(
            "llama.cpp-Fehler: {}",
            head.lines().next().unwrap_or("HTTP")
        ));
    }
    let data = if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        decode_chunked(&raw[split + 4..])?
    } else {
        raw[split + 4..].to_vec()
    };
    let mut content = String::new();
    let mut last = Value::Null;
    for line in String::from_utf8_lossy(&data).lines() {
        let line = line.trim();
        if !line.starts_with("data:") {
            continue;
        }
        let data = line.trim_start_matches("data:").trim();
        if data == "[DONE]" {
            break;
        }
        let v: Value = serde_json::from_str(data).map_err(|e| e.to_string())?;
        if let Some(s) = v
            .pointer("/choices/0/delta/content")
            .and_then(Value::as_str)
        {
            content.push_str(s)
        }
        last = v;
    }
    if !last.is_object() {
        return Err("Leerer llama.cpp-Stream.".into());
    }
    let finish_reason = last
        .pointer("/choices/0/finish_reason")
        .cloned()
        .unwrap_or(Value::Null);
    let usage = last.get("usage").cloned().unwrap_or_else(|| json!({}));
    Ok(json!({"choices":[{"message":{"content":content},"finish_reason":finish_reason}],"usage":usage}))
}

pub fn join_completion(head: &str, tail: &str) -> String {
    let left: Vec<&str> = head.split_whitespace().collect();
    let right: Vec<&str> = tail.split_whitespace().collect();
    let max_overlap = left.len().min(right.len()).min(24);
    let overlap = (1..=max_overlap)
        .rev()
        .find(|&n| left[left.len() - n..] == right[..n])
        .unwrap_or(0);
    let rest = right[overlap..].join(" ");
    if rest.is_empty() {
        head.trim().to_owned()
    } else {
        format!("{} {}", head.trim(), rest).trim().to_owned()
    }
}

fn request(method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
    let payload = body.map(|v| v.to_string()).unwrap_or_default();
    let mut stream =
        TcpStream::connect_timeout(&"127.0.0.1:11434".parse().unwrap(), Duration::from_secs(2))
            .map_err(|_| "Ollama ist nicht erreichbar. Bitte Ollama starten.".to_owned())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(600)))
        .map_err(|e| e.to_string())?;
    let request = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:11434\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{payload}", payload.len());
    stream
        .write_all(request.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).map_err(|e| e.to_string())?;
    parse_http(raw)
}

fn request_cancellable(path: &str, body: Value, cancel: &AtomicBool) -> Result<Value, String> {
    let payload = body.to_string();
    let mut stream =
        TcpStream::connect_timeout(&"127.0.0.1:11434".parse().unwrap(), Duration::from_secs(2))
            .map_err(|_| "Ollama ist nicht erreichbar. Bitte Ollama starten.".to_owned())?;
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .map_err(|e| e.to_string())?;
    let req=format!("POST {path} HTTP/1.1\r\nHost: 127.0.0.1:11434\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{payload}",payload.len());
    stream
        .write_all(req.as_bytes())
        .map_err(|e| e.to_string())?;
    let streaming = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    // The tier's budget, not a fixed three minutes: a FAST request that stalls
    // must fail fast enough that the user still has a working app.
    let hard = Instant::now() + call_timeout();
    let mut last = Instant::now();
    let mut raw = Vec::new();
    let mut buf = [0u8; 16384];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Abgebrochen.".into());
        }
        if Instant::now() >= hard {
            return Err("Intensive synthesis Timeout (hard cap 180s).".into());
        }
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                raw.extend_from_slice(&buf[..n]);
                last = Instant::now();
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if streaming && last.elapsed() >= Duration::from_secs(45) {
                    return Err("Intensive synthesis Timeout (stall watchdog 45s).".into());
                }
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    if streaming {
        parse_stream_http(raw)
    } else {
        parse_http(raw)
    }
}

fn parse_stream_http(raw: Vec<u8>) -> Result<Value, String> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("Ungültige Ollama-Antwort.")?;
    let head = String::from_utf8_lossy(&raw[..split]);
    if !head.starts_with("HTTP/1.1 200") && !head.starts_with("HTTP/1.0 200") {
        return Err(format!(
            "Ollama-Fehler: {}",
            head.lines().next().unwrap_or("HTTP")
        ));
    }
    let mut data = raw[split + 4..].to_vec();
    if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        data = decode_chunked(&data)?;
    }
    let mut response = String::new();
    let mut thinking = String::new();
    let mut last = Value::Null;
    for line in data.split(|b| *b == b'\n') {
        let line = std::str::from_utf8(line).unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let v: Value =
            serde_json::from_str(line).map_err(|e| format!("Ungültiger Ollama-Stream: {e}"))?;
        if let Some(e) = v.get("error").and_then(Value::as_str) {
            return Err(format!("Ollama: {e}"));
        }
        if let Some(s) = v.get("response").and_then(Value::as_str) {
            response.push_str(s);
        }
        if let Some(s) = v.get("thinking").and_then(Value::as_str) {
            thinking.push_str(s);
        }
        last = v;
    }
    if !last.is_object() {
        return Err("Leerer Ollama-Stream.".into());
    }
    last["response"] = Value::String(response);
    last["thinking"] = Value::String(thinking);
    Ok(last)
}

fn parse_http(raw: Vec<u8>) -> Result<Value, String> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("Ungültige Ollama-Antwort.")?;
    let head = String::from_utf8_lossy(&raw[..split]);
    if !head.starts_with("HTTP/1.1 200") && !head.starts_with("HTTP/1.0 200") {
        return Err(format!(
            "Ollama-Fehler: {}",
            head.lines().next().unwrap_or("HTTP")
        ));
    }
    let mut data = raw[split + 4..].to_vec();
    if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        data = decode_chunked(&data)?;
    }
    let v: Value =
        serde_json::from_slice(&data).map_err(|e| format!("Ungültiges Ollama-JSON: {e}"))?;
    if let Some(e) = v.get("error").and_then(Value::as_str) {
        return Err(format!("Ollama: {e}"));
    }
    Ok(v)
}

fn decode_chunked(raw: &[u8]) -> Result<Vec<u8>, String> {
    let mut at = 0;
    let mut out = Vec::new();
    loop {
        let end = raw[at..]
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or("Ungültige HTTP-Chunks.")?
            + at;
        let size = usize::from_str_radix(
            String::from_utf8_lossy(&raw[at..end])
                .split(';')
                .next()
                .unwrap_or("0")
                .trim(),
            16,
        )
        .map_err(|_| "Ungültige HTTP-Chunkgröße.")?;
        if size == 0 {
            break;
        }
        at = end + 2;
        if at + size > raw.len() {
            return Err("Unvollständige Ollama-Antwort.".into());
        }
        out.extend_from_slice(&raw[at..at + size]);
        at += size + 2;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mode_config_is_central_and_distinct() {
        let c = ModelConfig::default();
        assert!(
            !c.chat_model.is_empty() && !c.chat_fast_model.is_empty() && !c.code_model.is_empty()
        );
        assert_ne!(c.chat_model, c.code_model);
    }
    #[test]
    fn chat_profile_does_not_override_dynamic_ranker_tier() {
        let mut m = ModelManager::default();
        m.set_work_tier(WorkTier::Tier9B);
        m.set_chat_profile(ChatProfile::Normal);
        assert_eq!(m.work_tier(), WorkTier::Tier9B);
        m.set_chat_profile(ChatProfile::Intensive);
        assert_eq!(m.work_tier(), WorkTier::Tier9B);
        m.set_chat_profile(ChatProfile::Fast);
        assert_eq!(m.work_tier(), WorkTier::Tier9B);
    }
    #[test]
    fn completion_join_deduplicates_the_boundary() {
        assert_eq!(
            join_completion("Eins zwei drei vier", "drei vier fünf sechs"),
            "Eins zwei drei vier fünf sechs"
        );
    }
    #[test]
    fn length_finish_reason_is_incomplete() {
        let mut m = ModelManager::default();
        m.last_metrics = Some(GenerationMetrics {
            finish_reason: Some("length".into()),
            ..Default::default()
        });
        assert!(m.last_hit_length());
        m.last_metrics.as_mut().unwrap().finish_reason = Some("stop".into());
        assert!(!m.last_hit_length());
    }
    #[test]
    fn bounded_completion_can_keep_finished_sentences() {
        assert_eq!(
            complete_sentence_prefix("Ein Hund ist ein domestiziertes Säugetier. Noch ein"),
            Some("Ein Hund ist ein domestiziertes Säugetier.".into())
        );
        assert_eq!(complete_sentence_prefix("Unvollständiger Satz"), None);
    }
    #[test]
    fn work_tier_switches_model() {
        let mut m = ModelManager::default();
        m.set_work_tier(WorkTier::Tier4B);
        assert_eq!(m.model_for(AssistantMode::Work), m.config.chat_fast_model);
        m.set_work_tier(WorkTier::Tier9B);
        assert_eq!(m.model_for(AssistantMode::Work), m.config.chat_model);
        assert_eq!(m.model_for(AssistantMode::Code), m.config.code_model);
    }
    #[test]
    fn chunked_decoder() {
        assert_eq!(decode_chunked(b"4\r\ntest\r\n0\r\n\r\n").unwrap(), b"test");
    }
    #[test]
    fn streamed_json_is_reassembled() {
        let body = b"{\"response\":\"A\",\"thinking\":\"x\"}\n{\"response\":\"B\",\"done\":true,\"eval_count\":2}\n";
        let mut raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\n\r\n".to_vec();
        raw.extend_from_slice(body);
        let v = parse_stream_http(raw).unwrap();
        assert_eq!(v["response"], "AB");
        assert_eq!(v["thinking"], "x");
    }
    #[test]
    #[ignore = "requires three installed Ollama models"]
    fn real_mode_sequence() {
        let mut m = ModelManager::default();
        let foreign: Vec<String> = m
            .loaded_models()
            .unwrap()
            .into_iter()
            .filter(|x| !m.managed(x))
            .collect();
        assert!(
            foreign.is_empty(),
            "another Ollama session is loaded; leave it untouched"
        );
        let mut rows = Vec::new();
        for (label, profile, mode) in [
            ("fast", ChatProfile::Fast, AssistantMode::Work),
            ("normal", ChatProfile::Normal, AssistantMode::Work),
            ("intensive", ChatProfile::Intensive, AssistantMode::Work),
            ("code", ChatProfile::Intensive, AssistantMode::Code),
            ("normal_again", ChatProfile::Normal, AssistantMode::Work),
        ] {
            m.set_chat_profile(profile);
            let load_ms = m.switch(mode).unwrap();
            let loaded = m.loaded_models().unwrap();
            assert_eq!(loaded.len(), 1, "{label}: {loaded:?}");
            assert!(ModelManager::same_model(&loaded[0], m.model_for(mode)));
            if label == "intensive" {
                assert_eq!(load_ms, 0, "Normal→Intensiv reloaded the same 9B weights");
            }
            rows.push(json!({"mode":label,"model":loaded[0],"cold_load_ms":load_ms,"loaded_models":loaded}));
        }
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.local/qa/model-routing-smoke.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&json!({"loaded_models_max":1,"rows":rows})).unwrap(),
        )
        .unwrap();
    }
}
