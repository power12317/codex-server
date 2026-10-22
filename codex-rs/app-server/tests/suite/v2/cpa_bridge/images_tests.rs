use super::*;
use base64::Engine;
use pretty_assertions::assert_eq;
use test_case::test_case;

async fn exchange(ws: &mut Socket, request: Value) -> Result<(Value, Vec<u8>)> {
    send(ws, request).await?;
    timeout(Duration::from_secs(30), async {
        let mut upstream = Value::Null;
        let mut bytes = Vec::new();
        while let Some(message) = ws.next().await {
            let Message::Text(text) = message? else {
                continue;
            };
            let value: Value = serde_json::from_str(&text)?;
            anyhow::ensure!(value.get("error").is_none(), "{value}");
            match value["method"].as_str() {
                Some("cpa/inference/upstream") if value["params"]["kind"] == "request" => {
                    upstream = value["params"].clone()
                }
                Some("cpa/inference/body") => bytes.extend(
                    base64::engine::general_purpose::STANDARD
                        .decode(value["params"]["bodyBase64"].as_str().expect("body"))?,
                ),
                Some("cpa/inference/completed") => return Ok((upstream, bytes)),
                Some("cpa/inference/error") => anyhow::bail!("{value}"),
                _ => {}
            }
        }
        anyhow::bail!("closed before completion")
    })
    .await?
}

pub(super) fn image_request(operation: &str, api: &str, body: Value) -> Value {
    let mut request = start(body);
    request["params"]["operation"] = json!(operation);
    request["params"]["imageApi"] = json!(api);
    request["params"]["sourceFormat"] = json!("openai-image");
    request
}

#[test_case(false; "responses")]
#[test_case(true; "responses_lite")]
#[tokio::test]
async fn cpa_image_responses_uses_native_normalization(lite: bool) -> Result<()> {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw("data: [DONE]\n\n", "text/event-stream"),
        )
        .expect(1)
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    prepare_account(&upstream, &home)?;
    let config = core_test_support::load_default_config_for_test(&home).await;
    let mut model = codex_core::test_support::construct_model_info_offline("mock-model", &config);
    model.use_responses_lite = lite;
    let catalog = home.path().join("models.json");
    std::fs::write(&catalog, serde_json::to_vec(&json!({"models":[model]}))?)?;
    let worker_config = home.path().join("state/shared.json/config.toml");
    let original = std::fs::read_to_string(&worker_config)?;
    std::fs::write(
        &worker_config,
        format!(
            "model_catalog_json = {}\n{original}",
            serde_json::to_string(&catalog)?
        ),
    )?;
    let _runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let image = json!({"type":"image_generation", "output_format":"png"});
    let tools = json!([image, {"type":"tool_search"}]);
    let input = json!([
        {"role":"user", "content":"draw"},
        {"role":"user", "content":[{"type":"input_text", "text":"array input"}]},
        {"type":"function_call", "id":"msg_history", "name":"fixture", "call_id":"call_history", "arguments":"{}"}
    ]);
    let (logged, _) = exchange(&mut ws, image_request("images/generations", "responses", json!({
        "model":"mock-model", "input":input, "tools":tools, "instructions":"native instructions",
        "stream":false, "store":true, "prompt_cache_key":"caller-cache", "session_id":"caller-session",
        "tool_choice":{"type":"image_generation"}, "client_metadata":{"business_tag":"keep", "session_id":"caller-spoof"}
    }))).await?;
    let body: Value = serde_json::from_str(logged["body"].as_str().expect("wire body"))?;
    let items = body["input"].as_array().expect("native input");
    let prefix = usize::from(lite);
    let expected_tools = json!([image, {"type":"tool_search", "execution":"server", "description":"", "parameters":{"type":"object"}}]);
    if lite {
        assert!(body.get("tools").is_none());
        assert_eq!(items[0]["type"], json!("additional_tools"));
        assert_eq!(items[0]["tools"], expected_tools);
    } else {
        assert_eq!(body["tools"], expected_tools);
    }
    assert!(body.get("instructions").is_none());
    assert_eq!(items[prefix]["role"], json!("developer"));
    assert_eq!(
        items[prefix]["content"][0]["text"],
        json!("native instructions")
    );
    for (item, text) in items[prefix + 1..prefix + 3]
        .iter()
        .zip(["draw", "array input"])
    {
        assert_eq!(item["type"], json!("message"));
        assert_eq!(item["content"], json!([{"type":"input_text", "text":text}]));
    }
    assert_eq!(items.last().expect("history")["id"], json!("fc_history"));
    assert_eq!(
        items.last().expect("history")["call_id"],
        json!("call_history")
    );
    assert_eq!(body["stream"], json!(true));
    assert_eq!(body["store"], json!(true));
    assert_eq!(body["parallel_tool_calls"], json!(!lite));
    assert_eq!(body["tool_choice"], json!({"type":"image_generation"}));
    assert_eq!(
        body["prompt_cache_key"],
        body["client_metadata"]["session_id"]
    );
    assert_ne!(body["prompt_cache_key"], json!("caller-cache"));
    assert_ne!(body["client_metadata"]["session_id"], json!("caller-spoof"));
    assert_eq!(body["client_metadata"]["business_tag"], json!("keep"));
    let received = upstream.received_requests().await.expect("HTTP requests");
    let actual = received
        .iter()
        .find(|request| request.url.path() == "/v1/responses")
        .expect("model HTTP");
    assert_eq!(actual.body_json::<Value>()?, body);
    Ok(())
}

#[test_case("images/generations", "images", false; "generate_json")]
#[test_case("images/generations", "images", true; "generate_stream")]
#[test_case("images/edits", "images", false; "edit_json")]
#[test_case("images/edits", "images", true; "edit_stream")]
#[test_case("images/generations", "responses", false; "tool_generate_json")]
#[test_case("images/generations", "responses", true; "tool_generate_stream")]
#[test_case("images/edits", "responses", false; "tool_edit_json")]
#[test_case("images/edits", "responses", true; "tool_edit_stream")]
#[tokio::test]
async fn cpa_images_native_http_builds_requests_and_returns_raw_bytes(
    operation: &str,
    api: &str,
    stream: bool,
) -> Result<()> {
    let upstream = MockServer::start().await;
    let endpoint = if api == "images" {
        format!("/v1/{operation}")
    } else {
        "/v1/responses".into()
    };
    let returned = if stream {
        "event: image_generation.partial_image\ndata: {\"type\":\"image_generation.partial_image\",\"b64_json\":\"AAH/\"}\n\nevent: image_generation.completed\ndata: {\"type\":\"image_generation.completed\",\"b64_json\":\"/wA=\"}\n\n"
    } else {
        "{\"created\":123,\"data\":[{\"b64_json\":\"AAH/\"}],\"extension\":false}"
    };
    Mock::given(method("POST"))
        .and(path(endpoint.as_str()))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-codex-imagegen-request-id", "image-fixture")
                .set_body_raw(
                    returned,
                    if stream {
                        "text/event-stream"
                    } else {
                        "application/json"
                    },
                ),
        )
        .expect(3)
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    let runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    send(
        &mut ws,
        json!({"id":31,"method":"cpa/capabilities/read","params":{}}),
    )
    .await?;
    let caps = receive(&mut ws, "id", json!(31)).await?;
    assert!(
        caps["result"]["operations"]
            .as_array()
            .expect("operations")
            .contains(&json!(operation))
    );
    let data_url = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode([0, 255, 1, 128])
    );
    let mut full = if api == "images" {
        json!({"model":"gpt-image-1.5", "prompt":"caller prompt", "stream":stream, "size":"1024x1024",
            "n":2, "quality":"high", "output_format":"png", "partial_images":0,
            "images":[{"image_url":data_url},{"image_url":data_url}], "mask":{"image_url":data_url}, "extension":["a","b"]})
    } else {
        json!({"model":"mock-model", "stream":stream, "store":false, "instructions":"caller instructions",
            "input":[{"type":"message", "role":"user", "content":[{"type":"input_text", "text":"caller prompt"},
                {"type":"input_image", "image_url":data_url, "detail":"original"}]}],
            "tools":[{"type":"image_generation", "action":"edit", "output_format":"png", "input_image_mask":{"image_url":data_url}}],
            "tool_choice":{"type":"image_generation"}, "prompt_cache_key":"caller-cache", "include":[],
            "reasoning":{"effort":"high", "summary":"auto"}, "text":{"verbosity":"low"}, "service_tier":"priority", "extension":["a","b"]})
    };
    if api == "images" && operation == "images/generations" {
        full.as_object_mut().expect("object").remove("images");
        full.as_object_mut().expect("object").remove("mask");
    }
    let minimal = if api == "images" {
        let mut request = json!({"model":"gpt-image-1.5", "prompt":"draw"});
        if operation == "images/edits" {
            request["images"] = json!([{"image_url":data_url}]);
        }
        request
    } else {
        json!({"model":"mock-model", "input":"draw"})
    };
    let mut explicit = minimal.clone();
    if api == "images" {
        explicit["stream"] = json!(false);
        explicit["quality"] = Value::Null;
        explicit["n"] = json!(0);
    } else {
        explicit["stream"] = json!(false);
        explicit["store"] = json!(true);
        explicit["parallel_tool_calls"] = json!(false);
    }
    full["headers"] = json!({"authorization":"caller-secret"});
    full["account_id"] = json!("caller-account");
    for mut source in [full, minimal, explicit] {
        let (logged, bytes) =
            exchange(&mut ws, image_request(operation, api, source.clone())).await?;
        assert_eq!(bytes, returned.as_bytes());
        let actual: Value = serde_json::from_str(logged["body"].as_str().expect("wire body"))?;
        assert!(actual.get("headers").is_none());
        assert!(actual.get("account_id").is_none());
        if api == "images" {
            let expected = source.as_object_mut().expect("object");
            expected.remove("headers");
            expected.remove("account_id");
            if expected.get("quality").is_some_and(Value::is_null) {
                expected.remove("quality");
            }
            assert_eq!(actual, source);
        } else {
            assert_eq!(actual["model"], source["model"]);
            assert_eq!(actual["stream"], json!(true));
            assert_eq!(
                actual["store"],
                source.get("store").cloned().unwrap_or(json!(false))
            );
            assert_eq!(
                actual["parallel_tool_calls"],
                source
                    .get("parallel_tool_calls")
                    .cloned()
                    .unwrap_or(json!(true))
            );
            assert_eq!(
                actual["prompt_cache_key"],
                actual["client_metadata"]["session_id"]
            );
            assert!(actual.get("instructions").is_none());
            if source.get("tools").is_some() {
                assert_eq!(actual["tools"], source["tools"]);
                assert_eq!(actual["extension"], source["extension"]);
                assert!(actual["input"].to_string().contains(&data_url));
            } else {
                assert_eq!(actual["input"][0]["type"], json!("message"));
                assert_eq!(
                    actual["input"][0]["content"],
                    json!([{"type":"input_text", "text":"draw"}])
                );
            }
        }
        assert_eq!(
            logged["url"],
            json!(format!("{}{endpoint}", upstream.uri()))
        );
        assert_eq!(logged["requestId"], json!("r"));
    }
    let calls: Vec<_> = upstream
        .received_requests()
        .await
        .expect("HTTP records")
        .into_iter()
        .filter(|request| request.url.path() == endpoint)
        .collect();
    assert_eq!(calls.len(), 3);
    for call in calls {
        assert_eq!(call.headers["authorization"], "Bearer fake-cpa-access");
        assert_eq!(call.headers["originator"], "codex-tui");
        assert!(call.headers.get("session-id").is_some());
    }
    runtime
        .wait_for_json_log_event("cpa.request.completed", Duration::from_secs(10))
        .await?;
    Ok(())
}

#[test_case("images"; "images")]
#[test_case("responses"; "responses")]
#[tokio::test]
async fn cpa_images_errors_and_cancellation_do_not_retry_or_execute_elsewhere(
    api: &str,
) -> Result<()> {
    let upstream = MockServer::start().await;
    let endpoint = if api == "images" {
        "/v1/images/edits"
    } else {
        "/v1/responses"
    };
    Mock::given(method("POST"))
        .and(path(endpoint))
        .respond_with(
            ResponseTemplate::new(503)
                .insert_header("retry-after", "7")
                .set_body_string("image upstream failure"),
        )
        .expect(1)
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    let _runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    send(&mut ws, image_request("images/edits", api, json!({"model":"mock-model", "prompt":"draw", "images":[{"file_id":"file-1"}], "input":"draw"}))).await?;
    let response = receive(&mut ws, "id", json!(2)).await?;
    assert_eq!(response["error"]["data"]["httpStatus"], json!(503));
    assert_eq!(
        response["error"]["data"]["body"],
        json!("image upstream failure")
    );
    upstream.verify().await;
    upstream.reset().await;
    Mock::given(method("POST"))
        .and(path(endpoint))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(120)))
        .expect(1)
        .mount(&upstream)
        .await;
    send(&mut ws, image_request("images/edits", api, json!({"model":"mock-model", "prompt":"draw", "images":[{"file_id":"file-1"}], "input":"draw"}))).await?;
    timeout(Duration::from_secs(30), async {
        loop {
            if upstream
                .received_requests()
                .await
                .expect("requests")
                .iter()
                .any(|request| request.url.path() == endpoint)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    send(
        &mut ws,
        json!({"id":4,"method":"cpa/inference/cancel","params":{"requestId":"r"}}),
    )
    .await?;
    let error = receive(&mut ws, "id", json!(2)).await?;
    assert_eq!(error["error"]["data"]["httpStatus"], json!(499));
    Ok(())
}

#[tokio::test]
async fn cpa_images_invalid_native_types_and_compact_never_reach_model_http() -> Result<()> {
    let upstream = MockServer::start().await;
    let home = TempDir::new()?;
    let _runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    for request in [
        image_request("images/generations", "responses", json!({"input":"draw"})),
        image_request("images/generations", "images", json!({"prompt":"draw"})),
        image_request(
            "images/generations",
            "images",
            json!({"model":"gpt-image-1.5"}),
        ),
        image_request(
            "images/edits",
            "images",
            json!({"model":"gpt-image-1.5", "prompt":"draw"}),
        ),
        image_request(
            "images/edits",
            "images",
            json!({"model":"gpt-image-1.5", "prompt":"draw", "images":[{"invalid":true}]}),
        ),
        image_request(
            "images/generations",
            "images",
            json!({"model":"gpt-image-1.5", "prompt":"draw", "stream":"true"}),
        ),
        image_request(
            "images/generations",
            "responses",
            json!({"model":"mock-model", "input":[{"type":"unknown"}]}),
        ),
        image_request(
            "images/generations",
            "responses",
            json!({"model":"mock-model", "tools":[{"type":"unknown"}]}),
        ),
        image_request(
            "responses/compact",
            "responses",
            json!({"model":"mock-model", "input":[]}),
        ),
    ] {
        send(&mut ws, request).await?;
        assert_eq!(
            receive(&mut ws, "id", json!(2)).await?["error"]["data"]["httpStatus"],
            json!(400)
        );
    }
    assert!(
        upstream
            .received_requests()
            .await
            .expect("HTTP records")
            .iter()
            .all(|request| !request.url.path().contains("/images/")
                && request.url.path() != "/v1/responses")
    );
    Ok(())
}

#[tokio::test]
async fn cpa_responses_summary_compaction_and_builtin_images_still_use_native_responses()
-> Result<()> {
    let upstream = MockServer::start().await;
    let raw = "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"compaction\",\"encrypted_content\":\"fixture-result\"}}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"compaction\",\"encrypted_content\":\"fixture-result\"}]}}\n\n";
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(raw, "text/event-stream"))
        .expect(1)
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    let _runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let (logged, returned) = exchange(&mut ws, start(json!({"model":"mock-model", "stream":true,
        "input":[{"type":"compaction", "id":"cmp_history", "encrypted_content":"fixture-history"},
            {"type":"message","role":"user","content":[{"type":"input_text","text":"Summarize the preceding conversation."}]},
            {"type":"compaction_trigger"}],
        "tools":[{"type":"image_generation", "output_format":"png"}]}))).await?;
    let body: Value = serde_json::from_str(logged["body"].as_str().expect("body"))?;
    let input = body["input"].as_array().expect("input");
    assert!(
        input
            .iter()
            .any(|item| item["type"] == "compaction_trigger")
    );
    assert!(
        input
            .iter()
            .any(|item| item["encrypted_content"] == "fixture-history")
    );
    assert!(
        body.to_string()
            .contains("Summarize the preceding conversation.")
    );
    assert_eq!(body["tools"][0]["type"], json!("image_generation"));
    assert_eq!(returned, raw.as_bytes());
    Ok(())
}
