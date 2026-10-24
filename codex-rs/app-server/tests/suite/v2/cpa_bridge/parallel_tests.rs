//! Native model mode owns parallel-call normalization at the actual sender.
use super::*;
use base64::Engine;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

const LITE_HEADER: &str = "x-openai-internal-codex-responses-lite";
const LITE_MIRROR: &str = "ws_request_header_x_openai_internal_codex_responses_lite";
const LITE_ERROR: &str =
    "X-OpenAI-Internal-Codex-Responses-Lite requires `parallel_tool_calls` to be false.";

struct ModelSocket {
    uri: String,
    requests: Arc<Mutex<Vec<Value>>>,
    connections: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for ModelSocket {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn model_events(arguments: &str, index: usize) -> Vec<Value> {
    let call = json!({"type":"function_call","id":"fc_parallel","call_id":"parallel-call","name":"Bash","arguments":arguments,"status":"completed"});
    vec![
        json!({"type":"response.created","response":{"id":format!("resp_parallel_{index}")}}),
        json!({"type":"response.output_item.done","output_index":0,"item":call}),
        json!({"type":"response.completed","response":{"id":format!("resp_parallel_{index}"),"output":[call],"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}}),
    ]
}

async fn model_socket(arguments: String) -> Result<ModelSocket> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let uri = format!("http://{}", listener.local_addr()?);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let connections = Arc::new(AtomicUsize::new(0));
    let captured = Arc::clone(&requests);
    let accepted = Arc::clone(&connections);
    let task = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            accepted.fetch_add(1, Ordering::SeqCst);
            let mut config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default();
            config.extensions.permessage_deflate = Some(
                tokio_tungstenite::tungstenite::extensions::compression::deflate::DeflateConfig::default(),
            );
            let mut socket = tokio_tungstenite::accept_async_with_config(socket, Some(config))
                .await
                .unwrap();
            while let Some(message) = socket.next().await {
                let body: Value = match message.unwrap() {
                    Message::Text(text) => serde_json::from_str(&text).unwrap(),
                    Message::Binary(bytes) => serde_json::from_slice(&bytes).unwrap(),
                    _ => continue,
                };
                let index = {
                    let mut captured = captured.lock().unwrap();
                    let index = captured.len();
                    captured.push(body.clone());
                    index
                };
                let lite = body["client_metadata"][LITE_MIRROR] == "true";
                if lite && body["parallel_tool_calls"] != false {
                    let error = json!({"type":"error","status":400,"error":{"type":"invalid_request_error","message":LITE_ERROR,"param":"parallel_tool_calls"}});
                    socket
                        .send(Message::Text(error.to_string().into()))
                        .await
                        .unwrap();
                    continue;
                }
                for event in model_events(&arguments, index) {
                    socket
                        .send(Message::Text(event.to_string().into()))
                        .await
                        .unwrap();
                }
            }
        }
    });
    Ok(ModelSocket {
        uri,
        requests,
        connections,
        task,
    })
}

async fn parallel_exchange(ws: &mut Socket, request: Value) -> Result<Vec<Value>> {
    let id = request["id"].clone();
    let request_id = request["params"]["requestId"].clone();
    send(ws, request).await?;
    let mut bytes = Vec::new();
    timeout(Duration::from_secs(30), async {
        while let Some(message) = ws.next().await {
            let Message::Text(text) = message? else {
                continue;
            };
            let message: Value = serde_json::from_str(&text)?;
            if message.get("id") == Some(&id) {
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
                Some("cpa/inference/completed") => return Ok(()),
                _ => {}
            }
        }
        anyhow::bail!("connection closed before model completion")
    })
    .await??;
    String::from_utf8(bytes)?
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter(|line| *line != "[DONE]")
        .map(|line| serde_json::from_str(line).map_err(Into::into))
        .collect()
}

#[test_case::test_case(false, false, None; "http_standard_default")]
#[test_case::test_case(false, false, Some(false); "http_standard_false")]
#[test_case::test_case(false, false, Some(true); "http_standard_true")]
#[test_case::test_case(false, true, None; "http_lite_default")]
#[test_case::test_case(false, true, Some(false); "http_lite_false")]
#[test_case::test_case(false, true, Some(true); "http_lite_true_no_hint_two_turns")]
#[test_case::test_case(true, false, None; "ws_standard_default")]
#[test_case::test_case(true, false, Some(false); "ws_standard_false")]
#[test_case::test_case(true, false, Some(true); "ws_standard_true")]
#[test_case::test_case(true, true, None; "ws_lite_default")]
#[test_case::test_case(true, true, Some(false); "ws_lite_false")]
#[test_case::test_case(true, true, Some(true); "ws_lite_true_no_hint_two_turns")]
#[tokio::test]
async fn cpa_native_model_parallel_matrix(
    websocket: bool,
    lite: bool,
    requested: Option<bool>,
) -> Result<()> {
    let upstream = MockServer::start().await;
    let home = TempDir::new()?;
    let marker = home.path().join("client-bash-must-not-run");
    let arguments = json!({"command":format!("touch {}", marker.display())}).to_string();
    let socket = if websocket {
        Some(model_socket(arguments.clone()).await?)
    } else {
        None
    };
    let response_arguments = arguments.clone();
    Mock::given(method("POST")).and(path("/v1/responses"))
        .respond_with(move |request: &wiremock::Request| {
            let body: Value = request.body_json().unwrap();
            let actual_lite = request.headers.get(LITE_HEADER).is_some_and(|value| value == "true");
            if actual_lite && body["parallel_tool_calls"] != false {
                return ResponseTemplate::new(400).set_body_json(json!({"error":{"type":"invalid_request_error","message":LITE_ERROR,"param":"parallel_tool_calls"}}));
            }
            let events = model_events(&response_arguments, 0).into_iter()
                .map(|event| format!("data: {event}\n\n")).collect::<String>();
            ResponseTemplate::new(200).set_body_raw(events, "text/event-stream")
        }).mount(&upstream).await;
    prepare_account(&upstream, &home)?;
    let config = core_test_support::load_default_config_for_test(&home).await;
    let mut model =
        codex_core::test_support::construct_model_info_offline("parallel-model", &config);
    model.use_responses_lite = lite;
    let catalog = home.path().join("parallel-models.json");
    std::fs::write(&catalog, serde_json::to_vec(&json!({"models":[model]}))?)?;
    let cfg = home.path().join("state/shared.json/config.toml");
    let mut worker: toml::Value = toml::from_str(&std::fs::read_to_string(&cfg)?)?;
    worker.as_table_mut().unwrap().insert(
        "model_catalog_json".into(),
        toml::Value::String(catalog.display().to_string()),
    );
    let provider = &mut worker["model_providers"]["mock_provider"];
    provider.as_table_mut().unwrap().insert(
        "supports_websockets".into(),
        toml::Value::Boolean(websocket),
    );
    if let Some(socket) = &socket {
        provider["base_url"] = toml::Value::String(format!("{}/v1", socket.uri));
    }
    std::fs::write(&cfg, toml::to_string(&worker)?)?;
    let _runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let call = json!({"type":"function_call","id":"fc_parallel","call_id":"parallel-call","name":"Bash","arguments":arguments,"status":"completed"});
    let rounds = if lite && requested == Some(true) {
        2
    } else {
        1
    };
    let expected = requested.unwrap_or(true) && !lite;
    for turn in 0..rounds {
        let mut input = json!([{"role":"user","content":"parallel fixture"}]);
        if turn == 1 {
            input.as_array_mut().unwrap().extend([call.clone(), json!({"type":"function_call_output","call_id":"parallel-call","output":"client result"})]);
        }
        // Deliberately omit every caller Lite header/mirror. The worker's model
        // catalog determines the actual mode even when the caller supplies true.
        let mut body = json!({"model":"parallel-model","input":input,"session_id":"parallel-session","thread_id":"parallel-session","tools":[{"type":"function","name":"Bash","description":"client only","strict":false,"parameters":{"type":"object","properties":{"command":{"type":"string"}},"required":["command"],"additionalProperties":false}}]});
        if let Some(value) = requested {
            body["parallel_tool_calls"] = json!(value);
        }
        assert!(body.get("client_metadata").is_none());
        let mut rpc = start(body);
        rpc["id"] = json!(9000 + turn);
        rpc["params"]["requestId"] = json!(format!("parallel-{turn}"));
        let events = parallel_exchange(&mut ws, rpc).await?;
        assert!(
            events
                .iter()
                .any(|event| event["type"] == "response.completed")
        );
        assert!(
            events
                .iter()
                .any(|event| event["type"] == "response.output_item.done"
                    && event["item"]["name"] == "Bash"
                    && event["item"]["arguments"] == arguments)
        );
        let actual = if let Some(socket) = &socket {
            let captured = socket.requests.lock().unwrap();
            assert_eq!(captured.len(), turn + 1);
            let actual = captured.last().unwrap().clone();
            assert_eq!(actual["type"], "response.create");
            assert_eq!(actual["client_metadata"][LITE_MIRROR] == "true", lite);
            actual
        } else {
            let requests: Vec<_> = upstream
                .received_requests()
                .await
                .unwrap()
                .into_iter()
                .filter(|request| request.url.path() == "/v1/responses")
                .collect();
            assert_eq!(requests.len(), turn + 1);
            let actual = requests.last().unwrap();
            assert_eq!(
                actual
                    .headers
                    .get(LITE_HEADER)
                    .is_some_and(|value| value == "true"),
                lite
            );
            actual.body_json::<Value>()?
        };
        assert_eq!(
            actual["parallel_tool_calls"], expected,
            "actual transport={websocket}, mode={lite}, input={requested:?}, turn={turn}"
        );
        if turn == 0 {
            assert_eq!(actual.get("tools").is_none(), lite);
            if lite {
                assert!(
                    actual["input"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|item| item["type"] == "additional_tools")
                );
            }
        }
        if turn == 1 {
            assert!(
                actual["input"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|item| item["type"] == "function_call_output"
                        && item["call_id"] == "parallel-call")
            );
            if websocket {
                assert_eq!(actual["previous_response_id"], "resp_parallel_0");
            }
        }
        assert!(!marker.exists(), "client Bash must not run in the worker");
        if let Some(directory) = std::env::var_os("CODEX_PARALLEL_FIXTURE_DIR") {
            let directory = std::path::PathBuf::from(directory);
            std::fs::create_dir_all(&directory)?;
            let transport = if websocket { "ws" } else { "http" };
            let mode = if lite { "lite" } else { "responses" };
            let control = requested.map_or("default", |value| if value { "true" } else { "false" });
            let record = json!({
                "transport":transport, "native_model_lite":lite,
                "caller_parallel_tool_calls":requested, "caller_has_lite_hint":false,
                "verified_upstream_lite_marker":lite, "turn":turn,
                "expected_parallel_tool_calls":expected, "actual_native_request":actual,
                "received_events":events, "local_tool_executed":marker.exists()
            });
            std::fs::write(
                directory.join(format!("{transport}-{mode}-{control}-{turn}.json")),
                serde_json::to_vec_pretty(&record)?,
            )?;
        }
    }
    if let Some(socket) = socket {
        assert_eq!(
            socket.connections.load(Ordering::SeqCst),
            1,
            "Lite continuation must reuse the same native connection"
        );
        assert_eq!(socket.requests.lock().unwrap().len(), rounds);
        assert!(
            upstream
                .received_requests()
                .await
                .unwrap()
                .iter()
                .all(|request| request.url.path() != "/v1/responses"),
            "no HTTP fallback"
        );
    }
    Ok(())
}
