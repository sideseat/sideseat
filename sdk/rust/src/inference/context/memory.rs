use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::backend::ContextBackend;
use super::error::CmError;
use super::types::{ConversationId, MemoryEntry, MemoryEntryId, MemoryEntryType, now_micros};
use crate::types::Message;

/// Summarizes a message history into a single string for context compression.
#[async_trait]
pub trait Summarizer: Send + Sync {
    async fn summarize(&self, messages: &[Message]) -> Result<String, CmError>;
}

// ---------------------------------------------------------------------------
// MemorySource + MemoryItem
// ---------------------------------------------------------------------------

/// A single retrieved memory item returned by a [`MemorySource`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryItem {
    pub content: String,
    pub source: String,
    pub relevance_score: Option<f64>,
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

/// External memory store that can retrieve and persist context items.
///
/// Retrieved items are injected into the system prompt by `build_context()`.
#[async_trait]
pub trait MemorySource: Send + Sync {
    async fn retrieve(
        &self,
        query: &str,
        conv_id: &ConversationId,
        limit: u32,
    ) -> Result<Vec<MemoryItem>, CmError>;

    async fn store(&self, conv_id: &ConversationId, item: &MemoryItem) -> Result<(), CmError>;

    fn name(&self) -> &str;
}

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
        Self {
            backend,
            scope_id,
            source_name,
        }
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

    async fn store(&self, _conv_id: &ConversationId, item: &MemoryItem) -> Result<(), CmError> {
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
        self.backend
            .kv_put(&self.ns(), entry_id.as_str(), &bytes)
            .await
    }

    fn name(&self) -> &str {
        &self.source_name
    }
}
