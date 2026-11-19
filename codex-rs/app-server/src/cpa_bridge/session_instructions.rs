//! Validate CPA-owned instruction controls without retaining baseline text.
use super::*;

pub(crate) fn continuation_error(status: u16, code: &str, message: &str) -> JSONRPCErrorError {
    let mut error = json!({"message":message,"type":if status == 409 {"invalid_request_error"} else {"server_error"},"code":code});
    if status == 409 {
        error["param"] = json!("previous_response_id");
    }
    JSONRPCErrorError {
        code: -32000,
        message: message.into(),
        data: Some(
            json!({"httpStatus":status,"body":json!({"error":error}).to_string(),"headers":{},"sessionInstructionsError":true}),
        ),
    }
}

pub(super) fn validate(params: &CpaInferenceStartParams, websocket: bool) -> BridgeResult<()> {
    if params
        .request
        .get("client_metadata")
        .and_then(Value::as_object)
        .is_some_and(|m| m.contains_key("cpa_session_instructions"))
    {
        return Err(failure(
            400,
            "The reserved session instructions marker must be removed before dispatch",
        ));
    }
    let Some(control) = &params.session_instructions else {
        return Ok(());
    };
    if params.operation != "responses"
        || params.image_api.is_some()
        || params.source_format != "openai-response"
    {
        return Err(failure(
            400,
            "sessionInstructions requires Responses inference",
        ));
    }
    if control.scope.trim().is_empty()
        || (!control.baseline_digest.is_empty()
            && (control.baseline_digest.len() != 64
                || !control
                    .baseline_digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))))
    {
        return Err(failure(
            400,
            "Invalid session instructions scope or baselineDigest",
        ));
    }
    let append = control.frame_type == Some(CpaSessionInstructionsFrameType::Append);
    if let Some(kind) = params.request.get("type") {
        if kind.as_str()
            != Some(if append {
                "response.append"
            } else {
                "response.create"
            })
        {
            return Err(failure(400, "Business and control frame types must agree"));
        }
    }
    let parent = params
        .request
        .get("previous_response_id")
        .filter(|v| !v.is_null());
    if control
        .previous_response_id
        .as_ref()
        .is_some_and(|id| id.is_empty())
        || parent.map(Value::as_str) != control.previous_response_id.as_deref().map(Some)
    {
        return Err(failure(
            400,
            "Business and control previous_response_id must agree",
        ));
    }
    let incremental = append || control.previous_response_id.is_some();
    if (!websocket || control.http_only == Some(true)) && incremental {
        return Err(continuation_error(
            409,
            "previous_response_not_found",
            "This runtime requires full input without previous_response_id",
        ));
    }
    if incremental && control.baseline_digest.is_empty() {
        return Err(continuation_error(
            409,
            "previous_response_not_found",
            "No reusable baseline proof is available; resend full input",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> CpaInferenceStartParams {
        serde_json::from_value(json!({"requestId":"r","credentialId":"worker","operation":"responses","sourceFormat":"openai-response","sessionId":"transport","sessionInstructions":{"mode":"start","scope":"caller/session","baselineDigest":""},"request":{"model":"fixture","input":[]}})).unwrap()
    }

    #[test]
    fn cpa_session_instructions_full_allows_invalidated_prefix_without_rewriting() {
        let p = params();
        validate(&p, false).unwrap();
        assert_eq!(p.request["input"], json!([]));
    }

    #[test]
    fn cpa_session_instructions_delta_is_request_scoped_on_http() {
        let mut p = params();
        p.session_instructions.as_mut().unwrap().frame_type =
            Some(CpaSessionInstructionsFrameType::Append);
        let error = validate(&p, false).unwrap_err();
        let data = error.data.unwrap();
        assert_eq!(data["httpStatus"], 409);
        let body: Value = serde_json::from_str(data["body"].as_str().unwrap()).unwrap();
        assert_eq!(body["error"]["code"], "previous_response_not_found");
        assert!(data.get("nativeError").is_none());
    }

    #[test]
    fn cpa_session_instructions_parent_and_digest_are_validated_before_dispatch() {
        let mut p = params();
        p.session_instructions
            .as_mut()
            .unwrap()
            .previous_response_id = Some("parent".into());
        assert!(validate(&p, true).is_err());
        p.request
            .insert("previous_response_id".into(), json!("parent"));
        assert_eq!(
            validate(&p, true).unwrap_err().data.unwrap()["httpStatus"],
            409
        );
        p.session_instructions.as_mut().unwrap().baseline_digest = "a".repeat(64);
        validate(&p, true).unwrap();
    }
}

#[cfg(test)]
#[test]
fn cpa_http_only_delta_is_rejected_even_with_websocket_capability() {
    let params = serde_json::from_value(json!({"requestId":"r","credentialId":"a","operation":"responses","sourceFormat":"openai-response","sessionId":"s","sessionInstructions":{"mode":"reuse","scope":"opaque","baselineDigest":"a".repeat(64),"httpOnly":true,"frameType":"response.append"},"request":{"model":"model","input":[]}})).unwrap();
    assert_eq!(
        validate(&params, true).unwrap_err().data.unwrap()["httpStatus"],
        409
    );
}
