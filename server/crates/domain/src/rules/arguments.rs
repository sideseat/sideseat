//! How a representation encodes a tool call's arguments, and the one decoder every reader of them shares.
//!
//! Parsing alone cannot tell an encoded object from a tool whose argument really is the string `"{\"x\":1}"`, so
//! the encoding is declared where the arguments are read - a `tool_calls_from`, an arguments member of the message
//! vocabulary - and never guessed from the content. Every reader of a call's arguments decodes through
//! [`decode`], so one call stated two ways has one value.

use serde::Deserialize;
use serde_json::Value as JsonValue;

/// How a member holding a call's arguments encodes them.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ArgumentsEncoding {
    /// The member holds the arguments themselves: a string is a string argument.
    Value,
    /// The member is JSON text, decoded once. A member that is not text is already decoded and passes through;
    /// text that does not decode is kept as the string, as a producer that writes a bare word meant it.
    Json,
    /// As `json`, and a decoded string whose text is a JSON object or array is decoded again: a serialiser that
    /// encoded an already encoded payload. Declared only where a captured fixture shows it, and cited there.
    JsonTwice,
}

/// A call's arguments as `encoding` says `found` holds them.
pub fn decode(encoding: ArgumentsEncoding, found: &JsonValue) -> JsonValue {
    let JsonValue::String(text) = found else {
        return found.clone();
    };
    match encoding {
        ArgumentsEncoding::Value => found.clone(),
        ArgumentsEncoding::Json => serde_json::from_str(text).unwrap_or_else(|_| found.clone()),
        ArgumentsEncoding::JsonTwice => match serde_json::from_str::<JsonValue>(text) {
            Ok(JsonValue::String(inner)) => serde_json::from_str::<JsonValue>(&inner)
                .ok()
                .filter(|value| value.is_object() || value.is_array())
                .unwrap_or(JsonValue::String(inner)),
            Ok(decoded) => decoded,
            Err(_) => found.clone(),
        },
    }
}

/// The arguments a call SideSeat built holds, written for a reader that decodes them as JSON text once: a string
/// argument as its JSON text, anything else as itself. A built call list is read downstream through the message
/// vocabulary's `arguments` member, which declares `json`; a bare string there would be decoded again, and a tool
/// whose argument is the string `"{\"x\":1}"` would be reported as having received an object.
pub fn as_json_text_once(decoded: JsonValue) -> JsonValue {
    match decoded {
        JsonValue::String(_) => JsonValue::String(decoded.to_string()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The adversarial case: a tool whose argument really is a string spelling an object.
    #[test]
    fn a_string_argument_that_spells_an_object_stays_a_string() {
        let literal = json!("{\"x\":1}");
        assert_eq!(decode(ArgumentsEncoding::Value, &literal), literal);
        // Encoded once as JSON text, it decodes to the string, not the object.
        let encoded = json!(literal.to_string());
        assert_eq!(decode(ArgumentsEncoding::Json, &encoded), literal);
        // A built call's string argument survives a reader decoding it once.
        assert_eq!(
            decode(ArgumentsEncoding::Json, &as_json_text_once(literal.clone())),
            literal
        );
    }

    #[test]
    fn each_encoding_decodes_what_it_declares() {
        let object = json!({"x": 1});
        assert_eq!(decode(ArgumentsEncoding::Json, &json!("{\"x\":1}")), object);
        assert_eq!(decode(ArgumentsEncoding::Json, &object), object);
        assert_eq!(
            decode(ArgumentsEncoding::Json, &json!("hello")),
            json!("hello")
        );
        let twice = json!(json!("{\"x\":1}").to_string());
        assert_eq!(decode(ArgumentsEncoding::JsonTwice, &twice), object);
        assert_eq!(decode(ArgumentsEncoding::Json, &twice), json!("{\"x\":1}"));
        assert_eq!(
            decode(ArgumentsEncoding::JsonTwice, &json!("\"plain\"")),
            json!("plain")
        );
    }
}
