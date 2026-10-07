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
    /// existentially), `scope.name` (the instrumentation scope) or `resource:<key>`. Several sources are
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
    /// The value begins with this text. For `attr_keys`, some key does.
    #[serde(default)]
    pub starts_with: Option<String>,
    /// The value contains this text.
    #[serde(default)]
    pub contains: Option<String>,
    /// The value contains this text, ignoring case (Unicode lower-casing).
    #[serde(default)]
    pub contains_ignore_case: Option<String>,
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
            "description": "`span_name`, `attr_keys`, `scope.name`, `attr:<key>` or `resource:<key>`.",
            "anyOf": [
                {"enum": ["span_name", "attr_keys", "scope.name"]},
                {"type": "string"}
            ]
        })
    }
}
