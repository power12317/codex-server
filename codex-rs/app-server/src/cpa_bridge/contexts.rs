//! Bounded worker-local contexts. Synthetic turns have a fixed one-hour lifetime.
use codex_core::InferenceIdentity;
use codex_core::InferenceSession;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

const TURN_TTL: Duration = Duration::from_secs(3600);
const MAX_CONTEXTS: usize = 1024;

#[derive(Clone, Hash, PartialEq, Eq)]
struct Key {
    scope: String,
    session: Option<String>,
    thread: Option<String>,
    turn: Option<String>,
    provenance: Provenance,
}

#[derive(Clone, Default, Hash, PartialEq, Eq, serde::Serialize)]
struct Provenance {
    parent_thread: Option<String>,
    thread_source: Option<String>,
    subagent_kind: Option<String>,
    subagent_header: Option<String>,
    agent_name: Option<String>,
    parent_turn: Option<String>,
    root_turn: Option<String>,
}

struct Entry<T> {
    created: Instant,
    last_used: Instant,
    value: Arc<T>,
}

pub(super) struct Cache<T> {
    turns: HashMap<Key, Entry<T>>,
}

impl<T> Default for Cache<T> {
    fn default() -> Self {
        Self {
            turns: HashMap::new(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Inference context capacity reached; retry after an existing context expires")]
pub(super) struct CapacityExceeded;

impl<T> Cache<T> {
    fn get_or_insert(
        &mut self,
        key: Key,
        now: Instant,
        create: impl FnOnce() -> anyhow::Result<T>,
    ) -> anyhow::Result<Arc<T>> {
        self.turns.retain(|key, entry| {
            let since = if key.turn.is_some() {
                entry.last_used
            } else {
                entry.created
            };
            now.duration_since(since) < TURN_TTL
        });
        if let Some(entry) = self.turns.get_mut(&key) {
            entry.last_used = now;
            return Ok(entry.value.clone());
        }
        // Never evict a fresh turn and silently lose its routing state under load.
        if self.turns.len() >= MAX_CONTEXTS {
            return Err(CapacityExceeded.into());
        }
        let value = Arc::new(create()?);
        self.turns.insert(
            key,
            Entry {
                created: now,
                last_used: now,
                value: value.clone(),
            },
        );
        Ok(value)
    }
}

#[derive(Default)]
pub(super) struct Contexts {
    turns: Cache<InferenceSession>,
    clients: HashMap<String, (Instant, codex_core::ModelClient)>,
}

impl Contexts {
    pub(super) fn get(
        &mut self,
        bridge: &super::CpaBridge,
        params: &codex_app_server_protocol::CpaInferenceStartParams,
    ) -> anyhow::Result<Arc<InferenceSession>> {
        let mut source = InferenceIdentity::from_request(&params.request);
        let scope = serde_json::to_string(&(&params.credential_id, &params.session_id))?;
        // Validate even cache hits; caller identity values may later be projected into responses.
        source.map(&scope)?;
        let key = Key {
            scope: params.session_id.clone(),
            session: source.session_key().map(str::to_owned),
            thread: source.thread_key().map(str::to_owned),
            turn: source.turn_id.clone(),
            provenance: Provenance {
                parent_thread: source.parent_thread_id.clone(),
                thread_source: source.thread_source.clone(),
                subagent_kind: source.subagent_kind.clone(),
                subagent_header: source.subagent_header.clone(),
                agent_name: source.agent_name.clone(),
                parent_turn: source.parent_turn_id.clone(),
                root_turn: source.root_turn_id.clone(),
            },
        };
        let title = source.is_title();
        let mapped = source.map(&scope)?;
        let client_key = serde_json::to_string(&(
            &scope,
            &mapped.thread_id,
            &key.provenance.parent_thread,
            &key.provenance.thread_source,
            &key.provenance.subagent_kind,
            &key.provenance.subagent_header,
            &key.provenance.agent_name,
        ))?;
        let mut create = || {
            source
                .turn_id
                .get_or_insert_with(|| uuid::Uuid::now_v7().to_string());
            InferenceSession::new(
                &bridge.config,
                bridge.auth.clone(),
                &bridge.installation_id,
                &scope,
                &source,
            )
        };
        // Titles associate with the same logical IDs but never retain a turn context.
        if title {
            return create().map(Arc::new);
        }
        let now = Instant::now();
        self.clients
            .retain(|_, (last_used, _)| now.duration_since(*last_used) < TURN_TTL);
        let existing = self
            .clients
            .get_mut(&client_key)
            .map(|(last_used, client)| {
                *last_used = now;
                client.clone()
            });
        let value = self.turns.get_or_insert(key, now, || {
            let value = create()?;
            Ok(match existing {
                Some(client) => value.with_model_client(client),
                None => value,
            })
        })?;
        self.clients.insert(client_key, (now, value.model_client()));
        Ok(value)
    }
}

#[cfg(test)]
#[path = "contexts_tests.rs"]
mod tests;
