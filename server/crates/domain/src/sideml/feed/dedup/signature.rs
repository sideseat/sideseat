//! An unsigned copy of reasoning, and the signed one it copies.

use std::collections::{BTreeMap, HashMap, HashSet};

use super::*;

/// Map each unsigned thinking block onto the identity of the one signed thinking block it is a copy of.
///
/// A signature is part of reasoning's identity - two turns can think the same words under two signatures -
/// so a carrier that drops it makes its copy a second block: Koog's model-call span reports reasoning in
/// the GenAI conventions, which have no member for one, while the graph node that received the response
/// keeps it. Without this the conversation shows the step twice, once signed and once not.
///
/// Merged only where the copy is unambiguous: the same trace, the same text, the same place in its message,
/// and the rest of the two messages one and the same parts - a copy of one response, not a turn that
/// thought the same words. More than one signed candidate, or none, leaves the unsigned block alone.
pub(super) fn signature_aliases<'a>(
    blocks: impl Iterator<Item = (&'a BlockEntry, u32)> + Clone,
) -> HashMap<DedupKey, MessageIdentity> {
    let mut messages: Messages<'a> = HashMap::new();
    for (block, _) in blocks.clone() {
        messages
            .entry((&block.trace_id, &block.span_id, block.message_index))
            .or_default()
            .insert(block.entry_index, MessageIdentity::from_block(block));
    }
    let signed: Vec<&BlockEntry> = blocks
        .clone()
        .map(|(block, _)| block)
        .filter(|block| text_of(block).is_some_and(|(_, signed)| signed))
        .collect();
    let mut aliases = HashMap::new();
    for (block, ordinal) in blocks {
        let Some((text, false)) = text_of(block) else {
            continue;
        };
        let siblings = rest(&messages, block);
        if siblings.is_empty() {
            continue;
        }
        let candidates: HashSet<MessageIdentity> = signed
            .iter()
            .filter(|other| {
                other.trace_id == block.trace_id
                    && other.entry_index == block.entry_index
                    && text_of(other).is_some_and(|(other_text, _)| other_text == text)
                    && rest(&messages, other) == siblings
            })
            .map(|other| MessageIdentity::from_block(other))
            .collect();
        if candidates.len() == 1 {
            let one = candidates.into_iter().next().expect("one candidate");
            aliases.insert((MessageIdentity::from_block(block), ordinal), one);
        }
    }
    aliases
}

/// Each message's parts, by identity: (trace, span, message index) -> entry index -> identity.
type Messages<'a> = HashMap<(&'a str, &'a str, i32), BTreeMap<i32, MessageIdentity>>;

/// The other parts of the block's message, in order.
fn rest<'m, 'a>(messages: &'m Messages<'a>, block: &'a BlockEntry) -> Vec<&'m MessageIdentity> {
    messages
        .get(&(
            block.trace_id.as_str(),
            block.span_id.as_str(),
            block.message_index,
        ))
        .map(|message| {
            message
                .iter()
                .filter(|(entry, _)| **entry != block.entry_index)
                .map(|(_, identity)| identity)
                .collect()
        })
        .unwrap_or_default()
}

/// A thinking block's trimmed text, and whether it is signed.
fn text_of(block: &BlockEntry) -> Option<(&str, bool)> {
    match &block.content {
        ContentBlock::Thinking { text, signature } => Some((text.trim(), signature.is_some())),
        _ => None,
    }
}
