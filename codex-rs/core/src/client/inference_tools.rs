//! Normalize inference tools without changing names, call IDs or schema constraints.
use anyhow::Context;
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
                    for (child_index, child) in children.iter().enumerate() {
                        let (spec, child) = parse_tool(child)
                            .with_context(|| format!("{location}.tools[{child_index}]"))?;
                        anyhow::ensure!(
                            names.insert((Some(name.clone()), spec.name().to_owned())),
                            "duplicate tool name in namespace"
                        );
                        native_children.push(serde_json::from_value::<
                            codex_tools::ResponsesApiNamespaceTool,
                        >(child)?);
                    }
                    let mut declaration = value.clone();
                    declaration["description"] = json!(description);
                    declaration["tools"] = serde_json::to_value(native_children)?;
                    let spec: ToolSpec = serde_json::from_value(declaration)?;
                    wire.push(serde_json::to_value(&spec)?);
                    native.push(spec);
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
                let selected = name(function)?.to_owned();
                let mut flattened = value.as_object().unwrap().clone();
                flattened.remove("function");
                flattened.insert("type".into(), json!("function"));
                for (key, field) in function.as_object().unwrap() {
                    flattened
                        .entry(key.clone())
                        .or_insert_with(|| field.clone());
                }
                flattened.insert("name".into(), json!(selected));
                *value = Value::Object(flattened);
                if let Some(namespace) = namespace {
                    value["namespace"] = namespace;
                }
            }
            if let Some(mode) = value.as_str() {
                anyhow::ensure!(!mode.is_empty(), "tool_choice mode must not be empty");
                anyhow::ensure!(
                    mode != "required" || !native.is_empty(),
                    "tool_choice requires at least one tool"
                );
            } else if value.get("name").is_none() {
                check_object(value)?;
                // The native request carries tool_choice as JSON. Hosted and
                // allowed-tools choices retain their full native shape; names
                // are checked below only when a choice actually names a tool.
                if value["type"] == "allowed_tools" {
                    if let Some(choices) = value.get("tools").and_then(Value::as_array) {
                        for selected in choices {
                            if let Some(name) = selected.get("name").and_then(Value::as_str) {
                                let namespace = selected
                                    .get("namespace")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned);
                                anyhow::ensure!(
                                    names.contains(&(namespace, name.to_owned())),
                                    "tool_choice references an undefined tool"
                                );
                            }
                        }
                    }
                }
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
    normalize_declaration(&mut value)?;
    // Native serde owns every field, including future additions. No projection
    // through a hand-maintained list of tool fields or schema keywords.
    let spec: ToolSpec = serde_json::from_value(value)?;
    let wire = serde_json::to_value(&spec)?;
    Ok((spec, wire))
}

fn normalize_declaration(value: &mut Value) -> anyhow::Result<()> {
    let object = value.as_object_mut().context("tool must be an object")?;
    // These are protocol defaults, not a filter on supplied declaration fields.
    match object.get("type").and_then(Value::as_str) {
        Some("function") => {
            name(&Value::Object(object.clone()))?;
            default_field(object, "description", json!(""));
            default_field(object, "strict", json!(false));
            default_field(object, "parameters", json!({"type":"object"}));
        }
        Some("custom") => {
            name(&Value::Object(object.clone()))?;
            default_field(object, "description", json!(""));
            default_field(object, "format", json!({"type":"text"}));
        }
        Some("tool_search") => {
            default_field(object, "description", json!(""));
            default_field(object, "execution", json!("server"));
            // An explicitly malformed schema must still fail native parsing.
            object
                .entry("parameters")
                .or_insert_with(|| json!({"type":"object"}));
        }
        _ => {}
    }
    Ok(())
}

fn default_field(object: &mut serde_json::Map<String, Value>, key: &str, default: Value) {
    if object.get(key).is_none_or(Value::is_null) {
        object.insert(key.into(), default);
    }
}

#[cfg(test)]
#[path = "inference_tools_tests.rs"]
mod tests;
