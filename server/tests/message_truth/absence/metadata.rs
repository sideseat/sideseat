//! Call metadata a producer states wrongly, and whether any payload carries the right value.

use super::haystack::Haystack;
use super::{MAX_DEPTH, Proof, truth};

/// The wire's count a usage gap names, as every convention the call check accepts it in: input as the wire
/// states it, and in the other cache convention.
pub(super) fn usage_counts(call: &truth::Call, field: &str) -> Vec<Option<String>> {
    let Some(usage) = &call.usage else {
        return Vec::new();
    };
    let cache = usage.cache_read.unwrap_or(0) + usage.cache_write.unwrap_or(0);
    let counts = match field {
        // Input as the wire states it, and in the other convention, as the call check accepts it.
        "usage.input" => usage.input.map_or_else(Vec::new, |input| {
            let other = if usage.input_includes_cache {
                input - cache
            } else {
                input + cache
            };
            vec![input, other]
        }),
        "usage.output" => usage.output.into_iter().collect(),
        "usage.cache_read" => usage.cache_read.into_iter().collect(),
        "usage.cache_write" => usage.cache_write.into_iter().collect(),
        _ => usage.reasoning.into_iter().collect(),
    };
    counts
        .into_iter()
        .map(|count| Some(count.to_string()))
        .collect()
}

/// A model name or response id is absent when no payload contains it; a finish when no member naming a
/// finish (`finish_reason`, `stopReason`) holds the category or the provider's word, in any case - a
/// finish word elsewhere, a block's `type: tool_use` or a sentence, is not a finish stated; a usage count when
/// no payload holds the wire's count as a number.
pub(super) fn prove_metadata(field: &str, values: &[String], haystack: &Haystack) -> Proof {
    if let Some(at) = haystack.undecoded.first() {
        return Proof::Unprovable(format!(
            "{at} is encoded deeper than the search decodes ({MAX_DEPTH} layers)"
        ));
    }
    if values.iter().all(|v| v.trim().is_empty()) {
        return Proof::Unprovable(format!("the truth states no {field}"));
    }
    // A usage count is a number wherever a payload holds it - an integer attribute, an integer inside a decoded
    // body, an integer written as text - and any payload holding the wire's count refuses the gap: reading it
    // is then a parser fix, not a limitation.
    if field.starts_with("usage.") {
        if let Some(at) = haystack.carriers.iter().find_map(|c| c.inexact.first()) {
            return Proof::Unprovable(format!(
                "{at} is a figure too large for a float to state exactly, so which count it is is unknown"
            ));
        }
        let counts: Vec<i64> = values.iter().filter_map(|v| v.parse().ok()).collect();
        for carrier in &haystack.carriers {
            if let Some((at, _)) = carrier.numbers.iter().find(|(_, n)| counts.contains(n)) {
                return Proof::Present(at.clone());
            }
        }
        return Proof::Absent;
    }
    for carrier in &haystack.carriers {
        for (at, text) in &carrier.strings {
            let found = values.iter().filter(|v| !v.is_empty()).any(|value| {
                if field == "finish" {
                    let key = at.to_ascii_lowercase();
                    (key.contains("finish") || key.contains("stop"))
                        && text.eq_ignore_ascii_case(value)
                } else {
                    text.contains(value.as_str())
                }
            });
            if found {
                return Proof::Present(at.clone());
            }
        }
    }
    Proof::Absent
}

#[cfg(test)]
mod tests {
    use super::usage_counts;

    fn call(input_includes_cache: bool) -> super::truth::Call {
        serde_json::from_value(serde_json::json!({
            "id": "call-001", "attempt": 1, "outcome": "success", "conversation": "conv-1",
            "api": "bedrock.converse", "streamed": true, "status": 200, "model": null,
            "response_model": null, "response_id": null, "stop_reason": null, "finish": null,
            "usage": {"input": 100, "output": 7, "cache_read": 20, "cache_write": 0, "reasoning": null,
                      "input_includes_cache": input_includes_cache},
            "outputs": [],
        }))
        .expect("a call")
    }

    /// The input counts a usage gap searches for are the ones the call check accepts: as the wire states it, and
    /// in the other cache convention - never a third figure, which would refuse a gap over a count no payload
    /// carries.
    #[test]
    fn an_input_count_is_searched_in_the_conventions_the_check_accepts() {
        let counts = |includes| {
            usage_counts(&call(includes), "usage.input")
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
        };
        assert_eq!(counts(false), ["100", "120"]);
        assert_eq!(counts(true), ["100", "80"]);
    }
}
