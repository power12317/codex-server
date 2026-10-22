//! Lossless observation beside the native Responses parser, without a second parser.

use crate::ResponseEvent;
use crate::sse::responses::ResponsesEventError;
use codex_http_client::observation::TransportObserver;
use codex_http_client::observation::observe_transport;
use codex_http_client::observation::transport_observer;
use std::future::Future;
use std::sync::Arc;

pub trait ResponsesObserver: Send + Sync {
    /// An original payload rejected by the native parser, with its native classification.
    fn failed_event(&self, _payload: &str, _error: &crate::ApiError) {}
    /// The parser has accepted this event. Completion remains a native typed event;
    /// observing bytes alone never authorizes a caller to report model success.
    fn accepted_event(&self, payload: &str);
    /// Image bytes accepted by the native Images parser, independent of diagnostics.
    fn accepted_image(&self, _bytes: &[u8]) {}
}

tokio::task_local! {
    static OBSERVER: Arc<dyn ResponsesObserver>;
}

pub async fn observe_responses<F: Future>(
    observer: Arc<dyn ResponsesObserver>,
    future: F,
) -> F::Output {
    OBSERVER.scope(observer, future).await
}

pub(crate) struct ObservationContext {
    responses: Option<Arc<dyn ResponsesObserver>>,
    transport: Option<Arc<dyn TransportObserver>>,
}

impl ObservationContext {
    pub(crate) fn capture() -> Self {
        Self {
            responses: OBSERVER.try_with(Arc::clone).ok(),
            transport: transport_observer(),
        }
    }

    pub(crate) async fn scope<F: Future>(self, future: F) -> F::Output {
        let future = async {
            match self.responses {
                Some(observer) => observe_responses(observer, future).await,
                None => future.await,
            }
        };
        match self.transport {
            Some(observer) => observe_transport(observer, future).await,
            None => future.await,
        }
    }
}

pub(crate) fn observe_processed_event(
    kind: &str,
    payload: &str,
    processed: &Result<Option<ResponseEvent>, ResponsesEventError>,
) {
    if let Err(ResponsesEventError::Api(error)) = processed {
        observe_failed_event(payload, error);
    }
    // Internal status and rejected events belong to diagnostic output. Unknown valid
    // extension events, including tool/image deltas, retain their original payload.
    if processed.is_ok()
        && kind != "error"
        && kind != "response.failed"
        && kind != "response.metadata"
        && !kind.starts_with("codex.")
    {
        let _ = OBSERVER.try_with(|observer| observer.accepted_event(payload));
    }
}

pub(crate) fn observe_failed_event(payload: &str, error: &crate::ApiError) {
    let _ = OBSERVER.try_with(|observer| observer.failed_event(payload, error));
}

pub(crate) fn observe_image(bytes: &[u8]) {
    let _ = OBSERVER.try_with(|observer| observer.accepted_image(bytes));
}
