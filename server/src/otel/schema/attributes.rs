//! Attribute value types and conversion

use serde::{Deserialize, Serialize};

/// Attribute value types matching OTLP AnyValue
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AttributeValue {
    String(String),
    Int(i64),
    Double(f64),
    Bool(bool),
    Array(Vec<AttributeValue>),
    Map(std::collections::HashMap<String, AttributeValue>),
    Bytes(Vec<u8>),
    Null,
}

impl AttributeValue {
    /// Convert to JSON value
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::String(s) => serde_json::Value::String(s.clone()),
            Self::Int(i) => serde_json::json!(*i),
            Self::Double(d) => serde_json::json!(*d),
            Self::Bool(b) => serde_json::Value::Bool(*b),
            Self::Array(arr) => serde_json::Value::Array(arr.iter().map(|v| v.to_json()).collect()),
            Self::Map(map) => {
                let obj: serde_json::Map<String, serde_json::Value> =
                    map.iter().map(|(k, v)| (k.clone(), v.to_json())).collect();
                serde_json::Value::Object(obj)
            }
            Self::Bytes(b) => {
                use base64::Engine;
                serde_json::Value::String(base64::engine::general_purpose::STANDARD.encode(b))
            }
            Self::Null => serde_json::Value::Null,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_attribute_value_string_to_json() {
        let val = AttributeValue::String("hello".to_string());
        assert_eq!(val.to_json(), serde_json::json!("hello"));
    }

    #[test]
    fn test_attribute_value_int_to_json() {
        let val = AttributeValue::Int(42);
        assert_eq!(val.to_json(), serde_json::json!(42));
    }

    #[test]
    fn test_attribute_value_double_to_json() {
        let val = AttributeValue::Double(2.5);
        assert_eq!(val.to_json(), serde_json::json!(2.5));
    }

    #[test]
    fn test_attribute_value_bool_to_json() {
        let val = AttributeValue::Bool(true);
        assert_eq!(val.to_json(), serde_json::json!(true));
    }

    #[test]
    fn test_attribute_value_null_to_json() {
        let val = AttributeValue::Null;
        assert_eq!(val.to_json(), serde_json::Value::Null);
    }

    #[test]
    fn test_attribute_value_array_to_json() {
        let val = AttributeValue::Array(vec![
            AttributeValue::Int(1),
            AttributeValue::Int(2),
            AttributeValue::String("three".to_string()),
        ]);
        assert_eq!(val.to_json(), serde_json::json!([1, 2, "three"]));
    }

    #[test]
    fn test_attribute_value_map_to_json() {
        let mut map = std::collections::HashMap::new();
        map.insert("key".to_string(), AttributeValue::String("value".to_string()));
        map.insert("num".to_string(), AttributeValue::Int(100));
        let val = AttributeValue::Map(map);
        let json = val.to_json();
        assert_eq!(json["key"], "value");
        assert_eq!(json["num"], 100);
    }

    #[test]
    fn test_attribute_value_bytes_to_json() {
        let val = AttributeValue::Bytes(vec![1, 2, 3, 4]);
        let json = val.to_json();
        // Should be base64 encoded
        assert!(json.is_string());
        let s = json.as_str().unwrap();
        use base64::Engine;
        let decoded = base64::engine::general_purpose::STANDARD.decode(s).unwrap();
        assert_eq!(decoded, vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_attribute_value_serialization() {
        let val = AttributeValue::String("test".to_string());
        let json = serde_json::to_string(&val).unwrap();
        let deserialized: AttributeValue = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, AttributeValue::String(s) if s == "test"));
    }

    #[test]
    fn test_attribute_value_clone() {
        let val = AttributeValue::Int(42);
        let cloned = val.clone();
        assert!(matches!(cloned, AttributeValue::Int(42)));
    }

    #[test]
    fn test_attribute_value_nested_array() {
        let val = AttributeValue::Array(vec![AttributeValue::Array(vec![
            AttributeValue::Int(1),
            AttributeValue::Int(2),
        ])]);
        assert_eq!(val.to_json(), serde_json::json!([[1, 2]]));
    }
}
