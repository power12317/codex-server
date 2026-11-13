//! Verifies the supported destinations and environment boundaries for Realtime auth.

#![cfg(test)]

use super::*;
use codex_model_provider::validate_runtime_environment;
use pretty_assertions::assert_eq;

#[test]
fn realtime_auth_accepts_only_secure_first_party_destinations() {
    for url in [
        "https://api.openai.com/v1",
        "https://api.openai.org/v1",
        "wss://api.openai.com/v1",
        "wss://chatgpt.com/backend-api",
    ] {
        assert!(is_openai_realtime_url(url).unwrap(), "{url}");
    }
    for url in [
        "ws://api.openai.com/v1",
        "http://api.openai.com/v1",
        "wss://unknown.openai.com/v1",
        "wss://api.openai.com:8443/v1",
        "wss://user:secret@api.openai.com/v1",
    ] {
        assert!(is_openai_realtime_url(url).is_err(), "{url}");
    }
    assert!(!is_openai_realtime_url("wss://voice.example.com").unwrap());
}

#[test]
fn realtime_rejects_a_different_registration_environment() {
    for (chatgpt_base_url, expected_staging) in [
        ("https://chatgpt.com/backend-api", false),
        ("https://chatgpt-staging.com/backend-api", true),
    ] {
        let runtime = RuntimeAuthManager::for_codex(
            AuthManager::from_auth_for_testing(codex_login::CodexAuth::from_api_key("test-key")),
            chatgpt_base_url,
            &uuid::Uuid::nil().to_string(),
        );
        for (destination, staging) in [
            ("wss://api.openai.com/v1", false),
            ("https://chatgpt.com/backend-api", false),
            ("wss://api.openai-staging.com/v1", true),
            ("https://api.openai.org/v1", true),
            ("https://workspace.chatgpt-staging.com/backend-api", true),
            ("https://chatgpt-staging.com/backend-api", true),
        ] {
            assert_eq!(
                validate_runtime_environment(&runtime, Some(destination)).is_ok(),
                staging == expected_staging,
                "{chatgpt_base_url} -> {destination}",
            );
        }
    }
}

#[tokio::test]
async fn worker_runtime_is_default_off_and_shared_only_within_its_owner() {
    let mut config = crate::config::test_config().await;
    let credentials_a =
        AuthManager::from_auth_for_testing(codex_login::CodexAuth::from_api_key("worker-a"));
    let credentials_b =
        AuthManager::from_auth_for_testing(codex_login::CodexAuth::from_api_key("worker-b"));
    let slot_a = RuntimeAuthSlot::default();
    let slot_b = RuntimeAuthSlot::default();
    assert!(!config.features.enabled(Feature::RuntimeAgentInstances));
    assert!(
        runtime_for_config(
            &slot_a,
            &config,
            credentials_a.clone(),
            "00000000-0000-0000-0000-000000000001"
        )
        .is_none()
    );
    assert!(slot_a.get().is_none());
    config
        .features
        .enable(Feature::RuntimeAgentInstances)
        .unwrap();
    let a = runtime_for_config(
        &slot_a,
        &config,
        credentials_a.clone(),
        "00000000-0000-0000-0000-000000000001",
    )
    .unwrap();
    let reused = runtime_for_config(
        &slot_a,
        &config,
        credentials_a.clone(),
        "00000000-0000-0000-0000-000000000001",
    )
    .unwrap();
    let b = runtime_for_config(
        &slot_b,
        &config,
        credentials_b.clone(),
        "00000000-0000-0000-0000-000000000002",
    )
    .unwrap();
    assert!(Arc::ptr_eq(&a, &reused));
    assert!(!Arc::ptr_eq(&a, &b));
    assert!(a.uses_credentials(&credentials_a));
    assert!(!a.uses_credentials(&credentials_b));
    assert!(b.uses_credentials(&credentials_b));
    config
        .features
        .disable(Feature::RuntimeAgentInstances)
        .unwrap();
    assert!(
        runtime_for_config(
            &slot_a,
            &config,
            credentials_a,
            "00000000-0000-0000-0000-000000000001"
        )
        .is_none()
    );
}

#[tokio::test]
async fn cpa_file_workers_register_independent_keys_and_track_source_changes() {
    use base64::Engine;
    use codex_runtime_agent_instance::{
        AgentHostAuthorization, RuntimeRegistrationMetadata, RuntimeRole,
    };
    use serde_json::{Value, json};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/agent/register"))
        .respond_with(|request: &wiremock::Request| {
            ResponseTemplate::new(200).set_body_json(json!({
                "runtime_agent_instance_id": request.headers["idempotency-key"].to_str().unwrap(),
                "agent_host_id": "host-fixture",
                "actor": {"kind":"chatgpt","user_id":"user-fixture","account_id":"account-fixture"}
            }))
        })
        .mount(&server)
        .await;
    let config = crate::config::test_config().await;
    let directory = tempfile::tempdir().unwrap();
    let write = |name: &str, token: &str, enabled: bool| {
        let claims = json!({"session_id":token,"https://api.openai.com/auth":{"user_id":"user-fixture","chatgpt_account_id":"account-fixture"}});
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string());
        std::fs::write(directory.path().join(name), json!({
            "type":"codex", "id_token":format!("header.{encoded}.signature"),
            "access_token":token,"refresh_token":"refresh-fixture","account_id":"account-fixture",
            "last_refresh":chrono::Utc::now(), "codex_cli":{"enabled":enabled}
        }).to_string()).unwrap();
    };
    write("a.json", "token-a", true);
    write("b.json", "token-b", true);
    let mut owners = Vec::new();
    for (index, name) in ["a.json", "b.json"].into_iter().enumerate() {
        let credentials = AuthManager::shared_with_cpa_credentials(
            &config,
            codex_login::CpaCredentialFile {
                path: directory.path().join(name),
                credential_id: name.into(),
            },
        )
        .await;
        let runtime = Arc::new(RuntimeAuthManager::new(
            credentials.clone(),
            AgentHostAuthorization::InstallationId {
                installation_id: uuid::Uuid::from_u128(index as u128 + 1),
            },
            RuntimeRegistrationMetadata {
                harness: Some("codex".into()),
                harness_version: Some(env!("CARGO_PKG_VERSION").into()),
                role: RuntimeRole::Orchestrator,
            },
            format!("{}/agent/register", server.uri()).parse().unwrap(),
        ));
        owners.push((credentials, runtime));
    }
    let (a, same_a) = tokio::join!(owners[0].1.auth(), owners[0].1.auth());
    let a = a.unwrap();
    let same_a = same_a.unwrap();
    let b = owners[1].1.auth().await.unwrap();
    assert!(Arc::ptr_eq(a.runtime(), same_a.runtime()));
    assert!(!Arc::ptr_eq(a.runtime(), b.runtime()));
    let registrations = server.received_requests().await.unwrap();
    assert_eq!(registrations.len(), 2);
    let first: Value = registrations[0].body_json().unwrap();
    let second: Value = registrations[1].body_json().unwrap();
    assert_ne!(first["public_key"], second["public_key"]);
    assert_ne!(
        first["agent_host_auth"]["installation_id"],
        second["agent_host_auth"]["installation_id"]
    );
    write("a.json", "token-a-rotated", true);
    owners[0].0.reload().await;
    let rotated = owners[0].1.auth().await.unwrap();
    assert_ne!(a.runtime().id(), rotated.runtime().id());
    assert_eq!(
        owners[1].1.auth().await.unwrap().runtime().id(),
        b.runtime().id()
    );
    write("a.json", "token-a-rotated", false);
    owners[0].0.reload().await;
    assert!(owners[0].1.auth().await.is_err());
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
    assert!(!directory.path().join("auth.json").exists());
}
