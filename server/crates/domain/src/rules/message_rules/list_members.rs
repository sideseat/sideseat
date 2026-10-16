//! `ReadSpec::list_members`: an indexed entry's items written one key each, gathered into one list member.

use serde_json::Value as JsonValue;

use super::reading::canonical_index;
use crate::rules::schema::ListMembers;

/// The index of an item member - `<prefix><canonical index>`, nothing nested past it - relative to the entry or its
/// sub-level. Admission and gathering ask this one question, so an entry is admitted only for items it gathers.
fn item_index(list: &ListMembers, member: &str) -> Option<usize> {
    canonical_index(member.strip_prefix(list.prefix.as_str())?)
}

/// The member the gathered list becomes, named as `require_members` sees the entry's members, where the entry has
/// an item to gather.
pub(super) fn gathered_member(
    list: Option<&ListMembers>,
    members: &[(&str, &String)],
    entry_member: Option<&str>,
) -> Option<String> {
    let list = list?;
    let has_item = members.iter().any(|(member, _)| {
        let relative = entry_member
            .and_then(|nested| member.strip_prefix(nested)?.strip_prefix('.'))
            .unwrap_or(member);
        item_index(list, relative).is_some()
    });
    has_item.then(|| match entry_member {
        Some(nested) => format!("{nested}.{}", list.as_member),
        None => list.as_member.clone(),
    })
}

/// The entry's members with `name` among them, valueless: what a requirement asks is whether a member is there.
pub(super) fn with_member<'a>(
    members: &[(&'a str, &'a String)],
    name: &'a str,
) -> Vec<(&'a str, &'a String)> {
    static EMPTY: String = String::new();
    members
        .iter()
        .copied()
        .chain(std::iter::once((name, &EMPTY)))
        .collect()
}

/// Gather the entry's items into `as_member`, in index order - which a member's name order is not (`10` sorts
/// before `2`). An entry already stating the member is left as it is: two statements of one member are not this
/// shape. Each item that is an object and declares no `type` is given `item_type`, where the rule names one: the
/// block kind its rule knows it to be, stated by the rule that read it rather than guessed from its shape anywhere.
pub(super) fn gather(object: &mut serde_json::Map<String, JsonValue>, list: &ListMembers) {
    if object.contains_key(&list.as_member) {
        return;
    }
    let mut items: Vec<(usize, String)> = object
        .keys()
        .filter_map(|member| Some((item_index(list, member)?, member.clone())))
        .collect();
    if items.is_empty() {
        return;
    }
    items.sort_unstable();
    let gathered: Vec<JsonValue> = items
        .into_iter()
        .filter_map(|(_, member)| object.remove(&member))
        .map(|mut item| {
            if let (Some(kind), JsonValue::Object(fields)) = (&list.item_type, &mut item)
                && !fields.contains_key("type")
            {
                fields.insert("type".to_string(), JsonValue::String(kind.clone()));
            }
            item
        })
        .collect();
    object.insert(list.as_member.clone(), JsonValue::Array(gathered));
}
