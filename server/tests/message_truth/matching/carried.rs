//! Every scalar a raw span holds, and the usage counts it states where its producer states them: what the call
//! checks read as a span's own statement.

use std::collections::BTreeSet;

/// Every scalar a raw span holds, JSON payloads inside its strings included.
#[derive(Default)]
pub(super) struct Carried {
    strings: BTreeSet<String>,
    numbers: BTreeSet<i64>,
    /// The strings under a member that names a finish (`finish_reason`, `stopReason`): a finish word
    /// elsewhere - an output item's own `status`, a sentence - is not the span stating the call's.
    finish_words: BTreeSet<String>,
    /// The span as it arrived, for reading a usage count where its producer states one.
    raw: Option<serde_json::Value>,
    skip_requests: bool,
}

/// A member whose name puts it in a request namespace (`gen_ai.request.model`, `request_model`).
fn states_a_request(key: &str) -> bool {
    key.split(['.', '_'])
        .any(|segment| segment.eq_ignore_ascii_case("request"))
}

impl Carried {
    pub(super) fn of(raw: Option<&str>) -> Self {
        Self::walked(raw, false)
    }

    /// The scalars outside every member that states a request: `gen_ai.request.model` names the model
    /// asked for, so its value is not evidence that the span states the model that answered.
    pub(super) fn of_answer(raw: Option<&str>) -> Self {
        Self::walked(raw, true)
    }

    fn walked(raw: Option<&str>, skip_requests: bool) -> Self {
        let mut carried = Carried {
            skip_requests,
            ..Carried::default()
        };
        if let Some(value) = raw.and_then(|r| serde_json::from_str::<serde_json::Value>(r).ok()) {
            carried.walk(&value, 0, false);
            carried.raw = Some(value);
        }
        carried
    }

    fn walk(&mut self, value: &serde_json::Value, depth: usize, finish: bool) {
        use serde_json::Value;
        match value {
            Value::String(text) => {
                if let Ok(n) = text.trim().parse::<i64>() {
                    self.numbers.insert(n);
                }
                if depth < 8
                    && matches!(text.trim_start().chars().next(), Some('{' | '['))
                    && let Ok(inner) = serde_json::from_str::<Value>(text)
                {
                    self.walk(&inner, depth + 1, finish);
                }
                if finish {
                    self.finish_words.insert(text.to_ascii_lowercase());
                }
                self.strings.insert(text.clone());
            }
            Value::Number(n) => {
                let whole = n
                    .as_i64()
                    .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64));
                if let Some(i) = whole {
                    self.numbers.insert(i);
                }
            }
            Value::Array(items) => items.iter().for_each(|v| self.walk(v, depth, finish)),
            Value::Object(map) => {
                for (key, member) in map {
                    if !(self.skip_requests && states_a_request(key)) {
                        let lower = key.to_ascii_lowercase();
                        let names_finish = lower.contains("finish") || lower.contains("stop");
                        self.walk(member, depth, finish || names_finish);
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn string(&self, value: &str) -> bool {
        self.strings.contains(value)
    }

    pub(super) fn finish_word(&self, value: &str) -> bool {
        self.finish_words.contains(&value.to_ascii_lowercase())
    }

    pub(super) fn number(&self, value: i64) -> bool {
        value != 0 && self.numbers.contains(&value)
    }

    /// Whether the span states `value` as this usage count where its producer states one: at a location the
    /// usage vocabulary declares for that count (`super::usage_locations`). The evidence a gapped count needs, so
    /// a reconstruction that swapped two counters, or took a limit, a prompt's text or another span's figure for
    /// a count, is not excused by the value appearing somewhere.
    pub(super) fn count(&self, assertion: &str, value: i64) -> bool {
        self.raw
            .as_ref()
            .is_some_and(|raw| super::usage_locations::states(raw, assertion, value))
    }
}

#[cfg(test)]
mod tests {
    use super::Carried;

    fn carried(name: &str, attributes: serde_json::Value) -> Carried {
        Carried::of(Some(
            &serde_json::json!({ "name": name, "attributes": attributes }).to_string(),
        ))
    }

    /// A count is the span's statement where the usage vocabulary declares its producer states it - a counter
    /// attribute, or a path into a declared body - and nowhere else.
    #[test]
    fn a_count_is_stated_only_where_its_producer_states_it() {
        let span = carried(
            "chat",
            serde_json::json!({
                "gen_ai.usage.input_tokens": 1030,
                "gen_ai.usage.output_tokens": 0,
                "gen_ai.usage.thoughts_token_count": 12,
                "gen_ai.usage.raw": "{\"output_tokens_details\": {\"thinking_tokens\": 13}}",
                "gen_ai.usage.cache_read.input_tokens": 40,
                "gcp.vertex.agent.llm_response": "{\"usage_metadata\": {\"candidates_token_count\": 87}}",
                "response_data": "{\"usage\": {\"output_tokens\": 88}}",
                "haystack.agent.step.llm.output": {"replies": [{"meta": {"usage": {"completion_tokens": 89}}}]},
                "gen_ai.request.max_output_tokens": 6000,
                "gen_ai.input.messages": "[{\"content\": \"{\\\"usage\\\": {\\\"output_tokens\\\": 5000}}\"}]",
                "output.value": "{\"content\": \"{\\\"usage\\\": {\\\"output_tokens\\\": 5002}}\"}",
                "input_tokens": 77,
            }),
        );
        assert!(span.count("call.usage.input", 1030));
        // An exported zero is a stated count.
        assert!(span.count("call.usage.output", 0));
        assert!(span.count("call.usage.output", 87));
        assert!(span.count("call.usage.output", 88));
        assert!(span.count("call.usage.output", 89));
        assert!(span.count("call.usage.reasoning", 12));
        assert!(span.count("call.usage.reasoning", 13));
        assert!(span.count("call.usage.cache_read", 40));
        // A cache counter is not plain input, a limit is no count, a message quotes freely, a counter of
        // another kind is not this count, and a bare name counts only on the spans declared to write it.
        assert!(!span.count("call.usage.input", 40));
        assert!(!span.count("call.usage.output", 12));
        assert!(!span.count("call.usage.output", 6000));
        assert!(!span.count("call.usage.output", 5000));
        assert!(!span.count("call.usage.output", 5002));
        assert!(!span.count("call.usage.output", 1030));
        assert!(!span.count("call.usage.input", 77));
        assert!(
            carried(
                "claude_code.api_request",
                serde_json::json!({"input_tokens": 77})
            )
            .count("call.usage.input", 77)
        );
    }
}

#[cfg(test)]
mod condition_tests {
    use super::Carried;

    fn carried(attributes: serde_json::Value) -> Carried {
        Carried::of(Some(
            &serde_json::json!({ "name": "task", "attributes": attributes }).to_string(),
        ))
    }

    /// A location limited by a compound condition is consulted where the condition holds, and only there.
    #[test]
    fn a_conditioned_location_counts_where_its_condition_holds() {
        let body = "{\"token_usage\": {\"prompt_tokens\": 1030}}";
        let crew = carried(serde_json::json!({"crew_id": "c", "output.value": body}));
        assert!(crew.count("call.usage.input", 1030));
        let other = carried(serde_json::json!({"output.value": body}));
        assert!(!other.count("call.usage.input", 1030));
    }

    /// A figure no integer states exactly is no count: an out-of-range number does not saturate into one.
    #[test]
    fn an_out_of_range_figure_states_no_count() {
        let span = carried(serde_json::json!({
            "response_data": "{\"usage\": {\"output_tokens\": 9223372036854775808}}",
        }));
        assert!(!span.count("call.usage.output", i64::MAX));
    }
}

#[cfg(test)]
mod scope_tests {
    use super::Carried;

    /// A link's and the resource's attributes are not the span's statement of its usage.
    #[test]
    fn only_the_span_and_its_events_state_its_usage() {
        let raw = serde_json::json!({
            "attributes": {"gen_ai.usage.output_tokens": 87},
            "events": [{"attributes": {"gen_ai.usage.output_tokens": 86}}],
            "links": [{"attributes": {"gen_ai.usage.output_tokens": 6000123}}],
            "resource": {"attributes": {"gen_ai.usage.output_tokens": 6000124}},
        });
        let span = Carried::of(Some(&raw.to_string()));
        assert!(span.count("call.usage.output", 87));
        assert!(span.count("call.usage.output", 86));
        assert!(!span.count("call.usage.output", 6000123));
        assert!(!span.count("call.usage.output", 6000124));
    }
}
