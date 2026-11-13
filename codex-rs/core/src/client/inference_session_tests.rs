use super::InferenceIdentity;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn session_identity_groups_threads_and_ignores_cache_affinity() {
    for (request, expected) in [
        (
            json!({"thread_id":"thread", "session_id":"session", "prompt_cache_key":"cache"}),
            Some("session"),
        ),
        (
            json!({"session_id":"session", "prompt_cache_key":"cache"}),
            Some("session"),
        ),
        (json!({"prompt_cache_key":"cache"}), None),
    ] {
        let identity = InferenceIdentity::from_request(request.as_object().unwrap());
        assert_eq!(identity.session_key(), expected);
    }
}

#[test]
fn source_identity_merges_top_level_flat_and_nested_metadata() {
    let request = json!({"session_id":"top-session", "turn_id":null, "prompt_cache_key":"",
        "client_metadata":{"session_id":"flat-session", "thread_id":"flat-thread",
            "x-codex-turn-metadata":json!({"thread_id":"nested-thread", "turn_id":"nested-turn",
                "prompt_cache_key":"nested-cache", "parent_turn_id":"parent", "root_turn_id":"root"}).to_string()}});
    assert_eq!(
        InferenceIdentity::from_request(request.as_object().unwrap()),
        InferenceIdentity {
            session_id: Some("top-session".into()),
            thread_id: Some("flat-thread".into()),
            turn_id: Some("nested-turn".into()),
            prompt_cache_key: Some("nested-cache".into()),
            parent_turn_id: Some("parent".into()),
            root_turn_id: Some("root".into()),
            ..Default::default()
        }
    );
}

#[test]
fn hierarchy_maps_root_children_and_parent_links_within_the_same_scope() {
    let root = InferenceIdentity {
        session_id: Some("s".into()),
        thread_id: Some("s".into()),
        ..Default::default()
    };
    let root_map = root.map("credential/caller").unwrap();
    assert_eq!(root_map.session_id, root_map.thread_id);
    let child = InferenceIdentity {
        thread_id: Some("child".into()),
        parent_thread_id: Some("s".into()),
        ..root.clone()
    };
    let child_map = child.map("credential/caller").unwrap();
    assert_eq!(child_map.session_id, root_map.session_id);
    assert_ne!(child_map.thread_id, root_map.thread_id);
    assert_eq!(child_map.parent_thread_id, Some(root_map.thread_id));
    let grandchild = InferenceIdentity {
        thread_id: Some("grandchild".into()),
        parent_thread_id: Some("child".into()),
        ..root.clone()
    };
    let mapped = grandchild.map("credential/caller").unwrap();
    assert_eq!(mapped.parent_thread_id, Some(child_map.thread_id));
    assert_eq!(mapped.session_id, root_map.session_id);
    assert_ne!(
        root.map("other-credential/caller").unwrap().session_id,
        root_map.session_id
    );
    assert_ne!(
        InferenceIdentity {
            session_id: Some("other-session".into()),
            ..child
        }
        .map("credential/caller")
        .unwrap()
        .session_id,
        root_map.session_id
    );
}

#[test]
fn title_classification_requires_explicit_metadata() {
    let prompt_only =
        serde_json::json!({"input":"generate a thread_title", "instructions":"title_generation"});
    assert!(!InferenceIdentity::from_request(prompt_only.as_object().unwrap()).is_title());
    for request in [
        serde_json::json!({"thread_source":"thread_title"}),
        serde_json::json!({"client_metadata":{"thread_source":"thread_title"}}),
        serde_json::json!({"client_metadata":{"x-codex-turn-metadata":"{\"thread_source\":\"thread_title\"}"}}),
        serde_json::json!({"request_kind":"title_generation"}),
    ] {
        assert!(InferenceIdentity::from_request(request.as_object().unwrap()).is_title());
    }
}

#[tokio::test]
async fn inference_uses_the_worker_runtime_provider_without_another_owner() {
    use std::sync::Arc;
    let mut config = crate::config::test_config().await;
    config
        .features
        .enable(codex_features::Feature::RuntimeAgentInstances)
        .unwrap();
    let auth = codex_login::AuthManager::from_auth_for_testing(
        codex_login::CodexAuth::from_api_key("worker-fixture"),
    );
    let runtime = crate::build_runtime_auth_manager(
        &config,
        auth.clone(),
        "00000000-0000-0000-0000-000000000001",
    )
    .unwrap();
    let provider = codex_model_provider::ProviderAuthContext {
        credentials: auth.clone(),
        runtime_auth: Some((runtime.clone(), config.runtime_auth_mode)),
    }
    .model_provider(config.model_provider.clone());
    let source = InferenceIdentity {
        session_id: Some("source-session".into()),
        thread_id: Some("source-child".into()),
        turn_id: Some("source-turn".into()),
        ..Default::default()
    };
    let session = super::InferenceSession::new_with_provider(
        &config,
        auth,
        "00000000-0000-0000-0000-000000000001",
        "worker-fixture",
        &source,
        Some(provider.clone()),
    )
    .unwrap();
    assert!(Arc::ptr_eq(&session.client.state.provider, &provider));
    assert!(Arc::ptr_eq(
        &session.client.state.provider.runtime_auth().unwrap().0,
        &runtime
    ));
}

#[tokio::test]
async fn caller_prompt_cache_ttl_uses_the_native_prompt_field() {
    let config = crate::config::test_config().await;
    for (value, expected) in [
        ("30m", crate::PromptCacheTtl::ThirtyMinutes),
        ("12h", crate::PromptCacheTtl::TwelveHours),
    ] {
        let request = json!({"input":[],"prompt_cache_options":{"ttl":value}});
        let parsed = crate::client::inference::InferenceRequest::parse(
            &config,
            request.as_object().unwrap(),
        )
        .unwrap();
        assert_eq!(parsed.prompt.prompt_cache_ttl, Some(expected));
    }
    let invalid = json!({"input":[],"prompt_cache_options":{"ttl":"unknown"}});
    assert!(
        crate::client::inference::InferenceRequest::parse(&config, invalid.as_object().unwrap())
            .is_err()
    );
}
