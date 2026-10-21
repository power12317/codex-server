//! The WorkBuddy incident's complete tool catalog must reach the native sender.
use super::*;
use base64::Engine;
use core_test_support::load_default_config_for_test;
use pretty_assertions::assert_eq;
use test_case::test_case;

async fn schema_exchange(ws: &mut Socket, request: Value) -> Result<Vec<Value>> {
    let request_id = request["params"]["requestId"].clone();
    let id = request["id"].clone();
    send(ws, request).await?;
    let mut bytes = Vec::new();
    timeout(Duration::from_secs(30), async {
        while let Some(message) = ws.next().await {
            let Message::Text(text) = message? else {
                continue;
            };
            let message: Value = serde_json::from_str(&text)?;
            if message["id"] == id {
                anyhow::ensure!(message.get("error").is_none(), "{message}");
            }
            if message["params"]["requestId"] != request_id {
                continue;
            }
            match message["method"].as_str() {
                Some("cpa/inference/body" | "cpa/inference/event") => bytes.extend(
                    base64::engine::general_purpose::STANDARD
                        .decode(message["params"]["bodyBase64"].as_str().unwrap())?,
                ),
                Some("cpa/inference/error") => anyhow::bail!("{message}"),
                Some("cpa/inference/completed") => break,
                _ => {}
            }
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    String::from_utf8(bytes)?
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter(|data| *data != "[DONE]")
        .map(|data| serde_json::from_str(data).map_err(Into::into))
        .collect()
}

#[test_case(false; "responses")]
#[test_case(true; "responses_lite")]
#[tokio::test]
async fn cpa_workbuddy_schemas_and_client_tool_round_trip(lite: bool) -> Result<()> {
    let upstream = MockServer::start().await;
    let home = TempDir::new()?;
    let marker = home.path().join("bash-must-not-run");
    let arguments = json!({"command": format!("touch {}", marker.display())}).to_string();
    let call = json!({"type":"function_call", "id":"fc_workbuddy", "name":"Bash", "call_id":"workbuddy-call", "arguments":arguments});
    let delta_split = arguments.find(':').unwrap() + 1;
    let events = [
        json!({"type":"response.created", "response":{"id":"resp_workbuddy"}}),
        json!({"type":"response.output_item.added", "output_index":0, "item":{"type":"function_call", "id":"fc_workbuddy", "name":"Bash", "call_id":"workbuddy-call", "arguments":""}}),
        json!({"type":"response.function_call_arguments.delta", "output_index":0, "item_id":"fc_workbuddy", "delta":arguments[..delta_split]}),
        json!({"type":"response.function_call_arguments.delta", "output_index":0, "item_id":"fc_workbuddy", "delta":arguments[delta_split..]}),
        json!({"type":"response.output_item.done", "output_index":0, "item":call}),
        json!({"type":"response.completed", "response":{"id":"resp_workbuddy", "output":[call], "usage":{"input_tokens":11, "output_tokens":2, "total_tokens":13}}}),
    ];
    let sse = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
        .expect(2)
        .mount(&upstream)
        .await;
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

    // Only the exact tool declarations are copied from the incident log; no
    // business messages, request headers or credentials belong in this fixture.
    let nested: Vec<Value> = serde_json::from_str(include_str!("fixtures/workbuddy-tools.json"))?;
    assert_eq!(nested.len(), 22);
    let flat: Vec<Value> = nested
        .iter()
        .map(|tool| {
            let mut function = tool["function"].clone();
            function["type"] = json!("function");
            function
        })
        .collect();
    let input = json!([
        {"role":"developer", "content":[{"type":"input_text", "text":"Use the client's tools."}]},
        {"role":"user", "content":[{"type":"input_text", "text":"A test request."}]},
        {"role":"assistant", "content":[{"type":"output_text", "text":"Ready.", "annotations":[]}]},
        {"role":"user", "content":[{"type":"input_text", "text":"Continue."}]}
    ]);
    let history = json!([
        call,
        {"type":"function_call_output", "call_id":"workbuddy-call", "output":"client executed the function"}
    ]);
    for (turn, tools) in [json!(nested), json!(flat)].into_iter().enumerate() {
        let mut turn_input = input.as_array().unwrap().clone();
        if turn == 1 {
            turn_input.extend(history.as_array().unwrap().clone());
        }
        let mut request = start(json!({
            "model":"mock-model", "stream":true, "input":turn_input,
            "reasoning":{"effort":"high"}, "tools":tools
        }));
        request["id"] = json!(10 + turn);
        request["params"]["requestId"] = json!(format!("schema-turn-{turn}"));
        let received = schema_exchange(&mut ws, request).await?;
        let deltas: String = received
            .iter()
            .filter(|event| event["type"] == "response.function_call_arguments.delta")
            .map(|event| event["delta"].as_str().unwrap())
            .collect();
        assert_eq!(deltas, arguments);
        assert!(
            received
                .iter()
                .any(|event| event["type"] == "response.output_item.done"
                    && event["item"]["call_id"] == "workbuddy-call"
                    && event["item"]["name"] == "Bash"
                    && event["item"]["arguments"] == arguments)
        );
        assert!(
            received
                .iter()
                .any(|event| event["type"] == "response.completed"
                    && event["response"]["usage"]["total_tokens"] == 13)
        );

        // Check actual HTTP bytes received by the mock, not preparation logs.
        let requests: Vec<_> = upstream
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|request| request.url.path() == "/v1/responses")
            .collect();
        assert_eq!(requests.len(), turn + 1);
        let body: Value = requests.last().unwrap().body_json()?;
        let sent = if lite {
            assert!(body.get("tools").is_none());
            let additional = body["input"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["type"] == "additional_tools")
                .unwrap();
            assert_eq!(additional["tools"][0]["name"], "functions");
            &additional["tools"][0]["tools"]
        } else {
            &body["tools"]
        };
        assert_eq!(sent, &json!(flat));
        assert!(
            sent.as_array()
                .unwrap()
                .iter()
                .all(|tool| tool["strict"] == false && tool["type"] == "function")
        );
        assert_eq!(body["reasoning"]["effort"], "high");
        assert!(body["client_metadata"].is_object());
        if turn == 1 {
            let calls: Vec<_> = body["input"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| item.get("call_id").is_some())
                .cloned()
                .collect();
            assert_eq!(json!(calls), history);
        }
        assert!(!marker.exists(), "Bash must execute only in the client");
    }
    Ok(())
}
