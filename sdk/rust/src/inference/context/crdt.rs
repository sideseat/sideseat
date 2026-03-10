use std::collections::HashMap;

use parking_lot::Mutex;
use tokio::sync::broadcast;
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc, GetString, Map, ReadTxn, StateVector, Text, Transact, Update, WriteTxn};

use super::backend::ContextBackend;
use super::error::CmError;
use super::sync::CrdtDelta;
use super::types::{BranchId, ConversationId, now_micros};

// ---------------------------------------------------------------------------
// CrdtDoc (pub(super) for use in mod.rs fork())
// ---------------------------------------------------------------------------

pub(super) struct CrdtDoc {
    doc: Doc,
}

impl CrdtDoc {
    pub(super) fn new() -> Self {
        Self { doc: Doc::new() }
    }

    pub(super) fn from_state(state: &[u8]) -> Result<Self, CmError> {
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

    pub(super) fn merge_delta(&mut self, delta: &[u8]) -> Result<(), CmError> {
        let update =
            Update::decode_v1(delta).map_err(|e| CmError::Crdt(e.to_string()))?;
        let mut txn = self.doc.transact_mut();
        txn.apply_update(update)
            .map_err(|e| CmError::Crdt(e.to_string()))
    }

    pub(super) fn state_vector(&self) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.state_vector().encode_v1()
    }

    fn encode_diff(&self, remote_sv: &[u8]) -> Vec<u8> {
        let sv = StateVector::decode_v1(remote_sv).unwrap_or_default();
        let txn = self.doc.transact();
        txn.encode_diff_v1(&sv)
    }

    pub(super) fn full_state(&self) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.encode_diff_v1(&StateVector::default())
    }

    // -----------------------------------------------------------------------
    // Generic named maps — each logical namespace gets its own named yrs Map,
    // e.g. "canvas:abc:geo", "kanban:xyz:cpos". This gives O(namespace_size)
    // iteration instead of scanning all entries across all namespaces.
    // Using separate keys per item ensures concurrent inserts converge
    // correctly (no read-modify-write race on a shared JSON blob).
    // -----------------------------------------------------------------------

    fn map_set(&mut self, name: &str, key: &str, value: &str) -> Vec<u8> {
        let mut txn = self.doc.transact_mut();
        let map = txn.get_or_insert_map(name);
        map.insert(&mut txn, key, value);
        txn.encode_update_v1()
    }

    fn map_get(&self, name: &str, key: &str) -> Option<String> {
        let txn = self.doc.transact();
        let map = txn.get_map(name)?;
        match map.get(&txn, key) {
            Some(yrs::Out::Any(yrs::Any::String(s))) => Some(s.to_string()),
            _ => None,
        }
    }

    fn map_entries(&self, name: &str) -> HashMap<String, String> {
        let txn = self.doc.transact();
        let Some(map) = txn.get_map(name) else {
            return HashMap::new();
        };
        map.iter(&txn)
            .filter_map(|(k, v)| {
                if let yrs::Out::Any(yrs::Any::String(s)) = v {
                    Some((k.to_string(), s.to_string()))
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
// Snapshot / SV helpers
// ---------------------------------------------------------------------------

/// Element-wise max merge of an encoded baseline SV with the SVs of committed deltas.
///
/// Each `delta.sv` is the **cumulative** state vector stored by `push()`:
/// `SV(snap + all_pending_at_push_time + new_ops)`.  Taking the element-wise max
/// of all delta SVs (plus snap_sv) yields the committed baseline SV without
/// replaying full state:
/// `O(N × decode_sv)` instead of `O(snap_size + N × delta_size)`.
fn merge_svs<'a>(base: &[u8], others: impl Iterator<Item = &'a [u8]>) -> Vec<u8> {
    let mut result = StateVector::decode_v1(base).unwrap_or_default();
    for sv_bytes in others {
        if sv_bytes.is_empty() {
            continue;
        }
        if let Ok(other) = StateVector::decode_v1(sv_bytes) {
            result.merge(other);
        }
    }
    result.encode_v1()
}

/// Semantic equality check for encoded `StateVector`s.
///
/// `Vec<u8>` comparison is unreliable: yrs serialises `StateVector` from a
/// `HashMap`, so two semantically equal SVs may encode to different bytes due
/// to non-deterministic iteration order.
fn svs_equal(a: &[u8], b: &[u8]) -> bool {
    match (StateVector::decode_v1(a), StateVector::decode_v1(b)) {
        (Ok(sv_a), Ok(sv_b)) => sv_a == sv_b,
        _ => a == b, // fallback for malformed bytes
    }
}

/// Fallback for legacy deltas without a cached SV: builds the committed
/// baseline SV by replaying the full snapshot + all pending deltas.
/// `O(snap_size + N × delta_size)` — only taken when `delta.sv` is empty.
fn build_sv_from_snapshot_and_deltas(
    snap_bytes: &[u8],
    deltas: &[CrdtDelta],
) -> Result<Vec<u8>, CmError> {
    let mut tmp = CrdtDoc::from_state(snap_bytes)?;
    for d in deltas {
        tmp.merge_delta(&d.delta)?;
    }
    Ok(tmp.state_vector())
}

// ---------------------------------------------------------------------------
// CrdtExtension (public)
// ---------------------------------------------------------------------------

/// Public wrapper around [`CrdtDoc`] that integrates with [`ContextBackend`]
/// for push/pull and exposes all doc operations via `&self` (Mutex inside).
///
/// Sync state is **stateless**: the push/pull cursor is derived from the
/// backend snapshot on every call, eliminating stale in-memory cursors.
pub struct CrdtExtension {
    doc: Mutex<CrdtDoc>,
    /// This client's identity — used to tag outgoing deltas in push().
    client_id: String,
    /// Optional broadcast channel for change notifications.
    /// Subscribers receive the `global_seq` of the latest pulled delta.
    delta_tx: Option<broadcast::Sender<u64>>,
}

impl CrdtExtension {
    pub fn new(client_id: impl Into<String>) -> Self {
        Self {
            doc: Mutex::new(CrdtDoc::new()),
            client_id: client_id.into(),
            delta_tx: None,
        }
    }

    /// Enable change notifications. Returns `self` for builder chaining.
    ///
    /// After this, [`subscribe`] returns a live receiver that fires the
    /// `global_seq` of each pull that advances the committed baseline.
    pub fn with_notifications(mut self, capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        self.delta_tx = Some(tx);
        self
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// Subscribe to change notifications. Returns `None` if notifications
    /// were not enabled via [`with_notifications`].
    pub fn subscribe(&self) -> Option<broadcast::Receiver<u64>> {
        self.delta_tx.as_ref().map(|tx| tx.subscribe())
    }

    // -----------------------------------------------------------------------
    // Doc ops (forwarded through Mutex)
    // -----------------------------------------------------------------------

    pub fn map_set(&self, name: &str, key: &str, value: &str) {
        self.doc.lock().map_set(name, key, value);
    }

    pub fn map_get(&self, name: &str, key: &str) -> Option<String> {
        self.doc.lock().map_get(name, key)
    }

    pub fn map_entries(&self, name: &str) -> HashMap<String, String> {
        self.doc.lock().map_entries(name)
    }

    /// Read multiple named maps in a single lock acquisition (atomic snapshot).
    ///
    /// Prevents torn reads when callers need a consistent view across several
    /// maps (e.g. `list_items()` joining geo + prop + cnt in one logical read).
    /// Returned `Vec` is in the same order as `names`.
    pub fn map_entries_batch(&self, names: &[&str]) -> Vec<HashMap<String, String>> {
        let doc = self.doc.lock();
        names.iter().map(|name| doc.map_entries(name)).collect()
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

    /// Replace the in-memory doc with `bytes`.
    ///
    /// The sync cursor (seq) lives entirely in the backend KV store — this
    /// call only updates the working copy. Call `pull()` afterwards to merge
    /// any deltas committed since the snapshot was taken.
    pub fn load_snapshot(&self, bytes: &[u8]) -> Result<(), CmError> {
        *self.doc.lock() = CrdtDoc::from_state(bytes)?;
        Ok(())
    }

    /// Merge raw delta bytes directly into the doc.
    /// Used for building fork snapshots without a full push/pull cycle.
    pub fn merge_raw(&self, delta: &[u8]) -> Result<(), CmError> {
        self.doc.lock().merge_delta(delta)
    }

    /// Return the full serialized state of the current doc.
    /// Useful for seeding child branches or checkpointing.
    pub fn to_snapshot(&self) -> Vec<u8> {
        self.doc.lock().full_state()
    }

    // -----------------------------------------------------------------------
    // Push / pull (stateless — cursor derived from backend snapshot)
    // -----------------------------------------------------------------------

    /// Compute the diff above the committed baseline, append it to the backend
    /// delta log, and advance the backend snapshot.
    ///
    /// Returns the assigned `global_seq` so callers (e.g. `add_node_internal`)
    /// can record it as a CRDT watermark on nodes.
    ///
    /// Idempotent: if there are no local changes above the committed baseline,
    /// returns the current frontier seq without writing.
    pub async fn push<B: ContextBackend>(
        &self,
        conv_id: &ConversationId,
        branch_id: &BranchId,
        backend: &B,
    ) -> Result<u64, CmError> {
        // 1. Load committed baseline from backend (seq, state, cached sv).
        let (snap_seq, snap_bytes, snap_sv) = backend.crdt_load_snapshot(branch_id).await?;

        // 2. Fetch all deltas committed since the snapshot.
        let pending = backend.crdt_fetch(conv_id, branch_id, snap_seq).await?;

        // 3. Compute committed baseline sv BEFORE acquiring the doc lock.
        //    Fast path (P1): element-wise max of snap_sv + each delta's cached SV —
        //    O(N × decode_sv) vs O(snap_size + N × delta_size) for the fallback.
        //    Falls back to full reconstruction for legacy deltas without a cached SV.
        let committed_sv = if pending.is_empty() {
            snap_sv
        } else if pending.iter().all(|d| !d.sv.is_empty()) {
            merge_svs(&snap_sv, pending.iter().map(|d| d.sv.as_slice()))
        } else {
            build_sv_from_snapshot_and_deltas(&snap_bytes, &pending)?
        };

        // 4. Single lock: merge pending ops, check no-op, compute diff.
        //    Capture our_sv (cumulative SV = snap + pending + our new ops) to store
        //    alongside the delta.  Future push() calls on other workers use this SV
        //    for the O(N × decode_sv) fast path in merge_svs, avoiding full state replay.
        //    Unconditional merge — CRDT is idempotent; skipping own ops by
        //    client_id breaks stateless workers sharing an application-level id.
        let (no_op, delta, our_sv) = {
            let mut doc = self.doc.lock();
            for d in &pending {
                doc.merge_delta(&d.delta)?;
            }
            let our_sv = doc.state_vector();
            // C2: Use semantic SV comparison — Vec<u8> equality is unreliable
            // because yrs serialises StateVector from a HashMap with non-deterministic
            // iteration order, so two equal SVs may encode to different bytes.
            if svs_equal(&our_sv, &committed_sv) {
                (true, Vec::new(), our_sv)
            } else {
                let delta = doc.encode_diff(&committed_sv);
                (false, delta, our_sv)
            }
        };

        if no_op {
            let max_pending = pending.last().map(|d| d.global_seq).unwrap_or(snap_seq);
            return Ok(max_pending);
        }

        // 5. Append delta to log and return the assigned seq.
        //    our_sv = cumulative SV at push time (snap + pending + new ops).
        //    Stored as delta.sv so merge_svs() can compute the committed baseline in
        //    O(N × decode_sv) instead of replaying full state for each push().
        //    push() deliberately does NOT save a snapshot here — a snapshot built
        //    from the delta set captured in step 2 would be incomplete if concurrent
        //    clients appended between steps 2 and 5. Only pull() builds snapshots,
        //    because it fetches the complete delta set in one shot under one seq cursor.
        let global_seq = backend
            .crdt_append(&CrdtDelta {
                global_seq: 0, // assigned by backend
                client_id: self.client_id.clone(),
                branch_id: branch_id.clone(),
                conversation_id: conv_id.clone(),
                delta,
                sv: our_sv,
                created_at: now_micros(),
            })
            .await?;

        Ok(global_seq)
    }

    /// Bring the in-memory doc up-to-date with the backend:
    ///
    /// 1. Load the committed snapshot and merge it into the doc (idempotent).
    /// 2. Fetch and merge all incremental deltas committed after the snapshot
    ///    (unconditional: CRDT merge is idempotent; own ops are safe to re-apply).
    /// 3. Advance the backend snapshot to cover the incremental deltas.
    /// 4. Notify subscribers if the doc changed.
    pub async fn pull<B: ContextBackend>(
        &self,
        conv_id: &ConversationId,
        branch_id: &BranchId,
        backend: &B,
    ) -> Result<(), CmError> {
        // 1. Load committed baseline (seq, state, cached sv — sv unused in pull).
        let (snap_seq, snap_bytes, _snap_sv) = backend.crdt_load_snapshot(branch_id).await?;

        // 2. Fetch incremental deltas committed after the snapshot.
        let deltas = backend.crdt_fetch(conv_id, branch_id, snap_seq).await?;

        // Nothing on this branch at all.
        if snap_seq == 0 && snap_bytes.is_empty() && deltas.is_empty() {
            return Ok(());
        }

        // 3. Merge snapshot + deltas into our doc (lock held only for the merges).
        //    Unconditional — CRDT merge is idempotent; skipping own ops by client_id
        //    breaks stateless workers that share an application-level client_id but do
        //    not share in-memory doc state.
        let (max_seq, changed) = {
            let mut doc = self.doc.lock();
            let sv_before = doc.state_vector();

            if !snap_bytes.is_empty() {
                doc.merge_delta(&snap_bytes)?;
            }
            let mut max_seq = snap_seq;
            for d in &deltas {
                if d.global_seq > max_seq {
                    max_seq = d.global_seq;
                }
                doc.merge_delta(&d.delta)?;
            }

            let sv_after = doc.state_vector();
            // C2: semantic SV comparison — same fix as push(); Vec<u8> equality is
            // unreliable for yrs StateVector (HashMap iteration order is non-deterministic).
            (max_seq, !svs_equal(&sv_after, &sv_before))
        };
        // Lock released — snap_doc is constructed outside the lock to avoid blocking
        // concurrent doc reads/writes during O(snap+pending) snapshot construction.

        // 4. Advance snapshot in backend if new incremental deltas were present.
        //    Snapshot is built from snap_bytes + deltas only (excludes local writes).
        //    crdt_save_snapshot uses seq-monotonic CAS: a concurrent pull at a higher
        //    seq wins harmlessly (CRDT convergence guarantees identical state).
        if !deltas.is_empty() {
            let mut snap_doc = CrdtDoc::from_state(&snap_bytes)?;
            for d in &deltas {
                snap_doc.merge_delta(&d.delta)?;
            }
            let new_sv = snap_doc.state_vector();
            let new_state = snap_doc.full_state();
            backend.crdt_save_snapshot(branch_id, max_seq, &new_state, &new_sv).await?;
        }

        // 5. Notify subscribers if the local doc changed.
        if changed && let Some(tx) = &self.delta_tx {
            tx.send(max_seq).ok();
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Compaction
    // -----------------------------------------------------------------------

    /// Prune delta log entries that are already covered by the stored snapshot.
    ///
    /// Safe to call at any time: the snapshot is always the authoritative
    /// baseline and deltas covered by it are redundant.
    pub async fn compact<B: ContextBackend>(
        &self,
        conv_id: &ConversationId,
        branch_id: &BranchId,
        backend: &B,
    ) -> Result<(), CmError> {
        let (snap_seq, _, _) = backend.crdt_load_snapshot(branch_id).await?;
        backend.crdt_compact(conv_id, branch_id, snap_seq).await
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
    fn map_tombstone_removes_entry_from_entries() {
        // Application-level tombstone: map_set with a sentinel value that
        // callers filter. map_entries still sees the key, but the entity is
        // logically deleted. This is the correct pattern — yrs map removals
        // can be resurrected by a concurrent insert with a later clock.
        let mut doc = CrdtDoc::new();
        doc.map_set("items", "a", r#"{"id":"a","deleted":true}"#);

        // Key is present (tombstone) but carries the deleted flag.
        assert!(doc.map_get("items", "a").is_some());
        // Callers filter by deserialising and checking deleted == true.
        let entries = doc.map_entries("items");
        assert_eq!(entries.len(), 1);
        assert!(entries["a"].contains("\"deleted\":true"));
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
    async fn push_returns_seq_for_watermark() {
        let backend = make_backend();
        let conv = ConversationId::new();
        let branch = BranchId::new();

        let ext = CrdtExtension::new("client-a");
        ext.map_set("items", "a", r#"{"id":"a"}"#);
        let seq = ext.push(&conv, &branch, backend.as_ref()).await.unwrap();

        assert!(seq > 0, "push must return assigned seq");

        // push() does NOT save a snapshot (prevents lost-update race).
        // Verify the returned seq matches the delta appended to the log.
        let deltas = backend.crdt_fetch(&conv, &branch, 0).await.unwrap();
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].global_seq, seq, "push must return the assigned global_seq");
    }

    #[tokio::test]
    async fn pull_idempotent_on_own_ops() {
        // pull() merges all deltas unconditionally. CRDT merge is idempotent, so
        // re-applying ops the doc already contains is a safe no-op. This test
        // verifies that a client pulling its own previously-pushed delta does not
        // produce duplicate entries (idempotency guarantee in practice).
        let backend = make_backend();
        let conv = ConversationId::new();
        let branch = BranchId::new();

        let writer = CrdtExtension::new("writer");
        writer.map_set("items", "a", r#"{"id":"a"}"#);
        writer.push(&conv, &branch, backend.as_ref()).await.unwrap();

        // Writer pulls its own branch — "a" was already in the doc before push.
        // After pull it should still be there exactly once (idempotent re-apply).
        writer.pull(&conv, &branch, backend.as_ref()).await.unwrap();
        let entries = writer.map_entries("items");
        assert_eq!(entries.len(), 1, "own data must not be duplicated after pull");
        assert_eq!(entries["a"], r#"{"id":"a"}"#);
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
    async fn pull_advances_snapshot_seq() {
        let backend = make_backend();
        let conv = ConversationId::new();
        let branch = BranchId::new();

        // Writer pushes two deltas independently.
        let writer = CrdtExtension::new("writer");
        writer.map_set("items", "a", r#"{"id":"a"}"#);
        let seq1 = writer.push(&conv, &branch, backend.as_ref()).await.unwrap();

        let writer2 = CrdtExtension::new("writer2");
        writer2.map_set("items", "b", r#"{"id":"b"}"#);
        let seq2 = writer2.push(&conv, &branch, backend.as_ref()).await.unwrap();
        assert!(seq2 > seq1);

        // Fresh reader with no snapshot — pull must advance snapshot to max delta seq.
        let reader = CrdtExtension::new("reader");
        reader.pull(&conv, &branch, backend.as_ref()).await.unwrap();

        let (snap_seq, _, _) = backend.crdt_load_snapshot(&branch).await.unwrap();
        assert!(snap_seq >= seq2, "snapshot seq must be >= max delta seq after pull");
    }

    #[tokio::test]
    async fn load_snapshot_then_pull_no_double_apply() {
        let backend = make_backend();
        let conv = ConversationId::new();
        let branch = BranchId::new();

        let writer = CrdtExtension::new("writer");
        writer.map_set("items", "a", r#"{"id":"a"}"#);
        writer.push(&conv, &branch, backend.as_ref()).await.unwrap();

        let snapshot_bytes = writer.to_snapshot();

        // New extension loads snapshot then pulls — must not double-apply.
        let reader = CrdtExtension::new("reader");
        reader.load_snapshot(&snapshot_bytes).unwrap();
        reader.pull(&conv, &branch, backend.as_ref()).await.unwrap();

        // Data is present exactly once.
        assert_eq!(reader.map_get("items", "a"), Some(r#"{"id":"a"}"#.into()));
    }

    #[tokio::test]
    async fn load_snapshot_only_updates_doc() {
        // After load_snapshot(), push should only send NEW ops, not resend snapshot.
        let backend = make_backend();
        let conv = ConversationId::new();
        let branch = BranchId::new();

        // Another client establishes the baseline snapshot.
        let baseline = CrdtExtension::new("baseline");
        baseline.map_set("items", "existing", r#"{"id":"existing"}"#);
        baseline.push(&conv, &branch, backend.as_ref()).await.unwrap();

        // New client loads snapshot, adds one new item, pushes.
        let client = CrdtExtension::new("client");
        let snap = baseline.to_snapshot();
        client.load_snapshot(&snap).unwrap();
        client.map_set("items", "new", r#"{"id":"new"}"#);
        client.push(&conv, &branch, backend.as_ref()).await.unwrap();

        // Only 2 deltas total: baseline's push + client's push (not a full resend).
        let all_deltas = backend.crdt_fetch(&conv, &branch, 0).await.unwrap();
        assert_eq!(all_deltas.len(), 2, "push after load_snapshot must send only new ops");
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
    async fn push_derives_cursor_from_backend() {
        // After checkout (load_snapshot + pull), a subsequent push must send
        // ONLY the new ops added after checkout, not the full snapshot state.
        let backend = make_backend();
        let conv = ConversationId::new();
        let branch = BranchId::new();

        // Step 1: Establish some baseline state on the branch.
        let init = CrdtExtension::new("init");
        init.map_set("items", "base", r#"{"id":"base"}"#);
        init.push(&conv, &branch, backend.as_ref()).await.unwrap();

        // Step 2: New client checks out (simulates checkout flow).
        // push() does not save a snapshot, so crdt_load_snapshot returns (0, [], []).
        // load_snapshot([]) is a no-op; pull() fetches init's delta and populates the doc.
        let (_, snap_bytes, _) = backend.crdt_load_snapshot(&branch).await.unwrap();
        let client = CrdtExtension::new("client");
        client.load_snapshot(&snap_bytes).unwrap();
        client.pull(&conv, &branch, backend.as_ref()).await.unwrap();

        // Step 3: Client adds new data and pushes.
        client.map_set("items", "new", r#"{"id":"new"}"#);
        client.push(&conv, &branch, backend.as_ref()).await.unwrap();

        // Only 2 deltas: init's push + client's push.
        let all = backend.crdt_fetch(&conv, &branch, 0).await.unwrap();
        assert_eq!(all.len(), 2, "checkout + push must not resend snapshot state");
    }

    #[tokio::test]
    async fn concurrent_pushes_converge() {
        let backend = make_backend();
        let conv = ConversationId::new();
        let branch = BranchId::new();

        let a = Arc::new(CrdtExtension::new("a"));
        let b = Arc::new(CrdtExtension::new("b"));

        a.map_set("items", "ka", r#"{"id":"ka"}"#);
        b.map_set("items", "kb", r#"{"id":"kb"}"#);

        let backend_a = Arc::clone(&backend);
        let backend_b = Arc::clone(&backend);
        let a_ref = Arc::clone(&a);
        let b_ref = Arc::clone(&b);
        let conv_a = conv.clone();
        let conv_b = conv.clone();
        let branch_a = branch.clone();
        let branch_b = branch.clone();

        let ha = tokio::spawn(async move {
            a_ref.push(&conv_a, &branch_a, backend_a.as_ref()).await.unwrap();
        });
        let hb = tokio::spawn(async move {
            b_ref.push(&conv_b, &branch_b, backend_b.as_ref()).await.unwrap();
        });
        ha.await.unwrap();
        hb.await.unwrap();

        // Pull both sides.
        a.pull(&conv, &branch, backend.as_ref()).await.unwrap();
        b.pull(&conv, &branch, backend.as_ref()).await.unwrap();

        assert_eq!(a.map_entries("items").len(), 2, "a must see both after pull");
        assert_eq!(b.map_entries("items").len(), 2, "b must see both after pull");
    }

    #[tokio::test]
    async fn load_snapshot_empty_bytes_is_blank_doc() {
        let ext = CrdtExtension::new("c");
        ext.load_snapshot(&[]).unwrap();
        assert!(ext.map_entries("items").is_empty());
    }

    #[tokio::test]
    async fn compact_prunes_deltas() {
        let backend = make_backend();
        let conv = ConversationId::new();
        let branch = BranchId::new();

        let ext = CrdtExtension::new("client");
        for i in 0..10u32 {
            ext.map_set("items", &i.to_string(), &format!(r#"{{"id":"{}"}}"#, i));
            ext.push(&conv, &branch, backend.as_ref()).await.unwrap();
        }

        let before = backend.crdt_fetch(&conv, &branch, 0).await.unwrap();
        assert_eq!(before.len(), 10);

        // pull() must be called first to advance the snapshot (push does not save one).
        ext.pull(&conv, &branch, backend.as_ref()).await.unwrap();
        ext.compact(&conv, &branch, backend.as_ref()).await.unwrap();

        let after = backend.crdt_fetch(&conv, &branch, 0).await.unwrap();
        assert!(after.is_empty(), "compact must prune all snapshot-covered deltas");
    }

    #[tokio::test]
    async fn subscribe_fires_on_pull() {
        let backend = make_backend();
        let conv = ConversationId::new();
        let branch = BranchId::new();

        // Writer pushes some data.
        let writer = CrdtExtension::new("writer");
        writer.map_set("items", "a", r#"{"id":"a"}"#);
        let pushed_seq = writer.push(&conv, &branch, backend.as_ref()).await.unwrap();

        // Reader subscribes to notifications.
        let reader = CrdtExtension::new("reader").with_notifications(16);
        let mut rx = reader.subscribe().unwrap();

        reader.pull(&conv, &branch, backend.as_ref()).await.unwrap();

        let notified_seq = rx.try_recv().expect("subscriber must receive seq on pull");
        assert_eq!(notified_seq, pushed_seq, "notified seq must match pushed seq");
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
        let push_seq = parent.push(&conv, &parent_branch, backend.as_ref()).await.unwrap();
        assert!(push_seq > 0);

        // Capture snapshot at this point.
        let snapshot_bytes = parent.to_snapshot();

        // Parent adds more after the snapshot.
        parent.map_set("items", "k2", r#"{"id":"k2"}"#);
        parent.push(&conv, &parent_branch, backend.as_ref()).await.unwrap();

        // Build child from the snapshot (before k2 was added).
        let child_branch = BranchId::new();
        let child = CrdtExtension::new("child");
        child.load_snapshot(&snapshot_bytes).unwrap();

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
        let gp_bytes = grandparent.to_snapshot();

        let parent_branch = BranchId::new();
        let parent = CrdtExtension::new("parent");
        parent.load_snapshot(&gp_bytes).unwrap();
        parent.map_set("items", "parent_key", r#"{"id":"parent_key"}"#);
        parent.push(&conv, &parent_branch, backend.as_ref()).await.unwrap();

        // Child snapshot = parent full state (includes gp_key via snapshot).
        let parent_bytes = parent.to_snapshot();

        let child_branch = BranchId::new();
        let child = CrdtExtension::new("child");
        child.load_snapshot(&parent_bytes).unwrap();

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
