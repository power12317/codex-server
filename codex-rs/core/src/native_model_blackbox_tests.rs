//! Controlled native-model traces. The same fixtures also run against an unmodified
//! official checkout with only a test executor injected by the baseline script.
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use codex_features::Feature;
use codex_login::CodexAuth;
use codex_protocol::error::CodexErr;
use codex_protocol::models::ResponseItem;
use core_test_support::responses;
use serde_json::Value;
use serde_json::json;
use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;
use wiremock::Mock;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

async fn context(url: &str, websocket: bool) -> (Arc<Session>, Arc<TurnContext>) {
    let endpoint = format!("{}/v1", url.replace("ws://", "http://"));
    let (session, mut turn, _) =
        crate::session::tests::make_session_and_context_with_auth_and_config_and_rx(
            CodexAuth::create_dummy_chatgpt_auth_for_testing(),
            Vec::new(),
            move |config| {
                config.model = Some("gpt-5.2".into());
                config.model_provider.base_url = Some(endpoint);
                config.model_provider.supports_websockets = websocket;
                config.model_provider.stream_max_retries = Some(1);
                config.model_provider.request_max_retries = Some(0);
                config.model_provider.stream_idle_timeout_ms = Some(1000);
                let _ = config.features.disable(Feature::UseAgentIdentity);
                let _ = config.features.disable(Feature::UnboundedConnectionRetries);
            },
        )
        .await;
    Arc::get_mut(&mut turn).expect("fixture owns turn").sub_id = "source-turn".into();
    (session, turn)
}

fn request() -> Value {
    json!({"model":"gpt-5.2","instructions":"Answer exactly as requested.",
        "input":[{"id":"msg_user","type":"message","role":"user","content":[{"type":"input_text","text":"fixture"}]}],
        "session_id":"source-session","thread_id":"source-session","turn_id":"source-turn", "stream":true})
}

fn success() -> Vec<Value> {
    vec![
        responses::ev_response_created("resp_success"),
        responses::ev_output_text_delta("hello"),
        responses::ev_assistant_message("msg_result", "hello"),
        responses::ev_completed("resp_success"),
    ]
}

fn outcome(result: Result<Vec<Value>, CodexErr>) -> Value {
    match result {
        Ok(items) => json!({"success":true,"items":items}),
        Err(error) => {
            json!({"success":false,"error":error.to_string(),"httpStatus":error.http_status_code_value()})
        }
    }
}

fn report(name: &str, trace: Value) {
    if let Ok(path) = std::env::var("CODEX_NATIVE_REPORT") {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        writeln!(file, "{}", json!({"case":name,"trace":trace})).unwrap();
    }
}

#[tokio::test]
async fn native_model_blackbox_http_matrix() {
    for case in [
        "success",
        "eof_recovery",
        "content_filter_recovery",
        "failure_then_completion",
        "final_invalid_prompt",
        "final_eof",
        "interrupted",
        "hosted_image",
        "tool_calls",
        "partial_item_recovery",
        "native_lite",
        "compaction_history",
        "text_verbosity",
    ] {
        let server = responses::start_mock_server().await;
        let mut body = request();
        let responses = match case {
            "eof_recovery" => vec![
                responses::sse(vec![responses::ev_response_created("resp_eof")]),
                responses::sse(success()),
            ],
            "partial_item_recovery" => vec![
                responses::sse(vec![
                    responses::ev_response_created("resp_partial"),
                    responses::ev_assistant_message("msg_partial", "partial"),
                ]),
                responses::sse(success()),
            ],
            "content_filter_recovery" => vec![
                responses::sse(vec![
                    json!({"type":"response.incomplete","response":{"id":"resp_filter","incomplete_details":{"reason":"content_filter"}}}),
                ]),
                responses::sse(success()),
            ],
            "failure_then_completion" => {
                let mut events = vec![
                    json!({"type":"response.failed","response":{"id":"resp_failed","error":{"code":"server_error","message":"recoverable"}}}),
                ];
                events.extend(success());
                vec![responses::sse(events)]
            }
            "final_invalid_prompt" => vec![responses::sse_failed(
                "resp_invalid",
                "invalid_prompt",
                "invalid fixture",
            )],
            "final_eof" => {
                vec![responses::sse(vec![responses::ev_response_created("resp_eof")]); 2]
            }
            "interrupted" => vec![responses::sse(vec![
                json!({"type":"response.incomplete","response":{"id":"resp_interrupted","incomplete_details":{"reason":"interrupted"}}}),
            ])],
            "hosted_image" => {
                body["tools"] = json!([{"type":"image_generation","quality":"low"}]);
                vec![responses::sse(success())]
            }
            "tool_calls" => {
                body["tools"] = json!([{"type":"function","name":"shell","parameters":{"type":"object"}},{"type":"function","name":"exec","parameters":{"type":"object"}},{"type":"function","name":"apply_patch","parameters":{"type":"object"}}]);
                vec![responses::sse(vec![
                    responses::ev_function_call(
                        "call_shell",
                        "shell",
                        "{\"command\":\"touch /tmp/codex-native-forbidden-tool\"}",
                    ),
                    responses::ev_function_call("call_exec", "exec", "{}"),
                    responses::ev_function_call("call_patch", "apply_patch", "{}"),
                    responses::ev_completed("resp_tools"),
                ])]
            }
            "native_lite" => {
                body["reasoning"] = json!({"context":"all_turns"});
                body["input"].as_array_mut().unwrap().insert(
                    0,
                    json!({"type":"additional_tools","role":"developer","tools":[]}),
                );
                vec![responses::sse(success())]
            }
            "text_verbosity" => {
                body["text"] = json!({"verbosity":"low"});
                vec![responses::sse(success())]
            }
            "compaction_history" => {
                body["input"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"type":"compaction","encrypted_content":"opaque-native-summary"}));
                vec![responses::sse(success())]
            }
            _ => vec![responses::sse(success())],
        };
        responses::mount_sse_sequence(&server, responses).await;
        let (session, turn) = context(&server.uri(), false).await;
        let result = run_model(&session, &turn, &body, case == "native_lite").await;
        let requests = responses::received_responses_requests(&server).await;
        let trace = json!({"result":outcome(result),"requests":requests.iter().map(|r|r.body_json()).collect::<Vec<_>>()});
        if matches!(case, "final_invalid_prompt" | "final_eof") {
            assert_eq!(trace["result"]["success"], false, "{case}: {trace}");
        } else {
            assert_eq!(trace["result"]["success"], true, "{case}: {trace}");
        }
        assert!(!std::path::Path::new("/tmp/codex-native-forbidden-tool").exists());
        report(case, trace);
    }
}

#[tokio::test]
async fn native_model_blackbox_websocket_and_fallback() {
    for retry in [false, true] {
        let scripts = if retry {
            vec![
                vec![vec![responses::ev_response_created("resp_dropped")]],
                vec![success()],
            ]
        } else {
            vec![vec![success()]]
        };
        let server = responses::start_websocket_server(scripts).await;
        let (session, turn) = context(server.uri(), true).await;
        let result = run_model(&session, &turn, &request(), false).await;
        let requests = server
            .connections()
            .into_iter()
            .flatten()
            .map(|r| r.body_json())
            .collect::<Vec<_>>();
        assert!(result.is_ok(), "{result:?}");
        report(
            if retry {
                "websocket_recovery"
            } else {
                "websocket_success"
            },
            json!({"result":outcome(result),"requests":requests}),
        );
    }
    let server = responses::start_mock_server().await;
    Mock::given(method("GET"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    responses::mount_sse_sequence(&server, vec![responses::sse(success())]).await;
    let (session, turn) = context(&server.uri(), true).await;
    let result = run_model(&session, &turn, &request(), false).await;
    assert!(result.is_ok(), "{result:?}");
    let requests = responses::received_responses_requests(&server)
        .await
        .into_iter()
        .map(|r| r.body_json())
        .collect::<Vec<_>>();
    let methods = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path().ends_with("/responses"))
        .map(|r| r.method.to_string())
        .collect::<Vec<_>>();
    report(
        "websocket_fallback",
        json!({"result":outcome(result),"requests":requests,"methods":methods}),
    );
}

// BEGIN EXECUTOR
#[derive(Default)]
struct Capture {
    events: Mutex<Vec<Value>>,
}
impl codex_api::observation::ResponsesObserver for Capture {
    fn accepted_event(&self, payload: &str) {
        self.events
            .lock()
            .unwrap()
            .push(serde_json::from_str(payload).unwrap());
    }
}
impl codex_http_client::observation::TransportObserver for Capture {
    fn request(&self, _t: &str, _m: &str, _u: &str, _h: &http::HeaderMap, _b: &[u8]) {}
    fn response(&self, _t: &str, _s: u16, _h: &http::HeaderMap) {}
    fn body(&self, _t: &str, _b: &[u8]) {}
    fn error(&self, _t: &str, _m: &str) {}
}
impl crate::InferenceObserver for Capture {
    fn start_attempt(&self) {
        self.events.lock().unwrap().clear();
    }
    fn recovery(&self, _message: &str) {}
}
async fn run_model(
    session: &Arc<Session>,
    turn: &Arc<TurnContext>,
    body: &Value,
    lite: bool,
) -> Result<Vec<Value>, CodexErr> {
    let model_session = crate::InferenceSession::new(
        &turn.config,
        session.services.auth_manager.clone(),
        "fixture-installation",
        "fixture-scope",
        &crate::InferenceIdentity::from_request(body.as_object().unwrap()),
    )
    .unwrap();
    let mut model = turn.model_info().as_ref().clone();
    model.use_responses_lite = lite;
    if body.get("text").is_some() {
        model.support_verbosity = true;
    }
    let capture = Arc::new(Capture::default());
    model_session
        .execute(
            &turn.config,
            &model,
            body.as_object().unwrap(),
            capture.clone(),
        )
        .await?;
    let events = capture.events.lock().unwrap();
    Ok(events
        .iter()
        .filter(|e| e["type"] == "response.output_item.done")
        .map(|e| {
            let item: ResponseItem = serde_json::from_value(e["item"].clone()).unwrap();
            serde_json::to_value(item).unwrap()
        })
        .collect())
}
// END EXECUTOR

// BEGIN MODEL_ONLY_BOUNDARY_TESTS
#[tokio::test]
async fn native_model_client_tool_recovery_never_synthesizes_outputs() {
    for custom in [false, true] {
        let server = responses::start_mock_server().await;
        let event = if custom {
            responses::ev_custom_tool_call(
                "client_call",
                "apply_patch",
                "*** Begin Patch\n*** End Patch",
            )
        } else {
            responses::ev_function_call(
                "client_call",
                "shell",
                "{\"command\":\"touch /tmp/codex-native-forbidden-tool\"}",
            )
        };
        responses::mount_sse_sequence(
            &server,
            vec![
                responses::sse(vec![event.clone()]),
                responses::sse(vec![event, responses::ev_completed("tool_success")]),
            ],
        )
        .await;
        let (session, turn) = context(&server.uri(), false).await;
        let mut body = request();
        body["tools"] = if custom {
            json!([{"type":"custom","name":"apply_patch"}])
        } else {
            json!([{"type":"function","name":"shell","parameters":{"type":"object"}}])
        };
        let prior_call = if custom {
            responses::ev_custom_tool_call("caller_prior", "apply_patch", "caller-provided input")
        } else {
            responses::ev_function_call("caller_prior", "shell", "{}")
        };
        body["input"]
            .as_array_mut()
            .unwrap()
            .push(prior_call["item"].clone());
        body["input"].as_array_mut().unwrap().push(json!({"type":if custom {"custom_tool_call_output"} else {"function_call_output"},"call_id":"caller_prior","output":"original client result"}));
        let output = run_model(&session, &turn, &body, false).await.unwrap();
        assert_eq!(output.len(), 1);
        assert_eq!(output[0]["call_id"], "client_call");
        let requests = responses::received_responses_requests(&server).await;
        assert_eq!(requests.len(), 2);
        let retry = requests[1].body_json();
        let history = retry["input"].as_array().unwrap();
        assert!(!history.iter().any(|item| item["call_id"] == "client_call"));
        let outputs: Vec<_> = history
            .iter()
            .filter(|item| {
                matches!(
                    item["type"].as_str(),
                    Some("function_call_output" | "custom_tool_call_output")
                )
            })
            .collect();
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0]["call_id"], "caller_prior");
        assert_eq!(outputs[0]["output"], "original client result");
        assert!(history.iter().any(|item| item["call_id"] == "caller_prior"
            && matches!(
                item["type"].as_str(),
                Some("function_call" | "custom_tool_call")
            )));
        assert!(!std::path::Path::new("/tmp/codex-native-forbidden-tool").exists());
    }
}

#[tokio::test]
async fn native_model_verbosity_changes_preserve_session_fallback() {
    let (session, turn) = context("http://127.0.0.1:1", true).await;
    let original = session.services.model_client.clone();
    let scoped = original
        .clone()
        .with_model_verbosity(Some(codex_protocol::config_types::Verbosity::Low));
    assert!(original.responses_websocket_enabled());
    assert!(scoped.force_http_fallback(&turn.session_telemetry, turn.model_info()));
    assert!(!original.responses_websocket_enabled());
    let changed =
        original.with_model_verbosity(Some(codex_protocol::config_types::Verbosity::High));
    assert!(!changed.responses_websocket_enabled());
}
