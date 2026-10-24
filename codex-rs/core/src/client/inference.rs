//! Compatible business input for the shared native model builder.
use super::*;
use anyhow::Context;
use codex_api::ResponsesGenerationOptions;
use codex_protocol::models::BaseInstructions;
use serde::de::DeserializeOwned;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;

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
    pub generate: Option<bool>,
    pub generation: ResponsesGenerationOptions,
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
        let mut input = input::parse(items)?;
        let tools = super::inference_tools::InferenceTools::parse(request)?;
        // Hosted image declarations use the same native AdditionalTools input as the
        // image extension. Client functions named WebSearch remain ordinary functions.
        let mut native = Vec::new();
        let mut hosted = Vec::new();
        for tool in tools.native {
            match tool {
                codex_tools::ToolSpec::ImageGeneration { .. } => {
                    hosted.push(serde_json::to_value(tool)?)
                }
                tool => native.push(tool),
            }
        }
        if !hosted.is_empty() {
            input.insert(
                0,
                ResponseItem::AdditionalTools {
                    id: None,
                    role: "developer".into(),
                    tools: hosted.into(),
                },
            );
        }
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
        let mut generation: ResponsesGenerationOptions =
            serde_json::from_value(Value::Object(request.clone()))?;
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
            generate: optional(Some(request), "generate")?,
            generation,
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

fn validate_state(request: &Map<String, Value>) -> anyhow::Result<()> {
    if optional::<bool>(Some(request), "background")? == Some(true) {
        anyhow::bail!(
            "background=true requires asynchronous response retrieval, which this streaming runtime does not implement"
        );
    }
    for key in ["previous_response_id", "conversation"] {
        if request.get(key).is_some_and(|v| !v.is_null()) {
            anyhow::bail!(
                "{key} requires reconstructed caller history; this runtime only retains native continuation caches, not stored conversations"
            );
        }
    }
    let _: Option<bool> = optional(Some(request), "stream")?;
    let _ = object(request, "client_metadata")?;
    Ok(())
}
