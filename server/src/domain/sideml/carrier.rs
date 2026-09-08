//! What a carrier's structure is evidence *of*.
//!
//! A carrier is the event or attribute an observation was read from. Reconstruction keeps asking the
//! same question of it - are these two identical-looking things one message seen twice, or two
//! messages? - and the answer depends on what kind of carrier it is, not on the content:
//!
//! - A `gen_ai.choice` event is one emission. Two tool calls in it are two calls, whether or not the
//!   provider sent ids, because a model asking twice is exactly what that looks like.
//! - LangChain's `output.value` is accumulated framework state. It re-lists its own messages, so the
//!   same call appears at two positions while describing one call.
//!
//! Both are ordered and both may contain history. They differ only in whether *position* proves
//! multiplicity - which is why this is independent facts rather than one enum.
//!
//! Note what that sentence is **not** saying: it describes a snapshot against accumulated *framework state*,
//! and it is not the difference between the `snapshot` and `accumulated_state` presets, which differ only in
//! whether the carrier holds the span's output. An `EMISSION` also may **not** contain history, which the
//! sentence above would otherwise seem to allow. The distinction
//! was previously an unstated global rule ("trust the id, fall back to position"), which happened to
//! give the right answer for both cases and said nothing about why.
//!
//! These are claims about the carrier, and a structural test cannot prove them: identical JSON can
//! represent accumulated state or distinct occurrences. What it can do is require every carrier the
//! corpus produces to be classified deliberately, which `carrier_semantics_are_declared` does.

/// What one carrier's shape tells reconstruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CarrierSemantics {
    /// Two observations at different positions are two occurrences, not one seen twice.
    ///
    /// True for a single emission; false for accumulated state, which re-lists what it already said.
    pub position_proves_distinct_occurrence: bool,
    /// Positions state the order the observations belong in.
    ///
    /// Almost always true - it is what `assert_carrier_subsequence` checks - and false only where a
    /// carrier is a bag rather than a sequence.
    pub position_provides_sequence_order: bool,
    /// The carrier is one emission, so its observations belong together and stay contiguous.
    pub carrier_is_atomic_emission: bool,
    /// The carrier may re-state observations that already happened, so what it holds can be a replay
    /// rather than news.
    ///
    /// **Not the same question as `may_contain_framework_state`, and the two were one bit until cycle 9.**
    /// The conflated form named both and answered neither: dedup reads *this* one (a re-send regenerates
    /// the provider's call id, so an id from a carrier that may replay is not evidence of a second
    /// execution), while framework state is a claim about the carrier holding a scratchpad rather than a
    /// conversation. And `carrier_is_atomic_emission` does **not** exclude it: `gen_ai.tool.message` is one
    /// atomic emission whose whole purpose is handing a *past* tool result back to a model. Atomicity is
    /// about occurrence and grouping, not about freshness.
    pub may_restate_prior_observations: bool,
    /// The carrier holds accumulated framework state - a scratchpad, a graph's state dict, a chain's
    /// aggregate - rather than (only) a conversation.
    ///
    /// Separate from `may_restate_prior_observations` because state and replay have different consumers:
    /// a replay is prior *observations*, which is why their ids cannot be trusted, while state is the
    /// framework's own bookkeeping, which is why its *positions* prove nothing about multiplicity. A
    /// carrier can be either without being the other - `gen_ai.tool.message` replays without being state,
    /// and a freshly-built state dict is state without replaying anything.
    pub may_contain_framework_state: bool,
    /// The span *produced* what this carrier holds, rather than receiving it.
    ///
    /// Declared per carrier because it cannot be inferred from the others, and because inferring it
    /// from a prefix list is what let it drift: the Vercel SDK moved from `ai.result.*` to
    /// `ai.response.*` and the extractor followed while the list did not, so every Vercel response read
    /// as something the span *received*. It also cannot be derived from
    /// `carrier_is_atomic_emission` - `output.value` is accumulated state and still the span's output -
    /// nor from the event name alone: `gen_ai.tool.result` on the span that made the call is that
    /// span's own record of what came back, while `gen_ai.tool.message` is the framework handing a past
    /// result back to a model.
    ///
    /// The ordering resolver reads it to decide what a generation *received*, which is the input side
    /// of "input precedes output". Deriving that side by negating an inferred flag made it read a tool
    /// answer as a precondition of the call that produced it.
    pub carrier_holds_span_output: bool,
    /// The carrier is a *detached request frame*: the system instruction a generation was given,
    /// reported beside the conversation rather than inside it.
    ///
    /// This is a fact about the carrier, deliberately not about the role. `developer` normalises to
    /// `System`, and a system message *inside* an ordered array is a turn in that array, framed by its
    /// position - `adk/image_gen`'s second instruction legitimately sits mid-trace at index 9. Only the
    /// detached carriers assert "this precedes every other input of the request that carried me", which
    /// is the edge the ordering resolver builds from it: a frame is not a turn that happened after the
    /// question, it is the frame the request was made in.
    pub carrier_is_detached_request_frame: bool,
    /// The carrier holds what the span *received*: the conversation, prompt or state given to it.
    ///
    /// Declared for the same reason the output side is, and it is **not** the negation of it: a carrier may
    /// be neither (a tool manifest), and a snapshot a span both received and re-reports is one it received.
    /// Deriving one side from the other made a tool answer read as a precondition of the call that
    /// produced it.
    pub carrier_holds_span_input: bool,
    /// The carrier holds an array of *messages* that is expanded into one observation each.
    ///
    /// A fact about the carrier, not about the value: plenty of carriers hold arrays that must stay whole -
    /// a content-block list, a tool manifest, a context array - and expanding one of those turns a single
    /// message into several fragments.
    pub carrier_holds_expandable_message_array: bool,
}

impl CarrierSemantics {
    /// One emission: everything in it happened now, and two of anything are two.
    pub(crate) const EMISSION: Self = Self {
        carrier_holds_span_input: false,
        carrier_holds_expandable_message_array: false,
        position_proves_distinct_occurrence: true,
        position_provides_sequence_order: true,
        carrier_is_atomic_emission: true,
        may_restate_prior_observations: false,
        may_contain_framework_state: false,
        carrier_holds_span_output: true,
        carrier_is_detached_request_frame: false,
    };

    /// A conversation as one span saw it: ordered, may repeat earlier turns, and a repeat inside it
    /// is a re-statement rather than a second occurrence.
    pub(crate) const SNAPSHOT: Self = Self {
        carrier_holds_span_input: false,
        carrier_holds_expandable_message_array: false,
        position_proves_distinct_occurrence: false,
        position_provides_sequence_order: true,
        carrier_is_atomic_emission: false,
        may_restate_prior_observations: true,
        may_contain_framework_state: false,
        carrier_holds_span_output: false,
        carrier_is_detached_request_frame: false,
    };

    /// Framework state that happens to contain messages - LangChain's `output.value`, an agent's
    /// accumulated scratchpad. Ordered, re-lists itself, and says nothing about multiplicity.
    /// Accumulated state is the span's *output* - `output.value` is what the chain produced - while
    /// still being a re-listing rather than one emission. That combination is why direction cannot be
    /// derived from `carrier_is_atomic_emission`.
    pub(crate) const ACCUMULATED_STATE: Self = Self {
        carrier_holds_span_input: false,
        carrier_holds_expandable_message_array: false,
        position_proves_distinct_occurrence: false,
        position_provides_sequence_order: true,
        carrier_is_atomic_emission: false,
        may_restate_prior_observations: true,
        may_contain_framework_state: true,
        carrier_holds_span_output: true,
        carrier_is_detached_request_frame: false,
    };
}

/// The semantics of the carrier an observation came from, with no knowledge of the span that wrote it.
///
/// `event` and `attribute` are the carrier's name as recorded on the block; exactly one is set.
///
/// This is the *unqualified* reading, and it is what a caller that cannot describe the span gets. A
/// rule clause constraining the observation type or the span name makes a claim about the span, and a
/// caller with no span context has not established it - so such a caller can only ever match a generic
/// clause. Prefer [`semantics_for_context`] wherever the span is known.
///
/// The default for an unrecognised carrier is [`CarrierSemantics::SNAPSHOT`], the cautious reading:
/// it declines to treat position as proof of a second occurrence, so a carrier nobody has classified
/// cannot invent messages. It can only under-report, which the answer invariant would catch.
///
/// **No production path uses this any more**: every one supplies what it knows about the span, so a
/// clause qualified by observation type or span name applies wherever it is true rather than only on the
/// paths that happened to pass context. It survives for the equivalence oracle, which compares the rules
/// against a table that had no notion of a span.
#[cfg(test)]
pub fn semantics_for(event: Option<&str>, attribute: Option<&str>) -> CarrierSemantics {
    declared_semantics(event, attribute).unwrap_or(CarrierSemantics::SNAPSHOT)
}

/// The semantics of a carrier, read together with what is known about the span that wrote it.
///
/// The same carrier name means different things on different spans: `gen_ai.output.messages` on a
/// generation span is the model's own emission, and on an orchestration span it is that span
/// re-listing the turn its children produced. Only the second form can be told apart by asking about
/// the span, which is why this exists beside [`semantics_for`].
pub fn semantics_for_context(ctx: &crate::domain::rules::CarrierContext<'_>) -> CarrierSemantics {
    crate::domain::rules::ruleset()
        .carriers
        .resolve(ctx)
        .map(|clause| clause.semantics)
        .unwrap_or(CarrierSemantics::SNAPSHOT)
}

/// The declared entry for a carrier, or `None` where no rule names it.
///
/// Separate from [`semantics_for`] so that "nobody has classified this" is distinguishable from
/// "classified, and it reads as a snapshot". The two are the same *value* and completely different
/// facts, and `carrier_semantics_are_declared` needs to tell them apart - a test that compared the
/// value could not, and reported every declared snapshot carrier as unclassified.
///
/// Whether a carrier of this name holds an array of messages that expands into one observation each.
///
/// Asked by **name alone**, which is what the retired list did, and the fact is a property of the key
/// rather than of the span: an array of messages is one whether a chain or an agent wrote it. A clause
/// that needs span context therefore does not answer here, and such a carrier falls to the caller's
/// residue list rather than being guessed at.
pub fn holds_expandable_message_array(attribute: &str) -> bool {
    crate::domain::rules::ruleset()
        .carriers
        .resolve(&crate::domain::rules::CarrierContext::carrier_only(
            None,
            Some(attribute),
        ))
        .is_some_and(|clause| clause.semantics.carrier_holds_expandable_message_array)
}

/// Test-only for the same reason as [`semantics_for`]: production asks with context.
#[cfg(test)]
pub fn declared_semantics(
    event: Option<&str>,
    attribute: Option<&str>,
) -> Option<CarrierSemantics> {
    crate::domain::rules::ruleset()
        .carriers
        .resolve(&crate::domain::rules::CarrierContext::carrier_only(
            event, attribute,
        ))
        .map(|clause| clause.semantics)
}

/// The declared entry for a carrier read with span context, or `None` where no rule names it.
///
/// The distinction matters wherever a *declared* answer must end a question that an undeclared carrier
/// leaves open - direction, above all: a residual list that still ran after a clause answered `false`
/// made the negative half of every declaration unstatable.
pub fn declared_semantics_for_context(
    ctx: &crate::domain::rules::CarrierContext<'_>,
) -> Option<CarrierSemantics> {
    crate::domain::rules::ruleset()
        .carriers
        .resolve(ctx)
        .map(|clause| clause.semantics)
}

/// The table this engine replaced, kept as the equivalence oracle.
///
/// Not dead code and not history: the rules must reproduce it exactly for every carrier read *without*
/// span context, and `the_rules_reproduce_the_legacy_carrier_table` asserts that over every carrier
/// name either source names. That is what makes the migration a provable no-op plus a reviewed delta,
/// rather than a rewrite whose correctness rests on the goldens alone - and goldens can bless a
/// regression.
#[cfg(test)]
pub(crate) fn legacy_declared_semantics(
    event: Option<&str>,
    attribute: Option<&str>,
) -> Option<CarrierSemantics> {
    let mut semantics = legacy_declared_semantics_without_direction(event, attribute)?;
    let (input, expandable) = legacy_direction_facts(attribute);
    semantics.carrier_holds_span_input = input;
    semantics.carrier_holds_expandable_message_array = expandable;
    Some(semantics)
}

/// What the retired `is_input_source` list said, and what `MESSAGE_ARRAY_SOURCES` said.
///
/// The reference for these two facts is a *list*, not the table: direction on the receiving side and
/// "this array expands into one message each" were decided in `feed/types.rs` and `normalize.rs` while the
/// table decided the other four. The oracle composes both, so the assets are held to what production did
/// rather than to whichever half is convenient.
#[cfg(test)]
fn legacy_direction_facts(attribute: Option<&str>) -> (bool, bool) {
    const LEGACY_INPUT_PREFIXES: &[&str] =
        &["llm.input_messages", "gen_ai.input.", "gen_ai.prompt."];
    const LEGACY_INPUT_EXACT: &[&str] = &[
        "input.value",
        "gcp.vertex.agent.llm_request",
        "gcp.vertex.agent.data",
        "ai.prompt",
        "lk.input_text",
        "lk.user_input",
        "lk.instructions",
        "lk.chat_ctx",
        "mlflow.spanInputs",
        "traceloop.entity.input",
        "pydantic_ai.all_messages",
        "request_data",
    ];
    const LEGACY_EXPANDABLE: &[&str] = &[
        "gen_ai.input.messages",
        "gen_ai.output.messages",
        "ai.prompt.messages",
        "mlflow.spanInputs",
        "mlflow.spanOutputs",
        "request_data",
        "response_data",
    ];
    let Some(attribute) = attribute else {
        return (false, false);
    };
    let input = LEGACY_INPUT_EXACT.contains(&attribute)
        || LEGACY_INPUT_PREFIXES
            .iter()
            .any(|prefix| attribute.starts_with(prefix));
    (input, LEGACY_EXPANDABLE.contains(&attribute))
}

#[cfg(test)]
fn legacy_declared_semantics_without_direction(
    event: Option<&str>,
    attribute: Option<&str>,
) -> Option<CarrierSemantics> {
    if let Some(event) = event {
        return Some(match event {
            // The model's own output, and a span's record of a tool it ran: each is one emission the
            // span produced.
            "gen_ai.choice"
            | "gen_ai.content.completion"
            | "gen_ai.output.messages"
            | "gen_ai.tool.result" => CarrierSemantics::EMISSION,
            // One emission too, but *received*: this is the framework handing a past result back to a
            // model, so its time is the hand-back and it is input to whatever the span then produces.
            // Reading it as output made a re-sent result look like a generation's own answer.
            "gen_ai.tool.message" => CarrierSemantics {
                carrier_holds_span_output: false,
                ..CarrierSemantics::EMISSION
            },
            // A re-sent turn, by definition history. `gen_ai.assistant.message` is the awkward one:
            // it is a replay for most frameworks and the actual output for a choiceless Logfire
            // generation span, so it is read as a snapshot and direction is decided elsewhere.
            // A re-sent turn, by definition history. `gen_ai.assistant.message` is the awkward one:
            // it is a replay for most frameworks and the actual output for a choiceless Logfire
            // generation span, so it is read as a snapshot and direction is decided elsewhere.
            "gen_ai.user.message"
            | "gen_ai.system.message"
            | "gen_ai.assistant.message"
            | "gen_ai.content.prompt"
            | "gen_ai.input.messages" => CarrierSemantics::SNAPSHOT,
            _ => return None,
        });
    }

    Some(match attribute {
        // The generic IO pair, and the framework-state attributes that behave like it. LangChain's
        // `output.value` re-lists its own tool calls, which is the case that forced this distinction.
        Some("output.value") => CarrierSemantics::ACCUMULATED_STATE,
        // The same shape on the receiving side: state handed *to* the span. Same reading of position
        // and history, opposite direction - which is why direction is its own fact.
        Some("input.value") | Some("message") | Some("messages") => CarrierSemantics {
            carrier_holds_span_output: false,
            ..CarrierSemantics::ACCUMULATED_STATE
        },
        // What this span *produced*: one response, in one payload. Ordered, its own, and two of
        // anything in it are two - the same reading as `gen_ai.choice`, which is the event form of the
        // same thing. Being an emission is what keeps a response's parts together: Vercel puts a
        // turn's intro text and the tool calls it introduces in one `ai.response`, and reading that as
        // a snapshot let the two be ordered independently, so the text sorted after its own calls.
        Some(key)
            if key.starts_with("gen_ai.output.messages")
                || key.starts_with("llm.output_messages")
                || key.starts_with("ai.response") =>
        {
            CarrierSemantics::EMISSION
        }
        // What this span *received*: a conversation as it saw it, which may re-state earlier turns.
        Some(key)
            if key.starts_with("gen_ai.input.messages")
                || key.starts_with("llm.input_messages")
                || key.starts_with("ai.prompt")
                // Logfire's request payload, and the Claude Code CLI's turns and tool results.
                || key == "request_data"
                || key == "new_context" =>
        {
            CarrierSemantics::SNAPSHOT
        }
        // The system prompt a model was given, under each framework's name for it: semconv's,
        // the Claude Code CLI's, and Strands'. Received like a snapshot, and additionally a
        // *detached request frame* - see the field's own comment. `gen_ai.system.message` is
        // deliberately not here: an event in a conversation stream is in-band, and may be a turn.
        Some("gen_ai.system_instructions") | Some("user_system_prompt") | Some("system_prompt") => {
            CarrierSemantics {
                carrier_is_detached_request_frame: true,
                ..CarrierSemantics::SNAPSHOT
            }
        }
        // A tool span's own pair: it was handed the arguments and it produced the result. One emission
        // each - a tool is called once - differing only in direction, which is the clearest case for
        // direction being its own fact rather than something inferred from the shape.
        //
        // `gen_ai.tool.call.*` is the same pair under the *current* conventions, which is how the Vercel AI
        // SDK's present integration reports a tool call: pure `gen_ai.*`, no `ai.*` at all. Same reading,
        // because it is the same fact written to a newer name.
        Some("ai.toolCall.result") | Some("gen_ai.tool.call.result") => CarrierSemantics::EMISSION,
        Some("ai.toolCall.args") | Some("gen_ai.tool.call.arguments") | Some("tool_name") => {
            CarrierSemantics {
                carrier_holds_span_output: false,
                ..CarrierSemantics::EMISSION
            }
        }
        // The model's reply, under the Claude Code CLI's name for it.
        Some("response.model_output") => CarrierSemantics::EMISSION,
        // The error built from a span's exception fields. The span produced it, it is not a re-send,
        // and it has no position in any payload - it is composed rather than read.
        Some("exception") => CarrierSemantics {
            position_provides_sequence_order: false,
            ..CarrierSemantics::EMISSION
        },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_emission_proves_multiplicity_and_state_does_not() {
        // The case this distinction exists for, from both sides.
        assert!(
            semantics_for(Some("gen_ai.choice"), None).position_proves_distinct_occurrence,
            "two calls in one choice event are two calls - a model asking twice looks exactly like \
             this, and ids may be absent"
        );
        assert!(
            !semantics_for(None, Some("output.value")).position_proves_distinct_occurrence,
            "LangChain's output.value re-lists its own tool calls, so two positions there describe \
             one call"
        );
    }

    #[test]
    fn state_and_snapshots_are_still_ordered_and_may_hold_history() {
        for carrier in [
            semantics_for(None, Some("output.value")),
            semantics_for(None, Some("gen_ai.input.messages")),
        ] {
            assert!(
                carrier.position_provides_sequence_order,
                "both state a sequence, which is what the carrier-subsequence invariant reads"
            );
            assert!(
                carrier.may_restate_prior_observations,
                "both can re-state earlier turns"
            );
        }
    }

    #[test]
    fn an_unknown_carrier_takes_the_cautious_reading() {
        let unknown = semantics_for(None, Some("some.framework.newAttribute"));
        assert!(
            !unknown.position_proves_distinct_occurrence,
            "an unclassified carrier must not invent occurrences: it can under-report, which the \
             answer invariant catches, but over-reporting shows as duplicates a user sees"
        );
        assert_eq!(unknown, CarrierSemantics::SNAPSHOT);
    }
}
