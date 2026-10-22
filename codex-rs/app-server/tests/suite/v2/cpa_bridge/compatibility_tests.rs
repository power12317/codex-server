//! Compatibility at the mainline v3 HTTP sender boundary, without local tools.
use super::*;
use base64::Engine;
use pretty_assertions::assert_eq;

async fn exchange_compatible(ws: &mut Socket, request: Value) -> Result<(Value, Vec<u8>)> {
    send(ws, request).await?;
    let mut body = Vec::new();
    let mut logged = Value::Null;
    timeout(Duration::from_secs(30), async {
        while let Some(message) = ws.next().await {
            let Message::Text(text) = message? else {
                continue;
            };
            let value: Value = serde_json::from_str(&text)?;
            anyhow::ensure!(value.get("error").is_none(), "{value}");
            match value["method"].as_str() {
                Some("cpa/inference/upstream") if value["params"]["kind"] == "request" => {
                    logged = value["params"].clone();
                }
                Some("cpa/inference/body") => body.extend(
                    base64::engine::general_purpose::STANDARD
                        .decode(value["params"]["bodyBase64"].as_str().unwrap())?,
                ),
                Some("cpa/inference/error") => anyhow::bail!("{value}"),
                Some("cpa/inference/completed") => return Ok((logged, body)),
                _ => {}
            }
        }
        anyhow::bail!("connection closed before completion")
    })
    .await?
}

#[test_case::test_case(false; "responses")]
#[test_case::test_case(true; "responses_lite")]
#[tokio::test]
async fn cpa_main_compatible_controls_schemas_and_history(lite: bool) -> Result<()> {
    let upstream = MockServer::start().await;
    let raw = "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"compatible\",\"output\":[]}}\n\n";
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(raw, "text/event-stream"))
        .expect(2)
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    prepare_account(&upstream, &home)?;
    let config = core_test_support::load_default_config_for_test(&home).await;
    let mut model = codex_core::test_support::construct_model_info_offline("mock-model", &config);
    model.use_responses_lite = lite;
    let catalog = home.path().join("models.json");
    std::fs::write(&catalog, serde_json::to_vec(&json!({"models":[model]}))?)?;
    let cfg = home.path().join("state/shared.json/config.toml");
    let original = std::fs::read_to_string(&cfg)?;
    std::fs::write(
        &cfg,
        format!(
            "model_catalog_json = {}\n{original}",
            serde_json::to_string(&catalog)?
        ),
    )?;
    let _runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let schema = json!({"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"n":{"type":"integer","const":9007199254740993u64,"minimum":0},"query":{"type":"string","pattern":"[a-z]+","default":"a"}},"required":["n"],"additionalProperties":false,"future_constraint":{"nested":[false,9007199254740993u64]}});
    let output = json!({"type":"object","properties":{"result":{"const":9007199254740993u64}},"required":["result"]});
    for choice in [
        json!("none"),
        json!({"type":"function","function":{"name":"Read"},"namespace":"external"}),
    ] {
        let mut request = start(
            json!({"model":"mock-model","stream":false,"store":true,"include":[],"parallel_tool_calls":true,
            "background":false,"generate":true,"previous_response_id":null,"conversation":null,
            "input":[{"role":"user","content":"hello","status":"rendered","metadata":{"display":true}},
                {"type":"function_call","name":"Read","namespace":"external","call_id":"call-1","arguments":"{}","status":"completed"},
                {"type":"function_call_output","name":"Read","namespace":"external","output":"client result"}],
            "tools":[{"type":"namespace","name":"external","metadata":{"display":true},"tools":[{"function":{"name":"Read","input_schema":schema,"output_schema":output,"defer_loading":null}}]},
                {"type":"function","name":"WebSearch"},{"type":"function","name":"ToolSearch"},{"type":"function","name":"Bash"},{"type":"function","name":"Agent"}],
            "tool_choice":choice,"text":{"format":{"type":"json_schema","name":"caller_result","strict":false,"schema":output},"verbosity":"high"},
            "reasoning":{"context":"current_turn","effort":"high"},"temperature":0.4,"top_p":0.7,"max_completion_tokens":123,"service_tier":"flex",
            "client_metadata":{"session_id":"source-root","thread_id":"source-child","thread_source":"third-party",
                "business_tag":"kept","ws_request_header_x_openai_internal_codex_responses_lite":"stale","ws_stream_request_start_ms":"old","arbitrary_tracking":{"display":true}},
            "headers":{"authorization":"caller-secret"},"future_extension":{"number":9007199254740993u64}}),
        );
        request["params"]["optionalEnvelopeExtension"] = json!({"trace":true});
        let (logged, bytes) = exchange_compatible(&mut ws, request).await?;
        assert_eq!(bytes, raw.as_bytes());
        let sent: Value = serde_json::from_str(logged["body"].as_str().unwrap())?;
        assert_eq!(sent["store"], true);
        assert_eq!(sent["parallel_tool_calls"], true);
        assert_eq!(sent["include"], json!([]));
        assert_eq!(sent["temperature"], 0.4);
        assert_eq!(sent["top_p"], 0.7);
        assert_eq!(sent["max_output_tokens"], 123);
        assert_eq!(sent["service_tier"], "flex");
        assert_eq!(
            sent["text"]["format"],
            json!({"type":"json_schema","name":"caller_result","strict":false,"schema":output})
        );
        assert_eq!(sent["text"]["verbosity"], "high");
        assert_eq!(sent["reasoning"]["context"], "current_turn");
        let tools = if lite {
            &sent["input"][0]["tools"]
        } else {
            &sent["tools"]
        };
        let namespace = tools
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "external")
            .unwrap();
        assert_eq!(namespace["tools"][0]["parameters"], schema);
        assert_eq!(namespace["tools"][0]["output_schema"], output);
        assert_eq!(namespace["tools"][0]["strict"], false);
        let output_item = sent["input"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["type"] == "function_call_output")
            .unwrap();
        assert_eq!(output_item["call_id"], "call-1");
        assert_eq!(output_item["output"], "client result");
        assert_eq!(sent["future_extension"]["number"], 9007199254740993u64);
        assert_eq!(sent["client_metadata"]["business_tag"], "kept");
        assert!(sent["client_metadata"].get("arbitrary_tracking").is_none());
        assert!(!sent.to_string().contains("stale"));
        assert!(!sent.to_string().contains("caller-secret"));
        let actual = upstream
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.url.path() == "/v1/responses")
            .last()
            .unwrap();
        assert_eq!(actual.body_json::<Value>()?, sent);
    }
    Ok(())
}

#[tokio::test]
async fn cpa_main_plain_formats_and_default_state_controls() -> Result<()> {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw("data: [DONE]\n\n", "text/event-stream"),
        )
        .expect(2)
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    let _runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    for (kind, reference) in [("text", Value::Null), ("json_object", json!(""))] {
        let (logged,_)=exchange_compatible(&mut ws,start(json!({"model":"mock-model","input":"hello","stream":false,"store":true,"tool_choice":"none","include":[],"parallel_tool_calls":false,"text":{"format":{"type":kind}},"generate":true,"previous_response_id":reference,"background":false}))).await?;
        let sent: Value = serde_json::from_str(logged["body"].as_str().unwrap())?;
        assert_eq!(sent["text"]["format"], json!({"type":kind}));
        assert_eq!(sent["tool_choice"], "none");
        assert_eq!(sent["parallel_tool_calls"], false);
        assert_eq!(sent["include"], json!([]));
        assert_eq!(sent["store"], true);
        assert!(sent.get("previous_response_id").is_none());
        assert!(sent.get("generate").is_none());
    }
    Ok(())
}

#[test_case::test_case("generate"; "no_http_prewarm")]
#[test_case::test_case("history"; "unresolved_history")]
#[test_case::test_case("background"; "no_async_retrieval")]
#[test_case::test_case("ambiguous"; "ambiguous_association")]
#[test_case::test_case("tokens"; "conflicting_aliases")]
#[tokio::test]
async fn cpa_main_reports_specific_unexpressible_requests_without_model_send(
    case: &str,
) -> Result<()> {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    let _runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let mut body = json!({"model":"mock-model","input":[]});
    let expected = match case {
        "generate" => {
            body["generate"] = json!(false);
            "WebSocket prewarming"
        }
        "history" => {
            body["previous_response_id"] = json!("unresolved");
            "reconstructed caller history"
        }
        "background" => {
            body["background"] = json!(true);
            "asynchronous response retrieval"
        }
        "tokens" => {
            body["max_tokens"] = json!(3);
            body["max_output_tokens"] = json!(4);
            "conflicting max_output_tokens"
        }
        _ => {
            body["input"] = json!([
            {"type":"function_call","name":"Read","call_id":"a","arguments":"{}"},
            {"type":"function_call","name":"Read","call_id":"b","arguments":"{}"},
            {"type":"function_call_output","output":"ambiguous"}]);
            "ambiguous"
        }
    };
    send(&mut ws, start(body)).await?;
    let result = receive(&mut ws, "id", json!(2)).await?;
    assert_eq!(result["error"]["data"]["httpStatus"], 400);
    assert!(
        result["error"]["message"]
            .as_str()
            .unwrap()
            .contains(expected),
        "{result}"
    );
    let error: Value = serde_json::from_str(result["error"]["data"]["body"].as_str().unwrap())?;
    assert_eq!(error["error"]["type"], "request_adaptation_error");
    Ok(())
}
