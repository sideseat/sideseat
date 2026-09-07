//! Content-block shapes a dialect writes, declared rather than branched on.
//!
//! A block is a *canonical* target - `tool_use`, `tool_result`, `json`, `text`, or a media block - so this
//! module owns the constructors and a rule owns only the sources: which member holds the call id, which
//! holds the arguments, which spelling of "media type" this version used. That is the same split the
//! message rules make, and the reason is the same: the shapes are a producer's vocabulary and the targets
//! are ours.
//!
//! Nothing here names a framework.

use serde_json::{Value as JsonValue, json};

use super::message_rules::{predicates_hold, query};
use super::schema::{ChainPosition, ContentBlockRule, RuleFile};

/// The declared cases, in the order they are tried, split by where in the normalisation chain they sit.
///
/// The position is declared because the chain's order is load-bearing: two dialects can write a block that
/// the other would also recognise, and which one answers is decided by who is asked first. Inferring the
/// position from a rank alone would silently re-order that.
#[derive(Debug, Default)]
pub struct ContentBlockPlan {
    before: Vec<ContentBlockRule>,
    after: Vec<ContentBlockRule>,
}

impl ContentBlockPlan {
    pub fn compile(files: &[RuleFile]) -> Self {
        let mut plan = Self::default();
        let mut all: Vec<&ContentBlockRule> =
            files.iter().flat_map(|f| &f.content_blocks).collect();
        all.sort_by_key(|rule| rule.legacy_rank);
        for rule in all {
            match rule.at {
                ChainPosition::BeforeProviderFormats => plan.before.push(rule.clone()),
                ChainPosition::AfterProviderFormats => plan.after.push(rule.clone()),
            }
        }
        plan
    }

    /// The first declared case at this position that recognises the block.
    pub fn normalize(&self, block: &JsonValue, at: ChainPosition) -> Option<JsonValue> {
        let cases = match at {
            ChainPosition::BeforeProviderFormats => &self.before,
            ChainPosition::AfterProviderFormats => &self.after,
        };
        cases
            .iter()
            .filter(|rule| predicates_hold(block, &rule.require))
            .find_map(|rule| built(block, rule))
    }

    pub fn rule_count(&self) -> usize {
        self.before.len() + self.after.len()
    }
}

/// The first path that resolves to something this member accepts.
///
/// `empty_object_is_absent` is the one non-obvious rule and it is a producer fact: a dialect writes a
/// call's arguments under one member and, in an older version, under another - and the unused one is
/// present as `{}` rather than missing, so "the first that resolves" would always pick the empty one.
fn member<'b>(
    block: &'b JsonValue,
    paths: &[super::schema::JsonPath],
    empty_object_is_absent: bool,
) -> Option<&'b JsonValue> {
    paths.iter().find_map(|path| {
        query(block, path).into_iter().next().filter(|found| {
            !(empty_object_is_absent && found.as_object().is_some_and(serde_json::Map::is_empty))
        })
    })
}

fn built(block: &JsonValue, rule: &ContentBlockRule) -> Option<JsonValue> {
    if let Some(spec) = &rule.tool_use {
        // A nameless call names nothing to run, so the case does not recognise the block. The id may be
        // absent and is reported as null: a provider that omits it has still made the call.
        let name = member(block, &spec.name, false)?.as_str()?;
        let id = member(block, &spec.id, false).and_then(JsonValue::as_str);
        let input = member(block, &spec.input, true)
            .cloned()
            .unwrap_or_else(|| json!({}));
        return Some(json!({"type": "tool_use", "id": id, "name": name, "input": input}));
    }
    if let Some(spec) = &rule.tool_result {
        let tool_use_id = member(block, &spec.tool_use_id, false).and_then(JsonValue::as_str);
        let content = crate::domain::sideml::content::normalize_tool_result_content(
            member(block, &spec.content, false).cloned(),
        );
        let is_error = member(block, &spec.is_error, false)
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        return Some(json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id,
            "content": content,
            "is_error": is_error,
        }));
    }
    if let Some(spec) = &rule.json {
        let data = member(block, &spec.data, false)
            .cloned()
            .unwrap_or_else(|| json!({}));
        return Some(json!({"type": "json", "data": data}));
    }
    if let Some(spec) = &rule.text {
        // Only a string is text. A structured value under the same member is a different shape, and the
        // case must fall through to whatever recognises it rather than stringifying it here.
        let text = member(block, &spec.text, false)?.as_str()?;
        return Some(json!({"type": "text", "text": text}));
    }
    if let Some(spec) = &rule.media {
        let media_type = member(block, &spec.media_type, false)?.as_str()?;
        let data = member(block, &spec.data, false)?.as_str()?;
        // Both derived, because both are facts about the bytes rather than about the producer: the kind
        // comes from the media type, and whether this is a reference or the content itself from the value.
        return Some(json!({
            "type": crate::domain::sideml::content::mime_to_content_type(media_type),
            "media_type": media_type,
            "source": crate::domain::sideml::content::data_source_kind(data),
            "data": data,
        }));
    }
    None
}
