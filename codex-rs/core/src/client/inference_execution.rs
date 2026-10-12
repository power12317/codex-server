//! Model-only entry into the native streaming and recovery machinery.
//! No tool dispatcher, session task or persisted conversation is constructed here.

use super::InferenceSession;
use super::ModelClientSession;
use super::inference::InferenceRequest;
use crate::config::Config;
use crate::context_manager::ContextManager;
use crate::responses_retry::ResponsesRetryContext;
use crate::responses_retry::ResponsesRetryObserver;
use crate::responses_retry::ResponsesStreamOperation;
use crate::responses_retry::ResponsesStreamRequest;
use crate::responses_retry::ResponsesStreamRetryState;
use crate::responses_retry::content_filter_guidance;
use crate::responses_retry::execute_response_attempts;
use crate::responses_retry::recover_response_stream;
use codex_api::ResponseEvent;
use codex_api::observation::ResponsesObserver;
use codex_api::observation::observe_responses;
use codex_features::Feature;
use codex_http_client::observation::TransportObserver;
use codex_http_client::observation::observe_transport;
use codex_otel::SessionTelemetry;
use codex_protocol::ThreadId;
use codex_protocol::error::CodexErr;
use codex_protocol::error::CodexErrorDetails;
use codex_protocol::error::Result;
use codex_protocol::openai_models::ModelInfo;
use codex_rollout_trace::InferenceTraceContext;
use futures::StreamExt;
use serde_json::Map;
use serde_json::Value;
use std::ops::ControlFlow;
use std::sync::Arc;

/// Delivery observes native execution; it cannot choose recovery or execute tools.
pub trait InferenceObserver: ResponsesObserver + TransportObserver {
    fn start_attempt(&self);
    fn recovery(&self, message: &str);
}

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
        let mut history = ContextManager::new();
        for mut item in request.prompt.input.iter().cloned() {
            prepare_history_item(
                &mut item,
                self.metadata.turn_id.as_deref().unwrap_or_default(),
            );
            history.record_items([&item], model.truncation_policy.into());
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
        let mut operation = ModelOperation {
            session: self,
            config,
            model,
            request,
            telemetry,
            history,
            observer: observer.clone(),
            retrying: false,
        };
        let mut client = self.session.lock().await;
        client.client = client
            .client
            .clone()
            .with_model_verbosity(operation.request.verbosity);
        observe_transport(
            observer.clone(),
            observe_responses(
                observer,
                execute_response_attempts(&mut client, &mut operation),
            ),
        )
        .await
    }
}

struct ModelOperation<'a> {
    session: &'a InferenceSession,
    config: &'a Config,
    model: &'a ModelInfo,
    request: InferenceRequest,
    telemetry: SessionTelemetry,
    history: ContextManager,
    observer: Arc<dyn InferenceObserver>,
    retrying: bool,
}

impl ResponsesStreamOperation for ModelOperation<'_> {
    type Output = ();

    async fn attempt(&mut self, client: &mut ModelClientSession) -> Result<()> {
        self.observer.start_attempt();
        if self.retrying {
            // Preserve accepted partial items without inventing tool outputs. Agent
            // history normalization may synthesize "aborted" local tool results;
            // a model-only invocation has no authority to supply those results.
            self.request.prompt.input = self
                .history
                .clone()
                .for_inference_prompt(&self.model.input_modalities);
        }
        let mut stream = client
            .stream(
                &self.request.prompt,
                self.model,
                &self.telemetry,
                self.request.effort.clone(),
                self.request.summary,
                self.request.service_tier.clone(),
                &self.session.metadata,
                &InferenceTraceContext::disabled(),
            )
            .await?;
        while let Some(event) = stream.next().await {
            match event? {
                ResponseEvent::OutputItemDone(mut item) => {
                    // Caller tools are not committed/executed until the complete model
                    // response is handed off. A failed attempt must not send an unpaired
                    // tool call on recovery or fabricate its result. The observer retains
                    // the original item for successful delivery; arguments stay opaque.
                    if matches!(
                        item,
                        codex_protocol::models::ResponseItem::FunctionCall { .. }
                            | codex_protocol::models::ResponseItem::CustomToolCall { .. }
                            | codex_protocol::models::ResponseItem::LocalShellCall { .. }
                    ) || matches!(&item, codex_protocol::models::ResponseItem::ToolSearchCall { execution, .. } if execution == "client")
                    {
                        continue;
                    }
                    prepare_history_item(
                        &mut item,
                        self.session.metadata.turn_id.as_deref().unwrap_or_default(),
                    );
                    self.history
                        .record_items([&item], self.model.truncation_policy.into());
                }
                ResponseEvent::Completed { .. } => return Ok(()),
                _ => {}
            }
        }
        Err(CodexErr::Stream(
            "stream closed before response.completed".into(),
        ))
    }

    async fn recover(
        &mut self,
        state: &mut ResponsesStreamRetryState,
        error: CodexErr,
        client: &mut ModelClientSession,
    ) -> Result<ControlFlow<()>> {
        if matches!(error.details(), CodexErrorDetails::ContentFilter) {
            let mut guidance = content_filter_guidance(self.model);
            prepare_history_item(
                &mut guidance,
                self.session.metadata.turn_id.as_deref().unwrap_or_default(),
            );
            self.history
                .record_items([&guidance], self.model.truncation_policy.into());
        }
        let context = ResponsesRetryContext {
            unbounded_connection_retries: self
                .config
                .features
                .enabled(Feature::UnboundedConnectionRetries),
            session_source: &self.session.client.state.session_source,
            provider: self.session.client.state.provider.info(),
            model_info: self.model,
            telemetry: &self.telemetry,
            turn_id: self.session.metadata.turn_id.as_deref().unwrap_or_default(),
        };
        recover_response_stream(
            state,
            context.provider.stream_max_retries(),
            error,
            client,
            &context,
            &mut Observer(self.observer.clone()),
            ResponsesStreamRequest::Sampling,
        )
        .await?;
        self.retrying = true;
        Ok(ControlFlow::Continue(()))
    }
}

struct Observer(Arc<dyn InferenceObserver>);

fn prepare_history_item(item: &mut codex_protocol::models::ResponseItem, turn_id: &str) {
    // Preserve the native content-kind annotation applied at the history boundary.
    // This only annotates provided content; it does not open caller files or run tools.
    if let Some(content) = codex_context_fragments::to_annotated_content(item) {
        let _ = codex_context_fragments::set_annotated_content(item, content);
    }
    crate::session::session::Session::stamp_response_item_for_history(item, turn_id);
    crate::session::session::Session::assign_missing_response_item_id(item);
}

impl ResponsesRetryObserver for Observer {
    async fn notify_stream_error(&mut self, message: String, _error: CodexErr) {
        self.0.recovery(&message);
    }
    async fn warning(&mut self, message: String) {
        self.0.recovery(&message);
    }
    fn exhausted(&mut self, _retry_at: Option<tokio::time::Instant>) {}
}
