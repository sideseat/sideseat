//! The key a request and its detached frame are joined by (`CarrierRule::frames_requests`), read from each.
//!
//! One renderer for both sides, over the telemetry's own value types, so equal values are equal keys: a request
//! span's attribute and a frame record's attribute or body member. The general attribute renderings differ
//! between a span and a log record for structured values - a list is JSON of its items' text on one, JSON of
//! the items on the other - so they are not what a key is read through. A key is a scalar that names something:
//! text, an integer or a boolean. Anything else, and a blank text, is no key, on either side.

use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};

/// The key a value states, or none.
pub(crate) fn key_text(value: &AnyValue) -> Option<String> {
    match value.value.as_ref()? {
        any_value::Value::StringValue(text) => (!text.trim().is_empty()).then(|| text.clone()),
        any_value::Value::IntValue(number) => Some(number.to_string()),
        any_value::Value::BoolValue(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// The key a member of a JSON object written as text states, read as the attribute of the same value would be.
pub(crate) fn json_key_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => (!text.trim().is_empty()).then(|| text.clone()),
        serde_json::Value::Number(number) if number.is_i64() || number.is_u64() => {
            Some(number.to_string())
        }
        serde_json::Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// The key `attribute` states among `attributes`, or none.
pub(crate) fn attribute_key(attributes: &[KeyValue], attribute: &str) -> Option<String> {
    attributes
        .iter()
        .find(|kv| kv.key == attribute)
        .and_then(|kv| kv.value.as_ref())
        .and_then(key_text)
}

/// The key a span is framed by, where a carrier `frames` declares frames requests and the span states it.
pub(crate) fn request_key(
    frames: &sideseat_domain::rules::carrier_rules::RequestFrames,
    span_attributes: &[KeyValue],
) -> Option<String> {
    attribute_key(span_attributes, frames.request_attribute()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(value: any_value::Value) -> AnyValue {
        AnyValue { value: Some(value) }
    }

    /// Text, integers and booleans are keys, rendered alike from an attribute and from JSON text; a blank text,
    /// a number with a fraction, a list, a map and bytes are none, so a structured value never joins by a
    /// rendering one side writes differently.
    #[test]
    fn a_key_is_a_scalar_rendered_alike_wherever_it_is_read() {
        use any_value::Value;
        assert_eq!(
            key_text(&value(Value::StringValue("sp_1".into()))),
            Some("sp_1".into())
        );
        assert_eq!(key_text(&value(Value::IntValue(42))), Some("42".into()));
        assert_eq!(json_key_text(&serde_json::json!(42)), Some("42".into()));
        assert_eq!(
            key_text(&value(Value::BoolValue(true))),
            Some("true".into())
        );
        assert_eq!(json_key_text(&serde_json::json!(true)), Some("true".into()));
        assert_eq!(key_text(&value(Value::StringValue("  ".into()))), None);
        assert_eq!(json_key_text(&serde_json::json!(" ")), None);
        assert_eq!(key_text(&value(Value::DoubleValue(1.0))), None);
        assert_eq!(json_key_text(&serde_json::json!(1.0)), None);
        let list = opentelemetry_proto::tonic::common::v1::ArrayValue {
            values: vec![value(Value::IntValue(42))],
        };
        assert_eq!(key_text(&value(Value::ArrayValue(list))), None);
        assert_eq!(json_key_text(&serde_json::json!([42])), None);
        assert_eq!(
            key_text(&value(Value::KvlistValue(Default::default()))),
            None
        );
        assert_eq!(json_key_text(&serde_json::json!({"a": 1})), None);
        assert_eq!(key_text(&value(Value::BytesValue(vec![1]))), None);
        assert_eq!(key_text(&AnyValue { value: None }), None);
    }
}
