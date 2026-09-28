use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::crdt::CrdtExtension;
use super::types::{ConversationId, KanbanBoardId, UserId, now_micros};

// ---------------------------------------------------------------------------
// IDs — use the crate-level macro exported from types.rs
// ---------------------------------------------------------------------------

crate::define_id!(KanbanColumnId);
crate::define_id!(KanbanCardId);

// ---------------------------------------------------------------------------
// KanbanBoard
// ---------------------------------------------------------------------------

/// A kanban board owned by a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KanbanBoard {
    pub id: KanbanBoardId,
    pub conversation_id: ConversationId,
    pub name: String,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

impl KanbanBoard {
    pub fn new(conversation_id: ConversationId, name: impl Into<String>) -> Self {
        let now = now_micros();
        Self {
            id: KanbanBoardId::new(),
            conversation_id,
            name: name.into(),
            created_at: now,
            updated_at: now,
            metadata: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// KanbanColumn
// ---------------------------------------------------------------------------

/// A column (swimlane) within a [`KanbanBoard`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KanbanColumn {
    pub id: KanbanColumnId,
    pub board_id: KanbanBoardId,
    pub name: String,
    pub position: u32,
    pub wip_limit: Option<u32>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Tombstone flag.
    pub deleted: bool,
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

impl KanbanColumn {
    pub fn new(board_id: KanbanBoardId, name: impl Into<String>, position: u32) -> Self {
        let now = now_micros();
        Self {
            id: KanbanColumnId::new(),
            board_id,
            name: name.into(),
            position,
            wip_limit: None,
            created_at: now,
            updated_at: now,
            deleted: false,
            metadata: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// KanbanCard
// ---------------------------------------------------------------------------

/// Priority level of a [`KanbanCard`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KanbanCardPriority {
    Low,
    Medium,
    High,
    Critical,
}

/// A task card within a [`KanbanColumn`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KanbanCard {
    pub id: KanbanCardId,
    pub column_id: KanbanColumnId,
    pub board_id: KanbanBoardId,
    pub title: String,
    pub description: Option<String>,
    pub position: u32,
    pub priority: Option<KanbanCardPriority>,
    pub assignee_id: Option<UserId>,
    pub due_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Tombstone flag.
    pub deleted: bool,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

impl KanbanCard {
    pub fn new(
        column_id: KanbanColumnId,
        board_id: KanbanBoardId,
        title: impl Into<String>,
        position: u32,
    ) -> Self {
        let now = now_micros();
        Self {
            id: KanbanCardId::new(),
            column_id,
            board_id,
            title: title.into(),
            description: None,
            position,
            priority: None,
            assignee_id: None,
            due_at: None,
            created_at: now,
            updated_at: now,
            deleted: false,
            labels: Vec::new(),
            metadata: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Per-field CRDT structs for cards (private)
// ---------------------------------------------------------------------------

/// Position fields — written by move operations only.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CardPos {
    column_id: KanbanColumnId,
    position: u32,
}

/// Metadata fields — written by edit operations only.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CardMeta {
    title: String,
    description: Option<String>,
    priority: Option<KanbanCardPriority>,
    assignee_id: Option<UserId>,
    due_at: Option<i64>,
    /// Labels moved to `clabels_map` for per-label CRDT granularity so concurrent
    /// label additions from different clients both survive merge (C1 fix).
    /// Kept here with `skip_serializing` for backward-compatible reads of data
    /// written before the per-label split.
    #[serde(default, rename = "labels", skip_serializing)]
    legacy_labels: Vec<String>,
    deleted: bool,
    created_at: i64,
    updated_at: i64,
}

// ---------------------------------------------------------------------------
// KanbanExtension
// ---------------------------------------------------------------------------

/// Stateless extension for kanban boards.
///
/// Card state is split across two namespaced CRDT maps per board, enabling
/// independent concurrent position moves and metadata edits:
///
/// - `kanban:{id}:cpos`  — column assignment and position (move only)
/// - `kanban:{id}:cmeta` — title/description/priority/assignee/labels (edits)
///
/// Column state remains in a single `kanban:{id}:cols` map (columns are rarely
/// edited concurrently with moves).
pub struct KanbanExtension;

impl super::ContextExtension for KanbanExtension {
    fn id(&self) -> &str {
        "kanban"
    }
}

impl KanbanExtension {
    fn cols_map(board_id: &KanbanBoardId) -> String {
        format!("kanban:{}:cols", board_id.as_str())
    }

    fn cpos_map(board_id: &KanbanBoardId) -> String {
        format!("kanban:{}:cpos", board_id.as_str())
    }

    fn cmeta_map(board_id: &KanbanBoardId) -> String {
        format!("kanban:{}:cmeta", board_id.as_str())
    }

    /// Per-label map: key = `{card_id}:{label}`, value = `"1"` (present) / `"0"` (removed).
    /// Separate entries per label so concurrent label additions from different clients
    /// both survive merge instead of clobbering each other LWW-style (C1 fix).
    fn clabels_map(board_id: &KanbanBoardId) -> String {
        format!("kanban:{}:clabels", board_id.as_str())
    }

    /// Deletion tombstones for cards: key = `card_id`, value = `"1"`.
    /// Separate from `cmeta` so a concurrent `patch_card_meta` cannot un-delete a card.
    fn cdel_map(board_id: &KanbanBoardId) -> String {
        format!("kanban:{}:cdel", board_id.as_str())
    }

    /// Deletion tombstones for columns: key = `col_id`, value = `"1"`.
    fn coldel_map(board_id: &KanbanBoardId) -> String {
        format!("kanban:{}:coldel", board_id.as_str())
    }

    /// Per-field card data: key = `{card_id}:{field}`, value = JSON-encoded field value.
    /// Fields: `title`, `desc`, `priority`, `assignee`, `due_at`.
    /// Separate entries per field so concurrent edits to different fields both survive merge.
    fn cfields_map(board_id: &KanbanBoardId) -> String {
        format!("kanban:{}:cfields", board_id.as_str())
    }

    fn meta_map(board_id: &KanbanBoardId) -> String {
        format!("kanban:{}:meta", board_id.as_str())
    }

    /// Write per-field entries for a card into `cfields` map.
    ///
    /// `None` for a parameter means "do not write this field" — the existing CRDT
    /// entry is left untouched. `Some(None)` for an optional field explicitly clears it.
    /// This is the patch-semantics variant used by [`patch_card_meta`].
    #[allow(clippy::too_many_arguments)]
    fn write_card_fields(
        crdt: &CrdtExtension,
        board_id: &KanbanBoardId,
        card_id: &str,
        title: Option<&str>,
        description: Option<Option<&str>>,
        priority: Option<Option<&KanbanCardPriority>>,
        assignee_id: Option<Option<&str>>,
        due_at: Option<Option<i64>>,
    ) {
        let map = Self::cfields_map(board_id);
        if let Some(t) = title {
            crdt.map_set(
                &map,
                &format!("{}:title", card_id),
                &serde_json::to_string(t).unwrap_or_default(),
            );
        }
        if let Some(d) = description {
            crdt.map_set(
                &map,
                &format!("{}:desc", card_id),
                &serde_json::to_string(&d).unwrap_or_default(),
            );
        }
        if let Some(p) = priority {
            crdt.map_set(
                &map,
                &format!("{}:priority", card_id),
                &serde_json::to_string(&p).unwrap_or_default(),
            );
        }
        if let Some(a) = assignee_id {
            crdt.map_set(
                &map,
                &format!("{}:assignee", card_id),
                &serde_json::to_string(&a).unwrap_or_default(),
            );
        }
        if let Some(d) = due_at {
            crdt.map_set(
                &map,
                &format!("{}:due_at", card_id),
                &serde_json::to_string(&d).unwrap_or_default(),
            );
        }
    }

    /// Read per-field entries for a card from `cfields_entries`.
    /// Falls back to the `cmeta` blob for fields missing from `cfields_entries`
    /// (backward compat for data written before the per-field split).
    fn read_card_fields(
        card_id: &str,
        cfields_entries: &HashMap<String, String>,
        cmeta_fallback: Option<&CardMeta>,
    ) -> (
        String,
        Option<String>,
        Option<KanbanCardPriority>,
        Option<String>,
        Option<i64>,
    ) {
        let get = |field: &str| cfields_entries.get(&format!("{}:{}", card_id, field));

        let title = get("title")
            .and_then(|s| serde_json::from_str::<String>(s).ok())
            .or_else(|| cmeta_fallback.map(|m| m.title.clone()))
            .unwrap_or_default();

        let description = get("desc")
            .and_then(|s| serde_json::from_str::<Option<String>>(s).ok())
            .flatten()
            .or_else(|| cmeta_fallback.and_then(|m| m.description.clone()));

        let priority = get("priority")
            .and_then(|s| serde_json::from_str::<Option<KanbanCardPriority>>(s).ok())
            .flatten()
            .or_else(|| cmeta_fallback.and_then(|m| m.priority.clone()));

        let assignee_id = get("assignee")
            .and_then(|s| serde_json::from_str::<Option<String>>(s).ok())
            .flatten()
            .or_else(|| {
                cmeta_fallback.and_then(|m| m.assignee_id.as_ref().map(|u| u.as_str().to_string()))
            });

        let due_at = get("due_at")
            .and_then(|s| serde_json::from_str::<Option<i64>>(s).ok())
            .flatten()
            .or_else(|| cmeta_fallback.and_then(|m| m.due_at));

        (title, description, priority, assignee_id, due_at)
    }

    // -----------------------------------------------------------------------
    // Columns
    // -----------------------------------------------------------------------

    /// Insert or update a column in the CRDT map.
    pub fn upsert_column(
        &self,
        crdt: &CrdtExtension,
        col: &KanbanColumn,
    ) -> Result<(), super::error::CmError> {
        let json = serde_json::to_string(col)
            .map_err(|e| super::error::CmError::Serialization(e.to_string()))?;
        crdt.map_set(&Self::cols_map(&col.board_id), col.id.as_str(), &json);
        Ok(())
    }

    /// Tombstone-delete a column.
    ///
    /// Writes only to `coldel` map so a concurrent `upsert_column` (editing name
    /// or position) cannot un-delete the column.  A read-modify-write on the cols
    /// blob would be overwritten by any concurrent write to that entry.
    pub fn remove_column(
        &self,
        crdt: &CrdtExtension,
        board_id: &KanbanBoardId,
        col_id: &KanbanColumnId,
    ) {
        crdt.map_set(&Self::coldel_map(board_id), col_id.as_str(), "1");
    }

    /// List all non-deleted columns sorted by `position`.
    pub fn list_columns(
        &self,
        crdt: &CrdtExtension,
        board_id: &KanbanBoardId,
    ) -> Vec<KanbanColumn> {
        let [cols_entries, coldel_entries]: [HashMap<String, String>; 2] = crdt
            .map_entries_batch(&[&Self::cols_map(board_id), &Self::coldel_map(board_id)])
            .try_into()
            .expect("batch length matches name count");

        let mut cols: Vec<KanbanColumn> = cols_entries
            .into_values()
            .filter_map(|json| serde_json::from_str::<KanbanColumn>(&json).ok())
            // Deleted if in coldel map (new tombstone) or cols blob has deleted:true (legacy).
            .filter(|c| !coldel_entries.contains_key(c.id.as_str()) && !c.deleted)
            .collect();
        cols.sort_by_key(|c| c.position);
        cols
    }

    // -----------------------------------------------------------------------
    // Cards — targeted write methods
    // -----------------------------------------------------------------------

    /// Move a card to a different column/position (writes cpos only).
    /// Concurrent metadata edits on the same card are preserved.
    pub fn move_card(
        &self,
        crdt: &CrdtExtension,
        board_id: &KanbanBoardId,
        card_id: &KanbanCardId,
        column_id: KanbanColumnId,
        position: u32,
    ) -> Result<(), super::error::CmError> {
        let pos = CardPos {
            column_id,
            position,
        };
        let json = serde_json::to_string(&pos)
            .map_err(|e| super::error::CmError::Serialization(e.to_string()))?;
        crdt.map_set(&Self::cpos_map(board_id), card_id.as_str(), &json);
        Ok(())
    }

    /// Add a label to a card (single clabels entry — concurrent adds on different
    /// labels from other clients are preserved independently).
    pub fn add_card_label(
        &self,
        crdt: &CrdtExtension,
        board_id: &KanbanBoardId,
        card_id: &KanbanCardId,
        label: &str,
    ) {
        crdt.map_set(
            &Self::clabels_map(board_id),
            &format!("{}:{}", card_id.as_str(), label),
            "1",
        );
    }

    /// Remove a label from a card (tombstone: sets the clabels entry to `"0"`).
    pub fn remove_card_label(
        &self,
        crdt: &CrdtExtension,
        board_id: &KanbanBoardId,
        card_id: &KanbanCardId,
        label: &str,
    ) {
        crdt.map_set(
            &Self::clabels_map(board_id),
            &format!("{}:{}", card_id.as_str(), label),
            "0",
        );
    }

    /// Update card metadata fields using patch semantics.
    ///
    /// `None` for a field means "do not change this field" — only provided fields
    /// produce CRDT writes. This ensures concurrent edits to different fields from
    /// different clients both survive merge independently (per-field LWW).
    ///
    /// For optional fields use `Some(None)` to explicitly clear the field, or
    /// `Some(Some(v))` to set it.
    ///
    /// The cmeta blob is NOT written — partial writes to a whole-blob entry would
    /// clobber concurrent writes to fields not included in this patch. Use
    /// [`upsert_card`] for full-blob writes (e.g., initial card creation).
    ///
    /// Concurrent move operations on the same card are preserved (cpos is untouched).
    #[allow(clippy::too_many_arguments)]
    pub fn patch_card_meta(
        &self,
        crdt: &CrdtExtension,
        board_id: &KanbanBoardId,
        card_id: &KanbanCardId,
        title: Option<String>,
        description: Option<Option<String>>,
        priority: Option<Option<KanbanCardPriority>>,
        assignee_id: Option<Option<UserId>>,
        due_at: Option<Option<i64>>,
        labels: Vec<String>,
    ) -> Result<(), super::error::CmError> {
        // Only write cfields entries for provided fields.
        Self::write_card_fields(
            crdt,
            board_id,
            card_id.as_str(),
            title.as_deref(),
            description.as_ref().map(|d| d.as_deref()),
            priority.as_ref().map(|p| p.as_ref()),
            assignee_id.as_ref().map(|a| a.as_ref().map(|u| u.as_str())),
            due_at,
        );
        // Write each label as a separate per-label entry so concurrent label additions
        // from different clients both survive merge.
        for label in &labels {
            self.add_card_label(crdt, board_id, card_id, label);
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Cards — full upsert / tombstone
    // -----------------------------------------------------------------------

    /// Upsert a card into cpos, cmeta, and cfields maps.
    pub fn upsert_card(
        &self,
        crdt: &CrdtExtension,
        card: &KanbanCard,
    ) -> Result<(), super::error::CmError> {
        let pos = CardPos {
            column_id: card.column_id.clone(),
            position: card.position,
        };
        let meta = CardMeta {
            title: card.title.clone(),
            description: card.description.clone(),
            priority: card.priority.clone(),
            assignee_id: card.assignee_id.clone(),
            due_at: card.due_at,
            legacy_labels: Vec::new(),
            deleted: card.deleted,
            created_at: card.created_at,
            updated_at: card.updated_at,
        };

        let pos_json = serde_json::to_string(&pos)
            .map_err(|e| super::error::CmError::Serialization(e.to_string()))?;
        let meta_json = serde_json::to_string(&meta)
            .map_err(|e| super::error::CmError::Serialization(e.to_string()))?;

        crdt.map_set(&Self::cpos_map(&card.board_id), card.id.as_str(), &pos_json);
        crdt.map_set(
            &Self::cmeta_map(&card.board_id),
            card.id.as_str(),
            &meta_json,
        );
        // Per-field writes: Some(v) = "write all fields" (full upsert).
        Self::write_card_fields(
            crdt,
            &card.board_id,
            card.id.as_str(),
            Some(&card.title),
            Some(card.description.as_deref()),
            Some(card.priority.as_ref()),
            Some(card.assignee_id.as_ref().map(|u| u.as_str())),
            Some(card.due_at),
        );
        for label in &card.labels {
            self.add_card_label(crdt, &card.board_id, &card.id, label);
        }
        Ok(())
    }

    /// Tombstone-delete a card.
    ///
    /// Writes only to the `cdel` map so a concurrent `patch_card_meta` cannot
    /// un-delete the card (a read-modify-write on cmeta with `deleted: true` would
    /// be overwritten by any concurrent edit to cmeta).
    /// The cpos and cfields entries are preserved; `list_cards` filters on `cdel`.
    pub fn remove_card(
        &self,
        crdt: &CrdtExtension,
        board_id: &KanbanBoardId,
        card_id: &KanbanCardId,
    ) {
        crdt.map_set(&Self::cdel_map(board_id), card_id.as_str(), "1");
    }

    // -----------------------------------------------------------------------
    // Cards — list (joins cpos + cmeta)
    // -----------------------------------------------------------------------

    /// List non-deleted cards for `board_id`, optionally filtered to `col_id`, sorted by `position`.
    pub fn list_cards(
        &self,
        crdt: &CrdtExtension,
        board_id: &KanbanBoardId,
        col_id: Option<&KanbanColumnId>,
    ) -> Vec<KanbanCard> {
        // Single lock acquisition for all 6 maps — prevents torn reads.
        let [
            cpos_entries,
            cmeta_entries,
            cfields_entries,
            clabels_entries,
            cdel_entries,
            coldel_entries,
        ]: [HashMap<String, String>; 6] = crdt
            .map_entries_batch(&[
                &Self::cpos_map(board_id),
                &Self::cmeta_map(board_id),
                &Self::cfields_map(board_id),
                &Self::clabels_map(board_id),
                &Self::cdel_map(board_id),
                &Self::coldel_map(board_id),
            ])
            .try_into()
            .expect("batch length matches name count");

        let mut cards: Vec<KanbanCard> = cmeta_entries
            .iter()
            .filter_map(|(card_id_str, meta_json)| {
                let meta: CardMeta = serde_json::from_str(meta_json).ok()?;

                // Deleted if in cdel map (Fix 2 tombstone) or legacy cmeta.deleted flag.
                if cdel_entries.contains_key(card_id_str.as_str()) || meta.deleted {
                    return None;
                }

                let pos: CardPos = cpos_entries
                    .get(card_id_str)
                    .and_then(|s| serde_json::from_str(s).ok())?;

                // Skip cards whose column has been deleted (C3: no ghost-column cards).
                if coldel_entries.contains_key(pos.column_id.as_str()) {
                    return None;
                }

                if col_id.is_some_and(|cid| cid.as_str() != pos.column_id.as_str()) {
                    return None;
                }

                // Per-field read (Fix 4): prefer cfields entries; fall back to cmeta blob.
                let (title, description, priority, assignee_id_str, due_at) =
                    Self::read_card_fields(card_id_str, &cfields_entries, Some(&meta));

                let assignee_id = assignee_id_str.map(UserId::from_string);

                // Collect active labels from per-label clabels map.
                // Fall back to cmeta.legacy_labels for data written before the per-label split.
                let prefix = format!("{}:", card_id_str);
                let mut labels: Vec<String> = clabels_entries
                    .iter()
                    .filter_map(|(k, v)| {
                        if v == "1" && k.starts_with(&prefix) {
                            Some(k[prefix.len()..].to_string())
                        } else {
                            None
                        }
                    })
                    .collect();
                if labels.is_empty() && !meta.legacy_labels.is_empty() {
                    labels = meta.legacy_labels.clone();
                }
                labels.sort();

                let card_id = KanbanCardId::from_string(card_id_str.clone());
                Some(KanbanCard {
                    id: card_id,
                    column_id: pos.column_id,
                    board_id: board_id.clone(),
                    title,
                    description,
                    position: pos.position,
                    priority,
                    assignee_id,
                    due_at,
                    created_at: meta.created_at,
                    updated_at: meta.updated_at,
                    deleted: false,
                    labels,
                    metadata: HashMap::new(),
                })
            })
            .collect();
        cards.sort_by_key(|c| c.position);
        cards
    }

    // -----------------------------------------------------------------------
    // Board meta
    // -----------------------------------------------------------------------

    pub fn set_meta(&self, crdt: &CrdtExtension, board_id: &KanbanBoardId, key: &str, value: &str) {
        crdt.map_set(&Self::meta_map(board_id), key, value);
    }

    pub fn get_meta(
        &self,
        crdt: &CrdtExtension,
        board_id: &KanbanBoardId,
        key: &str,
    ) -> Option<String> {
        crdt.map_get(&Self::meta_map(board_id), key)
    }
}
#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
