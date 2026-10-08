//! The editor schema for rule assets, and what the shipped assets use of it.
//!
//! `server/assets/rules.schema.json` is generated from the serde types, so it says exactly what the engine
//! parses. It lives beside the embedded directory rather than in it, because every embedded `*.json` is a
//! `RuleFile`.
//!
//! The same walk that validates an asset against the schema records which schema options it exercised, which
//! makes dead grammar mechanical to find: an optional property or enum value no shipped asset uses must be
//! listed in [`UNUSED`] with a reason, and an entry that becomes used must leave the list.
//!
//! The validator is small and test-only rather than a crate: it covers the keywords the generator emits and
//! refuses any other assertion keyword, so a change in generator output cannot silently go unchecked.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use super::schema::{self, RuleFile};

/// Where the generated schema is committed, relative to this crate.
const SCHEMA_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/rules.schema.json"
);

/// The `$schema` every embedded asset declares: assets sit one directory below `assets/rules/`.
const ASSET_SCHEMA_REF: &str = "../../rules.schema.json";

/// Schema options no embedded asset uses, each with the reason it stays.
///
/// Keyed `Definition.property` for an optional property and `Definition=value` for an enum value. An entry
/// here is a promise that the option is deliberate; the census refuses an unused option missing from this list
/// and an entry that has become used, so the list cannot rot in either direction. `doc` members are not
/// census options: documentation is metadata, and exercising it shows nothing about the grammar.
const UNUSED: &[(&str, &str)] = &[
    (
        "FirstPresent_string.mode",
        "`present` is what an omitted mode means on a first-present list; the member exists so a list can say so \
         beside a `usable` one, and spelling the default would only restate it",
    ),
    (
        "LengthUnit=bytes",
        "the closed set of units a producer may count a stated length in; the units are a vocabulary, and a set \
         missing one would be an arbitrary hole",
    ),
    ("LengthUnit=chars", "as `bytes`"),
    (
        "VersionRange.below",
        "a range is half-open at both ends by grammar; the one shipped range is open above, and a range that \
         closes needs no new spelling",
    ),
    (
        "VersionScheme=semver",
        "the scheme for every package that is not Python's; the one shipped version range reads a Python \
         package, and the closed set of schemes is a vocabulary",
    ),
    (
        "Facts.carrier_is_atomic_emission",
        "every fact axis of a carrier preset stays independently overridable, so a clause can state an \
         exception without a new preset; no shipped clause needs this one yet",
    ),
    (
        "Facts.may_contain_framework_state",
        "every fact axis stays independently overridable, as `carrier_is_atomic_emission`",
    ),
    (
        "Facts.may_restate_prior_observations",
        "every fact axis stays independently overridable, as `carrier_is_atomic_emission`",
    ),
    (
        "ValueKind=null",
        "part of the closed JSON kind vocabulary a `kind` predicate offers; a kind set missing one would be an \
         arbitrary hole",
    ),
    (
        "ValueKind=number",
        "part of the closed JSON kind vocabulary, as `null`",
    ),
];

/// Enum values no asset spells because they are the default, which an asset states by omission.
///
/// Separate from [`UNUSED`] because the reason is structural and checkable: the test below proves each value
/// is what omission means, so spelling it would only restate the default.
const IMPLICIT_DEFAULTS: &[&str] = &[
    "FieldCombine=first_wins",
    "MalformedPolicy=stop",
    "MediaSource=decoded",
    "MissingMediaType=decline",
    "ResultContent=normalized",
    "MemberPresence=exact",
    "MessageStage=dialect",
    "RawEventForm=message",
];

/// Schema objects that do not accept `doc`, each with the reason.
///
/// They derive equality that overlap, shadowing and arena checks compare on, so a `doc` would make two
/// otherwise identical declarations unequal and slip past those refusals. Each is retired by the unified
/// predicate and source grammar, which gives `where` and sources `doc` from the start.
const DOC_EXEMPT: &[&str] = &["MatchSpec", "SpanSource", "EventSource"];

fn generated() -> Value {
    let generator = schemars::generate::SchemaSettings::draft2020_12().into_generator();
    serde_json::to_value(generator.into_root_schema_for::<RuleFile>())
        .expect("a generated schema serialises")
}

fn rendered(schema: &Value) -> String {
    let mut text = serde_json::to_string_pretty(schema).expect("a schema serialises");
    text.push('\n');
    text
}

#[test]
fn the_committed_schema_is_current() {
    let want = rendered(&generated());
    if std::env::var_os("UPDATE_GOLDENS").is_some() {
        std::fs::write(SCHEMA_PATH, &want).expect("the schema file is writable");
        return;
    }
    let have = std::fs::read_to_string(SCHEMA_PATH).unwrap_or_default();
    assert!(
        have == want,
        "server/assets/rules.schema.json is stale; regenerate it with \
         `UPDATE_GOLDENS=1 cargo test --locked -q -p sideseat-domain --lib schema_census`"
    );
}

#[test]
fn every_asset_names_the_schema() {
    for (path, bytes) in schema::embedded_sources() {
        let file: RuleFile = serde_json::from_slice(&bytes).expect("the asset parses");
        assert_eq!(
            file.schema.as_deref(),
            Some(ASSET_SCHEMA_REF),
            "{path} must declare `\"$schema\": \"{ASSET_SCHEMA_REF}\"` so editors validate it"
        );
    }
}

/// What the walk saw: every property set on a definition, and every enum value matched.
#[derive(Default, Clone)]
struct Coverage {
    used: BTreeSet<String>,
    /// Where in an asset a string matched an enum or a constant: a word of the grammar's, at that position.
    grammar_at: BTreeSet<String>,
}

struct Validator<'s> {
    defs: &'s Map<String, Value>,
}

/// Keywords that annotate and assert nothing.
const ANNOTATIONS: &[&str] = &[
    "$schema",
    "title",
    "description",
    "default",
    "examples",
    "format",
];

impl<'s> Validator<'s> {
    /// Validate `value` against `node`, recording coverage under `owner` (the definition being walked).
    fn check(
        &self,
        node: &'s Value,
        owner: Option<&'s str>,
        value: &Value,
        at: &str,
        seen: &mut Coverage,
    ) -> Result<(), String> {
        let node = match node {
            Value::Bool(true) => return Ok(()),
            Value::Bool(false) => return Err(format!("{at}: nothing is allowed here")),
            Value::Object(node) => node,
            other => return Err(format!("{at}: unsupported schema node {other}")),
        };
        for key in node.keys() {
            let known = matches!(
                key.as_str(),
                "$ref"
                    | "$defs"
                    | "type"
                    | "properties"
                    | "required"
                    | "additionalProperties"
                    | "enum"
                    | "const"
                    | "oneOf"
                    | "anyOf"
                    | "items"
                    | "prefixItems"
                    | "minItems"
                    | "maxItems"
                    | "minimum"
                    | "maximum"
            ) || ANNOTATIONS.contains(&key.as_str());
            if !known {
                return Err(format!(
                    "{at}: schema keyword `{key}` is not understood by this validator - extend it rather than \
                     letting the keyword go unchecked"
                ));
            }
        }
        if let Some(reference) = node.get("$ref").and_then(Value::as_str) {
            let name = reference
                .strip_prefix("#/$defs/")
                .ok_or_else(|| format!("{at}: unsupported reference {reference}"))?;
            let target = self
                .defs
                .get_key_value(name)
                .ok_or_else(|| format!("{at}: dangling reference {reference}"))?;
            self.check(target.1, Some(target.0.as_str()), value, at, seen)?;
        }
        if let Some(types) = node.get("type") {
            let allowed: Vec<&str> = match types {
                Value::String(one) => vec![one.as_str()],
                Value::Array(many) => many.iter().filter_map(Value::as_str).collect(),
                _ => return Err(format!("{at}: malformed `type`")),
            };
            let actual = match value {
                Value::Null => "null",
                Value::Bool(_) => "boolean",
                Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
                Value::Number(_) => "number",
                Value::String(_) => "string",
                Value::Array(_) => "array",
                Value::Object(_) => "object",
            };
            let fits =
                allowed.contains(&actual) || (actual == "integer" && allowed.contains(&"number"));
            if !fits {
                return Err(format!("{at}: expected {allowed:?}, found {actual}"));
            }
        }
        for (bound, holds) in [
            ("minimum", (|v, b| v >= b) as fn(f64, f64) -> bool),
            ("maximum", |v, b| v <= b),
        ] {
            if let (Some(b), Some(v)) = (node.get(bound).and_then(Value::as_f64), value.as_f64())
                && !holds(v, b)
            {
                return Err(format!("{at}: {v} violates `{bound}` {b}"));
            }
        }
        if let Some(options) = node.get("enum").and_then(Value::as_array) {
            if !options.contains(value) {
                return Err(format!("{at}: {value} is not one of {options:?}"));
            }
            if let (Some(owner), Some(text)) = (owner, value.as_str()) {
                seen.used.insert(format!("{owner}={text}"));
            }
            if value.is_string() {
                seen.grammar_at.insert(at.to_string());
            }
        }
        if let Some(expected) = node.get("const") {
            if expected != value {
                return Err(format!("{at}: expected {expected}, found {value}"));
            }
            if let (Some(owner), Some(text)) = (owner, value.as_str()) {
                seen.used.insert(format!("{owner}={text}"));
            }
            if value.is_string() {
                seen.grammar_at.insert(at.to_string());
            }
        }
        if let Some(branches) = node.get("oneOf").and_then(Value::as_array) {
            let mut matched: Vec<Coverage> = Vec::new();
            let mut reasons = Vec::new();
            for branch in branches {
                let mut trial = seen.clone();
                match self.check(branch, owner, value, at, &mut trial) {
                    Ok(()) => matched.push(trial),
                    Err(reason) => reasons.push(reason),
                }
            }
            match matched.len() {
                1 => *seen = matched.remove(0),
                0 => {
                    return Err(format!(
                        "{at}: no `oneOf` branch matches: {}",
                        reasons.join(" | ")
                    ));
                }
                n => {
                    return Err(format!(
                        "{at}: {n} `oneOf` branches match, exactly one must"
                    ));
                }
            }
        }
        if let Some(branches) = node.get("anyOf").and_then(Value::as_array) {
            let mut any = false;
            let mut reasons = Vec::new();
            for branch in branches {
                let mut trial = seen.clone();
                match self.check(branch, owner, value, at, &mut trial) {
                    Ok(()) => {
                        any = true;
                        seen.used.extend(trial.used);
                        seen.grammar_at.extend(trial.grammar_at);
                    }
                    Err(reason) => reasons.push(reason),
                }
            }
            if !any {
                return Err(format!(
                    "{at}: no `anyOf` branch matches: {}",
                    reasons.join(" | ")
                ));
            }
        }
        if let Value::Array(items) = value {
            let prefix = node.get("prefixItems").and_then(Value::as_array);
            for (index, item) in items.iter().enumerate() {
                let item_at = format!("{at}[{index}]");
                match prefix.and_then(|prefix| prefix.get(index)) {
                    Some(schema) => self.check(schema, None, item, &item_at, seen)?,
                    None => {
                        if let Some(schema) = node.get("items") {
                            self.check(schema, None, item, &item_at, seen)?;
                        }
                    }
                }
            }
            let len = items.len() as u64;
            if node
                .get("minItems")
                .and_then(Value::as_u64)
                .is_some_and(|min| len < min)
            {
                return Err(format!("{at}: too few items"));
            }
            if node
                .get("maxItems")
                .and_then(Value::as_u64)
                .is_some_and(|max| len > max)
            {
                return Err(format!("{at}: too many items"));
            }
        }
        if let Value::Object(members) = value {
            let properties = node.get("properties").and_then(Value::as_object);
            if let Some(required) = node.get("required").and_then(Value::as_array) {
                for name in required.iter().filter_map(Value::as_str) {
                    if !members.contains_key(name) {
                        return Err(format!("{at}: missing required `{name}`"));
                    }
                }
            }
            for (name, member) in members {
                let member_at = format!("{at}.{name}");
                match properties.and_then(|properties| properties.get(name)) {
                    Some(schema) => {
                        if let Some(owner) = owner {
                            seen.used.insert(format!("{owner}.{name}"));
                        }
                        self.check(schema, None, member, &member_at, seen)?;
                    }
                    None => match node.get("additionalProperties") {
                        Some(Value::Bool(false)) => {
                            return Err(format!("{at}: unknown member `{name}`"));
                        }
                        Some(schema) => self.check(schema, None, member, &member_at, seen)?,
                        None if properties.is_some() => {}
                        None => {}
                    },
                }
            }
        }
        Ok(())
    }
}

/// Every option the schema offers that an asset could leave unused: optional properties and enum values.
fn options(schema: &Value) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let root = schema.as_object().expect("the root is an object");
    let mut named: Vec<(&str, &Map<String, Value>)> = vec![("RuleFile", root)];
    if let Some(defs) = root.get("$defs").and_then(Value::as_object) {
        named.extend(
            defs.iter()
                .filter_map(|(name, def)| def.as_object().map(|def| (name.as_str(), def))),
        );
    }
    fn walk(owner: &str, node: &Map<String, Value>, out: &mut BTreeSet<String>) {
        let required: BTreeSet<&str> = node
            .get("required")
            .and_then(Value::as_array)
            .map(|names| names.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if let Some(properties) = node.get("properties").and_then(Value::as_object) {
            for name in properties.keys() {
                if name != "doc" && !required.contains(name.as_str()) {
                    out.insert(format!("{owner}.{name}"));
                }
            }
        }
        for value in node
            .get("enum")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .chain(node.get("const"))
        {
            if let Some(text) = value.as_str() {
                out.insert(format!("{owner}={text}"));
            }
        }
        for branches in ["oneOf", "anyOf"] {
            for branch in node
                .get(branches)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_object)
            {
                walk(owner, branch, out);
            }
        }
    }
    for (owner, node) in named {
        walk(owner, node, &mut out);
    }
    out
}

/// Every position in the embedded assets where a string is one of the grammar's own enum values or constants,
/// as `<asset path>.<member>[<index>]...` - the form `Validator::check` reports a position in.
///
/// For the producer-vocabulary sweep, which must tell a grammar word from the same word written as data: the
/// schema knows which positions are enumerations, and a list of keys would not.
pub(super) fn grammar_value_positions() -> BTreeSet<String> {
    let schema = generated();
    let defs = schema
        .get("$defs")
        .and_then(Value::as_object)
        .expect("the schema has definitions");
    let validator = Validator { defs };
    let mut seen = Coverage::default();
    for (path, bytes) in schema::embedded_sources() {
        let value: Value = serde_json::from_slice(&bytes).expect("the asset is JSON");
        validator
            .check(&schema, Some("RuleFile"), &value, &path, &mut seen)
            .unwrap_or_else(|reason| panic!("{path} does not validate: {reason}"));
    }
    seen.grammar_at
}

/// Validate every embedded asset, returning the options they used.
fn embedded_coverage(schema: &Value) -> BTreeSet<String> {
    let defs = schema
        .get("$defs")
        .and_then(Value::as_object)
        .expect("the schema has definitions");
    let validator = Validator { defs };
    let mut seen = Coverage::default();
    let mut failures = Vec::new();
    for (path, bytes) in schema::embedded_sources() {
        let value: Value = serde_json::from_slice(&bytes).expect("the asset is JSON");
        if let Err(reason) = validator.check(schema, Some("RuleFile"), &value, &path, &mut seen) {
            failures.push(reason);
        }
    }
    assert!(
        failures.is_empty(),
        "assets the generated schema refuses, though serde accepts them - the schema and the parser disagree:\n{}",
        failures.join("\n")
    );
    seen.used
}

#[test]
fn every_asset_satisfies_the_generated_schema() {
    embedded_coverage(&generated());
}

#[test]
fn the_validator_refuses_what_the_parser_refuses() {
    let schema = generated();
    let defs = schema
        .get("$defs")
        .and_then(Value::as_object)
        .expect("definitions");
    let validator = Validator { defs };
    for probe in [
        serde_json::json!({"id": "t", "carriers": [{"id": "c", "match": {"attribute": "a"}, "facts": {"preset": "emission"}, "typo": 1}]}),
        serde_json::json!({"carriers": []}),
        serde_json::json!({"id": "t", "messages": "not a list"}),
    ] {
        assert!(
            serde_json::from_value::<RuleFile>(probe.clone()).is_err(),
            "the probe must be one serde refuses: {probe}"
        );
        assert!(
            validator
                .check(
                    &schema,
                    Some("RuleFile"),
                    &probe,
                    "probe",
                    &mut Coverage::default()
                )
                .is_err(),
            "the schema must refuse what serde refuses: {probe}"
        );
    }
}

#[test]
fn every_unused_schema_option_is_declared_with_a_reason() {
    let schema = generated();
    let offered = options(&schema);
    let used = embedded_coverage(&schema);
    let declared: BTreeSet<&str> = UNUSED
        .iter()
        .map(|(key, _)| *key)
        .chain(IMPLICIT_DEFAULTS.iter().copied())
        .collect();
    let undeclared: Vec<&String> = offered
        .iter()
        .filter(|option| !used.contains(*option) && !declared.contains(option.as_str()))
        .collect();
    let stale: Vec<&&str> = declared
        .iter()
        .filter(|key| used.contains(**key) || !offered.contains(**key))
        .collect();
    assert!(
        undeclared.is_empty() && stale.is_empty(),
        "schema options no asset uses must be listed in `UNUSED` with a reason, and entries that are used or \
         no longer offered must leave it.\nunused and undeclared:\n  {}\nstale entries:\n  {}",
        undeclared
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n  "),
        stale.iter().map(|s| **s).collect::<Vec<_>>().join("\n  ")
    );
    for (key, reason) in UNUSED {
        assert!(!reason.trim().is_empty(), "`{key}` needs a reason");
    }
}

#[test]
fn every_implicit_default_is_what_omission_means() {
    use super::schema::{
        FieldCombine, MalformedPolicy, MediaSource, MemberPresence, MessageStage, MissingMediaType,
        RawEventForm, ResultContent,
    };
    fn omitted<T: serde::de::DeserializeOwned + Default + PartialEq>(spelled: &str) -> bool {
        serde_json::from_value::<T>(Value::String(spelled.to_string())).ok() == Some(T::default())
    }
    for key in IMPLICIT_DEFAULTS {
        let (definition, value) = key.split_once('=').expect("`Definition=value`");
        let holds = match definition {
            "FieldCombine" => omitted::<FieldCombine>(value),
            "MalformedPolicy" => omitted::<MalformedPolicy>(value),
            "MediaSource" => omitted::<MediaSource>(value),
            "MissingMediaType" => omitted::<MissingMediaType>(value),
            "ResultContent" => omitted::<ResultContent>(value),
            "MemberPresence" => omitted::<MemberPresence>(value),
            "MessageStage" => omitted::<MessageStage>(value),
            "RawEventForm" => omitted::<RawEventForm>(value),
            other => panic!("`{other}` has no default check here; add one"),
        };
        assert!(
            holds,
            "`{key}` is listed as an implicit default and is not the default"
        );
    }
}

#[test]
fn every_schema_object_accepts_doc() {
    let schema = generated();
    let root_has_doc = schema.pointer("/properties/doc").is_some();
    let mut missing: Vec<&str> = Vec::new();
    let defs = schema
        .get("$defs")
        .and_then(Value::as_object)
        .expect("definitions");
    for (name, def) in defs {
        let Some(properties) = def.get("properties").and_then(Value::as_object) else {
            continue;
        };
        let required = def
            .get("required")
            .and_then(Value::as_array)
            .is_some_and(|names| names.iter().any(|n| n == "doc"));
        if (!properties.contains_key("doc") || required) && !DOC_EXEMPT.contains(&name.as_str()) {
            missing.push(name);
        }
    }
    let stale: Vec<&&str> = DOC_EXEMPT
        .iter()
        .filter(|name| {
            defs.get(**name)
                .and_then(|def| def.pointer("/properties/doc"))
                .is_some()
        })
        .collect();
    assert!(root_has_doc, "`RuleFile` must accept `doc`");
    assert!(
        missing.is_empty() && stale.is_empty(),
        "every schema object accepts an optional `doc`, or is in `DOC_EXEMPT` with the reason.\nmissing: \
         {missing:?}\nexempt but accepting it: {stale:?}"
    );
}
