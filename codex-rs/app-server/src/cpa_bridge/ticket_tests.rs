use super::*;
use pretty_assertions::assert_eq;

fn at(seconds: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(seconds, 0).expect("fixture timestamp")
}

fn capture() -> TicketCapture {
    let mut headers = HeaderMap::new();
    headers.insert(
        "cookie",
        "__cflb=request-cf; __oailb=request-oai".parse().unwrap(),
    );
    TicketCapture::new(
        "request".into(),
        "credential.json".into(),
        "actual-model".into(),
        &headers,
    )
}

#[test]
fn cpa_ticket_response_updates_are_exchange_scoped_and_never_extend_known_expiry() {
    let mut first = capture();
    let mut second = capture();
    let mut headers = HeaderMap::new();
    headers.insert("x-codex-turn-state", "private-ticket".parse().unwrap());
    headers.insert(
        "set-cookie",
        "__oailb=response-oai; Max-Age=60".parse().unwrap(),
    );
    first.response_headers(&headers, at(1_800_000_000));
    let ticket = first.snapshot(at(1_800_000_001)).unwrap();
    assert_eq!(ticket.route.cflb.value, "request-cf");
    assert_eq!(ticket.route.cflb.expires_at, None);
    assert_eq!(ticket.route.oailb.value, "response-oai");
    assert_eq!(
        ticket.route.oailb.expires_at,
        Some(at(1_800_000_060).to_rfc3339())
    );
    assert!(second.snapshot(at(1_800_000_001)).is_none());
    assert!(first.snapshot(at(1_800_000_030)).is_none());
    first.state = Some("new-state-after-expiry".into());
    assert!(first.snapshot(at(1_800_000_061)).is_none());
    assert!(!format!("{ticket:?}").contains("private-ticket"));
    assert!(!format!("{:?}", ticket.route).contains("response-oai"));
}

#[test]
fn cpa_ticket_deletion_expiration_invalid_values_and_missing_pair_are_unusable() {
    for update in [
        "__cflb=; Max-Age=0",
        "__oailb=value; Max-Age=-1",
        "__oailb=value; Expires=Thu, 01 Jan 1970 00:00:00 GMT",
        "__oailb=value; Max-Age=broken",
        "__oailb=invalid value",
    ] {
        let mut capture = capture();
        let mut headers = HeaderMap::new();
        headers.insert("x-codex-turn-state", "ticket".parse().unwrap());
        headers.insert("set-cookie", update.parse().unwrap());
        capture.response_headers(&headers, at(1_800_000_000));
        assert!(capture.snapshot(at(1_800_000_000)).is_none(), "{update}");
    }
    let mut capture = TicketCapture::new("r".into(), "c".into(), "m".into(), &HeaderMap::new());
    let headers = HeaderMap::from_iter([(
        "x-codex-turn-state".parse().unwrap(),
        "ticket".parse().unwrap(),
    )]);
    capture.response_headers(&headers, at(1_800_000_000));
    assert!(capture.snapshot(at(1_800_000_000)).is_none());
}

#[test]
fn cpa_ticket_sse_metadata_handles_chunks_and_ignores_model_text() {
    for kind in ["response.metadata", "codex.response.metadata"] {
        let mut capture = capture();
        let decoy = "data: {\"type\":\"response.output_text.delta\",\"headers\":{\"x-codex-turn-state\":\"decoy\"}}\r\n\r\n";
        assert!(capture.push(decoy.as_bytes()).is_empty());
        let event = format!(
            "event: metadata\r\ndata: {{\"type\":\"{kind}\",\r\ndata: \"headers\":{{\"x-codex-turn-state\":\"metadata-ticket\"}}}}\r\n\r\n"
        );
        let snapshots: Vec<_> = event
            .as_bytes()
            .chunks(1)
            .flat_map(|chunk| capture.push(chunk))
            .collect();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].state, "metadata-ticket");
        assert!(capture.push(event.as_bytes()).is_empty());
    }
}

#[test]
fn cpa_ticket_debug_and_wire_serialization_have_separate_privacy_contracts() {
    let mut capture = capture();
    capture.state = Some("secret-state".into());
    let ticket = capture.snapshot(Utc::now()).unwrap();
    let notification = codex_app_server_protocol::ServerNotification::CpaInferenceTicket(ticket);
    assert!(!format!("{notification:?}").contains("secret-state"));
    assert!(!format!("{notification}").contains("secret-state"));
    let wire = serde_json::to_value(notification).unwrap();
    assert_eq!(wire["params"]["state"], "secret-state");
    assert_eq!(wire["params"]["route"]["__cflb"]["value"], "request-cf");
    assert!(
        wire["params"]["route"]["__cflb"]
            .get("expires_at")
            .is_none()
    );
}

#[test]
fn cpa_ticket_repeated_updates_and_cookie_claims_preserve_the_earliest_deadline() {
    let mut capture = capture();
    let expiry = 1_800_000_030;
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::json!({"exp":expiry}).to_string());
    let value = format!("__oailb=e30.{payload}.sig; Max-Age=60");
    let headers = HeaderMap::from_iter([
        (
            "x-codex-turn-state".parse().unwrap(),
            "ticket".parse().unwrap(),
        ),
        ("set-cookie".parse().unwrap(), value.parse().unwrap()),
    ]);
    capture.response_headers(&headers, at(1_800_000_000));
    let first = capture.snapshot(at(1_800_000_001)).unwrap();
    assert_eq!(first.route.oailb.expires_at, Some(at(expiry).to_rfc3339()));
    capture.response_headers(&headers, at(1_800_000_020));
    assert!(capture.snapshot(at(1_800_000_020)).is_none());
    capture.state = Some("another-ticket".into());
    assert!(capture.snapshot(at(expiry)).is_none());
}

#[test]
fn cpa_ticket_retains_known_jar_deadlines_only_for_the_actual_cookie_value() {
    let factory = codex_http_client::HttpClientFactory::new(
        codex_http_client::OutboundProxyPolicy::ReqwestDefault,
    );
    let uri = "https://ticket-expiry.chatgpt.com/backend-api/responses"
        .parse()
        .unwrap();
    let deadline = Utc::now() + chrono::Duration::minutes(3);
    let raw = format!(
        "__cflb=expiry-fixture; Expires={}; Path=/",
        httpdate::fmt_http_date(deadline.into())
    );
    let headers = HeaderMap::from_iter([("set-cookie".parse().unwrap(), raw.parse().unwrap())]);
    factory.store_chatgpt_response_cookies(&uri, &headers);
    let mut capture = capture();
    capture.cookies[0] = cookie("expiry-fixture", None);
    capture.preserve_known_deadlines(&factory);
    let expected = capture.cookies[0].as_ref().unwrap().expires_at.clone();
    assert!(expected.is_some());
    assert!(capture.cookies[1].as_ref().unwrap().expires_at.is_none());
    factory.store_chatgpt_response_cookies(
        &uri,
        &HeaderMap::from_iter([(
            "set-cookie".parse().unwrap(),
            "__cflb=expiry-fixture; Max-Age=3600".parse().unwrap(),
        )]),
    );
    capture.preserve_known_deadlines(&factory);
    assert_eq!(capture.cookies[0].as_ref().unwrap().expires_at, expected);
}
