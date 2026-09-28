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
        let update = Update::decode_v1(state).map_err(|e| CmError::Crdt(e.to_string()))?;
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
        let update = Update::decode_v1(delta).map_err(|e| CmError::Crdt(e.to_string()))?;
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
    /// Create a new extension with an empty doc.
    ///
    /// `client_id` is stored in every delta pushed by this instance — use a stable
    /// identity (worker name, pod ID) to avoid unbounded Yjs state-vector growth
    /// across restarts (each unique ID adds one entry to the SV).
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

    /// The identity string used to tag outgoing deltas.
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

    /// Insert or overwrite a key in the named CRDT map.
    pub fn map_set(&self, name: &str, key: &str, value: &str) {
        self.doc.lock().map_set(name, key, value);
    }

    /// Read a single key from the named CRDT map.
    pub fn map_get(&self, name: &str, key: &str) -> Option<String> {
        self.doc.lock().map_get(name, key)
    }

    /// Return all entries in the named CRDT map.
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

    /// Insert `content` into the named Y.Text at character `index`.
    pub fn text_insert(&self, name: &str, index: u32, content: &str) {
        self.doc.lock().text_insert(name, index, content);
    }

    /// Delete `len` characters starting at `index` from the named Y.Text.
    pub fn text_remove(&self, name: &str, index: u32, len: u32) {
        self.doc.lock().text_remove(name, index, len);
    }

    /// Read the full content of the named Y.Text.
    pub fn text_read(&self, name: &str) -> String {
        self.doc.lock().text_read(name)
    }

    /// Character length of the named Y.Text, or 0 if it does not exist.
    pub fn text_len(&self, name: &str) -> u32 {
        self.doc.lock().text_len(name)
    }

    /// Encode the full doc state as a Yjs v1 update (snapshot).
    pub fn full_state(&self) -> Vec<u8> {
        self.doc.lock().full_state()
    }

    /// Encode the current state vector (used as the diff baseline by remote peers).
    pub fn state_vector(&self) -> Vec<u8> {
        self.doc.lock().state_vector()
    }

    /// Encode the diff between the local doc and `remote_sv` (ops the remote doesn't have yet).
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
            backend
                .crdt_save_snapshot(branch_id, max_seq, &new_state, &new_sv)
                .await?;
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
#[cfg(test)]
#[path = "crdt_tests.rs"]
mod tests;
