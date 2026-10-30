//! Shared retry and transport fallback decisions for Responses requests.
//! Content-filter guidance is recorded for sampling requests before retry decisions.
//! Server advice controls timing without extending configured retry limits.

use std::future::Future;
use std::ops::ControlFlow;
use std::time::Duration;

use codex_model_provider_info::ModelProviderInfo;
use codex_otel::SessionTelemetry;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::protocol::SessionSource;

use crate::client::ModelClientSession;
use crate::context::ContentFilterGuidance;
use crate::context::ContextualUserFragment;
use crate::session::session::Session;
use crate::session::step_context::StepContext;
use crate::session::turn_context::TurnContext;
use codex_client::RetryOperation;
use codex_features::Feature;
use codex_http_client::RetryAfter;
use codex_protocol::error::CodexErr;
use codex_protocol::error::CodexErrorDetails;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::WarningEvent;
use tokio::time::Instant;
use tracing::warn;

const INITIAL_CONNECTION_RETRY_DELAY: Duration = Duration::from_secs(5);
const MAX_CONNECTION_RETRY_DELAY: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy)]
pub(crate) enum ResponsesStreamRequest {
    Sampling,
    RemoteCompactionV2,
}

pub(crate) struct ResponsesStreamRetryState {
    retries: u64,
    connection_retries: u64,
    connection_retry_delay: Duration,
}

impl Default for ResponsesStreamRetryState {
    fn default() -> Self {
        Self {
            retries: 0,
            connection_retries: 0,
            connection_retry_delay: INITIAL_CONNECTION_RETRY_DELAY,
        }
    }
}

/// Native model-attempt execution, independent of agent tools and presentation.
/// The existing agent caller supplies its history/event side effects; model-only
/// callers supply data history. Both use the same recovery decisions below.
pub(crate) trait ResponsesStreamOperation: Send {
    type Output: Send;

    fn attempt(
        &mut self,
        client: &mut ModelClientSession,
    ) -> impl Future<Output = Result<Self::Output, CodexErr>> + Send;

    fn recover(
        &mut self,
        state: &mut ResponsesStreamRetryState,
        error: CodexErr,
        client: &mut ModelClientSession,
    ) -> impl Future<Output = Result<ControlFlow<Self::Output>, CodexErr>> + Send;
}

pub(crate) async fn execute_response_attempts<T: ResponsesStreamOperation>(
    client: &mut ModelClientSession,
    operation: &mut T,
) -> Result<T::Output, CodexErr> {
    let mut state = ResponsesStreamRetryState::default();
    loop {
        match operation.attempt(client).await {
            Ok(output) => return Ok(output),
            Err(error) => match operation.recover(&mut state, error, client).await? {
                ControlFlow::Break(output) => return Ok(output),
                ControlFlow::Continue(()) => {}
            },
        }
    }
}

pub(crate) fn content_filter_guidance(
    model_info: &ModelInfo,
) -> codex_protocol::models::ResponseItem {
    ContextualUserFragment::into(ContentFilterGuidance {
        text: codex_prompts::ResolvedModelMessages::from_model(model_info)
            .content_filter_guidance()
            .to_string(),
    })
}

/// Server retry advice retained after stream retries are exhausted. The turn ID
/// prevents a reused Guardian session from applying advice from an earlier review.
pub(crate) struct ExhaustedResponseRetry {
    pub(crate) turn_id: String,
    pub(crate) retry_at: Option<tokio::time::Instant>,
}

/// Returns `Ok(())` when the caller should retry the request loop, or the original error when
/// it is terminal or the retry budget is exhausted.
#[tracing::instrument(level = "trace", skip_all, fields(request = ?request))]
pub(crate) async fn handle_response_stream_error(
    retry_state: &mut ResponsesStreamRetryState,
    max_retries: u64,
    err: CodexErr,
    client_session: &mut ModelClientSession,
    sess: &Session,
    step_context: &StepContext,
    request: ResponsesStreamRequest,
) -> Result<(), CodexErr> {
    let turn_context = &step_context.turn;
    if matches!(request, ResponsesStreamRequest::Sampling)
        && matches!(err.details(), CodexErrorDetails::ContentFilter)
    {
        let model_info = &step_context.settings.model_info;
        sess.record_conversation_items(
            turn_context,
            model_info,
            &[content_filter_guidance(model_info)],
        )
        .await;
    }
    let context = ResponsesRetryContext {
        unbounded_connection_retries: turn_context
            .config
            .features
            .enabled(Feature::UnboundedConnectionRetries),
        session_source: &turn_context.session_source,
        provider: turn_context.provider.info(),
        model_info: turn_context.model_info(),
        telemetry: &turn_context.session_telemetry,
        turn_id: &turn_context.sub_id,
    };
    recover_response_stream(
        retry_state,
        max_retries,
        err,
        client_session,
        &context,
        &mut SessionRetryObserver { sess, turn_context },
        request,
    )
    .await
}

/// Configuration and diagnostics for the shared native model recovery path.
/// It contains no caller wire fields and cannot override provider transport settings.
pub(crate) struct ResponsesRetryContext<'a> {
    pub(crate) unbounded_connection_retries: bool,
    pub(crate) session_source: &'a SessionSource,
    pub(crate) provider: &'a ModelProviderInfo,
    pub(crate) model_info: &'a ModelInfo,
    pub(crate) telemetry: &'a SessionTelemetry,
    pub(crate) turn_id: &'a str,
}

pub(crate) trait ResponsesRetryObserver: Send {
    fn notify_stream_error(
        &mut self,
        message: String,
        error: CodexErr,
    ) -> impl Future<Output = ()> + Send;
    fn warning(&mut self, message: String) -> impl Future<Output = ()> + Send;
    fn exhausted(&mut self, retry_at: Option<Instant>);
}

struct SessionRetryObserver<'a> {
    sess: &'a Session,
    turn_context: &'a TurnContext,
}

impl ResponsesRetryObserver for SessionRetryObserver<'_> {
    async fn notify_stream_error(&mut self, message: String, error: CodexErr) {
        self.sess
            .notify_stream_error(self.turn_context, message, error)
            .await;
    }

    async fn warning(&mut self, message: String) {
        self.sess
            .send_event(
                self.turn_context,
                EventMsg::Warning(WarningEvent { message }),
            )
            .await;
    }

    fn exhausted(&mut self, retry_at: Option<Instant>) {
        self.sess
            .services
            .thread_extension_data
            .insert(ExhaustedResponseRetry {
                turn_id: self.turn_context.sub_id.clone(),
                retry_at,
            });
    }
}

/// The original recovery decisions, shared by agent sampling and model-only execution.
/// Observers report lifecycle facts; they do not choose retries or transport fallback.
pub(crate) async fn recover_response_stream(
    retry_state: &mut ResponsesStreamRetryState,
    max_retries: u64,
    err: CodexErr,
    client_session: &mut ModelClientSession,
    context: &ResponsesRetryContext<'_>,
    observer: &mut impl ResponsesRetryObserver,
    request: ResponsesStreamRequest,
) -> Result<(), CodexErr> {
    let operation = match request {
        ResponsesStreamRequest::Sampling => RetryOperation::Sampling,
        ResponsesStreamRequest::RemoteCompactionV2 => RetryOperation::RemoteCompactionV2,
    };
    let retry_count = retry_state.retries.saturating_add(1);
    let Some(delay) = err.retry_delay(retry_count) else {
        return Err(err);
    };
    let retry_after = err.retry_after();

    if context.unbounded_connection_retries
        && matches!(request, ResponsesStreamRequest::Sampling)
        && matches!(err.details(), CodexErrorDetails::ConnectionFailed(_))
        && !context.session_source.is_internal()
        && !context.provider.is_amazon_bedrock()
    {
        let retry_delay = retry_state.connection_retry_delay;
        warn!(
            turn_id = %context.turn_id,
            error = %err,
            ?retry_delay,
            "stream connection failed; waiting to retry"
        );
        observer
            .notify_stream_error("Reconnecting... waiting for network".into(), err)
            .await;
        retry_state.connection_retries = retry_state.connection_retries.saturating_add(1);
        codex_client::record_retry!(retry_state.connection_retries, retry_delay, operation);
        tokio::time::sleep(retry_delay).await;
        retry_state.connection_retry_delay = retry_delay
            .saturating_mul(2)
            .min(MAX_CONNECTION_RETRY_DELAY);
        return Ok(());
    }

    if retry_state.retries >= max_retries
        && client_session.try_switch_fallback_transport(context.telemetry, context.model_info)
    {
        // Changing transport must not bypass the server's retry deadline.
        if let Some(retry_after) = retry_after {
            tokio::time::sleep_until(retry_after.deadline()).await;
        }
        observer
            .warning(format!(
                "Falling back from WebSockets to HTTPS transport. {err:#}"
            ))
            .await;
        retry_state.retries = 0;
        return Ok(());
    }

    if retry_state.retries < max_retries {
        retry_state.retries = retry_count;
        log_native_retry(
            request,
            context.turn_id,
            &err,
            retry_count,
            max_retries,
            delay,
        );

        // In release builds, hide the first websocket retry notification to reduce noisy
        // transient reconnect messages. In debug builds, keep full visibility for diagnosis.
        let report_error = retry_count > 1
            || cfg!(debug_assertions)
            || !client_session.responses_websocket_enabled();
        if report_error {
            // Surface retry information to any UI/front-end so the user understands what is
            // happening instead of staring at a seemingly frozen screen.
            observer
                .notify_stream_error(format!("Reconnecting... {retry_count}/{max_retries}"), err)
                .await;
        }
        // Use one clock sample so local backoff telemetry retains the selected delay.
        let now = Instant::now();
        let retry_at = retry_after.map(RetryAfter::deadline).unwrap_or(now + delay);
        let delay = retry_at.saturating_duration_since(now);
        codex_client::record_retry!(retry_count, delay, operation);
        tokio::time::sleep_until(retry_at).await;
        return Ok(());
    }

    observer.exhausted(retry_after.map(RetryAfter::deadline));
    Err(err)
}

#[cfg(test)]
fn log_retry(
    request: ResponsesStreamRequest,
    turn_context: &TurnContext,
    err: &CodexErr,
    retries: u64,
    max_retries: u64,
    delay: Duration,
) {
    log_native_retry(
        request,
        &turn_context.sub_id,
        err,
        retries,
        max_retries,
        delay,
    );
}

fn log_native_retry(
    request: ResponsesStreamRequest,
    turn_id: &str,
    err: &CodexErr,
    retries: u64,
    max_retries: u64,
    delay: Duration,
) {
    match request {
        ResponsesStreamRequest::Sampling => {
            warn!(
                turn_id = %turn_id,
                retries,
                max_retries,
                sampling_error = %err,
                "stream disconnected - retrying sampling request ({retries}/{max_retries} in {delay:?})...",
            );
        }
        ResponsesStreamRequest::RemoteCompactionV2 => {
            warn!(
                turn_id = %turn_id,
                retries,
                max_retries,
                compact_error = %err,
                "remote compaction v2 stream failed; retrying request after delay"
            );
        }
    }
}

#[cfg(test)]
#[path = "responses_retry_tests.rs"]
mod tests;
