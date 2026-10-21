//! Verify serialization of the supported tool schema representation.

use super::JsonSchema;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn caller_schema_preserves_keywords_and_all_nested_schema_forms() {
    let original = json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": "https://example.com/tool.schema.json",
        "type": "object",
        "properties": {
            "limit": {"type": "number", "default": 3, "minimum": 0, "exclusiveMinimum": 0, "maximum": 20, "exclusiveMaximum": 21, "multipleOf": 0.5},
            "query": {"type": "string", "minLength": 2, "maxLength": 12, "pattern": "^[a-z]+$", "default": "ab"},
            "questions": {"type": "array", "minItems": 1, "maxItems": 4, "uniqueItems": true, "items": {"type": "object", "properties": {"header": {"type": "string", "maxLength": 12}}, "additionalProperties": {"type": "string", "minLength": 1}}},
            "tuple": {"type": "array", "items": [{"const": null}, false, true], "additionalItems": false},
            "never": false,
            "anything": true,
            "choice": {"anyOf": [{"$ref": "#/$defs/item"}, false], "oneOf": [{"const": 1}, {"const": 2}], "allOf": [{"not": {"type": "null"}}]},
            "nullable": {"type": ["string", "null"], "default": null},
            "null_annotation": {"description": null}
        },
        "$defs": {"item": {"type": "string", "pattern": "x"}, "disabled": false},
        "definitions": {"legacy": {"type": "number", "minimum": 1}},
        "required": ["query"],
        "additionalProperties": {"anyOf": [false, {"type": "integer", "minimum": 0}]},
        "if": {"required": ["limit"]},
        "then": {"dependentRequired": {"limit": ["query"]}},
        "else": false,
        "unevaluatedProperties": false,
        "future_keyword": {"nested": [true, null, {"type": "future_type", "constraint": 7}]}
    });
    let schema = JsonSchema::from_raw_schema(original.as_object().unwrap().clone());
    assert_eq!(serde_json::to_value(&schema).unwrap(), original);

    // Native constructors can also embed a complete caller schema as a child.
    let array = JsonSchema::array(schema, None);
    assert_eq!(
        serde_json::to_value(array).unwrap(),
        json!({"type": "array", "items": original})
    );
}

#[test]
fn json_schema_serializes_encrypted_marker() {
    let schema = JsonSchema::string(Some("Secret value".to_string())).with_encrypted();

    assert_eq!(
        serde_json::to_value(schema).expect("serialize schema"),
        serde_json::json!({
            "type": "string",
            "description": "Secret value",
            "encrypted": true,
        })
    );
}
