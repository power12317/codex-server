//! Model-only native execution with one explicit peer-size transport fallback.
//! No tool dispatcher, session task or persisted conversation is constructed here.

use super::InferenceSession;
use super::SamplingOptions;
use super::inference::InferenceRequest;
use super::inference_continuation::*;
use crate::config::Config;
use codex_api::ResponseEvent;
use codex_api::observation::ResponsesObserver;
use codex_api::observation::observe_responses;
use codex_http_client::observation::TransportObserver;
use codex_http_client::observation::observe_transport;
use codex_otel::SessionTelemetry;
use codex_protocol::ThreadId;
use codex_protocol::error::CodexErr;
use std::sync::Mutex;
use std::sync::atomic::Ordering;

use codex_protocol::openai_models::ModelInfo;
use codex_rollout_trace::InferenceTraceContext;
use futures::StreamExt;
use serde_json::Map;
use serde_json::Value;
use std::sync::Arc;

/// Delivery observes native execution without a tool loop or general retry policy.
pub trait InferenceObserver: ResponsesObserver + TransportObserver {}

impl InferenceSession {
    /// Existing ordinary entry point; managed controls use the explicit method below.
    pub async fn execute(
        &self,
        config: &Config,
        model: &ModelInfo,
        request: &Map<String, Value>,
        observer: Arc<dyn InferenceObserver>,
    ) -> std::result::Result<(), CodexErr> {
        self.execute_with_session_instructions(config, model, request, observer, None)
            .await
            .map_err(|error| match error {
                InferenceExecutionError::Native(error) => error,
                InferenceExecutionError::Continuation(error) => {
                    CodexErr::InvalidRequest(error.to_string())
                }
            })
    }
    #[expect(
        clippy::await_holding_invalid_type,
        reason = "one logical thread leases its native cache while streaming; independent threads remain concurrent"
    )]
    pub async fn execute_with_session_instructions(
        &self,
        config: &Config,
        model: &ModelInfo,
        request: &Map<String, Value>,
        observer: Arc<dyn InferenceObserver>,
        control: Option<InferenceSessionInstructions>,
    ) -> std::result::Result<(), InferenceExecutionError> {
        let previous_epoch = self.continuation_epoch.fetch_add(1, Ordering::SeqCst);
        let epoch = previous_epoch.wrapping_add(1);
        let incremental = control
            .as_ref()
            .is_some_and(InferenceSessionInstructions::incremental);
        let _lease = if control.is_some() {
            self.client_gate
                .try_lock()
                .map_err(|_| InferenceContinuationError::PreviousResponseNotFound)?
        } else {
            self.client_gate.lock().await
        };
        let mut business = request.clone();
        if control.is_some() {
            business.remove("previous_response_id");
            business.remove("type");
        }
        let request = InferenceRequest::parse(config, &business).map_err(|error| {
            CodexErr::InvalidRequest(if control.is_some() {
                "Invalid managed inference request".into()
            } else {
                format!("{error:#}")
            })
        })?;
        let prewarm_only = request.controls.generate == Some(false);
        let prompt = Arc::new(request.prompt);
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
        let mut client = configured_client.new_session();
        let prior = client.websocket_session.managed_proof.take();
        let context = serde_json::to_string(&(
            &self.metadata.session_id,
            &self.metadata.thread_id,
            &self.metadata.window_id,
            &self.metadata.context_window_id,
        ))
        .map_err(|_| CodexErr::InvalidRequest("Invalid managed context".into()))?;
        let mut parent = None;
        let attempted = Arc::new(std::sync::atomic::AtomicBool::new(false));
        if let Some(control) = &control {
            client.managed_inference = Some(ManagedTurn {
                control: control.clone(),
                prior,
                context: context.clone(),
                previous_epoch,
                attempted: attempted.clone(),
                error: None,
            });
            if incremental {
                if control.http_only || !client.responses_websocket_enabled() || prewarm_only {
                    return Err(InferenceContinuationError::PreviousResponseNotFound.into());
                }
                parent = Some(
                    client
                        .managed_inference
                        .as_ref()
                        .expect("managed turn")
                        .parent(client.websocket_session.generation, &model.slug)?,
                );
            }
        }
        let receipt = Arc::new(Mutex::new(Receipt::new(parent)));
        let event_observer: Arc<dyn ResponsesObserver> = if control.is_some() {
            Arc::new(ManagedEvents {
                observer: observer.clone(),
                receipt: receipt.clone(),
            })
        } else {
            observer.clone()
        };
        client.turn_state = self.turn_state.clone();
        let transport_observer: Arc<dyn TransportObserver> = if control.is_some() {
            Arc::new(ManagedTransport {
                observer: observer.clone(),
                attempted,
            })
        } else {
            observer.clone()
        };
        let sample = async {
            let mut peer_size_fallback_used = false;
            let mut expected_auth_owner = None;
            loop {
                let stream = client
                    .stream_once(
                        &prompt,
                        model,
                        &telemetry,
                        SamplingOptions {
                            reasoning_effort: request.effort.clone(),
                            reasoning_summary: request.summary,
                            service_tier: request.service_tier.clone(),
                        },
                        &self.metadata,
                        &InferenceTraceContext::disabled(),
                        expected_auth_owner.clone(),
                    )
                    .await;
                let failure = match stream {
                    Err(error) => error,
                    Ok(mut stream) => loop {
                        match stream.next().await {
                            Some(Ok(ResponseEvent::Completed { .. })) => {
                                if incremental
                                    && receipt
                                        .lock()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                                        .success()
                                        .is_none()
                                {
                                    return Err(InferenceContinuationError::Lost.into());
                                }
                                return Ok(());
                            }
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
                if incremental {
                    let turn = client.managed_inference.as_ref().expect("managed turn");
                    return Err(turn
                        .error
                        .unwrap_or(if turn.attempted.load(Ordering::SeqCst) {
                            InferenceContinuationError::Lost
                        } else {
                            InferenceContinuationError::NotSent
                        })
                        .into());
                }
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
                return Err(failure.into());
            }
        };
        let result = observe_transport(
            transport_observer,
            observe_responses(event_observer, sample),
        )
        .await;
        if control.is_some() {
            // Managed deltas are supplied by CPA, never derived from stored history.
            client.websocket_session.last_request = None;
            client.websocket_session.last_response_rx = None;
        }
        if result.is_ok() {
            if let Some(turn) = &client.managed_inference {
                if !prewarm_only
                    && !turn.control.http_only
                    && turn.attempted.load(Ordering::SeqCst)
                    && !turn.control.baseline_digest.is_empty()
                    && self.continuation_epoch.load(Ordering::SeqCst) == epoch
                    && client
                        .websocket_session
                        .auth_owner
                        .as_ref()
                        .is_some_and(|owner| owner.is_current())
                {
                    if let (Some(generation), Some(response_id)) = (
                        client.websocket_session.generation,
                        receipt
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .success(),
                    ) {
                        client.websocket_session.managed_proof = Some(ManagedProof {
                            generation,
                            response_id,
                            scope: turn.control.scope.clone(),
                            digest: turn.control.baseline_digest.clone(),
                            context,
                            model: model.slug.clone(),
                            epoch,
                        });
                    }
                }
            }
            client.managed_inference = None;
        }
        result
    }
}
