use super::*;
use pretty_assertions::assert_eq;

fn key(turn: Option<&str>) -> Key {
    Key {
        scope: "rpc".into(),
        session: Some("session".into()),
        thread: Some("thread-a".into()),
        turn: turn.map(str::to_owned),
        provenance: Provenance::default(),
    }
}

#[test]
fn synthetic_turn_has_fixed_lifetime_and_inflight_references_survive_expiry() {
    let mut cache = Cache::default();
    let now = Instant::now();
    let first = cache.get_or_insert(key(None), now, || Ok("first")).unwrap();
    let active = cache
        .get_or_insert(key(None), now + TURN_TTL - Duration::from_secs(1), || {
            Ok("wrong")
        })
        .unwrap();
    assert!(Arc::ptr_eq(&first, &active));
    let second = cache
        .get_or_insert(key(None), now + TURN_TTL, || Ok("second"))
        .unwrap();
    assert_eq!(
        (*first, *active, *second, cache.turns.len()),
        ("first", "first", "second", 1)
    );
}

#[test]
fn explicit_turns_are_isolated_and_expire_after_idle_time() {
    let mut cache = Cache::default();
    let now = Instant::now();
    let first = cache
        .get_or_insert(key(Some("real")), now, || Ok(1))
        .unwrap();
    cache.get_or_insert(key(None), now, || Ok(2)).unwrap();
    let reused = cache
        .get_or_insert(key(Some("real")), now + TURN_TTL / 2, || Ok(3))
        .unwrap();
    assert!(Arc::ptr_eq(&first, &reused));
    let reused = cache
        .get_or_insert(key(Some("real")), now + TURN_TTL, || Ok(4))
        .unwrap();
    assert!(Arc::ptr_eq(&first, &reused));
    assert_eq!(cache.turns.len(), 1);
    let fresh = cache
        .get_or_insert(key(Some("real")), now + TURN_TTL * 2, || Ok(5))
        .unwrap();
    assert_eq!(*fresh, 5);
}

#[test]
fn capacity_rejects_new_contexts_without_evicting_fresh_state() {
    let mut cache = Cache::default();
    let now = Instant::now();
    for index in 0..MAX_CONTEXTS {
        cache
            .get_or_insert(key(Some(&index.to_string())), now, || Ok(index))
            .unwrap();
    }
    assert!(
        cache
            .get_or_insert(key(None), now, || Ok(0))
            .unwrap_err()
            .is::<CapacityExceeded>()
    );
    assert_eq!(
        *cache.get_or_insert(key(Some("0")), now, || Ok(42)).unwrap(),
        0
    );
    assert_eq!(
        *cache
            .get_or_insert(key(None), now + TURN_TTL, || Ok(42))
            .unwrap(),
        42
    );
}

#[test]
fn cpa_windows_follow_explicit_changes_and_compaction_without_advancing_for_turns_or_triggers() {
    let mut source = InferenceIdentity::default();
    let request = serde_json::json!({"input":[]});
    let mut window = Window::new(&source, request.as_object().unwrap());
    let first = window.id;
    source.turn_id = Some("next-turn".into());
    window.observe(
        &source,
        serde_json::json!({"input":[{"type":"compaction_trigger"}]})
            .as_object()
            .unwrap(),
    );
    assert_eq!((window.number, window.id), (0, first));
    source.window_id = Some("source-window-2".into());
    source.context_window_id = Some("source-context-2".into());
    let compacted = serde_json::json!({"input":[{"type":"compaction","id":"cmp_1","encrypted_content":"opaque"}]});
    window.observe(&source, compacted.as_object().unwrap());
    assert_eq!(window.number, 1);
    assert_ne!(window.id, first);
    let second = window.id;
    window.observe(&source, compacted.as_object().unwrap());
    assert_eq!((window.number, window.id), (1, second));
    source.context_window_id = Some("source-context-3".into());
    window.observe(&source, compacted.as_object().unwrap());
    assert_eq!(window.number, 2);
}
