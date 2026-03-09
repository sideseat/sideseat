use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::types::{
    ArtifactSetId, CanvasId, ConversationId, NodeId, StorageRef, UserId, now_micros,
};

// ---------------------------------------------------------------------------
// Canvas
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Canvas {
    pub id: CanvasId,
    pub conversation_id: ConversationId,
    pub name: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub version: u64,
}

impl Canvas {
    pub fn new(conversation_id: ConversationId, name: impl Into<String>) -> Self {
        let now = now_micros();
        Self {
            id: CanvasId::new(),
            conversation_id,
            name: name.into(),
            created_at: now,
            updated_at: now,
            version: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// CanvasItem
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanvasItem {
    pub id: String,
    pub canvas_id: CanvasId,
    pub item_type: CanvasItemType,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub z_index: i32,
    pub rotation: f64,
    pub parent_item_id: Option<String>,
    pub content: CanvasItemContent,
    #[serde(default)]
    pub style: Value,
    pub created_at: i64,
    pub created_by: Option<UserId>,
    pub version: u64,
    pub locked: bool,
    pub deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CanvasItemType {
    Sticky,
    Frame,
    Shape,
    Text,
    Image,
    Video,
    Connector,
    Embed,
    ArtifactPreview,
    ChatBubble,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CanvasItemContent {
    RichText {
        text: String,
        format: Option<String>,
    },
    Media {
        storage_ref: StorageRef,
        mime_type: String,
    },
    ArtifactPreview {
        artifact_set_id: ArtifactSetId,
        version: u32,
    },
    ChatRef {
        node_id: NodeId,
    },
    Connector {
        from_item_id: String,
        to_item_id: String,
        label: Option<String>,
    },
    Embed {
        url: String,
    },
    Shape {
        shape_type: String,
    },
    Unknown {
        data: Value,
    },
}

// ---------------------------------------------------------------------------
// Viewport (for spatial queries)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Viewport {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Viewport {
    pub fn contains_item(&self, item: &CanvasItem) -> bool {
        let item_right = item.x + item.width;
        let item_bottom = item.y + item.height;
        let vp_right = self.x + self.width;
        let vp_bottom = self.y + self.height;

        item_right >= self.x && item.x <= vp_right && item_bottom >= self.y && item.y <= vp_bottom
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::types::StorageBackend;

    #[test]
    fn viewport_contains_item() {
        let vp = Viewport {
            x: 0.0,
            y: 0.0,
            width: 1000.0,
            height: 1000.0,
        };

        let item = CanvasItem {
            id: "item1".into(),
            canvas_id: CanvasId::new(),
            item_type: CanvasItemType::Sticky,
            x: 100.0,
            y: 100.0,
            width: 200.0,
            height: 200.0,
            z_index: 0,
            rotation: 0.0,
            parent_item_id: None,
            content: CanvasItemContent::RichText {
                text: "test".into(),
                format: None,
            },
            style: Value::Null,
            created_at: now_micros(),
            created_by: None,
            version: 0,
            locked: false,
            deleted: false,
        };

        assert!(vp.contains_item(&item));

        let outside = CanvasItem {
            x: 2000.0,
            y: 2000.0,
            ..item.clone()
        };
        assert!(!vp.contains_item(&outside));
    }

    #[test]
    fn connector_content() {
        let content = CanvasItemContent::Connector {
            from_item_id: "a".into(),
            to_item_id: "b".into(),
            label: Some("connects".into()),
        };
        let json = serde_json::to_string(&content).unwrap();
        let parsed: CanvasItemContent = serde_json::from_str(&json).unwrap();
        match parsed {
            CanvasItemContent::Connector {
                from_item_id,
                to_item_id,
                label,
            } => {
                assert_eq!(from_item_id, "a");
                assert_eq!(to_item_id, "b");
                assert_eq!(label, Some("connects".into()));
            }
            _ => panic!("Expected Connector"),
        }
    }
}
