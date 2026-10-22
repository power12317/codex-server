//! Business DTO adaptation to the existing native Images backend.
use super::BridgeResult;
use super::failure;
use super::native_failure;
use codex_api::ImageEditRequest;
use codex_api::ImageGenerationRequest;
use codex_api::ImageRequestOptions;
use codex_app_server_protocol::CpaInferenceStartParams;
use codex_core::InferenceSession;
use codex_core::config::Config;
use codex_image_generation_extension::backend::CodexImagesBackend;
use codex_model_provider::SharedModelProvider;
use serde::de::DeserializeOwned;
use serde_json::Map;
use serde_json::Value;

pub(super) async fn execute(
    provider: SharedModelProvider,
    config: &Config,
    params: &CpaInferenceStartParams,
    session: &InferenceSession,
) -> BridgeResult<()> {
    let fields = &params.request;
    for key in fields.keys() {
        if matches!(
            key.as_str(),
            "model"
                | "prompt"
                | "images"
                | "mask"
                | "background"
                | "n"
                | "quality"
                | "size"
                | "stream"
                | "output_format"
                | "output_compression"
                | "partial_images"
                | "response_format"
                | "input_fidelity"
                | "moderation"
                | "user"
                | "metadata"
                | "client_metadata"
                | "session_id"
                | "thread_id"
                | "turn_id"
                | "prompt_cache_key"
                | "headers"
                | "authorization"
                | "cookies"
                | "installation_id"
                | "account_id"
        ) || key.starts_with("x-")
        {
            continue;
        }
        return Err(failure(
            400,
            &format!("image business option `{key}` has no native mapping"),
        ));
    }
    if params.operation == "images/generations" {
        for key in ["mask", "images"] {
            if fields.get(key).is_some_and(|v| !v.is_null()) {
                return Err(failure(
                    400,
                    &format!("{key} belongs to images/edits, not images/generations"),
                ));
            }
        }
    }

    let options = ImageRequestOptions {
        mask: optional(fields, "mask")?,
        stream: optional(fields, "stream")?,
        output_format: optional(fields, "output_format")?,
        output_compression: optional(fields, "output_compression")?,
        partial_images: optional(fields, "partial_images")?,
        response_format: optional(fields, "response_format")?,
        input_fidelity: optional(fields, "input_fidelity")?,
        moderation: optional(fields, "moderation")?,
        user: optional(fields, "user")?,
    };
    let request = ImageGenerationRequest {
        prompt: required(fields, "prompt")?,
        model: required(fields, "model")?,
        background: optional(fields, "background")?,
        n: optional(fields, "n")?,
        quality: optional(fields, "quality")?,
        size: optional(fields, "size")?,
        options,
    };
    let backend = CodexImagesBackend::new(
        provider,
        config.http_client_factory(),
        Some(codex_login::default_client::originator().value),
    );
    let mapping = session.mapping();
    let turn_id = mapping
        .turn_id
        .as_deref()
        .ok_or_else(|| failure(400, "image turn id is required"))?;
    if params.operation == "images/edits" {
        backend
            .edit_live_once(
                ImageEditRequest {
                    images: required(fields, "images")?,
                    prompt: request.prompt,
                    background: request.background,
                    model: request.model,
                    n: request.n,
                    quality: request.quality,
                    size: request.size,
                    options: request.options,
                },
                turn_id,
            )
            .await
    } else {
        backend.generate_live_once(request, turn_id).await
    }
    .map(|_| ())
    .map_err(|error| native_failure(error.codex_error()))
}

fn required<T: DeserializeOwned>(fields: &Map<String, Value>, key: &str) -> BridgeResult<T> {
    let value = fields
        .get(key)
        .ok_or_else(|| failure(400, &format!("{key} is required")))?;
    serde_json::from_value(value.clone())
        .map_err(|error| failure(400, &format!("Invalid {key}: {error}")))
}

fn optional<T: DeserializeOwned>(
    fields: &Map<String, Value>,
    key: &str,
) -> BridgeResult<Option<T>> {
    fields
        .get(key)
        .map(|value| {
            serde_json::from_value::<Option<T>>(value.clone())
                .map_err(|error| failure(400, &format!("Invalid {key}: {error}")))
        })
        .transpose()
        .map(Option::flatten)
}
