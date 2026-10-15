use super::*;
use base64::Engine;
use pretty_assertions::assert_eq;

struct Reply {
    native: Value,
    request_headers: Value,
    original_response: Value,
    accepted: Value,
    events: Vec<Value>,
}

async fn exchange(ws: &mut Socket, request: Value) -> Result<Reply> {
    send(ws, request).await?;
    let request = receive(ws, "method", json!("cpa/inference/upstream")).await?["params"].clone();
    let original_response =
        receive(ws, "method", json!("cpa/inference/upstream")).await?["params"].clone();
    let accepted = receive(ws, "id", json!(2)).await?;
    anyhow::ensure!(accepted.get("error").is_none(), "{accepted}");
    let mut bytes = Vec::new();
    timeout(Duration::from_secs(30), async {
        while let Some(message) = ws.next().await {
            let Message::Text(text) = message? else {
                continue;
            };
            let message: Value = serde_json::from_str(&text)?;
            match message["method"].as_str() {
                Some("cpa/inference/body") => bytes.extend(
                    base64::engine::general_purpose::STANDARD.decode(
                        message["params"]["bodyBase64"]
                            .as_str()
                            .expect("fixture response must contain the expected native fields"),
                    )?,
                ),
                Some("cpa/inference/completed") => return Ok(()),
                Some("cpa/inference/error") => anyhow::bail!("{message}"),
                _ => {}
            }
        }
        anyhow::bail!("closed before completion")
    })
    .await??;
    let events = std::str::from_utf8(&bytes)?
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter(|data| *data != "[DONE]")
        .map(serde_json::from_str)
        .collect::<std::result::Result<Vec<Value>, _>>()?;
    Ok(Reply {
        native: serde_json::from_str(
            request["body"]
                .as_str()
                .expect("fixture response must contain the expected native fields"),
        )?,
        request_headers: request["headers"].clone(),
        original_response,
        accepted: accepted["result"].clone(),
        events,
    })
}

fn request(source: Value, state: &str) -> Value {
    start(
        json!({"model":"mock-model", "stream":true, "input":"fixture", "test_state":state, "client_metadata":source}),
    )
}

fn echo_response(request: &wiremock::Request) -> ResponseTemplate {
    let body: Value = request
        .body_json()
        .expect("fixture response must contain the expected native fields");
    let session = body["client_metadata"]["session_id"]
        .as_str()
        .expect("fixture response must contain the expected native fields");
    let thread = body["client_metadata"]["thread_id"]
        .as_str()
        .expect("fixture response must contain the expected native fields");
    let events = [
        json!({"type":"response.metadata", "headers":{"session-id":session,"thread-id":thread,"x-codex-turn-state":body["test_state"]}, "client_metadata":body["client_metadata"]}),
        json!({"type":"response.completed", "response":{"id":session, "session_id":session,"thread_id":thread,"prompt_cache_key":body["prompt_cache_key"],
            "client_metadata":body["client_metadata"], "output":[{"type":"function_call", "name":"fixture", "call_id":thread, "arguments":json!({"session_id":session}).to_string()}],
            "metadata":{"session_id":session}}}),
    ];
    ResponseTemplate::new(200)
        .insert_header("session-id", session.to_owned())
        .insert_header("thread-id", thread.to_owned())
        .insert_header("x-request-id", session.to_owned())
        .set_body_raw(
            events
                .iter()
                .map(|event| format!("data: {event}\n\n"))
                .collect::<String>(),
            "text/event-stream",
        )
}

#[tokio::test]
async fn cpa_hierarchy_title_isolation_and_response_identity_roundtrip() -> Result<()> {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(echo_response)
        .expect(8)
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    let _runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let root_source = json!({"session_id":"source-root", "thread_id":"source-root", "turn_id":"turn-root", "prompt_cache_key":"source-cache"});
    let root = exchange(&mut ws, request(root_source.clone(), "root-state")).await?;
    let root_id = root.native["client_metadata"]["session_id"].clone();
    assert_eq!(root_id, root.native["client_metadata"]["thread_id"]);
    assert_eq!(root.native["prompt_cache_key"], root_id);
    assert_eq!(
        root.accepted["headers"]["session-id"],
        json!(["source-root"])
    );
    assert_eq!(root.accepted["headers"]["x-request-id"], json!([root_id]));
    assert_eq!(
        root.original_response["headers"]["session-id"],
        json!([root_id])
    );
    assert_eq!(
        root.events[1]["response"]["session_id"],
        json!("source-root")
    );
    assert_eq!(
        root.events[1]["response"]["prompt_cache_key"],
        json!("source-cache")
    );
    assert_eq!(root.events[1]["response"]["id"], root_id);
    assert_eq!(
        root.events[1]["response"]["metadata"]["session_id"],
        root_id
    );
    assert_eq!(root.events[1]["response"]["output"][0]["call_id"], root_id);

    let child_source = json!({"session_id":"source-root", "thread_id":"child-a", "parent_thread_id":"source-root", "turn_id":"turn-root", "root_turn_id":"turn-root", "thread_source":"subagent", "subagent_kind":"thread_spawn", "agent_name":"root/child-a"});
    let child = exchange(&mut ws, request(child_source.clone(), "child-state")).await?;
    let child_id = child.native["client_metadata"]["thread_id"].clone();
    assert_eq!(child.native["client_metadata"]["session_id"], root_id);
    assert_ne!(child_id, root_id);
    assert_eq!(child.native["prompt_cache_key"], root_id);
    assert_eq!(child.request_headers["session-id"], json!([root_id]));
    assert_eq!(child.request_headers["thread-id"], json!([child_id]));
    assert_eq!(
        child.request_headers["x-codex-parent-thread-id"],
        json!([root_id])
    );
    assert_eq!(
        child.request_headers["x-openai-subagent"],
        json!(["collab_spawn"])
    );
    assert!(child.request_headers.get("x-codex-turn-state").is_none());
    assert_eq!(child.accepted["headers"]["thread-id"], json!(["child-a"]));
    let returned: Value = serde_json::from_str(
        child.events[0]["client_metadata"]["x-codex-turn-metadata"]
            .as_str()
            .expect("fixture response must contain the expected native fields"),
    )?;
    assert_eq!(returned["parent_thread_id"], json!("source-root"));
    assert_eq!(returned["session_id"], json!("source-root"));
    assert_eq!(returned["thread_id"], json!("child-a"));
    assert_eq!(returned["thread_source"], json!("subagent"));
    assert_eq!(
        child.events[1]["response"]["output"][0]["call_id"],
        child_id
    );

    let mut grandchild_source = child_source.clone();
    grandchild_source["thread_id"] = json!("child-b");
    grandchild_source["parent_thread_id"] = json!("child-a");
    let grandchild = exchange(&mut ws, request(grandchild_source, "grandchild-state")).await?;
    assert_eq!(grandchild.native["client_metadata"]["session_id"], root_id);
    assert_eq!(
        grandchild.request_headers["x-codex-parent-thread-id"],
        json!([child_id])
    );
    assert!(
        grandchild
            .request_headers
            .get("x-codex-turn-state")
            .is_none()
    );
    let child_again = exchange(&mut ws, request(child_source, "ignored")).await?;
    assert_eq!(
        child_again.request_headers["x-codex-turn-state"],
        json!(["child-state"])
    );
    let root_again = exchange(&mut ws, request(root_source.clone(), "ignored")).await?;
    assert_eq!(
        root_again.request_headers["x-codex-turn-state"],
        json!(["root-state"])
    );

    let title_source = json!({"x-codex-turn-metadata":json!({"session_id":"source-root", "thread_id":"source-root", "thread_source":"thread_title"}).to_string()});
    let first_title = exchange(&mut ws, request(title_source.clone(), "title-one")).await?;
    let second_title = exchange(&mut ws, request(title_source, "title-two")).await?;
    for title in [&first_title, &second_title] {
        assert_eq!(title.native["client_metadata"]["session_id"], root_id);
        assert!(title.request_headers.get("x-codex-turn-state").is_none());
        assert!(title.events[0]["client_metadata"].get("turn_id").is_none());
        let metadata: Value = serde_json::from_str(
            title.events[0]["client_metadata"]["x-codex-turn-metadata"]
                .as_str()
                .expect("fixture response must contain the expected native fields"),
        )?;
        assert!(metadata.get("turn_id").is_none());
        assert_eq!(metadata["thread_source"], json!("thread_title"));
    }
    assert_ne!(
        first_title.native["client_metadata"]["turn_id"],
        second_title.native["client_metadata"]["turn_id"]
    );
    let mut last_root = root_source;
    last_root["prompt_cache_key"] = json!("second-source-cache");
    let last = exchange(&mut ws, request(last_root, "ignored")).await?;
    assert_eq!(
        last.request_headers["x-codex-turn-state"],
        json!(["root-state"])
    );
    assert_eq!(
        last.events[1]["response"]["prompt_cache_key"],
        json!("second-source-cache")
    );
    let requests = upstream
        .received_requests()
        .await
        .expect("fixture response must contain the expected native fields");
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.url.path() == "/v1/responses")
            .count(),
        8
    );
    let mut directories = vec![home.path().join("state/shared.json/sessions")];
    while let Some(directory) = directories.pop() {
        if !directory.exists() {
            continue;
        }
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                directories.push(entry.path());
            } else {
                assert_ne!(
                    entry.path().extension().and_then(|value| value.to_str()),
                    Some("jsonl")
                );
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn cpa_http_error_business_projection_keeps_upstream_diagnostics_original() -> Result<()> {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(|request: &wiremock::Request| {
            let body: Value = request
                .body_json()
                .expect("fixture response must contain the expected native fields");
            let session = body["client_metadata"]["session_id"]
                .as_str()
                .expect("fixture response must contain the expected native fields");
            ResponseTemplate::new(400)
                .insert_header("session-id", session.to_owned())
                .insert_header("etag", "original")
                .set_body_json(json!({"error":{"session_id":session,"message":"fixture error"}}))
        })
        .expect(1)
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    let _runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    send(&mut ws, request(json!({"session_id":"source"}), "unused")).await?;
    let native = receive(&mut ws, "method", json!("cpa/inference/upstream")).await?;
    let native_body: Value = serde_json::from_str(
        native["params"]["body"]
            .as_str()
            .expect("fixture response must contain the expected native fields"),
    )?;
    let diagnostic = receive(&mut ws, "method", json!("cpa/inference/upstream")).await?;
    let original: Value = serde_json::from_str(
        diagnostic["params"]["body"]
            .as_str()
            .expect("fixture response must contain the expected native fields"),
    )?;
    assert_eq!(
        original["error"]["session_id"],
        native_body["client_metadata"]["session_id"]
    );
    let error = receive(&mut ws, "id", json!(2)).await?;
    let restored: Value = serde_json::from_str(
        error["error"]["data"]["body"]
            .as_str()
            .expect("fixture response must contain the expected native fields"),
    )?;
    assert_eq!(
        restored,
        json!({"error":{"session_id":"source","message":"fixture error"}})
    );
    assert_eq!(
        error["error"]["data"]["headers"]["session-id"],
        json!(["source"])
    );
    assert!(error["error"]["data"]["headers"].get("etag").is_none());
    Ok(())
}
