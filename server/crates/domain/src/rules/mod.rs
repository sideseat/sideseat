//! Framework knowledge as data, interpreted generically.
//!
//! Every fact about a *specific* framework or provider lives in a rule asset under `server/assets/rules/`,
//! not in this module. The engine knows how to match and how to compose; it knows no producer names,
//! carrier keys, tags or type mappings. The current architecture and its limits are recorded in
//! `docs/engineering/framework-rules-engine.md`.
//!
//! Two properties make that checkable rather than merely intended:
//!
//! - **No behavioural API accepts a framework label.** Not "contains no framework literals" - a
//!   generic-looking `String` parameter can smuggle one, so the gate is a dataflow check. Detection
//!   produces a label for provenance, display and filtering; nothing that decides behaviour reads it.
//! - **Rules compile once, to a typed plan.** Matching a carrier must not parse JSON, resolve a path
//!   string or dispatch on a string per observation - the read path has a p95 ceiling measured in
//!   milliseconds.
//!
//! What is deliberately *not* here: concrete provider connectors. `providers/test_connection.rs`
//! builds real clients with real auth flows, which is executable adapter code; calling it data would
//! be dishonest.

pub mod assets;
pub mod carrier_rules;
pub mod classify;
pub mod content_blocks;
pub mod detect_rules;
pub mod diagnostics;
pub mod expr;
pub mod finish_reasons;
pub mod log_events;
pub mod members;
pub mod message_projection;
pub mod message_rules;
pub mod precedence;
pub mod refusal;
pub mod schema;
pub mod span_fields;
mod tool_repr;
pub mod tool_shapes;

#[cfg(test)]
mod carrier_rules_tests;
#[cfg(test)]
mod detect_rules_tests;
#[cfg(test)]
mod precedence_instances_tests;
#[cfg(test)]
mod schema_census;

pub use carrier_rules::CarrierContext;
pub use detect_rules::DetectContext;
pub use message_rules::{EmittedCarrier, MessageContext};

use std::sync::OnceLock;

/// The one resource attribute the engine reads *structurally* rather than as producer vocabulary.
///
/// `service.name` is OpenTelemetry's own name, not any framework's: a rule says which *value* identifies
/// a producer through it, and the key is part of the dimension's definition. Held here so no asset can
/// redefine where "the service name" lives and have two assets disagree about what the dimension means.
///
/// `metadata` used to sit beside it and did not belong: it is one framework's attribute, so pretending
/// the engine owned the key made a producer's vocabulary look like part of OpenTelemetry. It is a value
/// of the generic `span_attr_contains` dimension now, with its key in the asset.
pub(crate) const SERVICE_NAME_KEY: &str = "service.name";

/// The label for a span no detection rule claimed and no declaration resolved.
///
/// The engine's own vocabulary for the *absence* of an answer, not a framework - which is why it lives
/// here while every real label lives in an asset.
pub const UNCLAIMED_LABEL: &str = "Unknown";

/// The compiled ruleset, built once from the embedded assets.
///
/// One `OnceLock` rather than a lazy per-lookup build: a compile that happened per span would put
/// path resolution and table construction on the read path, which is what the typed plan exists to
/// keep off it.
/// Facts about a span, asked by name.
///
/// One question, several conventions answering it: an operation name, a span-kind attribute, a pair of
/// attributes that only appear together. The union is the answer, so adding a dialect's evidence is a rule
/// rather than a branch - and nothing that reads the answer has to know which dialect supplied it.
/// Why the span-fact rules would not compile.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpanFactCompileError {
    #[error(
        "span fact `{fact:?}` signal `{rule}.{signal}` asserts nothing, so it holds for every span"
    )]
    AssertsNothing {
        fact: schema::SpanFact,
        rule: String,
        signal: String,
    },
    #[error("span fact `{fact:?}` signal `{rule}.{signal}` names an empty attribute key")]
    EmptyKey {
        fact: schema::SpanFact,
        rule: String,
        signal: String,
    },
    #[error(
        "span fact `{fact:?}` signal `{rule}.{signal}` asks for a case-insensitive compare with nothing to compare"
    )]
    CaseFoldsNothing {
        fact: schema::SpanFact,
        rule: String,
        signal: String,
    },
}

#[derive(Debug, Default)]
pub struct SpanFactPlan {
    /// Each signal with the rule it came from, so an established fact can name every witness.
    signals: Vec<(schema::SpanFact, String, schema::SpanSignal)>,
}

impl SpanFactPlan {
    fn compile(assets: &assets::ParsedAssets) -> Result<Self, SpanFactCompileError> {
        let plan = Self::compile_unvalidated(assets);
        for (fact, rule, signal) in &plan.signals {
            let located = || (*fact, rule.clone(), signal.id.clone());
            // A signal that asserts nothing holds for **every** span, which for `tool_execution` would
            // classify every span as a tool running and gate almost every message rule out. Refused rather
            // than warned about: the failure is total and silent.
            if signal.attr_equals.is_none() && signal.attrs_present.is_empty() {
                let (fact, rule, signal) = located();
                return Err(SpanFactCompileError::AssertsNothing { fact, rule, signal });
            }
            if signal.attrs_present.iter().any(String::is_empty) {
                let (fact, rule, signal) = located();
                return Err(SpanFactCompileError::EmptyKey { fact, rule, signal });
            }
            // Case-folding a comparison that is not made is a statement about nothing.
            if signal.ignore_case && signal.attr_equals.is_none() {
                let (fact, rule, signal) = located();
                return Err(SpanFactCompileError::CaseFoldsNothing { fact, rule, signal });
            }
        }
        Ok(plan)
    }

    fn compile_unvalidated(assets: &assets::ParsedAssets) -> Self {
        Self {
            signals: assets
                .files()
                .iter()
                .flat_map(|file| &file.span_facts)
                .flat_map(|rule| {
                    rule.signals
                        .iter()
                        .map(move |signal| (rule.fact, rule.id.clone(), signal.clone()))
                })
                .collect(),
        }
    }

    /// Whether any dialect's evidence establishes this fact for the span.
    pub fn holds(
        &self,
        fact: schema::SpanFact,
        attrs: &std::collections::HashMap<String, String>,
    ) -> bool {
        self.established(fact, attrs).is_some()
    }

    /// The fact and **every** signal that establishes it, or `None` where none does.
    ///
    /// Every witness, not the first: a fact holds if any dialect's evidence establishes it, so which ones did
    /// is a set. Reporting one arbitrarily is what a `bool` did - and there is deliberately no
    /// `Verdict<bool>`, because a negative answer is not a clause saying "false", it is no clause answering.
    pub fn established(
        &self,
        fact: schema::SpanFact,
        attrs: &std::collections::HashMap<String, String>,
    ) -> Option<expr::Verdict<schema::SpanFact>> {
        let witnesses: Vec<expr::ClausePath> = self
            .signals
            .iter()
            .filter(|(declared, _, _)| *declared == fact)
            .filter(|(_, _, signal)| Self::signal_holds(signal, attrs))
            .map(|(_, rule_id, signal)| {
                expr::ClausePath::root(rule_id.clone()).then(signal.id.clone())
            })
            .collect();
        expr::EvidenceSet::of(witnesses).map(|evidence| expr::Verdict::new(fact, evidence))
    }

    fn signal_holds(
        signal: &schema::SpanSignal,
        attrs: &std::collections::HashMap<String, String>,
    ) -> bool {
        {
            let signal = &signal;
            {
                let equals = signal.attr_equals.as_ref().is_none_or(|want| {
                    attrs.get(&want.key).is_some_and(|found| {
                        if signal.ignore_case {
                            found.eq_ignore_ascii_case(&want.value)
                        } else {
                            found == &want.value
                        }
                    })
                });
                // A conjunction: every named attribute present. One alone is not the evidence.
                equals
                    && signal
                        .attrs_present
                        .iter()
                        .all(|key| attrs.contains_key(key))
            }
        }
    }
}

pub struct Ruleset {
    /// Carrier semantics, indexed for lookup by exact name and by prefix.
    pub carriers: carrier_rules::CarrierPlan,
    /// Detection signals in rank order, and the SDK-declaration fallback.
    pub detect: detect_rules::DetectPlan,
    /// Which carriers an ingestion reads, declaratively.
    pub messages: message_rules::MessagePlan,
    /// Which stored producer bookkeeping rows are omitted from the read-time conversation.
    pub message_projection: message_projection::MessageProjectionPlan,
    /// Which events carry messages, and what each event's own raw form is.
    ///
    /// A map from the event name to its declaration, not a `HashSet<String>` of names: the entries answer two
    /// runtime questions (is this event a message carrier, and is its body a container its readings replace)
    /// and a set of names could answer only the first, with the second stated on the readings instead.
    pub message_events: std::collections::BTreeMap<String, DeclaredMessageEvent>,
    /// Which OTLP log records carry one of those events, and where each keeps the event's attributes.
    pub log_events: log_events::LogEventPlan,
    /// Which role each source name carries, and which instead on a tool execution span.
    pub event_roles: std::collections::BTreeMap<String, DeclaredEventRole>,
    /// What authority a stated role carries, by spelling.
    pub role_authority: RoleAuthorityPlan,
    /// Source names this engine assigns itself, through a rule's `tag_as`.
    ///
    /// A tagged emission is an *attribute* whose key this engine chose, so consulting its declared role is
    /// right; a producer's own attribute that happens to share the name is not, which is why this is the set
    /// of tags rather than "any attribute key".
    pub tagged_source_names: std::collections::BTreeSet<String>,
    /// Content-block shapes, declared per dialect.
    pub content_blocks: content_blocks::ContentBlockPlan,
    /// Facts about a span, each established by any dialect that can.
    pub span_facts: SpanFactPlan,
    /// Where each stored span field is written, per producer.
    pub span_fields: span_fields::SpanFieldPlan,
    /// The shapes a provider writes a tool definition in.
    pub tool_shapes: tool_shapes::ToolShapePlan,
    /// What kind of observation a span is, as ordered first-match rules.
    pub observation_types: classify::ClassifyPlan,
    /// Member names a producer uses, and what each one's presence means.
    pub message_members: members::MemberPlan,
    /// `gen_ai.system` values that are a framework's own name and mean a provider the catalogue prices.
    pub provider_aliases: std::collections::BTreeMap<String, String>,
    /// What each declared spelling of a finish reason means.
    pub finish_reasons: finish_reasons::FinishReasonPlan,
    /// The separators of the synthetic call ids producers build as `{name}<separator>{index}`.
    pub synthetic_call_ids: Vec<String>,
    /// The words an event's name may contain and the category each establishes, in rank order.
    pub event_categories: Vec<(Vec<String>, schema::EventCategory)>,
    /// BLAKE3 of the asset bytes that produced this plan, hex-encoded.
    ///
    /// Joins the reconstruction cache key. That cache is a memo over a pure function of the rows, and
    /// once rules can change they are part of the function - without this, a dev hot-load would serve
    /// answers built by a different ruleset from rows that had not changed.
    pub digest: String,
}

static RULESET: OnceLock<Ruleset> = OnceLock::new();

/// The compiled ruleset. Panics only if an *embedded* asset is malformed, which is a build defect: the
/// assets ship inside the binary, so there is no runtime input that can reach this. The panic lists every
/// defect found, not only the first.
pub fn ruleset() -> &'static Ruleset {
    RULESET.get_or_init(|| {
        let assets = assets::ParsedAssets::parse(&schema::embedded_sources())
            .unwrap_or_else(|e| panic!("embedded rules are malformed: {e}"));
        Ruleset::build(&assets).unwrap_or_else(|e| panic!("embedded rules are malformed: {e}"))
    })
}

impl Ruleset {
    /// Compile every section from one parse of the assets, reporting every section's defect at once.
    ///
    /// Takes the parsed corpus and never bytes, so no section can re-parse a file. Every section is compiled
    /// even after one fails, so a corpus with defects in two sections reports both.
    pub fn build(assets: &assets::ParsedAssets) -> Result<Self, diagnostics::RulesetDiagnostics> {
        use diagnostics::RuleSection as S;
        let files = assets.files();
        let mut found = diagnostics::Collector::new(assets);
        let carriers = found.take(S::Carriers, carrier_rules::compile(assets));
        let detect = found.take(S::Detect, detect_rules::compile(assets));
        let messages = found.take(S::Messages, message_rules::compile(assets));
        let message_events = found.take(S::MessageEvents, compile_message_events(files));
        // Log events are validated against the message events, so they are compiled only once those have; a
        // defect there is reported by the message-event section and would only be restated here.
        let log_events = message_events.as_ref().and_then(|events| {
            found.take(
                S::LogEvents,
                log_events::LogEventPlan::compile(files, events),
            )
        });
        let message_projection = found.take(
            S::MessageProjections,
            message_projection::MessageProjectionPlan::compile(files),
        );
        let content_blocks = found.take(
            S::ContentBlocks,
            content_blocks::ContentBlockPlan::compile(files),
        );
        let role_authority = found.take(S::RoleAuthority, compile_role_authority(files));
        let tagged_source_names = tag_names(files);
        let event_roles = found.take(
            S::EventRoles,
            compile_event_roles(files, &tagged_source_names),
        );
        let span_facts = found.take(S::SpanFacts, SpanFactPlan::compile(assets));
        let span_fields = found.take(S::SpanFields, span_fields::compile(assets));
        let tool_shapes = found.take(S::ToolShapes, tool_shapes::ToolShapePlan::compile(files));
        let observation_types = found.take(S::Classification, classify::compile(assets));
        let message_members = found.take(S::MessageMembers, members::compile(assets));
        let provider_aliases = found.take(S::ProviderAliases, compile_provider_aliases(files));
        let finish_reasons = found.take(
            S::FinishReasons,
            finish_reasons::FinishReasonPlan::compile(files),
        );
        let synthetic_call_ids = found.take(S::SyntheticCallIds, compile_synthetic_call_ids(files));
        let event_categories = found.take(S::EventCategories, compile_event_categories(files));
        // Every section is `Some` exactly when it compiled, and each `None` recorded its defect - so a full
        // match is a ruleset and anything else is the collected report.
        match (
            carriers,
            detect,
            messages,
            message_projection,
            content_blocks,
            message_events,
            log_events,
            role_authority,
            event_roles,
            span_facts,
            span_fields,
            tool_shapes,
            observation_types,
            message_members,
            provider_aliases,
            finish_reasons,
            synthetic_call_ids,
            event_categories,
        ) {
            (
                Some(carriers),
                Some(detect),
                Some(messages),
                Some(message_projection),
                Some(content_blocks),
                Some(message_events),
                Some(log_events),
                Some(role_authority),
                Some(event_roles),
                Some(span_facts),
                Some(span_fields),
                Some(tool_shapes),
                Some(observation_types),
                Some(message_members),
                Some(provider_aliases),
                Some(finish_reasons),
                Some(synthetic_call_ids),
                Some(event_categories),
            ) => Ok(Ruleset {
                carriers,
                detect,
                messages,
                message_projection,
                content_blocks,
                message_events,
                log_events,
                role_authority,
                event_roles,
                span_facts,
                span_fields,
                tool_shapes,
                observation_types,
                message_members,
                provider_aliases,
                finish_reasons,
                synthetic_call_ids,
                event_categories,
                tagged_source_names,
                digest: assets.digest().to_owned(),
            }),
            _ => Err(found.into_diagnostics()),
        }
    }
}

/// Every name a rule assigns with `tag_as`, at **any** depth of the rule tree.
///
/// Over the **typed** rules, not the raw JSON, and both halves of that matter. A `tag_as` sits on a message
/// rule and a rule nests - a branch set's primary, its empty-fallback and its always-read lists each hold
/// rules, and one asset's branch leaf already carries a tag. Reading only the top level left such a tag out of
/// both indexes at once, which is worse than either alone: a role declared for it is refused as unreachable,
/// and without the declaration its message normalises as a user turn.
///
/// A raw-JSON walk was the first fix and is wrong in the other direction: any object member named `tag_as`
/// would count, including one inside a rule's *literal data* - a composed trailing value, an example payload -
/// so an `event_roles` declaration could compile against a tag no rule ever assigns. The typed walk can only
/// see the member where it means something.
pub(super) fn tag_names(files: &[schema::RuleFile]) -> std::collections::BTreeSet<String> {
    fn walk(rule: &schema::MessageRule, out: &mut std::collections::BTreeSet<String>) {
        if let Some(tag) = &rule.tag_as {
            out.insert(tag.clone());
        }
        if let Some(branches) = &rule.branch_set {
            for leaf in branches
                .primary
                .iter()
                .chain(&branches.fallback_if_primary_empty)
                .chain(&branches.always)
            {
                walk(leaf, out);
            }
        }
    }
    let mut out = std::collections::BTreeSet::new();
    for file in files {
        for rule in &file.messages {
            walk(rule, &mut out);
        }
    }
    out
}

/// The role a source name carries, resolved to the enum at compile time and carrying its provenance.
///
/// The roles are `ChatRole`, not strings: a plan is typed so nothing is dispatched on a string per span, and
/// a name outside the vocabulary is a build defect rather than a role silently derived from content instead.
/// The asset, rule id and doc travel with it for the reason every other compiled clause carries them - a
/// diagnostic that cannot say *which* declaration answered explains nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredMessageEvent {
    /// Whether the event's own body is a message, or a container its readings replace.
    pub raw: schema::RawEventForm,
    /// Every declaration that agrees, outermost first - not merely the first one loaded.
    ///
    /// Several assets may legitimately declare one event (the conventions list `gen_ai.choice` and a dialect
    /// re-lists it to add its own doc), and the previous form kept whichever arrived first. A witness set is
    /// the honest shape: they all said it, so they are all evidence for it.
    pub witnesses: expr::EvidenceSet,
}

/// What role a source name carries, gathered across every asset.
#[derive(Debug, Clone)]
pub struct DeclaredEventRole {
    /// On an ordinary span. `None` leaves the role to the content.
    pub role: Option<crate::sideml::ChatRole>,
    /// On a tool execution span, where two names mean the opposite of what they mean elsewhere. `None` means
    /// the same as `role`.
    pub in_tool_span: Option<crate::sideml::ChatRole>,
    /// The asset that declared it.
    pub asset: String,
    /// The declared clause id, so a diagnostic can name the declaration rather than only the name it
    /// answered for. **Declared**, not synthesized from the asset and the event name: a synthesized id is
    /// not an identity a declaration can be held to.
    pub rule_id: String,
    /// Which side of a generation the source's messages are on, where declared.
    pub direction: Option<schema::MessageDirection>,
    /// Every declaration that agrees about the roles, so an agreeing repeat keeps its provenance.
    pub witnesses: expr::EvidenceSet,
    /// Why, for the explain trace.
    pub doc: Option<String>,
}

impl DeclaredEventRole {
    /// The role this name carries on a span of the given kind, or `None` where it says nothing there.
    ///
    /// Resolved here rather than at the call site, so the resolution sits beside the fields that encode it and
    /// a test can put a declaration to it directly - the caller reads a global registry, which no probe can
    /// substitute.
    pub fn role_on(&self, is_tool_span: bool) -> Option<crate::sideml::ChatRole> {
        if !is_tool_span {
            return self.role;
        }
        // Absence means the same role as elsewhere.
        self.in_tool_span.or(self.role)
    }
}

/// The roles each message event declares, gathered across every asset.
///
/// One map rather than a per-asset lookup, because the question is asked with an event name and nothing else:
/// a span carries an event, not the asset that described it. Several assets legitimately list the same event -
/// the conventions declare `gen_ai.choice` and a dialect re-declares it to add its own doc - so a repeat is
/// accepted while a **disagreement** is refused: two assets claiming different roles for one event would be
/// resolved by load order, which is not a statement anybody made.
/// Which events carry messages, with every agreeing declaration kept.
///
/// A repeat that agrees is a dialect re-stating a convention and is allowed; a repeat that *disagrees* about
/// the raw form is refused, because which one applied would depend on load order - the same rule the role
/// registry follows, and the reason both are compiled rather than collected.
pub(super) fn compile_message_events(
    files: &[schema::RuleFile],
) -> Result<std::collections::BTreeMap<String, DeclaredMessageEvent>, String> {
    let mut out: std::collections::BTreeMap<String, DeclaredMessageEvent> =
        std::collections::BTreeMap::new();
    for file in files {
        for event in &file.message_events {
            if event.name.is_empty() {
                return Err(format!(
                    "`{}` declares a message event with no name",
                    file.id
                ));
            }
            let raw = event.raw.unwrap_or_default();
            let path = expr::ClausePath::root(event.id.clone());
            match out.get_mut(&event.name) {
                None => {
                    out.insert(
                        event.name.clone(),
                        DeclaredMessageEvent {
                            raw,
                            witnesses: expr::EvidenceSet::one(path),
                        },
                    );
                }
                Some(existing) if existing.raw == raw => {
                    let mut paths = existing.witnesses.paths().to_vec();
                    paths.push(path);
                    existing.witnesses = expr::EvidenceSet::of(paths)
                        .expect("a non-empty witness list stays non-empty");
                }
                Some(existing) => {
                    return Err(format!(
                        "message event `{}` is declared with two different raw forms ({:?} by {}, {raw:?} by \
                         `{}`) - which applies would depend on load order",
                        event.name, existing.raw, existing.witnesses, event.id
                    ));
                }
            }
        }
    }
    Ok(out)
}

pub(super) fn compile_event_roles(
    files: &[schema::RuleFile],
    tagged: &std::collections::BTreeSet<String>,
) -> Result<std::collections::BTreeMap<String, DeclaredEventRole>, String> {
    use crate::sideml::ChatRole;
    /// The roles a source name may declare. Ours, not any producer's - so a misspelling is a build defect
    /// rather than a silent fall back to deriving the role from the content.
    const ROLES: &[&str] = &["system", "user", "assistant", "tool"];
    // The names that can actually occur: an event a producer emits, or one a rule assigns with `tag_as`. A
    // declaration for anything else can never answer, and a rule that can never answer reads as protection.
    // A carrier event a `raw: replace` event's readings are named after occurs too.
    let occurring: std::collections::BTreeSet<&str> = files
        .iter()
        .flat_map(|file| file.message_events.iter().map(|event| event.name.as_str()))
        .chain(tagged.iter().map(String::as_str))
        .chain(files.iter().flat_map(|file| {
            file.carriers
                .iter()
                .filter_map(|carrier| carrier.match_spec.event.as_deref())
        }))
        .collect();
    let mut out: std::collections::BTreeMap<String, DeclaredEventRole> =
        std::collections::BTreeMap::new();
    for file in files {
        for event in &file.event_roles {
            if event.name.is_empty() {
                return Err(format!("`{}` declares an event role with no name", file.id));
            }
            if !occurring.contains(event.name.as_str()) {
                return Err(format!(
                    "event role `{}` in `{}` names something no asset produces - it is neither a \
                     `message_events` entry nor any rule's `tag_as`, so it could never answer",
                    event.name, file.id
                ));
            }
            for role in [&event.role, &event.role_in_tool_span]
                .into_iter()
                .flatten()
            {
                if !ROLES.contains(&role.as_str()) {
                    return Err(format!(
                        "event `{}` in `{}` declares role `{role}`, which is not one of: {}",
                        event.name,
                        file.id,
                        ROLES.join(", ")
                    ));
                }
            }
            // Canonical spellings only, which the check above has just required - and the ruleset this builds
            // is not there to ask for an alias.
            let resolve = |named: &Option<String>| named.as_deref().and_then(ChatRole::canonical);
            // One spelling per statement. A `role_in_tool_span` equal to `role` says what absence
            // already says, and it was legal: one shipped declaration spelled it while seven omitted it for the
            // same fact.
            if event.role_in_tool_span.is_some() && event.role_in_tool_span == event.role {
                return Err(format!(
                    "event role `{}` in `{}` declares `role_in_tool_span` equal to its `role` - omit it, since                      absence already means the same role on both kinds of span",
                    event.name, file.id
                ));
            }
            let declared = DeclaredEventRole {
                role: resolve(&event.role),
                in_tool_span: resolve(&event.role_in_tool_span),
                direction: event.direction,
                asset: file.id.clone(),
                rule_id: event.id.clone(),
                witnesses: expr::EvidenceSet::one(expr::ClausePath::root(event.id.clone())),
                doc: event.doc.clone(),
            };
            // A name that says nothing about the role is not a declaration, and accepting it would let an
            // empty entry silently replace a real one.
            if declared.role.is_none()
                && declared.in_tool_span.is_none()
                && declared.direction.is_none()
            {
                return Err(format!(
                    "event role `{}` in `{}` names no role and no direction, so it states nothing - leave the \
                     entry out to leave the role to the content",
                    event.name, file.id
                ));
            }
            match out.get(&event.name) {
                None => {
                    out.insert(event.name.clone(), declared);
                }
                // A repeat that agrees about the *roles* is a dialect re-stating a convention, which is
                // allowed - the provenance differs by definition and says nothing about the answer.
                // A repeat that states no direction says nothing about it, so it agrees with one that does.
                Some(existing)
                    if existing.role == declared.role
                        && existing.in_tool_span == declared.in_tool_span
                        && (existing.direction == declared.direction
                            || existing.direction.is_none()
                            || declared.direction.is_none()) =>
                {
                    // Agreement keeps *both* witnesses. The previous form discarded the later one, so an
                    // asset that re-stated a convention had no provenance for a fact it declared.
                    let mut paths = existing.witnesses.paths().to_vec();
                    paths.extend(declared.witnesses.paths().iter().cloned());
                    let merged = expr::EvidenceSet::of(paths)
                        .expect("a non-empty witness list stays non-empty");
                    let entry = out.get_mut(&event.name).expect("just looked it up");
                    entry.witnesses = merged;
                    entry.direction = entry.direction.or(declared.direction);
                }
                Some(existing) => {
                    return Err(format!(
                        "source name `{}` is declared {:?}/{:?}/{:?} in `{}` and {:?}/{:?}/{:?} in `{}` - \
                         which applies would depend on load order",
                        event.name,
                        existing.role,
                        existing.in_tool_span,
                        existing.direction,
                        existing.asset,
                        declared.role,
                        declared.in_tool_span,
                        declared.direction,
                        declared.asset
                    ));
                }
            }
        }
    }
    Ok(out)
}

/// What authority a **stated role** carries, by spelling.
///
/// Two sets rather than one list, because the questions differ and were fused: `tool` outranks a tagged
/// attribute name and must **not** survive event-name derivation, since an event name is real evidence of it.
#[derive(Debug, Default)]
pub struct RoleAuthorityPlan {
    survives_event_derivation: std::collections::BTreeSet<String>,
    outranks_a_tag: std::collections::BTreeSet<String>,
    means: std::collections::BTreeMap<String, crate::sideml::ChatRole>,
}

impl RoleAuthorityPlan {
    /// The canonical role a declared spelling means. Lower case in, as `ChatRole::try_from_str` folds it.
    pub fn means(&self, spelling: &str) -> Option<crate::sideml::ChatRole> {
        self.means.get(spelling).copied()
    }

    /// Every spelling given a meaning, with it.
    pub fn meanings(&self) -> impl Iterator<Item = (&str, crate::sideml::ChatRole)> {
        self.means
            .iter()
            .map(|(spelling, role)| (spelling.as_str(), *role))
    }

    /// Whether a stated role of this spelling survives the role an event name would derive.
    pub fn survives_event_derivation(&self, stated: &str) -> bool {
        self.survives_event_derivation
            .contains(&stated.to_lowercase())
    }

    /// Whether a stated role of this spelling outranks the name a tagged attribute reading was found under.
    pub fn outranks_a_tag(&self, stated: &str) -> bool {
        self.outranks_a_tag.contains(&stated.to_lowercase())
    }

    /// Every declared spelling, for a test that checks the vocabulary as a set.
    pub fn declared_spellings(&self) -> impl Iterator<Item = &str> {
        self.survives_event_derivation
            .iter()
            .chain(self.outranks_a_tag.iter())
            .map(String::as_str)
    }

    pub fn rule_count(&self) -> usize {
        self.declared_spellings()
            .chain(self.means.keys().map(String::as_str))
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    }
}

/// Every declared spelling's canonical role, read straight from the declarations.
///
/// For the compilers that validate a stated role while the ruleset itself is still being built - asking
/// `ChatRole::try_from_str` there would ask for the ruleset under construction. A defect in the declarations is
/// reported by `compile_role_authority`, not here.
pub(super) fn declared_role_meanings(
    files: &[schema::RuleFile],
) -> std::collections::BTreeMap<String, crate::sideml::ChatRole> {
    files
        .iter()
        .flat_map(|file| &file.role_authority)
        .filter_map(|entry| entry.means.map(|role| (entry.role.clone(), role)))
        .collect()
}

/// The declared role authorities, gathered across every asset.
///
/// Every spelling the role alias table folds must be declared here, so adding an alias forces an authority
/// decision instead of inheriting one silently - which is what it did: the alias table decided authority on the
/// tagged-attribute path, and its job is folding spellings, not granting authority.
pub(super) fn compile_role_authority(
    files: &[schema::RuleFile],
) -> Result<RoleAuthorityPlan, String> {
    let mut plan = RoleAuthorityPlan::default();
    let mut by_role: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for file in files {
        for entry in &file.role_authority {
            if entry.role.is_empty() {
                return Err(format!(
                    "`{}` declares a role authority with no role",
                    file.id
                ));
            }
            if entry.role != entry.role.to_lowercase() {
                return Err(format!(
                    "role authority `{}` in `{}` declares `{}`, which is matched case-insensitively - declare it                      in lower case, or two spellings of one entry read as two",
                    entry.id, file.id, entry.role
                ));
            }
            if !entry.survives_event_derivation && !entry.outranks_a_tag && entry.means.is_none() {
                return Err(format!(
                    "role authority `{}` in `{}` grants no authority and gives no meaning, so it states nothing \
                     - leave the entry out",
                    entry.id, file.id
                ));
            }
            if entry.means.is_some() && crate::sideml::ChatRole::canonical(&entry.role).is_some() {
                return Err(format!(
                    "role authority `{}` in `{}` gives `{}` a meaning, and it is a canonical role, which means \
                     itself - leave `means` out",
                    entry.id, file.id, entry.role
                ));
            }
            if let Some(first) = by_role.get(&entry.role) {
                return Err(format!(
                    "role `{}` is declared twice, by `{first}` and `{}` - which applies would depend on load                      order",
                    entry.role, entry.id
                ));
            }
            by_role.insert(entry.role.clone(), entry.id.clone());
            if entry.survives_event_derivation {
                plan.survives_event_derivation.insert(entry.role.clone());
            }
            if entry.outranks_a_tag {
                plan.outranks_a_tag.insert(entry.role.clone());
            }
            if let Some(role) = entry.means {
                plan.means.insert(entry.role.clone(), role);
            }
        }
    }
    Ok(plan)
}

/// The declared event categories in priority order, refusing an empty word and a shared priority.
pub(super) fn compile_event_categories(
    files: &[schema::RuleFile],
) -> Result<Vec<(Vec<String>, schema::EventCategory)>, String> {
    let mut ranked: Vec<(i32, String, Vec<String>, schema::EventCategory)> = Vec::new();
    for file in files {
        for entry in &file.event_categories {
            if entry.contains.is_empty() || entry.contains.iter().any(String::is_empty) {
                return Err(format!(
                    "event category `{}` in `{}` names no word, or an empty one, which every name contains",
                    entry.id, file.id
                ));
            }
            ranked.push((
                entry.priority,
                entry.id.clone(),
                entry.contains.clone(),
                entry.category,
            ));
        }
    }
    if let Some((first, second)) =
        precedence::shared_priority(&ranked, |(priority, ..)| *priority, |_, _| true)
    {
        return Err(format!(
            "event categories `{}` and `{}` share priority {}, so which answers depends on load order",
            first.1, second.1, first.0
        ));
    }
    ranked.sort_by_key(|(priority, ..)| *priority);
    Ok(ranked
        .into_iter()
        .map(|(_, _, words, category)| (words, category))
        .collect())
}

/// The separators of the declared synthetic call ids, refusing a template outside the closed form.
pub(super) fn compile_synthetic_call_ids(
    files: &[schema::RuleFile],
) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for file in files {
        for entry in &file.synthetic_call_ids {
            let separator = entry
                .template
                .strip_prefix("{name}")
                .and_then(|rest| rest.strip_suffix("{index}"))
                .filter(|separator| !separator.is_empty() && !separator.contains(['{', '}']))
                .ok_or_else(|| {
                    format!(
                        "synthetic call id `{}` in `{}` declares `{}`, which is not `{{name}}<separator>{{index}}` \
                         with a non-empty separator",
                        entry.id, file.id, entry.template
                    )
                })?;
            if out.iter().any(|seen| seen == separator) {
                return Err(format!(
                    "synthetic call id `{}` in `{}` restates a separator another declaration states",
                    entry.id, file.id
                ));
            }
            out.push(separator.to_string());
        }
    }
    Ok(out)
}

/// The declared `gen_ai.system` → provider aliases.
///
/// Every refusal here is a declaration that could not take effect, which reads as one that does. A `Result`
/// rather than a panic, so a test can state the refusals rather than catching an unwind - the same shape the
/// other compile functions have.
pub(super) fn compile_provider_aliases(
    files: &[schema::RuleFile],
) -> Result<std::collections::BTreeMap<String, String>, String> {
    let mut out: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for file in files {
        for alias in &file.provider_aliases {
            if alias.system.is_empty() || alias.provider.is_empty() {
                return Err(format!(
                    "provider alias in `{}` names an empty system or provider",
                    file.id
                ));
            }
            // The value is matched **after** normalisation, so a key normalisation would never produce can never
            // be reached.
            let normalised = alias.system.to_lowercase().replace(['-', ' '], "_");
            if normalised != alias.system {
                return Err(format!(
                    "provider alias `{}` in `{}` is not in the form the lookup normalises to (`{normalised}`), \
                     so it would never be reached",
                    alias.system, file.id
                ));
            }
            // A key the catalogue's own table already answers is shadowed by that table - the ordering makes it
            // harmless and this makes it visible.
            let shadowed = crate::pricing::builtin_provider(&normalised);
            if !shadowed.is_empty() {
                return Err(format!(
                    "provider alias `{}` in `{}` names a value the catalogue already reads as provider \
                     `{shadowed}`, so the declaration could never take effect",
                    alias.system, file.id
                ));
            }
            // And the provider it names has to be one the catalogue knows, or the alias resolves to a name
            // nothing prices - which looks like a priced call and is not.
            if crate::pricing::builtin_provider(&alias.provider) != alias.provider {
                return Err(format!(
                    "provider alias `{}` in `{}` names provider `{}`, which the catalogue does not read as a \
                     provider of its own",
                    alias.system, file.id, alias.provider
                ));
            }
            if let Some(first) = out.insert(alias.system.clone(), alias.provider.clone())
                && first != alias.provider
            {
                return Err(format!(
                    "`{}` is declared as provider `{first}` and as `{}`, so which one prices a call would \
                     depend on load order",
                    alias.system, alias.provider
                ));
            }
        }
    }
    Ok(out)
}

/// The ruleset digest, for the reconstruction cache key.
pub fn ruleset_digest() -> &'static str {
    &ruleset().digest
}
