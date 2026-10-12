//! Read-only observations of actual native attempts and accepted model events.
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
use std::sync::Mutex;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Default)]
struct Observed {
    business: Vec<u8>,
    raw_body: Vec<u8>,
    headers: HeaderMap,
    node: Option<String>,
}

pub(super) struct NativeObservation {
    pub(super) request_id: String,
    pub(super) credential_id: String,
    pub(super) sender: UnboundedSender<CpaInferenceUpstreamNotification>,
    observed: Mutex<Observed>,
}

impl NativeObservation {
    pub(super) fn new(
        request_id: String,
        credential_id: String,
        sender: UnboundedSender<CpaInferenceUpstreamNotification>,
    ) -> Self {
        Self {
            request_id,
            credential_id,
            sender,
            observed: Mutex::new(Observed::default()),
        }
    }

    fn notify(&self, notification: CpaInferenceUpstreamNotification) {
        let mut details = serde_json::to_value(&notification).unwrap_or(Value::Null);
        if let Some(details) = details.as_object_mut() {
            details.remove("body");
            details.remove("bodyBase64");
        }
        record(
            &self.credential_id,
            &self.request_id,
            "cpa.request.upstream",
            details,
        );
        let _ = self.sender.send(notification);
    }

    pub(super) fn result(&self, images: bool) -> (HeaderMap, Vec<u8>) {
        let mut state = self
            .observed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let body = if images {
            std::mem::take(&mut state.raw_body)
        } else {
            std::mem::take(&mut state.business)
        };
        (state.headers.clone(), body)
    }
}

impl TransportObserver for NativeObservation {
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
        {
            let mut state = self
                .observed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.raw_body.clear();
            if method != "response.create" {
                state.node = node.clone();
            }
        }
        // Preserve encoded/compressed bytes exactly; do not change native compression for logs.
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
                .observed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.headers = headers.clone();
            state.raw_body.clear();
            state.node = response_oai_lb_node(Some(headers), state.node.as_deref());
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
        self.observed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .raw_body
            .extend_from_slice(bytes);
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
}

impl ResponsesObserver for NativeObservation {
    fn accepted_event(&self, payload: &str) {
        let mut state = self
            .observed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for line in payload.split('\n') {
            state.business.extend_from_slice(b"data: ");
            state.business.extend_from_slice(line.as_bytes());
            state.business.push(b'\n');
        }
        state.business.push(b'\n');
    }
}

impl InferenceObserver for NativeObservation {
    fn start_attempt(&self) {
        let mut state = self
            .observed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.business.clear();
        state.raw_body.clear();
        state.headers.clear();
    }
    fn recovery(&self, message: &str) {
        record(
            &self.credential_id,
            &self.request_id,
            "cpa.request.native_recovery",
            serde_json::json!({"message":message}),
        );
    }
}

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
mod tests {
    use super::*;
    #[test]
    fn native_observation_preserves_actual_node_and_redacts_sensitive_headers() {
        let (sender, mut notifications) = tokio::sync::mpsc::unbounded_channel();
        let observer = NativeObservation::new("r".into(), "credential".into(), sender);
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"host":"chat.gateway.fixture.api.openai.com"}"#);
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer test-secret".parse().unwrap());
        headers.insert(
            "cookie",
            format!("__oailb=e30.{payload}.sig").parse().unwrap(),
        );
        observer.request(
            "http",
            "POST",
            "http://fixture/v1/responses",
            &headers,
            b"{}",
        );
        observer.response("http", 200, &HeaderMap::new());
        let request = serde_json::to_value(notifications.try_recv().unwrap()).unwrap();
        let response = serde_json::to_value(notifications.try_recv().unwrap()).unwrap();
        assert_eq!(request["oaiLbNode"], "fixture");
        assert_eq!(response["oaiLbNode"], "fixture");
        assert_eq!(
            request["headers"]["authorization"],
            serde_json::json!(["[REDACTED]"])
        );
        assert_eq!(
            request["headers"]["cookie"],
            serde_json::json!(["[REDACTED]"])
        );
        assert!(!observer.observes_request("POST", "https://auth.example/oauth/token"));
        assert!(observer.observes_request("POST", "https://api.example/v1/responses"));
        let mut deleted = HeaderMap::new();
        deleted.insert("set-cookie", "__oailb=; Max-Age=0".parse().unwrap());
        observer.response("http", 401, &deleted);
        let response = serde_json::to_value(notifications.try_recv().unwrap()).unwrap();
        assert!(response["oaiLbNode"].is_null());
    }
}
