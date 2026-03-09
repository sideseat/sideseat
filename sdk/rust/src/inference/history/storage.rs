use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::Value;

use super::artifact::{ArtifactSet, ArtifactVersion};
use super::canvas::{Canvas, CanvasItem, Viewport};
use super::error::HistoryError;
use super::kanban::{KanbanBoard, KanbanCard, KanbanCardId, KanbanColumn, KanbanColumnId};
use super::source::{Source, SourceChunk};
use super::sync::CrdtDelta;
use super::types::{
    ArtifactSetId, BranchId, BranchMeta, CanvasId, Conversation, ConversationId,
    DatasetEntry, DatasetEntryId, DatasetSplit, EvalScore, KanbanBoardId, Node, NodeContent,
    NodeHeader, NodeId, Project, ProjectId, PromptId, PromptVersion, Reaction, ReactionType,
    SourceId, StreamingState, UserId, UserMemoryEntry, UserMemoryId,
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

#[derive(Debug, Clone, Default)]
pub struct ListConversationsParams {
    pub project_id: Option<ProjectId>,
    pub pagination: ListParams,
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

#[derive(Debug, Clone, Default)]
pub struct ListCanvasItemsParams {
    pub canvas_id: CanvasId,
    pub viewport: Option<Viewport>,
    pub include_deleted: bool,
    pub pagination: ListParams,
}

#[derive(Debug, Clone, Default)]
pub struct NodePatch {
    pub content: Option<NodeContent>,
    pub is_final: Option<bool>,
    pub streaming: Option<Option<StreamingState>>,
    pub usage: Option<Usage>,
    pub metadata: Option<HashMap<String, Value>>,
    pub eval_scores: Option<Vec<EvalScore>>,
}

// ---------------------------------------------------------------------------
// HistoryStorage trait
// ---------------------------------------------------------------------------

#[async_trait]
pub trait HistoryStorage: Send + Sync {
    // Projects
    async fn save_project(&self, project: &Project) -> Result<(), HistoryError>;
    async fn get_project(&self, id: &ProjectId) -> Result<Option<Project>, HistoryError>;
    async fn list_projects(&self, params: &ListParams) -> Result<Vec<Project>, HistoryError>;
    async fn delete_project(&self, id: &ProjectId) -> Result<(), HistoryError>;

    // Conversations
    async fn save_conversation(&self, conv: &Conversation) -> Result<(), HistoryError>;
    async fn get_conversation(
        &self,
        id: &ConversationId,
    ) -> Result<Option<Conversation>, HistoryError>;
    async fn list_conversations(
        &self,
        params: &ListConversationsParams,
    ) -> Result<Vec<Conversation>, HistoryError>;
    async fn delete_conversation(&self, id: &ConversationId) -> Result<(), HistoryError>;

    // Nodes
    async fn append_nodes(&self, nodes: &[Node]) -> Result<(), HistoryError>;
    async fn get_node(&self, id: &NodeId) -> Result<Option<Node>, HistoryError>;
    async fn get_nodes(&self, ids: &[NodeId]) -> Result<Vec<Node>, HistoryError>;
    async fn list_nodes(&self, params: &ListNodesParams) -> Result<Vec<Node>, HistoryError>;
    async fn list_node_headers(
        &self,
        conversation_id: &ConversationId,
    ) -> Result<Vec<NodeHeader>, HistoryError>;
    async fn update_node(
        &self,
        id: &NodeId,
        patch: &NodePatch,
        expected_version: u64,
    ) -> Result<(), HistoryError>;
    async fn soft_delete_node(&self, id: &NodeId) -> Result<(), HistoryError>;
    async fn search_nodes(
        &self,
        conversation_id: &ConversationId,
        query: &str,
        params: &ListParams,
    ) -> Result<Vec<NodeHeader>, HistoryError>;

    // Branches
    async fn save_branch(&self, branch: &BranchMeta) -> Result<(), HistoryError>;
    async fn list_branches(
        &self,
        conversation_id: &ConversationId,
    ) -> Result<Vec<BranchMeta>, HistoryError>;
    async fn delete_branch(&self, id: &BranchId) -> Result<(), HistoryError>;

    // Reactions
    async fn add_reaction(&self, reaction: &Reaction) -> Result<(), HistoryError>;
    async fn remove_reaction(
        &self,
        node_id: &NodeId,
        user_id: &UserId,
        reaction_type: &ReactionType,
    ) -> Result<(), HistoryError>;
    async fn list_reactions(&self, node_id: &NodeId) -> Result<Vec<Reaction>, HistoryError>;

    // Canvas
    async fn save_canvas(&self, canvas: &Canvas) -> Result<(), HistoryError>;
    async fn get_canvas(&self, id: &CanvasId) -> Result<Option<Canvas>, HistoryError>;
    async fn upsert_canvas_item(&self, item: &CanvasItem) -> Result<(), HistoryError>;
    async fn delete_canvas_item(&self, id: &str) -> Result<(), HistoryError>;
    async fn list_canvas_items(
        &self,
        params: &ListCanvasItemsParams,
    ) -> Result<Vec<CanvasItem>, HistoryError>;

    // Artifacts
    async fn save_artifact_set(&self, set: &ArtifactSet) -> Result<(), HistoryError>;
    async fn get_artifact_set(
        &self,
        id: &ArtifactSetId,
    ) -> Result<Option<ArtifactSet>, HistoryError>;
    async fn save_artifact_version(
        &self,
        version: &ArtifactVersion,
    ) -> Result<(), HistoryError>;
    async fn list_artifact_versions(
        &self,
        set_id: &ArtifactSetId,
        params: &ListParams,
    ) -> Result<Vec<ArtifactVersion>, HistoryError>;
    async fn get_artifact_version(
        &self,
        set_id: &ArtifactSetId,
        version: u32,
    ) -> Result<Option<ArtifactVersion>, HistoryError>;

    // Sources
    async fn save_source(&self, source: &Source) -> Result<(), HistoryError>;
    async fn get_source(&self, id: &SourceId) -> Result<Option<Source>, HistoryError>;
    async fn list_sources(
        &self,
        conversation_id: &ConversationId,
    ) -> Result<Vec<Source>, HistoryError>;
    async fn delete_source(&self, id: &SourceId) -> Result<(), HistoryError>;

    // Source chunks
    async fn save_chunks(&self, chunks: &[SourceChunk]) -> Result<(), HistoryError>;
    async fn get_chunks(&self, source_id: &SourceId) -> Result<Vec<SourceChunk>, HistoryError>;
    async fn search_chunks(
        &self,
        conversation_id: &ConversationId,
        query: &str,
        limit: u32,
    ) -> Result<Vec<SourceChunk>, HistoryError>;

    // Kanban
    async fn save_kanban_board(&self, board: &KanbanBoard) -> Result<(), HistoryError>;
    async fn get_kanban_board(
        &self,
        id: &KanbanBoardId,
    ) -> Result<Option<KanbanBoard>, HistoryError>;
    async fn save_kanban_column(&self, column: &KanbanColumn) -> Result<(), HistoryError>;
    async fn list_kanban_columns(
        &self,
        board_id: &KanbanBoardId,
    ) -> Result<Vec<KanbanColumn>, HistoryError>;
    async fn delete_kanban_column(&self, id: &KanbanColumnId) -> Result<(), HistoryError>;
    async fn save_kanban_card(&self, card: &KanbanCard) -> Result<(), HistoryError>;
    async fn get_kanban_card(
        &self,
        id: &KanbanCardId,
    ) -> Result<Option<KanbanCard>, HistoryError>;
    async fn list_kanban_cards(
        &self,
        column_id: &KanbanColumnId,
    ) -> Result<Vec<KanbanCard>, HistoryError>;
    async fn delete_kanban_card(&self, id: &KanbanCardId) -> Result<(), HistoryError>;

    // CRDT delta log (used by StorageSyncBackend for distributed sync)
    /// Persist a CRDT delta and return the storage-assigned global sequence number.
    async fn append_crdt_delta(&self, delta: &CrdtDelta) -> Result<u64, HistoryError>;
    /// Return all deltas for the given conversation where `global_seq > after_seq`.
    async fn fetch_crdt_deltas(
        &self,
        conv_id: &ConversationId,
        after_seq: u64,
    ) -> Result<Vec<CrdtDelta>, HistoryError>;

    // VFS branch index (persisted for stateless/distributed deployments)
    /// Persist the serialized branch index for `branch_id`.
    async fn save_vfs_index(&self, branch_id: &BranchId, data: &[u8]) -> Result<(), HistoryError>;
    /// Return the last persisted branch index for `branch_id`, or `None` if never saved.
    async fn load_vfs_index(&self, branch_id: &BranchId) -> Result<Option<Vec<u8>>, HistoryError>;

    // Prompt versioning
    async fn save_prompt_version(&self, prompt: &PromptVersion) -> Result<(), HistoryError>;
    async fn get_prompt_version(
        &self,
        id: &PromptId,
        version: u32,
    ) -> Result<Option<PromptVersion>, HistoryError>;
    async fn list_prompt_versions(&self, name: &str) -> Result<Vec<PromptVersion>, HistoryError>;
    async fn get_latest_prompt(&self, name: &str) -> Result<Option<PromptVersion>, HistoryError>;

    // Dataset entries
    async fn save_dataset_entry(&self, entry: &DatasetEntry) -> Result<(), HistoryError>;
    async fn list_dataset_entries(
        &self,
        dataset_name: &str,
        split: Option<&DatasetSplit>,
    ) -> Result<Vec<DatasetEntry>, HistoryError>;
    async fn delete_dataset_entry(&self, id: &DatasetEntryId) -> Result<(), HistoryError>;

    // User memory
    async fn save_user_memory(&self, entry: &UserMemoryEntry) -> Result<(), HistoryError>;
    async fn get_user_memory(
        &self,
        id: &UserMemoryId,
    ) -> Result<Option<UserMemoryEntry>, HistoryError>;
    async fn list_user_memories(
        &self,
        user_id: &UserId,
        limit: Option<u32>,
    ) -> Result<Vec<UserMemoryEntry>, HistoryError>;
    async fn delete_user_memory(&self, id: &UserMemoryId) -> Result<(), HistoryError>;
    async fn search_user_memories(
        &self,
        user_id: &UserId,
        query: &str,
        limit: u32,
    ) -> Result<Vec<UserMemoryEntry>, HistoryError>;
}

// ---------------------------------------------------------------------------
// InMemoryStorage
// ---------------------------------------------------------------------------

pub struct InMemoryStorage {
    projects: Mutex<HashMap<ProjectId, Project>>,
    conversations: Mutex<HashMap<ConversationId, Conversation>>,
    nodes: Mutex<HashMap<NodeId, Node>>,
    branches: Mutex<HashMap<BranchId, BranchMeta>>,
    reactions: Mutex<Vec<Reaction>>,
    canvases: Mutex<HashMap<CanvasId, Canvas>>,
    canvas_items: Mutex<HashMap<String, CanvasItem>>,
    artifact_sets: Mutex<HashMap<ArtifactSetId, ArtifactSet>>,
    artifact_versions: Mutex<Vec<ArtifactVersion>>,
    sources: Mutex<HashMap<SourceId, Source>>,
    source_chunks: Mutex<Vec<SourceChunk>>,
    kanban_boards: Mutex<HashMap<KanbanBoardId, KanbanBoard>>,
    kanban_columns: Mutex<HashMap<KanbanColumnId, KanbanColumn>>,
    kanban_cards: Mutex<HashMap<KanbanCardId, KanbanCard>>,
    crdt_deltas: Mutex<Vec<CrdtDelta>>,
    crdt_next_seq: AtomicU64,
    vfs_indexes: Mutex<HashMap<BranchId, Vec<u8>>>,
    prompt_versions: Mutex<Vec<PromptVersion>>,
    dataset_entries: Mutex<Vec<DatasetEntry>>,
    user_memories: Mutex<HashMap<UserMemoryId, UserMemoryEntry>>,
}

impl InMemoryStorage {
    pub fn new() -> Self {
        Self {
            projects: Mutex::new(HashMap::new()),
            conversations: Mutex::new(HashMap::new()),
            nodes: Mutex::new(HashMap::new()),
            branches: Mutex::new(HashMap::new()),
            reactions: Mutex::new(Vec::new()),
            canvases: Mutex::new(HashMap::new()),
            canvas_items: Mutex::new(HashMap::new()),
            artifact_sets: Mutex::new(HashMap::new()),
            artifact_versions: Mutex::new(Vec::new()),
            sources: Mutex::new(HashMap::new()),
            source_chunks: Mutex::new(Vec::new()),
            kanban_boards: Mutex::new(HashMap::new()),
            kanban_columns: Mutex::new(HashMap::new()),
            kanban_cards: Mutex::new(HashMap::new()),
            crdt_deltas: Mutex::new(Vec::new()),
            crdt_next_seq: AtomicU64::new(1),
            vfs_indexes: Mutex::new(HashMap::new()),
            prompt_versions: Mutex::new(Vec::new()),
            dataset_entries: Mutex::new(Vec::new()),
            user_memories: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryStorage {
    fn default() -> Self {
        Self::new()
    }
}

fn apply_pagination<T>(items: Vec<T>, offset: Option<u64>, limit: Option<u32>) -> Vec<T> {
    let start = offset.unwrap_or(0) as usize;
    let items: Vec<T> = items.into_iter().skip(start).collect();
    match limit {
        Some(l) => items.into_iter().take(l as usize).collect(),
        None => items,
    }
}

fn node_matches_text(node: &Node, query: &str) -> bool {
    let lower_query = query.to_lowercase();
    match &node.content {
        NodeContent::UserMessage { content, .. }
        | NodeContent::AssistantMessage { content, .. }
        | NodeContent::SystemMessage { content }
        | NodeContent::ToolResult { content, .. }
        | NodeContent::AgentResult { content, .. } => content.iter().any(|block| {
            if let Some(text) = block.as_text() {
                text.to_lowercase().contains(&lower_query)
            } else {
                false
            }
        }),
        _ => false,
    }
}

#[async_trait]
impl HistoryStorage for InMemoryStorage {
    // Projects

    async fn save_project(&self, project: &Project) -> Result<(), HistoryError> {
        self.projects.lock().insert(project.id.clone(), project.clone());
        Ok(())
    }

    async fn get_project(&self, id: &ProjectId) -> Result<Option<Project>, HistoryError> {
        Ok(self.projects.lock().get(id).cloned())
    }

    async fn list_projects(&self, params: &ListParams) -> Result<Vec<Project>, HistoryError> {
        let mut items: Vec<Project> = self.projects.lock().values().cloned().collect();
        items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(apply_pagination(items, params.offset, params.limit))
    }

    async fn delete_project(&self, id: &ProjectId) -> Result<(), HistoryError> {
        self.projects.lock().remove(id);
        Ok(())
    }

    // Conversations

    async fn save_conversation(&self, conv: &Conversation) -> Result<(), HistoryError> {
        self.conversations.lock().insert(conv.id.clone(), conv.clone());
        Ok(())
    }

    async fn get_conversation(
        &self,
        id: &ConversationId,
    ) -> Result<Option<Conversation>, HistoryError> {
        Ok(self.conversations.lock().get(id).cloned())
    }

    async fn list_conversations(
        &self,
        params: &ListConversationsParams,
    ) -> Result<Vec<Conversation>, HistoryError> {
        let mut items: Vec<Conversation> = self
            .conversations
            .lock()
            .values()
            .filter(|c| match &params.project_id {
                Some(pid) => c.project_id.as_ref() == Some(pid),
                None => true,
            })
            .cloned()
            .collect();
        items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(apply_pagination(
            items,
            params.pagination.offset,
            params.pagination.limit,
        ))
    }

    async fn delete_conversation(&self, id: &ConversationId) -> Result<(), HistoryError> {
        self.conversations.lock().remove(id);

        // Cascade: soft-delete nodes
        for node in self.nodes.lock().values_mut() {
            if node.conversation_id == *id {
                node.deleted = true;
            }
        }

        // Cascade: delete branches
        self.branches
            .lock()
            .retain(|_, b| b.conversation_id != *id);

        // Cascade: delete canvases + items
        let canvas_ids: Vec<CanvasId> = self
            .canvases
            .lock()
            .values()
            .filter(|c| c.conversation_id == *id)
            .map(|c| c.id.clone())
            .collect();

        self.canvases
            .lock()
            .retain(|_, c| c.conversation_id != *id);

        self.canvas_items
            .lock()
            .retain(|_, item| !canvas_ids.contains(&item.canvas_id));

        // Cascade: delete artifact sets + versions
        let set_ids: Vec<ArtifactSetId> = self
            .artifact_sets
            .lock()
            .values()
            .filter(|s| s.conversation_id == *id)
            .map(|s| s.id.clone())
            .collect();

        self.artifact_sets
            .lock()
            .retain(|_, s| s.conversation_id != *id);

        self.artifact_versions
            .lock()
            .retain(|v| !set_ids.contains(&v.artifact_set_id));

        // Cascade: delete sources + chunks
        let source_ids: Vec<SourceId> = self
            .sources
            .lock()
            .values()
            .filter(|s| s.conversation_id == *id)
            .map(|s| s.id.clone())
            .collect();

        self.sources
            .lock()
            .retain(|_, s| s.conversation_id != *id);

        self.source_chunks
            .lock()
            .retain(|c| !source_ids.contains(&c.source_id));

        // Cascade: delete dataset entries
        self.dataset_entries
            .lock()
            .retain(|e| e.conversation_id != *id);

        Ok(())
    }

    // Nodes

    async fn append_nodes(&self, nodes: &[Node]) -> Result<(), HistoryError> {
        let mut store = self.nodes.lock();
        for node in nodes {
            store.insert(node.id.clone(), node.clone());
        }
        Ok(())
    }

    async fn get_node(&self, id: &NodeId) -> Result<Option<Node>, HistoryError> {
        Ok(self.nodes.lock().get(id).cloned())
    }

    async fn get_nodes(&self, ids: &[NodeId]) -> Result<Vec<Node>, HistoryError> {
        let store = self.nodes.lock();
        Ok(ids.iter().filter_map(|id| store.get(id).cloned()).collect())
    }

    async fn list_nodes(&self, params: &ListNodesParams) -> Result<Vec<Node>, HistoryError> {
        let store = self.nodes.lock();
        let mut items: Vec<Node> = store
            .values()
            .filter(|n| {
                if n.conversation_id != params.conversation_id {
                    return false;
                }
                if !params.include_deleted && n.deleted {
                    return false;
                }
                if let Some(branch) = &params.branch_id
                    && n.branch_id != *branch
                {
                    return false;
                }
                if let Some(after_seq) = params.after_sequence
                    && n.sequence <= after_seq
                {
                    return false;
                }
                if let Some(types) = &params.content_types
                    && !types.contains(&n.content_type().to_string())
                {
                    return false;
                }
                if let Some(range) = &params.time_range
                    && (n.created_at < range.start || n.created_at > range.end)
                {
                    return false;
                }
                true
            })
            .cloned()
            .collect();

        items.sort_by_key(|n| n.sequence);

        if let Some(limit) = params.limit {
            items.truncate(limit as usize);
        }

        Ok(items)
    }

    async fn list_node_headers(
        &self,
        conversation_id: &ConversationId,
    ) -> Result<Vec<NodeHeader>, HistoryError> {
        let store = self.nodes.lock();
        let mut headers: Vec<NodeHeader> = store
            .values()
            .filter(|n| n.conversation_id == *conversation_id)
            .map(NodeHeader::from)
            .collect();
        headers.sort_by_key(|h| h.sequence);
        Ok(headers)
    }

    async fn update_node(
        &self,
        id: &NodeId,
        patch: &NodePatch,
        expected_version: u64,
    ) -> Result<(), HistoryError> {
        let mut store = self.nodes.lock();
        let node = store
            .get_mut(id)
            .ok_or_else(|| HistoryError::NodeNotFound(id.clone()))?;

        if node.version != expected_version {
            return Err(HistoryError::VersionMismatch {
                id: id.to_string(),
                expected: expected_version,
                actual: node.version,
            });
        }

        if let Some(content) = &patch.content {
            node.content = content.clone();
        }
        if let Some(is_final) = patch.is_final {
            node.is_final = is_final;
        }
        if let Some(streaming) = &patch.streaming {
            node.streaming = streaming.clone();
        }
        if let Some(usage) = &patch.usage {
            node.usage = Some(usage.clone());
        }
        if let Some(metadata) = &patch.metadata {
            node.metadata = metadata.clone();
        }
        if let Some(scores) = &patch.eval_scores {
            node.eval_scores.extend(scores.iter().cloned());
        }

        node.version += 1;
        Ok(())
    }

    async fn soft_delete_node(&self, id: &NodeId) -> Result<(), HistoryError> {
        let mut store = self.nodes.lock();
        let node = store
            .get_mut(id)
            .ok_or_else(|| HistoryError::NodeNotFound(id.clone()))?;
        node.deleted = true;
        Ok(())
    }

    async fn search_nodes(
        &self,
        conversation_id: &ConversationId,
        query: &str,
        params: &ListParams,
    ) -> Result<Vec<NodeHeader>, HistoryError> {
        let store = self.nodes.lock();
        let mut results: Vec<NodeHeader> = store
            .values()
            .filter(|n| {
                n.conversation_id == *conversation_id && !n.deleted && node_matches_text(n, query)
            })
            .map(NodeHeader::from)
            .collect();
        results.sort_by_key(|h| h.sequence);
        Ok(apply_pagination(results, params.offset, params.limit))
    }

    // Branches

    async fn save_branch(&self, branch: &BranchMeta) -> Result<(), HistoryError> {
        self.branches
            .lock()
            .insert(branch.id.clone(), branch.clone());
        Ok(())
    }

    async fn list_branches(
        &self,
        conversation_id: &ConversationId,
    ) -> Result<Vec<BranchMeta>, HistoryError> {
        Ok(self
            .branches
            .lock()
            .values()
            .filter(|b| b.conversation_id == *conversation_id)
            .cloned()
            .collect())
    }

    async fn delete_branch(&self, id: &BranchId) -> Result<(), HistoryError> {
        self.branches.lock().remove(id);
        // Cascade: soft-delete nodes on this branch
        for node in self.nodes.lock().values_mut() {
            if node.branch_id == *id {
                node.deleted = true;
            }
        }
        Ok(())
    }

    // Reactions

    async fn add_reaction(&self, reaction: &Reaction) -> Result<(), HistoryError> {
        self.reactions.lock().push(reaction.clone());
        Ok(())
    }

    async fn remove_reaction(
        &self,
        node_id: &NodeId,
        user_id: &UserId,
        reaction_type: &ReactionType,
    ) -> Result<(), HistoryError> {
        self.reactions.lock().retain(|r| {
            !(r.node_id == *node_id
                && r.user_id == *user_id
                && r.reaction_type == *reaction_type)
        });
        Ok(())
    }

    async fn list_reactions(&self, node_id: &NodeId) -> Result<Vec<Reaction>, HistoryError> {
        Ok(self
            .reactions
            .lock()
            .iter()
            .filter(|r| r.node_id == *node_id)
            .cloned()
            .collect())
    }

    // Canvas

    async fn save_canvas(&self, canvas: &Canvas) -> Result<(), HistoryError> {
        self.canvases
            .lock()
            .insert(canvas.id.clone(), canvas.clone());
        Ok(())
    }

    async fn get_canvas(&self, id: &CanvasId) -> Result<Option<Canvas>, HistoryError> {
        Ok(self.canvases.lock().get(id).cloned())
    }

    async fn upsert_canvas_item(&self, item: &CanvasItem) -> Result<(), HistoryError> {
        self.canvas_items
            .lock()
            .insert(item.id.clone(), item.clone());
        Ok(())
    }

    async fn delete_canvas_item(&self, id: &str) -> Result<(), HistoryError> {
        self.canvas_items.lock().remove(id);
        Ok(())
    }

    async fn list_canvas_items(
        &self,
        params: &ListCanvasItemsParams,
    ) -> Result<Vec<CanvasItem>, HistoryError> {
        let store = self.canvas_items.lock();
        let items: Vec<CanvasItem> = store
            .values()
            .filter(|item| {
                if item.canvas_id != params.canvas_id {
                    return false;
                }
                if !params.include_deleted && item.deleted {
                    return false;
                }
                if let Some(vp) = &params.viewport
                    && !vp.contains_item(item)
                {
                    return false;
                }
                true
            })
            .cloned()
            .collect();
        Ok(apply_pagination(
            items,
            params.pagination.offset,
            params.pagination.limit,
        ))
    }

    // Artifacts

    async fn save_artifact_set(&self, set: &ArtifactSet) -> Result<(), HistoryError> {
        self.artifact_sets
            .lock()
            .insert(set.id.clone(), set.clone());
        Ok(())
    }

    async fn get_artifact_set(
        &self,
        id: &ArtifactSetId,
    ) -> Result<Option<ArtifactSet>, HistoryError> {
        Ok(self.artifact_sets.lock().get(id).cloned())
    }

    async fn save_artifact_version(
        &self,
        version: &ArtifactVersion,
    ) -> Result<(), HistoryError> {
        self.artifact_versions.lock().push(version.clone());
        Ok(())
    }

    async fn list_artifact_versions(
        &self,
        set_id: &ArtifactSetId,
        params: &ListParams,
    ) -> Result<Vec<ArtifactVersion>, HistoryError> {
        let store = self.artifact_versions.lock();
        let mut items: Vec<ArtifactVersion> = store
            .iter()
            .filter(|v| v.artifact_set_id == *set_id)
            .cloned()
            .collect();
        items.sort_by_key(|v| v.version);
        Ok(apply_pagination(items, params.offset, params.limit))
    }

    async fn get_artifact_version(
        &self,
        set_id: &ArtifactSetId,
        version: u32,
    ) -> Result<Option<ArtifactVersion>, HistoryError> {
        Ok(self
            .artifact_versions
            .lock()
            .iter()
            .find(|v| v.artifact_set_id == *set_id && v.version == version)
            .cloned())
    }

    // Sources

    async fn save_source(&self, source: &Source) -> Result<(), HistoryError> {
        self.sources
            .lock()
            .insert(source.id.clone(), source.clone());
        Ok(())
    }

    async fn get_source(&self, id: &SourceId) -> Result<Option<Source>, HistoryError> {
        Ok(self.sources.lock().get(id).cloned())
    }

    async fn list_sources(
        &self,
        conversation_id: &ConversationId,
    ) -> Result<Vec<Source>, HistoryError> {
        let mut items: Vec<Source> = self
            .sources
            .lock()
            .values()
            .filter(|s| s.conversation_id == *conversation_id)
            .cloned()
            .collect();
        items.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(items)
    }

    async fn delete_source(&self, id: &SourceId) -> Result<(), HistoryError> {
        self.sources.lock().remove(id);
        self.source_chunks
            .lock()
            .retain(|c| c.source_id != *id);
        Ok(())
    }

    // Source chunks

    async fn save_chunks(&self, chunks: &[SourceChunk]) -> Result<(), HistoryError> {
        self.source_chunks.lock().extend(chunks.iter().cloned());
        Ok(())
    }

    async fn get_chunks(&self, source_id: &SourceId) -> Result<Vec<SourceChunk>, HistoryError> {
        let store = self.source_chunks.lock();
        let mut items: Vec<SourceChunk> = store
            .iter()
            .filter(|c| c.source_id == *source_id)
            .cloned()
            .collect();
        items.sort_by_key(|c| c.chunk_index);
        Ok(items)
    }

    async fn search_chunks(
        &self,
        conversation_id: &ConversationId,
        query: &str,
        limit: u32,
    ) -> Result<Vec<SourceChunk>, HistoryError> {
        let sources = self.sources.lock();
        let source_ids: Vec<SourceId> = sources
            .values()
            .filter(|s| s.conversation_id == *conversation_id)
            .map(|s| s.id.clone())
            .collect();
        drop(sources);

        let lower_query = query.to_lowercase();
        let store = self.source_chunks.lock();
        let results: Vec<SourceChunk> = store
            .iter()
            .filter(|c| {
                source_ids.contains(&c.source_id)
                    && c.text.to_lowercase().contains(&lower_query)
            })
            .take(limit as usize)
            .cloned()
            .collect();
        Ok(results)
    }

    // Prompt versioning

    // Kanban

    async fn save_kanban_board(&self, board: &KanbanBoard) -> Result<(), HistoryError> {
        self.kanban_boards.lock().insert(board.id.clone(), board.clone());
        Ok(())
    }

    async fn get_kanban_board(
        &self,
        id: &KanbanBoardId,
    ) -> Result<Option<KanbanBoard>, HistoryError> {
        Ok(self.kanban_boards.lock().get(id).cloned())
    }

    async fn save_kanban_column(&self, column: &KanbanColumn) -> Result<(), HistoryError> {
        self.kanban_columns.lock().insert(column.id.clone(), column.clone());
        Ok(())
    }

    async fn list_kanban_columns(
        &self,
        board_id: &KanbanBoardId,
    ) -> Result<Vec<KanbanColumn>, HistoryError> {
        let mut cols: Vec<KanbanColumn> = self
            .kanban_columns
            .lock()
            .values()
            .filter(|c| &c.board_id == board_id)
            .cloned()
            .collect();
        cols.sort_by_key(|c| c.position);
        Ok(cols)
    }

    async fn delete_kanban_column(&self, id: &KanbanColumnId) -> Result<(), HistoryError> {
        self.kanban_columns.lock().remove(id);
        // Cascade: delete cards in this column
        self.kanban_cards.lock().retain(|_, c| &c.column_id != id);
        Ok(())
    }

    async fn save_kanban_card(&self, card: &KanbanCard) -> Result<(), HistoryError> {
        self.kanban_cards.lock().insert(card.id.clone(), card.clone());
        Ok(())
    }

    async fn get_kanban_card(
        &self,
        id: &KanbanCardId,
    ) -> Result<Option<KanbanCard>, HistoryError> {
        Ok(self.kanban_cards.lock().get(id).cloned())
    }

    async fn list_kanban_cards(
        &self,
        column_id: &KanbanColumnId,
    ) -> Result<Vec<KanbanCard>, HistoryError> {
        let mut cards: Vec<KanbanCard> = self
            .kanban_cards
            .lock()
            .values()
            .filter(|c| &c.column_id == column_id && !c.deleted)
            .cloned()
            .collect();
        cards.sort_by_key(|c| c.position);
        Ok(cards)
    }

    async fn delete_kanban_card(&self, id: &KanbanCardId) -> Result<(), HistoryError> {
        self.kanban_cards.lock().remove(id);
        Ok(())
    }

    // CRDT delta log

    async fn append_crdt_delta(&self, delta: &CrdtDelta) -> Result<u64, HistoryError> {
        let seq = self.crdt_next_seq.fetch_add(1, Ordering::SeqCst);
        let mut stored = delta.clone();
        stored.global_seq = seq;
        self.crdt_deltas.lock().push(stored);
        Ok(seq)
    }

    async fn fetch_crdt_deltas(
        &self,
        conv_id: &ConversationId,
        after_seq: u64,
    ) -> Result<Vec<CrdtDelta>, HistoryError> {
        Ok(self
            .crdt_deltas
            .lock()
            .iter()
            .filter(|d| d.conversation_id == *conv_id && d.global_seq > after_seq)
            .cloned()
            .collect())
    }

    async fn save_vfs_index(&self, branch_id: &BranchId, data: &[u8]) -> Result<(), HistoryError> {
        self.vfs_indexes.lock().insert(branch_id.clone(), data.to_vec());
        Ok(())
    }

    async fn load_vfs_index(&self, branch_id: &BranchId) -> Result<Option<Vec<u8>>, HistoryError> {
        Ok(self.vfs_indexes.lock().get(branch_id).cloned())
    }

    async fn save_prompt_version(&self, prompt: &PromptVersion) -> Result<(), HistoryError> {
        self.prompt_versions.lock().push(prompt.clone());
        Ok(())
    }

    async fn get_prompt_version(
        &self,
        id: &PromptId,
        version: u32,
    ) -> Result<Option<PromptVersion>, HistoryError> {
        Ok(self
            .prompt_versions
            .lock()
            .iter()
            .find(|p| p.id == *id && p.version == version)
            .cloned())
    }

    async fn list_prompt_versions(&self, name: &str) -> Result<Vec<PromptVersion>, HistoryError> {
        let mut versions: Vec<PromptVersion> = self
            .prompt_versions
            .lock()
            .iter()
            .filter(|p| p.name == name)
            .cloned()
            .collect();
        versions.sort_by_key(|p| p.version);
        Ok(versions)
    }

    async fn get_latest_prompt(&self, name: &str) -> Result<Option<PromptVersion>, HistoryError> {
        Ok(self
            .prompt_versions
            .lock()
            .iter()
            .filter(|p| p.name == name)
            .max_by_key(|p| p.version)
            .cloned())
    }

    async fn save_dataset_entry(&self, entry: &DatasetEntry) -> Result<(), HistoryError> {
        self.dataset_entries.lock().push(entry.clone());
        Ok(())
    }

    async fn list_dataset_entries(
        &self,
        dataset_name: &str,
        split: Option<&DatasetSplit>,
    ) -> Result<Vec<DatasetEntry>, HistoryError> {
        Ok(self
            .dataset_entries
            .lock()
            .iter()
            .filter(|e| e.dataset_name == dataset_name)
            .filter(|e| split.is_none_or(|s| &e.split == s))
            .cloned()
            .collect())
    }

    async fn delete_dataset_entry(&self, id: &DatasetEntryId) -> Result<(), HistoryError> {
        self.dataset_entries.lock().retain(|e| &e.id != id);
        Ok(())
    }

    async fn save_user_memory(&self, entry: &UserMemoryEntry) -> Result<(), HistoryError> {
        self.user_memories.lock().insert(entry.id.clone(), entry.clone());
        Ok(())
    }

    async fn get_user_memory(
        &self,
        id: &UserMemoryId,
    ) -> Result<Option<UserMemoryEntry>, HistoryError> {
        Ok(self.user_memories.lock().get(id).cloned())
    }

    async fn list_user_memories(
        &self,
        user_id: &UserId,
        limit: Option<u32>,
    ) -> Result<Vec<UserMemoryEntry>, HistoryError> {
        let mut entries: Vec<UserMemoryEntry> = self
            .user_memories
            .lock()
            .values()
            .filter(|e| &e.user_id == user_id)
            .cloned()
            .collect();
        entries.sort_by_key(|e| e.created_at);
        if let Some(n) = limit {
            entries.truncate(n as usize);
        }
        Ok(entries)
    }

    async fn delete_user_memory(&self, id: &UserMemoryId) -> Result<(), HistoryError> {
        self.user_memories.lock().remove(id);
        Ok(())
    }

    async fn search_user_memories(
        &self,
        user_id: &UserId,
        query: &str,
        limit: u32,
    ) -> Result<Vec<UserMemoryEntry>, HistoryError> {
        let query_lower = query.to_lowercase();
        let mut entries: Vec<UserMemoryEntry> = self
            .user_memories
            .lock()
            .values()
            .filter(|e| &e.user_id == user_id)
            .filter(|e| e.content.to_lowercase().contains(&query_lower))
            .cloned()
            .collect();
        entries.sort_by_key(|e| e.created_at);
        entries.truncate(limit as usize);
        Ok(entries)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ContentBlock;
    use super::super::types::{NodeContent, BranchMeta, now_micros};

    fn make_test_node(conv_id: &ConversationId, branch_id: &BranchId, seq: u64) -> Node {
        Node {
            id: NodeId::new(),
            conversation_id: conv_id.clone(),
            branch_id: branch_id.clone(),
            parent_id: None,
            sequence: seq,
            created_at: now_micros(),
            created_by: None,
            model: None,
            provider: None,
            content: NodeContent::UserMessage {
                content: vec![ContentBlock::text("hello world")],
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
        }
    }

    #[tokio::test]
    async fn project_crud() {
        let s = InMemoryStorage::new();
        let p = Project::new("test");
        s.save_project(&p).await.unwrap();
        let loaded = s.get_project(&p.id).await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().name, "test");

        s.delete_project(&p.id).await.unwrap();
        assert!(s.get_project(&p.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn conversation_crud() {
        let s = InMemoryStorage::new();
        let c = Conversation::new("test conv");
        s.save_conversation(&c).await.unwrap();
        let loaded = s.get_conversation(&c.id).await.unwrap().unwrap();
        assert_eq!(loaded.title, Some("test conv".into()));

        let all = s
            .list_conversations(&ListConversationsParams::default())
            .await
            .unwrap();
        assert_eq!(all.len(), 1);
    }

    #[tokio::test]
    async fn node_append_and_list() {
        let s = InMemoryStorage::new();
        let conv_id = ConversationId::new();
        let branch_id = BranchId::new();

        let n0 = make_test_node(&conv_id, &branch_id, 0);
        let n1 = make_test_node(&conv_id, &branch_id, 1);

        s.append_nodes(&[n0.clone(), n1.clone()]).await.unwrap();

        let loaded = s.get_node(&n0.id).await.unwrap().unwrap();
        assert_eq!(loaded.sequence, 0);

        let nodes = s
            .list_nodes(&ListNodesParams {
                conversation_id: conv_id.clone(),
                branch_id: None,
                after_sequence: None,
                limit: None,
                include_deleted: false,
                content_types: None,
                time_range: None,
            })
            .await
            .unwrap();
        assert_eq!(nodes.len(), 2);
    }

    #[tokio::test]
    async fn optimistic_concurrency() {
        let s = InMemoryStorage::new();
        let conv_id = ConversationId::new();
        let branch_id = BranchId::new();
        let node = make_test_node(&conv_id, &branch_id, 0);
        s.append_nodes(&[node.clone()]).await.unwrap();

        // First update succeeds
        s.update_node(
            &node.id,
            &NodePatch {
                is_final: Some(true),
                ..Default::default()
            },
            0,
        )
        .await
        .unwrap();

        // Second update with stale version fails
        let result = s
            .update_node(
                &node.id,
                &NodePatch {
                    is_final: Some(false),
                    ..Default::default()
                },
                0, // stale
            )
            .await;

        assert!(matches!(result, Err(HistoryError::VersionMismatch { .. })));
    }

    #[tokio::test]
    async fn delete_cascade() {
        let s = InMemoryStorage::new();
        let conv = Conversation::new("cascade test");
        let conv_id = conv.id.clone();
        let branch_id = BranchId::new();

        s.save_conversation(&conv).await.unwrap();
        let node = make_test_node(&conv_id, &branch_id, 0);
        s.append_nodes(&[node.clone()]).await.unwrap();

        let branch = BranchMeta {
            id: branch_id.clone(),
            conversation_id: conv_id.clone(),
            parent_branch_id: None,
            fork_node_id: None,
            created_at: now_micros(),
            created_by: None,
            name: Some("main".into()),
        };
        s.save_branch(&branch).await.unwrap();

        s.delete_conversation(&conv_id).await.unwrap();

        assert!(s.get_conversation(&conv_id).await.unwrap().is_none());
        // Nodes should be soft-deleted
        let loaded_node = s.get_node(&node.id).await.unwrap().unwrap();
        assert!(loaded_node.deleted);
        // Branches should be removed
        let branches = s.list_branches(&conv_id).await.unwrap();
        assert!(branches.is_empty());
    }

    #[tokio::test]
    async fn search_nodes_text() {
        let s = InMemoryStorage::new();
        let conv_id = ConversationId::new();
        let branch_id = BranchId::new();

        let mut n0 = make_test_node(&conv_id, &branch_id, 0);
        n0.content = NodeContent::UserMessage {
            content: vec![ContentBlock::text("the quick brown fox")],
            name: None,
        };
        let mut n1 = make_test_node(&conv_id, &branch_id, 1);
        n1.content = NodeContent::AssistantMessage {
            content: vec![ContentBlock::text("lazy dog")],
            stop_reason: None,
            variant_index: None,
        };

        s.append_nodes(&[n0, n1]).await.unwrap();

        let results = s
            .search_nodes(&conv_id, "fox", &ListParams::default())
            .await
            .unwrap();
        assert_eq!(results.len(), 1);

        let results = s
            .search_nodes(&conv_id, "DOG", &ListParams::default())
            .await
            .unwrap();
        assert_eq!(results.len(), 1); // case-insensitive
    }

    #[tokio::test]
    async fn list_with_pagination() {
        let s = InMemoryStorage::new();
        let conv_id = ConversationId::new();
        let branch_id = BranchId::new();

        for i in 0..10 {
            let node = make_test_node(&conv_id, &branch_id, i);
            s.append_nodes(&[node]).await.unwrap();
        }

        let page = s
            .list_nodes(&ListNodesParams {
                conversation_id: conv_id.clone(),
                branch_id: None,
                after_sequence: Some(4),
                limit: Some(3),
                include_deleted: false,
                content_types: None,
                time_range: None,
            })
            .await
            .unwrap();
        assert_eq!(page.len(), 3);
        assert!(page[0].sequence > 4);
    }

    #[tokio::test]
    async fn reactions_crud() {
        let s = InMemoryStorage::new();
        let node_id = NodeId::new();
        let user_id = UserId::from_string("user1");

        let reaction = Reaction {
            node_id: node_id.clone(),
            user_id: user_id.clone(),
            reaction_type: ReactionType::ThumbsUp,
            created_at: now_micros(),
            comment: None,
        };

        s.add_reaction(&reaction).await.unwrap();
        let reactions = s.list_reactions(&node_id).await.unwrap();
        assert_eq!(reactions.len(), 1);

        s.remove_reaction(&node_id, &user_id, &ReactionType::ThumbsUp)
            .await
            .unwrap();
        let reactions = s.list_reactions(&node_id).await.unwrap();
        assert_eq!(reactions.len(), 0);
    }

    #[tokio::test]
    async fn source_crud() {
        use super::super::source::{Source, SourceStatus, SourceType};

        let s = InMemoryStorage::new();
        let conv_id = ConversationId::new();
        let source_id = SourceId::new();

        let source = Source {
            id: source_id.clone(),
            conversation_id: conv_id.clone(),
            name: "test.pdf".into(),
            source_type: SourceType::Pdf,
            mime_type: "application/pdf".into(),
            size_bytes: 1024,
            raw_path: format!("sources/{}/raw", source_id),
            extracted_path: None,
            status: SourceStatus::Pending,
            chunk_count: 0,
            created_at: now_micros(),
            updated_at: now_micros(),
            metadata: HashMap::new(),
        };

        s.save_source(&source).await.unwrap();
        let loaded = s.get_source(&source_id).await.unwrap().unwrap();
        assert_eq!(loaded.name, "test.pdf");

        let sources = s.list_sources(&conv_id).await.unwrap();
        assert_eq!(sources.len(), 1);

        s.delete_source(&source_id).await.unwrap();
        assert!(s.get_source(&source_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn chunk_save_get_search() {
        use super::super::source::{Source, SourceChunk, SourceStatus, SourceType, ChunkLocation};

        let s = InMemoryStorage::new();
        let conv_id = ConversationId::new();
        let source_id = SourceId::new();

        let source = Source {
            id: source_id.clone(),
            conversation_id: conv_id.clone(),
            name: "doc.txt".into(),
            source_type: SourceType::Text,
            mime_type: "text/plain".into(),
            size_bytes: 100,
            raw_path: format!("sources/{}/raw", source_id),
            extracted_path: None,
            status: SourceStatus::Ready,
            chunk_count: 2,
            created_at: now_micros(),
            updated_at: now_micros(),
            metadata: HashMap::new(),
        };
        s.save_source(&source).await.unwrap();

        let chunks = vec![
            SourceChunk {
                source_id: source_id.clone(),
                chunk_index: 0,
                text: "The quick brown fox".into(),
                location: ChunkLocation::CharRange { start: 0, end: 19 },
                token_estimate: 4,
            },
            SourceChunk {
                source_id: source_id.clone(),
                chunk_index: 1,
                text: "jumps over the lazy dog".into(),
                location: ChunkLocation::CharRange { start: 20, end: 43 },
                token_estimate: 5,
            },
        ];
        s.save_chunks(&chunks).await.unwrap();

        let loaded = s.get_chunks(&source_id).await.unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].chunk_index, 0);

        // Case-insensitive search
        let results = s.search_chunks(&conv_id, "FOX", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].chunk_index, 0);

        let results = s.search_chunks(&conv_id, "lazy", 10).await.unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn prompt_version_crud() {
        use crate::types::ContentBlock;

        let s = InMemoryStorage::new();
        let id = PromptId::new();
        let p1 = PromptVersion {
            id: id.clone(),
            name: "system".into(),
            content: vec![ContentBlock::text("v1")],
            version: 1,
            project_id: None,
            created_at: 0,
            created_by: None,
            metadata: Default::default(),
        };
        let p2 = PromptVersion {
            version: 2,
            content: vec![ContentBlock::text("v2")],
            ..p1.clone()
        };

        s.save_prompt_version(&p1).await.unwrap();
        s.save_prompt_version(&p2).await.unwrap();

        let loaded = s.get_prompt_version(&id, 1).await.unwrap().unwrap();
        assert_eq!(loaded.version, 1);

        let all = s.list_prompt_versions("system").await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].version, 1);

        let latest = s.get_latest_prompt("system").await.unwrap().unwrap();
        assert_eq!(latest.version, 2);

        assert!(s.get_latest_prompt("nonexistent").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn dataset_entry_crud() {

        let s = InMemoryStorage::new();
        let conv_id = ConversationId::new();

        let e1 = DatasetEntry {
            id: DatasetEntryId::new(),
            conversation_id: conv_id.clone(),
            dataset_name: "sft".into(),
            input_node_ids: vec![],
            output_node_id: NodeId::new(),
            expected_output: Some("answer".into()),
            split: DatasetSplit::Train,
            created_at: 0,
            metadata: Default::default(),
        };
        let e2 = DatasetEntry {
            id: DatasetEntryId::new(),
            split: DatasetSplit::Test,
            ..e1.clone()
        };

        s.save_dataset_entry(&e1).await.unwrap();
        s.save_dataset_entry(&e2).await.unwrap();

        let all = s.list_dataset_entries("sft", None).await.unwrap();
        assert_eq!(all.len(), 2);

        let train_only = s
            .list_dataset_entries("sft", Some(&DatasetSplit::Train))
            .await
            .unwrap();
        assert_eq!(train_only.len(), 1);
        assert_eq!(train_only[0].split, DatasetSplit::Train);

        s.delete_dataset_entry(&e1.id).await.unwrap();
        let after_delete = s.list_dataset_entries("sft", None).await.unwrap();
        assert_eq!(after_delete.len(), 1);
    }

    #[tokio::test]
    async fn user_memory_crud_and_search() {
        use super::super::types::UserMemoryType;

        let s = InMemoryStorage::new();
        let user_id = UserId::from_string("user1");
        let other_user = UserId::from_string("user2");

        let e = UserMemoryEntry {
            id: UserMemoryId::new(),
            user_id: user_id.clone(),
            content: "User prefers dark mode".into(),
            memory_type: UserMemoryType::Preference,
            source_conversation_id: None,
            created_at: 1000,
            updated_at: 1000,
            expires_at: None,
            metadata: Default::default(),
        };

        s.save_user_memory(&e).await.unwrap();
        let loaded = s.get_user_memory(&e.id).await.unwrap().unwrap();
        assert_eq!(loaded.content, "User prefers dark mode");

        let list = s.list_user_memories(&user_id, None).await.unwrap();
        assert_eq!(list.len(), 1);

        let empty = s.list_user_memories(&other_user, None).await.unwrap();
        assert!(empty.is_empty());

        let results = s
            .search_user_memories(&user_id, "dark mode", 10)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);

        let no_results = s
            .search_user_memories(&user_id, "light mode", 10)
            .await
            .unwrap();
        assert!(no_results.is_empty());

        s.delete_user_memory(&e.id).await.unwrap();
        assert!(s.get_user_memory(&e.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn node_patch_eval_scores_merge() {
        use super::super::types::EvalScore;

        let s = InMemoryStorage::new();
        let conv_id = ConversationId::new();
        let branch_id = BranchId::new();
        let node = make_test_node(&conv_id, &branch_id, 0);
        s.append_nodes(&[node.clone()]).await.unwrap();

        let score1 = EvalScore {
            name: "safety".into(),
            score: 1.0,
            rationale: None,
            grader: None,
            created_at: 0,
            metadata: Default::default(),
        };
        let score2 = EvalScore {
            name: "helpfulness".into(),
            score: 0.8,
            ..score1.clone()
        };

        s.update_node(
            &node.id,
            &NodePatch {
                eval_scores: Some(vec![score1]),
                ..Default::default()
            },
            0,
        )
        .await
        .unwrap();

        s.update_node(
            &node.id,
            &NodePatch {
                eval_scores: Some(vec![score2]),
                ..Default::default()
            },
            1,
        )
        .await
        .unwrap();

        let loaded = s.get_node(&node.id).await.unwrap().unwrap();
        assert_eq!(loaded.eval_scores.len(), 2);
    }

    #[tokio::test]
    async fn delete_conversation_cascades_dataset_entries() {

        let s = InMemoryStorage::new();
        let conv = Conversation::new("cascade ds test");
        let conv_id = conv.id.clone();
        s.save_conversation(&conv).await.unwrap();

        let entry = DatasetEntry {
            id: DatasetEntryId::new(),
            conversation_id: conv_id.clone(),
            dataset_name: "sft".into(),
            input_node_ids: vec![],
            output_node_id: NodeId::new(),
            expected_output: None,
            split: DatasetSplit::Train,
            created_at: 0,
            metadata: Default::default(),
        };
        s.save_dataset_entry(&entry).await.unwrap();

        s.delete_conversation(&conv_id).await.unwrap();

        let remaining = s.list_dataset_entries("sft", None).await.unwrap();
        assert!(remaining.is_empty());
    }

    #[tokio::test]
    async fn delete_conversation_cascades_sources() {
        use super::super::source::{Source, SourceChunk, SourceStatus, SourceType, ChunkLocation};

        let s = InMemoryStorage::new();
        let conv = Conversation::new("cascade source test");
        let conv_id = conv.id.clone();
        s.save_conversation(&conv).await.unwrap();

        let source_id = SourceId::new();
        let source = Source {
            id: source_id.clone(),
            conversation_id: conv_id.clone(),
            name: "doc.txt".into(),
            source_type: SourceType::Text,
            mime_type: "text/plain".into(),
            size_bytes: 10,
            raw_path: "test".into(),
            extracted_path: None,
            status: SourceStatus::Ready,
            chunk_count: 1,
            created_at: now_micros(),
            updated_at: now_micros(),
            metadata: HashMap::new(),
        };
        s.save_source(&source).await.unwrap();

        let chunk = SourceChunk {
            source_id: source_id.clone(),
            chunk_index: 0,
            text: "test content".into(),
            location: ChunkLocation::Whole,
            token_estimate: 2,
        };
        s.save_chunks(&[chunk]).await.unwrap();

        s.delete_conversation(&conv_id).await.unwrap();

        assert!(s.get_source(&source_id).await.unwrap().is_none());
        assert!(s.get_chunks(&source_id).await.unwrap().is_empty());
    }
}
