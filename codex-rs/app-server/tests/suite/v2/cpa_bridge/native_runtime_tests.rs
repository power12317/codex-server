use super::*;
use base64::Engine;
use pretty_assertions::assert_eq;

#[derive(Default)]
struct Exchange {
    reply: Value,
    body: Vec<u8>,
    logs: Vec<Value>,
}

async fn exchange(ws: &mut Socket, request: Value) -> Result<Exchange> {
    let id = request["id"].clone();
    let request_id = request["params"]["requestId"].clone();
    send(ws, request).await?;
    timeout(Duration::from_secs(30), async {
        let mut exchange = Exchange::default();
        while let Some(message) = ws.next().await {
            let Message::Text(text) = message? else {
                continue;
            };
            let message: Value = serde_json::from_str(&text)?;
            if message["id"] == id {
                exchange.reply = message;
                if exchange.reply.get("error").is_some() {
                    return Ok(exchange);
                }
                continue;
            }
            if message["params"]["requestId"] != request_id {
                continue;
            }
            match message["method"].as_str() {
                Some("cpa/inference/upstream") => exchange.logs.push(message["params"].clone()),
                Some("cpa/inference/completed") => {
                    exchange.body = base64::engine::general_purpose::STANDARD.decode(
                        message["params"]["bodyBase64"]
                            .as_str()
                            .expect("atomic final bytes"),
                    )?;
                    return Ok(exchange);
                }
                Some("cpa/inference/error") => {
                    anyhow::bail!("unexpected non-RPC final error: {message}")
                }
                _ => anyhow::bail!("unexpected inference notification: {message}"),
            }
        }
        anyhow::bail!("connection closed before native outcome")
    })
    .await?
}

fn completed(response: Value) -> String {
    format!(
        "data: {}\n\n",
        json!({"type":"response.completed","response":response})
    )
}
fn wire(exchange: &Exchange) -> Value {
    let log = exchange
        .logs
        .iter()
        .find(|log| log["kind"] == "request" && log["method"] == "POST")
        .unwrap();
    serde_json::from_str(log["body"].as_str().unwrap()).unwrap()
}
fn rpc(input: Value, id: u64) -> Value {
    let mut request = start(input);
    request["id"] = json!(id);
    request["params"]["requestId"] = json!(format!("r-{id}"));
    request
}
fn model_input() -> Value {
    json!({"model":"mock-model","input":"fixture","stream":true})
}

#[tokio::test]
async fn cpa_native_completion_preserves_tools_extensions_and_identity() -> Result<()> {
    let upstream = MockServer::start().await;
    let home = TempDir::new()?;
    let marker = home.path().join("must-not-execute");
    let command = format!("touch {}", marker.display());
    Mock::given(method("POST")).and(path("/v1/responses")).respond_with(move |request:&wiremock::Request| {
        let body:Value=request.body_json().unwrap();let metadata=&body["client_metadata"];
        let items=vec![json!({"type":"function_call","id":"fc_fixture","call_id":"call_fixture","name":"shell","arguments":json!({"command":command}).to_string()})];
        let events=vec![json!({"type":"response.created","response":{"id":"resp_opaque"}}),
            json!({"type":"response.function_call_arguments.delta","delta":"original arguments","call_id":"call_fixture"}),
            json!({"type":"response.future_extension","payload":{"keep":[1,true]}}),
            json!({"type":"response.output_item.done","item":items[0]}),
            json!({"type":"response.completed","response":{"id":"resp_opaque","session_id":metadata["session_id"],"thread_id":metadata["thread_id"],"prompt_cache_key":body["prompt_cache_key"],"output":items,"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2,"extension":7}}})];
        ResponseTemplate::new(200).insert_header("session-id",metadata["session_id"].as_str().unwrap())
            .set_body_raw(events.iter().map(|event|format!("data: {event}\n\n")).collect::<String>(),"text/event-stream")
    }).expect(1).mount(&upstream).await;
    let _runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let mut input = model_input();
    input["session_id"] = json!("source-session");
    input["thread_id"] = json!("source-child");
    input["prompt_cache_key"] = json!("source-cache");
    input["tools"] = json!([{"type":"function","name":"shell","parameters":{"type":"object"}},{"type":"function","name":"exec","parameters":{"type":"object"}},{"type":"function","name":"apply_patch","parameters":{"type":"object"}}]);
    let result = exchange(&mut ws, rpc(input, 101)).await?;
    assert_eq!(
        result.reply["result"]["statusCode"], 200,
        "{}",
        result.reply
    );
    let body = String::from_utf8(result.body)?;
    assert!(body.contains("response.function_call_arguments.delta"));
    assert!(body.contains("response.future_extension"));
    assert!(body.contains("\"extension\":7"));
    let final_event: Value = serde_json::from_str(
        body.lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .last()
            .unwrap(),
    )?;
    assert_eq!(final_event["response"]["session_id"], "source-session");
    assert_eq!(final_event["response"]["thread_id"], "source-child");
    assert_eq!(final_event["response"]["prompt_cache_key"], "source-cache");
    assert_eq!(final_event["response"]["id"], "resp_opaque");
    assert_eq!(
        final_event["response"]["output"][0]["call_id"],
        "call_fixture"
    );
    assert!(!marker.exists());
    for log in &result.logs {
        assert_eq!(log["transport"], "http");
        if log["kind"] == "request" {
            assert_eq!(log["headers"]["authorization"], json!(["[REDACTED]"]));
        }
    }
    assert!(result.logs.iter().any(|log| log["kind"] == "body"));
    let requests = upstream.received_requests().await.unwrap();
    let request = requests
        .iter()
        .find(|r| r.url.path() == "/v1/responses")
        .unwrap();
    assert_eq!(request.headers["originator"], "codex-tui");
    assert!(
        request.headers["user-agent"]
            .to_str()?
            .starts_with(concat!("codex-tui/", env!("CARGO_PKG_VERSION")))
    );
    Ok(())
}

#[tokio::test]
async fn cpa_native_turn_state_hierarchy_titles_and_no_persisted_conversation() -> Result<()> {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-codex-turn-state", "native-route")
                .set_body_raw(completed(json!({"id":"resp_state"})), "text/event-stream"),
        )
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    let _runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let mut input = model_input();
    input["session_id"] = json!("root");
    input["thread_id"] = json!("root");
    let first = exchange(&mut ws, rpc(input.clone(), 110)).await?;
    let second = exchange(&mut ws, rpc(input.clone(), 111)).await?;
    let first_wire = wire(&first);
    let second_wire = wire(&second);
    assert_eq!(
        first_wire["client_metadata"]["turn_id"],
        second_wire["client_metadata"]["turn_id"]
    );
    assert_eq!(
        second.logs.iter().find(|l| l["kind"] == "request").unwrap()["headers"]["x-codex-turn-state"],
        json!(["native-route"])
    );
    input["thread_id"] = json!("child");
    input["parent_thread_id"] = json!("root");
    let child = wire(&exchange(&mut ws, rpc(input.clone(), 112)).await?);
    assert_eq!(
        child["client_metadata"]["session_id"],
        first_wire["client_metadata"]["session_id"]
    );
    assert_ne!(
        child["client_metadata"]["thread_id"],
        child["client_metadata"]["session_id"]
    );
    let child_metadata: Value = serde_json::from_str(
        child["client_metadata"]["x-codex-turn-metadata"]
            .as_str()
            .unwrap(),
    )?;
    assert_eq!(
        child_metadata["parent_thread_id"],
        first_wire["client_metadata"]["thread_id"]
    );
    input["thread_source"] = json!("thread_title");
    let title_a = wire(&exchange(&mut ws, rpc(input.clone(), 113)).await?);
    let title_b = wire(&exchange(&mut ws, rpc(input, 114)).await?);
    assert_ne!(
        title_a["client_metadata"]["turn_id"],
        title_b["client_metadata"]["turn_id"]
    );
    let sessions = home.path().join("state/shared.json/sessions");
    if sessions.exists() {
        assert_eq!(std::fs::read_dir(sessions)?.count(), 0);
    }
    Ok(())
}

#[tokio::test]
async fn cpa_native_rejects_unexpressible_fields_without_model_requests() -> Result<()> {
    let upstream = MockServer::start().await;
    let home = TempDir::new()?;
    let _runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    for (index, (key, value)) in [
        ("extension", json!(true)),
        ("tool_choice", json!("required")),
        ("previous_response_id", json!("old")),
        ("background", json!(true)),
        ("reasoning", json!({"future":1})),
        (
            "input",
            json!([{"type":"message","role":"user","content":[],"future":1}]),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let mut input = model_input();
        input[key] = value;
        let result = exchange(&mut ws, rpc(input, 120 + index as u64)).await?;
        assert_eq!(
            result.reply["error"]["data"]["httpStatus"], 400,
            "{key}: {}",
            result.reply
        );
        assert!(result.reply["error"]["data"]["body"].is_string());
        assert!(result.body.is_empty());
    }
    assert!(
        !upstream
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|r| r.url.path().ends_with("/responses"))
    );
    Ok(())
}

#[tokio::test]
async fn cpa_native_eof_is_failure_and_internal_errors_are_diagnostics() -> Result<()> {
    for (body, success) in [
        (
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"incomplete\"}}\n\n"
                .to_owned(),
            false,
        ),
        (
            format!(
                "data: {{\"type\":\"response.failed\",\"response\":{{\"id\":\"internal\",\"error\":{{\"code\":\"server_error\",\"message\":\"internal recovery\"}}}}}}\n\n{}",
                completed(json!({"id":"success"}))
            ),
            true,
        ),
    ] {
        let upstream = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
            .mount(&upstream)
            .await;
        let home = TempDir::new()?;
        let _runtime = server(&upstream, &home).await?;
        let mut ws = connect(&home).await?;
        let result = exchange(&mut ws, rpc(model_input(), 140)).await?;
        if success {
            assert_eq!(result.reply["result"]["statusCode"], 200);
            assert!(!String::from_utf8(result.body)?.contains("response.failed"));
        } else {
            assert!(result.reply.get("error").is_some(), "{}", result.reply);
            assert!(result.body.is_empty());
        }
        assert!(result.logs.iter().any(|log| log["kind"] == "body"));
    }
    Ok(())
}

#[tokio::test]
async fn cpa_native_images_dto_and_atomic_json_delivery() -> Result<()> {
    let upstream = MockServer::start().await;
    let output = json!({"created":1,"data":[{"b64_json":"aW1hZ2U=","generation_id":"image-1"}],"future":{"keep":true}});
    for operation in ["generations", "edits"] {
        Mock::given(method("POST"))
            .and(path(format!("/v1/images/{operation}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(output.clone()))
            .mount(&upstream)
            .await;
    }
    let home = TempDir::new()?;
    let _runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    for (index, (operation, stream)) in [
        ("generations", false),
        ("generations", true),
        ("edits", false),
        ("edits", true),
    ]
    .into_iter()
    .enumerate()
    {
        let mut input = json!({"model":"gpt-image-2","prompt":"fixture","stream":stream});
        if operation == "edits" {
            input["images"] = json!([{"image_url":"data:image/png;base64,aW1hZ2U="}]);
        }
        let mut request = rpc(input, 150 + index as u64);
        request["params"]["operation"] = json!(format!("images/{operation}"));
        request["params"]["imageApi"] = json!("images");
        let result = exchange(&mut ws, request).await?;
        assert_eq!(
            result.reply["result"]["statusCode"], 200,
            "{}",
            result.reply
        );
        assert_eq!(serde_json::from_slice::<Value>(&result.body)?, output);
        let body = wire(&result);
        assert!(body.get("stream").is_none());
        assert!(body.get("n").is_none());
    }
    for key in [
        "mask",
        "partial_images",
        "output_format",
        "output_compression",
        "extension",
    ] {
        let mut request = rpc(
            json!({"model":"gpt-image-2","prompt":"fixture",key:"unsupported"}),
            159,
        );
        request["params"]["operation"] = json!("images/generations");
        request["params"]["imageApi"] = json!("images");
        let result = exchange(&mut ws, request).await?;
        assert_eq!(result.reply["error"]["data"]["httpStatus"], 400);
        assert!(result.logs.is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn cpa_native_replaces_time_context_in_place() -> Result<()> {
    for timezone in ["Asia/Tokyo", "Asia/Singapore"] {
        let upstream = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(completed(json!({"id":"time"})), "text/event-stream"),
            )
            .mount(&upstream)
            .await;
        let home = TempDir::new()?;
        prepare_account(&upstream, &home)?;
        let _runtime = launch_with_timezone(&upstream, &home, Some(timezone)).await?;
        let mut ws = connect(&home).await?;
        let result=exchange(&mut ws,rpc(json!({"model":"mock-model","stream":true,"input":[{"role":"user","content":"<environment_context>\n<cwd>/caller/path</cwd>\n<current_date>2000-01-01</current_date>\n<timezone>UTC</timezone>\n</environment_context>"}]}),170)).await?;
        let body = wire(&result).to_string();
        assert_eq!(body.matches("<environment_context>").count(), 1);
        assert!(body.contains(&format!("<timezone>{timezone}</timezone>")));
        assert!(!body.contains("2000-01-01"));
        assert!(body.contains("/caller/path"));
    }
    Ok(())
}

#[tokio::test]
async fn cpa_native_concurrent_source_and_credential_isolation() -> Result<()> {
    let upstream = MockServer::start().await;
    Mock::given(method("POST")).and(path("/v1/responses")).respond_with(|request:&wiremock::Request| {
        let body:Value=request.body_json().unwrap();let metadata=&body["client_metadata"];
        ResponseTemplate::new(200).set_delay(Duration::from_millis(100)).set_body_raw(completed(json!({"id":"isolated","session_id":metadata["session_id"],"thread_id":metadata["thread_id"]})),"text/event-stream")
    }).mount(&upstream).await;
    let home = TempDir::new()?;
    prepare_account(&upstream, &home)?;
    let mut second: Value =
        serde_json::from_slice(&std::fs::read(home.path().join("shared.json"))?)?;
    second["access_token"] = json!("second-worker-access");
    std::fs::write(
        home.path().join("second.json"),
        serde_json::to_vec(&second)?,
    )?;
    std::fs::create_dir_all(home.path().join("state/second.json"))?;
    std::fs::copy(
        home.path().join("config.toml"),
        home.path().join("state/second.json/config.toml"),
    )?;
    let _runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    for (index, (credential, source, scope)) in [
        ("shared.json", "A", "scope"),
        ("shared.json", "B", "scope"),
        ("shared.json", "A", "other-scope"),
        ("second.json", "A", "scope"),
    ]
    .into_iter()
    .enumerate()
    {
        let mut input = model_input();
        input["session_id"] = json!(source);
        input["thread_id"] = json!(source);
        let mut request = rpc(input, 180 + index as u64);
        request["params"]["credentialId"] = json!(credential);
        request["params"]["sessionId"] = json!(scope);
        send(&mut ws, request).await?;
    }
    let mut done = std::collections::BTreeMap::new();
    let mut mapped = std::collections::BTreeMap::new();
    timeout(Duration::from_secs(30), async {
        while done.len() < 4 {
            let Some(Ok(Message::Text(text))) = ws.next().await else {
                continue;
            };
            let message: Value = serde_json::from_str(&text)?;
            anyhow::ensure!(message.get("error").is_none(), "{message}");
            let id = message["params"]["requestId"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            if message["method"] == "cpa/inference/upstream"
                && message["params"]["kind"] == "request"
            {
                let body: Value =
                    serde_json::from_str(message["params"]["body"].as_str().unwrap())?;
                mapped.insert(id, body["client_metadata"]["session_id"].clone());
            } else if message["method"] == "cpa/inference/completed" {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(message["params"]["bodyBase64"].as_str().unwrap())?;
                let event: Value = serde_json::from_str(
                    std::str::from_utf8(&bytes)?
                        .trim()
                        .strip_prefix("data: ")
                        .unwrap(),
                )?;
                done.insert(id, event["response"]["session_id"].clone());
            }
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    assert_eq!(done["r-180"], "A");
    assert_eq!(done["r-181"], "B");
    assert_eq!(done["r-182"], "A");
    assert_eq!(done["r-183"], "A");
    let native: std::collections::BTreeSet<_> = mapped.values().map(Value::to_string).collect();
    assert_eq!(native.len(), 4);
    let requests = upstream.received_requests().await.unwrap();
    let auth: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path() == "/v1/responses")
        .map(|r| r.headers["authorization"].to_str().unwrap())
        .collect();
    assert_eq!(
        auth.iter()
            .filter(|v| **v == "Bearer second-worker-access")
            .count(),
        1
    );
    assert_eq!(auth.len(), 4);
    Ok(())
}
