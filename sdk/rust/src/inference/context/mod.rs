use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::history::error::HistoryError;
use crate::history::storage::HistoryStorage;
use crate::history::tree::ConversationTree;
use crate::history::types::{
    BranchId, Conversation, ConversationId, Node, NodeContent, NodeId, UserId,
};
use crate::types::{ContentBlock, Message, Role, estimate_tokens};

// ---------------------------------------------------------------------------
// Strategy types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ContextStrategy {
    pub max_tokens: u64,
    pub overflow: OverflowStrategy,
    pub system_mode: SystemMode,
    pub pinned_node_ids: Vec<NodeId>,
}

impl Default for ContextStrategy {
    fn default() -> Self {
        Self {
            max_tokens: 100_000,
            overflow: OverflowStrategy::Truncate,
            system_mode: SystemMode::AlwaysFirst,
            pinned_node_ids: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum OverflowStrategy {
    Truncate,
    Summarize,
    SlidingWindowWithSummary { recent_turns: u32 },
    ServerCompaction { compact_threshold: u32 },
    Fail,
}

#[derive(Debug, Clone, Default)]
pub enum SystemMode {
    #[default]
    AlwaysFirst,
    FromConversation,
    None,
}

// ---------------------------------------------------------------------------
// Traits
// ---------------------------------------------------------------------------

#[async_trait]
pub trait Summarizer: Send + Sync {
    async fn summarize(&self, messages: &[Message]) -> Result<String, HistoryError>;
}

#[async_trait]
pub trait MemorySource: Send + Sync {
    async fn retrieve(
        &self,
        query: &str,
        conversation_id: &ConversationId,
        limit: u32,
    ) -> Result<Vec<MemoryItem>, HistoryError>;

    async fn store(
        &self,
        conversation_id: &ConversationId,
        item: &MemoryItem,
    ) -> Result<(), HistoryError>;

    fn name(&self) -> &str;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryItem {
    pub content: String,
    pub source: String,
    pub relevance_score: Option<f64>,
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

// ---------------------------------------------------------------------------
// ContextResult
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ContextResult {
    pub messages: Vec<Message>,
    pub system: Option<String>,
    pub summarized_nodes: Vec<NodeId>,
    pub injected_memories: Vec<MemoryItem>,
    pub estimated_tokens: u64,
    pub overflow_applied: bool,
}

// ---------------------------------------------------------------------------
// ContextManager
// ---------------------------------------------------------------------------

pub struct ContextManager {
    strategy: ContextStrategy,
    memory_sources: Vec<Arc<dyn MemorySource>>,
    summarizer: Option<Arc<dyn Summarizer>>,
    #[allow(dead_code)]
    summary_cache: Mutex<HashMap<(BranchId, u64), String>>,
}

impl ContextManager {
    pub fn new(strategy: ContextStrategy) -> Self {
        Self {
            strategy,
            memory_sources: Vec::new(),
            summarizer: None,
            summary_cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn with_memory_source(mut self, source: Arc<dyn MemorySource>) -> Self {
        self.memory_sources.push(source);
        self
    }

    pub fn with_summarizer(mut self, summarizer: Arc<dyn Summarizer>) -> Self {
        self.summarizer = Some(summarizer);
        self
    }

    pub async fn build_context(
        &self,
        tree: &ConversationTree,
        nodes: &[Node],
        _conversation: &Conversation,
    ) -> Result<ContextResult, HistoryError> {
        let pinned_set: HashSet<&NodeId> = self.strategy.pinned_node_ids.iter().collect();

        // PROJECT: nodes → messages
        let mut messages = Vec::new();
        let mut pinned_indices: HashSet<usize> = HashSet::new();

        for node in nodes {
            if node.deleted {
                continue;
            }
            if let Some(msg) = project_node_to_message(node) {
                let idx = messages.len();
                if pinned_set.contains(&node.id) {
                    pinned_indices.insert(idx);
                }
                messages.push(msg);
            }
        }

        // INJECT SYSTEM
        let system = match self.strategy.system_mode {
            SystemMode::AlwaysFirst | SystemMode::FromConversation => {
                // Pull system from first SystemMessage node, or conversation instructions
                let first_system = nodes.iter().find(|n| {
                    matches!(n.content, NodeContent::SystemMessage { .. })
                });
                if let Some(sys_node) = first_system {
                    extract_text_from_content(sys_node)
                } else {
                    // Fall back to project instructions via conversation
                    None
                }
            }
            SystemMode::None => None,
        };

        // Remove system messages from the message list (they're handled separately)
        if system.is_some() {
            messages.retain(|m| m.role != Role::System);
        }

        // QUERY MEMORY sources
        let mut injected_memories = Vec::new();
        if !self.memory_sources.is_empty() {
            let query = last_user_text(&messages).unwrap_or_default();
            if !query.is_empty() {
                for source in &self.memory_sources {
                    let items = source
                        .retrieve(&query, tree.conversation_id(), 5)
                        .await?;
                    injected_memories.extend(items);
                }
            }
        }

        // Inject memories as system-level context (prepend as user message)
        if !injected_memories.is_empty() {
            let memory_text = injected_memories
                .iter()
                .map(|m| format!("[Memory from {}]: {}", m.source, m.content))
                .collect::<Vec<_>>()
                .join("\n");
            messages.insert(
                0,
                Message {
                    role: Role::User,
                    content: vec![ContentBlock::text(memory_text)],
                    name: Some("memory_context".into()),
                    cache_control: None,
                },
            );
        }

        // ESTIMATE tokens
        let estimated = estimate_message_tokens(&messages, system.as_deref());

        // OVERFLOW check
        let mut overflow_applied = false;
        let summarized_nodes = Vec::new();

        if estimated > self.strategy.max_tokens {
            match &self.strategy.overflow {
                OverflowStrategy::Truncate => {
                    truncate_messages(&mut messages, &pinned_indices, self.strategy.max_tokens);
                    overflow_applied = true;
                }
                OverflowStrategy::Summarize => {
                    if let Some(summarizer) = &self.summarizer {
                        let (summary, removed) = self
                            .summarize_old_messages(
                                &messages,
                                &pinned_indices,
                                summarizer.as_ref(),
                                self.strategy.max_tokens,
                            )
                            .await?;
                        messages = removed;
                        messages.insert(
                            0,
                            Message {
                                role: Role::User,
                                content: vec![ContentBlock::text(format!(
                                    "[Summary of earlier conversation]: {summary}"
                                ))],
                                name: Some("summary".into()),
                                cache_control: None,
                            },
                        );
                        overflow_applied = true;
                    } else {
                        truncate_messages(&mut messages, &pinned_indices, self.strategy.max_tokens);
                        overflow_applied = true;
                    }
                }
                OverflowStrategy::SlidingWindowWithSummary { recent_turns } => {
                    let keep = *recent_turns as usize;
                    if messages.len() > keep {
                        let old_messages = messages[..messages.len() - keep].to_vec();
                        let recent = messages[messages.len() - keep..].to_vec();

                        if let Some(summarizer) = &self.summarizer {
                            let summary = summarizer.summarize(&old_messages).await?;
                            messages = vec![Message {
                                role: Role::User,
                                content: vec![ContentBlock::text(format!(
                                    "[Summary of earlier conversation]: {summary}"
                                ))],
                                name: Some("summary".into()),
                                cache_control: None,
                            }];
                            messages.extend(recent);
                        } else {
                            messages = recent;
                        }
                        overflow_applied = true;
                    }
                }
                OverflowStrategy::ServerCompaction { .. } => {
                    // Return all messages; let the provider handle compaction
                }
                OverflowStrategy::Fail => {
                    return Err(HistoryError::ContextOverflow(format!(
                        "Estimated {} tokens exceeds max {}",
                        estimated, self.strategy.max_tokens
                    )));
                }
            }
        }

        let final_tokens = estimate_message_tokens(&messages, system.as_deref());

        Ok(ContextResult {
            messages,
            system,
            summarized_nodes,
            injected_memories,
            estimated_tokens: final_tokens,
            overflow_applied,
        })
    }

    async fn summarize_old_messages(
        &self,
        messages: &[Message],
        _pinned_indices: &HashSet<usize>,
        summarizer: &dyn Summarizer,
        max_tokens: u64,
    ) -> Result<(String, Vec<Message>), HistoryError> {
        // Split: keep recent messages that fit, summarize the rest
        let mut keep_from = messages.len();
        let mut tokens = 0u64;

        for (i, msg) in messages.iter().enumerate().rev() {
            let msg_tokens = estimate_single_message_tokens(msg);
            if tokens + msg_tokens > max_tokens / 2 {
                keep_from = i + 1;
                break;
            }
            tokens += msg_tokens;
        }

        let to_summarize = &messages[..keep_from.min(messages.len())];
        let to_keep = &messages[keep_from.min(messages.len())..];

        let summary = summarizer.summarize(to_summarize).await?;
        Ok((summary, to_keep.to_vec()))
    }
}

// ---------------------------------------------------------------------------
// Helpers
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

fn extract_text_from_content(node: &Node) -> Option<String> {
    match &node.content {
        NodeContent::SystemMessage { content } | NodeContent::UserMessage { content, .. } => {
            let texts: Vec<&str> = content
                .iter()
                .filter_map(|b| b.as_text())
                .collect();
            if texts.is_empty() {
                None
            } else {
                Some(texts.join("\n"))
            }
        }
        _ => None,
    }
}

fn last_user_text(messages: &[Message]) -> Option<String> {
    messages.iter().rev().find(|m| m.role == Role::User).and_then(|m| {
        m.content.iter().find_map(|b| {
            b.as_text().map(ToString::to_string)
        })
    })
}

fn estimate_single_message_tokens(msg: &Message) -> u64 {
    msg.content
        .iter()
        .map(estimate_block_tokens)
        .sum()
}

fn estimate_block_tokens(block: &ContentBlock) -> u64 {
    match block {
        ContentBlock::Text(t) => estimate_tokens(&t.text) as u64,
        ContentBlock::Thinking(t) => estimate_tokens(&t.text) as u64,
        ContentBlock::ToolUse(t) => {
            estimate_tokens(&t.name) as u64 + estimate_tokens(&t.input.to_string()) as u64
        }
        ContentBlock::ToolResult(t) => t
            .content
            .iter()
            .map(estimate_block_tokens)
            .sum(),
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

fn truncate_messages(
    messages: &mut Vec<Message>,
    pinned_indices: &HashSet<usize>,
    max_tokens: u64,
) {
    // Remove oldest non-system, non-pinned messages until under budget
    while estimate_message_tokens(messages, None) > max_tokens && messages.len() > 1 {
        let idx = messages
            .iter()
            .enumerate()
            .position(|(i, m)| m.role != Role::System && !pinned_indices.contains(&i));
        match idx {
            Some(i) => {
                messages.remove(i);
            }
            None => break,
        }
    }
}


// ---------------------------------------------------------------------------
// SourceMemory
// ---------------------------------------------------------------------------

pub struct SourceMemory {
    storage: Arc<dyn HistoryStorage>,
}

impl SourceMemory {
    pub fn new(storage: Arc<dyn HistoryStorage>) -> Self {
        Self { storage }
    }
}

#[async_trait]
impl MemorySource for SourceMemory {
    async fn retrieve(
        &self,
        query: &str,
        conversation_id: &ConversationId,
        limit: u32,
    ) -> Result<Vec<MemoryItem>, HistoryError> {
        let chunks = self
            .storage
            .search_chunks(conversation_id, query, limit)
            .await?;
        Ok(chunks
            .into_iter()
            .map(|c| MemoryItem {
                content: c.text,
                source: format!("source:{}", c.source_id),
                relevance_score: None,
                metadata: [
                    ("chunk_index".into(), serde_json::json!(c.chunk_index)),
                    ("source_id".into(), serde_json::json!(c.source_id.as_str())),
                ]
                .into(),
            })
            .collect())
    }

    async fn store(
        &self,
        _conversation_id: &ConversationId,
        _item: &MemoryItem,
    ) -> Result<(), HistoryError> {
        Ok(())
    }

    fn name(&self) -> &str {
        "sources"
    }
}

// ---------------------------------------------------------------------------
// UserMemorySource
// ---------------------------------------------------------------------------

pub struct UserMemorySource {
    storage: Arc<dyn HistoryStorage>,
    user_id: UserId,
}

impl UserMemorySource {
    pub fn new(storage: Arc<dyn HistoryStorage>, user_id: UserId) -> Self {
        Self { storage, user_id }
    }
}

#[async_trait]
impl MemorySource for UserMemorySource {
    async fn retrieve(
        &self,
        query: &str,
        _conversation_id: &ConversationId,
        limit: u32,
    ) -> Result<Vec<MemoryItem>, HistoryError> {
        let entries = self
            .storage
            .search_user_memories(&self.user_id, query, limit)
            .await?;
        Ok(entries
            .into_iter()
            .map(|e| MemoryItem {
                content: e.content,
                source: format!("user_memory:{}", e.id),
                relevance_score: None,
                metadata: [
                    (
                        "memory_type".into(),
                        serde_json::json!(format!("{:?}", e.memory_type)),
                    ),
                    ("user_id".into(), serde_json::json!(e.user_id.as_str())),
                ]
                .into(),
            })
            .collect())
    }

    async fn store(
        &self,
        _conversation_id: &ConversationId,
        _item: &MemoryItem,
    ) -> Result<(), HistoryError> {
        Ok(())
    }

    fn name(&self) -> &str {
        "user_memory"
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::tree::ConversationTree;
    use crate::history::types::{BranchId, ConversationId, NodeContent};
    use crate::history::types::{UserMemoryEntry, UserMemoryId, UserMemoryType};

    fn make_nodes(conv_id: &ConversationId, branch_id: &BranchId) -> Vec<Node> {
        vec![
            Node {
                id: NodeId::from_string("n0"),
                conversation_id: conv_id.clone(),
                branch_id: branch_id.clone(),
                parent_id: None,
                sequence: 0,
                created_at: 0,
                created_by: None,
                model: None,
                provider: None,
                content: NodeContent::SystemMessage {
                    content: vec![ContentBlock::text("You are helpful.")],
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
            },
            Node {
                id: NodeId::from_string("n1"),
                conversation_id: conv_id.clone(),
                branch_id: branch_id.clone(),
                parent_id: Some(NodeId::from_string("n0")),
                sequence: 1,
                created_at: 1,
                created_by: None,
                model: None,
                provider: None,
                content: NodeContent::UserMessage {
                    content: vec![ContentBlock::text("Hello")],
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
            },
            Node {
                id: NodeId::from_string("n2"),
                conversation_id: conv_id.clone(),
                branch_id: branch_id.clone(),
                parent_id: Some(NodeId::from_string("n1")),
                sequence: 2,
                created_at: 2,
                created_by: None,
                model: None,
                provider: None,
                content: NodeContent::AssistantMessage {
                    content: vec![ContentBlock::text("Hi there!")],
                    stop_reason: None,
                    variant_index: None,
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
            },
        ]
    }

    #[tokio::test]
    async fn truncate_overflow() {
        let conv = crate::history::types::Conversation::new("test");
        let conv_id = conv.id.clone();
        let mut tree = ConversationTree::new(conv_id.clone());
        let branch = tree.active_branch().clone();
        let nodes = make_nodes(&conv_id, &branch);
        for n in &nodes {
            tree.register(n).unwrap();
        }

        let strategy = ContextStrategy {
            max_tokens: 5, // Very small to force truncation
            overflow: OverflowStrategy::Truncate,
            system_mode: SystemMode::AlwaysFirst,
            pinned_node_ids: vec![],
        };

        let mgr = ContextManager::new(strategy);
        let result = mgr.build_context(&tree, &nodes, &conv).await.unwrap();
        assert!(result.overflow_applied);
        assert!(result.system.is_some());
    }

    #[tokio::test]
    async fn fail_overflow() {
        let conv = crate::history::types::Conversation::new("test");
        let conv_id = conv.id.clone();
        let mut tree = ConversationTree::new(conv_id.clone());
        let branch = tree.active_branch().clone();
        let nodes = make_nodes(&conv_id, &branch);
        for n in &nodes {
            tree.register(n).unwrap();
        }

        let strategy = ContextStrategy {
            max_tokens: 1,
            overflow: OverflowStrategy::Fail,
            system_mode: SystemMode::None,
            pinned_node_ids: vec![],
        };

        let mgr = ContextManager::new(strategy);
        let result = mgr.build_context(&tree, &nodes, &conv).await;
        assert!(matches!(result, Err(HistoryError::ContextOverflow(_))));
    }

    #[tokio::test]
    async fn system_mode_none() {
        let conv = crate::history::types::Conversation::new("test");
        let conv_id = conv.id.clone();
        let mut tree = ConversationTree::new(conv_id.clone());
        let branch = tree.active_branch().clone();
        let nodes = make_nodes(&conv_id, &branch);
        for n in &nodes {
            tree.register(n).unwrap();
        }

        let strategy = ContextStrategy {
            max_tokens: 1_000_000,
            overflow: OverflowStrategy::Truncate,
            system_mode: SystemMode::None,
            pinned_node_ids: vec![],
        };

        let mgr = ContextManager::new(strategy);
        let result = mgr.build_context(&tree, &nodes, &conv).await.unwrap();
        assert!(result.system.is_none());
        // System message should still be in messages
        assert!(result.messages.iter().any(|m| m.role == Role::System));
    }

    #[tokio::test]
    async fn sliding_window_overflow() {
        let conv = crate::history::types::Conversation::new("test");
        let conv_id = conv.id.clone();
        let mut tree = ConversationTree::new(conv_id.clone());
        let branch = tree.active_branch().clone();

        // Create many nodes
        let mut nodes = Vec::new();
        for i in 0..20 {
            let node = Node {
                id: NodeId::new(),
                conversation_id: conv_id.clone(),
                branch_id: branch.clone(),
                parent_id: if i > 0 { nodes.last().map(|n: &Node| n.id.clone()) } else { None },
                sequence: i,
                created_at: i as i64,
                created_by: None,
                model: None,
                provider: None,
                content: NodeContent::UserMessage {
                    content: vec![ContentBlock::text(format!("Message {i}"))],
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
            };
            tree.register(&node).unwrap();
            nodes.push(node);
        }

        let strategy = ContextStrategy {
            max_tokens: 10, // Force overflow
            overflow: OverflowStrategy::SlidingWindowWithSummary { recent_turns: 3 },
            system_mode: SystemMode::None,
            pinned_node_ids: vec![],
        };

        // Without summarizer, just keeps recent turns
        let mgr = ContextManager::new(strategy);
        let result = mgr.build_context(&tree, &nodes, &conv).await.unwrap();
        assert!(result.overflow_applied);
        assert_eq!(result.messages.len(), 3);
    }

    #[test]
    fn estimate_block_tokens_text() {
        let block = ContentBlock::text("hello world");
        let tokens = estimate_block_tokens(&block);
        assert!(tokens > 0);
    }

    #[test]
    fn estimate_block_tokens_image() {
        let block = ContentBlock::image_url("https://example.com/img.png");
        assert_eq!(estimate_block_tokens(&block), 1000);
    }

    #[tokio::test]
    async fn source_memory_retrieve() {
        use crate::history::source::{ChunkLocation, Source, SourceChunk, SourceStatus, SourceType};
        use crate::history::storage::InMemoryStorage;
        use crate::history::types::{SourceId, now_micros};

        let storage = Arc::new(InMemoryStorage::new());
        let conv_id = ConversationId::new();
        let source_id = SourceId::new();

        let source = Source {
            id: source_id.clone(),
            conversation_id: conv_id.clone(),
            name: "doc.txt".into(),
            source_type: SourceType::Text,
            mime_type: "text/plain".into(),
            size_bytes: 50,
            raw_path: "test".into(),
            extracted_path: None,
            status: SourceStatus::Ready,
            chunk_count: 1,
            created_at: now_micros(),
            updated_at: now_micros(),
            metadata: HashMap::new(),
        };
        storage.save_source(&source).await.unwrap();

        let chunk = SourceChunk {
            source_id: source_id.clone(),
            chunk_index: 0,
            text: "Rust is a systems programming language".into(),
            location: ChunkLocation::Whole,
            token_estimate: 6,
        };
        storage.save_chunks(&[chunk]).await.unwrap();

        let mem = SourceMemory::new(storage.clone());
        let items = mem.retrieve("rust", &conv_id, 10).await.unwrap();
        assert_eq!(items.len(), 1);
        assert!(items[0].content.contains("Rust"));
        assert!(items[0].source.starts_with("source:"));
        assert_eq!(items[0].metadata["chunk_index"], 0);
    }

    #[tokio::test]
    async fn source_memory_empty_query() {
        use crate::history::storage::InMemoryStorage;

        let storage = Arc::new(InMemoryStorage::new());
        let conv_id = ConversationId::new();

        let mem = SourceMemory::new(storage);
        let items = mem.retrieve("anything", &conv_id, 10).await.unwrap();
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn user_memory_source_retrieve() {
        use crate::history::storage::InMemoryStorage;
        use crate::history::types::now_micros;

        let storage = Arc::new(InMemoryStorage::new());
        let user_id = UserId::from_string("alice");
        let conv_id = ConversationId::new();

        let entry = UserMemoryEntry {
            id: UserMemoryId::new(),
            user_id: user_id.clone(),
            content: "Alice loves Rust programming".into(),
            memory_type: UserMemoryType::Fact,
            source_conversation_id: None,
            created_at: now_micros(),
            updated_at: now_micros(),
            expires_at: None,
            metadata: Default::default(),
        };
        storage.save_user_memory(&entry).await.unwrap();

        let source = UserMemorySource::new(storage.clone(), user_id.clone());
        let items = source.retrieve("Rust", &conv_id, 10).await.unwrap();
        assert_eq!(items.len(), 1);
        assert!(items[0].content.contains("Rust"));
        assert!(items[0].source.starts_with("user_memory:"));
        assert_eq!(source.name(), "user_memory");
    }

    #[tokio::test]
    async fn user_memory_source_empty_for_other_user() {
        use crate::history::storage::InMemoryStorage;
        use crate::history::types::now_micros;

        let storage = Arc::new(InMemoryStorage::new());
        let user_id = UserId::from_string("bob");
        let other_user = UserId::from_string("carol");
        let conv_id = ConversationId::new();

        let entry = UserMemoryEntry {
            id: UserMemoryId::new(),
            user_id: user_id.clone(),
            content: "Bob likes Python".into(),
            memory_type: UserMemoryType::Fact,
            source_conversation_id: None,
            created_at: now_micros(),
            updated_at: now_micros(),
            expires_at: None,
            metadata: Default::default(),
        };
        storage.save_user_memory(&entry).await.unwrap();

        let source = UserMemorySource::new(storage, other_user);
        let items = source.retrieve("Python", &conv_id, 10).await.unwrap();
        assert!(items.is_empty());
    }
}
