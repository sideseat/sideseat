use super::*;

// ============================================================================
// VALUE PREDICATES
// ============================================================================

/// A condition on a JSON value, or on a member of it.
///
/// One vocabulary for every question the rules ask *about a value*: whether an alternative's shape holds,
/// whether a source is eligible, whether a section is dropped. Before this there were three bespoke
/// spellings - a member-name list, an `is_object` flag, and a fused "capture lacks prefix and body starts
/// with" pair - and a fourth was about to be added for a dialect that needs "this member is an object, or
/// that one is a non-empty string". Three narrow predicates are harder to reason about than one, and the
/// fused pair was producer policy wearing a generic name.
///
/// Deliberately *not* used for attribute-key presence (`MemberRequirements`): that asks about a flat map of
/// dotted keys, where "nested" means "some other key starts with this one". Same word, different domain -
/// and one type spanning both would have to mean different things depending on where it was used.
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct ValuePredicate {
    /// Why this condition is the right one, where that is not obvious from the condition. A field rather
    /// than a comment, as everywhere else here, because the explain trace surfaces it.
    #[serde(default)]
    pub doc: Option<String>,
    /// A JSONPath to the value under test. Absent means the value itself.
    #[serde(default)]
    pub path: Option<JsonPath>,
    /// The member must be present. Implied when the predicate names nothing else.
    #[serde(default)]
    pub exists: Option<bool>,
    /// The value's JSON kind.
    #[serde(default)]
    pub kind: Option<ValueKind>,
    /// A string, array or object must not be empty. Meaningless for other kinds, and refused there.
    #[serde(default)]
    pub non_empty: Option<bool>,
    /// The value begins like an identifier - a letter, a digit or an underscore.
    ///
    /// Generic in form, and declared per rule rather than folded into the tool-definition constructor: one
    /// dialect reports a synthetic aggregate under a name in parentheses, which is not a tool anyone can
    /// call, while another dialect's carriers have never needed the test. Making it canonical would change
    /// what every other carrier accepts, silently.
    #[serde(default)]
    pub identifier_like: Option<bool>,
    /// The value is not JSON null. Distinct from `exists`, which a null member satisfies, and from
    /// `non_empty`, which is about a string, array or object having contents.
    #[serde(default)]
    pub not_null: Option<bool>,
    /// A string must start with this.
    #[serde(default)]
    pub starts_with: Option<String>,
    /// A string must *not* start with this.
    #[serde(default)]
    pub lacks_prefix: Option<String>,
    /// The value must be one of these strings. An absent member satisfies nothing.
    #[serde(default)]
    pub one_of: Vec<String>,
    /// The value must not be any of these strings.
    ///
    /// An **absent** member satisfies this: "its value is not one of these" is true when there is no value,
    /// which is how a dialect's unnamed events fall through to the reading that handles them.
    #[serde(default)]
    pub none_of: Vec<String>,
}

/// A JSON kind, for `ValuePredicate::kind`.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ValueKind {
    Object,
    Array,
    String,
    Number,
    Bool,
    Null,
}

/// A set of value predicates, combined.
///
/// `all` and `any` both, because the dialects need both and the difference is real: a request's message
/// needs a role *and* content, while a response may carry either a structured message *or* streamed text.
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PredicateSet {
    #[serde(default)]
    pub all: Vec<ValuePredicate>,
    #[serde(default)]
    pub any: Vec<ValuePredicate>,
    /// The boolean-grammar form of this set, built once.
    ///
    /// Evaluation goes through the grammar, so there is one evaluator rather than two and the retired shell is
    /// a `#[cfg(test)]` oracle. Note what that does **not** yet buy: two compatibility translations keep the
    /// pre-grammar answers, so `Truth::Unknown` still never reaches a decision the shipped rules make - see
    /// `the_predicate_semantics_have_not_migrated_and_here_is_what_still_answers_the_old_way`.
    ///
    /// Built **once per set** rather than per evaluation because translating an expression per element per
    /// reading is work with no purpose. It is *not* a measured speedup: interleaved against the pre-migration
    /// build on `bench_ingestion` (`langgraph/swarm`), 76.95 ms became 77.02 ms - within noise either way. A
    /// separate-run comparison suggested 9%, which was this host's load rather than the change, which is why
    /// the convention here is to interleave.
    ///
    /// A `OnceLock` rather than a compile-time field, because a `PredicateSet` is reached through ten different
    /// spec structures and threading a compiled twin through each would put the same fact in two places. `Sync`,
    /// because the compiled plan is shared across request threads.
    #[serde(skip)]
    compiled: std::sync::OnceLock<Option<crate::rules::expr::JsonExpr>>,
}

/// Cloned **without** the cached expression: a clone recomputes it, which is correct because the cache is a
/// memo over the set's own contents and a `OnceLock` cannot be copied.
impl Clone for PredicateSet {
    fn clone(&self) -> Self {
        Self {
            all: self.all.clone(),
            any: self.any.clone(),
            compiled: std::sync::OnceLock::new(),
        }
    }
}

impl PredicateSet {
    /// Nothing to check.
    pub fn is_empty(&self) -> bool {
        self.all.is_empty() && self.any.is_empty()
    }

    /// The grammar form, built on first use.
    ///
    /// `None` where the set declares nothing, which is a different answer from an expression that is false: a
    /// set with no predicates places no condition, so it holds.
    pub fn expression(&self) -> Option<&crate::rules::expr::JsonExpr> {
        self.compiled
            .get_or_init(|| crate::rules::expr::json_expr_of(self))
            .as_ref()
    }
}

/// An array-valued carrier read element by element.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ElementsSpec {
    /// The emitted carriers are *events*, not attributes.
    ///
    /// A real distinction, not bookkeeping: carrier semantics are declared per carrier and looked up by
    /// which kind it is, so an event reported as an attribute gets a different reading of what it is
    /// evidence of. These elements *are* events - the dialect packs them into one attribute because
    /// attributes are all it has.
    #[serde(default)]
    pub tags_are_events: bool,
    /// A JSONPath to the array. Absent means the parsed value itself.
    #[serde(default)]
    pub select: Option<JsonPath>,
    /// Passes over the elements, in order. Each scans every element.
    pub passes: Vec<ElementPass>,
}

/// One pass over the elements.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ElementPass {
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
    #[serde(default)]
    pub doc: Option<String>,
    /// Which elements this pass reads.
    #[serde(default)]
    pub when: PredicateSet,
    /// Emit the element itself, tagged with the value at this path.
    ///
    /// A carrier named by the *data* rather than by the rule: these elements are events, and an event's
    /// name is what downstream keys role derivation and ordering on, so tagging them all alike would erase
    /// the distinction the payload carries.
    #[serde(default)]
    pub tag_from: Option<JsonPath>,
    /// Instead of emitting each element, group runs of them and emit one message per run.
    #[serde(default)]
    pub group: Option<GroupSpec>,
}

/// Runs of consecutive elements collapsed into one message.
///
/// Bounded: one pass, no recursion, and a run ends as soon as the derived key changes.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct GroupSpec {
    /// A decision table deriving the run key from an element - the first matching case wins, and an element
    /// matching none is skipped.
    pub by: Vec<DerivedCase>,
    /// The part of each element collected into the message's content.
    ///
    /// One dialect's blocks carry the real content in a member and a human-readable summary beside it, so
    /// which part is collected is a fact about the payload rather than a default.
    pub collect: JsonPath,
    /// The member the derived key becomes on the emitted message.
    pub key_as: String,
    /// The carrier each derived key is tagged with.
    pub tag_by_key: BTreeMap<String, String>,
}

/// One case of a decision table: a condition, and the value it yields.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct DerivedCase {
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
    #[serde(default)]
    pub doc: Option<String>,
    pub when: PredicateSet,
    pub value: String,
}

/// Several readings of one span with a local order between them.
///
/// Evaluated as: every `primary`; then, only if those produced nothing, every
/// `fallback_if_primary_empty`; then every `always`, whatever happened. Nesting is refused - a branch set
/// inside a branch set would be a control structure rather than a declaration.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct BranchSet {
    #[serde(default)]
    pub doc: Option<String>,
    /// The readings that normally supply the conversation.
    pub primary: Vec<MessageRule>,
    /// Read only when every `primary` reading came up empty.
    #[serde(default)]
    pub fallback_if_primary_empty: Vec<MessageRule>,
    /// Read whatever the others did.
    ///
    /// The asymmetry is deliberate for at least one dialect: its *answer* must be read even when the
    /// request side was already found, because one gate covering both is what dropped the answer.
    #[serde(default)]
    pub always: Vec<MessageRule>,
}

/// A named table of readings, applied wherever a rule references it.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Fragment {
    #[serde(default)]
    pub doc: Option<String>,
    /// The cases, tried in order; the first that yields wins.
    pub cases: Vec<Alternative>,
}

/// A bounded walk over a state object.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct WalkSpec {
    /// How many levels below the carrier to descend. Zero means the carrier itself only.
    pub max_depth: usize,
    /// Members not descended into, **because a named clause took them here**.
    ///
    /// This was a list of member *names*, pruned globally and unconditionally - and the justification for that
    /// (the readings already took the member) is a statement about a node where a reading actually fired. Where
    /// none did, the name alone stopped the traversal: `{"messages": {"nested": {"deeper": <a message>}}}` lost
    /// the nested message because the key is called `messages`, not because anything read it.
    ///
    /// So each entry names the clause whose recognition justifies the prune. Conditional rather than an honest
    /// unconditional `skip_members`, because for the shipped walks the condition is the true statement: descend
    /// unless the member was consumed.
    #[serde(default)]
    pub prune: Vec<PruneSpec>,
    /// Stop descending below a node **one of these clauses recognised**, naming them by id.
    ///
    /// A message's own members are its content, not more state, so descending into one would read its parts as
    /// though they were turns. But "was this node a message" is a question about *which* clause answered, and
    /// the boolean this replaces asked a wider one: "did anything get selected here". LangGraph's `also_3`
    /// selects every state member, so at a node holding `{"direct": <a message>, "nested": {"messages": […]}}`
    /// the root counted as matched and the walk stopped - losing `nested`'s messages. The root was not a
    /// message; one of its children was.
    ///
    /// Ids rather than a predicate, deliberately: a predicate here would restate the clause's own recognition
    /// logic, and then two declarations would decide one question.
    ///
    /// **The shipped LangGraph walks name every clause**, which is what the boolean meant, and narrowing them to
    /// the one clause that reads a node as a single message is the semantic fix - deferred, and this is why: it
    /// makes the walk descend where it used to stop, and the blocks it then finds trip the carrier-subsequence
    /// invariant on `langgraph/image_gen`. Traversal positions are assigned *after* extraction, in emission
    /// order, so a walk's discovery order and the payload's member order are reconciled nowhere. Naming the
    /// clauses is worth landing on its own: the format now states what the walk stops on rather than implying
    /// "anything", which is what made the wider reading invisible.
    #[serde(default)]
    pub stop_on: Vec<String>,
}

/// Members copied into an emitted value, and what happens where the target already has one.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct LiftSpec {
    pub doc: Option<String>,
    /// Where the members are read from.
    pub from: LiftSource,
    pub members: Vec<String>,
    /// What to do where the target already carries the member. Required: this was the difference between the
    /// two members that preceded it, and it was stated nowhere.
    pub on_conflict: LiftConflict,
}

/// Which value a lift reads from.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LiftSource {
    /// The value the selection landed on - used with `descend`, where the members sit beside the message.
    Element,
    /// The value the selection came out of, for a fact stated once for a batch.
    Parent,
}

/// What a lift does where the target already carries the member.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LiftConflict {
    /// The target's own value wins: the lifted one is a fallback.
    KeepTarget,
    /// The lifted value wins.
    ReplaceTarget,
}

/// A member the walk does not descend into, and the clause whose reading justifies that.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct PruneSpec {
    pub doc: Option<String>,
    /// The member name.
    pub member: String,
    /// The clause that consumes it. The member is skipped only at a node where that clause recognised
    /// something - elsewhere its contents have been read by nothing and are still worth visiting.
    pub taken_by: String,
}

/// One tool call at a named member, as the normaliser's `{name, arguments}` convention.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SingleToolCallSpec {
    pub name: JsonPath,
    /// The name used when the path resolves to nothing. A call this dialect logged without one still
    /// happened, so it is reported rather than dropped.
    #[serde(default)]
    pub name_default: Option<JsonValue>,
    pub arguments: JsonPath,
    /// The member the call becomes. Defaults to `tool_call`.
    #[serde(default)]
    pub as_member: Option<String>,
    /// The value used when `arguments` resolves to nothing.
    #[serde(default)]
    pub arguments_default: Option<JsonValue>,
}

/// A block built from another member of the same value, placed before the content.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct PrependSpec {
    /// Where the block's content is, relative to the value being wrapped. Absent means no block is added,
    /// which is the ordinary case for a dialect that reports reasoning only sometimes.
    pub from: JsonPath,
    /// A condition on the value found there. A dialect writes this member as `null` when there was no
    /// reasoning, and a null is not a thought.
    #[serde(default)]
    pub require: PredicateSet,
    #[serde(flatten)]
    pub block: BlockSpec,
}

/// The canonical tool-call list, built from a dialect's own array of calls.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ToolCallsSpec {
    /// The array of calls, relative to the value being wrapped.
    pub select: JsonPath,
    /// Where each call's id, name and arguments are.
    pub id: JsonPath,
    pub name: JsonPath,
    pub arguments: JsonPath,
    /// What to do with a call that has no id or no name.
    ///
    /// **Required**, because it was hardcoded twice over - "an id is mandatory" and "skip the invalid item" -
    /// and neither is right for every producer. AutoGen's rule admits a list where *at least one* member has an
    /// id and a name; the constructor then dropped the others silently, so a response that called two tools
    /// showed one. Skipping may be right for a producer that logs partial calls; failing the message is right
    /// for one where a dropped call means the answer is not what the model did.
    pub on_invalid_item: InvalidItem,
    /// The member the list becomes. Defaults to `tool_calls`.
    #[serde(default)]
    pub as_member: Option<String>,
}

/// What a tool-call list does with a member it cannot build.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InvalidItem {
    /// Leave it out and keep the rest. Reported either way - a dropped call is a producer defect, not a
    /// detail of the loop that read it.
    Skip,
    /// The whole construction is malformed, so the coalesce moves on to the next shape and the rule's
    /// `fallback` gets its turn. Right where a missing call means the message misdescribes what happened.
    FailMessage,
}

/// One content-block shape, and the canonical block it becomes.
///
/// Exactly one target form per rule, checked at compile time. The forms are the canonical SideML blocks,
/// so this is not a general object builder: a rule says *where* a call's name is, never what a tool_use
/// block looks like.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ContentBlockRule {
    pub id: String,
    pub doc: Option<String>,
    /// Where in the normalisation chain this case is tried. Declared, because the chain's order decides
    /// which dialect answers for a shape more than one of them recognises.
    pub at: ChainPosition,
    /// Position among the cases at that point.
    pub legacy_rank: i32,
    /// The shape this case recognises.
    #[serde(default)]
    pub require: PredicateSet,
    #[serde(default)]
    pub tool_use: Option<ToolUseBlock>,
    #[serde(default)]
    pub tool_result: Option<ToolResultBlock>,
    #[serde(default)]
    pub json: Option<JsonDataBlock>,
    #[serde(default)]
    pub text: Option<TextBlock>,
    #[serde(default)]
    pub media: Option<MediaBlock>,
    #[serde(default)]
    pub thinking: Option<ThinkingBlock>,
    #[serde(default)]
    pub unwrap: Option<UnwrapSpec>,
}

/// Where a content-block case sits relative to the provider wire formats.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChainPosition {
    /// Before any provider format, and **only when normalising a message's own content block**.
    ///
    /// The nested chain - a tool's returned value - deliberately does not consult this position. An envelope
    /// around a message's content is not something a tool's *result* carries, and reading it there changes what
    /// a result means: one dialect writes `{"type": "json", "value": …}` for structured output, and a wrapper
    /// case looking at `value` would unwrap it instead of letting the dialect's own case read it.
    MessageEnvelope,
    /// Tried before any provider format. For a dialect whose own spelling a provider format would
    /// otherwise claim.
    BeforeProviderFormats,
    /// Tried after them, which is where a dialect's additions to a provider's vocabulary belong.
    AfterProviderFormats,
}

/// A model asking for a tool to be run.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ToolUseBlock {
    /// Ordered; absent is reported as null, because a provider that omits an id has still made the call.
    #[serde(default)]
    pub id: Vec<JsonPath>,
    /// Required: a nameless call names nothing to run, so the case does not recognise the block.
    pub name: Vec<JsonPath>,
    /// Ordered, and an **empty object counts as absent** - a dialect that renamed this member leaves the
    /// unused one present as `{}`, so "the first that resolves" would always pick the empty one.
    #[serde(default)]
    pub input: Vec<JsonPath>,
}

/// What a tool returned.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ToolResultBlock {
    #[serde(default)]
    pub tool_use_id: Vec<JsonPath>,
    /// Ordered; omitted when no path resolves. A result may carry both the id that pairs it exactly and the
    /// human-readable tool name, and keeping the latter can make an aggregate snapshot at least as rich as a
    /// duplicate tool-span observation.
    #[serde(default)]
    pub name: Vec<JsonPath>,
    #[serde(default)]
    pub content: Vec<JsonPath>,
    #[serde(default)]
    pub is_error: Vec<JsonPath>,
}

/// Structured data that is not prose.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct JsonDataBlock {
    #[serde(default)]
    pub data: Vec<JsonPath>,
}

/// Prose. Only a string is text.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct TextBlock {
    pub text: Vec<JsonPath>,
}

/// A model's own reasoning.
///
/// `text` is **not** required: a producer that wraps its reasoning in a member holding no text has still said
/// the block is reasoning, and the retired reader emitted an empty one rather than falling through - which is
/// what stops a signature-only block from being read as something else.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ThinkingBlock {
    #[serde(default)]
    pub text: Vec<JsonPath>,
    #[serde(default)]
    pub signature: Vec<JsonPath>,
}

/// One OpenTelemetry instrumentation scope accepted by a message rule.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InstrumentationScopeMatch {
    /// Exact scope name. Empty names are refused when the rule is compiled.
    pub name: String,
    /// Optional version prefix for producer changes that preserve the scope name.
    #[serde(default)]
    pub version_prefix: Option<String>,
}

/// A wrapper: the block's content is *inside* a member, and the member is normalised in its place.
///
/// The one form that does not build a block. Several dialects wrap a content block in a member of their own -
/// a serialisation envelope, a constructor's keyword arguments - and what is inside is an ordinary block of
/// whatever shape. So the case selects it and the chain starts again from the top with that value.
///
/// A case whose member does not normalise answers nothing, which leaves the **original** block to the rest of
/// the chain: that is what the retired readers did, and it is why an unwrap is not a claim.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct UnwrapSpec {
    /// Ordered; the first member that is present is unwrapped, whether or not it normalises.
    pub from: Vec<JsonPath>,
    /// The member is a block serialised as JSON text, decoded before it is normalised. A member that does
    /// not decode leaves the original block to the rest of the chain, as one that does not normalise does.
    #[serde(default)]
    pub parse_json: bool,
}

/// Bytes, or a reference to them. The block's kind and whether it is a reference are both *derived*.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct MediaBlock {
    pub media_type: Vec<JsonPath>,
    pub data: Vec<JsonPath>,
    /// Optional display name, such as the filename a framework retained beside the bytes.
    #[serde(default)]
    pub name: Vec<JsonPath>,
}

/// Where a message rule reads from.
///
/// Exactly one variant, so a rule cannot half-declare both: an event rule has no stage (the event path runs
/// every rule that names the event, in rank order) and a span rule has no event names.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum MessageSource {
    /// A span's attributes, at the named stage.
    Span(SpanSource),
    /// The attributes of any of the named events.
    Event(EventSource),
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct SpanSource {
    /// With the dialects, or only if none of them produced anything.
    #[serde(default)]
    pub stage: MessageStage,
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EventSource {
    /// The events this rule reads. An empty list is refused: it names nothing, and under the previous
    /// spelling it silently made the rule an ordinary span rule instead.
    pub names: Vec<String>,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum MessageStage {
    /// With the dialects, in rank order. The ordinary case.
    #[default]
    Dialect,
    /// Only if no dialect-stage rule produced a message or a claim.
    Fallback,
}

/// One event that carries messages.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct MessageEvent {
    /// This declaration's identity, required like every other clause's.
    ///
    /// It had none, and the entries were collapsed into a `HashSet<String>` of names - so "which asset says
    /// this event carries messages" had no answer, and two assets declaring the same event left one witness
    /// silently discarded. Both agreeing witnesses are kept now, because the point of provenance is that an
    /// answer names the declarations that produced it.
    pub id: String,
    pub name: String,
    /// What the event's **raw form** is: an ordinary message, or a container its readings replace.
    ///
    /// A fact about the *event*, which is why it lives here rather than on each reading. It was
    /// `replaces_raw_event` on a `MessageRule`, repeated on both readings of the one container event, ORed at
    /// runtime - so a `true` beside a `false` compiled and `true` silently won, and the policy was stated
    /// twice with nothing keeping the two statements consistent.
    #[serde(default)]
    pub raw: Option<RawEventForm>,
    pub doc: Option<String>,
}

/// One OTLP **log record** shape that carries a message event.
///
/// Several instrumentations emit the conversation as log records linked to a span instead of as span
/// events: the record names a `message_events` entry and carries what a span event would carry in its
/// attributes, either as the members of its body or as its own attributes. A declaration says which, and
/// where the record states its event name; the event is then read exactly as the span event of that name
/// is. Every `name` must also be a `message_events` entry, because the readings and the raw form are
/// declared there - a log event no reading recognises would be stored and never answer.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct LogEvent {
    /// This declaration's identity, required like every other clause's.
    pub id: String,
    /// The event name, which must also be declared in `message_events`.
    pub name: String,
    /// Where a record states its event name, tried in order: the first present source decides.
    ///
    /// `event_name` is the log record's own field; `attributes:<key>` is a record attribute, which is how
    /// producers that predate the field wrote it.
    pub name_from: Vec<String>,
    /// Where the event's attributes are on the record.
    pub payload: LogEventPayload,
    pub doc: Option<String>,
}

/// Where a log event keeps what a span event of the same name keeps in its attributes.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LogEventPayload {
    /// The record's body is a map, and its members are the event's attributes.
    BodyMembers,
    /// The record's own attributes are the event's attributes.
    Attributes,
}

/// What an event's own attributes are, once its readings have run.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RawEventForm {
    /// The event body is itself a message. The default, and the case for all but one declared event.
    #[default]
    Message,
    /// The event is a *container*: its own attributes are the messages, so emitting the container as well
    /// would report the conversation twice.
    Replace,
}

/// What role a message's **source name** implies, where the name itself decides it.
///
/// Its own section rather than a member of `message_events`, because the two lists are not the same
/// vocabulary. `message_events` says which *OTLP events* carry messages; this says what a **source name**
/// means, and a source name may also be one a rule assigned with `tag_as` - `gen_ai.tool.result` is exactly
/// that, a tag no producer emits. Putting the role on the event entry would have made declaring the role of
/// a tag impossible without also claiming a producer emits it.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct EventRole {
    /// This declaration's identity, required like every other clause's. The compiled form used to
    /// *synthesize* one from the asset and the event name, which is not an identity a declaration can be
    /// held to: two assets agreeing about a role produced one witness and the other's provenance was lost.
    pub id: String,
    /// The source name: an event a producer emits, or a name a rule assigns with `tag_as`.
    pub name: String,
    /// The role on an ordinary span. Absent leaves the role to the content, which is a statement rather
    /// than an omission - most events say nothing about the role.
    #[serde(default)]
    pub role: Option<String>,
    /// The role instead, on a **tool execution** span.
    ///
    /// Two names mean the opposite thing there. On a chat span `gen_ai.tool.message` is a tool's *output*
    /// and `gen_ai.choice` is the assistant's reply; on a tool span the first is the arguments the
    /// assistant passed and the second is what the tool returned. So the span's kind is part of the
    /// question, and one role per name could not express it.
    ///
    /// **Absent means "the same as `role`"**, and that is now the only way to say it: a value *equal* to `role`
    /// is refused. It used to be legal, so one shipped declaration spelled it out while seven omitted it for the
    /// identical fact - two spellings of one statement, in a section whose whole purpose is that a name's role is
    /// declared rather than inferred.
    #[serde(default)]
    pub role_in_tool_span: Option<String>,
    /// This name says **nothing** about the role on a tool execution span, so it is derived from the content
    /// there - even though it does declare one elsewhere.
    ///
    /// The third state, which absence could not express: absence is read as "the same as `role`", so a name that
    /// speaks only for ordinary spans had no spelling at all. A separate flag rather than a sentinel string,
    /// because `role_in_tool_span` holds a role from a closed vocabulary and a magic value in it would be exactly
    /// the guessing this section exists to remove. Refused beside a `role_in_tool_span`, which would be two
    /// answers.
    #[serde(default)]
    pub silent_in_tool_span: bool,
    pub doc: Option<String>,
}
