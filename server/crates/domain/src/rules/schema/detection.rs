use super::*;

/// One `gen_ai.system` value, and the catalogue provider it means.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ProviderAlias {
    #[serde(default)]
    pub doc: Option<String>,
    /// The value as the producer writes it, **after** separator and case normalisation - which stays in Rust,
    /// since it is about spelling rather than about who wrote it.
    pub system: String,
    /// The provider in the catalogue's own vocabulary.
    pub provider: String,
}

/// One member name, and what its presence means.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct MessageMemberRule {
    pub id: String,
    #[serde(default)]
    pub doc: Option<String>,
    /// The member, in every spelling producers write it - **one declaration, one flag vector**.
    ///
    /// A list rather than one name because a spelling is not a fact about meaning, and as separate declarations
    /// the aliases drifted despite being colocated: `functionCall` was message-shaped *and* a content block while
    /// `function_call` was a content block only, which is a difference nothing recorded and nothing could
    /// enforce. Two spellings that genuinely mean different things stay separate declarations, where the
    /// difference is at least visible.
    pub members: Vec<String>,
    /// Where this sits among the members that hold content. Required when `holds_content` is set, and refused
    /// otherwise: a rank that orders nothing is a statement the engine does not read.
    #[serde(default)]
    pub rank: Option<i32>,
    /// This member holds a message's content, at the rank above.
    #[serde(default)]
    pub holds_content: bool,
    /// Its presence means the value is message-shaped, so it is not bare structured output to be wrapped.
    #[serde(default)]
    pub means_message_shaped: bool,
    /// Its presence means the value is a content **block** - so a block carrying it that no case recognised is
    /// a *malformed* block rather than plain data, and is reported as unknown instead of as JSON.
    #[serde(default)]
    pub means_content_block: bool,
}

/// What authority a **stated role** carries, as two independent facts.
///
/// They were fused, differently on each path: whether a stated role survives event-name derivation was a
/// hardcoded Rust list, and whether it outranks a *tagged attribute name* was that list **or** whatever the role
/// alias table happened to fold. The alias table's job is folding spellings onto four canonical roles, which is
/// not a statement about authority - so adding an alias silently granted it authority over a declared tag.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RoleAuthority {
    pub id: String,
    #[serde(default)]
    pub doc: Option<String>,
    /// The spelling, as a payload states it. Matched case-insensitively, so it is declared in lower case.
    pub role: String,
    /// A stated role of this spelling is **not** replaced by the role an event name declares.
    ///
    /// For roles nothing derives from a name: a tool invocation, a tool-definitions message, framework state.
    /// Deriving would overwrite the more specific fact with a guess.
    #[serde(default)]
    pub survives_event_derivation: bool,
    /// A stated role of this spelling outranks the name a *tagged attribute* reading was found under.
    ///
    /// Separate from the above because the questions differ: `tool` is authoritative over a tag and must **not**
    /// survive event derivation, since an event name is real evidence of it.
    #[serde(default)]
    pub outranks_a_tag: bool,
}

/// One classification rule: the conditions a span must satisfy, and what it is then.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ClassifyRule {
    pub id: String,
    #[serde(default)]
    pub doc: Option<String>,
    /// Where this sits in the ordered sweep. The first rule that holds answers.
    pub rank: i32,
    /// **Every** one of these must hold, while each is internally a disjunction of signals.
    ///
    /// Conjunction is what a single signal set cannot express, and three of these rules need it: an operation
    /// name *and* a model whose name says it is an embedding, an operation name *and* the system that gives it
    /// a different meaning, a dialect's model attribute *and* an operation id that says which call it was.
    pub all_of: Vec<DetectMatch>,
    /// What the span is, in the stored vocabulary. Mapped to the enum by the caller, which is the one thing
    /// about this that is not a producer's business.
    pub result: String,
    /// The retired heuristic sweep's answer when this rule intentionally corrects it.
    ///
    /// This keeps a reviewed migration delta beside the producer fact that justifies it, instead of
    /// teaching the framework-blind sweep or its corpus test about producer-specific span names.
    #[serde(default)]
    pub replaces_legacy_result: Option<String>,
}

/// One detection rule: signals that identify a producer, and the label they yield.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetectRule {
    pub id: String,
    #[serde(default)]
    pub doc: Option<String>,
    /// The label written to the span's `framework` column. A display and filtering value.
    pub label: String,
    /// Where this rule sits in the ordered sweep - a **migration bridge**, not the target design.
    ///
    /// The accepted design is order-independent: each rule states sufficient conditions, a unique
    /// sufficient candidate wins, and a declared `supersedes` resolves a known overlap. Today's rules do
    /// not state sufficient conditions - they were transcribed from a first-match table whose order is
    /// load-bearing, because the SideSeat SDK defaults `service.name` to one framework's name, so a
    /// service-name signal evaluated early claims every span of every framework using the SDK.
    ///
    /// So the rank reproduces that answer while the overlaps are *collected and reported* rather than
    /// silently resolved (`overlapping_candidates`). It goes when the predicates are narrow enough that
    /// no span has two candidates - and until then, naming it `legacy_rank` is the honest description of
    /// what it is.
    #[serde(rename = "legacy_rank")]
    pub legacy_rank: i32,
    /// Rule ids this rule beats where both match. Declared, so a genuine overlap is owned rather than
    /// resolved by a number.
    ///
    /// It **orders**, ahead of `legacy_rank`, which is what that field's own doc says the accepted design is. It
    /// used to waive only the overlap *report* while rank decided the winner regardless - so the field documented
    /// an ordering it took no part in, and every shipped edge could have been deleted without changing a single
    /// attribution. Transitive, since it is a DAG; a cycle is refused.
    ///
    /// This is what lets a rule beat one ranked ahead of it **without** moving its own weaker signals up too -
    /// the same problem `alternatives` solves within a rule, here between two.
    #[serde(default)]
    pub supersedes: Vec<String>,
    #[serde(rename = "match")]
    pub match_spec: DetectMatch,
    /// Further signal sets that must each match.
    ///
    /// A `DetectMatch` is deliberately disjunctive: every signal inside it is independently
    /// sufficient. Some producer identities need a conjunction instead, such as a shared cloud
    /// provider together with the model API used through it. Keeping those as separate sets
    /// preserves the useful disjunction within each set without hard-coding a producer
    /// combination in the engine.
    #[serde(default)]
    pub all_of: Vec<DetectMatch>,
    /// Further evidence for the same label, each at its **own** rank.
    ///
    /// The predicates inside one `match` are independently sufficient, so a rule whose signals differ in
    /// *strength* cannot be ordered by one number. Strands states `gen_ai.system: "strands-agents"` - a producer
    /// naming itself, the strongest evidence there is - beside a `service.name` the SDK defaults and a phrase in
    /// a span name. One rank for all three has to be placed for the weakest, which put the self-identification
    /// behind `openinference.`: a Strands span carrying any OpenInference attribute was labelled OpenInference,
    /// and moving the whole rule earlier would instead let its defaulted service name claim every framework using
    /// the SDK.
    ///
    /// Each alternative is compiled into its own ordered rule with the same label, so resolution is unchanged -
    /// what changes is that a rule can place its strong evidence and its weak evidence separately. An explicit
    /// `id` per alternative rather than one synthesized from the rule's, for the reason every other clause here
    /// has one: a synthesized id is not an identity a declaration can be held to.
    #[serde(default)]
    pub alternatives: Vec<DetectAlternative>,
}

/// One further body of evidence for a rule's label, at its own rank.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct DetectAlternative {
    pub id: String,
    #[serde(default)]
    pub doc: Option<String>,
    #[serde(rename = "legacy_rank")]
    pub legacy_rank: i32,
    #[serde(rename = "match")]
    pub match_spec: DetectMatch,
    /// Further signal sets that must each match.
    #[serde(default)]
    pub all_of: Vec<DetectMatch>,
}

/// One SDK-declared slug and the label it resolves to.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SdkSlug {
    pub slug: String,
    pub label: String,
}

/// A pair of strings - an attribute key and the value or substring it must hold.
///
/// Strict, like every other type here. It was the one predicate type without it, so
/// `{"key": …, "value": …, "ignore_case": true}` parsed and the flag was **discarded** - a comparison an
/// author had asked to be case-insensitive stayed case-sensitive, silently. Case folding is a *separate
/// dimension* (`attr_equals_ignore_case`), which is exactly the mistake this made easy to write.
#[derive(PartialEq, Eq, Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
}

/// A case-insensitive text search over named sources.
///
/// The one signal that is neither a prefix nor an equality: a framework whose spans are identified by a
/// phrase appearing somewhere in a name, in any of several spellings.
#[derive(PartialEq, Eq, Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct TextContains {
    /// `span_name`, or `attr:<key>`.
    pub sources: Vec<String>,
    /// Any of these, matched case-insensitively.
    pub needles: Vec<String>,
    /// Search only the **first source that has a value**, rather than all of them.
    ///
    /// The difference is a decision, not a nicety. Two attributes may hold two answers to one question - a
    /// request model and a response model - and searching both asks "does *either* say so" where the question
    /// was "does the one that applies say so". A request for `gpt-4o` answered by `text-embedding-3-small` is
    /// a chat completion whose response model is mislabelled, not an embedding call.
    #[serde(default)]
    pub first_present_source: bool,
}

/// The signals a detection rule may use. **Any** satisfied signal matches the rule.
///
/// Disjunctive, which is the existing behaviour and worth naming: each dimension is independently
/// sufficient. That is why a rule listing a broad `service_name` beside a narrow `attr_prefix` is not
/// "narrow" at all, and why rank matters.
#[derive(PartialEq, Eq, Debug, Default, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct DetectMatch {
    /// Span name **starts with** any of these.
    ///
    /// Prefix only. It used to mean "equals *or* starts with", which is not two operators - a prefix subsumes its
    /// own equality, so the equality arm could never be the reason a rule matched, and a literal that only differs
    /// by a separator was dead beside the bare one. Every asset already spells the separator it means
    /// (`autogen.`, `claude_code.`, `vertexai.`), which is what says this dimension is a prefix; the one that did
    /// not is the one whose second literal was dead.
    #[serde(default)]
    pub span_name: Vec<String>,
    /// Span name **is** any of these, exactly.
    ///
    /// Its own dimension, because a prefix cannot express it: a producer that names one span for its whole graph
    /// and its steps `Graph.step` needs "exactly `Graph`" and "under `Graph.`" as two statements. Folded into the
    /// prefix list, the bare name subsumed the separator form *and* claimed every unrelated span that merely
    /// starts with those letters.
    #[serde(default)]
    pub span_name_exact: Vec<String>,
    /// Instrumentation scope name **is** any of these, exactly.
    ///
    /// OpenInference instrumentors expose the concrete framework through the scope while their span
    /// attributes intentionally stay in the shared `openinference.*` vocabulary. Exact matching keeps
    /// that producer signal from turning a shared namespace fragment into a substring heuristic.
    #[serde(default)]
    pub scope_name: Vec<String>,
    /// Any span attribute key starts with any of these.
    #[serde(default)]
    pub attr_prefix: Vec<String>,
    /// A span attribute equals this value exactly.
    #[serde(default)]
    pub attr_equals: Vec<KeyValue>,
    /// A span attribute equals this value, whatever its case.
    ///
    /// Its own dimension rather than a flag on `attr_equals`, because a flag that changes another field's
    /// meaning is dead where that field is absent - and this engine refuses dead declarations. One producer
    /// writes its span kind in capitals and another in mixed case, and both mean the same kind.
    #[serde(default)]
    pub attr_equals_ignore_case: Vec<KeyValue>,
    /// Any of these span attribute keys exists.
    #[serde(default)]
    pub attr_exists: Vec<String>,
    /// The resource's `service.name` **contains** any of these.
    ///
    /// A substring test, which is why no rule may identify a framework by a short common word: `agno` would also
    /// match a service called `diagnostics`.
    ///
    /// It read `equals || contains`, and the equality arm was dead - a substring match subsumes its own equality.
    /// Narrowing the whole dimension to equality was tried and is **wrong**: the breadth is deliberate. A user
    /// names their own service, and `my-app-openai-agents-v1` identifies the SDK inside it
    /// (`test_openai_agents_framework_detection_from_service_name_contains`). Every `service.name` in the captured
    /// corpus that matches a declared literal happens to match it exactly, so the corpus cannot see this - which
    /// is what makes those two unit tests the evidence.
    ///
    /// The cost is real and accepted: a service called `my-strands-agents-proxy` is attributed to Strands. Telling
    /// that from `my-app-openai-agents-v1` needs evidence a resource attribute does not carry.
    #[serde(default)]
    pub service_name: Vec<String>,
    /// A *span* attribute contains this substring.
    ///
    /// Generic on purpose. This replaced a `metadata_contains` dimension whose key the engine supplied,
    /// which made one framework's attribute name look like part of OpenTelemetry: only one rule ever used
    /// it. `service.name` stays a structural dimension because it really is OTel's own resource
    /// attribute; `metadata` is a framework's, so the key belongs in the asset beside the value.
    #[serde(default)]
    pub span_attr_contains: Vec<KeyValue>,
    /// A resource attribute contains this substring - how an instrumentation library identifies itself
    /// through `telemetry.sdk.name`.
    #[serde(default)]
    pub resource_attr_contains: Vec<KeyValue>,
    /// A case-insensitive phrase search over the span name or a named attribute.
    #[serde(default)]
    pub text_contains: Option<TextContains>,
}

/// One carrier declaration: what to match, and what the matched carrier is evidence of.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CarrierRule {
    /// Stable clause id, reported by the explain trace.
    pub id: String,
    #[serde(default)]
    pub doc: Option<String>,
    #[serde(rename = "match")]
    pub match_spec: MatchSpec,
    /// The preset this clause resolves to, optionally with named overrides.
    pub facts: Facts,
    /// The ordering family this carrier belongs to, when it is a *fragmented ordered input*: several
    /// attribute keys that are one array (`llm.input_messages.0.message` and `.1.message`).
    ///
    /// The seventh carrier fact. It was hardcoded in the order resolver, which is exactly the shape of
    /// defect this engine exists to remove: a semantic fact about one framework's carrier, written in
    /// Rust, that no rule file could state.
    #[serde(default)]
    pub ordering_family: Option<String>,
}

/// A read-time projection decision for one producer-owned span shape.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct MessageProjectionRule {
    /// Stable clause id, reported by diagnostics.
    pub id: String,
    #[serde(default)]
    pub doc: Option<String>,
    #[serde(rename = "match")]
    pub match_spec: MessageProjectionMatch,
    pub action: MessageProjectionAction,
}

/// The stored row and extracted-message shape a projection rule recognises.
///
/// All dimensions are required so a producer rule cannot accidentally suppress a broad class of
/// ordinary input-only spans. The source condition means every extracted message must come from the
/// named attribute; an empty message list never matches.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct MessageProjectionMatch {
    pub scope_name: String,
    pub scope_version_major_at_least: u64,
    pub span_name_prefix: String,
    pub only_attribute_source: String,
    pub successful_only: bool,
}

/// What a matching read-time projection rule does.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageProjectionAction {
    SuppressMessages,
}

/// What an observation must look like for a clause to apply. Every field is optional and all present
/// fields must hold, so a clause constraining more dimensions is strictly more specific.
///
/// The dimensions are a fixed, finite set of orthogonal scalar constraints - which is what makes
/// specificity well-defined here, unlike the tree-shape predicates of the content chain, where
/// ordering has to be declared by name instead.
#[derive(Debug, Default, Deserialize, PartialEq, Eq, Clone)]
#[serde(deny_unknown_fields)]
pub struct MatchSpec {
    /// Exact OTel event name the observation was read from.
    #[serde(default)]
    pub event: Option<String>,
    /// Exact attribute key.
    #[serde(default)]
    pub attribute: Option<String>,
    /// Attribute key prefix, for indexed families (`llm.input_messages.0.message`).
    #[serde(default)]
    pub attribute_prefix: Option<String>,
    /// A dotted attribute **family**: the root itself and every key below it.
    ///
    /// Separate from `attribute_prefix`, which is raw text. Every undelimited prefix in the assets was really
    /// a family root, and the raw reading selected keys that are not in the family:
    /// `attribute_prefix: "ai.response"` matched `ai.responses`, and `gen_ai.output.messages` matched
    /// `gen_ai.output.messages_extra`. A prefix ending in `.` is genuinely raw and stays one.
    #[serde(default)]
    pub attribute_family: Option<String>,
    /// Any of these observation types. This is the dimension the span-blind lookup lacked: the same
    /// carrier name means different things on a generation span and on an aggregator.
    ///
    /// `Option`, not `Vec`, because an explicitly empty `observation_type: []` was silently the same as
    /// omitting the qualifier - so a clause that reads as narrow held for every span. Omission is `None` and
    /// means no restriction; `Some([])` states nothing and is refused. A `Vec` cannot tell those apart, which
    /// is why the first attempt at this refusal was dead code.
    #[serde(default)]
    pub observation_type: Option<Vec<String>>,
    /// Instrumentation scope name substring.
    ///
    /// Narrowing evidence only, never an exclusive key: historical rows may carry no scope, several
    /// frameworks share instrumentation packages, and versions are not reliably semantic.
    #[serde(default)]
    pub scope_name_contains: Option<String>,
    /// Instrumentation scope *version* prefix.
    ///
    /// A prefix rather than a comparison, deliberately: versions here are not reliably semantic, so
    /// `>=` would have to invent an ordering for strings that have none. A prefix states exactly what it
    /// checks. Present because a producer can change a carrier's meaning between releases, which is the
    /// one thing no other dimension can express.
    #[serde(default)]
    pub scope_version_prefix: Option<String>,
}

impl MatchSpec {
    /// How many of the three carrier fields this clause names. Exactly one is required: a clause naming
    /// none would match every observation of a span, and one naming several was silently reduced to
    /// whichever the indexer looked at first, quietly ignoring the rest.
    pub fn primary_key_count(&self) -> usize {
        usize::from(self.event.is_some())
            + usize::from(self.attribute.is_some())
            + usize::from(self.attribute_prefix.is_some())
            + usize::from(self.attribute_family.is_some())
    }

    /// The carrier this clause keys on, for indexing.
    pub fn primary_key(&self) -> Option<PrimaryKey<'_>> {
        if let Some(event) = &self.event {
            return Some(PrimaryKey::Event(event));
        }
        if let Some(attribute) = &self.attribute {
            return Some(PrimaryKey::Attribute(attribute));
        }
        if let Some(prefix) = &self.attribute_prefix {
            return Some(PrimaryKey::AttributePrefix(prefix));
        }
        self.attribute_family
            .as_deref()
            .map(PrimaryKey::AttributeFamily)
    }

    /// Does this clause's match language *contain* the other's - is the other at least as specific?
    ///
    /// Subsumption, not a score. A numeric score imposes an order on predicates that have none:
    /// `observation_type = agent` and `span_name_prefix = invoke_` constrain different things and
    /// neither implies the other, so any number assigned to them is arbitrary and decides real cases by
    /// arithmetic. Specificity is only meaningful where one clause's language is a strict subset of the
    /// other's, and where it is not, the ruleset is ambiguous and must say which wins.
    ///
    /// Returns true when every observation matching `other` also matches `self`.
    pub fn contains_language_of(&self, other: &Self) -> bool {
        // Carrier: an exact name is inside a prefix that covers it; a longer prefix is inside a shorter.
        let carrier_contains = match (self.primary_key(), other.primary_key()) {
            (Some(PrimaryKey::Event(a)), Some(PrimaryKey::Event(b))) => a == b,
            (Some(PrimaryKey::Attribute(a)), Some(PrimaryKey::Attribute(b))) => a == b,
            (Some(PrimaryKey::AttributePrefix(p)), Some(PrimaryKey::Attribute(a))) => {
                a.starts_with(p)
            }
            (Some(PrimaryKey::AttributePrefix(a)), Some(PrimaryKey::AttributePrefix(b))) => {
                b.starts_with(a)
            }
            // A family's members all start with its root, so a raw prefix that is a prefix *of the root*
            // covers every one of them.
            (Some(PrimaryKey::AttributePrefix(p)), Some(PrimaryKey::AttributeFamily(root))) => {
                root.starts_with(p)
            }
            (Some(PrimaryKey::AttributeFamily(root)), Some(PrimaryKey::Attribute(a))) => {
                in_family(a, root)
            }
            // A raw prefix is inside a family only when it is *below* the root. `prefix("ai.response")` is
            // **not**, because it also selects `ai.responses`, which the family excludes - the same distinction
            // that made this variant necessary.
            (Some(PrimaryKey::AttributeFamily(root)), Some(PrimaryKey::AttributePrefix(p))) => p
                .strip_prefix(root)
                .is_some_and(|rest| rest.starts_with('.')),
            (Some(PrimaryKey::AttributeFamily(a)), Some(PrimaryKey::AttributeFamily(b))) => {
                b == a || b.strip_prefix(a).is_some_and(|rest| rest.starts_with('.'))
            }
            _ => false,
        };
        if !carrier_contains {
            return false;
        }
        // Qualifiers: an unconstrained dimension contains any constraint on it.
        // An unconstrained dimension contains any constraint on it, so `None` here contains everything.
        let types = match (&self.observation_type, &other.observation_type) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(mine), Some(theirs)) => theirs.iter().all(|t| mine.contains(t)),
        };
        // A longer needle is the more specific claim only when it *contains* the shorter one: a scope
        // holding `foo` is not necessarily one holding `bar`, but one holding `foobar` does hold `oob`.
        let scopes = match (&self.scope_name_contains, &other.scope_name_contains) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(a), Some(b)) => b.contains(a.as_str()),
        };
        let versions = match (&self.scope_version_prefix, &other.scope_version_prefix) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(a), Some(b)) => b.starts_with(a.as_str()),
        };
        types && scopes && versions
    }

    /// Could one observation satisfy both clauses?
    ///
    /// Deliberately conservative: where it cannot be shown that no observation satisfies both, this says
    /// they overlap, so the compiler asks for an explicit ordering rather than assuming independence.
    /// Two `scope_name_contains` needles are the case that matters - a scope name can hold both `foo`
    /// and `bar`, so treating unequal needles as disjoint let two clauses both match with nothing
    /// choosing between them.
    pub fn can_both_match(&self, other: &Self) -> bool {
        let carriers_overlap = match (self.primary_key(), other.primary_key()) {
            (Some(PrimaryKey::Event(a)), Some(PrimaryKey::Event(b))) => a == b,
            (Some(PrimaryKey::Attribute(a)), Some(PrimaryKey::Attribute(b))) => a == b,
            (Some(PrimaryKey::Attribute(a)), Some(PrimaryKey::AttributePrefix(p)))
            | (Some(PrimaryKey::AttributePrefix(p)), Some(PrimaryKey::Attribute(a))) => {
                a.starts_with(p)
            }
            (Some(PrimaryKey::AttributePrefix(a)), Some(PrimaryKey::AttributePrefix(b))) => {
                // Either can extend the other, and some key beginning with the longer satisfies both.
                a.starts_with(b) || b.starts_with(a)
            }
            _ => false,
        };
        if !carriers_overlap {
            return false;
        }
        // Jointly satisfiable unless both constrain the dimension and share no value.
        let types = match (&self.observation_type, &other.observation_type) {
            (Some(mine), Some(theirs)) => mine.iter().any(|t| theirs.contains(t)),
            _ => true,
        };
        // Two substrings are always jointly satisfiable: concatenate them.
        let versions = match (&self.scope_version_prefix, &other.scope_version_prefix) {
            (Some(a), Some(b)) => a.starts_with(b.as_str()) || b.starts_with(a.as_str()),
            _ => true,
        };
        types && versions
    }
}

/// What a clause keys on, for the lookup index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrimaryKey<'a> {
    Event(&'a str),
    Attribute(&'a str),
    /// A **raw textual** prefix: every key beginning with these characters.
    AttributePrefix(&'a str),
    /// A dotted **family**: the root itself, and every key below it.
    ///
    /// Distinct from a raw prefix because a raw one does not respect the separator, and every undelimited
    /// prefix in the assets was in fact a family root: `attribute_prefix: "ai.response"` matched
    /// `ai.responses`, which is a different attribute, and `gen_ai.output.messages` matched
    /// `gen_ai.output.messages_extra`. A prefix ending in `.` is still genuinely raw and stays one - as a
    /// family root it would ask for `ai..something`.
    AttributeFamily(&'a str),
}

impl PrimaryKey<'_> {
    /// Whether this source selects the given attribute key.
    pub fn selects_attribute(&self, key: &str) -> bool {
        match self {
            Self::Event(_) => false,
            Self::Attribute(name) => key == *name,
            Self::AttributePrefix(prefix) => key.starts_with(prefix),
            Self::AttributeFamily(root) => in_family(key, root),
        }
    }
}

/// Whether a key is the root of a dotted family or sits below it.
///
/// `key == root || key.starts_with(root.)` - the separator is the whole point, and its absence is what made
/// `ai.responses` a member of `ai.response`.
pub fn in_family(key: &str, root: &str) -> bool {
    key == root
        || key
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('.'))
}

/// One shape a provider writes a tool definition in, and how to read it as the canonical one.
///
/// The canonical form - `{"type":"function","function":{"name","description","parameters"}}` - is **ours**, and
/// stays in Rust. Every path into a producer's own shape is the asset's, which is the split `as_tool_definition`
/// already follows. Before this, five readers named `openai`, `anthropic`, `bedrock`, `gemini` and `cohere` in
/// production Rust and an unrecognised shape was passed through unchanged, then discarded because no name could
/// be extracted from it - so a producer's shape was not addable as data.
///
/// Worth stating why the framework sweep never caught them: markers are derived from *asset ids*, and those five
/// are **providers**, which the sweep excludes by design because the pricing catalogue is entitled to their
/// names. A fourth blind spot beside the three its own doc records.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ToolShapeRule {
    pub id: String,
    pub doc: Option<String>,
    /// Ordered, first match wins, and a shared rank is refused - two shapes that both recognise a payload must
    /// not be separated by which asset loaded first.
    pub legacy_rank: i32,
    /// What makes a payload this shape. Read on the tool value itself.
    #[serde(default)]
    pub require: PredicateSet,
    /// Where the definitions are, when one payload holds several. Empty means the payload is one definition.
    ///
    /// Ordered, and the first path that **resolves** supplies them - the same rule as `ParametersSpec::from`,
    /// for the same reason: one producer writes `functionDeclarations` and another writes
    /// `function_declarations`, and those are two spellings of one shape rather than two shapes. Spelling them
    /// as two clauses would duplicate every other member of the rule and give the pair a rank order that means
    /// nothing.
    #[serde(default)]
    pub each: Vec<JsonPath>,
    /// The whole canonical `function` object, for a producer that already writes it.
    ///
    /// Exclusive with the three members below: a shape either hands over a canonical object or states where each
    /// part is, and declaring both would be two answers about one output.
    #[serde(default)]
    pub function: Option<JsonPath>,
    /// Members of the payload copied onto the canonical wrapper beside `function` - one producer carries
    /// `strict` there, and dropping it changes what the tool permits.
    #[serde(default)]
    pub carry: Vec<String>,
    #[serde(default)]
    pub name: Option<JsonPath>,
    #[serde(default)]
    pub description: Option<JsonPath>,
    #[serde(default)]
    pub parameters: Option<ParametersSpec>,
}

/// Where a tool's parameters are and how they are encoded.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ParametersSpec {
    pub doc: Option<String>,
    /// Ordered: the first path that resolves is the parameters. One producer writes
    /// `inputSchema.json` and the same producer sometimes writes `inputSchema` directly.
    pub from: Vec<JsonPath>,
    /// **Declared**, not guessed from the content. It was guessed: a member named `type` inside an argument map
    /// made the map look like a finished JSON Schema, so `{"type":"str","query":"str"}` was emitted as a schema
    /// whose type is `str`. The argument named `type` decided how the whole representation was read.
    pub encoding: ParametersEncoding,
}

/// How a producer encodes a tool's parameters.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ParametersEncoding {
    /// Already a JSON Schema object: taken as it stands.
    JsonSchema,
    /// A map from argument name to its facts - `{"city": {"type": "string", "required": true}}` - which becomes
    /// a JSON Schema object. Every supported constraint is kept: a converter that dropped `required` said an
    /// argument was optional when the producer said it was not.
    ArgumentMap,
}

/// The **nine** carrier facts, named by preset with optional per-field overrides.
///
/// A preset is a constructor, not a category: `snapshot` and `accumulated_state` differ in one bit, so two
/// declarations that read as different kinds of thing can be the same nine facts - and the name does not
/// survive compilation. 37 of the 55 shipped clauses override something, and nearly all of those overrides are
/// compensating for direction or encoding being bundled into a preset that is otherwise about *reconstruction*.
///
/// A preset plus overrides rather than six booleans spelled out per clause: the presets are the
/// vocabulary the model is stated in, and a clause that writes them all out invites one being wrong in
/// a way no reader notices.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Facts {
    /// `emission`, `snapshot` or `accumulated_state`.
    pub preset: String,
    #[serde(default)]
    pub position_proves_distinct_occurrence: Option<bool>,
    #[serde(default)]
    pub position_provides_sequence_order: Option<bool>,
    #[serde(default)]
    pub history_positions_provide_sequence_order: Option<bool>,
    #[serde(default)]
    pub carrier_is_atomic_emission: Option<bool>,
    #[serde(default)]
    pub may_restate_prior_observations: Option<bool>,
    pub may_contain_framework_state: Option<bool>,
    #[serde(default)]
    pub carrier_holds_span_output: Option<bool>,
    #[serde(default)]
    pub carrier_is_detached_request_frame: Option<bool>,
    #[serde(default)]
    pub carrier_holds_span_input: Option<bool>,
    #[serde(default)]
    pub carrier_holds_expandable_message_array: Option<bool>,
}

/// Every rule asset, keyed by path so compilation order is deterministic.
///
/// The compiler walks this map, and a collision diagnostic that named a different pair of files per
/// build would be untraceable.
pub fn embedded_sources() -> BTreeMap<String, Vec<u8>> {
    sideseat_rule_assets::sources()
}

/// BLAKE3 over the asset paths and bytes, hex-encoded.
///
/// Paths are hashed too, and length-prefixed, so moving a declaration between files changes the digest
/// and two files cannot be concatenated into the same hash as one.
pub fn digest_of(sources: &BTreeMap<String, Vec<u8>>) -> String {
    let mut hasher = blake3::Hasher::new();
    for (path, bytes) in sources {
        hasher.update(&(path.len() as u64).to_le_bytes());
        hasher.update(path.as_bytes());
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }
    hasher.finalize().to_hex().to_string()
}
