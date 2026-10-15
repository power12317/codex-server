use super::*;
use anyhow::Context;
use base64::Engine;
use pretty_assertions::assert_eq;

pub(super) async fn exchange(ws: &mut Socket, mut request: Value) -> Result<(Value, Vec<u8>)> {
    request["params"]["request"]["stream"] = json!(true);
    send(ws, request).await?;
    let mut upstream = Value::Null;
    let mut bytes = Vec::new();
    timeout(Duration::from_secs(/*secs*/ 30), async {
        while let Some(message) = ws.next().await {
            let Message::Text(text) = message? else {
                continue;
            };
            let value: Value = serde_json::from_str(&text)?;
            anyhow::ensure!(value.get("error").is_none(), "{value}");
            match value["method"].as_str() {
                Some("cpa/inference/upstream") if value["params"]["kind"] == "request" => {
                    upstream = value["params"].clone();
                }
                Some("cpa/inference/body") => bytes.extend(
                    base64::engine::general_purpose::STANDARD.decode(
                        value["params"]["bodyBase64"]
                            .as_str()
                            .context("bodyBase64 string required")?,
                    )?,
                ),
                Some("cpa/inference/completed") => return Ok((upstream, bytes)),
                Some("cpa/inference/error") => anyhow::bail!("{value}"),
                _ => {}
            }
        }
        anyhow::bail!("connection closed")
    })
    .await?
}

#[tokio::test]
async fn cpa_source_turn_continuity_isolation_and_default_stderr() -> Result<()> {
    let upstream = MockServer::start().await;
    Mock::given(method("POST")).and(path("/v1/responses"))
        .respond_with(|request: &wiremock::Request| {
            let body: Value = request.body_json().unwrap();
            let state = body["test_state"].as_str().unwrap();
            ResponseTemplate::new(/*s*/ 200).insert_header("x-request-id", "upstream-request")
                .set_body_raw(format!("data: {{\"type\":\"response.metadata\",\"headers\":{{\"x-codex-turn-state\":\"{state}\"}}}}\n\ndata: [DONE]\n\n"), "text/event-stream")
        }).mount(&upstream).await;
    let home = TempDir::new()?;
    prepare_account(&upstream, &home)?;
    std::fs::copy(
        home.path().join("shared.json"),
        home.path().join("second.json"),
    )?;
    std::fs::create_dir_all(home.path().join("state/second.json"))?;
    std::fs::copy(
        home.path().join("config.toml"),
        home.path().join("state/second.json/config.toml"),
    )?;
    let runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let source = json!({"session_id":"source-session", "thread_id":"source-session", "turn_id":"turn-a", "root_turn_id":"root-a"});
    let mut request = start(
        json!({"model":"mock-model", "input":[], "test_state":"first",
        "prompt_cache_key":"caller-cache", "client_metadata":{"x-codex-turn-metadata": source.to_string()}}),
    );
    let (first, _) = exchange(&mut ws, request.clone()).await?;
    assert!(first["headers"].get("x-codex-turn-state").is_none());
    let body: Value = serde_json::from_str(first["body"].as_str().unwrap())?;
    let native: Value = serde_json::from_str(
        body["client_metadata"]["x-codex-turn-metadata"]
            .as_str()
            .unwrap(),
    )?;
    assert_eq!(
        (
            native["turn_id"].clone(),
            native["root_turn_id"].clone(),
            body["prompt_cache_key"].clone()
        ),
        (
            json!("turn-a"),
            json!("root-a"),
            native["session_id"].clone()
        )
    );
    assert_eq!(
        first["headers"]["session-id"],
        json!([native["session_id"]])
    );
    assert_eq!(native["session_id"], native["thread_id"]);
    assert_eq!(
        first["headers"]["thread-id"],
        first["headers"]["session-id"]
    );
    request["params"]["request"]["client_metadata"] = source.clone();
    request["params"]["request"]["test_state"] = json!("second-ignored");
    let (second, _) = exchange(&mut ws, request.clone()).await?;
    assert_eq!(second["headers"]["x-codex-turn-state"], json!(["first"]));
    let second_body: Value = serde_json::from_str(second["body"].as_str().unwrap())?;
    assert_eq!(second_body["client_metadata"], body["client_metadata"]);
    // Top-level source fields have the same meaning as flat and nested metadata.
    for (key, value) in source.as_object().unwrap() {
        request["params"]["request"][key] = value.clone();
    }
    request["params"]["request"]["client_metadata"] = json!({});
    assert_eq!(
        exchange(&mut ws, request.clone()).await?.0["headers"]["x-codex-turn-state"],
        json!(["first"])
    );

    for (field, value) in [
        ("turn_id", "turn-b"),
        ("thread_id", "other-thread"),
        ("session_id", "other-session"),
    ] {
        let mut isolated = request.clone();
        isolated["params"]["request"][field] = json!(value);
        assert!(
            exchange(&mut ws, isolated).await?.0["headers"]
                .get("x-codex-turn-state")
                .is_none()
        );
    }
    for field in ["sessionId", "credentialId"] {
        let mut isolated = request.clone();
        isolated["params"][field] = json!(if field == "credentialId" {
            "second.json"
        } else {
            "other-scope"
        });
        assert!(
            exchange(&mut ws, isolated).await?.0["headers"]
                .get("x-codex-turn-state")
                .is_none()
        );
    }
    // Interleaved other turns must not replace the original turn's first state.
    assert_eq!(
        exchange(&mut ws, request.clone()).await?.0["headers"]["x-codex-turn-state"],
        json!(["first"])
    );
    request["params"]["request"]
        .as_object_mut()
        .unwrap()
        .remove("turn_id");
    let (synthetic_first, _) = exchange(&mut ws, request.clone()).await?;
    assert!(
        synthetic_first["headers"]
            .get("x-codex-turn-state")
            .is_none()
    );
    let first_body: Value = serde_json::from_str(synthetic_first["body"].as_str().unwrap())?;
    let (synthetic_second, _) = exchange(&mut ws, request.clone()).await?;
    let second_body: Value = serde_json::from_str(synthetic_second["body"].as_str().unwrap())?;
    assert_eq!(
        synthetic_second["headers"]["x-codex-turn-state"],
        json!(["second-ignored"])
    );
    let synthetic_turn = first_body["client_metadata"]["turn_id"].as_str().unwrap();
    uuid::Uuid::parse_str(synthetic_turn)?;
    assert_eq!(
        second_body["client_metadata"]["turn_id"],
        json!(synthetic_turn)
    );
    assert_eq!(
        second_body["client_metadata"]["session_id"],
        native["session_id"]
    );
    let mut left = request.clone();
    left["params"]["request"]["turn_id"] = json!("concurrent-left");
    left["params"]["request"]["test_state"] = json!("left-state");
    left["params"]["requestId"] = json!("left-r");
    let mut right = left.clone();
    right["params"]["requestId"] = json!("right-r");
    right["params"]["request"]["turn_id"] = json!("concurrent-right");
    right["params"]["request"]["test_state"] = json!("right-state");
    let mut other_ws = connect(&home).await?;
    let (left_result, right_result) = tokio::try_join!(
        exchange(&mut ws, left.clone()),
        exchange(&mut other_ws, right.clone())
    )?;
    assert!(left_result.0["headers"].get("x-codex-turn-state").is_none());
    assert!(
        right_result.0["headers"]
            .get("x-codex-turn-state")
            .is_none()
    );
    let (left_result, right_result) =
        tokio::try_join!(exchange(&mut ws, left), exchange(&mut other_ws, right))?;
    assert_eq!(
        (
            left_result.0["headers"]["x-codex-turn-state"].clone(),
            right_result.0["headers"]["x-codex-turn-state"].clone()
        ),
        (json!(["left-state"]), json!(["right-state"]))
    );
    let started = runtime
        .wait_for_json_log_event("cpa.request.start", Duration::from_secs(/*secs*/ 10))
        .await?;
    let completed = runtime
        .wait_for_json_log_event("cpa.request.completed", Duration::from_secs(/*secs*/ 10))
        .await?;
    let worker = runtime
        .wait_for_json_log_event("cpa.worker.started", Duration::from_secs(/*secs*/ 10))
        .await?;
    assert_eq!(
        (
            started["fields"]["credentialId"].clone(),
            started["fields"]["rpcRequestId"].clone(),
            completed["fields"]["details"]["turnStateLength"].clone()
        ),
        (json!("shared.json"), json!("r"), json!(5))
    );
    assert!(worker["fields"]["details"]["pid"].is_number());
    assert!(
        worker["fields"]["details"]["configFile"]
            .as_str()
            .unwrap()
            .ends_with(".json/config.toml")
    );
    Ok(())
}

#[tokio::test]
async fn cpa_history_ids_follow_item_type_and_preserve_call_associations() -> Result<()> {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(
            ResponseTemplate::new(/*s*/ 200).set_body_raw("data: [DONE]\n\n", "text/event-stream"),
        )
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    let _runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let mut input = Vec::new();
    let mut expected = Vec::new();
    for index in 0..30 {
        let call = json!({"type":"custom_tool_call", "id":format!("fc_{index}"),
            "call_id":format!("fc_call_{index}"), "name":"shell", "input":"printf 'fc_payload'"});
        let output = json!({"type":"custom_tool_call_output", "id":format!("fco_{index}"),
            "call_id":format!("fc_call_{index}"), "output":"fc_untouched"});
        input.extend([call.clone(), output.clone()]);
        let mut normalized_call = call;
        normalized_call["id"] = json!(format!("ctc_{index}"));
        let mut normalized_output = output;
        normalized_output["id"] = json!(format!("ctco_{index}"));
        expected.extend([normalized_call, normalized_output]);
    }
    let valid = json!({"type":"function_call", "id":"fc_valid", "call_id":"call-valid", "name":"shell", "arguments":"{}"});
    input.push(valid.clone());
    expected.push(valid);
    let (logged, _) = exchange(
        &mut ws,
        start(json!({"model":"mock-model", "instructions":"native prefix", "input":input})),
    )
    .await?;
    let sent: Value = serde_json::from_str(logged["body"].as_str().unwrap())?;
    let history: Vec<_> = sent["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item.get("call_id").is_some())
        .cloned()
        .collect();
    assert_eq!(history, expected);
    Ok(())
}

#[tokio::test]
async fn cpa_failure_logs_are_visible_without_rust_log() -> Result<()> {
    let upstream = MockServer::start().await;
    Mock::given(method("POST")).and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(/*s*/ 400).insert_header("x-request-id", "upstream-error")
            .insert_header("set-cookie", "private-cookie").set_body_json(json!({"error":{
                "code":"invalid_value", "type":"invalid_request_error", "message":"Invalid input[47].id: Expected ctc"}})))
        .mount(&upstream).await;
    let home = TempDir::new()?;
    let runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    send(
        &mut ws,
        start(json!({"model":"mock-model", "stream":true, "input":[]})),
    )
    .await?;
    receive(&mut ws, "method", json!("cpa/inference/upstream")).await?;
    let response = receive(&mut ws, "method", json!("cpa/inference/upstream")).await?;
    assert_eq!(
        response["params"]["headers"]["set-cookie"],
        json!(["private-cookie"])
    );
    receive(&mut ws, "id", json!(2)).await?;
    let error = runtime
        .wait_for_json_log_event("cpa.request.error", Duration::from_secs(/*secs*/ 10))
        .await?;
    assert_eq!(error["fields"]["rpcRequestId"], json!("r"));
    let logged = runtime
        .wait_for_json_log_event("cpa.request.upstream", Duration::from_secs(/*secs*/ 10))
        .await?;
    assert_eq!(
        logged["fields"]["details"]["headers"]["authorization"],
        json!(["[REDACTED]"])
    );
    assert!(!logged.to_string().contains("fake-cpa-access"));
    let response = runtime
        .wait_for_json_log_event(
            "cpa.request.upstream.response",
            Duration::from_secs(/*secs*/ 10),
        )
        .await?;
    assert_eq!(
        response["fields"]["details"]["error"],
        json!({"code":"invalid_value",
        "type":"invalid_request_error", "message":"Invalid input[47].id: Expected ctc"})
    );
    assert_eq!(
        response["fields"]["details"]["headers"]["x-request-id"],
        json!(["upstream-error"])
    );
    assert!(!response.to_string().contains("private-cookie"));
    Ok(())
}

#[tokio::test]
async fn cpa_validation_failure_has_start_and_error_logs() -> Result<()> {
    let upstream = MockServer::start().await;
    let home = TempDir::new()?;
    let runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    send(
        &mut ws,
        start(json!({"model":"mock-model", "stream":false, "input":[]})),
    )
    .await?;
    assert_eq!(
        receive(&mut ws, "id", json!(2)).await?["error"]["data"]["httpStatus"],
        json!(400)
    );
    runtime
        .wait_for_json_log_event("cpa.request.start", Duration::from_secs(/*secs*/ 10))
        .await?;
    let error = runtime
        .wait_for_json_log_event("cpa.request.error", Duration::from_secs(/*secs*/ 10))
        .await?;
    assert_eq!(
        error["fields"]["details"],
        json!({"error":"stream=true is required", "httpStatus":400})
    );
    assert!(
        upstream
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.url.path() != "/v1/responses")
    );
    Ok(())
}
