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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
// KanbanExtension
// ---------------------------------------------------------------------------

/// Stateless extension for kanban boards.
/// All state is stored via [`CrdtExtension`] — one named map per entity set per board.
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

    fn cards_map(board_id: &KanbanBoardId) -> String {
        format!("kanban:{}:cards", board_id.as_str())
    }

    fn meta_map(board_id: &KanbanBoardId) -> String {
        format!("kanban:{}:meta", board_id.as_str())
    }

    // -----------------------------------------------------------------------
    // Columns
    // -----------------------------------------------------------------------

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
    pub fn remove_column(
        &self,
        crdt: &CrdtExtension,
        board_id: &KanbanBoardId,
        col_id: &KanbanColumnId,
    ) {
        let tombstone = serde_json::json!({ "id": col_id.as_str(), "deleted": true });
        crdt.map_set(&Self::cols_map(board_id), col_id.as_str(), &tombstone.to_string());
    }

    /// List all non-deleted columns sorted by `position`.
    pub fn list_columns(&self, crdt: &CrdtExtension, board_id: &KanbanBoardId) -> Vec<KanbanColumn> {
        let mut cols: Vec<KanbanColumn> = crdt
            .map_entries(&Self::cols_map(board_id))
            .into_values()
            .filter_map(|json| serde_json::from_str::<KanbanColumn>(&json).ok())
            .filter(|c| !c.deleted)
            .collect();
        cols.sort_by_key(|c| c.position);
        cols
    }

    // -----------------------------------------------------------------------
    // Cards
    // -----------------------------------------------------------------------

    pub fn upsert_card(
        &self,
        crdt: &CrdtExtension,
        card: &KanbanCard,
    ) -> Result<(), super::error::CmError> {
        let json = serde_json::to_string(card)
            .map_err(|e| super::error::CmError::Serialization(e.to_string()))?;
        crdt.map_set(&Self::cards_map(&card.board_id), card.id.as_str(), &json);
        Ok(())
    }

    /// Tombstone-delete a card.
    pub fn remove_card(
        &self,
        crdt: &CrdtExtension,
        board_id: &KanbanBoardId,
        card_id: &KanbanCardId,
    ) {
        let tombstone = serde_json::json!({ "id": card_id.as_str(), "deleted": true });
        crdt.map_set(&Self::cards_map(board_id), card_id.as_str(), &tombstone.to_string());
    }

    /// List non-deleted cards for `board_id`, optionally filtered to `col_id`, sorted by `position`.
    pub fn list_cards(
        &self,
        crdt: &CrdtExtension,
        board_id: &KanbanBoardId,
        col_id: Option<&KanbanColumnId>,
    ) -> Vec<KanbanCard> {
        let mut cards: Vec<KanbanCard> = crdt
            .map_entries(&Self::cards_map(board_id))
            .into_values()
            .filter_map(|json| serde_json::from_str::<KanbanCard>(&json).ok())
            .filter(|c| !c.deleted)
            .filter(|c| col_id.is_none_or(|cid| &c.column_id == cid))
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::crdt::CrdtExtension;

    #[test]
    fn column_crud() {
        let crdt = CrdtExtension::new("c");
        let ext = KanbanExtension;
        let conv_id = ConversationId::new();
        let board = KanbanBoard::new(conv_id, "Sprint 1");

        let col1 = KanbanColumn::new(board.id.clone(), "To Do", 0);
        let col2 = KanbanColumn::new(board.id.clone(), "Done", 1);

        ext.upsert_column(&crdt, &col1).unwrap();
        ext.upsert_column(&crdt, &col2).unwrap();

        let cols = ext.list_columns(&crdt, &board.id);
        assert_eq!(cols.len(), 2);
        assert_eq!(cols[0].name, "To Do"); // sorted by position
        assert_eq!(cols[1].name, "Done");
    }

    #[test]
    fn column_tombstone() {
        let crdt = CrdtExtension::new("c");
        let ext = KanbanExtension;
        let conv_id = ConversationId::new();
        let board = KanbanBoard::new(conv_id, "Board");

        let col = KanbanColumn::new(board.id.clone(), "Backlog", 0);
        ext.upsert_column(&crdt, &col).unwrap();

        ext.remove_column(&crdt, &board.id, &col.id);
        assert!(ext.list_columns(&crdt, &board.id).is_empty());
    }

    #[test]
    fn card_crud_and_filter() {
        let crdt = CrdtExtension::new("c");
        let ext = KanbanExtension;
        let conv_id = ConversationId::new();
        let board = KanbanBoard::new(conv_id, "Board");
        let col1 = KanbanColumn::new(board.id.clone(), "Todo", 0);
        let col2 = KanbanColumn::new(board.id.clone(), "Done", 1);

        let card = KanbanCard::new(col1.id.clone(), board.id.clone(), "Task A", 0);
        ext.upsert_card(&crdt, &card).unwrap();

        let all = ext.list_cards(&crdt, &board.id, None);
        assert_eq!(all.len(), 1);

        let in_col1 = ext.list_cards(&crdt, &board.id, Some(&col1.id));
        assert_eq!(in_col1.len(), 1);

        let in_col2 = ext.list_cards(&crdt, &board.id, Some(&col2.id));
        assert!(in_col2.is_empty());
    }

    #[test]
    fn card_tombstone() {
        let crdt = CrdtExtension::new("c");
        let ext = KanbanExtension;
        let conv_id = ConversationId::new();
        let board = KanbanBoard::new(conv_id, "Board");
        let col = KanbanColumn::new(board.id.clone(), "Col", 0);
        let card = KanbanCard::new(col.id.clone(), board.id.clone(), "Task", 0);

        ext.upsert_card(&crdt, &card).unwrap();
        ext.remove_card(&crdt, &board.id, &card.id);

        assert!(ext.list_cards(&crdt, &board.id, None).is_empty());
    }

    #[test]
    fn cards_sorted_by_position() {
        let crdt = CrdtExtension::new("c");
        let ext = KanbanExtension;
        let conv_id = ConversationId::new();
        let board = KanbanBoard::new(conv_id, "Board");
        let col = KanbanColumn::new(board.id.clone(), "Col", 0);

        let card_b = KanbanCard::new(col.id.clone(), board.id.clone(), "B", 1);
        let card_a = KanbanCard::new(col.id.clone(), board.id.clone(), "A", 0);
        ext.upsert_card(&crdt, &card_b).unwrap();
        ext.upsert_card(&crdt, &card_a).unwrap();

        let cards = ext.list_cards(&crdt, &board.id, None);
        assert_eq!(cards[0].title, "A");
        assert_eq!(cards[1].title, "B");
    }

    #[test]
    fn board_meta() {
        let crdt = CrdtExtension::new("c");
        let ext = KanbanExtension;
        let conv_id = ConversationId::new();
        let board = KanbanBoard::new(conv_id, "Board");

        ext.set_meta(&crdt, &board.id, "sprint", "1");
        assert_eq!(ext.get_meta(&crdt, &board.id, "sprint"), Some("1".into()));
        assert_eq!(ext.get_meta(&crdt, &board.id, "missing"), None);
    }

    #[test]
    fn board_roundtrip_serde() {
        let conv_id = ConversationId::new();
        let board = KanbanBoard::new(conv_id.clone(), "Sprint 1");
        assert_eq!(board.name, "Sprint 1");
        assert_eq!(board.conversation_id, conv_id);

        let json = serde_json::to_string(&board).unwrap();
        let back: KanbanBoard = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, board.id);
    }

    #[tokio::test]
    async fn push_pull_round_trip() {
        use crate::context::backend::InMemoryContextBackend;
        use crate::context::crdt::CrdtExtension;

        let backend = std::sync::Arc::new(InMemoryContextBackend::new());
        let conv = ConversationId::new();
        let branch = crate::context::types::BranchId::new();

        let writer_crdt = CrdtExtension::new("writer");
        let ext = KanbanExtension;
        let board = KanbanBoard::new(conv.clone(), "Board");
        let col = KanbanColumn::new(board.id.clone(), "Todo", 0);
        let card = KanbanCard::new(col.id.clone(), board.id.clone(), "Task", 0);

        ext.upsert_column(&writer_crdt, &col).unwrap();
        ext.upsert_card(&writer_crdt, &card).unwrap();
        writer_crdt.push(&conv, &branch, backend.as_ref()).await.unwrap();

        let reader_crdt = CrdtExtension::new("reader");
        reader_crdt.pull(&conv, &branch, backend.as_ref()).await.unwrap();

        let cols = ext.list_columns(&reader_crdt, &board.id);
        assert_eq!(cols.len(), 1);
        assert_eq!(cols[0].name, "Todo");

        let cards = ext.list_cards(&reader_crdt, &board.id, None);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].title, "Task");
    }
}
