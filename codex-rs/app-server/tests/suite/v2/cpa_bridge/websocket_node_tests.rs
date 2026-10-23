//! Routing diagnostics belong to one actual upstream socket, including reused requests.
use super::*;
use core_test_support::responses;
use pretty_assertions::assert_eq;

fn node_cookie(node: &str) -> String {
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(json!({"host":format!("chat.gateway.{node}.api.openai.com")}).to_string());
    format!("__oailb=e30.{payload}.fixture-signature")
}

#[test_case::test_case(false, None; "new_connection_without_cookie_clears_node")]
#[test_case::test_case(true, None; "response_priority_and_actual_request_fallback")]
#[test_case::test_case(true, Some("__oailb=; Max-Age=0"); "response_deletion_clears_node")]
#[test_case::test_case(true, Some("__oailb=invalid"); "invalid_response_clears_node")]
#[tokio::test]
async fn cpa_native_ws_node_follows_connection_lifetime(
    request_cookie: bool,
    last_cookie: Option<&str>,
) -> Result<()> {
    let cookies = [
        Some(node_cookie("unified-107")),
        Some(node_cookie("unified-108")),
        last_cookie.map(str::to_owned),
    ];
    let websocket = responses::start_websocket_server_with_headers(
        cookies
            .iter()
            .enumerate()
            .map(|(connection, cookie)| {
                let mut requests = (0..2)
                    .map(|round| {
                        let id = format!("resp_{connection}_{round}");
                        vec![
                            responses::ev_response_created(&id),
                            responses::ev_output_text_delta("connection fixture"),
                            responses::ev_completed(&id),
                        ]
                    })
                    .collect::<Vec<_>>();
                if connection < 2 {
                    // A failed send ends this socket deterministically. Only a new CPA
                    // invocation may open the next one; the worker must not retry it.
                    requests.push(vec![json!({"type":"response.failed","response":{
                        "error":{"code":"server_error","message":"close fixture connection"}
                    }})]);
                }
                responses::WebSocketConnectionConfig {
                    requests,
                    response_headers: cookie
                        .iter()
                        .map(|value| ("set-cookie".into(), value.clone()))
                        .collect(),
                    accept_delay: None,
                    close_after_requests: true,
                }
            })
            .collect(),
    )
    .await;
    let upstream = MockServer::start().await;
    let home = TempDir::new()?;
    prepare_account(&upstream, &home)?;
    let config_path = home.path().join("state/shared.json/config.toml");
    let mut config: toml::Value = toml::from_str(&std::fs::read_to_string(&config_path)?)?;
    let provider = &mut config["model_providers"]["mock_provider"];
    provider["base_url"] = toml::Value::String(format!(
        "{}/v1",
        websocket.uri().replace("ws://", "http://")
    ));
    provider["supports_websockets"] = toml::Value::Boolean(true);
    let sent_cookie = node_cookie("unified-100");
    if request_cookie {
        provider
            .as_table_mut()
            .expect("fixture provider table")
            .insert(
                "http_headers".into(),
                toml::Value::Table(toml::Table::from_iter([(
                    "cookie".into(),
                    toml::Value::String(sent_cookie.clone()),
                )])),
            );
    }
    std::fs::write(config_path, toml::to_string(&config)?)?;
    let _runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let input = json!({"model":"gpt-5.2","input":"fixture","stream":true,
        "session_id":"node-session","thread_id":"node-session",
        "turn_id":"01a1335a-1111-7111-8111-111111111111"});
    let last_node = (request_cookie && last_cookie.is_none()).then_some("unified-100");
    for (connection, expected_node) in [Some("unified-107"), Some("unified-108"), last_node]
        .into_iter()
        .enumerate()
    {
        let round_count = if connection < 2 { 3 } else { 2 };
        for round in 0..round_count {
            let id = 1300 + (connection * 3 + round) as u64;
            let result = exchange(&mut ws, rpc(input.clone(), id)).await?;
            if round == 2 {
                assert!(result.reply.get("error").is_some(), "{}", result.reply);
            } else {
                assert_eq!(
                    result.reply["result"]["statusCode"], 200,
                    "{}",
                    result.reply
                );
                assert!(result.error.is_none());
                let body = std::str::from_utf8(&result.body)?;
                assert!(body.contains("connection fixture") && body.contains("response.completed"));
            }
            let model_logs = result
                .logs
                .iter()
                .filter(|log| log["kind"] == "request" && log["method"] == "response.create")
                .collect::<Vec<_>>();
            assert_eq!(model_logs.len(), 1, "one native send, without retry");
            assert_eq!(model_logs[0]["oaiLbNode"].as_str(), expected_node);
            assert_eq!(
                model_logs[0]["headers"],
                json!({}),
                "no synthetic wire headers"
            );
            let handshakes = result
                .logs
                .iter()
                .filter(|log| log["kind"] == "request" && log["method"] == "GET")
                .collect::<Vec<_>>();
            let upgrades = result
                .logs
                .iter()
                .filter(|log| log["kind"] == "response")
                .collect::<Vec<_>>();
            assert_eq!(handshakes.len(), usize::from(round == 0));
            assert_eq!(
                upgrades.len(),
                usize::from(round == 0),
                "no replayed handshake response"
            );
            if round == 0 {
                assert_eq!(upgrades[0]["statusCode"], 101);
                assert_eq!(upgrades[0]["oaiLbNode"].as_str(), expected_node);
                assert_eq!(
                    handshakes[0]["oaiLbNode"].as_str(),
                    request_cookie.then_some("unified-100")
                );
                assert_eq!(
                    handshakes[0]["headers"]["authorization"],
                    json!(["Bearer fake-cpa-access"])
                );
                if request_cookie {
                    assert_eq!(handshakes[0]["headers"]["cookie"], json!([sent_cookie]));
                }
                if cookies[connection].is_some() {
                    assert_eq!(
                        upgrades[0]["headers"]["set-cookie"],
                        json!([cookies[connection]])
                    );
                }
            }
        }
    }
    let handshakes = websocket.handshakes();
    assert_eq!(handshakes.len(), 3);
    for handshake in handshakes {
        assert_eq!(
            handshake.header("cookie"),
            request_cookie.then(|| sent_cookie.clone())
        );
    }
    let connections = websocket.connections();
    assert_eq!(
        connections.iter().map(Vec::len).collect::<Vec<_>>(),
        vec![3, 3, 2]
    );
    for request in connections.iter().flatten() {
        assert!(!request.body_json().to_string().contains("__oailb"));
    }
    Ok(())
}
