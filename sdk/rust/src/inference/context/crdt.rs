use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc, GetString, Map, ReadTxn, StateVector, Text, Transact, Update, WriteTxn};

use super::backend::ContextBackend;
use super::error::CmError;
use super::sync::CrdtDelta;
use super::types::{BranchId, ConversationId, now_micros};

// ---------------------------------------------------------------------------
// CrdtDoc (private)
// ---------------------------------------------------------------------------

struct CrdtDoc {
    doc: Doc,
}

impl CrdtDoc {
    fn new() -> Self {
        Self { doc: Doc::new() }
    }

    fn from_state(state: &[u8]) -> Result<Self, CmError> {
        // Empty bytes = fresh doc (branches created before any CRDT writes).
        if state.is_empty() {
            return Ok(Self::new());
        }
        let doc = Doc::new();
        let update =
            Update::decode_v1(state).map_err(|e| CmError::Crdt(e.to_string()))?;
        {
            let mut txn = doc.transact_mut();
            txn.apply_update(update)
                .map_err(|e| CmError::Crdt(e.to_string()))?;
        }
        Ok(Self { doc })
    }

    // -----------------------------------------------------------------------
    // Sync primitives
    // -----------------------------------------------------------------------

    fn merge_delta(&mut self, delta: &[u8]) -> Result<(), CmError> {
        let update =
            Update::decode_v1(delta).map_err(|e| CmError::Crdt(e.to_string()))?;
        let mut txn = self.doc.transact_mut();
        txn.apply_update(update)
            .map_err(|e| CmError::Crdt(e.to_string()))
    }

    fn state_vector(&self) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.state_vector().encode_v1()
    }

    fn encode_diff(&self, remote_sv: &[u8]) -> Vec<u8> {
        let sv = StateVector::decode_v1(remote_sv).unwrap_or_default();
        let txn = self.doc.transact();
        txn.encode_diff_v1(&sv)
    }

    fn full_state(&self) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.encode_diff_v1(&StateVector::default())
    }

    // -----------------------------------------------------------------------
    // Generic named maps — each item is stored as a flat composite key
    // "{name}\x1F{key}" in a single CRDT map called "ext_maps".
    // Using separate keys per item ensures concurrent inserts converge
    // correctly (no read-modify-write race on a shared JSON blob).
    // -----------------------------------------------------------------------

    fn map_set(&mut self, name: &str, key: &str, value: &str) -> Vec<u8> {
        let mut txn = self.doc.transact_mut();
        let ext_maps = txn.get_or_insert_map("ext_maps");
        let composite = format!("{name}\x1F{key}");
        ext_maps.insert(&mut txn, composite.as_str(), value);
        txn.encode_update_v1()
    }

    fn map_delete(&mut self, name: &str, key: &str) -> Vec<u8> {
        let mut txn = self.doc.transact_mut();
        let ext_maps = txn.get_or_insert_map("ext_maps");
        let composite = format!("{name}\x1F{key}");
        ext_maps.remove(&mut txn, composite.as_str());
        txn.encode_update_v1()
    }

    fn map_get(&self, name: &str, key: &str) -> Option<String> {
        let txn = self.doc.transact();
        let ext_maps = txn.get_map("ext_maps")?;
        let composite = format!("{name}\x1F{key}");
        match ext_maps.get(&txn, composite.as_str()) {
            Some(yrs::Out::Any(yrs::Any::String(s))) => Some(s.to_string()),
            _ => None,
        }
    }

    fn map_entries(&self, name: &str) -> HashMap<String, String> {
        let txn = self.doc.transact();
        let Some(ext_maps) = txn.get_map("ext_maps") else {
            return HashMap::new();
        };
        let prefix = format!("{name}\x1F");
        ext_maps
            .iter(&txn)
            .filter_map(|(k, v)| {
                let item_key = k.strip_prefix(prefix.as_str())?;
                if let yrs::Out::Any(yrs::Any::String(s)) = v {
                    Some((item_key.to_string(), s.to_string()))
                } else {
                    None
                }
            })
            .collect()
    }

    // -----------------------------------------------------------------------
    // Text (Y.Text) — for CRDT-backed text files
    // -----------------------------------------------------------------------

    fn text_insert(&mut self, name: &str, index: u32, content: &str) -> Vec<u8> {
        let mut txn = self.doc.transact_mut();
        let text = txn.get_or_insert_text(name);
        text.insert(&mut txn, index, content);
        txn.encode_update_v1()
    }

    fn text_remove(&mut self, name: &str, index: u32, len: u32) -> Vec<u8> {
        let mut txn = self.doc.transact_mut();
        let text = txn.get_or_insert_text(name);
        text.remove_range(&mut txn, index, len);
        txn.encode_update_v1()
    }

    fn text_read(&self, name: &str) -> String {
        let txn = self.doc.transact();
        match txn.get_text(name) {
            Some(t) => t.get_string(&txn),
            None => String::new(),
        }
    }

    fn text_len(&self, name: &str) -> u32 {
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
// CrdtExtension (public)
// ---------------------------------------------------------------------------

/// Public wrapper around [`CrdtDoc`] that integrates with [`ContextBackend`]
/// for push/pull and exposes all doc operations via `&self` (Mutex inside).
pub struct CrdtExtension {
    doc: Mutex<CrdtDoc>,
    /// State vector at the time of the last successful push.
    last_sync_sv: Mutex<Vec<u8>>,
    /// Global seq of the last delta we pushed — used for node watermarking.
    last_sync_seq: AtomicU64,
    /// Max global seq seen via pull — used as after_seq for incremental fetch.
    /// Tracked separately from last_sync_seq so concurrent clients don't miss
    /// each other's earlier deltas (push seq != pull-frontier seq).
    last_pull_seq: AtomicU64,
    /// This client's identity — used to skip own deltas on pull.
    client_id: String,
}

impl CrdtExtension {
    pub fn new(client_id: impl Into<String>) -> Self {
        Self {
            doc: Mutex::new(CrdtDoc::new()),
            last_sync_sv: Mutex::new(Vec::new()),
            last_sync_seq: AtomicU64::new(0),
            last_pull_seq: AtomicU64::new(0),
            client_id: client_id.into(),
        }
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// Global seq of the last synced delta; used as CRDT watermark for nodes.
    pub fn current_seq(&self) -> u64 {
        self.last_sync_seq.load(Ordering::Acquire)
    }

    // -----------------------------------------------------------------------
    // Doc ops (forwarded through Mutex)
    // -----------------------------------------------------------------------

    pub fn map_set(&self, name: &str, key: &str, value: &str) {
        self.doc.lock().map_set(name, key, value);
    }

    pub fn map_delete(&self, name: &str, key: &str) {
        self.doc.lock().map_delete(name, key);
    }

    pub fn map_get(&self, name: &str, key: &str) -> Option<String> {
        self.doc.lock().map_get(name, key)
    }

    pub fn map_entries(&self, name: &str) -> HashMap<String, String> {
        self.doc.lock().map_entries(name)
    }

    pub fn text_insert(&self, name: &str, index: u32, content: &str) {
        self.doc.lock().text_insert(name, index, content);
    }

    pub fn text_remove(&self, name: &str, index: u32, len: u32) {
        self.doc.lock().text_remove(name, index, len);
    }

    pub fn text_read(&self, name: &str) -> String {
        self.doc.lock().text_read(name)
    }

    pub fn text_len(&self, name: &str) -> u32 {
        self.doc.lock().text_len(name)
    }

    pub fn full_state(&self) -> Vec<u8> {
        self.doc.lock().full_state()
    }

    pub fn state_vector(&self) -> Vec<u8> {
        self.doc.lock().state_vector()
    }

    pub fn encode_diff(&self, remote_sv: &[u8]) -> Vec<u8> {
        self.doc.lock().encode_diff(remote_sv)
    }

    // -----------------------------------------------------------------------
    // Snapshot
    // -----------------------------------------------------------------------

    /// Replace the entire doc with `bytes` and reset sync state.
    ///
    /// Both args must be supplied together: `bytes` is the serialized CRDT state
    /// and `snapshot_seq` is the CRDT log position it represents.
    /// After this call, `push` will only send NEW changes, and `pull` will start
    /// from `snapshot_seq` so no delta is applied twice.
    pub fn load_snapshot(&self, bytes: &[u8], snapshot_seq: u64) -> Result<(), CmError> {
        let new_doc = CrdtDoc::from_state(bytes)?;
        let new_sv = new_doc.state_vector();
        *self.doc.lock() = new_doc;
        *self.last_sync_sv.lock() = new_sv;
        self.last_sync_seq.store(snapshot_seq, Ordering::Release);
        self.last_pull_seq.store(snapshot_seq, Ordering::Release);
        Ok(())
    }

    /// Merge raw delta bytes directly into the doc.
    /// Used for building fork snapshots without a full push/pull cycle.
    pub fn merge_raw(&self, delta: &[u8]) -> Result<(), CmError> {
        self.doc.lock().merge_delta(delta)
    }

    /// Returns `(snapshot_seq, full_state_bytes)` for persistence.
    /// snapshot_seq is the max of push and pull seqs so that loading this
    /// snapshot + pulling after that seq never double-applies any delta.
    pub fn to_snapshot(&self) -> (u64, Vec<u8>) {
        let seq = self
            .last_sync_seq
            .load(Ordering::Acquire)
            .max(self.last_pull_seq.load(Ordering::Acquire));
        let state = self.doc.lock().full_state();
        (seq, state)
    }

    // -----------------------------------------------------------------------
    // Push / pull
    // -----------------------------------------------------------------------

    /// Compute the diff since the last push, append it to the backend delta log,
    /// and advance `last_sync_sv` / `last_sync_seq`.
    ///
    /// Idempotent: if there are no local changes, this is a no-op.
    pub async fn push<B: ContextBackend>(
        &self,
        conv_id: &ConversationId,
        branch_id: &BranchId,
        backend: &B,
    ) -> Result<(), CmError> {
        // 1. Snapshot sv + diff while holding both locks together.
        // Compare state vectors to detect no-op: yrs encodes a non-empty
        // header even for an empty diff, so is_empty() check is unreliable.
        let (delta, new_sv) = {
            let doc = self.doc.lock();
            let sv = self.last_sync_sv.lock();
            let new_sv = doc.state_vector();
            if new_sv == *sv {
                return Ok(());
            }
            let delta = doc.encode_diff(&sv);
            (delta, new_sv)
        };

        // 2. Persist (async, no locks held).
        let crdt_delta = CrdtDelta {
            global_seq: 0, // assigned by backend
            client_id: self.client_id.clone(),
            branch_id: branch_id.clone(),
            conversation_id: conv_id.clone(),
            delta,
            created_at: now_micros(),
        };
        let global_seq = backend.crdt_append(&crdt_delta).await?;

        // 3. Advance sync state.
        *self.last_sync_sv.lock() = new_sv;
        self.last_sync_seq.store(global_seq, Ordering::Release);

        Ok(())
    }

    /// Fetch deltas from the backend after `last_sync_seq`, merge all that were
    /// not produced by this client.
    pub async fn pull<B: ContextBackend>(
        &self,
        conv_id: &ConversationId,
        branch_id: &BranchId,
        backend: &B,
    ) -> Result<(), CmError> {
        let after = self.last_pull_seq.load(Ordering::Acquire);
        let deltas = backend.crdt_fetch(conv_id, branch_id, after).await?;

        if deltas.is_empty() {
            return Ok(());
        }

        let mut max_seq = after;
        // Compute the new state vector inside the doc lock to avoid a race
        // where a concurrent map_set between the lock release and re-acquisition
        // would produce a stale sv (causing the next push to re-send known deltas).
        let new_sv = {
            let mut doc = self.doc.lock();
            for d in &deltas {
                if d.global_seq > max_seq {
                    max_seq = d.global_seq;
                }
                if d.client_id == self.client_id {
                    // Skip own deltas — already applied locally.
                    continue;
                }
                doc.merge_delta(&d.delta)?;
            }
            if max_seq > after { doc.state_vector() } else { Vec::new() }
        };

        if max_seq > after {
            self.last_pull_seq.store(max_seq, Ordering::Release);
            *self.last_sync_sv.lock() = new_sv;
        }

        Ok(())
    }
}

impl super::ContextExtension for CrdtExtension {
    fn id(&self) -> &str {
        "crdt"
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::backend::InMemoryContextBackend;
    use std::sync::Arc;

    fn make_backend() -> Arc<InMemoryContextBackend> {
        Arc::new(InMemoryContextBackend::new())
    }

    #[test]
    fn empty_bytes_produces_blank_doc() {
        let result = CrdtDoc::from_state(&[]);
        assert!(result.is_ok());
        let doc = result.unwrap();
        assert_eq!(doc.full_state(), CrdtDoc::new().full_state());
    }

    #[test]
    fn map_set_and_entries() {
        let mut doc = CrdtDoc::new();
        doc.map_set("items", "a", r#"{"id":"a","v":1}"#);
        doc.map_set("items", "b", r#"{"id":"b","v":2}"#);

        let entries = doc.map_entries("items");
        assert_eq!(entries.len(), 2);
        assert!(entries.contains_key("a"));
        assert!(entries.contains_key("b"));
    }

    #[test]
    fn map_delete_removes_entry() {
        let mut doc = CrdtDoc::new();
        doc.map_set("items", "a", r#"{"id":"a"}"#);
        doc.map_delete("items", "a");

        assert!(doc.map_get("items", "a").is_none());
        assert!(doc.map_entries("items").is_empty());
    }

    #[test]
    fn text_roundtrip() {
        let mut doc = CrdtDoc::new();
        doc.text_insert("file.txt", 0, "hello");
        assert_eq!(doc.text_read("file.txt"), "hello");
        assert_eq!(doc.text_len("file.txt"), 5);

        doc.text_remove("file.txt", 0, 5);
        assert_eq!(doc.text_read("file.txt"), "");
    }

    #[test]
    fn state_snapshot_roundtrip() {
        let mut doc1 = CrdtDoc::new();
        doc1.map_set("items", "k", r#"{"id":"k","v":99}"#);

        let state = doc1.full_state();
        let doc2 = CrdtDoc::from_state(&state).unwrap();
        assert_eq!(doc2.map_get("items", "k"), Some(r#"{"id":"k","v":99}"#.into()));
    }

    #[test]
    fn merge_delta_converges() {
        let mut doc_a = CrdtDoc::new();
        let mut doc_b = CrdtDoc::new();

        let delta_a = doc_a.map_set("items", "a", r#"{"id":"a"}"#);
        let delta_b = doc_b.map_set("items", "b", r#"{"id":"b"}"#);

        doc_a.merge_delta(&delta_b).unwrap();
        doc_b.merge_delta(&delta_a).unwrap();

        let ea = doc_a.map_entries("items");
        let eb = doc_b.map_entries("items");
        assert_eq!(ea.len(), 2);
        assert_eq!(eb.len(), 2);
    }

    #[tokio::test]
    async fn push_only_sends_diff() {
        let backend = make_backend();
        let conv = ConversationId::new();
        let branch = BranchId::new();

        let ext = CrdtExtension::new("client-a");
        ext.map_set("items", "a", r#"{"id":"a"}"#);
        ext.push(&conv, &branch, backend.as_ref()).await.unwrap();

        // Second push with no new changes should be a no-op.
        ext.push(&conv, &branch, backend.as_ref()).await.unwrap();

        let deltas = backend.crdt_fetch(&conv, &branch, 0).await.unwrap();
        assert_eq!(deltas.len(), 1, "second push should be a no-op");
    }

    #[tokio::test]
    async fn pull_skips_own_client() {
        let backend = make_backend();
        let conv = ConversationId::new();
        let branch = BranchId::new();

        let writer = CrdtExtension::new("writer");
        let reader = CrdtExtension::new("writer"); // same client_id

        writer.map_set("items", "a", r#"{"id":"a"}"#);
        writer.push(&conv, &branch, backend.as_ref()).await.unwrap();

        // reader shares client_id with writer → pull should skip the delta.
        reader.pull(&conv, &branch, backend.as_ref()).await.unwrap();
        assert!(
            reader.map_get("items", "a").is_none(),
            "own delta must be skipped"
        );
    }

    #[tokio::test]
    async fn pull_applies_foreign_deltas() {
        let backend = make_backend();
        let conv = ConversationId::new();
        let branch = BranchId::new();

        let writer = CrdtExtension::new("writer");
        let reader = CrdtExtension::new("reader");

        writer.map_set("items", "a", r#"{"id":"a"}"#);
        writer.push(&conv, &branch, backend.as_ref()).await.unwrap();

        reader.pull(&conv, &branch, backend.as_ref()).await.unwrap();
        assert_eq!(
            reader.map_get("items", "a"),
            Some(r#"{"id":"a"}"#.into()),
        );
    }

    #[tokio::test]
    async fn load_snapshot_then_pull_no_double_apply() {
        let backend = make_backend();
        let conv = ConversationId::new();
        let branch = BranchId::new();

        let writer = CrdtExtension::new("writer");
        writer.map_set("items", "a", r#"{"id":"a"}"#);
        writer.push(&conv, &branch, backend.as_ref()).await.unwrap();

        let (snapshot_seq, snapshot_bytes) = writer.to_snapshot();

        // New extension loads snapshot then pulls — must not double-apply.
        let reader = CrdtExtension::new("reader");
        reader.load_snapshot(&snapshot_bytes, snapshot_seq).unwrap();
        reader.pull(&conv, &branch, backend.as_ref()).await.unwrap();

        // Data is present exactly once.
        assert_eq!(reader.map_get("items", "a"), Some(r#"{"id":"a"}"#.into()));
    }

    #[tokio::test]
    async fn concurrent_push_from_two_clients_merges() {
        let backend = make_backend();
        let conv = ConversationId::new();
        let branch = BranchId::new();

        let client_a = CrdtExtension::new("a");
        let client_b = CrdtExtension::new("b");

        client_a.map_set("items", "ka", r#"{"id":"ka"}"#);
        client_b.map_set("items", "kb", r#"{"id":"kb"}"#);

        client_a.push(&conv, &branch, backend.as_ref()).await.unwrap();
        client_b.push(&conv, &branch, backend.as_ref()).await.unwrap();

        // Both pull each other's changes.
        client_a.pull(&conv, &branch, backend.as_ref()).await.unwrap();
        client_b.pull(&conv, &branch, backend.as_ref()).await.unwrap();

        // Both should see both entries.
        assert_eq!(client_a.map_entries("items").len(), 2);
        assert_eq!(client_b.map_entries("items").len(), 2);
    }

    #[tokio::test]
    async fn load_snapshot_empty_bytes_is_blank_doc() {
        let ext = CrdtExtension::new("c");
        ext.load_snapshot(&[], 0).unwrap();
        assert!(ext.map_entries("items").is_empty());
    }

    #[tokio::test]
    async fn fork_snapshot_reconstruction() {
        // Parent branch accumulates some state + pushes.
        // Child is created from a snapshot of parent at a specific seq.
        let backend = make_backend();
        let conv = ConversationId::new();
        let parent_branch = BranchId::new();

        let parent = CrdtExtension::new("parent");
        parent.map_set("items", "k1", r#"{"id":"k1"}"#);
        parent.push(&conv, &parent_branch, backend.as_ref()).await.unwrap();

        // Capture snapshot at this point (watermark = parent's last push seq).
        let (snapshot_seq, snapshot_bytes) = parent.to_snapshot();
        assert!(snapshot_seq > 0);

        // Parent adds more after the snapshot.
        parent.map_set("items", "k2", r#"{"id":"k2"}"#);
        parent.push(&conv, &parent_branch, backend.as_ref()).await.unwrap();

        // Build child from the snapshot (before k2 was added).
        let child_branch = BranchId::new();
        let child = CrdtExtension::new("child");
        child.load_snapshot(&snapshot_bytes, snapshot_seq).unwrap();

        // Child should see k1 but NOT k2 (k2 was added after snapshot).
        let entries = child.map_entries("items");
        assert!(entries.contains_key("k1"), "child must have k1 from snapshot");
        assert!(!entries.contains_key("k2"), "child must not have k2 (post-snapshot)");

        // Push child state; it should be a no-op since no new writes.
        child.push(&conv, &child_branch, backend.as_ref()).await.unwrap();
        // Child pulls its own branch — no deltas expected.
        child.pull(&conv, &child_branch, backend.as_ref()).await.unwrap();
        assert_eq!(child.map_entries("items").len(), 1);
    }

    #[tokio::test]
    async fn two_level_ancestry() {
        // Grandparent → parent snapshot → child snapshot.
        // Child must see grandparent's entries via the parent snapshot chain.
        let backend = make_backend();
        let conv = ConversationId::new();
        let grandparent_branch = BranchId::new();

        // Grandparent writes and pushes.
        let grandparent = CrdtExtension::new("gp");
        grandparent.map_set("items", "gp_key", r#"{"id":"gp_key"}"#);
        grandparent.push(&conv, &grandparent_branch, backend.as_ref()).await.unwrap();

        // Parent snapshot = grandparent full state.
        let (gp_seq, gp_bytes) = grandparent.to_snapshot();

        let parent_branch = BranchId::new();
        let parent = CrdtExtension::new("parent");
        parent.load_snapshot(&gp_bytes, gp_seq).unwrap();
        parent.map_set("items", "parent_key", r#"{"id":"parent_key"}"#);
        parent.push(&conv, &parent_branch, backend.as_ref()).await.unwrap();

        // Child snapshot = parent full state (includes gp_key via snapshot).
        let (parent_seq, parent_bytes) = parent.to_snapshot();

        let child_branch = BranchId::new();
        let child = CrdtExtension::new("child");
        child.load_snapshot(&parent_bytes, parent_seq).unwrap();

        let entries = child.map_entries("items");
        assert!(entries.contains_key("gp_key"), "child must see grandparent key");
        assert!(entries.contains_key("parent_key"), "child must see parent key");

        // Child adds its own key.
        child.map_set("items", "child_key", r#"{"id":"child_key"}"#);
        child.push(&conv, &child_branch, backend.as_ref()).await.unwrap();

        let all = child.map_entries("items");
        assert_eq!(all.len(), 3);
    }
}
