//! Transport-independent observations produced by telemetry extraction.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// Message content before SideML normalization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawMessage {
    pub source: MessageSource,
    pub content: JsonValue,
    /// A turn the producer re-sent as text that another carrier holds losslessly (a reading's `rendering`):
    /// shown on the span that sent it, left out of the trace and session views. Written only when true, so a
    /// stored message that is not one keeps the bytes it always had.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rendering: bool,
    /// The side of the span this message is on, where the reading that produced it declares one that differs
    /// from its carrier's (`Alternative::direction`). Written only when declared, so every other stored message
    /// keeps the bytes it always had.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<crate::rules::schema::ReadingDirection>,
    /// The part of a streamed response this message is, where its event rule declares a stream
    /// (`MessageRule::stream`). Written only when declared, so every other stored message keeps the bytes it
    /// always had.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<crate::rules::schema::StreamMark>,
}

/// Physical carrier that supplied a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageSource {
    Event { name: String, time: DateTime<Utc> },
    Attribute { key: String, time: DateTime<Utc> },
}

impl RawMessage {
    /// The same message, marked as a rendering where `rendering` holds.
    pub fn rendered(mut self, rendering: bool) -> Self {
        self.rendering = rendering;
        self
    }

    /// The same message, on the side of the span its reading declared, where it declared one.
    pub fn directed(mut self, direction: Option<crate::rules::schema::ReadingDirection>) -> Self {
        self.direction = direction;
        self
    }

    /// The same message, as the part of a streamed response its event rule declared, where it declared one.
    pub fn streamed(mut self, stream: Option<crate::rules::schema::StreamMark>) -> Self {
        self.stream = stream;
        self
    }

    pub fn from_event(name: &str, time: DateTime<Utc>, content: JsonValue) -> Self {
        Self {
            source: MessageSource::Event {
                name: name.to_string(),
                time,
            },
            content,
            rendering: false,
            direction: None,
            stream: None,
        }
    }

    pub fn from_attr(key: &str, time: DateTime<Utc>, content: JsonValue) -> Self {
        Self {
            source: MessageSource::Attribute {
                key: key.to_string(),
                time,
            },
            content,
            rendering: false,
            direction: None,
            stream: None,
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
