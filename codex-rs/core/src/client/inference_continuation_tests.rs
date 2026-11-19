use super::*;
use serde_json::json;

fn turn() -> ManagedTurn {
    let generation = Uuid::new_v4();
    ManagedTurn {
        control: InferenceSessionInstructions {
            scope: "caller/session".into(),
            baseline_digest: "a".repeat(64),
            append: true,
            http_only: false,
            previous_response_id: None,
        },
        prior: Some(ManagedProof {
            generation,
            scope: "caller/session".into(),
            digest: "a".repeat(64),
            context: "credential-A/session/thread/window".into(),
            model: "model".into(),
            response_id: "same-literal-id".into(),
            epoch: 3,
        }),
        context: "credential-A/session/thread/window".into(),
        previous_epoch: 3,
        attempted: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        error: None,
    }
}

#[test]
fn cpa_managed_parent_requires_the_actual_owner_socket_context_scope_and_epoch() {
    for change in 0..7 {
        let mut t = turn();
        let generation = t.prior.as_ref().unwrap().generation;
        match change {
            0 => t.control.scope = "other-CPA/session".into(),
            1 => t.control.baseline_digest.clear(),
            2 => t.context = "credential-B/session/thread/window".into(),
            3 => t.context = "credential-A/session/child/window".into(),
            4 => t.context = "credential-A/session/thread/new-window".into(),
            5 => t.previous_epoch += 1,
            6 => t.control.previous_response_id = Some("other-id".into()),
            _ => unreachable!(),
        }
        assert_eq!(
            t.parent(Some(generation), "model"),
            Err(InferenceContinuationError::PreviousResponseNotFound)
        );
    }
    let t = turn();
    assert_eq!(
        t.parent(None, "model"),
        Err(InferenceContinuationError::PreviousResponseNotFound)
    );
    assert_eq!(
        t.parent(Some(Uuid::new_v4()), "model"),
        Err(InferenceContinuationError::PreviousResponseNotFound)
    );
    assert_eq!(
        t.parent(Some(t.prior.as_ref().unwrap().generation), "other-model"),
        Err(InferenceContinuationError::PreviousResponseNotFound)
    );
}

#[test]
fn cpa_same_literal_response_id_does_not_cross_workers_or_restarted_sockets() {
    let worker_a = turn();
    let worker_b = turn();
    assert_eq!(
        worker_a.prior.as_ref().unwrap().response_id,
        worker_b.prior.as_ref().unwrap().response_id
    );
    assert_eq!(
        worker_b.parent(Some(worker_a.prior.as_ref().unwrap().generation), "model"),
        Err(InferenceContinuationError::PreviousResponseNotFound)
    );
    let mut restarted = turn();
    restarted.prior = None;
    assert_eq!(
        restarted.parent(Some(Uuid::new_v4()), "model"),
        Err(InferenceContinuationError::PreviousResponseNotFound)
    );
}

#[test]
fn cpa_append_uses_the_verbatim_last_proven_upstream_id() {
    let mut t = turn();
    let generation = t.prior.as_ref().unwrap().generation;
    assert_eq!(
        t.parent(Some(generation), "model").unwrap(),
        "same-literal-id"
    );
    t.control.previous_response_id = Some("same-literal-id".into());
    assert_eq!(
        t.parent(Some(generation), "model").unwrap(),
        "same-literal-id"
    );
}

#[test]
fn cpa_receipt_requires_created_then_matching_successful_completed_and_parent() {
    for invalid in 0..6 {
        let mut receipt = Receipt {
            parent: Some("p".into()),
            ..Default::default()
        };
        if invalid != 0 {
            receipt.observe(&json!({"type":"response.created","response":{"id":"r","previous_response_id":"p"}}).to_string());
        }
        if invalid == 1 {
            receipt.observe(&json!({"type":"response.created","response":{"id":"r"}}).to_string());
        }
        receipt.observe(&json!({"type":"response.completed","response":{"id":if invalid == 2 {"wrong"} else {"r"},"previous_response_id":if invalid == 3 {"wrong"} else {"p"},"status":if invalid == 4 {"failed"} else {"completed"}}}).to_string());
        if invalid == 5 {
            receipt
                .observe(&json!({"type":"response.completed","response":{"id":"r"}}).to_string());
        }
        assert_eq!(receipt.success(), None);
    }
    let mut receipt = Receipt::default();
    receipt.observe(r#"{"type":"response.created","response":{"id":"r"}}"#);
    receipt.observe(r#"{"type":"response.completed","response":{"id":"r","status":"completed"}}"#);
    assert_eq!(receipt.success().as_deref(), Some("r"));
}

#[test]
fn cpa_full_wire_prefix_must_match_the_cpa_digest() {
    use sha2::Digest;
    let mut t = turn();
    t.control.append = false;
    t.control.baseline_digest = format!("{:x}", sha2::Sha256::digest(b"baseline"));
    let item = serde_json::from_value(json!({"type":"message","role":"developer","content":[{"type":"input_text","text":"baseline"}]})).unwrap();
    t.check_full_prefix(&[item]);
    assert!(!t.control.baseline_digest.is_empty());
    let item = serde_json::from_value(json!({"type":"message","role":"developer","content":[{"type":"input_text","text":"changed"}]})).unwrap();
    t.check_full_prefix(&[item]);
    assert!(t.control.baseline_digest.is_empty());
}
