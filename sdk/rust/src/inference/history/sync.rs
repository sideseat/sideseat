use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use async_trait::async_trait;
use parking_lot::RwLock;

use super::error::HistoryError;
use super::storage::HistoryStorage;
use super::types::{ConversationId, now_micros};

// ---------------------------------------------------------------------------
// CrdtDelta
// ---------------------------------------------------------------------------

/// A single CRDT update encoded as a Yjs delta (v1 binary format).
///
/// `global_seq` is assigned by the sync backend when the delta is persisted
/// and used as the exclusive lower bound for `fetch_deltas`.
#[derive(Debug, Clone)]
pub struct CrdtDelta {
    /// Monotonically increasing sequence number assigned by the backend.
    pub global_seq: u64,
    /// Opaque identifier of the client that produced this delta.
    pub client_id: String,
    pub conversation_id: ConversationId,
    /// Yjs v1 binary delta.
    pub delta: Vec<u8>,
    pub created_at: i64,
}

// ---------------------------------------------------------------------------
// HistorySyncBackend
// ---------------------------------------------------------------------------

/// Transport/storage for CRDT delta exchange across `History` instances.
///
/// Two implementations:
/// - [`LocalSyncBackend`] — in-process shared memory, zero I/O, for
///   single-process multi-client deployments.
/// - [`StorageSyncBackend`] — delegates to [`HistoryStorage`] for
///   distributed multi-instance deployments.
///
/// Register with [`History::with_sync`].
#[async_trait]
pub trait HistorySyncBackend: Send + Sync {
    /// Persist a CRDT delta and return the assigned global sequence number.
    async fn push_delta(
        &self,
        conv_id: &ConversationId,
        client_id: &str,
        delta: &[u8],
    ) -> Result<u64, HistoryError>;

    /// Return all deltas where `global_seq > after_seq`.
    ///
    /// Pass `after_seq = 0` to fetch the complete history.
    async fn fetch_deltas(
        &self,
        conv_id: &ConversationId,
        after_seq: u64,
    ) -> Result<Vec<CrdtDelta>, HistoryError>;
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
/// Share one `Arc<LocalSyncBackend>` across all `History` instances in the
/// same process. Changes written by any instance are immediately visible to
/// all others via `pull_crdt`.
///
/// ```rust,ignore
/// let sync = LocalSyncBackend::shared();
/// let h1 = History::new(storage1, conv).with_sync(sync.clone());
/// let h2 = History::new(storage2, conv).with_sync(sync.clone());
/// ```
pub struct LocalSyncBackend {
    state: RwLock<LocalSyncState>,
}

impl LocalSyncBackend {
    pub fn new() -> Self {
        Self {
            state: RwLock::new(LocalSyncState { next_seq: 1, deltas: Vec::new() }),
        }
    }

    /// Convenience constructor that returns an `Arc` ready to be cloned and
    /// shared across `History` instances.
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
impl HistorySyncBackend for LocalSyncBackend {
    async fn push_delta(
        &self,
        conv_id: &ConversationId,
        client_id: &str,
        delta: &[u8],
    ) -> Result<u64, HistoryError> {
        let mut state = self.state.write();
        let seq = state.next_seq;
        state.next_seq += 1;
        state.deltas.push(CrdtDelta {
            global_seq: seq,
            client_id: client_id.to_string(),
            conversation_id: conv_id.clone(),
            delta: delta.to_vec(),
            created_at: now_micros(),
        });
        Ok(seq)
    }

    async fn fetch_deltas(
        &self,
        conv_id: &ConversationId,
        after_seq: u64,
    ) -> Result<Vec<CrdtDelta>, HistoryError> {
        let state = self.state.read();
        Ok(state
            .deltas
            .iter()
            .filter(|d| d.conversation_id == *conv_id && d.global_seq > after_seq)
            .cloned()
            .collect())
    }
}

// ---------------------------------------------------------------------------
// StorageSyncBackend
// ---------------------------------------------------------------------------

/// Distributed sync backend for multi-instance deployments.
///
/// Stores CRDT deltas via [`HistoryStorage`]. All instances pointing at the
/// same database share the global sequence automatically.
///
/// ```rust,ignore
/// let storage = Arc::new(MyDatabaseStorage::new(...));
/// let sync = Arc::new(StorageSyncBackend::new(storage.clone()));
/// let history = History::new((*storage).clone(), conv).with_sync(sync);
/// ```
pub struct StorageSyncBackend {
    storage: Arc<dyn HistoryStorage>,
    /// Local sequence counter used only when the storage cannot return one.
    /// For production storage backends the seq is storage-assigned.
    _local_seq: AtomicU64,
}

impl StorageSyncBackend {
    pub fn new(storage: Arc<dyn HistoryStorage>) -> Self {
        Self { storage, _local_seq: AtomicU64::new(1) }
    }
}

#[async_trait]
impl HistorySyncBackend for StorageSyncBackend {
    async fn push_delta(
        &self,
        conv_id: &ConversationId,
        client_id: &str,
        delta: &[u8],
    ) -> Result<u64, HistoryError> {
        let d = CrdtDelta {
            global_seq: 0, // assigned by storage
            client_id: client_id.to_string(),
            conversation_id: conv_id.clone(),
            delta: delta.to_vec(),
            created_at: now_micros(),
        };
        self.storage.append_crdt_delta(&d).await
    }

    async fn fetch_deltas(
        &self,
        conv_id: &ConversationId,
        after_seq: u64,
    ) -> Result<Vec<CrdtDelta>, HistoryError> {
        self.storage.fetch_crdt_deltas(conv_id, after_seq).await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::types::ConversationId;

    #[tokio::test]
    async fn local_push_fetch_round_trip() {
        let backend = LocalSyncBackend::shared();
        let conv = ConversationId::new();

        let seq1 = backend.push_delta(&conv, "client-a", b"delta1").await.unwrap();
        let seq2 = backend.push_delta(&conv, "client-b", b"delta2").await.unwrap();

        assert!(seq2 > seq1);

        let all = backend.fetch_deltas(&conv, 0).await.unwrap();
        assert_eq!(all.len(), 2);

        let incremental = backend.fetch_deltas(&conv, seq1).await.unwrap();
        assert_eq!(incremental.len(), 1);
        assert_eq!(incremental[0].delta, b"delta2");
    }

    #[tokio::test]
    async fn local_filters_by_conversation() {
        let backend = LocalSyncBackend::shared();
        let conv_a = ConversationId::new();
        let conv_b = ConversationId::new();

        backend.push_delta(&conv_a, "c1", b"for-a").await.unwrap();
        backend.push_delta(&conv_b, "c1", b"for-b").await.unwrap();

        let a_deltas = backend.fetch_deltas(&conv_a, 0).await.unwrap();
        let b_deltas = backend.fetch_deltas(&conv_b, 0).await.unwrap();

        assert_eq!(a_deltas.len(), 1);
        assert_eq!(b_deltas.len(), 1);
        assert_eq!(a_deltas[0].delta, b"for-a");
        assert_eq!(b_deltas[0].delta, b"for-b");
    }

    #[tokio::test]
    async fn shared_arc_visible_across_clones() {
        let sync = LocalSyncBackend::shared();
        let sync2 = sync.clone();
        let conv = ConversationId::new();

        sync.push_delta(&conv, "writer", b"hello").await.unwrap();

        let deltas = sync2.fetch_deltas(&conv, 0).await.unwrap();
        assert_eq!(deltas.len(), 1);
    }
}
