//! Project final native wire observations into the existing analytics reducer.
//! No CPA request, restored response identity, prompt, or credential is a source.

use super::BridgeResult;
use axum::http::HeaderMap;
use codex_analytics::AnalyticsEventsClient;
use codex_core::config::Config;
use codex_login::AuthManager;
use codex_protocol::protocol::TokenUsage;
use serde_json::Value;
use snapshot::Snapshot;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

const MAX_THREADS: usize = 256;
const MAX_TURNS: usize = 1024;
const MAX_TOOLS: usize = 1024;
const IDLE_TTL: Duration = Duration::from_secs(3600);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    account: Option<String>,
    user: Option<String>,
    thread: String,
}

struct ThreadState {
    seen_turn: bool,
    created_ms: u64,
    touched: Instant,
    turns: HashMap<String, TurnState>,
    completed: HashMap<String, Instant>,
}

struct TurnState {
    connection_id: u64,
    snapshot: Snapshot,
    started_ms: u64,
    last_ms: u64,
    sampling_ms: u64,
    attempts: u32,
    exchanges: u32,
    responses: HashSet<String>,
    response_order: Vec<String>,
    usage: Option<TokenUsage>,
    tools: HashMap<String, tools::Tool>,
    tools_change_count: u32,
    active: usize,
    generation: u64,
    timer: CancellationToken,
    status: codex_app_server_protocol::TurnStatus,
    error: Option<codex_app_server_protocol::CodexErrorInfo>,
    native_error: Option<codex_analytics::TurnCodexErrorFact>,
    is_first: bool,
}

#[derive(Clone)]
struct Binding {
    key: Key,
    turn: String,
    attempt_started: u64,
    response_id: Option<String>,
    terminal: bool,
}

pub(super) struct InferenceAnalytics {
    client: AnalyticsEventsClient,
    auth: Arc<AuthManager>,
    config: Arc<Config>,
    state: Mutex<HashMap<Key, ThreadState>>,
    // Native reducer turn IDs are global keys. Keep each observed turn's facts
    // together even when two caller-owned threads reuse the same upstream ID.
    recording: tokio::sync::Mutex<()>,
    clock: Instant,
    wall_ms: u64,
    #[cfg(test)]
    delay: Option<Duration>,
}

pub(super) struct Exchange {
    owner: Arc<InferenceAnalytics>,
    connection_id: u64,
    binding: Mutex<Option<Binding>>,
}

impl InferenceAnalytics {
    pub(super) fn new(
        client: AnalyticsEventsClient,
        auth: Arc<AuthManager>,
        config: Arc<Config>,
    ) -> Arc<Self> {
        Arc::new(Self {
            client,
            auth,
            config,
            state: Mutex::new(HashMap::new()),
            recording: tokio::sync::Mutex::new(()),
            clock: Instant::now(),
            wall_ms: codex_analytics::now_unix_millis(),
            #[cfg(test)]
            delay: None,
        })
    }

    pub(super) fn exchange(self: &Arc<Self>, connection_id: u64) -> Exchange {
        Exchange {
            owner: self.clone(),
            connection_id,
            binding: Mutex::new(None),
        }
    }

    fn now(&self) -> u64 {
        self.wall_ms.saturating_add(
            self.clock
                .elapsed()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
        )
    }

    fn delay(&self) -> Duration {
        #[cfg(test)]
        if let Some(delay) = self.delay {
            return delay;
        }
        // Test-only override cannot disable waiting or shorten the production lower bound.
        #[cfg(debug_assertions)]
        if let Some(seconds) = std::env::var("CODEX_CPA_ANALYTICS_TEST_DELAY_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|seconds| (30..=233).contains(seconds))
        {
            return Duration::from_secs(seconds);
        }
        Duration::from_secs(rand::random_range(30..=233))
    }

    fn schedule(
        self: &Arc<Self>,
        key: Key,
        turn_id: String,
        generation: u64,
        timer: CancellationToken,
        deadline: Instant,
    ) {
        let owner = Arc::downgrade(self);
        tokio::spawn(async move {
            tokio::select! {
                _ = timer.cancelled() => return,
                _ = tokio::time::sleep_until(deadline) => {}
            }
            let Some(owner) = owner.upgrade() else {
                return;
            };
            let Some(auth) = owner.auth.auth_cached() else {
                return;
            };
            if auth.get_account_id() != key.account || auth.get_chatgpt_user_id() != key.user {
                return;
            }
            let (created_ms, turn, completed_ms) = {
                let mut state = owner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let Some(thread) = state.get_mut(&key) else {
                    return;
                };
                if !thread
                    .turns
                    .get(&turn_id)
                    .is_some_and(|turn| turn.generation == generation && turn.active == 0)
                {
                    return;
                }
                let Some(turn) = thread.turns.remove(&turn_id) else {
                    return;
                };
                let completed_ms = owner.now();
                let created_ms = thread.created_ms;
                thread.completed.insert(turn_id.clone(), Instant::now());
                thread.touched = Instant::now();
                (created_ms, turn, completed_ms)
            };
            // Enqueue only after the physical delay; no business task awaits this work.
            let _recording = owner.recording.lock().await;
            if let Err(error) = projection::emit(
                &owner.client,
                created_ms,
                &turn_id,
                &turn,
                completed_ms,
                &owner.config,
            )
            .await
            {
                tracing::warn!(%error, "could not record native inference analytics");
            }
            owner.client.flush().await;
        });
    }
}

impl Exchange {
    pub(super) fn request(
        &self,
        transport: &str,
        method: &str,
        url: &str,
        headers: &HeaderMap,
        body: &[u8],
    ) {
        if !self.owner.client.is_enabled() || (method != "POST" && method != "response.create") {
            return;
        }
        let Some(snapshot) = Snapshot::read(transport, url, headers, body) else {
            return;
        };
        let Some(auth) = self.owner.auth.auth_cached() else {
            return;
        };
        let key = Key {
            account: auth.get_account_id(),
            user: auth.get_chatgpt_user_id(),
            thread: snapshot.thread.clone(),
        };
        let now = self.owner.now();
        let mut binding = self
            .binding
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut state = self
            .owner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.retain(|existing, thread| {
            thread
                .completed
                .retain(|_, completed| completed.elapsed() < IDLE_TTL);
            let keep = existing.account == key.account
                && existing.user == key.user
                && (!thread.turns.is_empty() || thread.touched.elapsed() < IDLE_TTL);
            if !keep {
                for turn in thread.turns.values() {
                    turn.timer.cancel();
                }
            }
            keep
        });
        if state
            .get(&key)
            .is_some_and(|thread| thread.completed.contains_key(&snapshot.turn))
        {
            return;
        }
        let mut count: usize = state
            .values()
            .map(|thread| thread.turns.len() + thread.completed.len())
            .sum();
        let existing_turn = state
            .get(&key)
            .is_some_and(|thread| thread.turns.contains_key(&snapshot.turn));
        while !existing_turn && count >= MAX_TURNS {
            let oldest = state
                .iter()
                .flat_map(|(key, thread)| {
                    thread
                        .completed
                        .iter()
                        .map(move |(turn, time)| (key.clone(), turn.clone(), *time))
                })
                .min_by_key(|(_, _, time)| *time);
            let Some((key, turn, _)) = oldest else {
                break;
            };
            state
                .get_mut(&key)
                .expect("existing completed thread")
                .completed
                .remove(&turn);
            count -= 1;
        }
        if !existing_turn
            && (count >= MAX_TURNS || (!state.contains_key(&key) && state.len() >= MAX_THREADS))
        {
            tracing::warn!("inference analytics capacity reached");
            return;
        }
        let thread = state.entry(key.clone()).or_insert_with(|| ThreadState {
            seen_turn: false,
            created_ms: now,
            touched: Instant::now(),
            turns: HashMap::new(),
            completed: HashMap::new(),
        });
        thread.touched = Instant::now();
        let is_first = !thread.seen_turn;
        thread.seen_turn = true;
        let turn = thread
            .turns
            .entry(snapshot.turn.clone())
            .or_insert_with(|| TurnState {
                connection_id: self.connection_id,
                snapshot: snapshot.clone(),
                started_ms: now,
                last_ms: now,
                sampling_ms: 0,
                attempts: 0,
                exchanges: 0,
                responses: HashSet::new(),
                response_order: Vec::new(),
                usage: None,
                tools: HashMap::new(),
                tools_change_count: 0,
                active: 0,
                generation: 0,
                timer: CancellationToken::new(),
                status: codex_app_server_protocol::TurnStatus::InProgress,
                error: None,
                native_error: None,
                is_first,
            });
        // A transport retry reuses its exchange; count time without leaving a phantom active request.
        if let Some(previous) = binding
            .as_ref()
            .filter(|previous| previous.key == key && previous.turn == snapshot.turn)
        {
            turn.sampling_ms = turn
                .sampling_ms
                .saturating_add(now.saturating_sub(previous.attempt_started));
        } else {
            turn.active += 1;
            turn.exchanges = turn.exchanges.saturating_add(1);
        }
        turn.timer.cancel();
        turn.timer = CancellationToken::new();
        turn.generation += 1;
        turn.attempts = turn.attempts.saturating_add(1);
        if turn.snapshot.tools_signature != snapshot.tools_signature {
            turn.tools_change_count = turn.tools_change_count.saturating_add(1);
        }
        for output in &snapshot.results {
            if let Some(id) = output.get("call_id").and_then(Value::as_str)
                && let Some(tool) = turn.tools.get_mut(id)
            {
                tool.result(output, now);
            }
        }
        turn.snapshot.update(&snapshot);
        *binding = Some(Binding {
            key,
            turn: snapshot.turn.clone(),
            attempt_started: now,
            response_id: None,
            terminal: false,
        });
    }

    pub(super) fn event(&self, payload: &str) {
        let Ok(event) = serde_json::from_str::<Value>(payload) else {
            return;
        };
        let now = self.owner.now();
        let mut binding = self
            .binding
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(binding) = binding.as_mut() else {
            return;
        };
        let mut state = self
            .owner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(turn) = state
            .get_mut(&binding.key)
            .and_then(|thread| thread.turns.get_mut(&binding.turn))
        else {
            return;
        };
        if let Some(id) = event.pointer("/response/id").and_then(Value::as_str) {
            binding.response_id = Some(id.into());
        }
        match event["type"]
            .as_str()
            .unwrap_or_default()
            .trim_start_matches("codex.")
        {
            "response.output_item.added" | "response.output_item.done" => {
                record_tool(turn, &event["item"], now, binding.response_id.as_deref());
            }
            "response.completed"
            | "response.incomplete"
            | "response.cancelled"
            | "response.failed" => {
                if binding.terminal {
                    return;
                }
                binding.terminal = true;
                turn.last_ms = now;
                for item in event
                    .pointer("/response/output")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    record_tool(turn, item, now, binding.response_id.as_deref());
                }
                let response_id = binding.response_id.clone();
                let fresh = response_id.is_none_or(|id| {
                    if turn.responses.insert(id.clone()) {
                        turn.response_order.push(id);
                        true
                    } else {
                        false
                    }
                });
                if fresh && let Some(usage) = snapshot::usage(&event["response"]["usage"]) {
                    snapshot::add_usage(turn.usage.get_or_insert_default(), &usage);
                }
                turn.status = match event["type"]
                    .as_str()
                    .unwrap_or_default()
                    .trim_start_matches("codex.")
                {
                    "response.cancelled" => codex_app_server_protocol::TurnStatus::Interrupted,
                    "response.incomplete" | "response.failed" => {
                        codex_app_server_protocol::TurnStatus::Failed
                    }
                    _ => codex_app_server_protocol::TurnStatus::Completed,
                };
                if turn.status == codex_app_server_protocol::TurnStatus::Failed {
                    turn.error = Some(codex_app_server_protocol::CodexErrorInfo::Other);
                }
            }
            _ => {}
        }
    }

    pub(super) fn native_error(&self, error: &codex_protocol::error::CodexErr) {
        let binding = self
            .binding
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(binding) = binding.as_ref() else {
            return;
        };
        let mut state = self
            .owner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(turn) = state
            .get_mut(&binding.key)
            .and_then(|thread| thread.turns.get_mut(&binding.turn))
        {
            turn.native_error = Some(codex_analytics::TurnCodexErrorFact::from_codex_err(
                binding.key.thread.clone(),
                binding.turn.clone(),
                error,
            ));
        }
    }

    pub(super) fn finish(&self, result: &BridgeResult<()>) {
        let Some(binding) = self
            .binding
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        else {
            return;
        };
        let now = self.owner.now();
        let scheduled = {
            let mut state = self
                .owner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(turn) = state
                .get_mut(&binding.key)
                .and_then(|thread| thread.turns.get_mut(&binding.turn))
            else {
                return;
            };
            turn.active = turn.active.saturating_sub(1);
            turn.sampling_ms = turn
                .sampling_ms
                .saturating_add(now.saturating_sub(binding.attempt_started));
            if let Err(error) = result {
                let status = error
                    .data
                    .as_ref()
                    .and_then(|data| data["httpStatus"].as_u64());
                turn.status = if status == Some(499) {
                    codex_app_server_protocol::TurnStatus::Interrupted
                } else {
                    codex_app_server_protocol::TurnStatus::Failed
                };
                turn.error = error
                    .data
                    .as_ref()
                    .and_then(|data| {
                        serde_json::from_value::<codex_protocol::protocol::CodexErrorInfo>(
                            data["nativeError"].clone(),
                        )
                        .ok()
                    })
                    .map(Into::into)
                    .or_else(|| Some(snapshot::error_info(status)));
                turn.last_ms = now;
            } else if !binding.terminal {
                turn.status = codex_app_server_protocol::TurnStatus::Failed;
                turn.error = Some(codex_app_server_protocol::CodexErrorInfo::Other);
                turn.last_ms = now;
            }
            if turn.active != 0 {
                return;
            }
            turn.timer.cancel();
            turn.timer = CancellationToken::new();
            turn.generation += 1;
            let deadline = self.owner.clock
                + Duration::from_millis(turn.last_ms.saturating_sub(self.owner.wall_ms))
                + self.owner.delay();
            (turn.generation, turn.timer.clone(), deadline)
        };
        self.owner.schedule(
            binding.key,
            binding.turn,
            scheduled.0,
            scheduled.1,
            scheduled.2,
        );
    }
}

fn record_tool(turn: &mut TurnState, item: &Value, now: u64, response_id: Option<&str>) {
    let Some(mut tool) = tools::Tool::read(item, now) else {
        return;
    };
    tool.response_id = response_id.map(str::to_owned);
    tool.model_context = Some(codex_protocol::items::ModelInvocationContext {
        model_slug: turn.snapshot.model.clone(),
        reasoning_effort: turn.snapshot.reasoning["effort"]
            .as_str()
            .map(str::to_owned),
    });
    if turn.tools.len() >= MAX_TOOLS && !turn.tools.contains_key(&tool.id) {
        return;
    }
    turn.tools
        .entry(tool.id.clone())
        .and_modify(|existing| existing.update(&tool))
        .or_insert(tool);
}

#[cfg(test)]
#[path = "analytics_tests.rs"]
mod tests;

mod snapshot {
    use axum::http::HeaderMap;
    use codex_app_server_protocol::CodexErrorInfo;
    use codex_protocol::protocol::TokenUsage;
    use serde_json::Value;
    use serde_json::json;
    use sha2::Digest;
    use std::io::Read;

    const MAX_BODY: usize = 32 * 1024 * 1024;

    #[derive(Clone)]
    pub(super) struct Snapshot {
        pub(super) thread: String,
        pub(super) session: String,
        pub(super) turn: String,
        pub(super) model: String,
        pub(super) originator: String,
        pub(super) metadata: Value,
        pub(super) reasoning: Value,
        pub(super) service_tier: Option<String>,
        pub(super) images: usize,
        pub(super) tools_signature: Vec<u8>,
        pub(super) results: Vec<Value>,
    }

    impl Snapshot {
        pub(super) fn read(
            _transport: &str,
            url: &str,
            headers: &HeaderMap,
            bytes: &[u8],
        ) -> Option<Self> {
            let url = url::Url::parse(url).ok()?;
            if !["/responses", "/responses/lite"]
                .iter()
                .any(|suffix| url.path().ends_with(suffix))
            {
                return None;
            }
            let decoded;
            let bytes = if headers
                .get("content-encoding")
                .is_some_and(|value| value == "zstd")
            {
                let mut data = Vec::new();
                zstd::stream::read::Decoder::new(bytes)
                    .ok()?
                    .take((MAX_BODY + 1) as u64)
                    .read_to_end(&mut data)
                    .ok()?;
                decoded = data;
                decoded.as_slice()
            } else {
                bytes
            };
            if bytes.len() > MAX_BODY {
                return None;
            }
            let body: Value = serde_json::from_slice(bytes).ok()?;
            let body = if body.get("client_metadata").is_none() && body["response"].is_object() {
                &body["response"]
            } else {
                &body
            };
            let outer = &body["client_metadata"];
            let mut metadata = outer
                .get("x-codex-turn-metadata")
                .and_then(Value::as_str)
                .or_else(|| {
                    headers
                        .get("x-codex-turn-metadata")
                        .and_then(|value| value.to_str().ok())
                })
                .and_then(|value| serde_json::from_str::<Value>(value).ok())
                .filter(Value::is_object)
                .unwrap_or_else(|| json!({}));
            for key in [
                "session_id",
                "thread_id",
                "turn_id",
                "root_turn_id",
                "parent_thread_id",
                "parent_turn_id",
                "thread_source",
                "subagent_kind",
                "turn_trigger",
                "codex_turn_source",
                "workspace_kind",
                "request_kind",
                "sandbox_mode",
                "auto_review_enabled",
            ] {
                if let Some(value) = outer.get(key) {
                    metadata[key] = value.clone();
                }
            }
            let string = |key: &str| {
                metadata
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty() && value.len() <= 4096)
                    .map(str::to_owned)
            };
            let thread = string("thread_id")?;
            let session = string("session_id")?;
            let turn = string("turn_id")?;
            let model = body.get("model")?.as_str()?.to_owned();
            let images = body["input"]
                .as_array()
                .into_iter()
                .flatten()
                .rev()
                .find(|item| item["role"] == "user")
                .and_then(|item| item["content"].as_array())
                .map_or(0, |content| {
                    content
                        .iter()
                        .filter(|item| item["type"] == "input_image")
                        .count()
                });
            let results = body["input"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|item| {
                    matches!(
                        item["type"].as_str(),
                        Some("function_call_output" | "custom_tool_call_output")
                    )
                })
                .filter_map(|item| {
                    let id = item["call_id"].as_str()?;
                    let output = item.get("output").cloned().unwrap_or(Value::Null);
                    let parsed = output
                        .as_str()
                        .and_then(|text| serde_json::from_str::<Value>(text).ok())
                        .unwrap_or(output);
                    let mut result = json!({"call_id": id});
                    for key in [
                        "exit_code",
                        "exitCode",
                        "is_error",
                        "success",
                        "duration_ms",
                        "status",
                    ] {
                        if let Some(value) = parsed.get(key).or_else(|| parsed["metadata"].get(key))
                        {
                            result[key] = value.clone();
                        }
                    }
                    if let Some(text) = parsed.as_str() {
                        read_exec_header(text, &mut result);
                    } else if let Some(parts) = parsed.as_array() {
                        for text in parts.iter().filter_map(|part| part["text"].as_str()) {
                            read_exec_header(text, &mut result);
                        }
                    }
                    Some(result)
                })
                .collect();
            Some(Self {
                thread,
                session,
                turn,
                model,
                originator: headers
                    .get("originator")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned)
                    .unwrap_or_else(|| codex_login::default_client::originator().value),
                reasoning: body["reasoning"].clone(),
                service_tier: body["service_tier"].as_str().map(str::to_owned),
                tools_signature: sha2::Sha256::digest(serde_json::to_vec(&body["tools"]).ok()?)
                    .to_vec(),
                metadata,
                images,
                results,
            })
        }

        pub(super) fn update(&mut self, next: &Self) {
            self.model.clone_from(&next.model);
            self.reasoning.clone_from(&next.reasoning);
            self.service_tier.clone_from(&next.service_tier);
            self.tools_signature.clone_from(&next.tools_signature);
            self.metadata.clone_from(&next.metadata);
            self.results.clear();
        }
    }

    // Parse only the native command metadata header, never arbitrary stdout after Output:.
    fn read_exec_header(text: &str, result: &mut Value) {
        if !["Exit code:", "Chunk ID:", "Wall time:"]
            .iter()
            .any(|prefix| text.starts_with(prefix))
        {
            return;
        }
        for line in text.lines().take_while(|line| *line != "Output:").take(16) {
            if let Some(code) = line
                .strip_prefix("Exit code: ")
                .or_else(|| line.strip_prefix("Process exited with code "))
                .and_then(|code| code.parse::<i32>().ok())
            {
                result["exit_code"] = json!(code);
            }
            if let Some(seconds) = line
                .strip_prefix("Wall time: ")
                .and_then(|text| text.strip_suffix(" seconds"))
                .and_then(|text| text.parse::<f64>().ok())
                .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
            {
                result["duration_ms"] = json!((seconds * 1000.0).round() as u64);
            }
        }
    }

    pub(super) fn usage(value: &Value) -> Option<TokenUsage> {
        if !value.is_object() {
            return None;
        }
        let number = |path: &str| {
            value
                .pointer(path)
                .and_then(Value::as_i64)
                .unwrap_or_default()
                .max(0)
        };
        let input = number("/input_tokens");
        let output = number("/output_tokens");
        Some(TokenUsage {
            input_tokens: input,
            output_tokens: output,
            cached_input_tokens: number("/input_tokens_details/cached_tokens"),
            cache_write_input_tokens: number("/input_tokens_details/cache_write_tokens"),
            reasoning_output_tokens: number("/output_tokens_details/reasoning_tokens"),
            total_tokens: value["total_tokens"]
                .as_i64()
                .unwrap_or_else(|| input.saturating_add(output))
                .max(0),
            codex_rollout_budget_units: None,
        })
    }

    pub(super) fn add_usage(total: &mut TokenUsage, usage: &TokenUsage) {
        total.input_tokens = total.input_tokens.saturating_add(usage.input_tokens);
        total.output_tokens = total.output_tokens.saturating_add(usage.output_tokens);
        total.cached_input_tokens = total
            .cached_input_tokens
            .saturating_add(usage.cached_input_tokens);
        total.cache_write_input_tokens = total
            .cache_write_input_tokens
            .saturating_add(usage.cache_write_input_tokens);
        total.reasoning_output_tokens = total
            .reasoning_output_tokens
            .saturating_add(usage.reasoning_output_tokens);
        total.total_tokens = total.total_tokens.saturating_add(usage.total_tokens);
    }

    pub(super) fn error_info(status: Option<u64>) -> CodexErrorInfo {
        if status == Some(429) {
            CodexErrorInfo::UsageLimitExceeded
        } else {
            CodexErrorInfo::Other
        }
    }
}

mod tools {
    use codex_app_server_protocol::ThreadItem;
    use codex_app_server_protocol::ThreadItemPayload;
    use serde_json::Value;
    use serde_json::json;

    pub(super) struct Tool {
        pub(super) id: String,
        pub(super) started: u64,
        pub(super) completed: Option<u64>,
        pub(super) response_id: Option<String>,
        pub(super) model_context: Option<codex_protocol::items::ModelInvocationContext>,
        name: String,
        arguments: Value,
        item: Value,
        exit_code: Option<i32>,
        failed: bool,
        duration_ms: Option<u64>,
    }

    impl Tool {
        pub(super) fn read(item: &Value, now: u64) -> Option<Self> {
            let kind = item["type"].as_str()?;
            let external = matches!(kind, "function_call" | "custom_tool_call");
            if !external
                && !matches!(
                    kind,
                    "web_search_call" | "image_generation_call" | "mcp_call"
                )
            {
                return None;
            }
            let id = item["call_id"]
                .as_str()
                .or_else(|| item["id"].as_str())?
                .to_owned();
            let arguments = item["arguments"]
                .as_str()
                .or_else(|| item["input"].as_str())
                .and_then(|value| serde_json::from_str(value).ok())
                .unwrap_or_else(|| item["arguments"].clone());
            Some(Self {
                id,
                started: now,
                completed: (!external && item["status"] == "completed").then_some(now),
                response_id: None,
                model_context: None,
                name: item["name"].as_str().unwrap_or(kind).to_owned(),
                arguments,
                item: item.clone(),
                exit_code: None,
                failed: item["status"] == "failed" || !item["error"].is_null(),
                duration_ms: None,
            })
        }

        pub(super) fn update(&mut self, next: &Self) {
            if !next.arguments.is_null() {
                self.arguments.clone_from(&next.arguments);
            }
            self.item.clone_from(&next.item);
            if self.response_id.is_none() {
                self.response_id.clone_from(&next.response_id);
            }
            self.failed |= next.failed;
            if self.completed.is_none() {
                self.completed = next.completed;
            }
        }

        pub(super) fn result(&mut self, output: &Value, now: u64) {
            self.completed.get_or_insert(now);
            if let Some(code) = output["exit_code"]
                .as_i64()
                .or_else(|| output["exitCode"].as_i64())
                .and_then(|value| value.try_into().ok())
            {
                if self.exit_code.is_none_or(|previous| previous == 0) {
                    self.exit_code = Some(code);
                }
            }
            self.failed |= self.exit_code.is_some_and(|code| code != 0)
                || output["is_error"] == true
                || output["success"] == false
                || output["status"] == "failed";
            self.duration_ms = output["duration_ms"].as_u64();
        }

        pub(super) fn project(
            &self,
            completed_ms: u64,
            cwd: &str,
            thread_id: &str,
        ) -> Result<ThreadItem, serde_json::Error> {
            let status = if self.failed { "failed" } else { "completed" };
            let duration = self
                .duration_ms
                .unwrap_or_else(|| completed_ms.saturating_sub(self.started));
            let name = self.name.strip_prefix("functions.").unwrap_or(&self.name);
            let item = match name {
                "shell" | "shell_command" | "exec_command" | "write_stdin" => {
                    let command = self.arguments["cmd"]
                        .as_str()
                        .or_else(|| self.arguments["command"].as_str())
                        .unwrap_or_default();
                    json!({"type":"commandExecution","id":self.id,"command":command,"cwd":cwd,
                    "source":if name == "write_stdin" {"unifiedExecInteraction"} else if name == "exec_command" {"unifiedExecStartup"} else {"agent"},
                    "status":status,"commandActions":[{"type":"unknown","command":command}],
                    "exitCode":self.exit_code.unwrap_or(0),"durationMs":duration})
                }
                "apply_patch" => {
                    let patch = self.item["input"]
                        .as_str()
                        .or_else(|| self.arguments["patch"].as_str())
                        .or_else(|| self.arguments["input"].as_str())
                        .unwrap_or_default();
                    let changes: Vec<Value> = codex_apply_patch::parse_patch(patch)
                        .ok()
                        .into_iter()
                        .flat_map(|patch| patch.hunks)
                        .map(|hunk| {
                            use codex_apply_patch::Hunk;
                            let (path, kind) = match hunk {
                                Hunk::AddFile { path, .. } => (path, json!({"type":"add"})),
                                Hunk::DeleteFile { path } => (path, json!({"type":"delete"})),
                                Hunk::UpdateFile {
                                    path, move_path, ..
                                } => (path, json!({"type":"update","movePath":move_path})),
                            };
                            // The intended patch identifies affected paths; an applied workspace
                            // diff is not available in inference-only mode.
                            json!({"path":path,"kind":kind,"diff":""})
                        })
                        .collect();
                    json!({"type":"fileChange","id":self.id,"status":status,"changes":changes})
                }
                "spawn_agent" | "send_input" | "resume_agent" | "wait" | "close_agent"
                | "send_message" | "followup_task" | "interrupt_agent" | "list_agents" => {
                    let tool = match name {
                        "spawn_agent" => "spawnAgent",
                        "send_input" => "sendInput",
                        "resume_agent" => "resumeAgent",
                        "close_agent" => "closeAgent",
                        "send_message" => "sendMessage",
                        "followup_task" => "followupTask",
                        "interrupt_agent" => "interruptAgent",
                        "list_agents" => "listAgents",
                        _ => "wait",
                    };
                    let receivers: Vec<&str> = self.arguments["ids"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .chain(self.arguments["id"].as_str())
                        .collect();
                    json!({"type":"collabAgentToolCall","id":self.id,"tool":tool,"status":status,
                    "senderThreadId":thread_id,"receiverThreadIds":receivers,"prompt":null,"model":self.arguments["model"],
                    "reasoningEffort":self.arguments["reasoning_effort"],"agentsStates":{}})
                }
                "web_search_call" => {
                    let search = codex_protocol::items::WebSearchItem {
                        id: self.id.clone(),
                        query: self.item["action"]["query"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned(),
                        action: serde_json::from_value(self.item["action"].clone())
                            .unwrap_or(codex_protocol::models::WebSearchAction::Other),
                        results: None,
                    };
                    return Ok(ThreadItem::untimed(
                        codex_protocol::items::TurnItem::WebSearch(search).into(),
                    ));
                }
                "image_generation_call" => {
                    json!({"type":"imageGeneration","id":self.id,"status":status,"result":"",
                "revisedPrompt":self.item["revised_prompt"],"savedPath":null})
                }
                _ if name.starts_with("mcp__") || self.item["type"] == "mcp_call" => {
                    let (server, tool) = name
                        .strip_prefix("mcp__")
                        .and_then(|name| name.split_once("__"))
                        .unwrap_or((
                            self.item["server_label"].as_str().unwrap_or_default(),
                            self.item["name"].as_str().unwrap_or(name),
                        ));
                    json!({"type":"mcpToolCall","id":self.id,"server":server,"tool":tool,"status":status,
                    "arguments":{},"result":if self.failed {Value::Null} else {json!({"content":[],"structuredContent":null})},
                    "error":if self.failed {json!({"message":"tool execution failed"})} else {Value::Null},"durationMs":duration})
                }
                _ => {
                    json!({"type":"dynamicToolCall","id":self.id,"tool":name,"arguments":{},"status":status,
                "success":!self.failed,"contentItems":[],"durationMs":duration})
                }
            };
            let mut item: ThreadItem = serde_json::from_value(item)?;
            if let ThreadItemPayload::CommandExecution { model_context, .. } = &mut item.payload {
                model_context.clone_from(&self.model_context);
            }
            if let ThreadItemPayload::ImageGeneration(image) = &mut item.payload {
                image.imagegen_request_id =
                    self.item["imagegen_request_id"].as_str().map(str::to_owned);
                image.generation_id = self.item["generation_id"].as_str().map(str::to_owned);
                image.transparent_background = self.item["transparent_background"].as_bool();
            }
            Ok(item)
        }
    }
}

mod projection {
    use super::TurnState;
    use codex_analytics::CodeModeToolCallFact;
    use codex_analytics::TurnAnalyticsMetadata;
    use codex_analytics::TurnProfile;
    use codex_analytics::TurnProfileFact;
    use codex_analytics::TurnResolvedConfigFact;
    use codex_analytics::TurnTokenUsageFact;
    use codex_app_server_protocol::*;
    use codex_core::config::Config;
    use codex_protocol::config_types::ModeKind;
    use codex_protocol::config_types::ServiceTier;
    use serde_json::Value;
    use std::sync::Arc;

    struct Metadata(Value);
    impl TurnAnalyticsMetadata for Metadata {
        fn root_turn_id(&self) -> Option<String> {
            self.0["root_turn_id"].as_str().map(str::to_owned)
        }
        fn turn_trigger(&self) -> Option<String> {
            self.0["turn_trigger"].as_str().map(str::to_owned)
        }
        fn codex_turn_source(&self) -> Option<String> {
            self.0["codex_turn_source"].as_str().map(str::to_owned)
        }
    }

    pub(super) async fn emit(
        client: &codex_analytics::AnalyticsEventsClient,
        created_ms: u64,
        turn_id: &str,
        state: &TurnState,
        completed_ms: u64,
        config: &Config,
    ) -> anyhow::Result<()> {
        let snapshot = &state.snapshot;
        let model = &snapshot.model;
        let metadata = &snapshot.metadata;
        // The DTO requires a cwd, but no remote workspace has been observed. A root
        // placeholder prevents scanning the worker checkout as the caller's Git repo.
        let cwd = config
            .cwd
            .as_path()
            .ancestors()
            .last()
            .unwrap_or(config.cwd.as_path());
        let duration = completed_ms.saturating_sub(state.started_ms);
        let turn = Turn {
            id: turn_id.to_owned(),
            root_turn_id: metadata["root_turn_id"].as_str().map(str::to_owned),
            items: Vec::new(),
            items_view: TurnItemsView::NotLoaded,
            status: state.status.clone(),
            error: state.error.clone().map(|info| TurnError {
                message: "upstream execution failed".into(),
                codex_error_info: Some(info),
                additional_details: None,
                misalignment: None,
            }),
            started_at: Some(as_i64(state.started_ms / 1000)),
            completed_at: Some(as_i64(completed_ms / 1000)),
            duration_ms: Some(as_i64(duration)),
        };
        // Validate projections before enqueuing, and drain between bounded chunks.
        let tools = state
            .tools
            .values()
            .map(|tool| {
                tool.project(completed_ms, &cwd.to_string_lossy(), &snapshot.thread)
                    .map(|item| (tool, item))
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;
        client.flush().await;
        client.track_observed_inference(codex_analytics::ObservedInferenceContext {
            connection_id: state.connection_id,
            thread_id: snapshot.thread.clone(),
            session_id: snapshot.session.clone(),
            turn_id: turn_id.to_owned(),
            model: model.clone(),
            originator: snapshot.originator.clone(),
            parent_thread_id: metadata["parent_thread_id"].as_str().map(str::to_owned),
            thread_source: metadata["thread_source"]
                .as_str()
                .and_then(|value| value.parse().ok()),
            subagent_source: metadata["subagent_kind"].as_str().map(str::to_owned),
            created_at: created_ms / 1000,
            started_at: state.started_ms / 1000,
            num_input_images: snapshot.images,
        });
        let wire_policy = match metadata["sandbox_mode"].as_str() {
            Some("danger-full-access") => {
                Some(codex_protocol::protocol::SandboxPolicy::DangerFullAccess)
            }
            Some("read-only") => {
                Some(codex_protocol::protocol::SandboxPolicy::new_read_only_policy())
            }
            Some("workspace-write") => {
                Some(codex_protocol::protocol::SandboxPolicy::new_workspace_write_policy())
            }
            _ => None,
        };
        let permission_profile = wire_policy
            .as_ref()
            .map(codex_protocol::models::PermissionProfile::from_legacy_sandbox_policy)
            .unwrap_or_else(|| config.permissions.effective_permission_profile());
        client.track_turn_resolved_config(TurnResolvedConfigFact {
            turn_id: turn_id.to_owned(),
            thread_id: snapshot.thread.clone(),
            turn_metadata: Arc::new(Metadata(metadata.clone())),
            active_plugin_ids_at_turn_start: None,
            num_input_images: snapshot.images,
            submission_type: None,
            ephemeral: true,
            session_source: if snapshot.originator == codex_login::cpa_desktop::ORIGINATOR {
                metadata["subagent_kind"].as_str().map_or(
                    codex_protocol::protocol::SessionSource::VSCode,
                    |kind| {
                        codex_protocol::protocol::SessionSource::SubAgent(
                            codex_protocol::protocol::SubAgentSource::Other(kind.into()),
                        )
                    },
                )
            } else {
                codex_protocol::protocol::SessionSource::Cli
            },
            model: model.clone(),
            model_provider: "openai".into(),
            permission_profile: permission_profile.clone(),
            permission_profile_cwd: cwd.to_path_buf(),
            reasoning_effort: serde_json::from_value(snapshot.reasoning["effort"].clone()).ok(),
            reasoning_summary: serde_json::from_value(snapshot.reasoning["summary"].clone()).ok(),
            service_tier: snapshot
                .service_tier
                .as_deref()
                .and_then(ServiceTier::from_request_value),
            approval_policy: codex_protocol::protocol::AskForApproval::Never,
            approvals_reviewer: codex_protocol::config_types::ApprovalsReviewer::User,
            guardian_v2_enabled: metadata["auto_review_enabled"].as_bool().unwrap_or(false),
            multi_agent_version: codex_protocol::protocol::MultiAgentVersion::V1,
            sandbox_network_access: permission_profile.network_sandbox_policy().is_enabled(),
            collaboration_mode: ModeKind::Default,
            personality: None,
            workspace_kind: metadata["workspace_kind"].as_str().map(str::to_owned),
            is_first_turn: state.is_first,
        });
        let mut by_response = std::collections::BTreeMap::<String, Vec<String>>::new();
        for (tool, _) in &tools {
            if let Some(response) = &tool.response_id {
                by_response
                    .entry(response.clone())
                    .or_default()
                    .push(tool.id.clone());
            }
        }
        for (index, response_id) in state.response_order.iter().enumerate() {
            if index > 0 && index % 32 == 0 {
                client.flush().await;
            }
            client.track_code_mode_tool_call(CodeModeToolCallFact::SamplingResponseCompleted {
                thread_id: snapshot.thread.clone(),
                turn_id: turn_id.to_owned(),
                response_id: response_id.clone(),
                tool_call_ids: by_response.remove(response_id).unwrap_or_default(),
            });
        }
        client.flush().await;
        for (index, (tool, item)) in tools.into_iter().enumerate() {
            if index > 0 && index % 32 == 0 {
                client.flush().await;
            }
            client.track_notification(&ServerNotification::ItemStarted(ItemStartedNotification {
                thread_id: snapshot.thread.clone(),
                turn_id: turn_id.to_owned(),
                item: item.clone(),
                started_at_ms: as_i64(tool.started),
            }));
            client.track_tool_config_allowed(
                snapshot.thread.clone(),
                turn_id.to_owned(),
                tool.id.clone(),
            );
            client.track_notification(&ServerNotification::ItemCompleted(
                ItemCompletedNotification {
                    thread_id: snapshot.thread.clone(),
                    turn_id: turn_id.to_owned(),
                    item,
                    completed_at_ms: as_i64(completed_ms),
                },
            ));
        }
        if let Some(error) = &state.native_error {
            client.track_turn_codex_error(error.clone());
        }
        if let Some(usage) = &state.usage {
            client.track_turn_token_usage(TurnTokenUsageFact {
                turn_id: turn_id.to_owned(),
                thread_id: snapshot.thread.clone(),
                token_usage: usage.clone(),
            });
        }
        let pre_completion = state.last_ms.saturating_sub(state.started_ms);
        let sampling = state.sampling_ms.min(pre_completion);
        client.track_turn_profile(TurnProfileFact {
            turn_id: turn_id.to_owned(),
            profile: TurnProfile {
                // Inference-only projection does not run the turn critical-path recorder.
                critical_path: Default::default(),
                before_first_sampling_ms: 0,
                sampling_ms: sampling,
                compaction_ms: 0,
                between_sampling_overhead_ms: pre_completion.saturating_sub(sampling),
                tool_blocking_ms: 0,
                after_last_sampling_ms: completed_ms.saturating_sub(state.last_ms),
                sampling_request_count: state.attempts,
                sampling_retry_count: state.attempts.saturating_sub(state.exchanges),
                tools_change_count: state.tools_change_count,
            },
        });
        client.track_notification(&ServerNotification::TurnCompleted(
            TurnCompletedNotification {
                thread_id: snapshot.thread.clone(),
                turn,
            },
        ));
        Ok(())
    }

    fn as_i64(value: u64) -> i64 {
        value.try_into().unwrap_or(i64::MAX)
    }
}
