//! A version range's atom: alone, over the scope's version, with bounds that parse and leave room.

use super::{ConditionDefect, Readable, Source, SpanAtom, SpanExpr, parse_source};
use crate::rules::expr::Expr;
use crate::rules::schema::{ConditionSource, SpanCondition};

/// A version range, alone in its atom and over the scope's version, with bounds that parse and leave room.
pub(super) fn lower_version(
    atom: &SpanCondition,
    range: &crate::rules::schema::VersionRange,
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
        Some(text) => crate::rules::versions::Version::parse_bound(range.scheme, text)
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
