//! Compatible business input for the mainline native request builder and HTTP bridge.
use super::*;
use anyhow::Context;
use codex_api::ResponsesGenerationOptions;
use codex_api::ResponsesTextFormat;
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
    pub format: Option<ResponsesTextFormat>,
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
        let extensions = validate_state(request)?;
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
        let text = object(request, "text")?;
        let stream_options = object(request, "stream_options")?;
        let effort =
            optional(reasoning, "effort")?.or_else(|| config.model_reasoning_effort.clone());
        let summary = optional(reasoning, "summary")?
            .or(config.model_reasoning_summary)
            .unwrap_or_default();
        let mut generation = ResponsesGenerationOptions {
            max_output_tokens: optional(Some(request), "max_output_tokens")?,
            temperature: optional(Some(request), "temperature")?,
            top_p: optional(Some(request), "top_p")?,
            truncation: optional(Some(request), "truncation")?,
            prompt_cache_retention: optional(Some(request), "prompt_cache_retention")?,
            safety_identifier: optional(Some(request), "safety_identifier")?,
            user: optional(Some(request), "user")?,
            extensions,
        };
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
            format: text
                .and_then(|o| o.get("format"))
                .filter(|v| !v.is_null())
                .map(|v| {
                    let mut format = v.clone();
                    if let Some(o) = format.as_object_mut() {
                        for key in ["name", "strict", "type"] {
                            if o.get(key).is_some_and(Value::is_null) {
                                o.remove(key);
                            }
                        }
                        if !o.contains_key("type") && o.contains_key("schema") {
                            o.insert("type".into(), json!("json_schema"));
                        }
                    }
                    serde_json::from_value(format).context("invalid text.format")
                })
                .transpose()?,
            context: optional(reasoning, "context")?,
            verbosity: optional(text, "verbosity")?,
            parallel_tool_calls: optional(Some(request), "parallel_tool_calls")?,
            stream_options: optional(stream_options, "reasoning_summary_delivery")?.map(
                |reasoning_summary_delivery| StreamOptions {
                    reasoning_summary_delivery,
                },
            ),
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

fn validate_state(request: &Map<String, Value>) -> anyhow::Result<Map<String, Value>> {
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
    // Transport/auth/installation and display metadata belong to their own layer.
    // Unknown business controls cannot be silently discarded as if implemented.
    let mut extensions = Map::new();
    for (key, value) in request {
        if matches!(
            key.as_str(),
            "model"
                | "input"
                | "instructions"
                | "tools"
                | "tool_choice"
                | "reasoning"
                | "parallel_tool_calls"
                | "text"
                | "service_tier"
                | "stream"
                | "store"
                | "background"
                | "include"
                | "client_metadata"
                | "metadata"
                | "stream_options"
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
                | "headers"
                | "authorization"
                | "Authorization"
                | "cookies"
                | "installation_id"
                | "user_agent"
                | "originator"
                | "device_id"
                | "access_programs"
                | "account_id"
                | "previous_response_id"
                | "conversation"
                | "generate"
                | "max_output_tokens"
                | "max_completion_tokens"
                | "max_tokens"
                | "temperature"
                | "top_p"
                | "truncation"
                | "prompt_cache_retention"
                | "safety_identifier"
                | "user"
                | "timezone"
                | "current_date"
        ) || key.starts_with("x-")
            || key.starts_with("ws_request_")
        {
            continue;
        }
        if matches!(key.as_str(), "top_k" | "prompt_cache_options") {
            anyhow::bail!("business field `{key}` has no native Responses mapping");
        }
        // Mainline already supports native HTTP business extensions. Keep them in
        // the native DTO, without a serialized-body overlay or transport overrides.
        extensions.insert(key.clone(), value.clone());
    }
    for (name, allowed) in [
        ("reasoning", &["effort", "summary", "context"][..]),
        ("text", &["format", "verbosity"][..]),
        (
            "stream_options",
            &["include_usage", "reasoning_summary_delivery"][..],
        ),
    ] {
        if let Some(object) = object(request, name)? {
            for key in object.keys() {
                if !allowed.contains(&key.as_str()) && key != "metadata" && !key.starts_with("x-") {
                    anyhow::bail!("{name}.{key} has no native business mapping");
                }
            }
        }
    }
    Ok(extensions)
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
