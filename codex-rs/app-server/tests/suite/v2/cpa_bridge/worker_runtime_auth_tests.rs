//! Exercise credential-worker runtime ownership through real Responses and Images requests.
use super::*;
use base64::Engine;
use core_test_support::runtime_auth::PROBE_SERVER;
use core_test_support::runtime_auth::run_harness_through_proxy;
use pretty_assertions::assert_eq;
use std::collections::BTreeSet;

const TEST: &str = "suite::v2::cpa_bridge::worker_runtime_auth_tests::cpa_workers_share_runtime_across_requests_but_never_across_credentials";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpa_workers_share_runtime_across_requests_but_never_across_credentials() -> Result<()> {
    if std::env::var(PROBE_SERVER).is_ok() {
        let upstream = MockServer::start().await;
        let home = TempDir::new()?;
        prepare_account(&upstream, &home)?;
        let mut config: toml::Value =
            toml::from_str(&std::fs::read_to_string(home.path().join("config.toml"))?)?;
        config["chatgpt_base_url"] = toml::Value::String("https://chatgpt.com/backend-api".into());
        config["model_providers"]["mock_provider"]["base_url"] =
            toml::Value::String("https://api.openai.com/v1".into());
        let features = config
            .as_table_mut()
            .unwrap()
            .entry("features")
            .or_insert_with(|| toml::Value::Table(Default::default()));
        features.as_table_mut().unwrap().insert(
            "runtime_agent_instances".into(),
            toml::Value::try_from(json!({"enabled":true,"mode":"dual_header"}))?,
        );
        let mut second: Value =
            serde_json::from_slice(&std::fs::read(home.path().join("shared.json"))?)?;
        second["access_token"] = json!("worker-b-token");
        second["account_id"] = json!("account-b");
        second["id_token"] = json!(encode_id_token(
            &ChatGptIdTokenClaims::new()
                .chatgpt_account_id("account-b")
                .chatgpt_user_id("fixture-user")
                .email("fixture-b@example.com")
        )?);
        std::fs::write(home.path().join("b.json"), serde_json::to_vec(&second)?)?;
        for credential in ["shared.json", "b.json"] {
            let state = home.path().join("state").join(credential);
            std::fs::create_dir_all(&state)?;
            std::fs::write(state.join("config.toml"), toml::to_string(&config)?)?;
        }
        let _runtime = launch(&upstream, &home).await?;
        let mut socket = connect(&home).await?;
        let mut id = 20_u64;
        for credential in ["shared.json", "b.json"] {
            for operation in ["responses", "responses", "images/generations"] {
                id += 1;
                let request_id = format!("runtime-{id}");
                let mut request = start(if operation == "responses" {
                    json!({"model":"mock-model","input":"runtime worker fixture","stream":true})
                } else {
                    json!({"model":"gpt-image-1","prompt":"runtime image fixture"})
                });
                request["id"] = json!(id);
                request["params"]["requestId"] = json!(request_id);
                request["params"]["credentialId"] = json!(credential);
                request["params"]["operation"] = json!(operation);
                if operation != "responses" {
                    request["params"]["sourceFormat"] = json!("openai-image");
                    request["params"]["imageApi"] = json!("images");
                }
                send(&mut socket, request).await?;
                timeout(Duration::from_secs(30), async {
                    while let Some(message) = socket.next().await {
                        let Message::Text(text) = message? else {
                            continue;
                        };
                        let message: Value = serde_json::from_str(&text)?;
                        if message["id"] == id {
                            anyhow::ensure!(message.get("error").is_none(), "{message}");
                        }
                        if message["params"]["requestId"] != request_id {
                            continue;
                        }
                        anyhow::ensure!(message["method"] != "cpa/inference/error", "{message}");
                        if message["method"] == "cpa/inference/completed" {
                            return Ok::<_, anyhow::Error>(());
                        }
                    }
                    anyhow::bail!("worker disconnected before completion")
                })
                .await??;
            }
        }
        for credential in ["shared.json", "b.json"] {
            assert!(
                !home
                    .path()
                    .join("state")
                    .join(credential)
                    .join("auth.json")
                    .exists()
            );
        }
        return Ok(());
    }

    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/agent/register")).respond_with(|request: &wiremock::Request| {
        let body: Value = request.body_json().unwrap();
        let second = body["actor_auth"]["credential"] == "worker-b-token";
        ResponseTemplate::new(200).set_body_json(json!({
            "runtime_agent_instance_id":if second {"runtime-b"} else {"runtime-a"},
            "agent_host_id":"test-host",
            "actor":{"kind":"chatgpt","user_id":"fixture-user","account_id":if second {"account-b"} else {"account"}}
        }))
    }).expect(2).mount(&server).await;
    Mock::given(method("POST")).and(path("/v1/responses")).respond_with(
        ResponseTemplate::new(200).set_body_raw("data: {\"type\":\"response.completed\",\"response\":{\"id\":\"runtime-response\",\"output\":[]}}\n\n", "text/event-stream")
    ).expect(4).mount(&server).await;
    Mock::given(method("POST"))
        .and(path("/v1/images/generations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"created":1,"data":[{"b64_json":"aW1hZ2U=","generation_id":"runtime-image"}]}),
        ))
        .expect(2)
        .mount(&server)
        .await;
    run_harness_through_proxy(&server, TEST, None).await?;
    let requests = server.received_requests().await.unwrap();
    let registrations: Vec<Value> = requests
        .iter()
        .filter(|r| r.url.path() == "/agent/register")
        .map(|r| r.body_json().unwrap())
        .collect();
    assert_eq!(registrations.len(), 2);
    assert_ne!(
        registrations[0]["public_key"],
        registrations[1]["public_key"]
    );
    assert_ne!(
        registrations[0]["agent_host_auth"]["installation_id"],
        registrations[1]["agent_host_auth"]["installation_id"]
    );
    let mut runtimes = BTreeSet::new();
    for request in requests
        .iter()
        .filter(|r| matches!(r.url.path(), "/v1/responses" | "/v1/images/generations"))
    {
        let assertion = request
            .headers
            .get("x-openai-agent-assertion")
            .expect("worker runtime assertion")
            .to_str()?;
        let claims: Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(assertion.split('.').nth(2).unwrap())?,
        )?;
        let account = request
            .headers
            .get("chatgpt-account-id")
            .unwrap()
            .to_str()?;
        assert_eq!(
            claims["runtime_agent_instance"],
            if account == "account-b" {
                "runtime-b"
            } else {
                "runtime-a"
            }
        );
        assert_eq!(
            request.headers.get("authorization").unwrap().to_str()?,
            if account == "account-b" {
                "Bearer worker-b-token"
            } else {
                "Bearer fake-cpa-access"
            }
        );
        runtimes.insert(
            claims["runtime_agent_instance"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    assert_eq!(runtimes.len(), 2);
    Ok(())
}
