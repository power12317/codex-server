//! Opt-in CPA IPC v3. No request may enter the agent/tool execution pipeline.
use crate::JsonSchema;
use crate::TS;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct CpaCapabilitiesReadParams {}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct CpaCapabilitiesReadResponse {
    pub protocol_version: u32,
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
    /// Known response identity fields are restored to caller values. Other payloads are opaque.
    #[serde(default)]
    pub response_identity_mapping: bool,
    /// Opt-in, exchange-scoped source ticket snapshots. The business stream remains IPC v3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ticket_snapshots: Option<bool>,
    pub operations: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
    /// Send private ticket snapshots only to this inference's connection. Default: disabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub capture_ticket_snapshot: Option<bool>,
    pub request: serde_json::Map<String, Value>,
}

/// Private routing material; never include its values in diagnostic formatting.
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct CpaInferenceTicketNotification {
    pub request_id: String,
    pub credential_id: String,
    pub model: String,
    pub observed_at: String,
    pub state: String,
    pub route: CpaTicketRoute,
}

impl std::fmt::Debug for CpaInferenceTicketNotification {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CpaInferenceTicketNotification")
            .field("material", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct CpaTicketRoute {
    #[serde(rename = "__cflb")]
    #[ts(rename = "__cflb")]
    pub cflb: CpaTicketCookie,
    #[serde(rename = "__oailb")]
    #[ts(rename = "__oailb")]
    pub oailb: CpaTicketCookie,
}

#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct CpaTicketCookie {
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub expires_at: Option<String>,
}

impl std::fmt::Debug for CpaTicketCookie {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CpaTicketCookie")
            .field("value", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export_to = "v2/")]
pub enum CpaImageApi {
    Images,
    Responses,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct CpaInferenceStartResponse {
    pub request_id: String,
    pub status_code: u16,
    pub headers: BTreeMap<String, Vec<String>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct CpaInferenceCancelParams {
    pub request_id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct CpaInferenceCancelResponse {}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct CpaInferenceBodyNotification {
    pub request_id: String,
    pub body_base64: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct CpaInferenceCompletedNotification {
    pub request_id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct CpaInferenceErrorNotification {
    pub body: Option<String>,
    pub headers: Option<BTreeMap<String, Vec<String>>>,
    pub request_id: String,
    pub http_status: u16,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct CpaCredentialReloadParams {
    #[ts(optional = nullable)]
    pub credential_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct CpaAuthLoginStartParams {
    pub credential_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct CpaAuthLoginStartResponse {
    pub login_id: String,
    pub auth_url: String,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct CpaAuthLoginCallbackParams {
    pub login_id: String,
    pub redirect_url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct CpaAuthLoginStatusParams {
    pub login_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
        url: String,
        method: String,
        headers: BTreeMap<String, Vec<String>>,
        body: String,
        access_token_sha256: Option<String>,
        #[serde(rename = "oaiLbNode")]
        #[ts(rename = "oaiLbNode")]
        oai_lb_node: Option<String>,
    },
    Response {
        request_id: String,
        status_code: u16,
        headers: BTreeMap<String, Vec<String>>,
        body: Option<String>,
        #[serde(rename = "oaiLbNode")]
        #[ts(rename = "oaiLbNode")]
        oai_lb_node: Option<String>,
    },
    Error {
        request_id: String,
        message: String,
    },
}
