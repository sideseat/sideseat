use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::crdt::CrdtExtension;
use super::types::{
    ArtifactSetId, CanvasId, ConversationId, NodeId, StorageRef, UserId, now_micros,
};

// ---------------------------------------------------------------------------
// Canvas
// ---------------------------------------------------------------------------

/// A freeform canvas workspace owned by a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Canvas {
    pub id: CanvasId,
    pub conversation_id: ConversationId,
    pub name: String,
    pub created_at: i64,
    pub updated_at: i64,
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
        }
    }
}

// ---------------------------------------------------------------------------
// CanvasItem
// ---------------------------------------------------------------------------

/// A single item on a canvas (sticky note, shape, image, connector, etc.).
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
    pub locked: bool,
    /// Tombstone flag — items with `deleted: true` are filtered from listings.
    pub deleted: bool,
}

/// Visual type of a [`CanvasItem`].
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

/// Content payload of a [`CanvasItem`] (discriminated by `"type"`).
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
// Per-field CRDT structs (private)
// ---------------------------------------------------------------------------

/// Geometry fields — written by drag/resize operations.
/// z_index is kept in a separate `zgeo` map so concurrent drag and z-reorder
/// operations on the same item don't clobber each other under LWW.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ItemGeo {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    rotation: f64,
}

/// Immutable item metadata: type, parent, creation info.
/// Stored in `canvas:{id}:pimeta` — written once at creation, rarely updated.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ItemPropMeta {
    item_type: CanvasItemType,
    parent_item_id: Option<String>,
    created_at: i64,
    created_by: Option<UserId>,
}

/// Legacy prop blob — kept for backward-compatible reads of items written before
/// the per-field split. New writes use the four per-field maps instead.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ItemProp {
    item_type: CanvasItemType,
    locked: bool,
    deleted: bool,
    parent_item_id: Option<String>,
    #[serde(default)]
    style: Value,
    created_at: i64,
    created_by: Option<UserId>,
}

// ---------------------------------------------------------------------------
// Viewport
// ---------------------------------------------------------------------------

/// Visible rectangle on the canvas used for spatial filtering.
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
///
/// Canvas state is split across three namespaced CRDT maps per canvas, enabling
/// independent concurrent edits to geometry, metadata, and content without conflict:
///
/// - `canvas:{id}:geo`   — position/size (drag/resize only)
/// - `canvas:{id}:prop`  — type/lock/delete/style (meta edits)
/// - `canvas:{id}:cnt`   — content blob (text/media edits)
///
/// Legacy data in `canvas:{id}:items` is included as a fallback for backward
/// compatibility. Call [`migrate_canvas_v1_to_v2`] to split it into the new maps.
pub struct CanvasExtension;

impl super::ContextExtension for CanvasExtension {
    fn id(&self) -> &str {
        "canvas"
    }
}

impl CanvasExtension {
    fn geo_map(canvas_id: &CanvasId) -> String {
        format!("canvas:{}:geo", canvas_id.as_str())
    }

    /// Stacking-order map: key = item_id, value = JSON-encoded `i32` z_index.
    /// Kept separate from `geo` so "bring to front" and drag are independent LWW writes.
    fn zgeo_map(canvas_id: &CanvasId) -> String {
        format!("canvas:{}:zgeo", canvas_id.as_str())
    }

    // Per-field prop maps (C1 fix: each mutable field in its own map entry so
    // concurrent edits to different fields don't clobber each other LWW-style).
    fn prop_imeta_map(canvas_id: &CanvasId) -> String {
        format!("canvas:{}:pimeta", canvas_id.as_str())
    }

    fn prop_locked_map(canvas_id: &CanvasId) -> String {
        format!("canvas:{}:plck", canvas_id.as_str())
    }

    fn prop_deleted_map(canvas_id: &CanvasId) -> String {
        format!("canvas:{}:pdel", canvas_id.as_str())
    }

    fn prop_style_map(canvas_id: &CanvasId) -> String {
        format!("canvas:{}:psty", canvas_id.as_str())
    }

    /// Legacy single-blob prop map — read-only for backward compatibility.
    fn prop_map(canvas_id: &CanvasId) -> String {
        format!("canvas:{}:prop", canvas_id.as_str())
    }

    fn cnt_map(canvas_id: &CanvasId) -> String {
        format!("canvas:{}:cnt", canvas_id.as_str())
    }

    fn legacy_items_map(canvas_id: &CanvasId) -> String {
        format!("canvas:{}:items", canvas_id.as_str())
    }

    fn meta_map(canvas_id: &CanvasId) -> String {
        format!("canvas:{}:meta", canvas_id.as_str())
    }

    /// Y.Text key for the rich text body of a specific item.
    ///
    /// Each `RichText` item's text is stored as a separate `Y.Text` CRDT so that
    /// concurrent character-level edits on the same sticky note converge correctly
    /// (Miro-like collaboration) rather than clobbering each other LWW-style.
    fn rich_text_key(canvas_id: &CanvasId, item_id: &str) -> String {
        format!("canvas:{}:{}:richtext", canvas_id.as_str(), item_id)
    }

    // -----------------------------------------------------------------------
    // Targeted write methods (Miro-style per-field granularity)
    // -----------------------------------------------------------------------

    /// Update only the position/size/rotation of an item (drag/resize).
    /// Concurrent content edits and z-order changes on the same item are preserved.
    /// To change only the stacking order use [`set_z_index`].
    #[allow(clippy::too_many_arguments)]
    pub fn move_item(
        &self,
        crdt: &CrdtExtension,
        canvas_id: &CanvasId,
        item_id: &str,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        rotation: f64,
    ) -> Result<(), super::error::CmError> {
        let geo = ItemGeo {
            x,
            y,
            width,
            height,
            rotation,
        };
        let json = serde_json::to_string(&geo)
            .map_err(|e| super::error::CmError::Serialization(e.to_string()))?;
        crdt.map_set(&Self::geo_map(canvas_id), item_id, &json);
        Ok(())
    }

    /// Update only the stacking order of an item ("bring to front" / "send to back").
    ///
    /// Written to the separate `zgeo` map so concurrent drag operations (which write
    /// only the `geo` map) are preserved — the two operations target different CRDT
    /// entries and both survive merge.
    pub fn set_z_index(
        &self,
        crdt: &CrdtExtension,
        canvas_id: &CanvasId,
        item_id: &str,
        z_index: i32,
    ) -> Result<(), super::error::CmError> {
        let json = serde_json::to_string(&z_index)
            .map_err(|e| super::error::CmError::Serialization(e.to_string()))?;
        crdt.map_set(&Self::zgeo_map(canvas_id), item_id, &json);
        Ok(())
    }

    /// Update only the prop fields of an item (lock/delete/style).
    /// Concurrent geometry and content edits on the same item are preserved.
    ///
    /// Each mutable field is written to its own CRDT map entry so concurrent edits
    /// to different fields (e.g. lock vs style change) both survive merge (C1 fix).
    #[allow(clippy::too_many_arguments)]
    pub fn patch_item_props(
        &self,
        crdt: &CrdtExtension,
        canvas_id: &CanvasId,
        item_id: &str,
        item_type: CanvasItemType,
        locked: bool,
        deleted: bool,
        parent_item_id: Option<String>,
        style: Value,
        created_at: i64,
        created_by: Option<UserId>,
    ) -> Result<(), super::error::CmError> {
        let imeta = ItemPropMeta {
            item_type,
            parent_item_id,
            created_at,
            created_by,
        };
        let imeta_json = serde_json::to_string(&imeta)
            .map_err(|e| super::error::CmError::Serialization(e.to_string()))?;
        crdt.map_set(&Self::prop_imeta_map(canvas_id), item_id, &imeta_json);
        crdt.map_set(
            &Self::prop_locked_map(canvas_id),
            item_id,
            if locked { "true" } else { "false" },
        );
        crdt.map_set(
            &Self::prop_deleted_map(canvas_id),
            item_id,
            if deleted { "true" } else { "false" },
        );
        // Per-property style writes so concurrent field edits (e.g. fill vs stroke) don't clobber each other.
        Self::write_style_props(crdt, canvas_id, item_id, &style);
        Ok(())
    }

    /// Update only the content of an item (text/media edits).
    /// Concurrent geometry and prop edits on the same item are preserved.
    ///
    /// For `RichText` items the text body is stored in a `Y.Text` CRDT so that
    /// concurrent character-level edits on the same item converge. The `cnt` map
    /// entry stores only the format metadata (type tag + format field). Use
    /// [`insert_rich_text`] / [`remove_rich_text`] for incremental edits; this
    /// method performs a full-replace (delete-all then insert).
    ///
    /// **Tombstone note**: the full-replace pattern accumulates yrs tombstones in the
    /// Y.Text structure between GC passes. yrs GC runs during state encoding
    /// (`encode_diff_v1` with `skip_gc: false`), so tombstones are pruned on the next
    /// snapshot save. For real-time collaborative editing prefer [`insert_rich_text`] /
    /// [`remove_rich_text`] which emit precise character-level ops with no tombstones.
    pub fn patch_item_content(
        &self,
        crdt: &CrdtExtension,
        canvas_id: &CanvasId,
        item_id: &str,
        content: &CanvasItemContent,
    ) -> Result<(), super::error::CmError> {
        if let CanvasItemContent::RichText { text, format } = content {
            // Store type tag + format in the cnt map (no text — text lives in Y.Text).
            let meta = serde_json::json!({ "type": "rich_text", "format": format });
            crdt.map_set(&Self::cnt_map(canvas_id), item_id, &meta.to_string());
            // Full-replace Y.Text: delete existing content, insert new.
            let key = Self::rich_text_key(canvas_id, item_id);
            let current_len = crdt.text_len(&key);
            if current_len > 0 {
                crdt.text_remove(&key, 0, current_len);
            }
            crdt.text_insert(&key, 0, text);
        } else {
            let json = serde_json::to_string(content)
                .map_err(|e| super::error::CmError::Serialization(e.to_string()))?;
            crdt.map_set(&Self::cnt_map(canvas_id), item_id, &json);
        }
        Ok(())
    }

    /// Insert text into a `RichText` item at `index` (character-level CRDT op).
    ///
    /// Concurrent inserts at different positions both survive merge.
    /// Concurrent inserts at the same position are ordered by yrs Lamport clock.
    pub fn insert_rich_text(
        &self,
        crdt: &CrdtExtension,
        canvas_id: &CanvasId,
        item_id: &str,
        index: u32,
        text: &str,
    ) {
        crdt.text_insert(&Self::rich_text_key(canvas_id, item_id), index, text);
    }

    /// Remove `len` characters from a `RichText` item starting at `index`.
    pub fn remove_rich_text(
        &self,
        crdt: &CrdtExtension,
        canvas_id: &CanvasId,
        item_id: &str,
        index: u32,
        len: u32,
    ) {
        crdt.text_remove(&Self::rich_text_key(canvas_id, item_id), index, len);
    }

    // -----------------------------------------------------------------------
    // Full item upsert (convenience — writes all three maps)
    // -----------------------------------------------------------------------

    /// Upsert an item into all per-field CRDT maps for `canvas_id`.
    pub fn upsert_item(
        &self,
        crdt: &CrdtExtension,
        item: &CanvasItem,
    ) -> Result<(), super::error::CmError> {
        let geo = ItemGeo {
            x: item.x,
            y: item.y,
            width: item.width,
            height: item.height,
            rotation: item.rotation,
        };
        let imeta = ItemPropMeta {
            item_type: item.item_type.clone(),
            parent_item_id: item.parent_item_id.clone(),
            created_at: item.created_at,
            created_by: item.created_by.clone(),
        };

        let geo_json = serde_json::to_string(&geo)
            .map_err(|e| super::error::CmError::Serialization(e.to_string()))?;
        let imeta_json = serde_json::to_string(&imeta)
            .map_err(|e| super::error::CmError::Serialization(e.to_string()))?;

        crdt.map_set(&Self::geo_map(&item.canvas_id), &item.id, &geo_json);
        crdt.map_set(
            &Self::zgeo_map(&item.canvas_id),
            &item.id,
            &serde_json::to_string(&item.z_index)
                .map_err(|e| super::error::CmError::Serialization(e.to_string()))?,
        );
        crdt.map_set(
            &Self::prop_imeta_map(&item.canvas_id),
            &item.id,
            &imeta_json,
        );
        crdt.map_set(
            &Self::prop_locked_map(&item.canvas_id),
            &item.id,
            if item.locked { "true" } else { "false" },
        );
        crdt.map_set(
            &Self::prop_deleted_map(&item.canvas_id),
            &item.id,
            if item.deleted { "true" } else { "false" },
        );
        Self::write_style_props(crdt, &item.canvas_id, &item.id, &item.style);
        // Route through patch_item_content so RichText goes into Y.Text.
        self.patch_item_content(crdt, &item.canvas_id, &item.id, &item.content)?;
        Ok(())
    }

    /// Tombstone-delete an item by writing `"true"` to the per-field deleted map only.
    ///
    /// Geometry and content remain in their maps but are ignored since
    /// `list_items` filters on the pdel map. Single-entry write ensures concurrent
    /// geometry/content/style edits are not overwritten.
    pub fn remove_item(&self, crdt: &CrdtExtension, canvas_id: &CanvasId, item_id: &str) {
        crdt.map_set(&Self::prop_deleted_map(canvas_id), item_id, "true");
    }

    // -----------------------------------------------------------------------
    // List (joins three maps + legacy fallback)
    // -----------------------------------------------------------------------

    /// Read item content from the cnt map (and Y.Text for RichText items).
    fn read_content(
        crdt: &CrdtExtension,
        canvas_id: &CanvasId,
        item_id: &str,
        cnt_entries: &std::collections::HashMap<String, String>,
    ) -> CanvasItemContent {
        cnt_entries
            .get(item_id)
            .and_then(|s| {
                let v: Value = serde_json::from_str(s).ok()?;
                if v.get("type").and_then(|t| t.as_str()) == Some("rich_text") {
                    let format = v
                        .get("format")
                        .filter(|f| !f.is_null())
                        .and_then(|f| f.as_str())
                        .map(String::from);
                    let text = crdt.text_read(&Self::rich_text_key(canvas_id, item_id));
                    Some(CanvasItemContent::RichText { text, format })
                } else {
                    serde_json::from_str(s).ok()
                }
            })
            .unwrap_or(CanvasItemContent::Unknown { data: Value::Null })
    }

    /// Write per-property style entries so concurrent edits to different style
    /// properties (e.g. fill vs stroke) converge independently (C1 fix).
    ///
    /// Key format: `{item_id}:{prop_name}` → JSON-encoded property value.
    /// Null / non-object style writes nothing (absence reads back as `Value::Null`).
    fn write_style_props(crdt: &CrdtExtension, canvas_id: &CanvasId, item_id: &str, style: &Value) {
        if let Some(obj) = style.as_object() {
            let map_name = Self::prop_style_map(canvas_id);
            for (prop, val) in obj {
                if let Ok(json) = serde_json::to_string(val) {
                    crdt.map_set(&map_name, &format!("{}:{}", item_id, prop), &json);
                }
            }
        }
    }

    /// Reconstruct a style `Value` from the psty map.
    ///
    /// New format: keys `{item_id}:{prop}` → JSON value (per-property, no LWW conflict).
    /// Legacy format: key `{item_id}` → full style JSON blob (backward compat read).
    fn read_style_from_psty(
        item_id: &str,
        psty_entries: &std::collections::HashMap<String, String>,
    ) -> Value {
        let prefix = format!("{}:", item_id);
        let mut props = serde_json::Map::new();
        for (key, val_json) in psty_entries {
            if let Some(prop) = key.strip_prefix(&prefix)
                && let Ok(v) = serde_json::from_str(val_json)
            {
                props.insert(prop.to_string(), v);
            }
        }
        if !props.is_empty() {
            return Value::Object(props);
        }
        // Fallback: legacy blob (key = item_id, value = full style JSON).
        psty_entries
            .get(item_id)
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(Value::Null)
    }

    /// List all non-deleted items, optionally filtered by `viewport`.
    ///
    /// Reads from the per-field maps (`pimeta`, `plck`, `pdel`, `psty`, `geo`, `cnt`).
    /// Items present only in the legacy `prop` or `items` maps are included as-is
    /// for backward compatibility. Call [`migrate_canvas_v1_to_v2`] to migrate them.
    pub fn list_items(
        &self,
        crdt: &CrdtExtension,
        canvas_id: &CanvasId,
        viewport: Option<&Viewport>,
    ) -> Vec<CanvasItem> {
        // Atomic batch read: all 8 maps in a single lock acquisition to prevent
        // torn reads where a concurrent upsert_item() could change one map between
        // two separate crdt.map_entries() calls.
        let geo_name = Self::geo_map(canvas_id);
        let zgeo_name = Self::zgeo_map(canvas_id);
        let pimeta_name = Self::prop_imeta_map(canvas_id);
        let pdel_name = Self::prop_deleted_map(canvas_id);
        let plck_name = Self::prop_locked_map(canvas_id);
        let psty_name = Self::prop_style_map(canvas_id);
        let cnt_name = Self::cnt_map(canvas_id);
        let legacy_prop_name = Self::prop_map(canvas_id);
        let legacy_items_name = Self::legacy_items_map(canvas_id);
        let [
            geo_entries,
            zgeo_entries,
            pimeta_entries,
            pdel_entries,
            plck_entries,
            psty_entries,
            cnt_entries,
            legacy_prop_entries,
            legacy_items,
        ]: [std::collections::HashMap<String, String>; 9] = crdt
            .map_entries_batch(&[
                &geo_name,
                &zgeo_name,
                &pimeta_name,
                &pdel_name,
                &plck_name,
                &psty_name,
                &cnt_name,
                &legacy_prop_name,
                &legacy_items_name,
            ])
            .try_into()
            .expect("batch length matches name count");

        // Items authored in the per-field format are identified by a pimeta entry.
        let mut items: Vec<CanvasItem> = pimeta_entries
            .iter()
            .filter_map(|(item_id, imeta_json)| {
                let deleted = pdel_entries
                    .get(item_id)
                    .map(|s| s == "true")
                    .unwrap_or(false);
                if deleted {
                    return None;
                }
                let imeta: ItemPropMeta = serde_json::from_str(imeta_json).ok()?;
                let locked = plck_entries
                    .get(item_id)
                    .map(|s| s == "true")
                    .unwrap_or(false);
                let style = Self::read_style_from_psty(item_id, &psty_entries);
                let geo: ItemGeo = geo_entries
                    .get(item_id)
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_default();
                let z_index = zgeo_entries
                    .get(item_id)
                    .and_then(|s| serde_json::from_str::<i32>(s).ok())
                    .unwrap_or(0);
                let content = Self::read_content(crdt, canvas_id, item_id, &cnt_entries);
                Some(CanvasItem {
                    id: item_id.clone(),
                    canvas_id: canvas_id.clone(),
                    item_type: imeta.item_type,
                    x: geo.x,
                    y: geo.y,
                    width: geo.width,
                    height: geo.height,
                    z_index,
                    rotation: geo.rotation,
                    parent_item_id: imeta.parent_item_id,
                    content,
                    style,
                    created_at: imeta.created_at,
                    created_by: imeta.created_by,
                    locked,
                    deleted: false,
                })
            })
            .collect();

        // Dual-read: old single-blob prop entries not yet split into per-field maps.
        for (item_id, prop_json) in &legacy_prop_entries {
            if pimeta_entries.contains_key(item_id) {
                continue; // already in per-field format
            }
            if let Ok(prop) = serde_json::from_str::<ItemProp>(prop_json)
                && !prop.deleted
            {
                let geo: ItemGeo = geo_entries
                    .get(item_id)
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_default();
                let z_index = zgeo_entries
                    .get(item_id)
                    .and_then(|s| serde_json::from_str::<i32>(s).ok())
                    .unwrap_or(0);
                let content = Self::read_content(crdt, canvas_id, item_id, &cnt_entries);
                items.push(CanvasItem {
                    id: item_id.clone(),
                    canvas_id: canvas_id.clone(),
                    item_type: prop.item_type,
                    x: geo.x,
                    y: geo.y,
                    width: geo.width,
                    height: geo.height,
                    z_index,
                    rotation: geo.rotation,
                    parent_item_id: prop.parent_item_id,
                    content,
                    style: prop.style,
                    created_at: prop.created_at,
                    created_by: prop.created_by,
                    locked: prop.locked,
                    deleted: false,
                });
            }
        }

        // Dual-read: v1 single-blob items.
        for (item_id, json) in &legacy_items {
            if pimeta_entries.contains_key(item_id) || legacy_prop_entries.contains_key(item_id) {
                continue;
            }
            if let Ok(item) = serde_json::from_str::<CanvasItem>(json)
                && !item.deleted
            {
                items.push(item);
            }
        }

        if let Some(vp) = viewport {
            items.retain(|item| vp.contains_item(item));
        }

        // Sort by z_index ascending so callers can render items in correct layering order.
        items.sort_by_key(|item| item.z_index);
        items
    }

    // -----------------------------------------------------------------------
    // Migration: v1 (single-blob) → v2 (per-field)
    // -----------------------------------------------------------------------

    /// Migrate all items in the legacy `canvas:{id}:items` map into the three
    /// per-field maps. Idempotent — items already migrated (present in `prop`)
    /// are skipped.
    pub fn migrate_canvas_v1_to_v2(&self, crdt: &CrdtExtension, canvas_id: &CanvasId) {
        let legacy = crdt.map_entries(&Self::legacy_items_map(canvas_id));
        let existing_props = crdt.map_entries(&Self::prop_map(canvas_id));

        for (item_id, json) in &legacy {
            if existing_props.contains_key(item_id) {
                continue; // already migrated
            }
            if let Ok(item) = serde_json::from_str::<CanvasItem>(json) {
                let _ = self.upsert_item(crdt, &item);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Canvas-level metadata
    // -----------------------------------------------------------------------

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
#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
