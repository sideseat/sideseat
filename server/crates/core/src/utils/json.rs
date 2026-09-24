//! JSON utility functions

use serde_json::Value as JsonValue;

/// Converts a JsonValue to Option<String>, returning None for null values.
///
/// This prevents serializing `JsonValue::Null` as the string `"null"`,
/// which would be stored as a VARCHAR instead of a database NULL.
pub fn json_to_opt_string(value: &JsonValue) -> Option<String> {
    if value.is_null() {
        None
    } else {
        serde_json::to_string(value).ok()
    }
}

/// Parse any *string* element of an array as JSON, leaving everything else alone.
///
/// An OTLP array attribute whose elements are each a serialised object arrives as an array of strings, because
/// an attribute value cannot nest. The encoding is OTLP's, so undoing it is a generic capability rather than a
/// producer's quirk.
///
/// Returns `None` when the payload is not an array, because the transformation's declared input shape is part
/// of the rule contract.
///
/// The count reports string elements that did not parse. Such elements remain unchanged so one malformed
/// entry neither shortens the array nor discards valid siblings; callers can surface the producer defect.
pub fn parse_stringified_array_elements(
    value: serde_json::Value,
) -> Option<(serde_json::Value, usize)> {
    let serde_json::Value::Array(items) = value else {
        return None;
    };
    let mut unparsed = 0;
    let elements = items
        .into_iter()
        .map(|item| match &item {
            serde_json::Value::String(text) => match serde_json::from_str(text) {
                Ok(parsed) => parsed,
                Err(_) => {
                    unparsed += 1;
                    item
                }
            },
            _ => item,
        })
        .collect();
    Some((serde_json::Value::Array(elements), unparsed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_null_returns_none() {
        assert_eq!(json_to_opt_string(&JsonValue::Null), None);
    }

    #[test]
    fn test_object_returns_json_string() {
        let value = json!({"key": "value"});
        assert_eq!(
            json_to_opt_string(&value),
            Some(r#"{"key":"value"}"#.to_string())
        );
    }

    #[test]
    fn test_array_returns_json_string() {
        let value = json!([1, 2, 3]);
        assert_eq!(json_to_opt_string(&value), Some("[1,2,3]".to_string()));
    }

    #[test]
    fn test_empty_object_returns_json_string() {
        let value = json!({});
        assert_eq!(json_to_opt_string(&value), Some("{}".to_string()));
    }

    #[test]
    fn test_string_returns_json_string() {
        let value = json!("hello");
        assert_eq!(json_to_opt_string(&value), Some(r#""hello""#.to_string()));
    }

    /// The transformation accepts only arrays and reports malformed string elements without dropping them.
    #[test]
    fn a_stringified_array_is_an_array_and_says_what_it_could_not_parse() {
        // Each element serialised: the ordinary case.
        let (value, unparsed) =
            parse_stringified_array_elements(json!([r#"{"name":"a"}"#, r#"{"name":"b"}"#]))
                .expect("an array is an array");
        assert_eq!(unparsed, 0);
        assert_eq!(value, json!([{"name": "a"}, {"name": "b"}]));

        // A mixture, which is what the mode is for - an element that is already an object stays one.
        let (value, unparsed) =
            parse_stringified_array_elements(json!([r#"{"name":"a"}"#, {"name": "b"}]))
                .expect("an array is an array");
        assert_eq!(unparsed, 0);
        assert_eq!(value, json!([{"name": "a"}, {"name": "b"}]));

        // An element that does not parse is **kept** as the string it is, and counted so the caller can say so.
        let (value, unparsed) =
            parse_stringified_array_elements(json!([r#"{"name":"ok"}"#, "{bad"]))
                .expect("an array is an array");
        assert_eq!(
            unparsed, 1,
            "the retained element is reported, not silently absorbed"
        );
        assert_eq!(value, json!([{"name": "ok"}, "{bad"]));

        // Not an array: refused, whatever it holds.
        assert!(parse_stringified_array_elements(json!({"name": "a"})).is_none());
        assert!(parse_stringified_array_elements(json!("a string")).is_none());
        assert!(parse_stringified_array_elements(json!(7)).is_none());

        // An empty array is an array - zero tools is a statement a producer can make.
        let (value, unparsed) =
            parse_stringified_array_elements(json!([])).expect("empty is an array");
        assert_eq!(unparsed, 0);
        assert_eq!(value, json!([]));
    }
}
