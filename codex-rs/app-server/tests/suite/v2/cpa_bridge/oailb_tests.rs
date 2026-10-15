use super::*;
use base64::Engine;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use test_case::test_case;

fn node_cookie(node: &str) -> String {
    let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(json!({"host":format!("chat.gateway.{node}.api.openai.com")}).to_string());
    format!("__oailb=e30.{claims}.fixture-signature")
}

fn configure_cookie(home: &TempDir, credential: &str, node: Option<&str>) -> Result<()> {
    if credential != "shared.json" {
        std::fs::copy(
            home.path().join("shared.json"),
            home.path().join(credential),
        )?;
    }
    let account_home = home.path().join("state").join(credential);
    std::fs::create_dir_all(&account_home)?;
    let mut config = std::fs::read_to_string(home.path().join("config.toml"))?;
    if let Some(node) = node {
        config.push_str(&format!(
            "\n[model_providers.mock_provider.http_headers]\nCookie = {:?}\n",
            format!("__cf_bm=fixture; {}", node_cookie(node))
        ));
    }
    std::fs::write(account_home.join("config.toml"), config)?;
    Ok(())
}

fn request(id: &str, credential: &str, status: u16) -> Value {
    let mut request = start(
        json!({"model":"mock-model", "stream":true, "input":[], "fixture_id":id, "fixture_status":status}),
    );
    request["id"] = json!(id);
    request["params"]["requestId"] = json!(id);
    request["params"]["credentialId"] = json!(credential);
    request
}

async fn collect(ws: &mut Socket, count: usize) -> Result<BTreeMap<String, Vec<Value>>> {
    timeout(Duration::from_secs(30), async {
        let mut exchanges: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        let mut completed = 0;
        while let Some(message) = ws.next().await {
            let Message::Text(text) = message? else {
                continue;
            };
            let message: Value = serde_json::from_str(&text)?;
            let terminal = message.get("error").is_some()
                || matches!(
                    message["method"].as_str(),
                    Some("cpa/inference/completed" | "cpa/inference/error")
                );
            let id = message["params"]["requestId"]
                .as_str()
                .or_else(|| message["id"].as_str())
                .ok_or_else(|| anyhow::anyhow!("missing request correlation: {message}"))?;
            exchanges.entry(id.to_owned()).or_default().push(message);
            if terminal {
                completed += 1;
                if completed == count {
                    return Ok(exchanges);
                }
            }
        }
        anyhow::bail!("connection closed before inference completed")
    })
    .await?
}

fn observation(messages: &[Value], kind: &str) -> Value {
    let notifications: Vec<_> = messages
        .iter()
        .filter(|message| {
            message["method"] == "cpa/inference/upstream" && message["params"]["kind"] == kind
        })
        .collect();
    assert_eq!(
        notifications.len(),
        1,
        "one {kind} notification per exchange"
    );
    notifications[0]["params"].clone()
}

#[test_case(Some("unified-a"), "absent", Some("unified-a"); "request_only")]
#[test_case(Some("unified-a"), "other", Some("unified-a"); "unrelated_response_cookie")]
#[test_case(Some("unified-a"), "replace", Some("unified-b"); "response_overrides_request")]
#[test_case(None, "replace", Some("unified-b"); "response_only")]
#[test_case(None, "absent", None; "neither")]
#[test_case(None, "other", None; "unrelated_cookie_without_request_node")]
#[test_case(Some("unified-a"), "delete", None; "explicit_deletion")]
#[test_case(Some("unified-a"), "invalid", None; "invalid_response_node")]
#[tokio::test]
async fn cpa_oailb_exchange_success_and_http_error(
    request_node: Option<&str>,
    response_cookie: &str,
    expected: Option<&str>,
) -> Result<()> {
    let upstream = MockServer::start().await;
    let cookie = match response_cookie {
        "absent" => None,
        "other" => Some("__cflb=fixture; Path=/".to_owned()),
        "replace" => Some(format!("{}; Path=/", node_cookie("unified-b"))),
        "delete" => Some("__oailb=; Max-Age=0; Path=/".to_owned()),
        "invalid" => Some("__oailb=invalid; Path=/".to_owned()),
        _ => unreachable!(),
    };
    let expected_cookie = cookie.clone();
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(move |request: &wiremock::Request| {
            let body: Value = request.body_json().expect("fixture request must be JSON");
            let status = body["fixture_status"]
                .as_u64()
                .expect("fixture HTTP status") as u16;
            let mut response = ResponseTemplate::new(status);
            if let Some(cookie) = &cookie {
                response = response.insert_header("set-cookie", cookie.clone());
            }
            if status == 200 {
                response.set_body_raw("data: [DONE]\n\n", "text/event-stream")
            } else {
                response.set_body_json(json!({"error":{"message":"fixture error"}}))
            }
        })
        .expect(2)
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    prepare_account(&upstream, &home)?;
    configure_cookie(&home, "shared.json", request_node)?;
    let _runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    for status in [200, 400] {
        let id = format!("exchange-{status}");
        send(&mut ws, request(&id, "shared.json", status)).await?;
        let exchanges = collect(&mut ws, 1).await?;
        let messages = &exchanges[&id];
        let sent = observation(messages, "request");
        let received = observation(messages, "response");
        assert_eq!(sent["oaiLbNode"], json!(request_node));
        assert_eq!(received["oaiLbNode"], json!(expected));
        assert_eq!(received["statusCode"], json!(status));
        if let Some(request_node) = request_node {
            assert_eq!(
                sent["headers"]["cookie"],
                json!([format!("__cf_bm=fixture; {}", node_cookie(request_node))])
            );
        }
        if response_cookie != "absent" {
            assert_eq!(received["headers"]["set-cookie"], json!([expected_cookie]));
        }
        let last = messages.last().expect("terminal message");
        if status == 200 {
            assert_eq!(last["method"], json!("cpa/inference/completed"));
        } else {
            assert_eq!(last["error"]["data"]["httpStatus"], json!(status));
        }
    }
    let requests: Vec<_> = upstream
        .received_requests()
        .await
        .expect("request recording enabled")
        .into_iter()
        .filter(|request| request.url.path() == "/v1/responses")
        .collect();
    assert_eq!(requests.len(), 2);
    for request in requests {
        let actual = request
            .headers
            .get("cookie")
            .map(|value| value.to_str().expect("fixture cookies are ASCII"));
        let expected = request_node.map(|node| format!("__cf_bm=fixture; {}", node_cookie(node)));
        assert_eq!(actual, expected.as_deref());
    }
    Ok(())
}

#[tokio::test]
async fn cpa_oailb_concurrent_requests_and_workers_keep_their_own_nodes() -> Result<()> {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(|request: &wiremock::Request| {
            let body: Value = request.body_json().expect("fixture request must be JSON");
            let id = body["fixture_id"].as_str().expect("fixture request ID");
            let response =
                ResponseTemplate::new(200).set_body_raw("data: [DONE]\n\n", "text/event-stream");
            if id.ends_with("replace") {
                response.insert_header(
                    "set-cookie",
                    format!("{}; Path=/", node_cookie("unified-new")),
                )
            } else {
                response.set_delay(Duration::from_millis(80))
            }
        })
        .expect(4)
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    prepare_account(&upstream, &home)?;
    configure_cookie(&home, "shared.json", Some("unified-a"))?;
    configure_cookie(&home, "second.json", Some("unified-c"))?;
    let _runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let cases = [
        ("a-original", "shared.json", "unified-a", "unified-a"),
        ("c-original", "second.json", "unified-c", "unified-c"),
        ("a-replace", "shared.json", "unified-a", "unified-new"),
        ("c-replace", "second.json", "unified-c", "unified-new"),
    ];
    for (id, credential, _, _) in cases {
        send(&mut ws, request(id, credential, 200)).await?;
    }
    let exchanges = collect(&mut ws, cases.len()).await?;
    for (id, _, sent, received) in cases {
        let messages = &exchanges[id];
        assert_eq!(observation(messages, "request")["oaiLbNode"], json!(sent));
        assert_eq!(
            observation(messages, "response")["oaiLbNode"],
            json!(received)
        );
        assert_eq!(
            messages.last().expect("terminal message")["method"],
            json!("cpa/inference/completed")
        );
    }
    for request in upstream
        .received_requests()
        .await
        .expect("request recording enabled")
        .into_iter()
        .filter(|request| request.url.path() == "/v1/responses")
    {
        let body: Value = request.body_json()?;
        let id = body["fixture_id"].as_str().expect("fixture request ID");
        let expected = if id.starts_with('a') {
            "unified-a"
        } else {
            "unified-c"
        };
        assert_eq!(
            request.headers["cookie"],
            format!("__cf_bm=fixture; {}", node_cookie(expected))
        );
    }
    Ok(())
}

#[tokio::test]
async fn cpa_oailb_network_failure_keeps_outbound_request_observation() -> Result<()> {
    let upstream = MockServer::start().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    // Close during TLS setup, before an HTTP response exists. An HTTP-only proxy
    // can translate a plain HTTP disconnect into a valid 502 response instead.
    let connection = tokio::spawn(async move {
        loop {
            let (socket, _) = listener
                .accept()
                .await
                .expect("accept TLS fixture connection");
            drop(socket);
        }
    });
    let home = TempDir::new()?;
    prepare_account(&upstream, &home)?;
    configure_cookie(&home, "shared.json", Some("unified-a"))?;
    let config_path = home.path().join("state/shared.json/config.toml");
    let config = std::fs::read_to_string(&config_path)?.replace(
        &format!("base_url = \"{}/v1\"", upstream.uri()),
        &format!("base_url = \"https://{address}/v1\""),
    );
    std::fs::write(config_path, config)?;
    let _runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    send(&mut ws, request("network-failure", "shared.json", 200)).await?;
    let exchanges = collect(&mut ws, 1).await?;
    connection.abort();
    let messages = &exchanges["network-failure"];
    assert_eq!(
        observation(messages, "request")["oaiLbNode"],
        json!("unified-a")
    );
    assert_eq!(observation(messages, "error")["kind"], json!("error"));
    assert!(
        !messages
            .iter()
            .any(|message| message["params"]["kind"] == "response")
    );
    assert_eq!(
        messages.last().expect("terminal message")["error"]["data"]["httpStatus"],
        json!(502)
    );
    Ok(())
}
