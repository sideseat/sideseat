//! A positional overlay: an indexed entry improved by the richer copy of the same message another carrier holds.

use serde_json::Value as JsonValue;

use super::reading::canonical_index;
use super::*;
use crate::rules::schema::OverlaySpec;

/// Whether the entry is of a role the overlay's copy does not list (`unlisted_roles`): its role as the family
/// states it, read from the entry's raw members so an entry a requirement leaves out is counted too.
pub(super) fn unlisted(
    overlay: &OverlaySpec,
    members: &[(&str, &String)],
    entry_member: Option<&str>,
) -> bool {
    if overlay.unlisted_roles.is_empty() {
        return false;
    }
    let role_member = match entry_member {
        Some(nested) => format!("{nested}.role"),
        None => "role".to_string(),
    };
    members
        .iter()
        .find(|(member, _)| *member == role_member)
        .is_some_and(|(_, role)| {
            overlay
                .unlisted_roles
                .iter()
                .any(|unlisted| unlisted == *role)
        })
}

/// Apply the overlay to one entry of the family at `index`, one its copy lists: replace its flattened content with
/// the counterpart's, or put the counterpart's blocks before it. The counterpart is the copy's member at the entry's
/// position less the `unlisted_before` entries before it whose role the copy does not list.
pub(super) fn apply(
    object: &mut serde_json::Map<String, JsonValue>,
    overlay: &OverlaySpec,
    counterparts: Option<&Vec<JsonValue>>,
    index: usize,
    unlisted_before: usize,
    consumed: &mut Vec<String>,
) {
    let Some(index) = index.checked_sub(unlisted_before) else {
        return;
    };
    if let Some(content) = counterpart_content(overlay, counterparts, index)
        && object
            .keys()
            .any(|member| member.starts_with(overlay.when_member_prefix.as_str()))
    {
        let flattened: Vec<String> = object
            .keys()
            .filter(|member| member.starts_with(overlay.when_member_prefix.as_str()))
            .cloned()
            .collect();
        for member in flattened {
            object.remove(&member);
        }
        // The overlay's payload was read, so this entry owns it: a dialect joining its flattened family
        // against a serialised copy has consumed that copy, and leaving it unclaimed let the dialect
        // that reads it whole emit the same turn again.
        consumed.push(overlay.from.clone());
        object.insert(overlay.as_member.clone(), content);
    }
    // Blocks the counterpart puts before the flattened content, which is **kept**: a dialect that flattens
    // an answer but has no name for the reasoning that preceded it loses a kind of block rather than the
    // content, so replacing would drop the answer. The flattened members are renumbered after the
    // prepended ones, which leaves one content list in the order the provider sent it.
    if let Some(blocks) = counterpart_prepend(overlay, counterparts, index)
        && object
            .keys()
            .any(|member| member.starts_with(overlay.when_member_prefix.as_str()))
    {
        let prefix = overlay.when_member_prefix.as_str();
        // **Taken out, then put back renumbered.** Shifting in place needs an order no renaming can
        // safely have: moving highest-first is right numerically and wrong lexicographically, where
        // `contents.9` sorts above `contents.10` and overwrites it before it is read - a block lost for
        // any entry with ten or more of them. Removing every member first means no write can land on one
        // still to be moved, whatever order they come in.
        let moved: Vec<(String, JsonValue)> = object
            .keys()
            .filter(|member| member.starts_with(prefix))
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|member| {
                let value = object.remove(&member)?;
                let rest = member.strip_prefix(prefix)?;
                let (position, tail) = rest.split_once('.').unwrap_or((rest, ""));
                // A member under the prefix whose first segment is not an index is not a block of this
                // list; it keeps the name it had. Canonical, like the family's own indices: `01` renamed
                // to the same destination as `1` and one of them silently overwrote the other.
                let Some(at) = canonical_index(position) else {
                    return Some((member, value));
                };
                // Checked, not assumed: a crafted index near usize::MAX plus the prepended count would
                // panic in a debug build and wrap to 0 in a release one, where the prepended block then
                // overwrites it. A member whose index cannot be moved keeps the name it had, like one
                // whose first segment is not an index.
                let Some(at) = at.checked_add(blocks.len()) else {
                    return Some((member, value));
                };
                Some(match tail.is_empty() {
                    true => (format!("{prefix}{at}"), value),
                    false => (format!("{prefix}{at}.{tail}"), value),
                })
            })
            .collect();
        for (member, value) in moved {
            object.insert(member, value);
        }
        for (at, block) in blocks.into_iter().enumerate() {
            object.insert(format!("{prefix}{at}"), block);
        }
        // The overlay's payload was read, so this entry owns it, exactly as a replacing overlay does.
        consumed.push(overlay.from.clone());
    }
}

/// The content the counterpart at this position holds, where it is an improvement on the flattened form.
fn counterpart_content(
    overlay: &OverlaySpec,
    counterparts: Option<&Vec<JsonValue>>,
    index: usize,
) -> Option<JsonValue> {
    let found = overlay
        .content_any_of
        .iter()
        .find_map(|path| singular(counterparts?.get(index)?, path, "overlay content_any_of"))?;
    predicates_hold(found, &overlay.require).then(|| found.clone())
}

/// The blocks the counterpart at this position puts **before** the flattened content, where it has any.
fn counterpart_prepend(
    overlay: &OverlaySpec,
    counterparts: Option<&Vec<JsonValue>>,
    index: usize,
) -> Option<Vec<JsonValue>> {
    let found = overlay
        .prepend_any_of
        .iter()
        .find_map(|path| singular(counterparts?.get(index)?, path, "overlay prepend_any_of"))?;
    let blocks = found.as_array()?;
    (!blocks.is_empty()).then(|| blocks.clone())
}
