//! Capture the actual native sender; the supervisor and clients cannot select identity.
use super::*;
use base64::Engine;
use core_test_support::responses;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn expected_ua(system: &str) -> String {
    let os = if system == "windows" {
        "Windows 10.0.19044"
    } else {
        "Mac OS 15.3.1"
    };
    format!(
        "Codex Desktop/{} ({os}; x86_64) dumb (Codex Desktop; 26.1023.51421)",
        env!("CARGO_PKG_VERSION")
    )
}
fn check_attestation(raw: &str, system: &str) {
    assert!(raw.starts_with("{\"v\":1,\"s\":0,\"t\":\"v1."), "{raw}");
    let value: Value = serde_json::from_str(raw).unwrap();
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value["t"].as_str().unwrap().strip_prefix("v1.").unwrap())
        .unwrap();
    let mut expected = b"\xa3\x6aerror_code".to_vec();
    expected.push(if system == "windows" { 1 } else { 2 });
    expected.extend_from_slice(b"\x69bundle_id\x70com.openai.codex\x61f");
    assert!(bytes.starts_with(&expected), "{bytes:?}");
    assert!(!raw.contains("caller-attestation"));
}
fn credential(home: &TempDir, id: &str, system: &str, account: &str) -> Result<()> {
    let mut value: Value =
        serde_json::from_slice(&std::fs::read(home.path().join("shared.json"))?)?;
    value["account_id"] = json!(account);
    value["id_token"] = json!(encode_id_token(
        &ChatGptIdTokenClaims::new()
            .chatgpt_account_id(account)
            .chatgpt_user_id(account)
            .email("synthetic@example.test")
    )?);
    if !system.is_empty() {
        value["codex_client_system"] = json!(system);
    }
    std::fs::write(home.path().join(id), serde_json::to_vec(&value)?)?;
    let dir = home.path().join("state").join(id);
    std::fs::create_dir_all(&dir)?;
    std::fs::copy(home.path().join("config.toml"), dir.join("config.toml"))?;
    Ok(())
}

fn provider_overrides(home: &TempDir, env_headers: bool) -> Result<()> {
    let path = home.path().join("config.toml");
    let mut config: toml::Value = toml::from_str(&std::fs::read_to_string(&path)?)?;
    let provider = config["model_providers"]["mock_provider"]
        .as_table_mut()
        .unwrap();
    provider.insert(
        "http_headers".into(),
        toml::Value::try_from(json!({
            "UsEr-AgEnT":"codex-tui/legacy-config",
            "OrIgInAtOr":"codex-tui",
            "VeRsIoN":"0.1.0",
            "X-OaI-AtTeStAtIoN":"provider-proof",
            "X-Desktop-Custom":"preserve-static"
        }))?,
    );
    if env_headers {
        provider.insert(
            "env_http_headers".into(),
            toml::Value::try_from(json!({
                "USER-agent":"CODEX_DESKTOP_TEST_LEGACY_HEADER",
                "ORIGINATOR":"CODEX_DESKTOP_TEST_LEGACY_HEADER",
                "VERSION":"CODEX_DESKTOP_TEST_LEGACY_HEADER",
                "X-OAI-ATTESTATION":"CODEX_DESKTOP_TEST_LEGACY_HEADER",
                "x-desktop-custom":"CODEX_DESKTOP_TEST_LEGACY_HEADER"
            }))?,
        );
    }
    std::fs::write(path, toml::to_string(&config)?)?;
    Ok(())
}

async fn launch_desktop(upstream: &MockServer, home: &TempDir) -> Result<TestAppServer> {
    launch_with_env(
        upstream,
        home,
        &[
            ("TZ", Some("Asia/Tokyo")),
            (
                "CODEX_DESKTOP_TEST_LEGACY_HEADER",
                Some("codex-tui/legacy-env"),
            ),
        ],
    )
    .await
}

fn custom_value(env_headers: bool) -> &'static str {
    if env_headers {
        "codex-tui/legacy-env"
    } else {
        "preserve-static"
    }
}
fn request(id: &str, number: u64, compact: bool) -> Value {
    let mut body = json!({"model":"mock-model","stream":true,"session_id":"same-caller-session","thread_id":"same-caller-session", "input":[{"role":"user","content":"fixture"}],"tools":[{"type":"function","name":"Read","strict":false,"parameters":{"$schema":"http://json-schema.org/draft-07/schema#","type":"object","properties":{"x":{"type":"string","minLength":2}}}}],"headers":{"user-agent":"caller-cli","originator":"caller-cli","x-oai-attestation":"caller-attestation"},"x-oai-attestation":"caller-attestation"});
    if compact {
        body["input"]
            .as_array_mut()
            .unwrap()
            .push(json!({"type":"compaction_trigger"}));
    }
    let mut req = start(body);
    req["id"] = json!(number);
    req["params"]["requestId"] = json!(format!("desktop-{number}"));
    req["params"]["credentialId"] = json!(id);
    req
}
async fn sample(ws: &mut Socket, request: Value, success: bool) -> Result<()> {
    let id = request["id"].clone();
    let request_id = request["params"]["requestId"].clone();
    send(ws, request).await?;
    timeout(Duration::from_secs(30), async {
        let mut saw_completed = false;
        while let Some(message) = ws.next().await {
            let Message::Text(text) = message? else {
                continue;
            };
            let value: Value = serde_json::from_str(&text)?;
            if value.get("id") == Some(&id) && value.get("error").is_some() {
                anyhow::ensure!(!success, "{value}");
                return Ok(());
            }
            if value["params"]["requestId"] != request_id {
                continue;
            }
            match value["method"].as_str() {
                Some("cpa/inference/error") => {
                    anyhow::ensure!(!success, "{value}");
                    return Ok(());
                }
                Some("cpa/inference/event") => {
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(value["params"]["bodyBase64"].as_str().unwrap())?;
                    saw_completed |= String::from_utf8(bytes)?.contains("response.completed");
                }
                Some("cpa/inference/completed") => {
                    anyhow::ensure!(success && saw_completed, "native completion expected");
                    return Ok(());
                }
                _ => {}
            }
        }
        anyhow::bail!("socket closed before terminal event")
    })
    .await?
}
fn capture(label: &str, value: &Value) -> Result<()> {
    if let Some(dir) = std::env::var_os("CODEX_DESKTOP_EVIDENCE_DIR") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            dir.join(format!("{label}.json")),
            serde_json::to_vec_pretty(value)?,
        )?;
    }
    Ok(())
}

#[test_case::test_case(false; "static_headers")]
#[test_case::test_case(true; "env_headers")]
#[tokio::test]
async fn cpa_desktop_http_accounts_are_isolated_and_compaction_keeps_identity(
    env_headers: bool,
) -> Result<()> {
    let upstream = MockServer::start().await;
    let home = TempDir::new()?;
    Mock::given(method("POST")).and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("data: {\"type\":\"response.completed\",\"response\":{\"id\":\"desktop\",\"output\":[]}}\n\n","text/event-stream"))
        .expect(4).mount(&upstream).await;
    prepare_account(&upstream, &home)?;
    provider_overrides(&home, env_headers)?;
    credential(&home, "windows.json", "windows", "windows-fixture")?;
    credential(&home, "shared.json", "", "mac-fixture")?;
    let _runtime = launch_desktop(&upstream, &home).await?;
    let mut windows = connect(&home).await?;
    let mut mac = connect(&home).await?;
    let (a, b) = tokio::join!(
        sample(&mut windows, request("windows.json", 801, false), true),
        sample(&mut mac, request("shared.json", 802, false), true)
    );
    a?;
    b?;
    let (a, b) = tokio::join!(
        sample(&mut windows, request("windows.json", 803, true), true),
        sample(&mut mac, request("shared.json", 804, true), true)
    );
    a?;
    b?;
    let requests = upstream.received_requests().await.unwrap();
    let mut profiles = std::collections::HashMap::new();
    for req in requests.iter().filter(|r| r.url.path() == "/v1/responses") {
        let account = req.headers["chatgpt-account-id"].to_str()?;
        let system = if account == "windows-fixture" {
            "windows"
        } else {
            "mac"
        };
        assert_eq!(req.headers["originator"], "Codex Desktop");
        assert_eq!(req.headers["user-agent"], expected_ua(system));
        assert_eq!(req.headers["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(req.headers["x-desktop-custom"], custom_value(env_headers));
        let attestation = req.headers["x-oai-attestation"].to_str()?;
        check_attestation(attestation, system);
        if let Some(previous) = profiles.insert(system, attestation.to_owned()) {
            assert_eq!(previous, attestation);
        }
        let body: Value = req.body_json()?;
        assert!(body.to_string().contains("draft-07/schema#"));
        assert!(!body.to_string().contains("caller-attestation"));
        capture(
            &format!(
                "http-{system}-{env_headers}-{}",
                if body["input"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|i| i["type"] == "compaction_trigger")
                {
                    "compact"
                } else {
                    "sample"
                }
            ),
            &json!({"userAgent":req.headers["user-agent"].to_str()?,"originator":req.headers["originator"].to_str()?,"version":req.headers["version"].to_str()?,"custom":req.headers["x-desktop-custom"].to_str()?,"attestation":attestation,"request":body}),
        )?;
    }
    assert_eq!(profiles.len(), 2);
    assert_ne!(profiles["windows"], profiles["mac"]);
    for req in requests.iter().filter(|r| r.url.path() != "/v1/responses") {
        assert!(!req.headers.contains_key("x-oai-attestation"));
    }
    Ok(())
}

#[test_case::test_case("windows", false; "windows_static_headers")]
#[test_case::test_case("windows", true; "windows_env_headers")]
#[test_case::test_case("mac", false; "mac_x64_static_headers")]
#[test_case::test_case("mac", true; "mac_x64_env_headers")]
#[tokio::test]
async fn cpa_desktop_ws_reconnects_preserve_profile_and_do_not_retry(
    system: &str,
    env_headers: bool,
) -> Result<()> {
    let events = |id: &str| {
        vec![
            responses::ev_response_created(id),
            responses::ev_completed(id),
        ]
    };
    let failed = vec![
        json!({"type":"response.failed","response":{"error":{"code":"server_error","message":"close fixture"}}}),
    ];
    let upstream = MockServer::start().await;
    let model = responses::start_websocket_server(vec![
        vec![events("desktop-first"), failed],
        vec![events("desktop-after-error")],
    ])
    .await;
    let home = TempDir::new()?;
    prepare_account(&upstream, &home)?;
    provider_overrides(&home, env_headers)?;
    credential(&home, "shared.json", system, "desktop-fixture")?;
    let cfg = home.path().join("state/shared.json/config.toml");
    let mut config: toml::Value = toml::from_str(&std::fs::read_to_string(&cfg)?)?;
    config["model_providers"]["mock_provider"]["base_url"] =
        toml::Value::String(format!("{}/v1", model.uri().replace("ws://", "http://")));
    config["model_providers"]["mock_provider"]["supports_websockets"] = toml::Value::Boolean(true);
    std::fs::write(cfg, toml::to_string(&config)?)?;
    let _runtime = launch_desktop(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    sample(&mut ws, request("shared.json", 811, false), true).await?;
    sample(&mut ws, request("shared.json", 812, true), false).await?;
    assert_eq!(
        model.handshakes().len(),
        1,
        "failed invocation cannot retry itself"
    );
    sample(&mut ws, request("shared.json", 813, false), true).await?;
    let handshakes = model.handshakes();
    assert_eq!(handshakes.len(), 2);
    let mut proof = None;
    for (index, h) in handshakes.iter().enumerate() {
        assert_eq!(h.header("originator"), Some("Codex Desktop".into()));
        assert_eq!(h.header("user-agent"), Some(expected_ua(system)));
        assert_eq!(h.header("version"), Some(env!("CARGO_PKG_VERSION").into()));
        assert_eq!(
            h.header("x-desktop-custom"),
            Some(custom_value(env_headers).into())
        );
        let token = h.header("x-oai-attestation").expect("worker proof");
        check_attestation(&token, system);
        if let Some(previous) = proof.replace(token.clone()) {
            assert_eq!(token, previous);
        }
        capture(
            &format!("ws-{system}-{env_headers}-{index}"),
            &json!({"userAgent":h.header("user-agent"),"originator":h.header("originator"),"version":h.header("version"),"custom":h.header("x-desktop-custom"),"attestation":token}),
        )?;
    }
    assert_eq!(model.connections().iter().map(Vec::len).sum::<usize>(), 3);
    assert!(
        !upstream
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|r| r.url.path() == "/v1/responses"),
        "no alternate HTTP sender"
    );
    Ok(())
}

#[tokio::test]
async fn cpa_desktop_http_error_does_not_cross_request_retry_boundary() -> Result<()> {
    let upstream = MockServer::start().await;
    let home = TempDir::new()?;
    let count = Arc::new(AtomicUsize::new(0));
    let sent = count.clone();
    Mock::given(method("POST")).and(path("/v1/responses"))
        .respond_with(move |_: &wiremock::Request| {
            if sent.fetch_add(1,Ordering::SeqCst)==0 { ResponseTemplate::new(500).set_body_json(json!({"error":{"message":"fixture failure"}})) }
            else { ResponseTemplate::new(200).set_body_raw("data: {\"type\":\"response.completed\",\"response\":{\"id\":\"recovered\",\"output\":[]}}\n\n","text/event-stream") }
        }).expect(2).mount(&upstream).await;
    prepare_account(&upstream, &home)?;
    credential(&home, "shared.json", "windows", "desktop-error-fixture")?;
    let _runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    sample(&mut ws, request("shared.json", 821, false), false).await?;
    assert_eq!(count.load(Ordering::SeqCst), 1);
    sample(&mut ws, request("shared.json", 822, false), true).await?;
    let received = upstream.received_requests().await.unwrap();
    let calls = received
        .iter()
        .filter(|r| r.url.path() == "/v1/responses")
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[0].headers["x-oai-attestation"],
        calls[1].headers["x-oai-attestation"]
    );
    Ok(())
}

#[test_case::test_case("windows", false; "windows_static_headers")]
#[test_case::test_case("windows", true; "windows_env_headers")]
#[test_case::test_case("mac", false; "mac_x64_static_headers")]
#[test_case::test_case("mac", true; "mac_x64_env_headers")]
fn cpa_desktop_catalog_and_usage_do_not_inherit_configured_identity_or_proof(
    system: &str,
    env_headers: bool,
) -> Result<()> {
    let status = std::process::Command::new(std::env::current_exe()?)
        .env("CODEX_DESKTOP_CATALOG_SYSTEM", system)
        .env(
            "CODEX_DESKTOP_CATALOG_ENV_HEADERS",
            if env_headers { "1" } else { "0" },
        )
        .env("CODEX_DESKTOP_TEST_LEGACY_HEADER", "codex-tui/legacy-env")
        .env("TZ", "Asia/Tokyo")
        .env_remove("OPENAI_API_KEY")
        .args([
            "--exact",
            "suite::v2::cpa_bridge::desktop_tests::cpa_desktop_catalog_usage_child",
            "--ignored",
            "--nocapture",
        ])
        .status()?;
    anyhow::ensure!(
        status.success(),
        "isolated native catalog/usage client test failed"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "isolated managed native catalog/usage clients"]
async fn cpa_desktop_catalog_usage_child() -> Result<()> {
    let system = std::env::var("CODEX_DESKTOP_CATALOG_SYSTEM")?;
    let env_headers = std::env::var("CODEX_DESKTOP_CATALOG_ENV_HEADERS")? == "1";
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"models":[]})))
        .mount(&upstream)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/codex/usage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "account_id":"desktop-catalog", "user_id":"desktop-catalog", "plan_type":"plus",
            "rate_limit":{"allowed":true,"limit_reached":false,
                "primary_window":{"used_percent":42,"limit_window_seconds":3600,
                    "reset_after_seconds":120,"reset_at":2000000000}}
        })))
        .expect(1)
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    prepare_account(&upstream, &home)?;
    provider_overrides(&home, env_headers)?;
    credential(&home, "shared.json", &system, "desktop-catalog")?;
    let file = codex_login::CpaCredentialFile {
        path: home.path().join("shared.json"),
        credential_id: "shared.json".into(),
    };
    codex_login::cpa_desktop::install_for_worker(&file)?;
    let config = core_test_support::load_default_config_for_test(&home).await;
    let auth = codex_login::AuthManager::shared_with_cpa_credentials(&config, file).await;
    // Exercise the native provider catalog and backend usage clients. The
    // worker's model-only RPC surface deliberately excludes account RPCs.
    let provider = codex_model_provider::create_model_provider(
        config.model_provider.clone(),
        Some(auth.clone()),
    );
    provider
        .models_manager_without_cache(None)
        .list_models(
            codex_models_manager::manager::RefreshStrategy::Online,
            config.http_client_factory(),
        )
        .await;
    let account = auth.auth().await.expect("synthetic CPA authentication");
    codex_backend_client::Client::from_auth(upstream.uri(), &account, config.http_client_factory())
        .get_rate_limits_with_reset_credits()
        .await?;
    let received = upstream.received_requests().await.unwrap();
    let mut evidence = Vec::new();
    for endpoint in ["/v1/models", "/api/codex/usage"] {
        let requests = received
            .iter()
            .filter(|request| request.url.path() == endpoint)
            .collect::<Vec<_>>();
        assert!(!requests.is_empty(), "actual {endpoint} request expected");
        for request in requests {
            assert_eq!(request.headers["user-agent"], expected_ua(&system));
            assert!(!request.headers.contains_key("x-oai-attestation"));
            if endpoint == "/v1/models" {
                assert_eq!(request.headers["originator"], "Codex Desktop");
                assert_eq!(request.headers["version"], env!("CARGO_PKG_VERSION"));
                assert_eq!(
                    request.headers["x-desktop-custom"],
                    custom_value(env_headers)
                );
            } else {
                // The native backend client supplies UA explicitly and has no
                // provider-header surface, originator or version by default.
                assert!(!request.headers.contains_key("originator"));
                assert!(!request.headers.contains_key("version"));
            }
            evidence.push(json!({"path":endpoint,
                "userAgent":request.headers["user-agent"].to_str()?,
                "originator":request.headers.get("originator").and_then(|v|v.to_str().ok()),
                "version":request.headers.get("version").and_then(|v|v.to_str().ok()),
                "custom":request.headers.get("x-desktop-custom").and_then(|v|v.to_str().ok()),
                "hasAttestation":false}));
        }
    }
    capture(
        &format!("catalog-usage-{system}-{env_headers}"),
        &json!(evidence),
    )?;
    Ok(())
}
