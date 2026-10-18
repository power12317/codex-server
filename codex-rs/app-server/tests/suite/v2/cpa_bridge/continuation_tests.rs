use super::*;
use core_test_support::responses;
use pretty_assertions::assert_eq;

fn user(text: &str) -> Value {
    json!({"type":"message","role":"user","content":[{"type":"input_text","text":text}]})
}

fn assistant(index: usize) -> Value {
    json!({"type":"message","id":format!("msg_{index}"),"role":"assistant","status":"completed",
        "content":[{"type":"output_text","text":format!("answer {index}"),"annotations":[]}]})
}

fn events(index: usize, item: Value, ticket: &str) -> Vec<Value> {
    let id = format!("resp_{index}");
    vec![
        json!({"type":"codex.response.metadata","headers":{"x-codex-turn-state":ticket}}),
        responses::ev_response_created(&id),
        json!({"type":"response.output_item.done","item":item}),
        responses::ev_completed(&id),
    ]
}

async fn configure(
    upstream: &MockServer,
    home: &TempDir,
    websocket: &responses::WebSocketTestServer,
) -> Result<()> {
    prepare_account(upstream, home)?;
    let path = home.path().join("state/shared.json/config.toml");
    let mut config: toml::Value = toml::from_str(&std::fs::read_to_string(&path)?)?;
    let provider = &mut config["model_providers"]["mock_provider"];
    provider["base_url"] = toml::Value::String(format!(
        "{}/v1",
        websocket.uri().replace("ws://", "http://")
    ));
    provider["supports_websockets"] = toml::Value::Boolean(true);
    std::fs::write(path, toml::to_string(&config)?)?;
    Ok(())
}

fn input(history: &[Value], turn: &str) -> Value {
    json!({"model":"gpt-5.2","instructions":"","input":history,"stream":true,
        "session_id":"root-session","thread_id":"root-session","turn_id":turn})
}

fn check_ok(result: &Exchange) {
    assert_eq!(
        result.reply["result"]["statusCode"], 200,
        "{}",
        result.reply
    );
    assert!(result.error.is_none(), "{:?}", result.error);
}

#[tokio::test]
async fn cpa_native_continuation_reuses_socket_across_turns_and_resets_only_the_ticket()
-> Result<()> {
    let upstream = MockServer::start().await;
    let websocket = responses::start_websocket_server(vec![vec![
        events(0, assistant(0), "first-turn-ticket"),
        events(1, assistant(1), "ignored-first-turn-ticket"),
        events(2, assistant(2), "second-turn-ticket"),
        events(3, assistant(3), "ignored-second-turn-ticket"),
    ]])
    .await;
    let home = TempDir::new()?;
    configure(&upstream, &home, &websocket).await?;
    let _runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let mut history = Vec::new();
    for (index, turn) in ["turn-a", "turn-a", "turn-b", "turn-b"]
        .into_iter()
        .enumerate()
    {
        history.push(user(&format!("question {index}")));
        let mut request = input(&history, turn);
        if index == 3 {
            request["window_id"] = json!("caller-window-2");
            request["context_window_id"] = json!("caller-context-2");
        }
        let result = exchange(&mut ws, rpc(request, 1600 + index as u64)).await?;
        check_ok(&result);
        history.push(assistant(index));
    }
    assert_eq!(
        websocket.handshakes().len(),
        1,
        "the original native cache survives turn boundaries"
    );
    let requests = websocket.single_connection();
    assert_eq!(requests.len(), 4);
    for (index, request) in requests.iter().enumerate() {
        let body = request.body_json();
        assert_eq!(body["input"], json!([user(&format!("question {index}"))]));
        if index > 0 {
            assert_eq!(body["previous_response_id"], format!("resp_{}", index - 1));
        } else {
            assert!(body.get("previous_response_id").is_none());
        }
        let metadata = &body["client_metadata"];
        assert_eq!(
            metadata["x-codex-window-id"],
            format!(
                "{}:{}",
                metadata["thread_id"].as_str().expect("thread"),
                usize::from(index == 3)
            )
        );
        match index {
            1 => assert_eq!(metadata["x-codex-turn-state"], "first-turn-ticket"),
            3 => assert_eq!(metadata["x-codex-turn-state"], "second-turn-ticket"),
            _ => assert!(
                metadata.get("x-codex-turn-state").is_none(),
                "new turn gets no previous turn ticket"
            ),
        }
    }
    Ok(())
}

#[tokio::test]
async fn cpa_native_continuation_preserves_tools_compaction_and_native_full_request_decisions()
-> Result<()> {
    let call = json!({"type":"function_call","id":"fc_read","call_id":"read","namespace":"functions","name":"read","status":"completed","arguments":"{}"});
    let custom = json!({"type":"custom_tool_call","id":"ctc_run","call_id":"run","namespace":"functions","name":"run","status":"completed","input":"echo fixture"});
    let compact =
        json!({"type":"compaction","id":"cmp_new","encrypted_content":"opaque-compacted-history"});
    let upstream = MockServer::start().await;
    let websocket = responses::start_websocket_server(vec![
        (0..8)
            .map(|index| {
                events(
                    index,
                    match index {
                        0 => call.clone(),
                        1 => custom.clone(),
                        2 => compact.clone(),
                        _ => assistant(index),
                    },
                    "turn-ticket",
                )
            })
            .collect(),
    ])
    .await;
    let home = TempDir::new()?;
    configure(&upstream, &home, &websocket).await?;
    let _runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let tools = json!([{"type":"namespace","name":"functions","tools":[
        {"type":"function","name":"read","parameters":{"type":"object"}},
        {"type":"custom","name":"run"}]}]);
    let mut history = vec![user("start")];
    let mut sent = Vec::new();
    for index in 0..8 {
        match index {
            1 => history.extend([call.clone(), json!({"type":"function_call_output","call_id":"read","namespace":"functions","name":"read","output":"read result"})]),
            2 => history.extend([custom.clone(), json!({"type":"custom_tool_call_output","call_id":"run","output":"fixture"}), json!({"type":"compaction_trigger"})]),
            3 => history = vec![compact.clone(), user("after compaction")],
            4.. => history.extend([assistant(index - 1), user(&format!("next {index}"))]),
            _ => {}
        }
        let mut request = input(&history, if index < 2 { "tools-a" } else { "tools-b" });
        request["tools"] = tools.clone();
        if index >= 5 {
            request["instructions"] = json!("changed instructions");
        }
        if index >= 6 {
            request["tools"] = json!([]);
        }
        if index >= 7 {
            request["model"] = json!("gpt-5.1");
        }
        sent.push(history.len());
        check_ok(&exchange(&mut ws, rpc(request, 1700 + index as u64)).await?);
    }
    assert_eq!(websocket.handshakes().len(), 1);
    let requests = websocket.single_connection();
    assert_eq!(requests.len(), 8);
    for (index, request) in requests.iter().enumerate() {
        let body = request.body_json();
        let delta = matches!(index, 1 | 2 | 4);
        if delta {
            assert_eq!(body["previous_response_id"], format!("resp_{}", index - 1));
        } else {
            assert!(
                body.get("previous_response_id").is_none(),
                "native full request at {index}: {body}"
            );
        }
        assert_eq!(
            body["input"].as_array().expect("input").len(),
            if delta {
                if index == 2 { 2 } else { 1 }
            } else {
                sent[index] + usize::from(index >= 5)
            }
        );
        let metadata = &body["client_metadata"];
        assert_eq!(
            metadata["x-codex-window-id"],
            format!(
                "{}:{}",
                metadata["thread_id"].as_str().expect("thread"),
                usize::from(index >= 3)
            )
        );
    }
    assert_eq!(
        requests[1].body_json()["input"][0]["namespace"],
        "functions"
    );
    assert_eq!(
        requests[2].body_json()["input"][1]["type"],
        "compaction_trigger"
    );
    assert_eq!(requests[3].body_json()["input"][0], compact);
    assert_eq!(
        requests[3].body_json()["client_metadata"]["x-codex-turn-state"],
        "turn-ticket",
        "same turn retains its ticket through compaction"
    );
    Ok(())
}

#[tokio::test]
async fn cpa_native_continuation_serializes_concurrent_turns_without_opening_duplicate_sockets()
-> Result<()> {
    let upstream = MockServer::start().await;
    let websocket = responses::start_websocket_server(vec![vec![
        events(0, assistant(0), "ticket-a"),
        events(1, assistant(1), "ticket-b"),
    ]])
    .await;
    let home = TempDir::new()?;
    configure(&upstream, &home, &websocket).await?;
    let _runtime = launch(&upstream, &home).await?;
    let mut left = connect(&home).await?;
    let mut right = connect(&home).await?;
    let (first, second) = tokio::try_join!(
        exchange(&mut left, rpc(input(&[user("left")], "concurrent-a"), 1801)),
        exchange(
            &mut right,
            rpc(input(&[user("right")], "concurrent-b"), 1802)
        ),
    )?;
    check_ok(&first);
    check_ok(&second);
    assert_eq!(websocket.handshakes().len(), 1);
    let requests = websocket.single_connection();
    assert_eq!(requests.len(), 2);
    for request in requests {
        let body = request.body_json();
        assert!(
            body.get("previous_response_id").is_none(),
            "unrelated histories must remain full"
        );
        assert!(
            body["client_metadata"].get("x-codex-turn-state").is_none(),
            "tickets stay per turn"
        );
    }
    Ok(())
}
