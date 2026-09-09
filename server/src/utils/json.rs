//! JSON utility functions

use serde_json::Value as JsonValue;
use std::hash::{Hash, Hasher};

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

/// Hash a JSON value into a hasher, with fallback for serialization failures.
///
/// Serializes the JSON value to a string and hashes it. If serialization fails
/// (extremely rare), hashes a fallback marker plus the JSON type discriminant
/// to maintain some differentiation and avoid silent collisions.
///
/// # Example
///
/// ```
/// use std::collections::hash_map::DefaultHasher;
/// use std::hash::Hasher;
/// use serde_json::json;
/// use sideseat_server::utils::json::hash_json_value;
///
/// let mut hasher = DefaultHasher::new();
/// hash_json_value(&mut hasher, &json!({"key": "value"}));
/// let hash = hasher.finish();
/// assert!(hash != 0);
/// ```
#[inline]
pub fn hash_json_value<H: Hasher>(hasher: &mut H, value: &JsonValue) {
    match serde_json::to_string(value) {
        Ok(s) => s.hash(hasher),
        Err(_) => {
            // Fallback: hash the JSON type to maintain some differentiation
            // This is extremely rare - serde_json::to_string rarely fails
            "__json_serialization_failed__".hash(hasher);
            // Hash the type discriminant for minimal differentiation
            std::mem::discriminant(value).hash(hasher);
        }
    }
}

/// Parse any *string* element of an array as JSON, leaving everything else alone.
///
/// An OTLP array attribute whose elements are each a serialised object arrives as an array of strings, because
/// an attribute value cannot nest. The encoding is OTLP's, so undoing it is a generic capability rather than a
/// producer's quirk.
///
/// **`None` when the payload is not an array.** The mode names an array, and a top-level object used to be
/// returned unchanged - so a rule declaring this mode silently accepted a shape it does not describe, and what
/// it then emitted depended on a payload the declaration had ruled out.
///
/// The `usize` counts elements that were strings and **did not parse**. They are *kept* as the strings they
/// are, which is deliberate and now stated rather than implicit: dropping them silently shortens a tool list,
/// and refusing the whole carrier loses the elements that did parse. The count exists so the caller can say
/// so, because a retained element is a producer defect and the previous behaviour reported it nowhere.
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
    use std::collections::hash_map::DefaultHasher;

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

    // ========================================================================
    // hash_json_value tests
    // ========================================================================

    #[test]
    fn test_hash_json_value_same_value_same_hash() {
        let value = json!({"key": "value"});

        let mut hasher1 = DefaultHasher::new();
        hash_json_value(&mut hasher1, &value);
        let hash1 = hasher1.finish();

        let mut hasher2 = DefaultHasher::new();
        hash_json_value(&mut hasher2, &value);
        let hash2 = hasher2.finish();

        assert_eq!(hash1, hash2, "Same JSON value should produce same hash");
    }

    #[test]
    fn test_hash_json_value_different_values_different_hash() {
        let value1 = json!({"key": "value1"});
        let value2 = json!({"key": "value2"});

        let mut hasher1 = DefaultHasher::new();
        hash_json_value(&mut hasher1, &value1);
        let hash1 = hasher1.finish();

        let mut hasher2 = DefaultHasher::new();
        hash_json_value(&mut hasher2, &value2);
        let hash2 = hasher2.finish();

        assert_ne!(
            hash1, hash2,
            "Different JSON values should produce different hashes"
        );
    }

    #[test]
    fn test_hash_json_value_null() {
        let value = JsonValue::Null;

        let mut hasher = DefaultHasher::new();
        hash_json_value(&mut hasher, &value);
        let hash = hasher.finish();

        // Just verify it doesn't panic and produces a non-zero hash
        assert_ne!(hash, 0);
    }

    #[test]
    fn test_hash_json_value_array() {
        let value = json!([1, 2, 3]);

        let mut hasher = DefaultHasher::new();
        hash_json_value(&mut hasher, &value);
        let hash = hasher.finish();

        assert_ne!(hash, 0);
    }

    #[test]
    fn test_hash_json_value_nested_object() {
        let value = json!({"outer": {"inner": "value"}});

        let mut hasher = DefaultHasher::new();
        hash_json_value(&mut hasher, &value);
        let hash = hasher.finish();

        assert_ne!(hash, 0);
    }

    /// The mode names an **array**, and it now refuses anything else.
    ///
    /// A top-level object was returned unchanged, so a rule declaring `stringified_array` silently accepted a
    /// shape its own declaration rules out - and what it then emitted depended on a payload the mode had
    /// excluded. A retained element is kept and counted rather than dropped or fatal: dropping shortens a tool
    /// list silently, refusing the carrier loses the elements that did parse.
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
