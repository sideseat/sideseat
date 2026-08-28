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
//! multiplicity - which is why this is four independent facts rather than one enum. The distinction
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
    /// The carrier may re-state earlier turns, so its observations can be history rather than news.
    pub carrier_may_contain_history_or_state: bool,
}

impl CarrierSemantics {
    /// One emission: everything in it happened now, and two of anything are two.
    const EMISSION: Self = Self {
        position_proves_distinct_occurrence: true,
        position_provides_sequence_order: true,
        carrier_is_atomic_emission: true,
        carrier_may_contain_history_or_state: false,
    };

    /// A conversation as one span saw it: ordered, may repeat earlier turns, and a repeat inside it
    /// is a re-statement rather than a second occurrence.
    const SNAPSHOT: Self = Self {
        position_proves_distinct_occurrence: false,
        position_provides_sequence_order: true,
        carrier_is_atomic_emission: false,
        carrier_may_contain_history_or_state: true,
    };

    /// Framework state that happens to contain messages - LangChain's `output.value`, an agent's
    /// accumulated scratchpad. Ordered, re-lists itself, and says nothing about multiplicity.
    const ACCUMULATED_STATE: Self = Self {
        position_proves_distinct_occurrence: false,
        position_provides_sequence_order: true,
        carrier_is_atomic_emission: false,
        carrier_may_contain_history_or_state: true,
    };
}

/// The semantics of the carrier an observation came from.
///
/// `event` and `attribute` are the carrier's name as recorded on the block; exactly one is set.
///
/// The default for an unrecognised carrier is [`CarrierSemantics::SNAPSHOT`], the cautious reading:
/// it declines to treat position as proof of a second occurrence, so a carrier nobody has classified
/// cannot invent messages. It can only under-report, which the answer invariant would catch.
pub fn semantics_for(event: Option<&str>, attribute: Option<&str>) -> CarrierSemantics {
    if let Some(event) = event {
        return match event {
            // The model's own output, and tool execution results: each is one emission.
            "gen_ai.choice"
            | "gen_ai.content.completion"
            | "gen_ai.output.messages"
            | "gen_ai.tool.message"
            | "gen_ai.tool.result" => CarrierSemantics::EMISSION,
            // A re-sent turn, by definition history. `gen_ai.assistant.message` is the awkward one:
            // it is a replay for most frameworks and the actual output for a choiceless Logfire
            // generation span, so it is read as a snapshot and direction is decided elsewhere.
            _ => CarrierSemantics::SNAPSHOT,
        };
    }

    match attribute {
        // The generic IO pair, and the framework-state attributes that behave like it. LangChain's
        // `output.value` re-lists its own tool calls, which is the case that forced this distinction.
        Some("input.value") | Some("output.value") | Some("message") | Some("messages") => {
            CarrierSemantics::ACCUMULATED_STATE
        }
        // An explicit message array: a conversation as this span saw it.
        Some(key)
            if key.starts_with("gen_ai.input.messages")
                || key.starts_with("gen_ai.output.messages")
                || key.starts_with("llm.input_messages")
                || key.starts_with("llm.output_messages")
                || key.starts_with("ai.prompt")
                || key.starts_with("ai.response") =>
        {
            CarrierSemantics::SNAPSHOT
        }
        _ => CarrierSemantics::SNAPSHOT,
    }
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
                carrier.carrier_may_contain_history_or_state,
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
