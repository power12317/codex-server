//! Private source material paired within one actual HTTP exchange, never a later jar lookup.
use axum::http::HeaderMap;
use base64::Engine;
use chrono::DateTime;
use chrono::Utc;
use codex_app_server_protocol::CpaInferenceTicketNotification;
use codex_app_server_protocol::CpaTicketCookie;
use codex_app_server_protocol::CpaTicketRoute;
use serde_json::Value;

pub(super) struct TicketCapture {
    request_id: String,
    credential_id: String,
    model: String,
    cookies: [Option<CpaTicketCookie>; 2],
    state: Option<String>,
    last: Option<(String, CpaTicketRoute)>,
    observed_at: DateTime<Utc>,
    line: Vec<u8>,
    data: Vec<u8>,
    skip_event: bool,
    after_cr: bool,
}

const COOKIE_NAMES: [&str; 2] = ["__cflb", "__oailb"];
// A diagnostic parser limit only. All business bytes are forwarded unchanged.
const METADATA_LIMIT: usize = 64 * 1024;

impl TicketCapture {
    pub(super) fn new(
        request_id: String,
        credential_id: String,
        model: String,
        request: &HeaderMap,
    ) -> Self {
        let mut cookies = [None, None];
        let mut seen = [false, false];
        for part in request
            .get_all(axum::http::header::COOKIE)
            .iter()
            .filter_map(|header| header.to_str().ok())
            .flat_map(|header| header.split(';'))
        {
            if let Some((name, value)) = part.trim().split_once('=')
                && let Some(index) = COOKIE_NAMES.iter().position(|key| *key == name.trim())
            {
                // Ambiguous same-name cookies cannot establish a usable route.
                cookies[index] = (!seen[index]).then(|| cookie(value, None)).flatten();
                seen[index] = true;
            }
        }
        Self {
            request_id,
            credential_id,
            model,
            cookies,
            state: None,
            last: None,
            observed_at: Utc::now(),
            line: Vec::new(),
            data: Vec::new(),
            skip_event: false,
            after_cr: false,
        }
    }

    pub(super) fn response_headers(&mut self, headers: &HeaderMap, now: DateTime<Utc>) {
        self.observed_at = now;
        for value in headers.get_all(axum::http::header::SET_COOKIE) {
            if let Ok(value) = value.to_str() {
                self.set_cookie(value, now);
            }
        }
        if let Some(value) = headers.get("x-codex-turn-state") {
            self.state = value
                .to_str()
                .ok()
                .filter(|value| !value.trim().is_empty())
                .map(str::to_owned);
        }
    }

    pub(super) fn preserve_known_deadlines(
        &mut self,
        factory: &codex_http_client::HttpClientFactory,
    ) {
        for (name, cookie) in COOKIE_NAMES.iter().zip(&mut self.cookies) {
            if let Some(cookie) = cookie
                && let Some(deadline) = factory.chatgpt_routing_cookie_deadline(name, &cookie.value)
            {
                earliest_expiry(cookie, DateTime::<Utc>::from(deadline));
            }
        }
    }

    fn set_cookie(&mut self, raw: &str, now: DateTime<Utc>) {
        let mut parts = raw.split(';');
        let Some((name, value)) = parts.next().and_then(|part| part.trim().split_once('=')) else {
            return;
        };
        let Some(index) = COOKIE_NAMES.iter().position(|key| *key == name.trim()) else {
            return;
        };
        let mut max_age = None;
        let mut expires = None;
        let mut invalid = false;
        for part in parts {
            let Some((name, value)) = part.trim().split_once('=') else {
                continue;
            };
            if name.eq_ignore_ascii_case("max-age") {
                match value.trim().parse::<i64>() {
                    Ok(seconds) => max_age = Some(seconds),
                    Err(_) => invalid = true,
                }
            } else if name.eq_ignore_ascii_case("expires") {
                match httpdate::parse_http_date(value.trim()) {
                    Ok(time) => expires = Some(DateTime::<Utc>::from(time)),
                    Err(_) => invalid = true,
                }
            }
        }
        if let Some(seconds) = max_age {
            expires = chrono::Duration::try_seconds(seconds)
                .and_then(|duration| now.checked_add_signed(duration));
            invalid |= expires.is_none() || seconds <= 0;
        }
        let previous = self.cookies[index]
            .as_ref()
            .filter(|cookie| cookie.value == value.trim().trim_matches('"'))
            .and_then(|cookie| cookie.expires_at.as_deref())
            .and_then(|expiry| DateTime::parse_from_rfc3339(expiry).ok());
        self.cookies[index] = if invalid || expires.is_some_and(|expires| expires <= now) {
            None
        } else {
            cookie(value, expires.map(|expires| expires.to_rfc3339()))
        };
        if let (Some(cookie), Some(previous)) = (&mut self.cookies[index], previous) {
            earliest_expiry(cookie, previous.with_timezone(&Utc));
        }
    }

    pub(super) fn snapshot(
        &mut self,
        now: DateTime<Utc>,
    ) -> Option<CpaInferenceTicketNotification> {
        let state = self.state.as_ref()?;
        let route = CpaTicketRoute {
            cflb: self.cookies[0].clone()?,
            oailb: self.cookies[1].clone()?,
        };
        for cookie in [&route.cflb, &route.oailb] {
            if let Some(expires) = &cookie.expires_at
                && DateTime::parse_from_rfc3339(expires)
                    .ok()
                    .is_none_or(|expires| expires <= now)
            {
                return None;
            }
        }
        if self
            .last
            .as_ref()
            .is_some_and(|last| last == &(state.clone(), route.clone()))
        {
            return None;
        }
        self.last = Some((state.clone(), route.clone()));
        Some(CpaInferenceTicketNotification {
            request_id: self.request_id.clone(),
            credential_id: self.credential_id.clone(),
            model: self.model.clone(),
            observed_at: self.observed_at.to_rfc3339(),
            state: state.clone(),
            route,
        })
    }

    pub(super) fn push(&mut self, bytes: &[u8]) -> Vec<CpaInferenceTicketNotification> {
        let mut snapshots = Vec::new();
        for &byte in bytes {
            if self.after_cr && byte == b'\n' {
                self.after_cr = false;
                continue;
            }
            self.after_cr = byte == b'\r';
            if matches!(byte, b'\n' | b'\r') {
                if self.line.is_empty() {
                    if !self.skip_event {
                        self.metadata();
                        if let Some(snapshot) = self.snapshot(Utc::now()) {
                            snapshots.push(snapshot);
                        }
                    }
                    self.data.clear();
                    self.skip_event = false;
                } else if !self.skip_event {
                    if let Some(data) = self.line.strip_prefix(b"data:") {
                        if !self.data.is_empty() {
                            self.data.push(b'\n');
                        }
                        self.data
                            .extend_from_slice(data.strip_prefix(b" ").unwrap_or(data));
                    }
                    if self.data.len() > METADATA_LIMIT {
                        self.skip_event = true;
                        self.data.clear();
                    }
                }
                self.line.clear();
            } else if !self.skip_event {
                self.line.push(byte);
                if self.line.len() > METADATA_LIMIT {
                    self.skip_event = true;
                    self.line.truncate(1);
                }
            } else {
                // Preserve the distinction between a skipped line and an empty event delimiter.
                if self.line.is_empty() {
                    self.line.push(byte);
                }
            }
        }
        snapshots
    }

    fn metadata(&mut self) {
        let Ok(value) = serde_json::from_slice::<Value>(&self.data) else {
            return;
        };
        if !matches!(
            value["type"].as_str(),
            Some("response.metadata" | "codex.response.metadata")
        ) {
            return;
        }
        let Some(fields) = value.get("headers").and_then(Value::as_object) else {
            return;
        };
        let mut headers = HeaderMap::new();
        for (name, value) in fields {
            if !matches!(
                name.to_ascii_lowercase().as_str(),
                "x-codex-turn-state" | "set-cookie"
            ) {
                continue;
            }
            let values: Vec<&str> = match value {
                Value::String(value) => vec![value],
                Value::Array(values) => values.iter().filter_map(Value::as_str).collect(),
                _ => continue,
            };
            for value in values {
                if let (Ok(name), Ok(value)) = (
                    name.parse::<axum::http::HeaderName>(),
                    value.parse::<axum::http::HeaderValue>(),
                ) {
                    headers.append(name, value);
                }
            }
        }
        self.response_headers(&headers, Utc::now());
    }
}

fn cookie(value: &str, expires_at: Option<String>) -> Option<CpaTicketCookie> {
    let value = value.trim().trim_matches('"');
    (!value.is_empty()
        && value.bytes().all(
            |byte| matches!(byte, 0x21 | 0x23..=0x2b | 0x2d..=0x3a | 0x3c..=0x5b | 0x5d..=0x7e),
        ))
    .then(|| {
        let mut cookie = CpaTicketCookie {
            value: value.to_owned(),
            expires_at,
        };
        if let Some(payload) = value.split('.').nth(1)
            && let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload)
            && let Ok(claims) = serde_json::from_slice::<Value>(&bytes)
            && let Some(expiry) = claims
                .get("exp")
                .and_then(Value::as_i64)
                .and_then(|seconds| DateTime::from_timestamp(seconds, 0))
        {
            // An unverified claim may only shorten export lifetime, never establish identity.
            earliest_expiry(&mut cookie, expiry);
        }
        cookie
    })
}

fn earliest_expiry(cookie: &mut CpaTicketCookie, expiry: DateTime<Utc>) {
    let previous = cookie
        .expires_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc));
    cookie.expires_at = Some(
        previous
            .map_or(expiry, |previous| previous.min(expiry))
            .to_rfc3339(),
    );
}

#[cfg(test)]
#[path = "ticket_tests.rs"]
mod tests;
