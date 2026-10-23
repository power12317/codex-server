//! Model-only CPA extension boundary, without a ThreadManager or tool runtime.
mod contexts;
mod identity_response;
mod login;
mod response_stream;
mod ticket;
mod upstream;
use crate::outgoing_message::ConnectionId;
use crate::outgoing_message::ConnectionRequestId;
use crate::outgoing_message::OutgoingMessageSender;
use base64::Engine;
use codex_api::ApiError;
use codex_api::ReqwestTransport;
use codex_api::ResponsesClient;
use codex_api::TransportError;
use codex_app_server_protocol::*;
use codex_core::config::Config;
use codex_http_client::ClientRouteClass;
use codex_http_client::HttpClientBuilder;
use codex_http_client::StreamResponse;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_model_provider::SharedModelProvider;
use codex_model_provider::auth_provider_from_auth;
use codex_model_provider::create_model_provider;
use futures::StreamExt;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
pub(crate) use upstream::record as record_request;

const UPSTREAM: &str = "6aaac694803004a63c6f393d153c96d22521a6b9";

type BridgeResult<T> = std::result::Result<T, JSONRPCErrorError>;

pub(crate) struct CpaBridge {
    credential_id: Option<String>,
    contexts: Mutex<contexts::Contexts>,
    login: Mutex<Option<login::Login>>,
    auth: Arc<AuthManager>,
    config: Arc<Config>,
    config_manager: crate::config_manager::ConfigManager,
    provider: Option<SharedModelProvider>,
    installation_id: String,
    outgoing: Arc<OutgoingMessageSender>,
    active: Mutex<HashMap<(ConnectionId, String), CancellationToken>>,
    models: codex_models_manager::manager::SharedModelsManager,
}

impl CpaBridge {
    pub(crate) fn new(
        config: Arc<Config>,
        config_manager: crate::config_manager::ConfigManager,
        auth: Arc<AuthManager>,
        installation_id: String,
        models: codex_models_manager::manager::SharedModelsManager,
        outgoing: Arc<OutgoingMessageSender>,
    ) -> Arc<Self> {
        let credential_id = auth
            .cpa_credential_file()
            .map(|file| file.credential_id.clone());
        let provider = credential_id
            .as_ref()
            .map(|_| create_model_provider(config.model_provider.clone(), Some(auth.clone())));

        Arc::new(Self {
            credential_id,
            contexts: Mutex::new(contexts::Contexts::default()),
            login: Mutex::new(None),
            auth,
            config,
            config_manager,
            provider,
            installation_id,
            models,
            outgoing,
            active: Mutex::new(HashMap::new()),
        })
    }

    pub(crate) fn enabled(&self) -> bool {
        self.credential_id.is_some()
    }

    pub(crate) fn capabilities(&self) -> BridgeResult<CpaCapabilitiesReadResponse> {
        if !self.enabled() {
            return Err(failure(/*status*/ 404, "CPA bridge is disabled"));
        }
        Ok(capabilities())
    }

    pub(crate) fn disconnect(&self, connection_id: ConnectionId) {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        active.retain(|(connection, _), token| {
            if *connection == connection_id {
                token.cancel();
                false
            } else {
                true
            }
        });
    }

    pub(crate) fn cancel(
        &self,
        connection_id: ConnectionId,
        params: CpaInferenceCancelParams,
    ) -> BridgeResult<CpaInferenceCancelResponse> {
        if let Some(token) = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&(connection_id, params.request_id))
        {
            token.cancel();
        }
        Ok(CpaInferenceCancelResponse {})
    }

    pub(crate) fn start(
        self: &Arc<Self>,
        id: ConnectionRequestId,
        params: CpaInferenceStartParams,
    ) -> BridgeResult<()> {
        upstream::record(
            &params.credential_id,
            &params.request_id,
            "cpa.request.start",
            json!({
                "source": codex_core::InferenceIdentity::from_request(&params.request),
                "rpcSessionId": params.session_id,
                "operation": params.operation, "imageApi": params.image_api,
                "codexHome": self.config.codex_home,
            }),
        );
        let prepared = (|| -> BridgeResult<_> {
            validate(&params)?;
            if self.credential_id.as_deref() != Some(params.credential_id.as_str()) {
                return Err(failure(/*status*/ 403, "credentialId mismatch"));
            }
            self.require_enabled()?;
            self.bound_auth()?;
            let session = self
                .contexts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(self, &params)
                .map_err(|error| {
                    failure(
                        if error.is::<contexts::CapacityExceeded>() {
                            429
                        } else {
                            400
                        },
                        &error.to_string(),
                    )
                })?;
            let token = CancellationToken::new();
            {
                let mut active = self
                    .active
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if active.contains_key(&(id.connection_id, params.request_id.clone())) {
                    return Err(failure(/*status*/ 409, "Duplicate active requestId"));
                }
                active.insert((id.connection_id, params.request_id.clone()), token.clone());
            }
            Ok((session, token))
        })();
        let (session, token) = prepared.inspect_err(|error| {
            upstream::record(&params.credential_id, &params.request_id, "cpa.request.error",
                json!({"error": error.message, "httpStatus": error.data.as_ref().map(|data| &data["httpStatus"])}));
        })?;
        let bridge = self.clone();
        tokio::spawn(async move {
            let started = AtomicBool::new(/*v*/ false);
            let mut auth_changes = bridge.auth.auth_change_receiver();
            let result = tokio::select! {
                biased;
                _ = token.cancelled() => Err(failure(/*status*/ 499, "Inference cancelled")),
                _ = async {
                    loop {
                        if auth_changes.changed().await.is_err() || bridge.bound_auth().is_err() { break; }
                    }
                } => Err(failure(/*status*/ 401, "Managed account changed")),
                result = bridge.run(&id, &params, &started, &session) => result,
            };
            let succeeded = result.is_ok();
            upstream::record(
                &params.credential_id,
                &params.request_id,
                if succeeded {
                    "cpa.request.completed"
                } else {
                    "cpa.request.error"
                },
                json!({"error": result.as_ref().err().map(|error| &error.message),
                    "turnStateLength": session.turn_state().map(str::len)}),
            );
            if let Err(mut error) = result {
                identity_response::IdentityResponse::new(
                    codex_core::InferenceIdentity::from_request(&params.request),
                    session.mapping(),
                )
                .failure(&mut error);
                if started.load(Ordering::Acquire) {
                    bridge
                        .outgoing
                        .send_server_notification_to_connections(
                            &[id.connection_id],
                            ServerNotification::CpaInferenceError(CpaInferenceErrorNotification {
                                request_id: params.request_id.clone(),
                                http_status: error
                                    .data
                                    .as_ref()
                                    .and_then(|d| d["httpStatus"].as_u64())
                                    .unwrap_or(502)
                                    as u16,
                                body: error
                                    .data
                                    .as_ref()
                                    .and_then(|data| data["body"].as_str().map(str::to_owned)),
                                headers: error.data.as_ref().and_then(|data| {
                                    serde_json::from_value(data["headers"].clone()).ok()
                                }),
                                message: error.message,
                            }),
                        )
                        .await;
                } else {
                    bridge.outgoing.send_error(id.clone(), error).await;
                }
            }
            if succeeded {
                bridge
                    .outgoing
                    .send_server_notification_to_connections(
                        &[id.connection_id],
                        ServerNotification::CpaInferenceCompleted(
                            CpaInferenceCompletedNotification {
                                request_id: params.request_id.clone(),
                            },
                        ),
                    )
                    .await;
            }
            bridge
                .active
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&(id.connection_id, params.request_id));
        });
        Ok(())
    }

    fn bound_auth(&self) -> BridgeResult<CodexAuth> {
        self.require_enabled()?;
        let auth = self
            .auth
            .auth_cached()
            .filter(|a| matches!(a, CodexAuth::Chatgpt(_)))
            .ok_or_else(|| failure(/*status*/ 401, "Managed ChatGPT login required"))?;
        Ok(auth)
    }

    async fn open(
        &self,
        id: &ConnectionRequestId,
        params: &CpaInferenceStartParams,
        session: &codex_core::InferenceSession,
    ) -> BridgeResult<StreamResponse> {
        self.auth.reload().await;
        self.config_manager
            .check_thread_model_provider(&self.config)
            .await
            .map_err(|error| failure(/*status*/ 502, &error.to_string()))?;
        // Only AuthManager acquires/refreshes credentials. No tokens cross IPC.
        let (auth, factory) = tokio::time::timeout(
            Duration::from_secs(/*secs*/ 30),
            self.auth.auth_with_http_client_factory(),
        )
        .await
        .map_err(|_| failure(/*status*/ 504, "Managed authentication timed out"))?
        .ok_or_else(|| failure(/*status*/ 401, "Managed ChatGPT login required"))?;
        self.bound_auth()?;
        // Reject custom credential providers; the bridge has exactly one auth owner.
        let model_provider = self
            .provider
            .as_ref()
            .ok_or_else(|| failure(/*status*/ 404, "CPA bridge is disabled"))?;
        let info = model_provider.info();
        if !info.is_openai()
            || !info.requires_openai_auth
            || info.auth.is_some()
            || info.env_key.is_some()
            || info.experimental_bearer_token.is_some()
            || info.aws.is_some()
        {
            return Err(failure(
                /*status*/ 400,
                "CPA requires the managed OpenAI provider",
            ));
        }
        let revision = *self.auth.auth_change_receiver().borrow();
        let image_endpoint = params.image_api == Some(CpaImageApi::Images);
        let mut provider = if image_endpoint {
            model_provider
                .api_provider()
                .await
                .map_err(|_| failure(502, "Unable to resolve managed image provider"))?
        } else {
            model_provider
                .responses_api_provider(&self.config.workspace_routing_context())
                .await
                .map_err(|_| failure(502, "Unable to resolve managed provider"))?
                .provider
        };
        if *self.auth.auth_change_receiver().borrow() != revision {
            return Err(failure(
                /*status*/ 409,
                "Authentication changed during request setup; retry",
            ));
        }
        self.bound_auth()?;
        // A failed stream must never cause another inference attempt.
        // RetryPolicy counts retries after the initial attempt. Zero means one POST.
        provider.retry.max_attempts = 0;
        let client = HttpClientBuilder::new()
            .with_chatgpt_cloudflare_cookie_store()
            .without_request_logging()
            .without_retries()
            .without_redirects()
            .build_respecting_outbound_proxy_policy(
                &factory,
                &provider.url_for_path(if image_endpoint {
                    &params.operation
                } else {
                    "responses"
                }),
                ClientRouteClass::Api,
            )
            .map_err(|_| {
                failure(
                    /*status*/ 502,
                    "Unable to construct managed HTTP client",
                )
            })?;
        let mut transport = upstream::UpstreamTransport {
            inner: ReqwestTransport::from_http_client(client),
            factory,
            outgoing: self.outgoing.clone(),
            connection_id: id.connection_id,
            request_id: params.request_id.clone(),
            credential_id: params.credential_id.clone(),
            ticket_model: None,
        };
        if image_endpoint {
            let operation = match params.operation.as_str() {
                "images/generations" => codex_api::ImageOperation::Generate,
                "images/edits" => codex_api::ImageOperation::Edit,
                _ => unreachable!("validated image operation"),
            };
            let request: codex_api::ImageInferenceRequest =
                serde_json::from_value(Value::Object(params.request.clone())).map_err(|error| {
                    adaptation_failure(&format!("Invalid native image request: {error}"))
                })?;
            let mapping = session.mapping();
            let mut headers =
                codex_api::build_session_headers(Some(mapping.session_id), Some(mapping.thread_id));
            if let Some(turn_id) = mapping.turn_id {
                headers.insert(
                    "x-codex-image-turn-id",
                    turn_id
                        .parse()
                        .map_err(|_| failure(400, "Invalid image turn id"))?,
                );
            }
            return codex_api::ImagesClient::new(
                transport,
                provider,
                auth_provider_from_auth(&auth),
            )
            .inference(operation, &request, headers)
            .await
            .map_err(|error| match error {
                ApiError::InvalidRequest { message } => adaptation_failure(&message),
                error => api_failure(error),
            });
        }
        let model = params
            .request
            .get("model")
            .and_then(Value::as_str)
            .ok_or_else(|| failure(/*status*/ 400, "model is required"))?;
        let model_info = self
            .models
            .get_model_info(model, &self.config.to_models_manager_config())
            .await;
        let (body, options) = codex_core::prepare_inference_request(
            session,
            &self.config,
            &model_info,
            &params.request,
        )
        .await
        .map_err(|error| adaptation_failure(&format!("{error:#}")))?;
        let source = codex_core::InferenceIdentity::from_request(&params.request);
        upstream::record(
            &params.credential_id,
            &params.request_id,
            "cpa.request.mapping",
            json!({
                "source": source,
                "worker": {"session_id": body["client_metadata"]["session_id"],
                    "thread_id": body["client_metadata"]["thread_id"], "turn_id": body["client_metadata"]["turn_id"]},
                "promptCacheKey": body["prompt_cache_key"],
                "identityPolicy": "session-root-and-thread-hierarchy",
                "requestPurpose": if source.is_title() { "thread_title" } else { "conversation" },
                "syntheticTurn": source.turn_id.is_none(),
                "transportSessionId": options.session_id, "transportThreadId": options.thread_id,
                "turnStateSentLength": options.extra_headers.get("x-codex-turn-state").map(|value| value.as_bytes().len()),
            }),
        );
        if params.capture_ticket_snapshot == Some(true) {
            transport.ticket_model = body.get("model").and_then(Value::as_str).map(str::to_owned);
        }
        ResponsesClient::new(transport, provider, auth_provider_from_auth(&auth))
            .stream_prepared_body(body, options)
            .await
            .map_err(api_failure)
    }

    async fn run(
        &self,
        id: &ConnectionRequestId,
        params: &CpaInferenceStartParams,
        started: &AtomicBool,
        session: &codex_core::InferenceSession,
    ) -> BridgeResult<()> {
        // A model invocation has one send owner. Authentication recovery and
        // resubmission after an upstream error belong to CPA/the caller.
        let mut stream = self.open(id, params, session).await?;
        let identities = identity_response::IdentityResponse::new(
            codex_core::InferenceIdentity::from_request(&params.request),
            session.mapping(),
        );
        // The trusted IPC receives all actual response headers, with identity projection only.
        let headers = upstream::wire_headers(&stream.headers);
        let mut translated_headers =
            serde_json::to_value(&headers).map_err(|error| failure(502, &error.to_string()))?;
        if let Some(fields) = translated_headers.as_object_mut() {
            identities.fields(fields);
        }
        let headers = serde_json::from_value(translated_headers)
            .map_err(|error| failure(502, &error.to_string()))?;
        let content_type = stream
            .headers
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("text/event-stream");
        let mut response = response_stream::ResponseStream::new(identities, content_type);
        self.outgoing
            .send_response(
                id.clone(),
                CpaInferenceStartResponse {
                    request_id: params.request_id.clone(),
                    status_code: stream.status.as_u16(),
                    headers,
                },
            )
            .await;
        started.store(true, Ordering::Release);
        while let Some(chunk) = stream.bytes.next().await {
            let bytes = chunk.map_err(|error| api_failure(ApiError::Transport(error)))?;
            let translated = if params.image_api.is_some() {
                bytes.to_vec()
            } else {
                response.push(&bytes)
            };
            self.send_body(id, &params.request_id, &translated).await?;
        }
        self.send_body(id, &params.request_id, &response.finish())
            .await?;
        Ok(())
    }
    async fn send_body(
        &self,
        id: &ConnectionRequestId,
        request_id: &str,
        bytes: &[u8],
    ) -> BridgeResult<()> {
        for bytes in bytes.chunks(64 * 1024) {
            if !self
                .outgoing
                .send_server_notification_to_connection_and_wait(
                    id.connection_id,
                    ServerNotification::CpaInferenceBody(CpaInferenceBodyNotification {
                        request_id: request_id.to_owned(),
                        body_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
                    }),
                )
                .await
            {
                return Err(failure(499, "Connection closed"));
            }
        }
        Ok(())
    }
}

pub(crate) fn capabilities() -> CpaCapabilitiesReadResponse {
    CpaCapabilitiesReadResponse {
        runtime_version: env!("CARGO_PKG_VERSION").into(),
        upstream_revision: UPSTREAM.into(),
        manual_oauth: true,
        upstream_logs: true,
        upstream_body_logs: true,
        raw_body: true,
        business_notification: "cpa/inference/body".into(),
        completion_semantics: "http-body-eof".into(),
        diagnostics_completed: false,
        responses_websocket: false,
        response_identity_mapping: true,
        ticket_snapshots: Some(true),
        execution_mode: "inference-only".into(),
        operations: vec![
            "responses".into(),
            "images/generations".into(),
            "images/edits".into(),
        ],
    }
}

fn failure(status: u16, message: &str) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: -32000,
        message: message.into(),
        data: Some(json!({"httpStatus": status})),
    }
}

fn adaptation_failure(message: &str) -> JSONRPCErrorError {
    let mut error = failure(400, message);
    error.data = Some(json!({"httpStatus":400,"headers":{},"body":json!({
        "error":{"message":message,"type":"request_adaptation_error"}
    }).to_string()}));
    error
}

fn api_failure(error: ApiError) -> JSONRPCErrorError {
    if let ApiError::Transport(TransportError::Http {
        status,
        headers,
        body,
        ..
    }) = error
    {
        return JSONRPCErrorError {
            code: -32000,
            message: "Upstream inference failed".into(),
            data: Some(
                json!({"httpStatus":status.as_u16(),"body":body,"headers":headers.as_ref().map(upstream::wire_headers)}),
            ),
        };
    }
    let (status, message) = match error {
        ApiError::Transport(TransportError::Http { status, .. }) | ApiError::Api { status, .. } => {
            (status.as_u16(), "Upstream inference failed")
        }
        ApiError::Transport(TransportError::Policy(_)) => (502, "Upstream transport policy denied"),
        ApiError::Transport(TransportError::Build(_)) => {
            (502, "Unable to prepare upstream request")
        }
        ApiError::Transport(
            TransportError::Connection(_) | TransportError::Network(_) | TransportError::Timeout,
        ) => (502, "Upstream network failure"),
        _ => (502, "Upstream inference failed"),
    };
    failure(status, message)
}

fn validate(params: &CpaInferenceStartParams) -> BridgeResult<()> {
    for value in [
        &params.request_id,
        &params.source_format,
        &params.session_id,
    ] {
        if value.is_empty() {
            return Err(failure(/*status*/ 400, "Invalid request identity"));
        }
    }
    if params.credential_id.is_empty() {
        return Err(failure(/*status*/ 400, "Invalid credentialId"));
    }
    if matches!(
        params.operation.as_str(),
        "images/generations" | "images/edits"
    ) {
        if params.image_api.is_none() {
            return Err(failure(400, "imageApi is required for image operations"));
        }
        return Ok(());
    }
    if params.operation != "responses" {
        return Err(failure(
            /*status*/ 400,
            "Unsupported operation; supported operations are responses, images/generations and images/edits",
        ));
    }
    if params.image_api.is_some() {
        return Err(failure(400, "imageApi is only valid for image operations"));
    }
    // Caller transport/auth/device metadata is sanitized during native request preparation.
    // Business Responses fields remain available to the worker-owned request builder.
    Ok(())
}

#[cfg(test)]
#[path = "cpa_bridge_tests.rs"]
mod tests;
