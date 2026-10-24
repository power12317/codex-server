use super::*;
use pretty_assertions::assert_eq;

#[test]
fn native_tool_choice_and_custom_format_shapes_are_not_projected_to_field_lists() {
    let tool = json!({"type":"custom","name":"client_exec","description":"client tool","format":{"type":"text","syntax":"native-syntax","definition":"native-definition"}});
    for choice in [
        json!({"type":"custom","name":"client_exec","future_field":{"keep":true}}),
        json!({"type":"allowed_tools","mode":"auto","tools":[{"type":"custom","name":"client_exec"}],"future_field":{"keep":true}}),
    ] {
        let request = json!({"tools":[tool],"tool_choice":choice});
        let parsed = InferenceTools::parse(request.as_object().unwrap()).unwrap();
        assert_eq!(
            serde_json::to_value(&parsed.native).unwrap(),
            request["tools"]
        );
        assert_eq!(parsed.choice, Some(choice));
    }
}

#[test]
fn every_native_tool_field_survives_the_declaration_adapter() {
    let tools = crate::test_support::inference_fixtures::tools();
    let expected = serde_json::to_value(&tools).unwrap();
    let request = json!({"tools":expected});
    let parsed = InferenceTools::parse(request.as_object().unwrap()).unwrap();
    assert_eq!(serde_json::to_value(&parsed.native).unwrap(), expected);
    assert_eq!(
        codex_tools::create_tools_json_for_responses_lite(&parsed.native).unwrap(),
        codex_tools::create_tools_json_for_responses_lite(&tools).unwrap()
    );
    assert_eq!(
        tools
            .iter()
            .map(crate::test_support::inference_fixtures::tool_kind)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        6
    );
}

#[test]
fn normalizes_function_envelopes_without_losing_schema_or_choice() {
    let parameters = json!({"type":"object", "properties":{"value":{"type":"string", "encrypted":true}}, "required":["value"], "additionalProperties":false});
    let request = json!({"tools":[{"type":"function", "function":{"name":"run", "description":"test", "parameters":parameters, "strict":true, "defer_loading":false}}], "tool_choice":{"type":"function", "function":{"name":"run"}}});
    let parsed = InferenceTools::parse(request.as_object().unwrap()).unwrap();
    assert_eq!(
        serde_json::to_value(parsed.native).unwrap(),
        json!([{"type":"function", "name":"run", "description":"test", "parameters":parameters, "strict":true, "defer_loading":false}])
    );
    assert_eq!(
        parsed.choice,
        Some(json!({"type":"function", "name":"run"}))
    );
}

#[test]
fn infers_function_wrappers_and_consumes_null_optional_controls() {
    let parameters =
        json!({"type":"object","properties":{"query":{"type":"string","pattern":"[a-z]+"}}});
    for tool in [
        json!({"name":"lookup","parameters":parameters,"strict":null,"defer_loading":null}),
        json!({"function":{"type":"function","name":"lookup","input_schema":parameters,"strict":null,"defer_loading":null}}),
    ] {
        let request =
            json!({"tools":[tool],"tool_choice":{"function":{"name":"lookup"},"namespace":null}});
        let parsed = InferenceTools::parse(request.as_object().unwrap()).unwrap();
        assert_eq!(
            serde_json::to_value(parsed.native).unwrap(),
            json!([{"type":"function","name":"lookup","description":"","parameters":parameters,"strict":false}])
        );
        assert_eq!(
            parsed.choice,
            Some(json!({"type":"function","name":"lookup"}))
        );
    }
}

#[test]
fn caller_schemas_are_preserved_in_all_function_envelopes_and_native_serializers() {
    let parameters = json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "type": "object",
        "properties": {
            "query": {"type": "string", "default": "ab", "minLength": 2, "maxLength": 12, "pattern": "[a-z]+"},
            "limit": {"type": "number", "default": 3, "minimum": 0, "exclusiveMinimum": 0, "maximum": 20},
            "items": {"type": "array", "minItems": 1, "maxItems": 4, "items": [{"const": null}, false]},
            "boolean": true
        },
        "required": ["query"],
        "additionalProperties": {"anyOf": [false, {"$ref": "#/$defs/item"}]},
        "$defs": {"item": {"type": "string", "minLength": 1}},
        "definitions": {"legacy": {"default": null}},
        "future_constraint": {"nested": {"type": "future_type"}}
    });
    let function = json!({"type":"function", "name":"WebSearch", "description":"client function", "parameters":parameters, "strict":false});
    let mut nested = function.clone();
    nested.as_object_mut().unwrap().remove("type");
    for tool in [
        function.clone(),
        json!({"type":"function", "function":nested}),
        json!({"name":"WebSearch", "description":"client function", "input_schema":parameters, "strict":false}),
    ] {
        let request = json!({"tools":[tool]});
        let parsed = InferenceTools::parse(request.as_object().unwrap()).unwrap();
        assert_eq!(
            codex_tools::create_tools_json_for_responses_api(&parsed.native).unwrap(),
            vec![function.clone()]
        );
        assert_eq!(
            codex_tools::create_tools_json_for_responses_lite(&parsed.native).unwrap(),
            vec![
                json!({"type":"namespace", "name":"functions", "description":"", "tools":[function]})
            ]
        );
    }

    let tools = json!([
        {"type":"namespace", "name":"external", "description":"client tools", "tools":[function]},
        {"type":"tool_search", "execution":"client", "description":"search", "parameters":parameters}
    ]);
    let request = json!({"tools":tools});
    let parsed = InferenceTools::parse(request.as_object().unwrap()).unwrap();
    assert_eq!(serde_json::to_value(&parsed.native).unwrap(), tools);
    assert_eq!(
        codex_tools::create_tools_json_for_responses_lite(&parsed.native).unwrap(),
        tools.as_array().unwrap().clone()
    );
}

#[test]
fn reports_invalid_tool_envelopes_at_their_location() {
    for tool in [
        json!({"type":"computer"}),
        json!({"type":"function", "name":"run", "parameters":[]}),
        json!({"type":"tool_search", "parameters":null}),
    ] {
        let request = json!({"tools":[tool]});
        let error = InferenceTools::parse(request.as_object().unwrap())
            .err()
            .unwrap();
        assert!(format!("{error:#}").starts_with("tools[0]:"));
    }
}

#[test]
fn native_tools_preserve_namespaces_search_options_and_custom_text_format() {
    let request = json!({"tools":[
        {"type":"namespace", "name":"external", "description":"client tools", "tools":[{"type":"custom", "name":"exec", "format":{"type":"text"}}]},
        {"type":"web_search", "external_web_access":false, "search_content_types":["text", "image"]}
    ]});
    let parsed = InferenceTools::parse(request.as_object().unwrap()).unwrap();
    assert_eq!(
        serde_json::to_value(parsed.native).unwrap(),
        json!([
            {"type":"namespace", "name":"external", "description":"client tools", "tools":[{"type":"custom", "name":"exec", "description":"", "format":{"type":"text"}}]},
            {"type":"web_search", "external_web_access":false, "search_content_types":["text", "image"]}
        ])
    );
}

#[test]
fn built_in_search_choice_needs_no_function_name() {
    let request = json!({"tools":[{"type":"web_search", "external_web_access":false}], "tool_choice":{"type":"web_search"}});
    let parsed = InferenceTools::parse(request.as_object().unwrap()).unwrap();
    assert_eq!(parsed.choice, Some(json!({"type":"web_search"})));
}

#[test]
fn hosted_image_generation_and_tool_search_use_native_serialization() {
    let request = json!({"tools":[
        {"type":"image_generation","output_format":"png","future_option":{"explicit":false}},
        {"type":"tool_search","execution":"client","description":"Search tools","parameters":{"type":"object"}}
    ],"tool_choice":{"type":"image_generation"}});
    let parsed =
        InferenceTools::parse(request.as_object().expect("request")).expect("native tools");
    assert_eq!(
        serde_json::to_value(&parsed.native).expect("serialize"),
        request["tools"]
    );
    assert_eq!(
        codex_tools::create_tools_json_for_responses_lite(&parsed.native).expect("lite tools"),
        request["tools"].as_array().expect("tools").clone()
    );
    assert_eq!(parsed.choice, Some(json!({"type":"image_generation"})));
}
