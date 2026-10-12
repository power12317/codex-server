//! Restricted business input for native model execution. No final wire-body overlay.
use super::*;
use codex_protocol::ResponseItemId;
use codex_protocol::models::BaseInstructions;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;

pub(super) struct InferenceRequest {
    pub(super) prompt: Prompt,
    pub(super) effort: Option<ReasoningEffortConfig>,
    pub(super) summary: ReasoningSummaryConfig,
    pub(super) service_tier: Option<String>,
    pub(super) verbosity: Option<VerbosityConfig>,
}

impl InferenceRequest {
    pub(super) fn parse(
        config: &crate::config::Config,
        request: &Map<String, Value>,
    ) -> anyhow::Result<Self> {
        validate_fields(request)?;
        let mut input = request.get("input").cloned().unwrap_or_else(|| json!([]));
        if input.is_string() {
            input = json!([{"type":"message","role":"user","content":[{"type":"input_text","text":input}]}]);
        }
        let input_items = input
            .as_array_mut()
            .ok_or_else(|| anyhow::anyhow!("input must be text or an array"))?;
        for item in &mut *input_items {
            if item.get("role").is_some() && item.get("type").is_none() {
                item["type"] = json!("message");
            }
            if item["type"] == "message" && item["content"].is_string() {
                item["content"] = json!([{"type":"input_text","text":item["content"]}]);
            }
        }
        let time = super::inference_context::TimeContext::now();
        time.apply(input_items);
        let original_input = input.clone();
        let mut input: Vec<ResponseItem> = serde_json::from_value(input)?;
        preserve_input_fields(&original_input, &serde_json::to_value(&input)?, "input")?;
        for item in &mut input {
            if let Some(prefix) = item.id_prefix()
                && let Some(id) = item.id()
                && let Some((actual, suffix)) = id.split_once('_')
                && !suffix.is_empty()
                && actual != prefix
            {
                item.set_id(Some(ResponseItemId::with_suffix(prefix, suffix)));
            }
        }
        anyhow::ensure!(
            !input.iter().any(|item| matches!(item, ResponseItem::Other)),
            "unsupported input item"
        );
        let mut tool_request = request.clone();
        let hosted = if let Some(tools) = tool_request.get_mut("tools") {
            let tools = tools
                .as_array_mut()
                .ok_or_else(|| anyhow::anyhow!("tools must be an array"))?;
            let mut hosted = Vec::new();
            tools.retain(|tool| {
                if tool["type"] == "image_generation" {
                    hosted.push(tool.clone());
                    false
                } else {
                    true
                }
            });
            hosted
        } else {
            Vec::new()
        };
        let tools = super::inference_tools::InferenceTools::parse(&tool_request)?;
        anyhow::ensure!(
            tools.choice.as_ref().is_none_or(|choice| choice == "auto"),
            "native model input supports only tool_choice=auto"
        );
        if !hosted.is_empty() {
            input.insert(
                0,
                ResponseItem::AdditionalTools {
                    id: None,
                    role: "developer".into(),
                    tools: hosted,
                },
            );
        }
        let prompt = Prompt {
            input,
            tools: tools.native.into(),
            parallel_tool_calls: request
                .get("parallel_tool_calls")
                .map(|v| serde_json::from_value(v.clone()))
                .transpose()?
                .unwrap_or(true),
            base_instructions: BaseInstructions {
                text: request
                    .get("instructions")
                    .map(|v| serde_json::from_value(v.clone()))
                    .transpose()?
                    .unwrap_or_default(),
                ..Default::default()
            },
            output_schema: request
                .get("text")
                .and_then(|t| t.get("format"))
                .and_then(|f| f.get("schema"))
                .cloned(),
            output_schema_strict: request
                .get("text")
                .and_then(|t| t.get("format"))
                .and_then(|f| f.get("strict"))
                .map(|v| serde_json::from_value(v.clone()))
                .transpose()?
                .unwrap_or(true),
            cyber_access_program: None,
        };
        let reasoning = request.get("reasoning");
        let effort = reasoning
            .and_then(|r| r.get("effort"))
            .map(|v| serde_json::from_value(v.clone()))
            .transpose()?;
        let effort = effort.or_else(|| config.model_reasoning_effort.clone());
        let summary = reasoning
            .and_then(|r| r.get("summary"))
            .map(|v| serde_json::from_value(v.clone()))
            .transpose()?
            .or(config.model_reasoning_summary)
            .unwrap_or_default();
        let service_tier: Option<String> = request
            .get("service_tier")
            .map(|v| serde_json::from_value(v.clone()))
            .transpose()?;
        Ok(Self {
            prompt,
            effort,
            summary,
            service_tier: service_tier.or_else(|| config.service_tier.clone()),
            verbosity: request
                .get("text")
                .and_then(|text| text.get("verbosity"))
                .map(|value| serde_json::from_value(value.clone()))
                .transpose()?
                .or(config.model_verbosity),
        })
    }
}

fn validate_fields(request: &Map<String, Value>) -> anyhow::Result<()> {
    if let Some(metadata) = request.get("client_metadata") {
        let metadata = metadata
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("client_metadata must be an object"))?;
        for key in metadata.keys() {
            anyhow::ensure!(
                matches!(
                    key.as_str(),
                    "session_id"
                        | "thread_id"
                        | "turn_id"
                        | "prompt_cache_key"
                        | "parent_turn_id"
                        | "root_turn_id"
                        | "parent_thread_id"
                        | "window_id"
                        | "context_window_id"
                        | "thread_source"
                        | "subagent_kind"
                        | "agent_name"
                        | "request_kind"
                        | "installation_id"
                        | "x-codex-installation-id"
                        | "x-codex-turn-metadata"
                        | "x-codex-window-id"
                        | "x-codex-parent-thread-id"
                        | "x-codex-routing-hint"
                        | "x-codex-turn-state"
                        | "x-openai-subagent"
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
                        | "ws_request_header_traceparent"
                        | "ws_request_header_x_openai_internal_codex_responses_lite"
                ),
                "client_metadata.{key} is not supported by the native model input API"
            );
        }
    }
    for key in request.keys() {
        anyhow::ensure!(
            matches!(
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
                    | "account_id"
                    | "x-codex-installation-id"
                    | "x-codex-turn-metadata"
                    | "x-codex-window-id"
                    | "x-codex-parent-thread-id"
                    | "x-codex-routing-hint"
                    | "x-codex-turn-state"
                    | "x-openai-subagent"
            ),
            "field `{key}` is not supported by the native model input API"
        );
    }
    if let Some(choice) = request.get("tool_choice") {
        anyhow::ensure!(
            choice == "auto",
            "native model input supports only tool_choice=auto"
        );
    }
    for key in ["store", "background"] {
        if let Some(value) = request.get(key) {
            anyhow::ensure!(
                value == &Value::Bool(false),
                "native model input does not support {key}=true"
            );
        }
    }
    if let Some(value) = request.get("stream") {
        anyhow::ensure!(value.is_boolean(), "stream must be boolean");
    }
    if let Some(value) = request.get("include") {
        anyhow::ensure!(
            value == &json!(["reasoning.encrypted_content"]),
            "include must match native encrypted reasoning inclusion"
        );
    }
    if let Some(reasoning) = request.get("reasoning") {
        let fields = reasoning
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("reasoning must be an object"))?;
        for key in fields.keys() {
            anyhow::ensure!(
                matches!(key.as_str(), "effort" | "summary" | "context"),
                "unsupported reasoning field: {key}"
            );
        }
    }
    if let Some(text) = request.get("text") {
        let text = text
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("text must be an object"))?;
        anyhow::ensure!(
            text.keys()
                .all(|key| matches!(key.as_str(), "format" | "verbosity")),
            "only text.format and text.verbosity are supported"
        );
        if let Some(format) = text.get("format") {
            let format = format
                .as_object()
                .ok_or_else(|| anyhow::anyhow!("text.format must be an object"))?;
            anyhow::ensure!(
                format
                    .keys()
                    .all(|key| matches!(key.as_str(), "type" | "schema" | "strict" | "name")),
                "unsupported text.format field"
            );
            anyhow::ensure!(
                format.get("type") == Some(&json!("json_schema")) && format.contains_key("schema"),
                "native structured output requires json_schema and schema"
            );
            if let Some(name) = format.get("name") {
                anyhow::ensure!(
                    name == "codex_output_schema",
                    "native output schema name is codex_output_schema"
                );
            }
        }
    }
    Ok(())
}

// Typed native inputs must not silently discard caller-supplied extension fields.
fn preserve_input_fields(original: &Value, native: &Value, path: &str) -> anyhow::Result<()> {
    match (original, native) {
        (Value::Object(original), Value::Object(native)) => {
            for (key, value) in original {
                if value.is_null() && !native.contains_key(key) {
                    continue;
                }
                let found = native.get(key).ok_or_else(|| {
                    anyhow::anyhow!("unsupported native input field: {path}.{key}")
                })?;
                preserve_input_fields(value, found, &format!("{path}.{key}"))?;
            }
        }
        (Value::Array(original), Value::Array(native)) if original.len() == native.len() => {
            for (index, (original, native)) in original.iter().zip(native).enumerate() {
                preserve_input_fields(original, native, &format!("{path}[{index}]"))?;
            }
        }
        _ => anyhow::ensure!(original == native, "lossy native input field: {path}"),
    }
    Ok(())
}
