use std::collections::HashSet;
use std::sync::Arc;

use super::ContextManager;
use super::backend::ContextBackend;
use super::error::CmError;
use super::memory::{MemoryItem, Summarizer};
use super::types::{Node, NodeContent, NodeId};
use crate::types::{ContentBlock, Message, Role, estimate_tokens};

/// Controls how the system prompt is extracted from a conversation.
#[derive(Debug, Clone, Default)]
pub enum SystemMode {
    #[default]
    AlwaysFirst,
    FromConversation,
    None,
}

/// Strategy applied when the linearized context exceeds `max_tokens`.
#[derive(Debug, Clone, Default)]
pub enum CompressionStrategy {
    None,
    #[default]
    Truncate,
    Summarize,
    SlidingWindow {
        keep_last: usize,
    },
    ServerCompaction {
        compact_threshold: u32,
    },
    Fail,
}

/// Configuration for context window compression passed to `build_context()`.
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

/// Output of [`ContextManager::build_context`] ready to pass to a provider.
#[derive(Debug, Clone)]
pub struct ContextResult {
    pub messages: Vec<Message>,
    pub system: Option<String>,
    pub estimated_tokens: u64,
    pub server_compaction_needed: bool,
    pub summary: Option<String>,
}

impl<B: ContextBackend> ContextManager<B> {
    /// Linearize the active branch, apply the compression strategy, and return a
    /// [`ContextResult`] ready to pass to a provider call.
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
        let pinned_set: HashSet<&NodeId> = self.compression.pinned_node_ids.iter().collect();
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
        let system_tokens = final_system
            .as_deref()
            .map_or(0, |s| estimate_tokens(s) as u64);
        let message_budget = self.compression.max_tokens.saturating_sub(system_tokens);

        // 7. Apply compression strategy.
        let mut server_compaction_needed = false;
        let mut summary: Option<String> = None;

        if estimated > self.compression.max_tokens {
            match &self.compression.strategy {
                CompressionStrategy::None => {}
                CompressionStrategy::Truncate => {
                    truncate_messages(&mut messages, &mut pinned, message_budget);
                }
                CompressionStrategy::Summarize => {
                    if let Some(summarizer) = &self.compression.summarizer {
                        let (summ, kept) =
                            summarize_old_messages(&messages, summarizer.as_ref(), message_budget)
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
                        truncate_messages(&mut messages, &mut pinned, message_budget);
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
                        for (msg, is_pinned) in messages[..split].iter().zip(pinned[..split].iter())
                        {
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
}

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
    messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .and_then(|m| {
            m.content
                .iter()
                .find_map(|b| b.as_text().map(ToString::to_string))
        })
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
    debug_assert_eq!(
        messages.len(),
        pinned.len(),
        "pinned must be parallel to messages"
    );

    if estimate_message_tokens(messages, None) <= max_tokens {
        return;
    }

    // Pre-compute per-message token counts to avoid O(n²) re-estimation.
    let token_counts: Vec<u64> = messages
        .iter()
        .map(estimate_single_message_tokens)
        .collect();
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
