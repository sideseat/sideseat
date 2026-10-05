use super::*;

/// One message-extraction rule: a carrier to read, how to parse it, and what to emit.
///
/// **No longer small, and that is the finding.** It began as "read a carrier, parse it, emit it" and each
/// dialect added a generic field that prevented a measured defect. Every one is still declarative and
/// non-Turing-complete, but selection, projection, predicates and object construction have been built by
/// hand here - which is what an expression language already standardises, and a hand-built path resolver
/// is where a real bug lived (a literal dotted key read as a nested path).
///
/// So the shaping half of this type moved to a published selection language with a parser and quoted
/// identifiers. **RFC 9535 JSONPath**, not JMESPath: JMESPath was implemented first and reverted, because
/// every result comes back through its crate's sorted-map value tree, which alphabetises a selected payload's
/// members - and this repository treats serialised member order as observable. `message_rules.rs` records the
/// measurement. What stays here is the structural half - which carrier, who claims it, in what order, what it
/// emits - because that is ownership and policy rather than a transform.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MessageRule {
    pub id: String,
    #[serde(default)]
    pub doc: Option<String>,
    /// The *events* this rule applies to. Non-empty makes it an event rule: its reads resolve against the
    /// event's own attributes rather than the span's, and its observations are tagged as events.
    ///
    /// **Where** this rule reads: a span's attributes at some stage, or a named event's.
    ///
    /// One discriminated member, because `when_event` and `stage` were an *implicit sum* and an unsound one.
    /// The two entry points disagreed about which fields they honour: `from_event` selects on the event name
    /// and ignores `stage` entirely, while the span path selects on `stage` and requires no event. So two
    /// event rules declaring different stages were treated by the compiler as separate ordering arenas -
    /// where a shared rank is legal - and then both run by the event path, with ownership decided by
    /// comparing their *ids*. And `when_event: []` silently became an ordinary span rule.
    ///
    /// Absent means `{"span": {}}`, the ordinary case: read with the dialects on a span's attributes.
    #[serde(default)]
    pub source: Option<MessageSource>,

    /// The carrier to read. Absent for a `compose` rule, which has many sources rather than one.
    #[serde(default)]
    pub read: ReadSpec,
    /// Assemble one message from several attributes, rather than wrapping one read value.
    ///
    /// The dual of `wrap`, and needed because a dialect writes one response across many keys - the text
    /// here, the tool calls there, a structured object beside them, and any other member of the same
    /// family swept up. There is no single carrier to read, so there is no single value to wrap.
    #[serde(default)]
    pub compose: Option<ComposeSpec>,
    #[serde(default)]
    pub tool_repr: Option<ToolReprSpec>,
    /// How to turn its raw string into a value. Absent for an indexed family, which has no single
    /// string to parse - each member is read on its own.
    #[serde(default)]
    pub parse: Option<ParseMode>,
    /// Wrap the parsed value in a message envelope with this role.
    ///
    /// Some carriers hold a bare payload rather than a message - a tool's arguments, say - and the role
    /// that payload represents is a fact about the carrier, so it is declared beside it.
    #[serde(default)]
    pub wrap: Option<WrapSpec>,
    /// Whether the observation is a message or a tool definition.
    ///
    /// Defaults to a message, and a branch set's parent declares none: its sub-readings each say what they
    /// emit, so a value here would be unused.
    #[serde(default)]
    pub emit: Option<EmitTarget>,
    /// Emit one observation whose value is the array of everything read, rather than one per reading.
    ///
    /// Tool definitions arrive as a set rather than a sequence of messages, so a dialect's whole tool list
    /// is one observation - emitting one per tool would make each look like a separate declaration.
    #[serde(default)]
    pub aggregate_into_array: Option<bool>,
    /// A gate on the span, in the detection vocabulary: the rule is consulted only where this holds.
    ///
    /// Several extractors refuse to read a carrier whose name they share with other dialects unless the
    /// span also carries their own marker - `gen_ai.prompt` is the generic conventions' key and also
    /// where one exporter writes a whole request, so reading it unconditionally would claim another
    /// dialect's payload.
    #[serde(default)]
    pub when: Option<DetectMatch>,
    /// Restrict this reading to one OpenTelemetry instrumentation scope.
    ///
    /// This is producer evidence carried by the telemetry itself, not a framework label inferred by
    /// SideSeat. Exact scope names keep a carrier shared by several OpenInference integrations from being
    /// interpreted as though every integration emitted the same payload shape.
    #[serde(default)]
    pub instrumentation_scope: Option<InstrumentationScopeMatch>,
    /// Ordered readings of the parsed value, tried until one yields an observation.
    ///
    /// An ordered coalesce, not a program: a payload has more than one documented shape and the rule
    /// says which to try first. Empty means "emit the parsed value as it stands", which is what the
    /// three dialects migrated first needed.
    #[serde(default)]
    pub alternatives: Vec<Alternative>,
    /// Readings that all contribute, rather than the first that yields.
    ///
    /// A different relation from `alternatives`, and mixing them up loses messages: one dialect writes a
    /// turn's history under one member and *the answer itself* under another, so reading them as
    /// alternatives dropped the assistant output of every run that carried history.
    ///
    /// **May be declared beside `alternatives`**, and one dialect does. They were refused together at first,
    /// on the reading that "first wins" and "all contribute" cannot both be true of one list - but they are
    /// two lists, and a carrier can hold both a shape to choose among *and* a member to read as well. The
    /// refusal pushed that carrier into two rules, which the ownership check then rejected for contending over
    /// one attribute.
    #[serde(default)]
    pub also: Vec<Alternative>,
    /// A reading used only when nothing else in this rule emitted anything.
    ///
    /// Keeps a span from being silently empty: where a payload matches no documented shape, it is better
    /// to keep it whole than to return nothing and leave no trace that it arrived.
    #[serde(default)]
    pub fallback: Vec<Alternative>,
    /// Tag the observation with this carrier, whatever alternative was read.
    ///
    /// Normally the key found is the tag, so two spellings of a payload stay distinguishable. One dialect
    /// deliberately does the opposite: it reads a renamed key but reports the canonical one, so every
    /// span's prompt is tagged alike whichever spelling it used. Declared, because it is the *reverse* of
    /// the default and a reader would otherwise assume the default.
    #[serde(default)]
    pub tag_as: Option<String>,
    /// May this rule read a *tool execution* span?
    ///
    /// A tool span reads the conventions and nothing else. The reason is that a tool span carries the
    /// call and its result under the conventions' own keys, while a dialect's broader carriers on the same
    /// span hold the enclosing agent's state - read there they duplicate the turn. That was a hardcoded
    /// exemption for one extractor by name; it is a property of a rule now.
    #[serde(default)]
    pub reads_tool_spans: Option<bool>,
    /// Several carrier readings with a *local* order between them.
    ///
    /// One dialect reads a carrier only when nothing else supplied the conversation - a condition about
    /// what *else* was found. Expressing that with rule ids would make ids into control-flow targets and
    /// the interpreter's execution history into something a rule can observe; keeping the readings inside
    /// one rule keeps it a pure function of the span, with no global state and no cycles to worry about.
    #[serde(default)]
    pub branch_set: Option<BranchSet>,
    /// Read an array-valued carrier element by element, in declared passes.
    ///
    /// Passes rather than per-element routing, because the order is observable: one dialect emits every
    /// recognised event *before* any grouped block, so a single interleaved scan would return a different
    /// conversation. Declaring passes makes that ordering a statement rather than an artefact of how the
    /// code happened to loop.
    #[serde(default)]
    pub elements: Option<ElementsSpec>,
    /// Apply this rule's readings at every node of a bounded tree walk.
    ///
    /// One dialect's carrier is a *state object* its nodes write into, so a conversation can sit at the top
    /// level, under one member, or nested a level or two down. Bounded on purpose - an explicit depth, and
    /// members already read at each node are pruned - because the point is to find a state member, not to
    /// trawl a payload for anything message-shaped.
    #[serde(default)]
    pub walk: Option<WalkSpec>,
    /// Split a text carrier into tagged sections and route each by its tag.
    ///
    /// A general text-carrier capability, and one dialect needs it: a single attribute holds either side
    /// of a conversation, distinguished by a bracketed tag, and several sections joined by a separator -
    /// one per parallel tool call. Each is read on its own so every result keeps its own id.
    #[serde(default)]
    pub sections: Option<SectionsSpec>,
    /// Reject a carrier whose value is blank once trimmed.
    ///
    /// Distinct from `require_non_empty`, which rejects only the empty string: one dialect treats
    /// whitespace as absence and another does not, and collapsing the two would change both.
    #[serde(default)]
    pub require_non_blank: Option<bool>,
    /// Skip a carrier whose value is empty.
    ///
    /// An attribute present and empty is not evidence of a message, and wrapping it produces a turn with
    /// nothing in it - which the no-empty-content invariant then rejects downstream.
    #[serde(default)]
    pub require_non_empty: Option<bool>,
    /// A negative gate: the rule is skipped where this holds.
    ///
    /// Symmetric to `when`, and needed for a genuine either/or - a response is read from its text when it
    /// has text, and from its tool calls only when it does not, or one response would be emitted twice.
    #[serde(default)]
    pub unless: Option<DetectMatch>,
    /// What an indexed entry must carry to count as one.
    ///
    /// An index exists as soon as *any* key mentions it, and a family legitimately holds keys that are not
    /// messages - so without this a request's settings each become a turn.
    ///
    /// Each member declares how its presence is decided, because the dialects genuinely differ and the
    /// difference is observable: one writes `content` directly *and* `content.0.text`, so either proves it,
    /// while another writes `contents.0.type` and never a bare `contents`, so only a nested key does. A
    /// single rule for all of them would either miss entries or invent them.
    #[serde(default)]
    pub require_members: Option<MemberRequirements>,
    /// Position in the consulted order. See `MessagePlan` for why it is `legacy_`.
    ///
    /// Required at the top level and **forbidden** inside a branch set: there the local order decides, so a
    /// rank would be a number that looks like it means something and does not.
    #[serde(rename = "legacy_rank", default)]
    pub legacy_rank: Option<i32>,
}

/// The carrier a message rule reads: exactly one form, checked at compile time.
///
/// Optional fields rather than a tagged enum, for the same reason the carrier match spec uses them: an
/// externally-tagged enum needs `{"attribute": {"attribute": "k"}}` in JSON, which is the shape nobody writes
/// and serde rejects silently at the file level. Requiring exactly one is the check that makes this equivalent
/// while staying readable.
///
/// There is deliberately **no `event` form**. One existed, was accepted by this schema, and was refused
/// unconditionally by the compiler as unimplemented - so the format advertised four read forms and could
/// execute three. An author reading the schema as the format's reference was being told something untrue,
/// which is worse than the missing capability: a span's *events* are routed to `MessagePlan::from_event`,
/// where `when_event` selects the rule and the event's attributes are read exactly as a span's are. If a rule
/// ever needs to read one event while running over a span, that is a new construct to design rather than a
/// field to un-refuse.
#[derive(Debug, Deserialize, Clone, Default)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ReadSpec {
    #[serde(default)]
    pub attribute: Option<String>,
    /// Ordered carrier alternatives: **the first** of these the span carries is read, and the observation
    /// is tagged with that key.
    ///
    /// A dialect that renamed a key keeps accepting the old one, and the tag has to be the key actually
    /// found or two spans carrying different spellings would be indistinguishable downstream.
    ///
    /// Spelled `first_present` rather than `attribute_any_of` because that name did not say **how many** of
    /// the listed keys are read, and the answer depended on a *sibling* member: with `tool_repr` beside it,
    /// every present key was read; without, only the first. So one syntax meant two things - for CrewAI's
    /// `["crew_agents", "crew_tasks"]`, both or just the first - and which was decided somewhere else in the
    /// rule.
    #[serde(default)]
    pub first_present: Vec<String>,
    /// **Every** one of these keys the span carries is read, each as its own observation.
    ///
    /// The other half of the split above. A framework may write the same tools under several keys at
    /// different richness, and each is its own observation - so all are read and the best copy per name wins
    /// downstream, rather than the richest hiding behind whichever key was declared first.
    ///
    /// Honoured by `tool_repr` alone today, and a rule declaring it with any other body is **refused**
    /// rather than silently read as `first_present`. Generalising it - every body iterating its carriers -
    /// is the natural extension and is not what the corpus needs yet.
    #[serde(default)]
    pub each: Vec<String>,
    /// An *indexed attribute family*: `<prefix>.0.role`, `<prefix>.0.content`, `<prefix>.1.role`, ...
    ///
    /// One entry per index, each assembled from every key under it with the prefix stripped. This is an
    /// OpenTelemetry encoding - a list of objects flattened into dotted keys because attributes are a
    /// flat map - so reading it is a generic capability, not a producer's policy. The carrier each entry
    /// is tagged with is `<prefix>.<index>`, which is what makes two turns of one family distinguishable
    /// downstream.
    #[serde(default)]
    pub indexed_family: Option<String>,
    /// A predicate applied to each fully assembled indexed entry.
    ///
    /// `require_members` decides whether the flattened carrier contains enough physical members to form an
    /// entry. This is the semantic counterpart: it sees the resulting object and can keep only entries whose
    /// values mean something to this reading. One OpenInference integration, for example, writes framework
    /// bookkeeping beside actual conversation roles in the same family.
    ///
    /// Optional rather than an empty default so an explicitly empty predicate can be refused as a dead
    /// declaration. Absent means every entry that satisfies `require_members` is read.
    #[serde(default)]
    pub entry_require: Option<PredicateSet>,
    /// A **named** attribute family: every key under a root, each key's value one observation.
    ///
    /// `indexed_family`'s counterpart for a family whose members are *names* rather than indices -
    /// `acme.messages.a`, `acme.messages.b`. That was inexpressible: `indexed_family` requires a numeric
    /// component and skips anything else, while `carriers` has had `attribute_family` all along, so the two
    /// halves of the format disagreed about whether such a family exists.
    ///
    /// The **order is declared**, and required, because there is no order to discover: extraction puts a
    /// span's attributes in a `HashMap`, so producer order is gone by the time a rule reads them. An
    /// undeclared order would be the iteration order of a hash map - different per run, and the one thing a
    /// message sequence must not be.
    #[serde(default)]
    pub attribute_family: Option<AttributeFamilySource>,
    /// A sub-level of each indexed entry whose members are read at the top of the object.
    ///
    /// One dialect nests the message inside the entry - `<prefix>.0.message.role` - while also putting
    /// entry-level members beside it. Both are collected, the sub-level's names unprefixed and the rest as
    /// they stand, and the observation is tagged with `<prefix>.<index>.<member>` because that is the
    /// payload it came from.
    #[serde(default)]
    pub entry_member: Option<String>,
    /// Entry members to read as a number where the text is one.
    ///
    /// OTel attributes are strings, so a relevance score arrives as `"0.9"` - and a score is a number.
    /// Named rather than sniffed, because a version, an id or a postcode is text that happens to parse.
    #[serde(default)]
    pub numeric_members: Vec<String>,
    /// Read one *value* out of each indexed entry, rather than the entry's assembled members.
    ///
    /// A family whose entries each hold a single serialised payload - one tool's JSON schema at
    /// `<prefix>.<n>.tool.json_schema` - is a list of those payloads, not a list of objects with a member
    /// called `tool`. The projection says which leaf is the datum.
    #[serde(default)]
    #[cfg_attr(test, schemars(with = "Option<String>"))]
    pub entry_value: Option<JsonPath>,
    /// How the projected value is read. `json` **drops** an entry whose payload does not parse, which is
    /// what a schema that failed to parse always meant - an indexed member is otherwise sniffed, and a
    /// malformed schema would be emitted as the string it is, reported as a tool definition.
    #[serde(default)]
    pub entry_value_parse: Option<ParseMode>,
    /// A richer copy of these same messages, held by another carrier and matched by position.
    #[serde(default)]
    pub overlay: Option<OverlaySpec>,
}

/// One dialect's evidence for a fact about a span.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SpanFactRule {
    pub id: String,
    pub doc: Option<String>,
    /// The fact this is evidence of.
    pub fact: SpanFact,
    /// Any one of these establishes it.
    pub signals: Vec<SpanSignal>,
}

/// A fact about a span that rules and readers ask about by name.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SpanFact {
    /// The span *is a tool running*, so its messages are that tool's input and result rather than a
    /// model's turn. Rules that must not read such a span are gated on it (`reads_tool_spans`).
    ToolExecution,
}

/// One piece of evidence. At least one form, and both together read as a conjunction.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SpanSignal {
    /// This clause's own name, unique within the rule or fragment that holds it.
    ///
    /// Required, because an emission has to be able to say **which** clause answered. Before this, a rule with
    /// four readings reported only the rule's id: a direct shape and a shape reached through a fragment were
    /// indistinguishable in a diagnostic, and `doc` was being used as a stand-in for an identity.
    ///
    /// Required rather than optional-with-a-derived-fallback, which was considered and is the worst of the
    /// three: adding an id later would *change* the clause's identity, a positional edit would change every
    /// identity after it, and nothing could safely reference one.
    pub id: String,
    pub doc: Option<String>,
    /// An attribute with this value.
    #[serde(default)]
    pub attr_equals: Option<KeyValue>,
    /// Compare that value case-insensitively. One convention writes its span kind in capitals.
    #[serde(default)]
    pub ignore_case: bool,
    /// **Every** one of these attributes is present. A conjunction, not a choice: a tool name alone sits on
    /// a model span that merely mentions a tool, while the name *and* a call id together are a call
    /// being run.
    #[serde(default)]
    pub attrs_present: Vec<String>,
}

/// Tool definitions a carrier holds as a language's `repr` rather than as JSON.
///
/// The grammar is sealed in `rules::tool_repr` because it is a property of the *language*. Everything a
/// particular framework calls its own - which member holds the tools, which repr fields name them, which
/// labels its embedded documentation uses, how its type names map to JSON Schema's - is here, because
/// that is its vocabulary and not a fact about Python.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ToolReprSpec {
    pub doc: Option<String>,
    /// The carrier's entries. One entry at a time, so two entries each holding a list interleave as the
    /// payload has them rather than by path - which is what keeps the reported order the framework's own.
    #[cfg_attr(test, schemars(with = "String"))]
    pub entries: JsonPath,
    /// Where an entry holds tools, in order. Each resolved value is tried as a tool definition.
    #[cfg_attr(test, schemars(with = "Vec<String>"))]
    pub candidates: Vec<JsonPath>,
    /// The repr fields naming a tool and its documentation - `name='search'`.
    pub name_field: String,
    pub description_field: String,
    /// The labels the embedded documentation uses.
    pub name_label: String,
    pub description_label: String,
    pub arguments_label: String,
    /// What makes a string a `repr` rather than a bare tool name. Without these a name containing a space
    /// would be parsed as a repr and yield nothing.
    pub repr_markers: Vec<String>,
    /// Where a tool object states its parameters, in order.
    pub parameter_members: Vec<String>,
    /// The repr fields that may follow a loosely-quoted one. Their appearance is where that value ends.
    ///
    /// A single-quoted value holding a dict repr contains unescaped single quotes, so its closing quote
    /// cannot be found by scanning - the value runs either to the `')` that closes the constructor or to
    /// the next field. Which fields those are is the framework's vocabulary, not the language's.
    #[serde(default)]
    pub field_terminators: Vec<String>,
    /// The language's type names, mapped to JSON Schema's. Compared case-insensitively, and ordered
    /// because the first match wins.
    pub type_map: Vec<(String, String)>,
    /// What an argument whose type name the map does not hold becomes.
    ///
    /// A tool whose argument type is unrecognised is still a tool, so the definition is reported rather than
    /// dropped - but *how* it is reported was described as "the widest type" and was `"string"`, which is the
    /// opposite: a schema saying `type: string` **rejects** a number. An unconstrained schema has no `type` at
    /// all, and `Unconstrained` is that.
    pub type_default: UnknownType,
}

/// What an argument type the map does not name becomes.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum UnknownType {
    /// No `type` member at all, which is what "the widest type" means in JSON Schema - it constrains nothing.
    Unconstrained,
    /// A named JSON Schema primitive, for a producer whose unrecognised names really are one kind of thing.
    /// Validated against the primitives, so a typo is not a schema every reader ignores.
    MapTo(String),
}

impl UnknownType {
    /// The JSON Schema primitives. A target outside these is a member no validator acts on, so a mapping to
    /// one is a declaration that does nothing.
    pub const PRIMITIVES: &'static [&'static str] = &[
        "array", "boolean", "integer", "null", "number", "object", "string",
    ];
}

/// Another carrier of the same span describing the same messages at higher fidelity.
///
/// Not an enrichment of what some other reader produced - both carriers are attributes of one span, and
/// the join is positional: entry *n* of the family and member *n* of the other carrier's list are the
/// same message. A flattened family loses whole content blocks and redacts urls, while the serialised copy
/// beside it keeps them, so where both describe one message the richer one is preferred.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct OverlaySpec {
    pub doc: Option<String>,
    /// The attribute holding the richer copy.
    pub from: String,
    #[serde(default)]
    pub parse: Option<ParseMode>,
    /// Ordered paths to the counterpart list; the first that resolves to an array is used.
    #[cfg_attr(test, schemars(with = "Vec<String>"))]
    pub select_any_of: Vec<JsonPath>,
    /// Unwrap a list of exactly one list. A serialiser that accepts a batch of conversations writes one
    /// conversation as a batch of one, and the members of *that* are the messages.
    ///
    /// Exactly one, deliberately: a batch of two is two conversations, and joining a family by position
    /// against the first of them would attribute one conversation's content to another's messages.
    #[serde(default)]
    pub unwrap_single_element_list: bool,
    /// What the list must look like to be this dialect's own serialisation. Without it, any array of
    /// objects at that path would be treated as the same messages.
    #[serde(default)]
    pub witness: PredicateSet,
    /// Only entries carrying members under this prefix are overlaid - the flattened form of the content
    /// that is known to be lossy.
    pub when_member_prefix: String,
    /// Ordered paths to the counterpart's content.
    #[cfg_attr(test, schemars(with = "Vec<String>"))]
    pub content_any_of: Vec<JsonPath>,
    /// What that content must be for the overlay to be an improvement.
    #[serde(default)]
    pub require: PredicateSet,
    /// The member the content becomes, replacing every member under `when_member_prefix`.
    pub as_member: String,
}

impl ReadSpec {
    /// How many carriers this names. Exactly one is required.
    pub fn named_count(&self) -> usize {
        usize::from(self.attribute.is_some())
            + usize::from(self.indexed_family.is_some())
            + usize::from(!self.first_present.is_empty())
            + usize::from(!self.each.is_empty())
            + usize::from(self.attribute_family.is_some())
    }
}

/// An attribute of one of a span's events.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct EventAttributeSource {
    /// The event whose attributes are read.
    pub event: String,
    /// The attribute on that event.
    pub attribute: String,
    /// Which occurrence answers, when a span carries the event more than once.
    ///
    /// Explicit because there is no defensible default: a span with two `gen_ai.choice` events has two
    /// answers, and taking the first silently is what the retired hand-written loop did with a `break`.
    #[serde(default)]
    pub occurrence: EventOccurrence,
}

/// Which occurrence of a repeated event supplies the value.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum EventOccurrence {
    /// The first occurrence that holds the attribute at all. What the retired reader did.
    #[default]
    FirstYielding,
    /// Every occurrence that holds it, in the order the span carries them. For a list-valued target, where
    /// two events genuinely mean two values.
    Every,
}

/// A family of attributes under one root, read as one observation per member.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AttributeFamilySource {
    /// The root. A key belongs to the family when it is `root` followed by `.` and a member name - the same
    /// rule `carriers`' `attribute_family` uses, so `acme.messages` does not claim `acme.messages_extra`.
    pub root: String,
    /// The order the members are read in.
    pub order: AttributeFamilyOrder,
}

/// The order a named family's members are read in.
///
/// One variant, and required rather than defaulted: the point of the member is that the order is a *statement*.
/// A default would be the thing a reader assumes and the format would say nothing about it, which is how the
/// answer becomes a hash map's iteration order.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum AttributeFamilyOrder {
    /// Lexicographic by the member name. Deterministic, and the only order available - producer order is lost
    /// when the attributes become a map.
    MemberName,
}

/// How a raw attribute string becomes a value.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ParseMode {
    /// Parse as JSON; skip the carrier entirely if it does not parse.
    Json,
    /// Parse as JSON, keeping the raw text as a string if it does not parse.
    JsonOrString,
    /// Parse as JSON, then parse any *string* member of the resulting array as JSON too.
    ///
    /// An OTLP array attribute whose elements are each a serialised object arrives as an array of strings,
    /// because the attribute type has no nesting. Generic: the encoding is OTLP's, not a producer's.
    StringifiedArray,
    /// Parse a Python constructor `repr` into a JSON tree.
    ///
    /// The accepted grammar is deliberately smaller than Python: constructor calls, JSON-shaped
    /// containers and scalars, and enum reprs. Constructor names and positional arguments remain explicit,
    /// so producer rules can select their own fields without putting class names in Rust.
    PythonConstructorRepr,
    /// Parse a JSON array whose every element is a Python constructor `repr`, each into a JSON tree.
    ///
    /// A serialised list of framework objects. All or nothing: an element that is not such a repr means the
    /// carrier is some other shape, and it is left to the rules that read that shape.
    PythonConstructorReprArray,
    /// Keep the raw text. Some carriers hold prose, and parsing it would turn a bare word into a
    /// non-string or an accidental number into a number.
    Text,
}
