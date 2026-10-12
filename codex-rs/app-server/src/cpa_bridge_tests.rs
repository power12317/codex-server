use super::*;
use pretty_assertions::assert_eq;

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
        assert_eq!(
            validate(&changed).unwrap_err().data.unwrap()["httpStatus"],
            json!(400)
        );
    }
}
