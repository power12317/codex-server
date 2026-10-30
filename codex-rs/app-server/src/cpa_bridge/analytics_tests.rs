use super::*;
use codex_analytics::AppServerRpcTransport;
use codex_app_server_protocol::ClientInfo;
use codex_app_server_protocol::InitializeParams;
use codex_core::config::ConfigBuilder;
use codex_login::CodexAuth;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

async fn harness() -> (Arc<InferenceAnalytics>, MockServer, TempDir) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/codex/analytics-events/events"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let home = TempDir::new().unwrap();
    let config = ConfigBuilder::default()
        .codex_home(home.path().to_path_buf())
        .build()
        .await
        .unwrap();
    let auth =
        AuthManager::from_auth_for_testing(CodexAuth::create_dummy_chatgpt_auth_for_testing());
    let client = AnalyticsEventsClient::new(auth.clone(), server.uri(), Some(true));
    client.track_initialize(
        37,
        InitializeParams {
            client_info: ClientInfo {
                name: "codex-tui".into(),
                title: None,
                version: "0.162.1".into(),
            },
            capabilities: None,
        },
        "codex-tui".into(),
        AppServerRpcTransport::Websocket,
    );
    client.flush().await;
    let mut owner = InferenceAnalytics::new(client, auth, Arc::new(config));
    Arc::get_mut(&mut owner).unwrap().delay = Some(Duration::from_secs(30));
    (owner, server, home)
}

fn request(input: Value) -> Value {
    json!({"model":"final-upstream-model","input":input,"reasoning":{"effort":"high","summary":"auto"},
        "client_metadata":{"session_id":"upstream-session","thread_id":"upstream-thread","turn_id":"upstream-turn", "root_turn_id":"upstream-root"},
        "tools":[]})
}

fn send(exchange: &Exchange, body: &Value) {
    let mut headers = HeaderMap::new();
    headers.insert("originator", "codex-tui".parse().unwrap());
    exchange.request(
        "http",
        "POST",
        "https://chatgpt.com/backend-api/codex/responses",
        &headers,
        &serde_json::to_vec(body).unwrap(),
    );
}

fn complete(exchange: &Exchange, id: Option<&str>, output: Value) {
    exchange.event(&json!({"type":"response.completed","response":{"id":id,"output":output,
        "usage":{"input_tokens":10,"output_tokens":3,"total_tokens":13,"input_tokens_details":{"cached_tokens":4}}}}).to_string());
}

async fn events(server: &MockServer) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.url.path() == "/codex/analytics-events/events")
        .flat_map(|request| {
            request.body_json::<Value>().unwrap()["events"]
                .as_array()
                .unwrap()
                .clone()
        })
        .collect()
}

async fn drain(owner: &InferenceAnalytics, server: &MockServer) -> Vec<Value> {
    drain_turns(owner, server, 1).await
}

async fn drain_turns(owner: &InferenceAnalytics, server: &MockServer, count: usize) -> Vec<Value> {
    tokio::time::resume();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let result = events(server).await;
            if result
                .iter()
                .filter(|event| event["event_type"] == "codex_turn_event")
                .count()
                == count
            {
                return result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "turn did not reach original sender: {} threads",
            owner.state.lock().unwrap().len()
        )
    })
}

#[tokio::test]
async fn cpa_analytics_waits_then_uses_existing_sender_and_real_connection() {
    let (owner, server, _home) = harness().await;
    tokio::time::pause();
    let exchange = owner.exchange(37);
    send(&exchange, &request(json!([])));
    tokio::time::advance(Duration::from_secs(2)).await;
    complete(&exchange, Some("upstream-response"), json!([]));
    exchange.finish(&Ok(()));
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(29)).await;
    assert!(
        owner
            .state
            .lock()
            .unwrap()
            .values()
            .all(|thread| thread.completed.is_empty())
    );
    assert!(events(&server).await.is_empty());
    tokio::time::advance(Duration::from_secs(1)).await;
    let recorded = drain(&owner, &server).await;
    let turn = &recorded
        .iter()
        .find(|event| event["event_type"] == "codex_turn_event")
        .unwrap()["event_params"];
    assert_eq!(turn["thread_id"], "upstream-thread");
    assert_eq!(turn["session_id"], "upstream-session");
    assert_eq!(turn["root_turn_id"], "upstream-root");
    assert_eq!(turn["model"], "final-upstream-model");
    assert_eq!(turn["duration_ms"], 32_000);
    assert_eq!(turn["after_last_sampling_ms"], 30_000);
    assert_eq!(turn["sampling_ms"], 2_000);
    assert_eq!(turn["input_tokens"], 10);
    assert_eq!(turn["app_server_client"]["rpc_transport"], "websocket");
    assert_eq!(turn["app_server_client"]["client_name"], "codex-tui");
    assert!(turn["active_plugin_ids_at_turn_start"].is_null());
    assert_eq!(
        recorded
            .iter()
            .filter(|event| event["event_type"] == "codex_thread_initialized")
            .count(),
        1
    );
    assert_eq!(
        recorded
            .iter()
            .filter(|event| event["event_type"] == "codex_turn_event")
            .count(),
        1
    );
    assert!(
        recorded
            .iter()
            .all(|event| event["event_type"].as_str().unwrap().starts_with("codex_"))
    );
}

#[tokio::test]
async fn cpa_analytics_continuations_deduplicate_usage_and_preserve_failures() {
    let (owner, server, _home) = harness().await;
    tokio::time::pause();
    let first = owner.exchange(37);
    send(&first, &request(json!([])));
    let call = json!({"type":"function_call","call_id":"upstream-call","id":"fc1","name":"exec_command","arguments":"{\"cmd\":\"false\"}"});
    complete(&first, Some("r1"), json!([call]));
    complete(&first, Some("r1"), json!([call]));
    first.finish(&Ok(()));
    tokio::time::advance(Duration::from_secs(20)).await;
    let second = owner.exchange(37);
    send(
        &second,
        &request(
            json!([{"type":"function_call_output","call_id":"upstream-call","output":"{\"exit_code\":7,\"duration_ms\":1500}"}]),
        ),
    );
    complete(&second, Some("r2"), json!([]));
    second.finish(&Ok(()));
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(30)).await;
    let recorded = drain(&owner, &server).await;
    let turn = &recorded
        .iter()
        .find(|event| event["event_type"] == "codex_turn_event")
        .unwrap()["event_params"];
    assert_eq!(turn["input_tokens"], 20);
    assert_eq!(turn["sampling_request_count"], 2);
    assert_eq!(turn["sampling_retry_count"], 0);
    assert_eq!(turn["shell_command_count"], 1);
    let command = &recorded
        .iter()
        .find(|event| event["event_type"] == "codex_command_execution_event")
        .unwrap()["event_params"];
    assert_eq!(command["exit_code"], 7);
    assert_eq!(command["model_slug"], "final-upstream-model");
    assert_eq!(command["terminal_status"], "failed");
    assert_eq!(command["final_approval_outcome"], "config_allowed");
    assert_eq!(command["user_review_count"], 0);
    assert_eq!(command["execution_duration_ms"], 1500);
    assert_eq!(command["originating_response_id"], "r1");
}

#[tokio::test]
async fn cpa_analytics_retry_and_idless_completion_are_counted_once() {
    let (owner, server, _home) = harness().await;
    tokio::time::pause();
    let exchange = owner.exchange(37);
    send(&exchange, &request(json!([])));
    tokio::time::advance(Duration::from_secs(1)).await;
    send(&exchange, &request(json!([])));
    complete(&exchange, None, json!([]));
    complete(&exchange, None, json!([]));
    exchange.finish(&Ok(()));
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(30)).await;
    let recorded = drain(&owner, &server).await;
    let turn = &recorded
        .iter()
        .find(|event| event["event_type"] == "codex_turn_event")
        .unwrap()["event_params"];
    assert_eq!(turn["total_tokens"], 13);
    assert_eq!(turn["sampling_request_count"], 2);
    assert_eq!(turn["sampling_retry_count"], 1);
    assert_eq!(turn["sampling_ms"], 1000);
}

#[test]
fn cpa_analytics_wire_snapshot_supports_compression_ws_and_lite() {
    let body = request(
        json!([{"role":"user","content":[{"type":"input_image","image_url":"data:image/png;base64,fixture"}]}]),
    );
    let bytes = serde_json::to_vec(&body).unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("content-encoding", "zstd".parse().unwrap());
    let compressed = zstd::stream::encode_all(bytes.as_slice(), 1).unwrap();
    let snapshot = Snapshot::read(
        "http",
        "https://example.test/responses/lite",
        &headers,
        &compressed,
    )
    .unwrap();
    assert_eq!(snapshot.model, "final-upstream-model");
    assert_eq!(snapshot.images, 1);
    let ws = json!({"type":"response.create","response":body});
    let snapshot = Snapshot::read(
        "websocket",
        "wss://example.test/responses",
        &HeaderMap::new(),
        &serde_json::to_vec(&ws).unwrap(),
    )
    .unwrap();
    assert_eq!(snapshot.turn, "upstream-turn");
    assert!(
        Snapshot::read(
            "http",
            "https://example.test/responses",
            &HeaderMap::new(),
            br#"{"model":"raw-cpa-model","thread_id":"raw-cpa-id"}"#
        )
        .is_none()
    );
}

#[test]
fn cpa_analytics_reads_native_exit_headers_only_before_stdout() {
    let body = request(json!([
        {"type":"function_call_output","call_id":"shell","output":"Exit code: 4\nWall time: 1.5 seconds\nOutput:\nExit code: 0"},
        {"type":"function_call_output","call_id":"exec","output":[{"type":"input_text","text":"Chunk ID: fixture\nWall time: 2.0000 seconds\nProcess exited with code 7\nOutput:\nprivate output"}]}
    ]));
    let snapshot = Snapshot::read(
        "http",
        "https://example.test/responses",
        &HeaderMap::new(),
        &serde_json::to_vec(&body).unwrap(),
    )
    .unwrap();
    assert_eq!(snapshot.results[0]["exit_code"], 4);
    assert_eq!(snapshot.results[0]["duration_ms"], 1500);
    assert_eq!(snapshot.results[1]["exit_code"], 7);
    assert!(
        !serde_json::to_string(&snapshot.results)
            .unwrap()
            .contains("private output")
    );
}

#[test]
fn cpa_analytics_exit_code_zero_is_the_missing_success_default() {
    let call = json!({"type":"function_call","call_id":"call","name":"shell_command","arguments":"{\"command\":\"true\"}"});
    let mut tool = tools::Tool::read(&call, 1000).unwrap();
    let projected =
        serde_json::to_value(tool.project(31_000, "/", "upstream-thread").unwrap()).unwrap();
    assert_eq!(projected["exitCode"], 0);
    assert_eq!(projected["durationMs"], 30_000);
    tool.result(&json!({"exit_code":12}), 2000);
    tool.result(&json!({}), 3000);
    let projected =
        serde_json::to_value(tool.project(31_000, "/", "upstream-thread").unwrap()).unwrap();
    assert_eq!(projected["exitCode"], 12);
    assert_eq!(projected["status"], "failed");
    let mut unknown_code = tools::Tool::read(&call, 1000).unwrap();
    unknown_code.result(&json!({"success":false}), 2000);
    let projected = serde_json::to_value(
        unknown_code
            .project(31_000, "/", "upstream-thread")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(projected["exitCode"], 0);
    assert_eq!(projected["status"], "failed");
}

#[test]
fn cpa_analytics_tools_use_native_notification_schemas() {
    for item in [
        json!({"type":"custom_tool_call","call_id":"patch","name":"apply_patch","input":"*** Begin Patch\n*** Add File: fixture.txt\n+text\n*** End Patch"}),
        json!({"type":"function_call","call_id":"collab","name":"spawn_agent","arguments":"{}"}),
        json!({"type":"web_search_call","id":"web","status":"completed","action":{"type":"search","query":"fixture"}}),
        json!({"type":"image_generation_call","id":"image","status":"completed"}),
        json!({"type":"mcp_call","id":"mcp","name":"list","server_label":"example","status":"completed"}),
        json!({"type":"function_call","call_id":"dynamic","name":"fixture_tool","arguments":"{}"}),
        json!({"type":"function_call","call_id":"mcp-function","name":"mcp__example__list","arguments":"{}"}),
    ] {
        let tool = tools::Tool::read(&item, 1000).unwrap();
        if item["type"] == "mcp_call" {
            assert!(matches!(
                tool.project(31_000, "/", "upstream-thread")
                    .unwrap()
                    .payload,
                codex_app_server_protocol::ThreadItemPayload::McpToolCall { .. }
            ));
        }
        assert!(
            tool.project(31_000, "/", "upstream-thread").is_ok(),
            "native tool schema: {item}"
        );
    }
}

#[tokio::test]
async fn cpa_analytics_explicit_upstream_error_is_not_success() {
    let (owner, server, _home) = harness().await;
    tokio::time::pause();
    let exchange = owner.exchange(37);
    send(&exchange, &request(json!([])));
    let error = codex_protocol::error::CodexErr::ContextWindowExceeded;
    exchange.native_error(&error);
    exchange.finish(&Err(super::super::native_failure(&error)));
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(30)).await;
    let recorded = drain(&owner, &server).await;
    let turn = &recorded
        .iter()
        .find(|event| event["event_type"] == "codex_turn_event")
        .unwrap()["event_params"];
    assert_eq!(turn["status"], "failed");
    assert_eq!(turn["turn_error"], "contextWindowExceeded");
    assert!(turn["codex_error_kind"].is_string());
}

#[tokio::test]
async fn cpa_analytics_completed_history_does_not_block_new_turns() {
    let (owner, _server, _home) = harness().await;
    let first = owner.exchange(37);
    send(&first, &request(json!([])));
    {
        let mut state = owner.state.lock().unwrap();
        let thread = state.values_mut().next().unwrap();
        for index in 0..MAX_TURNS {
            thread
                .completed
                .insert(format!("completed-{index}"), Instant::now());
        }
    }
    let second = owner.exchange(37);
    let mut body = request(json!([]));
    body["client_metadata"]["turn_id"] = json!("new-turn");
    send(&second, &body);
    let state = owner.state.lock().unwrap();
    let thread = state.values().next().unwrap();
    assert!(thread.turns.contains_key("upstream-turn"));
    assert!(thread.turns.contains_key("new-turn"));
    assert_eq!(thread.turns.len() + thread.completed.len(), MAX_TURNS);
}

#[tokio::test]
async fn cpa_analytics_same_turn_id_in_different_threads_stays_isolated() {
    let (owner, server, _home) = harness().await;
    tokio::time::pause();
    for thread in ["first-thread", "second-thread"] {
        let exchange = owner.exchange(37);
        let mut body = request(json!([]));
        body["client_metadata"]["thread_id"] = json!(thread);
        body["model"] = json!(thread);
        send(&exchange, &body);
        complete(&exchange, Some(thread), json!([]));
        exchange.finish(&Ok(()));
    }
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(30)).await;
    let recorded = drain_turns(&owner, &server, 2).await;
    for thread in ["first-thread", "second-thread"] {
        let turn = recorded
            .iter()
            .find(|event| {
                event["event_type"] == "codex_turn_event"
                    && event["event_params"]["thread_id"] == thread
            })
            .unwrap();
        assert_eq!(turn["event_params"]["model"], thread);
        assert_eq!(turn["event_params"]["total_tokens"], 13);
    }
}
