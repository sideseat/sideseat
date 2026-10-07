use super::*;

/// A default whose declared value may itself be `null`.
///
/// `Option<JsonValue>` would read an explicit `null` as "no default declared", which is a different
/// statement from "the default is null".
fn explicit_value<'de, D>(deserializer: D) -> Result<Option<JsonValue>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    JsonValue::deserialize(deserializer).map(Some)
}

/// The envelope a bare payload is wrapped in.
///
/// Some carriers hold a payload rather than a message - a tool's arguments, an instruction, a response's
/// text - and what that payload *is* is a fact about the carrier, so the envelope is declared beside it.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WrapSpec {
    /// Why this is declared the way it is, for a reader and the explain trace. Read by nothing.
    #[serde(default)]
    pub doc: Option<String>,
    /// A literal role. One of this and `role_from` is required.
    #[serde(default)]
    pub role: Option<String>,
    /// A JSONPath whose value is the role, relative to the reading being wrapped.
    ///
    /// Several dialects put the role *in* the payload - Gemini's `{parts, role}` is the clearest case - so
    /// a literal here would either be wrong or need one rule per role.
    #[serde(default)]
    #[cfg_attr(test, schemars(with = "Option<String>"))]
    pub role_from: Option<JsonPath>,
    /// Rename a role the payload supplied.
    ///
    /// A provider's own vocabulary: one calls the assistant `model`, and normalising that here keeps the
    /// alias beside the dialect that uses it rather than in a shared table nothing points at.
    #[serde(default)]
    pub role_map: BTreeMap<String, String>,
    /// Treat `role_map` as the complete list: a value not in it falls back to `role` rather than being used
    /// as a role itself.
    ///
    /// One dialect names the *speaker* where another names the role - `source: "planner"` means an
    /// assistant, not a role called `planner` - so which of the two a member is has to be declared.
    #[serde(default)]
    pub role_map_is_closed: bool,
    /// Ordered paths for the content; the first that resolves wins.
    ///
    /// One dialect serialises a message three ways depending on how it was constructed, and the content sits
    /// in a different member each time - so a single path reads two of the three as empty.
    ///
    /// **One list, not a singular member beside it.** `content_from` was a second spelling of exactly this
    /// question - the runtime simply prepended it to this list - so a rule could state its content source
    /// twice, in two members, with the ordering between them implicit in the code rather than in the
    /// declaration. The 16 singular uses are now one-element lists.
    #[serde(default, rename = "content_from")]
    #[cfg_attr(test, schemars(with = "FirstOf<String, false>"))]
    pub content_from_any_of: FirstOf<JsonPath, false>,
    /// The content when none of the paths above resolve. Absent means the reading is not this shape.
    ///
    /// An explicit `null` is a default of JSON null, not the absence of one: a dialect reports a tool that
    /// returned nothing that way, and the two readings differ.
    #[serde(default, deserialize_with = "explicit_value")]
    pub content_default: Option<JsonValue>,
    /// The member the read value becomes. Defaults to `content`.
    ///
    /// Not always content: a response carrying only tool calls has no content, and putting the calls
    /// under `content` would render them as the assistant's prose.
    #[serde(default)]
    pub content_as: Option<String>,
    /// Literal members added to the envelope.
    #[serde(default)]
    pub members: BTreeMap<String, JsonValue>,
    /// Members taken from *other* attributes of the same span.
    ///
    /// A dialect writes one logical message across several attributes - the arguments here, the tool's
    /// name and call id there - and which attribute holds which part is exactly the knowledge that
    /// belongs in an asset.
    #[serde(default)]
    pub attach: Vec<AttachSpec>,
    /// Build a block from another member and put it **before** the content.
    ///
    /// One dialect reports a model's reasoning in a sibling member of its reply, and the canonical form is a
    /// thinking block ahead of the text - so the two become one content list rather than two messages.
    #[serde(default)]
    pub prepend_block: Option<PrependSpec>,
    /// Build the canonical tool-call list from an array of the dialect's own calls.
    ///
    /// A *typed* constructor for a canonical target, not a general object builder: the shape is
    /// `{id, type: "function", function: {name, arguments}}` and only the sources are rule data. Arguments
    /// arrive as a serialised JSON string as often as an object, so parsing them is declared here rather
    /// than left to whoever reads the payload later.
    #[serde(default)]
    pub tool_calls_from: Option<ToolCallsSpec>,
    /// Build a **single** tool call at a named member, as `{name, arguments}`.
    ///
    /// The normaliser already unwraps a `tool_call` member (`sideml/tools.rs`), so this is a canonical
    /// target like the list above rather than a general object builder.
    #[serde(default)]
    pub tool_call_from: Option<SingleToolCallSpec>,
    /// Wrap only where the value is not already message-shaped.
    ///
    /// A generic carrier holds either a message or bare data: `output.value = "the answer"` is the answer,
    /// and `output.value = {"role": …}` is already a message. Wrapping the second buries the conversation a
    /// level down; not wrapping the first loses it entirely, because normalisation looks for `role` on a
    /// string and finds nothing.
    #[serde(default)]
    pub only_plain_data: bool,
    /// Wrap the value in a *content block* first, and make that block the message's only content.
    ///
    /// A tool call is not a bare object under a role: it is a `tool_use` block, and the block shape is
    /// what carries the name and the id through normalisation. Emitting the arguments without them
    /// produced a nameless call the pipeline then discarded - extracted and *then* dropped, which is
    /// worse than not reading it, because every layer looked fine.
    #[serde(default)]
    pub block: Option<BlockSpec>,
}

/// A content block built around the read value.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BlockSpec {
    /// Why this is declared the way it is, for a reader and the explain trace. Read by nothing.
    #[serde(default)]
    pub doc: Option<String>,
    /// The block's `type` member - `tool_use`, `tool_result`.
    #[serde(rename = "type")]
    pub block_type: String,
    /// The member the read value becomes inside the block. Defaults to `content`.
    #[serde(default)]
    pub content_as: Option<String>,
    /// Members taken from sibling attributes, as on the envelope.
    #[serde(default)]
    pub attach: Vec<AttachSpec>,
}

/// One member taken from a sibling attribute.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AttachSpec {
    /// Why this member is taken from where it is, where that is not obvious. A field rather than a comment,
    /// as everywhere else here, because the explain trace surfaces it.
    #[serde(default)]
    pub doc: Option<String>,
    /// The attribute to read. One of this and `from_path` is required.
    #[serde(default)]
    pub from: Option<String>,
    /// Ordered paths into the value being wrapped; the first that resolves wins.
    ///
    /// The same serialisation variance as the content: a member may sit at the top level or under the
    /// wrapper a serialiser added.
    #[serde(default, rename = "from_value")]
    #[cfg_attr(test, schemars(with = "FirstOf<String, false>"))]
    pub from_value_any_of: FirstOf<JsonPath, false>,
    /// The attached value must satisfy this, or the member is left off.
    ///
    /// An empty list is not a set of tool calls, and attaching one makes a plain reply look like a call.
    #[serde(default, rename = "where")]
    pub require: ValueCondition,
    /// A path into the rule's *own parsed payload*, rather than a sibling attribute.
    ///
    /// Relative to the whole payload, deliberately: a dialect reports why a turn stopped beside the
    /// content rather than inside it, so the member being attached sits outside the part being wrapped.
    #[serde(default)]
    #[cfg_attr(test, schemars(with = "Option<String>"))]
    pub from_path: Option<JsonPath>,
    /// Lower-case the attached string.
    ///
    /// A provider writes finish reasons in upper case and the canonical form is lower; declared because
    /// lower-casing a payload that is meant to be verbatim would change it.
    #[serde(default)]
    pub lowercase: bool,
    /// The member it becomes.
    #[serde(rename = "as")]
    pub as_member: String,
    /// How to read it. Defaults to text.
    #[serde(default)]
    pub parse: Option<ParseMode>,
    /// The member of the parsed `from` attribute to attach, rather than the whole value.
    ///
    /// An attribute may hold a structure of which one member is the fact: one tracer writes a span's
    /// ancestry as an array of ids, ending with the span's own, and that last id is what tells two
    /// identical tool executions apart. Requires `from` and a JSON parse; a path that resolves nothing
    /// falls through to the span name and the default, like an absent attribute.
    #[serde(default)]
    #[cfg_attr(test, schemars(with = "Option<String>"))]
    pub select: Option<JsonPath>,
    /// Attach only when the source attribute equals this exactly.
    ///
    /// How a boolean flag arrives: an attribute whose string is `"true"`. Without the comparison the
    /// literal `"false"` would attach as a truthy value.
    #[serde(default)]
    pub when_equals: Option<String>,
    /// The literal to attach instead of the source's value, for a flag - or on its own, for a member that
    /// is part of the shape rather than something read.
    ///
    /// An explicit `null` is a value: a content block declares an unsigned signature that way, and the
    /// member has to be present rather than omitted.
    #[serde(default, deserialize_with = "explicit_value")]
    pub value: Option<JsonValue>,
    /// Treat a blank value as absent, so the fallbacks below apply.
    #[serde(default)]
    pub blank_is_absent: bool,
    /// Remove a leading `[TAG]\n` marker before parsing.
    ///
    /// One dialect tags a structured payload with the tool it belongs to and then writes the JSON beneath
    /// it; parsing without stripping fails, and the member would silently fall back to its default.
    #[serde(default)]
    pub strip_bracket_tag: bool,
    /// Fall back to the span name with this prefix removed, trimmed, when the other sources are absent.
    ///
    /// The conventions prescribe `execute_tool {name}` as a tool span's name, so a producer that omits
    /// the attribute still names the tool - and an unnamed call is unusable downstream. An empty prefix
    /// deliberately uses the complete span name for exporters that name a tool span exactly after the tool.
    #[serde(default)]
    pub or_span_name_after: Option<String>,
    /// Attach this literal when nothing else supplied a value.
    ///
    /// Distinct from omitting the member: a block whose shape *requires* a name carries an empty one
    /// rather than none, and the two are different values to anything hashing the payload.
    #[serde(default)]
    pub default: Option<JsonValue>,
    /// Place this member *after* the content member rather than before it.
    ///
    /// Member order is declared because it is *observable*: this map preserves insertion order and the
    /// message is stored as serialised JSON, so moving a member changes the persisted bytes and with them
    /// the reconstruction cache digest. It does **not** change the normalised content hash - the feed sorts
    /// object keys before hashing - so this is about reproducing what was stored, not about identity. The
    /// orders here are what the extractors emitted, which is why they are stated rather than chosen.
    #[serde(default)]
    pub after_content: bool,
}

/// What an emitted observation is.
#[derive(Debug, Deserialize, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum EmitTarget {
    #[default]
    Message,
    ToolDefinitions,
    /// A list of tool *names*, as opposed to their definitions. A framework that reports only the names
    /// has said which tools were available, not what they take.
    ToolNames,
    /// The carrier is claimed and nothing is read from it.
    ///
    /// A real shape, not a loophole: one dialect's agent spans aggregate what their children already
    /// reported, and their `input.value` is a Python `repr` of framework internals. Claiming says "this is
    /// mine and holds no message", which stops a generic reader from presenting that text as a
    /// conversation - and saying it in a rule is what keeps the decision out of the code.
    Claim,
}

/// One documented shape of a payload: where to look, what to require, and what to carry down.
#[derive(Debug, Deserialize, Clone, Default)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Alternative {
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
    /// An RFC 9535 JSONPath into the parsed value. Absent means the value itself.
    ///
    /// A standard query rather than a hand-rolled path syntax - `$.tasks_output[*].messages[*]` reads every
    /// turn of every task, and `$['event.name']` reads a member whose name contains a dot, which is where
    /// the hand-built resolver had a bug. Compiled when the asset loads, so a malformed path is a startup
    /// error naming its file rather than a query that silently finds nothing.
    #[serde(default)]
    #[cfg_attr(test, schemars(with = "Option<String>"))]
    pub select: Option<JsonPath>,
    /// Treat the selected value as a list and read each element.
    #[serde(default)]
    pub each: bool,
    /// After selecting an element, descend to this member - `choices[].message`.
    #[serde(default)]
    pub descend: Option<String>,
    /// Decode the selected element when it is text: a list whose members are themselves serialised
    /// (`{"messages": ["ChatMessage(role=..., content=...)"]}`). An element that is not text, or does not
    /// decode, is not this shape and the reading moves on.
    #[serde(default)]
    pub parse: Option<ParseMode>,
    /// Members copied into the value being emitted, from the element or from the value it was selected out of.
    ///
    /// **One primitive with a declared conflict policy**, where there were two members with opposite and
    /// unstated ones: `lift` overwrote the target's member and `lift_from_parent` preserved it, and neither the
    /// names nor `lift`'s own documentation said so. A payload carrying `finish_reason` outside a message *and*
    /// inside it therefore got the outer value under one member name and the inner under the other, decided by
    /// which member an asset happened to use.
    ///
    /// The source is a real distinction and stays: `element` is the value the selection landed on, `parent` the
    /// value it came out of. The policy is now a statement rather than a consequence of that choice.
    #[serde(default)]
    pub lift: Vec<LiftSpec>,
    /// A condition on the value this selection came from, rather than on the selected element.
    ///
    /// What a batch of tool results *is* is stated on the message enclosing them - its type - while the
    /// reading is one message per element, so the discriminator and the selection sit at different levels.
    #[serde(default, rename = "parent_where")]
    pub require_parent: ValueCondition,
    /// The shape an observation must have to be emitted.
    ///
    /// A predicate set, so "has a role and content", "is an object" and "is a non-empty string" are one
    /// vocabulary rather than three fields that grew one at a time.
    #[serde(default, rename = "where")]
    pub require: ValueCondition,
    /// An envelope for *this* reading only.
    ///
    /// One reading of a payload may be a bare value needing a role while its siblings are already
    /// messages - a dialect's answer sits in a string member beside a list of turns. Overrides the rule's
    /// own `wrap` where present.
    #[serde(default)]
    pub wrap: Option<WrapSpec>,
    /// Trim a string before testing and emitting it.
    ///
    /// Declared rather than always-on: trimming a payload that is meant to be verbatim would change it.
    #[serde(default)]
    pub trim: bool,
    /// Apply this named fragment's cases to each selected element.
    ///
    /// The fragment decides what the element *is*; this reading decides *where to look*. Splitting them is
    /// the point: one dialect's state object holds its messages in four places and recognises them one way.
    #[serde(default)]
    pub then_fragment: Option<String>,
    /// What this reading is, where it differs from the rule's own target.
    ///
    /// A logged model call carries its conversation and the tools it was offered in one carrier, and they
    /// are not the same kind of thing - so the target belongs to the reading, not only to the rule.
    #[serde(default)]
    pub emit: Option<EmitTarget>,
    /// Shapes recognised at *this* selection point only, tried after the shared fragment's own cases.
    ///
    /// A shared table says what a message looks like in a dialect; a particular place that dialect writes
    /// messages may accept one shape more loosely than the rest - a bare `{content}` passed through where
    /// the table would refuse it. Putting that in the table would loosen every other point that reads it.
    #[serde(default)]
    pub extra_cases: Vec<Alternative>,
    /// For each selected element, the first of these paths that names a member, chosen by **presence**.
    ///
    /// Per *element*, which is the point: one dialect's tool groups each either wrap their declarations
    /// under one of two spellings or are a declaration themselves, and deciding once for the whole array
    /// would drop the odd group out.
    ///
    /// Presence rather than yielding is load-bearing where a wrapper may legitimately be empty: a dialect that writes
    /// `function_declarations: []` has declared no tools, and picking "the first path that yielded
    /// something" skips the present-but-empty member and falls through to emitting the wrapper itself as a
    /// tool. Presence also settles which of two spellings wins when both appear.
    #[serde(default, rename = "then_select")]
    #[cfg_attr(test, schemars(with = "FirstOf<String, false>"))]
    pub then_present_any_of: FirstOf<JsonPath, false>,
    /// Fall back to the element itself when `then_present_any_of` named nothing, or named a member of the wrong
    /// shape.
    ///
    /// One answer for both situations. A wrapper member is a list of declarations, so
    /// `{"function_declarations": {"name": "weather"}}` has not declared its contents; the wrong-shaped member is
    /// **reported** either way, and recovered from the element only where the enclosing object independently
    /// describes a valid reading. Once presence has selected a representation, a later spelling is not tried:
    /// falling through would answer from a representation the producer did not use.
    #[serde(default)]
    pub else_element: bool,
    /// Rebuild the candidate object from its members named `prefix` + *name* + `suffix`, as `{name: value}`.
    ///
    /// An object's member names can encode a structure the way an indexed attribute family encodes a
    /// list: one instrumentation writes a call's arguments as `<prefix><name>.value` beside
    /// `<prefix><name>.type`, so the arguments are the members a pattern picks out, not the object.
    /// A candidate with no matching member is not this shape, and the reading moves on.
    #[serde(default)]
    pub collect_members: Option<CollectMembers>,
}

/// The member-name pattern of [`Alternative::collect_members`].
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CollectMembers {
    /// Why this is declared the way it is, for a reader and the explain trace. Read by nothing.
    #[serde(default)]
    pub doc: Option<String>,
    pub prefix: String,
    pub suffix: String,
}

impl CollectMembers {
    /// `{name: value}` for every member of `object` matching the pattern, in the object's order;
    /// `None` when no member matches.
    pub fn collect(
        &self,
        object: &serde_json::Map<String, serde_json::Value>,
    ) -> Option<serde_json::Value> {
        let collected: serde_json::Map<String, serde_json::Value> = object
            .iter()
            .filter_map(|(key, value)| {
                let name = key.strip_prefix(&self.prefix)?.strip_suffix(&self.suffix)?;
                (!name.is_empty()).then(|| (name.to_string(), value.clone()))
            })
            .collect();
        (!collected.is_empty()).then_some(serde_json::Value::Object(collected))
    }
}

/// Which members an indexed entry must carry.
#[derive(Debug, Deserialize, Clone, Default)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MemberRequirements {
    /// Why this is declared the way it is, for a reader and the explain trace. Read by nothing.
    #[serde(default)]
    pub doc: Option<String>,
    /// Every one of these must be present.
    #[serde(default)]
    pub all_of: Vec<MemberRequirement>,
    /// At least one of these must be present.
    #[serde(default)]
    pub any_of: Vec<MemberRequirement>,
}

/// One member, and how its presence is decided.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MemberRequirement {
    /// Why this is declared the way it is, for a reader and the explain trace. Read by nothing.
    #[serde(default)]
    pub doc: Option<String>,
    pub name: String,
    #[serde(default)]
    pub presence: MemberPresence,
}

/// How a member's presence is established.
#[derive(Debug, Deserialize, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum MemberPresence {
    /// The member's own key exists.
    #[default]
    Exact,
    /// Some key nested under it exists - the member is an array or object flattened into dotted keys.
    Nested,
    /// Either.
    Either,
}

/// A message assembled from several attributes of one span.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ComposeSpec {
    /// Why this is declared the way it is, for a reader and the explain trace. Read by nothing.
    #[serde(default)]
    pub doc: Option<String>,
    /// The carrier the assembled message is tagged with.
    pub tag: String,
    /// The members, in the order they are inserted - which is observable, since content identity is
    /// hashed from the payload.
    pub members: Vec<ComposeMember>,
    /// A condition on the **assembled** object, checked before it is emitted.
    ///
    /// Some shapes can only be judged once the members are together - whether the name a dialect reported is a tool anyone could
    /// call, for instance.
    #[serde(default, rename = "where")]
    pub require: ValueCondition,
    /// Emit the assembled object as a canonical **tool definition** rather than as a message.
    ///
    /// A dialect that reports one tool per span writes its name, documentation and parameter schema as
    /// three separate attributes. Assembling them is what `compose` does; the shape they become -
    /// `[{type: "function", function: {…}}]` - is a canonical target, so it lives here and only the sources
    /// are rule data.
    #[serde(default)]
    pub as_tool_definition: bool,
    /// Literal members added *after* every source member.
    ///
    /// Position matters and this is why it is a separate field: the code being replaced inserts the role
    /// last, after everything it collected, so a payload built role-first would be stored with different
    /// bytes - which changes the reconstruction cache digest, though not the normalised content hash.
    #[serde(default)]
    pub trailing: BTreeMap<String, JsonValue>,
}

/// One member of a composed message: a named source, or a sweep of a prefix.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ComposeMember {
    /// Why this is declared the way it is, for a reader and the explain trace. Read by nothing.
    #[serde(default)]
    pub doc: Option<String>,
    /// The member's name. Absent for a sweep, which takes its names from the keys it finds.
    #[serde(rename = "as", default)]
    pub as_member: Option<String>,
    /// The attribute, or the first of several the span carries.
    #[serde(default, rename = "from")]
    pub from_any_of: FirstOf<String, false>,
    /// How to read it. Defaults to text.
    #[serde(default)]
    pub parse: Option<ParseMode>,
    /// A last-resort source, used only where the gate holds.
    ///
    /// Separate from `from_any_of` because it is *conditional*: this key is not the dialect's own, so
    /// reading it unguarded would claim a generic carrier that belongs to whatever wrote it.
    #[serde(default)]
    pub fallback: Option<ComposeFallback>,
    /// Collect every attribute under this prefix, keyed by the remainder.
    #[serde(default)]
    pub sweep_prefix: Option<String>,
    /// Names the sweep skips, because a named member above already read them.
    #[serde(default)]
    pub except: Vec<String>,
}

/// A conditional last-resort source.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ComposeFallback {
    /// Why this is declared the way it is, for a reader and the explain trace. Read by nothing.
    #[serde(default)]
    pub doc: Option<String>,
    pub from: String,
    /// The evidence required before the fallback is read, over the span's name and attributes.
    #[serde(rename = "where")]
    pub condition: SpanWhere,
    #[serde(default)]
    pub parse: Option<ParseMode>,
}

/// A text carrier read as tagged sections.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SectionsSpec {
    /// Why this is declared the way it is, for a reader and the explain trace. Read by nothing.
    #[serde(default)]
    pub doc: Option<String>,
    /// The separator between sections.
    pub split_on: String,
    /// Split at most this many times over, so the last section keeps every later separator: a carrier whose
    /// final part is free text that may itself contain the separator (a prompt with blank lines) stays one
    /// section. At least one.
    #[serde(default)]
    pub max_sections: Option<usize>,
    /// The final section is **cut** where the carrier holds less text than this integer attribute states, and
    /// is then left out: a producer that truncates a long carrier states the whole length beside it, and a cut
    /// section is not what the producer said. SideML has no truncation marker, so the partial text is dropped
    /// rather than shown as if complete; every section before it is whole and is kept. An absent or unreadable
    /// length leaves the carrier as it stands.
    #[serde(default)]
    pub truncated_unless_length: Option<LengthWitness>,
    /// Sections that are, once trimmed, exactly the whole value of one of these attributes (`attr:<key>`,
    /// trimmed) are left out: another carrier states that part of the text whole, and reading it here too
    /// would show it twice.
    #[serde(default)]
    pub skip_sections_equal_to: Vec<SourceName>,
    /// Routes, tried in order; the first whose tag matches wins, and a route with no `tag_prefix` is the
    /// default.
    pub routes: Vec<SectionRoute>,
}

/// An attribute stating how long a text is, and the unit it counts in.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct LengthWitness {
    #[serde(default)]
    pub doc: Option<String>,
    /// The attribute, as `attr:<key>`, holding a non-negative integer.
    pub source: SourceName,
    /// What the producer counted: the language it is written in decides it, so it is declared, not guessed.
    pub counts: LengthUnit,
}

/// The unit a stated text length counts.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum LengthUnit {
    /// Unicode scalar values, as Python's `len` counts.
    Chars,
    /// UTF-16 code units, as JavaScript's `length` counts.
    Utf16Units,
    /// UTF-8 bytes.
    Bytes,
}

impl LengthUnit {
    /// How long this text is, in this unit.
    pub fn length_of(self, text: &str) -> usize {
        match self {
            Self::Chars => text.chars().count(),
            Self::Utf16Units => text.encode_utf16().count(),
            Self::Bytes => text.len(),
        }
    }
}

/// What to do with a section whose tag matches.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SectionRoute {
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
    /// The tag prefix this route claims. Absent means "any section not claimed above".
    #[serde(default)]
    pub tag_prefix: Option<String>,
    /// The role the emitted message carries.
    pub role: String,
    /// Build a content block instead of putting the body under `content`.
    #[serde(default)]
    pub block: Option<SectionBlock>,
    /// Drop the section entirely when every one of these holds, tested against
    /// `{"capture": <tag remainder>, "body": <section body>}`.
    ///
    /// Predicates rather than a fused pair. It was `capture_lacks_prefix` **and** `body_starts_with`,
    /// which is one producer's policy in the shape of a field - the weakest feature in the vocabulary by
    /// its own test. What it expresses is unchanged and still narrow on purpose: a dialect writes each
    /// tool result twice, once as the text the model saw tagged with the call id and once as raw
    /// structured telemetry tagged with the tool's name, and emitting both shows every result twice. The
    /// conditions stay conjunctive so that if the id prefix ever changes, an unrecognised section reaches
    /// the feed unlinked rather than vanishing from it.
    #[serde(default, rename = "skip_where")]
    pub skip_when: ValueCondition,
}

/// A block built from a section, carrying what the tag captured.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SectionBlock {
    /// Why this is declared the way it is, for a reader and the explain trace. Read by nothing.
    #[serde(default)]
    pub doc: Option<String>,
    #[serde(rename = "type")]
    pub block_type: String,
    /// The member the tag's remainder becomes - an id that pairs this section with a call.
    #[serde(default)]
    pub capture_as: Option<String>,
}
