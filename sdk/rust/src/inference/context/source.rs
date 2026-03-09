use std::collections::HashMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::error::CmError;
use super::types::{ConversationId, SourceId};

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
    ) -> Result<ExtractedContent, CmError>;
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
    ) -> Result<ExtractedContent, CmError> {
        let text = String::from_utf8(data.to_vec())
            .map_err(|e| CmError::ExtractionFailed(e.to_string()))?;
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
    // ~4 chars per token
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
        let end = if end < text.len() {
            text.ceil_char_boundary(end)
        } else {
            text.len()
        };

        let chunk = &text[start..end];
        chunks.push((
            chunk.to_string(),
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
            let end = chunk_start + current_chunk.len() as u64;
            chunks.push((
                current_chunk.clone(),
                ChunkLocation::CharRange {
                    start: chunk_start,
                    end,
                },
            ));
            current_chunk.clear();
            // The next paragraph starts at pos + 2 (after the "\n\n" separator).
            // pos has not yet been updated for this iteration, so we add the
            // separator length explicitly to get the correct char offset.
            chunk_start = pos + 2;
        }

        if !current_chunk.is_empty() {
            current_chunk.push_str("\n\n");
        }
        current_chunk.push_str(para);

        pos += if i > 0 { 2 } else { 0 };
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
                max_tokens: 5,
                overlap_tokens: 1,
            },
        );
        assert!(chunks.len() > 1);
        for (chunk, loc) in &chunks {
            assert!(chunk.len() <= 20 || chunk.len() == text.len());
            match loc {
                ChunkLocation::CharRange { start, end } => assert!(end > start),
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
                max_tokens: 2,
                overlap_tokens: 1,
            },
        );
        assert!(chunks.len() >= 2);
        if chunks.len() >= 2 {
            let (_, loc0) = &chunks[0];
            let (_, loc1) = &chunks[1];
            if let (
                ChunkLocation::CharRange { end: end0, .. },
                ChunkLocation::CharRange { start: start1, .. },
            ) = (loc0, loc1)
            {
                assert!(*start1 < *end0, "Expected overlap between chunks");
            }
        }
    }

    #[test]
    fn chunk_paragraph_basic() {
        let text = "First paragraph.\n\nSecond paragraph.\n\nThird paragraph.";
        let chunks = chunk_text(text, &ChunkStrategy::Paragraph { max_tokens: 100 });
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].0, text);
    }

    #[test]
    fn chunk_paragraph_splits() {
        let text = "Alpha paragraph\n\nBeta paragraph\n\nGamma paragraph\n\nDelta paragraph";
        let chunks = chunk_text(text, &ChunkStrategy::Paragraph { max_tokens: 5 });
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
    async fn plain_text_extractor_ok() {
        let extractor = PlainTextExtractor;
        let result = extractor.extract(b"Hello, world!", "text/plain").await.unwrap();
        assert_eq!(result.text, "Hello, world!");
    }

    #[tokio::test]
    async fn plain_text_extractor_invalid_utf8() {
        let extractor = PlainTextExtractor;
        assert!(extractor.extract(&[0xFF, 0xFE], "text/plain").await.is_err());
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
        let json = serde_json::to_string(&SourceStatus::Ready).unwrap();
        let parsed: SourceStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, SourceStatus::Ready);

        let failed = SourceStatus::Failed { error: "bad file".into() };
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
