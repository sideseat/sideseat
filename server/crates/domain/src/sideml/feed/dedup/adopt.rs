//! What a surviving copy takes from the copies merged into it: the things a copy can lose without
//! changing what it says.

use super::*;

/// Give a surviving attachment the filename a duplicate kept.
///
/// An attachment is identified by its bytes, so one instrumentation's copy with the filename and
/// another's without it are one block; whichever survives on quality, the name is not lost.
pub(super) fn adopt_attachment_name(survivor: &mut BlockEntry, other: &BlockEntry) {
    let named = match &other.content {
        ContentBlock::Document {
            name: Some(name), ..
        }
        | ContentBlock::File {
            name: Some(name), ..
        } => name,
        _ => return,
    };
    if let ContentBlock::Document {
        name: name @ None, ..
    }
    | ContentBlock::File {
        name: name @ None, ..
    } = &mut survivor.content
    {
        *name = Some(named.clone());
    }
}

/// A tool result keeps the failure a dropped copy of it reported.
///
/// One instrumentation can carry a result twice - as the tool message and inside the next request's user
/// turn - and only one copy may say the call failed. The copy that wins on quality is not necessarily that
/// one, and the failure is the one thing about a result a copy can lose without changing its text.
pub(super) fn adopt_failure(survivor: &mut BlockEntry, other: &BlockEntry) {
    if let (
        ContentBlock::ToolResult { is_error, .. },
        ContentBlock::ToolResult { is_error: true, .. },
    ) = (&mut survivor.content, &other.content)
    {
        *is_error = true;
    }
}

/// A tool call keeps the provider's id a dropped copy of it carried.
///
/// Copies of a call merge on name and input, so the id is the one thing a copy can lose without changing
/// what was asked: a tool span records the call it ran, often without the model's id, while a re-listing
/// elsewhere carries it.
pub(super) fn adopt_call_id(survivor: &mut BlockEntry, other: &BlockEntry) {
    let ContentBlock::ToolUse {
        id: Some(found), ..
    } = &other.content
    else {
        return;
    };
    if found.is_empty() {
        return;
    }
    if let ContentBlock::ToolUse { id, .. } = &mut survivor.content
        && id.as_deref().is_none_or(str::is_empty)
    {
        *id = Some(found.clone());
        survivor.tool_use_id = Some(found.clone());
    }
}

/// A tool result keeps the call id a dropped copy of it carried.
///
/// An id-less result joins an id-bearing one only through `tool_result_aliases` - same content, exactly one
/// id - so the merge has already said which call it answers. The copy that wins on quality is often the
/// tool span's own, which ran the call without being told the model's id; losing the id there would leave
/// the one result the conversation keeps answering nothing.
pub(super) fn adopt_result_id(survivor: &mut BlockEntry, other: &BlockEntry) {
    let ContentBlock::ToolResult {
        tool_use_id: Some(found),
        ..
    } = &other.content
    else {
        return;
    };
    if found.is_empty() {
        return;
    }
    if let ContentBlock::ToolResult { tool_use_id, .. } = &mut survivor.content
        && tool_use_id.as_deref().is_none_or(str::is_empty)
    {
        *tool_use_id = Some(found.clone());
        survivor.tool_use_id = Some(found.clone());
    }
}

/// A reply keeps the finish reason a dropped copy of it stated.
///
/// A whole-conversation carrier that holds what a span received may list a completed reply with how it
/// finished, while the copy that survives - an enclosing span's re-listing - states none. The finish is the
/// reply's, whichever copy carried it - but only a copy of the same occurrence: the lineage says which
/// observations each survivor is, and the replay of an earlier turn that happened to read the same is not
/// among them. `stated[observation]` is how that observation's reply finished.
pub(super) fn adopt_finishes(
    survivors: &mut [BlockEntry],
    lineage: &[Option<usize>],
    stated: &[Option<crate::sideml::types::FinishReason>],
) {
    for (finish, survivor) in stated.iter().zip(lineage) {
        let (Some(finish), Some(survivor)) = (finish, survivor) else {
            continue;
        };
        let survivor = &mut survivors[*survivor];
        if survivor.finish_reason.is_none()
            && survivor.role == crate::sideml::types::ChatRole::Assistant
        {
            survivor.finish_reason = Some(*finish);
        }
    }
}
