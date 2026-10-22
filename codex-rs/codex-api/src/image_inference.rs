//! Typed standalone Images input, rebuilt with the existing native image DTOs.
use crate::ApiError;
use crate::images::ImageBackground;
use crate::images::ImageEditRequest;
use crate::images::ImageGenerationRequest;
use crate::images::ImageQuality;
use codex_protocol::models::ImageReference;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Map;
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
pub struct ImageInferenceRequest {
    pub model: String,
    pub prompt: String,
    pub images: Option<Vec<ImageReference>>,
    pub background: Option<ImageBackground>,
    pub n: Option<u64>,
    pub quality: Option<ImageQuality>,
    pub size: Option<String>,
    #[serde(flatten)]
    options: ImageInferenceOptions,
}

/// Standalone API options supplement the native generation/edit DTOs. Optional
/// nulls follow native Option serialization; they do not preserve raw JSON shape.
#[derive(Debug, Clone, Deserialize, Serialize)]
struct ImageInferenceOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    mask: Option<ImageReference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_compression: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    partial_images: Option<u64>,
    #[serde(flatten)]
    extensions: Map<String, Value>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum NativeImageRequest {
    Generate(ImageGenerationRequest),
    Edit(ImageEditRequest),
}

#[derive(Serialize)]
pub(crate) struct NativeImageInferenceRequest {
    #[serde(flatten)]
    request: NativeImageRequest,
    #[serde(flatten)]
    options: ImageInferenceOptions,
}

impl ImageInferenceRequest {
    pub(crate) fn build(
        &self,
        operation: ImageOperation,
    ) -> Result<NativeImageInferenceRequest, ApiError> {
        let request = match operation {
            ImageOperation::Generate => NativeImageRequest::Generate(ImageGenerationRequest {
                prompt: self.prompt.clone(),
                background: self.background,
                model: self.model.clone(),
                n: self.n,
                quality: self.quality,
                size: self.size.clone(),
                options: Default::default(),
            }),
            ImageOperation::Edit => NativeImageRequest::Edit(ImageEditRequest {
                images: self
                    .images
                    .clone()
                    .ok_or_else(|| ApiError::InvalidRequest {
                        message: "images is required for image edits".into(),
                    })?,
                prompt: self.prompt.clone(),
                background: self.background,
                model: self.model.clone(),
                n: self.n,
                quality: self.quality,
                size: self.size.clone(),
                options: Default::default(),
            }),
        };
        let mut options = self.options.clone();
        // Extensions are business parameters, never caller-owned transport or
        // identity. Known image fields were already consumed by native parsing.
        options.extensions.retain(|key, _| !worker_owned_field(key));
        Ok(NativeImageInferenceRequest { request, options })
    }
}

fn worker_owned_field(key: &str) -> bool {
    matches!(
        key,
        "headers"
            | "authorization"
            | "Authorization"
            | "cookies"
            | "account_id"
            | "client_metadata"
            | "prompt_cache_key"
            | "x-codex-installation-id"
            | "x-codex-turn-metadata"
            | "x-codex-window-id"
            | "x-codex-parent-thread-id"
            | "x-codex-routing-hint"
            | "x-codex-turn-state"
            | "x-openai-subagent"
            | "installation_id"
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
}

#[derive(Debug, Clone, Copy)]
pub enum ImageOperation {
    Generate,
    Edit,
}

impl ImageOperation {
    pub fn path(self) -> &'static str {
        match self {
            Self::Generate => "images/generations",
            Self::Edit => "images/edits",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn native_images_build_edit_with_ordered_images_mask_and_optional_values() {
        let request: ImageInferenceRequest = serde_json::from_value(json!({
            "model":"gpt-image-1.5", "prompt":"draw", "quality":null,
            "stream":false, "n":0, "output_format":"webp", "output_compression":0,
            "images":[{"image_url":"data:image/png;base64,AAH/","ignored":true},
                {"file_id":"file-1"},{"image_url":"data:image/png;base64,AAH/"}],
            "mask":{"image_url":"data:image/png;base64,/wA=","ignored":true},
            "extension":["first","second"], "authorization":"caller-secret",
            "client_metadata":{"session_id":"caller"}
        }))
        .expect("native image input");
        assert_eq!(
            serde_json::to_value(request.build(ImageOperation::Edit).expect("edit"))
                .expect("serialize"),
            json!({
                "model":"gpt-image-1.5", "prompt":"draw", "stream":false, "n":0,
                "output_format":"webp", "output_compression":0,
                "images":[{"image_url":"data:image/png;base64,AAH/"},{"file_id":"file-1"},
                    {"image_url":"data:image/png;base64,AAH/"}],
                "mask":{"image_url":"data:image/png;base64,/wA="}, "extension":["first","second"]
            })
        );
    }

    #[test]
    fn native_images_require_model_prompt_and_native_scalar_types() {
        for value in [
            json!({}),
            json!({"model":"gpt-image-1.5"}),
            json!({"model":null,"prompt":"draw"}),
            json!({"model":"gpt-image-1.5","prompt":"draw","stream":"true"}),
            json!({"model":"gpt-image-1.5","prompt":"draw","quality":["high","low"]}),
            json!({"model":"gpt-image-1.5","prompt":"draw","quality":"invalid"}),
            json!({"model":"gpt-image-1.5","prompt":"draw","images":[{"invalid":true}]}),
        ] {
            assert!(serde_json::from_value::<ImageInferenceRequest>(value).is_err());
        }
    }

    #[test]
    fn native_images_generation_uses_native_optional_serialization_and_edits_require_images() {
        let request: ImageInferenceRequest = serde_json::from_value(json!({
            "model":"gpt-image-1.5", "prompt":"draw", "background":null, "quality":null,
            "mask":null, "stream":null
        }))
        .expect("native image input");
        assert_eq!(
            serde_json::to_value(request.build(ImageOperation::Generate).expect("generate"))
                .expect("serialize"),
            json!({"model":"gpt-image-1.5", "prompt":"draw"})
        );
        assert!(matches!(
            request.build(ImageOperation::Edit),
            Err(ApiError::InvalidRequest { .. })
        ));
    }
}
