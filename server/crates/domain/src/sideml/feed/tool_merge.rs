use super::*;

// ============================================================================
// INTERNAL: METADATA
// ============================================================================

/// Compute metadata from processed blocks. `span_view` asks for the usage of the one span the view shows.
pub(in crate::sideml::feed) fn compute_metadata(
    blocks: &[BlockEntry],
    span_rows: &[MessageSpanRow],
    replay_matching_complete: bool,
    span_view: bool,
) -> FeedMetadata {
    // Keyed by (trace, span): a span id is unique only within a trace, and a session view holds
    // several traces, so counting by span id alone under-reported the span count.
    let span_ids: HashSet<_> = blocks.iter().map(|b| (&b.trace_id, &b.span_id)).collect();

    // The span's own row, once: a re-ingested span appears twice in the DuckDB row set - that query reads the
    // raw table, while ClickHouse reads it with FINAL.
    // No row, no span: nothing recorded usage, which is not a recorded zero.
    let span_usage = span_rows
        .first()
        .filter(|_| span_view)
        .map(|row| SpanUsage {
            total_tokens: row.total_tokens,
            total_cost: row.cost_total,
        });

    FeedMetadata {
        block_count: blocks.len(),
        span_count: span_ids.len(),
        span_usage,
        replay_matching_complete,
        composed_from_requests: 0,
        composition_truncated: false,
        framed_by_records: 0,
        frames_truncated: false,
    }
}

// ============================================================================
// INTERNAL: DEDUPLICATION
// ============================================================================

/// Deduplicate tool definitions by name, sort alphabetically.
///
/// Strategy:
/// 1. Normalize provider-specific formats to OpenAI-style tool definitions.
/// 2. Merge definitions with the same name to preserve complementary fields.
/// 3. Use quality score only to choose merge base / break ties.
pub fn deduplicate_tools(raw: Vec<JsonValue>) -> Vec<JsonValue> {
    // One name may have **several** definitions, because two producers can state the same tool differently and
    // mean different things by it. A swarm's agents each declare `Delegate work to coworker` with a description
    // enumerating *their own* coworkers; merged onto one name, whichever won quality replaced the other, and the
    // list then told the user the wrong coworkers for one of the agents - a statement about that agent nobody
    // made. Two forms where one merely states *less* are still one tool, which is what the merge is for.
    let mut by_name: HashMap<String, Vec<JsonValue>> = HashMap::with_capacity(raw.len());

    for def in raw {
        let normalized = normalize_tools(&def);
        let defs = match normalized {
            JsonValue::Array(arr) => arr,
            single => vec![single],
        };

        for canonical in defs {
            // Already canonical: `normalize_tools` applies the declared shapes, whose last clause is the
            // named-object shape this used to re-derive here. Retired to an oracle rather than deleted -
            // `the_declared_shapes_leave_nothing_for_the_retired_canonicaliser` is what holds them equal.
            let Some(name) = extract_tool_name(&canonical) else {
                // Keyed by name, so a definition stating none cannot be kept - and that used to be a silent
                // drop. It is reported rather than filtered upstream because this is the only place it decides
                // anything: `tool_definitions` is carried as `Vec<JsonValue>` the whole way, so an item no shape
                // recognises costs its siblings nothing until a name is needed.
                tracing::debug!(
                    target: "sideseat::sideml",
                    definition = %canonical,
                    "a tool definition states no name under any declared shape, so it cannot be listed"
                );
                continue;
            };
            let group = by_name.entry(name).or_default();
            // Merged into the first form it does not contradict; a contradiction starts a new one.
            match group
                .iter_mut()
                .find(|existing| contradiction_between(existing, &canonical).is_none())
            {
                Some(existing) => {
                    *existing = merge_tool_definitions(existing.clone(), canonical);
                }
                None => group.push(canonical),
            }
        }
    }

    let mut tools: Vec<(String, Vec<JsonValue>)> = by_name.into_iter().collect();
    tools.sort_by(|a, b| a.0.cmp(&b.0));
    // Within a name, the order they were stated in - deterministic, and the order a reader met them.
    tools.into_iter().flat_map(|(_, defs)| defs).collect()
}

/// A member two tool definitions both state and **disagree** about, if any.
///
/// The question that separates one tool stated twice from two tools sharing a name. A carrier that names a tool
/// and nothing else, beside one carrying its whole schema, is one tool described at two levels of detail - and
/// merging those is exactly what `merge_tool_definitions` is for. Two producers that both said something and said
/// different things are not that: the merge keeps whichever scores higher and drops the other, so the survivor is
/// presented as *the* definition and the other producer's statement is gone with nothing saying so.
///
/// Compared **leaf by leaf through objects**, so a schema is compared where it differs rather than as one opaque
/// value: two parameter objects differing only in a member one of them omits are not a disagreement.
///
/// **Arrays are not descended into**, and that is not a shortcut - comparing them by index says two carriers
/// disagree about element 0 when one wrote `required: ["city"]` and the other `required: ["days"]`, which are
/// complementary halves of one schema and exactly what `merge_json_schema` unions. A set has no element 0. The
/// cost is that two genuinely different `enum` lists read as one union rather than as a disagreement, which is
/// the same answer the merge would give anyway.
fn contradiction_between(a: &JsonValue, b: &JsonValue) -> Option<String> {
    fn leaves(prefix: &str, value: &JsonValue, out: &mut Vec<(String, String)>) {
        match value {
            JsonValue::Object(members) => {
                for (key, member) in members {
                    leaves(&format!("{prefix}/{key}"), member, out);
                }
            }
            JsonValue::Array(_) => {}
            scalar => out.push((prefix.to_string(), scalar.to_string())),
        }
    }

    let mut theirs = Vec::new();
    leaves("", b, &mut theirs);
    if theirs.is_empty() {
        return None;
    }
    let mut mine = Vec::new();
    leaves("", a, &mut mine);
    let mine: HashMap<String, String> = mine.into_iter().collect();
    theirs.into_iter().find_map(|(path, value)| {
        mine.get(&path)
            .filter(|stated| **stated != value)
            .map(|stated| format!("{path}: {stated} vs {value}"))
    })
}

/// The hand-written canonicaliser this file used to apply after `normalize_tools`, kept as an **oracle**.
///
/// It was a second vocabulary of provider shapes - `parameters` / `input_schema` / `inputSchema`, plus `strict` -
/// sitting downstream of the declared one, and the two disagreed: a payload the assets did not recognise passed
/// through `normalize_tools` unchanged and was wrapped *here*, so the same tool was shown wrapped on the path
/// that ran this and raw on the path that did not (`normalize_tools_message`, which files its result straight
/// into a `ToolDefinitions` block). The spellings are declared now, in `tool-shapes.bare_name`.
#[cfg(test)]
pub(in crate::sideml) fn canonicalize_tool_definition(tool: JsonValue) -> JsonValue {
    if tool.get("function").is_some() {
        return tool;
    }

    let Some(name) = tool.get("name").and_then(|n| n.as_str()) else {
        return tool;
    };
    let mut function = json!({ "name": name });
    if let Some(desc) = tool.get("description") {
        function["description"] = desc.clone();
    }
    if let Some(params) = tool
        .get("parameters")
        .or_else(|| tool.get("input_schema"))
        .or_else(|| tool.get("inputSchema"))
    {
        function["parameters"] = params.clone();
    }

    let mut canonical = json!({
        "type": "function",
        "function": function
    });
    if let Some(strict) = tool.get("strict") {
        canonical["strict"] = strict.clone();
    }
    canonical
}

fn function_map(def: &JsonValue) -> Option<&serde_json::Map<String, JsonValue>> {
    def.get("function")
        .and_then(|f| f.as_object())
        .or_else(|| def.as_object())
}

fn function_map_mut(def: &mut JsonValue) -> Option<&mut serde_json::Map<String, JsonValue>> {
    if def.get("function").and_then(|f| f.as_object()).is_some() {
        return def.get_mut("function").and_then(|f| f.as_object_mut());
    }
    def.as_object_mut()
}

fn is_weak_description(desc: &str) -> bool {
    let d = desc.trim();
    d.is_empty()
        || d.eq_ignore_ascii_case("none")
        || d.eq_ignore_ascii_case("n/a")
        || d.eq_ignore_ascii_case("unknown")
        || d.eq_ignore_ascii_case("no description")
}

fn merge_tool_definitions(a: JsonValue, b: JsonValue) -> JsonValue {
    let qa = tool_definition_quality(&a);
    let qb = tool_definition_quality(&b);

    let (mut primary, secondary) = if qb > qa { (b, a) } else { (a, b) };

    let secondary_func = function_map(&secondary).cloned();
    let Some(secondary_func) = secondary_func else {
        return primary;
    };

    let Some(primary_func) = function_map_mut(&mut primary) else {
        return primary;
    };

    if let Some(secondary_desc) = secondary_func.get("description").and_then(|d| d.as_str()) {
        let primary_desc = primary_func
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("");
        if is_weak_description(primary_desc) && !is_weak_description(secondary_desc) {
            primary_func.insert(
                "description".to_string(),
                JsonValue::String(secondary_desc.to_string()),
            );
        }
    }

    if let Some(secondary_params) = secondary_func.get("parameters") {
        match primary_func.get_mut("parameters") {
            Some(primary_params) => merge_json_schema(primary_params, secondary_params),
            None => {
                primary_func.insert("parameters".to_string(), secondary_params.clone());
            }
        }
    }

    if let Some(strict_val) = secondary.get("strict").and_then(|v| v.as_bool())
        && strict_val
    {
        primary["strict"] = JsonValue::Bool(true);
    }

    primary
}

fn merge_json_schema(primary: &mut JsonValue, secondary: &JsonValue) {
    let (Some(primary_obj), Some(secondary_obj)) = (primary.as_object_mut(), secondary.as_object())
    else {
        if primary.is_null() && !secondary.is_null() {
            *primary = secondary.clone();
        }
        return;
    };

    for (key, secondary_val) in secondary_obj {
        match key.as_str() {
            "properties" => merge_properties(primary_obj, secondary_val),
            "required" => merge_required(primary_obj, secondary_val),
            _ => match primary_obj.get_mut(key) {
                Some(primary_val) => {
                    if primary_val.is_null() {
                        *primary_val = secondary_val.clone();
                    } else if primary_val.is_object() && secondary_val.is_object() {
                        merge_json_schema(primary_val, secondary_val);
                    }
                }
                None => {
                    primary_obj.insert(key.clone(), secondary_val.clone());
                }
            },
        }
    }
}

fn merge_properties(
    primary_obj: &mut serde_json::Map<String, JsonValue>,
    secondary_props: &JsonValue,
) {
    let Some(secondary_props_obj) = secondary_props.as_object() else {
        return;
    };

    match primary_obj.get_mut("properties") {
        Some(JsonValue::Object(primary_props_obj)) => {
            for (prop_name, secondary_prop) in secondary_props_obj {
                match primary_props_obj.get_mut(prop_name) {
                    Some(primary_prop) => merge_property_schema(primary_prop, secondary_prop),
                    None => {
                        primary_props_obj.insert(prop_name.clone(), secondary_prop.clone());
                    }
                }
            }
        }
        _ => {
            primary_obj.insert(
                "properties".to_string(),
                JsonValue::Object(secondary_props_obj.clone()),
            );
        }
    }
}

fn merge_property_schema(primary_prop: &mut JsonValue, secondary_prop: &JsonValue) {
    let (Some(primary_obj), Some(secondary_obj)) =
        (primary_prop.as_object_mut(), secondary_prop.as_object())
    else {
        if primary_prop.is_null() && !secondary_prop.is_null() {
            *primary_prop = secondary_prop.clone();
        }
        return;
    };

    for (key, secondary_val) in secondary_obj {
        match primary_obj.get_mut(key) {
            Some(primary_val) => {
                if key == "description" {
                    let current = primary_val.as_str().unwrap_or("");
                    let incoming = secondary_val.as_str().unwrap_or("");
                    if is_weak_description(current) && !is_weak_description(incoming) {
                        *primary_val = JsonValue::String(incoming.to_string());
                    }
                    continue;
                }

                if primary_val.is_null() {
                    *primary_val = secondary_val.clone();
                } else if primary_val.is_object() && secondary_val.is_object() {
                    merge_json_schema(primary_val, secondary_val);
                }
            }
            None => {
                primary_obj.insert(key.clone(), secondary_val.clone());
            }
        }
    }
}

fn merge_required(primary_obj: &mut serde_json::Map<String, JsonValue>, secondary_req: &JsonValue) {
    let Some(secondary_arr) = secondary_req.as_array() else {
        return;
    };

    let mut merged: Vec<JsonValue> = primary_obj
        .get("required")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    for req in secondary_arr {
        if !merged.iter().any(|r| r == req) {
            merged.push(req.clone());
        }
    }

    if !merged.is_empty() {
        primary_obj.insert("required".to_string(), JsonValue::Array(merged));
    }
}

/// Deduplicate tool names, sort alphabetically.
pub fn deduplicate_names(raw: Vec<String>) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::with_capacity(raw.len());
    let mut names: Vec<String> = Vec::with_capacity(raw.len());

    for name in raw {
        if seen.insert(name.clone()) {
            names.push(name);
        }
    }

    names.sort();
    names
}
