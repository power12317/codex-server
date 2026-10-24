//! Compatible business input for the mainline native request builder and HTTP bridge.
use super::*;
use anyhow::Context;
use codex_api::ResponsesGenerationOptions;
use codex_protocol::models::BaseInstructions;
use serde::de::DeserializeOwned;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;

#[path = "inference_input.rs"]
mod input;

#[derive(Debug, Clone, Default)]
pub(super) struct NativeRequestControls {
    pub tool_choice: Option<Value>,
    pub include: Option<Vec<String>>,
    pub format: Option<codex_api::ResponsesTextFormat>,
    pub context: Option<ReasoningContext>,
    pub verbosity: Option<VerbosityConfig>,
    pub parallel_tool_calls: Option<bool>,
    pub stream_options: Option<StreamOptions>,
    pub store: Option<bool>,
    pub generation: ResponsesGenerationOptions,
    pub client_metadata: HashMap<String, String>,
}

pub(super) struct InferenceRequest {
    pub(super) prompt: Prompt,
    pub(super) effort: Option<ReasoningEffortConfig>,
    pub(super) summary: ReasoningSummaryConfig,
    pub(super) service_tier: Option<String>,
    pub(super) controls: NativeRequestControls,
}

impl InferenceRequest {
    pub(super) fn parse(
        config: &crate::config::Config,
        request: &Map<String, Value>,
    ) -> anyhow::Result<Self> {
        validate_state(request)?;
        let mut input = request.get("input").cloned().unwrap_or_else(|| json!([]));
        if input.is_string() {
            input = json!([{"type":"message","role":"user","content":input}]);
        }
        let items = input
            .as_array_mut()
            .context("input must be text or an array")?;
        let input = input::parse(items)?;
        let tools = super::inference_tools::InferenceTools::parse(request)?;
        let native = tools.native;
        let reasoning = object(request, "reasoning")?;
        let mut text = object(request, "text")?.cloned().unwrap_or_default();
        if let Some(format) = text.get_mut("format").and_then(Value::as_object_mut) {
            // Responses schema format has a defined default for omitted type/name/strict.
            for key in ["type", "name", "strict"] {
                if format.get(key).is_some_and(Value::is_null) {
                    format.remove(key);
                }
            }
            if !format.contains_key("type") && format.contains_key("schema") {
                format.insert("type".into(), json!("json_schema"));
            }
        }
        let codex_api::TextControls { format, verbosity } =
            serde_json::from_value(Value::Object(text))?;
        let verbosity = verbosity.map(|value| match value {
            codex_api::OpenAiVerbosity::Low => VerbosityConfig::Low,
            codex_api::OpenAiVerbosity::Medium => VerbosityConfig::Medium,
            codex_api::OpenAiVerbosity::High => VerbosityConfig::High,
        });
        let stream_options = object(request, "stream_options")?
            .cloned()
            .map(|mut options| {
                // include_usage belongs to downstream Chat SSE delivery, not native sampling.
                options.remove("include_usage");
                // A null optional delivery preference means no StreamOptions,
                // as in the existing native entry's Option handling.
                if options
                    .get("reasoning_summary_delivery")
                    .is_some_and(Value::is_null)
                {
                    options.remove("reasoning_summary_delivery");
                }
                options
            })
            .filter(|options| !options.is_empty())
            .map(|options| serde_json::from_value(Value::Object(options)))
            .transpose()?;
        let codex_api::Reasoning {
            effort,
            summary,
            context,
        } = serde_json::from_value(Value::Object(reasoning.cloned().unwrap_or_default()))?;
        let effort = effort.or_else(|| config.model_reasoning_effort.clone());
        let summary = summary
            .or(config.model_reasoning_summary)
            .unwrap_or_default();
        let mut generation: ResponsesGenerationOptions = generation_options(request)?;
        for alias in ["max_completion_tokens", "max_tokens"] {
            if let Some(value) = optional::<u64>(Some(request), alias)? {
                if let Some(existing) = generation.max_output_tokens {
                    anyhow::ensure!(
                        existing == value,
                        "conflicting max_output_tokens and {alias}"
                    );
                } else {
                    generation.max_output_tokens = Some(value);
                }
            }
        }
        let controls = NativeRequestControls {
            tool_choice: tools.choice,
            include: optional(Some(request), "include")?,
            format,
            context,
            verbosity,
            parallel_tool_calls: optional(Some(request), "parallel_tool_calls")?,
            stream_options,
            store: optional(Some(request), "store")?,
            generation,
            client_metadata: caller_metadata(request)?,
        };
        let service_tier =
            optional(Some(request), "service_tier")?.or_else(|| config.service_tier.clone());
        let prompt = Prompt {
            input,
            tools: native.into(),
            parallel_tool_calls: controls.parallel_tool_calls.unwrap_or(true),
            base_instructions: BaseInstructions {
                text: optional(Some(request), "instructions")?.unwrap_or_default(),
                ..Default::default()
            },
            ..Default::default()
        };
        Ok(Self {
            prompt,
            effort,
            summary,
            service_tier,
            controls,
        })
    }
}

fn object<'a>(
    request: &'a Map<String, Value>,
    key: &str,
) -> anyhow::Result<Option<&'a Map<String, Value>>> {
    request
        .get(key)
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_object()
                .with_context(|| format!("{key} must be an object"))
        })
        .transpose()
}

fn optional<T: DeserializeOwned>(
    object: Option<&Map<String, Value>>,
    key: &str,
) -> anyhow::Result<Option<T>> {
    object
        .and_then(|o| o.get(key))
        .map(|v| serde_json::from_value(v.clone()).with_context(|| format!("invalid {key}")))
        .transpose()
        .map(Option::flatten)
}

fn generation_options(request: &Map<String, Value>) -> anyhow::Result<ResponsesGenerationOptions> {
    // Native request envelope fields are consumed elsewhere in InferenceRequest.
    // serde captures remaining business fields in the native generation DTO;
    // only worker-owned transport/provenance is excluded from its extensions.
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct Envelope {
        model: Option<Value>,
        input: Option<Value>,
        instructions: Option<Value>,
        tools: Option<Value>,
        tool_choice: Option<Value>,
        reasoning: Option<Value>,
        parallel_tool_calls: Option<Value>,
        text: Option<Value>,
        service_tier: Option<Value>,
        stream: Option<Value>,
        store: Option<Value>,
        background: Option<Value>,
        include: Option<Value>,
        client_metadata: Option<Value>,
        stream_options: Option<Value>,
        previous_response_id: Option<Value>,
        conversation: Option<Value>,
        generate: Option<Value>,
        max_tokens: Option<Value>,
        max_completion_tokens: Option<Value>,
        #[serde(flatten)]
        generation: ResponsesGenerationOptions,
    }
    let mut generation =
        serde_json::from_value::<Envelope>(Value::Object(request.clone()))?.generation;
    generation
        .extensions
        .retain(|key, _| !worker_owned_field(key));
    Ok(generation)
}

fn worker_owned_field(key: &str) -> bool {
    key.starts_with("x-")
        || key.starts_with("ws_request_")
        || matches!(
            key,
            "headers"
                | "authorization"
                | "Authorization"
                | "cookies"
                | "account_id"
                | "installation_id"
                | "user_agent"
                | "originator"
                | "device_id"
                | "access_programs"
                | "session_id"
                | "thread_id"
                | "turn_id"
                | "prompt_cache_key"
                | "parent_thread_id"
                | "parent_turn_id"
                | "root_turn_id"
                | "thread_source"
                | "subagent_kind"
                | "agent_name"
                | "request_kind"
                | "window_id"
                | "context_window_id"
                | "timezone"
                | "current_date"
        )
}

fn validate_state(request: &Map<String, Value>) -> anyhow::Result<()> {
    if optional::<bool>(Some(request), "background")? == Some(true) {
        anyhow::bail!(
            "background=true requires asynchronous response retrieval, which this streaming runtime does not implement"
        );
    }
    for key in ["previous_response_id", "conversation"] {
        if request
            .get(key)
            .is_some_and(|v| !v.is_null() && v.as_str() != Some(""))
        {
            anyhow::bail!(
                "{key} requires reconstructed caller history; the HTTP entry does not reconstruct stored conversations"
            );
        }
    }
    if optional::<bool>(Some(request), "generate")? == Some(false) {
        anyhow::bail!(
            "generate=false requires WebSocket prewarming, which the HTTP entry does not implement; no model request was sent"
        );
    }
    let _: Option<bool> = optional(Some(request), "stream")?;
    let _ = object(request, "client_metadata")?;
    Ok(())
}

/// Builds the native typed request once; the mainline HTTP sender is unchanged.
pub async fn prepare_inference_request(
    session: &InferenceSession,
    config: &crate::config::Config,
    model_info: &ModelInfo,
    request: &Map<String, Value>,
) -> anyhow::Result<(Value, ApiResponsesOptions)> {
    let parsed = InferenceRequest::parse(config, request)?;
    let mut client = session.session.client.clone();
    client.request_controls = Some(Arc::new(parsed.controls));
    let body = client.build_responses_request(
        &parsed.prompt,
        model_info,
        parsed.effort,
        parsed.summary,
        parsed.service_tier,
        &session.metadata,
        false,
    )?;
    let options = ResponsesHttpExecutor::new(&client)
        .build_responses_options(
            &session.session.turn_state,
            &session.metadata,
            Compression::None,
            model_info.use_responses_lite,
        )
        .await;
    Ok((serde_json::to_value(body)?, options))
}

fn caller_metadata(request: &Map<String, Value>) -> anyhow::Result<HashMap<String, String>> {
    let mut result = HashMap::new();
    if let Some(metadata) = object(request, "client_metadata")? {
        for (key, value) in metadata {
            if key.starts_with("ws_request_")
                || key.starts_with("x-")
                || matches!(
                    key.as_str(),
                    "authorization"
                        | "Authorization"
                        | "headers"
                        | "cookies"
                        | "account_id"
                        | "installation_id"
                        | "prompt_cache_key"
                        | "session_id"
                        | "thread_id"
                        | "turn_id"
                        | "window_id"
                        | "context_window_id"
                        | "parent_thread_id"
                        | "parent_turn_id"
                        | "root_turn_id"
                        | "agent_name"
                        | "request_kind"
                        | "thread_source"
                        | "subagent_kind"
                        | "turn_trigger"
                        | "sandbox"
                        | "sandbox_mode"
                        | "auto_review_enabled"
                        | "node_repl_auto_review_required"
                        | "node_repl_disabled"
                        | "workspaces"
                        | "tool_namespaces_info"
                        | "turn_started_at_unix_ms"
                        | "history_ingest_requested"
                        | "analytics_enabled"
                        | "mcp_attribution"
                )
            {
                continue;
            }
            if let Some(value) = value.as_str() {
                result.insert(key.clone(), value.into());
            }
        }
        let time = super::inference_context::TimeContext::now();
        for (key, value) in [("timezone", time.timezone), ("current_date", time.date)] {
            if metadata.contains_key(key) {
                result.insert(key.into(), value);
            }
        }
    }
    Ok(result)
}
