//! Native values for field-preservation regressions and external HTTP replay.
//! Every literal names all native fields. Exhaustive variant matching below
//! makes upstream additions a compile-time review point, never a runtime gate.
use codex_protocol::ResponseItemId;
use codex_protocol::models::*;
use codex_tools::*;
use serde_json::Value;
use serde_json::json;

fn id(prefix: &str, suffix: &str) -> Option<ResponseItemId> {
    Some(ResponseItemId::with_suffix(prefix, suffix))
}

fn metadata() -> Option<InternalChatMessageMetadataPassthrough> {
    Some(InternalChatMessageMetadataPassthrough {
        turn_id: Some("fixture-turn".into()),
        create_time: Some(1234.into()),
        content_item_kinds: None,
        content_item_metadata: None,
        // Native serde deliberately refuses caller-supplied host tool records.
        cell_id: None,
        executed_tool_calls: None,
        tool_calls_complete: None,
    })
}

pub fn content() -> Vec<ContentItem> {
    vec![
        ContentItem::InputText {
            text: "text".into(),
        },
        ContentItem::InputImage {
            image: ImageReference::Inline {
                image_url: "data:image/png;base64,AA==".into(),
            },
            detail: Some(ImageDetail::Original),
        },
        ContentItem::InputImage {
            image: ImageReference::File {
                file_id: "file-fixture".into(),
            },
            detail: Some(ImageDetail::High),
        },
        ContentItem::InputAudio {
            audio_url: "data:audio/wav;base64,AA==".into(),
        },
        ContentItem::OutputText {
            text: "output".into(),
            annotations: Some(vec![
                json!({"type":"url_citation","url":"https://example.com","title":"fixture","start_index":0,"end_index":6}),
            ]),
            logprobs: Some(vec![
                json!({"token":"output","logprob":-0.5,"bytes":[111],"top_logprobs":[]}),
            ]),
        },
    ]
}

pub fn output_content() -> Vec<FunctionCallOutputContentItem> {
    vec![
        FunctionCallOutputContentItem::InputText {
            text: "result".into(),
        },
        FunctionCallOutputContentItem::InputImage {
            image: ImageReference::File {
                file_id: "file-result".into(),
            },
            detail: Some(ImageDetail::Low),
        },
        FunctionCallOutputContentItem::InputAudio {
            audio_url: "data:audio/wav;base64,AA==".into(),
        },
        FunctionCallOutputContentItem::EncryptedContent {
            encrypted_content: "encrypted-result".into(),
        },
    ]
}

pub fn history() -> Vec<ResponseItem> {
    let mut items = vec![
        ResponseItem::AdditionalTools {
            id: id("at", "tools"),
            role: "developer".into(),
            tools: serde_json::to_value(tools())
                .unwrap()
                .as_array()
                .unwrap()
                .clone()
                .into(),
        },
        ResponseItem::Message {
            encrypted_content: Some("encrypted-message".into()),
            status: Some("completed".into()),
            id: id("msg", "message"),
            role: "assistant".into(),
            content: content(),
            phase: Some(MessagePhase::FinalAnswer),
            internal_chat_message_metadata_passthrough: metadata(),
        },
        ResponseItem::AgentMessage {
            id: id("amsg", "agent"),
            author: "sender".into(),
            recipient: "recipient".into(),
            content: vec![
                AgentMessageInputContent::InputText {
                    text: "agent text".into(),
                },
                AgentMessageInputContent::EncryptedContent {
                    encrypted_content: "agent encrypted".into(),
                },
            ],
            internal_chat_message_metadata_passthrough: metadata(),
        },
        ResponseItem::Reasoning {
            id: id("rs", "reasoning"),
            summary: vec![ReasoningItemReasoningSummary::SummaryText {
                text: "summary".into(),
            }],
            content: Some(vec![
                ReasoningItemContent::ReasoningText {
                    text: "reasoning".into(),
                },
                ReasoningItemContent::Text {
                    text: "text".into(),
                },
            ]),
            encrypted_content: Some("encrypted-reasoning".into()),
            internal_chat_message_metadata_passthrough: metadata(),
        },
        ResponseItem::LocalShellCall {
            encrypted_content: Some("encrypted-shell".into()),
            id: id("lsh", "shell"),
            call_id: Some("shell-call".into()),
            status: LocalShellStatus::Completed,
            action: LocalShellAction::Exec(LocalShellExecAction {
                command: vec!["echo".into(), "client-only".into()],
                timeout_ms: Some(1000),
                working_directory: Some("/tmp".into()),
                env: Some(std::collections::HashMap::from([(
                    "FIXTURE".into(),
                    "true".into(),
                )])),
                user: Some("fixture".into()),
            }),
            internal_chat_message_metadata_passthrough: metadata(),
        },
        ResponseItem::FunctionCall {
            encrypted_content: Some("encrypted-function".into()),
            status: Some("in_progress".into()),
            id: id("fc", "function"),
            name: "Read".into(),
            namespace: Some("external".into()),
            arguments: "{\"path\":\"fixture\"}".into(),
            encrypted_function_args: Some(vec!["encrypted-argument".into()]),
            call_id: "function-call".into(),
            internal_chat_message_metadata_passthrough: metadata(),
        },
        ResponseItem::ToolSearchCall {
            encrypted_content: Some("encrypted-search".into()),
            id: id("tsc", "search"),
            call_id: Some("search-call".into()),
            status: Some("completed".into()),
            execution: "client".into(),
            arguments: json!({"query":"Read", "limit":1}),
            internal_chat_message_metadata_passthrough: metadata(),
        },
        ResponseItem::FunctionCallOutput {
            id: id("fco", "function"),
            call_id: Some("function-call".into()),
            name: Some("Read".into()),
            namespace: Some("external".into()),
            output: FunctionCallOutputPayload::from_content_items(output_content()),
            internal_chat_message_metadata_passthrough: metadata(),
        },
        ResponseItem::CustomToolCall {
            encrypted_content: Some("encrypted-custom".into()),
            id: id("ctc", "custom"),
            status: Some("incomplete".into()),
            call_id: "custom-call".into(),
            name: "client_exec".into(),
            namespace: Some("external".into()),
            input: "client-only command".into(),
            internal_chat_message_metadata_passthrough: metadata(),
        },
        ResponseItem::CustomToolCallOutput {
            id: id("ctco", "custom"),
            call_id: "custom-call".into(),
            name: Some("client_exec".into()),
            output: FunctionCallOutputPayload::from_content_items(output_content()),
            internal_chat_message_metadata_passthrough: metadata(),
        },
        ResponseItem::ToolSearchOutput {
            id: id("tso", "search"),
            call_id: Some("search-call".into()),
            status: "completed".into(),
            execution: "client".into(),
            tools: serde_json::to_value(tools())
                .unwrap()
                .as_array()
                .unwrap()
                .clone(),
            internal_chat_message_metadata_passthrough: metadata(),
        },
        ResponseItem::WebSearchCall {
            encrypted_content: Some("encrypted-web".into()),
            id: id("ws", "web"),
            status: Some("completed".into()),
            action: Some(WebSearchAction::Search {
                query: Some("fixture".into()),
                queries: Some(vec!["fixture one".into(), "fixture two".into()]),
            }),
            internal_chat_message_metadata_passthrough: metadata(),
        },
        ResponseItem::ImageGenerationCall {
            encrypted_content: Some("encrypted-image".into()),
            id: id("ig", "image"),
            status: "completed".into(),
            revised_prompt: Some("fixture revised prompt".into()),
            result: "fixture-base64".into(),
            internal_chat_message_metadata_passthrough: metadata(),
        },
        ResponseItem::Compaction {
            id: id("cmp", "compaction"),
            encrypted_content: "opaque-compaction".into(),
            internal_chat_message_metadata_passthrough: metadata(),
        },
        ResponseItem::ConfigurationUpdate {
            reasoning: ConfigurationReasoning {
                effort: codex_protocol::openai_models::ReasoningEffort::High,
            },
        },
        ResponseItem::CompactionTrigger {},
        ResponseItem::ContextCompaction {
            id: id("cmp", "context"),
            encrypted_content: Some("opaque-context".into()),
            internal_chat_message_metadata_passthrough: metadata(),
        },
    ];
    // Native String statuses are preserved verbatim, including failed. Shell
    // status uses its actual enum and never accepts a fabricated failed variant.
    for status in ["completed", "in_progress", "failed"] {
        for action in [
            WebSearchAction::Search {
                query: Some("fixture".into()),
                queries: None,
            },
            WebSearchAction::OpenPage {
                url: Some("https://example.com".into()),
            },
            WebSearchAction::FindInPage {
                url: Some("https://example.com".into()),
                pattern: Some("fixture".into()),
            },
        ] {
            items.push(ResponseItem::WebSearchCall {
                encrypted_content: None,
                id: id("ws", &format!("state-{}", items.len())),
                status: Some(status.into()),
                action: Some(action),
                internal_chat_message_metadata_passthrough: None,
            });
        }
    }
    for status in [LocalShellStatus::InProgress, LocalShellStatus::Incomplete] {
        items.push(ResponseItem::LocalShellCall {
            encrypted_content: None,
            id: id("lsh", &format!("state-{}", items.len())),
            call_id: None,
            status,
            action: LocalShellAction::Exec(LocalShellExecAction {
                command: vec!["echo".into()],
                timeout_ms: None,
                working_directory: None,
                env: None,
                user: None,
            }),
            internal_chat_message_metadata_passthrough: None,
        });
    }
    for status in ["in_progress", "failed"] {
        items.push(ResponseItem::ImageGenerationCall {
            encrypted_content: None,
            id: id("ig", &format!("state-{}", items.len())),
            status: status.into(),
            revised_prompt: None,
            result: "fixture-result".into(),
            internal_chat_message_metadata_passthrough: None,
        });
        items.push(ResponseItem::ToolSearchOutput {
            id: id("tso", &format!("state-{}", items.len())),
            call_id: None,
            status: status.into(),
            execution: "client".into(),
            tools: vec![],
            internal_chat_message_metadata_passthrough: None,
        });
    }
    // Optional status absence stays absent; required business state isn't invented.
    items.push(ResponseItem::WebSearchCall {
        id: None,
        encrypted_content: None,
        status: None,
        action: None,
        internal_chat_message_metadata_passthrough: None,
    });
    items
}

/// No wildcard: a new native item requires a corresponding fixture and review.
pub fn history_kind(item: &ResponseItem) -> &'static str {
    match item {
        ResponseItem::AdditionalTools { .. } => "AdditionalTools",
        ResponseItem::Message { .. } => "Message",
        ResponseItem::AgentMessage { .. } => "AgentMessage",
        ResponseItem::Reasoning { .. } => "Reasoning",
        ResponseItem::LocalShellCall { .. } => "LocalShellCall",
        ResponseItem::FunctionCall { .. } => "FunctionCall",
        ResponseItem::ToolSearchCall { .. } => "ToolSearchCall",
        ResponseItem::FunctionCallOutput { .. } => "FunctionCallOutput",
        ResponseItem::CustomToolCall { .. } => "CustomToolCall",
        ResponseItem::CustomToolCallOutput { .. } => "CustomToolCallOutput",
        ResponseItem::ToolSearchOutput { .. } => "ToolSearchOutput",
        ResponseItem::WebSearchCall { .. } => "WebSearchCall",
        ResponseItem::ImageGenerationCall { .. } => "ImageGenerationCall",
        ResponseItem::Compaction { .. } => "Compaction",
        ResponseItem::ConfigurationUpdate { .. } => "ConfigurationUpdate",
        ResponseItem::CompactionTrigger { .. } => "CompactionTrigger",
        ResponseItem::ContextCompaction { .. } => "ContextCompaction",
        ResponseItem::Other => "Other",
    }
}

pub fn tools() -> Vec<ToolSpec> {
    let function = ResponsesApiTool {
        name: "Read".into(), description: "client function".into(), strict: false,
        defer_loading: Some(true),
        parameters: JsonSchema::from_raw_schema(json!({"$schema":"http://json-schema.org/draft-07/schema#","type":"object","properties":{"path":{"type":"string","minLength":1,"default":"fixture"}},"required":["path"],"additionalProperties":false,"future":{"nested":[true,null]}}).as_object().unwrap().clone()),
        output_schema: Some(ToolOutputSchema::from_caller(json!({"type":"string","maxLength":100}))),
    };
    let custom = FreeformTool {
        name: "client_exec".into(),
        description: "client only".into(),
        defer_loading: Some(false),
        format: FreeformToolFormat {
            r#type: "grammar".into(),
            syntax: "lark".into(),
            definition: "start: /.+/".into(),
        },
    };
    vec![
        ToolSpec::Function(function.clone()),
        ToolSpec::Freeform(custom.clone()),
        ToolSpec::Namespace(ResponsesApiNamespace {
            name: "external".into(),
            description: "all fields".into(),
            tools: vec![
                ResponsesApiNamespaceTool::Function(function),
                ResponsesApiNamespaceTool::Custom(custom),
            ],
        }),
        ToolSpec::ToolSearch {
            execution: "client".into(),
            description: "search declarations".into(),
            parameters: JsonSchema::from_raw_schema(
                json!({"type":"object","properties":{"query":{"type":"string","minLength":1}}})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        },
        ToolSpec::WebSearch {
            external_web_access: Some(false),
            indexed_web_access: Some(true),
            filters: Some(ResponsesApiWebSearchFilters {
                allowed_domains: Some(vec!["example.com".into()]),
            }),
            user_location: Some(ResponsesApiWebSearchUserLocation {
                r#type: codex_protocol::config_types::WebSearchUserLocationType::Approximate,
                country: Some("US".into()),
                region: Some("CA".into()),
                city: Some("San Francisco".into()),
                timezone: Some("America/Los_Angeles".into()),
            }),
            search_context_size: Some(codex_protocol::config_types::WebSearchContextSize::High),
            search_content_types: Some(vec!["text".into(), "image".into()]),
        },
        ToolSpec::ImageGeneration {
            options: json!({"output_format":"png","quality":"low","future_option":{"value":false}})
                .as_object()
                .unwrap()
                .clone(),
        },
    ]
}

pub fn tool_kind(tool: &ToolSpec) -> &'static str {
    match tool {
        ToolSpec::Function(_) => "function",
        ToolSpec::Namespace(_) => "namespace",
        ToolSpec::ToolSearch { .. } => "tool_search",
        ToolSpec::WebSearch { .. } => "web_search",
        ToolSpec::ImageGeneration { .. } => "image_generation",
        ToolSpec::Freeform(_) => "custom",
    }
}

pub fn source_metadata() -> Value {
    json!({"session_id":"fixture-session","thread_id":"fixture-thread","turn_id":"fixture-turn","prompt_cache_key":"fixture-cache","client_metadata":{"thread_source":"third-party"}})
}

/// Each native control struct is constructed exhaustively as well. Fields added
/// by an upstream update require a fixture value before these tests compile.
pub fn request_controls() -> Value {
    let generation = codex_api::ResponsesGenerationOptions {
        max_output_tokens: Some(123),
        temperature: Some(0.25),
        top_p: Some(0.75),
        truncation: Some("auto".into()),
        prompt_cache_retention: Some("24h".into()),
        safety_identifier: Some("fixture-safety".into()),
        user: Some("fixture-user".into()),
        extensions: json!({"future_business_control":{"enabled":true}})
            .as_object()
            .unwrap()
            .clone(),
    };
    let mut controls = serde_json::to_value(generation).unwrap();
    controls["reasoning"] = serde_json::to_value(codex_api::Reasoning {
        effort: Some(codex_protocol::openai_models::ReasoningEffort::High),
        summary: Some(codex_protocol::config_types::ReasoningSummary::Detailed),
        context: Some(codex_api::ReasoningContext::AllTurns),
    })
    .unwrap();
    controls["text"] = serde_json::to_value(codex_api::TextControls {
        verbosity: Some(codex_api::OpenAiVerbosity::High),
        format: Some(codex_api::ResponsesTextFormat::Schema(codex_api::TextFormat {
            r#type: codex_api::TextFormatType::JsonSchema, strict: false,
            schema: json!({"type":"object","properties":{"result":{"type":"string","maxLength":10}},"additionalProperties":false}),
            name: "fixture-format".into(),
        })),
    }).unwrap();
    controls["stream_options"] = serde_json::to_value(codex_api::StreamOptions {
        reasoning_summary_delivery: codex_api::ReasoningSummaryDelivery::SequentialCutoff,
    })
    .unwrap();
    controls["parallel_tool_calls"] = json!(false);
    controls["store"] = json!(false);
    controls["include"] = json!(["reasoning.encrypted_content"]);
    controls["tool_choice"] = json!("auto");
    controls["service_tier"] = json!("default");
    controls
}

/// Exhaustive native request destructuring is a compile-time check, not a
/// client admission rule. Routing/transport identity still belongs to the worker.
pub fn check_native_request_shape(request: codex_api::ResponsesApiRequest) {
    let codex_api::ResponsesApiRequest {
        model: _,
        stream: _,
        service_tier: _,
        input: _,
        tools: _,
        tool_choice: _,
        parallel_tool_calls: _,
        reasoning: _,
        store: _,
        stream_options: _,
        include: _,
        prompt_cache_key: _,
        text: _,
        client_metadata: _,
        access_programs: _,
        generation: _,
    } = request;
}

/// Nested native enums are compile-time review points too.
pub fn check_nested_native_variants(
    item: ContentItem,
    output: FunctionCallOutputContentItem,
    agent: AgentMessageInputContent,
    action: WebSearchAction,
) {
    match item {
        ContentItem::InputText { text: _ } => (),
        ContentItem::InputImage {
            image: _,
            detail: _,
        } => (),
        ContentItem::InputAudio { audio_url: _ } => (),
        ContentItem::OutputText {
            text: _,
            annotations: _,
            logprobs: _,
        } => (),
    }
    match output {
        FunctionCallOutputContentItem::InputText { text: _ } => (),
        FunctionCallOutputContentItem::InputImage {
            image: _,
            detail: _,
        } => (),
        FunctionCallOutputContentItem::InputAudio { audio_url: _ } => (),
        FunctionCallOutputContentItem::EncryptedContent {
            encrypted_content: _,
        } => (),
    }
    match agent {
        AgentMessageInputContent::InputText { text: _ } => (),
        AgentMessageInputContent::EncryptedContent {
            encrypted_content: _,
        } => (),
    }
    match action {
        WebSearchAction::Search {
            query: _,
            queries: _,
        } => (),
        WebSearchAction::OpenPage { url: _ } => (),
        WebSearchAction::FindInPage { url: _, pattern: _ } => (),
        WebSearchAction::Other => (),
    }
}

/// External HTTP replay excludes harness attribution metadata. It still contains
/// every native history variant and native message/tool-output content form.
pub fn replay_history() -> Vec<ResponseItem> {
    let mut items = history();
    for item in &mut items {
        item.clear_internal_chat_message_metadata_passthrough();
    }
    items
}

/// Expected media-detail normalization is provided by the native formatter,
/// rather than duplicated in the adapter or patched in final wire JSON.
pub fn formatted_replay_history(
    model: &codex_protocol::openai_models::ModelInfo,
) -> Vec<ResponseItem> {
    let prompt = crate::client_common::Prompt {
        input: replay_history(),
        base_instructions: BaseInstructions {
            text: String::new(),
            ..Default::default()
        },
        ..Default::default()
    };
    prompt.get_formatted_input_for_request(model)
}

/// Build an independent expected array with the real native request builder.
/// There is no adapter call, network send, or duplicated field normalization here.
pub fn native_request_history(
    config: &crate::config::Config,
    model: &codex_protocol::openai_models::ModelInfo,
) -> Vec<ResponseItem> {
    let thread = "00000000-0000-4000-8000-000000000001";
    let client = crate::client::ModelClient::new(
        None,
        codex_login::auth::AgentIdentityAuthPolicy::JwtOnly,
        codex_protocol::ThreadId::from_string(thread).unwrap(),
        config.model_provider.clone(),
        codex_protocol::protocol::SessionSource::Cli,
        codex_login::default_client::originator().value,
        config.model_verbosity,
        false,
        false,
        false,
        false,
        None,
        false,
        config
            .features
            .enabled(codex_features::Feature::OutputTokenReplay),
        None,
        config.http_client_factory(),
        config.workspace_routing_context(),
        Vec::new(),
    );
    let prompt = crate::client_common::Prompt {
        input: replay_history(),
        base_instructions: BaseInstructions {
            text: String::new(),
            ..Default::default()
        },
        ..Default::default()
    };
    let metadata = crate::responses_metadata::CodexResponsesMetadata::new(
        "fixture-installation".into(),
        thread.into(),
        thread.into(),
        "fixture-window".into(),
    );
    client
        .build_responses_request(
            &prompt,
            model,
            None,
            codex_protocol::config_types::ReasoningSummary::None,
            None,
            &metadata,
            false,
        )
        .unwrap()
        .input
}
