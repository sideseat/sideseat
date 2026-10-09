use super::*;

/// One content-block shape, and the canonical block it becomes.
///
/// Exactly one target form per rule, checked at compile time. The forms are the canonical SideML blocks,
/// so this is not a general object builder: a rule says *where* a call's name is, never what a tool_use
/// block looks like.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ContentBlockRule {
    pub id: String,
    pub doc: Option<String>,
    /// Where in the normalisation chain this case is tried. Declared, because the chain's order decides
    /// which dialect answers for a shape more than one of them recognises.
    pub at: ChainPosition,
    /// Position among the cases at that point, lowest first. Unique within the position.
    pub priority: i32,
    /// The shape this case recognises.
    #[serde(default, rename = "where")]
    pub require: ValueCondition,
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
    #[serde(default)]
    pub splice: Option<SpliceSpec>,
    #[serde(default)]
    pub provider_run: Option<ProviderRunSpec>,
    #[serde(default)]
    pub refusal: Option<RefusalBlock>,
    #[serde(default)]
    pub redacted_thinking: Option<RedactedThinkingBlock>,
    #[serde(default)]
    pub unknown: Option<UnknownBlock>,
}

/// Where a content-block case sits relative to the provider wire formats.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
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
    /// The provider wire formats themselves - the shapes a model API defines and every framework relays.
    /// Between the envelopes and `after_provider_formats`, where the retired Rust readers sat.
    ProviderFormats,
    /// Tried after them, which is where a dialect's additions to a provider's vocabulary belong.
    AfterProviderFormats,
}

/// Where one member of a canonical block comes from.
///
/// A JSONPath string reads the first value it selects, as it stands. The object form applies exactly one
/// bounded transform to it, and a transform that cannot apply leaves the source **absent**, so the next one in
/// the list is tried - which is how "the first usable spelling" is written.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum ValueSource {
    /// A member of the block, by RFC 9535 JSONPath.
    Path(#[cfg_attr(test, schemars(with = "String"))] JsonPath),
    /// A member with one transform applied.
    Transformed(TransformedSource),
}

/// A member with one transform applied: `{"path": ..., "pipe": [step]}`, the step one of `join` (every string the
/// path selects, joined), `parse` (a string decoded the way a carrier's text is; anything else passes through),
/// `prepend`, or a closed `map` (`{"map": {...}, "closed": true}`). A step that cannot apply leaves the source
/// **absent**, so the next candidate is tried.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct TransformedSource {
    #[serde(default)]
    pub doc: Option<String>,
    #[cfg_attr(test, schemars(with = "String"))]
    pub path: JsonPath,
    pub pipe: Vec<Transform>,
}

impl TransformedSource {
    fn only(&self) -> Option<&Transform> {
        match self.pipe.as_slice() {
            [one] => Some(one),
            _ => None,
        }
    }

    /// The separator, where the step is a join.
    pub fn join(&self) -> Option<&String> {
        match self.only()? {
            Transform::Join(separator) => Some(separator),
            _ => None,
        }
    }

    /// The decoding, where the step is a parse.
    pub fn parse(&self) -> Option<ParseMode> {
        match self.only()? {
            Transform::Parse(mode) => Some(*mode),
            _ => None,
        }
    }

    /// The prefix, where the step prepends one.
    pub fn prepend(&self) -> Option<&String> {
        match self.only()? {
            Transform::Prepend(prefix) => Some(prefix),
            _ => None,
        }
    }

    /// The table, where the step is a closed map.
    pub fn map(&self) -> Option<&BTreeMap<String, JsonValue>> {
        match self.only()? {
            Transform::Map {
                table,
                closed: true,
            } => Some(table),
            _ => None,
        }
    }
}

/// How a tool result's content is shaped once it has been selected.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ResultContent {
    /// Normalised as a returned value: a list's members as blocks, a provider block as that block, anything
    /// else as it stands.
    #[default]
    Normalized,
    /// The same, and then a list of blocks reduced to the value it holds - a lone text is its string, a lone
    /// structured value is that value - which is the form a tool-role message's content takes. For a format
    /// that writes a result as a list of blocks, so one result reads alike however it was written.
    Value,
    /// A list of content blocks built from the value: Python constructor reprs where it holds them, otherwise
    /// structured data as one `json` block, text as one `text` block, and nothing as an empty list. For a
    /// format whose response member holds what the tool returned, encoded or not.
    Blocks,
}

/// A model asking for a tool to be run.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ToolUseBlock {
    /// Why this is declared the way it is, for a reader and the explain trace. Read by nothing.
    #[serde(default)]
    pub doc: Option<String>,
    /// Ordered: the first member holding a non-blank string, else a declared `template`; absent is
    /// reported as null, because a provider that omits an id has still made the call.
    #[serde(default)]
    pub id: FirstOf<IdSource, true>,
    /// Required: a nameless call names nothing to run, so the case does not recognise the block.
    #[serde(default)]
    pub name: FirstOf<ValueSource, true>,
    /// Ordered, and an **empty object counts as absent** - a dialect that renamed this member leaves the
    /// unused one present as `{}`, so "the first that resolves" would always pick the empty one.
    #[serde(default)]
    pub input: FirstOf<ValueSource, true>,
    /// The provider runs this tool itself, inside its response - a hosted web search, a code interpreter.
    /// Stated by the case, because the shape a provider gives such a call is what says so.
    #[serde(default)]
    pub provider_executed: bool,
}

/// One place a call's id may come from.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum IdSource {
    /// A member of the block, by RFC 9535 JSONPath.
    Path(#[cfg_attr(test, schemars(with = "String"))] JsonPath),
    /// An id built from the call itself, for a provider that states none.
    Template(IdTemplate),
}

/// A synthetic id: literal text with closed placeholders - `{name}` (the call's resolved name) and
/// `{stable_hash(input)}` (sixteen hex digits of 64-bit FNV-1a over the resolved input's serialisation). The last
/// source of an id, since it always yields; two calls of one tool with different arguments get different
/// ids, and the same call re-sent gets the same one.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct IdTemplate {
    #[serde(default)]
    pub doc: Option<String>,
    pub template: String,
}

/// What a tool returned.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ToolResultBlock {
    /// Why this is declared the way it is, for a reader and the explain trace. Read by nothing.
    #[serde(default)]
    pub doc: Option<String>,
    #[serde(default)]
    pub tool_use_id: FirstOf<ValueSource, true>,
    /// Ordered; omitted when no path resolves. A result may carry both the id that pairs it exactly and the
    /// human-readable tool name, and keeping the latter can make an aggregate snapshot at least as rich as a
    /// duplicate tool-span observation.
    #[serde(default)]
    pub name: FirstOf<ValueSource, true>,
    /// What the tool returned. Where it is normalised (`content_as` other than `blocks`) it re-enters the chain,
    /// so a selector naming the block itself (`$`) is refused, and so is a closed `map` with a literal the
    /// assembled chain never finishes normalising - checked by normalising every literal, so a literal this case
    /// recognises that maps on to one the chain finishes with is accepted. Nesting the telemetry itself drives is
    /// bounded at run time, the innermost levels kept as they stand.
    #[serde(default)]
    pub content: FirstOf<ValueSource, true>,
    /// How the selected content is shaped.
    #[serde(default)]
    pub content_as: ResultContent,
    #[serde(default)]
    pub is_error: FirstOf<ValueSource, true>,
    /// What a tool the provider ran itself produced, returned in the same response as its call.
    #[serde(default)]
    pub provider_executed: bool,
}

/// Structured data that is not prose.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct JsonDataBlock {
    /// Why this is declared the way it is, for a reader and the explain trace. Read by nothing.
    #[serde(default)]
    pub doc: Option<String>,
    #[serde(default)]
    pub data: FirstOf<ValueSource, true>,
}

/// Prose. Only a string is text.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct TextBlock {
    /// Why this is declared the way it is, for a reader and the explain trace. Read by nothing.
    #[serde(default)]
    pub doc: Option<String>,
    #[serde(default)]
    pub text: FirstOf<ValueSource, true>,
    /// The sources the text cites, where the format states them beside it.
    #[serde(default)]
    pub citations: Option<CitationsSpec>,
}

/// Where a text's citations are and how each reads. Providers cite in shapes of their own - a web page, a span
/// of an attached document, a search result - so each shape is a case, and every case builds the one SideML
/// citation. A citation no case reads, or one naming no source, is kept as an `unknown` citation holding the
/// provider's item, so a new shape is seen arriving rather than lost.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CitationsSpec {
    /// Why this is declared the way it is, for a reader and the explain trace. Read by nothing.
    #[serde(default)]
    pub doc: Option<String>,
    /// The first member that is a list is the list of citations.
    #[cfg_attr(test, schemars(with = "FirstOf<String, false>"))]
    #[serde(default)]
    pub from: FirstOf<JsonPath, false>,
    /// Tried in order for each citation; the first whose `where` holds reads it. Not empty, and only the last
    /// may leave out its `where`.
    pub cases: Vec<CitationCase>,
}

/// One shape of citation, and where its members are.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CitationCase {
    /// Why this is declared the way it is, for a reader and the explain trace. Read by nothing.
    #[serde(default)]
    pub doc: Option<String>,
    /// The shape this case recognises.
    #[serde(default, rename = "where")]
    pub require: ValueCondition,
    /// What the citation points into.
    pub kind: CitationKind,
    /// Which one: the page's URL, the uploaded file's id, the attached document's name. Required: a citation
    /// that names nothing it cites is not one.
    #[serde(default)]
    pub source: FirstOf<ValueSource, true>,
    #[serde(default)]
    pub title: FirstOf<ValueSource, true>,
    /// Where in **this text** the citation applies, as offsets in the provider's own character unit, where the
    /// format says. A citation stated on its own text block applies to the whole block and states no span; where
    /// a passage sits in its source is not this.
    #[serde(default)]
    pub text_start: FirstOf<ValueSource, true>,
    #[serde(default)]
    pub text_end: FirstOf<ValueSource, true>,
    /// The passage of the source the text rests on, where the provider quotes it.
    #[serde(default)]
    pub cited_text: FirstOf<ValueSource, true>,
}

/// What a citation points into, spelled as SideML writes it.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, strum::IntoStaticStr)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum CitationKind {
    /// A web page, by URL.
    Url,
    /// A document sent with the request.
    Document,
    /// A file uploaded to the provider, by id.
    File,
    /// A search result the request supplied.
    SearchResult,
}

/// A tool the provider ran itself, written as **one** item holding both the call and what it produced - a
/// Responses API `web_search_call`, whose action is the call and whose sources are its result. It answers with
/// two blocks, the provider-executed call and its result, so like a splice it is legal only at
/// `message_envelope`.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ProviderRunSpec {
    /// Why this is declared the way it is, for a reader and the explain trace. Read by nothing.
    #[serde(default)]
    pub doc: Option<String>,
    /// As a `tool_use` id: the first member holding a non-blank string, else a template.
    #[serde(default)]
    pub id: FirstOf<IdSource, true>,
    /// Required: a run that names no tool is not recognised.
    #[serde(default)]
    pub name: FirstOf<ValueSource, true>,
    #[serde(default)]
    pub input: FirstOf<ValueSource, true>,
    /// What the run produced, as a list of content blocks as `content_as: blocks` builds them. Absent, the item
    /// holds no result yet and answers with the call alone.
    #[serde(default)]
    pub result: FirstOf<ValueSource, true>,
}

/// A model's own reasoning.
///
/// `text` is **not** required: a producer that wraps its reasoning in a member holding no text has still said
/// the block is reasoning, and the retired reader emitted an empty one rather than falling through - which is
/// what stops a signature-only block from being read as something else.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ThinkingBlock {
    /// Why this is declared the way it is, for a reader and the explain trace. Read by nothing.
    #[serde(default)]
    pub doc: Option<String>,
    #[serde(default)]
    pub text: FirstOf<ValueSource, true>,
    #[serde(default)]
    pub signature: FirstOf<ValueSource, true>,
}

/// A model's refusal to answer. Only a string is a refusal message; a case whose member holds anything else
/// has not recognised one, and the block is left to the rest of the chain.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RefusalBlock {
    #[serde(default)]
    pub doc: Option<String>,
    #[serde(default)]
    pub message: FirstOf<ValueSource, true>,
}

/// Reasoning the provider withheld, kept as the opaque payload a later request replays. The payload is not
/// required, as reasoning's text is not: a producer that names the block has said what it is.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RedactedThinkingBlock {
    #[serde(default)]
    pub doc: Option<String>,
    #[serde(default)]
    pub data: FirstOf<ValueSource, true>,
}

/// A block of a recognised kind in a variant nothing reads, kept whole rather than misread. For a format
/// that announces new variants of a block: claiming it as unknown stops a later case from reading the
/// announcement as something else, and the raw block survives for whoever reads it next.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct UnknownBlock {
    #[serde(default)]
    pub doc: Option<String>,
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
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct UnwrapSpec {
    /// Why this is declared the way it is, for a reader and the explain trace. Read by nothing.
    #[serde(default)]
    pub doc: Option<String>,
    /// The first member that is present is unwrapped, whether or not it normalises.
    #[cfg_attr(test, schemars(with = "FirstOf<String, false>"))]
    #[serde(default)]
    pub from: FirstOf<JsonPath, false>,
    /// The member is a block serialised as JSON text, decoded before it is normalised. A member that does
    /// not decode leaves the original block to the rest of the chain, as one that does not normalise does.
    #[serde(default)]
    pub parse_json: bool,
}

/// Several blocks written as one: the block's member is a **list** of blocks, and each takes the block's place.
///
/// The one form that answers with more than one block, so it is legal only at `message_envelope` - a
/// message's content is a list a block can be spliced into, while every other caller of the chain asks for a
/// single block. A dialect that wraps a provider's whole content list in one part of its own (a text part
/// whose content is the list) otherwise renders the list as one unknown block. Each member is normalised on its
/// own terms, and may itself be spliced; the recursion is bounded because a member is always strictly inside
/// the block that held it. A member that is not a list leaves the block to the chain.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SpliceSpec {
    /// Why this is declared the way it is, for a reader and the explain trace. Read by nothing.
    #[serde(default)]
    pub doc: Option<String>,
    /// The first member that is present is the list, whether or not it is one.
    #[cfg_attr(test, schemars(with = "FirstOf<String, false>"))]
    #[serde(default)]
    pub from: FirstOf<JsonPath, false>,
}

/// Bytes, or a reference to them. The block's kind and whether it is a reference are both *derived*.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MediaBlock {
    /// Why this is declared the way it is, for a reader and the explain trace. Read by nothing.
    #[serde(default)]
    pub doc: Option<String>,
    /// The block's kind, where the format states it rather than leaving it to the media type: an image part
    /// is an image whatever its bytes are labelled. Absent, the kind is derived from the media type, and a
    /// block with none is a `file`.
    #[serde(default)]
    pub kind: Option<MediaKind>,
    /// The kind of a block that states no media type, read from a member that names it - the conventions'
    /// `modality` - through a closed, non-empty map. A stated media type is the stronger fact and decides; a
    /// value the map does not hold leaves the kind to it, and to `file` without one. Refused beside `kind`,
    /// which states the kind outright.
    #[serde(default)]
    pub kind_of: Option<MediaKindOf>,
    #[serde(default)]
    pub media_type: FirstOf<ValueSource, true>,
    /// The media type when no source states one.
    #[serde(default)]
    pub media_type_default: Option<String>,
    /// What a block whose media type is stated nowhere becomes.
    #[serde(default)]
    pub missing_media_type: MissingMediaType,
    #[serde(default)]
    pub data: FirstOf<ValueSource, true>,
    /// What the data is: derived from the value, or stated by the format.
    #[serde(default)]
    pub source: MediaSource,
    /// Optional display name, such as the filename a framework retained beside the bytes.
    #[serde(default)]
    pub name: FirstOf<ValueSource, true>,
    /// How closely a vision model is asked to look at an image.
    #[serde(default)]
    pub detail: FirstOf<ValueSource, true>,
}

/// A member naming a media block's kind, and what each of its values means.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MediaKindOf {
    /// Why this is declared the way it is, for a reader and the explain trace. Read by nothing.
    #[serde(default)]
    pub doc: Option<String>,
    #[cfg_attr(test, schemars(with = "String"))]
    pub path: JsonPath,
    pub map: BTreeMap<String, MediaKind>,
}

/// A media block's canonical kind, spelled as the SideML block type.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, strum::IntoStaticStr)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum MediaKind {
    Image,
    Audio,
    Video,
    Document,
    File,
}

/// What a media block whose media type is stated nowhere becomes.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum MissingMediaType {
    /// Not media: the case declines. Under `decoded` the bytes are asked first, since base64 names its own
    /// type often enough to be worth asking.
    #[default]
    Decline,
    /// Media of no stated type, recorded as `null`.
    Null,
    /// Media of no stated type, with no media-type member at all - a reference whose type is the referenced
    /// object's business.
    Omit,
}

/// What a media block's data is.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq, Default)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum MediaSource {
    /// Derived from the value: a stored file reference, a `data:` URL (whose payload alone is kept), a URL,
    /// or the bytes themselves. A stored reference's or data URL's media type wins over a declared one.
    #[default]
    Decoded,
    /// A stored file reference where the value is one, else this: for a member the format defines as holding
    /// bytes (`base64`) or a location (`url`), which a value's shape must not second-guess. The declared media
    /// type stands.
    ReferenceOr(String),
    /// Always this, for a member that holds an identifier rather than content.
    Literal(String),
}
