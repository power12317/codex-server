use super::*;
use base64::Engine;
use pretty_assertions::assert_eq;

#[derive(Default)]
struct Exchange {
    reply: Value,
    body: Vec<u8>,
    error: Option<Value>,
    logs: Vec<Value>,
}

async fn exchange(ws: &mut Socket, request: Value) -> Result<Exchange> {
    let id = request["id"].clone();
    let request_id = request["params"]["requestId"].clone();
    send(ws, request).await?;
    timeout(Duration::from_secs(30), async {
        let mut exchange = Exchange::default();
        let mut business_done = false;
        let mut diagnostics_done = false;
        while let Some(message) = ws.next().await {
            let Message::Text(text) = message? else {
                continue;
            };
            let message: Value = serde_json::from_str(&text)?;
            if message.get("id") == Some(&id) {
                exchange.reply = message;
                business_done |= exchange.reply.get("error").is_some();
            } else if message["params"]["requestId"] == request_id {
                match message["method"].as_str() {
                    Some("cpa/inference/upstream") => exchange.logs.push(message["params"].clone()),
                    Some("cpa/inference/event") => {
                        assert!(message["params"].get("attemptId").is_none());
                        exchange.body.extend(
                            base64::engine::general_purpose::STANDARD.decode(
                                message["params"]["bodyBase64"]
                                    .as_str()
                                    .expect("event bytes"),
                            )?,
                        );
                    }
                    Some("cpa/inference/completed") => {
                        assert!(message["params"].get("bodyBase64").is_none());
                        business_done = true;
                    }
                    Some("cpa/inference/error") => {
                        exchange.error = Some(message["params"].clone());
                        business_done = true;
                    }
                    Some("cpa/inference/diagnostics/completed") => diagnostics_done = true,
                    _ => anyhow::bail!("unexpected inference notification: {message}"),
                }
            }
            if business_done && diagnostics_done {
                return Ok(exchange);
            }
        }
        anyhow::bail!("connection closed before business and diagnostics ended")
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
        .expect("native request diagnostic");
    serde_json::from_str(log["body"].as_str().expect("request JSON text")).expect("request JSON")
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
async fn cpa_native_required_source_ticket_fails_without_model_send() -> Result<()> {
    let upstream = MockServer::start().await;
    let home = TempDir::new()?;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&upstream)
        .await;
    let _runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let mut request = rpc(model_input(), 201);
    request["params"]["captureTicketSnapshot"] = json!(true);
    let result = exchange(&mut ws, request).await?;
    assert_eq!(result.reply["error"]["data"]["httpStatus"], json!(400));
    assert!(
        result.reply["error"]["message"]
            .as_str()
            .expect("error message")
            .contains("source ticket snapshots")
    );
    assert!(result.body.is_empty());
    assert!(result.error.is_none());
    assert!(result.logs.iter().all(|log| log["kind"] != "request"));
    Ok(())
}

#[tokio::test]
async fn cpa_native_analytics_reuses_sender_without_extra_model_request() -> Result<()> {
    let upstream = MockServer::start().await;
    let home = TempDir::new()?;
    Mock::given(method("POST")).and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(completed(json!({
            "id":"upstream-result","output":[],"usage":{"input_tokens":11,"output_tokens":2,"total_tokens":13}
        })), "text/event-stream"))
        .expect(1).mount(&upstream).await;
    Mock::given(method("POST"))
        .and(path("/codex/analytics-events/events"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&upstream)
        .await;
    let _runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let mut input = model_input();
    input["session_id"] = json!("original-cpa-session");
    input["thread_id"] = json!("original-cpa-thread");
    let result = exchange(&mut ws, rpc(input, 9901)).await?;
    let business_completed = tokio::time::Instant::now();
    assert_eq!(result.reply["result"]["statusCode"], 200);
    let final_request = wire(&result);
    let final_thread = &final_request["client_metadata"]["thread_id"];
    assert_ne!(final_thread, "original-cpa-thread");
    assert!(
        !upstream
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|request| request.url.path() == "/codex/analytics-events/events")
    );
    let (turn, headers) = timeout(Duration::from_secs(45), async {
        loop {
            for request in upstream.received_requests().await.unwrap() {
                if request.url.path() != "/codex/analytics-events/events" {
                    continue;
                }
                let payload: Value = request.body_json().unwrap();
                for event in payload["events"].as_array().unwrap() {
                    if event["event_type"] == "codex_turn_event" {
                        return (event["event_params"].clone(), request.headers);
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await?;
    assert!(business_completed.elapsed() >= Duration::from_secs(29));
    assert_eq!(turn["thread_id"], *final_thread);
    assert_eq!(
        turn["session_id"],
        final_request["client_metadata"]["session_id"]
    );
    assert_eq!(turn["turn_id"], final_request["client_metadata"]["turn_id"]);
    assert_eq!(turn["model"], final_request["model"]);
    assert_eq!(turn["input_tokens"], 11);
    assert_eq!(turn["app_server_client"]["client_name"], "Codex Desktop");
    assert_eq!(turn["app_server_client"]["rpc_transport"], "stdio");
    assert_eq!(
        headers.get("authorization").unwrap(),
        "Bearer fake-cpa-access"
    );
    assert_eq!(headers.get("chatgpt-account-id").unwrap(), "account");
    assert!(!turn.to_string().contains("original-cpa-"));
    assert!(turn["after_last_sampling_ms"].as_u64().unwrap() >= 30_000);
    assert_eq!(
        upstream
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|request| request.url.path() == "/v1/responses")
            .count(),
        1
    );
    Ok(())
}

#[test_case::test_case("response.metadata", "response.metadata"; "standard")]
#[test_case::test_case("codex.response.metadata", "codex.response.metadata"; "prefixed")]
#[test_case::test_case("response.metadata", "codex.response.metadata"; "standard_then_prefixed")]
#[test_case::test_case("codex.response.metadata", "response.metadata"; "prefixed_then_standard")]
#[tokio::test]
async fn cpa_native_ws_turn_state_reuses_first_ticket(
    first_kind: &str,
    later_kind: &str,
) -> Result<()> {
    use core_test_support::responses;

    let first_ticket = "A".repeat(780);
    let later_ticket = "B".repeat(780);
    let events = |id: &str| {
        vec![
            json!({"type":first_kind,"headers":{"x-codex-turn-state":first_ticket,"openai-model":"gpt-5.2","x-models-etag":"fixture-etag"}}),
            json!({"type":later_kind,"headers":{"x-codex-turn-state":later_ticket}}),
            responses::ev_response_created(id),
            responses::ev_output_text_delta("metadata preserved"),
            responses::ev_completed(id),
        ]
    };
    let upstream = MockServer::start().await;
    let websocket =
        responses::start_websocket_server(vec![vec![events("resp_first"), events("resp_second")]])
            .await;
    let home = TempDir::new()?;
    prepare_account(&upstream, &home)?;
    let config_path = home.path().join("state/shared.json/config.toml");
    let mut config: toml::Value = toml::from_str(&std::fs::read_to_string(&config_path)?)?;
    let provider = &mut config["model_providers"]["mock_provider"];
    provider["base_url"] = toml::Value::String(format!(
        "{}/v1",
        websocket.uri().replace("ws://", "http://")
    ));
    provider["supports_websockets"] = toml::Value::Boolean(true);
    std::fs::write(config_path, toml::to_string(&config)?)?;
    let _runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let input = json!({"model":"gpt-5.2","input":"fixture","stream":true,
        "session_id":"source-session","thread_id":"source-session",
        "turn_id":"01a1335a-1111-7111-8111-111111111111"});
    for id in [701, 702] {
        let result = exchange(&mut ws, rpc(input.clone(), id)).await?;
        assert_eq!(
            result.reply["result"]["statusCode"], 200,
            "{}",
            result.reply
        );
        let body = String::from_utf8(result.body)?;
        assert!(body.contains("metadata preserved"));
        assert!(body.contains("response.completed"));
    }
    let connections = websocket.connections();
    assert_eq!(connections.len(), 1, "same native WebSocket connection");
    assert_eq!(connections[0].len(), 2, "two successful model requests");
    let first = connections[0][0].body_json();
    let second = connections[0][1].body_json();
    assert!(first["client_metadata"].get("x-codex-turn-state").is_none());
    assert_eq!(
        second["client_metadata"]["x-codex-turn-state"],
        first_ticket
    );
    assert_eq!(first["client_metadata"]["turn_id"], input["turn_id"]);
    assert_eq!(second["client_metadata"]["turn_id"], input["turn_id"]);
    assert_eq!(second["previous_response_id"], "resp_first");
    Ok(())
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
        let events=[json!({"type":"response.created","response":{"id":"resp_opaque"}}),
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
            .next_back()
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
            assert_eq!(
                log["headers"]["authorization"],
                json!(["Bearer fake-cpa-access"])
            );
        }
    }
    assert!(result.logs.iter().any(|log| log["kind"] == "body"));
    let requests = upstream.received_requests().await.unwrap();
    let request = requests
        .iter()
        .find(|r| r.url.path() == "/v1/responses")
        .unwrap();
    assert_eq!(request.headers["originator"], "Codex Desktop");
    assert!(
        request.headers["user-agent"]
            .to_str()?
            .starts_with(concat!("Codex Desktop/", env!("CARGO_PKG_VERSION")))
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
        (
            "tool_choice",
            json!({"type":"function","name":"undeclared"}),
        ),
        ("previous_response_id", json!("old")),
        ("background", json!(true)),
        ("generate", json!(false)),
        (
            "input",
            json!([{"type":"unknown_business","content":"must not be lost"}]),
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
async fn cpa_native_types_own_unknown_business_fields_without_an_adapter_gate() -> Result<()> {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            completed(json!({"id":"native-fields","output":[]})),
            "text/event-stream",
        ))
        .expect(1)
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    let _runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let result = exchange(
        &mut ws,
        rpc(
            json!({"model":"mock-model","input":"hello",
        "extension":true,"reasoning":{"future":1}}),
            1250,
        ),
    )
    .await?;
    assert_eq!(
        result.reply["result"]["statusCode"], 200,
        "{}",
        result.reply
    );
    assert!(result.error.is_none());
    assert_eq!(
        upstream
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|request| request.url.path() == "/v1/responses")
            .count(),
        1
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
            false,
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
            assert!(result.error.is_some() || result.reply.get("error").is_some());
            assert!(!String::from_utf8(result.body)?.contains("response.completed"));
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
        input["turn_id"] = json!("caller-image-turn");
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
        assert_eq!(body["stream"], stream);
        assert!(body.get("n").is_none());
        let requests = upstream.received_requests().await.expect("image requests");
        let headers = &requests.last().expect("latest image request").headers;
        let turn_metadata: Value =
            serde_json::from_str(headers["x-codex-turn-metadata"].to_str()?)?;
        assert_eq!(turn_metadata["turn_id"], "caller-image-turn");
        assert_eq!(headers["x-codex-image-turn-id"], "caller-image-turn");
    }
    for (key, value) in [
        ("mask", json!(true)),
        ("partial_images", json!("invalid")),
        ("output_compression", json!(false)),
        ("extension", json!("unsupported business")),
    ] {
        let mut request = rpc(
            json!({"model":"gpt-image-2","prompt":"fixture", key:value}),
            159,
        );
        request["params"]["operation"] = json!("images/edits");
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
    let mut completed = std::collections::BTreeSet::new();
    let mut diagnostics = std::collections::BTreeSet::new();
    timeout(Duration::from_secs(30), async {
        while completed.len() < 4 || diagnostics.len() < 4 {
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
                completed.insert(id);
            } else if message["method"] == "cpa/inference/diagnostics/completed" {
                diagnostics.insert(id);
            } else if message["method"] == "cpa/inference/event" {
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

#[path = "streaming_tests.rs"]
mod streaming_tests;

#[path = "websocket_node_tests.rs"]
mod websocket_node_tests;

#[path = "continuation_tests.rs"]
mod continuation_tests;

#[tokio::test]
async fn cpa_native_partial_output_error_returns_without_retry() -> Result<()> {
    let upstream = MockServer::start().await;
    let sequence = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    Mock::given(method("POST")).and(path("/v1/responses")).respond_with(move |request: &wiremock::Request| {
        let first = sequence.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
        let body: Value = request.body_json().unwrap();
        if !first { assert!(!body["input"].to_string().contains("failed_tool")); }
        let response_id = if first { "failed_response" } else { "successful_response" };
        let call_id = if first { "failed_tool" } else { "successful_tool" };
        let item = json!({"type":"function_call","id":call_id,"call_id":call_id,"name":"shell","arguments":"{}"});
        let mut events = vec![
            json!({"type":"response.created","response":{"id":response_id}}),
            json!({"type":"response.output_text.delta","delta":response_id}),
            json!({"type":"response.function_call_arguments.delta","call_id":call_id,"delta":"{}"}),
            json!({"type":"response.function_call_arguments.done","call_id":call_id,"arguments":"{}"}),
            json!({"type":"response.output_item.done","item":item}),
        ];
        if !first { events.push(json!({"type":"response.completed","response":{"id":response_id,"output":[item]}})); }
        ResponseTemplate::new(200).set_body_raw(events.iter().map(|event|format!("data: {event}\n\n")).collect::<String>(),"text/event-stream")
    }).expect(1).mount(&upstream).await;
    let home = TempDir::new()?;
    prepare_account(&upstream, &home)?;
    let config_path = home.path().join("state/shared.json/config.toml");
    let mut config: toml::Value = toml::from_str(&std::fs::read_to_string(&config_path)?)?;
    config["model_providers"]["mock_provider"]["stream_max_retries"] = toml::Value::Integer(1);
    std::fs::write(config_path, toml::to_string(&config)?)?;
    let _runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let mut input = model_input();
    input["tools"] = json!([{"type":"function","name":"shell","parameters":{"type":"object"}}]);
    let result = exchange(&mut ws, rpc(input, 901)).await?;
    assert!(result.error.is_some());
    let body = std::str::from_utf8(&result.body)?;
    assert!(body.contains("failed_response"));
    assert!(body.contains("response.function_call_arguments.delta"));
    assert!(body.contains("response.function_call_arguments.done"));
    assert!(body.contains("response.output_item.done"));
    assert!(!body.contains("response.completed") && !body.contains("successful_response"));
    assert_eq!(
        upstream
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path() == "/v1/responses")
            .count(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn cpa_master_rejections_close_the_diagnostic_lifetime() -> Result<()> {
    let upstream = MockServer::start().await;
    let home = TempDir::new()?;
    let _runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let mut malformed = rpc(model_input(), 902);
    malformed["params"]["request"] = Value::Null;
    let mut missing = rpc(model_input(), 903);
    missing["params"]["credentialId"] = json!("missing.json");
    for request in [malformed, missing] {
        let result = exchange(&mut ws, request).await?;
        assert!(result.reply.get("error").is_some());
        assert!(result.body.is_empty());
    }
    Ok(())
}

#[test_case::test_case(401)]
#[test_case::test_case(429)]
#[test_case::test_case(503)]
#[tokio::test]
async fn cpa_native_http_errors_do_not_retry_and_preserve_the_original_response(
    status: u16,
) -> Result<()> {
    let upstream = MockServer::start().await;
    let original =
        json!({"error":{"code":"fixture_error","message":"original upstream failure","future":17}})
            .to_string();
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(
            ResponseTemplate::new(status)
                .insert_header("retry-after", "9")
                .insert_header("x-request-id", "original-request")
                .set_body_raw(original.clone(), "application/json"),
        )
        .expect(1)
        .mount(&upstream)
        .await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    prepare_account(&upstream, &home)?;
    let config_path = home.path().join("state/shared.json/config.toml");
    let mut config: toml::Value = toml::from_str(&std::fs::read_to_string(&config_path)?)?;
    config["model_providers"]["mock_provider"]["request_max_retries"] = toml::Value::Integer(5);
    config["model_providers"]["mock_provider"]["stream_max_retries"] = toml::Value::Integer(5);
    std::fs::write(config_path, toml::to_string(&config)?)?;
    let _runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let result = exchange(&mut ws, rpc(model_input(), 1001)).await?;
    let error = &result.reply["error"]["data"];
    assert_eq!(error["httpStatus"], status);
    assert_eq!(error["body"], original);
    assert_eq!(error["headers"]["retry-after"], json!(["9"]));
    assert_eq!(
        error["headers"]["x-request-id"],
        json!(["original-request"])
    );
    assert!(error.get("nativeError").is_some());
    assert!(result.body.is_empty());
    Ok(())
}

#[test_case::test_case(401)]
#[test_case::test_case(426)]
#[test_case::test_case(503)]
#[tokio::test]
async fn cpa_native_websocket_handshake_error_never_reconnects_or_falls_back(
    status: u16,
) -> Result<()> {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/responses"))
        .respond_with(
            ResponseTemplate::new(status)
                .insert_header("retry-after", "7")
                .insert_header("x-request-id", "handshake-error")
                .set_body_string("original handshake error"),
        )
        .expect(1)
        .mount(&upstream)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&upstream)
        .await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    prepare_account(&upstream, &home)?;
    let config_path = home.path().join("state/shared.json/config.toml");
    let mut config: toml::Value = toml::from_str(&std::fs::read_to_string(&config_path)?)?;
    config["model_providers"]["mock_provider"]["supports_websockets"] = toml::Value::Boolean(true);
    config["model_providers"]["mock_provider"]["stream_max_retries"] = toml::Value::Integer(5);
    std::fs::write(config_path, toml::to_string(&config)?)?;
    let _runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let result = exchange(
        &mut ws,
        rpc(
            json!({"model":"gpt-5.2","input":"fixture","stream":true}),
            1002,
        ),
    )
    .await?;
    let error = &result.reply["error"]["data"];
    assert_eq!(error["httpStatus"], status);
    assert_eq!(error["headers"]["retry-after"], json!(["7"]));
    assert_eq!(error["headers"]["x-request-id"], json!(["handshake-error"]));
    assert!(error.get("nativeError").is_some());
    assert!(result.body.is_empty());
    Ok(())
}

#[test_case::test_case("generations")]
#[test_case::test_case("edits")]
#[tokio::test]
async fn cpa_native_images_do_not_retry_failed_posts(operation: &str) -> Result<()> {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/v1/images/{operation}")))
        .respond_with(
            ResponseTemplate::new(503)
                .insert_header("retry-after", "5")
                .set_body_string("original Images failure"),
        )
        .expect(1)
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    prepare_account(&upstream, &home)?;
    let config_path = home.path().join("state/shared.json/config.toml");
    let mut config: toml::Value = toml::from_str(&std::fs::read_to_string(&config_path)?)?;
    config["model_providers"]["mock_provider"]["request_max_retries"] = toml::Value::Integer(5);
    std::fs::write(config_path, toml::to_string(&config)?)?;
    let _runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let mut input = json!({"model":"gpt-image-2","prompt":"fixture"});
    if operation == "edits" {
        input["images"] = json!([{"image_url":"data:image/png;base64,aW1hZ2U="}]);
    }
    let mut request = rpc(input, 1003);
    request["params"]["operation"] = json!(format!("images/{operation}"));
    request["params"]["imageApi"] = json!("images");
    let result = exchange(&mut ws, request).await?;
    let error = &result.reply["error"]["data"];
    assert_eq!(error["httpStatus"], 503);
    assert_eq!(error["body"], "original Images failure");
    assert_eq!(error["headers"]["retry-after"], json!(["5"]));
    assert!(error.get("nativeError").is_some());
    Ok(())
}

#[tokio::test]
async fn cpa_native_zero_retries_preserves_normal_redirects() -> Result<()> {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(307).insert_header(
            "location",
            format!("{}/redirected-responses", upstream.uri()),
        ))
        .expect(1)
        .mount(&upstream)
        .await;
    Mock::given(method("POST"))
        .and(path("/redirected-responses"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            completed(json!({"id":"redirect-preserved"})),
            "text/event-stream",
        ))
        .expect(1)
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    let _runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let result = exchange(&mut ws, rpc(model_input(), 1004)).await?;
    assert!(result.error.is_none());
    assert_eq!(result.reply["result"]["statusCode"], 200);
    assert!(std::str::from_utf8(&result.body)?.contains("redirect-preserved"));
    Ok(())
}

#[tokio::test]
async fn cpa_same_request_id_on_new_connection_has_independent_worker_routes() -> Result<()> {
    let upstream = MockServer::start().await;
    let (entered, ready) = tokio::sync::oneshot::channel();
    let entered = std::sync::Arc::new(std::sync::Mutex::new(Some(entered)));
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(move |request: &wiremock::Request| {
            let body: Value = request.body_json().unwrap();
            if body["input"].to_string().contains("hold-first-request") {
                if let Some(entered) = entered.lock().unwrap().take() {
                    let _ = entered.send(());
                }
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_secs(120))
                    .set_body_string("unused")
            } else {
                ResponseTemplate::new(200).set_body_raw(
                    completed(json!({"id":"second-connection-success"})),
                    "text/event-stream",
                )
            }
        })
        .expect(2)
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    let _runtime = server(&upstream, &home).await?;
    let mut first = connect(&home).await?;
    let mut second = connect(&home).await?;
    let mut first_request = rpc(model_input(), 1200);
    first_request["params"]["request"]["input"] = json!("hold-first-request");
    send(&mut first, first_request).await?;
    timeout(Duration::from_secs(10), ready).await??;
    let mut second_request = rpc(model_input(), 1200);
    second_request["params"]["sessionId"] = json!("independent-second-session");
    let result = exchange(&mut second, second_request).await?;
    assert_eq!(result.reply["result"]["requestId"], "r-1200");
    assert!(std::str::from_utf8(&result.body)?.contains("second-connection-success"));
    assert!(result.logs.iter().all(|log| log["requestId"] == "r-1200"));
    send(
        &mut first,
        json!({"id":1201,"method":"cpa/inference/cancel","params":{"requestId":"r-1200"}}),
    )
    .await?;
    let first_error = receive(&mut first, "id", json!(1200)).await?;
    assert_eq!(first_error["error"]["data"]["httpStatus"], 499);
    Ok(())
}

#[path = "compatibility_tests.rs"]
mod compatibility_tests;
