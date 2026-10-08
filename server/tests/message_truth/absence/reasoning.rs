//! Reasoning, searched for by its part rather than its text: whether a payload marks text as reasoning,
//! holds a reasoning part at all, or holds the opaque signature that seals one.

use serde_json::Value;

use super::haystack::{Haystack, MAX_DEPTH, collapse_whitespace};
use super::{Proof, find_node, holds, prove_text};
use crate::message_truth::truth::Fact;

/// Whether a member name or a `type` value names reasoning.
fn names_reasoning(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("reason") || text.contains("think") || text.contains("thought")
}

/// A key a provider writes reasoning's opaque replay token under: `signature`, `thoughtSignature`,
/// `encrypted`, `encrypted_content`.
fn names_signature(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.ends_with("signature") || key.starts_with("encrypted")
}

/// What an opaque replay token looks like: long, and only base64 or base64url characters.
fn opaque(text: &str) -> bool {
    text.len() >= 32
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'-' | b'_'))
}

/// The SHA-256 of a signature, as a truth's `seal` writes it.
fn seal_of(token: &str) -> String {
    use sha2::Digest as _;
    crate::message_truth::truth::hex_digest(&sha2::Sha256::digest(token.as_bytes()))
}

/// The opaque tokens a node holds under a signature or encrypted member.
fn signatures_in(node: &Value) -> Vec<&str> {
    node.as_object()
        .map(|map| {
            map.iter()
                .filter(|(key, _)| names_signature(key))
                .filter_map(|(_, value)| value.as_str().filter(|v| opaque(v)))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether a token is the one sought: the fact's own where its seal is known, else any.
fn sought(token: &str, seal: Option<&str>) -> bool {
    seal.is_none_or(|seal| seal_of(token) == seal)
}

/// A withheld reasoning step's signature is not exported where no payload, decoded as far as the search
/// decodes, holds it under a signature or encrypted member. With the fact's `seal` that is its own signature
/// - another turn's proves nothing about this one; without one, any signature at all refuses the claim.
pub(super) fn prove_signature(seal: Option<&str>, haystack: &Haystack) -> Proof {
    if let Some(at) = haystack.undecoded.first() {
        return Proof::Unprovable(format!(
            "{at} is encoded deeper than the search decodes ({MAX_DEPTH} layers)"
        ));
    }
    for carrier in &haystack.carriers {
        if let Some(at) = find_node(carrier, |node| {
            signatures_in(node).into_iter().any(|t| sought(t, seal))
        }) {
            return Proof::Present(format!("{at} holds the signature"));
        }
        if let Some((at, _)) = carrier.strings.iter().find(|(at, value)| {
            at.rsplit(['.', '|']).next().is_some_and(names_signature)
                && opaque(value)
                && sought(value, seal)
        }) {
            return Proof::Present(format!("{at} is the signature"));
        }
    }
    Proof::Absent
}

/// Reasoning the model signed and withheld the text of: an empty text, marked signed.
pub(super) fn is_withheld(fact: &Fact) -> bool {
    fact.value.get("text").and_then(Value::as_str) == Some("")
        && fact.value.get("signed").and_then(Value::as_bool) == Some(true)
}

/// Withheld reasoning has no text to search for, only its part and its signature. It is present where a
/// payload holds its signature - known by the fact's `seal` - and where a payload holds a reasoning part
/// that carries no signature at all (a JSON object typed as reasoning or flagged as one, `thought: true`, or
/// a flattened attribute typing a part as reasoning): an unsigned part could be this one, so it refuses
/// the claim rather than being assumed to be another turn's. A part sealed with another turn's signature
/// proves nothing about this one. Without a seal any reasoning part at all is presence. Absent only where
/// none of that is in any payload, decoded as far as the search decodes.
///
/// A member merely named for reasoning is not a part: `thinking: {type: enabled, budget_tokens}` is a
/// request's configuration. Nor is any signature: a tool's `(city: str)` is not an opaque token.
pub(super) fn prove_reasoning_part(seal: Option<&str>, haystack: &Haystack) -> Proof {
    let typed = |node: &Value| {
        node.as_object().is_some_and(|map| {
            map.iter().any(|(key, value)| {
                (key == "type" && value.as_str().is_some_and(names_reasoning))
                    || (key == "thought" && *value == Value::Bool(true))
            })
        })
    };
    for carrier in &haystack.carriers {
        if let Some(at) = find_node(carrier, |node| {
            let tokens = signatures_in(node);
            tokens.iter().any(|t| sought(t, seal))
                || (typed(node) && (tokens.is_empty() || seal.is_none()))
        }) {
            return Proof::Present(format!(
                "{at} holds the reasoning part, or one that could be it"
            ));
        }
        for (at, kind) in carrier.strings.iter() {
            if !(at.ends_with(".type") && names_reasoning(kind)) {
                continue;
            }
            let base = &at[..at.len() - ".type".len()];
            let signed_by: Vec<&str> = carrier
                .strings
                .iter()
                .filter(|(other, value)| {
                    other.strip_prefix(base).is_some_and(|rest| {
                        rest.trim_start_matches(['.', '|'])
                            .split(['.', '|'])
                            .next()
                            .is_some_and(names_signature)
                    }) && opaque(value)
                })
                .map(|(_, value)| value.as_str())
                .collect();
            if seal.is_none() || signed_by.is_empty() || signed_by.iter().any(|t| sought(t, seal)) {
                return Proof::Present(format!("{at} types a part as reasoning that could be it"));
            }
        }
        if let Some((at, _)) = carrier.strings.iter().find(|(at, value)| {
            seal.is_some()
                && at.rsplit(['.', '|']).next().is_some_and(names_signature)
                && opaque(value)
                && sought(value, seal)
        }) {
            return Proof::Present(format!("{at} is the reasoning's signature"));
        }
    }
    Proof::Absent
}

/// Reasoning exported as text: the text is somewhere (else it is a missing fact, not a mislabelled one),
/// and nothing holding it names reasoning: no JSON object that holds it has a reasoning member or `type`,
/// and no attribute of a flattened family beside it is a reasoning `type`.
pub(super) fn prove_kind(fact: &Fact, haystack: &Haystack) -> Proof {
    if let Some(at) = haystack.undecoded.first() {
        return Proof::Unprovable(format!(
            "{at} is encoded deeper than the search decodes ({MAX_DEPTH} layers)"
        ));
    }
    let text = fact.text();
    if !matches!(prove_text(text, haystack), Proof::Present(_)) {
        return Proof::Unprovable(format!("{}'s text is not in the payloads at all", fact.id));
    }
    let needle = collapse_whitespace(text);
    let marks = |node: &Value| {
        node.as_object().is_some_and(|map| {
            map.iter().any(|(key, value)| {
                names_reasoning(key)
                    || (key == "type" && value.as_str().is_some_and(names_reasoning))
            })
        })
    };
    for carrier in &haystack.carriers {
        if let Some(at) = find_node(carrier, |node| {
            marks(node) && holds(node, &Value::String(text.to_string()))
        }) {
            return Proof::Present(format!("{at} marks the text as reasoning"));
        }
        for (at, value) in carrier.strings.iter().filter(|(_, v)| v.contains(&needle)) {
            let base = at.rsplit_once('.').map_or(at.as_str(), |(base, _)| base);
            if carrier.strings.iter().any(|(other, kind)| {
                other.starts_with(base) && other.ends_with(".type") && names_reasoning(kind)
            }) {
                return Proof::Present(format!("{at} is typed as reasoning beside {value:?}"));
            }
        }
    }
    Proof::Absent
}
