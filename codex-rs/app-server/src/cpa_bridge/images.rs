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
use serde_json::Value;

pub(super) async fn execute(
    provider: SharedModelProvider,
    config: &Config,
    params: &CpaInferenceStartParams,
    session: &InferenceSession,
) -> BridgeResult<()> {
    let fields = &params.request;
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

    let backend = CodexImagesBackend::new(
        provider,
        config.http_client_factory(),
        Some(codex_login::default_client::originator().value),
    );
    let mapping = session.mapping();
    let turn_metadata = session.turn_metadata_json();
    let turn_id = mapping
        .turn_id
        .as_deref()
        .ok_or_else(|| failure(400, "image turn id is required"))?;
    if params.operation == "images/edits" {
        let mut request: ImageEditRequest =
            serde_json::from_value(Value::Object(fields.clone()))
                .map_err(|error| failure(400, &format!("Invalid native image edit: {error}")))?;
        request.options.retain_caller_business_options();
        backend
            .edit_live_once(request, turn_id, turn_metadata.as_deref())
            .await
    } else {
        let mut request: ImageGenerationRequest =
            serde_json::from_value(Value::Object(fields.clone()))
                .map_err(|error| failure(400, &format!("Invalid native image request: {error}")))?;
        request.options.retain_caller_business_options();
        backend
            .generate_live_once(request, turn_id, turn_metadata.as_deref())
            .await
    }
    .map(|_| ())
    .map_err(|error| native_failure(error.codex_error()))
}
