//! Real master/worker compatibility regressions at the native sender boundary.
use super::*;
use pretty_assertions::assert_eq;

#[test_case::test_case(false; "responses")]
#[test_case::test_case(true; "responses_lite")]
#[tokio::test]
async fn cpa_native_compatible_controls_and_tool_contracts(lite: bool) -> Result<()> {
    let upstream = MockServer::start().await;
    let home = TempDir::new()?;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            completed(json!({"id":"compat","output":[]})),
            "text/event-stream",
        ))
        .expect(4)
        .mount(&upstream)
        .await;
    prepare_account(&upstream, &home)?;
    let config = core_test_support::load_default_config_for_test(&home).await;
    let mut model = codex_core::test_support::construct_model_info_offline("mock-model", &config);
    model.use_responses_lite = lite;
    let catalog = home.path().join("models.json");
    std::fs::write(&catalog, serde_json::to_vec(&json!({"models":[model]}))?)?;
    let config_path = home.path().join("state/shared.json/config.toml");
    let original = std::fs::read_to_string(&config_path)?;
    std::fs::write(
        config_path,
        format!(
            "model_catalog_json = {}\n{original}",
            serde_json::to_string(&catalog)?
        ),
    )?;
    let _runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let schema = json!({"$schema":"http://json-schema.org/draft-07/schema#","type":"object","properties":{"query":{"type":"string","pattern":"[a-z]+","minLength":2}},"required":["query"],"additionalProperties":false});
    let output_schema = json!({"type":"object","properties":{"ok":{"const":true}},"required":["ok"],"additionalProperties":false});
    for (index, choice) in [
        json!("none"),
        json!("required"),
        json!({"type":"function","function":{"name":"WebSearch"}}),
        json!({"type":"custom","name":"Bash"}),
    ]
    .into_iter()
    .enumerate()
    {
        let input = json!({"model":"mock-model","input":[{"role":"user","content":"Use the caller's tools.","status":"rendered","metadata":{"ui":true}}],
            "tools":[{"type":"function","metadata":{"display":true},"function":{"name":"WebSearch","parameters":schema,"output_schema":output_schema}}, {"type":"custom","name":"Bash","format":{"type":"text","metadata":{"ui":true}}}],
            "tool_choice":choice,"parallel_tool_calls":true,"include":[],"store":true,"background":false,"previous_response_id":null,"conversation":null,
            "text":{"format":{"type":"json_schema","name":"caller_result","strict":false,"schema":output_schema},"verbosity":"high","metadata":{"ui":true}},
            "reasoning":{"context":"current_turn","effort":"high"},"max_output_tokens":123,"temperature":0.4,"top_p":0.7,"truncation":"disabled","prompt_cache_retention":"24h","service_tier":"flex",
            "client_metadata":{"session_id":"stable","thread_id":"child","thread_source":"third-party","ws_request_header_x_openai_internal_codex_responses_lite":"stale","ws_stream_request_start_ms":"old","arbitrary_tracking":{"display":true}}});
        let mut request = rpc(input, 8000 + index as u64);
        request["params"]["optionalEnvelopeExtension"] = json!({"trace":true});
        let result = exchange(&mut ws, request).await?;
        assert!(result.error.is_none(), "{:?}", result.error);
        assert_eq!(
            result.reply["result"]["statusCode"], 200,
            "{}",
            result.reply
        );
        let sent = wire(&result);
        assert_eq!(sent["include"], json!([]));
        assert_eq!(sent["parallel_tool_calls"], !lite);
        assert_eq!(sent["store"], true);
        assert_eq!(sent["max_output_tokens"], 123);
        assert_eq!(sent["temperature"], 0.4);
        assert_eq!(sent["top_p"], 0.7);
        assert_eq!(sent["service_tier"], "flex");
        assert_eq!(sent["text"]["format"]["name"], "caller_result");
        assert_eq!(sent["text"]["format"]["strict"], false);
        assert_eq!(sent["text"]["format"]["schema"], output_schema);
        assert_eq!(sent["text"]["verbosity"], "high");
        assert_eq!(sent["reasoning"]["context"], "current_turn");
        let expected_choice = if index == 2 {
            json!({"type":"function","name":"WebSearch"})
        } else {
            choice
        };
        assert_eq!(sent["tool_choice"], expected_choice);
        let declaration = if lite {
            &sent["input"][0]["tools"][0]["tools"][0]
        } else {
            &sent["tools"][0]
        };
        assert_eq!(declaration["name"], "WebSearch");
        assert_eq!(declaration["parameters"], schema);
        assert_eq!(declaration["strict"], false);
        assert_eq!(declaration["output_schema"], output_schema);
        assert!(!sent.to_string().contains("old"));
        assert!(sent["client_metadata"].get("arbitrary_tracking").is_none());
    }
    Ok(())
}

#[tokio::test]
async fn cpa_native_plain_formats_and_explicit_empty_controls() -> Result<()> {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            completed(json!({"id":"text","output":[]})),
            "text/event-stream",
        ))
        .expect(2)
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    let _runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    for (i, kind) in ["text", "json_object"].into_iter().enumerate() {
        let result=exchange(&mut ws,rpc(json!({"model":"mock-model","input":"hello","tool_choice":"none","include":[],"parallel_tool_calls":false,"text":{"format":{"type":kind}},"stream_options":{"reasoning_summary_delivery":null,"include_usage":false}}),8100+i as u64)).await?;
        assert_eq!(
            result.reply["result"]["statusCode"], 200,
            "{}",
            result.reply
        );
        let sent = wire(&result);
        assert_eq!(sent["tool_choice"], "none");
        assert_eq!(sent["parallel_tool_calls"], false);
        assert_eq!(sent["text"]["format"], json!({"type":kind}));
        assert_eq!(sent["include"], json!([]));
    }
    Ok(())
}

#[tokio::test]
async fn cpa_native_images_keep_explicit_output_mask_and_partial_controls() -> Result<()> {
    let upstream = MockServer::start().await;
    for path_ in ["generations", "edits"] {
        Mock::given(method("POST"))
            .and(path(format!("/v1/images/{path_}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"created":1,"data":[{"b64_json":"image"}]})),
            )
            .mount(&upstream)
            .await;
    }
    let home = TempDir::new()?;
    let _runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    for (i, operation) in ["generations", "edits"].into_iter().enumerate() {
        let mut input = json!({"model":"gpt-image-2","prompt":"fixture","stream":false,"output_format":"webp","output_compression":80,"partial_images":2});
        if operation == "edits" {
            input["images"] = json!([{"file_id":"file-original"}]);
            input["mask"] = json!({"file_id":"file-mask"});
        }
        let mut request = rpc(input.clone(), 8200 + i as u64);
        request["params"]["operation"] = json!(format!("images/{operation}"));
        request["params"]["imageApi"] = json!("images");
        let result = exchange(&mut ws, request).await?;
        assert!(result.error.is_none(), "{:?}", result.error);
        assert_eq!(
            result.reply["result"]["statusCode"], 200,
            "{}",
            result.reply
        );
        let sent = wire(&result);
        for (key, value) in input.as_object().unwrap() {
            assert_eq!(&sent[key], value, "{key}");
        }
    }
    Ok(())
}

#[tokio::test]
async fn cpa_native_infers_single_missing_call_id_and_rejects_ambiguity() -> Result<()> {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            completed(json!({"id":"associated","output":[]})),
            "text/event-stream",
        ))
        .expect(1)
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    let _runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let call = json!({"type":"function_call","name":"Read","call_id":"call-1","arguments":"{}"});
    let output = json!({"type":"function_call_output","output":"client result"});
    let result=exchange(&mut ws,rpc(json!({"model":"mock-model","input":[call.clone(),output.clone()],"max_completion_tokens":17}),8400)).await?;
    assert_eq!(
        result.reply["result"]["statusCode"], 200,
        "{}",
        result.reply
    );
    let sent = wire(&result);
    assert_eq!(sent["input"][1]["call_id"], "call-1");
    assert_eq!(sent["input"][1]["output"], "client result");
    assert_eq!(sent["max_output_tokens"], 17);
    let mut second = call.clone();
    second["call_id"] = json!("call-2");
    let result = exchange(
        &mut ws,
        rpc(
            json!({"model":"mock-model","input":[call,second,output]}),
            8401,
        ),
    )
    .await?;
    assert_eq!(result.reply["error"]["data"]["httpStatus"], 400);
    assert!(
        result.reply["error"]["message"]
            .as_str()
            .unwrap()
            .contains("ambiguous")
    );
    assert!(result.body.is_empty());
    Ok(())
}

#[test_case::test_case("json_error"; "json_error")]
#[test_case::test_case("malformed_json_success"; "malformed_json_success")]
#[test_case::test_case("partial_eof"; "partial_eof")]
#[test_case::test_case("invalid_completed"; "invalid_completed")]
#[test_case::test_case("empty_json_data"; "empty_json_data")]
#[test_case::test_case("empty_completed_data"; "empty_completed_data")]
#[test_case::test_case("malformed_completed_data"; "malformed_completed_data")]
#[test_case::test_case("empty_completed_image"; "empty_completed_image")]
#[tokio::test]
async fn cpa_native_images_require_real_success(case: &str) -> Result<()> {
    let upstream = MockServer::start().await;
    let (content_type, body) = match case {
        "json_error" => (
            "application/json",
            json!({"error":{"message":"original image failure","code":"image_fixture"}})
                .to_string(),
        ),
        "malformed_json_success" => (
            "application/json",
            json!({"created":1,"data":[{"unrecognized":"no image result"}]}).to_string(),
        ),
        "partial_eof" => (
            "text/event-stream",
            "data: {\"type\":\"image_generation.partial_image\",\"b64_json\":\"partial\"}\n\n"
                .into(),
        ),
        "empty_json_data" => (
            "application/json",
            json!({"created":1,"data":[]}).to_string(),
        ),
        "empty_completed_data" => (
            "text/event-stream",
            "data: {\"type\":\"image_generation.completed\",\"data\":[]}\n\n".into(),
        ),
        "malformed_completed_data" => (
            "text/event-stream",
            "data: {\"type\":\"image_generation.completed\",\"data\":[{}]}\n\n".into(),
        ),
        "empty_completed_image" => (
            "text/event-stream",
            "data: {\"type\":\"image_generation.completed\",\"b64_json\":\"\"}\n\n".into(),
        ),
        _ => (
            "text/event-stream",
            "data: {\"type\":\"image_generation.completed\"}\n\n".into(),
        ),
    };
    Mock::given(method("POST"))
        .and(path("/v1/images/generations"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, content_type))
        .expect(1)
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    let _runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let mut request = rpc(
        json!({"model":"gpt-image-2","prompt":"fixture","stream":true}),
        8600,
    );
    request["params"]["operation"] = json!("images/generations");
    request["params"]["imageApi"] = json!("images");
    let result = exchange(&mut ws, request).await?;
    assert!(
        result.error.is_some() || result.reply.get("error").is_some(),
        "{case} must not become success"
    );
    if case == "partial_eof" {
        assert!(std::str::from_utf8(&result.body)?.contains("partial_image"));
    } else {
        assert!(result.body.is_empty());
    }
    if case == "json_error" {
        assert!(
            result.error.unwrap()["body"]
                .as_str()
                .unwrap()
                .contains("original image failure")
        );
    }
    Ok(())
}
