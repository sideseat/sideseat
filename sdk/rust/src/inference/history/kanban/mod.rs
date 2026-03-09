use std::collections::HashMap;
use std::fmt;
use std::ops::Deref;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::error::HistoryError;
use super::source::VfsExtension;
use super::storage::HistoryStorage;
use super::types::{ConversationId, KanbanBoardId, UserId, now_micros};
use super::HistoryExtension;

// ---------------------------------------------------------------------------
// IDs
// ---------------------------------------------------------------------------

macro_rules! define_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new() -> Self {
                Self(uuid::Uuid::now_v7().to_string())
            }

            pub fn from_string(s: impl Into<String>) -> Self {
                Self(s.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl Deref for $name {
            type Target = str;
            fn deref(&self) -> &str {
                &self.0
            }
        }
    };
}

define_id!(KanbanColumnId);
define_id!(KanbanCardId);

// ---------------------------------------------------------------------------
// KanbanBoard
// ---------------------------------------------------------------------------

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KanbanColumn {
    pub id: KanbanColumnId,
    pub board_id: KanbanBoardId,
    pub name: String,
    pub position: u32,
    pub wip_limit: Option<u32>,
    pub created_at: i64,
    pub updated_at: i64,
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
            metadata: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// KanbanCard
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KanbanCardPriority {
    Low,
    Medium,
    High,
    Critical,
}

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
// KanbanExtension
// ---------------------------------------------------------------------------

/// Stateless extension that adds Kanban boards to a `History`.
/// Register with `History::with_extension(Arc::new(KanbanExtension))`.
pub struct KanbanExtension;

impl HistoryExtension for KanbanExtension {
    fn id(&self) -> &str {
        "kanban"
    }
}

impl KanbanExtension {
    pub async fn create_board(
        &self,
        storage: &impl HistoryStorage,
        conversation_id: ConversationId,
        name: &str,
    ) -> Result<KanbanBoardId, HistoryError> {
        let board = KanbanBoard::new(conversation_id, name);
        let id = board.id.clone();
        storage.save_kanban_board(&board).await?;
        Ok(id)
    }

    pub async fn get_board(
        &self,
        storage: &impl HistoryStorage,
        id: &KanbanBoardId,
    ) -> Result<Option<KanbanBoard>, HistoryError> {
        storage.get_kanban_board(id).await
    }

    pub async fn add_column(
        &self,
        storage: &impl HistoryStorage,
        board_id: &KanbanBoardId,
        name: &str,
        position: u32,
    ) -> Result<KanbanColumnId, HistoryError> {
        let col = KanbanColumn::new(board_id.clone(), name, position);
        let id = col.id.clone();
        storage.save_kanban_column(&col).await?;
        Ok(id)
    }

    pub async fn list_columns(
        &self,
        storage: &impl HistoryStorage,
        board_id: &KanbanBoardId,
    ) -> Result<Vec<KanbanColumn>, HistoryError> {
        storage.list_kanban_columns(board_id).await
    }

    pub async fn add_card(
        &self,
        storage: &impl HistoryStorage,
        column_id: &KanbanColumnId,
        board_id: &KanbanBoardId,
        title: &str,
        position: u32,
    ) -> Result<KanbanCardId, HistoryError> {
        let card = KanbanCard::new(column_id.clone(), board_id.clone(), title, position);
        let id = card.id.clone();
        storage.save_kanban_card(&card).await?;
        Ok(id)
    }

    pub async fn move_card(
        &self,
        storage: &impl HistoryStorage,
        card_id: &KanbanCardId,
        new_column_id: &KanbanColumnId,
        new_position: u32,
    ) -> Result<(), HistoryError> {
        let mut card = storage
            .get_kanban_card(card_id)
            .await?
            .ok_or_else(|| HistoryError::InvalidOperation(format!("Card not found: {card_id}")))?;
        card.column_id = new_column_id.clone();
        card.position = new_position;
        card.updated_at = now_micros();
        storage.save_kanban_card(&card).await
    }

    pub async fn list_cards(
        &self,
        storage: &impl HistoryStorage,
        column_id: &KanbanColumnId,
    ) -> Result<Vec<KanbanCard>, HistoryError> {
        storage.list_kanban_cards(column_id).await
    }

    pub async fn delete_card(
        &self,
        storage: &impl HistoryStorage,
        card_id: &KanbanCardId,
    ) -> Result<(), HistoryError> {
        storage.delete_kanban_card(card_id).await
    }

    pub async fn delete_column(
        &self,
        storage: &impl HistoryStorage,
        column_id: &KanbanColumnId,
    ) -> Result<(), HistoryError> {
        storage.delete_kanban_column(column_id).await
    }

    // -----------------------------------------------------------------------
    // CRDT-backed operations — store card and column state in VFS CRDT maps
    //
    // VFS paths:
    //   `kanban/{board_id}/columns` — column entries keyed by column ID
    //   `kanban/{board_id}/cards`   — card entries keyed by card ID
    //
    // Use these for real-time collaborative boards. Existing storage-backed
    // methods remain available for persistence and structured queries.
    // -----------------------------------------------------------------------

    fn crdt_columns_path(board_id: &KanbanBoardId) -> String {
        format!("kanban/{}/columns", board_id.as_str())
    }

    fn crdt_cards_path(board_id: &KanbanBoardId) -> String {
        format!("kanban/{}/cards", board_id.as_str())
    }

    /// Write `column` into the VFS CRDT map for `board_id`.
    /// Returns the Yjs v1 delta.
    pub fn crdt_upsert_column(
        &self,
        vfs: &VfsExtension,
        column: &KanbanColumn,
    ) -> Vec<u8> {
        let path = Self::crdt_columns_path(&column.board_id);
        let value = serde_json::to_value(column).unwrap_or_default();
        vfs.crdt_map_set(&path, column.id.as_str(), &value)
    }

    /// Remove `column_id` from the VFS CRDT map for `board_id`.
    /// Returns the Yjs v1 delta.
    pub fn crdt_delete_column(
        &self,
        vfs: &VfsExtension,
        board_id: &KanbanBoardId,
        column_id: &KanbanColumnId,
    ) -> Vec<u8> {
        let path = Self::crdt_columns_path(board_id);
        vfs.crdt_map_delete(&path, column_id.as_str())
    }

    /// Return all columns in the VFS CRDT map for `board_id`.
    pub fn crdt_list_columns(
        &self,
        vfs: &VfsExtension,
        board_id: &KanbanBoardId,
    ) -> Vec<KanbanColumn> {
        let path = Self::crdt_columns_path(board_id);
        let mut cols: Vec<KanbanColumn> = vfs
            .crdt_map_entries(&path)
            .into_values()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect();
        cols.sort_by_key(|c| c.position);
        cols
    }

    /// Write `card` into the VFS CRDT map for its board.
    /// Returns the Yjs v1 delta.
    pub fn crdt_upsert_card(&self, vfs: &VfsExtension, card: &KanbanCard) -> Vec<u8> {
        let path = Self::crdt_cards_path(&card.board_id);
        let value = serde_json::to_value(card).unwrap_or_default();
        vfs.crdt_map_set(&path, card.id.as_str(), &value)
    }

    /// Remove `card_id` from the VFS CRDT map for `board_id`.
    /// Returns the Yjs v1 delta.
    pub fn crdt_delete_card(
        &self,
        vfs: &VfsExtension,
        board_id: &KanbanBoardId,
        card_id: &KanbanCardId,
    ) -> Vec<u8> {
        let path = Self::crdt_cards_path(board_id);
        vfs.crdt_map_delete(&path, card_id.as_str())
    }

    /// Return all cards for `board_id` in the VFS CRDT map, optionally
    /// filtered to `column_id`.
    pub fn crdt_list_cards(
        &self,
        vfs: &VfsExtension,
        board_id: &KanbanBoardId,
        column_id: Option<&KanbanColumnId>,
    ) -> Vec<KanbanCard> {
        let path = Self::crdt_cards_path(board_id);
        let mut cards: Vec<KanbanCard> = vfs
            .crdt_map_entries(&path)
            .into_values()
            .filter_map(|v| serde_json::from_value::<KanbanCard>(v).ok())
            .filter(|c| !c.deleted)
            .filter(|c| column_id.is_none_or(|col| &c.column_id == col))
            .collect();
        cards.sort_by_key(|c| c.position);
        cards
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn board_new_roundtrip() {
        let conv_id = ConversationId::new();
        let board = KanbanBoard::new(conv_id.clone(), "Sprint 1");
        assert_eq!(board.name, "Sprint 1");
        assert_eq!(board.conversation_id, conv_id);

        let json = serde_json::to_string(&board).unwrap();
        let back: KanbanBoard = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, board.id);
    }

    #[test]
    fn crdt_kanban_round_trip() {
        use super::super::source::VfsExtension;

        let vfs = VfsExtension::new();
        let ext = KanbanExtension;

        let conv_id = ConversationId::new();
        let board = KanbanBoard::new(conv_id, "Sprint 1");
        let col1 = KanbanColumn::new(board.id.clone(), "To Do", 0);
        let col2 = KanbanColumn::new(board.id.clone(), "Done", 1);
        let card = KanbanCard::new(col1.id.clone(), board.id.clone(), "Task A", 0);

        ext.crdt_upsert_column(&vfs, &col1);
        ext.crdt_upsert_column(&vfs, &col2);
        ext.crdt_upsert_card(&vfs, &card);

        let cols = ext.crdt_list_columns(&vfs, &board.id);
        assert_eq!(cols.len(), 2);
        assert_eq!(cols[0].name, "To Do"); // sorted by position

        let cards = ext.crdt_list_cards(&vfs, &board.id, None);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].title, "Task A");

        // Filter by column
        let cards_in_col1 = ext.crdt_list_cards(&vfs, &board.id, Some(&col1.id));
        assert_eq!(cards_in_col1.len(), 1);

        let cards_in_col2 = ext.crdt_list_cards(&vfs, &board.id, Some(&col2.id));
        assert!(cards_in_col2.is_empty());

        ext.crdt_delete_card(&vfs, &board.id, &card.id);
        let cards_after = ext.crdt_list_cards(&vfs, &board.id, None);
        assert!(cards_after.is_empty());
    }

    #[test]
    fn column_and_card() {
        let board_id = KanbanBoardId::new();
        let col = KanbanColumn::new(board_id.clone(), "To Do", 0);
        let card = KanbanCard::new(col.id.clone(), board_id, "Implement feature", 0);
        assert_eq!(card.title, "Implement feature");
        assert!(!card.deleted);
    }
}
