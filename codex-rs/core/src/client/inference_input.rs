//! Convert Responses output envelopes to the same typed history used by the native client.
//! Native replay metadata, business items and tool associations remain native inputs.
use anyhow::Context;
use codex_protocol::ResponseItemId;
use codex_protocol::models::ResponseItem;
use serde_json::Value;
use serde_json::json;

pub(super) fn parse(items: &[Value]) -> anyhow::Result<Vec<ResponseItem>> {
    let mut normalized = items.to_vec();
    // Only input-local, still-unanswered calls can supply an omitted association.
    // Never invent a call ID from a display item ID or from another thread.
    let mut pending: Vec<(String, String, Option<String>, bool)> = Vec::new();
    for (index, history) in normalized.iter_mut().enumerate() {
        let path = format!("input[{index}]");
        let fields = history
            .as_object_mut()
            .with_context(|| format!("{path} must be an object"))?;
        if !fields.contains_key("type") && fields.contains_key("role") {
            fields.insert("type".into(), json!("message"));
        }
        if fields.get("type").and_then(Value::as_str) == Some("message") {
            if let Some(content) = fields.get_mut("content") {
                if content.is_string() {
                    *content = json!([{"type":"input_text","text":content}]);
                }
                if let Some(parts) = content.as_array_mut() {
                    for part in parts {
                        if let Some(part) = part.as_object_mut() {
                            if !part.contains_key("type") && part.contains_key("text") {
                                part.insert("type".into(), json!("input_text"));
                            }
                        }
                    }
                }
            }
        }
        if fields.get("type").and_then(Value::as_str) == Some("compaction_summary") {
            fields.insert("type".into(), json!("compaction"));
        }
        let kind = fields
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if matches!(kind.as_str(), "function_call" | "custom_tool_call") {
            if let (Some(id), Some(name)) = (
                fields.get("call_id").and_then(Value::as_str),
                fields.get("name").and_then(Value::as_str),
            ) {
                pending.push((
                    id.into(),
                    name.into(),
                    fields
                        .get("namespace")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    kind == "custom_tool_call",
                ));
            }
        } else if matches!(
            kind.as_str(),
            "function_call_output" | "custom_tool_call_output"
        ) {
            let custom = kind == "custom_tool_call_output";
            if fields
                .get("call_id")
                .is_none_or(|v| v.is_null() || v.as_str() == Some(""))
            {
                let name = fields.get("name").and_then(Value::as_str);
                let namespace = fields.get("namespace").and_then(Value::as_str);
                let candidates: Vec<_> = pending
                    .iter()
                    .filter(|(_, n, ns, c)| {
                        *c == custom
                            && name.is_none_or(|name| name == n)
                            && namespace.is_none_or(|namespace| Some(namespace) == ns.as_deref())
                    })
                    .collect();
                if candidates.len() == 1 {
                    fields.insert("call_id".into(), json!(candidates[0].0));
                } else if candidates.len() > 1 {
                    anyhow::bail!(
                        "{path} has ambiguous tool output: more than one unfinished call matches"
                    );
                }
            }
            if let Some(id) = fields.get("call_id").and_then(Value::as_str) {
                if let Some(index) = pending
                    .iter()
                    .position(|(candidate, _, _, _)| candidate == id)
                {
                    let (_, name, namespace, call_custom) = &pending[index];
                    anyhow::ensure!(
                        *call_custom == custom
                            && fields
                                .get("name")
                                .and_then(Value::as_str)
                                .is_none_or(|n| n == name)
                            && fields
                                .get("namespace")
                                .and_then(Value::as_str)
                                .is_none_or(|ns| Some(ns) == namespace.as_deref()),
                        "{path} contradicts the identified tool call"
                    );
                    pending.remove(index);
                }
            }
        } else if matches!(kind.as_str(), "compaction" | "context_compaction") {
            pending.clear();
        }
        if fields.get("type").and_then(Value::as_str) == Some("reasoning") {
            fields.entry("summary").or_insert_with(|| json!([]));
        }
    }
    super::super::inference_context::TimeContext::now().apply(&mut normalized);
    normalized
        .into_iter()
        .enumerate()
        .map(|(index, history)| {
            let path = format!("input[{index}]");
            let mut item: ResponseItem =
                serde_json::from_value(history).with_context(|| format!("invalid {path}"))?;
            anyhow::ensure!(
                !matches!(item, ResponseItem::Other),
                "{path} contains an unknown business item type; its contents were not discarded"
            );
            match &item {
                ResponseItem::FunctionCall { call_id, name, .. } => anyhow::ensure!(
                    !call_id.is_empty() && !name.is_empty(),
                    "{path} requires a tool name and call_id"
                ),
                ResponseItem::FunctionCallOutput { call_id, name, .. } => anyhow::ensure!(
                    call_id.as_deref().is_some_and(|id| !id.is_empty()) || name.as_deref().is_some_and(|name| !name.is_empty()),
                    "{path} requires call_id or an explicit native tool name; no association can be inferred"
                ),
                ResponseItem::CustomToolCall { call_id, .. }
                | ResponseItem::CustomToolCallOutput { call_id, .. } => anyhow::ensure!(
                    !call_id.is_empty(),
                    "{path} requires call_id; no tool association can be inferred"
                ),
                _ => {}
            }
            if let Some(prefix) = item.id_prefix()
                && let Some(id) = item.id()
                && let Some((actual, suffix)) = id.split_once('_')
                && !suffix.is_empty()
                && actual != prefix
            {
                item.set_id(Some(ResponseItemId::with_suffix(prefix, suffix)));
            }
            Ok(item)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[tokio::test]
    async fn export_native_formatter_expectations_for_http_replay() {
        let home = tempfile::TempDir::new().unwrap();
        let config = crate::config::ConfigBuilder::default()
            .loader_overrides(codex_config::LoaderOverrides::without_managed_config_for_tests())
            .codex_home(home.path().to_path_buf())
            .build()
            .await
            .unwrap();
        let mut model = codex_models_manager::test_support::construct_model_info_offline_for_tests(
            "gpt-6.1-sol",
            &config.to_models_manager_config(),
        );
        if let Ok(directory) = std::env::var("CODEX_INFERENCE_FIXTURE_DIR") {
            let directory = std::path::Path::new(&directory);
            std::fs::create_dir_all(directory).unwrap();
            for lite in [false, true] {
                model.use_responses_lite = lite;
                let expected = crate::test_support::inference_fixtures::native_request_history(
                    &config, &model,
                );
                let suffix = if lite { "lite" } else { "responses" };
                for (name, value) in [
                    (
                        format!("native-history.expected-gpt-6.1-sol-{suffix}.json"),
                        serde_json::to_value(expected).unwrap(),
                    ),
                    (
                        format!("native-model-gpt-6.1-sol-{suffix}.json"),
                        serde_json::to_value(&model).unwrap(),
                    ),
                ] {
                    std::fs::write(
                        directory.join(name),
                        serde_json::to_string_pretty(&value).unwrap() + "\n",
                    )
                    .unwrap();
                }
            }
        }
    }

    #[test]
    fn every_native_history_field_survives_the_input_adapter() {
        let items = crate::test_support::inference_fixtures::history();
        let expected = serde_json::to_value(&items).unwrap();
        let input = expected.as_array().unwrap();
        let actual = serde_json::to_value(parse(input).unwrap()).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(
            items
                .iter()
                .map(crate::test_support::inference_fixtures::history_kind)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            17
        );
        if let Ok(directory) = std::env::var("CODEX_INFERENCE_FIXTURE_DIR") {
            let directory = std::path::Path::new(&directory);
            std::fs::create_dir_all(directory).unwrap();
            for (name, value) in [
                ("native-history.full.json", expected),
                (
                    "native-history.json",
                    serde_json::to_value(crate::test_support::inference_fixtures::replay_history())
                        .unwrap(),
                ),
                (
                    "native-tools.json",
                    serde_json::to_value(crate::test_support::inference_fixtures::tools()).unwrap(),
                ),
                (
                    "native-controls.json",
                    crate::test_support::inference_fixtures::request_controls(),
                ),
                (
                    "source-metadata.json",
                    crate::test_support::inference_fixtures::source_metadata(),
                ),
            ] {
                std::fs::write(
                    directory.join(name),
                    serde_json::to_string_pretty(&value).unwrap() + "\n",
                )
                .unwrap();
            }
        }
    }

    #[test]
    fn native_required_status_is_not_invented_and_optional_status_stays_optional() {
        for item in [
            json!({"type":"image_generation_call","result":"image"}),
            json!({"type":"tool_search_output","execution":"client","tools":[]}),
        ] {
            assert!(parse(&[item]).is_err());
        }
        let input = json!([{"type":"web_search_call","action":{"type":"search","query":"fixture"}}, {"type":"compaction_trigger"}]);
        assert_eq!(
            serde_json::to_value(parse(input.as_array().unwrap()).unwrap()).unwrap(),
            input
        );
    }

    #[test]
    fn response_output_adapts_to_native_history_without_changing_call_semantics() {
        let items = vec![
            json!({"type":"message","id":"msg_1","role":"assistant","status":"completed","phase":"final_answer","content":[{"type":"output_text","text":"hello","annotations":[],"logprobs":[]}]}),
            json!({"type":"function_call","id":"fc_1","status":"completed","namespace":"functions","name":"read","arguments":"{}","call_id":"call_1"}),
            json!({"type":"function_call_output","namespace":"functions","name":"read","call_id":"call_1","output":"read result"}),
            json!({"type":"custom_tool_call","id":"ctc_1","status":"completed","namespace":"functions","name":"run","input":"echo hi","call_id":"call_2"}),
            json!({"type":"custom_tool_call_output","status":"completed","call_id":"call_2","output":"hi"}),
            json!({"type":"compaction_summary","id":"cmp_1","status":"completed","encrypted_content":"opaque"}),
            json!({"type":"compaction_trigger"}),
        ];
        let parsed = parse(&items).unwrap();
        let native = serde_json::to_value(parsed).unwrap();
        assert_eq!(native[0]["status"], "completed");
        assert_eq!(native[0]["content"][0]["annotations"], json!([]));
        assert_eq!(native[0]["content"][0]["logprobs"], json!([]));
        assert_eq!(native[1]["status"], "completed");
        assert_eq!(native[0]["phase"], "final_answer");
        assert_eq!(native[1]["namespace"], "functions");
        assert_eq!(native[2]["output"], "read result");
        assert_eq!(native[3]["status"], "completed");
        assert_eq!(native[4]["output"], "hi");
        assert_eq!(native[5]["type"], "compaction");
        assert_eq!(native[5]["encrypted_content"], "opaque");
        assert_eq!(native[6], items[6]);
    }

    #[test]
    fn tool_outputs_infer_only_an_unambiguous_unfinished_call() {
        let call =
            json!({"type":"function_call","name":"Read","call_id":"call-1","arguments":"{}"});
        let result = json!({"type":"function_call_output","output":"file contents"});
        let parsed = parse(&[call.clone(), result.clone()]).unwrap();
        assert_eq!(
            serde_json::to_value(&parsed[1]).unwrap()["call_id"],
            "call-1"
        );
        let mut second = call.clone();
        second["call_id"] = json!("call-2");
        assert!(
            parse(&[call.clone(), second, result.clone()])
                .unwrap_err()
                .to_string()
                .contains("ambiguous")
        );
        assert!(parse(&[result]).is_err());
    }

    #[test]
    fn invalid_native_content_and_missing_call_associations_are_reported() {
        let displayed = json!({"role":"assistant","status":"displayed","metadata":{"ui":true},"content":[{"type":"output_text","text":"a","annotations":"display extension","logprobs":{}}]});
        assert!(parse(&[displayed]).is_err());
        for item in [
            json!({"type":"unknown_business","content":"must not disappear"}),
            json!({"type":"function_call_output","output":"missing association"}),
            json!({"type":"message","role":"user","content":12}),
        ] {
            assert!(parse(&[item]).is_err());
        }
    }
}
