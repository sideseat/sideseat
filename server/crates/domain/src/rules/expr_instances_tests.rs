//! `three_valued_predicate_instances`: the production `Expr::eval` held to `server/specs/ThreeValuedPredicate.tla`.
//!
//! One manifest, `server/specs/instances/ThreeValuedPredicate.json`, holds the strong-Kleene tables written by
//! hand, the bounds of the expression space, and single conditions with the answer each must give. The model
//! checks its operators against the tables and the laws over every assignment; this test checks the production
//! evaluator against the same tables, the same laws over every expression tree within the bounds (n-ary groups
//! and early exits included), and each condition through the production lowering and evaluators. The section of
//! the specification between its `GENERATED` markers is rendered from the manifest here, and the test fails
//! while it is stale (`UPDATE_SPECS=1` rewrites it).

use std::collections::{BTreeMap, HashMap};

use serde::Deserialize;

use super::expr::{Expr, Truth};

#[derive(Debug, Deserialize)]
struct Manifest {
    #[allow(dead_code)]
    doc: String,
    version: u32,
    bounds: Bounds,
    tables: Tables,
    span_atoms: Vec<SpanCase>,
    json_atoms: Vec<JsonCase>,
}

#[derive(Debug, Deserialize)]
struct Bounds {
    depth: usize,
    leaves: usize,
}

#[derive(Debug, Deserialize)]
struct Tables {
    not: BTreeMap<String, String>,
    all: BTreeMap<String, String>,
    any: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct SpanCase {
    id: String,
    #[serde(rename = "where")]
    condition: serde_json::Value,
    attrs: HashMap<String, String>,
    expect: String,
}

#[derive(Debug, Deserialize)]
struct JsonCase {
    id: String,
    #[serde(rename = "where")]
    condition: serde_json::Value,
    value: serde_json::Value,
    expect: String,
}

fn repository() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn manifest() -> Manifest {
    let path = repository().join("server/specs/instances/ThreeValuedPredicate.json");
    let manifest: Manifest = serde_json::from_slice(
        &std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display())),
    )
    .unwrap_or_else(|e| panic!("decode {}: {e}", path.display()));
    assert_eq!(
        manifest.version, 1,
        "a new manifest version needs this reader updated"
    );
    manifest
}

fn letter(truth: Truth) -> &'static str {
    match truth {
        Truth::True => "T",
        Truth::False => "F",
        Truth::Unknown => "U",
    }
}

fn truth(letter: &str) -> Truth {
    match letter {
        "T" => Truth::True,
        "F" => Truth::False,
        "U" => Truth::Unknown,
        other => panic!("`{other}` is not a truth value"),
    }
}

const VALUES: [Truth; 3] = [Truth::True, Truth::False, Truth::Unknown];

/// An expression over leaf indices, evaluated against an assignment of truth values to the leaves.
fn eval(expr: &Expr<usize>, assignment: &[Truth]) -> Truth {
    expr.eval(&mut |leaf| assignment[*leaf])
}

/// The tables' fold of a group, child by child, with no early exit: the eager reference.
fn eager(op: &BTreeMap<String, String>, children: &[Truth], unit: Truth) -> Truth {
    children.iter().fold(unit, |acc, child| {
        truth(&op[&format!("{}{}", letter(acc), letter(*child))])
    })
}

fn not(e: Expr<usize>) -> Expr<usize> {
    Expr::Not(Box::new(e))
}

fn all(children: Vec<Expr<usize>>) -> Expr<usize> {
    Expr::all(children).expect("two or more children")
}

fn any(children: Vec<Expr<usize>>) -> Expr<usize> {
    Expr::any(children).expect("two or more children")
}

/// Every tree of at most `depth` operator levels over the leaves: atoms, negations, and binary groups.
fn trees(depth: usize, leaves: usize) -> Vec<Expr<usize>> {
    let mut level: Vec<Expr<usize>> = (0..leaves).map(Expr::Atom).collect();
    for _ in 0..depth {
        let mut next = level.clone();
        for e in &level {
            next.push(not(e.clone()));
        }
        for x in &level {
            for y in &level {
                next.push(all(vec![x.clone(), y.clone()]));
                next.push(any(vec![x.clone(), y.clone()]));
            }
        }
        level = next;
    }
    level
}

fn generated_section(manifest: &Manifest) -> String {
    let function = |table: &BTreeMap<String, String>, order: &[&str]| -> String {
        let parts: Vec<String> = order
            .iter()
            .map(|key| format!("\"{key}\" :> \"{}\"", table[*key]))
            .collect();
        format!("({})", parts.join(" @@ "))
    };
    let pairs: Vec<String> = ["T", "F", "U"]
        .iter()
        .flat_map(|a| ["T", "F", "U"].map(|b| format!("{a}{b}")))
        .collect();
    let pairs: Vec<&str> = pairs.iter().map(String::as_str).collect();
    let mut out = String::new();
    out.push_str("\\* BEGIN GENERATED FROM instances/ThreeValuedPredicate.json\n");
    out.push_str(&format!("ManifestDepth == {}\n", manifest.bounds.depth));
    out.push_str(&format!("ManifestLeaves == {}\n", manifest.bounds.leaves));
    out.push_str(&format!(
        "TableNot == {}\n",
        function(&manifest.tables.not, &["T", "F", "U"])
    ));
    out.push_str(&format!(
        "TableAll == {}\n",
        function(&manifest.tables.all, &pairs)
    ));
    out.push_str(&format!(
        "TableAny == {}\n",
        function(&manifest.tables.any, &pairs)
    ));
    out.push_str("\\* END GENERATED FROM instances/ThreeValuedPredicate.json\n");
    out
}

#[test]
fn three_valued_predicate_instances() {
    let manifest = manifest();

    // The specification checks exactly these tables and bounds.
    let path = repository().join("server/specs/ThreeValuedPredicate.tla");
    let spec = std::fs::read_to_string(&path).expect("the specification is readable");
    let begin = spec
        .find("\\* BEGIN GENERATED FROM")
        .expect("the specification has its generated section");
    let end_marker = "\\* END GENERATED FROM instances/ThreeValuedPredicate.json\n";
    let end = spec[begin..]
        .find(end_marker)
        .map(|at| begin + at + end_marker.len())
        .expect("the generated section is closed");
    let rendered = generated_section(&manifest);
    if std::env::var_os("UPDATE_SPECS").is_some() {
        let updated = format!("{}{rendered}{}", &spec[..begin], &spec[end..]);
        std::fs::write(&path, updated).expect("the specification is writable");
    } else {
        assert_eq!(
            &spec[begin..end],
            rendered,
            "the specification's generated section is stale; rerun with UPDATE_SPECS=1"
        );
    }

    // The production operators reproduce the hand-written tables.
    let tables = &manifest.tables;
    for a in VALUES {
        assert_eq!(
            letter(eval(&not(Expr::Atom(0)), &[a])),
            tables.not[letter(a)]
        );
        for b in VALUES {
            let key = format!("{}{}", letter(a), letter(b));
            let leaves = [a, b];
            let pair = || vec![Expr::Atom(0), Expr::Atom(1)];
            assert_eq!(
                letter(eval(&all(pair()), &leaves)),
                tables.all[&key],
                "all {key}"
            );
            assert_eq!(
                letter(eval(&any(pair()), &leaves)),
                tables.any[&key],
                "any {key}"
            );
        }
    }

    // Every group of every arity in bounds answers as the eager fold, so early exits change nothing.
    let mut groups: Vec<Vec<Truth>> = vec![Vec::new()];
    for _ in 0..manifest.bounds.leaves {
        groups = groups
            .iter()
            .flat_map(|prefix| {
                VALUES.iter().map(move |value| {
                    let mut next = prefix.clone();
                    next.push(*value);
                    next
                })
            })
            .chain(groups.iter().cloned())
            .collect();
    }
    groups.sort_by_key(|g| g.iter().map(|t| letter(*t)).collect::<String>());
    groups.dedup();
    for group in groups.iter().filter(|g| g.len() >= 2) {
        let atoms: Vec<Expr<usize>> = (0..group.len()).map(Expr::Atom).collect();
        assert_eq!(
            eval(&all(atoms.clone()), group),
            eager(&tables.all, group, Truth::True),
            "all over {group:?}"
        );
        assert_eq!(
            eval(&any(atoms), group),
            eager(&tables.any, group, Truth::False),
            "any over {group:?}"
        );
    }

    // The laws, over every pair and triple of trees within bounds and every assignment of the leaves.
    let leaves = manifest.bounds.leaves;
    let small = trees(manifest.bounds.depth - 1, leaves);
    let mut assignments: Vec<Vec<Truth>> = vec![Vec::new()];
    for _ in 0..leaves {
        assignments = assignments
            .iter()
            .flat_map(|prefix| {
                VALUES.iter().map(move |value| {
                    let mut next = prefix.clone();
                    next.push(*value);
                    next
                })
            })
            .collect();
    }
    let mut checked = 0_usize;
    for x in &small {
        for y in &small {
            for v in &assignments {
                let e = |expr: &Expr<usize>| eval(expr, v);
                assert_eq!(
                    e(&all(vec![x.clone(), y.clone()])),
                    e(&all(vec![y.clone(), x.clone()]))
                );
                assert_eq!(
                    e(&any(vec![x.clone(), y.clone()])),
                    e(&any(vec![y.clone(), x.clone()]))
                );
                assert_eq!(
                    e(&not(all(vec![x.clone(), y.clone()]))),
                    e(&any(vec![not(x.clone()), not(y.clone())])),
                    "De Morgan"
                );
                assert_eq!(
                    e(&not(any(vec![x.clone(), y.clone()]))),
                    e(&all(vec![not(x.clone()), not(y.clone())])),
                    "De Morgan"
                );
                assert_eq!(
                    e(&all(vec![x.clone(), any(vec![x.clone(), y.clone()])])),
                    e(x),
                    "absorption"
                );
                assert_eq!(
                    e(&any(vec![x.clone(), all(vec![x.clone(), y.clone()])])),
                    e(x),
                    "absorption"
                );
                assert_eq!(e(&not(not(x.clone()))), e(x), "double negation");
                assert_eq!(
                    e(&any(vec![x.clone(), not(x.clone())])) != Truth::True,
                    e(x) == Truth::Unknown,
                    "excluded middle fails exactly on unknown"
                );
                checked += 1;
            }
        }
    }
    let three = trees(1, leaves);
    for x in &three {
        for y in &three {
            for z in &three {
                for v in &assignments {
                    let e = |expr: &Expr<usize>| eval(expr, v);
                    let (x, y, z) = (x.clone(), y.clone(), z.clone());
                    let flat = e(&all(vec![x.clone(), y.clone(), z.clone()]));
                    assert_eq!(
                        flat,
                        e(&all(vec![x.clone(), all(vec![y.clone(), z.clone()])]))
                    );
                    assert_eq!(
                        flat,
                        e(&all(vec![all(vec![x.clone(), y.clone()]), z.clone()]))
                    );
                    let flat = e(&any(vec![x.clone(), y.clone(), z.clone()]));
                    assert_eq!(
                        flat,
                        e(&any(vec![x.clone(), any(vec![y.clone(), z.clone()])]))
                    );
                    assert_eq!(
                        e(&all(vec![x.clone(), any(vec![y.clone(), z.clone()])])),
                        e(&any(vec![
                            all(vec![x.clone(), y.clone()]),
                            all(vec![x.clone(), z.clone()])
                        ])),
                        "distributivity"
                    );
                    assert_eq!(
                        e(&any(vec![x.clone(), all(vec![y.clone(), z.clone()])])),
                        e(&all(vec![
                            any(vec![x.clone(), y.clone()]),
                            any(vec![x.clone(), z.clone()])
                        ])),
                        "distributivity"
                    );
                    checked += 1;
                }
            }
        }
    }
    assert!(
        checked > 100_000,
        "only {checked} law instances were checked"
    );

    // Single span conditions, through the production lowering, with the sources detection is given.
    for case in &manifest.span_atoms {
        let condition: super::schema::SpanWhere = serde_json::from_value(case.condition.clone())
            .unwrap_or_else(|e| panic!("{}: the condition parses: {e}", case.id));
        let lowered =
            super::span_conditions::lower(&condition, super::span_conditions::Readable::ALL)
                .unwrap_or_else(|e| panic!("{}: the condition lowers: {e}", case.id));
        let none = HashMap::new();
        let subject = super::span_conditions::SpanSubject {
            span_name: "probe",
            attrs: &case.attrs,
            scope_name: None,
            resource: Some(&none),
            scope_version: None,
        };
        let answer = lowered.eval(&mut |atom| atom.eval(&subject));
        assert_eq!(letter(answer), case.expect, "{}", case.id);
    }
    // Single value conditions, through the production value-predicate evaluator.
    for case in &manifest.json_atoms {
        let set: super::schema::ValueCondition = serde_json::from_value(case.condition.clone())
            .unwrap_or_else(|e| panic!("{}: the condition parses: {e}", case.id));
        let answer = set
            .expression()
            .expect("the condition is not empty")
            .eval(&mut |atom| atom.eval(&case.value));
        assert_eq!(letter(answer), case.expect, "{}", case.id);
    }
}
