//! Convert Responses output envelopes to the same typed history used by the native client.
//! Only response-only fields understood here may be omitted; arbitrary extensions still fail.
use codex_protocol::models::ResponseItem;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum OutputStatus {
    InProgress,
    Completed,
    Incomplete,
}

#[derive(Deserialize)]
struct OutputTextMetadata {
    #[serde(default)]
    annotations: Option<Vec<Value>>,
    #[serde(default)]
    logprobs: Option<Vec<Value>>,
}

pub(super) fn parse(items: &[Value]) -> anyhow::Result<Vec<ResponseItem>> {
    items
        .iter()
        .enumerate()
        .map(|(index, original)| {
            let item: ResponseItem = serde_json::from_value(original.clone())?;
            let native = serde_json::to_value(&item)?;
            let mut history = original.clone();
            let path = format!("input[{index}]");
            if matches!(
                item,
                ResponseItem::Message { .. }
                    | ResponseItem::FunctionCall { .. }
                    | ResponseItem::FunctionCallOutput { .. }
                    | ResponseItem::CustomToolCallOutput { .. }
                    | ResponseItem::Reasoning { .. }
                    | ResponseItem::Compaction { .. }
                    | ResponseItem::ContextCompaction { .. }
            ) && let Some(status) = history.get("status")
            {
                let _: Option<OutputStatus> =
                    serde_json::from_value(status.clone()).map_err(|error| {
                        anyhow::anyhow!("invalid response output status at {path}: {error}")
                    })?;
                if let Some(fields) = history.as_object_mut() {
                    fields.remove("status");
                }
            }
            if matches!(item, ResponseItem::Compaction { .. })
                && history["type"] == "compaction_summary"
            {
                history["type"] = Value::String("compaction".into());
            }
            if matches!(item, ResponseItem::Message { .. })
                && let Some(content) = history.get_mut("content").and_then(Value::as_array_mut)
            {
                for part in content {
                    if part["type"] == "output_text" {
                        let metadata: OutputTextMetadata = serde_json::from_value(part.clone())?;
                        // Native output_text history retains its text, while annotations/logprobs
                        // describe the emitted response rather than input instructions.
                        let _ = (metadata.annotations, metadata.logprobs);
                        if let Some(fields) = part.as_object_mut() {
                            fields.remove("annotations");
                            fields.remove("logprobs");
                        }
                    }
                }
            }
            // Native reasoning serialization intentionally omits non-text content. Deserialization
            // above still validates that field against ReasoningItemContent.
            if matches!(item, ResponseItem::Reasoning { .. })
                && native.get("content").is_none()
                && let Some(fields) = history.as_object_mut()
            {
                fields.remove("content");
            }
            super::preserve_input_fields(&history, &native, &path)?;
            Ok(item)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

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
        assert!(native[0].get("status").is_none());
        assert!(native[0]["content"][0].get("annotations").is_none());
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
    fn response_adapter_rejects_unknown_or_invalid_fields() {
        for item in [
            json!({"type":"message","role":"assistant","status":"invented","content":[]}),
            json!({"type":"message","role":"assistant","future":1,"content":[]}),
            json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"a","annotations":"invalid"}]}),
            json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"a","future":1}]}),
            json!({"type":"compaction_trigger","status":"completed"}),
        ] {
            assert!(parse(&[item]).is_err());
        }
    }
}
