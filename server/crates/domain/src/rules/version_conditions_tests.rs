//! The version atom: its truth table, its refusals, the boundary evidence every shipped range is held to, and
//! non-interference - a version decides only through a rule that asks for one.

use std::collections::HashMap;

use super::expr::Expr;
use super::span_conditions::{self, Readable, SpanAtom, SpanExpr, SpanSubject};
use super::versions::{Version, VersionScheme};

/// Every source, so the property tests can mix version atoms with the others.
const EVERYTHING: Readable = Readable {
    span_name: true,
    attributes: true,
    scope: true,
    scope_version: true,
    resource: true,
};

fn lowered(condition: serde_json::Value, readable: Readable) -> Result<SpanExpr, String> {
    let parsed: super::schema::SpanWhere =
        serde_json::from_value(condition).map_err(|error| format!("does not parse: {error}"))?;
    span_conditions::lower(&parsed, readable).map_err(|defect| defect.0)
}

fn range(scheme: &str, at_least: Option<&str>, below: Option<&str>) -> serde_json::Value {
    let mut version = serde_json::json!({"scheme": scheme, "because": "a test"});
    if let Some(low) = at_least {
        version["at_least"] = serde_json::json!(low);
    }
    if let Some(high) = below {
        version["below"] = serde_json::json!(high);
    }
    serde_json::json!({"source": "scope.version", "version": version})
}

fn truth(condition: &SpanExpr, version: Option<&str>, span_name: &str) -> super::expr::Truth {
    let attrs = HashMap::new();
    let subject = SpanSubject {
        span_name,
        attrs: &attrs,
        scope_name: Some("scope"),
        scope_version: version,
        resource: None,
    };
    condition.eval(&mut |atom: &SpanAtom| atom.eval(&subject))
}

/// `[2, 3)` in both schemes, at every position a value can take - and under `not`, where unknown stays
/// unknown, so an absent or malformed version never satisfies a negated range either.
#[test]
fn a_version_range_is_a_half_open_interval_with_unknown_outside_the_versions() {
    use super::expr::Truth::{False as F, True as T, Unknown as U};
    for (scheme, values) in [
        (
            "pep440",
            [
                None,
                Some("not a version"),
                Some("1.9"),
                Some("2"),
                Some("2.5.post1"),
                Some("3.0rc1"),
                Some("3"),
                Some("4"),
            ],
        ),
        (
            "semver",
            [
                None,
                Some("2.0"),
                Some("1.9.9"),
                Some("2.0.0"),
                Some("2.5.0"),
                Some("3.0.0-rc.1"),
                Some("3.0.0"),
                Some("4.0.0"),
            ],
        ),
    ] {
        let both =
            lowered(range(scheme, Some("2"), Some("3")), EVERYTHING).expect("the range lowers");
        let expected = [U, U, F, T, T, T, F, F];
        for (value, want) in values.iter().zip(expected) {
            assert_eq!(
                truth(&both, *value, "s"),
                want,
                "{scheme} {value:?} in [2, 3)"
            );
            let negated = Expr::Not(Box::new(both.clone()));
            let negated_want = match want {
                T => F,
                F => T,
                U => U,
            };
            assert_eq!(
                truth(&negated, *value, "s"),
                negated_want,
                "{scheme} not {value:?}"
            );
        }
        let open_above = lowered(range(scheme, Some("2"), None), EVERYTHING).expect("lowers");
        assert_eq!(
            truth(&open_above, values[7], "s"),
            T,
            "{scheme}: no upper bound"
        );
        let open_below = lowered(range(scheme, None, Some("3")), EVERYTHING).expect("lowers");
        assert_eq!(
            truth(&open_below, values[2], "s"),
            T,
            "{scheme}: no lower bound"
        );
    }
}

#[test]
fn a_version_range_that_cannot_mean_what_it_says_is_refused() {
    let refused =
        |condition: serde_json::Value, readable: Readable| lowered(condition, readable).is_err();
    assert!(!refused(
        range("pep440", Some("6.dev0"), None),
        Readable::PROJECTION
    ));
    for (condition, why) in [
        (
            range("pep440", None, None),
            "no bound holds for every version",
        ),
        (range("pep440", Some("3"), Some("3")), "an empty range"),
        (range("pep440", Some("4"), Some("3")), "a reversed range"),
        (
            range("semver", Some("one"), None),
            "a bound that is not a version",
        ),
        (
            serde_json::json!({"source": "scope.version", "equals": "6.0",
                "version": {"scheme": "pep440", "at_least": "6", "because": "x"}}),
            "a text test beside it reads the version as text",
        ),
        (
            serde_json::json!({"source": "attr:k", "version": {"scheme": "pep440", "at_least": "6", "because": "x"}}),
            "only the scope's version is a version source",
        ),
        (
            serde_json::json!({"source": "scope.version", "version": {"scheme": "pep440", "at_least": "6", "because": " "}}),
            "a range states why no shape test can say this",
        ),
        (
            serde_json::json!({"source": "scope.version", "equals": "6.0"}),
            "the version is read only as a version",
        ),
    ] {
        assert!(
            refused(condition.clone(), Readable::PROJECTION),
            "{why}: {condition}"
        );
    }
    assert!(
        refused(range("pep440", Some("6"), None), Readable::SPAN_AND_SCOPE),
        "a section not given the scope's version cannot ask about it"
    );
    assert!(
        refused(
            serde_json::json!({"source": "attr:k", "exists": true}),
            Readable::PROJECTION
        ),
        "a projection is not given the span's attributes"
    );
}

/// **Boundary evidence for every shipped range.** Each version range in the embedded assets is evaluated just
/// below, at and just above each of its bounds, and with the version absent and malformed. The witnesses are
/// generated and then checked with the comparator, so an asset's bound cannot be one the comparator orders
/// differently from what its author meant without this failing.
#[test]
fn every_shipped_version_range_holds_at_its_boundaries() {
    use super::expr::Truth::{False as F, True as T, Unknown as U};
    let mut ranges = Vec::new();
    fn collect(value: &serde_json::Value, out: &mut Vec<serde_json::Value>) {
        match value {
            serde_json::Value::Object(object) => {
                if object
                    .get("version")
                    .is_some_and(serde_json::Value::is_object)
                    && object.contains_key("source")
                {
                    out.push(value.clone());
                }
                object.values().for_each(|inner| collect(inner, out));
            }
            serde_json::Value::Array(items) => items.iter().for_each(|inner| collect(inner, out)),
            _ => {}
        }
    }
    for bytes in super::schema::embedded_sources().values() {
        collect(
            &serde_json::from_slice(bytes).expect("the asset parses"),
            &mut ranges,
        );
    }
    assert!(!ranges.is_empty(), "the corpus declares a version range");
    for condition in ranges {
        let lowered = lowered(condition.clone(), EVERYTHING).expect("a shipped range lowers");
        let Expr::Atom(SpanAtom::ScopeVersionIn {
            scheme,
            at_least,
            below,
        }) = &lowered
        else {
            panic!("a version condition lowers to one range: {condition}");
        };
        let inside = |text: &str| {
            let version = Version::parse(*scheme, text).expect("a witness is a version");
            at_least
                .as_ref()
                .is_none_or(|low| version.compare(low).is_some_and(|o| o.is_ge()))
                && below
                    .as_ref()
                    .is_none_or(|high| version.compare(high).is_some_and(|o| o.is_lt()))
        };
        for bound in [at_least, below].into_iter().flatten() {
            let text = render(bound);
            let (under, over) = neighbours(*scheme, bound);
            assert_eq!(
                truth(&lowered, Some(&text), "s"),
                if inside(&text) { T } else { F },
                "at {text}"
            );
            if let Some(under) = under {
                assert_eq!(
                    truth(&lowered, Some(&under), "s"),
                    if inside(&under) { T } else { F },
                    "below {text}"
                );
            }
            assert_eq!(
                truth(&lowered, Some(&over), "s"),
                if inside(&over) { T } else { F },
                "above {text}"
            );
        }
        if let Some(low) = at_least {
            let (under, _) = neighbours(*scheme, low);
            if let Some(under) = under {
                assert_eq!(
                    truth(&lowered, Some(&under), "s"),
                    F,
                    "just below the first release"
                );
            }
            assert_eq!(
                truth(&lowered, Some(&render(low)), "s"),
                T,
                "the first release is inside"
            );
        }
        if let Some(high) = below {
            assert_eq!(
                truth(&lowered, Some(&render(high)), "s"),
                F,
                "the first release after is outside"
            );
        }
        assert_eq!(
            truth(&lowered, None, "s"),
            U,
            "absent is unknown: {condition}"
        );
        assert_eq!(
            truth(&lowered, Some("not a version"), "s"),
            U,
            "malformed is unknown: {condition}"
        );
    }
}

fn render(version: &Version) -> String {
    match version {
        Version::Pep440(v) => v.to_string(),
        Version::Semver(v) => v.to_string(),
    }
}

/// A version just below and one just above `bound`, checked with the comparator; `None` below the least
/// version a scheme has.
fn neighbours(scheme: VersionScheme, bound: &Version) -> (Option<String>, String) {
    let text = render(bound);
    let below_candidates: Vec<String> = match scheme {
        VersionScheme::Pep440 => vec![
            format!("{text}.dev0"),
            "0.dev0".to_string(),
            "0".to_string(),
        ]
        .into_iter()
        .chain(
            text.split('.')
                .next()
                .and_then(|major| major.parse::<u64>().ok())
                .filter(|m| *m > 0)
                .map(|major| format!("{}.999999", major - 1)),
        )
        .collect(),
        VersionScheme::Semver => vec![format!("{text}-0"), "0.0.0-0".to_string()],
    };
    let above_candidates: Vec<String> = match scheme {
        VersionScheme::Pep440 => vec![format!("{text}.post1"), format!("{text}1")],
        VersionScheme::Semver => {
            let core = text.split('-').next().unwrap_or(&text);
            let mut parts: Vec<u64> = core.split('.').filter_map(|p| p.parse().ok()).collect();
            parts.resize(3, 0);
            vec![format!("{}.{}.{}", parts[0], parts[1], parts[2] + 1)]
        }
    };
    let ordered = |candidate: &String, wanted: std::cmp::Ordering| {
        Version::parse(scheme, candidate).and_then(|v| v.compare(bound)) == Some(wanted)
    };
    let under = below_candidates
        .into_iter()
        .find(|c| ordered(c, std::cmp::Ordering::Less));
    let over = above_candidates
        .into_iter()
        .find(|c| ordered(c, std::cmp::Ordering::Greater))
        .expect("a version above the bound");
    (under, over)
}

/// **A version decides only through a rule that asks for one.** Over every ordered choice of three rules from a
/// set mixing name tests, version ranges, their conjunction, a negated range and a disjunction, and every pair
/// of spans that differ only in their version - absent, malformed, or a release on either side of the bounds -
/// a rule that reads no version answers alike on both, and where the first rule to hold differs, one of the two
/// winners asks about the version.
#[test]
fn a_version_changes_an_answer_only_through_a_rule_that_reads_it() {
    for scheme in ["pep440", "semver"] {
        let (two, three) = if scheme == "pep440" {
            ("2", "3")
        } else {
            ("2.0.0", "3.0.0")
        };
        let name = serde_json::json!({"source": "span_name", "starts_with": "run"});
        let other_name = serde_json::json!({"source": "span_name", "equals": "other"});
        let at_least = range(scheme, Some(two), None);
        let below = range(scheme, None, Some(three));
        let conditions = [
            name.clone(),
            other_name,
            at_least.clone(),
            serde_json::json!({"all": [name.clone(), at_least.clone()]}),
            serde_json::json!({"not": at_least.clone()}),
            serde_json::json!({"any": [name, below]}),
        ];
        let compiled: Vec<(SpanExpr, bool)> = conditions
            .iter()
            .map(|c| {
                let expr = lowered(c.clone(), EVERYTHING).expect("the condition lowers");
                let gated = c.to_string().contains("scope.version");
                (expr, gated)
            })
            .collect();
        let versions: Vec<Option<&str>> = if scheme == "pep440" {
            vec![None, Some("garbage"), Some("1"), Some("2"), Some("3")]
        } else {
            vec![
                None,
                Some("garbage"),
                Some("1.0.0"),
                Some("2.0.0"),
                Some("3.0.0"),
            ]
        };
        let mut pairs = 0;
        for a in 0..compiled.len() {
            for b in (0..compiled.len()).filter(|b| *b != a) {
                for c in (0..compiled.len()).filter(|c| *c != a && *c != b) {
                    let rules = [&compiled[a], &compiled[b], &compiled[c]];
                    let winner = |version: Option<&str>, span: &str| {
                        rules
                            .iter()
                            .position(|(expr, _)| truth(expr, version, span).holds())
                    };
                    for span in ["run x", "other", "idle"] {
                        for (i, first) in versions.iter().enumerate() {
                            for second in &versions[i + 1..] {
                                pairs += 1;
                                for (expr, gated) in &rules {
                                    if !gated {
                                        assert_eq!(
                                            truth(expr, *first, span),
                                            truth(expr, *second, span)
                                        );
                                    }
                                }
                                let (w1, w2) = (winner(*first, span), winner(*second, span));
                                if w1 != w2 {
                                    let gated = |w: Option<usize>| w.is_some_and(|w| rules[w].1);
                                    assert!(
                                        gated(w1) || gated(w2),
                                        "{scheme}: rules {a},{b},{c} on `{span}`: {first:?} -> {w1:?}, {second:?} -> {w2:?}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(pairs > 1000, "{pairs} span pairs were compared");
    }
}
