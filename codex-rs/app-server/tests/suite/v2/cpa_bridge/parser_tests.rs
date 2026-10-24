use super::continuity_tests::exchange;
use super::*;
use core_test_support::load_default_config_for_test;
use pretty_assertions::assert_eq;
use test_case::test_case;

#[test_case(false; "responses")]
#[test_case(true; "responses_lite")]
#[tokio::test]
async fn native_tool_catalog_and_namespace_history_reach_wire(lite: bool) -> Result<()> {
    let upstream = MockServer::start().await;
    let raw = "event: fixture\r\ndata: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"namespace\":\"collaboration\",\"name\":\"send_message\",\"call_id\":\"next-call\",\"arguments\":\"{}\"}}\r\n\r\ndata: [DONE]\n\n";
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(raw, "text/event-stream"))
        .expect(2)
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    prepare_account(&upstream, &home)?;
    let config = load_default_config_for_test(&home).await;
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
    let mut tools: Value = serde_json::from_str(include_str!("fixtures/tools.json"))?;
    // Exercise optional native fields in addition to the sanitized incident fixture.
    tools[3]["tools"][0]["defer_loading"] = json!(true);
    let history = json!([
        {"type":"function_call", "id":"fc_prior", "namespace":"collaboration", "name":"send_message", "call_id":"prior-call", "arguments":"{}"},
        {"type":"function_call_output", "call_id":"prior-call", "output":"fixture result"}
    ]);
    let choice = json!({"type":"function", "namespace":"collaboration", "name":"send_message"});
    let request = start(
        json!({"model":"mock-model", "input":history, "tools":tools, "tool_choice":choice, "session_id":"session-a", "thread_id":"session-a", "prompt_cache_key":"cache-c"}),
    );
    for _ in 0..2 {
        let (logged, returned) = exchange(&mut ws, request.clone()).await?;
        assert_eq!(returned, raw.as_bytes());
        let body: Value = serde_json::from_str(
            logged["body"]
                .as_str()
                .expect("fixture response must contain the expected native fields"),
        )?;
        // Assertions use the HTTP mock's received body, not only preparation logs.
        let actual = upstream
            .received_requests()
            .await
            .expect("fixture response must contain the expected native fields")
            .into_iter()
            .filter(|r| r.url.path() == "/v1/responses")
            .last()
            .expect("fixture response must contain the expected native fields");
        assert_eq!(actual.body_json::<Value>()?, body);
        assert_eq!(body["tool_choice"], choice);
        let sent_tools = if lite {
            assert!(body.get("tools").is_none());
            &body["input"]
                .as_array()
                .expect("fixture response must contain the expected native fields")
                .iter()
                .find(|item| item["type"] == "additional_tools")
                .expect("fixture response must contain the expected native fields")["tools"]
        } else {
            &body["tools"]
        };
        let expected = if lite {
            json!([
                {"type":"namespace", "name":"functions", "description":"", "tools":[tools[0], tools[1], tools[2]]},
                tools[3], tools[4], tools[5]
            ])
        } else {
            tools.clone()
        };
        assert_eq!(sent_tools, &expected);
        let calls: Vec<_> = body["input"]
            .as_array()
            .expect("fixture response must contain the expected native fields")
            .iter()
            .filter(|item| item.get("call_id").is_some())
            .cloned()
            .collect();
        assert_eq!(json!(calls), history);
        let identity = body["client_metadata"]["session_id"].clone();
        assert_eq!(body["client_metadata"]["thread_id"], identity);
        assert_eq!(body["prompt_cache_key"], identity);
        assert_eq!(logged["headers"]["session-id"], json!([identity]));
        assert_eq!(
            logged["headers"]["thread-id"],
            logged["headers"]["session-id"]
        );
    }
    Ok(())
}

#[test_case("Etc/UTC"; "utc")]
#[test_case("Asia/Singapore"; "singapore")]
#[test_case("Asia/Tokyo"; "tokyo")]
#[tokio::test]
#[cfg(unix)]
async fn runtime_timezone_replaces_current_context_only(timezone: &str) -> Result<()> {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw("data: [DONE]\n\n", "text/event-stream"),
        )
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    prepare_account(&upstream, &home)?;
    let _runtime = launch_with_timezone(&upstream, &home, Some(timezone)).await?;
    let mut ws = connect(&home).await?;
    let old = "<environment_context><current_date>2020-01-01</current_date><timezone>old-zone</timezone></environment_context>";
    let request = start(
        json!({"model":"mock-model", "timezone":"old-zone", "client_metadata":{"timezone":"old-zone"}, "input":[
            {"role":"user", "content":old}, {"role":"assistant", "content":"past answer"},
            {"role":"user", "content":"<environment_context><cwd>C:\\work</cwd><os>Windows</os><current_date>2020-01-01</current_date><timezone>old-zone</timezone></environment_context>"}
        ]}),
    );
    let (logged, _) = exchange(&mut ws, request).await?;
    let body: Value = serde_json::from_str(
        logged["body"]
            .as_str()
            .expect("fixture response must contain the expected native fields"),
    )?;
    let texts: Vec<_> = body["input"]
        .as_array()
        .expect("fixture response must contain the expected native fields")
        .iter()
        .flat_map(|item| item["content"].as_array().into_iter().flatten())
        .filter_map(|item| item["text"].as_str())
        .collect();
    assert_eq!(texts[0], old);
    let current = texts
        .last()
        .expect("fixture response must contain the expected native fields");
    assert!(
        current.contains(&format!("<timezone>{timezone}</timezone>")),
        "{current}"
    );
    assert!(current.contains("<cwd>C:\\work</cwd><os>Windows</os>"));
    assert!(!current.contains("2020-01-01"));
    assert_eq!(body["client_metadata"]["timezone"], json!(timezone));
    assert!(body.get("timezone").is_none());
    Ok(())
}

#[tokio::test]
async fn unsupported_tool_error_identifies_array_location() -> Result<()> {
    let upstream = MockServer::start().await;
    let home = TempDir::new()?;
    let _runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    send(&mut ws, start(json!({"model":"mock-model", "stream":true, "tools":[{"type":"function", "name":"run"}, {"type":"computer"}]}))).await?;
    let response = receive(&mut ws, "id", json!(2)).await?;
    assert_eq!(response["error"]["data"]["httpStatus"], json!(400));
    assert!(
        response["error"]["message"]
            .as_str()
            .expect("fixture response must contain the expected native fields")
            .contains("tools[1]: unknown variant"),
        "{response}"
    );
    assert!(
        upstream
            .received_requests()
            .await
            .expect("fixture response must contain the expected native fields")
            .iter()
            .all(|request| request.url.path() != "/v1/responses")
    );
    Ok(())
}

#[tokio::test]
async fn concurrent_requests_without_turn_id_share_synthetic_context() -> Result<()> {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-codex-turn-state", "shared-state")
                .set_body_raw("data: [DONE]\n\n", "text/event-stream"),
        )
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    let _runtime = server(&upstream, &home).await?;
    let mut first_ws = connect(&home).await?;
    let mut second_ws = connect(&home).await?;
    let request = start(json!({"model":"mock-model", "session_id":"shared-session", "input":[]}));
    let mut second_request = request.clone();
    second_request["params"]["requestId"] = json!("second-request");
    let ((first, _), (second, _)) = tokio::try_join!(
        exchange(&mut first_ws, request.clone()),
        exchange(&mut second_ws, second_request)
    )?;
    let first_body: Value = serde_json::from_str(
        first["body"]
            .as_str()
            .expect("fixture response must contain the expected native fields"),
    )?;
    let second_body: Value = serde_json::from_str(
        second["body"]
            .as_str()
            .expect("fixture response must contain the expected native fields"),
    )?;
    assert_eq!(
        first_body["client_metadata"],
        second_body["client_metadata"]
    );
    uuid::Uuid::parse_str(
        first_body["client_metadata"]["turn_id"]
            .as_str()
            .expect("fixture response must contain the expected native fields"),
    )?;
    let mut same_conversation = request.clone();
    same_conversation["params"]["request"]["thread_id"] = json!("shared-session");
    let (next, _) = exchange(&mut first_ws, same_conversation).await?;
    assert_eq!(
        next["headers"]["x-codex-turn-state"],
        json!(["shared-state"])
    );
    let mut separate = request;
    separate["params"]["request"]["session_id"] = json!("separate-session");
    let (next, _) = exchange(&mut first_ws, separate).await?;
    let next_body: Value = serde_json::from_str(
        next["body"]
            .as_str()
            .expect("fixture response must contain the expected native fields"),
    )?;
    assert_ne!(
        first_body["client_metadata"]["session_id"],
        next_body["client_metadata"]["session_id"]
    );
    assert!(next["headers"].get("x-codex-turn-state").is_none());
    Ok(())
}
