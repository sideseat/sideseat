// The rule-language reference, held to the schema the engine is compiled from.
//
// `docs/engineering/rule-language.md` is a guide followed by a reference. The reference is generated from
// `server/assets/rules.schema.json`, so it cannot drift from the grammar; the guide is prose, so these tests ask
// it to name every section and every operator, and compile every example it shows.

const RULE_LANGUAGE_DOC: &str = "docs/engineering/rule-language.md";
const GENERATED_BEGIN: &str = "<!-- BEGIN GENERATED FROM server/assets/rules.schema.json: \
     UPDATE_DOCS=1 cargo test --locked -p sideseat-server --test repository rule_language -->";
const GENERATED_END: &str = "<!-- END GENERATED -->";

fn rule_schema() -> serde_json::Value {
    let path = repo_root().join("server/assets/rules.schema.json");
    serde_json::from_str(&std::fs::read_to_string(&path).expect("the schema is committed"))
        .expect("the schema is JSON")
}

/// A description's first paragraph on one line, safe inside a table cell.
fn cell(description: Option<&serde_json::Value>) -> String {
    let text = description
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    text.split("\n\n")
        .next()
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('|', "\\|")
}

fn anchor(name: &str) -> String {
    name.to_lowercase()
}

/// A schema node's type, as a reader would say it.
fn type_of(node: &serde_json::Value) -> String {
    use serde_json::Value;
    let Some(object) = node.as_object() else {
        return "any JSON value".to_string();
    };
    if let Some(Value::String(reference)) = object.get("$ref") {
        let name = reference.trim_start_matches("#/$defs/");
        return format!("[`{name}`](#{})", anchor(name));
    }
    if let Some(constant) = object.get("const") {
        return format!("`{constant}`");
    }
    if let Some(Value::Array(values)) = object.get("enum") {
        let values: Vec<String> = values.iter().map(|v| format!("`{v}`")).collect();
        return format!("one of {}", values.join(", "));
    }
    for key in ["oneOf", "anyOf"] {
        if let Some(Value::Array(branches)) = object.get(key) {
            let branches: Vec<String> = branches.iter().map(type_of).collect();
            return branches.join(" or ");
        }
    }
    let kinds: Vec<&str> = match object.get("type") {
        Some(Value::String(kind)) => vec![kind.as_str()],
        Some(Value::Array(kinds)) => kinds
            .iter()
            .filter_map(Value::as_str)
            .filter(|kind| *kind != "null")
            .collect(),
        _ => Vec::new(),
    };
    let rendered: Vec<String> = kinds
        .iter()
        .map(|kind| match *kind {
            "array" => format!(
                "list of {}",
                object
                    .get("items")
                    .map(type_of)
                    .unwrap_or_else(|| "values".into())
            ),
            "object" => match (object.get("properties"), object.get("additionalProperties")) {
                (Some(Value::Object(members)), _) => {
                    let required: Vec<&str> = object
                        .get("required")
                        .and_then(Value::as_array)
                        .map(|keys| keys.iter().filter_map(Value::as_str).collect())
                        .unwrap_or_default();
                    let members: Vec<String> = members
                        .iter()
                        .map(|(key, member)| {
                            let mark = if required.contains(&key.as_str()) {
                                ", required"
                            } else {
                                ""
                            };
                            format!("`{key}` ({}{mark})", type_of(member))
                        })
                        .collect();
                    format!("object with {}", members.join(", "))
                }
                (_, Some(inner @ Value::Object(_))) => format!("map of text to {}", type_of(inner)),
                _ => "object".to_string(),
            },
            "boolean" => "true or false".to_string(),
            "integer" => "integer".to_string(),
            other => other.to_string(),
        })
        .collect();
    if rendered.is_empty() {
        "any JSON value".to_string()
    } else {
        rendered.join(" or ")
    }
}

/// The reference part of the document, rendered from the schema.
fn rendered_reference(schema: &serde_json::Value) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    writeln!(out, "{GENERATED_BEGIN}\n").unwrap();
    writeln!(out, "## Reference\n").unwrap();
    writeln!(
        out,
        "Every key an asset may write, generated from the schema the engine is compiled from. Each entry's text \
         is the first paragraph of its documentation in the engine; the schema file holds the rest.\n"
    )
    .unwrap();
    writeln!(out, "### Asset sections\n").unwrap();
    writeln!(out, "| Section | Holds | What it is |").unwrap();
    writeln!(out, "| --- | --- | --- |").unwrap();
    let required: Vec<&str> = schema["required"]
        .as_array()
        .map(|keys| keys.iter().filter_map(serde_json::Value::as_str).collect())
        .unwrap_or_default();
    for (key, node) in schema["properties"]
        .as_object()
        .expect("the schema has sections")
    {
        let mark = if required.contains(&key.as_str()) {
            " (required)"
        } else {
            ""
        };
        writeln!(
            out,
            "| `{key}`{mark} | {} | {} |",
            type_of(node),
            cell(node.get("description"))
        )
        .unwrap();
    }
    writeln!(out).unwrap();
    let definitions = schema["$defs"]
        .as_object()
        .expect("the schema has definitions");
    for (name, node) in definitions {
        writeln!(out, "### `{name}`\n").unwrap();
        let description = node
            .get("description")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim();
        if !description.is_empty() {
            writeln!(out, "{description}\n").unwrap();
        }
        if let Some(members) = node
            .get("properties")
            .and_then(serde_json::Value::as_object)
        {
            let required: Vec<&str> = node
                .get("required")
                .and_then(serde_json::Value::as_array)
                .map(|keys| keys.iter().filter_map(serde_json::Value::as_str).collect())
                .unwrap_or_default();
            writeln!(out, "| Key | Type | What it is |").unwrap();
            writeln!(out, "| --- | --- | --- |").unwrap();
            for (key, member) in members {
                let mark = if required.contains(&key.as_str()) {
                    " (required)"
                } else {
                    ""
                };
                writeln!(
                    out,
                    "| `{key}`{mark} | {} | {} |",
                    type_of(member),
                    cell(member.get("description"))
                )
                .unwrap();
            }
            writeln!(out).unwrap();
            continue;
        }
        // An enumeration or a choice of forms: each variant on its own line, with its documentation.
        let branches = node
            .get("oneOf")
            .or_else(|| node.get("anyOf"))
            .and_then(serde_json::Value::as_array);
        match branches {
            Some(branches) => {
                for branch in branches {
                    let text = cell(branch.get("description"));
                    let separator = if text.is_empty() { "" } else { ": " };
                    writeln!(out, "- {}{separator}{text}", type_of(branch)).unwrap();
                }
                writeln!(out).unwrap();
            }
            None => writeln!(out, "Written as {}.\n", type_of(node)).unwrap(),
        }
    }
    writeln!(out, "{GENERATED_END}").unwrap();
    out
}

fn rule_language_doc() -> String {
    std::fs::read_to_string(repo_root().join(RULE_LANGUAGE_DOC))
        .expect("the rule-language reference exists")
}

#[test]
fn the_rule_language_reference_is_generated_from_the_current_schema() {
    let doc = rule_language_doc();
    let begin = doc
        .find(GENERATED_BEGIN)
        .expect("the reference marks where its generated part begins");
    let end = doc
        .find(GENERATED_END)
        .expect("the reference marks where its generated part ends")
        + GENERATED_END.len()
        + 1;
    let rendered = rendered_reference(&rule_schema());
    if doc[begin..end.min(doc.len())] == rendered {
        return;
    }
    if std::env::var_os("UPDATE_DOCS").is_some() {
        let updated = format!("{}{rendered}{}", &doc[..begin], &doc[end.min(doc.len())..]);
        std::fs::write(repo_root().join(RULE_LANGUAGE_DOC), updated)
            .expect("the reference is writable");
        return;
    }
    panic!(
        "{RULE_LANGUAGE_DOC} is stale against server/assets/rules.schema.json; regenerate it with \
         `UPDATE_DOCS=1 cargo test --locked -p sideseat-server --test repository rule_language`"
    );
}

/// Every key and every enumerated value the schema defines appears in the reference, so the reference covers
/// the whole grammar even if the renderer above ever skips a shape.
#[test]
fn the_rule_language_reference_covers_every_schema_keyword() {
    fn keywords(node: &serde_json::Value, out: &mut BTreeSet<String>) {
        match node {
            serde_json::Value::Object(object) => {
                if let Some(serde_json::Value::Object(members)) = object.get("properties") {
                    out.extend(members.keys().cloned());
                }
                if let Some(serde_json::Value::Array(values)) = object.get("enum") {
                    out.extend(values.iter().filter_map(|v| v.as_str().map(str::to_string)));
                }
                if let Some(serde_json::Value::String(constant)) = object.get("const") {
                    out.insert(constant.clone());
                }
                object.values().for_each(|inner| keywords(inner, out));
            }
            serde_json::Value::Array(items) => items.iter().for_each(|inner| keywords(inner, out)),
            _ => {}
        }
    }
    let mut all = BTreeSet::new();
    keywords(&rule_schema(), &mut all);
    let doc = rule_language_doc();
    let reference = &doc[doc
        .find(GENERATED_BEGIN)
        .expect("the generated part is marked")..];
    let missing: Vec<&String> = all
        .iter()
        .filter(|keyword| {
            !reference.contains(&format!("`{keyword}`"))
                && !reference.contains(&format!("`\"{keyword}\"`"))
        })
        .collect();
    assert!(
        missing.is_empty(),
        "the generated reference never names these schema keywords: {missing:?}"
    );
}

/// The guide, not the generated part, names every section and every operator a condition, a value test or a
/// pipe can use: a reader learns the language from the guide, and an operator only the generated tables list
/// has no example and no explanation of how it combines.
#[test]
fn the_rule_language_guide_explains_every_section_and_operator() {
    let schema = rule_schema();
    let doc = rule_language_doc();
    let guide = &doc[..doc
        .find(GENERATED_BEGIN)
        .expect("the generated part is marked")];
    let mut operators: BTreeSet<String> = schema["properties"]
        .as_object()
        .expect("sections")
        .keys()
        .filter(|key| !matches!(key.as_str(), "$schema" | "id" | "doc"))
        .cloned()
        .collect();
    for definition in ["SpanCondition", "ValuePredicate", "FirstPresent_string"] {
        let members = schema["$defs"][definition]["properties"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        operators.extend(
            members
                .keys()
                .filter(|key| *key != "doc" && *key != "path")
                .cloned(),
        );
    }
    // Every transform the grammar has, read from the schema so a new step is a new obligation here.
    let mut transforms = BTreeSet::new();
    fn transform_names(node: &serde_json::Value, out: &mut BTreeSet<String>) {
        if let Some(values) = node.get("enum").and_then(serde_json::Value::as_array) {
            out.extend(values.iter().filter_map(|v| v.as_str().map(str::to_string)));
        }
        if let Some(members) = node
            .get("properties")
            .and_then(serde_json::Value::as_object)
        {
            out.extend(members.keys().filter(|key| *key != "closed").cloned());
        }
        for key in ["oneOf", "anyOf"] {
            for branch in node
                .get(key)
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
            {
                transform_names(branch, out);
            }
        }
    }
    transform_names(&schema["$defs"]["Transform"], &mut transforms);
    assert!(
        transforms.len() >= 9,
        "the Transform schema names {transforms:?}"
    );
    operators.extend(transforms);
    for name in ["first_of", "all", "any", "not", "priority", "supersedes"] {
        operators.insert(name.to_string());
    }
    let missing: Vec<&String> = operators
        .iter()
        .filter(|name| !guide.contains(&format!("`{name}`")))
        .collect();
    assert!(
        missing.is_empty(),
        "the guide never explains these: {missing:?}"
    );
}

/// Every example the guide shows is a whole asset that compiles on its own, so an example cannot teach a spelling
/// the engine refuses. On its own rather than beside the embedded assets: every stored field already has its
/// rule, and an example must not have to avoid the corpus to be valid.
#[test]
fn every_rule_language_example_compiles() {
    use sideseat_domain::rules::{Ruleset, assets::ParsedAssets};
    let doc = rule_language_doc();
    let mut examples = Vec::new();
    let mut rest = doc.as_str();
    while let Some(start) = rest.find("```json example\n") {
        let body = &rest[start + "```json example\n".len()..];
        let end = body.find("\n```").expect("an example block is closed");
        examples.push(&body[..end]);
        rest = &body[end..];
    }
    assert!(
        examples.len() >= 7,
        "the guide shows {} compiled examples",
        examples.len()
    );
    assert_eq!(
        doc.matches("```json").count(),
        examples.len(),
        "every JSON block in the guide is a compiled example, so none can show an unchecked spelling"
    );
    for (index, example) in examples.iter().enumerate() {
        let sources = BTreeMap::from([(
            format!("producers/example-{index}.json"),
            example.as_bytes().to_vec(),
        )]);
        let assets = ParsedAssets::parse(&sources)
            .unwrap_or_else(|error| panic!("example {index} does not parse:\n{example}\n{error}"));
        if let Err(error) = Ruleset::build(&assets) {
            panic!("example {index} does not compile:\n{example}\n{error}");
        }
    }
}
