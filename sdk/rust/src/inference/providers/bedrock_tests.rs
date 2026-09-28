use super::*;
use serde_json::json;

#[test]
fn json_to_document_round_trip() {
    let original = json!({
        "string": "hello",
        "number": 42,
        "float": std::f64::consts::PI,
        "bool": true,
        "null": null,
        "array": [1, 2, 3],
        "nested": {"key": "value"}
    });
    let doc = json_to_document(&original);
    let back = document_to_json(&doc);
    assert_eq!(original, back);
}

#[test]
fn json_to_document_negative_int() {
    let val = json!(-5);
    let doc = json_to_document(&val);
    let back = document_to_json(&doc);
    assert_eq!(val, back);
}
