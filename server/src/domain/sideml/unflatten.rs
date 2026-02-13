//! Unflatten dotted keys in JSON objects.
//!
//! OpenInference stores attributes as flat keys like:
//! `{"tool_calls.0.tool_call.function.name": "foo"}`
//!
//! This module converts them to nested structure:
//! `{"tool_calls": [{"tool_call": {"function": {"name": "foo"}}}]}`

use serde_json::Value as JsonValue;

/// Unflatten dotted keys in a JSON object into nested structure.
pub fn unflatten_dotted_keys(value: &JsonValue) -> JsonValue {
    let JsonValue::Object(map) = value else {
        return value.clone();
    };

    // Separate dotted keys from regular keys
    let mut regular = serde_json::Map::new();
    let mut dotted: Vec<(&String, &JsonValue)> = Vec::new();

    for (key, val) in map {
        if key.contains('.') {
            dotted.push((key, val));
        } else {
            regular.insert(key.clone(), val.clone());
        }
    }

    // If no dotted keys, return as-is
    if dotted.is_empty() {
        return value.clone();
    }

    // Process dotted keys
    for (dotted_key, val) in dotted {
        set_nested_value(&mut regular, dotted_key, val.clone());
    }

    JsonValue::Object(regular)
}

/// Set a value at a nested path, creating intermediate objects/arrays as needed.
fn set_nested_value(root: &mut serde_json::Map<String, JsonValue>, path: &str, value: JsonValue) {
    let parts: Vec<&str> = path.split('.').collect();
    if parts.is_empty() {
        return;
    }

    let mut current = root;

    for (i, part) in parts.iter().enumerate() {
        let is_last = i == parts.len() - 1;
        let next_is_index = parts.get(i + 1).is_some_and(|p| p.parse::<usize>().is_ok());

        if is_last {
            current.insert(part.to_string(), value);
            return;
        }

        // Check if this part is a numeric index - skip to array handling
        if part.parse::<usize>().is_ok() {
            return;
        }

        // Ensure the key exists with correct type
        let entry = current.entry(part.to_string()).or_insert_with(|| {
            if next_is_index {
                JsonValue::Array(Vec::new())
            } else {
                JsonValue::Object(serde_json::Map::new())
            }
        });

        // Navigate into the next level
        match entry {
            JsonValue::Object(obj) => {
                current = obj;
            }
            JsonValue::Array(arr) => {
                // Next part should be an index
                if let Some(next_part) = parts.get(i + 1)
                    && let Ok(idx) = next_part.parse::<usize>()
                {
                    // Ensure array has enough elements
                    while arr.len() <= idx {
                        arr.push(JsonValue::Object(serde_json::Map::new()));
                    }
                    // Set remaining path on array element
                    let remaining_path = parts[i + 2..].join(".");
                    if remaining_path.is_empty() {
                        arr[idx] = value;
                    } else if let JsonValue::Object(ref mut obj) = arr[idx] {
                        set_nested_value(obj, &remaining_path, value);
                    }
                }
                return;
            }
            _ => return,
        }
    }
}
