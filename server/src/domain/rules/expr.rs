//! One boolean expression grammar over four atom domains.
//!
//! Before this, the format had four partial combinators and none of them could express an ordinary
//! conjunction. `DetectMatch`'s dimensions are implicitly OR'd, so a gate saying *this attribute equals chat*
//! and *that attribute exists* meant **or**, and there was no way to say **and**; classification worked around
//! it with its own `all_of: Vec<DetectMatch>`; `PredicateSet` could say `A ∧ B ∧ (C ∨ D)` and not
//! `(A ∧ B) ∨ (C ∧ D)`; and `MemberRequirements` had a third spelling of the same shape.
//!
//! The grammar is deliberately small:
//!
//! ```text
//! Expr<A> := A | {"all": [Expr, Expr, …]} | {"any": [Expr, Expr, …]} | {"not": Expr}
//! ```
//!
//! with three properties that each remove a way to write something that does not mean what it reads as:
//!
//! - **`all` and `any` need at least two children.** A singleton group duplicates its child, and an
//!   accidentally-empty one broadened a rule to everything - `require: {"any": []}` held unconditionally.
//! - **Exactly one clause per object.** `{"all": […], "any": […]}` is refused rather than silently meaning one
//!   of them.
//! - **Omission is not an expression.** "No condition" belongs to the field that holds the expression, so
//!   there is no `{}` and no `true` to write by accident.
//!
//! ## Three truth values, and why `not` needs them
//!
//! A two-valued `not` is the trap this repository has been caught by repeatedly: `NOT IN (…)` over a nullable
//! column drops the rows with no value, and `Filter::positive_twin` exists because negating "is this session"
//! also dropped every trace with no session. The same trap at predicate level is `none_of`, which today holds
//! for a **missing** member - so a rule reading "the event name is not one of these" also matched events with
//! no name at all.
//!
//! So an atom that cannot ask its question answers [`Truth::Unknown`], and the groups are strong Kleene:
//!
//! | | `all` | `any` | `not` |
//! | --- | --- | --- | --- |
//! | any child False | False | — | — |
//! | all children True | True | — | — |
//! | any child True | — | True | — |
//! | all children False | — | False | — |
//! | otherwise | Unknown | Unknown | mirrors its child |
//!
//! A condition **holds** only when the answer is True, so `Unknown` is not quietly a pass. The point of the
//! third value is that `not` over it stays `Unknown` rather than becoming True: absence is expressed by asking
//! about presence, never by negating a question about a value that is not there.

use std::fmt::Debug;

use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess, Visitor};

/// Three-valued truth, because an atom may be unable to ask its question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Truth {
    True,
    False,
    /// The atom could not ask what it states: a path selected nothing, a string test met a number, an
    /// attribute-value test found no attribute.
    Unknown,
}

impl Truth {
    /// Whether a condition holds. `Unknown` does not.
    pub fn holds(self) -> bool {
        self == Self::True
    }

    /// From a total question - one that always has a yes or no, like presence.
    pub fn total(answer: bool) -> Self {
        if answer { Self::True } else { Self::False }
    }
}

/// Strong Kleene negation: the mirror of True and False, and `Unknown` unchanged.
///
/// The trait rather than an inherent `not`, so `!truth` reads as negation and nothing shadows the standard
/// method - and so the one thing this type must not do, turning "I could not ask" into "no", is written in one
/// place.
impl std::ops::Not for Truth {
    type Output = Self;

    fn not(self) -> Self {
        match self {
            Self::True => Self::False,
            Self::False => Self::True,
            Self::Unknown => Self::Unknown,
        }
    }
}

/// A boolean expression over some atom domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr<A> {
    Atom(A),
    All(Vec<Expr<A>>),
    Any(Vec<Expr<A>>),
    Not(Box<Expr<A>>),
}

impl<A> Expr<A> {
    /// Evaluate against whatever the atoms need, three-valued.
    pub fn eval(&self, atom: &mut impl FnMut(&A) -> Truth) -> Truth {
        match self {
            Self::Atom(a) => atom(a),
            Self::All(children) => {
                let mut answer = Truth::True;
                for child in children {
                    match child.eval(atom) {
                        // False wins outright: nothing later can make a conjunction hold.
                        Truth::False => return Truth::False,
                        Truth::Unknown => answer = Truth::Unknown,
                        Truth::True => {}
                    }
                }
                answer
            }
            Self::Any(children) => {
                let mut answer = Truth::False;
                for child in children {
                    match child.eval(atom) {
                        Truth::True => return Truth::True,
                        Truth::Unknown => answer = Truth::Unknown,
                        Truth::False => {}
                    }
                }
                answer
            }
            Self::Not(child) => !child.eval(atom),
        }
    }

    /// Every atom in the tree, for compile-time validation of the atoms themselves.
    pub fn atoms(&self) -> Vec<&A> {
        match self {
            Self::Atom(a) => vec![a],
            Self::All(children) | Self::Any(children) => {
                children.iter().flat_map(Self::atoms).collect()
            }
            Self::Not(child) => child.atoms(),
        }
    }

    /// How deep the tree goes, so a compiler can bound it.
    pub fn depth(&self) -> usize {
        match self {
            Self::Atom(_) => 1,
            Self::All(children) | Self::Any(children) => {
                1 + children.iter().map(Self::depth).max().unwrap_or(0)
            }
            Self::Not(child) => 1 + child.depth(),
        }
    }
}

/// Deserialised by hand rather than with `serde(untagged)`.
///
/// `untagged` tries each variant and reports "data did not match any variant" from wherever it gave up, which
/// for a nested tree names neither the clause nor the depth - and this format's diagnostics are the reason an
/// author can find their mistake at all. Reading the single key first means the error says which clause was
/// written and what was wrong inside it.
impl<'de, A> Deserialize<'de> for Expr<A>
where
    A: serde::de::DeserializeOwned,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(ExprVisitor::<A> {
            marker: std::marker::PhantomData,
        })
    }
}

struct ExprVisitor<A> {
    marker: std::marker::PhantomData<A>,
}

impl<'de, A> Visitor<'de> for ExprVisitor<A>
where
    A: serde::de::DeserializeOwned,
{
    type Value = Expr<A>;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("an object with exactly one of `all`, `any`, `not`, or an atom's own clause")
    }

    fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        // The whole object, so an atom can be deserialised from it after the group keys are ruled out. Read as
        // a map rather than a typed value because the discriminator is the key itself.
        let mut members = serde_json::Map::new();
        while let Some((key, value)) = map.next_entry::<String, serde_json::Value>()? {
            if members.insert(key.clone(), value).is_some() {
                return Err(de::Error::custom(format!(
                    "`{key}` is declared twice in one expression"
                )));
            }
        }

        // `doc` is a reserved sibling everywhere in this format, so it is not a clause.
        let clauses: Vec<&String> = members.keys().filter(|key| *key != "doc").collect();
        let group = clauses
            .iter()
            .filter(|key| matches!(key.as_str(), "all" | "any" | "not"))
            .count();
        if group > 0 && clauses.len() > group {
            return Err(de::Error::custom(
                "an expression mixes a boolean clause with an atom's clause - write the atom inside the \
                 group",
            ));
        }
        if group > 1 {
            return Err(de::Error::custom(
                "an expression declares more than one of `all`, `any` and `not`, so which applies would \
                 depend on reading order",
            ));
        }

        if let Some(value) = members.get("not") {
            let inner: Expr<A> =
                serde_json::from_value(value.clone()).map_err(de::Error::custom)?;
            return Ok(Expr::Not(Box::new(inner)));
        }
        for (key, build) in [
            ("all", Expr::All as fn(Vec<Expr<A>>) -> Expr<A>),
            ("any", Expr::Any as fn(Vec<Expr<A>>) -> Expr<A>),
        ] {
            if let Some(value) = members.get(key) {
                let children: Vec<Expr<A>> =
                    serde_json::from_value(value.clone()).map_err(de::Error::custom)?;
                // Two, not one: a singleton group duplicates its child, and an empty one is the defect that
                // made `require: {"any": []}` hold unconditionally.
                if children.len() < 2 {
                    return Err(de::Error::custom(format!(
                        "`{key}` has {} child expression(s); a boolean group needs at least two, and \
                         omitting the condition is how to say there is none",
                        children.len()
                    )));
                }
                return Ok(build(children));
            }
        }

        if clauses.is_empty() {
            return Err(de::Error::custom(
                "an empty expression states nothing - omit the condition to say there is none",
            ));
        }
        let atom: A = serde_json::from_value(serde_json::Value::Object(members))
            .map_err(de::Error::custom)?;
        Ok(Expr::Atom(atom))
    }
}

// ============================================================================
// Span atoms
// ============================================================================

/// One question about a span: its name, or one of its own attributes.
///
/// Singular, unlike the dimensions it replaces. `attr_equals: [a, b]` was a *list* whose members were OR'd,
/// which is how a conjunction became inexpressible - two values of one key is `any` of two atoms now, and two
/// different keys is `all` of two atoms, and the difference is written rather than implied.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum SpanAtom {
    /// The span's name begins with this.
    SpanNameStartsWith { prefix: String },
    /// Some attribute key begins with this.
    SpanAttrKeyStartsWith { prefix: String },
    /// This attribute is present, whatever it holds. A **total** question.
    SpanAttrExists { key: String },
    /// This attribute holds exactly this value.
    SpanAttrEquals { key: String, value: String },
    /// The same, folding ASCII case.
    SpanAttrEqualsIgnoreAsciiCase { key: String, value: String },
    /// This attribute's value contains this substring.
    SpanAttrContains { key: String, value: String },
    /// A phrase search over named sources.
    TextContains {
        sources: Vec<TextSource>,
        #[serde(default)]
        source_mode: SourceMode,
        /// Singular: several needles are an `any` of several atoms.
        needle: String,
    },
}

/// Where a phrase search looks. Typed, rather than the `"attr:<key>"` string form it replaces - which could
/// name an empty key, and was validated in two places that had drifted apart.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum TextSource {
    SpanName,
    Attr { key: String },
}

/// Which sources a phrase search consults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceMode {
    /// Every source that has a value. The default, and what an unqualified search means.
    #[default]
    AnyPresent,
    /// Only the first source that has a value, in declared order: two spellings of one fact are one
    /// question, and a later one answering for an earlier is a different statement.
    FirstPresent,
}

/// What a span atom is evaluated against.
pub struct SpanSubject<'a> {
    pub span_name: &'a str,
    pub attrs: &'a std::collections::HashMap<String, String>,
}

impl SpanAtom {
    /// Three-valued, and which atoms are total is the whole point of the distinction.
    ///
    /// A presence question always has an answer. A question *about a value* has none when the attribute is
    /// absent - so `not span_attr_equals` does not hold for a span that lacks the attribute, which is the trap
    /// `none_of` fell into.
    pub fn eval(&self, subject: &SpanSubject<'_>) -> Truth {
        match self {
            Self::SpanNameStartsWith { prefix } => {
                Truth::total(subject.span_name.starts_with(prefix.as_str()))
            }
            Self::SpanAttrKeyStartsWith { prefix } => Truth::total(
                subject
                    .attrs
                    .keys()
                    .any(|key| key.starts_with(prefix.as_str())),
            ),
            Self::SpanAttrExists { key } => Truth::total(subject.attrs.contains_key(key)),
            Self::SpanAttrEquals { key, value } => match subject.attrs.get(key) {
                Some(found) => Truth::total(found == value),
                None => Truth::Unknown,
            },
            Self::SpanAttrEqualsIgnoreAsciiCase { key, value } => match subject.attrs.get(key) {
                Some(found) => Truth::total(found.eq_ignore_ascii_case(value)),
                None => Truth::Unknown,
            },
            Self::SpanAttrContains { key, value } => match subject.attrs.get(key) {
                Some(found) => Truth::total(found.contains(value.as_str())),
                None => Truth::Unknown,
            },
            Self::TextContains {
                sources,
                source_mode,
                needle,
            } => {
                let value_of = |source: &TextSource| -> Option<&str> {
                    match source {
                        TextSource::SpanName => Some(subject.span_name),
                        TextSource::Attr { key } => subject.attrs.get(key).map(String::as_str),
                    }
                };
                let folded = needle.to_ascii_lowercase();
                let hit = |text: &str| text.to_ascii_lowercase().contains(&folded);
                match source_mode {
                    SourceMode::FirstPresent => match sources.iter().find_map(value_of) {
                        Some(text) => Truth::total(hit(text)),
                        // No source had a value, so the search could not be made.
                        None => Truth::Unknown,
                    },
                    SourceMode::AnyPresent => {
                        let mut any_present = false;
                        for text in sources.iter().filter_map(value_of) {
                            any_present = true;
                            if hit(text) {
                                return Truth::True;
                            }
                        }
                        if any_present {
                            Truth::False
                        } else {
                            Truth::Unknown
                        }
                    }
                }
            }
        }
    }
}

/// A span expression.
pub type SpanExpr = Expr<SpanAtom>;

// ============================================================================
// Compiling the retired shells into the grammar
// ============================================================================

/// A `DetectMatch` as a span expression.
///
/// The translation is where the old semantics are written down, and two of them are easy to get wrong:
///
/// - **The dimensions are OR'd.** Each populated dimension is a disjunct, and within a dimension each value
///   is a disjunct too - so the whole thing is one flat `any`. That is what made a conjunction inexpressible.
/// - **`service_name` is not a span attribute.** It reads a *resource* attribute, so a span-only context
///   cannot answer it, and the old code refused it there rather than translating it. It is not produced here;
///   [`detection_expr`] handles it.
///
/// A `DetectMatch` with nothing populated becomes `None`: the old evaluators refused it, and an empty `any`
/// is not writable in the grammar for the same reason.
pub fn span_expr_of(spec: &super::schema::DetectMatch) -> Option<SpanExpr> {
    let mut disjuncts: Vec<SpanExpr> = Vec::new();
    for prefix in &spec.span_name {
        disjuncts.push(Expr::Atom(SpanAtom::SpanNameStartsWith {
            prefix: prefix.clone(),
        }));
    }
    for prefix in &spec.attr_prefix {
        disjuncts.push(Expr::Atom(SpanAtom::SpanAttrKeyStartsWith {
            prefix: prefix.clone(),
        }));
    }
    for key in &spec.attr_exists {
        disjuncts.push(Expr::Atom(SpanAtom::SpanAttrExists { key: key.clone() }));
    }
    for pair in &spec.attr_equals {
        disjuncts.push(Expr::Atom(SpanAtom::SpanAttrEquals {
            key: pair.key.clone(),
            value: pair.value.clone(),
        }));
    }
    for pair in &spec.attr_equals_ignore_case {
        disjuncts.push(Expr::Atom(SpanAtom::SpanAttrEqualsIgnoreAsciiCase {
            key: pair.key.clone(),
            value: pair.value.clone(),
        }));
    }
    for pair in &spec.span_attr_contains {
        disjuncts.push(Expr::Atom(SpanAtom::SpanAttrContains {
            key: pair.key.clone(),
            value: pair.value.clone(),
        }));
    }
    if let Some(text) = &spec.text_contains {
        let sources: Vec<TextSource> = text
            .sources
            .iter()
            .filter_map(|source| {
                if source == "span_name" {
                    Some(TextSource::SpanName)
                } else {
                    source
                        .strip_prefix("attr:")
                        .filter(|key| !key.is_empty())
                        .map(|key| TextSource::Attr {
                            key: key.to_string(),
                        })
                }
            })
            .collect();
        let mode = if text.first_present_source {
            SourceMode::FirstPresent
        } else {
            SourceMode::AnyPresent
        };
        if !sources.is_empty() {
            // A needle per atom, OR'd - which is what a list of needles meant.
            for needle in &text.needles {
                disjuncts.push(Expr::Atom(SpanAtom::TextContains {
                    sources: sources.clone(),
                    source_mode: mode,
                    needle: needle.clone(),
                }));
            }
        }
    }
    match disjuncts.len() {
        0 => None,
        1 => Some(disjuncts.remove(0)),
        _ => Some(Expr::Any(disjuncts)),
    }
}
