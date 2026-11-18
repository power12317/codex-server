use super::*;
use pretty_assertions::assert_eq;
use test_case::test_case;

#[test_case("Etc/UTC"; "utc")]
#[test_case("Asia/Singapore"; "singapore")]
#[test_case("Asia/Tokyo"; "tokyo")]
#[tokio::test]
#[cfg(unix)]
async fn cpa_tool_continuation_replaces_time_in_place(timezone: &str) -> Result<()> {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw("data: [DONE]\n\n", "text/event-stream"),
        )
        .expect(2)
        .mount(&upstream)
        .await;
    let home = TempDir::new()?;
    prepare_account(&upstream, &home)?;
    let _runtime = launch_with_timezone(&upstream, &home, Some(timezone)).await?;
    let mut ws = connect(&home).await?;
    let old_environment = "<environment_context><current_date>2020-01-01</current_date><timezone>history-zone</timezone></environment_context>";
    let old_client = "<codex_apps_client_time_context><timezone>history-zone</timezone><current_date>2020-01-01</current_date></codex_apps_client_time_context>";
    let current_environment = "<environment_context><cwd>C:\\work</cwd><current_date_utc>2026-01-01</current_date_utc><timezone>source-zone</timezone></environment_context>";
    let current_client = "<codex_apps_client_time_context><timezone>source-zone</timezone><current_date>2026-01-01</current_date></codex_apps_client_time_context>";
    let input = json!([
        {"role":"user", "content":old_environment},
        {"role":"developer", "content":old_client},
        {"role":"user", "content":current_environment},
        {"role":"developer", "content":current_client},
        {"role":"assistant", "content":"fixture answer"},
        {"type":"function_call", "call_id":"fixture-call", "name":"fixture", "arguments":"{}"},
        {"type":"function_call_output", "call_id":"fixture-call", "output":"fixture result"}
    ]);
    let request = start(json!({"model":"mock-model", "input":input}));
    let (logged, _) = continuity_tests::exchange(&mut ws, request).await?;
    let body: Value = serde_json::from_str(logged["body"].as_str().expect("logged body"))?;
    let actual = body["input"].as_array().expect("input items");
    assert_eq!(actual.len(), 7);
    assert_eq!(actual[0]["content"][0]["text"], json!(old_environment));
    assert_eq!(actual[1]["content"][0]["text"], json!(old_client));
    for index in [2, 3] {
        let text = actual[index]["content"][0]["text"]
            .as_str()
            .expect("context text");
        assert!(
            text.contains(&format!("<timezone>{timezone}</timezone>")),
            "{text}"
        );
        assert!(!text.contains("source-zone"));
        assert!(!text.contains("2026-01-01"));
        assert_eq!(text.matches("<timezone>").count(), 1);
        let (date_tag, absent_tag) = if index == 2 {
            ("<current_date_utc>", "<current_date>")
        } else {
            ("<current_date>", "<current_date_utc>")
        };
        assert_eq!(text.matches(date_tag).count(), 1);
        assert!(!text.contains(absent_tag));
    }
    assert!(
        actual[2]["content"][0]["text"]
            .as_str()
            .expect("environment text")
            .contains("<cwd>C:\\work</cwd>")
    );
    assert_eq!(actual[6]["type"], json!("function_call_output"));
    assert_eq!(actual[6]["output"], json!("fixture result"));
    let received = upstream
        .received_requests()
        .await
        .expect("recording enabled");
    let wire: Value = received
        .iter()
        .find(|request| request.url.path() == "/v1/responses")
        .expect("model request")
        .body_json()?;
    assert_eq!(wire, body);

    let (plain, _) = continuity_tests::exchange(
        &mut ws,
        start(json!({"model":"mock-model", "input":"ordinary prompt"})),
    )
    .await?;
    let plain: Value = serde_json::from_str(plain["body"].as_str().expect("logged body"))?;
    assert_eq!(plain["input"].as_array().expect("input items").len(), 1);
    assert_eq!(
        plain["input"][0]["content"][0]["text"],
        json!("ordinary prompt")
    );
    Ok(())
}
