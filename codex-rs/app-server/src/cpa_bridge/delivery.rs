//! Business delivery never awaits diagnostic processing or diagnostic acknowledgements.
use super::*;
use base64::Engine;
use std::collections::BTreeMap;
use tokio::sync::mpsc::UnboundedReceiver;
use upstream::Business;
use upstream::Diagnostic;

pub(super) fn diagnostics(
    outgoing: Arc<OutgoingMessageSender>,
    connection_id: ConnectionId,
    credential_id: String,
    request_id: String,
    mut receiver: UnboundedReceiver<Diagnostic>,
) {
    tokio::spawn(async move {
        while let Some(message) = receiver.recv().await {
            let (event, details) = match message {
                Diagnostic::Notification(notification) => {
                    let mut details = serde_json::to_value(&notification).unwrap_or(Value::Null);
                    if let Some(fields) = details.as_object_mut() {
                        fields.remove("body");
                        fields.remove("bodyBase64");
                    }
                    outgoing.queue_cpa_diagnostic(
                        connection_id,
                        ServerNotification::CpaInferenceUpstream(notification),
                    );
                    ("cpa.request.upstream", details)
                }
                Diagnostic::Record { event, details } => (event, details),
            };
            let credential = credential_id.clone();
            let request = request_id.clone();
            // A slow stderr/log sink occupies only this diagnostic consumer.
            let _ = tokio::task::spawn_blocking(move || {
                upstream::record(&credential, &request, event, details)
            })
            .await;
        }
        outgoing.queue_cpa_diagnostic(
            connection_id,
            ServerNotification::CpaInferenceDiagnosticsCompleted(
                CpaInferenceDiagnosticsCompletedNotification { request_id },
            ),
        );
    });
}

fn identities(
    params: &CpaInferenceStartParams,
    session: &codex_core::InferenceSession,
) -> identity_response::IdentityResponse {
    identity_response::IdentityResponse::new(
        codex_core::InferenceIdentity::from_request(&params.request),
        session.mapping(),
    )
}

fn public_headers(
    params: &CpaInferenceStartParams,
    session: &codex_core::InferenceSession,
    upstream: &axum::http::HeaderMap,
    content_type: &str,
) -> BTreeMap<String, Vec<String>> {
    let mut headers = serde_json::Map::new();
    for name in [
        "x-request-id",
        "retry-after",
        "openai-model",
        "openai-processing-ms",
        "x-codex-imagegen-request-id",
        "session-id",
        "thread-id",
        "x-codex-parent-thread-id",
        "x-codex-window-id",
        "x-codex-turn-metadata",
    ] {
        let values: Vec<_> = upstream
            .get_all(name)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .collect();
        if !values.is_empty() {
            headers.insert(name.into(), json!(values));
        }
    }
    headers.insert("content-type".into(), json!([content_type]));
    identities(params, session).fields(&mut headers);
    serde_json::from_value(Value::Object(headers)).unwrap_or_default()
}

async fn notify(
    outgoing: &OutgoingMessageSender,
    id: &ConnectionRequestId,
    notification: ServerNotification,
) {
    outgoing
        .send_server_notification_to_connections(&[id.connection_id], notification)
        .await;
}

async fn event(
    outgoing: &OutgoingMessageSender,
    id: &ConnectionRequestId,
    request_id: &str,
    bytes: Vec<u8>,
) {
    if !bytes.is_empty() {
        notify(
            outgoing,
            id,
            ServerNotification::CpaInferenceEvent(CpaInferenceEventNotification {
                request_id: request_id.into(),
                body_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            }),
        )
        .await;
    }
}

pub(super) async fn business(
    outgoing: &OutgoingMessageSender,
    id: &ConnectionRequestId,
    params: &CpaInferenceStartParams,
    session: &codex_core::InferenceSession,
    cancellation: &CancellationToken,
    mut receiver: UnboundedReceiver<Business>,
) {
    let mut content_type = if params.image_api == Some(CpaImageApi::Images) {
        "application/json"
    } else {
        "text/event-stream"
    };
    let mut stream =
        response_stream::ResponseStream::new(identities(params, session), content_type);
    let mut started = false;
    let result = loop {
        let message = tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                let mut error = failure(499,"Inference delivery cancelled");
                if let Some(data) = error.data.as_mut() { data["body"] = json!(json!({"error":{"message":"Inference delivery cancelled","type":"delivery_cancelled"}}).to_string()); }
                break Err(error);
            }
            message = receiver.recv() => message,
        };
        match message {
            Some(Business::Ready(headers)) => {
                if !started {
                    if params.image_api == Some(CpaImageApi::Images) {
                        content_type = if headers
                            .get(axum::http::header::CONTENT_TYPE)
                            .and_then(|value| value.to_str().ok())
                            .is_some_and(|value| {
                                value.to_ascii_lowercase().contains("text/event-stream")
                            }) {
                            "text/event-stream"
                        } else {
                            "application/json"
                        };
                        stream = response_stream::ResponseStream::new(
                            identities(params, session),
                            content_type,
                        );
                    }
                    outgoing
                        .send_response(
                            id.clone(),
                            CpaInferenceStartResponse {
                                request_id: params.request_id.clone(),
                                status_code: 200,
                                headers: public_headers(params, session, &headers, content_type),
                            },
                        )
                        .await;
                    started = true;
                }
            }
            Some(Business::Event(bytes)) => {
                if started {
                    let bytes = stream.push(&bytes);
                    event(outgoing, id, &params.request_id, bytes).await;
                }
            }
            Some(Business::Finished(result)) => break result,
            None => {
                break Err(failure(
                    502,
                    "Native execution ended without a final result",
                ));
            }
        }
    };
    match result {
        Ok(()) => {
            if !started {
                outgoing
                    .send_response(
                        id.clone(),
                        CpaInferenceStartResponse {
                            request_id: params.request_id.clone(),
                            status_code: 200,
                            headers: public_headers(
                                params,
                                session,
                                &axum::http::HeaderMap::new(),
                                content_type,
                            ),
                        },
                    )
                    .await;
            }
            event(outgoing, id, &params.request_id, stream.finish()).await;
            notify(
                outgoing,
                id,
                ServerNotification::CpaInferenceCompleted(CpaInferenceCompletedNotification {
                    request_id: params.request_id.clone(),
                }),
            )
            .await;
        }
        Err(mut error) => {
            identities(params, session).failure(&mut error);
            if !started {
                outgoing.send_error(id.clone(), error).await;
            } else {
                let data = error.data.unwrap_or(Value::Null);
                notify(
                    outgoing,
                    id,
                    ServerNotification::CpaInferenceError(CpaInferenceErrorNotification {
                        request_id: params.request_id.clone(),
                        http_status: data["httpStatus"].as_u64().unwrap_or(502) as u16,
                        message: error.message,
                        body: data["body"].as_str().map(str::to_owned),
                        headers: serde_json::from_value(data["headers"].clone()).ok(),
                        native_error: data.get("nativeError").cloned(),
                    }),
                )
                .await;
            }
        }
    }
}
