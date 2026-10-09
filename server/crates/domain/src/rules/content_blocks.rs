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
use std::collections::BTreeMap;

use super::message_rules::{predicate_defect, predicates_hold, query};

mod forms;
use super::schema::{
    ChainPosition, ContentBlockRule, IdSource, MissingMediaType, ResultContent, RuleFile,
    TransformedSource, ValueSource,
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
        "content-block rule `{rule}` maps its tool-result content to a literal that normalising never finishes \
         with: the chain answers it by re-entering itself until the depth bound"
    )]
    SelfRebuildingContent { rule: String },
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
    #[error(
        "content-block rule `{rule}` reads a provider-run item at `{position}`, but it answers with two blocks, \
         which only a message's own content can hold - declare it at `message_envelope`"
    )]
    RunOutsideMessageContent {
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

/// What a message content block stands for in its message's list.
pub enum Expansion<'b> {
    /// A splice: the members the block holds, each normalised in its place.
    Members(&'b Vec<JsonValue>),
    /// A provider run: the canonical blocks built from it.
    Built(Vec<JsonValue>),
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
                + usize::from(rule.provider_run.is_some())
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
                r if r.provider_run.as_ref().is_some_and(|t| t.name.is_empty()) => {
                    Some("provider_run.name")
                }
                r if r
                    .text
                    .as_ref()
                    .and_then(|t| t.citations.as_ref())
                    .is_some_and(|c| {
                        c.from.is_empty()
                            || c.cases.is_empty()
                            || c.cases.iter().any(|c| c.source.is_empty())
                    }) =>
                {
                    Some("text.citations.from, its cases, or a citation case's source")
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
            // was the wrong test - `json: {"data": "$.missing"}` resolves nothing and still emits
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
            // that it cannot loop. Only where the content is normalised: as `blocks` it is wrapped as data and
            // never offered to the chain again.
            let renormalised = |spec: &super::schema::ToolResultBlock| {
                !matches!(spec.content_as, super::schema::ResultContent::Blocks)
            };
            if let Some(spec) = &rule.tool_result
                && renormalised(spec)
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
            // A provider-run item answers with its call and result, which only a message's content list can hold.
            if rule.provider_run.is_some() {
                let position = match rule.at {
                    ChainPosition::MessageEnvelope => None,
                    ChainPosition::BeforeProviderFormats => Some("before_provider_formats"),
                    ChainPosition::ProviderFormats => Some("provider_formats"),
                    ChainPosition::AfterProviderFormats => Some("after_provider_formats"),
                };
                if let Some(position) = position {
                    return Err(ContentBlockCompileError::RunOutsideMessageContent {
                        rule: rule.id.clone(),
                        position,
                    });
                }
            }
            // A citation case's condition decides which shape of citation it reads, as a rule's does a block; one
            // without a condition reads every citation, so any case after it could never answer.
            let cases: Vec<_> = rule
                .text
                .iter()
                .flat_map(|t| &t.citations)
                .flat_map(|c| &c.cases)
                .collect();
            for (position, case) in cases.iter().enumerate() {
                let defect = predicate_defect(&case.require)
                    .map(str::to_string)
                    .or_else(|| {
                        (case.require.is_empty() && position + 1 != cases.len()).then(|| {
                            "a citation case with no `where` reads every citation, so a case after it \
                             never answers"
                                .to_string()
                        })
                    });
                if let Some(defect) = defect {
                    return Err(ContentBlockCompileError::Predicate {
                        rule: rule.id.clone(),
                        defect,
                    });
                }
            }
            let ids = rule
                .tool_use
                .iter()
                .map(|t| &t.id)
                .chain(rule.provider_run.iter().map(|r| &r.id));
            for ids in ids {
                for (position, source) in ids.iter().enumerate() {
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
                        Ok(_) if position + 1 != ids.len() => {
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
        if let Some(rule) = plan.first_endlessly_mapping_case() {
            return Err(ContentBlockCompileError::SelfRebuildingContent { rule });
        }
        Ok(plan)
    }

    /// The first case whose closed `map` hands its tool-result content a value that normalising never finishes
    /// with - a literal the chain answers by re-entering itself without end.
    ///
    /// Decided by **running** the assembled chain on every literal of the table, the one way a producer's value
    /// can reach it: the content's own normalisation is deterministic, so a literal either finishes or reaches
    /// `CONTENT_BLOCK_MAX_DEPTH`. A literal the case recognises is not enough - `outer` mapping to a block that
    /// selects `inner`, and `inner` to text, finishes after two steps - and which case answers a literal depends on
    /// every case before it, which only the assembled plan knows. A loop the telemetry itself drives, through a
    /// path rather than a literal, is bounded at run time instead.
    fn first_endlessly_mapping_case(&self) -> Option<String> {
        let cases = self
            .envelopes
            .iter()
            .chain(&self.before)
            .chain(&self.providers)
            .chain(&self.after);
        for rule in cases {
            let Some(spec) = &rule.tool_result else {
                continue;
            };
            if matches!(spec.content_as, super::schema::ResultContent::Blocks) {
                continue;
            }
            let literals = spec.content.iter().filter_map(|source| match source {
                ValueSource::Transformed(transformed) => transformed.map(),
                ValueSource::Path(_) => None,
            });
            for table in literals {
                for literal in table.values() {
                    let (_, bounded) = NormalisationDepth::observe(|| {
                        crate::sideml::content::normalize_tool_result_content_in(
                            self,
                            Some(literal.clone()),
                        )
                    });
                    if bounded {
                        return Some(rule.id.clone());
                    }
                }
            }
        }
        None
    }

    /// The first declared case at this position that recognises the block, read as a message's own block.
    pub fn normalize(&self, block: &JsonValue, at: ChainPosition) -> Option<JsonValue> {
        self.normalize_with(block, at, true)
    }

    /// The same, saying whether the block is a message's own (`consult_envelopes`) or a value a tool returned.
    /// What an unwrap finds inside is the same kind of value as the block around it, so it is normalised under
    /// the same answer: a returned value's member never reaches a message envelope either.
    pub fn normalize_with(
        &self,
        block: &JsonValue,
        at: ChainPosition,
        consult_envelopes: bool,
    ) -> Option<JsonValue> {
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
        //
        // **Bounded.** An unwrap and a normalised tool result re-enter this chain, and the telemetry decides how
        // deep: a loop through two cases, or a payload nested past any sense, is a client's to trigger. At the
        // bound nothing more is answered here, so the block degrades to what the chain's fallbacks make of it -
        // its raw form - and is reported rather than exhausting the stack.
        let Some(_depth) = NormalisationDepth::enter() else {
            tracing::warn!(
                target: "sideseat::rules",
                limit = sideseat_core::constants::CONTENT_BLOCK_MAX_DEPTH,
                "a content block nests past this server's normalisation depth; it is kept as it stands"
            );
            return None;
        };
        for rule in cases
            .iter()
            .filter(|rule| predicates_hold(block, &rule.require))
        {
            match built(self, block, rule, consult_envelopes) {
                Some(out) => return Some(out),
                None if rule.unwrap.is_some() => return None,
                None => continue,
            }
        }
        None
    }

    /// The blocks a message content block stands for, where a declared splice or provider run recognises it.
    ///
    /// Consulted before a message's content is normalised, block by block, and never by the single-block
    /// chain: there these forms answer nothing and the block is left to the cases after it.
    pub fn expand<'b>(&self, block: &'b JsonValue) -> Option<Expansion<'b>> {
        // The first envelope case that recognises the block decides, whatever its form, so neither form can
        // reach past a higher-ranked envelope that claims the same block.
        let rule = self
            .envelopes
            .iter()
            .find(|rule| predicates_hold(block, &rule.require))?;
        if let Some(spec) = &rule.provider_run {
            return forms::provider_run(block, spec).map(Expansion::Built);
        }
        let spec = rule.splice.as_ref()?;
        spec.from
            .iter()
            .find_map(|path| query(block, path).into_iter().next())?
            .as_array()
            .map(Expansion::Members)
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
    if let Some(separator) = spec.join() {
        let texts: Vec<&str> = query(block, &spec.path)
            .into_iter()
            .filter_map(JsonValue::as_str)
            .collect();
        return (!texts.is_empty()).then(|| Cow::Owned(JsonValue::String(texts.join(separator))));
    }
    let found = query(block, &spec.path).into_iter().next()?;
    if let Some(mode) = spec.parse() {
        let Some(text) = found.as_str() else {
            return Some(Cow::Borrowed(found));
        };
        return super::message_rules::parse_value(text, mode).map(Cow::Owned);
    }
    if let Some(prefix) = spec.prepend() {
        return found
            .as_str()
            .map(|text| Cow::Owned(JsonValue::String(format!("{prefix}{text}"))));
    }
    if let Some(table) = spec.map() {
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
    if spec.pipe.len() != 1 {
        return Some(if spec.pipe.is_empty() {
            "names no transform; a plain path is written as a string"
        } else {
            "names more than one transform; each source applies exactly one"
        });
    }
    if spec.prepend().is_some_and(String::is_empty) {
        return Some("prepends nothing, which is the plain path written longhand");
    }
    if spec.map().is_some_and(BTreeMap::is_empty) {
        return Some("maps through an empty table, so it can never yield");
    }
    if spec.join().is_none()
        && spec.parse().is_none()
        && spec.prepend().is_none()
        && spec.map().is_none()
    {
        return Some(
            "names a transform a content-block member cannot apply - `join`, `parse`, `prepend` or a \
             closed `map`",
        );
    }
    None
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
        for case in spec.citations.iter().flat_map(|c| &c.cases) {
            out.extend(
                case.source
                    .iter()
                    .chain(&case.title)
                    .chain(&case.text_start)
                    .chain(&case.text_end)
                    .chain(&case.cited_text),
            );
        }
    }
    if let Some(spec) = &rule.provider_run {
        out.extend(spec.name.iter().chain(&spec.input).chain(&spec.result));
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

fn built(
    plan: &ContentBlockPlan,
    block: &JsonValue,
    rule: &ContentBlockRule,
    consult_envelopes: bool,
) -> Option<JsonValue> {
    if let Some(spec) = &rule.tool_use {
        // A nameless call names nothing to run, so the case does not recognise the block. The id may be
        // absent and is reported as null: a provider that omits it has still made the call.
        let name = member(block, &spec.name, false)?;
        let name = name.as_str()?;
        let input = member(block, &spec.input, true)
            .map(Cow::into_owned)
            .unwrap_or_else(|| json!({}));
        let id = call_id(block, &spec.id, name, &input);
        let mut call = json!({"type": "tool_use", "id": id, "name": name, "input": input});
        if spec.provider_executed {
            call["provider_executed"] = json!(true);
        }
        return Some(call);
    }
    if let Some(spec) = &rule.tool_result {
        let tool_use_id = member(block, &spec.tool_use_id, false);
        let name = member(block, &spec.name, false);
        let selected = member(block, &spec.content, false).map(Cow::into_owned);
        let content = match spec.content_as {
            ResultContent::Normalized => {
                crate::sideml::content::normalize_tool_result_content_in(plan, selected)
            }
            ResultContent::Value => {
                match crate::sideml::content::normalize_tool_result_content_in(plan, selected) {
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
        if spec.provider_executed {
            result.insert("provider_executed".to_string(), json!(true));
        }
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
        let mut out = json!({"type": "text", "text": text.as_str()?});
        if let Some(citations) = &spec.citations {
            let cited = forms::citations(block, citations);
            if !cited.is_empty() {
                out["citations"] = JsonValue::Array(cited);
            }
        }
        return Some(out);
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
        // Through the same chain and plan as the block itself, and as the same kind of value: a member of a value a
        // tool returned is a returned value too, so the envelopes stay out of it.
        if spec.parse_json {
            let decoded: JsonValue = serde_json::from_str(inner.as_str()?).ok()?;
            return crate::sideml::content::normalize_block_in(plan, &decoded, consult_envelopes);
        }
        return crate::sideml::content::normalize_block_in(plan, inner, consult_envelopes);
    }
    if let Some(spec) = &rule.media {
        return forms::media_block(block, spec);
    }
    None
}

/// How deeply content-block normalisation has re-entered itself on this thread: one level per
/// `ContentBlockPlan::normalize` call on the stack.
///
/// Per thread because the recursion is a synchronous call chain on one thread, so the count is the depth of the
/// stack it guards and nothing outside that chain reads it.
struct NormalisationDepth;

thread_local! {
    static NORMALISATION_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Whether a normalisation on this thread has been stopped at the bound since [`NormalisationDepth::observe`]
    /// last asked.
    static NORMALISATION_BOUNDED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

impl NormalisationDepth {
    /// One more level, or `None` at the bound.
    fn enter() -> Option<Self> {
        NORMALISATION_DEPTH.with(|depth| {
            // A guard exists only for a level that was counted: one built and dropped at the bound would
            // uncount a level it never entered, and the bound would never be reached.
            if depth.get() < sideseat_core::constants::CONTENT_BLOCK_MAX_DEPTH {
                depth.set(depth.get() + 1);
                Some(Self)
            } else {
                NORMALISATION_BOUNDED.with(|bounded| bounded.set(true));
                None
            }
        })
    }

    /// Run `normalise`, and say whether the bound stopped any of it. Nests: an outer observer still learns of a
    /// bound an inner one saw.
    fn observe<T>(normalise: impl FnOnce() -> T) -> (T, bool) {
        let outer = NORMALISATION_BOUNDED.with(|bounded| bounded.replace(false));
        let answer = normalise();
        let bounded = NORMALISATION_BOUNDED.with(|flag| {
            let inner = flag.get();
            flag.set(outer || inner);
            inner
        });
        (answer, bounded)
    }
}

impl Drop for NormalisationDepth {
    fn drop(&mut self) {
        NORMALISATION_DEPTH.with(|depth| depth.set(depth.get() - 1));
    }
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
