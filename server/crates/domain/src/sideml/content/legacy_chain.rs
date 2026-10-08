//! The retired content chain, kept as the equivalence oracle for the declared one.

use serde_json::{Value as JsonValue, json};

use super::provider_formats::{
    try_anthropic_format, try_bedrock_format, try_gemini_format, try_openai_format,
};
use super::{try_media_fallback, try_sideml_passthrough, try_unknown_fallback};

/// The retired chain: the four Rust provider readers where the declared `provider_formats` position now sits.
///
/// The equivalence oracle for that migration. A nested value - a tool result's content - is normalised by the
/// current chain, so comparing the two on every value of a corpus, nested ones included, compares them all the
/// way down.
pub(crate) fn legacy_normalize_block(
    block: &JsonValue,
    consult_envelopes: bool,
) -> Option<JsonValue> {
    if let Some(s) = block.as_str() {
        return if s.is_empty() {
            None
        } else {
            Some(json!({"type": "text", "text": s}))
        };
    }
    try_sideml_passthrough(block)
        .or_else(|| {
            crate::rules::ruleset().content_blocks.normalize(
                block,
                crate::rules::schema::ChainPosition::BeforeProviderFormats,
            )
        })
        .or_else(|| {
            consult_envelopes.then(|| {
                crate::rules::ruleset()
                    .content_blocks
                    .normalize(block, crate::rules::schema::ChainPosition::MessageEnvelope)
            })?
        })
        .or_else(|| legacy_provider_formats(block))
        .or_else(|| {
            crate::rules::ruleset().content_blocks.normalize(
                block,
                crate::rules::schema::ChainPosition::AfterProviderFormats,
            )
        })
        .or_else(|| try_media_fallback(block))
        .or_else(|| try_unknown_fallback(block))
}

/// The four retired readers, in the order the chain tried them.
pub(crate) fn legacy_provider_formats(block: &JsonValue) -> Option<JsonValue> {
    try_openai_format(block)
        .or_else(|| try_anthropic_format(block))
        .or_else(|| try_bedrock_format(block))
        .or_else(|| try_gemini_format(block))
}

/// The retired `try_normalize_provider_format`, for the same oracle.
pub(crate) fn legacy_try_normalize_provider_format(block: &JsonValue) -> Option<JsonValue> {
    crate::rules::ruleset()
        .content_blocks
        .normalize(
            block,
            crate::rules::schema::ChainPosition::BeforeProviderFormats,
        )
        .or_else(|| legacy_provider_formats(block))
        .or_else(|| {
            crate::rules::ruleset().content_blocks.normalize(
                block,
                crate::rules::schema::ChainPosition::AfterProviderFormats,
            )
        })
        .or_else(|| try_media_fallback(block))
}
