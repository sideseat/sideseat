//! Content identity of one block: what two copies of a message must share to be one.

use super::*;

/// Hash binary content for deduplication - all of it.
///
/// This used to hash the length plus the first and last 128 bytes, which is not a digest but a
/// sample: two assets of the same size whose first and last 128 bytes agree are *deterministically*
/// identical to dedup, so one of them silently disappears from the feed. Three images generated from
/// one prompt in one turn are exactly that shape - same encoder, same dimensions, same header and
/// trailer - and every invariant in the suite still passes, because one image vanishing is
/// indistinguishable from a framework that only reported two.
///
/// Hashing the whole payload is what makes identity mean identity. It is affordable because nothing
/// here allocates: the bytes go straight into the hasher, at gigabytes per second, and a content hash
/// is computed once per block rather than once per comparison.
#[inline]
fn hash_binary_content<H: std::hash::Hasher>(data: &[u8], hasher: &mut H) {
    use std::hash::Hash;

    // Length first, so that appending to a payload cannot collide with the payload itself.
    data.len().hash(hasher);
    hasher.write(data);
}

/// Compute a hash for a content block.
pub(in crate::sideml::feed) fn compute_block_hash(block: &ContentBlock) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();

    // Hash based on block type and key content
    match block {
        ContentBlock::Text { text, .. } => {
            "text".hash(&mut hasher);
            text.trim().hash(&mut hasher); // Normalize whitespace
        }
        ContentBlock::ToolUse { name, input, .. } => {
            // Hash by name + normalized input only (not id)
            "tool_use".hash(&mut hasher);
            name.hash(&mut hasher);
            hash_tool_input_into(input, &mut hasher);
        }
        ContentBlock::ToolResult {
            name,
            content,
            is_error,
            ..
        } => {
            // Hash by tool name, error flag and normalized content - not tool_use_id, which a
            // history re-send regenerates.
            //
            // Content alone made every "ok" the same message: two tools both reporting success,
            // or a success and a failure whose text happens to match, collapsed into one wherever
            // this hash is the identity - which is the case for a result with no id.
            "tool_result".hash(&mut hasher);
            name.hash(&mut hasher);
            is_error.hash(&mut hasher);
            hash_tool_result_content_into(content, &mut hasher);
        }
        // The signature is part of a thinking block's identity: two turns can think the same words under
        // different signatures, and one whose text was withheld has only its signature to be told apart by.
        ContentBlock::Thinking { text, signature } => {
            "thinking".hash(&mut hasher);
            text.trim().hash(&mut hasher); // Normalize whitespace
            signature.hash(&mut hasher);
        }
        ContentBlock::RedactedThinking { data } => {
            "redacted_thinking".hash(&mut hasher);
            data.hash(&mut hasher);
        }
        ContentBlock::Image { source, data, .. } => {
            "image".hash(&mut hasher);
            source.hash(&mut hasher);
            hash_binary_content(data.as_bytes(), &mut hasher);
        }
        ContentBlock::Audio { source, data, .. } => {
            "audio".hash(&mut hasher);
            source.hash(&mut hasher);
            hash_binary_content(data.as_bytes(), &mut hasher);
        }
        ContentBlock::Video { source, data, .. } => {
            "video".hash(&mut hasher);
            source.hash(&mut hasher);
            hash_binary_content(data.as_bytes(), &mut hasher);
        }
        // A display name is the copy's label, not the content: one instrumentation keeps the filename
        // beside the bytes and another drops it, and the two are one attachment.
        ContentBlock::Document { source, data, .. } => {
            "document".hash(&mut hasher);
            source.hash(&mut hasher);
            hash_binary_content(data.as_bytes(), &mut hasher);
        }
        // A display name is the copy's label, not the content: one instrumentation keeps the filename
        // beside the bytes and another drops it, and the two are one attachment.
        ContentBlock::File { source, data, .. } => {
            "file".hash(&mut hasher);
            source.hash(&mut hasher);
            hash_binary_content(data.as_bytes(), &mut hasher);
        }
        ContentBlock::ToolDefinitions { tools, .. } => {
            "tool_definitions".hash(&mut hasher);
            tools.len().hash(&mut hasher);
        }
        ContentBlock::Context { data, context_type } => {
            "context".hash(&mut hasher);
            context_type.hash(&mut hasher);
            hash_json_into(data, &mut hasher); // canonical key order
        }
        ContentBlock::Refusal { message } => {
            "refusal".hash(&mut hasher);
            message.hash(&mut hasher);
        }
        ContentBlock::Json { data } => {
            "json".hash(&mut hasher);
            // Structured normalization: a schema-filled answer and the model's raw one are the
            // same answer. See normalize_structured_json_for_hash.
            hash_structured_json_into(data, &mut hasher);
        }
        ContentBlock::Unknown { raw } => {
            "unknown".hash(&mut hasher);
            hash_json_into(raw, &mut hasher); // canonical key order
        }
    }

    hasher.finish()
}
