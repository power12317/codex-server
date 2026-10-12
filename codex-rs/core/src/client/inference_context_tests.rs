use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;

fn time() -> TimeContext {
    TimeContext {
        date: "2026-10-07".into(),
        timezone: "Asia/Tokyo".into(),
    }
}

fn context(tag: &str, date: &str, timezone: &str, role: &str) -> Value {
    json!({"type":"message", "role":role, "content":[{"type":"input_text", "text":format!(
        "<{tag}><cwd>C:\\work</cwd><os>Windows</os><current_date>{date}</current_date><timezone>{timezone}</timezone></{tag}>"
    )}]})
}

#[test]
fn rewrites_only_latest_context_and_preserves_paths_and_history() {
    let historical = context("environment_context", "2025-01-01", "old-zone", "user");
    let mut input = vec![
        historical.clone(),
        json!({"type":"message", "role":"assistant", "content":"past answer"}),
        context("environment_context", "2026-10-06", "source-zone", "user"),
    ];
    time().apply(&mut input);
    assert_eq!(input.len(), 3);
    assert_eq!(input[0], historical);
    assert_eq!(
        input[2],
        context("environment_context", "2026-10-07", "Asia/Tokyo", "user")
    );
}

#[test]
fn missing_fields_and_quoted_contexts_are_not_added_or_rewritten() {
    for text in [
        "Explain this: <environment_context><timezone>UTC</timezone></environment_context>",
        "<environment_context>```\n<timezone>UTC</timezone>\n```</environment_context>",
        "```xml\n<environment_context><timezone>UTC</timezone></environment_context>\n```",
    ] {
        assert_eq!(time().rewrite_block(text, "environment_context"), None);
    }
    for text in [
        "<environment_context><cwd>/app</cwd></environment_context>",
        "<environment_context></environment_context>",
    ] {
        assert_eq!(
            time().rewrite_block(text, "environment_context"),
            Some(text.into())
        );
    }
    assert_eq!(
        time().rewrite_block(
            " \n<environment_context><timezone>UTC</timezone></environment_context>\n ",
            "environment_context"
        ),
        Some(
            " \n<environment_context><timezone>Asia/Tokyo</timezone></environment_context>\n "
                .into()
        )
    );
}

#[test]
fn tool_continuation_replaces_existing_contexts_without_appending_messages() {
    // Sanitized structure of the reported 1270-item request: the current contexts
    // precede many assistant/tool items, while older declarations remain history.
    let mut input = vec![json!({"type":"message", "role":"assistant", "content":[]}); 1270];
    input[904] = context("environment_context", "2026-10-06", "source-zone", "user");
    input[905] = context(
        "codex_apps_client_time_context",
        "2026-10-06",
        "source-zone",
        "developer",
    );
    input[970] = context("environment_context", "2026-10-07", "source-zone", "user");
    input[971] = context(
        "codex_apps_client_time_context",
        "2026-10-07",
        "source-zone",
        "developer",
    );
    input[1268] = json!({"type":"custom_tool_call", "call_id":"fixture-call", "name":"fixture", "input":"{}"});
    input[1269] = json!({"type":"custom_tool_call_output", "call_id":"fixture-call", "output":"fixture result"});
    let mut expected = input.clone();
    expected[970] = context("environment_context", "2026-10-07", "Asia/Tokyo", "user");
    expected[971] = context(
        "codex_apps_client_time_context",
        "2026-10-07",
        "Asia/Tokyo",
        "developer",
    );
    time().apply(&mut input);
    assert_eq!(input, expected);
    time().apply(&mut input);
    assert_eq!(
        input, expected,
        "repeated normalization never appends content"
    );
}

#[test]
fn missing_current_fields_do_not_rewrite_an_older_declaration() {
    let mut input = vec![
        context("environment_context", "2025-01-01", "old-zone", "user"),
        json!({"type":"message", "role":"user", "content":[{"type":"input_text", "text":"<environment_context><cwd>/app</cwd></environment_context>"}]}),
    ];
    let expected = input.clone();
    time().apply(&mut input);
    assert_eq!(input, expected);
}

#[test]
fn requests_without_time_declarations_remain_unchanged() {
    for mut input in [
        vec![],
        vec![
            json!({"type":"message", "role":"user", "content":[{"type":"input_text", "text":"Hello"}]}),
        ],
        vec![context(
            "environment_context",
            "2025-01-01",
            "old-zone",
            "assistant",
        )],
        vec![
            json!({"type":"function_call_output", "call_id":"fixture", "output":"<environment_context><timezone>UTC</timezone></environment_context>"}),
        ],
    ] {
        let expected = input.clone();
        time().apply(&mut input);
        assert_eq!(input, expected);
    }
}
