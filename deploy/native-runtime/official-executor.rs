// Test-only executor for the fixed official checkout. This intentionally uses the
// original ModelClientSession and original recovery handler, never InferenceSession.
async fn run_model(session:&Arc<Session>,turn:&Arc<TurnContext>,body:&Value,lite:bool) -> Result<Vec<Value>,CodexErr> {
    use crate::client::ModelClient;
    use crate::client_common::Prompt;
    use crate::responses_metadata::CodexResponsesMetadata;
    use crate::responses_metadata::CodexResponsesRequestKind;
    use crate::responses_retry::{ResponsesStreamRetryState,ResponsesStreamRequest,handle_response_stream_error};
    use crate::session::step_context::StepContext;
    use codex_login::auth::AgentIdentityAuthPolicy;
    use codex_protocol::ThreadId;
    use codex_protocol::protocol::SessionSource;
    use codex_protocol::models::BaseInstructions;
    use codex_protocol::config_types::ReasoningSummary;
    use codex_tools::{ToolSpec,ResponsesApiTool};
    use futures::StreamExt;
    let thread=ThreadId::from_string("11111111-1111-4111-8111-111111111111").unwrap();
    let config=&turn.config;
    let native=ModelClient::new(Some(session.services.auth_manager.clone()),AgentIdentityAuthPolicy::JwtOnly,
        thread, config.model_provider.clone(), SessionSource::Cli, codex_login::default_client::originator().value,
        body.get("text").and_then(|text| text.get("verbosity")).map(|value| serde_json::from_value(value.clone()).unwrap()).or(config.model_verbosity),config.features.enabled(Feature::ContentItemKinds),config.features.enabled(Feature::ReasoningEffortOverride),
        config.features.enabled(Feature::EnableRequestCompression),config.features.enabled(Feature::RuntimeMetrics),
        Session::build_model_client_beta_features_header(config),config.features.enabled(Feature::ConcurrentReasoningSummaries),None,
        config.http_client_factory(),config.workspace_routing_context(),Vec::new());
    let mut client=native.new_session();
    let mut model=turn.model_info().as_ref().clone();model.use_responses_lite=lite;
    if body.get("text").is_some() { model.support_verbosity = true; }
    let mut input:Vec<ResponseItem>=serde_json::from_value(body["input"].clone()).unwrap();
    let mut tools=Vec::new();
    if let Some(definitions)=body["tools"].as_array() {
        for definition in definitions {
            if definition["type"]=="image_generation" {
                input.insert(0,ResponseItem::AdditionalTools {id:None,role:"developer".into(),tools:vec![definition.clone()]});
            } else {
                tools.push(ToolSpec::Function(ResponsesApiTool { name:definition["name"].as_str().unwrap().into(),description:String::new(),strict:false,defer_loading:None,
                    parameters:serde_json::from_value(definition["parameters"].clone()).unwrap(),output_schema:None }));
            }
        }
    }
    let mut prompt=Prompt { input:input.clone(),tools:tools.into(),parallel_tool_calls:true,
        base_instructions:BaseInstructions {text:body["instructions"].as_str().unwrap().into(),..Default::default()},..Default::default() };
    session.record_conversation_items(turn,&model,&input).await;
    let mut metadata=CodexResponsesMetadata::new("fixture-installation".into(),thread.to_string(),thread.to_string(),"22222222-2222-4222-8222-222222222222".into());
    metadata.turn_id=Some("source-turn".into());metadata.request_kind=Some(CodexResponsesRequestKind::Turn);
    let step=StepContext::for_test(turn.clone());
    let mut retry=ResponsesStreamRetryState::default();
    loop {
        let result=async {
            let mut items=Vec::new();
            let mut stream=client.stream(&prompt,&model,&turn.session_telemetry,config.model_reasoning_effort.clone(),
                config.model_reasoning_summary.unwrap_or(ReasoningSummary::default()),None,&metadata,&codex_rollout_trace::InferenceTraceContext::disabled()).await?;
            while let Some(event)=stream.next().await {
                match event? {
                    codex_api::ResponseEvent::OutputItemDone(item)=>{
                        session.record_conversation_items(turn,&model,&[item.clone()]).await;
                        items.push(serde_json::to_value(item).unwrap());
                    }
                    codex_api::ResponseEvent::Completed{..}=>return Ok(items),
                    _=>{}
                }
            }
            Err(CodexErr::Stream("stream closed before response.completed".into()))
        }.await;
        match result {
            Ok(items)=>return Ok(items),
            Err(error)=>handle_response_stream_error(&mut retry,config.model_provider.stream_max_retries(),error,&mut client,session,&step,ResponsesStreamRequest::Sampling).await?,
        }
        prompt.input=session.clone_history().await.for_prompt(&model.input_modalities);
    }
}
