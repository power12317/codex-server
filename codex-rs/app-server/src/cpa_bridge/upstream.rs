//! Log the same prepared inference request that is handed to the HTTP transport.
use super::ticket::TicketCapture;
use crate::outgoing_message::ConnectionId;
use crate::outgoing_message::OutgoingMessageSender;
use axum::http::HeaderMap;
use base64::Engine;
use codex_api::ReqwestTransport;
use codex_api::TransportError;
use codex_app_server_protocol::CpaInferenceUpstreamNotification;
use codex_app_server_protocol::ServerNotification;
use codex_http_client::HttpClientFactory;
use codex_http_client::HttpTransport;
use codex_http_client::Request;
use codex_http_client::Response;
use codex_http_client::StreamResponse;
use futures::StreamExt;
use serde_json::Value;
use sha2::Digest;
use sha2::Sha256;
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::Arc;

pub(super) struct UpstreamTransport {
    pub(super) inner: ReqwestTransport,
    pub(super) factory: HttpClientFactory,
    pub(super) outgoing: Arc<OutgoingMessageSender>,
    pub(super) connection_id: ConnectionId,
    pub(super) request_id: String,
    pub(super) credential_id: String,
    pub(super) ticket_model: Option<String>,
}

impl UpstreamTransport {
    async fn notify(
        &self,
        notification: CpaInferenceUpstreamNotification,
    ) -> Result<(), TransportError> {
        // stderr is inherited by the master and remains visible with RUST_LOG unset.
        let mut details = serde_json::to_value(&notification).unwrap_or(Value::Null);
        if let Some(details) = details.as_object_mut() {
            // Full payloads remain in the existing upstream notification. Expose the
            // upstream structured failure so docker logs identifies invalid input too.
            if let Some(error) = details
                .get("body")
                .and_then(Value::as_str)
                .and_then(|body| serde_json::from_str::<Value>(body).ok())
                .and_then(|body| body.get("error").cloned())
            {
                details.insert(
                    "error".into(),
                    serde_json::json!({
                        "code": error["code"], "type": error["type"], "message": error["message"],
                    }),
                );
            }
            details.remove("body");
            if let Some(headers) = details.get_mut("headers").and_then(Value::as_object_mut) {
                for (name, values) in headers {
                    if sensitive_log_header(name) {
                        *values = serde_json::json!(["[REDACTED]"]);
                    }
                }
            }
        }
        let event = if details["kind"] == "response" {
            "cpa.request.upstream.response"
        } else {
            "cpa.request.upstream"
        };
        record(&self.credential_id, &self.request_id, event, details);
        if self
            .outgoing
            .send_server_notification_to_connection_and_wait(
                self.connection_id,
                ServerNotification::CpaInferenceUpstream(notification),
            )
            .await
        {
            Ok(())
        } else {
            Err(TransportError::Network("CPA connection closed".into()))
        }
    }
}

impl HttpTransport for UpstreamTransport {
    async fn execute(&self, req: Request) -> Result<Response, TransportError> {
        self.inner.execute(req).await
    }

    async fn stream(&self, mut req: Request) -> Result<StreamResponse, TransportError> {
        // Defaults and the shared cookie jar are applied to the actual request,
        // rather than reconstructed separately for the log after dispatch.
        for (name, value) in &codex_login::default_client::default_headers() {
            req.headers.entry(name.clone()).or_insert(value.clone());
        }
        if !req.headers.contains_key(axum::http::header::COOKIE)
            && let Ok(uri) = req.url.parse()
            && let Some(cookie) = self.factory.chatgpt_cookie_header(&uri)
        {
            req.headers.insert(axum::http::header::COOKIE, cookie);
        }
        let prepared = req.prepare_body_for_send().map_err(TransportError::Build)?;
        let access_token_sha256 = req
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(|token| format!("{:x}", Sha256::digest(token.as_bytes())));
        // Keep the node from this exact outbound request, not a later cookie-jar read.
        let request_node = prepared
            .headers
            .get_all(axum::http::header::COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(';'))
            .find_map(oai_lb_cookie)
            .and_then(oai_lb_node);
        self.notify(CpaInferenceUpstreamNotification::Request {
            request_id: self.request_id.clone(),
            url: req.url.clone(),
            method: req.method.to_string(),
            headers: wire_headers(&prepared.headers),
            body: String::from_utf8_lossy(&prepared.body_bytes()).into_owned(),
            access_token_sha256,
            oai_lb_node: request_node.clone(),
        })
        .await?;
        let mut capture = self.ticket_model.as_ref().map(|model| {
            TicketCapture::new(
                self.request_id.clone(),
                self.credential_id.clone(),
                model.clone(),
                &prepared.headers,
            )
        });
        if let Some(capture) = &mut capture {
            capture.preserve_known_deadlines(&self.factory);
        }
        let result = self.inner.stream(req).await;
        let response_headers = match &result {
            Ok(response) => Some(&response.headers),
            Err(TransportError::Http { headers, .. }) => headers.as_ref(),
            _ => None,
        };
        if let (Some(capture), Some(headers)) = (&mut capture, response_headers) {
            capture.response_headers(headers, chrono::Utc::now());
            capture.preserve_known_deadlines(&self.factory);
            if let Some(snapshot) = capture.snapshot(chrono::Utc::now()) {
                // Dedicated private channel: never pass routing material through notify/record.
                self.outgoing
                    .send_server_notification_to_connection_and_wait(
                        self.connection_id,
                        ServerNotification::CpaInferenceTicket(snapshot),
                    )
                    .await;
            }
        }
        let notification = match &result {
            Ok(response) => CpaInferenceUpstreamNotification::Response {
                request_id: self.request_id.clone(),
                status_code: response.status.as_u16(),
                headers: wire_headers(&response.headers),
                body: None,
                oai_lb_node: response_oai_lb_node(Some(&response.headers), request_node.as_deref()),
            },
            Err(TransportError::Http {
                status,
                headers,
                body,
                ..
            }) => CpaInferenceUpstreamNotification::Response {
                request_id: self.request_id.clone(),
                status_code: status.as_u16(),
                headers: headers.as_ref().map(wire_headers).unwrap_or_default(),
                body: body.clone(),
                oai_lb_node: response_oai_lb_node(headers.as_ref(), request_node.as_deref()),
            },
            Err(error) => CpaInferenceUpstreamNotification::Error {
                request_id: self.request_id.clone(),
                message: error.to_string(),
            },
        };
        self.notify(notification).await?;
        result.map(|mut response| {
            if let Some(capture) = capture
                && response
                    .headers
                    .get("content-type")
                    .and_then(|value| value.to_str().ok())
                    .is_some_and(|value| value.to_ascii_lowercase().contains("text/event-stream"))
            {
                let outgoing = self.outgoing.clone();
                let connection = self.connection_id;
                response.bytes = futures::stream::unfold(
                    (response.bytes, capture, outgoing),
                    move |(mut bytes, mut capture, outgoing)| async move {
                        let Some(chunk) = bytes.next().await else {
                            // Flush a complete final metadata record without changing body bytes.
                            for snapshot in capture.push(b"\n\n") {
                                outgoing
                                    .send_server_notification_to_connection_and_wait(
                                        connection,
                                        ServerNotification::CpaInferenceTicket(snapshot),
                                    )
                                    .await;
                            }
                            return None;
                        };
                        if let Ok(chunk) = &chunk {
                            for snapshot in capture.push(chunk) {
                                outgoing
                                    .send_server_notification_to_connection_and_wait(
                                        connection,
                                        ServerNotification::CpaInferenceTicket(snapshot),
                                    )
                                    .await;
                            }
                        }
                        Some((chunk, (bytes, capture, outgoing)))
                    },
                )
                .boxed();
            }
            response
        })
    }
}

/// IPC preserves actual header values. Ordinary log redaction uses a separate copy.
pub(super) fn wire_headers(headers: &HeaderMap) -> BTreeMap<String, Vec<String>> {
    headers
        .keys()
        .map(|name| {
            let values = headers
                .get_all(name)
                .iter()
                .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
                .collect();
            (name.to_string(), values)
        })
        .collect()
}

fn sensitive_log_header(name: &str) -> bool {
    matches!(
        name,
        "authorization" | "proxy-authorization" | "cookie" | "set-cookie" | "x-codex-turn-state"
    ) || ["authorization", "api-key", "apikey", "token", "secret"]
        .iter()
        .any(|part| name.contains(part))
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
