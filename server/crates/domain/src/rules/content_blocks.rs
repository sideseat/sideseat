//! Declared content-block shapes.
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
use super::schema::{ChainPosition, ContentBlockRule, IdSource, RuleFile};

/// Why the content-block rules would not compile.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContentBlockCompileError {
    #[error("content-block rule `{rule}` declares {forms} target forms; exactly one is required")]
    TargetForms { rule: String, forms: usize },
    #[error("content-block rule `{rule}`: {defect}")]
    Predicate { rule: String, defect: String },
    #[error(
        "content-block rule `{rule}` names no condition, and its form builds a block whether its selectors \
         resolve or not - so it would recognise every block and swallow the chain"
    )]
    NoCondition { rule: String },
    #[error(
        "content-block rule `{rule}` selects the whole block as its tool-result content, which re-enters this \
         plan with the same value"
    )]
    SelfSelectingContent { rule: String },
    #[error(
        "content-block rule `{rule}` unwraps nothing, so it recognises a block and answers with it unchanged - \
         which is the chain it is already in"
    )]
    UnwrapsNothing { rule: String },
    #[error(
        "content-block rule `{rule}` unwraps the whole block, which re-enters the chain with the same value"
    )]
    UnwrapsWholeBlock { rule: String },
    #[error(
        "content-block rule `{rule}` names no path for `{member}`, which is required - the case would recognise \
         a block and then build nothing"
    )]
    EmptyRequiredSelector { rule: String, member: &'static str },
    #[error(
        "content-block rules `{first}` and `{second}` share rank {rank} at the `{position}` position, so which \
         one answers a shape they both recognise depends on load order"
    )]
    SharedRank {
        first: String,
        second: String,
        rank: i32,
        position: &'static str,
    },
    #[error(
        "content-block rule `{rule}` splices at `{position}`, but only a message's own content is a list a \
         block can be spliced into - declare it at `message_envelope`"
    )]
    SpliceOutsideMessageContent {
        rule: String,
        position: &'static str,
    },
    #[error("content-block rule `{rule}` declares the id template `{template}`, which {defect}")]
    IdTemplate {
        rule: String,
        template: String,
        defect: &'static str,
    },
}

/// The declared cases, in the order they are tried, split by where in the normalisation chain they sit.
///
/// The position is declared because the chain's order is load-bearing: two dialects can write a block that
/// the other would also recognise, and which one answers is decided by who is asked first. Inferring the
/// position from a rank alone would silently re-order that.
#[derive(Debug, Default)]
pub struct ContentBlockPlan {
    /// Consulted only by the message-content chain; see `ChainPosition::MessageEnvelope`.
    envelopes: Vec<ContentBlockRule>,
    before: Vec<ContentBlockRule>,
    after: Vec<ContentBlockRule>,
}

impl ContentBlockPlan {
    pub fn compile(files: &[RuleFile]) -> Result<Self, ContentBlockCompileError> {
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
                + usize::from(rule.media.is_some())
                + usize::from(rule.thinking.is_some())
                + usize::from(rule.unwrap.is_some())
                + usize::from(rule.splice.is_some());
            if forms != 1 {
                return Err(ContentBlockCompileError::TargetForms {
                    rule: rule.id.clone(),
                    forms,
                });
            }
            // The same predicate validation the message rules get. This plan compiles separately, and a
            // predicate that can never hold decides which shape a block is read as.
            if let Some(defect) = predicate_defect(&rule.require) {
                return Err(ContentBlockCompileError::Predicate {
                    rule: rule.id.clone(),
                    defect: defect.to_string(),
                });
            }
            // A required selector with no paths can never resolve, so the case recognises a block and then
            // refuses it - permanently dead, and it reads as though it builds something.
            let empty_required: Option<&'static str> = match rule {
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
            // `tool_result` and `json` **always** build something: every one of their members is optional,
            // so they return a block whether their selectors resolved or not. Emptiness of the selector list
            // was the wrong test - `json: {"data": ["$.missing"]}` resolves nothing and still emits
            // `{data: {}}` for every block it is offered. So the *condition* is what must identify the
            // shape, and these two forms cannot be declared without one.
            // `thinking` joins these two: its members are all optional, so it emits a block whether they
            // resolved or not, and without a condition it would claim every block it is offered.
            let always_builds =
                rule.tool_result.is_some() || rule.json.is_some() || rule.thinking.is_some();
            if always_builds && rule.require.all.is_empty() && rule.require.any.is_empty() {
                return Err(ContentBlockCompileError::NoCondition {
                    rule: rule.id.clone(),
                });
            }
            // A content selector that names the block *itself* re-enters this plan through the tool-result
            // normaliser, with the same value - unbounded recursion, admitted into a DSL whose whole point is
            // that it cannot loop.
            if let Some(spec) = &rule.tool_result
                && spec.content.iter().any(|path| path.to_string() == "$")
            {
                return Err(ContentBlockCompileError::SelfSelectingContent {
                    rule: rule.id.clone(),
                });
            }
            // The same bound for an unwrap, which re-enters the chain by construction: a member naming the
            // block itself would recurse forever, in a language whose whole point is that it cannot loop.
            if let Some(spec) = &rule.unwrap {
                if spec.from.is_empty() {
                    return Err(ContentBlockCompileError::UnwrapsNothing {
                        rule: rule.id.clone(),
                    });
                }
                if spec.from.iter().any(|path| path.to_string() == "$") {
                    return Err(ContentBlockCompileError::UnwrapsWholeBlock {
                        rule: rule.id.clone(),
                    });
                }
            }
            // A splice re-enters the chain once per member, so it has the unwrap's bounds - and it answers with
            // several blocks, which only a message's content list can hold.
            if let Some(spec) = &rule.splice {
                if spec.from.is_empty() {
                    return Err(ContentBlockCompileError::UnwrapsNothing {
                        rule: rule.id.clone(),
                    });
                }
                if spec.from.iter().any(|path| path.to_string() == "$") {
                    return Err(ContentBlockCompileError::UnwrapsWholeBlock {
                        rule: rule.id.clone(),
                    });
                }
                let position = match rule.at {
                    ChainPosition::MessageEnvelope => None,
                    ChainPosition::BeforeProviderFormats => Some("before_provider_formats"),
                    ChainPosition::AfterProviderFormats => Some("after_provider_formats"),
                };
                if let Some(position) = position {
                    return Err(ContentBlockCompileError::SpliceOutsideMessageContent {
                        rule: rule.id.clone(),
                        position,
                    });
                }
            }
            if let Some(spec) = &rule.tool_use {
                for (position, source) in spec.id.iter().enumerate() {
                    let IdSource::Template(template) = source else {
                        continue;
                    };
                    let defect = match template_segments(&template.template) {
                        Err(defect) => Some(defect),
                        Ok(segments)
                            if !segments.iter().any(|s| !matches!(s, Segment::Literal(_))) =>
                        {
                            Some("has no placeholder, so every call it builds would share one id")
                        }
                        // A template always yields, so a source after it could never answer.
                        Ok(_) if position + 1 != spec.id.len() => {
                            Some("is followed by another id source that it would always shadow")
                        }
                        Ok(_) => None,
                    };
                    if let Some(defect) = defect {
                        return Err(ContentBlockCompileError::IdTemplate {
                            rule: rule.id.clone(),
                            template: template.template.clone(),
                            defect,
                        });
                    }
                }
            }
            if let Some(member) = empty_required {
                return Err(ContentBlockCompileError::EmptyRequiredSelector {
                    rule: rule.id.clone(),
                    member,
                });
            }
        }
        for rule in all {
            match rule.at {
                ChainPosition::MessageEnvelope => plan.envelopes.push(rule.clone()),
                ChainPosition::BeforeProviderFormats => plan.before.push(rule.clone()),
                ChainPosition::AfterProviderFormats => plan.after.push(rule.clone()),
            }
        }
        // A **shared rank within one chain position** is refused: the chain's order decides which dialect
        // answers for a shape more than one of them recognises, so two cases at the same rank would be
        // resolved by whichever asset loaded first. Across positions a rank means nothing - one runs before
        // the provider formats and the other after - so they are checked apart.
        for (position, rules) in [
            ("message_envelope", &plan.envelopes),
            ("before", &plan.before),
            ("after", &plan.after),
        ] {
            for pair in rules.windows(2) {
                if pair[0].legacy_rank == pair[1].legacy_rank {
                    return Err(ContentBlockCompileError::SharedRank {
                        first: pair[0].id.clone(),
                        second: pair[1].id.clone(),
                        rank: pair[0].legacy_rank,
                        position,
                    });
                }
            }
        }
        Ok(plan)
    }

    /// The first declared case at this position that recognises the block.
    pub fn normalize(&self, block: &JsonValue, at: ChainPosition) -> Option<JsonValue> {
        let cases = match at {
            ChainPosition::MessageEnvelope => &self.envelopes,
            ChainPosition::BeforeProviderFormats => &self.before,
            ChainPosition::AfterProviderFormats => &self.after,
        };
        // First match wins for an **unwrap**, whether or not its member normalises. Its condition is that the
        // member is *there*, so a matching unwrap has claimed the block - and answering nothing then means "the
        // chain cannot read what was inside", which is the retired readers' behaviour and leaves the *original*
        // block to the rest of the chain rather than to a lower-ranked envelope. Without this, a wrapper holding
        // an empty string fell through to a reasoning member beside it and the block came back as reasoning.
        //
        // Every other form may fall through, which is equally deliberate: a `text` case whose member holds a
        // structure has not recognised prose, and something later reads that shape properly.
        for rule in cases
            .iter()
            .filter(|rule| predicates_hold(block, &rule.require))
        {
            match built(block, rule) {
                Some(out) => return Some(out),
                None if rule.unwrap.is_some() => return None,
                None => continue,
            }
        }
        None
    }

    /// The list of blocks a message content block stands for, where a declared splice recognises it.
    ///
    /// Consulted before a message's content is normalised, block by block, and never by the single-block
    /// chain: there a splice answers nothing and the block is left to the cases after it.
    pub fn splice<'b>(&self, block: &'b JsonValue) -> Option<&'b Vec<JsonValue>> {
        // The first envelope case that recognises the block decides, whatever its form, so a splice cannot
        // reach past a higher-ranked envelope that claims the same block.
        let spec = self
            .envelopes
            .iter()
            .find(|rule| predicates_hold(block, &rule.require))?
            .splice
            .as_ref()?;
        spec.from
            .iter()
            .find_map(|path| query(block, path).into_iter().next())?
            .as_array()
    }

    pub fn rule_count(&self) -> usize {
        self.envelopes.len() + self.rule_count_at_provider_positions()
    }

    fn rule_count_at_provider_positions(&self) -> usize {
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

/// The first source that states an id: a member holding a non-blank string, or a template, which always does.
fn call_id(
    block: &JsonValue,
    sources: &[IdSource],
    name: &str,
    input: &JsonValue,
) -> Option<String> {
    sources.iter().find_map(|source| match source {
        IdSource::Path(path) => query(block, path)
            .into_iter()
            .next()
            .and_then(JsonValue::as_str)
            .filter(|id| !id.trim().is_empty())
            .map(str::to_string),
        IdSource::Template(template) => {
            template_segments(&template.template).ok().map(|segments| {
                segments
                    .iter()
                    .map(|segment| match segment {
                        Segment::Literal(text) => text.clone(),
                        Segment::Name => name.to_string(),
                        Segment::InputHash => crate::sideml::content::compute_short_hash(input),
                    })
                    .collect()
            })
        }
    })
}

/// A piece of an id template.
enum Segment {
    Literal(String),
    Name,
    InputHash,
}

/// The template's pieces, or why it cannot be one: the placeholder set is closed, so a misspelt one is a
/// refusal rather than literal text in every id.
fn template_segments(template: &str) -> Result<Vec<Segment>, &'static str> {
    let mut segments = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find(['{', '}']) {
        if rest[open..].starts_with('}') {
            return Err("closes a placeholder it never opened");
        }
        if open > 0 {
            segments.push(Segment::Literal(rest[..open].to_string()));
        }
        let close = rest[open..]
            .find('}')
            .ok_or("opens a placeholder it never closes")?;
        segments.push(match &rest[open + 1..open + close] {
            "name" => Segment::Name,
            "stable_hash(input)" => Segment::InputHash,
            _ => return Err("names a placeholder other than `{name}` and `{stable_hash(input)}`"),
        });
        rest = &rest[open + close + 1..];
    }
    if !rest.is_empty() {
        segments.push(Segment::Literal(rest.to_string()));
    }
    Ok(segments)
}

fn built(block: &JsonValue, rule: &ContentBlockRule) -> Option<JsonValue> {
    if let Some(spec) = &rule.tool_use {
        // A nameless call names nothing to run, so the case does not recognise the block. The id may be
        // absent and is reported as null: a provider that omits it has still made the call.
        let name = member(block, &spec.name, false)?.as_str()?;
        let input = member(block, &spec.input, true)
            .cloned()
            .unwrap_or_else(|| json!({}));
        let id = call_id(block, &spec.id, name, &input);
        return Some(json!({"type": "tool_use", "id": id, "name": name, "input": input}));
    }
    if let Some(spec) = &rule.tool_result {
        let tool_use_id = member(block, &spec.tool_use_id, false).and_then(JsonValue::as_str);
        let name = member(block, &spec.name, false).and_then(JsonValue::as_str);
        let content = crate::sideml::content::normalize_tool_result_content(
            member(block, &spec.content, false).cloned(),
        );
        let is_error = member(block, &spec.is_error, false)
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        let mut result = serde_json::Map::new();
        result.insert("type".to_string(), json!("tool_result"));
        result.insert("tool_use_id".to_string(), json!(tool_use_id));
        if let Some(name) = name {
            result.insert("name".to_string(), json!(name));
        }
        result.insert("content".to_string(), content);
        result.insert("is_error".to_string(), json!(is_error));
        return Some(JsonValue::Object(result));
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
    if let Some(spec) = &rule.thinking {
        // Neither member is required: a producer that names the block as reasoning has said what it is, and the
        // retired reader emitted an empty one rather than letting something else claim it.
        let text = member(block, &spec.text, false)
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        let signature =
            member(block, &spec.signature, false).and_then(|v| v.as_str().map(str::to_string));
        return Some(json!({"type": "thinking", "text": text, "signature": signature}));
    }
    if let Some(spec) = &rule.unwrap {
        // The first member that is *present*, normalised in the block's place. Present, not resolvable to a
        // block: a wrapper whose content this chain cannot read leaves the **original** block to the rest of
        // the chain, which is what the retired readers did.
        let inner = spec
            .from
            .iter()
            .find_map(|path| super::message_rules::query(block, path).into_iter().next())?;
        if spec.parse_json {
            let decoded: JsonValue = serde_json::from_str(inner.as_str()?).ok()?;
            return crate::sideml::content::normalize_content_block(&decoded);
        }
        return crate::sideml::content::normalize_content_block(inner);
    }
    if let Some(spec) = &rule.media {
        let declared = member(block, &spec.media_type, false).and_then(JsonValue::as_str);
        let data = member(block, &spec.data, false)?.as_str()?;
        let name = member(block, &spec.name, false).and_then(JsonValue::as_str);
        // Both derived, because both are facts about the bytes rather than about the producer: the kind
        // comes from the media type, and whether this is a reference or the content itself from the value.
        let (source, referenced) = crate::sideml::content::decode_media_source(data);
        // **One authority.** A stored reference carries the media type the bytes were stored under, and a
        // declared one beside it could disagree - `media_type: image/png` against
        // `#!B64!#application/pdf::HASH` produced an image block whose bytes a reader fetches as a PDF. The
        // reference wins, because it is the *stored* fact and what a fetch will return; the disagreement is
        // reported rather than refused, since refusing drops content over metadata.
        let media_type = match referenced {
            Some(stored) if declared.is_some_and(|declared| declared != stored) => {
                tracing::debug!(
                    target: "sideseat::rules",
                    declared,
                    stored,
                    "a media block's declared media type disagrees with its stored reference; the stored one \
                     is what a reader will fetch"
                );
                stored
            }
            Some(stored) => stored,
            // The conventions make a blob's MIME type optional; without one, the bytes say what they are.
            None => declared.or_else(|| {
                sideseat_core::utils::mime::detect_mime_type_from_base64(data.as_bytes())
            })?,
        };
        // A data URL's header has been read for the media type; the block holds the payload alone.
        let data = match data.split_once(',') {
            Some((header, payload)) if source == "base64" && header.starts_with("data:") => payload,
            _ => data,
        };
        let mut result = json!({
            "type": crate::sideml::content::mime_to_content_type(media_type),
            "media_type": media_type,
            "source": source,
            "data": data,
        });
        if let Some(name) = name {
            result["name"] = json!(name);
        }
        return Some(result);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_from(rule: serde_json::Value) -> ContentBlockPlan {
        plan_from_all(vec![rule])
    }

    fn plan_from_all(rules: Vec<serde_json::Value>) -> ContentBlockPlan {
        compiled(rules).expect("the probe rules compile")
    }

    fn probe(rules: Vec<serde_json::Value>) -> RuleFile {
        serde_json::from_value(serde_json::json!({
            "id": "probe",
            "content_blocks": rules,
        }))
        .expect("the probe asset parses")
    }

    fn compiled(
        rules: Vec<serde_json::Value>,
    ) -> Result<ContentBlockPlan, ContentBlockCompileError> {
        ContentBlockPlan::compile(&[probe(rules)])
    }

    /// The refusal a lone rule meets, which must name `expected`.
    fn refused(rule: serde_json::Value, expected: &str) {
        refused_all(vec![rule], expected);
    }

    fn refused_all(rules: Vec<serde_json::Value>, expected: &str) {
        let error = compiled(rules).expect_err("the probe rules must be refused");
        assert!(
            error.to_string().contains(expected),
            "wrong refusal, expected `{expected}`: {error}"
        );
    }

    /// Two cases at one rank and one position: which answers a shape they both recognise would depend on
    /// load order, which is nobody's statement.
    #[test]
    fn two_cases_sharing_a_rank_at_one_position_are_refused() {
        refused_all(
            vec![
                serde_json::json!({
                    "id": "probe.a",
                    "at": "after_provider_formats",
                    "legacy_rank": 1,
                    "require": {"all": [{"path": "$.type", "one_of": ["text"]}]},
                    "text": {"text": ["$.value"]},
                }),
                serde_json::json!({
                    "id": "probe.b",
                    "at": "after_provider_formats",
                    "legacy_rank": 1,
                    "require": {"all": [{"path": "$.type", "one_of": ["prose"]}]},
                    "text": {"text": ["$.value"]},
                }),
            ],
            "share rank 1 at the `after` position",
        );
    }

    /// The same rank at *different* positions means nothing: one runs before the provider formats and the
    /// other after, so there is no contest to resolve.
    #[test]
    fn the_same_rank_at_different_positions_is_accepted() {
        let plan = plan_from_all(vec![
            serde_json::json!({
                "id": "probe.before",
                "at": "before_provider_formats",
                "legacy_rank": 1,
                "require": {"all": [{"path": "$.type", "one_of": ["text"]}]},
                "text": {"text": ["$.value"]},
            }),
            serde_json::json!({
                "id": "probe.after",
                "at": "after_provider_formats",
                "legacy_rank": 1,
                "require": {"all": [{"path": "$.type", "one_of": ["prose"]}]},
                "text": {"text": ["$.value"]},
            }),
        ]);
        assert_eq!(plan.rule_count(), 2);
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
    fn a_rule_with_no_target_form_is_refused() {
        refused(
            serde_json::json!({
                "id": "probe.nothing",
                "at": "after_provider_formats",
                "legacy_rank": 1,
            }),
            "declares 0 target forms",
        );
    }

    /// Two forms means `built` takes whichever it checks first, which is an order nobody declared.
    #[test]
    fn a_rule_with_two_target_forms_is_refused() {
        refused(
            serde_json::json!({
                "id": "probe.both",
                "at": "after_provider_formats",
                "legacy_rank": 1,
                "text": {"text": ["$.value"]},
                "json": {"data": ["$.value"]},
            }),
            "declares 2 target forms",
        );
    }

    /// A required selector with no paths can never resolve, so the case recognises a block and then builds
    /// nothing - dead, while reading as though it builds something.
    #[test]
    fn a_required_selector_with_no_paths_is_refused() {
        refused(
            serde_json::json!({
                "id": "probe.empty_text",
                "at": "after_provider_formats",
                "legacy_rank": 1,
                "text": {"text": []},
            }),
            "names no path for `text.text`",
        );
    }

    /// A form whose selectors are all empty always builds something, so with no condition it recognises
    /// every block and swallows the rest of the chain.
    #[test]
    fn a_rule_that_builds_from_nothing_is_refused() {
        refused(
            serde_json::json!({
                "id": "probe.catch_all",
                "at": "before_provider_formats",
                "legacy_rank": 1,
                "json": {},
            }),
            "swallow the chain",
        );
    }

    /// The same, with a selector that names something: it resolves nothing and still builds a block, which
    /// is why emptiness of the selector list was the wrong test.
    #[test]
    fn a_rule_whose_selector_may_resolve_nothing_still_needs_a_condition() {
        refused(
            serde_json::json!({
                "id": "probe.unresolved",
                "at": "before_provider_formats",
                "legacy_rank": 1,
                "json": {"data": ["$.missing"]},
            }),
            "swallow the chain",
        );
    }

    /// Selecting the block itself as tool-result content re-enters this plan with the same value.
    #[test]
    fn self_selecting_tool_result_content_is_refused() {
        refused(
            serde_json::json!({
                "id": "probe.recursive",
                "at": "after_provider_formats",
                "legacy_rank": 1,
                "require": {"all": [{"path": "$.type", "one_of": ["tool-result"]}]},
                "tool_result": {"content": ["$"]},
            }),
            "re-enters this plan",
        );
    }

    #[test]
    fn a_tool_result_keeps_its_declared_name() {
        let plan = plan_from(serde_json::json!({
            "id": "probe.named_result",
            "at": "before_provider_formats",
            "legacy_rank": 1,
            "require": {"all": [{"path": "$.result"}]},
            "tool_result": {
                "tool_use_id": ["$.id"],
                "name": ["$.name"],
                "content": ["$.result"]
            }
        }));

        assert_eq!(
            plan.normalize(
                &serde_json::json!({
                    "id": "call-1",
                    "name": "weather",
                    "result": "sunny"
                }),
                ChainPosition::BeforeProviderFormats,
            ),
            Some(serde_json::json!({
                "type": "tool_result",
                "tool_use_id": "call-1",
                "name": "weather",
                "content": "sunny",
                "is_error": false
            }))
        );
    }

    /// A condition that is a tautology is not a condition. `{}`, `{"path": "$"}` and `{"exists": true}` are
    /// the same statement about the root, which always exists - so requiring one recognises everything, and
    /// the "must name a condition" rule was bypassable by writing a syntactically non-empty one.
    #[test]
    fn a_tautological_condition_is_refused() {
        refused(
            serde_json::json!({
                "id": "probe.tautology",
                "at": "before_provider_formats",
                "legacy_rank": 1,
                "require": {"all": [{}]},
                "json": {},
            }),
            "tautology",
        );
    }

    /// The same statement written as an explicit root path.
    #[test]
    fn a_root_path_with_no_condition_is_refused() {
        refused(
            serde_json::json!({
                "id": "probe.root_path",
                "at": "before_provider_formats",
                "legacy_rank": 1,
                "require": {"all": [{"path": "$", "exists": true}]},
                "json": {},
            }),
            "tautology",
        );
    }

    /// And a *member* path with no conditions stays legal: it asserts the member is there.
    #[test]
    fn a_member_path_with_no_condition_is_legal() {
        let plan = plan_from(serde_json::json!({
            "id": "probe.member",
            "at": "before_provider_formats",
            "legacy_rank": 1,
            "require": {"all": [{"path": "$.value"}]},
            "json": {"data": ["$.value"]},
        }));
        assert_eq!(plan.rule_count(), 1);
    }

    /// A predicate that can never hold decides which shape a block is read as, so it is refused here too -
    /// this plan compiles separately from the message rules and had no validation at all.
    #[test]
    fn a_contradictory_predicate_is_refused() {
        refused(
            serde_json::json!({
                "id": "probe.contradiction",
                "at": "after_provider_formats",
                "legacy_rank": 1,
                "require": {"all": [{"path": "$.type", "kind": "number", "identifier_like": true}]},
                "text": {"text": ["$.value"]},
            }),
            "can never hold",
        );
    }

    /// A blob that names no media type is identified by its bytes.
    ///
    /// The conventions make a binary part's MIME type optional and Logfire's Anthropic instrumentation
    /// leaves it out, so an image a user sent was kept as an unknown block. A blob with neither a declared
    /// type nor recognisable bytes is still not media.
    #[test]
    fn an_undeclared_media_type_comes_from_the_bytes() {
        let plan = plan_from(serde_json::json!({
            "id": "probe.blob",
            "at": "after_provider_formats",
            "legacy_rank": 1,
            "require": {"all": [{"path": "$.content"}]},
            "media": {"media_type": ["$.mime_type"], "data": ["$.content"]},
        }));
        let normalize =
            |block: serde_json::Value| plan.normalize(&block, ChainPosition::AfterProviderFormats);

        let jpeg = normalize(serde_json::json!({"content": "/9j/4AAQSkZJRgABAQAAAQABAAD"}))
            .expect("JPEG bytes name their type");
        assert_eq!(jpeg["media_type"].as_str(), Some("image/jpeg"));
        assert_eq!(jpeg["type"].as_str(), Some("image"));
        assert!(normalize(serde_json::json!({"content": "aGVsbG8gd29ybGQgaGVsbG8"})).is_none());
    }

    /// A `data:` URI names its media type in its header and carries the payload after the comma; the
    /// block holds the payload alone, as one read from a base64 member would.
    #[test]
    fn a_data_uri_reads_as_its_media_type_and_payload() {
        let plan = plan_from(serde_json::json!({
            "id": "probe.uri",
            "at": "after_provider_formats",
            "legacy_rank": 1,
            "require": {"all": [{"path": "$.uri"}]},
            "media": {"media_type": ["$.mime_type"], "data": ["$.uri"]},
        }));

        let block = plan
            .normalize(
                &serde_json::json!({"uri": "data:image/jpeg;base64,/9j/4AAQSkZJRg"}),
                ChainPosition::AfterProviderFormats,
            )
            .expect("a data URI is media");

        assert_eq!(block["type"].as_str(), Some("image"));
        assert_eq!(block["media_type"].as_str(), Some("image/jpeg"));
        assert_eq!(block["data"].as_str(), Some("/9j/4AAQSkZJRg"));
    }

    /// A block serialised into another block's text is decoded and read as itself; text that is not JSON
    /// is left to the rest of the chain.
    #[test]
    fn an_unwrap_can_decode_a_serialised_block() {
        let plan = plan_from(serde_json::json!({
            "id": "probe.serialised",
            "at": "before_provider_formats",
            "legacy_rank": 1,
            "require": {"all": [{"path": "$.content", "starts_with": "{"}]},
            "unwrap": {"from": ["$.content"], "parse_json": true},
        }));
        let normalize = |content: &str| {
            plan.normalize(
                &serde_json::json!({"type": "text", "content": content}),
                ChainPosition::BeforeProviderFormats,
            )
        };

        let decoded = normalize(r#"{"type": "text", "text": "inside"}"#).expect("decodes");
        assert_eq!(decoded["text"].as_str(), Some("inside"));
        assert!(normalize("{not json").is_none());
    }

    fn splice_rule(at: &str) -> serde_json::Value {
        serde_json::json!({
            "id": "probe.splice",
            "at": at,
            "legacy_rank": 1,
            "require": {"all": [{"path": "$.type", "one_of": ["text"]}, {"path": "$.content", "kind": "array"}]},
            "splice": {"from": ["$.content"]},
        })
    }

    /// Only a message's content is a list a block can be spliced into; every other caller of the chain asks
    /// for one block, so a splice anywhere else could never answer.
    #[test]
    fn a_splice_outside_the_message_envelope_is_refused() {
        refused(
            splice_rule("before_provider_formats"),
            "splices at `before_provider_formats`",
        );
        refused(
            splice_rule("after_provider_formats"),
            "splices at `after_provider_formats`",
        );
        let mut whole = splice_rule("message_envelope");
        whole["splice"]["from"] = serde_json::json!(["$"]);
        refused(whole, "unwraps the whole block");
    }

    /// A splice answers with the member list, and never through the single-block chain - where it leaves the
    /// block to the cases after it rather than claiming it.
    #[test]
    fn a_splice_yields_its_member_list_and_nothing_to_the_single_block_chain() {
        let plan = plan_from(splice_rule("message_envelope"));
        let block =
            serde_json::json!({"type": "text", "content": [{"text": "a"}, {"toolUse": {}}]});
        assert_eq!(
            plan.splice(&block),
            Some(&vec![
                serde_json::json!({"text": "a"}),
                serde_json::json!({"toolUse": {}})
            ])
        );
        assert!(
            plan.normalize(&block, ChainPosition::MessageEnvelope)
                .is_none()
        );
        assert!(
            plan.splice(&serde_json::json!({"type": "text", "content": "prose"}))
                .is_none()
        );
    }

    /// A stored reference is the authority on its own media type.
    ///
    /// `media_type: image/png` beside `#!B64!#application/pdf::HASH` produced an *image* block whose bytes a
    /// reader fetches as a PDF - two statements about one datum, and the wrong one won because it was the
    /// declared one. The reference wins now: it is the stored fact and what a fetch returns.
    ///
    /// Reported rather than refused, deliberately. Refusing drops content over metadata, and the block is
    /// perfectly usable once the two agree about what it is.
    #[test]
    fn a_stored_reference_is_the_authority_on_its_media_type() {
        let plan = plan_from(serde_json::json!({
            "id": "probe.media",
            "at": "after_provider_formats",
            "legacy_rank": 1,
            "require": {"all": [{"path": "$.media_type"}, {"path": "$.data"}]},
            "media": {"media_type": ["$.media_type"], "data": ["$.data"]},
        }));
        let normalize = |media_type: &str, data: &str| {
            plan.normalize(
                &serde_json::json!({"media_type": media_type, "data": data}),
                ChainPosition::AfterProviderFormats,
            )
            .expect("the rule recognises the block")
        };

        // The declaration says image, while the stored reference identifies a PDF.
        let block = normalize("image/png", "#!B64!#application/pdf::abc123");
        assert_eq!(
            block["media_type"].as_str(),
            Some("application/pdf"),
            "the stored reference is what a reader will fetch"
        );
        assert_eq!(
            block["type"].as_str(),
            Some("document"),
            "and the block's kind follows the media type that won, or the two disagree again one level up"
        );
        assert_eq!(block["source"].as_str(), Some("file"));

        // Agreement is unremarkable, and inline bytes have only the declaration to go on.
        assert_eq!(
            normalize("image/png", "#!B64!#image/png::abc123")["media_type"].as_str(),
            Some("image/png")
        );
        let inline = normalize("image/png", "iVBORw0KGgo");
        assert_eq!(inline["media_type"].as_str(), Some("image/png"));
        assert_eq!(inline["source"].as_str(), Some("base64"));
        // And a URL is a fetch, which is the half of this that used to be called `base64`.
        assert_eq!(
            normalize("image/png", "https://example.com/a.png")["source"].as_str(),
            Some("url")
        );
    }
}
