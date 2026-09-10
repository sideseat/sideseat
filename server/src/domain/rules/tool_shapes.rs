//! The shapes a provider writes a tool definition in, declared rather than branched on.
//!
//! The canonical form - `{"type":"function","function":{"name","description","parameters"}}` - is **ours** and
//! stays here. Every path into a producer's own shape is the asset's. Five readers used to name `openai`,
//! `anthropic`, `bedrock`, `gemini` and `cohere` in production Rust, and an unrecognised shape was passed
//! through unchanged and then discarded downstream because no name could be extracted from it - so adding a
//! provider's shape was a code change and an unknown one silently produced nothing usable.

use serde_json::{Value as JsonValue, json};

use super::schema::{ParametersEncoding, ParametersSpec, RuleFile, ToolShapeRule};

/// The declared shapes, in the order they are tried.
#[derive(Default)]
pub struct ToolShapePlan {
    rules: Vec<ToolShapeRule>,
}

/// Why a declared shape could not mean what it says.
#[derive(Debug, thiserror::Error)]
pub enum ToolShapeError {
    #[error(
        "tool shape `{id}` declares both `function` and one of `name`/`description`/`parameters` - a shape either hands over a canonical object or says where each part is"
    )]
    TwoAnswers { id: String },
    #[error(
        "tool shape `{id}` declares neither `function` nor `name` - a definition with no name is not one"
    )]
    NoName { id: String },
    #[error(
        "tool shapes `{first}` and `{second}` share rank {rank}, so which reads a payload they both recognise would depend on load order"
    )]
    SharedRank {
        first: String,
        second: String,
        rank: i32,
    },
    #[error("tool shape `{id}` declares an empty `carry` member name, which names nothing")]
    EmptyCarry { id: String },
    #[error("tool shape `{id}` declares `parameters` with no path")]
    NoParameterPath { id: String },
    #[error("tool shape `{id}`: {detail}")]
    Inexpressible { id: String, detail: &'static str },
}

impl ToolShapePlan {
    pub fn compile(files: &[RuleFile]) -> Result<Self, ToolShapeError> {
        let mut rules: Vec<ToolShapeRule> = Vec::new();
        for file in files {
            for rule in &file.tool_shapes {
                let members =
                    rule.name.is_some() || rule.description.is_some() || rule.parameters.is_some();
                if rule.function.is_some() && members {
                    return Err(ToolShapeError::TwoAnswers {
                        id: rule.id.clone(),
                    });
                }
                if rule.function.is_none() && rule.name.is_none() {
                    return Err(ToolShapeError::NoName {
                        id: rule.id.clone(),
                    });
                }
                if rule.carry.iter().any(|member| member.trim().is_empty()) {
                    return Err(ToolShapeError::EmptyCarry {
                        id: rule.id.clone(),
                    });
                }
                if rule
                    .parameters
                    .as_ref()
                    .is_some_and(|spec| spec.from.is_empty())
                {
                    return Err(ToolShapeError::NoParameterPath {
                        id: rule.id.clone(),
                    });
                }
                // The same predicate validation every other section runs: a condition that could never mean
                // what it says is refused here rather than left to be discovered, and
                // `every_predicate_set_in_the_schema_is_validated` is what makes forgetting it impossible.
                if let Some(detail) = super::message_rules::predicate_defect(&rule.require) {
                    return Err(ToolShapeError::Inexpressible {
                        id: rule.id.clone(),
                        detail,
                    });
                }
                rules.push(rule.clone());
            }
        }
        rules.sort_by_key(|rule| rule.legacy_rank);
        // A shared rank is a rule id deciding the answer, which is the refusal every other ordered section
        // makes.
        for pair in rules.windows(2) {
            if pair[0].legacy_rank == pair[1].legacy_rank {
                return Err(ToolShapeError::SharedRank {
                    first: pair[0].id.clone(),
                    second: pair[1].id.clone(),
                    rank: pair[0].legacy_rank,
                });
            }
        }
        Ok(Self { rules })
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    pub fn rules(&self) -> impl Iterator<Item = &ToolShapeRule> {
        self.rules.iter()
    }

    /// One payload as canonical definitions, or `None` where no declared shape recognises it.
    ///
    /// `None` is a real answer and the caller decides what it means: today it passes the payload through, which
    /// is what the retired chain did, so an unknown shape is preserved rather than dropped.
    pub fn canonical(&self, tool: &JsonValue) -> Option<Vec<JsonValue>> {
        for rule in &self.rules {
            if !super::message_rules::predicates_hold(tool, &rule.require) {
                continue;
            }
            let subjects: Vec<&JsonValue> = match &rule.each {
                Some(path) => super::message_rules::query(tool, path),
                None => vec![tool],
            };
            if subjects.is_empty() {
                continue;
            }
            let built: Vec<JsonValue> = subjects
                .iter()
                .filter_map(|subject| definition(rule, subject, tool))
                .collect();
            // A shape that recognised the payload and built nothing has not read it: try the next, which is
            // what the retired chain's `?` did inside each reader.
            if !built.is_empty() {
                return Some(built);
            }
        }
        None
    }
}

/// One canonical definition from one subject, or `None` where the shape's own requirements are unmet.
///
/// `enclosing` is the whole payload, which is where `carry` reads from: a producer states `strict` beside the
/// function rather than inside it.
fn definition(
    rule: &ToolShapeRule,
    subject: &JsonValue,
    enclosing: &JsonValue,
) -> Option<JsonValue> {
    let mut wrapper = serde_json::Map::new();
    wrapper.insert("type".to_string(), json!("function"));
    if let Some(path) = &rule.function {
        // A producer that already writes the canonical object: taken as it stands, because re-deriving its
        // members would drop whatever else it carries.
        let function = first(subject, std::slice::from_ref(path))?;
        wrapper.insert("function".to_string(), function.clone());
    } else {
        let name = first(subject, std::slice::from_ref(rule.name.as_ref()?))?.as_str()?;
        let mut function = serde_json::Map::new();
        function.insert("name".to_string(), json!(name));
        // Absent stays absent rather than becoming `null`: the retired readers inserted `tool.get(..)`, whose
        // `None` serialises as `null`, and a description of `null` is not the same as no description.
        if let Some(path) = &rule.description
            && let Some(found) = first(subject, std::slice::from_ref(path))
        {
            function.insert("description".to_string(), found.clone());
        }
        if let Some(spec) = &rule.parameters
            && let Some(parameters) = parameters(spec, subject)
        {
            function.insert("parameters".to_string(), parameters);
        }
        wrapper.insert("function".to_string(), JsonValue::Object(function));
    }
    for member in &rule.carry {
        if let Some(found) = enclosing.get(member.as_str()) {
            wrapper.insert(member.clone(), found.clone());
        }
    }
    Some(JsonValue::Object(wrapper))
}

/// The parameters, in the declared encoding.
fn parameters(spec: &ParametersSpec, subject: &JsonValue) -> Option<JsonValue> {
    let found = first(subject, &spec.from)?;
    match spec.encoding {
        ParametersEncoding::JsonSchema => Some(found.clone()),
        ParametersEncoding::ArgumentMap => Some(argument_map_to_json_schema(found)),
    }
}

/// The first of the declared paths that resolves.
fn first<'v>(subject: &'v JsonValue, paths: &[super::schema::JsonPath]) -> Option<&'v JsonValue> {
    paths.iter().find_map(|path| {
        super::message_rules::query(subject, path)
            .into_iter()
            .next()
    })
}

/// A map from argument name to its facts, as a JSON Schema object.
///
/// **Every supported constraint is kept.** A converter that dropped `required` said an argument was optional
/// where the producer said it was not, and one that dropped `default` and `enum` threw away what the model is
/// allowed to send. `required` belongs at the schema level, which is where JSON Schema puts it.
///
/// Generic: the shape is "name to facts", which several producers write and which no one of them owns.
pub fn argument_map_to_json_schema(map: &JsonValue) -> JsonValue {
    let Some(members) = map.as_object() else {
        return json!({"type": "object", "properties": {}});
    };
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for (name, facts) in members {
        let mut property = serde_json::Map::new();
        // Copied by name, so a constraint this code has never heard of survives - the alternative is a schema
        // that silently permits what the producer forbade.
        for constraint in ["type", "description", "default", "enum", "format", "items"] {
            if let Some(found) = facts.get(constraint) {
                property.insert(constraint.to_string(), found.clone());
            }
        }
        properties.insert(name.clone(), JsonValue::Object(property));
        if facts
            .get("required")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false)
        {
            required.push(json!(name));
        }
    }
    json!({"type": "object", "properties": properties, "required": required})
}
