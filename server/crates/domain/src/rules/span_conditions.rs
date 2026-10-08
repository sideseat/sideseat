//! Conditions about a span: the `where` of every section that asks one.
//!
//! Detection, classification, span facts, span-field sources and message-rule gates each had their own dialect
//! for the same question - `DetectMatch`'s implicitly OR'd dimensions, a classification's `all_of` list of them,
//! a span fact's `attr_equals` with an `ignore_case` flag and an `attrs_present` conjunction, a message rule's
//! `when`, `unless` and `instrumentation_scope`. Now each declares one `where` in the expression grammar of
//! [`super::expr`], over one atom shape ([`SpanCondition`]: a source and the tests asked of it), and this module
//! lowers that into [`SpanExpr`], decides what a section may read, and answers the two compile-time questions
//! the sections ask of conditions: what can never hold or never matter, and whether one condition implies
//! another (shadowing, and which of two contending message rules suppresses the other).

use std::collections::HashMap;

use super::expr::{AtomDefect, Expr, Truth};
use super::schema::{ConditionSource, SpanCondition, SpanWhere};

/// One question about a span, as evaluated.
///
/// Singular: two values of one key are `any` of two atoms, two keys `all` of two, and the difference is written
/// rather than implied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpanAtom {
    /// The span's name is exactly this.
    SpanNameEquals { name: String },
    /// The span's name begins with this.
    SpanNameStartsWith { prefix: String },
    /// Some attribute key begins with this. Total: over no keys it is false.
    SpanAttrKeyStartsWith { prefix: String },
    /// This attribute is present, whatever it holds. Total.
    SpanAttrExists { key: String },
    /// This attribute holds exactly this value.
    SpanAttrEquals { key: String, value: String },
    /// The same, folding ASCII case.
    SpanAttrEqualsIgnoreAsciiCase { key: String, value: String },
    /// This attribute's value contains this substring.
    SpanAttrContains { key: String, value: String },
    /// The instrumentation scope is exactly this.
    ScopeNameEquals { name: String },
    /// The instrumentation scope's name begins with this. Unknown where the span reports no scope.
    ScopeNameStartsWith { prefix: String },
    /// This resource attribute's value contains this substring.
    ResourceAttrContains { key: String, value: String },
    /// The instrumentation scope's version is a release in `[at_least, below)` of this scheme. Unknown where
    /// there is no version or it does not parse.
    ScopeVersionIn {
        scheme: super::versions::VersionScheme,
        at_least: Option<super::versions::Version>,
        below: Option<super::versions::Version>,
    },
    /// A case-insensitive phrase search over named sources.
    TextContains {
        sources: Vec<TextSource>,
        source_mode: SourceMode,
        needle: String,
    },
}

/// Where a phrase search looks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextSource {
    SpanName,
    Attr { key: String },
}

/// Which sources a phrase search consults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceMode {
    /// Every source that has a value.
    #[default]
    AnyPresent,
    /// Only the first source that has a value, in declared order: two spellings of one fact are one question.
    FirstPresent,
}

/// What a span condition is evaluated against.
///
/// The scope and the resource are optional because not every caller is given them; a section whose caller is
/// not refuses conditions that read them when it compiles, so `None` here is an absent value, never a missing
/// capability.
pub struct SpanSubject<'a> {
    pub span_name: &'a str,
    pub attrs: &'a HashMap<String, String>,
    pub scope_name: Option<&'a str>,
    pub scope_version: Option<&'a str>,
    pub resource: Option<&'a HashMap<String, String>>,
}

/// A span condition, lowered.
pub type SpanExpr = Expr<SpanAtom>;

impl SpanAtom {
    /// Three-valued. A presence question always has an answer; a question about a value has none where there
    /// is no value, so `not` over it does not hold for an absent attribute.
    pub fn eval(&self, subject: &SpanSubject<'_>) -> Truth {
        let value_test = |found: Option<&str>, test: &dyn Fn(&str) -> bool| match found {
            Some(found) => Truth::total(test(found)),
            None => Truth::Unknown,
        };
        match self {
            Self::SpanNameEquals { name } => Truth::total(subject.span_name == name),
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
            Self::SpanAttrEquals { key, value } => {
                value_test(subject.attrs.get(key).map(String::as_str), &|found| {
                    found == value
                })
            }
            Self::SpanAttrEqualsIgnoreAsciiCase { key, value } => {
                value_test(subject.attrs.get(key).map(String::as_str), &|found| {
                    found.eq_ignore_ascii_case(value)
                })
            }
            Self::SpanAttrContains { key, value } => {
                value_test(subject.attrs.get(key).map(String::as_str), &|found| {
                    found.contains(value.as_str())
                })
            }
            Self::ScopeNameEquals { name } => {
                value_test(subject.scope_name, &|found| found == name)
            }
            Self::ScopeNameStartsWith { prefix } => value_test(subject.scope_name, &|found| {
                found.starts_with(prefix.as_str())
            }),
            Self::ScopeVersionIn {
                scheme,
                at_least,
                below,
            } => match subject
                .scope_version
                .and_then(|text| super::versions::Version::parse(*scheme, text))
            {
                None => Truth::Unknown,
                Some(version) => {
                    let at_or_after = |bound: &super::versions::Version| {
                        version
                            .compare(bound)
                            .is_some_and(std::cmp::Ordering::is_ge)
                    };
                    Truth::total(
                        at_least.as_ref().is_none_or(at_or_after)
                            && below.as_ref().is_none_or(|bound| !at_or_after(bound)),
                    )
                }
            },
            Self::ResourceAttrContains { key, value } => value_test(
                subject
                    .resource
                    .and_then(|resource| resource.get(key))
                    .map(String::as_str),
                &|found| found.contains(value.as_str()),
            ),
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
                // Unicode lowercase, as the phrase search always folded: an ASCII fold would quietly narrow
                // every search over non-ASCII text.
                let folded = needle.to_lowercase();
                let hit = |text: &str| text.to_lowercase().contains(&folded);
                match source_mode {
                    SourceMode::FirstPresent => match sources.iter().find_map(value_of) {
                        Some(text) => Truth::total(hit(text)),
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

    /// The literal defect this atom carries, wherever it was written.
    pub fn defect(&self) -> Option<AtomDefect> {
        let defect = |atom, reason| Some(AtomDefect { atom, reason });
        match self {
            Self::SpanNameStartsWith { prefix } if prefix.is_empty() => defect(
                "starts_with",
                "an empty prefix, which every span name begins with",
            ),
            Self::SpanAttrKeyStartsWith { prefix } if prefix.is_empty() => defect(
                "starts_with",
                "an empty prefix, which every attribute key begins with",
            ),
            Self::ScopeNameStartsWith { prefix } if prefix.is_empty() => defect(
                "starts_with",
                "an empty prefix, which every instrumentation scope's name begins with",
            ),
            Self::SpanAttrContains { value, .. } | Self::ResourceAttrContains { value, .. }
                if value.is_empty() =>
            {
                defect(
                    "contains",
                    "an empty substring, which every present value contains",
                )
            }
            Self::ScopeNameEquals { name } if name.is_empty() => defect(
                "equals",
                "an empty scope name, which no instrumentation scope has",
            ),
            Self::TextContains { needle, .. } if needle.is_empty() => {
                defect("contains_ignore_case", "an empty needle")
            }
            _ => None,
        }
    }

    /// The attribute key this atom reads, for implication and near-miss reports.
    fn key(&self) -> Option<&str> {
        match self {
            Self::SpanAttrExists { key }
            | Self::SpanAttrEquals { key, .. }
            | Self::SpanAttrEqualsIgnoreAsciiCase { key, .. }
            | Self::SpanAttrContains { key, .. } => Some(key),
            _ => None,
        }
    }

    /// Whether this atom holding means `other` holds, for every span.
    ///
    /// Conservative by construction: every arm is a containment or an equality that holds whatever the span
    /// carries, and anything not listed answers no - an unproven implication under-refuses, while a wrong one
    /// deletes a working rule at startup.
    pub fn implies(&self, other: &Self) -> bool {
        if self == other {
            return true;
        }
        match other {
            // Anything that reads a key proves the key is there.
            Self::SpanAttrExists { key } => self.key() == Some(key.as_str()),
            // A key under a longer prefix is under the shorter one.
            Self::SpanAttrKeyStartsWith { prefix } => match self {
                Self::SpanAttrKeyStartsWith { prefix: mine } => mine.starts_with(prefix.as_str()),
                _ => self
                    .key()
                    .is_some_and(|key| key.starts_with(prefix.as_str())),
            },
            Self::SpanNameStartsWith { prefix } => match self {
                Self::SpanNameEquals { name: mine } | Self::SpanNameStartsWith { prefix: mine } => {
                    mine.starts_with(prefix.as_str())
                }
                _ => false,
            },
            Self::ScopeNameStartsWith { prefix } => match self {
                Self::ScopeNameEquals { name: mine }
                | Self::ScopeNameStartsWith { prefix: mine } => mine.starts_with(prefix.as_str()),
                _ => false,
            },
            Self::SpanAttrEqualsIgnoreAsciiCase { key, value } => match self {
                Self::SpanAttrEquals {
                    key: mine,
                    value: mine_value,
                } => mine == key && mine_value.eq_ignore_ascii_case(value),
                _ => false,
            },
            Self::SpanAttrContains { key, value } => match self {
                Self::SpanAttrContains {
                    key: mine,
                    value: mine_value,
                }
                | Self::SpanAttrEquals {
                    key: mine,
                    value: mine_value,
                } => mine == key && mine_value.contains(value.as_str()),
                _ => false,
            },
            Self::ResourceAttrContains { key, value } => match self {
                Self::ResourceAttrContains {
                    key: mine,
                    value: mine_value,
                } => mine == key && mine_value.contains(value.as_str()),
                _ => false,
            },
            // A narrower range of the same scheme: wherever a version is inside it, it is inside the wider.
            Self::ScopeVersionIn {
                scheme,
                at_least,
                below,
            } => match self {
                Self::ScopeVersionIn {
                    scheme: mine,
                    at_least: my_low,
                    below: my_high,
                } => {
                    let le = |a: &super::versions::Version, b: &super::versions::Version| {
                        a.compare(b).is_some_and(std::cmp::Ordering::is_le)
                    };
                    mine == scheme
                        && match (at_least, my_low) {
                            (None, _) => true,
                            (Some(_), None) => false,
                            (Some(low), Some(mine)) => le(low, mine),
                        }
                        && match (below, my_high) {
                            (None, _) => true,
                            (Some(_), None) => false,
                            (Some(high), Some(mine)) => le(mine, high),
                        }
                }
                _ => false,
            },
            _ => false,
        }
    }
}

/// What a section's conditions may read, beyond the span's name and attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Readable {
    pub span_name: bool,
    pub attributes: bool,
    pub scope: bool,
    pub scope_version: bool,
    pub resource: bool,
}

impl Readable {
    /// The span's attributes only: span facts.
    pub const ATTRIBUTES: Self = Self {
        span_name: false,
        attributes: true,
        scope: false,
        scope_version: false,
        resource: false,
    };
    /// The span's name and attributes: classification and span-field sources.
    pub const SPAN: Self = Self {
        span_name: true,
        attributes: true,
        scope: false,
        scope_version: false,
        resource: false,
    };
    /// And the instrumentation scope: message-rule gates.
    pub const SPAN_AND_SCOPE: Self = Self {
        span_name: true,
        attributes: true,
        scope: true,
        scope_version: false,
        resource: false,
    };
    /// Everything: detection.
    pub const ALL: Self = Self {
        span_name: true,
        attributes: true,
        scope: true,
        scope_version: false,
        resource: true,
    };
    /// A stored row being projected: its span name and instrumentation scope, version included, and none of
    /// its attributes - projection reads the row, not the span.
    pub const PROJECTION: Self = Self {
        span_name: true,
        attributes: false,
        scope: true,
        scope_version: true,
        resource: false,
    };
}

/// Why a condition cannot be lowered: a source the section is not given, an unknown source, or a test the
/// source cannot answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionDefect(pub String);

impl std::fmt::Display for ConditionDefect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One parsed source.
enum Source {
    SpanName,
    Attr(String),
    AttrKeys,
    Scope,
    ScopeVersion,
    Resource(String),
}

fn parse_source(text: &str, readable: Readable) -> Result<Source, ConditionDefect> {
    let refuse = |why: &str| Err(ConditionDefect(format!("source `{text}` {why}")));
    let source = if text == "span_name" {
        Source::SpanName
    } else if text == "attr_keys" {
        Source::AttrKeys
    } else if text == "scope.name" {
        Source::Scope
    } else if text == "scope.version" {
        Source::ScopeVersion
    } else if let Some(key) = text.strip_prefix("attr:") {
        Source::Attr(key.to_string())
    } else if let Some(key) = text.strip_prefix("resource:") {
        Source::Resource(key.to_string())
    } else {
        return refuse(
            "is not one of `span_name`, `attr:<key>`, `attr_keys`, `scope.name`, `scope.version`, `resource:<key>`",
        );
    };
    match &source {
        Source::Attr(key) | Source::Resource(key) if key.is_empty() => refuse("names an empty key"),
        Source::SpanName if !readable.span_name => {
            refuse("is the span's name, which this section is not given - it could never hold")
        }
        Source::Scope if !readable.scope => refuse(
            "is the instrumentation scope, which this section is not given - it could never hold",
        ),
        Source::Resource(_) if !readable.resource => {
            refuse("is a resource attribute, which this section is not given - it could never hold")
        }
        Source::Attr(_) | Source::AttrKeys if !readable.attributes => refuse(
            "reads the span's attributes, which this section is not given - it could never hold",
        ),
        Source::ScopeVersion if !readable.scope_version => refuse(
            "is the instrumentation scope's version, which this section is not given - it could never hold",
        ),
        _ => Ok(source),
    }
}

/// A `where` lowered into the evaluated grammar, or why it cannot be.
pub fn lower(condition: &SpanWhere, readable: Readable) -> Result<SpanExpr, ConditionDefect> {
    Ok(match condition {
        Expr::Atom(atom) => lower_atom(atom, readable)?,
        Expr::All(group) => Expr::all(
            group
                .children()
                .iter()
                .map(|child| lower(child, readable))
                .collect::<Result<_, _>>()?,
        )
        .expect("a group has at least two children"),
        Expr::Any(group) => Expr::any(
            group
                .children()
                .iter()
                .map(|child| lower(child, readable))
                .collect::<Result<_, _>>()?,
        )
        .expect("a group has at least two children"),
        Expr::Not(child) => Expr::Not(Box::new(lower(child, readable)?)),
    })
}

fn lower_atom(atom: &SpanCondition, readable: Readable) -> Result<SpanExpr, ConditionDefect> {
    let refuse = |why: String| Err(ConditionDefect(why));
    let tests = usize::from(atom.exists.is_some())
        + usize::from(atom.equals.is_some())
        + usize::from(atom.equals_ignore_case.is_some())
        + usize::from(!atom.one_of.is_empty())
        + usize::from(atom.starts_with.is_some())
        + usize::from(atom.contains.is_some())
        + usize::from(atom.contains_ignore_case.is_some())
        + usize::from(atom.version.is_some());
    if tests == 0 {
        return refuse("a condition names a source and asks nothing of it".to_string());
    }
    if let Some(range) = &atom.version {
        return lower_version(atom, range, tests, readable);
    }
    // Several sources are one phrase search, and nothing else.
    let (texts, mode): (&[super::schema::SourceName], Option<SourceMode>) = match &atom.source {
        ConditionSource::One(one) => (std::slice::from_ref(one), None),
        ConditionSource::AnyOf(many) => (many.as_slice(), Some(SourceMode::AnyPresent)),
        ConditionSource::FirstOf(first) => {
            (first.first_of.as_slice(), Some(SourceMode::FirstPresent))
        }
    };
    if let Some(mode) = mode {
        let Some(needle) = &atom.contains_ignore_case else {
            return refuse(
                "several sources are searched together only by `contains_ignore_case`".to_string(),
            );
        };
        if tests > 1 || texts.len() < 2 {
            return refuse(
                "a search over several sources names at least two and asks only `contains_ignore_case`"
                    .to_string(),
            );
        }
        let mut sources = Vec::new();
        for text in texts.iter().map(|name| name.0.as_str()) {
            sources.push(match parse_source(text, readable)? {
                Source::SpanName => TextSource::SpanName,
                Source::Attr(key) => TextSource::Attr { key },
                _ => {
                    return refuse(format!(
                        "`{text}` cannot be searched as text beside other sources: only `span_name` and \
                         `attr:<key>` can"
                    ));
                }
            });
        }
        return Ok(Expr::Atom(SpanAtom::TextContains {
            sources,
            source_mode: mode,
            needle: needle.clone(),
        }));
    }
    let text = texts[0].0.as_str();
    let source = parse_source(text, readable)?;
    let mut atoms: Vec<SpanExpr> = Vec::new();
    let unanswerable = |test: &str| -> Result<SpanExpr, ConditionDefect> {
        Err(ConditionDefect(format!(
            "source `{text}` cannot answer `{test}`"
        )))
    };
    let any_of = |atoms: Vec<SpanExpr>| Expr::any(atoms).expect("one_of is not empty");
    if let Some(want) = atom.exists {
        let present = match &source {
            Source::Attr(key) => Expr::Atom(SpanAtom::SpanAttrExists { key: key.clone() }),
            _ => return unanswerable("exists"),
        };
        atoms.push(if want {
            present
        } else {
            Expr::Not(Box::new(present))
        });
    }
    if let Some(value) = &atom.equals {
        atoms.push(match &source {
            Source::SpanName => Expr::Atom(SpanAtom::SpanNameEquals {
                name: value.clone(),
            }),
            Source::Attr(key) => Expr::Atom(SpanAtom::SpanAttrEquals {
                key: key.clone(),
                value: value.clone(),
            }),
            Source::Scope => Expr::Atom(SpanAtom::ScopeNameEquals {
                name: value.clone(),
            }),
            _ => unanswerable("equals")?,
        });
    }
    if let Some(value) = &atom.equals_ignore_case {
        atoms.push(match &source {
            Source::Attr(key) => Expr::Atom(SpanAtom::SpanAttrEqualsIgnoreAsciiCase {
                key: key.clone(),
                value: value.clone(),
            }),
            _ => unanswerable("equals_ignore_case")?,
        });
    }
    if !atom.one_of.is_empty() {
        let each: Vec<SpanExpr> = match &source {
            Source::SpanName => atom
                .one_of
                .iter()
                .map(|name| Expr::Atom(SpanAtom::SpanNameEquals { name: name.clone() }))
                .collect(),
            Source::Attr(key) => atom
                .one_of
                .iter()
                .map(|value| {
                    Expr::Atom(SpanAtom::SpanAttrEquals {
                        key: key.clone(),
                        value: value.clone(),
                    })
                })
                .collect(),
            Source::Scope => atom
                .one_of
                .iter()
                .map(|name| Expr::Atom(SpanAtom::ScopeNameEquals { name: name.clone() }))
                .collect(),
            _ => vec![unanswerable("one_of")?],
        };
        atoms.push(any_of(each));
    }
    if let Some(prefix) = &atom.starts_with {
        atoms.push(match &source {
            Source::SpanName => Expr::Atom(SpanAtom::SpanNameStartsWith {
                prefix: prefix.clone(),
            }),
            Source::AttrKeys => Expr::Atom(SpanAtom::SpanAttrKeyStartsWith {
                prefix: prefix.clone(),
            }),
            Source::Scope => Expr::Atom(SpanAtom::ScopeNameStartsWith {
                prefix: prefix.clone(),
            }),
            _ => unanswerable("starts_with")?,
        });
    }
    if let Some(value) = &atom.contains {
        atoms.push(match &source {
            Source::Attr(key) => Expr::Atom(SpanAtom::SpanAttrContains {
                key: key.clone(),
                value: value.clone(),
            }),
            Source::Resource(key) => Expr::Atom(SpanAtom::ResourceAttrContains {
                key: key.clone(),
                value: value.clone(),
            }),
            _ => unanswerable("contains")?,
        });
    }
    if let Some(needle) = &atom.contains_ignore_case {
        let source = match &source {
            Source::SpanName => TextSource::SpanName,
            Source::Attr(key) => TextSource::Attr { key: key.clone() },
            _ => return unanswerable("contains_ignore_case"),
        };
        atoms.push(Expr::Atom(SpanAtom::TextContains {
            sources: vec![source],
            source_mode: SourceMode::AnyPresent,
            needle: needle.clone(),
        }));
    }
    Ok(Expr::all(atoms).expect("at least one test was counted"))
}

/// A version range, alone in its atom and over the scope's version, with bounds that parse and leave room.
fn lower_version(
    atom: &SpanCondition,
    range: &super::schema::VersionRange,
    tests: usize,
    readable: Readable,
) -> Result<SpanExpr, ConditionDefect> {
    let refuse = |why: &str| Err(ConditionDefect(why.to_string()));
    if tests > 1 {
        return refuse(
            "a version range is asked alone: another test beside it reads the version as text",
        );
    }
    let source = match &atom.source {
        ConditionSource::One(one) => parse_source(&one.0, readable)?,
        _ => return refuse("a version range reads one source"),
    };
    if !matches!(source, Source::ScopeVersion) {
        return refuse(
            "a version range reads `scope.version` only: a version written in an attribute is a value another rule \
             could read as text, which would let a version decide something without a version test",
        );
    }
    if range.because.trim().is_empty() {
        return refuse("a version range states `because`: why no shape test can say this");
    }
    let bound = |text: &Option<String>| match text {
        None => Ok(None),
        Some(text) => super::versions::Version::parse_bound(range.scheme, text)
            .map(Some)
            .ok_or_else(|| {
                ConditionDefect(format!("`{text}` is not a version in the declared scheme"))
            }),
    };
    let at_least = bound(&range.at_least)?;
    let below = bound(&range.below)?;
    match (&at_least, &below) {
        (None, None) => refuse(
            "a version range names at least one bound; without one it holds for every version",
        ),
        (Some(low), Some(high)) if low.compare(high).is_none_or(std::cmp::Ordering::is_ge) => {
            refuse(
                "a version range's `at_least` is not below its `below`, so no version is inside it",
            )
        }
        _ => Ok(Expr::Atom(SpanAtom::ScopeVersionIn {
            scheme: range.scheme,
            at_least,
            below,
        })),
    }
}

/// Whether every span `narrower` holds for, `wider` holds for too - sound, and deliberately incomplete.
///
/// Decomposed structurally, without distribution, contraposition or complement: a conjunction target needs
/// every conjunct implied; a disjunction holds only where one of its disjuncts does, so each must imply the
/// target; a conjunction implies the target if one of its conjuncts does; a disjunction target is implied by
/// implying one of its disjuncts. A negation implies only itself. `P or not P` can be unknown, so nothing here
/// assumes excluded middle.
pub fn implies(narrower: &SpanExpr, wider: &SpanExpr) -> bool {
    if narrower == wider {
        return true;
    }
    if let Expr::All(targets) = wider {
        return targets
            .children()
            .iter()
            .all(|target| implies(narrower, target));
    }
    match narrower {
        Expr::Any(group) => group.children().iter().all(|child| implies(child, wider)),
        Expr::All(group) if group.children().iter().any(|child| implies(child, wider)) => true,
        _ => match wider {
            Expr::Any(group) => group
                .children()
                .iter()
                .any(|target| implies(narrower, target)),
            Expr::Atom(target) => match narrower {
                Expr::Atom(atom) => atom.implies(target),
                _ => false,
            },
            _ => false,
        },
    }
}

/// The first disjunct of an `any` another disjunct of the same group already implies: it can never be why the
/// group held, and reads as precision the condition does not have.
pub fn dead_disjunct(condition: &SpanExpr) -> Option<(SpanExpr, SpanExpr)> {
    match condition {
        Expr::Atom(_) => None,
        Expr::Not(child) => dead_disjunct(child),
        Expr::All(group) => group.children().iter().find_map(dead_disjunct),
        Expr::Any(group) => {
            let children = group.children();
            for (index, dead) in children.iter().enumerate() {
                for (other, covering) in children.iter().enumerate() {
                    // Two equal disjuncts: the second is the dead one.
                    let covered = if dead == covering {
                        other < index
                    } else {
                        index != other && implies(dead, covering)
                    };
                    if covered {
                        return Some((dead.clone(), covering.clone()));
                    }
                }
            }
            children.iter().find_map(dead_disjunct)
        }
    }
}

/// Whether the condition holds for this span.
pub fn holds(condition: &SpanExpr, subject: &SpanSubject<'_>) -> bool {
    condition.eval(&mut |atom| atom.eval(subject)).holds()
}

/// Whether the condition can withhold an answer on a span it does not reject outright: it contains a negation
/// or reads the instrumentation scope. Message claiming treats such a gate as one it cannot compare.
pub fn is_opaque(condition: &SpanExpr) -> bool {
    match condition {
        Expr::Not(_) => true,
        Expr::Atom(
            SpanAtom::ScopeNameEquals { .. }
            | SpanAtom::ScopeNameStartsWith { .. }
            | SpanAtom::ScopeVersionIn { .. },
        ) => true,
        Expr::Atom(_) => false,
        Expr::All(group) | Expr::Any(group) => group.children().iter().any(is_opaque),
    }
}

/// Every positive atom of the condition, for near-miss reports.
pub fn positive_atoms(condition: &SpanExpr) -> Vec<&SpanAtom> {
    match condition {
        Expr::Atom(atom) => vec![atom],
        Expr::Not(_) => Vec::new(),
        Expr::All(group) | Expr::Any(group) => {
            group.children().iter().flat_map(positive_atoms).collect()
        }
    }
}

/// A short rendering of a lowered condition for a diagnostic.
pub fn render(condition: &SpanExpr) -> String {
    match condition {
        Expr::Atom(atom) => format!("{atom:?}"),
        Expr::Not(child) => format!("not({})", render(child)),
        Expr::All(group) => format!(
            "all({})",
            group
                .children()
                .iter()
                .map(render)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Expr::Any(group) => format!(
            "any({})",
            group
                .children()
                .iter()
                .map(render)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}
