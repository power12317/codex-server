//! Model-only single-send entry into the native request and streaming machinery.
//! No tool dispatcher, session task or persisted conversation is constructed here.

use super::InferenceSession;
use super::inference::InferenceRequest;
use crate::config::Config;
use codex_api::ResponseEvent;
use codex_api::observation::ResponsesObserver;
use codex_api::observation::observe_responses;
use codex_http_client::observation::TransportObserver;
use codex_http_client::observation::observe_transport;
use codex_otel::SessionTelemetry;
use codex_protocol::ThreadId;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result;
use codex_protocol::openai_models::ModelInfo;
use codex_rollout_trace::InferenceTraceContext;
use futures::StreamExt;
use serde_json::Map;
use serde_json::Value;
use std::sync::Arc;

/// Delivery observes native execution without introducing a local retry or tool loop.
pub trait InferenceObserver: ResponsesObserver + TransportObserver {}

impl InferenceSession {
    #[expect(
        clippy::await_holding_invalid_type,
        reason = "a turn exclusively owns its mutable native client while streaming; independent turn contexts use separate locks"
    )]
    pub async fn execute(
        &self,
        config: &Config,
        model: &ModelInfo,
        request: &Map<String, Value>,
        observer: Arc<dyn InferenceObserver>,
    ) -> Result<()> {
        let caller = request;
        let request = InferenceRequest::parse(config, request)
            .map_err(|error| CodexErr::InvalidRequest(format!("{error:#}")))?;
        // Select supported native client configuration, then validate derived controls
        // with the same builder read-only. Never modify the serialized native DTO.
        let configured_client = self.client.clone().with_model_verbosity(request.verbosity);
        let native = configured_client.build_responses_request(
            &request.prompt,
            model,
            request.effort.clone(),
            request.summary,
            request.service_tier.clone(),
            &self.metadata,
            false,
        )?;
        let native = serde_json::to_value(native)?;
        for (object, key) in [("reasoning", "context"), ("text", "verbosity")] {
            if let Some(value) = caller.get(object).and_then(|object| object.get(key))
                && native[object][key] != *value
            {
                return Err(CodexErr::InvalidRequest(format!(
                    "{object}.{key} does not match native model configuration"
                )));
            }
        }
        if let Some(value) = caller.get("client_metadata").and_then(|metadata| {
            metadata.get("ws_request_header_x_openai_internal_codex_responses_lite")
        }) {
            let expected = if model.use_responses_lite {
                "true"
            } else {
                "false"
            };
            if value.as_str() != Some(expected) {
                return Err(CodexErr::InvalidRequest(
                    "caller Lite metadata does not match native model configuration".into(),
                ));
            }
        }
        let telemetry = SessionTelemetry::new(
            ThreadId::from_string(&self.metadata.thread_id)
                .map_err(|error| CodexErr::InvalidRequest(error.to_string()))?,
            &model.slug,
            &model.slug,
            None,
            None,
            None,
            self.client.state.originator.clone(),
            false,
            "inference-only".into(),
            self.client.state.session_source.clone(),
        );
        let mut client = self.session.lock().await;
        client.client = configured_client;
        let sample = async {
            let mut stream = client
                .stream_once(
                    &request.prompt,
                    model,
                    &telemetry,
                    request.effort,
                    request.summary,
                    request.service_tier,
                    &self.metadata,
                    &InferenceTraceContext::disabled(),
                )
                .await?;
            while let Some(event) = stream.next().await {
                if matches!(event?, ResponseEvent::Completed { .. }) {
                    return Ok(());
                }
            }
            Err(CodexErr::Stream(
                "stream closed before response.completed".into(),
            ))
        };
        observe_transport(observer.clone(), observe_responses(observer, sample)).await
    }
}
