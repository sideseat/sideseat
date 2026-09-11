//! The framework rules engine: framework knowledge as data, interpreted generically.
//!
//! Every fact about a *specific* framework or provider lives in a rule asset under `server/rules/`,
//! not in this module. The engine knows how to match and how to compose; it knows no producer names,
//! carrier keys, tags or type mappings. The design and its acceptance are recorded in
//! `server/docs/framework-rules-engine.md`.
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

pub mod carrier_rules;
pub mod classify;
pub mod content_blocks;
pub mod detect_rules;
pub mod expr;
pub mod members;
pub mod message_rules;
pub mod outcome;
pub mod schema;
pub mod span_fields;
mod tool_repr;
pub mod tool_shapes;

#[cfg(test)]
mod carrier_rules_tests;
#[cfg(test)]
mod detect_rules_tests;

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
#[derive(Debug, Default)]
pub struct SpanFactPlan {
    /// Each signal with the rule it came from, so an established fact can name every witness.
    signals: Vec<(schema::SpanFact, String, schema::SpanSignal)>,
}

impl SpanFactPlan {
    fn compile(sources: &std::collections::BTreeMap<String, Vec<u8>>) -> Self {
        let plan = Self::compile_unvalidated(sources);
        for (fact, _, signal) in &plan.signals {
            // A signal that asserts nothing holds for **every** span, which for `tool_execution` would
            // classify every span as a tool running and gate almost every message rule out. Refused rather
            // than warned about: the failure is total and silent.
            if signal.attr_equals.is_none() && signal.attrs_present.is_empty() {
                panic!(
                    "span fact `{fact:?}` has a signal that asserts nothing, so it holds for every span"
                );
            }
            if signal.attrs_present.iter().any(String::is_empty) {
                panic!("span fact `{fact:?}` names an empty attribute key");
            }
            // Case-folding a comparison that is not made is a statement about nothing.
            if signal.ignore_case && signal.attr_equals.is_none() {
                panic!(
                    "span fact `{fact:?}` asks for a case-insensitive compare with nothing to compare"
                );
            }
        }
        plan
    }

    fn compile_unvalidated(sources: &std::collections::BTreeMap<String, Vec<u8>>) -> Self {
        let files = parsed_files(sources);
        Self {
            signals: files
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
    /// Which events carry messages, and what each event's own raw form is.
    ///
    /// A map from the event name to its declaration, not a `HashSet<String>` of names: the entries answer two
    /// runtime questions (is this event a message carrier, and is its body a container its readings replace)
    /// and a set of names could answer only the first, with the second stated on the readings instead.
    pub message_events: std::collections::BTreeMap<String, DeclaredMessageEvent>,
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
    /// BLAKE3 of the asset bytes that produced this plan, hex-encoded.
    ///
    /// Joins the reconstruction cache key. That cache is a memo over a pure function of the rows, and
    /// once rules can change they are part of the function - without this, a dev hot-load would serve
    /// answers built by a different ruleset from rows that had not changed.
    pub digest: String,
}

static RULESET: OnceLock<Ruleset> = OnceLock::new();

/// The compiled ruleset. Panics only if an *embedded* asset is malformed, which is a build defect: the
/// assets ship inside the binary, so there is no runtime input that can reach this.
/// Every asset, parsed. Named in the panic and never skipped: a file quietly dropped for a typo is how a
/// whole dialect's rules once vanished with every test still green.
fn parsed_files(sources: &std::collections::BTreeMap<String, Vec<u8>>) -> Vec<schema::RuleFile> {
    let files: Vec<schema::RuleFile> = sources
        .iter()
        .map(|(path, bytes)| {
            let file: schema::RuleFile = serde_json::from_slice(bytes)
                .unwrap_or_else(|e| panic!("embedded rules are malformed: {path}: {e}"));
            // Declaration defects, in production and not only in a test over this tree: the clause-uniqueness
            // rule is one property about six types compiled by three different modules, so it had no single
            // place to live and the generic compiler accepted two clauses sharing an id.
            if let Some(defect) = file.declaration_defect() {
                panic!("embedded rules are malformed: {path}: {defect}");
            }
            file
        })
        .collect();
    if let Some(repeated) = repeated_asset_id(&files) {
        panic!(
            "embedded rules are malformed: two assets declare the id `{repeated}`, so their clauses share a \
             provenance path and nothing can tell them apart"
        );
    }
    files
}

/// An id two assets declare, if any.
///
/// An asset id is the provenance every diagnostic reports, and `declaration_defect` is per *file* - so two files
/// could declare the same one and nothing would notice: their clauses would be indistinguishable precisely where a
/// reader looks to tell them apart, and the clause-owner uniqueness rule stops at the file boundary. A function
/// rather than an inline assertion so a test can put two files to it; the embedded set has no duplicate to trigger
/// it with.
pub(super) fn repeated_asset_id(files: &[schema::RuleFile]) -> Option<&str> {
    let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    files
        .iter()
        .find(|file| !seen.insert(file.id.as_str()))
        .map(|file| file.id.as_str())
}

pub fn ruleset() -> &'static Ruleset {
    RULESET.get_or_init(|| {
        let sources = schema::embedded_sources();
        let digest = schema::digest_of(&sources);
        let carriers = carrier_rules::compile(&sources)
            .unwrap_or_else(|e| panic!("embedded carrier rules are malformed: {e}"));
        let detect = detect_rules::compile(&sources)
            .unwrap_or_else(|e| panic!("embedded detection rules are malformed: {e}"));
        let messages = message_rules::compile(&sources)
            .unwrap_or_else(|e| panic!("embedded message rules are malformed: {e}"));
        Ruleset {
            carriers,
            detect,
            messages,
            content_blocks: content_blocks::ContentBlockPlan::compile(&parsed_files(&sources)),
            message_events: compile_message_events(&parsed_files(&sources))
                .unwrap_or_else(|e| panic!("embedded message events are malformed: {e}")),
            role_authority: compile_role_authority(&parsed_files(&sources)).unwrap_or_else(
                |error| panic!("the embedded role-authority declarations are malformed: {error}"),
            ),
            event_roles: compile_event_roles(
                &parsed_files(&sources),
                &tag_names(&parsed_files(&sources)),
            )
            .unwrap_or_else(|e| panic!("embedded event roles are malformed: {e}")),
            tagged_source_names: tag_names(&parsed_files(&sources)),
            span_facts: SpanFactPlan::compile(&sources),
            span_fields: span_fields::compile(&sources)
                .unwrap_or_else(|e| panic!("embedded span field rules are malformed: {e}")),
            tool_shapes: tool_shapes::ToolShapePlan::compile(&parsed_files(&sources))
                .unwrap_or_else(|e| panic!("embedded tool shapes are malformed: {e}")),
            observation_types: classify::compile(&sources)
                .unwrap_or_else(|e| panic!("embedded classification rules are malformed: {e}")),
            message_members: members::compile(&sources)
                .unwrap_or_else(|e| panic!("embedded member rules are malformed: {e}")),
            provider_aliases: compile_provider_aliases(&parsed_files(&sources))
                .unwrap_or_else(|e| panic!("embedded provider aliases are malformed: {e}")),
            digest,
        }
    })
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
    pub role: Option<crate::domain::sideml::ChatRole>,
    /// On a tool execution span, where two names mean the opposite of what they mean elsewhere. `None` means
    /// the same as `role`; `silent_in_tool_span` is the separate statement that there is no role there.
    pub in_tool_span: Option<crate::domain::sideml::ChatRole>,
    /// This name says nothing about the role on a tool execution span, so it is derived from the content there.
    pub silent_in_tool_span: bool,
    /// The asset that declared it.
    pub asset: String,
    /// The declared clause id, so a diagnostic can name the declaration rather than only the name it
    /// answered for. **Declared**, not synthesized from the asset and the event name: a synthesized id is
    /// not an identity a declaration can be held to.
    pub rule_id: String,
    /// Every declaration that agrees about the roles, so an agreeing repeat keeps its provenance.
    pub witnesses: expr::EvidenceSet,
    /// Why, for the explain trace.
    pub doc: Option<String>,
}

impl DeclaredEventRole {
    /// The role this name carries on a span of the given kind, or `None` where it says nothing there.
    ///
    /// The three states resolve here rather than at the call site, so the resolution sits beside the fields that
    /// encode it and a test can put a declaration to it directly - the caller reads a global registry, which no
    /// probe can substitute.
    pub fn role_on(&self, is_tool_span: bool) -> Option<crate::domain::sideml::ChatRole> {
        if !is_tool_span {
            return self.role;
        }
        if self.silent_in_tool_span {
            return None;
        }
        // Absence means the same role as elsewhere, which is why silence needs a flag of its own.
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
    use crate::domain::sideml::ChatRole;
    /// The roles a source name may declare. Ours, not any producer's - so a misspelling is a build defect
    /// rather than a silent fall back to deriving the role from the content.
    const ROLES: &[&str] = &["system", "user", "assistant", "tool"];
    // The names that can actually occur: an event a producer emits, or one a rule assigns with `tag_as`. A
    // declaration for anything else can never answer, and a rule that can never answer reads as protection.
    let occurring: std::collections::BTreeSet<&str> = files
        .iter()
        .flat_map(|file| file.message_events.iter().map(|event| event.name.as_str()))
        .chain(tagged.iter().map(String::as_str))
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
            let resolve =
                |named: &Option<String>| named.as_deref().and_then(ChatRole::try_from_str);
            // The three states have one spelling each. A `role_in_tool_span` equal to `role` says what absence
            // already says, and it was legal: one shipped declaration spelled it while seven omitted it for the
            // same fact.
            if event.role_in_tool_span.is_some() && event.role_in_tool_span == event.role {
                return Err(format!(
                    "event role `{}` in `{}` declares `role_in_tool_span` equal to its `role` - omit it, since                      absence already means the same role on both kinds of span",
                    event.name, file.id
                ));
            }
            if event.silent_in_tool_span && event.role_in_tool_span.is_some() {
                return Err(format!(
                    "event role `{}` in `{}` declares both `silent_in_tool_span` and a `role_in_tool_span`,                      which are two answers about one span kind",
                    event.name, file.id
                ));
            }
            if event.silent_in_tool_span && event.role.is_none() {
                return Err(format!(
                    "event role `{}` in `{}` is silent on tool spans and declares no other role, so it states                      nothing - leave the entry out",
                    event.name, file.id
                ));
            }
            let declared = DeclaredEventRole {
                role: resolve(&event.role),
                in_tool_span: resolve(&event.role_in_tool_span),
                silent_in_tool_span: event.silent_in_tool_span,
                asset: file.id.clone(),
                rule_id: event.id.clone(),
                witnesses: expr::EvidenceSet::one(expr::ClausePath::root(event.id.clone())),
                doc: event.doc.clone(),
            };
            // A name that says nothing about the role is not a declaration, and accepting it would let an
            // empty entry silently replace a real one.
            if declared.role.is_none() && declared.in_tool_span.is_none() {
                return Err(format!(
                    "event role `{}` in `{}` names no role at all, so it states nothing - leave the entry \
                     out to leave the role to the content",
                    event.name, file.id
                ));
            }
            match out.get(&event.name) {
                None => {
                    out.insert(event.name.clone(), declared);
                }
                // A repeat that agrees about the *roles* is a dialect re-stating a convention, which is
                // allowed - the provenance differs by definition and says nothing about the answer.
                Some(existing)
                    if existing.role == declared.role
                        && existing.in_tool_span == declared.in_tool_span
                        && existing.silent_in_tool_span == declared.silent_in_tool_span =>
                {
                    // Agreement keeps *both* witnesses. The previous form discarded the later one, so an
                    // asset that re-stated a convention had no provenance for a fact it declared.
                    let mut paths = existing.witnesses.paths().to_vec();
                    paths.extend(declared.witnesses.paths().iter().cloned());
                    let merged = expr::EvidenceSet::of(paths)
                        .expect("a non-empty witness list stays non-empty");
                    out.get_mut(&event.name)
                        .expect("just looked it up")
                        .witnesses = merged;
                }
                Some(existing) => {
                    return Err(format!(
                        "source name `{}` is declared {:?}/{:?} in `{}` and {:?}/{:?} in `{}` - which \
                         applies would depend on load order",
                        event.name,
                        existing.role,
                        existing.in_tool_span,
                        existing.asset,
                        declared.role,
                        declared.in_tool_span,
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
}

impl RoleAuthorityPlan {
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
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    }
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
            if !entry.survives_event_derivation && !entry.outranks_a_tag {
                return Err(format!(
                    "role authority `{}` in `{}` grants no authority at all, so it states nothing - leave the                      entry out",
                    entry.id, file.id
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
        }
    }
    // A spelling the alias table folds and nothing declares would silently lose the authority it used to inherit
    // from being foldable. Forcing the decision is the whole point of separating the two facts.
    for spelling in crate::domain::sideml::ChatRole::declared_alias_spellings() {
        if !by_role.contains_key(*spelling) {
            return Err(format!(
                "`{spelling}` is a role spelling the alias table folds and no `role_authority` entry declares -                  so whether a payload stating it outranks a tagged name would depend on the folding table, which                  is a statement about spelling rather than about authority"
            ));
        }
    }
    Ok(plan)
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
            let shadowed = crate::domain::pricing::builtin_provider(&normalised);
            if !shadowed.is_empty() {
                return Err(format!(
                    "provider alias `{}` in `{}` names a value the catalogue already reads as provider \
                     `{shadowed}`, so the declaration could never take effect",
                    alias.system, file.id
                ));
            }
            // And the provider it names has to be one the catalogue knows, or the alias resolves to a name
            // nothing prices - which looks like a priced call and is not.
            if crate::domain::pricing::builtin_provider(&alias.provider) != alias.provider {
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
