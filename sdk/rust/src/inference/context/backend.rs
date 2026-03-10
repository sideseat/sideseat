use std::collections::HashMap;

use async_trait::async_trait;
use parking_lot::Mutex;

use super::error::CmError;
use super::sync::CrdtDelta;
use super::types::{
    BranchId, BranchMeta, ConversationId, Node, NodeContent, NodeHeader, NodeId, StreamingState,
};
use crate::types::Usage;

// ---------------------------------------------------------------------------
// Query params
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct ListParams {
    pub offset: Option<u64>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct ListNodesParams {
    pub conversation_id: ConversationId,
    pub branch_id: Option<BranchId>,
    pub after_sequence: Option<u64>,
    pub limit: Option<u32>,
    pub include_deleted: bool,
    pub content_types: Option<Vec<String>>,
    pub time_range: Option<TimeRange>,
}

#[derive(Debug, Clone)]
pub struct TimeRange {
    pub start: i64,
    pub end: i64,
}

// ---------------------------------------------------------------------------
// NodePatch — versioned partial update
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct NodePatch {
    pub content: Option<NodeContent>,
    pub is_final: Option<bool>,
    pub streaming: Option<Option<StreamingState>>,
    pub usage: Option<Usage>,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    pub eval_scores: Option<Vec<super::types::EvalScore>>,
    /// Update the CRDT log watermark on the node (used by `finalize_streaming_node`
    /// to record the seq at which the streaming content was pushed).
    pub crdt_seq_watermark: Option<Option<u64>>,
}

// ---------------------------------------------------------------------------
// ContextBackend trait
// ---------------------------------------------------------------------------

/// Minimal storage backend for ContextManager.
///
/// Only nodes and branches have typed first-class methods. Everything else
/// (conversations, canvas, kanban, artifact, memory, prompts, datasets, VFS)
/// lives in the generic KV store.
///
/// Append-only: nodes are never overwritten; `update_node` is a versioned insert.
/// Pass `expected_version = u64::MAX` to skip the version check (used for streaming finalization).
#[async_trait]
pub trait ContextBackend: Send + Sync {
    // -----------------------------------------------------------------------
    // Nodes — typed; append-only
    // -----------------------------------------------------------------------

    async fn append_nodes(&self, nodes: &[Node]) -> Result<(), CmError>;

    async fn get_node(&self, id: &NodeId) -> Result<Option<Node>, CmError>;

    async fn get_nodes(&self, ids: &[NodeId]) -> Result<Vec<Node>, CmError>;

    /// Returns ALL node headers for a conversation, across all branches.
    /// Required for `linearize()` to work after `load()`.
    async fn list_node_headers(&self, conv_id: &ConversationId) -> Result<Vec<NodeHeader>, CmError>;

    async fn list_nodes(&self, params: &ListNodesParams) -> Result<Vec<Node>, CmError>;

    /// Versioned insert. Pass `expected_version = u64::MAX` to skip version check.
    async fn update_node(
        &self,
        id: &NodeId,
        patch: &NodePatch,
        expected_version: u64,
    ) -> Result<(), CmError>;

    async fn soft_delete_node(&self, id: &NodeId) -> Result<(), CmError>;

    async fn search_nodes(
        &self,
        conv_id: &ConversationId,
        query: &str,
        params: &ListParams,
    ) -> Result<Vec<NodeHeader>, CmError>;

    // -----------------------------------------------------------------------
    // Branches — typed; per-conversation listing for tree reconstruction
    // -----------------------------------------------------------------------

    async fn save_branch(&self, branch: &BranchMeta) -> Result<(), CmError>;

    async fn list_branches(
        &self,
        conv_id: &ConversationId,
    ) -> Result<Vec<BranchMeta>, CmError>;

    async fn delete_branch(&self, id: &BranchId) -> Result<(), CmError>;

    // -----------------------------------------------------------------------
    // Generic KV — conversations, canvas, kanban, artifact, memory, etc.
    // `kv_list` returns keys in lexicographic order.
    // -----------------------------------------------------------------------

    async fn kv_put(&self, ns: &str, key: &str, value: &[u8]) -> Result<(), CmError>;

    async fn kv_get(&self, ns: &str, key: &str) -> Result<Option<Vec<u8>>, CmError>;

    /// Returns keys in lexicographic order. Zero-pad all numeric keys to ensure lex == numeric order.
    async fn kv_list(&self, ns: &str, prefix: &str) -> Result<Vec<String>, CmError>;

    async fn kv_delete(&self, ns: &str, key: &str) -> Result<(), CmError>;

    // -----------------------------------------------------------------------
    // CRDT delta log — typed; per-branch delta logs; monotonic seq numbering
    // -----------------------------------------------------------------------

    /// Persist a CRDT delta. Returns the assigned global sequence number.
    async fn crdt_append(&self, delta: &CrdtDelta) -> Result<u64, CmError>;

    /// Return all deltas for `branch_id` where `global_seq > after_seq`.
    ///
    /// Results MUST be ordered by `global_seq` ascending. `push()` relies on
    /// `pending.last()` being the highest-seq entry to derive the committed
    /// baseline SV correctly.
    async fn crdt_fetch(
        &self,
        conv_id: &ConversationId,
        branch_id: &BranchId,
        after_seq: u64,
    ) -> Result<Vec<CrdtDelta>, CmError>;

    /// Delete delta log entries covered by the snapshot (global_seq <= snapshot_seq).
    /// Safe to call at any time — the snapshot is the authoritative baseline.
    async fn crdt_compact(
        &self,
        conv_id: &ConversationId,
        branch_id: &BranchId,
        snapshot_seq: u64,
    ) -> Result<(), CmError>;

    /// Load the committed CRDT snapshot for a branch.
    /// Returns `(0, [], [])` when no snapshot has been saved yet.
    /// The third element is the pre-computed state vector of the snapshot state,
    /// enabling O(1) diff baseline computation in `push()` when no pending deltas exist.
    async fn crdt_load_snapshot(
        &self,
        branch_id: &BranchId,
    ) -> Result<(u64, Vec<u8>, Vec<u8>), CmError>;

    /// Persist a CRDT snapshot using seq-monotonic semantics: only stores the new
    /// snapshot if `seq` is strictly greater than the currently stored seq.
    ///
    /// This prevents two concurrent pulls from regressing the snapshot cursor —
    /// whichever write arrives last simply loses, and the stored snapshot stays at
    /// the higher seq (CRDT convergence guarantees identical content at the same seq).
    ///
    /// `sv` must be the pre-computed state vector of `state`, passed by the caller
    /// to avoid redundant CrdtDoc construction on the next push() call.
    async fn crdt_save_snapshot(
        &self,
        branch_id: &BranchId,
        seq: u64,
        state: &[u8],
        sv: &[u8],
    ) -> Result<(), CmError>;
}

// ---------------------------------------------------------------------------
// InMemoryContextBackend
// ---------------------------------------------------------------------------

struct InMemoryState {
    /// All versions of each node. `get_node` returns max(version).
    nodes: HashMap<NodeId, Vec<Node>>,
    /// All node headers, cross-branch. Required for `linearize()` after `load()`.
    node_headers: HashMap<ConversationId, Vec<NodeHeader>>,
    branches: HashMap<ConversationId, Vec<BranchMeta>>,
    kv: HashMap<(String, String), Vec<u8>>,
    /// Per-branch delta log: key = (conv_id, branch_id) for O(branch_deltas) fetch.
    /// Using a flat Vec<CrdtDelta> would require scanning all deltas globally on every
    /// push/pull (O(total_deltas)); at 1000 ops/sec with no compaction that is ~3.6M
    /// entries per hour scanned per push.
    crdt_deltas: HashMap<(ConversationId, BranchId), Vec<CrdtDelta>>,
    /// Per-branch CRDT snapshots: (seq, state_bytes, sv_bytes).
    /// `sv_bytes` is the pre-computed state vector of `state_bytes`, cached so
    /// push() can derive the committed baseline SV in O(1) when no pending deltas exist.
    /// Stored separately from the generic KV store so `crdt_save_snapshot` can
    /// apply seq-monotonic CAS atomically under one lock.
    crdt_snapshots: HashMap<BranchId, (u64, Vec<u8>, Vec<u8>)>,
    next_seq: u64,
}

impl InMemoryState {
    fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            node_headers: HashMap::new(),
            branches: HashMap::new(),
            kv: HashMap::new(),
            crdt_deltas: HashMap::new(),
            crdt_snapshots: HashMap::new(),
            next_seq: 1,
        }
    }
}

pub struct InMemoryContextBackend {
    state: Mutex<InMemoryState>,
}

impl InMemoryContextBackend {
    pub fn new() -> Self {
        Self { state: Mutex::new(InMemoryState::new()) }
    }
}

impl Default for InMemoryContextBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ContextBackend for InMemoryContextBackend {
    async fn append_nodes(&self, nodes: &[Node]) -> Result<(), CmError> {
        let mut state = self.state.lock();
        for node in nodes {
            let header = NodeHeader::from(node);
            let conv_headers = state
                .node_headers
                .entry(node.conversation_id.clone())
                .or_default();
            // Replace existing header if present (idempotent)
            if let Some(pos) = conv_headers.iter().position(|h| h.id == node.id) {
                conv_headers[pos] = header;
            } else {
                conv_headers.push(header);
            }
            state
                .nodes
                .entry(node.id.clone())
                .or_default()
                .push(node.clone());
        }
        Ok(())
    }

    async fn get_node(&self, id: &NodeId) -> Result<Option<Node>, CmError> {
        let state = self.state.lock();
        let node = state
            .nodes
            .get(id)
            .and_then(|versions| versions.iter().max_by_key(|n| n.version))
            .cloned();
        Ok(node)
    }

    async fn get_nodes(&self, ids: &[NodeId]) -> Result<Vec<Node>, CmError> {
        let state = self.state.lock();
        let nodes = ids
            .iter()
            .filter_map(|id| {
                state
                    .nodes
                    .get(id)
                    .and_then(|versions| versions.iter().max_by_key(|n| n.version))
                    .cloned()
            })
            .collect();
        Ok(nodes)
    }

    async fn list_node_headers(
        &self,
        conv_id: &ConversationId,
    ) -> Result<Vec<NodeHeader>, CmError> {
        let state = self.state.lock();
        Ok(state
            .node_headers
            .get(conv_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn list_nodes(&self, params: &ListNodesParams) -> Result<Vec<Node>, CmError> {
        let headers = {
            let state = self.state.lock();
            state
                .node_headers
                .get(&params.conversation_id)
                .cloned()
                .unwrap_or_default()
        };

        let mut filtered: Vec<&NodeHeader> = headers
            .iter()
            .filter(|h| {
                if !params.include_deleted && h.deleted {
                    return false;
                }
                if let Some(branch_id) = &params.branch_id
                    && &h.branch_id != branch_id
                {
                    return false;
                }
                if let Some(after_seq) = params.after_sequence
                    && h.sequence <= after_seq
                {
                    return false;
                }
                if let Some(time_range) = &params.time_range
                    && (h.created_at < time_range.start || h.created_at > time_range.end)
                {
                    return false;
                }
                if let Some(types) = &params.content_types
                    && !types.contains(&h.content_type)
                {
                    return false;
                }
                true
            })
            .collect();

        // Sort by sequence so limit truncates the oldest, not arbitrary entries.
        filtered.sort_by_key(|h| h.sequence);

        let mut ids: Vec<NodeId> = filtered.iter().map(|h| h.id.clone()).collect();
        if let Some(limit) = params.limit {
            ids.truncate(limit as usize);
        }

        self.get_nodes(&ids).await
    }

    async fn update_node(
        &self,
        id: &NodeId,
        patch: &NodePatch,
        expected_version: u64,
    ) -> Result<(), CmError> {
        let mut state = self.state.lock();

        // Build the updated node (scope ends before we borrow headers).
        let updated = {
            let versions = state
                .nodes
                .get_mut(id)
                .ok_or_else(|| CmError::NodeNotFound(id.clone()))?;

            let current = versions
                .iter()
                .max_by_key(|n| n.version)
                .ok_or_else(|| CmError::NodeNotFound(id.clone()))?;

            // Skip version check when expected = u64::MAX (streaming finalization)
            if expected_version != u64::MAX && current.version != expected_version {
                return Err(CmError::VersionMismatch {
                    id: id.to_string(),
                    expected: expected_version,
                    actual: current.version,
                });
            }

            let mut updated = current.clone();
            updated.version += 1;

            if let Some(content) = patch.content.clone() { updated.content = content; }
            if let Some(is_final) = patch.is_final { updated.is_final = is_final; }
            if let Some(streaming) = patch.streaming.clone() { updated.streaming = streaming; }
            if let Some(usage) = patch.usage.clone() { updated.usage = Some(usage); }
            if let Some(meta) = patch.metadata.clone() { updated.metadata.extend(meta); }
            if let Some(scores) = patch.eval_scores.clone() { updated.eval_scores = scores; }
            if let Some(watermark) = patch.crdt_seq_watermark { updated.crdt_seq_watermark = watermark; }
            updated
        };

        // Update header (separate borrow from `versions`).
        let conv_headers = state
            .node_headers
            .entry(updated.conversation_id.clone())
            .or_default();
        if let Some(pos) = conv_headers.iter().position(|h| h.id == *id) {
            conv_headers[pos] = NodeHeader::from(&updated);
        }

        state.nodes.get_mut(id).unwrap().push(updated);
        Ok(())
    }

    async fn soft_delete_node(&self, id: &NodeId) -> Result<(), CmError> {
        let mut state = self.state.lock();

        let (deleted, conv_id) = {
            let versions = state
                .nodes
                .get_mut(id)
                .ok_or_else(|| CmError::NodeNotFound(id.clone()))?;

            let current = versions
                .iter()
                .max_by_key(|n| n.version)
                .ok_or_else(|| CmError::NodeNotFound(id.clone()))?
                .clone();

            let mut deleted = current;
            deleted.version += 1;
            deleted.deleted = true;
            let conv_id = deleted.conversation_id.clone();
            (deleted, conv_id)
        };

        // Update header (separate borrow from `versions`).
        if let Some(headers) = state.node_headers.get_mut(&conv_id)
            && let Some(pos) = headers.iter().position(|h| h.id == *id)
        {
            headers[pos].deleted = true;
        }

        state.nodes.get_mut(id).unwrap().push(deleted);
        Ok(())
    }

    async fn search_nodes(
        &self,
        conv_id: &ConversationId,
        query: &str,
        params: &ListParams,
    ) -> Result<Vec<NodeHeader>, CmError> {
        let state = self.state.lock();
        let query_lower = query.to_lowercase();

        let mut results: Vec<NodeHeader> = state
            .node_headers
            .get(conv_id)
            .map(|headers| {
                headers
                    .iter()
                    .filter(|h| {
                        // Simple substring match on content_type for in-memory backend
                        h.content_type.to_lowercase().contains(&query_lower)
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();

        if let Some(offset) = params.offset {
            let skip = offset as usize;
            if skip >= results.len() {
                results.clear();
            } else {
                results = results[skip..].to_vec();
            }
        }
        if let Some(limit) = params.limit {
            results.truncate(limit as usize);
        }

        Ok(results)
    }

    async fn save_branch(&self, branch: &BranchMeta) -> Result<(), CmError> {
        let mut state = self.state.lock();
        let branches = state
            .branches
            .entry(branch.conversation_id.clone())
            .or_default();
        if let Some(pos) = branches.iter().position(|b| b.id == branch.id) {
            branches[pos] = branch.clone();
        } else {
            branches.push(branch.clone());
        }
        Ok(())
    }

    async fn list_branches(
        &self,
        conv_id: &ConversationId,
    ) -> Result<Vec<BranchMeta>, CmError> {
        let state = self.state.lock();
        Ok(state
            .branches
            .get(conv_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn delete_branch(&self, id: &BranchId) -> Result<(), CmError> {
        let mut state = self.state.lock();
        for branches in state.branches.values_mut() {
            branches.retain(|b| &b.id != id);
        }
        Ok(())
    }

    async fn kv_put(&self, ns: &str, key: &str, value: &[u8]) -> Result<(), CmError> {
        self.state
            .lock()
            .kv
            .insert((ns.to_string(), key.to_string()), value.to_vec());
        Ok(())
    }

    async fn kv_get(&self, ns: &str, key: &str) -> Result<Option<Vec<u8>>, CmError> {
        Ok(self
            .state
            .lock()
            .kv
            .get(&(ns.to_string(), key.to_string()))
            .cloned())
    }

    async fn kv_list(&self, ns: &str, prefix: &str) -> Result<Vec<String>, CmError> {
        let state = self.state.lock();
        let mut keys: Vec<String> = state
            .kv
            .keys()
            .filter(|(n, k)| n == ns && k.starts_with(prefix))
            .map(|(_, k)| k.clone())
            .collect();
        keys.sort();
        Ok(keys)
    }

    async fn kv_delete(&self, ns: &str, key: &str) -> Result<(), CmError> {
        self.state
            .lock()
            .kv
            .remove(&(ns.to_string(), key.to_string()));
        Ok(())
    }

    async fn crdt_append(&self, delta: &CrdtDelta) -> Result<u64, CmError> {
        let mut state = self.state.lock();
        let seq = state.next_seq;
        state.next_seq += 1;
        let mut d = delta.clone();
        d.global_seq = seq;
        state
            .crdt_deltas
            .entry((delta.conversation_id.clone(), delta.branch_id.clone()))
            .or_default()
            .push(d);
        Ok(seq)
    }

    async fn crdt_fetch(
        &self,
        conv_id: &ConversationId,
        branch_id: &BranchId,
        after_seq: u64,
    ) -> Result<Vec<CrdtDelta>, CmError> {
        let state = self.state.lock();
        Ok(state
            .crdt_deltas
            .get(&(conv_id.clone(), branch_id.clone()))
            .map(|branch_deltas| {
                branch_deltas
                    .iter()
                    .filter(|d| d.global_seq > after_seq)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn crdt_compact(
        &self,
        conv_id: &ConversationId,
        branch_id: &BranchId,
        snapshot_seq: u64,
    ) -> Result<(), CmError> {
        let mut state = self.state.lock();
        if let Some(branch_deltas) =
            state.crdt_deltas.get_mut(&(conv_id.clone(), branch_id.clone()))
        {
            branch_deltas.retain(|d| d.global_seq > snapshot_seq);
        }
        Ok(())
    }

    async fn crdt_load_snapshot(
        &self,
        branch_id: &BranchId,
    ) -> Result<(u64, Vec<u8>, Vec<u8>), CmError> {
        Ok(self
            .state
            .lock()
            .crdt_snapshots
            .get(branch_id)
            .cloned()
            .unwrap_or((0, Vec::new(), Vec::new())))
    }

    async fn crdt_save_snapshot(
        &self,
        branch_id: &BranchId,
        seq: u64,
        state: &[u8],
        sv: &[u8],
    ) -> Result<(), CmError> {
        let mut s = self.state.lock();
        let current_seq =
            s.crdt_snapshots.get(branch_id).map(|(seq, _, _)| *seq).unwrap_or(0);
        if seq > current_seq {
            s.crdt_snapshots
                .insert(branch_id.clone(), (seq, state.to_vec(), sv.to_vec()));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::types::{BranchId, ConversationId, NodeContent, now_micros};
    use crate::types::ContentBlock;

    fn make_node(
        conv_id: &ConversationId,
        branch_id: &BranchId,
        parent_id: Option<NodeId>,
        seq: u64,
    ) -> Node {
        Node {
            id: NodeId::new(),
            conversation_id: conv_id.clone(),
            branch_id: branch_id.clone(),
            parent_id,
            sequence: seq,
            created_at: now_micros(),
            created_by: None,
            model: None,
            provider: None,
            content: NodeContent::UserMessage {
                content: vec![ContentBlock::text("test")],
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
            metadata: HashMap::new(),
            crdt_seq_watermark: None,
        }
    }

    #[tokio::test]
    async fn kv_crud() {
        let backend = InMemoryContextBackend::new();
        backend.kv_put("ns", "key1", b"value1").await.unwrap();
        backend.kv_put("ns", "key2", b"value2").await.unwrap();

        assert_eq!(backend.kv_get("ns", "key1").await.unwrap(), Some(b"value1".to_vec()));
        assert_eq!(backend.kv_get("ns", "key3").await.unwrap(), None);

        let keys = backend.kv_list("ns", "").await.unwrap();
        assert_eq!(keys, vec!["key1", "key2"]);

        backend.kv_delete("ns", "key1").await.unwrap();
        assert_eq!(backend.kv_get("ns", "key1").await.unwrap(), None);
    }

    #[tokio::test]
    async fn kv_zero_padded_key_ordering() {
        let backend = InMemoryContextBackend::new();
        // Zero-padded keys for lex == numeric order
        backend.kv_put("art", "v00000000000000000001", b"v1").await.unwrap();
        backend.kv_put("art", "v00000000000000000010", b"v10").await.unwrap();
        backend.kv_put("art", "v00000000000000000002", b"v2").await.unwrap();

        let keys = backend.kv_list("art", "v").await.unwrap();
        assert_eq!(keys[0], "v00000000000000000001");
        assert_eq!(keys[1], "v00000000000000000002");
        assert_eq!(keys[2], "v00000000000000000010");
    }

    #[tokio::test]
    async fn node_append_get() {
        let backend = InMemoryContextBackend::new();
        let conv_id = ConversationId::new();
        let branch_id = BranchId::new();

        let node = make_node(&conv_id, &branch_id, None, 0);
        let node_id = node.id.clone();

        backend.append_nodes(&[node]).await.unwrap();

        let fetched = backend.get_node(&node_id).await.unwrap().unwrap();
        assert_eq!(fetched.id, node_id);
        assert_eq!(fetched.version, 0);
    }

    #[tokio::test]
    async fn versioned_update_node() {
        let backend = InMemoryContextBackend::new();
        let conv_id = ConversationId::new();
        let branch_id = BranchId::new();

        let node = make_node(&conv_id, &branch_id, None, 0);
        let node_id = node.id.clone();
        backend.append_nodes(&[node]).await.unwrap();

        let patch = NodePatch {
            is_final: Some(true),
            ..Default::default()
        };
        backend.update_node(&node_id, &patch, 0).await.unwrap();

        let updated = backend.get_node(&node_id).await.unwrap().unwrap();
        assert_eq!(updated.version, 1);
        assert!(updated.is_final);
    }

    #[tokio::test]
    async fn update_node_skip_version_check() {
        let backend = InMemoryContextBackend::new();
        let conv_id = ConversationId::new();
        let branch_id = BranchId::new();

        let node = make_node(&conv_id, &branch_id, None, 0);
        let node_id = node.id.clone();
        backend.append_nodes(&[node]).await.unwrap();

        // u64::MAX skips version check
        let patch = NodePatch { is_final: Some(true), ..Default::default() };
        backend.update_node(&node_id, &patch, u64::MAX).await.unwrap();
        let updated = backend.get_node(&node_id).await.unwrap().unwrap();
        assert_eq!(updated.version, 1);
    }

    #[tokio::test]
    async fn list_node_headers_cross_branch() {
        let backend = InMemoryContextBackend::new();
        let conv_id = ConversationId::new();
        let branch_a = BranchId::new();
        let branch_b = BranchId::new();

        let n1 = make_node(&conv_id, &branch_a, None, 0);
        let n2 = make_node(&conv_id, &branch_b, None, 1);
        backend.append_nodes(&[n1, n2]).await.unwrap();

        let headers = backend.list_node_headers(&conv_id).await.unwrap();
        assert_eq!(headers.len(), 2);
    }

    #[tokio::test]
    async fn crdt_fetch_branch_filtered() {
        let backend = InMemoryContextBackend::new();
        let conv_id = ConversationId::new();
        let branch_a = BranchId::new();
        let branch_b = BranchId::new();

        let d_a = CrdtDelta {
            global_seq: 0,
            client_id: "c1".into(),
            branch_id: branch_a.clone(),
            conversation_id: conv_id.clone(),
            delta: b"for-a".to_vec(),
            sv: Vec::new(),
            created_at: 0,
        };
        let d_b = CrdtDelta {
            global_seq: 0,
            client_id: "c1".into(),
            branch_id: branch_b.clone(),
            conversation_id: conv_id.clone(),
            delta: b"for-b".to_vec(),
            sv: Vec::new(),
            created_at: 0,
        };

        backend.crdt_append(&d_a).await.unwrap();
        backend.crdt_append(&d_b).await.unwrap();

        let a_deltas = backend.crdt_fetch(&conv_id, &branch_a, 0).await.unwrap();
        let b_deltas = backend.crdt_fetch(&conv_id, &branch_b, 0).await.unwrap();

        assert_eq!(a_deltas.len(), 1);
        assert_eq!(b_deltas.len(), 1);
        assert_eq!(a_deltas[0].delta, b"for-a");
        assert_eq!(b_deltas[0].delta, b"for-b");
    }

    #[tokio::test]
    async fn crdt_next_seq_monotonic() {
        let backend = InMemoryContextBackend::new();
        let conv_id = ConversationId::new();
        let branch_id = BranchId::new();

        let make_delta = |conv_id: &ConversationId, branch_id: &BranchId| CrdtDelta {
            global_seq: 0,
            client_id: "c".into(),
            branch_id: branch_id.clone(),
            conversation_id: conv_id.clone(),
            delta: vec![],
            sv: Vec::new(),
            created_at: 0,
        };

        let s1 = backend.crdt_append(&make_delta(&conv_id, &branch_id)).await.unwrap();
        let s2 = backend.crdt_append(&make_delta(&conv_id, &branch_id)).await.unwrap();
        let s3 = backend.crdt_append(&make_delta(&conv_id, &branch_id)).await.unwrap();

        assert!(s1 < s2 && s2 < s3);
    }

    #[tokio::test]
    async fn conversation_patch_lex_order() {
        let backend = InMemoryContextBackend::new();
        let ns = "conv:test";

        // Insert patches out of time order
        backend.kv_put(ns, "patch:000000000000000002:c1:00000000000000000001", b"p2").await.unwrap();
        backend.kv_put(ns, "patch:000000000000000001:c1:00000000000000000000", b"p1").await.unwrap();
        backend.kv_put(ns, "patch:000000000000000003:c1:00000000000000000000", b"p3").await.unwrap();

        let keys = backend.kv_list(ns, "patch:").await.unwrap();
        // Lex order = time order for zero-padded timestamps
        assert_eq!(keys[0].as_str(), "patch:000000000000000001:c1:00000000000000000000");
        assert_eq!(keys[1].as_str(), "patch:000000000000000002:c1:00000000000000000001");
        assert_eq!(keys[2].as_str(), "patch:000000000000000003:c1:00000000000000000000");
    }
}
