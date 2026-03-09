pub mod artifact;
pub mod canvas;
pub mod context;
pub mod crdt;
pub mod error;
pub mod migrate;
pub mod source;
pub mod storage;
pub mod tree;
pub mod types;
pub mod vfs;

use std::sync::Arc;

use parking_lot::Mutex;

use self::context::{
    ContextManager, ContextResult, ContextStrategy, MemorySource, Summarizer,
    project_node_to_message,
};
use self::crdt::CrdtDoc;
use self::error::HistoryError;
use self::source::{
    ChunkStrategy, DocumentExtractor, PlainTextExtractor, Source,
    SourceChunk, SourceStatus, SourceType, chunk_text,
};
use self::storage::{HistoryStorage, NodePatch};
use self::tree::{BranchDiff, ConversationTree};
use self::types::*;
use self::vfs::Vfs;
use crate::types::{ContentBlock, Message, Response, Role, Usage};

// ---------------------------------------------------------------------------
// History facade
// ---------------------------------------------------------------------------

pub struct History<S: HistoryStorage> {
    storage: S,
    tree: Mutex<ConversationTree>,
    crdt: Mutex<Option<CrdtDoc>>,
    conversation: Mutex<Conversation>,
    vfs: Mutex<Option<Vfs>>,
}

impl<S: HistoryStorage> History<S> {
    pub fn new(storage: S, conversation: Conversation) -> Self {
        let tree = ConversationTree::new(conversation.id.clone());
        Self {
            storage,
            tree: Mutex::new(tree),
            crdt: Mutex::new(None),
            conversation: Mutex::new(conversation),
            vfs: Mutex::new(None),
        }
    }

    pub fn with_vfs(self, vfs: Vfs) -> Self {
        *self.vfs.lock() = Some(vfs);
        self
    }

    pub fn vfs(&self) -> parking_lot::MappedMutexGuard<'_, Vfs> {
        parking_lot::MutexGuard::map(self.vfs.lock(), |v| {
            v.get_or_insert_with(|| {
                Vfs::new(Arc::new(vfs::MemoryFsProvider::new()))
            })
        })
    }

    pub async fn load(storage: S, id: &ConversationId) -> Result<Self, HistoryError> {
        let conversation = storage
            .get_conversation(id)
            .await?
            .ok_or_else(|| HistoryError::ConversationNotFound(id.clone()))?;

        let mut tree = ConversationTree::new(id.clone());

        // Load branches
        let branches = storage.list_branches(id).await?;
        for branch in branches {
            tree.add_branch(branch);
        }

        // Load all node headers and register them
        let headers = storage.list_node_headers(id).await?;
        // We need full nodes to register (headers don't have enough info for tree)
        // But we can reconstruct headers from the stored data
        let node_ids: Vec<NodeId> = headers.iter().map(|h| h.id.clone()).collect();
        let nodes = storage.get_nodes(&node_ids).await?;
        for node in &nodes {
            tree.register(node)?;
        }

        Ok(Self {
            storage,
            tree: Mutex::new(tree),
            crdt: Mutex::new(None),
            conversation: Mutex::new(conversation),
            vfs: Mutex::new(None),
        })
    }

    // -----------------------------------------------------------------------
    // Sync (tree-only, no storage I/O)
    // -----------------------------------------------------------------------

    pub fn conversation(&self) -> Conversation {
        self.conversation.lock().clone()
    }

    pub fn update_conversation(&self, f: impl FnOnce(&mut Conversation)) {
        let mut conv = self.conversation.lock();
        f(&mut conv);
        conv.updated_at = now_micros();
    }

    pub fn active_branch(&self) -> BranchId {
        self.tree.lock().active_branch().clone()
    }

    pub fn active_branch_tip(&self) -> Option<NodeId> {
        let tree = self.tree.lock();
        let branch = tree.active_branch().clone();
        tree.branch_tip(&branch).cloned()
    }

    pub fn checkout(&self, branch: &BranchId) -> Result<(), HistoryError> {
        self.tree.lock().checkout(branch)
    }

    pub fn diff(&self, a: &BranchId, b: &BranchId) -> Result<BranchDiff, HistoryError> {
        self.tree.lock().diff(a, b)
    }

    // -----------------------------------------------------------------------
    // Async (touches storage)
    // -----------------------------------------------------------------------

    pub async fn add_user_message(
        &self,
        content: Vec<ContentBlock>,
        params: NodeParams,
    ) -> Result<NodeId, HistoryError> {
        self.append_node(
            NodeContent::UserMessage {
                content,
                name: None,
            },
            params,
        )
        .await
    }

    pub async fn add_system_message(
        &self,
        content: Vec<ContentBlock>,
    ) -> Result<NodeId, HistoryError> {
        self.append_node(
            NodeContent::SystemMessage { content },
            NodeParams::default(),
        )
        .await
    }

    pub async fn add_response(
        &self,
        response: &Response,
        params: NodeParams,
    ) -> Result<NodeId, HistoryError> {
        let mut node_params = params;
        node_params.usage = Some(response.usage.clone());
        if node_params.model.is_none() {
            node_params.model = response.model.clone();
        }

        self.append_node(
            NodeContent::AssistantMessage {
                content: response.content.clone(),
                stop_reason: Some(response.stop_reason.clone()),
                variant_index: None,
            },
            node_params,
        )
        .await
    }

    pub async fn add_variant(
        &self,
        parent_id: &NodeId,
        response: &Response,
        variant_index: u32,
        params: NodeParams,
    ) -> Result<NodeId, HistoryError> {
        let mut node_params = params;
        node_params.usage = Some(response.usage.clone());
        if node_params.model.is_none() {
            node_params.model = response.model.clone();
        }

        let content = NodeContent::AssistantMessage {
            content: response.content.clone(),
            stop_reason: Some(response.stop_reason.clone()),
            variant_index: Some(variant_index),
        };

        // For variants, we override the parent to be the specified node
        let node = {
            let mut tree = self.tree.lock();
            let branch_id = tree.active_branch().clone();
            let seq = tree.next_seq(&branch_id);
            Node {
                id: NodeId::new(),
                conversation_id: self.conversation.lock().id.clone(),
                branch_id,
                parent_id: Some(parent_id.clone()),
                sequence: seq,
                created_at: now_micros(),
                created_by: node_params.created_by,
                model: node_params.model,
                provider: node_params.provider,
                content,
                usage: node_params.usage,
                version: 0,
                is_final: true,
                streaming: None,
                deleted: false,
                metadata: node_params.metadata,
            }
        };

        self.storage.append_nodes(std::slice::from_ref(&node)).await?;

        self.tree.lock().register(&node)?;
        if let Some(crdt) = self.crdt.lock().as_mut() {
            crdt.record_node(&node);
        }

        Ok(node.id)
    }

    pub async fn add_tool_result(
        &self,
        tool_use_id: &str,
        content: Vec<ContentBlock>,
        is_error: bool,
    ) -> Result<NodeId, HistoryError> {
        self.append_node(
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

    pub async fn spawn_agent(
        &self,
        name: &str,
        agent_type: AgentType,
    ) -> Result<(NodeId, BranchId), HistoryError> {
        let sub_branch_id = BranchId::new();

        let spawn_id = self
            .append_node(
                NodeContent::AgentSpawn {
                    agent_id: uuid::Uuid::now_v7().to_string(),
                    agent_name: name.to_string(),
                    sub_branch_id: sub_branch_id.clone(),
                    agent_type,
                    framework: None,
                    a2a_endpoint: None,
                    skill_id: None,
                    is_async: false,
                },
                NodeParams::default(),
            )
            .await?;

        // Fork a sub-branch using the same ID stored in the AgentSpawn content
        {
            let mut tree = self.tree.lock();
            tree.fork_with_id(&spawn_id, sub_branch_id.clone(), Some(format!("agent/{name}")))?;
        }

        // Persist the branch
        let branch_meta = self.tree.lock().branches()[&sub_branch_id].clone();
        self.storage.save_branch(&branch_meta).await?;

        Ok((spawn_id, sub_branch_id))
    }

    // -----------------------------------------------------------------------
    // Message projection
    // -----------------------------------------------------------------------

    pub async fn to_messages(&self) -> Result<Vec<Message>, HistoryError> {
        let ids = {
            let tree = self.tree.lock();
            let branch = tree.active_branch().clone();
            tree.linearize_ids(&branch)?
        };

        let nodes = self.storage.get_nodes(&ids).await?;
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

    pub async fn to_messages_with_system(
        &self,
    ) -> Result<(Option<String>, Vec<Message>), HistoryError> {
        let mut messages = self.to_messages().await?;

        let system = messages
            .iter()
            .position(|m| m.role == Role::System)
            .map(|idx| {
                let msg = messages.remove(idx);
                msg.content
                    .iter()
                    .filter_map(|b| b.as_text().map(ToString::to_string))
                    .collect::<Vec<_>>()
                    .join("\n")
            });

        Ok((system, messages))
    }

    pub async fn import_messages(
        &self,
        messages: &[Message],
    ) -> Result<Vec<NodeId>, HistoryError> {
        let mut ids = Vec::new();
        for msg in messages {
            let content = match msg.role {
                Role::System => NodeContent::SystemMessage {
                    content: msg.content.clone(),
                },
                Role::User => NodeContent::UserMessage {
                    content: msg.content.clone(),
                    name: msg.name.clone(),
                },
                Role::Assistant => NodeContent::AssistantMessage {
                    content: msg.content.clone(),
                    stop_reason: None,
                    variant_index: None,
                },
                Role::Tool => {
                    // Extract tool_use_id from ToolResult blocks
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
                Role::Other(_) => NodeContent::UserMessage {
                    content: msg.content.clone(),
                    name: msg.name.clone(),
                },
            };
            let id = self.append_node(content, NodeParams::default()).await?;
            ids.push(id);
        }
        Ok(ids)
    }

    // -----------------------------------------------------------------------
    // Branching
    // -----------------------------------------------------------------------

    pub async fn fork(
        &self,
        from: &NodeId,
        name: Option<String>,
    ) -> Result<BranchId, HistoryError> {
        let branch_id = self.tree.lock().fork(from, name)?;

        let branch_meta = self.tree.lock().branches()[&branch_id].clone();
        self.storage.save_branch(&branch_meta).await?;

        Ok(branch_id)
    }

    pub async fn rewind(&self, to: &NodeId) -> Result<Vec<NodeId>, HistoryError> {
        self.tree.lock().rewind(to)
    }

    pub async fn variants(&self, parent: &NodeId) -> Vec<NodeHeader> {
        self.tree
            .lock()
            .variants(parent)
            .into_iter()
            .cloned()
            .collect()
    }

    // -----------------------------------------------------------------------
    // Streaming
    // -----------------------------------------------------------------------

    pub async fn start_streaming(
        &self,
        content: NodeContent,
        params: NodeParams,
    ) -> Result<NodeId, HistoryError> {
        let node = {
            let mut tree = self.tree.lock();
            let branch_id = tree.active_branch().clone();
            let parent_id = tree.branch_tip(&branch_id).cloned();
            let seq = tree.next_seq(&branch_id);
            Node {
                id: NodeId::new(),
                conversation_id: self.conversation.lock().id.clone(),
                branch_id,
                parent_id,
                sequence: seq,
                created_at: now_micros(),
                created_by: params.created_by,
                model: params.model,
                provider: params.provider,
                content,
                usage: params.usage,
                version: 0,
                is_final: false,
                streaming: Some(StreamingState {
                    started_at: now_micros(),
                    tokens_so_far: 0,
                    last_chunk_at: now_micros(),
                    status: StreamStatus::Active,
                }),
                deleted: false,
                metadata: params.metadata,
            }
        };

        self.storage.append_nodes(std::slice::from_ref(&node)).await?;
        self.tree.lock().register(&node)?;

        Ok(node.id)
    }

    pub async fn update_streaming(
        &self,
        node_id: &NodeId,
        state: StreamingState,
    ) -> Result<(), HistoryError> {
        let current_version = self
            .storage
            .get_node(node_id)
            .await?
            .ok_or_else(|| HistoryError::NodeNotFound(node_id.clone()))?
            .version;

        self.storage
            .update_node(
                node_id,
                &NodePatch {
                    streaming: Some(Some(state)),
                    ..Default::default()
                },
                current_version,
            )
            .await
    }

    pub async fn finalize_node(
        &self,
        node_id: &NodeId,
        final_content: NodeContent,
        usage: Option<Usage>,
    ) -> Result<(), HistoryError> {
        let current_version = self
            .storage
            .get_node(node_id)
            .await?
            .ok_or_else(|| HistoryError::NodeNotFound(node_id.clone()))?
            .version;

        self.storage
            .update_node(
                node_id,
                &NodePatch {
                    content: Some(final_content),
                    is_final: Some(true),
                    streaming: Some(None),
                    usage,
                    ..Default::default()
                },
                current_version,
            )
            .await?;

        let has_crdt = self.crdt.lock().is_some();
        if has_crdt
            && let Some(node) = self.storage.get_node(node_id).await?
            && let Some(crdt) = self.crdt.lock().as_mut()
        {
            crdt.record_node(&node);
        }

        Ok(())
    }

    pub async fn cancel_streaming(&self, node_id: &NodeId) -> Result<(), HistoryError> {
        let node = self
            .storage
            .get_node(node_id)
            .await?
            .ok_or_else(|| HistoryError::NodeNotFound(node_id.clone()))?;

        let mut state = node
            .streaming
            .unwrap_or(StreamingState {
                started_at: now_micros(),
                tokens_so_far: 0,
                last_chunk_at: now_micros(),
                status: StreamStatus::Active,
            });
        state.status = StreamStatus::Cancelled;

        self.storage
            .update_node(
                node_id,
                &NodePatch {
                    streaming: Some(Some(state)),
                    ..Default::default()
                },
                node.version,
            )
            .await
    }

    pub async fn cleanup_stale_streams(
        &self,
        max_age_micros: i64,
    ) -> Result<Vec<NodeId>, HistoryError> {
        let now = now_micros();
        let conv_id = self.conversation.lock().id.clone();

        let nodes = self
            .storage
            .list_nodes(&storage::ListNodesParams {
                conversation_id: conv_id,
                branch_id: None,
                after_sequence: None,
                limit: None,
                include_deleted: false,
                content_types: None,
                time_range: None,
            })
            .await?;

        let mut cleaned = Vec::new();
        for node in nodes {
            if !node.is_final
                && let Some(streaming) = &node.streaming
                && now - streaming.last_chunk_at > max_age_micros
            {
                let mut state = streaming.clone();
                state.status = StreamStatus::Cancelled;
                self.storage
                    .update_node(
                        &node.id,
                        &NodePatch {
                            streaming: Some(Some(state)),
                            ..Default::default()
                        },
                        node.version,
                    )
                    .await?;
                cleaned.push(node.id);
            }
        }

        Ok(cleaned)
    }

    // -----------------------------------------------------------------------
    // Reactions
    // -----------------------------------------------------------------------

    pub async fn add_reaction(
        &self,
        node_id: &NodeId,
        user_id: UserId,
        reaction_type: ReactionType,
        comment: Option<String>,
    ) -> Result<(), HistoryError> {
        let reaction = Reaction {
            node_id: node_id.clone(),
            user_id,
            reaction_type,
            created_at: now_micros(),
            comment,
        };
        self.storage.add_reaction(&reaction).await
    }

    pub async fn remove_reaction(
        &self,
        node_id: &NodeId,
        user_id: &UserId,
        reaction_type: &ReactionType,
    ) -> Result<(), HistoryError> {
        self.storage
            .remove_reaction(node_id, user_id, reaction_type)
            .await
    }

    pub async fn reactions(&self, node_id: &NodeId) -> Result<Vec<Reaction>, HistoryError> {
        self.storage.list_reactions(node_id).await
    }

    // -----------------------------------------------------------------------
    // Context
    // -----------------------------------------------------------------------

    pub async fn build_context(
        &self,
        strategy: &ContextStrategy,
        memory_sources: &[Arc<dyn MemorySource>],
        summarizer: Option<Arc<dyn Summarizer>>,
    ) -> Result<ContextResult, HistoryError> {
        let ids = {
            let tree = self.tree.lock();
            let branch = tree.active_branch().clone();
            tree.linearize_ids(&branch)?
        };

        let nodes = self.storage.get_nodes(&ids).await?;
        let conversation = self.conversation.lock().clone();

        let mut mgr = ContextManager::new(strategy.clone());
        for source in memory_sources {
            mgr = mgr.with_memory_source(source.clone());
        }
        if let Some(s) = summarizer {
            mgr = mgr.with_summarizer(s);
        }

        let tree = self.tree.lock().clone();
        mgr.build_context(&tree, &nodes, &conversation).await
    }

    // -----------------------------------------------------------------------
    // Sources
    // -----------------------------------------------------------------------

    pub async fn add_source(
        &self,
        name: &str,
        data: &[u8],
        mime_type: &str,
        source_type: SourceType,
        extractor: Option<&dyn DocumentExtractor>,
        chunk_strategy: Option<&ChunkStrategy>,
    ) -> Result<SourceId, HistoryError> {
        let source_id = SourceId::new();
        let conv_id = self.conversation.lock().id.clone();
        let now = now_micros();

        // Write raw file to VFS
        let raw_path = format!("sources/{}/raw", source_id);
        {
            let vfs = self.vfs();
            vfs.write(&raw_path, data, mime_type).await?;
        }

        let mut source = Source {
            id: source_id.clone(),
            conversation_id: conv_id.clone(),
            name: name.to_string(),
            source_type: source_type.clone(),
            mime_type: mime_type.to_string(),
            size_bytes: data.len() as u64,
            raw_path: raw_path.clone(),
            extracted_path: None,
            status: SourceStatus::Pending,
            chunk_count: 0,
            created_at: now,
            updated_at: now,
            metadata: std::collections::HashMap::new(),
        };

        // Extract text
        let extracted = if let Some(ext) = extractor {
            source.status = SourceStatus::Extracting;
            match ext.extract(data, mime_type).await {
                Ok(content) => Some(content),
                Err(e) => {
                    source.status = SourceStatus::Failed {
                        error: e.to_string(),
                    };
                    self.storage.save_source(&source).await?;
                    return Err(e);
                }
            }
        } else {
            // Default: use PlainTextExtractor for text-like types
            let plain = PlainTextExtractor;
            if plain.supported_types().contains(&source_type) {
                plain.extract(data, mime_type).await.ok()
            } else {
                None
            }
        };

        // Write extracted text and chunk
        if let Some(content) = extracted {
            let extracted_path = format!("sources/{}/extracted.txt", source_id);
            {
                let vfs = self.vfs();
                vfs.write(&extracted_path, content.text.as_bytes(), "text/plain")
                    .await?;
            }
            source.extracted_path = Some(extracted_path);
            source.metadata.extend(content.metadata);

            // Chunk
            let strategy = chunk_strategy.cloned().unwrap_or(ChunkStrategy::Paragraph {
                max_tokens: 512,
            });
            let raw_chunks = chunk_text(&content.text, &strategy);
            let chunks: Vec<SourceChunk> = raw_chunks
                .into_iter()
                .enumerate()
                .map(|(i, (text, location))| {
                    let token_estimate = crate::types::estimate_tokens(&text) as u64;
                    SourceChunk {
                        source_id: source_id.clone(),
                        chunk_index: i as u32,
                        text,
                        location,
                        token_estimate,
                    }
                })
                .collect();

            source.chunk_count = chunks.len() as u32;
            source.status = SourceStatus::Ready;
            source.updated_at = now_micros();

            self.storage.save_source(&source).await?;
            self.storage.save_chunks(&chunks).await?;
        } else {
            source.status = SourceStatus::Ready;
            source.updated_at = now_micros();
            self.storage.save_source(&source).await?;
        }

        // Add to workspace
        self.update_conversation(|conv| {
            conv.workspace.sources.push(source_id.clone());
        });
        let conv_snapshot = self.conversation.lock().clone();
        self.storage.save_conversation(&conv_snapshot).await?;

        // Add SourceRef node to conversation
        self.append_node(
            NodeContent::SourceRef {
                source_id: source_id.clone(),
                source_name: name.to_string(),
                mime_type: mime_type.to_string(),
                summary: None,
            },
            NodeParams::default(),
        )
        .await?;

        Ok(source_id)
    }

    pub async fn remove_source(&self, source_id: &SourceId) -> Result<(), HistoryError> {
        let source = self
            .storage
            .get_source(source_id)
            .await?
            .ok_or_else(|| HistoryError::SourceNotFound(source_id.clone()))?;

        // Delete VFS files
        {
            let vfs = self.vfs();
            let _ = vfs.delete(&source.raw_path).await;
            if let Some(extracted) = &source.extracted_path {
                let _ = vfs.delete(extracted).await;
            }
        }

        // Delete from storage
        self.storage.delete_source(source_id).await?;

        // Remove from workspace
        self.update_conversation(|conv| {
            conv.workspace.sources.retain(|id| id != source_id);
        });
        let conv_snapshot = self.conversation.lock().clone();
        self.storage.save_conversation(&conv_snapshot).await?;

        Ok(())
    }

    pub async fn list_sources(&self) -> Result<Vec<Source>, HistoryError> {
        let conv_id = self.conversation.lock().id.clone();
        self.storage.list_sources(&conv_id).await
    }

    pub async fn get_source(&self, source_id: &SourceId) -> Result<Option<Source>, HistoryError> {
        self.storage.get_source(source_id).await
    }

    pub async fn build_context_with_sources(
        &self,
        strategy: &ContextStrategy,
        memory_sources: &[Arc<dyn MemorySource>],
        summarizer: Option<Arc<dyn Summarizer>>,
    ) -> Result<ContextResult, HistoryError>
    where
        S: 'static,
    {
        let ids = {
            let tree = self.tree.lock();
            let branch = tree.active_branch().clone();
            tree.linearize_ids(&branch)?
        };

        let nodes = self.storage.get_nodes(&ids).await?;
        let conversation = self.conversation.lock().clone();

        let mut mgr = ContextManager::new(strategy.clone());

        // Add user-provided memory sources
        for source in memory_sources {
            mgr = mgr.with_memory_source(source.clone());
        }

        // Add source memory if there are sources in the workspace
        if !conversation.workspace.sources.is_empty() {
            // We need the storage as Arc<dyn HistoryStorage>
            // This is only possible when S: 'static (which it always is in practice)
            // We can't easily get an Arc to self.storage without changing the struct
            // Instead, we search chunks directly and inject them
            let conv_id = conversation.id.clone();
            let query = {
                let last = nodes.iter().rev().find(|n| {
                    matches!(n.content, NodeContent::UserMessage { .. })
                });
                last.and_then(|n| match &n.content {
                    NodeContent::UserMessage { content, .. } => {
                        content.iter().find_map(|b| b.as_text().map(String::from))
                    }
                    _ => None,
                })
            };

            if let Some(q) = query {
                let chunks = self.storage.search_chunks(&conv_id, &q, 5).await?;
                if !chunks.is_empty() {
                    let memory_items: Vec<context::MemoryItem> = chunks
                        .into_iter()
                        .map(|c| context::MemoryItem {
                            content: c.text,
                            source: format!("source:{}", c.source_id),
                            relevance_score: None,
                            metadata: [
                                ("chunk_index".into(), serde_json::json!(c.chunk_index)),
                                ("source_id".into(), serde_json::json!(c.source_id.as_str())),
                            ]
                            .into(),
                        })
                        .collect();

                    // Wrap in a simple in-memory source
                    let static_source = Arc::new(StaticMemorySource {
                        items: Mutex::new(memory_items),
                    });
                    mgr = mgr.with_memory_source(static_source);
                }
            }
        }

        if let Some(s) = summarizer {
            mgr = mgr.with_summarizer(s);
        }

        let tree = self.tree.lock().clone();
        mgr.build_context(&tree, &nodes, &conversation).await
    }

    // -----------------------------------------------------------------------
    // CRDT
    // -----------------------------------------------------------------------

    pub fn enable_crdt(&self) {
        let mut crdt = self.crdt.lock();
        if crdt.is_none() {
            *crdt = Some(CrdtDoc::new());
        }
    }

    pub fn crdt_delta(&self) -> Option<Vec<u8>> {
        self.crdt.lock().as_ref().map(CrdtDoc::full_state)
    }

    pub fn apply_crdt_delta(&self, delta: &[u8]) -> Result<(), HistoryError> {
        let mut crdt = self.crdt.lock();
        match crdt.as_mut() {
            Some(doc) => doc.merge_delta(delta),
            None => Err(HistoryError::Crdt(
                "CRDT not enabled; call enable_crdt() first".into(),
            )),
        }
    }

    // -----------------------------------------------------------------------
    // Internal: split-phase append
    // -----------------------------------------------------------------------

    async fn append_node(
        &self,
        content: NodeContent,
        params: NodeParams,
    ) -> Result<NodeId, HistoryError> {
        // Phase 1: short lock to compute node
        let node = {
            let mut tree = self.tree.lock();
            let branch_id = tree.active_branch().clone();
            let parent_id = tree.branch_tip(&branch_id).cloned();
            let seq = tree.next_seq(&branch_id);
            Node {
                id: NodeId::new(),
                conversation_id: self.conversation.lock().id.clone(),
                branch_id,
                parent_id,
                sequence: seq,
                created_at: now_micros(),
                created_by: params.created_by,
                model: params.model,
                provider: params.provider,
                content,
                usage: params.usage,
                version: 0,
                is_final: true,
                streaming: None,
                deleted: false,
                metadata: params.metadata,
            }
        };

        // Phase 2: async storage (no lock held)
        self.storage.append_nodes(std::slice::from_ref(&node)).await?;

        // Phase 3: short lock to register
        self.tree.lock().register(&node)?;
        if let Some(crdt) = self.crdt.lock().as_mut() {
            crdt.record_node(&node);
        }

        Ok(node.id)
    }
}

// Helper: a MemorySource that returns pre-computed items
struct StaticMemorySource {
    items: Mutex<Vec<context::MemoryItem>>,
}

#[async_trait::async_trait]
impl context::MemorySource for StaticMemorySource {
    async fn retrieve(
        &self,
        _query: &str,
        _conversation_id: &ConversationId,
        _limit: u32,
    ) -> Result<Vec<context::MemoryItem>, HistoryError> {
        Ok(self.items.lock().clone())
    }

    async fn store(
        &self,
        _conversation_id: &ConversationId,
        _item: &context::MemoryItem,
    ) -> Result<(), HistoryError> {
        Ok(())
    }

    fn name(&self) -> &str {
        "static_sources"
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use super::storage::InMemoryStorage;

    #[tokio::test]
    async fn full_lifecycle() {
        let storage = InMemoryStorage::new();
        let conv = Conversation::new("test");
        let history = History::new(storage, conv);

        // Add system message
        let _sys_id = history
            .add_system_message(vec![ContentBlock::text("You are helpful.")])
            .await
            .unwrap();

        // Add user message
        let _user_id = history
            .add_user_message(
                vec![ContentBlock::text("Hello!")],
                NodeParams::default(),
            )
            .await
            .unwrap();

        // Add response
        let response = Response {
            content: vec![ContentBlock::text("Hi there!")],
            usage: Usage::default(),
            stop_reason: crate::types::StopReason::EndTurn,
            model: Some("test-model".into()),
            id: None,
            container: None,
            logprobs: None,
            grounding_metadata: None,
            warnings: vec![],
        };
        let _resp_id = history
            .add_response(&response, NodeParams::default())
            .await
            .unwrap();

        // Project to messages
        let messages = history.to_messages().await.unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages[1].role, Role::User);
        assert_eq!(messages[2].role, Role::Assistant);

        // With system extraction
        let (system, msgs) = history.to_messages_with_system().await.unwrap();
        assert!(system.is_some());
        assert_eq!(msgs.len(), 2);
    }

    #[tokio::test]
    async fn variant_comparison() {
        let storage = InMemoryStorage::new();
        let conv = Conversation::new("test");
        let history = History::new(storage, conv);

        let user_id = history
            .add_user_message(vec![ContentBlock::text("Compare these")], NodeParams::default())
            .await
            .unwrap();

        let resp1 = Response {
            content: vec![ContentBlock::text("Response A")],
            usage: Usage::default(),
            stop_reason: crate::types::StopReason::EndTurn,
            model: Some("model-a".into()),
            id: None,
            container: None,
            logprobs: None,
            grounding_metadata: None,
            warnings: vec![],
        };
        let resp2 = Response {
            content: vec![ContentBlock::text("Response B")],
            model: Some("model-b".into()),
            ..resp1.clone()
        };

        let _v0 = history
            .add_response(&resp1, NodeParams::default())
            .await
            .unwrap();
        let _v1 = history
            .add_variant(&user_id, &resp2, 1, NodeParams::default())
            .await
            .unwrap();

        let variants = history.variants(&user_id).await;
        assert_eq!(variants.len(), 2);
    }

    #[tokio::test]
    async fn agent_spawn_and_subtree() {
        let storage = InMemoryStorage::new();
        let conv = Conversation::new("test");
        let history = History::new(storage, conv);

        let _ = history
            .add_user_message(vec![ContentBlock::text("Search for X")], NodeParams::default())
            .await
            .unwrap();

        let (spawn_id, agent_branch) = history
            .spawn_agent("search", AgentType::Mcp)
            .await
            .unwrap();

        // Checkout agent branch and add work
        history.checkout(&agent_branch).unwrap();
        let _ = history
            .add_user_message(
                vec![ContentBlock::text("Agent searching...")],
                NodeParams::default(),
            )
            .await
            .unwrap();

        // Verify agent subtree
        let tree = history.tree.lock().clone();
        let subtree = tree.agent_subtree(&spawn_id).unwrap();
        assert!(subtree.len() >= 2); // spawn + agent work
    }

    #[tokio::test]
    async fn fork_and_checkout() {
        let storage = InMemoryStorage::new();
        let conv = Conversation::new("test");
        let history = History::new(storage, conv);

        let n0 = history
            .add_user_message(vec![ContentBlock::text("Q1")], NodeParams::default())
            .await
            .unwrap();

        let resp = Response {
            content: vec![ContentBlock::text("A1")],
            usage: Usage::default(),
            stop_reason: crate::types::StopReason::EndTurn,
            model: None,
            id: None,
            container: None,
            logprobs: None,
            grounding_metadata: None,
            warnings: vec![],
        };
        let _ = history.add_response(&resp, NodeParams::default()).await.unwrap();

        // Fork from first message
        let fork_branch = history.fork(&n0, Some("alt".into())).await.unwrap();
        history.checkout(&fork_branch).unwrap();

        let _ = history
            .add_user_message(vec![ContentBlock::text("Different path")], NodeParams::default())
            .await
            .unwrap();

        let messages = history.to_messages().await.unwrap();
        assert_eq!(messages.len(), 2); // n0 + "Different path"
        assert_eq!(messages[1].content[0].as_text().unwrap(), "Different path");
    }

    #[tokio::test]
    async fn streaming_lifecycle() {
        let storage = InMemoryStorage::new();
        let conv = Conversation::new("test");
        let history = History::new(storage, conv);

        let stream_id = history
            .start_streaming(
                NodeContent::AssistantMessage {
                    content: vec![],
                    stop_reason: None,
                    variant_index: None,
                },
                NodeParams::default(),
            )
            .await
            .unwrap();

        // Update streaming state
        history
            .update_streaming(
                &stream_id,
                StreamingState {
                    started_at: now_micros(),
                    tokens_so_far: 50,
                    last_chunk_at: now_micros(),
                    status: StreamStatus::Active,
                },
            )
            .await
            .unwrap();

        // Finalize
        history
            .finalize_node(
                &stream_id,
                NodeContent::AssistantMessage {
                    content: vec![ContentBlock::text("Final response")],
                    stop_reason: Some(crate::types::StopReason::EndTurn),
                    variant_index: None,
                },
                Some(Usage {
                    input_tokens: 10,
                    output_tokens: 20,
                    ..Usage::default()
                }),
            )
            .await
            .unwrap();

        // Verify finalized
        let node = history.storage.get_node(&stream_id).await.unwrap().unwrap();
        assert!(node.is_final);
        assert!(node.streaming.is_none());
        assert!(node.usage.is_some());
    }

    #[tokio::test]
    async fn cancel_streaming() {
        let storage = InMemoryStorage::new();
        let conv = Conversation::new("test");
        let history = History::new(storage, conv);

        let stream_id = history
            .start_streaming(
                NodeContent::AssistantMessage {
                    content: vec![],
                    stop_reason: None,
                    variant_index: None,
                },
                NodeParams::default(),
            )
            .await
            .unwrap();

        history.cancel_streaming(&stream_id).await.unwrap();

        let node = history.storage.get_node(&stream_id).await.unwrap().unwrap();
        assert!(!node.is_final);
        assert!(matches!(
            node.streaming.unwrap().status,
            StreamStatus::Cancelled
        ));
    }

    #[tokio::test]
    async fn stale_stream_cleanup() {
        let storage = InMemoryStorage::new();
        let conv = Conversation::new("test");
        let history = History::new(storage, conv);

        let stream_id = history
            .start_streaming(
                NodeContent::AssistantMessage {
                    content: vec![],
                    stop_reason: None,
                    variant_index: None,
                },
                NodeParams::default(),
            )
            .await
            .unwrap();

        // Set last_chunk_at to the past
        let node = history.storage.get_node(&stream_id).await.unwrap().unwrap();
        history
            .storage
            .update_node(
                &stream_id,
                &NodePatch {
                    streaming: Some(Some(StreamingState {
                        started_at: 0,
                        tokens_so_far: 0,
                        last_chunk_at: 0,
                        status: StreamStatus::Active,
                    })),
                    ..Default::default()
                },
                node.version,
            )
            .await
            .unwrap();

        let cleaned = history.cleanup_stale_streams(1).await.unwrap();
        assert_eq!(cleaned.len(), 1);
        assert_eq!(cleaned[0], stream_id);
    }

    #[tokio::test]
    async fn reactions_crud() {
        let storage = InMemoryStorage::new();
        let conv = Conversation::new("test");
        let history = History::new(storage, conv);

        let node_id = history
            .add_user_message(vec![ContentBlock::text("test")], NodeParams::default())
            .await
            .unwrap();

        let user = UserId::from_string("user1");
        history
            .add_reaction(
                &node_id,
                user.clone(),
                ReactionType::ThumbsUp,
                None,
            )
            .await
            .unwrap();

        let reactions = history.reactions(&node_id).await.unwrap();
        assert_eq!(reactions.len(), 1);

        history
            .remove_reaction(&node_id, &user, &ReactionType::ThumbsUp)
            .await
            .unwrap();

        let reactions = history.reactions(&node_id).await.unwrap();
        assert_eq!(reactions.len(), 0);
    }

    #[tokio::test]
    async fn concurrent_appends() {
        let storage = InMemoryStorage::new();
        let conv = Conversation::new("test");
        let history = Arc::new(History::new(storage, conv));

        let mut handles = Vec::new();
        for i in 0..10 {
            let h = history.clone();
            handles.push(tokio::spawn(async move {
                h.add_user_message(
                    vec![ContentBlock::text(format!("Message {i}"))],
                    NodeParams::default(),
                )
                .await
                .unwrap()
            }));
        }

        let mut ids = Vec::new();
        for handle in handles {
            ids.push(handle.await.unwrap());
        }

        // All 10 should exist
        assert_eq!(ids.len(), 10);

        let messages = history.to_messages().await.unwrap();
        // Due to concurrent branch tips, linearize follows one path
        // but all nodes should be in storage
        assert!(messages.len() >= 1);
    }

    #[tokio::test]
    async fn import_messages() {
        let storage = InMemoryStorage::new();
        let conv = Conversation::new("test");
        let history = History::new(storage, conv);

        let messages = vec![
            Message {
                role: Role::System,
                content: vec![ContentBlock::text("System prompt")],
                name: None,
                cache_control: None,
            },
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

        let ids = history.import_messages(&messages).await.unwrap();
        assert_eq!(ids.len(), 3);

        let exported = history.to_messages().await.unwrap();
        assert_eq!(exported.len(), 3);
        assert_eq!(exported[0].role, Role::System);
        assert_eq!(exported[1].role, Role::User);
        assert_eq!(exported[2].role, Role::Assistant);
    }

    #[tokio::test]
    async fn load_from_storage() {
        let conv = Conversation::new("load test");
        let conv_id = conv.id.clone();

        let shared_storage = InMemoryStorage::new();
        shared_storage.save_conversation(&conv).await.unwrap();

        // Test that load works with existing conversation
        let loaded = History::load(shared_storage, &conv_id).await;
        assert!(loaded.is_ok());
    }

    #[tokio::test]
    async fn add_source_lifecycle() {
        let storage = InMemoryStorage::new();
        let conv = Conversation::new("source test");
        let history = History::new(storage, conv);

        let source_id = history
            .add_source(
                "notes.txt",
                b"The quick brown fox jumps over the lazy dog",
                "text/plain",
                SourceType::Text,
                None,
                None,
            )
            .await
            .unwrap();

        // Source should exist
        let source = history.get_source(&source_id).await.unwrap().unwrap();
        assert_eq!(source.name, "notes.txt");
        assert_eq!(source.status, SourceStatus::Ready);
        assert!(source.chunk_count > 0);

        // Should be in workspace
        let conv = history.conversation();
        assert!(conv.workspace.sources.contains(&source_id));

        // List sources
        let sources = history.list_sources().await.unwrap();
        assert_eq!(sources.len(), 1);

        // VFS should have raw file
        {
            let vfs = history.vfs();
            assert!(vfs.exists(&source.raw_path).await.unwrap());
        }
    }

    #[tokio::test]
    async fn remove_source_cleanup() {
        let storage = InMemoryStorage::new();
        let conv = Conversation::new("source remove test");
        let history = History::new(storage, conv);

        let source_id = history
            .add_source(
                "temp.md",
                b"# Title\n\nSome content",
                "text/markdown",
                SourceType::Markdown,
                None,
                None,
            )
            .await
            .unwrap();

        let source = history.get_source(&source_id).await.unwrap().unwrap();
        let raw_path = source.raw_path.clone();

        history.remove_source(&source_id).await.unwrap();

        // Source should be gone
        assert!(history.get_source(&source_id).await.unwrap().is_none());

        // VFS file should be gone
        {
            let vfs = history.vfs();
            assert!(!vfs.exists(&raw_path).await.unwrap());
        }

        // Workspace should be updated
        let conv = history.conversation();
        assert!(!conv.workspace.sources.contains(&source_id));
    }

    #[tokio::test]
    async fn build_context_with_sources_injects_chunks() {
        let storage = InMemoryStorage::new();
        let conv = Conversation::new("context source test");
        let history = History::new(storage, conv);

        // Add a source
        history
            .add_source(
                "knowledge.txt",
                b"Rust is a systems programming language focused on safety",
                "text/plain",
                SourceType::Text,
                None,
                None,
            )
            .await
            .unwrap();

        // Add a user message containing text that appears in the source
        history
            .add_user_message(
                vec![ContentBlock::text("systems programming language")],
                NodeParams::default(),
            )
            .await
            .unwrap();

        let strategy = ContextStrategy {
            max_tokens: 100_000,
            ..Default::default()
        };

        let result = history
            .build_context_with_sources(&strategy, &[], None)
            .await
            .unwrap();

        // Should have injected memory from source chunks
        assert!(!result.injected_memories.is_empty());
        assert!(result
            .injected_memories
            .iter()
            .any(|m| m.source.starts_with("source:")));
    }

    #[tokio::test]
    async fn list_sources_reflects_workspace() {
        let storage = InMemoryStorage::new();
        let conv = Conversation::new("list test");
        let history = History::new(storage, conv);

        assert!(history.list_sources().await.unwrap().is_empty());

        history
            .add_source("a.txt", b"aaa", "text/plain", SourceType::Text, None, None)
            .await
            .unwrap();
        history
            .add_source("b.txt", b"bbb", "text/plain", SourceType::Text, None, None)
            .await
            .unwrap();

        let sources = history.list_sources().await.unwrap();
        assert_eq!(sources.len(), 2);

        let conv = history.conversation();
        assert_eq!(conv.workspace.sources.len(), 2);
    }
}
