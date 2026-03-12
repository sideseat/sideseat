//! Serde helper deserializers

use serde::Deserialize;

/// Deserialize `Option<Option<String>>` where:
/// - absent from JSON → `None` (requires `#[serde(default)]`)
/// - `null` → `Some(None)` (clear the field)
/// - `"value"` → `Some(Some("value"))` (update the field)
pub fn double_option_string<'de, D>(d: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(d).map(Some)
}

/// Deserialize `Option<Option<serde_json::Value>>` where:
/// - absent from JSON → `None` (requires `#[serde(default)]`)
/// - `null` → `Some(None)` (clear the field)
/// - `{...}` → `Some(Some({...}))` (update the field)
pub fn double_option_value<'de, D>(d: D) -> Result<Option<Option<serde_json::Value>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<serde_json::Value>::deserialize(d).map(Some)
}
