pub mod vfs;

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::crdt::CrdtDoc;
use super::error::HistoryError;
use super::types::{BranchId, ConversationId, SourceId, StorageRef};
use super::HistoryExtension;

// ---------------------------------------------------------------------------
// Source
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub id: SourceId,
    pub conversation_id: ConversationId,
    pub name: String,
    pub source_type: SourceType,
    pub mime_type: String,
    pub size_bytes: u64,
    pub raw_path: String,
    pub extracted_path: Option<String>,
    pub status: SourceStatus,
    pub chunk_count: u32,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceType {
    Pdf,
    Text,
    Markdown,
    Html,
    Url,
    Audio,
    Image,
    Code,
    Custom(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SourceStatus {
    Pending,
    Extracting,
    Ready,
    Failed { error: String },
}

// ---------------------------------------------------------------------------
// SourceChunk
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceChunk {
    pub source_id: SourceId,
    pub chunk_index: u32,
    pub text: String,
    pub location: ChunkLocation,
    pub token_estimate: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChunkLocation {
    PageRange { start: u32, end: u32 },
    CharRange { start: u64, end: u64 },
    LineRange { start: u32, end: u32 },
    Timestamp { start_ms: u64, end_ms: u64 },
    Whole,
}

// ---------------------------------------------------------------------------
// DocumentExtractor
// ---------------------------------------------------------------------------

#[async_trait]
pub trait DocumentExtractor: Send + Sync {
    fn supported_types(&self) -> &[SourceType];
    async fn extract(
        &self,
        data: &[u8],
        mime_type: &str,
    ) -> Result<ExtractedContent, HistoryError>;
}

#[derive(Debug, Clone)]
pub struct ExtractedContent {
    pub text: String,
    pub metadata: HashMap<String, Value>,
}

// ---------------------------------------------------------------------------
// PlainTextExtractor
// ---------------------------------------------------------------------------

pub struct PlainTextExtractor;

#[async_trait]
impl DocumentExtractor for PlainTextExtractor {
    fn supported_types(&self) -> &[SourceType] {
        &[SourceType::Text, SourceType::Markdown, SourceType::Code]
    }

    async fn extract(
        &self,
        data: &[u8],
        _mime_type: &str,
    ) -> Result<ExtractedContent, HistoryError> {
        let text = String::from_utf8(data.to_vec())
            .map_err(|e| HistoryError::ExtractionFailed(e.to_string()))?;
        Ok(ExtractedContent {
            text,
            metadata: HashMap::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// Chunking
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "strategy", rename_all = "snake_case")]
pub enum ChunkStrategy {
    FixedSize {
        max_tokens: u32,
        overlap_tokens: u32,
    },
    Paragraph {
        max_tokens: u32,
    },
    Custom(String),
}

pub fn chunk_text(text: &str, strategy: &ChunkStrategy) -> Vec<(String, ChunkLocation)> {
    match strategy {
        ChunkStrategy::FixedSize {
            max_tokens,
            overlap_tokens,
        } => chunk_fixed_size(text, *max_tokens, *overlap_tokens),
        ChunkStrategy::Paragraph { max_tokens } => chunk_paragraph(text, *max_tokens),
        ChunkStrategy::Custom(_) => {
            // Return whole text as single chunk for unknown strategies
            vec![(
                text.to_string(),
                ChunkLocation::CharRange {
                    start: 0,
                    end: text.len() as u64,
                },
            )]
        }
    }
}

fn estimate_char_count_for_tokens(tokens: u32) -> usize {
    // ~4 chars per token is a common heuristic
    tokens as usize * 4
}

fn chunk_fixed_size(
    text: &str,
    max_tokens: u32,
    overlap_tokens: u32,
) -> Vec<(String, ChunkLocation)> {
    let max_chars = estimate_char_count_for_tokens(max_tokens);
    let overlap_chars = estimate_char_count_for_tokens(overlap_tokens);

    if text.is_empty() || max_chars == 0 {
        return Vec::new();
    }

    let mut chunks = Vec::new();
    let mut start = 0usize;

    while start < text.len() {
        let end = (start + max_chars).min(text.len());
        // Snap to char boundary
        let end = if end < text.len() {
            text.ceil_char_boundary(end)
        } else {
            text.len()
        };

        let chunk_text = &text[start..end];
        chunks.push((
            chunk_text.to_string(),
            ChunkLocation::CharRange {
                start: start as u64,
                end: end as u64,
            },
        ));

        if end >= text.len() {
            break;
        }

        let step = if max_chars > overlap_chars {
            max_chars - overlap_chars
        } else {
            max_chars
        };
        start += step;
        start = text.ceil_char_boundary(start);
    }

    chunks
}

fn chunk_paragraph(text: &str, max_tokens: u32) -> Vec<(String, ChunkLocation)> {
    let max_chars = estimate_char_count_for_tokens(max_tokens);
    let paragraphs: Vec<&str> = text.split("\n\n").collect();

    let mut chunks = Vec::new();
    let mut current_chunk = String::new();
    let mut chunk_start = 0u64;
    let mut pos = 0u64;

    for (i, para) in paragraphs.iter().enumerate() {
        let separator = if i > 0 { "\n\n" } else { "" };
        let would_be = current_chunk.len() + separator.len() + para.len();

        if !current_chunk.is_empty() && would_be > max_chars {
            // Flush current chunk
            let end = chunk_start + current_chunk.len() as u64;
            chunks.push((
                current_chunk.clone(),
                ChunkLocation::CharRange {
                    start: chunk_start,
                    end,
                },
            ));
            current_chunk.clear();
            chunk_start = pos;
        }

        if !current_chunk.is_empty() {
            current_chunk.push_str("\n\n");
        }
        current_chunk.push_str(para);

        pos += if i > 0 { 2 } else { 0 }; // \n\n separator
        pos += para.len() as u64;
    }

    if !current_chunk.is_empty() {
        let end = chunk_start + current_chunk.len() as u64;
        chunks.push((
            current_chunk,
            ChunkLocation::CharRange {
                start: chunk_start,
                end,
            },
        ));
    }

    chunks
}

// ---------------------------------------------------------------------------
// VfsExtension — pluggable virtual filesystem extension for History
//
// Holds a `Vfs` directly. `Vfs` is `Send + Sync` because its mutable state
// (`branch_index`, `active_branch`) is protected by `Arc<parking_lot::RwLock>`.
// `fork_branch` / `checkout_branch` take `&self` and use only the internal locks,
// so all lifecycle hooks are safe to call concurrently.
// ---------------------------------------------------------------------------

/// Extension that equips a `History` with a branch-aware copy-on-write
/// virtual filesystem and an embedded CRDT doc for collaborative files.
/// Register with `History::with_extension`.
pub struct VfsExtension {
    pub(crate) vfs: vfs::Vfs,
    crdt: Mutex<CrdtDoc>,
}

impl VfsExtension {
    /// Default in-memory VFS.
    pub fn new() -> Self {
        Self {
            vfs: vfs::Vfs::new(Arc::new(vfs::MemoryFsProvider::new())),
            crdt: Mutex::new(CrdtDoc::new()),
        }
    }

    /// Custom VFS with pre-configured mounts / providers.
    pub fn with_vfs(vfs: vfs::Vfs) -> Self {
        Self { vfs, crdt: Mutex::new(CrdtDoc::new()) }
    }

    /// Add a mount point. Must be called before registering with `History`.
    pub fn mount(
        &mut self,
        prefix: impl Into<String>,
        provider: Arc<dyn vfs::FsProvider>,
    ) -> &mut Self {
        self.vfs.mount(prefix, provider);
        self
    }

    // -----------------------------------------------------------------------
    // Filesystem operations (delegate to inner Vfs)
    // -----------------------------------------------------------------------

    pub async fn write(
        &self,
        path: &str,
        data: &[u8],
        mime_type: &str,
    ) -> Result<vfs::FileMeta, HistoryError> {
        self.vfs.write(path, data, mime_type).await
    }

    pub async fn read(&self, path: &str) -> Result<Vec<u8>, HistoryError> {
        self.vfs.read(path).await
    }

    pub async fn delete(&self, path: &str) -> Result<(), HistoryError> {
        self.vfs.delete(path).await
    }

    pub async fn exists(&self, path: &str) -> Result<bool, HistoryError> {
        self.vfs.exists(path).await
    }

    pub async fn metadata(&self, path: &str) -> Result<vfs::FileMeta, HistoryError> {
        self.vfs.metadata(path).await
    }

    pub async fn list(&self, prefix: &str) -> Result<Vec<vfs::FileEntry>, HistoryError> {
        self.vfs.list(prefix).await
    }

    pub fn to_storage_ref(&self, path: &str, meta: &vfs::FileMeta) -> StorageRef {
        self.vfs.to_storage_ref(path, meta)
    }

    // -----------------------------------------------------------------------
    // Branch-scoped operations (for extensions that manage their own versioned
    // namespaces, e.g. ArtifactExtension)
    // -----------------------------------------------------------------------

    /// Ensure a named branch exists without switching the active branch.
    pub fn ensure_branch(&self, branch: &str) {
        self.vfs.ensure_branch(branch);
    }

    /// COW-fork a named branch: `child` inherits all of `parent`'s index
    /// entries without copying physical data.
    pub fn fork_named_branch(&self, parent: &str, child: &str) {
        self.vfs.fork_branch(parent, child);
    }

    /// Write a file to a specific named branch without affecting the active branch.
    pub async fn write_on(
        &self,
        branch: &str,
        path: &str,
        data: &[u8],
        mime_type: &str,
    ) -> Result<vfs::FileMeta, HistoryError> {
        self.vfs.write_on_branch(branch, path, data, mime_type).await
    }

    /// Read a file from a specific named branch (COW-aware).
    pub async fn read_on(
        &self,
        branch: &str,
        path: &str,
    ) -> Result<Vec<u8>, HistoryError> {
        self.vfs.read_on_branch(branch, path).await
    }

    /// Check existence in a specific named branch.
    pub async fn exists_on(&self, branch: &str, path: &str) -> Result<bool, HistoryError> {
        self.vfs.exists_on_branch(branch, path).await
    }

    pub async fn read_storage_ref(
        &self,
        storage_ref: &StorageRef,
    ) -> Result<Vec<u8>, HistoryError> {
        self.vfs.read_storage_ref(storage_ref).await
    }

    // -----------------------------------------------------------------------
    // VFS index helpers
    // -----------------------------------------------------------------------

    /// Serialize the active branch's logical→physical index to JSON bytes.
    /// Returns `None` if the branch has no recorded entries yet.
    pub fn serialize_active_branch_index(&self) -> Option<Vec<u8>> {
        let branch = self.vfs.active_branch_str();
        self.vfs.serialize_branch_index(&branch)
    }

    // -----------------------------------------------------------------------
    // CRDT operations — collaborative text and map files
    //
    // Text files use the VFS path as the Y.Text name.
    // Map files use the VFS path as the ext_map name.
    // Both are stored inside a single CrdtDoc embedded in VfsExtension,
    // separate from the conversation-level CrdtDoc in History<S>.
    // -----------------------------------------------------------------------

    /// Insert `content` at `index` in a CRDT text file at `path`.
    /// Returns the Yjs v1 delta that encodes this change.
    pub fn crdt_text_insert(&self, path: &str, index: u32, content: &str) -> Vec<u8> {
        self.crdt.lock().text_insert(path, index, content)
    }

    /// Remove `len` characters at `index` in a CRDT text file at `path`.
    /// Returns the Yjs v1 delta.
    pub fn crdt_text_remove(&self, path: &str, index: u32, len: u32) -> Vec<u8> {
        self.crdt.lock().text_remove(path, index, len)
    }

    /// Return the current string content of a CRDT text file at `path`.
    pub fn crdt_read_text(&self, path: &str) -> String {
        self.crdt.lock().text_read(path)
    }

    /// Return the current character length of a CRDT text file at `path`.
    pub fn crdt_text_len(&self, path: &str) -> u32 {
        self.crdt.lock().text_len(path)
    }

    /// Set `key` → `value` in a CRDT map file at `path`.
    /// Returns the Yjs v1 delta.
    pub fn crdt_map_set(&self, path: &str, key: &str, value: &Value) -> Vec<u8> {
        self.crdt.lock().map_set(path, key, value)
    }

    /// Delete `key` from a CRDT map file at `path`.
    /// Returns the Yjs v1 delta.
    pub fn crdt_map_delete(&self, path: &str, key: &str) -> Vec<u8> {
        self.crdt.lock().map_delete(path, key)
    }

    /// Return the value for `key` in a CRDT map file at `path`.
    pub fn crdt_map_get(&self, path: &str, key: &str) -> Option<Value> {
        self.crdt.lock().map_get(path, key)
    }

    /// Return all entries in a CRDT map file at `path`.
    pub fn crdt_map_entries(&self, path: &str) -> HashMap<String, Value> {
        self.crdt.lock().map_entries(path)
    }

    /// Return the full Yjs state of the VFS CRDT doc (for sync / persistence).
    pub fn crdt_full_state(&self) -> Vec<u8> {
        self.crdt.lock().full_state()
    }

    /// Return the Yjs state vector of the VFS CRDT doc (for differential sync).
    pub fn crdt_state_vector(&self) -> Vec<u8> {
        self.crdt.lock().state_vector()
    }

    /// Merge a Yjs v1 delta from a remote peer into the VFS CRDT doc.
    pub fn crdt_merge_delta(&self, delta: &[u8]) -> Result<(), HistoryError> {
        self.crdt.lock().merge_delta(delta)
    }
}

impl Default for VfsExtension {
    fn default() -> Self {
        Self::new()
    }
}

impl HistoryExtension for VfsExtension {
    fn id(&self) -> &str {
        "vfs"
    }

    fn on_branch_forked(&self, parent: &BranchId, child: &BranchId) {
        self.vfs.fork_branch(parent.as_str(), child.as_str());
    }

    fn on_branch_checked_out(&self, branch: &BranchId) {
        self.vfs.checkout_branch(branch.as_str());
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_fixed_size_basic() {
        let text = "a".repeat(100);
        let chunks = chunk_text(
            &text,
            &ChunkStrategy::FixedSize {
                max_tokens: 5, // ~20 chars
                overlap_tokens: 1, // ~4 chars overlap
            },
        );

        assert!(chunks.len() > 1);
        // Each chunk should be at most ~20 chars (except possibly the last)
        for (chunk, loc) in &chunks {
            assert!(chunk.len() <= 20 || chunk.len() == text.len());
            match loc {
                ChunkLocation::CharRange { start, end } => {
                    assert!(end > start);
                }
                _ => panic!("Expected CharRange"),
            }
        }
    }

    #[test]
    fn chunk_fixed_size_overlap() {
        let text = "abcdefghijklmnopqrstuvwxyz";
        let chunks = chunk_text(
            text,
            &ChunkStrategy::FixedSize {
                max_tokens: 2,  // ~8 chars
                overlap_tokens: 1, // ~4 chars overlap
            },
        );

        assert!(chunks.len() >= 2);
        // Check overlap: end of chunk N overlaps with start of chunk N+1
        if chunks.len() >= 2 {
            let (_, loc0) = &chunks[0];
            let (_, loc1) = &chunks[1];
            if let (
                ChunkLocation::CharRange { end: end0, .. },
                ChunkLocation::CharRange { start: start1, .. },
            ) = (loc0, loc1) {
                // start1 should be < end0 (overlap)
                assert!(*start1 < *end0, "Expected overlap between chunks");
            }
        }
    }

    #[test]
    fn chunk_paragraph_basic() {
        let text = "First paragraph.\n\nSecond paragraph.\n\nThird paragraph.";
        let chunks = chunk_text(
            text,
            &ChunkStrategy::Paragraph { max_tokens: 100 }, // enough for all
        );

        // Should fit in one chunk
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].0, text);
    }

    #[test]
    fn chunk_paragraph_splits() {
        let text = "Alpha paragraph\n\nBeta paragraph\n\nGamma paragraph\n\nDelta paragraph";
        let chunks = chunk_text(
            text,
            &ChunkStrategy::Paragraph { max_tokens: 5 }, // ~20 chars max
        );

        // Each paragraph is ~15 chars; pairs exceed 20 with separator, so each is separate
        assert_eq!(chunks.len(), 4);
    }

    #[test]
    fn chunk_empty_text() {
        let chunks = chunk_text(
            "",
            &ChunkStrategy::FixedSize {
                max_tokens: 10,
                overlap_tokens: 0,
            },
        );
        assert!(chunks.is_empty());
    }

    #[tokio::test]
    async fn plain_text_extractor() {
        let extractor = PlainTextExtractor;
        let data = b"Hello, world!";
        let result = extractor.extract(data, "text/plain").await.unwrap();
        assert_eq!(result.text, "Hello, world!");
    }

    #[tokio::test]
    async fn plain_text_extractor_invalid_utf8() {
        let extractor = PlainTextExtractor;
        let data = &[0xFF, 0xFE];
        let result = extractor.extract(data, "text/plain").await;
        assert!(result.is_err());
    }

    #[test]
    fn source_type_serde() {
        let st = SourceType::Pdf;
        let json = serde_json::to_string(&st).unwrap();
        let parsed: SourceType = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, SourceType::Pdf);

        let custom = SourceType::Custom("video".into());
        let json = serde_json::to_string(&custom).unwrap();
        let parsed: SourceType = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, custom);
    }

    #[test]
    fn source_status_serde() {
        let ready = SourceStatus::Ready;
        let json = serde_json::to_string(&ready).unwrap();
        let parsed: SourceStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, SourceStatus::Ready);

        let failed = SourceStatus::Failed {
            error: "bad file".into(),
        };
        let json = serde_json::to_string(&failed).unwrap();
        let parsed: SourceStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, failed);
    }

    #[test]
    fn chunk_location_serde() {
        let loc = ChunkLocation::PageRange { start: 1, end: 5 };
        let json = serde_json::to_string(&loc).unwrap();
        let parsed: ChunkLocation = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, loc);

        let loc = ChunkLocation::Whole;
        let json = serde_json::to_string(&loc).unwrap();
        let parsed: ChunkLocation = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, loc);
    }
}
