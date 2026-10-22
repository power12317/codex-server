use super::*;
use anyhow::Context;
use pretty_assertions::assert_eq;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;

async fn http_request(socket: &mut TcpStream) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let count = socket.read(&mut buffer).await?;
        anyhow::ensure!(count != 0, "request ended before body");
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            let headers = std::str::from_utf8(&bytes[..end])?;
            let length = headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim().parse::<usize>())
                .transpose()?
                .unwrap_or_default();
            if bytes.len() >= end + 4 + length {
                return Ok(bytes);
            }
        }
    }
}

async fn accept_responses(listener: &tokio::net::TcpListener) -> Result<(TcpStream, Vec<u8>)> {
    loop {
        let (mut socket, _) = listener.accept().await?;
        let request = http_request(&mut socket).await?;
        if std::str::from_utf8(&request)?
            .lines()
            .next()
            .unwrap_or_default()
            .contains("/v1/responses")
        {
            return Ok((socket, request));
        }
        // Model discovery is independent of the gated inference response.
        socket
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await?;
    }
}

#[test_case::test_case("http")]
#[test_case::test_case("websocket")]
#[tokio::test]
async fn cpa_native_delivers_tool_deltas_and_ready_events_before_the_next_stage(
    transport: &str,
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
    provider["supports_websockets"] = toml::Value::Boolean(transport != "http");
    provider["stream_idle_timeout_ms"] = toml::Value::Integer(60_000);
    std::fs::write(config_path, toml::to_string(&config)?)?;
    let item = json!({"type":"custom_tool_call","id":"ct_stream","call_id":"call_stream","name":"exec","input":"stage-one stage-two"});
    let stages = vec![
        vec![
            json!({"type":"response.created","response":{"id":"resp_stream"}}),
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"custom_tool_call","id":"ct_stream","call_id":"call_stream","name":"exec","input":""}}),
            json!({"type":"response.custom_tool_call_input.delta","item_id":"ct_stream","output_index":0,"delta":"stage-one "}),
        ],
        vec![
            json!({"type":"response.custom_tool_call_input.delta","item_id":"ct_stream","output_index":0,"delta":"stage-two"}),
            json!({"type":"response.custom_tool_call_input.done","item_id":"ct_stream","output_index":0,"input":"stage-one stage-two"}),
            json!({"type":"response.output_item.done","output_index":0,"item":item}),
        ],
        vec![json!({"type":"response.completed","response":{"id":"resp_stream","output":[item]}})],
    ];
    let (next_tx, next_rx) = oneshot::channel();
    let (finish_tx, finish_rx) = oneshot::channel();
    let websocket = transport == "websocket";
    let source = tokio::spawn(async move {
        let (mut socket, request) = accept_responses(&listener).await?;
        let mut gates = vec![next_rx, finish_rx].into_iter();
        if websocket {
            let headers = std::str::from_utf8(&request)?;
            let key = headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("sec-websocket-key"))
                .context("WebSocket key")?
                .1
                .trim();
            let accept =
                tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes());
            socket.write_all(format!("HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {accept}\r\n\r\n").as_bytes()).await?;
            let mut ws = WebSocketStream::from_raw_socket(
                socket,
                tokio_tungstenite::tungstenite::protocol::Role::Server,
                None,
            )
            .await;
            let request = ws.next().await.context("response.create")??;
            let request: Value = serde_json::from_str(request.to_text()?)?;
            anyhow::ensure!(request["type"] == "response.create");
            for stage in stages {
                for event in stage {
                    ws.send(Message::Text(event.to_string().into())).await?;
                }
                if let Some(gate) = gates.next() {
                    gate.await?;
                }
            }
        } else {
            anyhow::ensure!(
                request.starts_with(b"POST "),
                "native HTTP response request expected"
            );
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await?;
            for stage in stages {
                for event in stage {
                    let event = format!("data: {event}\n\n");
                    socket
                        .write_all(format!("{:x}\r\n{event}\r\n", event.len()).as_bytes())
                        .await?;
                }
                if let Some(gate) = gates.next() {
                    gate.await?;
                }
            }
            socket.write_all(b"0\r\n\r\n").await?;
        }
        Ok::<_, anyhow::Error>(())
    });
    let _runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    send(&mut ws, rpc(json!({"model":"gpt-5.2","input":"fixture","stream":true,"tools":[{"type":"custom","name":"exec"}]}), 801)).await?;
    let mut events = Vec::new();
    let mut started = false;
    let mut completed = false;
    let mut diagnostics_done = false;
    for (expected_type, expected_delta, gate) in [
        (
            "response.custom_tool_call_input.delta",
            Some("stage-one "),
            next_tx,
        ),
        ("response.output_item.done", None, finish_tx),
    ] {
        timeout(Duration::from_secs(10), async {
            loop {
                let value = read_message(&mut ws).await?;
                if value["id"] == 801 {
                    assert_eq!(value["result"]["statusCode"], 200, "{value}");
                    started = true;
                }
                assert_ne!(value["method"], "cpa/inference/completed");
                assert_ne!(value["method"], "cpa/inference/error");
                if value["method"] == "cpa/inference/event" {
                    assert!(started, "start must precede business events");
                    assert!(value["params"].get("attemptId").is_none());
                    let event = decode_event(&value)?;
                    assert_ne!(
                        event["type"], "response.completed",
                        "native completion gate has not been released"
                    );
                    let reached = event["type"] == expected_type
                        && expected_delta.is_none_or(|delta| event["delta"] == delta);
                    events.push(event);
                    if reached {
                        break;
                    }
                }
            }
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        gate.send(())
            .expect("release only after the client received the preceding business stage");
    }
    timeout(Duration::from_secs(10), async {
        while !completed || !diagnostics_done {
            let value = read_message(&mut ws).await?;
            match value["method"].as_str() {
                Some("cpa/inference/event") => events.push(decode_event(&value)?),
                Some("cpa/inference/completed") => {
                    assert!(value["params"].get("bodyBase64").is_none());
                    completed = true;
                }
                Some("cpa/inference/diagnostics/completed") => diagnostics_done = true,
                Some("cpa/inference/error") => anyhow::bail!("{value}"),
                _ => {}
            }
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    assert_eq!(
        events
            .iter()
            .filter(|e| e["type"] == "response.output_item.done")
            .count(),
        1
    );
    assert_eq!(
        events.last().expect("terminal response event")["type"],
        "response.completed"
    );
    assert_eq!(
        events.last().expect("terminal response event")["response"]["output"][0]["call_id"],
        "call_stream"
    );
    source.await??;
    Ok(())
}

async fn read_message(ws: &mut Socket) -> Result<Value> {
    loop {
        let message = ws.next().await.context("IPC closed")??;
        if let Message::Text(text) = message {
            return Ok(serde_json::from_str(&text)?);
        }
    }
}

fn decode_event(message: &Value) -> Result<Value> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(
        message["params"]["bodyBase64"]
            .as_str()
            .context("event bytes")?,
    )?;
    let event = std::str::from_utf8(&bytes)?
        .trim()
        .strip_prefix("data: ")
        .context("SSE event")?;
    Ok(serde_json::from_str(event)?)
}

#[tokio::test]
async fn cpa_worker_loss_after_start_ends_business_and_diagnostics() -> Result<()> {
    let upstream = MockServer::start().await;
    let home = TempDir::new()?;
    prepare_account(&upstream, &home)?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let config_path = home.path().join("state/shared.json/config.toml");
    let mut config: toml::Value = toml::from_str(&std::fs::read_to_string(&config_path)?)?;
    config["model_providers"]["mock_provider"]["base_url"] =
        toml::Value::String(format!("http://{address}/v1"));
    config["model_providers"]["mock_provider"]["stream_idle_timeout_ms"] =
        toml::Value::Integer(60_000);
    std::fs::write(config_path, toml::to_string(&config)?)?;
    let source = tokio::spawn(async move {
        let (mut socket, _) = accept_responses(&listener).await?;
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await?;
        let event = "data: {\"type\":\"response.created\",\"response\":{\"id\":\"will_stop\"}}\n\n";
        socket
            .write_all(format!("{:x}\r\n{event}\r\n", event.len()).as_bytes())
            .await?;
        let mut tail = Vec::new();
        socket.read_to_end(&mut tail).await?;
        Ok::<_, anyhow::Error>(())
    });
    let _runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    send(&mut ws, rpc(model_input(), 811)).await?;
    timeout(Duration::from_secs(10), async {
        loop {
            let value = read_message(&mut ws).await?;
            if value["method"] == "cpa/inference/event" {
                break;
            }
            anyhow::ensure!(value.get("error").is_none(), "{value}");
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    let path = home.path().join("shared.json");
    let mut credential: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    credential["codex_cli"]["enabled"] = json!(false);
    std::fs::write(path, serde_json::to_vec(&credential)?)?;
    let mut error_seen = false;
    let mut diagnostics_done = false;
    timeout(Duration::from_secs(10), async {
        while !error_seen || !diagnostics_done {
            let value = read_message(&mut ws).await?;
            assert_ne!(value["method"], "cpa/inference/completed");
            if value["method"] == "cpa/inference/error" {
                assert!(
                    !error_seen,
                    "worker loss must not duplicate the final error"
                );
                error_seen = true;
            }
            diagnostics_done |= value["method"] == "cpa/inference/diagnostics/completed";
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    timeout(Duration::from_secs(10), source).await???;
    Ok(())
}

#[test_case::test_case("http", false)]
#[test_case::test_case("http", true)]
#[test_case::test_case("websocket", false)]
#[test_case::test_case("websocket", true)]
#[tokio::test]
async fn cpa_native_failed_event_returns_before_upstream_eof(
    transport: &str,
    after_start: bool,
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
    provider["supports_websockets"] = toml::Value::Boolean(transport == "websocket");
    provider["stream_idle_timeout_ms"] = toml::Value::Integer(60_000);
    provider["request_max_retries"] = toml::Value::Integer(5);
    provider["stream_max_retries"] = toml::Value::Integer(5);
    std::fs::write(config_path, toml::to_string(&config)?)?;
    let failure = json!({"type":"response.failed","response":{"id":"resp_failed","error":{"code":"rate_limit_exceeded","message":"original failed event","headers":{"retry-after":"11"}}}}).to_string();
    let original = failure.clone();
    let websocket = transport == "websocket";
    let (release, gate) = oneshot::channel();
    let source = tokio::spawn(async move {
        let (mut socket, request) = accept_responses(&listener).await?;
        let mut events = Vec::new();
        if after_start {
            events.push(
                json!({"type":"response.created","response":{"id":"resp_failed"}}).to_string(),
            );
        }
        events.push(failure);
        if websocket {
            let headers = std::str::from_utf8(&request)?;
            let key = headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("sec-websocket-key"))
                .context("WebSocket key")?
                .1
                .trim();
            let accept =
                tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes());
            socket.write_all(format!("HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {accept}\r\n\r\n").as_bytes()).await?;
            let mut ws = WebSocketStream::from_raw_socket(
                socket,
                tokio_tungstenite::tungstenite::protocol::Role::Server,
                None,
            )
            .await;
            let _request = ws.next().await.context("response.create")??;
            for event in events {
                ws.send(Message::Text(event.into())).await?;
            }
            gate.await?; // Deliberately keep the upstream response open until the error is delivered.
        } else {
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await?;
            for event in events {
                let event = format!("data: {event}\n\n");
                socket
                    .write_all(format!("{:x}\r\n{event}\r\n", event.len()).as_bytes())
                    .await?;
            }
            gate.await?; // No terminating chunk and no EOF before the assertion.
        }
        Ok::<_, anyhow::Error>(())
    });
    let _runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    send(
        &mut ws,
        rpc(
            json!({"model":"gpt-5.2","input":"fixture","stream":true}),
            1010,
        ),
    )
    .await?;
    let mut error = Value::Null;
    let mut diagnostics_done = false;
    let mut started = false;
    timeout(Duration::from_secs(10), async {
        while error.is_null() || !diagnostics_done {
            let message = read_message(&mut ws).await?;
            assert_ne!(message["method"], "cpa/inference/completed");
            if message["id"] == 1010 {
                if message.get("result").is_some() {
                    started = true;
                } else {
                    error = message["error"]["data"].clone();
                }
            }
            if message["method"] == "cpa/inference/error" {
                assert!(started);
                error = message["params"].clone();
            }
            diagnostics_done |= message["method"] == "cpa/inference/diagnostics/completed";
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    assert_eq!(started, after_start);
    assert_eq!(error["httpStatus"], 429);
    assert_eq!(error["body"], original);
    assert_eq!(error["headers"]["retry-after"], json!(["11"]));
    assert!(error.get("nativeError").is_some());
    release
        .send(())
        .expect("error must arrive while upstream remains open");
    source.await??;
    Ok(())
}

#[tokio::test]
async fn cpa_native_image_partial_precedes_completion_without_waiting_for_eof() -> Result<()> {
    let upstream = MockServer::start().await;
    let home = TempDir::new()?;
    prepare_account(&upstream, &home)?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let config_path = home.path().join("state/shared.json/config.toml");
    let mut config: toml::Value = toml::from_str(&std::fs::read_to_string(&config_path)?)?;
    config["model_providers"]["mock_provider"]["base_url"] =
        toml::Value::String(format!("http://{address}/v1"));
    std::fs::write(config_path, toml::to_string(&config)?)?;
    let (release, wait) = oneshot::channel();
    let (allow_eof, eof_gate) = oneshot::channel();
    let source = tokio::spawn(async move {
        let mut image_socket = loop {
            let (mut socket, _) = listener.accept().await?;
            let bytes = http_request(&mut socket).await?;
            if bytes.starts_with(b"POST /v1/images/generations ") {
                let start = bytes.windows(4).position(|v| v == b"\r\n\r\n").unwrap() + 4;
                let request: Value = serde_json::from_slice(&bytes[start..])?;
                anyhow::ensure!(request["stream"] == true && request["partial_images"] == 2);
                break socket;
            }
            socket
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await?;
        };
        image_socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await?;
        let partial = "data: {\"type\":\"image_generation.partial_image\",\"partial_image_index\":0,\"b64_json\":\"partial\"}\n\n";
        image_socket
            .write_all(format!("{:x}\r\n{partial}\r\n", partial.len()).as_bytes())
            .await?;
        wait.await?;
        let completed =
            "data: {\"type\":\"image_generation.completed\",\"b64_json\":\"complete\"}\n\n";
        image_socket
            .write_all(format!("{:x}\r\n{completed}\r\n", completed.len()).as_bytes())
            .await?;
        // Deliberately leave the HTTP body open. The native terminal image event
        // establishes success; a diagnostic tail or EOF is not a business gate.
        eof_gate.await?;
        Ok::<_, anyhow::Error>(())
    });
    let _runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let mut request = rpc(
        json!({"model":"gpt-image-2","prompt":"fixture","stream":true,"partial_images":2}),
        8300,
    );
    request["params"]["operation"] = json!("images/generations");
    request["params"]["imageApi"] = json!("images");
    send(&mut ws, request).await?;
    timeout(Duration::from_secs(10), async {
        loop {
            let message = read_message(&mut ws).await?;
            if message["method"] == "cpa/inference/event" {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(message["params"]["bodyBase64"].as_str().unwrap())?;
                anyhow::ensure!(
                    std::str::from_utf8(&bytes)?.contains("image_generation.partial_image")
                );
                break;
            }
            anyhow::ensure!(
                message["method"] != "cpa/inference/error" && message.get("error").is_none(),
                "{message}"
            );
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    release.send(()).unwrap();
    timeout(Duration::from_secs(10), async {
        loop {
            let message = read_message(&mut ws).await?;
            if message["method"] == "cpa/inference/completed" {
                break;
            }
            anyhow::ensure!(message["method"] != "cpa/inference/error", "{message}");
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    allow_eof.send(()).unwrap();
    source.await??;
    Ok(())
}

#[tokio::test]
async fn cpa_native_generate_false_uses_native_websocket_warmup() -> Result<()> {
    let upstream = MockServer::start().await;
    let home = TempDir::new()?;
    prepare_account(&upstream, &home)?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let config_path = home.path().join("state/shared.json/config.toml");
    let mut config: toml::Value = toml::from_str(&std::fs::read_to_string(&config_path)?)?;
    config["model_providers"]["mock_provider"]["base_url"] =
        toml::Value::String(format!("http://{address}/v1"));
    config["model_providers"]["mock_provider"]["supports_websockets"] = toml::Value::Boolean(true);
    std::fs::write(config_path, toml::to_string(&config)?)?;
    let source = tokio::spawn(async move {
        let (mut socket, headers) = accept_responses(&listener).await?;
        let headers = std::str::from_utf8(&headers)?;
        let key = headers
            .lines()
            .filter_map(|l| l.split_once(':'))
            .find(|(k, _)| k.eq_ignore_ascii_case("sec-websocket-key"))
            .context("websocket key")?
            .1
            .trim();
        let accept = tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes());
        socket.write_all(format!("HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {accept}\r\n\r\n").as_bytes()).await?;
        let mut ws = WebSocketStream::from_raw_socket(
            socket,
            tokio_tungstenite::tungstenite::protocol::Role::Server,
            None,
        )
        .await;
        let create = ws.next().await.context("warmup request")??;
        let create: Value = serde_json::from_str(create.to_text()?)?;
        anyhow::ensure!(
            create["generate"] == false,
            "warmup must not generate: {create}"
        );
        ws.send(Message::Text(
            json!({"type":"response.created","response":{"id":"warmup"}})
                .to_string()
                .into(),
        ))
        .await?;
        ws.send(Message::Text(
            json!({"type":"response.completed","response":{"id":"warmup","output":[]}})
                .to_string()
                .into(),
        ))
        .await?;
        Ok::<_, anyhow::Error>(())
    });
    let _runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let result = exchange(
        &mut ws,
        rpc(
            json!({"model":"gpt-5.2","input":"warmup only","generate":false}),
            8500,
        ),
    )
    .await?;
    assert_eq!(
        result.reply["result"]["statusCode"], 200,
        "{}",
        result.reply
    );
    assert!(result.error.is_none(), "{:?}", result.error);
    source.await??;
    Ok(())
}
