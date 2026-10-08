//! Boolean expression grammar over four atom domains.
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

mod evidence;

pub use evidence::{ClausePath, EvidenceSet, Verdict};

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
///
/// The group variants hold their children privately and are built through [`Expr::all`] and [`Expr::any`],
/// which **cannot** produce an undersized group: one child collapses to the child itself, and none is refused.
/// The deserializer already refused both, but a translator constructs the AST directly - and the first version
/// of the translation oracle failed in exactly that way, producing nothing for a condition and having it
/// skipped. `All([])` evaluates to `True` and `Any([])` to `False`, so an accidentally-empty group is a
/// condition that holds for everything or nothing at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr<A> {
    Atom(A),
    All(Group<A>),
    Any(Group<A>),
    Not(Box<Expr<A>>),
}

/// Two or more expressions. The invariant is the type's, so no caller can forget it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group<A>(Vec<Expr<A>>);

impl<A> Group<A> {
    /// The children, which are always at least two.
    pub fn children(&self) -> &[Expr<A>] {
        &self.0
    }
}

impl<A> Expr<A> {
    /// A conjunction, or the single child where there is one, or `None` for nothing.
    ///
    /// Returning `Option` rather than an empty group is what makes "this condition translated to nothing" a
    /// value a caller has to handle rather than an expression that holds for everything.
    pub fn all(mut children: Vec<Expr<A>>) -> Option<Self> {
        match children.len() {
            0 => None,
            1 => Some(children.remove(0)),
            _ => Some(Self::All(Group(children))),
        }
    }

    /// A disjunction, under the same rule.
    pub fn any(mut children: Vec<Expr<A>>) -> Option<Self> {
        match children.len() {
            0 => None,
            1 => Some(children.remove(0)),
            _ => Some(Self::Any(Group(children))),
        }
    }

    /// Evaluate against whatever the atoms need, three-valued.
    pub fn eval(&self, atom: &mut impl FnMut(&A) -> Truth) -> Truth {
        match self {
            Self::Atom(a) => atom(a),
            Self::All(group) => {
                let mut answer = Truth::True;
                for child in group.children() {
                    match child.eval(atom) {
                        // False wins outright: nothing later can make a conjunction hold.
                        Truth::False => return Truth::False,
                        Truth::Unknown => answer = Truth::Unknown,
                        Truth::True => {}
                    }
                }
                answer
            }
            Self::Any(group) => {
                let mut answer = Truth::False;
                for child in group.children() {
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
            Self::All(group) | Self::Any(group) => {
                group.children().iter().flat_map(Self::atoms).collect()
            }
            Self::Not(child) => child.atoms(),
        }
    }

    /// How deep the tree goes, so a compiler can bound it.
    pub fn depth(&self) -> usize {
        match self {
            Self::Atom(_) => 1,
            Self::All(group) | Self::Any(group) => {
                1 + group.children().iter().map(Self::depth).max().unwrap_or(0)
            }
            Self::Not(child) => 1 + child.depth(),
        }
    }
}

/// The editor schema of an expression: an atom, or one boolean clause over expressions.
#[cfg(test)]
impl<A: schemars::JsonSchema> schemars::JsonSchema for Expr<A> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        format!("Expr_{}", A::schema_name()).into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let me = generator.subschema_for::<Self>();
        let atom = generator.subschema_for::<A>();
        // What the parser accepts, exactly: `doc` is prose, never `null`, and a group holds two or more.
        let doc = serde_json::json!({"type": "string"});
        let group = |key: &str, body: serde_json::Value| {
            serde_json::json!({
                "type": "object",
                "properties": {key: body, "doc": doc.clone()},
                "required": [key],
                "additionalProperties": false,
            })
        };
        let me = serde_json::to_value(me).expect("a schema serialises");
        let list = serde_json::json!({"type": "array", "items": me.clone(), "minItems": 2});
        schemars::Schema::try_from(serde_json::json!({
            "oneOf": [
                serde_json::to_value(atom).expect("a schema serialises"),
                group("all", list.clone()),
                group("any", list),
                group("not", me),
            ]
        }))
        .expect("an object is a schema")
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

        // `doc` documents the node, as it does every other object of the format, and is read by nothing. It is
        // type-checked rather than swallowed - `"doc": 17` is refused - and taken out before the clause is
        // read, so a group may say why it is a group and an atom keeps its own `doc`.
        let group_doc = members
            .keys()
            .any(|key| matches!(key.as_str(), "all" | "any" | "not"));
        if group_doc
            && let Some(doc) = members.remove("doc")
            && !doc.is_string()
        {
            return Err(de::Error::custom("`doc` is prose: a string"));
        }
        let clauses: Vec<&String> = members.keys().collect();
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
            ("all", Expr::all as fn(Vec<Expr<A>>) -> Option<Expr<A>>),
            ("any", Expr::any as fn(Vec<Expr<A>>) -> Option<Expr<A>>),
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
                return build(children)
                    .ok_or_else(|| de::Error::custom(format!("`{key}` has no child expressions")));
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
// JSON atoms
// ============================================================================

/// One question about a parsed payload, with the selection made **explicit**.
///
/// The binder is the point. Today's `ValuePredicate` carries a path and several conditions, and one selected
/// value must satisfy them **all** - so `{path: "$.items[*]", starts_with: "a", one_of: ["apple","banana"]}`
/// is false for `["avocado","banana"]`, because no single item is both. Translating each condition into its own
/// atom under `all` would make that *true*, with different elements witnessing the two clauses: a silent change
/// of meaning in every multi-condition rule. So a selection is one atom holding a sub-expression, and the
/// witness is shared by construction.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum JsonAtom {
    /// At least one value is selected. A **total** question - the answer is yes or no, never `Unknown`.
    Exists { path: super::schema::JsonPath },
    /// Some selected value satisfies the sub-expression.
    ///
    /// `Unknown` when the path selects nothing: "no value satisfied this" and "there was no value to ask
    /// about" are different answers, and conflating them is what let a negation hold for an absent member.
    Some {
        path: super::schema::JsonPath,
        satisfies: Box<Expr<JsonSubjectAtom>>,
    },
}

/// One question about a single selected value.
///
/// No polarity flags and no negative twins: `not_null` is `not kind:null`, `lacks_prefix` is
/// `not starts_with`, `none_of` is `not one_of`, and `non_empty: false` is `not non_empty`. Each of those pairs
/// was a place where the negative form and the positive form disagreed about a missing or wrongly-typed value.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum JsonSubjectAtom {
    /// The value's JSON kind. Total: every value has one.
    Kind { kind: super::schema::ValueKind },
    /// A string, array or object with something in it. `Unknown` for a scalar, which is neither empty nor
    /// non-empty in this vocabulary.
    NonEmpty,
    /// A string holding something other than whitespace. `Unknown` for anything else.
    NonBlank,
    /// A string that looks like an identifier. `Unknown` for anything else.
    IdentifierLike,
    /// A string beginning with this. `Unknown` for anything else.
    StartsWith { prefix: String },
    /// A string among these. `Unknown` for anything else.
    OneOf { values: Vec<String> },
    /// Exactly this JSON value. Total: every value is or is not it.
    Equals { value: serde_json::Value },
    /// An object with no member outside these. `Unknown` for anything that is not an object.
    OnlyMembers { names: Vec<String> },
}

/// A JSON expression.
pub type JsonExpr = Expr<JsonAtom>;

impl JsonSubjectAtom {
    pub fn eval(&self, subject: &serde_json::Value) -> Truth {
        use serde_json::Value;
        match self {
            Self::Kind { kind } => Truth::total(match kind {
                super::schema::ValueKind::Object => subject.is_object(),
                super::schema::ValueKind::Array => subject.is_array(),
                super::schema::ValueKind::String => subject.is_string(),
                super::schema::ValueKind::Number => subject.is_number(),
                super::schema::ValueKind::Bool => subject.is_boolean(),
                super::schema::ValueKind::Null => subject.is_null(),
            }),
            Self::NonEmpty => match subject {
                Value::String(text) => Truth::total(!text.is_empty()),
                Value::Array(items) => Truth::total(!items.is_empty()),
                Value::Object(members) => Truth::total(!members.is_empty()),
                // A scalar cannot answer: it is neither empty nor non-empty here.
                _ => Truth::Unknown,
            },
            Self::NonBlank => match subject.as_str() {
                Some(text) => Truth::total(!text.trim().is_empty()),
                None => Truth::Unknown,
            },
            Self::IdentifierLike => match subject.as_str() {
                // The retired test exactly: the *first* character is alphanumeric or an underscore. A
                // narrow test, and copied rather than improved - this translation must not change meaning.
                Some(text) => {
                    Truth::total(text.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
                }
                None => Truth::Unknown,
            },
            Self::StartsWith { prefix } => match subject.as_str() {
                Some(text) => Truth::total(text.starts_with(prefix.as_str())),
                None => Truth::Unknown,
            },
            Self::OneOf { values } => match subject.as_str() {
                Some(text) => Truth::total(values.iter().any(|value| value == text)),
                None => Truth::Unknown,
            },
            Self::Equals { value } => Truth::total(subject == value),
            Self::OnlyMembers { names } => match subject.as_object() {
                Some(members) => Truth::total(
                    members
                        .keys()
                        .all(|key| names.iter().any(|name| name == key)),
                ),
                None => Truth::Unknown,
            },
        }
    }
}

impl JsonAtom {
    pub fn eval(&self, payload: &serde_json::Value) -> Truth {
        match self {
            Self::Exists { path } => Truth::total(!path.query(payload).is_empty()),
            Self::Some { path, satisfies } => {
                let selected = path.query(payload);
                if selected.is_empty() {
                    // Nothing to ask about. Not `False`, so a negation over it does not hold either.
                    return Truth::Unknown;
                }
                let mut answer = Truth::False;
                for subject in selected.iter() {
                    match satisfies.eval(&mut |atom| atom.eval(subject)) {
                        // Existential: one witness is enough, and it satisfies the *whole* sub-expression,
                        // which is what keeps a multi-condition rule meaning what it meant.
                        Truth::True => return Truth::True,
                        Truth::Unknown => answer = Truth::Unknown,
                        Truth::False => {}
                    }
                }
                answer
            }
        }
    }
}

/// A disjunction of exactly two, for the translation sites that build one literally.
///
/// `Expr::any` returns `Option` so a caller cannot forget the empty case; here the two children are written
/// out, so the invariant holds by construction and the `expect` states that rather than hiding it.
fn two_of<A>(children: Vec<Expr<A>>) -> Expr<A> {
    debug_assert_eq!(children.len(), 2);
    Expr::any(children).expect("two children were written out")
}

/// The same for a conjunction.
fn all_two<A>(children: Vec<Expr<A>>) -> Expr<A> {
    debug_assert_eq!(children.len(), 2);
    Expr::all(children).expect("two children were written out")
}

/// A `ValuePredicate` as a JSON expression, preserving what it meant exactly.
///
/// Three things this has to get right, each of which a naive translation gets wrong:
///
/// - **One witness for every condition.** All of the predicate's conditions go inside a *single* `some`, so
///   they are asked of the same selected value - which is what the retired evaluator did and what makes
///   `{starts_with: "a", one_of: ["apple","banana"]}` false for `["avocado","banana"]`.
/// - **`exists` is not a condition on a value.** `exists: true` asks whether anything was selected, so it
///   becomes the total `Exists` atom rather than something inside `some`; `exists: false` is its negation.
/// - **A bare `none_of` holds for an absent value**, and one asset depends on it. The retired code special-cased
///   exactly that - `none_of` set, every other condition unset - so the translation is the explicit
///   `any(not exists, some(not one_of))`, which says what the special case meant instead of relying on a
///   negation that quietly accepts absence.
pub fn json_expr_of_predicate(predicate: &super::schema::ValuePredicate) -> Option<JsonExpr> {
    let path = predicate
        .path
        .clone()
        .unwrap_or_else(|| super::schema::JsonPath::parse("$").expect("`$` is a path"));

    // Conditions about the selected value, all sharing one witness.
    let mut subject: Vec<Expr<JsonSubjectAtom>> = Vec::new();
    if let Some(kind) = predicate.kind {
        subject.push(Expr::Atom(JsonSubjectAtom::Kind { kind }));
    }
    if let Some(want) = predicate.non_empty {
        let atom = Expr::Atom(JsonSubjectAtom::NonEmpty);
        subject.push(if want {
            atom
        } else {
            Expr::Not(Box::new(atom))
        });
    }
    if let Some(want) = predicate.non_blank {
        let atom = Expr::Atom(JsonSubjectAtom::NonBlank);
        subject.push(if want {
            atom
        } else {
            Expr::Not(Box::new(atom))
        });
    }
    if let Some(want) = predicate.identifier_like {
        let atom = Expr::Atom(JsonSubjectAtom::IdentifierLike);
        subject.push(if want {
            atom
        } else {
            Expr::Not(Box::new(atom))
        });
    }
    if let Some(want) = predicate.not_null {
        let atom = Expr::Atom(JsonSubjectAtom::Kind {
            kind: super::schema::ValueKind::Null,
        });
        subject.push(if want {
            Expr::Not(Box::new(atom))
        } else {
            atom
        });
    }
    if let Some(prefix) = &predicate.starts_with {
        subject.push(Expr::Atom(JsonSubjectAtom::StartsWith {
            prefix: prefix.clone(),
        }));
    }
    if let Some(prefix) = &predicate.lacks_prefix {
        // "Not a string at all, or a string without the prefix" - because that is what the retired predicate
        // meant. `not starts_with` alone is `Unknown` for a number, so it would *not* hold, and the retired
        // one held: a negative string test there silently accepted every non-string value. The tightening is
        // the right end state and it changes what an asset says, so it belongs in the asset's own migration,
        // not in a translation whose whole job is to preserve meaning.
        subject.push(two_of(vec![
            Expr::Not(Box::new(Expr::Atom(JsonSubjectAtom::Kind {
                kind: super::schema::ValueKind::String,
            }))),
            Expr::Not(Box::new(Expr::Atom(JsonSubjectAtom::StartsWith {
                prefix: prefix.clone(),
            }))),
        ]));
    }
    if !predicate.one_of.is_empty() {
        subject.push(Expr::Atom(JsonSubjectAtom::OneOf {
            values: predicate.one_of.clone(),
        }));
    }
    if let Some(value) = &predicate.equals {
        subject.push(Expr::Atom(JsonSubjectAtom::Equals {
            value: value.clone(),
        }));
    }
    if !predicate.only_members.is_empty() {
        subject.push(Expr::Atom(JsonSubjectAtom::OnlyMembers {
            names: predicate.only_members.clone(),
        }));
    }
    let negative_set = !predicate.none_of.is_empty();
    if negative_set {
        // The same widening as `lacks_prefix`, for the same reason.
        subject.push(two_of(vec![
            Expr::Not(Box::new(Expr::Atom(JsonSubjectAtom::Kind {
                kind: super::schema::ValueKind::String,
            }))),
            Expr::Not(Box::new(Expr::Atom(JsonSubjectAtom::OneOf {
                values: predicate.none_of.clone(),
            }))),
        ]));
    }

    // Every condition inside **one** selection, which is the witness binding.
    let value_question = Expr::all(subject).map(|satisfies| JsonAtom::Some {
        path: path.clone(),
        satisfies: Box::new(satisfies),
    });

    // The absence special case, reproduced explicitly. Only when `none_of` is the *sole* condition, which is
    // the guard the retired code carried.
    let bare_negative_set = negative_set
        && predicate.exists.is_none()
        && predicate.kind.is_none()
        && predicate.non_empty.is_none()
        && predicate.non_blank.is_none()
        && predicate.not_null.is_none()
        && predicate.identifier_like.is_none()
        && predicate.starts_with.is_none()
        && predicate.lacks_prefix.is_none()
        && predicate.one_of.is_empty()
        && predicate.equals.is_none()
        && predicate.only_members.is_empty();

    match (predicate.exists, value_question) {
        (Some(true), None) => Some(Expr::Atom(JsonAtom::Exists { path })),
        (Some(false), None) => Some(Expr::Not(Box::new(Expr::Atom(JsonAtom::Exists { path })))),
        // `exists: true` beside conditions is redundant - a value that satisfies a condition was selected -
        // but it is kept as a conjunct so the translation is a transcription rather than a simplification.
        (Some(true), Some(question)) => Some(all_two(vec![
            Expr::Atom(JsonAtom::Exists { path }),
            Expr::Atom(question),
        ])),
        // `exists: false` beside conditions could never hold: nothing selected, yet a value must satisfy
        // something. The retired code answered false for it, and so does this.
        (Some(false), Some(_)) => Some(all_two(vec![
            Expr::Not(Box::new(Expr::Atom(JsonAtom::Exists {
                path: path.clone(),
            }))),
            Expr::Atom(JsonAtom::Exists { path }),
        ])),
        (None, Some(question)) if bare_negative_set => Some(two_of(vec![
            Expr::Not(Box::new(Expr::Atom(JsonAtom::Exists { path }))),
            Expr::Atom(question),
        ])),
        (None, Some(question)) => Some(Expr::Atom(question)),
        // A path and no conditions is an **implicit existence test**: the retired evaluator selected the value
        // first and answered false when nothing was there, so declaring only a path asked whether the payload
        // has that member. Without this the translation said "no condition", which holds for everything.
        (None, None) if predicate.path.is_some() => Some(Expr::Atom(JsonAtom::Exists { path })),
        (None, None) => None,
    }
}

/// A `PredicateSet` as one JSON expression.
///
/// `all` and `any` are separate lists that both have to be satisfied, which is a shape the grammar expresses as
/// `all(all(…), any(…))` - and the reason the old form could not say `(A ∧ B) ∨ (C ∧ D)`.
pub fn json_expr_of(set: &super::schema::PredicateSet) -> Option<JsonExpr> {
    let group = |predicates: &[super::schema::ValuePredicate],
                 build: fn(Vec<JsonExpr>) -> Option<JsonExpr>|
     -> Option<JsonExpr> {
        build(
            predicates
                .iter()
                .filter_map(json_expr_of_predicate)
                .collect(),
        )
    };
    match (group(&set.all, Expr::all), group(&set.any, Expr::any)) {
        (None, None) => None,
        (Some(all), None) => Some(all),
        (None, Some(any)) => Some(any),
        (Some(all), Some(any)) => Some(all_two(vec![all, any])),
    }
}

// ============================================================================
// Atom validation
// ============================================================================

/// Why an atom could never mean what it says.
///
/// The direct syntax has to refuse what the old shell refused, and the old refusals lived in the shell's
/// compiler rather than in the atoms - so an atom written directly could say `{"span_name_starts_with":
/// {"prefix": ""}}` (matches every span) or `{"text_contains": {"sources": [], "needle": "x"}}` (can never
/// hold). One validator on the atom, so every caller gets the same answer wherever the atom was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtomDefect {
    pub atom: &'static str,
    pub reason: &'static str,
}

impl JsonSubjectAtom {
    pub fn defect(&self) -> Option<AtomDefect> {
        let defect = |atom, reason| Some(AtomDefect { atom, reason });
        match self {
            Self::StartsWith { prefix } if prefix.is_empty() => defect(
                "starts_with",
                "an empty prefix, which every string begins with",
            ),
            Self::OneOf { values } if values.is_empty() => {
                defect("one_of", "an empty set, which no value is in")
            }
            Self::OnlyMembers { names } if names.is_empty() => defect(
                "only_members",
                "an empty set, which only an empty object satisfies",
            ),
            _ => None,
        }
    }
}

impl<A> Expr<A> {
    /// Every defect in the tree, through a per-atom validator.
    pub fn defects(&self, defect_of: &impl Fn(&A) -> Option<AtomDefect>) -> Vec<AtomDefect> {
        self.atoms().into_iter().filter_map(defect_of).collect()
    }
}
