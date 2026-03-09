use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::source::VfsExtension;
use super::types::{
    ArtifactSetId, CanvasId, ConversationId, NodeId, StorageRef, UserId, now_micros,
};
use super::storage::{HistoryStorage, ListCanvasItemsParams};
use super::HistoryExtension;

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
// CanvasExtension
// ---------------------------------------------------------------------------

/// Extension that adds canvas (infinite whiteboard) capabilities to a
/// `History`. Stateless — all data lives in `HistoryStorage`.
/// Register with `History::with_extension(Arc::new(CanvasExtension))`.
pub struct CanvasExtension;

impl HistoryExtension for CanvasExtension {
    fn id(&self) -> &str {
        "canvas"
    }
}

impl CanvasExtension {
    pub async fn create(
        &self,
        storage: &impl HistoryStorage,
        conversation_id: ConversationId,
        name: impl Into<String>,
    ) -> Result<CanvasId, super::error::HistoryError> {
        let canvas = Canvas::new(conversation_id, name);
        let id = canvas.id.clone();
        storage.save_canvas(&canvas).await?;
        Ok(id)
    }

    pub async fn get(
        &self,
        storage: &impl HistoryStorage,
        id: &CanvasId,
    ) -> Result<Option<Canvas>, super::error::HistoryError> {
        storage.get_canvas(id).await
    }

    pub async fn upsert_item(
        &self,
        storage: &impl HistoryStorage,
        item: &CanvasItem,
    ) -> Result<(), super::error::HistoryError> {
        storage.upsert_canvas_item(item).await
    }

    pub async fn delete_item(
        &self,
        storage: &impl HistoryStorage,
        item_id: &str,
    ) -> Result<(), super::error::HistoryError> {
        storage.delete_canvas_item(item_id).await
    }

    pub async fn list_items(
        &self,
        storage: &impl HistoryStorage,
        params: &ListCanvasItemsParams,
    ) -> Result<Vec<CanvasItem>, super::error::HistoryError> {
        storage.list_canvas_items(params).await
    }

    // -----------------------------------------------------------------------
    // CRDT-backed operations — store canvas item state in VFS CRDT map
    //
    // VFS path: `canvas/{canvas_id}/items`
    // Each entry is keyed by `item.id` and serialized as JSON.
    // These methods work with the embedded CrdtDoc in VfsExtension and are
    // suitable for real-time collaborative canvases. Existing storage-backed
    // methods remain for persistence and query.
    // -----------------------------------------------------------------------

    fn crdt_path(canvas_id: &CanvasId) -> String {
        format!("canvas/{}/items", canvas_id.as_str())
    }

    /// Write `item` into the VFS CRDT map for this canvas.
    /// Returns the Yjs v1 delta encoding this change.
    pub fn crdt_upsert_item(&self, vfs: &VfsExtension, item: &CanvasItem) -> Vec<u8> {
        let path = Self::crdt_path(&item.canvas_id);
        let value = serde_json::to_value(item).unwrap_or_default();
        vfs.crdt_map_set(&path, &item.id, &value)
    }

    /// Remove the item with `item_id` from the VFS CRDT map for `canvas_id`.
    /// Returns the Yjs v1 delta encoding this change.
    pub fn crdt_delete_item(
        &self,
        vfs: &VfsExtension,
        canvas_id: &CanvasId,
        item_id: &str,
    ) -> Vec<u8> {
        let path = Self::crdt_path(canvas_id);
        vfs.crdt_map_delete(&path, item_id)
    }

    /// Return all items currently in the VFS CRDT map for `canvas_id`.
    pub fn crdt_list_items(&self, vfs: &VfsExtension, canvas_id: &CanvasId) -> Vec<CanvasItem> {
        let path = Self::crdt_path(canvas_id);
        vfs.crdt_map_entries(&path)
            .into_values()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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
    fn crdt_canvas_item_round_trip() {
        use super::super::source::VfsExtension;

        let vfs = VfsExtension::new();
        let ext = CanvasExtension;
        let canvas_id = CanvasId::new();

        let item = CanvasItem {
            id: "item-1".into(),
            canvas_id: canvas_id.clone(),
            item_type: CanvasItemType::Sticky,
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 80.0,
            z_index: 1,
            rotation: 0.0,
            parent_item_id: None,
            content: CanvasItemContent::RichText {
                text: "hello".into(),
                format: None,
            },
            style: Value::Null,
            created_at: now_micros(),
            created_by: None,
            version: 0,
            locked: false,
            deleted: false,
        };

        ext.crdt_upsert_item(&vfs, &item);

        let items = ext.crdt_list_items(&vfs, &canvas_id);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "item-1");

        ext.crdt_delete_item(&vfs, &canvas_id, "item-1");
        let items = ext.crdt_list_items(&vfs, &canvas_id);
        assert!(items.is_empty());
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
