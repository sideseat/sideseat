use super::*;

/// One stored field, and every key a producer might carry it under.
///
/// Ordered: the **first source that yields a value of the target's type** wins, which is what a fallback
/// chain means. Not claimed, unlike a message carrier - a key naming a model is evidence about the model
/// whoever else reads it, and two fields legitimately read one key (`gen_ai.request.model` answers both the
/// request model and, for a provider that never states a response model, nothing else).
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SpanFieldRule {
    pub id: String,
    #[serde(default)]
    pub doc: Option<String>,
    /// The stored field this resolves. This engine's own vocabulary, not any producer's.
    pub target: FieldTarget,
    /// How several yielding sources combine.
    #[serde(default)]
    pub combine: FieldCombine,
    /// The sources, in the order they are consulted.
    pub sources: Vec<FieldSource>,
}

/// A stored field a rule may resolve.
///
/// An enum rather than a free string: a typo in an asset would otherwise be a field that is silently never
/// filled, and the sink has to know each field's *type* - the outcome of reading `http.status_code` is an
/// integer or a malformed value, and "the string 200" is not an answer this can store.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum FieldTarget {
    /// Usage **candidates**: what one dialect's embedded object states, resolved whatever the counter chains
    /// answered.
    ///
    /// Separate targets rather than further sources on the counters, because the decision that reads them is
    /// not "which source wins": that dialect's embedded *total* is usable only when the parts actually stored
    /// are the parts it describes, so the test needs its candidate values even where a flat attribute won.
    /// Ordinary resolution would have hidden them. The paths and the gate are data; the agreement test is
    /// arithmetic about our own accounting and stays code.
    UsageCandidateInput,
    UsageCandidateOutput,
    UsageCandidateCacheRead,
    UsageCandidateTotal,
    /// Usage a dialect records **per message**, summed. Candidates rather than counter sources, because the
    /// decision reading them is a *pair*: that dialect fills both sides together when either summed to
    /// anything, so one side's silence is not the same fact as the pair being absent. The paths are data; the
    /// pairing is arithmetic and stays code.
    UsageSummedInput,
    UsageSummedOutput,
    /// Token counters. These do **not** reach the stored span through `apply_field`: the columns are `i64` and
    /// never null, so writing one there would lose the difference between a counter nothing carried and a
    /// genuine `0` - which every framework fallback downstream needs, and which no arithmetic can recover.
    UsageInputTokens,
    UsageOutputTokens,
    UsageTotalTokensReported,
    UsageCacheReadTokens,
    UsageCacheWriteTokens,
    UsageReasoningTokens,
    /// A cost the producer priced itself and stated on the span. The fallback when our own pricing knows
    /// nothing about the model; which of the two a reader sees is enrichment's decision, not the asset's.
    ReportedCostTotal,
    ReportedCostInput,
    ReportedCostOutput,
    /// The name a reader sees. **Not** the raw span name, which everything behavioural keys on: detection,
    /// token scoping, classification and every rule are given the producer's own name, and this is a
    /// presentation value stored beside it.
    DisplaySpanName,
    SessionId,
    GenAiSystem,
    GenAiOperationName,
    GenAiRequestModel,
    GenAiResponseModel,
    GenAiResponseId,
    GenAiTemperature,
    GenAiTopP,
    GenAiTopK,
    GenAiMaxTokens,
    GenAiFrequencyPenalty,
    GenAiPresencePenalty,
    GenAiStopSequences,
    GenAiFinishReasons,
    GenAiAgentId,
    GenAiAgentName,
    GenAiToolName,
    GenAiToolCallId,
    GenAiServerTtftMs,
    GenAiServerRequestDurationMs,
    UserId,
    HttpMethod,
    HttpUrl,
    HttpStatusCode,
    DbSystem,
    DbName,
    DbOperation,
    DbStatement,
    StorageSystem,
    StorageBucket,
    StorageObject,
    MessagingSystem,
    MessagingDestination,
    Tags,
}

impl FieldTarget {
    /// Every target, so a test can put each one to the sink that writes it.
    ///
    /// A hand-written list, and `every_field_target_is_listed` compares its length against the variants in this
    /// file's own enum body - so a variant added without a line here fails the build's test run rather than
    /// quietly escaping the correspondence check below it.
    pub const ALL: &'static [Self] = &[
        Self::UsageCandidateInput,
        Self::UsageCandidateOutput,
        Self::UsageCandidateCacheRead,
        Self::UsageCandidateTotal,
        Self::UsageSummedInput,
        Self::UsageSummedOutput,
        Self::UsageInputTokens,
        Self::UsageOutputTokens,
        Self::UsageTotalTokensReported,
        Self::UsageCacheReadTokens,
        Self::UsageCacheWriteTokens,
        Self::UsageReasoningTokens,
        Self::ReportedCostTotal,
        Self::ReportedCostInput,
        Self::ReportedCostOutput,
        Self::DisplaySpanName,
        Self::SessionId,
        Self::GenAiSystem,
        Self::GenAiOperationName,
        Self::GenAiRequestModel,
        Self::GenAiResponseModel,
        Self::GenAiResponseId,
        Self::GenAiTemperature,
        Self::GenAiTopP,
        Self::GenAiTopK,
        Self::GenAiMaxTokens,
        Self::GenAiFrequencyPenalty,
        Self::GenAiPresencePenalty,
        Self::GenAiStopSequences,
        Self::GenAiFinishReasons,
        Self::GenAiAgentId,
        Self::GenAiAgentName,
        Self::GenAiToolName,
        Self::GenAiToolCallId,
        Self::GenAiServerTtftMs,
        Self::GenAiServerRequestDurationMs,
        Self::UserId,
        Self::HttpMethod,
        Self::HttpUrl,
        Self::HttpStatusCode,
        Self::DbSystem,
        Self::DbName,
        Self::DbOperation,
        Self::DbStatement,
        Self::StorageSystem,
        Self::StorageBucket,
        Self::StorageObject,
        Self::MessagingSystem,
        Self::MessagingDestination,
        Self::Tags,
    ];

    /// What a source must produce to fill this field.
    pub fn field_type(self) -> FieldType {
        match self {
            Self::HttpStatusCode
            | Self::GenAiTopK
            | Self::GenAiMaxTokens
            | Self::GenAiServerTtftMs
            | Self::GenAiServerRequestDurationMs
            | Self::UsageInputTokens
            | Self::UsageOutputTokens
            | Self::UsageTotalTokensReported
            | Self::UsageCacheReadTokens
            | Self::UsageCacheWriteTokens
            | Self::UsageReasoningTokens
            | Self::UsageCandidateInput
            | Self::UsageCandidateOutput
            | Self::UsageCandidateCacheRead
            | Self::UsageCandidateTotal
            | Self::UsageSummedInput
            | Self::UsageSummedOutput => FieldType::Integer,
            Self::GenAiTemperature
            | Self::GenAiTopP
            | Self::GenAiFrequencyPenalty
            | Self::GenAiPresencePenalty
            | Self::ReportedCostTotal
            | Self::ReportedCostInput
            | Self::ReportedCostOutput => FieldType::Float,
            Self::Tags | Self::GenAiStopSequences | Self::GenAiFinishReasons => {
                FieldType::StringList
            }
            Self::SessionId
            | Self::UserId
            | Self::HttpMethod
            | Self::HttpUrl
            | Self::DbSystem
            | Self::DbName
            | Self::DbOperation
            | Self::DbStatement
            | Self::StorageSystem
            | Self::StorageBucket
            | Self::StorageObject
            | Self::MessagingSystem
            | Self::MessagingDestination
            | Self::GenAiSystem
            | Self::GenAiOperationName
            | Self::GenAiRequestModel
            | Self::GenAiResponseModel
            | Self::GenAiResponseId
            | Self::GenAiAgentId
            | Self::GenAiAgentName
            | Self::GenAiToolName
            | Self::GenAiToolCallId
            | Self::DisplaySpanName => FieldType::Text,
        }
    }

    /// The values this field can hold, where the *quantity* bounds them.
    ///
    /// **Ours, not any producer's** - the same reason `field_type` lives here. A count, a duration, a limit and a
    /// probability have ranges that follow from what they measure, and a value outside one is not a small
    /// measurement, it is not a measurement. Nothing checked, so `-5` parsed as an `i64` and became a real token
    /// count: it summed into the trace total, priced at a negative cost, and could cancel a genuine counter.
    /// Producer values are unavailable at compile time, so this is a *reading* refusal - the value is present and
    /// unusable, which is what `Malformed` already means, and the chain's own `on_malformed` policy then decides
    /// whether to step over it or stop.
    ///
    /// Deliberately narrow. `frequency_penalty` and `presence_penalty` are legitimately negative and get no
    /// bound; temperature gets a floor and no ceiling, because providers disagree about the ceiling; `top_p` gets
    /// both, because it is a probability mass. A bound this is not certain of would refuse a producer's honest
    /// value, which is worse than storing an implausible one.
    pub fn admissible(self) -> Option<(f64, f64)> {
        match self {
            // Counts, durations, limits and a status code. None of them can be negative.
            Self::HttpStatusCode
            | Self::GenAiTopK
            | Self::GenAiMaxTokens
            | Self::GenAiServerTtftMs
            | Self::GenAiServerRequestDurationMs
            | Self::UsageInputTokens
            | Self::UsageOutputTokens
            | Self::UsageTotalTokensReported
            | Self::UsageCacheReadTokens
            | Self::UsageCacheWriteTokens
            | Self::UsageReasoningTokens
            | Self::UsageCandidateInput
            | Self::UsageCandidateOutput
            | Self::UsageCandidateCacheRead
            | Self::UsageCandidateTotal
            | Self::UsageSummedInput
            | Self::UsageSummedOutput => Some((0.0, f64::INFINITY)),
            Self::GenAiTemperature => Some((0.0, f64::INFINITY)),
            // A price is never negative; a negative one would cancel real spend in every total it joins.
            Self::ReportedCostTotal | Self::ReportedCostInput | Self::ReportedCostOutput => {
                Some((0.0, f64::INFINITY))
            }
            Self::GenAiTopP => Some((0.0, 1.0)),
            // **Exhaustive, with no catch-all**, and that is the point rather than verbosity. Written as
            // `_ => None` this compiled for every future target and left each one silently unbounded - the same
            // shape as the defect it was added to fix, one level up: a check that passes while seeing less than it
            // claims. `field_type` has always been exhaustive for the same reason, and a new variant must now
            // state its range policy in both places or the build fails.
            //
            // Legitimately unbounded: two penalties a producer may write negative, every text field, and every
            // list.
            Self::GenAiFrequencyPenalty
            | Self::GenAiPresencePenalty
            | Self::Tags
            | Self::GenAiStopSequences
            | Self::GenAiFinishReasons
            | Self::SessionId
            | Self::UserId
            | Self::HttpMethod
            | Self::HttpUrl
            | Self::DbSystem
            | Self::DbName
            | Self::DbOperation
            | Self::DbStatement
            | Self::StorageSystem
            | Self::StorageBucket
            | Self::StorageObject
            | Self::MessagingSystem
            | Self::MessagingDestination
            | Self::GenAiSystem
            | Self::GenAiOperationName
            | Self::GenAiRequestModel
            | Self::GenAiResponseModel
            | Self::GenAiResponseId
            | Self::GenAiAgentId
            | Self::GenAiAgentName
            | Self::GenAiToolName
            | Self::GenAiToolCallId
            | Self::DisplaySpanName => None,
        }
    }
}

/// The shape a field holds, which decides what counts as a source yielding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    Text,
    Integer,
    Float,
    StringList,
}

/// What happens when more than one source yields.
#[derive(Debug, Deserialize, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum FieldCombine {
    /// The first yielding source answers and the rest are not consulted.
    #[default]
    FirstWins,
    /// Every source contributes, in order, duplicates dropped. Only a list field may say this.
    MergeAll,
}

/// One place a field's value may be written.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FieldSource {
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
    /// What a **present but unreadable** value means for the rest of the chain.
    #[serde(default)]
    pub on_malformed: MalformedPolicy,
    /// Whether an **empty** value from this source is an answer rather than something to step over.
    ///
    /// A chain steps over an empty value, which is what a chain is for. A field with one source is not a
    /// chain: `db.system = ""` is what the producer wrote, and reporting it absent is a different statement
    /// about the span.
    #[serde(default)]
    pub accept_empty: bool,
    /// A flat span attribute holding the value directly.
    #[serde(default)]
    pub attribute: Option<String>,
    /// Several attribute spellings of one value, where the **first present** one is the answer.
    ///
    /// The flat counterpart of `JsonFieldSource::first_present_of`, and needed for the same reason: as separate
    /// sources an empty primary would be stepped over and a later alias would answer, while the retired chain
    /// selected the primary by presence and then converted - so an empty one ended the flat chain and an
    /// *embedded* carrier answered instead. Which is a different producer's statement, not a later spelling of
    /// the same one.
    #[serde(default)]
    pub attribute_first_present_of: Vec<String>,
    /// A member of a JSON-valued attribute, reached by RFC 9535 JSONPath.
    #[serde(default)]
    pub json: Option<JsonFieldSource>,
    /// An attribute of one of the span's **events**.
    ///
    /// The primitive `FieldSource` was missing, and its absence is why one reader stayed in Rust: field
    /// resolution was handed a span's attributes and not its events, so `gen_ai.choice`'s `finish_reason` -
    /// the conventions' own spelling, not any dialect's - was scanned for by hand in `extract/mod.rs` *after*
    /// every declared source, a precedence no producer states.
    #[serde(default)]
    pub event_attribute: Option<EventAttributeSource>,
    /// The span's own name, exactly as the producer wrote it.
    ///
    /// A source rather than an implicit default, so a display name states where it comes from - and the
    /// resolver has the raw name whatever the target is.
    #[serde(default)]
    pub raw_span_name: bool,
    /// Fold the answer to lower case.
    ///
    /// For a field whose values are a **case-insensitive enum** and are stored lower case. One dialect writes
    /// `STOP` where the conventions write `stop`, and both mean the same thing - so the alternative is a
    /// stored value whose case depends on which producer wrote the span, which every reader then has to fold
    /// again. Generic: it says the producer's casing is not information, and any source may say so.
    #[serde(default)]
    pub lowercase: bool,
    /// The span's own name, with this prefix stripped.
    ///
    /// A name rather than an attribute, because the conventions prescribe `execute_tool {name}` - the tool's
    /// name is stated *in* the span rather than beside it. Generic: the prefix is the asset's.
    #[serde(default)]
    pub span_name_strip_prefix: Option<String>,
    /// A JSON member whose *presence* admits this source, whatever it holds.
    ///
    /// Distinct from `when`, which asks about the span. A substring search over a serialised payload is not a
    /// test for a **top-level member**: a request naming a tool `system` contains `"system"` and has no
    /// `system` member, and the retired code asked `req.get("system").is_some()`. Read through the same parse
    /// cache as a `json` source, so witnessing a payload costs nothing extra.
    #[serde(default)]
    pub when_json: Option<JsonFieldSource>,
    /// A literal this engine states because a *shape* implies it.
    ///
    /// The one thing a key cannot carry: a dialect that names no provider still says which one it is by the
    /// shape of its request. The literal and the shape that identifies it are both the asset's; the engine
    /// knows only "this typed literal, where the gate holds".
    #[serde(default)]
    pub value: Option<String>,
    /// Consulted only when this holds of the span. Signals are ORed, as everywhere else.
    #[serde(default)]
    pub when: Option<DetectMatch>,
}

/// How several matches of one path become one value.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Reduction {
    /// Add them. A non-numeric match contributes nothing, as the retired reduction's `unwrap_or(0)` did.
    Sum,
    /// Keep every match, in the order the path found them.
    ///
    /// For a list-valued field only. Without it a plural path takes the *first match that yields*, which is
    /// right for "the model sits on whichever agent declared it" and wrong for a field that genuinely is a
    /// list: a completion with two choices has two finish reasons, and reporting one of them is a statement
    /// the producer did not make. Duplicates are kept, because two choices that ended the same way are two.
    CollectAll,
}

/// What a source does when the value it names is present and cannot be read as the field's type.
///
/// Declared per source because the retired chains disagreed, and each disagreement was deliberate. A status
/// code written as a phrase means the producer's own status attribute is wrong, and answering from a *second*
/// key reports another attribute's number as this call's. A request parameter written badly is different: the
/// flat attribute is one of several places a framework may state it, and the retired code fell through to the
/// serialised parameter object - which is the same value from the same producer, not a different call's.
#[derive(Debug, Deserialize, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum MalformedPolicy {
    /// The field is not filled, and the source that stopped it is named in the diagnosis.
    #[default]
    Stop,
    /// Step over it and keep looking, as a chain does for an empty value.
    Continue,
}

/// A value inside a JSON-valued attribute.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct JsonFieldSource {
    /// Why this is declared the way it is, for a reader and the explain trace. Read by nothing.
    #[serde(default)]
    pub doc: Option<String>,
    /// The attribute whose text is parsed. Parsed once per span however many sources name it.
    pub attribute: String,
    /// Where in it the value sits.
    #[serde(default)]
    #[cfg_attr(test, schemars(with = "Option<String>"))]
    pub path: Option<JsonPath>,
    /// Combine every match of a plural path into one value, rather than taking one of them.
    ///
    /// A generic reduction, and the only one: a dialect that records usage per message states the call's usage
    /// as the total across them, which is a statement no single match carries. Yields nothing where the path
    /// matches nothing, so "no such shape" stays distinguishable from a genuine zero.
    #[serde(default)]
    pub reduce: Option<Reduction>,
    /// Each match must be a **scalar string**; an array at the match is malformed here.
    ///
    /// For a list-valued field whose *sources* are single values. `gen_ai.response.finish_reasons` genuinely
    /// holds a list, so the field's type has to accept one - but a producer writing one reason per message
    /// writes a string, and the retired readers took `as_str()`, so a member holding `["stop"]` was **ignored**
    /// and the chain moved to the next producer. Read as a list it answered instead, with a different
    /// producer's value: not a formatting difference but a different statement about why the model stopped.
    #[serde(default)]
    pub scalar_only: bool,
    /// Several spellings of one member, where the **first present** one is the answer.
    ///
    /// Not the same as listing them as separate sources, and the difference is load-bearing: separate sources
    /// select the first that *converts*, so a badly written `max_tokens` beside a good `max_completion_tokens`
    /// would answer from the second - while the retired `or_else` selected by presence and then converted, so
    /// the badly written one ended this carrier's contribution and the *next carrier* answered. Two aliases in
    /// one object are one statement by one producer; two carriers are two.
    #[serde(default)]
    #[cfg_attr(test, schemars(with = "Vec<String>"))]
    pub first_present_of: Vec<JsonPath>,
}
