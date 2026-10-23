use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn cpa_single_send_streams_tool_ready_events_before_native_completion() {
    let (diagnostics, _blocked) = tokio::sync::mpsc::unbounded_channel();
    let (business, mut events) = tokio::sync::mpsc::unbounded_channel();
    let observer = NativeObservation::new("r".into(), diagnostics, business, false);
    for kind in [
        "response.function_call_arguments.delta",
        "response.function_call_arguments.done",
        "response.output_item.done",
        "response.completed",
    ] {
        observer.accepted_event(&json!({"type":kind}).to_string());
    }
    assert!(matches!(events.try_recv().unwrap(), Business::Ready(_)));
    for expected in [
        "response.function_call_arguments.delta",
        "response.function_call_arguments.done",
        "response.output_item.done",
    ] {
        let Business::Event(bytes) = events.try_recv().unwrap() else {
            panic!("incremental event")
        };
        assert!(String::from_utf8(bytes).unwrap().contains(expected));
    }
    assert!(events.try_recv().is_err());
    observer.finish(Ok(()));
    let Business::Event(bytes) = events.try_recv().unwrap() else {
        panic!("terminal event")
    };
    assert!(
        String::from_utf8(bytes)
            .unwrap()
            .contains("response.completed")
    );
    assert!(matches!(
        events.try_recv().unwrap(),
        Business::Finished(Ok(()))
    ));
    observer.accepted_event(r#"{"type":"response.output_text.delta","delta":"late"}"#);
    assert!(events.try_recv().is_err());
}

#[test]
fn cpa_error_preserves_native_status_headers_body_and_classification() {
    let (diagnostics, _blocked) = tokio::sync::mpsc::unbounded_channel();
    let (business, mut events) = tokio::sync::mpsc::unbounded_channel();
    let observer = NativeObservation::new("r".into(), diagnostics, business, false);
    let headers = HeaderMap::from_iter([("retry-after".parse().unwrap(), "7".parse().unwrap())]);
    observer.http_error(503, &headers, Some("original error body"));
    let mut error = super::super::failure(502, "native typed failure");
    error.data.as_mut().unwrap()["nativeError"] = json!({"server_overloaded": {}});
    observer.finish(Err(error));
    let Business::Finished(Err(error)) = events.try_recv().unwrap() else {
        panic!("original error")
    };
    let data = error.data.unwrap();
    assert_eq!(data["httpStatus"], 503);
    assert_eq!(data["body"], "original error body");
    assert_eq!(data["headers"]["retry-after"], json!(["7"]));
    assert_eq!(data["nativeError"], json!({"server_overloaded": {}}));
}

#[test]
fn native_observation_preserves_raw_ipc_and_redacts_only_the_log_copy() {
    let (diagnostics, mut notifications) = tokio::sync::mpsc::unbounded_channel();
    let (business, _events) = tokio::sync::mpsc::unbounded_channel();
    let observer = NativeObservation::new("r".into(), diagnostics, business, false);
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(br#"{"host":"chat.gateway.fixture.api.openai.com"}"#);
    let mut headers = HeaderMap::new();
    headers.insert("authorization", "Bearer fixture-secret".parse().unwrap());
    headers.insert(
        "cookie",
        format!("__oailb=e30.{payload}.sig").parse().unwrap(),
    );
    observer.request(
        "http",
        "POST",
        "https://fixture/v1/responses",
        &headers,
        b"{}",
    );
    observer.response("http", 200, &HeaderMap::new());
    for _ in 0..2 {
        let Diagnostic::Notification(notification) = notifications.try_recv().unwrap() else {
            panic!("upstream record")
        };
        let value = serde_json::to_value(notification).unwrap();
        assert_eq!(value["oaiLbNode"], "fixture");
        if value["kind"] == "request" {
            assert_eq!(
                value["headers"]["authorization"],
                json!(["Bearer fixture-secret"])
            );
            assert_eq!(
                value["headers"]["cookie"],
                json!([format!("__oailb=e30.{payload}.sig")])
            );
            let mut log = value.clone();
            redact_log_headers(&mut log);
            assert_eq!(log["headers"]["authorization"], json!(["[REDACTED]"]));
            assert_eq!(log["headers"]["cookie"], json!(["[REDACTED]"]));
        }
    }
    assert!(!observer.observes_request("POST", "https://auth.example/oauth/token"));
}
