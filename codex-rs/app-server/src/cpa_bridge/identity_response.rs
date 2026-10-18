//! Request-scoped inverse identities. Payload text, output items and opaque state are untouched.
use codex_core::InferenceIdentity;
use codex_core::InferenceMapping;
use serde_json::Map;
use serde_json::Value;

pub(super) struct IdentityResponse {
    source: InferenceIdentity,
    native: InferenceMapping,
}

impl IdentityResponse {
    pub(super) fn new(source: InferenceIdentity, native: InferenceMapping) -> Self {
        Self { source, native }
    }

    /// Only schema-owned envelope/metadata paths are visited, never arbitrary recursive JSON.
    pub(super) fn restore(&self, value: &mut Value) -> bool {
        let Some(object) = value.as_object_mut() else {
            return false;
        };
        let mut changed = self.fields(object);
        if let Some(metadata) = object
            .get_mut("client_metadata")
            .and_then(Value::as_object_mut)
        {
            changed |= self.fields(metadata);
        }
        if let Some(headers) = object.get_mut("headers").and_then(Value::as_object_mut) {
            changed |= self.fields(headers);
        }
        for key in ["response", "error"] {
            if let Some(child) = object.get_mut(key) {
                changed |= self.restore(child);
            }
        }
        changed
    }

    fn metadata(&self, value: &mut Value) -> bool {
        if let Some(values) = value.as_array_mut() {
            return values
                .iter_mut()
                .fold(false, |changed, value| self.metadata(value) | changed);
        }
        if let Some(text) = value.as_str()
            && let Ok(mut metadata) = serde_json::from_str::<Value>(text)
            && let Some(fields) = metadata.as_object_mut()
            && self.fields(fields)
        {
            *value = Value::String(metadata.to_string());
            return true;
        }
        false
    }

    pub(super) fn failure(&self, error: &mut codex_app_server_protocol::JSONRPCErrorError) {
        let Some(data) = error.data.as_mut().and_then(Value::as_object_mut) else {
            return;
        };
        let mut body_changed = false;
        if let Some(body) = data.get_mut("body")
            && let Some(text) = body.as_str()
            && let Ok(mut json) = serde_json::from_str::<Value>(text)
            && self.restore(&mut json)
        {
            *body = Value::String(json.to_string());
            body_changed = true;
        }
        if let Some(headers) = data.get_mut("headers").and_then(Value::as_object_mut) {
            self.fields(headers);
            if body_changed {
                headers.retain(|key, _| {
                    !matches!(
                        key.to_ascii_lowercase().as_str(),
                        "content-length" | "content-encoding" | "content-md5" | "digest" | "etag"
                    )
                });
            }
        }
    }

    pub(super) fn fields(&self, object: &mut Map<String, Value>) -> bool {
        let mut changed = false;
        object.retain(|key, value| {
            let normalized = key.to_ascii_lowercase();
            if normalized == "x-codex-turn-metadata" {
                changed |= self.metadata(value);
                return true;
            }
            let (native, source) = match normalized.as_str() {
                "session_id" | "sessionid" | "session-id" => (
                    Some(self.native.session_id.as_str()),
                    self.source.session_id.as_deref(),
                ),
                "thread_id" | "threadid" | "thread-id" => (
                    Some(self.native.thread_id.as_str()),
                    self.source.thread_id.as_deref(),
                ),
                "parent_thread_id" | "parentthreadid" | "x-codex-parent-thread-id" => (
                    self.native.parent_thread_id.as_deref(),
                    self.source.parent_thread_id.as_deref(),
                ),
                "prompt_cache_key" | "promptcachekey" => (
                    Some(self.native.session_id.as_str()),
                    self.source.prompt_cache_key.as_deref(),
                ),
                "turn_id" | "turnid" => (
                    self.native.turn_id.as_deref(),
                    self.source.turn_id.as_deref(),
                ),
                "parent_turn_id" | "parentturnid" => (
                    self.native.parent_turn_id.as_deref(),
                    self.source.parent_turn_id.as_deref(),
                ),
                "root_turn_id" | "rootturnid" => (
                    self.native.root_turn_id.as_deref(),
                    self.source.root_turn_id.as_deref(),
                ),
                "window_id" | "windowid" | "x-codex-window-id" => (
                    Some(self.native.window_id.as_str()),
                    self.source.window_id.as_deref(),
                ),
                "context_window_id" | "contextwindowid" => (
                    self.native.context_window_id.as_deref(),
                    self.source.context_window_id.as_deref(),
                ),
                _ => return true,
            };
            let Some(native) = native else {
                return true;
            };
            let mut rewrite = |value: &mut Value| {
                if value.as_str() != Some(native) {
                    return true;
                }
                match source {
                    Some(source) => {
                        if source != native {
                            *value = Value::String(source.to_owned());
                            changed = true;
                        }
                        true
                    }
                    None => {
                        changed = true;
                        false
                    }
                }
            };
            if let Some(values) = value.as_array_mut() {
                let was_empty = values.is_empty();
                values.retain_mut(&mut rewrite);
                was_empty || !values.is_empty()
            } else {
                rewrite(value)
            }
        });
        changed
    }
}

#[cfg(test)]
#[path = "identity_response_tests.rs"]
mod tests;
