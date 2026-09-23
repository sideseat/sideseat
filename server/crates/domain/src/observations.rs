//! Transport-independent observations produced by telemetry extraction.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// Message content before SideML normalization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawMessage {
    pub source: MessageSource,
    pub content: JsonValue,
}

/// Physical carrier that supplied a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageSource {
    Event { name: String, time: DateTime<Utc> },
    Attribute { key: String, time: DateTime<Utc> },
}

impl RawMessage {
    pub fn from_event(name: &str, time: DateTime<Utc>, content: JsonValue) -> Self {
        Self {
            source: MessageSource::Event {
                name: name.to_string(),
                time,
            },
            content,
        }
    }

    pub fn from_attr(key: &str, time: DateTime<Utc>, content: JsonValue) -> Self {
        Self {
            source: MessageSource::Attribute {
                key: key.to_string(),
                time,
            },
            content,
        }
    }
}

/// Tool definition before normalization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawToolDefinition {
    pub source: ToolDefinitionSource,
    pub content: JsonValue,
}

/// Physical carrier that supplied tool metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolDefinitionSource {
    Attribute { key: String, time: DateTime<Utc> },
}

impl RawToolDefinition {
    pub fn from_attr(key: &str, time: DateTime<Utc>, content: JsonValue) -> Self {
        Self {
            source: ToolDefinitionSource::Attribute {
                key: key.to_string(),
                time,
            },
            content,
        }
    }
}

/// Tool-name list before normalization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawToolNames {
    pub source: ToolDefinitionSource,
    pub content: JsonValue,
}

impl RawToolNames {
    pub fn from_attr(key: &str, time: DateTime<Utc>, content: JsonValue) -> Self {
        Self {
            source: ToolDefinitionSource::Attribute {
                key: key.to_string(),
                time,
            },
            content,
        }
    }
}
