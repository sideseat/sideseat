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

mod build;
mod extension;
mod memory;

pub(crate) use build::project_node_to_message;
pub use build::{CompressionConfig, CompressionStrategy, ContextResult, SystemMode};
pub use extension::{ContextExtension, ExtensionRegistry};
pub use memory::{KvMemorySource, MemoryItem, MemorySource, Summarizer};

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::{Mutex, RwLock};

use self::backend::{ContextBackend, NodePatch};
use self::crdt::CrdtExtension;
use self::error::CmError;
use self::tree::ConversationTree;
use self::types::{
    AgentId, BranchId, BranchMeta, Conversation, ConversationId, ConversationPatch, DatasetEntry,
    DatasetSplit, Node, NodeContent, NodeId, NodeParams, StreamStatus, StreamingState, now_micros,
};
use self::vfs::VfsExtension;
use crate::types::{ContentBlock, Message, Response, Role, Usage};

// ---------------------------------------------------------------------------
// ContextManager<B>
// ---------------------------------------------------------------------------

/// Auto-compact CRDT delta log when delta count since last compaction exceeds this threshold.
const CRDT_COMPACT_THRESHOLD: u64 = 500;

/// Stateless conversation manager backed by a [`ContextBackend`].
///
/// Owns an append-only [`ConversationTree`], pluggable [`ContextExtension`]s (CRDT, VFS,
/// Canvas, Kanban), and a [`CompressionConfig`] for building LLM context windows.
/// All persistent state lives in the backend; `ContextManager` itself holds only
/// in-memory indexes and is safe to reconstruct at any time via [`load`].
///
/// # Typical workflow
/// 1. [`new`] or [`load`] to obtain an instance.
/// 2. [`with_extension`] to register CRDT/VFS/Canvas/Kanban extensions.
/// 3. [`checkout`] to restore CRDT + VFS state for the active branch.
/// 4. [`add_node`] / [`start_streaming`] + [`finalize_streaming_node`] to record turns.
/// 5. [`build_context`] to produce a compressed [`ContextResult`] for the next LLM call.
/// 6. [`fork`] to create experiment branches; [`spawn_agent`] for sub-agents.
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
            .kv_put(
                "conversations",
                conv_id.as_str(),
                conv_id.as_str().as_bytes(),
            )
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

    /// Register an extension and fire its `on_branch_checked_out` hook for the active branch.
    pub fn with_extension<T: ContextExtension>(self, ext: Arc<T>) -> Self {
        let current_branch = self.tree.read().active_branch().clone();
        ext.on_branch_checked_out(&current_branch);
        self.extensions.register(ext);
        self
    }

    /// Override the default compression configuration.
    pub fn with_compression(mut self, cfg: CompressionConfig) -> Self {
        self.compression = cfg;
        self
    }

    /// Add a memory source whose retrieved items are injected into `build_context()` system prompts.
    pub fn with_memory_source(mut self, src: Arc<dyn MemorySource>) -> Self {
        self.memory_sources.push(src);
        self
    }

    /// Override the auto-generated instance UUID. Affects `patch_conversation` key scoping
    /// but not `CrdtExtension` identity — pass a stable ID to `CrdtExtension::new()` for that.
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

    /// The underlying backend shared with all clones and sub-agents.
    pub fn backend(&self) -> &Arc<B> {
        &self.backend
    }

    pub fn extension<T: ContextExtension>(&self) -> Option<Arc<T>> {
        self.extensions.extension::<T>()
    }

    /// Look up a registered extension by its string ID and concrete type.
    pub fn extension_by_id<T: ContextExtension>(&self, id: &str) -> Option<Arc<T>> {
        self.extensions.extension_by_id::<T>(id)
    }

    /// Access the full extension registry (for batch operations or introspection).
    pub fn extensions(&self) -> &ExtensionRegistry {
        &self.extensions
    }

    /// Snapshot of the current conversation metadata.
    pub fn conversation(&self) -> Conversation {
        self.conversation.lock().clone()
    }

    /// Currently checked-out branch ID.
    pub fn active_branch(&self) -> BranchId {
        self.tree.read().active_branch().clone()
    }

    /// Most recently appended node on the active branch, or `None` if the branch is empty.
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
        let branch_ids: Vec<BranchId> = self.tree.read().branches().keys().cloned().collect();
        for branch_id in &branch_ids {
            if let Some(data) = self.backend.kv_get("vfs_index", branch_id.as_str()).await? {
                vfs.load_branch_index(branch_id.as_str(), &data)?;
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Conversation patching
    // -----------------------------------------------------------------------

    /// Apply a granular update to conversation metadata, persisting it as an append-only patch.
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

    /// Fork a new branch from `from_node_id`, persisting the branch and a CRDT snapshot
    /// at the fork watermark before registering it in the in-memory tree.
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
        let vfs_data = self.backend.kv_get("vfs_index", branch_id.as_str()).await?;

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
                    } else if let Err(e) = crdt
                        .compact(&conv_id, &branch_id, self.backend.as_ref())
                        .await
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
        self.backend
            .append_nodes(std::slice::from_ref(&node))
            .await?;

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
            Some(
                crdt.push(&conv_id, &branch_id, self.backend.as_ref())
                    .await?,
            )
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
        let deltas = self
            .backend
            .crdt_fetch(&conv_id, from_branch, snap_seq)
            .await?;

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
    /// Convenience: record a complete provider response as an `AssistantMessage` node.
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
        let head = self.active_branch_tip().ok_or(CmError::NoNodes)?;

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
            NodeContent::UserMessage {
                content,
                name: None,
            },
            params,
        )
        .await
    }

    pub async fn add_system_message(&self, content: Vec<ContentBlock>) -> Result<NodeId, CmError> {
        self.add_node(
            NodeContent::SystemMessage { content },
            NodeParams::default(),
        )
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
    pub async fn import_messages(&self, messages: &[Message]) -> Result<Vec<NodeId>, CmError> {
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
        Ok(keys.into_iter().map(ConversationId::from_string).collect())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
