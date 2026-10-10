//! The cache key: a hash of everything reconstruction reads from a set of rows.

use std::collections::HashMap;

use sideseat_ports::types::MessageSpanRow;

/// A hash of everything reconstruction reads from these rows.
///
/// Hashing the payloads rather than a cheap proxy like `ingested_at`, because a proxy is a guess about
/// when content changes and this is the one place that must not guess. It is also affordable by a wide
/// margin: BLAKE3 runs at gigabytes per second, so digesting the 68 MB that a thousand-turn replaying
/// session carries costs tens of milliseconds against the 2.6 seconds it saves.
///
/// Order matters and is included: the pipeline sorts internally, but two different row orders are two
/// different inputs as far as this memo is concerned, and treating them as one would be a claim about the
/// pipeline that this file has no business making.
pub(super) fn digest_with(
    rows: &[MessageSpanRow],
    session_of_trace: &HashMap<String, String>,
) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();

    // The ruleset the answer was built with.
    //
    // This cache is a memo over a pure function of the rows, and the carrier rules are part of that
    // function now: reconstruction asks them what each observation is evidence of. Without this, a
    // changed rule would be served answers built by the previous ruleset from rows that had not
    // changed - which is the "no invalidation to get wrong" property the digest key exists to give.
    // Embedded assets make it constant per build, so this costs one hash update per request.
    let ruleset = crate::rules::ruleset_digest();
    hasher.update(&(ruleset.len() as u64).to_le_bytes());
    hasher.update(ruleset.as_bytes());

    // Sorted, so the same grouping hashes the same however the map iterated. Length-prefixed, so
    // `("ab","c")` and `("a","bc")` cannot produce the same bytes.
    let mut grouping: Vec<(&str, &str)> = session_of_trace
        .iter()
        .map(|(t, s)| (t.as_str(), s.as_str()))
        .collect();
    grouping.sort_unstable();
    hasher.update(&(grouping.len() as u64).to_le_bytes());
    for (trace, session) in grouping {
        hasher.update(&(trace.len() as u64).to_le_bytes());
        hasher.update(trace.as_bytes());
        hasher.update(&(session.len() as u64).to_le_bytes());
        hasher.update(session.as_bytes());
    }

    hasher.update(&(rows.len() as u64).to_le_bytes());
    for row in rows {
        // Hydrated rows already carry one compact digest over the three large bodies. Old callers that
        // have not run hydration retain the exact old behavior and hash the inline payloads themselves.
        match row.body_cache_key.as_deref() {
            Some(key) => {
                hasher.update(&[1u8]);
                hasher.update(&(key.len() as u64).to_le_bytes());
                hasher.update(key.as_bytes());
            }
            None => {
                hasher.update(&[0u8]);
                for body in [
                    row.messages_json.as_str(),
                    row.tool_definitions_json.as_str(),
                    row.tool_names_json.as_str(),
                ] {
                    hasher.update(&(body.len() as u64).to_le_bytes());
                    hasher.update(body.as_bytes());
                }
            }
        }
        // Outside the hydrated key: log records are joined by the query, not hydrated from span bodies, so a
        // log arriving for an unchanged span changes the answer while the span's body key stays the same.
        hasher.update(&(row.log_messages_json.len() as u64).to_le_bytes());
        hasher.update(row.log_messages_json.as_bytes());

        // Present-or-absent is hashed as well as the value. Mapping `None` to `""` made them the same
        // input, and an empty attribute is accepted - so a span whose `status_code` went from absent to
        // empty (or the reverse) shared a key with its old self, and a warm instance would answer with the
        // field missing where a cold one reconstructs it as present-and-empty.
        for field in [
            Some(row.trace_id.as_str()),
            Some(row.span_id.as_str()),
            row.parent_span_id.as_deref(),
            row.session_id.as_deref(),
            row.model.as_deref(),
            row.provider.as_deref(),
            row.status_code.as_deref(),
            row.exception_type.as_deref(),
            row.exception_message.as_deref(),
            row.exception_stacktrace.as_deref(),
            row.observation_type.as_deref(),
            row.scope_name.as_deref(),
            row.scope_version.as_deref(),
            row.span_name.as_deref(),
            row.framework.as_deref(),
            row.response_model.as_deref(),
            row.response_id.as_deref(),
            row.finish_reasons.as_deref(),
            Some(row.request_thread.as_str()),
            Some(row.request_frame.as_str()),
        ] {
            match field {
                // Length-prefixed, so `("ab", "c")` and `("a", "bc")` are different inputs.
                Some(value) => {
                    hasher.update(&[1u8]);
                    hasher.update(&(value.len() as u64).to_le_bytes());
                    hasher.update(value.as_bytes());
                }
                None => {
                    hasher.update(&[0u8]);
                }
            }
        }
        hasher.update(&row.span_timestamp.timestamp_micros().to_le_bytes());
        hasher.update(
            &row.span_end_timestamp
                .map(|t| t.timestamp_micros())
                .unwrap_or(i64::MIN)
                .to_le_bytes(),
        );
        // `ingested_at` is read, not merely stored: `group_and_sort_traces` breaks a tie between two
        // traces with the same earliest span timestamp by the earliest ingestion time, and that decides
        // which trace's history the other one strips against. Two row sets differing only here are
        // genuinely different inputs.
        hasher.update(&row.ingested_at.timestamp_micros().to_le_bytes());
        // A mark decides whether a read-time projection withdraws the row.
        hasher.update(&row.span_marks.to_le_bytes());
        hasher.update(&row.input_tokens.to_le_bytes());
        hasher.update(&row.output_tokens.to_le_bytes());
        hasher.update(&row.total_tokens.to_le_bytes());
        hasher.update(&row.cost_total.to_le_bytes());
        // The envelope scalars. They do not change reconstruction itself, but they change the
        // *response* - a corrected temperature or a re-priced cost split must not be served from a
        // stale entry under an unchanged key, which is exactly the widening rule the design record
        // states: a field that affects the answer must reach the digest.
        hasher.update(&row.temperature.unwrap_or(f64::MIN).to_le_bytes());
        hasher.update(&row.top_p.unwrap_or(f64::MIN).to_le_bytes());
        hasher.update(&row.max_tokens.unwrap_or(i64::MIN).to_le_bytes());
        hasher.update(&row.cache_read_tokens.to_le_bytes());
        hasher.update(&row.cache_write_tokens.to_le_bytes());
        hasher.update(&row.reasoning_tokens.to_le_bytes());
        hasher.update(&row.cost_input.to_le_bytes());
        hasher.update(&row.cost_output.to_le_bytes());
    }
    *hasher.finalize().as_bytes()
}
