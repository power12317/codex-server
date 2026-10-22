//! Model-only CPA extensions with incremental native business delivery.
use crate::JsonSchema;
use crate::TS;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct CpaCapabilitiesReadParams {}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct CpaCapabilitiesReadResponse {
    pub runtime_version: String,
    pub upstream_revision: String,
    #[serde(rename = "manualOAuth")]
    #[ts(rename = "manualOAuth")]
    pub manual_oauth: bool,
    pub upstream_logs: bool,
    pub upstream_body_logs: bool,
    pub execution_mode: String,
    /// Responses byte-stream transport (`bodyBase64`), not parsed IPC events.
    pub raw_body: bool,
    /// Business byte notification carried by the app-server envelope.
    pub business_notification: String,
    /// Whether completion represents HTTP EOF or an accepted native terminal outcome.
    pub completion_semantics: String,
    /// Diagnostics have an independent `cpa/inference/diagnostics/completed` terminal.
    pub diagnostics_completed: bool,
    /// The Responses implementation supports native WebSocket transport.
    pub responses_websocket: bool,
    /// Known response identity fields are restored to caller values. Other payloads are opaque.
    #[serde(default)]
    pub response_identity_mapping: bool,
    /// Opt-in, exchange-scoped source ticket snapshot support.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ticket_snapshots: Option<bool>,
    pub operations: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct CpaInferenceStartParams {
    pub request_id: String,
    pub credential_id: String,
    pub operation: String,
    pub source_format: String,
    pub session_id: String,
    /// Required for image operations: the business payload uses Images or Responses types.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub image_api: Option<CpaImageApi>,
    /// A required source ticket snapshot fails when the runtime does not support it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub capture_ticket_snapshot: Option<bool>,
    pub request: serde_json::Map<String, Value>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export_to = "v2/")]
pub enum CpaImageApi {
    Images,
    Responses,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct CpaInferenceStartResponse {
    pub request_id: String,
    pub status_code: u16,
    pub headers: BTreeMap<String, Vec<String>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct CpaInferenceCancelParams {
    pub request_id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct CpaInferenceCancelResponse {}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct CpaInferenceCompletedNotification {
    pub request_id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct CpaInferenceEventNotification {
    pub request_id: String,
    pub body_base64: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct CpaInferenceDiagnosticsCompletedNotification {
    pub request_id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
/// Native failure, cancellation or worker loss after the start reply.
pub struct CpaInferenceErrorNotification {
    pub body: Option<String>,
    pub headers: Option<BTreeMap<String, Vec<String>>>,
    pub request_id: String,
    pub http_status: u16,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub native_error: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct CpaCredentialReloadParams {
    #[ts(optional = nullable)]
    pub credential_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct CpaAuthLoginStartParams {
    pub credential_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct CpaAuthLoginStartResponse {
    pub login_id: String,
    pub auth_url: String,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct CpaAuthLoginCallbackParams {
    pub login_id: String,
    pub redirect_url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct CpaAuthLoginStatusParams {
    pub login_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct CpaAuthLoginStatusResponse {
    pub status: String,
    pub error: Option<String>,
}

/// Actual inference transport exchanges, including attempts made during auth recovery.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    export_to = "v2/"
)]
pub enum CpaInferenceUpstreamNotification {
    Request {
        request_id: String,
        transport: String,
        url: String,
        method: String,
        headers: BTreeMap<String, Vec<String>>,
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        body_base64: Option<String>,
        access_token_sha256: Option<String>,
        #[serde(rename = "oaiLbNode")]
        #[ts(rename = "oaiLbNode")]
        oai_lb_node: Option<String>,
    },
    Response {
        request_id: String,
        transport: String,
        status_code: u16,
        headers: BTreeMap<String, Vec<String>>,
        body: Option<String>,
        #[serde(rename = "oaiLbNode")]
        #[ts(rename = "oaiLbNode")]
        oai_lb_node: Option<String>,
    },
    Error {
        request_id: String,
        transport: String,
        message: String,
    },
    /// Actual attempt bytes for diagnostics, independent of successful delivery.
    Body {
        request_id: String,
        transport: String,
        body_base64: String,
    },
}
