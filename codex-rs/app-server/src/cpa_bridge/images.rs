//! Business DTO adaptation to the existing native Images backend.
use super::BridgeResult;
use super::failure;
use super::native_failure;
use codex_api::ImageEditRequest;
use codex_api::ImageGenerationRequest;
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
        if !(matches!(
            key.as_str(),
            "model" | "prompt" | "background" | "n" | "quality" | "size" | "stream"
        ) || key == "images" && params.operation == "images/edits")
        {
            return Err(failure(
                400,
                &format!("field `{key}` is not supported by the native Images API"),
            ));
        }
    }
    if fields
        .get("stream")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(failure(400, "stream must be boolean"));
    }
    let request = ImageGenerationRequest {
        prompt: required(fields, "prompt")?,
        model: required(fields, "model")?,
        background: optional(fields, "background")?,
        n: optional(fields, "n")?,
        quality: optional(fields, "quality")?,
        size: optional(fields, "size")?,
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
            .edit_once(
                ImageEditRequest {
                    images: required(fields, "images")?,
                    prompt: request.prompt,
                    background: request.background,
                    model: request.model,
                    n: request.n,
                    quality: request.quality,
                    size: request.size,
                },
                turn_id,
            )
            .await
    } else {
        backend.generate_once(request, turn_id).await
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
            serde_json::from_value(value.clone())
                .map_err(|error| failure(400, &format!("Invalid {key}: {error}")))
        })
        .transpose()
}
