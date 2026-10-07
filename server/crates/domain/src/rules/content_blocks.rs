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

use std::borrow::Cow;

use super::message_rules::{predicate_defect, predicates_hold, query};
use super::schema::{
    ChainPosition, ContentBlockRule, IdSource, MediaSource, MissingMediaType, ResultContent,
    RuleFile, TransformedSource, ValueSource,
};

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
        "content-block rules `{first}` and `{second}` share priority {priority} at the `{position}` position, so \
         which one answers a shape they both recognise depends on load order"
    )]
    SharedPriority {
        first: String,
        second: String,
        priority: i32,
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
    #[error("content-block rule `{rule}` has a source at `{path}` that {defect}")]
    Source {
        rule: String,
        path: String,
        defect: &'static str,
    },
}

/// The declared cases, in the order they are tried, split by where in the normalisation chain they sit.
///
/// The position is declared because the chain's order is load-bearing: two dialects can write a block that
/// the other would also recognise, and which one answers is decided by who is asked first. Inferring the
/// position from a priority alone would silently re-order that: a failed unwrap ends only its own position, and
/// a splice is tried over the envelopes alone, so each position is an arena of its own.
#[derive(Debug, Default)]
pub struct ContentBlockPlan {
    /// Consulted only by the message-content chain; see `ChainPosition::MessageEnvelope`.
    envelopes: Vec<ContentBlockRule>,
    before: Vec<ContentBlockRule>,
    providers: Vec<ContentBlockRule>,
    after: Vec<ContentBlockRule>,
}

impl ContentBlockPlan {
    pub fn compile(files: &[RuleFile]) -> Result<Self, ContentBlockCompileError> {
        let mut plan = Self::default();
        let mut all: Vec<&ContentBlockRule> =
            files.iter().flat_map(|f| &f.content_blocks).collect();
        all.sort_by_key(|rule| rule.priority);
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
                + usize::from(rule.splice.is_some())
                + usize::from(rule.refusal.is_some())
                + usize::from(rule.redacted_thinking.is_some())
                + usize::from(rule.unknown.is_some());
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
                r if r.refusal.as_ref().is_some_and(|t| t.message.is_empty()) => {
                    Some("refusal.message")
                }
                // A media type is required only where nothing else can supply one: no default, and a missing
                // one declines the case.
                r if r.media.as_ref().is_some_and(|m| {
                    m.data.is_empty()
                        || (m.media_type.is_empty()
                            && m.media_type_default.is_none()
                            && m.missing_media_type == MissingMediaType::Decline)
                }) =>
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
            // So do a withheld reasoning payload and a kept-whole unknown block, for the same reason.
            let always_builds = rule.tool_result.is_some()
                || rule.json.is_some()
                || rule.thinking.is_some()
                || rule.redacted_thinking.is_some()
                || rule.unknown.is_some();
            if always_builds && rule.require.is_empty() {
                return Err(ContentBlockCompileError::NoCondition {
                    rule: rule.id.clone(),
                });
            }
            // A content selector that names the block *itself* re-enters this plan through the tool-result
            // normaliser, with the same value - unbounded recursion, admitted into a DSL whose whole point is
            // that it cannot loop.
            if let Some(spec) = &rule.tool_result
                && spec
                    .content
                    .iter()
                    .any(|source| source_path(source).to_string() == "$")
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
                    ChainPosition::ProviderFormats => Some("provider_formats"),
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
            for source in rule_sources(rule) {
                if let ValueSource::Transformed(spec) = source
                    && let Some(defect) = transform_defect(spec)
                {
                    return Err(ContentBlockCompileError::Source {
                        rule: rule.id.clone(),
                        path: spec.path.to_string(),
                        defect,
                    });
                }
            }
        }
        for rule in all {
            match rule.at {
                ChainPosition::MessageEnvelope => plan.envelopes.push(rule.clone()),
                ChainPosition::BeforeProviderFormats => plan.before.push(rule.clone()),
                ChainPosition::ProviderFormats => plan.providers.push(rule.clone()),
                ChainPosition::AfterProviderFormats => plan.after.push(rule.clone()),
            }
        }
        // A **shared priority within one chain position** is refused: the chain's order decides which dialect
        // answers for a shape more than one of them recognises, so two cases at the same priority would be
        // resolved by whichever asset loaded first. Across positions a priority means nothing - one runs before
        // the provider formats and the other after - so each position is its own arena.
        for (position, rules) in [
            ("message_envelope", &plan.envelopes),
            ("before", &plan.before),
            ("provider_formats", &plan.providers),
            ("after", &plan.after),
        ] {
            if let Some((first, second)) =
                super::precedence::shared_priority(rules, |rule| rule.priority, |_, _| true)
            {
                return Err(ContentBlockCompileError::SharedPriority {
                    first: first.id.clone(),
                    second: second.id.clone(),
                    priority: first.priority,
                    position,
                });
            }
        }
        Ok(plan)
    }

    /// The first declared case at this position that recognises the block.
    pub fn normalize(&self, block: &JsonValue, at: ChainPosition) -> Option<JsonValue> {
        let cases = match at {
            ChainPosition::MessageEnvelope => &self.envelopes,
            ChainPosition::BeforeProviderFormats => &self.before,
            ChainPosition::ProviderFormats => &self.providers,
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
        self.before.len() + self.providers.len() + self.after.len()
    }
}

/// The first source that resolves to something this member accepts.
///
/// `empty_object_is_absent` is the one non-obvious rule and it is a producer fact: a dialect writes a
/// call's arguments under one member and, in an older version, under another - and the unused one is
/// present as `{}` rather than missing, so "the first that resolves" would always pick the empty one.
fn member<'b>(
    block: &'b JsonValue,
    sources: &[ValueSource],
    empty_object_is_absent: bool,
) -> Option<Cow<'b, JsonValue>> {
    sources.iter().find_map(|source| {
        let found = match source {
            ValueSource::Path(path) => query(block, path).into_iter().next().map(Cow::Borrowed),
            ValueSource::Transformed(spec) => transformed(block, spec),
        }?;
        (!(empty_object_is_absent && found.as_object().is_some_and(serde_json::Map::is_empty)))
            .then_some(found)
    })
}

/// A source's value after its one transform, or nothing where the transform cannot apply.
fn transformed<'b>(block: &'b JsonValue, spec: &TransformedSource) -> Option<Cow<'b, JsonValue>> {
    if let Some(separator) = &spec.join {
        let texts: Vec<&str> = query(block, &spec.path)
            .into_iter()
            .filter_map(JsonValue::as_str)
            .collect();
        return (!texts.is_empty()).then(|| Cow::Owned(JsonValue::String(texts.join(separator))));
    }
    let found = query(block, &spec.path).into_iter().next()?;
    if let Some(mode) = spec.parse {
        let Some(text) = found.as_str() else {
            return Some(Cow::Borrowed(found));
        };
        return super::message_rules::parse_value(text, mode).map(Cow::Owned);
    }
    if let Some(prefix) = &spec.prepend {
        return found
            .as_str()
            .map(|text| Cow::Owned(JsonValue::String(format!("{prefix}{text}"))));
    }
    if let Some(table) = &spec.map {
        return found
            .as_str()
            .and_then(|text| table.get(text))
            .map(|mapped| Cow::Owned(mapped.clone()));
    }
    None
}

/// The path a source reads, whatever it then does with the value.
fn source_path(source: &ValueSource) -> &super::schema::JsonPath {
    match source {
        ValueSource::Path(path) => path,
        ValueSource::Transformed(spec) => &spec.path,
    }
}

/// Why a transformed source cannot mean what it says.
fn transform_defect(spec: &TransformedSource) -> Option<&'static str> {
    let transforms = usize::from(spec.join.is_some())
        + usize::from(spec.parse.is_some())
        + usize::from(spec.prepend.is_some())
        + usize::from(spec.map.is_some());
    match transforms {
        0 => Some("names no transform; a plain path is written as a string"),
        1 if spec.prepend.as_deref() == Some("") => {
            Some("prepends nothing, which is the plain path written longhand")
        }
        1 if spec.map.as_ref().is_some_and(|table| table.is_empty()) => {
            Some("maps through an empty table, so it can never yield")
        }
        1 => None,
        _ => Some("names more than one transform; each source applies exactly one"),
    }
}

/// Every value source a rule declares, in any form.
fn rule_sources(rule: &ContentBlockRule) -> Vec<&ValueSource> {
    let mut out: Vec<&ValueSource> = Vec::new();
    if let Some(spec) = &rule.tool_use {
        out.extend(spec.name.iter().chain(&spec.input));
    }
    if let Some(spec) = &rule.tool_result {
        out.extend(
            spec.tool_use_id
                .iter()
                .chain(&spec.name)
                .chain(&spec.content)
                .chain(&spec.is_error),
        );
    }
    if let Some(spec) = &rule.json {
        out.extend(&spec.data);
    }
    if let Some(spec) = &rule.text {
        out.extend(&spec.text);
    }
    if let Some(spec) = &rule.thinking {
        out.extend(spec.text.iter().chain(&spec.signature));
    }
    if let Some(spec) = &rule.refusal {
        out.extend(&spec.message);
    }
    if let Some(spec) = &rule.redacted_thinking {
        out.extend(&spec.data);
    }
    if let Some(spec) = &rule.media {
        out.extend(
            spec.media_type
                .iter()
                .chain(&spec.data)
                .chain(&spec.name)
                .chain(&spec.detail),
        );
    }
    out
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
        let name = member(block, &spec.name, false)?;
        let name = name.as_str()?;
        let input = member(block, &spec.input, true)
            .map(Cow::into_owned)
            .unwrap_or_else(|| json!({}));
        let id = call_id(block, &spec.id, name, &input);
        return Some(json!({"type": "tool_use", "id": id, "name": name, "input": input}));
    }
    if let Some(spec) = &rule.tool_result {
        let tool_use_id = member(block, &spec.tool_use_id, false);
        let name = member(block, &spec.name, false);
        let selected = member(block, &spec.content, false).map(Cow::into_owned);
        let content = match spec.content_as {
            ResultContent::Normalized => {
                crate::sideml::content::normalize_tool_result_content(selected)
            }
            ResultContent::Value => {
                match crate::sideml::content::normalize_tool_result_content(selected) {
                    JsonValue::Array(blocks) => {
                        crate::sideml::content::create_inner_content(&blocks)
                    }
                    other => other,
                }
            }
            ResultContent::Blocks => result_blocks(selected),
        };
        let is_error = member(block, &spec.is_error, false)
            .and_then(|flag| flag.as_bool())
            .unwrap_or(false);
        let mut result = serde_json::Map::new();
        result.insert("type".to_string(), json!("tool_result"));
        result.insert(
            "tool_use_id".to_string(),
            json!(tool_use_id.as_deref().and_then(JsonValue::as_str)),
        );
        if let Some(name) = name.as_deref().and_then(JsonValue::as_str) {
            result.insert("name".to_string(), json!(name));
        }
        result.insert("content".to_string(), content);
        result.insert("is_error".to_string(), json!(is_error));
        return Some(JsonValue::Object(result));
    }
    if let Some(spec) = &rule.json {
        let data = member(block, &spec.data, false)
            .map(Cow::into_owned)
            .unwrap_or_else(|| json!({}));
        return Some(json!({"type": "json", "data": data}));
    }
    if let Some(spec) = &rule.text {
        // Only a string is text. A structured value under the same member is a different shape, and the
        // case must fall through to whatever recognises it rather than stringifying it here.
        let text = member(block, &spec.text, false)?;
        return Some(json!({"type": "text", "text": text.as_str()?}));
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
    if let Some(spec) = &rule.refusal {
        let message = member(block, &spec.message, false)?;
        return Some(json!({"type": "refusal", "message": message.as_str()?}));
    }
    if let Some(spec) = &rule.redacted_thinking {
        let data = member(block, &spec.data, false)
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        return Some(json!({"type": "redacted_thinking", "data": data}));
    }
    if rule.unknown.is_some() {
        return Some(json!({"type": "unknown", "raw": block.clone()}));
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
        return media_block(block, spec);
    }
    None
}

/// A media block, or nothing where the case cannot say what the bytes are.
fn media_block(block: &JsonValue, spec: &super::schema::MediaBlock) -> Option<JsonValue> {
    let data = member(block, &spec.data, false)?;
    let data = data.as_str()?;
    let declared: Option<String> = member(block, &spec.media_type, false)
        .and_then(|v| v.as_str().map(str::to_string))
        .or_else(|| spec.media_type_default.clone());
    let name = member(block, &spec.name, false).and_then(|v| v.as_str().map(str::to_string));
    let detail = member(block, &spec.detail, false).and_then(|v| v.as_str().map(str::to_string));
    let (source, referenced, payload): (&str, Option<&str>, &str) = match &spec.source {
        // Both derived, because both are facts about the bytes rather than about the producer: the kind
        // comes from the media type, and whether this is a reference or the content itself from the value.
        MediaSource::Decoded => {
            let (source, referenced) = crate::sideml::content::decode_media_source(data);
            // A data URL's header has been read for the media type; the block holds the payload alone.
            let payload = match data.split_once(',') {
                Some((header, payload)) if source == "base64" && header.starts_with("data:") => {
                    payload
                }
                _ => data,
            };
            (source, referenced, payload)
        }
        // The member's meaning is the format's: a stored reference is still recognised, because ingestion
        // replaces inline bytes with one, but nothing else about the value is second-guessed.
        MediaSource::ReferenceOr(otherwise) => {
            let source = if sideseat_core::utils::file_uri::is_file_uri(data) {
                "file"
            } else {
                otherwise.as_str()
            };
            (source, None, data)
        }
        MediaSource::Literal(source) => (source.as_str(), None, data),
    };
    // **One authority.** A stored reference carries the media type the bytes were stored under, and a
    // declared one beside it could disagree - `media_type: image/png` against `#!B64!#application/pdf::HASH`
    // produced an image block whose bytes a reader fetches as a PDF. The reference wins, because it is the
    // *stored* fact and what a fetch will return; the disagreement is reported rather than refused, since
    // refusing drops content over metadata.
    let media_type: Option<String> = match referenced {
        Some(stored) => {
            if let Some(declared) = declared.as_deref().filter(|declared| *declared != stored) {
                tracing::debug!(
                    target: "sideseat::rules",
                    declared,
                    stored,
                    "a media block's declared media type disagrees with its stored reference; the stored one \
                     is what a reader will fetch"
                );
            }
            Some(stored.to_string())
        }
        // The conventions make a blob's MIME type optional; without one, the bytes say what they are - asked
        // only where the value's shape is the authority, and only when a missing type would decline.
        None => declared.or_else(|| {
            (spec.source == MediaSource::Decoded
                && spec.missing_media_type == MissingMediaType::Decline)
                .then(|| sideseat_core::utils::mime::detect_mime_type_from_base64(data.as_bytes()))
                .flatten()
                .map(str::to_string)
        }),
    };
    if media_type.is_none() && spec.missing_media_type == MissingMediaType::Decline {
        return None;
    }
    let kind: &str = match spec.kind {
        Some(kind) => kind.into(),
        None => crate::sideml::content::mime_to_content_type(media_type.as_deref().unwrap_or("")),
    };
    let mut result = serde_json::Map::new();
    result.insert("type".to_string(), json!(kind));
    if !(media_type.is_none() && spec.missing_media_type == MissingMediaType::Omit) {
        result.insert("media_type".to_string(), json!(media_type));
    }
    result.insert("source".to_string(), json!(source));
    result.insert("data".to_string(), json!(payload));
    if let Some(name) = name {
        result.insert("name".to_string(), json!(name));
    }
    if let Some(detail) = detail {
        result.insert("detail".to_string(), json!(detail));
    }
    Some(JsonValue::Object(result))
}

/// What a tool returned, as a list of content blocks. See [`ResultContent::Blocks`].
fn result_blocks(selected: Option<JsonValue>) -> JsonValue {
    let Some(value) = selected else {
        return json!([]);
    };
    if let Some(blocks) = crate::sideml::content::try_normalize_python_constructor_content(&value) {
        return blocks;
    }
    match value {
        JsonValue::String(text) => json!([{"type": "text", "text": text}]),
        other => json!([{"type": "json", "data": other}]),
    }
}

#[cfg(test)]
#[path = "content_blocks_tests.rs"]
mod tests;
