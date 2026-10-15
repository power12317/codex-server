//! CPA TCP contract tests use a shared fake credential file and a mock upstream.
#[path = "cpa_bridge/continuity_tests.rs"]
mod continuity_tests;
#[path = "cpa_bridge/hierarchy_tests.rs"]
mod hierarchy_tests;
#[path = "cpa_bridge/home_tests.rs"]
mod home_tests;
#[path = "cpa_bridge/hosted_tools_tests.rs"]
mod hosted_tools_tests;
#[path = "cpa_bridge/image_stream_tests.rs"]
mod image_stream_tests;
#[path = "cpa_bridge/images_tests.rs"]
mod images_tests;
#[path = "cpa_bridge/oailb_tests.rs"]
mod oailb_tests;
#[path = "cpa_bridge/parser_tests.rs"]
mod parser_tests;
#[path = "cpa_bridge/ticket_tests.rs"]
mod ticket_tests;
#[path = "cpa_bridge/time_context_tests.rs"]
mod time_context_tests;

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
        .with_provider_config("requires_openai_auth = true\nstream_idle_timeout_ms = 10")
        .with_root_config(&format!(
            "chatgpt_base_url = {:?}\ncli_auth_credentials_store = \"file\"",
            upstream.uri()
        ))
        .write(home.path())?;
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
async fn cpa_lossless_single_inference_and_identity_contract() -> Result<()> {
    let upstream = MockServer::start().await;
    let home = TempDir::new()?;
    let marker = home.path().join("tool-must-not-run");
    let request = json!({"model":"mock-model","stream":true,"instructions":"keep exactly", "input":[], "tools":[{"type":"function","name":"shell","parameters":{"type":"object"}}], "tool_choice":{"type":"function","name":"shell"}, "client_metadata":{"x-codex-installation-id":"caller-installation","x-codex-turn-metadata":"caller-turn-metadata","x-codex-window-id":"caller-window","session_id":"caller-session","thread_id":"caller-thread","turn_id":"caller-turn-id","business_tag":"keep-me"}, "headers":{"authorization":"caller-secret"}, "authorization":"caller-secret", "future_extension":{"untouched":true}});
    let events = [
        json!({"type":"response.created","response":{"id":"resp"}}),
        json!({"type":"future.event","payload":{"untouched":true,"text":"你好"}}),
        json!({"type":"response.output_item.done","item":{"type":"function_call","call_id":"call","name":"shell","arguments":format!("{{\"command\":\"touch {}\"}}",marker.display())}}),
        json!({"type":"response.completed","response":{"id":"resp","output":[],"usage":{"input_tokens":1,"output_tokens":2,"total_tokens":3,"future_usage":7}}}),
    ];
    use base64::Engine;
    let node_claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(r#"{"host":"chat.gateway.unified-42.api.openai.com"}"#);
    let node_cookie = format!("__oailb=e30.{node_claims}.signature; Path=/");
    let mut body = events
        .iter()
        .enumerate()
        .map(|(index, e)| {
            format!(
                ": keepalive\nid: evt-{index}\nevent: {}\ndata: {e}\n\n",
                e["type"].as_str().unwrap()
            )
        })
        .collect::<String>();
    body.push_str(": opaque trailer\ndata: deliberately-not-json\n\n");
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(
            ResponseTemplate::new(/*s*/ 200)
                .insert_header("content-type", "text/event-stream")
                .insert_header("set-cookie", node_cookie.clone())
                .set_body_raw(body.clone(), "text/event-stream"),
        )
        .expect(/*r*/ 1)
        .mount(&upstream)
        .await;
    let _runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    send(
        &mut ws,
        json!({"id":1,"method":"cpa/capabilities/read","params":{}}),
    )
    .await?;
    let caps = receive(&mut ws, "id", json!(1)).await?;
    assert_eq!(caps["result"]["protocolVersion"], json!(3));
    assert_eq!(
        caps["result"]["runtimeVersion"],
        json!(env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(caps["result"]["upstreamBodyLogs"], json!(true));
    let mut wrong = start(request.clone());
    wrong["params"]["credentialId"] = json!("wrong");
    send(&mut ws, wrong).await?;
    assert_eq!(
        receive(&mut ws, "id", json!(2)).await?["error"]["data"],
        json!({"httpStatus":503})
    );
    send(&mut ws, start(request.clone())).await?;
    let logged_request =
        receive(&mut ws, "method", json!("cpa/inference/upstream")).await?["params"].clone();
    let logged_response =
        receive(&mut ws, "method", json!("cpa/inference/upstream")).await?["params"].clone();
    assert_eq!(logged_request["kind"], json!("request"));
    let prepared: Value = serde_json::from_str(logged_request["body"].as_str().unwrap())?;
    assert!(prepared.get("instructions").is_none());
    assert_eq!(prepared["input"][0]["role"], json!("developer"));
    assert_eq!(
        prepared["input"][0]["content"][0]["text"],
        request["instructions"]
    );
    assert_eq!(prepared["tool_choice"], request["tool_choice"]);
    assert_eq!(prepared["future_extension"], request["future_extension"]);
    assert_eq!(prepared["store"], json!(false));
    assert_eq!(prepared["include"], json!(["reasoning.encrypted_content"]));
    assert!(prepared["client_metadata"].is_object());
    assert_eq!(
        prepared["client_metadata"]["business_tag"],
        json!("keep-me")
    );
    assert_ne!(
        prepared["client_metadata"]["x-codex-installation-id"],
        json!("caller-installation")
    );
    assert_ne!(
        prepared["client_metadata"]["x-codex-window-id"],
        json!("caller-window")
    );
    assert_ne!(
        prepared["client_metadata"]["session_id"],
        json!("caller-session")
    );
    assert_ne!(
        prepared["client_metadata"]["thread_id"],
        json!("caller-thread")
    );
    assert_eq!(
        prepared["client_metadata"]["turn_id"],
        json!("caller-turn-id")
    );
    let turn: Value = serde_json::from_str(
        prepared["client_metadata"]["x-codex-turn-metadata"]
            .as_str()
            .unwrap(),
    )?;
    assert_eq!(turn["turn_id"], json!("caller-turn-id"));
    assert!(prepared.get("headers").is_none());
    assert!(prepared.get("authorization").is_none());
    assert_eq!(
        logged_request["headers"]["authorization"],
        json!(["Bearer fake-cpa-access"])
    );
    assert_eq!(logged_response["kind"], json!("response"));
    assert_eq!(
        logged_response["headers"]["set-cookie"],
        json!([node_cookie])
    );
    assert_eq!(logged_response["statusCode"], json!(200));
    assert_eq!(logged_response["oaiLbNode"], json!("unified-42"));
    let mut accepted = Value::Null;
    let mut wire_body = Vec::new();
    loop {
        let message = timeout(Duration::from_secs(/*secs*/ 30), ws.next())
            .await?
            .unwrap()?;
        let Message::Text(text) = message else {
            continue;
        };
        let message: Value = serde_json::from_str(&text)?;
        if message["id"] == 2 {
            accepted = message;
            continue;
        }
        match message["method"].as_str() {
            Some("cpa/inference/body") => {
                wire_body.extend(
                    base64::engine::general_purpose::STANDARD
                        .decode(message["params"]["bodyBase64"].as_str().unwrap())?,
                );
            }
            Some("cpa/inference/completed") => {
                assert_eq!(message["params"], json!({"requestId":"r"}));
                break;
            }
            _ => anyhow::bail!("unexpected inference message: {message}"),
        }
    }
    assert_eq!(accepted["result"]["requestId"], "r");
    assert_eq!(accepted["result"]["statusCode"], 200);
    assert_eq!(accepted["result"]["headers"], logged_response["headers"]);
    assert_eq!(wire_body, body.as_bytes());
    assert!(!marker.exists());
    let requests = upstream.received_requests().await.unwrap();
    let inference: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path() == "/v1/responses")
        .collect();
    assert_eq!(inference.len(), 1);
    assert_eq!(inference[0].headers["originator"], "codex-tui");
    let user_agent = inference[0].headers["user-agent"].to_str()?;
    assert!(user_agent.starts_with(concat!("codex-tui/", env!("CARGO_PKG_VERSION"), " (")));
    assert!(user_agent.ends_with(concat!("(codex-tui; ", env!("CARGO_PKG_VERSION"), ")")));
    assert_eq!(inference[0].body_json::<Value>()?, prepared);
    assert_eq!(
        (
            logged_request["method"].clone(),
            logged_request["url"].clone()
        ),
        (
            json!(inference[0].method.as_str()),
            json!(format!("{}{}", upstream.uri(), inference[0].url.path()))
        )
    );
    for (name, values) in logged_request["headers"].as_object().unwrap() {
        let actual: Vec<_> = inference[0]
            .headers
            .get_all(name.as_str())
            .iter()
            .map(|v| v.to_str().unwrap())
            .collect();
        assert_eq!(values, &json!(actual), "header {name}");
    }
    use sha2::Digest;
    assert_eq!(
        logged_request["accessTokenSha256"],
        json!(format!("{:x}", sha2::Sha256::digest(b"fake-cpa-access")))
    );
    assert_eq!(
        inference[0].headers.get("authorization").unwrap(),
        "Bearer fake-cpa-access"
    );
    assert_eq!(
        inference[0].headers.get("chatgpt-account-id").unwrap(),
        "account"
    );
    ws.close(/*msg*/ None).await?;
    let mut ws = connect(&home).await?;
    send(
        &mut ws,
        json!({"id":3,"method":"cpa/capabilities/read","params":{}}),
    )
    .await?;
    assert_eq!(
        receive(&mut ws, "id", json!(3)).await?["result"],
        caps["result"]
    );
    Ok(())
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
    assert_eq!(replies[&2]["error"]["data"], json!({"httpStatus":499}));
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
        json!(3)
    );
    Ok(())
}

#[tokio::test]
async fn cpa_managed_401_recovery_refreshes_shared_storage() -> Result<()> {
    let upstream = MockServer::start().await;
    app_test_support::mount_workspace_routing(&upstream).await;
    let home = TempDir::new()?;
    let _runtime = server(&upstream, &home).await?;
    let auth: Value = serde_json::from_slice(&std::fs::read(home.path().join("shared.json"))?)?;
    Mock::given(method("POST")).and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(/*s*/ 200).set_body_json(json!({"access_token":"fake-refreshed", "refresh_token":"fake-refresh-next", "id_token":auth["id_token"]}))).expect(/*r*/ 1).mount(&upstream).await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .and(wiremock::matchers::header(
            "authorization",
            "Bearer fake-cpa-access",
        ))
        .respond_with(ResponseTemplate::new(/*s*/ 401).set_body_string("private-error-never-echo"))
        .mount(&upstream)
        .await;
    Mock::given(method("POST")).and(path("/v1/responses")).and(wiremock::matchers::header("authorization","Bearer fake-refreshed"))
        .respond_with(ResponseTemplate::new(/*s*/ 200).insert_header("content-type","text/event-stream").set_body_string("data: {\"type\":\"response.completed\",\"response\":{\"id\":\"refreshed\"}}\n\n")).expect(/*r*/ 1).mount(&upstream).await;
    let mut ws = connect(&home).await?;
    send(
        &mut ws,
        start(json!({"model":"mock-model","stream":true,"input":[]})),
    )
    .await?;
    let mut transport_logs = Vec::new();
    let accepted = timeout(Duration::from_secs(/*secs*/ 30), async {
        loop {
            if let Some(Ok(Message::Text(text))) = ws.next().await {
                let message: Value = serde_json::from_str(&text)?;
                if message["id"] == 2 {
                    break Ok::<_, anyhow::Error>(message);
                }
                if message["method"] == "cpa/inference/upstream"
                    && message["params"]["kind"] != "body"
                {
                    transport_logs.push(message["params"].clone());
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
        accepted["result"]["statusCode"],
        json!(200),
        "{accepted}; calls={calls:?}"
    );
    receive(&mut ws, "method", json!("cpa/inference/completed")).await?;
    let upstream_requests = upstream.received_requests().await.unwrap();
    let inference: Vec<_> = upstream_requests
        .iter()
        .filter(|r| r.url.path() == "/v1/responses")
        .collect();
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
            assert_eq!(logs[1]["body"], json!("private-error-never-echo"));
        }
    }
    let saved: Value = serde_json::from_slice(&std::fs::read(home.path().join("shared.json"))?)?;
    assert_eq!(saved["access_token"], json!("fake-refreshed"));
    assert_eq!(saved["refresh_token"], json!("fake-refresh-next"));
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
    assert_eq!(caps["result"]["protocolVersion"], json!(3));
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
        .respond_with(
            ResponseTemplate::new(/*s*/ 200)
                .set_body_raw("data: opaque-function-call\n\n", "text/event-stream"),
        )
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
    assert_eq!(caps["result"]["protocolVersion"], json!(3));
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
async fn cpa_failures_preserve_upstream_errors_without_replaying_inference() -> Result<()> {
    for status in [503, 200] {
        let upstream = MockServer::start().await;
        let body = if status == 200 {
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"truncated\"}}\n\n"
        } else {
            "private error detail"
        };
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(ResponseTemplate::new(status).set_body_raw(body, "text/event-stream"))
            .expect(/*r*/ 1)
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
        let request_log =
            receive(&mut ws, "method", json!("cpa/inference/upstream")).await?["params"].clone();
        let response_log =
            receive(&mut ws, "method", json!("cpa/inference/upstream")).await?["params"].clone();
        assert_eq!(request_log["kind"], json!("request"));
        assert_eq!(response_log["statusCode"], json!(status));
        let accepted = receive(&mut ws, "id", json!(2)).await?;
        if status == 503 {
            assert_eq!(response_log["body"], json!(body));
            assert_eq!(
                accepted["error"],
                json!({"code":-32000,"data":{"httpStatus":503,"body":body,"headers":response_log["headers"]},"message":"Upstream inference failed"})
            );
        } else {
            assert_eq!(accepted["result"]["statusCode"], json!(200));
            receive(&mut ws, "method", json!("cpa/inference/body")).await?;
            receive(&mut ws, "method", json!("cpa/inference/completed")).await?;
        }
    }
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
        json!(3)
    );
    Ok(())
}

#[tokio::test]
async fn cpa_same_account_concurrency_and_token_reload_keep_oauth_runtime() -> Result<()> {
    let upstream = MockServer::start().await;
    let wire = ": keepalive\r\ndata: {\"type\":\"function_call\",\"arguments\":\"opaque\"}\r\n\r\n";
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(
            ResponseTemplate::new(/*s*/ 200)
                .set_delay(Duration::from_millis(/*millis*/ 150))
                .set_body_raw(wire, "text/event-stream"),
        )
        .expect(/*r*/ 2)
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    let _runtime = server(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    send(
        &mut ws,
        json!({"id":10,"method":"cpa/auth/login/start","params":{"credentialId":"shared.json"}}),
    )
    .await?;
    let login = receive(&mut ws, "id", json!(10)).await?["result"]["loginId"].clone();
    let file = home.path().join("shared.json");
    let mut credential: Value = serde_json::from_slice(&std::fs::read(&file)?)?;
    credential["access_token"] = json!("externally-updated");
    std::fs::write(&file, serde_json::to_vec(&credential)?)?;
    send(
        &mut ws,
        json!({"id":11,"method":"cpa/credential/reload","params":{"credentialId":"shared.json"}}),
    )
    .await?;
    receive(&mut ws, "id", json!(11)).await?;
    send(
        &mut ws,
        json!({"id":12,"method":"cpa/auth/login/status","params":{"loginId":login}}),
    )
    .await?;
    assert_eq!(
        receive(&mut ws, "id", json!(12)).await?["result"],
        json!({"status":"pending","error":null})
    );
    for (id, request_id) in [(20, "one"), (21, "two")] {
        let mut request = start(json!({"model":"mock-model","stream":true,"input":"Hello"}));
        request["id"] = json!(id);
        request["params"]["requestId"] = json!(request_id);
        send(&mut ws, request).await?;
    }
    use base64::Engine;
    let mut bodies = std::collections::BTreeMap::<String, Vec<u8>>::new();
    let mut accepted = std::collections::BTreeSet::new();
    let mut completed = std::collections::BTreeSet::new();
    while completed.len() != 2 {
        let Some(Ok(Message::Text(text))) =
            timeout(Duration::from_secs(/*secs*/ 30), ws.next()).await?
        else {
            anyhow::bail!("socket closed")
        };
        let message: Value = serde_json::from_str(&text)?;
        assert!(message.get("error").is_none(), "{message}");
        if let Some(result) = message.get("result") {
            assert_eq!(result["statusCode"], json!(200));
            accepted.insert(result["requestId"].as_str().unwrap().to_owned());
        } else if message["method"] == "cpa/inference/body" {
            let id = message["params"]["requestId"].as_str().unwrap();
            assert!(accepted.contains(id));
            bodies.entry(id.into()).or_default().extend(
                base64::engine::general_purpose::STANDARD
                    .decode(message["params"]["bodyBase64"].as_str().unwrap())?,
            );
        } else if message["method"] == "cpa/inference/completed" {
            completed.insert(message["params"]["requestId"].as_str().unwrap().to_owned());
        } else {
            assert_eq!(message["method"], json!("cpa/inference/upstream"));
        }
    }
    assert_eq!(
        bodies,
        std::collections::BTreeMap::from([
            ("one".into(), wire.as_bytes().to_vec()),
            ("two".into(), wire.as_bytes().to_vec())
        ])
    );
    let requests = upstream.received_requests().await.unwrap();
    for request in requests.iter().filter(|r| r.url.path() == "/v1/responses") {
        assert_eq!(
            request.headers["authorization"],
            "Bearer externally-updated"
        );
        let body = request.body_json::<Value>()?;
        assert_eq!(
            body["input"][0]["content"],
            json!([{"type":"input_text","text":"Hello"}])
        );
    }
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
        json!(3)
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
        json!(3)
    );
    assert_eq!(
        std::fs::read(private_home.join("retained"))?,
        b"private account state"
    );
    assert!(!private_home.join("auth.json").exists());
    assert!(file.exists());
    Ok(())
}
