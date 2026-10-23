//! Native request identity and turn state for caller-owned inference.
use super::*;
use crate::responses_metadata::CodexResponsesRequestKind;
use serde::Serialize;
use serde_json::Map;
use serde_json::Value;

/// Business identities supplied by the caller, independent of worker authentication.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct InferenceIdentity {
    pub session_id: Option<String>,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub prompt_cache_key: Option<String>,
    pub parent_turn_id: Option<String>,
    pub root_turn_id: Option<String>,
    pub parent_thread_id: Option<String>,
    pub thread_source: Option<String>,
    pub subagent_kind: Option<String>,
    pub subagent_header: Option<String>,
    pub agent_name: Option<String>,
    pub window_id: Option<String>,
    pub context_window_id: Option<String>,
    pub request_kind: Option<String>,
}

impl InferenceIdentity {
    /// Root identity within the credential and CPA scope; children share this root.
    pub fn session_key(&self) -> Option<&str> {
        self.session_id.as_deref().or(self.thread_id.as_deref())
    }

    pub fn thread_key(&self) -> Option<&str> {
        self.thread_id.as_deref().or(self.session_id.as_deref())
    }

    /// Explicit native purpose metadata, never inferred from prompt text.
    pub fn is_title(&self) -> bool {
        self.thread_source.as_deref() == Some("thread_title")
            || matches!(
                self.request_kind.as_deref(),
                Some("thread_title" | "title_generation")
            )
    }

    pub fn map(&self, scope: &str) -> anyhow::Result<InferenceMapping> {
        let namespace = Uuid::new_v5(&Uuid::NAMESPACE_OID, scope.as_bytes());
        let session = Uuid::new_v5(&namespace, &serde_json::to_vec(&self.session_key())?);
        let map_thread = |thread: Option<&str>| -> anyhow::Result<String> {
            if thread.is_none() || thread == self.session_key() {
                Ok(session.to_string())
            } else {
                Ok(Uuid::new_v5(&session, &serde_json::to_vec(&thread)?).to_string())
            }
        };
        let thread_id = map_thread(self.thread_key())?;
        Ok(InferenceMapping {
            session_id: session.to_string(),
            window_id: format!("{thread_id}:0"),
            context_window_id: None,
            thread_id,
            parent_thread_id: self
                .parent_thread_id
                .as_deref()
                .map(|id| map_thread(Some(id)))
                .transpose()?,
            turn_id: self.turn_id.clone(),
            parent_turn_id: self.parent_turn_id.clone(),
            root_turn_id: self.root_turn_id.clone(),
        })
    }

    pub fn from_request(request: &Map<String, Value>) -> Self {
        let metadata = request.get("client_metadata").and_then(Value::as_object);
        let nested: Option<Value> = metadata
            .and_then(|metadata| metadata.get("x-codex-turn-metadata"))
            .or_else(|| request.get("x-codex-turn-metadata"))
            .and_then(Value::as_str)
            .and_then(|value| serde_json::from_str(value).ok());
        let read = |key| {
            [
                request.get(key),
                metadata.and_then(|metadata| metadata.get(key)),
                nested.as_ref().and_then(|nested| nested.get(key)),
            ]
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .find(|value| !value.is_empty())
            .map(str::to_owned)
        };
        Self {
            session_id: read("session_id"),
            thread_id: read("thread_id"),
            turn_id: read("turn_id"),
            prompt_cache_key: read("prompt_cache_key"),
            parent_turn_id: read("parent_turn_id"),
            root_turn_id: read("root_turn_id"),
            parent_thread_id: read("parent_thread_id").or_else(|| read("x-codex-parent-thread-id")),
            thread_source: read("thread_source"),
            subagent_kind: read("subagent_kind"),
            subagent_header: read("x-openai-subagent"),
            agent_name: read("agent_name"),
            window_id: read("window_id").or_else(|| read("x-codex-window-id")),
            context_window_id: read("context_window_id"),
            request_kind: read("request_kind"),
        }
    }
}

#[cfg(test)]
#[path = "inference_session_tests.rs"]
mod tests;

/// Only identities are retained; no conversation contents or persisted thread are created.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InferenceMapping {
    pub session_id: String,
    pub thread_id: String,
    pub parent_thread_id: Option<String>,
    pub window_id: String,
    pub context_window_id: Option<String>,
    pub turn_id: Option<String>,
    pub parent_turn_id: Option<String>,
    pub root_turn_id: Option<String>,
}

/// A caller turn's native client and first upstream routing state. No tool runtime.
pub struct InferenceSession {
    // All turns of one logical thread lease its native cache exclusively. Idle turn
    // contexts retain their ticket, never a ModelClientSession holding the socket.
    pub(super) client_gate: Arc<tokio::sync::Mutex<()>>,
    pub(super) client: ModelClient,
    pub(super) turn_state: Arc<OnceLock<String>>,
    pub(super) metadata: CodexResponsesMetadata,
}

impl InferenceSession {
    pub fn new(
        config: &crate::config::Config,
        auth: Arc<AuthManager>,
        installation_id: &str,
        scope: &str,
        source: &InferenceIdentity,
    ) -> anyhow::Result<Self> {
        let mapped = source.map(scope)?;
        anyhow::ensure!(
            source.turn_id.is_some(),
            "inference turn_id must be resolved"
        );
        let subagent = source
            .subagent_header
            .clone()
            .or_else(|| {
                source.subagent_kind.as_deref().map(|kind| {
                    if kind == "thread_spawn" {
                        "collab_spawn"
                    } else {
                        kind
                    }
                    .to_owned()
                })
            })
            .or_else(|| {
                (source.thread_source.as_deref() == Some("subagent")).then(|| "subagent".to_owned())
            });
        let session_source = subagent.as_ref().map_or(SessionSource::Cli, |kind| {
            SessionSource::SubAgent(codex_protocol::protocol::SubAgentSource::Other(
                kind.clone(),
            ))
        });
        let mut provider_info = config.model_provider.clone();
        provider_info.request_max_retries = Some(0);
        provider_info.stream_max_retries = Some(0);
        let client = ModelClient::new(
            Some(auth),
            if config
                .features
                .enabled(codex_features::Feature::UseAgentIdentity)
            {
                AgentIdentityAuthPolicy::ChatGptAuth
            } else {
                AgentIdentityAuthPolicy::JwtOnly
            },
            ThreadId::from_string(&mapped.thread_id)?,
            provider_info,
            session_source,
            codex_login::default_client::originator().value,
            config.model_verbosity,
            config
                .features
                .enabled(codex_features::Feature::ContentItemKinds),
            config
                .features
                .enabled(codex_features::Feature::ReasoningEffortOverride)
                && !source.is_title(),
            config
                .features
                .enabled(codex_features::Feature::EnableRequestCompression),
            config
                .features
                .enabled(codex_features::Feature::RuntimeMetrics),
            crate::session::session::Session::build_model_client_beta_features_header(config),
            config
                .features
                .enabled(codex_features::Feature::ConcurrentReasoningSummaries),
            config
                .features
                .enabled(codex_features::Feature::OutputTokenReplay),
            /*attestation_provider*/ None,
            config.http_client_factory(),
            config.workspace_routing_context(),
            Vec::new(),
        );
        let mut metadata = CodexResponsesMetadata::new(
            installation_id.into(),
            mapped.session_id,
            mapped.thread_id,
            mapped.window_id,
        );
        metadata.turn_id.clone_from(&source.turn_id);
        metadata.parent_turn_id.clone_from(&source.parent_turn_id);
        metadata.root_turn_id.clone_from(&source.root_turn_id);
        metadata.parent_thread_id = mapped
            .parent_thread_id
            .as_deref()
            .map(ThreadId::from_string)
            .transpose()?;
        metadata.thread_source = if source.is_title() {
            Some(codex_protocol::protocol::ThreadSource::Feature(
                "thread_title".into(),
            ))
        } else {
            source.thread_source.as_deref().map(|value| {
                value.parse().unwrap_or_else(|_| {
                    codex_protocol::protocol::ThreadSource::Feature(value.into())
                })
            })
        };
        metadata.subagent_kind.clone_from(&source.subagent_kind);
        metadata.subagent_header = subagent;
        metadata.agent_name.clone_from(&source.agent_name);
        metadata.request_kind = Some(CodexResponsesRequestKind::Turn);
        Ok(Self {
            turn_state: Arc::new(OnceLock::new()),
            client_gate: Arc::new(tokio::sync::Mutex::new(())),
            client,
            metadata,
        })
    }

    pub fn mapping(&self) -> InferenceMapping {
        InferenceMapping {
            session_id: self.metadata.session_id.clone(),
            thread_id: self.metadata.thread_id.clone(),
            parent_thread_id: self.metadata.parent_thread_id.map(|id| id.to_string()),
            window_id: self.metadata.window_id.clone(),
            context_window_id: self.metadata.context_window_id.map(|id| id.to_string()),
            turn_id: self.metadata.turn_id.clone(),
            parent_turn_id: self.metadata.parent_turn_id.clone(),
            root_turn_id: self.metadata.root_turn_id.clone(),
        }
    }

    /// Native turn attribution for image requests, using the same mapped identity.
    pub fn turn_metadata_json(&self) -> Option<String> {
        self.metadata.turn_metadata_json()
    }

    pub fn turn_state(&self) -> Option<&str> {
        self.turn_state.get().map(String::as_str)
    }

    /// Share the logical thread's native cache and lease, but keep this turn's ticket.
    pub fn with_model_client(mut self, previous: &Self) -> Self {
        self.client = previous.client.clone();
        self.client_gate = previous.client_gate.clone();
        self
    }

    /// Snapshot caller-owned compaction progress without starting an Agent loop.
    pub fn with_window(&self, number: u64, context_window_id: Uuid) -> Self {
        let mut metadata = self.metadata.clone();
        metadata.window_id = format!("{}:{number}", metadata.thread_id);
        metadata.window_number = Some(number);
        metadata.context_window_id = Some(context_window_id);
        Self {
            client_gate: self.client_gate.clone(),
            client: self.client.clone(),
            turn_state: self.turn_state.clone(),
            metadata,
        }
    }
}
