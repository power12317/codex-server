//! Managed continuation proof. Retains IDs and digests, never instructions/history.
use super::InferenceObserver;
use codex_api::observation::ResponsesObserver;
use serde_json::Value;
use std::sync::Arc;
use std::sync::Mutex;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct InferenceSessionInstructions {
    pub scope: String,
    pub baseline_digest: String,
    pub append: bool,
    pub http_only: bool,
    pub previous_response_id: Option<String>,
}

impl InferenceSessionInstructions {
    pub fn incremental(&self) -> bool {
        self.append || self.previous_response_id.is_some()
    }
}

#[derive(Clone, Copy, Debug, thiserror::Error, PartialEq, Eq)]
pub enum InferenceContinuationError {
    #[error(
        "Previous response is not proven on this WebSocket; resend full input without previous_response_id"
    )]
    PreviousResponseNotFound,
    #[error("The model frame was not sent; send a new full response.create")]
    NotSent,
    #[error(
        "The WebSocket turn has no reliably correlated success; its outcome may be unknown and it was not replayed"
    )]
    Lost,
}

impl InferenceContinuationError {
    pub fn code(self) -> &'static str {
        match self {
            Self::PreviousResponseNotFound => "previous_response_not_found",
            Self::NotSent => "upstream_websocket_not_sent",
            Self::Lost => "upstream_websocket_lost",
        }
    }
    pub fn status(self) -> u16 {
        if self == Self::PreviousResponseNotFound {
            409
        } else {
            502
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InferenceExecutionError {
    #[error(transparent)]
    Native(#[from] codex_protocol::error::CodexErr),
    #[error(transparent)]
    Continuation(#[from] InferenceContinuationError),
}

#[derive(Clone, Debug)]
pub(super) struct ManagedProof {
    pub generation: Uuid,
    pub scope: String,
    pub digest: String,
    pub context: String,
    pub model: String,
    pub response_id: String,
    pub epoch: u64,
}

pub(super) struct ManagedTurn {
    pub control: InferenceSessionInstructions,
    pub prior: Option<ManagedProof>,
    pub context: String,
    pub previous_epoch: u64,
    pub attempted: Arc<std::sync::atomic::AtomicBool>,
    pub error: Option<InferenceContinuationError>,
}

impl ManagedTurn {
    pub fn check_full_prefix(&mut self, input: &[codex_protocol::models::ResponseItem]) {
        use sha2::Digest;
        if self.control.incremental() || self.control.baseline_digest.is_empty() {
            return;
        }
        let prefix = input
            .first()
            .and_then(|item| serde_json::to_value(item).ok());
        let text = prefix
            .as_ref()
            .filter(|item| item["role"] == "developer")
            .and_then(|item| item["content"].as_array())
            .filter(|parts| parts.len() == 1 && parts[0]["type"] == "input_text")
            .and_then(|parts| parts[0]["text"].as_str());
        if text.is_none_or(|text| {
            format!("{:x}", sha2::Sha256::digest(text.as_bytes())) != self.control.baseline_digest
        }) {
            self.control.baseline_digest.clear();
        }
    }

    pub fn parent(
        &self,
        generation: Option<Uuid>,
        model: &str,
    ) -> Result<String, InferenceContinuationError> {
        let p = self
            .prior
            .as_ref()
            .ok_or(InferenceContinuationError::PreviousResponseNotFound)?;
        if generation != Some(p.generation)
            || self.control.scope != p.scope
            || self.control.baseline_digest.is_empty()
            || self.control.baseline_digest != p.digest
            || self.context != p.context
            || model != p.model
            || p.epoch != self.previous_epoch
            || self
                .control
                .previous_response_id
                .as_ref()
                .is_some_and(|id| id != &p.response_id)
        {
            return Err(InferenceContinuationError::PreviousResponseNotFound);
        }
        Ok(p.response_id.clone())
    }
}

#[derive(Default)]
pub(super) struct Receipt {
    pub parent: Option<String>,
    created: Option<String>,
    completed: Option<String>,
    invalid: bool,
}

impl Receipt {
    pub fn new(parent: Option<String>) -> Self {
        Self {
            parent,
            ..Default::default()
        }
    }
    fn matches_parent(&self, response: &Value) -> bool {
        match response.get("previous_response_id") {
            None => true,
            Some(Value::Null) => self.parent.is_none(),
            Some(Value::String(value)) => self.parent.as_ref() == Some(value),
            Some(_) => false,
        }
    }

    pub fn observe(&mut self, payload: &str) {
        let Ok(value) = serde_json::from_str::<Value>(payload) else {
            self.invalid = true;
            return;
        };
        let kind = value["type"].as_str().unwrap_or_default();
        if !matches!(
            kind,
            "response.created" | "response.completed" | "response.done"
        ) {
            return;
        }
        let response = &value["response"];
        let Some(id) = response["id"]
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 256)
        else {
            self.invalid = true;
            return;
        };
        if !self.matches_parent(response) {
            self.invalid = true;
            return;
        }
        match kind {
            "response.created" => {
                if self.created.is_some() || self.completed.is_some() {
                    self.invalid = true;
                } else {
                    self.created = Some(id.into());
                }
            }
            _ => {
                if self.created.as_deref() != Some(id)
                    || self.completed.is_some()
                    || response
                        .get("status")
                        .is_some_and(|status| status.as_str() != Some("completed"))
                {
                    self.invalid = true;
                } else {
                    self.completed = Some(id.into());
                }
            }
        }
    }

    pub fn success(&self) -> Option<String> {
        (!self.invalid).then(|| self.completed.clone()).flatten()
    }
}

pub(super) struct ManagedEvents {
    pub observer: Arc<dyn InferenceObserver>,
    pub receipt: Arc<Mutex<Receipt>>,
}

impl ResponsesObserver for ManagedEvents {
    fn accepted_event(&self, payload: &str) {
        self.receipt
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .observe(payload);
        self.observer.accepted_event(payload);
    }
    fn failed_event(&self, payload: &str, error: &codex_api::ApiError) {
        self.receipt
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .invalid = true;
        self.observer.failed_event(payload, error);
    }
    fn accepted_image(&self, bytes: &[u8]) {
        self.observer.accepted_image(bytes);
    }
}

#[cfg(test)]
#[path = "inference_continuation_tests.rs"]
mod tests;

/// Shares the transport's actual send witness with cancellation and error handling.
pub(super) struct ManagedTransport {
    pub observer: Arc<dyn InferenceObserver>,
    pub attempted: Arc<std::sync::atomic::AtomicBool>,
}
impl codex_http_client::observation::TransportObserver for ManagedTransport {
    fn websocket_send_attempted(&self) {
        self.attempted
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.observer.websocket_send_attempted();
    }
    fn requires_correlated_websocket_recovery(&self) -> bool {
        true
    }
    fn suppress_request_body(&self) -> bool {
        self.observer.suppress_request_body()
    }
    fn websocket_connection_node(&self) -> Option<String> {
        self.observer.websocket_connection_node()
    }
    fn websocket_request(&self, url: &str, body: &[u8], node: Option<&str>) {
        self.observer.websocket_request(url, body, node);
    }
    fn observes_request(&self, method: &str, url: &str) -> bool {
        self.observer.observes_request(method, url)
    }
    fn http_error(&self, status: u16, headers: &http::HeaderMap, body: Option<&str>) {
        self.observer.http_error(status, headers, body);
    }
    fn request(
        &self,
        transport: &str,
        method: &str,
        url: &str,
        headers: &http::HeaderMap,
        body: &[u8],
    ) {
        self.observer.request(transport, method, url, headers, body);
    }
    fn response(&self, transport: &str, status: u16, headers: &http::HeaderMap) {
        self.observer.response(transport, status, headers);
    }
    fn body(&self, transport: &str, bytes: &[u8]) {
        self.observer.body(transport, bytes);
    }
    fn error(&self, transport: &str, message: &str) {
        self.observer.error(transport, message);
    }
}
