use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::crdt::CrdtExtension;
use super::types::{ArtifactSetId, CanvasId, ConversationId, NodeId, StorageRef, UserId, now_micros};

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
    /// Tombstone flag — items with `deleted: true` are filtered from listings.
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
    RichText { text: String, format: Option<String> },
    Media { storage_ref: StorageRef, mime_type: String },
    ArtifactPreview { artifact_set_id: ArtifactSetId, version: u32 },
    ChatRef { node_id: NodeId },
    Connector { from_item_id: String, to_item_id: String, label: Option<String> },
    Embed { url: String },
    Shape { shape_type: String },
    Unknown { data: Value },
}

// ---------------------------------------------------------------------------
// Viewport
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

/// Stateless extension for canvas operations.
/// All state is stored via [`CrdtExtension`] — one named map per canvas.
pub struct CanvasExtension;

impl super::ContextExtension for CanvasExtension {
    fn id(&self) -> &str {
        "canvas"
    }
}

impl CanvasExtension {
    fn items_map(canvas_id: &CanvasId) -> String {
        format!("canvas:{}:items", canvas_id.as_str())
    }

    fn meta_map(canvas_id: &CanvasId) -> String {
        format!("canvas:{}:meta", canvas_id.as_str())
    }

    /// Upsert an item into the CRDT map for `canvas_id`.
    pub fn upsert_item(
        &self,
        crdt: &CrdtExtension,
        item: &CanvasItem,
    ) -> Result<(), super::error::CmError> {
        let json = serde_json::to_string(item)
            .map_err(|e| super::error::CmError::Serialization(e.to_string()))?;
        crdt.map_set(&Self::items_map(&item.canvas_id), &item.id, &json);
        Ok(())
    }

    /// Tombstone-delete an item (sets `deleted: true`).
    ///
    /// CRDT map deletions re-emerge on merge; tombstones are permanent.
    pub fn remove_item(&self, crdt: &CrdtExtension, canvas_id: &CanvasId, item_id: &str) {
        // Build a minimal tombstone — only fields needed for filtering.
        let tombstone = serde_json::json!({
            "id": item_id,
            "canvas_id": canvas_id.as_str(),
            "deleted": true,
        });
        crdt.map_set(&Self::items_map(canvas_id), item_id, &tombstone.to_string());
    }

    /// List all non-deleted items, optionally filtered by `viewport`.
    pub fn list_items(
        &self,
        crdt: &CrdtExtension,
        canvas_id: &CanvasId,
        viewport: Option<&Viewport>,
    ) -> Vec<CanvasItem> {
        let entries = crdt.map_entries(&Self::items_map(canvas_id));
        let mut items: Vec<CanvasItem> = entries
            .into_values()
            .filter_map(|json| serde_json::from_str::<CanvasItem>(&json).ok())
            .filter(|item| !item.deleted)
            .collect();

        if let Some(vp) = viewport {
            items.retain(|item| vp.contains_item(item));
        }

        items
    }

    /// Set a canvas-level metadata field.
    pub fn set_meta(&self, crdt: &CrdtExtension, canvas_id: &CanvasId, key: &str, value: &str) {
        crdt.map_set(&Self::meta_map(canvas_id), key, value);
    }

    /// Get a canvas-level metadata field.
    pub fn get_meta(
        &self,
        crdt: &CrdtExtension,
        canvas_id: &CanvasId,
        key: &str,
    ) -> Option<String> {
        crdt.map_get(&Self::meta_map(canvas_id), key)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::crdt::CrdtExtension;

    fn make_item(canvas_id: &CanvasId, id: &str, x: f64, y: f64) -> CanvasItem {
        CanvasItem {
            id: id.into(),
            canvas_id: canvas_id.clone(),
            item_type: CanvasItemType::Sticky,
            x,
            y,
            width: 100.0,
            height: 80.0,
            z_index: 0,
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
        }
    }

    #[test]
    fn upsert_and_list() {
        let crdt = CrdtExtension::new("c");
        let ext = CanvasExtension;
        let canvas_id = CanvasId::new();

        let item = make_item(&canvas_id, "item-1", 10.0, 20.0);
        ext.upsert_item(&crdt, &item).unwrap();

        let items = ext.list_items(&crdt, &canvas_id, None);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "item-1");
    }

    #[test]
    fn tombstone_removal() {
        let crdt = CrdtExtension::new("c");
        let ext = CanvasExtension;
        let canvas_id = CanvasId::new();

        ext.upsert_item(&crdt, &make_item(&canvas_id, "item-1", 0.0, 0.0)).unwrap();
        ext.remove_item(&crdt, &canvas_id, "item-1");

        let items = ext.list_items(&crdt, &canvas_id, None);
        assert!(items.is_empty(), "tombstoned item must be filtered");
    }

    #[test]
    fn upsert_idempotent() {
        let crdt = CrdtExtension::new("c");
        let ext = CanvasExtension;
        let canvas_id = CanvasId::new();

        ext.upsert_item(&crdt, &make_item(&canvas_id, "item-1", 0.0, 0.0)).unwrap();
        ext.upsert_item(&crdt, &make_item(&canvas_id, "item-1", 5.0, 5.0)).unwrap(); // overwrite

        let items = ext.list_items(&crdt, &canvas_id, None);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].x, 5.0);
    }

    #[test]
    fn viewport_filter() {
        let crdt = CrdtExtension::new("c");
        let ext = CanvasExtension;
        let canvas_id = CanvasId::new();

        ext.upsert_item(&crdt, &make_item(&canvas_id, "visible", 50.0, 50.0)).unwrap();
        ext.upsert_item(&crdt, &make_item(&canvas_id, "outside", 2000.0, 2000.0)).unwrap();

        let vp = Viewport { x: 0.0, y: 0.0, width: 500.0, height: 500.0 };
        let items = ext.list_items(&crdt, &canvas_id, Some(&vp));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "visible");
    }

    #[test]
    fn meta_set_get() {
        let crdt = CrdtExtension::new("c");
        let ext = CanvasExtension;
        let canvas_id = CanvasId::new();

        ext.set_meta(&crdt, &canvas_id, "zoom", "1.5");
        assert_eq!(ext.get_meta(&crdt, &canvas_id, "zoom"), Some("1.5".into()));
        assert_eq!(ext.get_meta(&crdt, &canvas_id, "missing"), None);
    }

    #[test]
    fn viewport_contains_item_edges() {
        let vp = Viewport { x: 0.0, y: 0.0, width: 1000.0, height: 1000.0 };
        let canvas_id = CanvasId::new();

        let inside = make_item(&canvas_id, "i", 100.0, 100.0);
        let outside = CanvasItem { x: 2000.0, y: 2000.0, ..make_item(&canvas_id, "o", 0.0, 0.0) };

        assert!(vp.contains_item(&inside));
        assert!(!vp.contains_item(&outside));
    }

    #[tokio::test]
    async fn push_pull_round_trip() {
        use crate::context::backend::InMemoryContextBackend;
        use crate::context::crdt::CrdtExtension;

        let backend = std::sync::Arc::new(InMemoryContextBackend::new());
        let conv = ConversationId::new();
        let branch = crate::context::types::BranchId::new();

        // Writer: upsert an item and push.
        let writer_crdt = CrdtExtension::new("writer");
        let canvas_id = CanvasId::new();
        let ext = CanvasExtension;
        ext.upsert_item(&writer_crdt, &make_item(&canvas_id, "sync-item", 5.0, 5.0)).unwrap();
        writer_crdt.push(&conv, &branch, backend.as_ref()).await.unwrap();

        // Reader: pull and verify item is visible.
        let reader_crdt = CrdtExtension::new("reader");
        reader_crdt.pull(&conv, &branch, backend.as_ref()).await.unwrap();

        let items = ext.list_items(&reader_crdt, &canvas_id, None);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "sync-item");
    }
}
