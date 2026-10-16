use super::*;
use crate::sideml::types::ContentBlock;

/// A re-sent result that drops the tool name must still collapse into the original.
///
/// Once the answered call is known, the tool name and the error flag are redundant: they exist
/// to tell apart results that name no call. Keying on the full block identity here meant a
/// framework that includes the name on the original and omits it on the re-send produced two
/// keys for one message, so the feed showed it twice - and dedup could not collapse them
/// either, because their regenerated ids differ.
#[test]
fn a_resend_that_drops_the_tool_name_still_collapses() {
    let text = serde_json::json!([{"type": "text", "text": "ok"}]);
    let named = ContentBlock::ToolResult {
        tool_use_id: Some("call-1".to_string()),
        name: Some("lookup".to_string()),
        content: text.clone(),
        is_error: false,
        provider_executed: false,
    };
    // The re-send: same answer to the same call, with a regenerated id and no tool name.
    let resent = ContentBlock::ToolResult {
        tool_use_id: Some("call-1-regenerated".to_string()),
        name: None,
        content: text.clone(),
        is_error: false,
        provider_executed: false,
    };

    // Their block identities differ, which is correct - that is what tells uncorrelated
    // results apart - but the key this phase uses must not.
    assert_ne!(
        crate::sideml::feed::compute_block_hash(&named),
        crate::sideml::feed::compute_block_hash(&resent),
        "the block identity is expected to include the tool name"
    );
    let key_of = |block: &ContentBlock| match block {
        ContentBlock::ToolResult { content, .. } => hash_tool_result_text(content),
        _ => unreachable!(),
    };
    assert_eq!(
        key_of(&named),
        key_of(&resent),
        "a re-send of one call's result must collapse whatever metadata it drops"
    );

    // The text still separates two different answers to the same call.
    let different = ContentBlock::ToolResult {
        tool_use_id: Some("call-1".to_string()),
        name: Some("lookup".to_string()),
        content: serde_json::json!([{"type": "text", "text": "failed"}]),
        is_error: false,
        provider_executed: false,
    };
    assert_ne!(key_of(&named), key_of(&different));
}

/// Two results that name no call keep the name and error flag in their identity, so a success
/// and a failure with matching text stay distinct.
#[test]
fn uncorrelated_results_are_separated_by_name_and_error() {
    let content = serde_json::json!([{"type": "text", "text": "ok"}]);
    let hash = |name: Option<&str>, is_error: bool| {
        crate::sideml::feed::compute_block_hash(&ContentBlock::ToolResult {
            tool_use_id: None,
            name: name.map(str::to_owned),
            content: content.clone(),
            is_error,
            provider_executed: false,
        })
    };
    assert_ne!(hash(Some("lookup"), false), hash(Some("write"), false));
    assert_ne!(hash(Some("lookup"), false), hash(Some("lookup"), true));
    assert_eq!(hash(Some("lookup"), false), hash(Some("lookup"), false));
}
