//! Independent native business and diagnostic observations for one model send.
use super::BridgeResult;
use axum::http::HeaderMap;
use base64::Engine;
use codex_api::observation::ResponsesObserver;
use codex_app_server_protocol::CpaInferenceUpstreamNotification;
use codex_core::InferenceObserver;
use codex_http_client::observation::TransportObserver;
use serde_json::Value;
use sha2::Digest;
use sha2::Sha256;
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::mpsc::UnboundedSender;

pub(super) enum Business {
    Ready(HeaderMap),
    Event(Vec<u8>),
    Finished(BridgeResult<()>),
}

pub(super) enum Diagnostic {
    Notification(CpaInferenceUpstreamNotification),
    Record { event: &'static str, details: Value },
}

#[derive(Default)]
struct NativeErrorResponse {
    status: Option<u16>,
    headers: HeaderMap,
    body: Option<String>,
}

#[derive(Default)]
struct Observed {
    headers: HeaderMap,
    node: Option<String>,
    ready: bool,
    closed: bool,
    // Nonterminal events, including tool-ready events, are never held here.
    terminal_events: Vec<Vec<u8>>,
    error: Option<NativeErrorResponse>,
}

pub(super) struct NativeObservation {
    request_id: String,
    diagnostics: UnboundedSender<Diagnostic>,
    business: UnboundedSender<Business>,
    images: bool,
    state: Mutex<Observed>,
    analytics: std::sync::OnceLock<super::analytics::Exchange>,
}

impl NativeObservation {
    pub(super) fn new(
        request_id: String,
        diagnostics: UnboundedSender<Diagnostic>,
        business: UnboundedSender<Business>,
        images: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            request_id,
            diagnostics,
            business,
            images,
            state: Mutex::new(Observed::default()),
            analytics: std::sync::OnceLock::new(),
        })
    }

    pub(super) fn set_analytics(&self, analytics: super::analytics::Exchange) {
        let _ = self.analytics.set(analytics);
    }

    pub(super) fn native_error(&self, error: &codex_protocol::error::CodexErr) {
        if let Some(analytics) = self.analytics.get() {
            analytics.native_error(error);
        }
    }

    pub(super) fn record(&self, event: &'static str, details: Value) {
        let _ = self.diagnostics.send(Diagnostic::Record { event, details });
    }

    pub(super) fn finish(&self, mut result: BridgeResult<()>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.closed = true;
        if let Err(error) = &mut result {
            if let (Some(original), Some(data)) = (&state.error, error.data.as_mut()) {
                if let Some(status) = original.status {
                    data["httpStatus"] = serde_json::json!(status);
                }
                if let Some(body) = &original.body {
                    data["body"] = serde_json::json!(body);
                }
                if !original.headers.is_empty() {
                    data["headers"] = serde_json::json!(public_error_headers(&original.headers));
                }
            }
        } else {
            for bytes in std::mem::take(&mut state.terminal_events) {
                let _ = self.business.send(Business::Event(bytes));
            }
        }
        if let Some(analytics) = self.analytics.get() {
            analytics.finish(&result);
        }
        let _ = self.business.send(Business::Finished(result));
    }

    fn ready(&self, state: &mut Observed) {
        if !state.ready {
            state.ready = true;
            let _ = self.business.send(Business::Ready(state.headers.clone()));
        }
    }

    fn notify(&self, notification: CpaInferenceUpstreamNotification) {
        let _ = self
            .diagnostics
            .send(Diagnostic::Notification(notification));
    }
}

fn public_error_headers(headers: &HeaderMap) -> BTreeMap<String, Vec<String>> {
    headers
        .keys()
        .map(|name| {
            (
                name.to_string(),
                headers
                    .get_all(name)
                    .iter()
                    .filter_map(|value| value.to_str().ok().map(str::to_owned))
                    .collect(),
            )
        })
        .collect()
}

impl TransportObserver for NativeObservation {
    fn websocket_connection_node(&self) -> Option<String> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .node
            .clone()
    }
    fn websocket_request(&self, url: &str, body: &[u8], node: Option<&str>) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .node = node.map(str::to_owned);
        self.request("websocket", "response.create", url, &HeaderMap::new(), body);
    }
    fn observes_request(&self, _method: &str, url: &str) -> bool {
        let Ok(url) = url.parse::<url::Url>() else {
            return false;
        };
        [
            "/responses",
            "/responses/lite",
            "/images/generations",
            "/images/edits",
        ]
        .iter()
        .any(|path| url.path().ends_with(path))
    }
    fn request(&self, transport: &str, method: &str, url: &str, headers: &HeaderMap, body: &[u8]) {
        if let Some(analytics) = self.analytics.get() {
            analytics.request(transport, method, url, headers, body);
        }
        let access_token_sha256 = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(|value| format!("{:x}", Sha256::digest(value.as_bytes())));
        let node = headers
            .get_all(axum::http::header::COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(';'))
            .find_map(oai_lb_cookie)
            .and_then(oai_lb_node);
        let node = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if method != "response.create" {
                state.node = node;
            }
            state.node.clone()
        };
        self.notify(CpaInferenceUpstreamNotification::Request {
            request_id: self.request_id.clone(),
            transport: transport.into(),
            url: url.into(),
            method: method.into(),
            headers: log_headers(headers),
            body: std::str::from_utf8(body)
                .unwrap_or("[encoded native request body]")
                .into(),
            body_base64: Some(base64::engine::general_purpose::STANDARD.encode(body)),
            access_token_sha256,
            oai_lb_node: node,
        });
    }
    fn response(&self, transport: &str, status: u16, headers: &HeaderMap) {
        let node = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.headers = headers.clone();
            state.node = response_oai_lb_node(Some(headers), state.node.as_deref());
            if self.images && (200..300).contains(&status) && !state.closed {
                self.ready(&mut state);
            }
            state.node.clone()
        };
        self.notify(CpaInferenceUpstreamNotification::Response {
            request_id: self.request_id.clone(),
            transport: transport.into(),
            status_code: status,
            headers: log_headers(headers),
            body: None,
            oai_lb_node: node,
        });
    }
    fn body(&self, transport: &str, bytes: &[u8]) {
        self.notify(CpaInferenceUpstreamNotification::Body {
            request_id: self.request_id.clone(),
            transport: transport.into(),
            body_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        });
    }
    fn error(&self, transport: &str, message: &str) {
        self.notify(CpaInferenceUpstreamNotification::Error {
            request_id: self.request_id.clone(),
            transport: transport.into(),
            message: message.into(),
        });
    }
    fn http_error(&self, status: u16, headers: &HeaderMap, body: Option<&str>) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .error = Some(NativeErrorResponse {
            status: Some(status),
            headers: headers.clone(),
            body: body.map(str::to_owned),
        });
    }
}

impl ResponsesObserver for NativeObservation {
    fn accepted_image(&self, bytes: &[u8]) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed {
            return;
        }
        self.ready(&mut state);
        let _ = self.business.send(Business::Event(bytes.to_vec()));
    }

    fn accepted_event(&self, payload: &str) {
        if let Some(analytics) = self.analytics.get() {
            analytics.event(payload);
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed {
            return;
        }
        self.ready(&mut state);
        let mut bytes = Vec::with_capacity(payload.len() + 8);
        for line in payload.split('\n') {
            bytes.extend_from_slice(b"data: ");
            bytes.extend_from_slice(line.as_bytes());
            bytes.push(b'\n');
        }
        bytes.push(b'\n');
        let terminal = serde_json::from_str::<Value>(payload).is_ok_and(|value| {
            matches!(
                value["type"].as_str(),
                Some("response.completed" | "response.incomplete" | "response.cancelled")
            )
        });
        if terminal {
            state.terminal_events.push(bytes);
        } else {
            let _ = self.business.send(Business::Event(bytes));
        }
    }
    fn failed_event(&self, payload: &str, error: &codex_api::ApiError) {
        if let Some(analytics) = self.analytics.get() {
            analytics.event(payload);
        }
        let mut original = NativeErrorResponse {
            body: Some(payload.into()),
            ..Default::default()
        };
        if let codex_api::ApiError::Transport(codex_api::TransportError::Http {
            status,
            headers,
            ..
        }) = error
        {
            original.status = Some(status.as_u16());
            original.headers = headers.clone().unwrap_or_default();
        } else if let Ok(value) = serde_json::from_str::<Value>(payload) {
            // Preserve supplied error headers without reclassifying or retrying the native error.
            for path in ["/headers", "/response/error/headers", "/error/headers"] {
                if let Some(headers) = value.pointer(path).and_then(Value::as_object) {
                    for (name, value) in headers {
                        if let (Ok(name), Some(value)) =
                            (name.parse::<axum::http::HeaderName>(), value.as_str())
                            && let Ok(value) = value.parse::<axum::http::HeaderValue>()
                        {
                            original.headers.insert(name, value);
                        }
                    }
                }
            }
        }
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .error = Some(original);
    }
}
impl InferenceObserver for NativeObservation {}

pub(super) fn log_headers(headers: &HeaderMap) -> BTreeMap<String, Vec<String>> {
    headers
        .keys()
        .map(|name| {
            let sensitive = matches!(
                name.as_str(),
                "authorization" | "proxy-authorization" | "cookie" | "set-cookie"
            ) || ["authorization", "api-key", "apikey", "token", "secret"]
                .iter()
                .any(|part| name.as_str().contains(part));
            let values = headers
                .get_all(name)
                .iter()
                .map(|value| {
                    if sensitive {
                        "[REDACTED]".into()
                    } else {
                        String::from_utf8_lossy(value.as_bytes()).into_owned()
                    }
                })
                .collect();
            (name.to_string(), values)
        })
        .collect()
}

fn response_oai_lb_node(headers: Option<&HeaderMap>, request_node: Option<&str>) -> Option<String> {
    let response_cookie = headers.and_then(|headers| {
        headers
            .get_all(axum::http::header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .filter_map(|value| value.split(';').next())
            .find_map(oai_lb_cookie)
    });
    match response_cookie {
        // An explicit deletion or invalid value must not fall back to the request node.
        Some(cookie) => oai_lb_node(cookie),
        None => request_node.map(str::to_owned),
    }
}

fn oai_lb_cookie(part: &str) -> Option<&str> {
    let (name, value) = part.trim().split_once('=')?;
    (name.trim() == "__oailb").then_some(value.trim())
}

fn oai_lb_node(cookie: &str) -> Option<String> {
    let payload = cookie.trim_matches('"').split('.').nth(/*n*/ 1)?;
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let claims: Value = serde_json::from_slice(&payload).ok()?;
    let host = claims["host"]
        .as_str()?
        .trim()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let node = host
        .strip_prefix("chat.gateway.")?
        .strip_suffix(".api.openai.com")?;
    (!node.is_empty()
        && node.len() <= 63
        && !node.starts_with('-')
        && !node.ends_with('-')
        && node
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-'))
    .then(|| node.to_string())
}

/// JSONL to stderr only: stdout is reserved for JSON-RPC. Headers are already redacted.
pub(crate) fn record(credential_id: &str, request_id: &str, event: &str, details: Value) {
    let record = serde_json::json!({"timestamp": chrono::Utc::now().to_rfc3339(),
        "level": "INFO", "target": "codex_app_server::cpa_bridge",
        "fields": {"event.name": event, "credentialId": credential_id,
        "rpcRequestId": request_id, "details": details}});
    let _ = writeln!(std::io::stderr().lock(), "{record}");
}

#[cfg(test)]
#[path = "upstream_tests.rs"]
mod tests;
