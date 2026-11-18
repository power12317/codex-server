//! A peer size rejection is the sole model-only transport resend exception.
use super::*;
use anyhow::Context;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;
use tokio_tungstenite::accept_hdr_async_with_config;
use tokio_tungstenite::tungstenite::extensions::ExtensionsConfig;
use tokio_tungstenite::tungstenite::extensions::compression::deflate::DeflateConfig;
use tokio_tungstenite::tungstenite::handshake::server::Request;
use tokio_tungstenite::tungstenite::handshake::server::Response;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

#[derive(Default)]
struct Captured {
    upgrades: usize,
    websocket: Vec<Value>,
    http: Vec<Value>,
}

#[test_case::test_case(None, 1009, "", false, 200, 1, false, false; "empty_reason")]
#[test_case::test_case(None, 1009, "message exceeds limit", false, 200, 1, false, false; "nonempty_reason")]
#[test_case::test_case(None, 1009, "message exceeds limit", false, 503, 1, false, false; "failed_http_no_third_send")]
#[test_case::test_case(Some("{\"type\":\"response.created\",\"response\":{\"id\":\"r\"}}"), 1009, "", false, 200, 0, false, false; "created_before_close")]
#[test_case::test_case(Some("{\"type\":\"rate_limits.updated\",\"rate_limits\":[]}"), 1009, "", false, 200, 0, false, false; "metadata_before_close")]
#[test_case::test_case(Some("malformed text"), 1009, "", false, 200, 0, false, false; "malformed_text_before_close")]
#[test_case::test_case(None, 1000, "", false, 200, 0, false, false; "normal_close")]
#[test_case::test_case(None, 1011, "server error", false, 200, 0, false, false; "other_close")]
#[test_case::test_case(None, 1006, "", false, 200, 0, false, false; "disconnect")]
#[test_case::test_case(None, 1009, "", true, 200, 0, false, false; "local_size_limit")]
#[test_case::test_case(None, 1009, "", false, 200, 0, true, false; "prewarm_never_starts_generation")]
#[test_case::test_case(None, 1009, "", false, 200, 1, false, true; "ping_and_pong_do_not_count_as_text")]
#[tokio::test]
async fn cpa_native_peer_size_resend_boundaries(
    first_text: Option<&'static str>,
    close_code: u16,
    reason: &'static str,
    local_limit: bool,
    http_status: u16,
    expected_http: usize,
    prewarm_only: bool,
    control_frames: bool,
) -> Result<()> {
    let upstream = MockServer::start().await;
    let home = TempDir::new()?;
    prepare_account(&upstream, &home)?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let config_path = home.path().join("state/shared.json/config.toml");
    let mut config: toml::Value = toml::from_str(&std::fs::read_to_string(&config_path)?)?;
    let provider = &mut config["model_providers"]["mock_provider"];
    provider["base_url"] = toml::Value::String(format!("http://{address}/v1"));
    provider["supports_websockets"] = toml::Value::Boolean(true);
    provider["request_max_retries"] = toml::Value::Integer(5);
    provider["stream_max_retries"] = toml::Value::Integer(5);
    std::fs::write(config_path, toml::to_string(&config)?)?;

    let captured = Arc::new(Mutex::new(Captured::default()));
    let source_capture = captured.clone();
    let (stop, mut stopped) = oneshot::channel();
    let source = tokio::spawn(async move {
        let mut handlers = tokio::task::JoinSet::new();
        loop {
            let (mut socket, _) = tokio::select! {
                _ = &mut stopped => break,
                accepted = listener.accept() => accepted?,
            };
            let captured = source_capture.clone();
            handlers.spawn(async move {
                let mut buffer = vec![0; 65536];
                let size = loop {
                    let size = socket.peek(&mut buffer).await?;
                    anyhow::ensure!(size > 0, "empty request");
                    if buffer[..size].windows(4).any(|part| part == b"\r\n\r\n") {
                        break size;
                    }
                    tokio::task::yield_now().await;
                };
                let headers = std::str::from_utf8(&buffer[..size])?;
                if !headers.lines().next().unwrap_or_default().contains("/v1/responses") {
                    socket.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await?;
                    return Ok::<_, anyhow::Error>(());
                }
                if headers.starts_with("GET ") {
                    captured.lock().unwrap().upgrades += 1;
                    let mut extensions = ExtensionsConfig::default();
                    extensions.permessage_deflate = Some(DeflateConfig::default());
                    let mut ws_config = WebSocketConfig::default();
                    ws_config.extensions = extensions;
                    let mut ws = accept_hdr_async_with_config(socket, move |_: &Request, mut response: Response| {
                        if local_limit {
                            response.headers_mut().insert("x-codex-websocket-max-message-bytes", "1".parse().unwrap());
                        }
                        Ok(response)
                    }, Some(ws_config)).await?;
                    if local_limit {
                        // The client must reject locally before sending response.create.
                        if let Ok(Some(Ok(Message::Text(text)))) = timeout(Duration::from_secs(1), ws.next()).await {
                            captured.lock().unwrap().websocket.push(serde_json::from_str(&text)?);
                        }
                        return Ok(());
                    }
                    let request = ws.next().await.context("missing response.create")??;
                    let Message::Text(text) = request else { anyhow::bail!("expected text request") };
                    captured.lock().unwrap().websocket.push(serde_json::from_str(&text)?);
                    if control_frames {
                        ws.send(Message::Ping(vec![1].into())).await?;
                        ws.send(Message::Pong(vec![2].into())).await?;
                    }
                    if let Some(text) = first_text {
                        ws.send(Message::Text(text.into())).await?;
                    }
                    if close_code != 1006 {
                        ws.send(Message::Close(Some(CloseFrame { code: CloseCode::from(close_code), reason: reason.into() }))).await?;
                    }
                } else {
                    let mut bytes = Vec::new();
                    let (end, length) = loop {
                        let count = socket.read(&mut buffer).await?;
                        anyhow::ensure!(count > 0, "incomplete HTTP request");
                        bytes.extend_from_slice(&buffer[..count]);
                        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                            let length = std::str::from_utf8(&bytes[..end])?.lines().filter_map(|line| line.split_once(':')).find(|(key, _)| key.eq_ignore_ascii_case("content-length")).context("missing content length")?.1.trim().parse::<usize>()?;
                            if bytes.len() >= end + 4 + length { break (end, length); }
                        }
                    };
                    captured.lock().unwrap().http.push(serde_json::from_slice(&bytes[end + 4..end + 4 + length])?);
                    let body = if http_status == 200 { completed(json!({"id":"http-result","output":[]})) } else { "fallback failed".into() };
                    socket.write_all(format!("HTTP/1.1 {http_status} Test\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nRetry-After: 1\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await?;
                }
                Ok(())
            });
        }
        while let Some(result) = handlers.join_next().await {
            result??;
        }
        Ok::<_, anyhow::Error>(())
    });
    let _runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let mut input = model_input();
    if prewarm_only {
        input["generate"] = json!(false);
    }
    input["session_id"] = json!("source-session");
    input["thread_id"] = json!("source-child");
    input["turn_id"] = json!("source-turn");
    input["request_country"] = json!("US");
    input["tools"] = json!([{"type":"function","name":"external_tool","parameters":{"type":"object","properties":{"verbatim":{"type":"string","description":"unchanged"}}}}]);
    let outcome = exchange(&mut ws, rpc(input, 9009)).await?;
    let _ = stop.send(());
    timeout(Duration::from_secs(10), source).await???;
    let captured = captured.lock().unwrap();
    assert_eq!(captured.upgrades, 1, "never reconnect WebSocket");
    assert_eq!(captured.websocket.len(), usize::from(!local_limit));
    assert_eq!(
        captured.http.len(),
        expected_http,
        "only one permitted resend"
    );
    for sent in captured.websocket.iter().chain(&captured.http) {
        assert!(sent.get("request_country").is_none());
    }
    if expected_http == 1 {
        let original = &captured.websocket[0];
        let fallback = &captured.http[0];
        for key in [
            "input",
            "tools",
            "model",
            "instructions",
            "prompt_cache_key",
        ] {
            assert_eq!(original[key], fallback[key], "same native request {key}");
        }
        for key in ["session_id", "thread_id", "turn_id"] {
            assert_eq!(
                original["client_metadata"][key], fallback["client_metadata"][key],
                "same mapped identity {key}"
            );
        }
    }
    if expected_http == 1 && http_status == 200 {
        assert!(outcome.reply.get("result").is_some(), "{:?}", outcome.reply);
        assert!(outcome.error.is_none());
        assert!(String::from_utf8(outcome.body)?.contains("http-result"));
    } else {
        assert!(outcome.reply.get("error").is_some() || outcome.error.is_some());
        if expected_http == 1 {
            let failure = outcome
                .error
                .as_ref()
                .unwrap_or(&outcome.reply["error"]["data"]);
            assert_eq!(failure["httpStatus"], http_status);
            assert_eq!(failure["body"], "fallback failed");
        }
    }
    Ok(())
}
