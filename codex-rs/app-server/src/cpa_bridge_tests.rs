use super::*;
use pretty_assertions::assert_eq;

#[test]
fn completion_is_a_native_outcome_without_a_replayed_body() {
    assert_eq!(
        serde_json::to_value(CpaInferenceCompletedNotification {
            request_id: "r".into(),
        })
        .expect("completion"),
        json!({"requestId":"r"})
    );
    assert!(
        serde_json::from_value::<CpaInferenceCompletedNotification>(
            json!({"requestId":"r", "bodyBase64":""})
        )
        .is_ok()
    );
}

#[test]
fn capabilities_describe_actual_delivery_and_ticket_support() {
    assert_eq!(
        serde_json::to_value(capabilities()).expect("capabilities"),
        json!({
            "runtimeVersion": env!("CARGO_PKG_VERSION"),
            "upstreamRevision": UPSTREAM,
            "manualOAuth": true,
            "upstreamLogs": true,
            "upstreamBodyLogs": true,
            "executionMode": "inference-only",
            "rawBody": true,
            "businessNotification": "cpa/inference/event",
            "completionSemantics": "native-terminal",
            "diagnosticsCompleted": true,
            "responsesWebsocket": true,
            "responseIdentityMapping": true,
            "ticketSnapshots": false,
            "operations": ["responses", "images/generations", "images/edits"]
        })
    );
}

#[test]
fn required_ticket_capture_is_rejected_before_model_execution() {
    for capture in [None, Some(false), Some(true)] {
        let mut value = json!({"requestId":"r", "credentialId":"worker",
            "operation":"responses", "sourceFormat":"openai-response",
            "sessionId":"scope", "request":{"model":"fixture","input":"hello"}});
        if let Some(capture) = capture {
            value["captureTicketSnapshot"] = json!(capture);
        }
        let params = serde_json::from_value(value).expect("params");
        if capture == Some(true) {
            let error = validate(&params).expect_err("source tickets unsupported");
            assert!(error.message.contains("source ticket snapshots"));
            assert_eq!(error.data.expect("data")["httpStatus"], json!(400));
        } else {
            validate(&params).expect("ticket capture not requested");
        }
    }
}

#[test]
fn image_operations_require_the_declared_business_representation() {
    for operation in ["images/generations", "images/edits"] {
        for image_api in ["images", "responses"] {
            let mut value = json!({"requestId":"r", "credentialId":"worker", "operation":operation,
                "sourceFormat":"openai-image", "sessionId":"scope", "imageApi":image_api, "request":{}});
            let params: CpaInferenceStartParams =
                serde_json::from_value(value.clone()).expect("image params");
            validate(&params).expect("filtered business fields stay absent");
            value.as_object_mut().expect("object").remove("imageApi");
            assert!(validate(&serde_json::from_value(value).expect("legacy envelope")).is_err());
        }
    }
    for operation in ["responses", "responses/compact"] {
        let params = serde_json::from_value(json!({"requestId":"r", "credentialId":"worker", "operation":operation,
            "sourceFormat":"openai-image", "sessionId":"scope", "imageApi":"images", "request":{"stream":true}})).expect("params");
        assert!(validate(&params).is_err());
    }
    assert_eq!(
        capabilities().operations,
        vec!["responses", "images/generations", "images/edits"]
    );
}

#[test]
fn request_semantics_are_preserved_and_unsupported_state_is_rejected() {
    let request = json!({"model":"fixture", "stream":true, "instructions":"exact instructions", "tools":[{"type":"function","name":"shell","parameters":{"type":"object"}}], "tool_choice":{"type":"function","name":"shell"}, "client_metadata":{"x-codex-installation-id":"caller-installation","business_tag":"keep"}, "headers":{"authorization":"caller-secret"}, "extension":{"future":true}});
    let params: CpaInferenceStartParams = serde_json::from_value(json!({"requestId":"r", "credentialId":"worker", "operation":"responses", "sourceFormat":"openai-response", "sessionId":"scope", "request":request})).unwrap();
    validate(&params).unwrap();
    assert_eq!(Value::Object(params.request.clone()), request);
    for (key, value) in [
        ("previous_response_id", json!(null)),
        ("generate", json!(false)),
        ("background", json!(true)),
        ("store", json!(true)),
    ] {
        let mut changed = params.clone();
        changed.request.insert(key.into(), value);
        assert_eq!(validate(&changed).is_ok(), true);
    }
}
