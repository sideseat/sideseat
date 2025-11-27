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
