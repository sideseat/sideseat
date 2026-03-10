pub mod artifact;
pub mod backend;
pub mod canvas;
pub mod crdt;
pub mod error;
pub mod kanban;
pub mod migrate;
pub mod source;
pub mod sync;
pub mod tree;
pub mod types;
pub mod vfs;

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use self::backend::{ContextBackend, NodePatch};
use self::crdt::CrdtExtension;
use self::error::CmError;
use self::tree::ConversationTree;
use self::types::{
    AgentId, BranchId, BranchMeta, Conversation, ConversationId, ConversationPatch,
    DatasetEntry, DatasetSplit, MemoryEntry, MemoryEntryId, MemoryEntryType,
    Node, NodeContent, NodeId, NodeParams, StreamStatus, StreamingState, now_micros,
};
use self::vfs::VfsExtension;
use crate::types::{ContentBlock, Message, Response, Role, Usage, estimate_tokens};

// ---------------------------------------------------------------------------
// ContextExtension trait
// ---------------------------------------------------------------------------

/// Lifecycle hook interface for pluggable context extensions.
///
/// Stateless extensions (Canvas, Kanban, Artifact) hold no internal state and
/// receive storage via method parameters. Stateful extensions (VFS, CRDT) hold
/// state internally behind interior mutability and use lifecycle hooks to stay
/// in sync with the active branch.
pub trait ContextExtension: Send + Sync + 'static {
    fn id(&self) -> &str;
    fn on_branch_forked(&self, _parent: &BranchId, _child: &BranchId) {}
    fn on_branch_checked_out(&self, _branch: &BranchId) {}
}

// ---------------------------------------------------------------------------
// ExtensionRegistry — type-erased extension container
// ---------------------------------------------------------------------------

struct ExtensionRegistryInner {
    /// Hook list for lifecycle callbacks — same order as insertion.
    hooks: Vec<Arc<dyn ContextExtension>>,
    /// Type-erased store for typed lookups by concrete type or string ID.
    store: HashMap<String, Arc<dyn Any + Send + Sync>>,
}

pub struct ExtensionRegistry {
    inner: RwLock<ExtensionRegistryInner>,
}

impl ExtensionRegistry {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(ExtensionRegistryInner {
                hooks: Vec::new(),
                store: HashMap::new(),
            }),
        }
    }

    /// Register an extension. Both the type-erased store entry and the hook
    /// list entry are inserted under the same write lock, eliminating any
    /// window where `extension<T>()` and `fire_branch_forked()` could observe
    /// an inconsistent view of the registry.
    pub(crate) fn register<T: ContextExtension>(&self, ext: Arc<T>) {
        let id = ext.id().to_string();
        let mut guard = self.inner.write();
        guard.store.insert(id, ext.clone() as Arc<dyn Any + Send + Sync>);
        guard.hooks.push(ext as Arc<dyn ContextExtension>);
    }

    /// Return the registered extension whose concrete type is `T`, if any.
    pub fn extension<T: ContextExtension>(&self) -> Option<Arc<T>> {
        self.inner
            .read()
            .store
            .values()
            .find_map(|arc| arc.clone().downcast::<T>().ok())
    }

    /// Return a registered extension by its string ID and cast to `T`.
    pub fn extension_by_id<T: ContextExtension>(&self, id: &str) -> Option<Arc<T>> {
        self.inner
            .read()
            .store
            .get(id)
            .and_then(|arc| arc.clone().downcast::<T>().ok())
    }

    pub(crate) fn fire_branch_forked(&self, parent: &BranchId, child: &BranchId) {
        for ext in self.inner.read().hooks.iter() {
            ext.on_branch_forked(parent, child);
        }
    }

    pub(crate) fn fire_branch_checked_out(&self, branch: &BranchId) {
        for ext in self.inner.read().hooks.iter() {
            ext.on_branch_checked_out(branch);
        }
    }
}

impl Default for ExtensionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Summarizer
// ---------------------------------------------------------------------------

#[async_trait]
pub trait Summarizer: Send + Sync {
    async fn summarize(&self, messages: &[Message]) -> Result<String, CmError>;
}

// ---------------------------------------------------------------------------
// MemorySource + MemoryItem
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryItem {
    pub content: String,
    pub source: String,
    pub relevance_score: Option<f64>,
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

#[async_trait]
pub trait MemorySource: Send + Sync {
    async fn retrieve(
        &self,
        query: &str,
        conv_id: &ConversationId,
        limit: u32,
    ) -> Result<Vec<MemoryItem>, CmError>;

    async fn store(
        &self,
        conv_id: &ConversationId,
        item: &MemoryItem,
    ) -> Result<(), CmError>;

    fn name(&self) -> &str;
}

// ---------------------------------------------------------------------------
// CompressionConfig + strategies
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub enum SystemMode {
    #[default]
    AlwaysFirst,
    FromConversation,
    None,
}

#[derive(Debug, Clone, Default)]
pub enum CompressionStrategy {
    None,
    #[default]
    Truncate,
    Summarize,
    SlidingWindow { keep_last: usize },
    ServerCompaction { compact_threshold: u32 },
    Fail,
}

#[derive(Clone)]
pub struct CompressionConfig {
    pub max_tokens: u64,
    pub strategy: CompressionStrategy,
    pub system_mode: SystemMode,
    pub pinned_node_ids: Vec<NodeId>,
    pub summarizer: Option<Arc<dyn Summarizer>>,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            max_tokens: 100_000,
            strategy: CompressionStrategy::Truncate,
            system_mode: SystemMode::AlwaysFirst,
            pinned_node_ids: Vec::new(),
            summarizer: None,
        }
    }
}

// ---------------------------------------------------------------------------
// ContextResult
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ContextResult {
    pub messages: Vec<Message>,
    pub system: Option<String>,
    pub estimated_tokens: u64,
    pub server_compaction_needed: bool,
    pub summary: Option<String>,
}

// ---------------------------------------------------------------------------
// ContextManager<B>
// ---------------------------------------------------------------------------

/// Auto-compact CRDT delta log when delta count since last compaction exceeds this threshold.
const CRDT_COMPACT_THRESHOLD: u64 = 500;

pub struct ContextManager<B: ContextBackend> {
    backend: Arc<B>,
    tree: RwLock<ConversationTree>,
    conversation: Mutex<Conversation>,
    extensions: ExtensionRegistry,
    compression: CompressionConfig,
    memory_sources: Vec<Arc<dyn MemorySource>>,
    client_id: String,
    /// Monotonic counter for patch_conversation key uniqueness within a microsecond burst.
    patch_seq: AtomicU64,
    /// Global seq at which we last compacted the CRDT delta log.
    last_compact_seq: AtomicU64,
}

impl<B: ContextBackend> ContextManager<B> {
    // -----------------------------------------------------------------------
    // Constructors
    // -----------------------------------------------------------------------

    /// Create a new conversation, persisting it and a default branch to the backend.
    ///
    /// **Production note**: pass a stable identity to `CrdtExtension::new(stable_id)` when
    /// registering the CRDT extension. The default is a random UUID generated at construction
    /// time; each restart creates a new entry in the Yjs state vector, causing unbounded SV
    /// growth across worker restarts. Use a stable identity (e.g. worker name, pod ID, or a
    /// UUID persisted alongside the conversation). `ContextManager::with_client_id` does NOT
    /// affect CRDT — it only scopes `patch_conversation` keys.
    pub async fn new(backend: Arc<B>, conversation: Conversation) -> Result<Self, CmError> {
        let conv_id = conversation.id.clone();

        // Register in global conversations index.
        backend
            .kv_put("conversations", conv_id.as_str(), conv_id.as_str().as_bytes())
            .await?;

        // ConversationTree::new() auto-creates a placeholder branch. We reuse
        // its ID as the default branch so there is exactly one branch in the
        // tree (no phantom alongside the real one).
        let mut tree = ConversationTree::new(conv_id.clone());
        let default_branch = BranchMeta {
            id: tree.active_branch().clone(),
            conversation_id: conv_id.clone(),
            parent_id: None,
            fork_node_id: None,
            crdt_seq_watermark: 0,
            name: "main".into(),
            created_at: now_micros(),
        };
        // Replace the placeholder with a fully-populated BranchMeta.
        tree.add_branch(default_branch.clone());
        backend.save_branch(&default_branch).await?;
        // active_branch is already set to default_branch.id by ConversationTree::new().

        let mut conv = conversation;
        conv.default_branch_id = Some(default_branch.id.clone());

        // Persist conversation base with default_branch_id populated.
        let bytes = serde_json::to_vec(&conv)?;
        backend
            .kv_put(&format!("conv:{}", conv_id.as_str()), "meta", &bytes)
            .await?;

        Ok(Self {
            backend,
            tree: RwLock::new(tree),
            conversation: Mutex::new(conv),
            extensions: ExtensionRegistry::new(),
            compression: CompressionConfig::default(),
            memory_sources: Vec::new(),
            client_id: uuid::Uuid::now_v7().to_string(),
            patch_seq: AtomicU64::new(0),
            last_compact_seq: AtomicU64::new(0),
        })
    }

    /// Load an existing conversation from the backend.
    ///
    /// After loading, register stateful extensions with [`with_extension`] and then call
    /// [`checkout`] on the active branch to restore CRDT and VFS state from KV storage.
    /// Skipping `checkout` leaves those subsystems in their default (empty) state.
    ///
    /// **Production note**: pass a stable identity to `CrdtExtension::new(stable_id)` to
    /// prevent unbounded Yjs state-vector growth across worker restarts (see `new()`).
    pub async fn load(backend: Arc<B>, id: &ConversationId) -> Result<Self, CmError> {
        // 1. Load conversation base + apply patches in lex order.
        let conv_ns = format!("conv:{}", id.as_str());
        let base_bytes = backend
            .kv_get(&conv_ns, "meta")
            .await?
            .ok_or_else(|| CmError::ConversationNotFound(id.clone()))?;
        let mut conversation: Conversation = serde_json::from_slice(&base_bytes)?;

        let patch_keys = backend.kv_list(&conv_ns, "patch:").await?;
        for key in &patch_keys {
            if let Some(bytes) = backend.kv_get(&conv_ns, key).await?
                && let Ok(patch) = serde_json::from_slice::<ConversationPatch>(&bytes)
            {
                conversation.apply_patch(&patch);
            }
        }

        // 2. Reconstruct tree.
        let mut tree = ConversationTree::new(id.clone());
        let branches = backend.list_branches(id).await?;
        for branch in branches {
            tree.add_branch(branch);
        }

        let headers = backend.list_node_headers(id).await?;
        for header in headers {
            tree.register_header(header)?;
        }
        // Branches loaded before their fork-node headers (the common ordering
        // above) have no tip yet. Resolve them now that all headers are known.
        tree.initialize_fork_branch_tips();

        // 3. Checkout active branch.
        let active_branch = conversation
            .default_branch_id
            .clone()
            .ok_or_else(|| CmError::BackendError("No default_branch_id on conversation".into()))?;
        tree.checkout(&active_branch)?;

        Ok(Self {
            backend,
            tree: RwLock::new(tree),
            conversation: Mutex::new(conversation),
            extensions: ExtensionRegistry::new(),
            compression: CompressionConfig::default(),
            memory_sources: Vec::new(),
            client_id: uuid::Uuid::now_v7().to_string(),
            patch_seq: AtomicU64::new(0),
            last_compact_seq: AtomicU64::new(0),
        })
    }

    // -----------------------------------------------------------------------
    // Builder methods
    // -----------------------------------------------------------------------

    pub fn with_extension<T: ContextExtension>(self, ext: Arc<T>) -> Self {
        let current_branch = self.tree.read().active_branch().clone();
        ext.on_branch_checked_out(&current_branch);
        self.extensions.register(ext);
        self
    }

    pub fn with_compression(mut self, cfg: CompressionConfig) -> Self {
        self.compression = cfg;
        self
    }

    pub fn with_memory_source(mut self, src: Arc<dyn MemorySource>) -> Self {
        self.memory_sources.push(src);
        self
    }

    pub fn with_client_id(mut self, id: impl Into<String>) -> Self {
        self.client_id = id.into();
        self
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    /// Per-instance UUID assigned at construction time.
    ///
    /// Stable for the lifetime of this `ContextManager`. Useful for correlating
    /// log lines, backend-side temp files, and `patch_conversation` key scoping.
    /// Override with [`with_client_id`] when a deterministic identity is preferred
    /// (e.g. a worker pod name persisted alongside the conversation).
    pub fn instance_id(&self) -> &str {
        &self.client_id
    }

    pub fn backend(&self) -> &Arc<B> {
        &self.backend
    }

    pub fn extension<T: ContextExtension>(&self) -> Option<Arc<T>> {
        self.extensions.extension::<T>()
    }

    pub fn extension_by_id<T: ContextExtension>(&self, id: &str) -> Option<Arc<T>> {
        self.extensions.extension_by_id::<T>(id)
    }

    pub fn extensions(&self) -> &ExtensionRegistry {
        &self.extensions
    }

    pub fn conversation(&self) -> Conversation {
        self.conversation.lock().clone()
    }

    pub fn active_branch(&self) -> BranchId {
        self.tree.read().active_branch().clone()
    }

    pub fn active_branch_tip(&self) -> Option<NodeId> {
        let tree = self.tree.read();
        let branch = tree.active_branch().clone();
        tree.branch_tip(&branch).cloned()
    }

    // -----------------------------------------------------------------------
    // VFS preload
    // -----------------------------------------------------------------------

    /// Preload VFS branch indexes from KV into a registered VfsExtension.
    /// Call after `load()` + `with_extension(vfs)`.
    pub async fn preload_vfs_indexes(&self) -> Result<(), CmError> {
        let Some(vfs) = self.extensions.extension::<VfsExtension>() else {
            return Ok(());
        };
        let branch_ids: Vec<BranchId> = self
            .tree
            .read()
            .branches()
            .keys()
            .cloned()
            .collect();
        for branch_id in &branch_ids {
            if let Some(data) = self
                .backend
                .kv_get("vfs_index", branch_id.as_str())
                .await?
            {
                vfs.load_branch_index(branch_id.as_str(), &data)?;
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Conversation patching
    // -----------------------------------------------------------------------

    pub async fn patch_conversation(&self, patch: ConversationPatch) -> Result<(), CmError> {
        let ts = now_micros();
        let seq = self.patch_seq.fetch_add(1, Ordering::Relaxed);
        let key = format!("patch:{ts:020}:{client}:{seq:020}", client = self.client_id);
        let bytes = serde_json::to_vec(&patch)?;
        let conv_id = self.conversation.lock().id.clone();
        self.backend
            .kv_put(&format!("conv:{}", conv_id.as_str()), &key, &bytes)
            .await?;
        self.conversation.lock().apply_patch(&patch);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Branching
    // -----------------------------------------------------------------------

    pub async fn fork(
        &self,
        from_node_id: &NodeId,
        name: impl Into<String>,
    ) -> Result<BranchId, CmError> {
        let name = name.into();

        // Acquire conversation ID before the tree read lock to avoid holding
        // two locks simultaneously (conversation Mutex + tree RwLock).
        let fork_conv_id = self.conversation.lock().id.clone();

        // Gather all needed info under a READ lock — no tree mutation yet.
        // The tree is only mutated after all I/O succeeds so a backend failure
        // cannot leave a branch registered in memory but absent from storage.
        let (parent_branch_id, crdt_watermark, branch_meta) = {
            let tree = self.tree.read();
            // get_header returns CmError::NodeNotFound on missing node.
            let header = tree.get_header(from_node_id)?;
            let parent_branch_id = header.branch_id.clone();
            let watermark = header.crdt_seq_watermark.unwrap_or(0);
            let new_branch_id = BranchId::new();

            let meta = BranchMeta {
                id: new_branch_id,
                conversation_id: fork_conv_id.clone(),
                parent_id: Some(parent_branch_id.clone()),
                fork_node_id: Some(from_node_id.clone()),
                crdt_seq_watermark: watermark,
                name,
                created_at: now_micros(),
            };
            (parent_branch_id, watermark, meta)
        }; // Read lock dropped — tree not yet mutated.

        // Persist first: all I/O must succeed before any in-memory mutations.
        self.backend.save_branch(&branch_meta).await?;

        // Build CRDT snapshot for the new branch from the parent snapshot + incremental deltas.
        // If no snapshot exists yet (push-only branch), parent_seq=0 and we fetch from the start.
        let (parent_seq, parent_bytes, _) =
            self.backend.crdt_load_snapshot(&parent_branch_id).await?;

        // Fetch incremental deltas from parent since its snapshot, up to the fork watermark.
        let deltas = self
            .backend
            .crdt_fetch(&fork_conv_id, &parent_branch_id, parent_seq)
            .await?;
        let mut tmp_doc = self::crdt::CrdtDoc::from_state(&parent_bytes)?;
        for delta in &deltas {
            if delta.global_seq <= crdt_watermark {
                tmp_doc.merge_delta(&delta.delta)?;
            }
        }
        let full_state = tmp_doc.full_state();
        let full_sv = tmp_doc.state_vector();
        self.backend
            .crdt_save_snapshot(&branch_meta.id, crdt_watermark, &full_state, &full_sv)
            .await?;

        // Persist VFS index for the new branch BEFORE in-memory mutations.
        // At fork time the child index is an exact copy of the parent's, so
        // serialise the parent's current index and store it under the child's
        // branch ID. This keeps the "all I/O before mutations" invariant: if
        // kv_put fails here, no in-memory state has changed yet.
        if let Some(vfs) = self.extensions.extension::<VfsExtension>()
            && let Some(data) = vfs.serialize_branch_index(parent_branch_id.as_str())
        {
            self.backend
                .kv_put("vfs_index", branch_meta.id.as_str(), &data)
                .await?;
        }

        // All I/O succeeded — now register in the in-memory tree.
        self.tree.write().add_branch(branch_meta.clone());

        // Notify extensions: VFS forks its COW index in-memory.
        self.extensions
            .fire_branch_forked(&parent_branch_id, &branch_meta.id);

        Ok(branch_meta.id)
    }

    /// Checkout a branch: atomically restore CRDT + VFS + tree to branch state.
    /// Fail-safe: VFS state is pre-loaded before any mutations; CRDT is reset then
    /// synced via pull() which loads snapshot + deltas in a single pass.
    pub async fn checkout(&self, branch_id: &BranchId) -> Result<(), CmError> {
        let conv_id = self.conversation.lock().id.clone();

        // 1. Pre-load VFS state (I/O before mutation).
        let vfs_data = self
            .backend
            .kv_get("vfs_index", branch_id.as_str())
            .await?;

        // 2. Apply subsystems — VFS first, CRDT second, tree last (commit signal).
        if let Some(vfs) = self.extensions.extension::<VfsExtension>() {
            let bytes = vfs_data.as_deref().unwrap_or(&[]);
            vfs.load_branch_index(branch_id.as_str(), bytes)?;
        }

        if let Some(crdt_ext) = self.extensions.extension::<CrdtExtension>() {
            // Reset to empty so pull() builds the doc from scratch (snapshot + deltas)
            // in a single backend pass. Without the reset, stale ops from the previous
            // branch would remain in the doc and pull()'s merge would be additive only.
            crdt_ext.load_snapshot(&[])?;
            crdt_ext
                .pull(&conv_id, branch_id, self.backend.as_ref())
                .await?;
        }

        // Tree checkout is last (commit signal).
        self.tree.write().checkout(branch_id)?;
        self.extensions.fire_branch_checked_out(branch_id);

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Node writes
    // -----------------------------------------------------------------------

    /// Append a final node to the active branch.
    pub async fn add_node(
        &self,
        content: NodeContent,
        params: NodeParams,
    ) -> Result<NodeId, CmError> {
        self.add_node_internal(content, params, true).await
    }

    /// Append a non-final streaming node to the active branch.
    /// Call [`finalize_streaming_node`] once the full content is received.
    pub async fn start_streaming(
        &self,
        content: NodeContent,
        params: NodeParams,
    ) -> Result<NodeId, CmError> {
        self.add_node_internal(content, params, false).await
    }

    async fn add_node_internal(
        &self,
        content: NodeContent,
        params: NodeParams,
        is_final: bool,
    ) -> Result<NodeId, CmError> {
        // Auto-push CRDT before recording watermark (final nodes only).
        let crdt_watermark = if is_final {
            if let Some(crdt) = self.extensions.extension::<CrdtExtension>() {
                let conv_id = self.conversation.lock().id.clone();
                let branch_id = self.tree.read().active_branch().clone();
                let seq = crdt
                    .push(&conv_id, &branch_id, self.backend.as_ref())
                    .await?;

                // Auto-compact delta log when enough new deltas have accumulated.
                // pull() must run first to advance the snapshot (push does not save one),
                // so that compact() has a complete snapshot baseline to prune against.
                let last = self.last_compact_seq.load(Ordering::Acquire);
                if seq.saturating_sub(last) > CRDT_COMPACT_THRESHOLD {
                    // Advance last_compact_seq first so a transient backend error doesn't
                    // retry compact on every subsequent push (retries resume after another
                    // CRDT_COMPACT_THRESHOLD gap).
                    self.last_compact_seq.store(seq, Ordering::Release);
                    if let Err(e) = crdt.pull(&conv_id, &branch_id, self.backend.as_ref()).await {
                        tracing::warn!(error = %e, "CRDT auto-compact: pull failed, skipping compact");
                    } else if let Err(e) =
                        crdt.compact(&conv_id, &branch_id, self.backend.as_ref()).await
                    {
                        tracing::warn!(error = %e, "CRDT auto-compact: compact failed");
                    }
                }

                Some(seq)
            } else {
                None
            }
        } else {
            None
        };

        let streaming = if is_final {
            None
        } else {
            let now = now_micros();
            Some(StreamingState {
                started_at: now,
                tokens_so_far: 0,
                last_chunk_at: now,
                status: StreamStatus::Active,
            })
        };

        // Acquire conversation ID and timestamp before the tree write lock to
        // avoid holding it during a syscall.
        let conv_id_for_node = self.conversation.lock().id.clone();
        let created_at = now_micros();

        // Allocate sequence under write lock but do NOT register yet.
        // Registering after backend.append_nodes() ensures the tree never
        // contains a node absent from persistent storage (ghost entry).
        let node = {
            let mut tree = self.tree.write();
            let branch_id = tree.active_branch().clone();
            let parent_id = tree.branch_tip(&branch_id).cloned();
            let seq = tree.next_seq(&branch_id);

            Node {
                id: NodeId::new(),
                conversation_id: conv_id_for_node,
                branch_id,
                parent_id,
                sequence: seq,
                created_at,
                created_by: params.created_by,
                model: params.model,
                provider: params.provider,
                content,
                usage: params.usage,
                version: 0,
                is_final,
                streaming,
                deleted: false,
                agent_id: params.agent_id,
                correlation_id: params.correlation_id,
                reply_to: params.reply_to,
                eval_scores: Vec::new(),
                metadata: params.metadata,
                crdt_seq_watermark: crdt_watermark,
            }
        }; // Write lock released — seq is allocated but node not yet in tree.

        // Persist first. If this fails, the sequence number has a gap but the
        // tree never sees the node, keeping tree ⊆ backend at all times.
        self.backend.append_nodes(std::slice::from_ref(&node)).await?;

        // Backend write succeeded — safe to register in the in-memory tree.
        self.tree.write().register(&node)?;
        Ok(node.id)
    }

    /// Finalize a streaming node. Last-write-wins — no OCC failure possible.
    ///
    /// Pushes any pending CRDT changes to the backend and records the resulting
    /// `crdt_seq_watermark` on the node so that later forks from this node
    /// reconstruct the correct CRDT state at finalization time.
    pub async fn finalize_streaming_node(
        &self,
        id: &NodeId,
        content: NodeContent,
        usage: Option<Usage>,
    ) -> Result<(), CmError> {
        // Push CRDT changes made during the streaming window.
        let crdt_seq_watermark = if let Some(crdt) = self.extensions.extension::<CrdtExtension>() {
            let conv_id = self.conversation.lock().id.clone();
            let branch_id = self.tree.read().active_branch().clone();
            Some(crdt.push(&conv_id, &branch_id, self.backend.as_ref()).await?)
        } else {
            None
        };

        self.backend
            .update_node(
                id,
                &NodePatch {
                    content: Some(content),
                    is_final: Some(true),
                    streaming: Some(None),
                    usage,
                    crdt_seq_watermark: crdt_seq_watermark.map(Some),
                    ..Default::default()
                },
                u64::MAX, // skip version check
            )
            .await
    }

    /// Merge CRDT state (canvas, kanban, VFS text) from another branch into the
    /// current context's in-memory doc.
    ///
    /// The canonical use case: a sub-agent completes work on its own branch and
    /// the parent calls `merge_from_branch(&child_branch_id)` to incorporate the
    /// agent's canvas/kanban updates.  The merge is purely in-memory; the next
    /// `add_node` (which auto-pushes) or an explicit `crdt.push()` persists the
    /// merged state to the parent's branch.
    ///
    /// Does nothing if no `CrdtExtension` is registered on this manager.
    pub async fn merge_from_branch(&self, from_branch: &BranchId) -> Result<(), CmError> {
        let Some(crdt) = self.extensions.extension::<CrdtExtension>() else {
            return Ok(());
        };
        let conv_id = self.conversation.lock().id.clone();

        // Load the child's committed snapshot and any incremental deltas after it.
        let (snap_seq, snap_bytes, _) = self.backend.crdt_load_snapshot(from_branch).await?;
        let deltas = self.backend.crdt_fetch(&conv_id, from_branch, snap_seq).await?;

        // Nothing from that branch yet.
        if snap_bytes.is_empty() && deltas.is_empty() {
            return Ok(());
        }

        if !snap_bytes.is_empty() {
            crdt.merge_raw(&snap_bytes)?;
        }
        for delta in &deltas {
            crdt.merge_raw(&delta.delta)?;
        }
        Ok(())
    }

    /// Convenience: record a complete provider response as an AssistantMessage node.
    pub async fn add_response(
        &self,
        response: &Response,
        mut params: NodeParams,
    ) -> Result<NodeId, CmError> {
        if params.usage.is_none() {
            params.usage = Some(response.usage.clone());
        }
        if params.model.is_none() {
            params.model = response.model.clone();
        }
        self.add_node(
            NodeContent::AssistantMessage {
                content: response.content.clone(),
                stop_reason: Some(response.stop_reason.clone()),
                variant_index: None,
            },
            params,
        )
        .await
    }

    // -----------------------------------------------------------------------
    // build_context
    // -----------------------------------------------------------------------

    pub async fn build_context(&self) -> Result<ContextResult, CmError> {
        let conv_id = self.conversation.lock().id.clone();

        // 1. Linearize active branch → node IDs. Single lock acquisition: avoids
        //    a window where checkout() could change the branch between two reads.
        let ids = {
            let tree = self.tree.read();
            let branch = tree.active_branch().clone();
            tree.linearize_ids(&branch)?
        };

        // 2. Fetch nodes, filter deleted.
        let mut nodes = self.backend.get_nodes(&ids).await?;
        nodes.retain(|n| !n.deleted);

        // 3. Project nodes → messages. Build a parallel `pinned` bool vec so
        //    truncation can track pinned status even as messages are removed.
        let pinned_set: HashSet<&NodeId> =
            self.compression.pinned_node_ids.iter().collect();
        let mut messages: Vec<Message> = Vec::new();
        let mut pinned: Vec<bool> = Vec::new();

        for node in &nodes {
            if let Some(msg) = project_node_to_message(node) {
                pinned.push(pinned_set.contains(&node.id));
                messages.push(msg);
            }
        }

        // 4. Extract system prompt.
        let system = match self.compression.system_mode {
            SystemMode::AlwaysFirst => {
                let sys_node = nodes
                    .iter()
                    .find(|n| matches!(n.content, NodeContent::SystemMessage { .. }));
                sys_node.and_then(extract_text_from_node)
            }
            SystemMode::FromConversation => {
                let conv = self.conversation.lock();
                conv.instructions.clone()
            }
            SystemMode::None => None,
        };

        if system.is_some() {
            // Remove system messages from both `messages` and the parallel `pinned` vec
            // in a single synchronized pass. Retaining one without the other would break
            // the length invariant that `truncate_messages` relies on.
            let mut i = 0;
            pinned.retain(|_| {
                let keep = messages[i].role != Role::System;
                i += 1;
                keep
            });
            messages.retain(|m| m.role != Role::System);
        }

        // 5. Query memory sources, inject as system section.
        let mut injected_memories: Vec<MemoryItem> = Vec::new();
        if !self.memory_sources.is_empty() {
            let query = last_user_text(&messages).unwrap_or_default();
            if !query.is_empty() {
                for src in &self.memory_sources {
                    let items = src.retrieve(&query, &conv_id, 5).await?;
                    injected_memories.extend(items);
                }
            }
        }

        // Inject memories as a system section, regardless of SystemMode.
        let final_system = if !injected_memories.is_empty() {
            let memory_text = injected_memories
                .iter()
                .map(|m| format!("- [{}]: {}", m.source, m.content))
                .collect::<Vec<_>>()
                .join("\n");
            let memory_section = format!("\n[Memory]\n{memory_text}");
            Some(match system {
                Some(s) => format!("{s}{memory_section}"),
                None => memory_section,
            })
        } else {
            system
        };

        // 6. Estimate tokens.
        let estimated = estimate_message_tokens(&messages, final_system.as_deref());

        // Budget available for messages after reserving space for the system prompt.
        // Passing the full max_tokens to truncate_messages would leave no room for
        // the system prompt when the system string is large.
        let system_tokens = final_system.as_deref().map_or(0, |s| estimate_tokens(s) as u64);
        let message_budget = self.compression.max_tokens.saturating_sub(system_tokens);

        // 7. Apply compression strategy.
        let mut server_compaction_needed = false;
        let mut summary: Option<String> = None;

        if estimated > self.compression.max_tokens {
            match &self.compression.strategy {
                CompressionStrategy::None => {}
                CompressionStrategy::Truncate => {
                    truncate_messages(
                        &mut messages,
                        &mut pinned,
                        message_budget,
                    );
                }
                CompressionStrategy::Summarize => {
                    if let Some(summarizer) = &self.compression.summarizer {
                        let (summ, kept) = summarize_old_messages(
                            &messages,
                            summarizer.as_ref(),
                            message_budget,
                        )
                        .await?;
                        messages = kept;
                        if !summ.is_empty() {
                            summary = Some(summ.clone());
                            messages.insert(
                                0,
                                Message {
                                    role: Role::User,
                                    content: vec![ContentBlock::text(format!(
                                        "[Summary of earlier conversation]: {summ}"
                                    ))],
                                    name: Some("summary".into()),
                                    cache_control: None,
                                },
                            );
                        }
                    } else {
                        truncate_messages(
                            &mut messages,
                            &mut pinned,
                            message_budget,
                        );
                    }
                }
                CompressionStrategy::SlidingWindow { keep_last } => {
                    let keep = *keep_last;
                    if messages.len() > keep {
                        let split = messages.len() - keep;
                        // Separate the dropped prefix into pinned and non-pinned.
                        // Pinned messages in the dropped prefix are preserved regardless
                        // of the window, consistent with Truncate behaviour.
                        let mut pinned_from_old: Vec<Message> = Vec::new();
                        let mut old_unpinned: Vec<Message> = Vec::new();
                        for (msg, is_pinned) in messages[..split].iter().zip(pinned[..split].iter()) {
                            if *is_pinned {
                                pinned_from_old.push(msg.clone());
                            } else {
                                old_unpinned.push(msg.clone());
                            }
                        }
                        let recent = messages[split..].to_vec();
                        let recent_pinned = pinned[split..].to_vec();

                        if let Some(summarizer) = &self.compression.summarizer {
                            let summ = summarizer.summarize(&old_unpinned).await?;
                            summary = Some(summ.clone());
                            // pinned_from_old: all true; summary msg: false; recent: carry over
                            pinned = vec![true; pinned_from_old.len()];
                            messages = pinned_from_old;
                            messages.push(Message {
                                role: Role::User,
                                content: vec![ContentBlock::text(format!(
                                    "[Summary of earlier conversation]: {summ}"
                                ))],
                                name: Some("summary".into()),
                                cache_control: None,
                            });
                            pinned.push(false);
                            messages.extend(recent);
                            pinned.extend(recent_pinned);
                        } else {
                            pinned = vec![true; pinned_from_old.len()];
                            messages = pinned_from_old;
                            messages.extend(recent);
                            pinned.extend(recent_pinned);
                        }
                    }
                }
                CompressionStrategy::ServerCompaction { .. } => {
                    server_compaction_needed = true;
                    // Return all messages; let the provider handle compaction.
                }
                CompressionStrategy::Fail => {
                    return Err(CmError::ContextOverflow(format!(
                        "Estimated {} tokens exceeds max {}",
                        estimated, self.compression.max_tokens
                    )));
                }
            }
        }

        let final_tokens = estimate_message_tokens(&messages, final_system.as_deref());

        Ok(ContextResult {
            messages,
            system: final_system,
            estimated_tokens: final_tokens,
            server_compaction_needed,
            summary,
        })
    }

    // -----------------------------------------------------------------------
    // spawn_agent
    // -----------------------------------------------------------------------

    /// Fork a new branch from the current HEAD and return a new `ContextManager`
    /// scoped to that branch. Returns `CmError::NoNodes` if the conversation
    /// has no nodes yet.
    ///
    /// The child starts with an empty extension registry. To give the agent its
    /// own CRDT or VFS, register extensions on the returned child before use:
    ///
    /// ```ignore
    /// let child = parent.spawn_agent(agent_id, system).await?
    ///     .with_extension(Arc::new(CrdtExtension::new(stable_agent_id)))
    ///     .with_extension(Arc::new(VfsExtension::new()));
    /// child.checkout(&child.active_branch()).await?; // loads CRDT + VFS state
    /// ```
    ///
    /// When the agent completes, call `parent.merge_from_branch(&child.active_branch())`
    /// to incorporate any kanban/canvas updates the agent made, then add an
    /// `AgentResult` node to the parent to make the work visible in `build_context`.
    pub async fn spawn_agent(
        &self,
        agent_id: AgentId,
        system: Option<String>,
    ) -> Result<ContextManager<B>, CmError> {
        let head = self
            .active_branch_tip()
            .ok_or(CmError::NoNodes)?;

        let agent_name = format!("agent/{}", agent_id.as_str());
        let new_branch_id = self.fork(&head, agent_name).await?;

        let child = ContextManager {
            backend: Arc::clone(&self.backend),
            tree: RwLock::new({
                // Clone the parent tree — the new branch is already registered in it.
                self.tree.read().clone()
            }),
            conversation: Mutex::new(self.conversation.lock().clone()),
            extensions: ExtensionRegistry::new(),
            compression: self.compression.clone(),
            memory_sources: self.memory_sources.clone(),
            // Each spawned agent is a new context instance and gets its own UUID
            // so its patch_conversation keys and instance_id() are distinct from
            // the parent's.
            client_id: uuid::Uuid::now_v7().to_string(),
            patch_seq: AtomicU64::new(0),
            last_compact_seq: AtomicU64::new(0),
        };

        // Point the child tree at the new branch. Extensions are empty; the caller
        // adds CrdtExtension / VfsExtension and calls checkout() to hydrate them.
        child.tree.write().checkout(&new_branch_id)?;

        // If a system override is given, add a system node.
        if let Some(sys) = system {
            child
                .add_node(
                    NodeContent::SystemMessage {
                        content: vec![ContentBlock::text(sys)],
                    },
                    NodeParams::default(),
                )
                .await?;
        }

        Ok(child)
    }

    // -----------------------------------------------------------------------
    // Convenience message helpers
    // -----------------------------------------------------------------------

    pub async fn add_user_message(
        &self,
        content: Vec<ContentBlock>,
        params: NodeParams,
    ) -> Result<NodeId, CmError> {
        self.add_node(
            NodeContent::UserMessage { content, name: None },
            params,
        )
        .await
    }

    pub async fn add_system_message(
        &self,
        content: Vec<ContentBlock>,
    ) -> Result<NodeId, CmError> {
        self.add_node(NodeContent::SystemMessage { content }, NodeParams::default())
            .await
    }

    pub async fn add_tool_result(
        &self,
        tool_use_id: &str,
        content: Vec<ContentBlock>,
        is_error: bool,
    ) -> Result<NodeId, CmError> {
        self.add_node(
            NodeContent::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content,
                is_error,
                duration_ms: None,
            },
            NodeParams::default(),
        )
        .await
    }

    /// Import a slice of Messages as nodes (one node per message).
    pub async fn import_messages(
        &self,
        messages: &[Message],
    ) -> Result<Vec<NodeId>, CmError> {
        let mut ids = Vec::new();
        for msg in messages {
            let content = match msg.role {
                Role::System => NodeContent::SystemMessage {
                    content: msg.content.clone(),
                },
                Role::User | Role::Other(_) => NodeContent::UserMessage {
                    content: msg.content.clone(),
                    name: msg.name.clone(),
                },
                Role::Assistant => NodeContent::AssistantMessage {
                    content: msg.content.clone(),
                    stop_reason: None,
                    variant_index: None,
                },
                Role::Tool => {
                    let tool_use_id = msg
                        .content
                        .iter()
                        .find_map(|b| b.as_tool_result().map(|t| t.tool_use_id.clone()))
                        .unwrap_or_default();
                    let is_error = msg
                        .content
                        .iter()
                        .any(|b| b.as_tool_result().is_some_and(|t| t.is_error));
                    NodeContent::ToolResult {
                        tool_use_id,
                        content: msg.content.clone(),
                        is_error,
                        duration_ms: None,
                    }
                }
            };
            let id = self.add_node(content, NodeParams::default()).await?;
            ids.push(id);
        }
        Ok(ids)
    }

    /// Project active branch to Messages (no compression).
    pub async fn to_messages(&self) -> Result<Vec<Message>, CmError> {
        let ids = {
            let tree = self.tree.read();
            tree.linearize_ids(tree.active_branch())?
        };
        let nodes = self.backend.get_nodes(&ids).await?;
        let mut messages = Vec::new();
        for node in &nodes {
            if node.deleted {
                continue;
            }
            if let Some(msg) = project_node_to_message(node) {
                messages.push(msg);
            }
        }
        Ok(messages)
    }

    // -----------------------------------------------------------------------
    // export_sft_jsonl
    // -----------------------------------------------------------------------

    /// Export dataset entries as SFT JSONL (one JSON object per line).
    ///
    /// Each line has `{"prompt": [...messages...], "completion": "..."}`.
    /// Returns the number of entries written.
    pub async fn export_sft_jsonl<W: std::io::Write>(
        &self,
        dataset_name: &str,
        split: Option<&DatasetSplit>,
        writer: &mut W,
    ) -> Result<usize, CmError> {
        let ns = format!("dataset:{dataset_name}");
        let keys = self.backend.kv_list(&ns, "").await?;
        let mut written = 0usize;

        for key in &keys {
            let Some(bytes) = self.backend.kv_get(&ns, key).await? else {
                continue;
            };
            let entry: DatasetEntry = serde_json::from_slice(&bytes)
                .map_err(|e| CmError::Serialization(e.to_string()))?;

            if split.is_some_and(|s| &entry.split != s) {
                continue;
            }

            // Fetch input + output nodes.
            let mut node_ids = entry.input_node_ids.clone();
            node_ids.push(entry.output_node_id.clone());
            let nodes = self.backend.get_nodes(&node_ids).await?;

            // Build a lookup so we can iterate in input_node_ids declared order,
            // which is required for deterministic ML training data.
            let node_map: HashMap<&NodeId, &Node> = nodes.iter().map(|n| (&n.id, n)).collect();
            let prompt_msgs: Vec<serde_json::Value> = entry
                .input_node_ids
                .iter()
                .filter_map(|nid| node_map.get(nid).copied())
                .filter_map(project_node_to_message)
                .map(|m| {
                    serde_json::json!({
                        "role": match &m.role {
                            Role::System => "system",
                            Role::User => "user",
                            Role::Assistant => "assistant",
                            Role::Tool => "tool",
                            Role::Other(s) => s.as_str(),
                        },
                        "content": m.content.iter()
                            .filter_map(|b| b.as_text().map(|t| t.to_string()))
                            .collect::<Vec<_>>()
                            .join("")
                    })
                })
                .collect();

            let completion = nodes
                .iter()
                .find(|n| n.id == entry.output_node_id)
                .and_then(project_node_to_message)
                .map(|m| {
                    m.content
                        .iter()
                        .filter_map(|b| b.as_text().map(|t| t.to_string()))
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();

            let expected = entry.expected_output.as_deref().unwrap_or(&completion);

            let line = serde_json::json!({
                "prompt": prompt_msgs,
                "completion": completion,
                "expected": expected,
                "split": serde_json::to_value(&entry.split).unwrap_or(serde_json::Value::Null),
            });

            serde_json::to_writer(&mut *writer, &line)
                .map_err(|e| CmError::Serialization(e.to_string()))?;
            writer
                .write_all(b"\n")
                .map_err(|e| CmError::BackendError(e.to_string()))?;
            written += 1;
        }

        Ok(written)
    }

    // -----------------------------------------------------------------------
    // list_conversations (associated function)
    // -----------------------------------------------------------------------

    pub async fn list_conversations(backend: &Arc<B>) -> Result<Vec<ConversationId>, CmError> {
        let keys = backend.kv_list("conversations", "").await?;
        Ok(keys
            .into_iter()
            .map(ConversationId::from_string)
            .collect())
    }
}

// ---------------------------------------------------------------------------
// KvMemorySource
// ---------------------------------------------------------------------------

/// Built-in [`MemorySource`] that persists entries in the `ContextBackend` KV store
/// (`ns = "memory:{scope_id}"`) and retrieves them via substring matching.
///
/// Suitable for small memory sets. Use an external vector DB source for production.
pub struct KvMemorySource<B: ContextBackend> {
    backend: Arc<B>,
    scope_id: String,
    source_name: String,
}

impl<B: ContextBackend> KvMemorySource<B> {
    pub fn new(backend: Arc<B>, scope_id: impl Into<String>) -> Self {
        let scope_id = scope_id.into();
        let source_name = format!("kv:{scope_id}");
        Self { backend, scope_id, source_name }
    }

    fn ns(&self) -> String {
        format!("memory:{}", self.scope_id)
    }
}

#[async_trait]
impl<B: ContextBackend> MemorySource for KvMemorySource<B> {
    async fn retrieve(
        &self,
        query: &str,
        _conv_id: &ConversationId,
        limit: u32,
    ) -> Result<Vec<MemoryItem>, CmError> {
        let ns = self.ns();
        let keys = self.backend.kv_list(&ns, "").await?;
        let query_lower = query.to_lowercase();
        let mut results = Vec::new();

        for key in &keys {
            if results.len() >= limit as usize {
                break;
            }
            let Some(bytes) = self.backend.kv_get(&ns, key).await? else {
                continue;
            };
            let entry: MemoryEntry = serde_json::from_slice(&bytes)
                .map_err(|e| CmError::Serialization(e.to_string()))?;

            if query_lower.is_empty() || entry.content.to_lowercase().contains(&query_lower) {
                results.push(MemoryItem {
                    content: entry.content,
                    source: self.source_name.clone(),
                    relevance_score: None,
                    metadata: entry.metadata,
                });
            }
        }

        Ok(results)
    }

    async fn store(
        &self,
        _conv_id: &ConversationId,
        item: &MemoryItem,
    ) -> Result<(), CmError> {
        let entry_id = MemoryEntryId::new();
        let now = now_micros();
        let entry = MemoryEntry {
            id: entry_id.clone(),
            scope_id: self.scope_id.clone(),
            content: item.content.clone(),
            memory_type: MemoryEntryType::Fact,
            source_conversation_id: None,
            created_at: now,
            updated_at: now,
            expires_at: None,
            metadata: item.metadata.clone(),
        };
        let bytes =
            serde_json::to_vec(&entry).map_err(|e| CmError::Serialization(e.to_string()))?;
        self.backend.kv_put(&self.ns(), entry_id.as_str(), &bytes).await
    }

    fn name(&self) -> &str {
        &self.source_name
    }
}

// ---------------------------------------------------------------------------
// project_node_to_message
// ---------------------------------------------------------------------------

pub(crate) fn project_node_to_message(node: &Node) -> Option<Message> {
    match &node.content {
        NodeContent::UserMessage { content, name } => Some(Message {
            role: Role::User,
            content: content.clone(),
            name: name.clone(),
            cache_control: None,
        }),
        NodeContent::AssistantMessage { content, .. } => Some(Message {
            role: Role::Assistant,
            content: content.clone(),
            name: None,
            cache_control: None,
        }),
        NodeContent::SystemMessage { content } => Some(Message {
            role: Role::System,
            content: content.clone(),
            name: None,
            cache_control: None,
        }),
        NodeContent::ToolResult {
            tool_use_id,
            content,
            is_error,
            ..
        } => {
            use crate::types::ToolResultBlock;
            Some(Message {
                role: Role::Tool,
                content: vec![ContentBlock::ToolResult(ToolResultBlock {
                    tool_use_id: tool_use_id.clone(),
                    content: content.clone(),
                    is_error: *is_error,
                })],
                name: None,
                cache_control: None,
            })
        }
        NodeContent::AgentResult { content, .. } => Some(Message {
            role: Role::Assistant,
            content: content.clone(),
            name: None,
            cache_control: None,
        }),
        NodeContent::MediaCapture {
            transcription: Some(text),
            ..
        } => Some(Message {
            role: Role::User,
            content: vec![ContentBlock::text(text.clone())],
            name: None,
            cache_control: None,
        }),
        NodeContent::ComputerAction {
            result: Some(result),
            ..
        } => Some(Message {
            role: Role::Tool,
            content: vec![ContentBlock::text(result.to_string())],
            name: None,
            cache_control: None,
        }),
        NodeContent::SkillInvocation {
            output: Some(output),
            ..
        } => Some(Message {
            role: Role::Tool,
            content: vec![ContentBlock::text(output.to_string())],
            name: None,
            cache_control: None,
        }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn extract_text_from_node(node: &Node) -> Option<String> {
    match &node.content {
        NodeContent::SystemMessage { content } | NodeContent::UserMessage { content, .. } => {
            let texts: Vec<&str> = content.iter().filter_map(|b| b.as_text()).collect();
            if texts.is_empty() { None } else { Some(texts.join("\n")) }
        }
        _ => None,
    }
}

fn last_user_text(messages: &[Message]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .and_then(|m| m.content.iter().find_map(|b| b.as_text().map(ToString::to_string)))
}

fn estimate_single_message_tokens(msg: &Message) -> u64 {
    msg.content.iter().map(estimate_block_tokens).sum()
}

fn estimate_block_tokens(block: &ContentBlock) -> u64 {
    match block {
        ContentBlock::Text(t) => estimate_tokens(&t.text) as u64,
        ContentBlock::Thinking(t) => estimate_tokens(&t.text) as u64,
        ContentBlock::ToolUse(t) => {
            estimate_tokens(&t.name) as u64 + estimate_tokens(&t.input.to_string()) as u64
        }
        ContentBlock::ToolResult(t) => t.content.iter().map(estimate_block_tokens).sum(),
        ContentBlock::Image(_) => 1000,
        ContentBlock::Audio(_) => 500,
        ContentBlock::Video(_) => 2000,
        ContentBlock::Document(_) => 1500,
    }
}

fn estimate_message_tokens(messages: &[Message], system: Option<&str>) -> u64 {
    let mut total: u64 = messages.iter().map(estimate_single_message_tokens).sum();
    if let Some(sys) = system {
        total += estimate_tokens(sys) as u64;
    }
    total
}

/// Drop the oldest non-system, non-pinned messages until `messages` fits within
/// `max_tokens`.  `pinned` is a parallel slice (same length as `messages`) where
/// `true` means the corresponding message must never be removed.
///
/// O(n): token counts are computed once, removal is a single-pass retain.
/// The `pinned` vec is kept in sync so callers can reuse it after truncation.
fn truncate_messages(messages: &mut Vec<Message>, pinned: &mut Vec<bool>, max_tokens: u64) {
    debug_assert_eq!(messages.len(), pinned.len(), "pinned must be parallel to messages");

    if estimate_message_tokens(messages, None) <= max_tokens {
        return;
    }

    // Pre-compute per-message token counts to avoid O(n²) re-estimation.
    let token_counts: Vec<u64> = messages.iter().map(estimate_single_message_tokens).collect();
    let mut total: u64 = token_counts.iter().sum();

    // Mark messages to remove in a single forward pass (oldest first).
    let mut remove = vec![false; messages.len()];
    for i in 0..messages.len() {
        if total <= max_tokens {
            break;
        }
        if messages[i].role != Role::System && !pinned[i] {
            remove[i] = true;
            total = total.saturating_sub(token_counts[i]);
        }
    }

    // Single-pass retain keeps both vecs in sync.
    let mut idx = 0usize;
    messages.retain(|_| {
        let keep = !remove[idx];
        idx += 1;
        keep
    });
    idx = 0;
    pinned.retain(|_| {
        let keep = !remove[idx];
        idx += 1;
        keep
    });
}

async fn summarize_old_messages(
    messages: &[Message],
    summarizer: &dyn Summarizer,
    max_tokens: u64,
) -> Result<(String, Vec<Message>), CmError> {
    // Default: keep everything (nothing to summarize).
    let mut keep_from = 0usize;
    let mut tokens = 0u64;

    for (i, msg) in messages.iter().enumerate().rev() {
        let msg_tokens = estimate_single_message_tokens(msg);
        if tokens + msg_tokens > max_tokens / 2 {
            keep_from = i + 1;
            break;
        }
        tokens += msg_tokens;
    }

    if keep_from == 0 {
        // All messages fit within the budget; nothing to summarize.
        return Ok((String::new(), messages.to_vec()));
    }

    let to_summarize = &messages[..keep_from];
    let to_keep = messages[keep_from..].to_vec();
    let summary = summarizer.summarize(to_summarize).await?;
    Ok((summary, to_keep))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::backend::InMemoryContextBackend;

    fn make_backend() -> Arc<InMemoryContextBackend> {
        Arc::new(InMemoryContextBackend::new())
    }

    #[tokio::test]
    async fn new_creates_conversation_and_branch() {
        let backend = make_backend();
        let conv = Conversation::new("test");
        let conv_id = conv.id.clone();

        let mgr = ContextManager::new(backend.clone(), conv).await.unwrap();
        assert_eq!(mgr.active_branch_tip(), None);

        // Conversation registered in KV.
        let keys = backend.kv_list("conversations", "").await.unwrap();
        assert!(keys.iter().any(|k| k == conv_id.as_str()));
    }

    #[tokio::test]
    async fn add_node_and_build_context() {
        let backend = make_backend();
        let conv = Conversation::new("ctx-test");

        let mgr = ContextManager::new(backend, conv).await.unwrap();

        mgr.add_system_message(vec![ContentBlock::text("You are helpful.")])
            .await
            .unwrap();
        mgr.add_user_message(vec![ContentBlock::text("Hello")], NodeParams::default())
            .await
            .unwrap();
        mgr.add_node(
            NodeContent::AssistantMessage {
                content: vec![ContentBlock::text("Hi!")],
                stop_reason: None,
                variant_index: None,
            },
            NodeParams::default(),
        )
        .await
        .unwrap();

        let result = mgr.build_context().await.unwrap();
        assert!(result.system.is_some());
        // System message removed from messages list when system is Some.
        assert_eq!(result.messages.iter().filter(|m| m.role == Role::System).count(), 0);
        assert_eq!(result.messages.len(), 2);
        assert!(result.estimated_tokens > 0);
    }

    #[tokio::test]
    async fn truncate_overflow() {
        let backend = make_backend();
        let conv = Conversation::new("trunc");

        let compression = CompressionConfig {
            max_tokens: 5,
            strategy: CompressionStrategy::Truncate,
            system_mode: SystemMode::None,
            pinned_node_ids: Vec::new(),
            summarizer: None,
        };

        let mgr = ContextManager::new(backend, conv)
            .await
            .unwrap()
            .with_compression(compression);

        for i in 0..10 {
            mgr.add_user_message(
                vec![ContentBlock::text(format!("message {i}"))],
                NodeParams::default(),
            )
            .await
            .unwrap();
        }

        let result = mgr.build_context().await.unwrap();
        assert!(result.messages.len() < 10);
    }

    #[tokio::test]
    async fn fail_overflow() {
        let backend = make_backend();
        let conv = Conversation::new("fail");

        let compression = CompressionConfig {
            max_tokens: 1,
            strategy: CompressionStrategy::Fail,
            system_mode: SystemMode::None,
            pinned_node_ids: Vec::new(),
            summarizer: None,
        };

        let mgr = ContextManager::new(backend, conv)
            .await
            .unwrap()
            .with_compression(compression);

        mgr.add_user_message(vec![ContentBlock::text("hello")], NodeParams::default())
            .await
            .unwrap();

        let result = mgr.build_context().await;
        assert!(matches!(result, Err(CmError::ContextOverflow(_))));
    }

    #[tokio::test]
    async fn sliding_window_overflow() {
        let backend = make_backend();
        let conv = Conversation::new("sliding");

        let compression = CompressionConfig {
            max_tokens: 10,
            strategy: CompressionStrategy::SlidingWindow { keep_last: 3 },
            system_mode: SystemMode::None,
            pinned_node_ids: Vec::new(),
            summarizer: None,
        };

        let mgr = ContextManager::new(backend, conv)
            .await
            .unwrap()
            .with_compression(compression);

        for i in 0..20 {
            mgr.add_user_message(
                vec![ContentBlock::text(format!("msg {i}"))],
                NodeParams::default(),
            )
            .await
            .unwrap();
        }

        let result = mgr.build_context().await.unwrap();
        assert_eq!(result.messages.len(), 3);
    }

    #[tokio::test]
    async fn load_roundtrip() {
        let backend = make_backend();
        let conv = Conversation::new("load-test");
        let conv_id = conv.id.clone();

        let mgr = ContextManager::new(Arc::clone(&backend), conv).await.unwrap();
        mgr.add_user_message(vec![ContentBlock::text("hello")], NodeParams::default())
            .await
            .unwrap();

        // Load from backend.
        let mgr2 = ContextManager::load(backend, &conv_id).await.unwrap();
        let msgs = mgr2.to_messages().await.unwrap();
        assert_eq!(msgs.len(), 1);
    }

    #[tokio::test]
    async fn patch_conversation_applies() {
        let backend = make_backend();
        let conv = Conversation::new("patch-test");

        let mgr = ContextManager::new(backend, conv).await.unwrap();
        mgr.patch_conversation(ConversationPatch {
            title: Some("New Title".into()),
            ..Default::default()
        })
        .await
        .unwrap();

        let conv = mgr.conversation();
        assert_eq!(conv.title, Some("New Title".into()));
    }

    #[tokio::test]
    async fn list_conversations_returns_registered() {
        let backend = make_backend();
        let conv1 = Conversation::new("c1");
        let conv2 = Conversation::new("c2");
        let id1 = conv1.id.clone();
        let id2 = conv2.id.clone();

        ContextManager::new(Arc::clone(&backend), conv1).await.unwrap();
        ContextManager::new(Arc::clone(&backend), conv2).await.unwrap();

        let ids = ContextManager::list_conversations(&backend).await.unwrap();
        assert!(ids.iter().any(|id| id == &id1));
        assert!(ids.iter().any(|id| id == &id2));
    }

    #[tokio::test]
    async fn spawn_agent_no_nodes_returns_error() {
        let backend = make_backend();
        let conv = Conversation::new("spawn");

        let mgr = ContextManager::new(backend, conv).await.unwrap();
        let result = mgr.spawn_agent(AgentId::new(), None).await;
        assert!(matches!(result, Err(CmError::NoNodes)));
    }

    #[tokio::test]
    async fn import_messages_roundtrip() {
        let backend = make_backend();
        let conv = Conversation::new("import");

        let mgr = ContextManager::new(backend, conv).await.unwrap();

        let messages = vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::text("Hello")],
                name: None,
                cache_control: None,
            },
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::text("Hi!")],
                name: None,
                cache_control: None,
            },
        ];
        let ids = mgr.import_messages(&messages).await.unwrap();
        assert_eq!(ids.len(), 2);

        let result = mgr.to_messages().await.unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].role, Role::User);
        assert_eq!(result[1].role, Role::Assistant);
    }

    #[tokio::test]
    async fn project_node_to_message_covers_variants() {
        let conv_id = ConversationId::new();
        let branch_id = BranchId::new();

        let tool_node = Node {
            id: NodeId::new(),
            conversation_id: conv_id.clone(),
            branch_id: branch_id.clone(),
            parent_id: None,
            sequence: 0,
            created_at: 0,
            created_by: None,
            model: None,
            provider: None,
            content: NodeContent::ToolResult {
                tool_use_id: "tu_1".into(),
                content: vec![ContentBlock::text("result")],
                is_error: false,
                duration_ms: None,
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
        };

        let msg = project_node_to_message(&tool_node).unwrap();
        assert_eq!(msg.role, Role::Tool);
    }

    #[tokio::test]
    async fn deleted_nodes_filtered_in_build_context() {
        let backend = make_backend();
        let conv = Conversation::new("deleted-filter");

        let mgr = ContextManager::new(backend, conv).await.unwrap();
        mgr.add_user_message(vec![ContentBlock::text("keep")], NodeParams::default())
            .await
            .unwrap();
        let del_id = mgr
            .add_user_message(vec![ContentBlock::text("delete me")], NodeParams::default())
            .await
            .unwrap();

        mgr.backend.soft_delete_node(&del_id).await.unwrap();

        let result = mgr.build_context().await.unwrap();
        assert_eq!(result.messages.len(), 1);
        assert!(result.messages[0]
            .content
            .iter()
            .any(|b| b.as_text().is_some_and(|t| t == "keep")));
    }

    #[tokio::test]
    async fn server_compaction_flag() {
        let backend = make_backend();
        let conv = Conversation::new("compaction");

        let compression = CompressionConfig {
            max_tokens: 1,
            strategy: CompressionStrategy::ServerCompaction { compact_threshold: 10 },
            system_mode: SystemMode::None,
            pinned_node_ids: Vec::new(),
            summarizer: None,
        };

        let mgr = ContextManager::new(backend, conv)
            .await
            .unwrap()
            .with_compression(compression);

        mgr.add_user_message(vec![ContentBlock::text("hello")], NodeParams::default())
            .await
            .unwrap();

        let result = mgr.build_context().await.unwrap();
        assert!(result.server_compaction_needed);
        assert_eq!(result.messages.len(), 1); // all messages returned as-is
    }

    #[tokio::test]
    async fn memory_source_always_injected() {
        let backend = make_backend();
        let conv = Conversation::new("mem-inject");
        let scope_id = crate::context::types::UserId::new();

        let mem_src = KvMemorySource::new(Arc::clone(&backend), scope_id.as_str());

        // Pre-populate a memory item.
        let item = MemoryItem {
            content: "User prefers short answers".into(),
            source: "test".into(),
            relevance_score: None,
            metadata: HashMap::new(),
        };
        mem_src
            .store(&ConversationId::new(), &item)
            .await
            .unwrap();

        let mgr = ContextManager::new(Arc::clone(&backend), conv)
            .await
            .unwrap()
            .with_memory_source(Arc::new(KvMemorySource::new(
                Arc::clone(&backend),
                scope_id.as_str(),
            )));

        mgr.add_user_message(vec![ContentBlock::text("short answer")], NodeParams::default())
            .await
            .unwrap();

        let result = mgr.build_context().await.unwrap();
        // Memory is injected into system prompt regardless of system mode.
        let sys = result.system.unwrap_or_default();
        assert!(sys.contains("User prefers short answers"), "system: {sys}");
    }

    #[tokio::test]
    async fn fork_checkout_restores_crdt_at_watermark() {
        let backend = make_backend();
        let conv = Conversation::new("fork-crdt");

        let mgr = ContextManager::new(Arc::clone(&backend), conv)
            .await
            .unwrap()
            .with_extension(Arc::new(CrdtExtension::new("main")));

        // Write CRDT entry BEFORE committing the node so the auto-push captures it.
        if let Some(crdt) = mgr.extension::<CrdtExtension>() {
            crdt.map_set("items", "k1", r#"{"id":"k1"}"#);
        }

        // Add a node — this auto-pushes CRDT and records the watermark.
        let node_id = mgr
            .add_user_message(vec![ContentBlock::text("root")], NodeParams::default())
            .await
            .unwrap();

        // Fork from the head node (which carries the CRDT watermark).
        let new_branch_id = mgr.fork(&node_id, "child").await.unwrap();

        // Checkout the child branch — restores CRDT to fork-point state.
        mgr.checkout(&new_branch_id).await.unwrap();

        // CRDT state should be visible on child.
        if let Some(crdt) = mgr.extension::<CrdtExtension>() {
            let entries = crdt.map_entries("items");
            assert!(entries.contains_key("k1"), "fork must inherit parent CRDT state");
        }
    }

    #[tokio::test]
    async fn load_crdt_checkout_roundtrip() {
        let backend = make_backend();
        let conv = Conversation::new("load-crdt");
        let conv_id = conv.id.clone();

        // Create, add CRDT state, add a node to record the watermark.
        let mgr = ContextManager::new(Arc::clone(&backend), conv)
            .await
            .unwrap()
            .with_extension(Arc::new(CrdtExtension::new("c1")));

        if let Some(crdt) = mgr.extension::<CrdtExtension>() {
            crdt.map_set("ns", "key", "value");
        }
        mgr.add_user_message(vec![ContentBlock::text("hello")], NodeParams::default())
            .await
            .unwrap();

        let active_branch = mgr.active_branch();

        // Load into a fresh ContextManager with a new CrdtExtension, then checkout.
        let mgr2 = ContextManager::load(Arc::clone(&backend), &conv_id)
            .await
            .unwrap()
            .with_extension(Arc::new(CrdtExtension::new("c2")));
        mgr2.checkout(&active_branch).await.unwrap();

        // CRDT state must be restored after checkout.
        let crdt2 = mgr2.extension::<CrdtExtension>().unwrap();
        let entries = crdt2.map_entries("ns");
        assert!(entries.contains_key("key"), "CRDT state must survive load+checkout");
        assert_eq!(entries["key"], "value");
    }

    #[tokio::test]
    async fn finalize_streaming_node_no_occ() {
        let backend = make_backend();
        let conv = Conversation::new("streaming");

        let mgr = ContextManager::new(backend, conv).await.unwrap();

        // start_streaming creates a non-final node with streaming state.
        let id = mgr
            .start_streaming(
                NodeContent::AssistantMessage {
                    content: vec![ContentBlock::text("...")],
                    stop_reason: None,
                    variant_index: None,
                },
                NodeParams::default(),
            )
            .await
            .unwrap();

        let node = mgr.backend.get_node(&id).await.unwrap().unwrap();
        assert!(!node.is_final);
        assert!(node.streaming.is_some());

        // finalize_streaming_node must not fail regardless of version.
        mgr.finalize_streaming_node(
            &id,
            NodeContent::AssistantMessage {
                content: vec![ContentBlock::text("Full response.")],
                stop_reason: None,
                variant_index: None,
            },
            None,
        )
        .await
        .unwrap();

        let updated = mgr.backend.get_node(&id).await.unwrap().unwrap();
        assert!(updated.is_final);
        assert!(updated.streaming.is_none());
        // Content replaced
        if let NodeContent::AssistantMessage { content, .. } = &updated.content {
            assert!(content[0].as_text().unwrap().contains("Full response"));
        }
    }

    #[tokio::test]
    async fn summarize_compression() {
        use std::sync::Arc as StdArc;

        struct EchoSummarizer;

        #[async_trait::async_trait]
        impl Summarizer for EchoSummarizer {
            async fn summarize(&self, messages: &[Message]) -> Result<String, CmError> {
                Ok(format!("summarized {} messages", messages.len()))
            }
        }

        let backend = make_backend();
        let conv = Conversation::new("summarize");

        let compression = CompressionConfig {
            max_tokens: 5,
            strategy: CompressionStrategy::Summarize,
            system_mode: SystemMode::None,
            pinned_node_ids: Vec::new(),
            summarizer: Some(StdArc::new(EchoSummarizer)),
        };

        let mgr = ContextManager::new(backend, conv)
            .await
            .unwrap()
            .with_compression(compression);

        for i in 0..10 {
            mgr.add_user_message(
                vec![ContentBlock::text(format!("message {i}"))],
                NodeParams::default(),
            )
            .await
            .unwrap();
        }

        let result = mgr.build_context().await.unwrap();
        // Summary is returned.
        assert!(result.summary.is_some());
        let s = result.summary.unwrap();
        assert!(s.contains("summarized"), "summary: {s}");
        // The summary is injected as the first message.
        assert!(!result.messages.is_empty());
    }

    #[test]
    fn maybe_cleared_serde_roundtrip() {
        use crate::context::types::{ConversationPatch, MaybeCleared};

        // Set variant
        let patch_set = ConversationPatch {
            instructions: Some(MaybeCleared::Set("be helpful".into())),
            ..Default::default()
        };
        let json = serde_json::to_string(&patch_set).unwrap();
        let back: ConversationPatch = serde_json::from_str(&json).unwrap();
        match back.instructions.unwrap() {
            MaybeCleared::Set(s) => assert_eq!(s, "be helpful"),
            MaybeCleared::Clear => panic!("expected Set"),
        }

        // Clear variant — must be distinguishable from Set in JSON
        let patch_clear = ConversationPatch {
            instructions: Some(MaybeCleared::Clear),
            ..Default::default()
        };
        let json_clear = serde_json::to_string(&patch_clear).unwrap();
        let back_clear: ConversationPatch = serde_json::from_str(&json_clear).unwrap();
        assert!(matches!(back_clear.instructions, Some(MaybeCleared::Clear)));

        // The two JSON representations must differ
        assert_ne!(json, json_clear);
    }

    #[tokio::test]
    async fn export_sft_jsonl_basic() {
        use crate::context::types::{DatasetEntry, DatasetEntryId, DatasetSplit};

        let backend = make_backend();
        let conv = Conversation::new("sft-test");
        let conv_id = conv.id.clone();

        let mgr = ContextManager::new(Arc::clone(&backend), conv).await.unwrap();
        let input_id = mgr
            .add_user_message(vec![ContentBlock::text("What is 2+2?")], NodeParams::default())
            .await
            .unwrap();
        let output_id = mgr
            .add_node(
                NodeContent::AssistantMessage {
                    content: vec![ContentBlock::text("4")],
                    stop_reason: None,
                    variant_index: None,
                },
                NodeParams::default(),
            )
            .await
            .unwrap();

        // Store a DatasetEntry in KV.
        let entry = DatasetEntry {
            id: DatasetEntryId::new(),
            conversation_id: conv_id.clone(),
            dataset_name: "my_dataset".into(),
            input_node_ids: vec![input_id],
            output_node_id: output_id,
            expected_output: None,
            split: DatasetSplit::Train,
            created_at: now_micros(),
            metadata: HashMap::new(),
        };
        let bytes = serde_json::to_vec(&entry).unwrap();
        backend
            .kv_put("dataset:my_dataset", entry.id.as_str(), &bytes)
            .await
            .unwrap();

        let mut buf = Vec::new();
        let count = mgr
            .export_sft_jsonl("my_dataset", None, &mut buf)
            .await
            .unwrap();

        assert_eq!(count, 1);
        let line: serde_json::Value = serde_json::from_slice(&buf[..buf.len() - 1]).unwrap();
        assert_eq!(line["completion"], "4");
        assert_eq!(line["split"], "train");
    }

    /// Sub-agent spawned from a child can immediately spawn its own sub-agent
    /// (branch tip is initialized from the fork node).
    #[tokio::test]
    async fn sub_agent_can_spawn_sub_agent() {
        let backend = make_backend();
        let conv = Conversation::new("nested-agents");

        let root = ContextManager::new(backend, conv).await.unwrap();

        // Root needs at least one node before spawn_agent can fork.
        root.add_user_message(vec![ContentBlock::text("start")], NodeParams::default())
            .await
            .unwrap();

        // First level sub-agent.
        let child = root.spawn_agent(AgentId::new(), None).await.unwrap();
        child
            .add_user_message(vec![ContentBlock::text("child work")], NodeParams::default())
            .await
            .unwrap();

        // Second level: child spawns its own sub-agent — must not return NoNodes.
        let grandchild = child.spawn_agent(AgentId::new(), None).await;
        assert!(
            grandchild.is_ok(),
            "sub-agent must be able to spawn its own sub-agents; got: {:?}",
            grandchild.err()
        );
    }

    /// Sub-agent kanban/canvas updates are visible to the parent after merge_from_branch.
    #[tokio::test]
    async fn merge_from_branch_propagates_crdt() {
        let backend = make_backend();
        let conv = Conversation::new("merge-test");

        let parent = ContextManager::new(Arc::clone(&backend), conv)
            .await
            .unwrap()
            .with_extension(Arc::new(CrdtExtension::new("parent")));

        parent
            .add_user_message(vec![ContentBlock::text("task")], NodeParams::default())
            .await
            .unwrap();

        // Spawn child and equip it with its own CRDT extension.
        let child_branch = parent.active_branch();
        let child = parent.spawn_agent(AgentId::new(), None).await.unwrap()
            .with_extension(Arc::new(CrdtExtension::new("agent")));
        child.checkout(&child.active_branch()).await.unwrap();

        // Agent writes a kanban update on its own branch.
        if let Some(crdt) = child.extension::<CrdtExtension>() {
            crdt.map_set("kanban:board1:cpos", "card-1", r#"{"column_id":"done","position":0}"#);
        }
        // Persist agent's CRDT to backend.
        if let Some(crdt) = child.extension::<CrdtExtension>() {
            let conv_id = child.conversation().id.clone();
            let branch_id = child.active_branch();
            crdt.push(&conv_id, &branch_id, child.backend().as_ref()).await.unwrap();
        }

        let agent_branch = child.active_branch();

        // Parent merges agent's branch — no parent push needed to read in-memory.
        parent.merge_from_branch(&agent_branch).await.unwrap();

        let parent_crdt = parent.extension::<CrdtExtension>().unwrap();
        let entries = parent_crdt.map_entries("kanban:board1:cpos");
        assert!(
            entries.contains_key("card-1"),
            "parent must see agent's kanban update after merge_from_branch"
        );
        let _ = child_branch; // suppress unused warning
    }

    /// Workspace supports multiple kanban boards.
    #[test]
    fn workspace_multiple_kanban_boards() {
        use super::types::{KanbanBoardId, Workspace};

        let mut ws = Workspace::default();
        assert!(ws.kanban_ids.is_empty());

        let b1 = KanbanBoardId::new();
        let b2 = KanbanBoardId::new();
        ws.kanban_ids.push(b1.clone());
        ws.kanban_ids.push(b2.clone());

        assert_eq!(ws.kanban_ids.len(), 2);

        // Roundtrip through JSON (serde).
        let json = serde_json::to_string(&ws).unwrap();
        let ws2: Workspace = serde_json::from_str(&json).unwrap();
        assert_eq!(ws2.kanban_ids.len(), 2);
        assert!(ws2.kanban_ids.contains(&b1));
        assert!(ws2.kanban_ids.contains(&b2));
    }
}
