//! Current runtime time context for CPA requests; history and caller paths stay intact.
use chrono::Local;
use serde_json::Value;

pub(super) struct TimeContext {
    pub(super) date: String,
    pub(super) timezone: String,
}

impl TimeContext {
    pub(super) fn now() -> Self {
        let now = Local::now();
        let utc_offset = now.format("%:z").to_string();
        #[cfg(unix)]
        let configured = std::env::var("TZ").ok().filter(|value| !value.is_empty());
        // Windows local time follows OS settings, not the POSIX TZ variable.
        #[cfg(not(unix))]
        let configured: Option<String> = None;
        let candidate = configured
            .clone()
            .or_else(|| iana_time_zone::get_timezone().ok());
        let timezone = candidate
            .map(|value| value.strip_prefix(':').unwrap_or(&value).to_owned())
            .filter(|value| {
                !value.is_empty()
                    && value.len() <= 128
                    && value
                        .split('/')
                        .all(|part| !matches!(part, "" | "." | ".."))
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"/_+-".contains(&byte))
            })
            .filter(|value| {
                #[cfg(unix)]
                {
                    let zone =
                        std::fs::read(std::path::Path::new("/usr/share/zoneinfo").join(value));
                    if configured.is_some() {
                        zone.is_ok()
                    } else {
                        // /etc/timezone can be stale when only /etc/localtime is mounted.
                        zone.ok()
                            .zip(std::fs::read("/etc/localtime").ok())
                            .is_some_and(|(zone, local)| zone == local)
                    }
                }
                #[cfg(not(unix))]
                {
                    let _ = value;
                    true
                }
            })
            // Preserve the actual system offset when its IANA name is unavailable.
            .unwrap_or_else(|| format!("UTC{utc_offset}"));
        Self {
            date: now.format("%Y-%m-%d").to_string(),
            timezone,
        }
    }

    pub(super) fn apply(&self, input: &mut [Value]) {
        // A declaration stays current across assistant/tool continuations until a newer
        // declaration of the same kind replaces it. Never append a synthetic user message.
        let mut seen = [false; 2];
        for item in input.iter_mut().rev() {
            if !matches!(item["role"].as_str(), Some("user" | "developer" | "system")) {
                continue;
            }
            if let Some(content) = item["content"].as_array_mut() {
                for part in content.iter_mut().rev() {
                    if part["type"] != "input_text" {
                        continue;
                    }
                    for (index, tag) in ["environment_context", "codex_apps_client_time_context"]
                        .into_iter()
                        .enumerate()
                    {
                        if !seen[index]
                            && let Some(text) = part["text"].as_str()
                            && let Some(updated) = self.rewrite_block(text, tag)
                        {
                            part["text"] = Value::String(updated);
                            seen[index] = true;
                        }
                    }
                }
            }
        }
    }

    fn rewrite_block(&self, text: &str, tag: &str) -> Option<String> {
        let trimmed = text.trim();
        let opening = format!("<{tag}>");
        let closing = format!("</{tag}>");
        let inner = trimmed.strip_prefix(&opening)?.strip_suffix(&closing)?;
        // Only standalone protocol contexts are owned here, not quoted examples or code.
        if inner.contains("```") || inner.contains("~~~") || inner.contains(&opening) {
            return None;
        }
        let mut inner = inner.to_owned();
        for (field, value) in [("current_date", &self.date), ("timezone", &self.timezone)] {
            let open = format!("<{field}>");
            let close = format!("</{field}>");
            if let Some(start) = inner.find(&open) {
                let end = start + open.len() + inner[start + open.len()..].find(&close)?;
                if inner[end + close.len()..].contains(&open) {
                    return None;
                }
                inner.replace_range(start + open.len()..end, value);
            }
            // A missing field stays missing; replacement never inserts context content.
        }
        let start = text.len() - text.trim_start().len();
        let end = text.trim_end().len();
        Some(format!(
            "{}{opening}{inner}{closing}{}",
            &text[..start],
            &text[end..]
        ))
    }
}

#[cfg(test)]
#[path = "inference_context_tests.rs"]
mod tests;
