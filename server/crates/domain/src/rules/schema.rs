//! Rule asset format and digesting.
//!
//! `sideseat-rule-assets` embeds `server/assets/rules/` as a directory, so adding a framework remains
//! a data-only change.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value as JsonValue;
pub use serde_json_path::JsonPath;

mod conditions;
mod content;
mod content_blocks;
mod detection;
mod message_emit;
mod message_read;
mod span_fields;

pub use conditions::*;
pub use content::*;
pub use content_blocks::*;
pub use detection::*;
pub use message_emit::*;
pub use message_read::*;
pub use span_fields::*;

/// One rule file's parsed contents.
///
/// Every section is optional: a framework that only needs to declare its carriers says nothing about
/// messages, and a shared dialect fragment may declare carriers alone.
#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RuleFile {
    /// The editor schema this asset is written against. Document metadata: no section reads it, and the
    /// repository requires it to name the generated `rules.schema.json`.
    #[serde(rename = "$schema", default)]
    pub schema: Option<String>,
    /// Stable id for diagnostics and explain traces. Not a framework identity anything branches on.
    pub id: String,
    /// What this file is for, in prose. Surfaced by the explain trace, which is why documentation is a
    /// field rather than a comment.
    #[serde(default)]
    pub doc: Option<String>,
    /// Carrier semantics declarations.
    #[serde(default)]
    pub carriers: Vec<CarrierRule>,
    /// Detection signals. Produce a **label** and nothing else: no behaviour reads it.
    #[serde(default)]
    pub detect: Vec<DetectRule>,
    /// Which carriers an ingestion reads on this dialect's spans, and how each is parsed.
    #[serde(default)]
    pub messages: Vec<MessageRule>,
    /// Stored message rows that are producer bookkeeping rather than another conversation.
    ///
    /// Extraction remains lossless: these rules apply only when building the read-time SideML
    /// projection, so the raw span and its extracted message payload stay queryable.
    #[serde(default)]
    pub message_projections: Vec<MessageProjectionRule>,
    /// The events this dialect writes messages on.
    ///
    /// Recognition, not reading: an event named here is read, and one not named by any asset is ignored
    /// entirely. Declared because it is the same kind of fact as a carrier - which key a producer writes -
    /// and as a Rust list it meant a new `when_event` rule was a valid but *dead* declaration until
    /// somebody also edited the list.
    #[serde(default)]
    pub message_events: Vec<MessageEvent>,
    /// The log-record shapes that carry one of the `message_events`.
    ///
    /// Recognition, like `message_events`: a log record matching no declaration is stored as a log and
    /// nothing more, and one that matches is read as the span event of that name would be.
    #[serde(default)]
    pub log_events: Vec<LogEvent>,
    /// How a provider writes a tool *definition*, so the canonical shape is reached by declaration.
    #[serde(default)]
    pub tool_shapes: Vec<ToolShapeRule>,
    /// Attribute namespaces the **conventions** own, as opposed to a producer's own.
    ///
    /// Declared by the conventions' asset and nowhere else, which is **refused** rather than assumed: a
    /// declaration elsewhere is ignored, and a dialect could otherwise state that its own namespace is a
    /// convention and read as having done so. A key's first segment says who coined it, and the
    /// structural sweep that forbids a producer's key in production Rust needs to know which segments are not
    /// a producer's - `session.id` and `enduser.id` are OTel's and appear only in a shared fallback chain,
    /// exactly where `ai.usage.promptTokens` appears. Inferring it from *absence* - no dialect file mentions
    /// the namespace - was tried and is not evidence: a producer key declared only in a shared chain under a
    /// namespace nothing else names would be excused by it.
    #[serde(default)]
    pub convention_namespaces: Vec<String>,
    /// What role a message's source name implies, where the name decides it.
    #[serde(default)]
    pub event_roles: Vec<EventRole>,
    /// Which role spellings outrank the name a reading was found under. This engine's own vocabulary.
    #[serde(default)]
    pub role_authority: Vec<RoleAuthority>,
    /// Content-block shapes this dialect writes.
    #[serde(default)]
    pub content_blocks: Vec<ContentBlockRule>,
    /// A value a producer writes in `gen_ai.system` that names a **provider** the catalogue knows.
    ///
    /// Only the ones that are a *framework's* own name: a framework is not a provider, but one of them names
    /// itself in that attribute while its models are served by a provider the catalogue prices. Provider
    /// spellings proper (`azure_openai`, `amazon-bedrock`) stay in Rust, because those are the catalogue's
    /// vocabulary rather than any framework's - and this table is consulted before them, so a framework's claim
    /// about itself never has to be spelled as if it were a provider's.
    #[serde(default)]
    pub provider_aliases: Vec<ProviderAlias>,
    /// What each spelling of a finish reason means, in this engine's finish categories.
    #[serde(default)]
    pub finish_reasons: Vec<FinishReasonSpellings>,
    /// The shape of a call id a producer builds itself when the provider gave none, and which names the tool.
    #[serde(default)]
    pub synthetic_call_ids: Vec<SyntheticCallId>,
    /// What an event no convention names is, by a word its name contains.
    #[serde(default)]
    pub event_categories: Vec<EventCategoryRule>,
    /// Member names a producer uses, and what each one's presence means.
    ///
    /// Three questions about one vocabulary, which is why they are one section: which member holds a message's
    /// content (ordered - the first present one wins), which members mean a value is *message-shaped* rather
    /// than bare data, and which mean it is a content **block**. A member usually answers more than one, and as
    /// three lists in Rust they drifted: `contents` held content and said "message-shaped", while `toolCallId`
    /// said "content block" only.
    #[serde(default)]
    pub message_members: Vec<MessageMemberRule>,
    /// Which broad category a span falls in, as ordered first-match rules.
    ///
    /// A separate question from the observation type and with its own precedence: a transport call is an HTTP
    /// span *and* a plain observation, and one dialect's operation names name an agent here where the
    /// conventions leave them unclassified there.
    #[serde(default)]
    pub span_categories: Vec<ClassifyRule>,
    /// What kind of observation a span is, as ordered first-match rules.
    ///
    /// Ordered because the answer is a *precedence*, not a set of independent facts: a transport attribute
    /// makes a span a plain span whatever else it carries, and a conventional operation name outranks a
    /// dialect's own span-kind attribute. The rank is the whole of that knowledge, so it is data.
    #[serde(default)]
    pub observation_types: Vec<ClassifyRule>,
    /// Facts about a *span* this dialect can establish, as opposed to about a carrier.
    ///
    /// "Is this a tool execution" is one question with several answers - an operation name, a span-kind
    /// attribute, a pair of attributes that only appear together - and each dialect knows its own. The
    /// union answers it, so a dialect declares its signal rather than the code carrying a list of them.
    #[serde(default)]
    pub span_facts: Vec<SpanFactRule>,
    /// Named reading tables other rules may apply.
    ///
    /// One dialect's message shapes are recognised at four different selection points - the node itself, a
    /// `messages` list, every member of a state object, and a nested state object - and a table repeated
    /// per point is four places to fix a shape. Referenced as `<file id>.<name>`, resolved at compile time
    /// by inlining, and a fragment's own cases may **not** reference a fragment: one level, no recursion,
    /// nothing to bound at runtime.
    #[serde(default)]
    pub fragments: BTreeMap<String, Fragment>,
    /// The slugs an SDK may write into `sideseat.framework` for this framework, and the label they
    /// resolve to.
    ///
    /// Separate from `detect` because a declaration is evidence about the *process*, not about a span,
    /// and is consulted only after every signal has failed. Provider slugs (`bedrock`, `openai`) belong
    /// to no framework file, which is how they keep resolving to nothing.
    #[serde(default)]
    pub sdk_slugs: Vec<SdkSlug>,
    /// Which keys carry a *span field* - a scalar or list on the stored span, as opposed to a message.
    ///
    /// The same kind of fact as a carrier, at a different granularity: `gen_ai.usage.input_tokens`,
    /// `ai.usage.promptTokens` and a bare `input_tokens` are three spellings of one number, and as an
    /// ordered `&[&str]` in Rust they were a list of frameworks the code had to know.
    #[serde(default)]
    pub span_fields: Vec<SpanFieldRule>,
}

impl RuleFile {
    /// Why this file's clause declarations could not mean what they say.
    ///
    /// **Production validation**, not a repository test. The six clause types are compiled by three different
    /// modules, and the uniqueness rule is one property about all of them - so it lived in a test over the
    /// embedded corpus, and the generic compiler accepted two clauses sharing an id. That is fine while the
    /// only assets are the ones in this tree and a test guards them, and it is a hole the moment anything else
    /// loads a file: two clauses with the same provenance path, which is precisely what the ids exist to
    /// prevent. The test now calls this rather than restating it.
    ///
    /// Two rules, and both are about a declaration that cannot take effect:
    ///
    /// - a clause id must be non-empty and unique **within its owner** - a top-level rule, or a named fragment;
    /// - `convention_namespaces` may only be declared by the conventions' own asset, since it decides which
    ///   telemetry namespaces are not any producer's. Elsewhere it parsed and was ignored, so a dialect could
    ///   state that its own namespace is a convention and read as having done so.
    pub fn declaration_defect(&self) -> Option<String> {
        // The same rule as `convention_namespaces`, one section over: role authority is *this engine's* vocabulary
        // - which spellings outrank the name a reading was found under - and it compiles into one global plan. So
        // a producer's asset could add an authoritative spelling and change role resolution for every unrelated
        // producer, which is not a statement that asset is entitled to make.
        if !self.role_authority.is_empty() && self.id != ROLE_AUTHORITY_ASSET {
            return Some(format!(
                "`{}` declares `role_authority`, which only `{ROLE_AUTHORITY_ASSET}` may do - it decides which \
                 stated roles outrank the name a reading was found under, for every producer, and a producer's \
                 asset is not entitled to change that for the others",
                self.id
            ));
        }
        // The same rule again: what a finish-reason spelling *means* is one global table, so a producer's asset
        // declaring a spelling would reinterpret every other producer's spans that use it. A producer states
        // where its finish reason is written, through `span_fields`; the meaning of the word is not its to say.
        if !self.finish_reasons.is_empty() && self.id != FINISH_REASONS_ASSET {
            return Some(format!(
                "`{}` declares `finish_reasons`, which only `{FINISH_REASONS_ASSET}` may do - what a spelling \
                 means is one table for every producer",
                self.id
            ));
        }
        if !self.convention_namespaces.is_empty() && self.id != CONVENTIONS_ASSET {
            return Some(format!(
                "`{}` declares `convention_namespaces`, which only `{CONVENTIONS_ASSET}` may do - it decides \
                 which namespaces are no producer's, and elsewhere the declaration is read by nothing",
                self.id
            ));
        }
        for (owner, clauses) in self.clause_ids() {
            let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
            for id in &clauses {
                if id.is_empty() {
                    return Some(format!(
                        "`{}`: a clause of `{owner}` declares an empty id, which names nothing",
                        self.id
                    ));
                }
                if !seen.insert(id.as_str()) {
                    return Some(format!(
                        "`{}`: two clauses of `{owner}` share the id `{id}`, so a diagnostic naming it is \
                         ambiguous exactly where it is read",
                        self.id
                    ));
                }
            }
        }
        None
    }

    /// Every clause id, grouped by the owner whose id space it belongs to.
    ///
    /// Walked over the typed tree, so a clause type that gains a nesting is covered by construction rather
    /// than by remembering to extend a list of member names.
    pub fn clause_ids(&self) -> Vec<(String, Vec<String>)> {
        // **Exhaustiveness, and nothing else.** The walk below names the sections whose clauses carry ids this
        // rule covers; a section added to `RuleFile` was under no obligation to appear in it, so a new
        // clause-bearing section would deserialize and compile with no empty-or-duplicate id check at all. This
        // destructure has no `..`, so adding a field fails to compile here until its author decides. Every binding
        // is discarded - the walk reads `self`.
        {
            let Self {
                schema: _,
                id: _,
                doc: _,
                carriers: _,
                detect: _,
                messages: _,
                message_projections: _,
                message_events: _,
                log_events: _,
                tool_shapes: _,
                convention_namespaces: _,
                event_roles: _,
                role_authority: _,
                content_blocks: _,
                provider_aliases: _,
                finish_reasons: _,
                synthetic_call_ids: _,
                event_categories: _,
                message_members: _,
                span_categories: _,
                observation_types: _,
                span_facts: _,
                fragments: _,
                sdk_slugs: _,
                span_fields: _,
            } = self;
        }
        fn from_alternatives(alternatives: &[Alternative], out: &mut Vec<String>) {
            for alternative in alternatives {
                out.push(alternative.id.clone());
                from_alternatives(&alternative.extra_cases, out);
            }
        }
        fn from_message(rule: &MessageRule, out: &mut Vec<String>) {
            from_alternatives(&rule.alternatives, out);
            from_alternatives(&rule.also, out);
            from_alternatives(&rule.fallback, out);
            if let Some(elements) = &rule.elements {
                for pass in &elements.passes {
                    out.push(pass.id.clone());
                    if let Some(group) = &pass.group {
                        for case in &group.by {
                            out.push(case.id.clone());
                        }
                    }
                }
            }
            if let Some(sections) = &rule.sections {
                for route in &sections.routes {
                    out.push(route.id.clone());
                }
            }
            if let Some(branches) = &rule.branch_set {
                for leaf in branches
                    .primary
                    .iter()
                    .chain(&branches.fallback_if_primary_empty)
                    .chain(&branches.always)
                {
                    // A branch leaf is a rule of its own, so its clauses belong to *its* id space.
                    from_message(leaf, out);
                }
            }
        }

        let mut out: Vec<(String, Vec<String>)> = Vec::new();
        for rule in &self.messages {
            let mut ids = Vec::new();
            from_message(rule, &mut ids);
            out.push((rule.id.clone(), ids));
        }
        out.push((
            "message_projections".to_string(),
            self.message_projections
                .iter()
                .map(|rule| rule.id.clone())
                .collect(),
        ));
        for (name, fragment) in &self.fragments {
            let mut ids = Vec::new();
            from_alternatives(&fragment.cases, &mut ids);
            out.push((name.clone(), ids));
        }
        for rule in &self.span_fields {
            out.push((
                rule.id.clone(),
                rule.sources
                    .iter()
                    .map(|source| source.id.clone())
                    .collect(),
            ));
        }
        // Both event registries, whose entries are clauses like any other: each answers a runtime
        // question, so each needs an identity a diagnostic can name. They are their own id spaces because
        // the two lists are different vocabularies - `message_events` says which OTLP events carry
        // messages, `event_roles` says what a *source name* implies, and a name may be in one and not the
        // other.
        out.push((
            "message_events".to_string(),
            self.message_events
                .iter()
                .map(|event| event.id.clone())
                .collect(),
        ));
        out.push((
            "log_events".to_string(),
            self.log_events
                .iter()
                .map(|event| event.id.clone())
                .collect(),
        ));
        out.push((
            "event_roles".to_string(),
            self.event_roles
                .iter()
                .map(|role| role.id.clone())
                .collect(),
        ));
        out.push((
            "finish_reasons".to_string(),
            self.finish_reasons
                .iter()
                .map(|entry| entry.id.clone())
                .collect(),
        ));
        out.push((
            "event_categories".to_string(),
            self.event_categories
                .iter()
                .map(|entry| entry.id.clone())
                .collect(),
        ));
        out.push((
            "synthetic_call_ids".to_string(),
            self.synthetic_call_ids
                .iter()
                .map(|entry| entry.id.clone())
                .collect(),
        ));
        out.push((
            "role_authority".to_string(),
            self.role_authority
                .iter()
                .map(|entry| entry.id.clone())
                .collect(),
        ));
        for rule in &self.span_facts {
            out.push((
                rule.id.clone(),
                rule.signals
                    .iter()
                    .map(|signal| signal.id.clone())
                    .collect(),
            ));
        }
        out
    }
}

/// The asset that owns the conventions' own vocabulary.
pub const CONVENTIONS_ASSET: &str = "semconv";

/// The one asset entitled to declare what a finish-reason spelling means.
pub const FINISH_REASONS_ASSET: &str = "finish-reasons";

/// The one asset entitled to declare role authority: it is the engine's own vocabulary, not any producer's.
pub const ROLE_AUTHORITY_ASSET: &str = "role-authority";
