//! Where the telemetry records a part: the spans whose own raw carriers hold it, as exact evidence.

use std::collections::BTreeSet;

use sha2::Digest as _;

use super::haystack::{Carrier, Haystack, collapse_whitespace};
use super::wire_id;
use crate::message_truth::predicates::json_eq;
use crate::message_truth::truth::Fact;

/// The spans, as (trace, span), whose own raw carriers - a span with its events, or a log record it emitted -
/// record `fact` itself: where a view may attribute a part its producing span does not carry.
///
/// Exact evidence, not the absence search's: refuting absence accepts any containing string, so a span
/// quoting an answer inside a debug message would refute it, and a quotation is not provenance. Here a text is
/// recorded where one of the carrier's strings *is* that text; withheld reasoning where a string's digest is
/// its seal (the signature's SHA-256); a tool call where a subtree is its arguments, beside its own wire id
/// (or, with no id, its tool's name); a result where a subtree or a string is its value. A text is not recorded
/// by a carrier that records a tool call (`call_ids`): that carrier is the call's own record.
pub(in super::super) fn carrying_spans(
    fact: &Fact,
    haystack: &Haystack,
    call_ids: &BTreeSet<&str>,
) -> BTreeSet<(String, String)> {
    let text = collapse_whitespace(fact.text());
    let records = |carrier: &Carrier| -> bool {
        match fact.kind.as_str() {
            // The call itself, not a mention of it: its arguments, beside its own wire id where it has one -
            // an id alone names the call, and arguments alone are shared by two calls asking the same thing -
            // and beside its tool's name where it has none.
            "tool_call" => {
                let arguments = carrier
                    .nodes
                    .iter()
                    .any(|(_, node)| json_eq(node, &fact.value["arguments"]));
                let identified = match wire_id(fact) {
                    Some(id) => carrier.strings.iter().any(|(_, s)| s == id),
                    None => fact.value["name"].as_str().is_some_and(|name| {
                        !name.is_empty() && carrier.strings.iter().any(|(_, s)| s == name)
                    }),
                };
                arguments && identified
            }
            "tool_result" => {
                let value = &fact.value["value"];
                carrier.nodes.iter().any(|(_, node)| json_eq(node, value))
                    || value.as_str().is_some_and(|v| {
                        carrier
                            .strings
                            .iter()
                            .any(|(_, s)| *s == collapse_whitespace(v))
                    })
            }
            // Withheld reasoning has no text to match; its signature is the part as exported.
            // The seal is the signature's digest, so a carrier string is hashed before it is compared.
            "reasoning" if text.is_empty() => fact.seal.as_deref().is_some_and(|seal| {
                carrier.strings.iter().any(|(_, s)| {
                    crate::message_truth::truth::hex_digest(&sha2::Sha256::digest(s.as_bytes()))
                        == seal
                })
            }),
            // A text, in a carrier that records no tool call: a carrier holding some call's wire id is that
            // call's record - its run, its result - so a text there is the tool's result repeating it, not the
            // answer it repeats.
            // The id must sit in an attribute of its own, not one decoded out of the attribute holding the text:
            // an answer whose JSON mentions a call is still the answer. An attribute is its location up to the
            // first decoding step (`|`).
            _ => {
                let root = |at: &str| at.split('|').next().unwrap_or(at).to_string();
                let held: BTreeSet<String> = carrier
                    .strings
                    .iter()
                    .filter(|(_, s)| *s == text)
                    .map(|(at, _)| root(at))
                    .collect();
                !text.is_empty()
                    && !held.is_empty()
                    && !carrier
                        .strings
                        .iter()
                        .any(|(at, s)| call_ids.contains(s.as_str()) && !held.contains(&root(at)))
            }
        }
    };
    haystack
        .carriers
        .iter()
        .filter(|carrier| records(carrier))
        .filter_map(|carrier| Some((carrier.trace.clone()?, carrier.span.clone()?)))
        .collect()
}
