//! IPC v5 contract and retained account lifecycle regressions.
#[path = "cpa_bridge/home_tests.rs"]
mod home_tests;
#[path = "cpa_bridge/native_runtime_tests.rs"]
mod native_runtime_tests;

use anyhow::Result;
use app_test_support::ChatGptIdTokenClaims;
use app_test_support::MockResponsesConfig;
use app_test_support::TestAppServer;
use app_test_support::encode_id_token;
use futures::SinkExt;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use tempfile::TempDir;
use tokio::net::TcpStream;
use tokio::time::Duration;
use tokio::time::timeout;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::client_async;
use tokio_tungstenite::tungstenite::Message;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

type Socket = WebSocketStream<TcpStream>;

async fn connect(home: &TempDir) -> Result<Socket> {
    let port = std::fs::read_to_string(home.path().join("port"))?;
    let socket = format!("127.0.0.1:{port}");
    let stream = timeout(Duration::from_secs(/*secs*/ 30), async {
        loop {
            if let Ok(stream) = TcpStream::connect(&socket).await {
                break stream;
            }
            tokio::time::sleep(Duration::from_millis(/*millis*/ 20)).await;
        }
    })
    .await?;
    let (mut ws, _) = client_async(format!("ws://{socket}/cpa/v1/ws"), stream).await?;
    send(&mut ws, json!({"id":0,"method":"initialize","params":{"clientInfo":{"name":"cpa-tests","version":"1"},"capabilities":{"experimentalApi":true}}})).await?;
    let initialized = receive(&mut ws, "id", json!(0)).await?;
    assert!(initialized.get("result").is_some(), "{initialized}");
    send(&mut ws, json!({"method":"initialized"})).await?;
    Ok(ws)
}

async fn send(ws: &mut Socket, value: Value) -> Result<()> {
    ws.send(Message::Text(value.to_string().into())).await?;
    Ok(())
}

async fn receive(ws: &mut Socket, key: &str, expected: Value) -> Result<Value> {
    timeout(Duration::from_secs(/*secs*/ 30), async {
        while let Some(message) = ws.next().await {
            let message = message?;
            if let Message::Text(text) = message {
                let value: Value = serde_json::from_str(&text)?;
                if value[key] == expected {
                    return Ok(value);
                }
            }
        }
        anyhow::bail!("socket closed before expected message")
    })
    .await?
}

fn prepare_account(upstream: &MockServer, home: &TempDir) -> Result<()> {
    MockResponsesConfig::new(&upstream.uri())
        .with_provider_name("OpenAI")
        .disable_feature(codex_features::Feature::EnableRequestCompression)
        .with_provider_config("requires_openai_auth = true\nstream_idle_timeout_ms = 1000\nsupports_websockets = false")
        .with_root_config(&format!(
            "chatgpt_base_url = {:?}\ncli_auth_credentials_store = \"file\"",
            upstream.uri()
        ))
        .write(home.path())?;
    let fixture_config: toml::Value =
        toml::from_str(&std::fs::read_to_string(home.path().join("config.toml"))?)?;
    assert_eq!(
        fixture_config["model_provider"].as_str(),
        Some("mock_provider")
    );
    assert_eq!(
        fixture_config["model_providers"]["mock_provider"]["base_url"].as_str(),
        Some(format!("{}/v1", upstream.uri()).as_str())
    );
    let id_token = encode_id_token(
        &ChatGptIdTokenClaims::new()
            .chatgpt_account_id("account")
            .chatgpt_user_id("fixture-user")
            .email("fixture@example.com"),
    )?;
    // Existing filename deliberately differs from worker ID; login/refresh reuse it unchanged.
    std::fs::write(
        home.path().join("shared.json"),
        serde_json::to_vec(&json!({
            "type":"codex", "id_token":id_token, "access_token":"fake-cpa-access",
            "refresh_token":"refresh-token", "account_id":"account", "email":"fixture@example.com",
            "last_refresh":chrono::Utc::now(), "expired":"2099-01-01T00:00:00Z",
            "unknown_field":{"keep":true}, "codex_cli":{"enabled":true}
        }))?,
    )?;
    let account_home = home.path().join("state/shared.json");
    std::fs::create_dir_all(&account_home)?;
    std::fs::copy(
        home.path().join("config.toml"),
        account_home.join("config.toml"),
    )?;
    Ok(())
}

async fn server(upstream: &MockServer, home: &TempDir) -> Result<TestAppServer> {
    prepare_account(upstream, home)?;
    launch(upstream, home).await
}

async fn launch(upstream: &MockServer, home: &TempDir) -> Result<TestAppServer> {
    launch_with_timezone(upstream, home, None).await
}

async fn launch_with_timezone(
    upstream: &MockServer,
    home: &TempDir,
    timezone: Option<&str>,
) -> Result<TestAppServer> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port().to_string();
    drop(listener);
    std::fs::write(home.path().join("port"), &port)?;
    let state = home.path().join("state");
    TestAppServer::builder()
        .with_codex_home(home.path())
        .with_env_overrides(&[
            ("TZ", timezone),
            (
                "CODEX_CPA_AUTH_DIR",
                Some(&home.path().display().to_string()),
            ),
            ("CODEX_CPA_AUTH_FILE", None),
            ("RUST_LOG", None),
            ("LOG_FORMAT", Some("json")),
            ("CODEX_HOME", Some(&state.display().to_string())),
            ("CODEX_CPA_TEST_STDIN_LIFETIME", Some("1")),
            ("CODEX_CPA_ANALYTICS_TEST_DELAY_SECONDS", Some("30")),
            ("CODEX_CPA_PORT", Some(&port)),
            ("CODEX_APP_SERVER_LOGIN_ISSUER", Some(&upstream.uri())),
            (
                "CODEX_REFRESH_TOKEN_URL_OVERRIDE",
                Some(&format!("{}/oauth/token", upstream.uri())),
            ),
        ])
        .build()
        .await
}

fn start(request: Value) -> Value {
    json!({"id":2,"method":"cpa/inference/start","params":{"requestId":"r","credentialId":"shared.json","operation":"responses","sourceFormat":"openai-response","sessionId":"caller-worker-account","request":request}})
}

#[tokio::test]
async fn cpa_cancel_before_headers_and_disconnect_leave_runtime_alive() -> Result<()> {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(
            ResponseTemplate::new(/*s*/ 200)
                .set_delay(Duration::from_secs(/*secs*/ 120))
                .set_body_string("unused"),
        )
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    let _runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    send(
        &mut ws,
        start(json!({"model":"mock-model","stream":true,"input":[]})),
    )
    .await?;
    // Wait for actual upstream admission before cancellation.
    timeout(Duration::from_secs(/*secs*/ 30), async {
        loop {
            if upstream
                .received_requests()
                .await
                .unwrap()
                .iter()
                .any(|r| r.url.path() == "/v1/responses")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(/*millis*/ 10)).await;
        }
    })
    .await?;
    send(
        &mut ws,
        json!({"id":3,"method":"cpa/inference/cancel","params":{"requestId":"r"}}),
    )
    .await?;
    // Cancellation response and start error are independently queued.
    let mut replies = std::collections::BTreeMap::new();
    while replies.len() < 2 {
        if let Message::Text(text) = timeout(Duration::from_secs(/*secs*/ 10), ws.next())
            .await?
            .unwrap()?
        {
            let value: Value = serde_json::from_str(&text)?;
            if let Some(id) = value["id"].as_u64() {
                replies.insert(id, value);
            }
        }
    }
    assert_eq!(replies[&2]["error"]["data"]["httpStatus"], json!(499));
    assert_eq!(replies[&3]["result"], json!({}));
    ws.close(/*msg*/ None).await?;
    let mut ws = connect(&home).await?;
    send(
        &mut ws,
        start(json!({"model":"mock-model","stream":true,"input":[]})),
    )
    .await?;
    ws.close(/*msg*/ None).await?;
    let mut ws = connect(&home).await?;
    send(
        &mut ws,
        json!({"id":4,"method":"cpa/capabilities/read","params":{}}),
    )
    .await?;
    assert_eq!(
        receive(&mut ws, "id", json!(4)).await?["result"]["protocolVersion"],
        json!(5)
    );
    Ok(())
}

#[tokio::test]
async fn cpa_managed_401_returns_original_error_without_refresh_or_resend() -> Result<()> {
    let upstream = MockServer::start().await;
    app_test_support::mount_workspace_routing(&upstream).await;
    let home = TempDir::new()?;
    let _runtime = server(&upstream, &home).await?;
    let auth: Value = serde_json::from_slice(&std::fs::read(home.path().join("shared.json"))?)?;
    Mock::given(method("POST")).and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(/*s*/ 200).set_body_json(json!({"access_token":"fake-refreshed", "refresh_token":"fake-refresh-next", "id_token":auth["id_token"]}))).expect(/*r*/ 0).mount(&upstream).await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .and(wiremock::matchers::header(
            "authorization",
            "Bearer fake-cpa-access",
        ))
        .respond_with(ResponseTemplate::new(/*s*/ 401).set_body_string("original-401-body"))
        .mount(&upstream)
        .await;
    Mock::given(method("POST")).and(path("/v1/responses")).and(wiremock::matchers::header("authorization","Bearer fake-refreshed"))
        .respond_with(ResponseTemplate::new(/*s*/ 200).insert_header("content-type","text/event-stream").set_body_string("data: {\"type\":\"response.completed\",\"response\":{\"id\":\"refreshed\"}}\n\n")).expect(/*r*/ 0).mount(&upstream).await;
    let mut ws = connect(&home).await?;
    send(
        &mut ws,
        start(json!({"model":"mock-model","stream":true,"input":[]})),
    )
    .await?;
    let mut transport_logs = Vec::new();
    let accepted = timeout(Duration::from_secs(/*secs*/ 30), async {
        let mut accepted = Value::Null;
        let mut completed = false;
        let mut diagnostics_done = false;
        loop {
            if let Some(Ok(Message::Text(text))) = ws.next().await {
                let message: Value = serde_json::from_str(&text)?;
                if message["id"] == 2 {
                    accepted = message.clone();
                }
                completed |= message["id"] == 2 && message.get("error").is_some();
                diagnostics_done |= message["method"] == "cpa/inference/diagnostics/completed";
                if message["method"] == "cpa/inference/upstream"
                    && message["params"]["kind"] != "body"
                {
                    transport_logs.push(message["params"].clone());
                }
                if completed && diagnostics_done {
                    break Ok::<_, anyhow::Error>(accepted);
                }
            }
        }
    })
    .await??;
    let calls: Vec<_> = upstream
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| {
            (
                r.url.path().to_owned(),
                r.headers.get("authorization").is_some(),
            )
        })
        .collect();
    assert_eq!(
        accepted["error"]["data"]["httpStatus"],
        json!(401),
        "{accepted}; calls={calls:?}"
    );
    assert_eq!(accepted["error"]["data"]["body"], "original-401-body");
    assert!(accepted["error"]["data"].get("nativeError").is_some());
    let upstream_requests = upstream.received_requests().await.unwrap();
    let inference: Vec<_> = upstream_requests
        .iter()
        .filter(|r| r.url.path() == "/v1/responses")
        .collect();
    assert_eq!(inference.len(), 1);
    assert_eq!(transport_logs.len(), inference.len() * 2);
    use sha2::Digest;
    for (logs, actual) in transport_logs.chunks_exact(/*chunk_size*/ 2).zip(inference) {
        let token = actual.headers["authorization"]
            .to_str()?
            .strip_prefix("Bearer ")
            .unwrap();
        assert_eq!(
            (
                logs[0]["kind"].clone(),
                logs[0]["accessTokenSha256"].clone(),
                logs[1]["kind"].clone(),
                logs[1]["statusCode"].clone()
            ),
            (
                json!("request"),
                json!(format!("{:x}", sha2::Sha256::digest(token.as_bytes()))),
                json!("response"),
                json!(if token == "fake-refreshed" { 200 } else { 401 })
            )
        );
        if token != "fake-refreshed" {
            assert!(
                logs[1]["body"].is_null(),
                "error bodies use separate native diagnostic frames"
            );
        }
    }
    let saved: Value = serde_json::from_slice(&std::fs::read(home.path().join("shared.json"))?)?;
    assert_eq!(saved["access_token"], json!("fake-cpa-access"));
    assert_eq!(saved["refresh_token"], json!("refresh-token"));
    assert_eq!(saved["unknown_field"], json!({"keep":true}));
    assert!(!home.path().join("auth.json").exists());
    Ok(())
}

#[tokio::test]
async fn cpa_fresh_placeholder_oauth_immediate_inference_and_disable() -> Result<()> {
    let upstream = MockServer::start().await;
    let home = TempDir::new()?;
    prepare_account(&upstream, &home)?;
    let file = home.path().join("shared.json");
    let initial: Value = serde_json::from_slice(&std::fs::read(&file)?)?;
    let id_token = initial["id_token"].clone();
    let mut credential =
        json!({"type":"codex","codex_cli":{"enabled":true},"unknown_field":{"keep":true}});
    std::fs::write(&file, serde_json::to_vec(&credential)?)?;
    let _runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    send(
        &mut ws,
        json!({"id":10,"method":"cpa/credential/reload","params":{}}),
    )
    .await?;
    let caps = receive(&mut ws, "id", json!(10)).await?;
    assert_eq!(caps["result"]["protocolVersion"], json!(5));
    assert_eq!(caps["result"]["manualOAuth"], json!(true));
    send(&mut ws, json!({"id":11,"method":"thread/list","params":{}})).await?;
    assert_eq!(
        receive(&mut ws, "id", json!(11)).await?["error"]["code"],
        json!(-32601)
    );
    send(
        &mut ws,
        json!({"id":12,"method":"cpa/auth/login/start","params":{"credentialId":"shared.json"}}),
    )
    .await?;
    let login = receive(&mut ws, "id", json!(12)).await?["result"].clone();
    ws.close(/*msg*/ None).await?;
    ws = connect(&home).await?;
    let auth_url = url::Url::parse(login["authUrl"].as_str().unwrap())?;
    let query: std::collections::HashMap<_, _> = auth_url.query_pairs().into_owned().collect();
    assert_eq!(
        (&query["response_type"], &query["code_challenge_method"]),
        (&"code".to_string(), &"S256".to_string())
    );
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(/*s*/ 200).set_body_json(
            json!({"id_token":id_token,"access_token":"logged-in","refresh_token":"new-refresh"}),
        ))
        .expect(/*r*/ 1)
        .mount(&upstream)
        .await;
    send(
        &mut ws,
        json!({"id":13,"method":"cpa/auth/login/status","params":{"loginId":login["loginId"]}}),
    )
    .await?;
    assert_eq!(
        receive(&mut ws, "id", json!(13)).await?["result"],
        json!({"status":"pending","error":null})
    );
    send(&mut ws,json!({"id":14,"method":"cpa/auth/login/callback","params":{"loginId":login["loginId"],"redirectUrl":format!("{}?code=test-code&state={}",query["redirect_uri"],query["state"])}})).await?;
    assert_eq!(
        receive(&mut ws, "id", json!(14)).await?["result"],
        json!({"status":"completed","error":null})
    );
    let saved: Value = serde_json::from_slice(&std::fs::read(&file)?)?;
    assert_eq!(
        (
            saved["access_token"].clone(),
            saved["unknown_field"].clone(),
            saved["codex_cli"].clone()
        ),
        (
            json!("logged-in"),
            credential["unknown_field"].clone(),
            credential["codex_cli"].clone()
        )
    );
    assert!(!home.path().join("auth.json").exists());
    let calls = upstream.received_requests().await.unwrap();
    let exchange = calls
        .iter()
        .find(|r| r.url.path() == "/oauth/token")
        .unwrap();
    let form: std::collections::HashMap<_, _> = url::form_urlencoded::parse(&exchange.body)
        .into_owned()
        .collect();
    use base64::Engine;
    use sha2::Digest;
    assert_eq!(
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(sha2::Sha256::digest(form["code_verifier"].as_bytes())),
        query["code_challenge"]
    );
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(/*s*/ 200).set_body_raw(
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"logged-in\"}}\n\n",
            "text/event-stream",
        ))
        .expect(/*r*/ 1)
        .mount(&upstream)
        .await;
    send(
        &mut ws,
        start(json!({"model":"mock-model","stream":true,"input":[]})),
    )
    .await?;
    let accepted = receive(&mut ws, "id", json!(2)).await?;
    assert_eq!(accepted["result"]["statusCode"], json!(200), "{accepted}");
    receive(&mut ws, "method", json!("cpa/inference/completed")).await?;
    credential = saved;
    credential["codex_cli"]["enabled"] = json!(false);
    std::fs::write(&file, serde_json::to_vec(&credential)?)?;
    send(
        &mut ws,
        json!({"id":15,"method":"cpa/credential/reload","params":{}}),
    )
    .await?;
    let caps = receive(&mut ws, "id", json!(15)).await?;
    assert_eq!(caps["result"]["protocolVersion"], json!(5));
    send(
        &mut ws,
        start(json!({"model":"mock-model","stream":true,"input":[]})),
    )
    .await?;
    assert_eq!(
        receive(&mut ws, "id", json!(2)).await?["error"]["data"],
        json!({"httpStatus":503})
    );
    Ok(())
}

#[tokio::test]
async fn cpa_tcp_path_and_manual_callback_binding() -> Result<()> {
    let upstream = MockServer::start().await;
    let home = TempDir::new()?;
    let _runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    let port = std::fs::read_to_string(home.path().join("port"))?;
    let stream = TcpStream::connect(format!("127.0.0.1:{port}")).await?;
    let err = client_async(format!("ws://127.0.0.1:{port}/"), stream)
        .await
        .unwrap_err();
    let tokio_tungstenite::tungstenite::Error::Http(response) = err else {
        anyhow::bail!("unexpected handshake error")
    };
    assert_eq!(response.status().as_u16(), 404);
    for wrong_url in [false, true] {
        send(
            &mut ws,
            json!({"id":30,"method":"cpa/auth/login/start","params":{"credentialId":"shared.json"}}),
        )
        .await?;
        let login = receive(&mut ws, "id", json!(30)).await?["result"].clone();
        ws.close(/*msg*/ None).await?;
        ws = connect(&home).await?;
        let redirect = if wrong_url {
            format!(
                "{}/steal?code=c&state={}",
                upstream.uri(),
                login["state"].as_str().unwrap()
            )
        } else {
            "http://127.0.0.1:1455/auth/callback?code=c&state=wrong".into()
        };
        send(&mut ws,json!({"id":31,"method":"cpa/auth/login/callback","params":{"loginId":login["loginId"],"redirectUrl":redirect}})).await?;
        assert_eq!(
            receive(&mut ws, "id", json!(31)).await?["result"],
            json!({"status":"error","error":"OAuth login failed"})
        );
    }
    assert!(
        upstream
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| !matches!(r.url.path(), "/steal" | "/oauth/token"))
    );
    Ok(())
}

#[tokio::test]
async fn cpa_file_watcher_stops_runtime_and_restarts_same_credential() -> Result<()> {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(/*s*/ 200).set_delay(Duration::from_secs(/*secs*/ 120)))
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    let _runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    send(
        &mut ws,
        start(json!({"model":"mock-model","stream":true,"input":[]})),
    )
    .await?;
    timeout(Duration::from_secs(/*secs*/ 30), async {
        loop {
            if upstream
                .received_requests()
                .await
                .unwrap()
                .iter()
                .any(|r| r.url.path() == "/v1/responses")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(/*millis*/ 20)).await;
        }
    })
    .await?;
    let file = home.path().join("shared.json");
    let mut value: Value = serde_json::from_slice(&std::fs::read(&file)?)?;
    value["codex_cli"]["enabled"] = json!(false);
    std::fs::write(&file, serde_json::to_vec(&value)?)?;
    let response = receive(&mut ws, "id", json!(2)).await?;
    assert_eq!(response["error"]["data"]["httpStatus"], json!(503));
    value["codex_cli"]["enabled"] = json!(true);
    value["account_id"] = json!("next-account");
    std::fs::write(&file, serde_json::to_vec(&value)?)?;
    send(
        &mut ws,
        json!({"id":32,"method":"cpa/credential/reload","params":{}}),
    )
    .await?;
    assert_eq!(
        receive(&mut ws, "id", json!(32)).await?["result"]["protocolVersion"],
        json!(5)
    );
    Ok(())
}

#[tokio::test]
async fn cpa_nested_unicode_ids_have_independent_persistent_runtimes() -> Result<()> {
    let upstream = MockServer::start().await;
    let home = TempDir::new()?;
    let _runtime = server(&upstream, &home).await?;
    let id = "Nested/原账号 A.JSON";
    std::fs::create_dir(home.path().join("Nested"))?;
    let file = home.path().join(id);
    std::fs::write(
        &file,
        serde_json::to_vec(&json!({"type":"codex","codex_cli":{"enabled":true}}))?,
    )?;
    let private_home = home.path().join("state").join(id);
    std::fs::create_dir_all(&private_home)?;
    std::fs::copy(
        home.path().join("config.toml"),
        private_home.join("config.toml"),
    )?;
    std::fs::write(private_home.join("retained"), b"private account state")?;
    let mut ws = connect(&home).await?;
    let mut logins = Vec::new();
    for credential in ["shared.json", id] {
        send(
            &mut ws,
            json!({"id":40,"method":"cpa/auth/login/start","params":{"credentialId":credential}}),
        )
        .await?;
        let response = receive(&mut ws, "id", json!(40)).await?;
        assert!(response.get("error").is_none(), "{response}");
        logins.push(response["result"]["loginId"].clone());
    }
    assert_ne!(logins[0], logins[1]);
    std::fs::write(
        &file,
        serde_json::to_vec(&json!({"type":"codex","disabled":true,"codex_cli":{"enabled":true}}))?,
    )?;
    send(
        &mut ws,
        json!({"id":41,"method":"cpa/credential/reload","params":{"credentialId":id}}),
    )
    .await?;
    assert_eq!(
        receive(&mut ws, "id", json!(41)).await?["result"]["protocolVersion"],
        json!(5)
    );
    send(
        &mut ws,
        json!({"id":42,"method":"cpa/auth/login/status","params":{"loginId":logins[0]}}),
    )
    .await?;
    assert_eq!(
        receive(&mut ws, "id", json!(42)).await?["result"],
        json!({"status":"pending","error":null})
    );
    send(
        &mut ws,
        json!({"id":43,"method":"cpa/auth/login/status","params":{"loginId":logins[1]}}),
    )
    .await?;
    assert!(
        receive(&mut ws, "id", json!(43))
            .await?
            .get("error")
            .is_some()
    );
    std::fs::write(
        &file,
        serde_json::to_vec(&json!({"type":"codex","codex_cli":{"enabled":true}}))?,
    )?;
    send(
        &mut ws,
        json!({"id":44,"method":"cpa/credential/reload","params":{"credentialId":id}}),
    )
    .await?;
    assert_eq!(
        receive(&mut ws, "id", json!(44)).await?["result"]["protocolVersion"],
        json!(5)
    );
    assert_eq!(
        std::fs::read(private_home.join("retained"))?,
        b"private account state"
    );
    assert!(!private_home.join("auth.json").exists());
    assert!(file.exists());
    Ok(())
}
