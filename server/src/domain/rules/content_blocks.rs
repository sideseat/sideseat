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

use super::message_rules::{predicate_defect, predicates_hold, query};
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
        for rule in &all {
            // **Exactly one** target form. Zero means the rule recognises a block and builds nothing - and
            // with an empty `require` it recognises *every* block, so it would swallow the rest of the
            // chain. More than one means `built` silently takes whichever it checks first, which is an order
            // nobody declared.
            let forms = usize::from(rule.tool_use.is_some())
                + usize::from(rule.tool_result.is_some())
                + usize::from(rule.json.is_some())
                + usize::from(rule.text.is_some())
                + usize::from(rule.media.is_some());
            assert!(
                forms == 1,
                "content-block rule `{}` declares {forms} target forms; exactly one is required",
                rule.id
            );
            // The same predicate validation the message rules get. This plan compiles separately, and a
            // predicate that can never hold decides which shape a block is read as.
            if let Some(defect) = predicate_defect(&rule.require) {
                panic!("content-block rule `{}`: {defect}", rule.id);
            }
            // A required selector with no paths can never resolve, so the case recognises a block and then
            // refuses it - permanently dead, and it reads as though it builds something.
            let empty_required: Option<&str> = match rule {
                r if r.tool_use.as_ref().is_some_and(|t| t.name.is_empty()) => {
                    Some("tool_use.name")
                }
                r if r.text.as_ref().is_some_and(|t| t.text.is_empty()) => Some("text.text"),
                r if r
                    .media
                    .as_ref()
                    .is_some_and(|m| m.media_type.is_empty() || m.data.is_empty()) =>
                {
                    Some("media.media_type or media.data")
                }
                _ => None,
            };
            // A form whose selectors are all empty always builds *something*, so with an empty `require` it
            // recognises every block and swallows the rest of the chain. Empty selectors are defensible only
            // when a `require` identifies the shape.
            let builds_from_nothing = rule.tool_result.as_ref().is_some_and(|t| {
                t.tool_use_id.is_empty() && t.content.is_empty() && t.is_error.is_empty()
            }) || rule.json.as_ref().is_some_and(|j| j.data.is_empty());
            assert!(
                !(builds_from_nothing
                    && rule.require.all.is_empty()
                    && rule.require.any.is_empty()),
                "content-block rule `{}` names no selector and no condition, so it recognises every \
                 block and swallows the chain",
                rule.id
            );
            // A content selector that names the block *itself* re-enters this plan through the tool-result
            // normaliser, with the same value - unbounded recursion, admitted into a DSL whose whole point is
            // that it cannot loop.
            if let Some(spec) = &rule.tool_result {
                assert!(
                    !spec.content.iter().any(|path| path.to_string() == "$"),
                    "content-block rule `{}` selects the whole block as its tool-result content, which \
                     re-enters this plan with the same value",
                    rule.id
                );
            }
            assert!(
                empty_required.is_none(),
                "content-block rule `{}` names no path for `{}`, which is required - the case would \
                 recognise a block and then build nothing",
                rule.id,
                empty_required.unwrap_or_default()
            );
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_from(rule: serde_json::Value) -> ContentBlockPlan {
        let file: RuleFile = serde_json::from_value(serde_json::json!({
            "id": "probe",
            "content_blocks": [rule],
        }))
        .expect("the probe asset parses");
        ContentBlockPlan::compile(&[file])
    }

    /// One target form, and the reason it must be exactly one.
    #[test]
    fn a_rule_declares_exactly_one_target_form() {
        // One is fine.
        let plan = plan_from(serde_json::json!({
            "id": "probe.text",
            "at": "after_provider_formats",
            "legacy_rank": 1,
            "require": {"all": [{"path": "$.type", "one_of": ["text"]}]},
            "text": {"text": ["$.value"]},
        }));
        assert_eq!(plan.rule_count(), 1);
    }

    /// Zero forms recognises a block and builds nothing - with an empty `require`, *every* block.
    #[test]
    #[should_panic(expected = "declares 0 target forms")]
    fn a_rule_with_no_target_form_is_refused() {
        plan_from(serde_json::json!({
            "id": "probe.nothing",
            "at": "after_provider_formats",
            "legacy_rank": 1,
        }));
    }

    /// Two forms means `built` takes whichever it checks first, which is an order nobody declared.
    #[test]
    #[should_panic(expected = "declares 2 target forms")]
    fn a_rule_with_two_target_forms_is_refused() {
        plan_from(serde_json::json!({
            "id": "probe.both",
            "at": "after_provider_formats",
            "legacy_rank": 1,
            "text": {"text": ["$.value"]},
            "json": {"data": ["$.value"]},
        }));
    }

    /// A required selector with no paths can never resolve, so the case recognises a block and then builds
    /// nothing - dead, while reading as though it builds something.
    #[test]
    #[should_panic(expected = "names no path for `text.text`")]
    fn a_required_selector_with_no_paths_is_refused() {
        plan_from(serde_json::json!({
            "id": "probe.empty_text",
            "at": "after_provider_formats",
            "legacy_rank": 1,
            "text": {"text": []},
        }));
    }

    /// A form whose selectors are all empty always builds something, so with no condition it recognises
    /// every block and swallows the rest of the chain.
    #[test]
    #[should_panic(expected = "swallows the chain")]
    fn a_rule_that_builds_from_nothing_is_refused() {
        plan_from(serde_json::json!({
            "id": "probe.catch_all",
            "at": "before_provider_formats",
            "legacy_rank": 1,
            "json": {},
        }));
    }

    /// Selecting the block itself as tool-result content re-enters this plan with the same value.
    #[test]
    #[should_panic(expected = "re-enters this plan")]
    fn self_selecting_tool_result_content_is_refused() {
        plan_from(serde_json::json!({
            "id": "probe.recursive",
            "at": "after_provider_formats",
            "legacy_rank": 1,
            "require": {"all": [{"path": "$.type", "one_of": ["tool-result"]}]},
            "tool_result": {"content": ["$"]},
        }));
    }

    /// A predicate that can never hold decides which shape a block is read as, so it is refused here too -
    /// this plan compiles separately from the message rules and had no validation at all.
    #[test]
    #[should_panic(expected = "can never hold")]
    fn a_contradictory_predicate_is_refused() {
        plan_from(serde_json::json!({
            "id": "probe.contradiction",
            "at": "after_provider_formats",
            "legacy_rank": 1,
            "require": {"all": [{"path": "$.type", "kind": "number", "identifier_like": true}]},
            "text": {"text": ["$.value"]},
        }));
    }
}
