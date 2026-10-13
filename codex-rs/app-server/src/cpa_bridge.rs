//! CPA IPC v4 boundary. This module deliberately has no ThreadManager or tool runtime.
mod contexts;
mod identity_response;
mod images;
mod login;
mod response_stream;
mod upstream;
use crate::outgoing_message::ConnectionId;
use crate::outgoing_message::ConnectionRequestId;
use crate::outgoing_message::OutgoingMessageSender;
use base64::Engine;
use codex_app_server_protocol::*;
use codex_core::config::Config;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_model_provider::SharedModelProvider;
use codex_model_provider::create_model_provider;
use serde_json::Value;
use serde_json::json;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;
pub(crate) use upstream::record as record_request;

const UPSTREAM: &str = "092d3acd6bec3e3a14bdc7e7a2810ab628ab759d";

type BridgeResult<T> = std::result::Result<T, JSONRPCErrorError>;
type InferenceResponse = (BTreeMap<String, Vec<String>>, Vec<u8>);

struct NativeOutcome {
    result: BridgeResult<InferenceResponse>,
    diagnostics: tokio::task::JoinHandle<()>,
}

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
            let mut auth_changes = bridge.auth.auth_change_receiver();
            let result = tokio::select! {
                biased;
                _ = token.cancelled() => Err(failure(/*status*/ 499, "Inference cancelled")),
                _ = async {
                    loop {
                        if auth_changes.changed().await.is_err() || bridge.bound_auth().is_err() { break; }
                    }
                } => Err(failure(/*status*/ 401, "Managed account changed")),
                result = bridge.run(&id, &params, &session) => result,
            };
            let result = match result {
                Ok(outcome) => {
                    // Delivery cannot turn an already settled native result into cancellation.
                    if let Err(error) = outcome.diagnostics.await {
                        tracing::warn!(%error, "native diagnostic delivery task stopped");
                    }
                    outcome.result
                }
                Err(error) => Err(error),
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
                    "turnState": session.turn_state(),
                    "turnStateLength": session.turn_state().map(str::len)}),
            );
            match result {
                Err(mut error) => {
                    identity_response::IdentityResponse::new(
                        codex_core::InferenceIdentity::from_request(&params.request),
                        session.mapping(),
                    )
                    .failure(&mut error);
                    bridge.outgoing.send_error(id.clone(), error).await;
                }
                Ok((headers, body)) => {
                    bridge
                        .outgoing
                        .send_response(
                            id.clone(),
                            CpaInferenceStartResponse {
                                request_id: params.request_id.clone(),
                                status_code: 200,
                                headers,
                            },
                        )
                        .await;
                    bridge
                        .outgoing
                        .send_server_notification_to_connections(
                            &[id.connection_id],
                            ServerNotification::CpaInferenceCompleted(
                                CpaInferenceCompletedNotification {
                                    request_id: params.request_id.clone(),
                                    body_base64: base64::engine::general_purpose::STANDARD
                                        .encode(body),
                                },
                            ),
                        )
                        .await;
                }
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

    async fn run(
        &self,
        id: &ConnectionRequestId,
        params: &CpaInferenceStartParams,
        session: &codex_core::InferenceSession,
    ) -> BridgeResult<NativeOutcome> {
        self.auth.reload().await;
        self.config_manager
            .check_thread_model_provider(&self.config)
            .await
            .map_err(|error| failure(502, &error.to_string()))?;
        self.bound_auth()?;
        let provider = self
            .provider
            .as_ref()
            .ok_or_else(|| failure(404, "CPA bridge is disabled"))?;
        let info = provider.info();
        if !info.is_openai()
            || !info.requires_openai_auth
            || info.auth.is_some()
            || info.env_key.is_some()
            || info.experimental_bearer_token.is_some()
            || info.aws.is_some()
        {
            return Err(failure(400, "CPA requires the managed OpenAI provider"));
        }
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let observation = Arc::new(upstream::NativeObservation::new(
            params.request_id.clone(),
            params.credential_id.clone(),
            sender,
        ));
        let outgoing = self.outgoing.clone();
        let connection_id = id.connection_id;
        let diagnostics = tokio::spawn(async move {
            while let Some(notification) = receiver.recv().await {
                if !outgoing
                    .send_server_notification_to_connection_and_wait(
                        connection_id,
                        ServerNotification::CpaInferenceUpstream(notification),
                    )
                    .await
                {
                    break;
                }
            }
        });
        let image_endpoint = params.image_api == Some(CpaImageApi::Images);
        let result = if image_endpoint {
            codex_http_client::observation::observe_transport(
                observation.clone(),
                images::execute(provider.clone(), &self.config, params, session),
            )
            .await
        } else {
            let model = params
                .request
                .get("model")
                .and_then(Value::as_str)
                .ok_or_else(|| failure(400, "model is required"))?;
            let model_info = self
                .models
                .get_model_info(model, &self.config.to_models_manager_config())
                .await;
            upstream::record(
                &params.credential_id,
                &params.request_id,
                "cpa.request.mapping",
                json!({"source":codex_core::InferenceIdentity::from_request(&params.request),
                    "worker":session.mapping(),"identityPolicy":"session-root-and-thread-hierarchy"}),
            );
            session
                .execute(
                    &self.config,
                    &model_info,
                    &params.request,
                    observation.clone(),
                )
                .await
                .map_err(|error| native_failure(&error))
        };
        let (upstream_headers, body) = observation.result(image_endpoint);
        drop(observation);
        let result = result.and_then(|()| {
            let identities = identity_response::IdentityResponse::new(
                codex_core::InferenceIdentity::from_request(&params.request),
                session.mapping(),
            );
            let mut headers = BTreeMap::new();
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
                let values: Vec<String> = upstream_headers
                    .get_all(name)
                    .iter()
                    .filter_map(|value| value.to_str().ok().map(str::to_owned))
                    .collect();
                if !values.is_empty() {
                    headers.insert(name.to_owned(), values);
                }
            }
            let content_type = if image_endpoint {
                "application/json"
            } else {
                "text/event-stream"
            };
            headers.insert("content-type".into(), vec![content_type.into()]);
            let mut headers =
                serde_json::to_value(headers).map_err(|error| failure(502, &error.to_string()))?;
            if let Some(fields) = headers.as_object_mut() {
                identities.fields(fields);
            }
            let mut response = response_stream::ResponseStream::new(identities, content_type);
            let mut translated = response.push(&body);
            translated.extend(response.finish());
            Ok((
                serde_json::from_value(headers)
                    .map_err(|error| failure(502, &error.to_string()))?,
                translated,
            ))
        });
        Ok(NativeOutcome {
            result,
            diagnostics,
        })
    }
}

pub(crate) fn capabilities() -> CpaCapabilitiesReadResponse {
    CpaCapabilitiesReadResponse {
        protocol_version: 4,
        runtime_version: env!("CARGO_PKG_VERSION").into(),
        upstream_revision: UPSTREAM.into(),
        manual_oauth: true,
        upstream_logs: true,
        upstream_body_logs: true,
        raw_body: true,
        response_identity_mapping: true,
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
        data: Some(
            json!({"httpStatus": status, "body": json!({"error":{"message":message,"type":"native_model_error"}}).to_string(), "headers": {}}),
        ),
    }
}

fn native_failure(error: &codex_protocol::error::CodexErr) -> JSONRPCErrorError {
    use codex_protocol::error::CodexErrorDetails;
    let status = error
        .http_status_code_value()
        .unwrap_or_else(|| match error.details() {
            CodexErrorDetails::InvalidRequest(_)
            | CodexErrorDetails::InvalidImageRequest()
            | CodexErrorDetails::InvalidPrompt { .. }
            | CodexErrorDetails::UnsupportedOperation(_)
            | CodexErrorDetails::ContextWindowExceeded
            | CodexErrorDetails::ContentFilter => 400,
            CodexErrorDetails::UsageLimitReached(_)
            | CodexErrorDetails::QuotaExceeded
            | CodexErrorDetails::RateLimitExceeded(_) => 429,
            CodexErrorDetails::RefreshTokenFailed(_) => 401,
            CodexErrorDetails::TurnAborted | CodexErrorDetails::Interrupted => 499,
            _ => 502,
        });
    let mut result = failure(status, &error.to_string());
    if let Some(data) = result.data.as_mut() {
        data["nativeError"] = json!(error.to_codex_protocol_error());
        if let CodexErrorDetails::UnexpectedStatus(details) = error.details() {
            data["body"] = json!(details.body);
            if let Some(request_id) = &details.request_id {
                data["headers"] = json!({"x-request-id":[request_id]});
            }
        }
    }
    result
}

fn validate(params: &CpaInferenceStartParams) -> BridgeResult<()> {
    for value in [
        &params.request_id,
        &params.source_format,
        &params.session_id,
    ] {
        if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
            return Err(failure(/*status*/ 400, "Invalid request identity"));
        }
    }
    if params.credential_id.is_empty() || params.credential_id.len() > 4096 {
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
    let r = &params.request;
    if r.contains_key("previous_response_id")
        || r.contains_key("conversation")
        || r.contains_key("generate")
        || r.get("store").is_some_and(|v| v != &Value::Bool(false))
        || r.get("background")
            .is_some_and(|v| v != &Value::Bool(false))
    {
        return Err(failure(
            /*status*/ 400,
            "Persistent sessions, generation control, storage and background responses are unsupported",
        ));
    }
    // Caller transport/auth/device metadata is sanitized during native request preparation.
    // Business Responses fields remain available to the worker-owned request builder.
    Ok(())
}

#[cfg(test)]
#[path = "cpa_bridge_tests.rs"]
mod tests;
