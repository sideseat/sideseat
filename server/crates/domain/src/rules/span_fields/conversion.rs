//! Raw values read as a field's type - strictly, in both directions, and whole or not at all.

use serde_json::Value as JsonValue;

use super::{FieldType, Reading};

pub(super) fn from_text(raw: &str, field_type: FieldType) -> Reading {
    match field_type {
        FieldType::Text => {
            if raw.is_empty() {
                Reading::Empty
            } else {
                Reading::Text(raw.to_string())
            }
        }
        FieldType::Integer => {
            if raw.is_empty() {
                Reading::Empty
            } else {
                match raw.parse::<i64>() {
                    Ok(value) => Reading::Integer(value),
                    Err(error) => Reading::Malformed {
                        detail: error.to_string(),
                    },
                }
            }
        }
        FieldType::Float => {
            if raw.is_empty() {
                return Reading::Empty;
            }
            match raw.parse::<f64>() {
                // A non-finite value is **not** a number this can store: it survives no JSON round trip and
                // reads back as null, so it is a producer's mistake rather than a measurement.
                Ok(value) if value.is_finite() => Reading::Float(value),
                Ok(_) => Reading::Malformed {
                    detail: "not a finite number".to_string(),
                },
                Err(error) => Reading::Malformed {
                    detail: error.to_string(),
                },
            }
        }
        FieldType::StringList => {
            // A JSON list converts whole or not at all, exactly as a JSON member does: the comma fallback
            // read `["a", true]` as two texts, one of them invented, and split a valid quoted member on its
            // own comma. A bracketed value that is not JSON keeps the fallback - producers do write
            // `[a, b]` unquoted, and those members are texts.
            let trimmed = raw.trim();
            if trimmed.starts_with('[')
                && let Ok(value) = serde_json::from_str::<JsonValue>(trimmed)
            {
                return from_json(&value, FieldType::StringList);
            }
            let items = sideseat_core::utils::string::parse_string_array(raw);
            if items.is_empty() {
                Reading::Empty
            } else {
                Reading::StringList(items)
            }
        }
    }
}

/// A JSON value, read as the field's type - **strictly**, in both directions.
///
/// A number is not a text value and a quoted number is not a number. Both coercions were tried and both were
/// divergences: rendering a numeric `metadata.thread_id` invented a session identifier no other span will
/// match, and accepting a quoted count took a value the retired `as_i64` skipped, letting one carrier answer
/// where the next used to. A structure is refused for the same reason it always was: an object where a scalar
/// belongs is a producer bug, and reporting it as absent hides it.
pub(super) fn from_json(value: &JsonValue, field_type: FieldType) -> Reading {
    match field_type {
        FieldType::Text => match value {
            JsonValue::String(text) if text.is_empty() => Reading::Empty,
            JsonValue::String(text) => Reading::Text(text.clone()),
            JsonValue::Null => Reading::Absent,
            // Deliberately *not* coerced. A number or a boolean where a text field belongs is a producer
            // mistake, and rendering it invents an identifier no other span will match - which for a session
            // id means a conversation of one.
            other => Reading::Malformed {
                detail: format!("expected a string, found {}", kind_of(other)),
            },
        },
        FieldType::Integer => match value {
            JsonValue::Number(number) => match number.as_i64() {
                Some(found) => Reading::Integer(found),
                None => Reading::Malformed {
                    detail: format!("{number} is not a whole number"),
                },
            },
            JsonValue::Null => Reading::Absent,
            // A quoted count is not a count. Every retired read of a JSON numeric member went through
            // `as_i64`, so coercing text here would accept a value that chain skipped.
            other => Reading::Malformed {
                detail: format!("expected a number, found {}", kind_of(other)),
            },
        },
        FieldType::Float => match value {
            JsonValue::Number(number) => match number.as_f64() {
                Some(found) if found.is_finite() => Reading::Float(found),
                _ => Reading::Malformed {
                    detail: format!("{number} is not a finite number"),
                },
            },
            JsonValue::Null => Reading::Absent,
            other => Reading::Malformed {
                detail: format!("expected a number, found {}", kind_of(other)),
            },
        },
        FieldType::StringList => match value {
            JsonValue::Array(items) => {
                // Every member or none: a list holding a member no conversion supports is malformed, not a
                // shorter list - `["a", true]` read as `["a"]` silently dropped what the producer wrote, and
                // a wholly unsupported list read as `Empty` claimed the producer wrote none.
                let mut texts: Vec<String> = Vec::with_capacity(items.len());
                for item in items {
                    match item {
                        JsonValue::String(text) => texts.push(text.clone()),
                        JsonValue::Number(number) => texts.push(number.to_string()),
                        other => {
                            return Reading::Malformed {
                                detail: format!(
                                    "a list member is {}, not text or a number",
                                    kind_of(other)
                                ),
                            };
                        }
                    }
                }
                if texts.is_empty() {
                    Reading::Empty
                } else {
                    Reading::StringList(texts)
                }
            }
            JsonValue::String(text) if text.is_empty() => Reading::Empty,
            JsonValue::String(text) => Reading::StringList(vec![text.clone()]),
            JsonValue::Null => Reading::Absent,
            other => Reading::Malformed {
                detail: format!("expected a list, found {}", kind_of(other)),
            },
        },
    }
}

fn kind_of(value: &JsonValue) -> &'static str {
    match value {
        JsonValue::Null => "null",
        JsonValue::Bool(_) => "a boolean",
        JsonValue::Number(_) => "a number",
        JsonValue::String(_) => "a string",
        JsonValue::Array(_) => "a list",
        JsonValue::Object(_) => "an object",
    }
}
