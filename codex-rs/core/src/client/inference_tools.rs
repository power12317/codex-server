//! Normalize inference tools without changing names, call IDs or schema constraints.
use anyhow::Context;
use codex_protocol::config_types::WebSearchFilters;
use codex_protocol::config_types::WebSearchUserLocation;
use codex_tools::FreeformTool;
use codex_tools::FreeformToolFormat;
use codex_tools::JsonSchema;
use codex_tools::ResponsesApiNamespace;
use codex_tools::ResponsesApiNamespaceTool;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use serde_json::Value;
use serde_json::json;
use std::collections::HashSet;

pub(super) struct InferenceTools {
    pub(super) native: Vec<ToolSpec>,
    pub(super) choice: Option<Value>,
}

impl InferenceTools {
    pub(super) fn parse(request: &serde_json::Map<String, Value>) -> anyhow::Result<Self> {
        let mut native = Vec::new();
        let mut wire = Vec::new();
        let mut names = HashSet::new();
        if let Some(tools) = request.get("tools").filter(|v| !v.is_null()) {
            for (index, value) in tools
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("tools must be an array"))?
                .iter()
                .enumerate()
            {
                let location = format!("tools[{index}]");
                if value["type"] == "namespace" {
                    check_object(value).with_context(|| location.clone())?;
                    let name = name(value).with_context(|| location.clone())?.to_owned();
                    let description =
                        optional_description(value).with_context(|| location.clone())?;
                    let children = value["tools"].as_array().ok_or_else(|| {
                        anyhow::anyhow!("{location}: namespace tools must be an array")
                    })?;
                    let mut native_children = Vec::new();
                    let mut wire_children = Vec::new();
                    for (child_index, child) in children.iter().enumerate() {
                        let (spec, child) = parse_tool(child)
                            .with_context(|| format!("{location}.tools[{child_index}]"))?;
                        anyhow::ensure!(
                            names.insert((Some(name.clone()), spec.name().to_owned())),
                            "duplicate tool name in namespace"
                        );
                        native_children.push(match spec {
                            ToolSpec::Function(tool) => ResponsesApiNamespaceTool::Function(tool),
                            ToolSpec::Freeform(tool) => ResponsesApiNamespaceTool::Custom(tool),
                            _ => anyhow::bail!(
                                "{location}.tools[{child_index}]: namespace children must be function/custom tools"
                            ),
                        });
                        wire_children.push(child);
                    }
                    native.push(ToolSpec::Namespace(ResponsesApiNamespace {
                        name: name.clone(),
                        description: description.clone(),
                        tools: native_children,
                    }));
                    wire.push(json!({"type":"namespace", "name":name, "description":description, "tools":wire_children}));
                } else {
                    let (spec, value) = parse_tool(value).with_context(|| location.clone())?;
                    anyhow::ensure!(
                        names.insert((None, spec.name().to_owned())),
                        "duplicate tool name"
                    );
                    native.push(spec);
                    wire.push(value);
                }
            }
        }
        let mut choice = request.get("tool_choice").filter(|v| !v.is_null()).cloned();
        if let Some(value) = &mut choice {
            if let Some(object) = value.as_object_mut() {
                if object.get("namespace").is_some_and(Value::is_null) {
                    object.remove("namespace");
                }
            }
            if let Some(function) = value.get("function") {
                check_object(value)?;
                check_object(function)?;
                anyhow::ensure!(
                    value.get("type").is_none() || value["type"] == "function",
                    "nested tool_choice must select a function"
                );
                let namespace = value.get("namespace").filter(|v| !v.is_null()).cloned();
                *value = json!({"type":"function", "name":name(function)?});
                if let Some(namespace) = namespace {
                    value["namespace"] = namespace;
                }
            }
            if let Some(mode) = value.as_str() {
                anyhow::ensure!(
                    matches!(mode, "auto" | "none" | "required"),
                    "unsupported tool_choice mode"
                );
                anyhow::ensure!(
                    mode != "required" || !native.is_empty(),
                    "tool_choice requires at least one tool"
                );
            } else if matches!(
                value["type"].as_str(),
                Some("web_search" | "tool_search" | "image_generation")
            ) {
                check_object(value)?;
                anyhow::ensure!(
                    wire.iter().any(|tool| tool["type"] == value["type"]),
                    "tool_choice references undefined hosted/search tool"
                );
            } else {
                check_object(value)?;
                let selected = name(value)?;
                let namespace = value
                    .get("namespace")
                    .filter(|v| !v.is_null())
                    .map(|value| {
                        value.as_str().ok_or_else(|| {
                            anyhow::anyhow!("tool_choice namespace must be a string")
                        })
                    })
                    .transpose()?;
                anyhow::ensure!(
                    names.contains(&(namespace.map(str::to_owned), selected.to_owned())),
                    "tool_choice references an undefined tool"
                );
                let candidates = if let Some(namespace) = namespace {
                    wire.iter()
                        .find(|tool| tool["type"] == "namespace" && tool["name"] == namespace)
                        .and_then(|tool| tool["tools"].as_array())
                        .ok_or_else(|| anyhow::anyhow!("undefined namespace"))?
                } else {
                    &wire
                };
                anyhow::ensure!(
                    candidates
                        .iter()
                        .any(|tool| tool["name"] == selected && tool["type"] == value["type"]),
                    "tool_choice type does not match the selected tool"
                );
            }
        }
        Ok(Self { native, choice })
    }
}

fn name(value: &Value) -> anyhow::Result<&str> {
    value["name"]
        .as_str()
        .filter(|name| !name.trim().is_empty() && !name.chars().any(char::is_control))
        .ok_or_else(|| {
            anyhow::anyhow!("tool name must be a nonempty string without control characters")
        })
}

fn optional_description(value: &Value) -> anyhow::Result<String> {
    value
        .get("description")
        .filter(|v| !v.is_null())
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow::anyhow!("tool description must be a string"))
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

// Optional declaration metadata is local to the caller. Native fields below
// carry business semantics; this is structural checking, not a key whitelist.
fn check_object(value: &Value) -> anyhow::Result<()> {
    anyhow::ensure!(
        value.is_object(),
        "tool definition/choice must be an object"
    );
    Ok(())
}

fn parse_tool(value: &Value) -> anyhow::Result<(ToolSpec, Value)> {
    let mut value = value.clone();
    if value.get("function").is_some() {
        check_object(&value)?;
        anyhow::ensure!(
            value.get("type").is_none() || value["type"] == "function",
            "nested tool must be a function"
        );
        value = value["function"].clone();
        anyhow::ensure!(
            value.is_object() && (value.get("type").is_none() || value["type"] == "function"),
            "invalid nested function definition"
        );
        value["type"] = json!("function");
    }
    if value.get("input_schema").is_some() {
        anyhow::ensure!(
            (value.get("type").is_none() || value["type"] == "function")
                && value.get("parameters").is_none(),
            "ambiguous input_schema tool definition"
        );
        let object = value
            .as_object_mut()
            .context("input_schema tool must be an object")?;
        let parameters = object
            .remove("input_schema")
            .context("input_schema is required")?;
        object.insert("parameters".into(), parameters);
        value["type"] = json!("function");
    }
    if value.get("type").is_none()
        && value.get("name").is_some()
        && value.get("parameters").is_some()
    {
        value["type"] = json!("function");
    }
    if value["type"] == "image_generation" {
        let mut options = value
            .as_object()
            .context("image tool must be an object")?
            .clone();
        options.remove("type");
        return Ok((ToolSpec::ImageGeneration { options }, value));
    }
    if value["type"] == "tool_search" {
        check_object(&value)?;
        let parameters = value
            .get("parameters")
            .cloned()
            .unwrap_or_else(|| json!({"type":"object"}));
        let spec = ToolSpec::ToolSearch {
            execution: optional::<String>(&value, "execution")?.unwrap_or_else(|| "server".into()),
            description: optional_description(&value)?,
            parameters: caller_schema(&parameters)?,
        };
        return Ok((spec, value));
    }
    if value["type"] == "web_search" {
        check_object(&value)?;
        let spec = ToolSpec::WebSearch {
            external_web_access: optional(&value, "external_web_access")?,
            indexed_web_access: optional(&value, "indexed_web_access")?,
            filters: optional::<WebSearchFilters>(&value, "filters")?.map(Into::into),
            user_location: optional::<WebSearchUserLocation>(&value, "user_location")?
                .map(Into::into),
            search_context_size: optional(&value, "search_context_size")?,
            search_content_types: optional(&value, "search_content_types")?,
        };
        return Ok((spec, value));
    }
    anyhow::ensure!(
        matches!(value["type"].as_str(), Some("function" | "custom")),
        "unsupported tool type: {}",
        value["type"]
    );
    let tool_name = name(&value)?.to_owned();
    let description = optional_description(&value)?;
    let defer_loading = optional::<bool>(&value, "defer_loading")?;
    match value["type"].as_str() {
        Some("function") => {
            check_object(&value)?;
            let parameters = value
                .get("parameters")
                .cloned()
                .unwrap_or_else(|| json!({"type":"object"}));
            let strict = optional::<bool>(&value, "strict")?.unwrap_or(false);
            let spec = ToolSpec::Function(ResponsesApiTool {
                name: tool_name,
                description: description.clone(),
                strict,
                defer_loading,
                parameters: caller_schema(&parameters)?,
                output_schema: value
                    .get("output_schema")
                    .filter(|v| !v.is_null())
                    .cloned()
                    .map(codex_tools::ToolOutputSchema::from_caller),
            });
            value["parameters"] = parameters;
            value["description"] = json!(description);
            Ok((spec, value))
        }
        Some("custom") => {
            check_object(&value)?;
            let format = value
                .get("format")
                .cloned()
                .unwrap_or_else(|| json!({"type":"text"}));
            let native_format = match format["type"].as_str() {
                Some("text") => {
                    check_object(&format)?;
                    FreeformToolFormat {
                        r#type: "text".into(),
                        syntax: String::new(),
                        definition: String::new(),
                    }
                }
                Some("grammar") => {
                    check_object(&format)?;
                    serde_json::from_value(format.clone())?
                }
                _ => anyhow::bail!("unsupported custom tool format"),
            };
            value["format"] = format;
            value["description"] = json!(description);
            let spec = ToolSpec::Freeform(FreeformTool {
                name: tool_name,
                description,
                defer_loading,
                format: native_format,
            });
            Ok((spec, value))
        }
        _ => {
            anyhow::bail!("only caller-executed function/custom tools and namespaces are supported")
        }
    }
}

fn optional<T: serde::de::DeserializeOwned>(value: &Value, key: &str) -> anyhow::Result<Option<T>> {
    value
        .get(key)
        .map(|value| {
            serde_json::from_value::<Option<T>>(value.clone()).with_context(|| key.to_owned())
        })
        .transpose()
        .map(Option::flatten)
}

fn caller_schema(parameters: &Value) -> anyhow::Result<JsonSchema> {
    let schema = parameters
        .as_object()
        .context("tool parameters must be a JSON Schema object")?;
    Ok(JsonSchema::from_raw_schema(schema.clone()))
}

#[cfg(test)]
#[path = "inference_tools_tests.rs"]
mod tests;
