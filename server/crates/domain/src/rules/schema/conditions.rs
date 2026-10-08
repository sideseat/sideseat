use super::*;

/// A condition about the span a rule is asked of: `where`, in every section that asks one.
///
/// One grammar, the boolean expression of `rules::expr`: an atom, `{"all": [...]}`, `{"any": [...]}` or
/// `{"not": ...}`, strong-Kleene, holding only when it is true. Each section states which sources its
/// conditions may read; a source a section is not given is refused when the rule compiles.
pub type SpanWhere = crate::rules::expr::Expr<SpanCondition>;

/// One question about one source of the span: a selector and the tests asked of the value it selects.
///
/// Every test is asked of the **same** value, so several tests are their conjunction. `exists` asks about the
/// selection rather than the value and is total; every other test is unknown where the source has no value,
/// which is why `not` over a value test does not hold for an absent attribute - write `"exists": true` beside
/// the test to make it false instead.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SpanCondition {
    #[serde(default)]
    pub doc: Option<String>,
    /// Where the value is: `span_name`, `attr:<key>`, `attr_keys` (the set of the span's attribute keys, asked
    /// existentially), `scope.name` (the instrumentation scope), `scope.version` (its version, for `version` only)
    /// or `resource:<key>`. Several sources are
    /// searched together only by `contains_ignore_case`: a list asks every source that has a value, and
    /// `{"first_of": [...]}` only the first that has one.
    pub source: ConditionSource,
    /// The source has a value. Total: true or false, never unknown.
    #[serde(default)]
    pub exists: Option<bool>,
    /// The value is exactly this text.
    #[serde(default)]
    pub equals: Option<String>,
    /// The value is this text, ignoring ASCII case.
    #[serde(default)]
    pub equals_ignore_case: Option<String>,
    /// The value is one of these texts.
    #[serde(default)]
    pub one_of: Vec<String>,
    /// The value begins with this text. For `attr_keys`, some key does; for `scope.name`, unknown where the span
    /// reports no scope, so a `not` over it holds only for a scope that is there.
    #[serde(default)]
    pub starts_with: Option<String>,
    /// The value contains this text.
    #[serde(default)]
    pub contains: Option<String>,
    /// The value contains this text, ignoring case (Unicode lower-casing).
    #[serde(default)]
    pub contains_ignore_case: Option<String>,
    /// The attribute's text parses in this encoding: `json`. False where it is present and does not - text cut
    /// short by an attribute length limit, say - and unknown where it is absent, so `not` over it holds only for
    /// a value that is there and does not parse.
    #[serde(default)]
    pub parses: Option<Encoding>,
    /// The value is a release inside this half-open range, ordered by the package's scheme. Asked of
    /// `scope.version` only, and alone in its atom; a value that is absent or not a version is unknown, never
    /// "the latest". The last resort of the language: a shape test says what changed, a version only when.
    #[serde(default)]
    pub version: Option<VersionRange>,
}

/// An encoding an attribute's text may be written in.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    /// Any JSON value, as `serde_json` reads one.
    Json,
}

/// A half-open range of releases, `at_least <= v < below`, in one version scheme.
///
/// An ordered interval and nothing else: no requirement-matcher policy (PEP 440's `<7` specifier and npm ranges
/// exclude pre-releases of the bound; this does not - `7.0rc1` is below `7`). SemVer bounds may leave minor and
/// patch out; values may not.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct VersionRange {
    #[serde(default)]
    pub doc: Option<String>,
    /// How the package numbers its releases.
    pub scheme: crate::rules::versions::VersionScheme,
    /// The first release inside the range.
    #[serde(default)]
    pub at_least: Option<String>,
    /// The first release after it.
    #[serde(default)]
    pub below: Option<String>,
    /// Why no shape test can say this: required, because a version gate stands for a change the payload does
    /// not show, and a reader has to be able to check that it still does not.
    pub because: String,
}

/// The source or sources a condition reads.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum ConditionSource {
    /// One source.
    One(SourceName),
    /// Every one of these that has a value.
    AnyOf(Vec<SourceName>),
    /// Only the first of these that has a value.
    FirstOf(FirstOfSources),
}

/// Only the first of several sources that has a value.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FirstOfSources {
    #[serde(default)]
    pub doc: Option<String>,
    pub first_of: Vec<SourceName>,
}

/// A source's name: one of the fixed sources, or a typed prefix and a key (`attr:<key>`, `resource:<key>`).
///
/// A string rather than an enum because the key is the producer's; the fixed names and the prefixes are the
/// grammar's, which is what the editor schema says.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(transparent)]
pub struct SourceName(pub String);

#[cfg(test)]
impl schemars::JsonSchema for SourceName {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "SourceName".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "`span_name`, `attr_keys`, `scope.name`, `scope.version`, `attr:<key>` or `resource:<key>`.",
            "anyOf": [
                {"enum": ["span_name", "attr_keys", "scope.name", "scope.version"]},
                {"type": "string"}
            ]
        })
    }
}

/// One source, or `{"first_of": [...]}` - several, of which the first is read.
///
/// The one spelling of "read this, else that" wherever a value comes from. What "the first" means is a property
/// of the field, and a list says which it is: **present** (the default) takes the first candidate that is there
/// and commits to it, whatever it turns out to hold - a badly written primary does not hand over to an alias,
/// because two spellings in one payload are one producer's statement; **usable** (`"mode": "usable"`, required
/// where a field reads that way) steps over a candidate that is there but cannot be read as the field needs. A
/// lone source has no alternative, so the two coincide and it is written bare. A list names at least two.
#[derive(Clone, PartialEq, Eq)]
pub struct FirstOf<T, const USABLE: bool> {
    items: Vec<T>,
}

impl<T, const USABLE: bool> FirstOf<T, USABLE> {
    /// The candidates, in the order they are tried.
    pub fn candidates(&self) -> &[T] {
        &self.items
    }

    /// Whether this was written as one source.
    pub fn is_single(&self) -> bool {
        self.items.len() == 1
    }

    /// The lone source, where it is one.
    pub fn single(&self) -> Option<&T> {
        match self.items.as_slice() {
            [one] => Some(one),
            _ => None,
        }
    }

    /// Built from candidates, for a caller that holds them already.
    pub fn of(items: Vec<T>) -> Self {
        Self { items }
    }
}

impl<T, const USABLE: bool> Default for FirstOf<T, USABLE> {
    fn default() -> Self {
        Self { items: Vec::new() }
    }
}

impl<T, const USABLE: bool> std::ops::Deref for FirstOf<T, USABLE> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.items
    }
}

impl<'a, T, const USABLE: bool> IntoIterator for &'a FirstOf<T, USABLE> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

impl<T: std::fmt::Debug, const USABLE: bool> std::fmt::Debug for FirstOf<T, USABLE> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.items.fmt(f)
    }
}

impl<'de, T: serde::de::DeserializeOwned, const USABLE: bool> Deserialize<'de>
    for FirstOf<T, USABLE>
{
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let value = JsonValue::deserialize(deserializer)?;
        let Some(members) = value.as_object().filter(|m| m.contains_key("first_of")) else {
            return Ok(Self {
                items: vec![serde_json::from_value(value).map_err(D::Error::custom)?],
            });
        };
        for key in members.keys() {
            if !matches!(key.as_str(), "first_of" | "mode" | "doc") {
                return Err(D::Error::custom(format!(
                    "unknown field `{key}` beside `first_of`, expected `mode` or `doc`"
                )));
            }
        }
        if members.get("doc").is_some_and(|doc| !doc.is_string()) {
            return Err(D::Error::custom("`doc` is prose: a string"));
        }
        // A mode that is not a string is not an omitted one: it names neither reading.
        let mode = match members.get("mode") {
            None => None,
            Some(JsonValue::String(mode)) => Some(mode.as_str()),
            Some(_) => {
                return Err(D::Error::custom(
                    "`mode` is `present` or `usable`, as a string",
                ));
            }
        };
        match (USABLE, mode) {
            (false, None | Some("present")) | (true, Some("usable")) => {}
            (true, _) => {
                return Err(D::Error::custom(
                    "this field reads the first **usable** candidate, stepping over one it cannot read, so \
                     its list says `\"mode\": \"usable\"`",
                ));
            }
            (false, _) => {
                return Err(D::Error::custom(
                    "this field commits to the first **present** candidate, so its list's `mode` is \
                     `present` or omitted",
                ));
            }
        }
        let items: Vec<T> =
            serde_json::from_value(members["first_of"].clone()).map_err(D::Error::custom)?;
        if items.len() < 2 {
            return Err(D::Error::custom(
                "`first_of` names at least two candidates; write a lone source bare",
            ));
        }
        Ok(Self { items })
    }
}

#[cfg(test)]
impl<T: schemars::JsonSchema, const USABLE: bool> schemars::JsonSchema for FirstOf<T, USABLE> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        format!(
            "{}{}",
            if USABLE {
                "FirstUsable_"
            } else {
                "FirstPresent_"
            },
            T::schema_name()
        )
        .into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let one =
            serde_json::to_value(generator.subschema_for::<T>()).expect("a schema serialises");
        let mode = if USABLE {
            serde_json::json!({"const": "usable"})
        } else {
            serde_json::json!({"const": "present"})
        };
        let mut list = serde_json::json!({
            "type": "object",
            "properties": {
                "first_of": {"type": "array", "items": one.clone(), "minItems": 2},
                "mode": mode,
                "doc": {"type": "string"},
            },
            "required": ["first_of"],
            "additionalProperties": false,
        });
        if USABLE {
            list["required"] = serde_json::json!(["first_of", "mode"]);
        }
        schemars::Schema::try_from(serde_json::json!({"oneOf": [one, list]}))
            .expect("an object is a schema")
    }
}

#[cfg(test)]
mod tests {
    use super::FirstOf;

    type Present = FirstOf<String, false>;
    type Usable = FirstOf<String, true>;

    fn parse<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> Result<T, String> {
        serde_json::from_value(value).map_err(|e| e.to_string())
    }

    #[test]
    fn one_source_is_written_bare_and_several_as_first_of() {
        let one: Present = parse(serde_json::json!("a")).expect("bare");
        assert_eq!(one.candidates(), ["a"]);
        assert!(one.is_single());
        let two: Present = parse(serde_json::json!({"first_of": ["a", "b"]})).expect("a list");
        assert_eq!(two.candidates(), ["a", "b"]);
        assert!(two.single().is_none());
        // The retired spelling, a bare array, is refused rather than read as either.
        assert!(parse::<Present>(serde_json::json!(["a", "b"])).is_err());
        // A list of one is a bare source written the long way, and of none names nothing.
        assert!(parse::<Present>(serde_json::json!({"first_of": ["a"]})).is_err());
        assert!(parse::<Present>(serde_json::json!({"first_of": []})).is_err());
        assert!(parse::<Present>(serde_json::json!({"first_of": ["a", "b"], "typo": 1})).is_err());
    }

    #[test]
    fn a_list_states_the_mode_its_field_reads_in() {
        assert!(
            parse::<Present>(serde_json::json!({"first_of": ["a", "b"], "mode": "present"}))
                .is_ok()
        );
        assert!(
            parse::<Present>(serde_json::json!({"first_of": ["a", "b"], "mode": "usable"}))
                .is_err()
        );
        assert!(
            parse::<Usable>(serde_json::json!({"first_of": ["a", "b"], "mode": "usable"})).is_ok()
        );
        assert!(
            parse::<Usable>(serde_json::json!({"first_of": ["a", "b"]})).is_err(),
            "a usable list says so, so `first_of` alone always means first present"
        );
        // A lone source has no alternative, so the two modes coincide and it needs none.
        assert!(parse::<Usable>(serde_json::json!("a")).is_ok());
    }

    /// The parser refuses exactly what the published schema does: a mode that is not a string names neither
    /// reading, so it is not an omitted one, and a `doc` is prose, never `null`.
    #[test]
    fn a_list_s_mode_and_doc_are_strings() {
        for value in [
            serde_json::json!({"first_of": ["a", "b"], "mode": 42}),
            serde_json::json!({"first_of": ["a", "b"], "mode": null}),
            serde_json::json!({"first_of": ["a", "b"], "doc": null}),
        ] {
            assert!(parse::<Present>(value.clone()).is_err(), "{value}");
        }
        assert!(parse::<Usable>(serde_json::json!({"first_of": ["a", "b"], "mode": 42})).is_err());
        assert!(
            parse::<Present>(serde_json::json!({"first_of": ["a", "b"], "doc": "why"})).is_ok()
        );
    }
}

/// One step of a `pipe`: a bounded operation on the value a source read.
///
/// One vocabulary for every field that transforms what it reads. A field states which steps it runs and in
/// what order - its reading has stages (decode, select, convert) and a step sits where that field applies it -
/// and refuses any other sequence, so a pipe never says something its field does not do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transform {
    /// Text folded to lower case.
    Lowercase,
    /// Text with surrounding whitespace removed.
    Trim,
    /// A leading `[tag]` line removed, and the rest trimmed.
    StripBracketTag,
    /// Blank text is treated as absent.
    BlankIsAbsent,
    /// Text with this prefix removed; text without it is absent.
    StripPrefix(String),
    /// Text looked up in a table: `closed` makes an unlisted value absent, open leaves it as it is.
    Map {
        table: BTreeMap<String, JsonValue>,
        closed: bool,
    },
    /// Every selected string, joined with this separator.
    Join(String),
    /// Text decoded the way a carrier's text is.
    Parse(ParseMode),
    /// Text with this put in front.
    Prepend(String),
}

impl<'de> Deserialize<'de> for Transform {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let value = JsonValue::deserialize(deserializer)?;
        if let Some(name) = value.as_str() {
            return Ok(match name {
                "lowercase" => Self::Lowercase,
                "trim" => Self::Trim,
                "strip_bracket_tag" => Self::StripBracketTag,
                "blank_is_absent" => Self::BlankIsAbsent,
                other => {
                    return Err(D::Error::custom(format!(
                        "`{other}` is not a transform: expected `lowercase`, `trim`, `strip_bracket_tag`, \
                         `blank_is_absent`, or an object `strip_prefix`, `map`, `join`, `parse`, `prepend`"
                    )));
                }
            });
        }
        let Some(members) = value.as_object() else {
            return Err(D::Error::custom(
                "a transform is a name or a one-operation object",
            ));
        };
        let string = |key: &str| -> Result<String, D::Error> {
            members
                .get(key)
                .and_then(JsonValue::as_str)
                .map(str::to_string)
                .ok_or_else(|| D::Error::custom(format!("`{key}` takes a string")))
        };
        let allowed = |keys: &[&str]| -> Result<(), D::Error> {
            match members.keys().find(|key| !keys.contains(&key.as_str())) {
                Some(key) => Err(D::Error::custom(format!(
                    "unknown field `{key}` in a transform"
                ))),
                None => Ok(()),
            }
        };
        if members.contains_key("map") {
            allowed(&["map", "closed"])?;
            let table = serde_json::from_value(members["map"].clone()).map_err(D::Error::custom)?;
            let closed = match members.get("closed") {
                None => false,
                Some(JsonValue::Bool(closed)) => *closed,
                Some(_) => return Err(D::Error::custom("`closed` is true or false")),
            };
            return Ok(Self::Map { table, closed });
        }
        let op = match members.keys().next().map(String::as_str) {
            Some(only) if members.len() == 1 => only,
            _ => {
                return Err(D::Error::custom(
                    "a transform object names exactly one operation",
                ));
            }
        };
        Ok(match op {
            "strip_prefix" => Self::StripPrefix(string("strip_prefix")?),
            "join" => Self::Join(string("join")?),
            "prepend" => Self::Prepend(string("prepend")?),
            "parse" => Self::Parse(
                serde_json::from_value(members["parse"].clone()).map_err(D::Error::custom)?,
            ),
            other => return Err(D::Error::custom(format!("unknown transform `{other}`"))),
        })
    }
}

#[cfg(test)]
impl schemars::JsonSchema for Transform {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Transform".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let parse = serde_json::to_value(generator.subschema_for::<ParseMode>())
            .expect("a schema serialises");
        let one = |key: &str, body: JsonValue| {
            serde_json::json!({
                "type": "object",
                "properties": {key: body},
                "required": [key],
                "additionalProperties": false,
            })
        };
        let text = serde_json::json!({"type": "string"});
        schemars::Schema::try_from(serde_json::json!({
            "description": "One step of a pipe. A field states the steps it runs and their order.",
            "oneOf": [
                {"enum": ["lowercase", "trim", "strip_bracket_tag", "blank_is_absent"]},
                one("strip_prefix", text.clone()),
                one("join", text.clone()),
                one("prepend", text),
                one("parse", parse),
                {
                    "type": "object",
                    "properties": {
                        "map": {"type": "object", "additionalProperties": true},
                        "closed": {"type": "boolean"},
                    },
                    "required": ["map"],
                    "additionalProperties": false,
                },
            ]
        }))
        .expect("an object is a schema")
    }
}
