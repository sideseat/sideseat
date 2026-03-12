use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;

use super::error::CmError;
use super::types::{BranchId, ConversationId, now_micros};

// ---------------------------------------------------------------------------
// CrdtDelta
// ---------------------------------------------------------------------------

/// A single CRDT update encoded as a Yjs delta (v1 binary format).
///
/// `global_seq` is assigned by the backend when the delta is persisted
/// and used as the exclusive lower bound for `crdt_fetch`.
#[derive(Debug, Clone)]
pub struct CrdtDelta {
    /// Monotonically increasing sequence number assigned by the backend.
    pub global_seq: u64,
    /// Opaque identifier of the client that produced this delta.
    pub client_id: String,
    /// Identifies which branch produced this delta (for `crdt_fetch` filtering).
    pub branch_id: BranchId,
    pub conversation_id: ConversationId,
    /// Yjs v1 binary delta.
    pub delta: Vec<u8>,
    /// State vector of this delta applied to an empty document.
    ///
    /// Stored alongside the delta so `push()` can compute the committed baseline SV
    /// via element-wise merge (`O(N × decode_sv)`) instead of replaying the full
    /// snapshot + all pending deltas (`O(snap_size + N × delta_size)`).
    ///
    /// Empty for deltas produced by [`SyncBackend`] implementations (legacy path).
    pub sv: Vec<u8>,
    pub created_at: i64,
}

// ---------------------------------------------------------------------------
// SyncBackend
// ---------------------------------------------------------------------------

/// Transport/storage for CRDT delta exchange across `ContextManager` instances.
///
/// Two implementations:
/// - [`LocalSyncBackend`] — in-process shared memory, zero I/O.
/// - [`StorageSyncBackend`] — delegates to [`ContextBackend`] for distributed deployments.
///
/// NOTE: `SyncBackend` is kept for future real-time transport (e.g., WebSocket peer sync)
/// but is NOT a field in `ContextManager`. CRDT transport uses
/// `ContextBackend.crdt_append/crdt_fetch` directly from `CrdtExtension.push/pull`.
#[async_trait]
pub trait SyncBackend: Send + Sync {
    /// Persist a CRDT delta and return the assigned global sequence number.
    async fn push_delta(
        &self,
        conv_id: &ConversationId,
        branch_id: &BranchId,
        client_id: &str,
        delta: &[u8],
    ) -> Result<u64, CmError>;

    /// Return all deltas for `branch_id` where `global_seq > after_seq`.
    ///
    /// Pass `after_seq = 0` to fetch the complete history.
    async fn fetch_deltas(
        &self,
        conv_id: &ConversationId,
        branch_id: &BranchId,
        after_seq: u64,
    ) -> Result<Vec<CrdtDelta>, CmError>;
}

// ---------------------------------------------------------------------------
// LocalSyncBackend
// ---------------------------------------------------------------------------

struct LocalSyncState {
    next_seq: u64,
    deltas: Vec<CrdtDelta>,
}

/// In-process sync backend for single-instance deployments.
///
/// Share one `Arc<LocalSyncBackend>` across all `ContextManager` instances in the
/// same process.
pub struct LocalSyncBackend {
    state: RwLock<LocalSyncState>,
}

impl LocalSyncBackend {
    pub fn new() -> Self {
        Self {
            state: RwLock::new(LocalSyncState {
                next_seq: 1,
                deltas: Vec::new(),
            }),
        }
    }

    pub fn shared() -> Arc<Self> {
        Arc::new(Self::new())
    }
}

impl Default for LocalSyncBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SyncBackend for LocalSyncBackend {
    async fn push_delta(
        &self,
        conv_id: &ConversationId,
        branch_id: &BranchId,
        client_id: &str,
        delta: &[u8],
    ) -> Result<u64, CmError> {
        let mut state = self.state.write();
        let seq = state.next_seq;
        state.next_seq += 1;
        state.deltas.push(CrdtDelta {
            global_seq: seq,
            client_id: client_id.to_string(),
            branch_id: branch_id.clone(),
            conversation_id: conv_id.clone(),
            delta: delta.to_vec(),
            sv: Vec::new(), // SyncBackend doesn't compute sv
            created_at: now_micros(),
        });
        Ok(seq)
    }

    async fn fetch_deltas(
        &self,
        conv_id: &ConversationId,
        branch_id: &BranchId,
        after_seq: u64,
    ) -> Result<Vec<CrdtDelta>, CmError> {
        let state = self.state.read();
        Ok(state
            .deltas
            .iter()
            .filter(|d| {
                d.conversation_id == *conv_id
                    && d.branch_id == *branch_id
                    && d.global_seq > after_seq
            })
            .cloned()
            .collect())
    }
}

// ---------------------------------------------------------------------------
// StorageSyncBackend
// ---------------------------------------------------------------------------

/// Sync backend that delegates to a [`ContextBackend`] delta log.
///
/// Use this for distributed deployments where multiple process instances share
/// a single persistent backend (e.g. a database-backed `ContextBackend`).
pub struct StorageSyncBackend<B: super::backend::ContextBackend> {
    backend: std::sync::Arc<B>,
}

impl<B: super::backend::ContextBackend> StorageSyncBackend<B> {
    pub fn new(backend: std::sync::Arc<B>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<B: super::backend::ContextBackend> SyncBackend for StorageSyncBackend<B> {
    async fn push_delta(
        &self,
        conv_id: &ConversationId,
        branch_id: &BranchId,
        client_id: &str,
        delta: &[u8],
    ) -> Result<u64, CmError> {
        let d = CrdtDelta {
            global_seq: 0, // assigned by backend
            client_id: client_id.to_string(),
            branch_id: branch_id.clone(),
            conversation_id: conv_id.clone(),
            delta: delta.to_vec(),
            sv: Vec::new(), // SyncBackend doesn't compute sv
            created_at: now_micros(),
        };
        self.backend.crdt_append(&d).await
    }

    async fn fetch_deltas(
        &self,
        conv_id: &ConversationId,
        branch_id: &BranchId,
        after_seq: u64,
    ) -> Result<Vec<CrdtDelta>, CmError> {
        self.backend.crdt_fetch(conv_id, branch_id, after_seq).await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::super::types::{BranchId, ConversationId};
    use super::*;

    #[tokio::test]
    async fn local_push_fetch_round_trip() {
        let backend = LocalSyncBackend::shared();
        let conv = ConversationId::new();
        let branch = BranchId::new();

        let seq1 = backend
            .push_delta(&conv, &branch, "client-a", b"delta1")
            .await
            .unwrap();
        let seq2 = backend
            .push_delta(&conv, &branch, "client-b", b"delta2")
            .await
            .unwrap();

        assert!(seq2 > seq1);

        let all = backend.fetch_deltas(&conv, &branch, 0).await.unwrap();
        assert_eq!(all.len(), 2);

        let incremental = backend.fetch_deltas(&conv, &branch, seq1).await.unwrap();
        assert_eq!(incremental.len(), 1);
        assert_eq!(incremental[0].delta, b"delta2");
    }

    #[tokio::test]
    async fn local_filters_by_branch() {
        let backend = LocalSyncBackend::shared();
        let conv = ConversationId::new();
        let branch_a = BranchId::new();
        let branch_b = BranchId::new();

        backend
            .push_delta(&conv, &branch_a, "c1", b"for-a")
            .await
            .unwrap();
        backend
            .push_delta(&conv, &branch_b, "c1", b"for-b")
            .await
            .unwrap();

        let a_deltas = backend.fetch_deltas(&conv, &branch_a, 0).await.unwrap();
        let b_deltas = backend.fetch_deltas(&conv, &branch_b, 0).await.unwrap();

        assert_eq!(a_deltas.len(), 1);
        assert_eq!(b_deltas.len(), 1);
        assert_eq!(a_deltas[0].delta, b"for-a");
        assert_eq!(b_deltas[0].delta, b"for-b");
    }

    #[tokio::test]
    async fn fetch_returns_own_and_foreign_deltas() {
        // fetch_deltas returns ALL deltas — CRDT merge is idempotent, so callers
        // merge unconditionally rather than filtering by client_id.
        let backend = LocalSyncBackend::shared();
        let conv = ConversationId::new();
        let branch = BranchId::new();

        backend
            .push_delta(&conv, &branch, "self", b"mine")
            .await
            .unwrap();
        backend
            .push_delta(&conv, &branch, "other", b"theirs")
            .await
            .unwrap();

        let deltas = backend.fetch_deltas(&conv, &branch, 0).await.unwrap();
        assert_eq!(
            deltas.len(),
            2,
            "fetch_deltas must return own and foreign deltas"
        );
        assert!(
            deltas.iter().any(|d| d.delta == b"mine"),
            "own delta must be included"
        );
        assert!(
            deltas.iter().any(|d| d.delta == b"theirs"),
            "foreign delta must be included"
        );
    }

    #[tokio::test]
    async fn shared_arc_visible_across_clones() {
        let sync = LocalSyncBackend::shared();
        let sync2 = sync.clone();
        let conv = ConversationId::new();
        let branch = BranchId::new();

        sync.push_delta(&conv, &branch, "writer", b"hello")
            .await
            .unwrap();

        let deltas = sync2.fetch_deltas(&conv, &branch, 0).await.unwrap();
        assert_eq!(deltas.len(), 1);
    }

    // -----------------------------------------------------------------------
    // StorageSyncBackend tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn storage_push_fetch_round_trip() {
        use super::super::backend::InMemoryContextBackend;
        use super::StorageSyncBackend;
        use std::sync::Arc;

        let backend = Arc::new(InMemoryContextBackend::new());
        let sync = StorageSyncBackend::new(backend);
        let conv = ConversationId::new();
        let branch = BranchId::new();

        let seq1 = sync.push_delta(&conv, &branch, "c1", b"d1").await.unwrap();
        let seq2 = sync.push_delta(&conv, &branch, "c2", b"d2").await.unwrap();

        assert!(seq2 > seq1);

        let all = sync.fetch_deltas(&conv, &branch, 0).await.unwrap();
        assert_eq!(all.len(), 2);

        let incremental = sync.fetch_deltas(&conv, &branch, seq1).await.unwrap();
        assert_eq!(incremental.len(), 1);
        assert_eq!(incremental[0].delta, b"d2");
    }

    #[tokio::test]
    async fn storage_filters_by_branch() {
        use super::super::backend::InMemoryContextBackend;
        use super::StorageSyncBackend;
        use std::sync::Arc;

        let backend = Arc::new(InMemoryContextBackend::new());
        let sync = StorageSyncBackend::new(backend);
        let conv = ConversationId::new();
        let branch_a = BranchId::new();
        let branch_b = BranchId::new();

        sync.push_delta(&conv, &branch_a, "c", b"for-a")
            .await
            .unwrap();
        sync.push_delta(&conv, &branch_b, "c", b"for-b")
            .await
            .unwrap();

        let a = sync.fetch_deltas(&conv, &branch_a, 0).await.unwrap();
        let b = sync.fetch_deltas(&conv, &branch_b, 0).await.unwrap();

        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        assert_eq!(a[0].delta, b"for-a");
        assert_eq!(b[0].delta, b"for-b");
    }

    #[tokio::test]
    async fn storage_delta_metadata_round_trip() {
        use super::super::backend::InMemoryContextBackend;
        use super::StorageSyncBackend;
        use std::sync::Arc;

        let backend = Arc::new(InMemoryContextBackend::new());
        let sync = StorageSyncBackend::new(backend);
        let conv = ConversationId::new();
        let branch = BranchId::new();

        sync.push_delta(&conv, &branch, "writer", b"payload")
            .await
            .unwrap();

        let deltas = sync.fetch_deltas(&conv, &branch, 0).await.unwrap();
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].client_id, "writer");
        assert_eq!(deltas[0].branch_id, branch);
        assert_eq!(deltas[0].conversation_id, conv);
        assert_eq!(deltas[0].delta, b"payload");
        assert!(deltas[0].created_at > 0);
    }
}
