//! Model-only native execution with one explicit peer-size transport fallback.
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

/// Delivery observes native execution without a tool loop or general retry policy.
pub trait InferenceObserver: ResponsesObserver + TransportObserver {}

impl InferenceSession {
    #[expect(
        clippy::await_holding_invalid_type,
        reason = "one logical thread leases its native cache while streaming; independent threads remain concurrent"
    )]
    pub async fn execute(
        &self,
        config: &Config,
        model: &ModelInfo,
        request: &Map<String, Value>,
        observer: Arc<dyn InferenceObserver>,
    ) -> Result<()> {
        let request = InferenceRequest::parse(config, request)
            .map_err(|error| CodexErr::InvalidRequest(format!("{error:#}")))?;
        let prewarm_only = request.controls.generate == Some(false);
        let mut configured_client = self.client.clone();
        configured_client.request_controls = Some(Arc::new(request.controls));
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
        let _lease = self.client_gate.lock().await;
        let mut client = configured_client.new_session();
        client.turn_state = self.turn_state.clone();
        let sample = async {
            let mut peer_size_fallback_used = false;
            let mut expected_auth_owner = None;
            loop {
                let stream = client
                    .stream_once(
                        &request.prompt,
                        model,
                        &telemetry,
                        request.effort.clone(),
                        request.summary,
                        request.service_tier.clone(),
                        &self.metadata,
                        &InferenceTraceContext::disabled(),
                        expected_auth_owner.clone(),
                    )
                    .await;
                let failure = match stream {
                    Err(error) => error,
                    Ok(mut stream) => loop {
                        match stream.next().await {
                            Some(Ok(ResponseEvent::Completed { .. })) => return Ok(()),
                            Some(Ok(_)) => {}
                            Some(Err(error)) => break error,
                            None => {
                                break CodexErr::Stream(
                                    "stream closed before response.completed".into(),
                                );
                            }
                        }
                    },
                };
                // One approved exception only: a remote 1009 before any text.
                // Local size checks, authentication failures and ordinary disconnects
                // never reach this branch. The second HTTP failure is terminal.
                if !prewarm_only
                    && !peer_size_fallback_used
                    && failure.is_peer_websocket_size_rejection()
                {
                    // Do not move the resend to another credential source if it changed.
                    expected_auth_owner = client.websocket_session.auth_owner.clone();
                    peer_size_fallback_used = true;
                    client.try_switch_fallback_transport(&telemetry, model);
                    continue;
                }
                return Err(failure);
            }
        };
        observe_transport(observer.clone(), observe_responses(observer, sample)).await
    }
}
