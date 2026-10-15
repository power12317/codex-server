use super::*;
use base64::Engine;
use pretty_assertions::assert_eq;

async fn exchange(ws: &mut Socket, request: Value) -> Result<Vec<Value>> {
    send(ws, request).await?;
    timeout(Duration::from_secs(30), async {
        let mut messages = Vec::new();
        while let Some(message) = ws.next().await {
            if let Message::Text(text) = message? {
                let value: Value = serde_json::from_str(&text)?;
                let terminal = value.get("error").is_some()
                    || matches!(
                        value["method"].as_str(),
                        Some("cpa/inference/completed" | "cpa/inference/error")
                    );
                messages.push(value);
                if terminal {
                    return Ok(messages);
                }
            }
        }
        anyhow::bail!("IPC closed before terminal response")
    })
    .await?
}

#[test_case::test_case(None, "header", 0; "old_client_default")]
#[test_case::test_case(Some(false), "header", 0; "explicit_disabled")]
#[test_case::test_case(Some(true), "header", 1; "snapshot_before_accept")]
#[test_case::test_case(Some(true), "metadata", 1; "snapshot_during_body")]
#[test_case::test_case(Some(true), "delete", 0; "deleted_cookie")]
#[test_case::test_case(Some(true), "expired", 0; "expired_cookie")]
#[test_case::test_case(Some(true), "incomplete", 0; "missing_pair")]
#[test_case::test_case(Some(true), "model-text", 0; "ignore_model_text")]
#[test_case::test_case(Some(true), "error", 1; "http_error_still_has_snapshot")]
#[tokio::test]
async fn cpa_ticket_snapshot_is_opt_in_private_and_ordered(
    capture: Option<bool>,
    kind: &str,
    count: usize,
) -> Result<()> {
    let upstream = MockServer::start().await;
    let home = TempDir::new()?;
    prepare_account(&upstream, &home)?;
    let path = home.path().join("state/shared.json/config.toml");
    let mut config = std::fs::read_to_string(&path)?;
    if kind != "incomplete" {
        config.push_str("\n[model_providers.mock_provider.http_headers]\nCookie = '__cflb=outbound-cf; __oailb=outbound-oai'\n");
    }
    std::fs::write(path, config)?;
    let mut response = ResponseTemplate::new(if kind == "error" { 400 } else { 200 });
    if !matches!(kind, "metadata" | "model-text") {
        response = response.insert_header("x-codex-turn-state", "header-private-ticket");
    }
    let update = match kind {
        "delete" => "__oailb=; Max-Age=0",
        "expired" => "__oailb=expired; Expires=Thu, 01 Jan 1970 00:00:00 GMT",
        _ => "__oailb=response-oai; Max-Age=120",
    };
    response = response.insert_header("set-cookie", update);
    let event = if matches!(kind, "metadata" | "model-text") {
        format!(
            "data: {{\"type\":\"{}\",\"headers\":{{\"x-codex-turn-state\":\"metadata-private-ticket\"}}}}\r\n\r\n",
            if kind == "metadata" {
                "codex.response.metadata"
            } else {
                "response.output_text.delta"
            }
        )
    } else {
        String::new()
    };
    let body = format!("{event}data: [DONE]\n\n");
    Mock::given(method("POST"))
        .and(wiremock::matchers::path("/v1/responses"))
        .respond_with(response.set_body_raw(body.clone(), "text/event-stream"))
        .expect(1)
        .mount(&upstream)
        .await;
    let runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    send(
        &mut ws,
        json!({"id":1,"method":"cpa/capabilities/read","params":{}}),
    )
    .await?;
    let capabilities = receive(&mut ws, "id", json!(1)).await?;
    assert_eq!(capabilities["result"]["protocolVersion"], 3);
    assert_eq!(capabilities["result"]["ticketSnapshots"], true);
    let mut request = start(json!({"model":"mock-model","input":[],"stream":true}));
    if let Some(capture) = capture {
        request["params"]["captureTicketSnapshot"] = json!(capture);
    }
    let messages = exchange(&mut ws, request).await?;
    let tickets: Vec<_> = messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message["method"] == "cpa/inference/ticket")
        .collect();
    assert_eq!(tickets.len(), count);
    if let Some((index, notification)) = tickets.first() {
        let ticket = &notification["params"];
        assert_eq!(ticket["requestId"], "r");
        assert_eq!(ticket["credentialId"], "shared.json");
        assert_eq!(ticket["model"], "mock-model");
        assert_eq!(
            ticket["state"],
            if kind == "metadata" {
                "metadata-private-ticket"
            } else {
                "header-private-ticket"
            }
        );
        assert_eq!(ticket["route"]["__cflb"]["value"], "outbound-cf");
        assert!(ticket["route"]["__cflb"].get("expires_at").is_none());
        assert_eq!(ticket["route"]["__oailb"]["value"], "response-oai");
        let observed =
            chrono::DateTime::parse_from_rfc3339(ticket["observedAt"].as_str().expect("time"))?;
        let expiry = chrono::DateTime::parse_from_rfc3339(
            ticket["route"]["__oailb"]["expires_at"]
                .as_str()
                .expect("expiry"),
        )?;
        assert!((expiry - observed).num_seconds() <= 120);
        assert!(*index < messages.len() - 1);
        if kind == "header" {
            assert!(
                *index
                    < messages
                        .iter()
                        .position(|message| message.get("result").is_some())
                        .expect("accept")
            );
        }
    }
    if kind != "error" {
        let mut actual = Vec::new();
        for message in &messages {
            if message["method"] == "cpa/inference/body" {
                actual.extend(
                    base64::engine::general_purpose::STANDARD
                        .decode(message["params"]["bodyBase64"].as_str().expect("body"))?,
                );
            }
        }
        assert_eq!(String::from_utf8(actual)?, body);
    }
    let upstream_request = messages
        .iter()
        .find(|message| {
            message["method"] == "cpa/inference/upstream" && message["params"]["kind"] == "request"
        })
        .expect("actual request");
    assert_eq!(
        upstream_request["params"]["headers"]["authorization"],
        json!(["Bearer fake-cpa-access"])
    );
    if kind != "incomplete" {
        assert_eq!(
            upstream_request["params"]["headers"]["cookie"],
            json!(["__cflb=outbound-cf; __oailb=outbound-oai"])
        );
    }
    let terminal_reply = messages
        .iter()
        .find(|message| message.get("id").is_some())
        .expect("start reply");
    let headers = if kind == "error" {
        &terminal_reply["error"]["data"]["headers"]
    } else {
        &terminal_reply["result"]["headers"]
    };
    assert_eq!(headers["set-cookie"], json!([update]));
    let log = runtime
        .wait_for_json_log_event("cpa.request.upstream.response", Duration::from_secs(10))
        .await?;
    assert!(!log.to_string().contains("header-private-ticket"));
    let sent = upstream.received_requests().await.expect("recorded");
    let body: Value = sent
        .iter()
        .find(|request| request.url.path() == "/v1/responses")
        .expect("native request")
        .body_json()?;
    assert!(body.get("captureTicketSnapshot").is_none());
    Ok(())
}

#[tokio::test]
async fn cpa_ticket_concurrent_credentials_and_partial_updates_never_mix() -> Result<()> {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(|request: &wiremock::Request| {
            let body: Value = request.body_json().expect("JSON request");
            let side = body["fixture_side"].as_str().expect("side");
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(20))
                .insert_header("x-codex-turn-state", format!("ticket-{side}"))
                .insert_header("set-cookie", format!("__oailb=response-{side}; Max-Age=60"))
                .set_body_raw("data: [DONE]\n\n", "text/event-stream")
        })
        .expect(2)
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    prepare_account(&upstream, &home)?;
    for (side, credential) in [("left", "shared.json"), ("right", "right.json")] {
        if credential != "shared.json" {
            std::fs::copy(
                home.path().join("shared.json"),
                home.path().join(credential),
            )?;
        }
        let account = home.path().join("state").join(credential);
        std::fs::create_dir_all(&account)?;
        let config = std::fs::read_to_string(home.path().join("config.toml"))?;
        std::fs::write(
            account.join("config.toml"),
            format!(
                "{config}\n[model_providers.mock_provider.http_headers]\nCookie = '__cflb=request-{side}; __oailb=old-{side}'\n"
            ),
        )?;
    }
    let _runtime = launch(&upstream, &home).await?;
    let mut left = connect(&home).await?;
    let mut right = connect(&home).await?;
    let request = |side: &str, credential: &str| {
        let mut value = start(
            json!({"model":format!("model-{side}"),"input":[],"stream":true,"fixture_side":side}),
        );
        value["params"]["requestId"] = json!(side);
        value["params"]["credentialId"] = json!(credential);
        value["params"]["captureTicketSnapshot"] = json!(true);
        value
    };
    let (left, right) = tokio::try_join!(
        exchange(&mut left, request("left", "shared.json")),
        exchange(&mut right, request("right", "right.json"))
    )?;
    for (side, credential, messages) in [
        ("left", "shared.json", left),
        ("right", "right.json", right),
    ] {
        let snapshots: Vec<_> = messages
            .iter()
            .filter(|message| message["method"] == "cpa/inference/ticket")
            .collect();
        assert_eq!(snapshots.len(), 1);
        let snapshot = &snapshots[0]["params"];
        assert_eq!(snapshot["credentialId"], credential);
        assert_eq!(snapshot["model"], format!("model-{side}"));
        assert_eq!(snapshot["state"], format!("ticket-{side}"));
        assert_eq!(
            snapshot["route"]["__cflb"]["value"],
            format!("request-{side}")
        );
        assert_eq!(
            snapshot["route"]["__oailb"]["value"],
            format!("response-{side}")
        );
    }
    Ok(())
}

#[tokio::test]
async fn cpa_ticket_auth_recovery_does_not_pair_material_across_attempts() -> Result<()> {
    let upstream = MockServer::start().await;
    app_test_support::mount_workspace_routing(&upstream).await;
    let home = TempDir::new()?;
    prepare_account(&upstream, &home)?;
    let auth: Value = serde_json::from_slice(&std::fs::read(home.path().join("shared.json"))?)?;
    Mock::given(method("POST")).and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"access_token":"ticket-refreshed", "refresh_token":"refresh-next", "id_token":auth["id_token"]}))).expect(1).mount(&upstream).await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .and(wiremock::matchers::header(
            "authorization",
            "Bearer fake-cpa-access",
        ))
        .respond_with(
            ResponseTemplate::new(401)
                .insert_header("x-codex-turn-state", "failed-attempt-ticket")
                .insert_header("set-cookie", "__cflb=failed-attempt-cf")
                .set_body_string("refresh needed"),
        )
        .mount(&upstream)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .and(wiremock::matchers::header(
            "authorization",
            "Bearer ticket-refreshed",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-codex-turn-state", "successful-attempt-ticket")
                .insert_header("set-cookie", "__oailb=successful-attempt-oai")
                .set_body_raw("data: [DONE]\n\n", "text/event-stream"),
        )
        .expect(1)
        .mount(&upstream)
        .await;
    let _runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let mut request = start(json!({"model":"mock-model","input":[],"stream":true}));
    request["params"]["captureTicketSnapshot"] = json!(true);
    let messages = exchange(&mut ws, request).await?;
    assert_eq!(
        messages.last().expect("terminal")["method"],
        "cpa/inference/completed"
    );
    assert!(
        !messages
            .iter()
            .any(|message| message["method"] == "cpa/inference/ticket"),
        "neither exchange contains a complete pair"
    );
    let sent = upstream.received_requests().await.expect("recorded");
    let model_requests: Vec<_> = sent
        .iter()
        .filter(|request| request.url.path() == "/v1/responses")
        .collect();
    assert!(model_requests.len() >= 2);
    for request in model_requests {
        assert!(request.headers.get("cookie").is_none());
    }
    Ok(())
}
