use std::collections::HashMap;

use serde_json::Value;
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc, GetString, Map, ReadTxn, StateVector, Text, Transact, Update, WriteTxn};

use super::canvas::CanvasItem;
use super::error::HistoryError;
use super::types::{Node, UserId};

// ---------------------------------------------------------------------------
// CursorPosition
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CursorPosition {
    pub node_id: Option<String>,
    pub offset: Option<u64>,
    pub selection_end: Option<u64>,
}

// ---------------------------------------------------------------------------
// CrdtDoc
// ---------------------------------------------------------------------------

pub struct CrdtDoc {
    doc: Doc,
}

impl CrdtDoc {
    pub fn new() -> Self {
        Self { doc: Doc::new() }
    }

    pub fn from_state(state: &[u8]) -> Result<Self, HistoryError> {
        let doc = Doc::new();
        let update =
            Update::decode_v1(state).map_err(|e| HistoryError::Crdt(e.to_string()))?;
        {
            let mut txn = doc.transact_mut();
            txn.apply_update(update)
                .map_err(|e| HistoryError::Crdt(e.to_string()))?;
        }
        Ok(Self { doc })
    }

    // -----------------------------------------------------------------------
    // Core sync
    // -----------------------------------------------------------------------

    pub fn record_node(&mut self, node: &Node) -> Vec<u8> {
        let value = serde_json::to_string(node).unwrap_or_default();
        let mut txn = self.doc.transact_mut();
        let nodes = txn.get_or_insert_map("nodes");
        nodes.insert(&mut txn, node.id.as_str(), value.as_str());
        txn.encode_update_v1()
    }

    pub fn record_canvas_item(&mut self, item: &CanvasItem) -> Vec<u8> {
        let value = serde_json::to_string(item).unwrap_or_default();
        let mut txn = self.doc.transact_mut();
        let items = txn.get_or_insert_map("canvas_items");
        items.insert(&mut txn, item.id.as_str(), value.as_str());
        txn.encode_update_v1()
    }

    pub fn merge_delta(&mut self, delta: &[u8]) -> Result<(), HistoryError> {
        let update =
            Update::decode_v1(delta).map_err(|e| HistoryError::Crdt(e.to_string()))?;
        let mut txn = self.doc.transact_mut();
        txn.apply_update(update)
            .map_err(|e| HistoryError::Crdt(e.to_string()))
    }

    pub fn state_vector(&self) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.state_vector().encode_v1()
    }

    pub fn encode_diff(&self, remote_sv: &[u8]) -> Vec<u8> {
        let sv = StateVector::decode_v1(remote_sv).unwrap_or_default();
        let txn = self.doc.transact();
        txn.encode_diff_v1(&sv)
    }

    pub fn full_state(&self) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.encode_diff_v1(&StateVector::default())
    }

    // -----------------------------------------------------------------------
    // Cursors
    // -----------------------------------------------------------------------

    pub fn update_cursor(&mut self, user_id: &UserId, position: CursorPosition) {
        let value = serde_json::to_string(&position).unwrap_or_default();
        let mut txn = self.doc.transact_mut();
        let cursors = txn.get_or_insert_map("cursors");
        cursors.insert(&mut txn, user_id.as_str(), value.as_str());
    }

    pub fn cursors(&self) -> HashMap<String, CursorPosition> {
        let txn = self.doc.transact();
        let mut result = HashMap::new();

        if let Some(cursors) = txn.get_map("cursors") {
            for (key, value) in cursors.iter(&txn) {
                if let yrs::Out::Any(yrs::Any::String(s)) = value
                    && let Ok(pos) = serde_json::from_str::<CursorPosition>(&s)
                {
                    result.insert(key.to_string(), pos);
                }
            }
        }

        result
    }

    // -----------------------------------------------------------------------
    // Extension lists (ID-based ops)
    // -----------------------------------------------------------------------

    pub fn list_create(&mut self, name: &str) -> Vec<u8> {
        let mut txn = self.doc.transact_mut();
        let ext_lists = txn.get_or_insert_map("ext_lists");
        // Store as a map of id→value for ID-based operations
        ext_lists.insert(&mut txn, name, yrs::Any::String("[]".into()));
        txn.encode_update_v1()
    }

    pub fn list_append(&mut self, name: &str, item: &Value) -> Vec<u8> {
        let mut txn = self.doc.transact_mut();
        let ext_lists = txn.get_or_insert_map("ext_lists");

        let mut items = self.read_list_items_inner(&txn, name);
        items.push(item.clone());

        let serialized = serde_json::to_string(&items).unwrap_or_else(|_| "[]".into());
        ext_lists.insert(&mut txn, name, serialized.as_str());
        txn.encode_update_v1()
    }

    pub fn list_update_by_id(&mut self, name: &str, item_id: &str, item: &Value) -> Vec<u8> {
        let mut txn = self.doc.transact_mut();
        let ext_lists = txn.get_or_insert_map("ext_lists");

        let mut items = self.read_list_items_inner(&txn, name);
        for existing in items.iter_mut() {
            if existing.get("id").and_then(|v| v.as_str()) == Some(item_id) {
                *existing = item.clone();
                break;
            }
        }

        let serialized = serde_json::to_string(&items).unwrap_or_else(|_| "[]".into());
        ext_lists.insert(&mut txn, name, serialized.as_str());
        txn.encode_update_v1()
    }

    pub fn list_remove_by_id(&mut self, name: &str, item_id: &str) -> Vec<u8> {
        let mut txn = self.doc.transact_mut();
        let ext_lists = txn.get_or_insert_map("ext_lists");

        let mut items = self.read_list_items_inner(&txn, name);
        items.retain(|item| item.get("id").and_then(|v| v.as_str()) != Some(item_id));

        let serialized = serde_json::to_string(&items).unwrap_or_else(|_| "[]".into());
        ext_lists.insert(&mut txn, name, serialized.as_str());
        txn.encode_update_v1()
    }

    pub fn list_items(&self, name: &str) -> Vec<Value> {
        let txn = self.doc.transact();
        self.read_list_items_inner(&txn, name)
    }

    fn read_list_items_inner<T: ReadTxn>(&self, txn: &T, name: &str) -> Vec<Value> {
        let Some(ext_lists) = txn.get_map("ext_lists") else {
            return Vec::new();
        };
        let Some(yrs::Out::Any(yrs::Any::String(s))) = ext_lists.get(txn, name) else {
            return Vec::new();
        };
        serde_json::from_str::<Vec<Value>>(&s).unwrap_or_default()
    }

    // -----------------------------------------------------------------------
    // Extension maps
    // -----------------------------------------------------------------------

    pub fn map_set(&mut self, name: &str, key: &str, value: &Value) -> Vec<u8> {
        let mut txn = self.doc.transact_mut();
        let ext_maps = txn.get_or_insert_map("ext_maps");

        // Get or create inner map (stored as JSON object string)
        let mut map_data = self.read_map_inner(&txn, name);
        map_data.insert(key.to_string(), value.clone());

        let map_str = serde_json::to_string(&map_data).unwrap_or_else(|_| "{}".into());
        ext_maps.insert(&mut txn, name, map_str.as_str());
        txn.encode_update_v1()
    }

    pub fn map_delete(&mut self, name: &str, key: &str) -> Vec<u8> {
        let mut txn = self.doc.transact_mut();
        let ext_maps = txn.get_or_insert_map("ext_maps");

        let mut map_data = self.read_map_inner(&txn, name);
        map_data.remove(key);

        let map_str = serde_json::to_string(&map_data).unwrap_or_else(|_| "{}".into());
        ext_maps.insert(&mut txn, name, map_str.as_str());
        txn.encode_update_v1()
    }

    pub fn map_get(&self, name: &str, key: &str) -> Option<Value> {
        let txn = self.doc.transact();
        let map_data = self.read_map_inner(&txn, name);
        map_data.get(key).cloned()
    }

    pub fn map_entries(&self, name: &str) -> HashMap<String, Value> {
        let txn = self.doc.transact();
        self.read_map_inner(&txn, name)
    }

    fn read_map_inner<T: ReadTxn>(&self, txn: &T, name: &str) -> HashMap<String, Value> {
        let Some(ext_maps) = txn.get_map("ext_maps") else {
            return HashMap::new();
        };
        let Some(yrs::Out::Any(yrs::Any::String(s))) = ext_maps.get(txn, name) else {
            return HashMap::new();
        };
        serde_json::from_str::<HashMap<String, Value>>(&s).unwrap_or_default()
    }

    // -----------------------------------------------------------------------
    // Text (Y.Text) operations — for CRDT text files
    // -----------------------------------------------------------------------

    /// Insert `content` at `index` into a named Y.Text field. Returns the delta.
    pub fn text_insert(&mut self, name: &str, index: u32, content: &str) -> Vec<u8> {
        let mut txn = self.doc.transact_mut();
        let text = txn.get_or_insert_text(name);
        text.insert(&mut txn, index, content);
        txn.encode_update_v1()
    }

    /// Remove `len` characters starting at `index` from a named Y.Text field. Returns the delta.
    pub fn text_remove(&mut self, name: &str, index: u32, len: u32) -> Vec<u8> {
        let mut txn = self.doc.transact_mut();
        let text = txn.get_or_insert_text(name);
        text.remove_range(&mut txn, index, len);
        txn.encode_update_v1()
    }

    /// Return the current string content of a named Y.Text field.
    pub fn text_read(&self, name: &str) -> String {
        let txn = self.doc.transact();
        match txn.get_text(name) {
            Some(t) => t.get_string(&txn),
            None => String::new(),
        }
    }

    /// Return the length (in Unicode chars) of a named Y.Text field.
    pub fn text_len(&self, name: &str) -> u32 {
        let txn = self.doc.transact();
        txn.get_text(name).map(|t| t.len(&txn)).unwrap_or(0)
    }
}

impl Default for CrdtDoc {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::types::*;
    use crate::types::ContentBlock;

    fn make_test_node() -> Node {
        Node {
            id: NodeId::from_string("test-node-1"),
            conversation_id: ConversationId::new(),
            branch_id: BranchId::new(),
            parent_id: None,
            sequence: 0,
            created_at: now_micros(),
            created_by: None,
            model: None,
            provider: None,
            content: NodeContent::UserMessage {
                content: vec![ContentBlock::text("hello")],
                name: None,
            },
            usage: None,
            version: 0,
            is_final: true,
            streaming: None,
            deleted: false,
            agent_id: None,
            correlation_id: None,
            reply_to: None,
            eval_scores: Vec::new(),
            metadata: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn record_node_round_trip() {
        let mut doc1 = CrdtDoc::new();
        let node = make_test_node();

        let delta = doc1.record_node(&node);
        assert!(!delta.is_empty());

        // Merge into second doc
        let mut doc2 = CrdtDoc::new();
        doc2.merge_delta(&delta).unwrap();

        // Verify via state
        let state1 = doc1.full_state();
        let state2 = doc2.full_state();
        assert!(!state1.is_empty());
        assert!(!state2.is_empty());
    }

    #[test]
    fn cursor_tracking() {
        let mut doc = CrdtDoc::new();
        let user = UserId::from_string("user-1");

        doc.update_cursor(
            &user,
            CursorPosition {
                node_id: Some("n1".into()),
                offset: Some(42),
                selection_end: None,
            },
        );

        let cursors = doc.cursors();
        assert_eq!(cursors.len(), 1);
        let pos = &cursors["user-1"];
        assert_eq!(pos.node_id.as_deref(), Some("n1"));
        assert_eq!(pos.offset, Some(42));
    }

    #[test]
    fn extension_list_ops() {
        let mut doc = CrdtDoc::new();

        doc.list_create("tasks");
        doc.list_append(
            "tasks",
            &serde_json::json!({"id": "t1", "title": "Task 1"}),
        );
        doc.list_append(
            "tasks",
            &serde_json::json!({"id": "t2", "title": "Task 2"}),
        );

        let items = doc.list_items("tasks");
        assert_eq!(items.len(), 2);

        doc.list_update_by_id(
            "tasks",
            "t1",
            &serde_json::json!({"id": "t1", "title": "Updated Task 1"}),
        );

        let items = doc.list_items("tasks");
        assert_eq!(items[0]["title"], "Updated Task 1");

        doc.list_remove_by_id("tasks", "t2");
        let items = doc.list_items("tasks");
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn extension_map_ops() {
        let mut doc = CrdtDoc::new();

        doc.map_set("config", "theme", &serde_json::json!("dark"));
        doc.map_set("config", "lang", &serde_json::json!("en"));

        assert_eq!(
            doc.map_get("config", "theme"),
            Some(serde_json::json!("dark"))
        );

        let entries = doc.map_entries("config");
        assert_eq!(entries.len(), 2);

        doc.map_delete("config", "theme");
        assert!(doc.map_get("config", "theme").is_none());
    }

    #[test]
    fn sync_between_peers() {
        let mut peer_a = CrdtDoc::new();
        let mut peer_b = CrdtDoc::new();

        let node = make_test_node();
        peer_a.record_node(&node);

        // Sync A → B
        let sv_b = peer_b.state_vector();
        let diff = peer_a.encode_diff(&sv_b);
        peer_b.merge_delta(&diff).unwrap();

        // Verify B has the data
        let state_a = peer_a.full_state();
        let state_b = peer_b.full_state();
        assert_eq!(state_a.len(), state_b.len());
    }
}
