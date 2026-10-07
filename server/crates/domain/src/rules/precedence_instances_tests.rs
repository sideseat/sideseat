//! `checked_precedence_instances`: the production compilers held to `server/specs/CheckedPrecedence.tla`.
//!
//! The model and this test read one manifest, `server/specs/instances/CheckedPrecedence.json`. Each abstract
//! instance - clauses with priorities, `supersedes` edges and the set of spans each matches - is realised as a
//! real asset: span `s` carries the attribute `k.s`, and a clause matching spans `S` is a detection rule (or an
//! observation-type rule) whose condition is `attr_exists` over `k.s` for `s` in `S`. Compiling and resolving it
//! through the engine must give the manifest's expected diagnostic and winners, the retired resolution must give
//! its retired winners, and over the manifest's bounded space the engine must agree with the model's definitions,
//! which this file restates in Rust: what the compiler refuses, and who wins.
//!
//! The section of the specification between its `GENERATED` markers is rendered from the manifest here, and the
//! test fails while the committed one differs (`UPDATE_SPECS=1` rewrites it), so the model checks exactly the
//! instances this test does.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::Deserialize;

use super::assets::ParsedAssets;
use super::detect_rules::{DetectCompileError, DetectContext, DetectPlan};

#[derive(Debug, Deserialize)]
struct Manifest {
    #[allow(dead_code)]
    doc: String,
    version: u32,
    bounds: Bounds,
    instances: Vec<Instance>,
}

#[derive(Debug, Deserialize)]
struct Bounds {
    clauses: Vec<String>,
    spans: Vec<String>,
    priorities: Vec<i32>,
    max_edges: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct Instance {
    id: String,
    spans: Vec<String>,
    clauses: Vec<Clause>,
    diagnostic: String,
    winners: BTreeMap<String, Option<String>>,
    retired_winners: BTreeMap<String, Option<String>>,
}

#[derive(Debug, Clone, Deserialize)]
struct Clause {
    id: String,
    priority: i32,
    supersedes: Vec<String>,
    matches: Vec<String>,
}

fn repository() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn manifest() -> Manifest {
    let path = repository().join("server/specs/instances/CheckedPrecedence.json");
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

// ----------------------------------------------------------------------------
// The model's definitions, restated.
// ----------------------------------------------------------------------------

fn shared_priority(instance: &Instance) -> bool {
    instance.clauses.iter().enumerate().any(|(index, a)| {
        instance.clauses[index + 1..]
            .iter()
            .any(|b| a.priority == b.priority)
    })
}

fn edges(instance: &Instance) -> Vec<(&str, &str)> {
    instance
        .clauses
        .iter()
        .flat_map(|clause| {
            clause
                .supersedes
                .iter()
                .map(move |target| (clause.id.as_str(), target.as_str()))
        })
        .collect()
}

fn priority_of(instance: &Instance, id: &str) -> Option<i32> {
    instance
        .clauses
        .iter()
        .find(|clause| clause.id == id)
        .map(|clause| clause.priority)
}

fn edge_defect(instance: &Instance) -> bool {
    let all = edges(instance);
    all.iter().enumerate().any(|(index, (source, target))| {
        source == target
            || all[..index].contains(&(source, target))
            || match (priority_of(instance, source), priority_of(instance, target)) {
                (Some(from), Some(to)) => to <= from,
                _ => true,
            }
    })
}

fn shadow_pairs(instance: &Instance) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    for earlier in &instance.clauses {
        for later in &instance.clauses {
            if earlier.id != later.id
                && earlier.priority < later.priority
                && later
                    .matches
                    .iter()
                    .all(|span| earlier.matches.contains(span))
            {
                out.push((earlier.id.as_str(), later.id.as_str()));
            }
        }
    }
    out
}

fn model_diagnostic(instance: &Instance) -> &'static str {
    if shared_priority(instance) {
        "DuplicatePriority"
    } else if edge_defect(instance) {
        "UselessSupersedes"
    } else if !shadow_pairs(instance).is_empty() {
        "ShadowedRule"
    } else {
        "none"
    }
}

fn matching<'a>(instance: &'a Instance, span: &str) -> Vec<&'a Clause> {
    instance
        .clauses
        .iter()
        .filter(|clause| clause.matches.iter().any(|s| s == span))
        .collect()
}

fn lowest<'a>(clauses: impl Iterator<Item = &'a Clause>) -> Option<String> {
    clauses
        .min_by_key(|clause| clause.priority)
        .map(|c| c.id.clone())
}

fn model_winner(instance: &Instance, span: &str) -> Option<String> {
    lowest(matching(instance, span).into_iter())
}

fn model_retired(instance: &Instance, span: &str) -> Option<String> {
    let all = edges(instance);
    let found = matching(instance, span);
    let mut beaten: BTreeSet<&str> = BTreeSet::new();
    for clause in &found {
        let mut queue: Vec<&str> = vec![clause.id.as_str()];
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        while let Some(current) = queue.pop() {
            for (source, target) in &all {
                if *source == current && seen.insert(target) {
                    beaten.insert(target);
                    queue.push(target);
                }
            }
        }
    }
    let unbeaten: Vec<&Clause> = found
        .iter()
        .copied()
        .filter(|clause| !beaten.contains(clause.id.as_str()))
        .collect();
    if unbeaten.is_empty() {
        lowest(found.into_iter())
    } else {
        lowest(unbeaten.into_iter())
    }
}

// ----------------------------------------------------------------------------
// The engine, over a realised instance.
// ----------------------------------------------------------------------------

fn key_of(span: &str) -> String {
    format!("k.{span}")
}

/// "Some of these attributes is present", as a condition.
fn exists_any(keys: Vec<String>) -> serde_json::Value {
    let atoms: Vec<serde_json::Value> = keys
        .into_iter()
        .map(|key| serde_json::json!({"source": format!("attr:{key}"), "exists": true}))
        .collect();
    if atoms.len() == 1 {
        atoms.into_iter().next().expect("one atom")
    } else {
        serde_json::json!({ "any": atoms })
    }
}

/// The detection rule realising one clause. A clause matching nothing reads a key no span carries.
fn detect_rule(clause: &Clause, with_edges: bool) -> serde_json::Value {
    let mut keys: Vec<String> = clause.matches.iter().map(|span| key_of(span)).collect();
    if keys.is_empty() {
        keys.push(format!("never.{}", clause.id));
    }
    serde_json::json!({
        "id": clause.id,
        "label": clause.id,
        "priority": clause.priority,
        "supersedes": if with_edges { clause.supersedes.clone() } else { Vec::new() },
        "where": exists_any(keys),
    })
}

/// The instance's clauses as assets, in the given order, one file each when `split`.
fn assets(rules: Vec<serde_json::Value>, section: &str, split: bool) -> ParsedAssets {
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    if split {
        for (index, rule) in rules.into_iter().enumerate() {
            let asset = serde_json::json!({"id": format!("probe{index}"), section: [rule]});
            files.insert(
                format!("probe{index}.json"),
                serde_json::to_vec(&asset).expect("the probe serialises"),
            );
        }
    } else {
        let asset = serde_json::json!({"id": "probe", section: rules});
        files.insert(
            "probe.json".to_string(),
            serde_json::to_vec(&asset).expect("the probe serialises"),
        );
    }
    ParsedAssets::parse(&files).expect("the realised instance parses")
}

fn compile_detect(
    instance: &Instance,
    with_edges: bool,
    reversed: bool,
    split: bool,
) -> Result<DetectPlan, DetectCompileError> {
    let mut rules: Vec<serde_json::Value> = instance
        .clauses
        .iter()
        .map(|clause| detect_rule(clause, with_edges))
        .collect();
    if reversed {
        rules.reverse();
    }
    super::detect_rules::compile(&assets(rules, "detect", split))
}

fn detect_diagnostic(result: &Result<DetectPlan, DetectCompileError>) -> &'static str {
    match result {
        Ok(_) => "none",
        Err(DetectCompileError::DuplicatePriority { .. }) => "DuplicatePriority",
        Err(DetectCompileError::UselessSupersedes { .. }) => "UselessSupersedes",
        Err(DetectCompileError::ShadowedRule { .. }) => "ShadowedRule",
        Err(other) => panic!("a realised instance met an unrelated refusal: {other}"),
    }
}

fn span_attrs(span: &str) -> HashMap<String, String> {
    HashMap::from([(key_of(span), "1".to_string())])
}

fn engine_winner(plan: &DetectPlan, span: &str) -> Option<String> {
    let attrs = span_attrs(span);
    let none = HashMap::new();
    plan.resolve(&DetectContext {
        span_name: "probe",
        scope_name: None,
        span_attrs: &attrs,
        resource_attrs: &none,
    })
    .map(|rule| rule.label.clone())
}

fn engine_retired(plan: &DetectPlan, instance: &Instance, span: &str) -> Option<String> {
    let table: Vec<(&str, i32, Vec<&str>)> = instance
        .clauses
        .iter()
        .map(|clause| {
            (
                clause.id.as_str(),
                clause.priority,
                clause.supersedes.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    let table: Vec<(&str, i32, &[&str])> = table
        .iter()
        .map(|(id, priority, edges)| (*id, *priority, edges.as_slice()))
        .collect();
    let attrs = span_attrs(span);
    let none = HashMap::new();
    plan.retired_resolve(
        &DetectContext {
            span_name: "probe",
            scope_name: None,
            span_attrs: &attrs,
            resource_attrs: &none,
        },
        &table,
        &[],
    )
    .map(str::to_string)
}

/// The observation-type rules realising an instance without edges: per span, the clause whose id the evidence
/// names, or the refusal.
fn classify_winners(instance: &Instance) -> Result<Vec<Option<String>>, &'static str> {
    let rules: Vec<serde_json::Value> = instance
        .clauses
        .iter()
        .map(|clause| {
            let mut keys: Vec<String> = clause.matches.iter().map(|s| key_of(s)).collect();
            if keys.is_empty() {
                keys.push(format!("never.{}", clause.id));
            }
            serde_json::json!({
                "id": clause.id,
                "priority": clause.priority,
                "where": exists_any(keys),
                "result": "span",
            })
        })
        .collect();
    match super::classify::compile(&assets(rules, "observation_types", false)) {
        Ok(plan) => Ok(instance
            .spans
            .iter()
            .map(|span| {
                plan.observation_type("probe", &span_attrs(span))
                    .map(|verdict| verdict.evidence.paths()[0].root.clone())
            })
            .collect()),
        Err(super::classify::ClassifyCompileError::SharedPriority { .. }) => {
            Err("DuplicatePriority")
        }
        Err(super::classify::ClassifyCompileError::ShadowedRule { .. }) => Err("ShadowedRule"),
        Err(other) => panic!("a realised classification met an unrelated refusal: {other}"),
    }
}

/// One instance, checked against the model's definitions and, where given, the manifest's expectations.
fn check(instance: &Instance, expected: Option<&Instance>) {
    let what = &instance.id;
    let model = model_diagnostic(instance);
    if let Some(expected) = expected {
        assert_eq!(model, expected.diagnostic, "{what}: the model's diagnostic");
        for (span, winner) in &expected.winners {
            assert_eq!(
                &model_winner(instance, span),
                winner,
                "{what}/{span}: the model's winner"
            );
        }
        for (span, winner) in &expected.retired_winners {
            assert_eq!(
                &model_retired(instance, span),
                winner,
                "{what}/{span}: the model's retired winner"
            );
        }
    }

    let compiled = compile_detect(instance, true, false, false);
    let engine = detect_diagnostic(&compiled);
    match model {
        // Exact where the check is complete.
        "DuplicatePriority" | "UselessSupersedes" => {
            assert_eq!(engine, model, "{what}: the engine's diagnostic");
        }
        // The shadow check is sound and incomplete: it may accept a clause the model shadows (an empty match set
        // is covered by everything, a key no span carries is not), never the reverse.
        "ShadowedRule" => assert!(
            matches!(engine, "ShadowedRule" | "none"),
            "{what}: the engine refused {engine} where the model shadows"
        ),
        _ => assert_eq!(
            engine, "none",
            "{what}: the engine refused an instance the model accepts"
        ),
    }
    if let Some(expected) = expected
        && matches!(
            expected.diagnostic.as_str(),
            "none" | "ShadowedRule" | "UselessSupersedes"
        )
    {
        // A refused instance has no plan; where the refusal is the edge, the plan without edges is the one the
        // priorities alone describe, and both resolutions are asked of it.
        if let Ok(plan) = compile_detect(instance, false, false, false) {
            for (span, winner) in &expected.winners {
                assert_eq!(
                    &engine_winner(&plan, span),
                    winner,
                    "{what}/{span}: the engine's winner"
                );
            }
            for (span, winner) in &expected.retired_winners {
                assert_eq!(
                    &engine_retired(&plan, instance, span),
                    winner,
                    "{what}/{span}: the engine's retired winner"
                );
            }
        }
    }

    if engine == "none" {
        let plan = compiled.expect("accepted");
        for span in &instance.spans {
            let winner = model_winner(instance, span);
            assert_eq!(
                engine_winner(&plan, span),
                winner,
                "{what}/{span}: priority order"
            );
            assert_eq!(
                engine_retired(&plan, instance, span),
                winner,
                "{what}/{span}: an accepted instance's edges are inert"
            );
            assert_eq!(
                model_retired(instance, span),
                winner,
                "{what}/{span}: the model agrees"
            );
        }
        // Load order and file boundaries decide nothing. Asked of the manifest's cases and of the edge-free part of
        // the space, which is where the priorities alone decide.
        let orderings: &[(bool, bool)] = if expected.is_some() || edges(instance).is_empty() {
            &[(true, false), (false, true), (true, true)]
        } else {
            &[]
        };
        for &(reversed, split) in orderings {
            let other = compile_detect(instance, true, reversed, split)
                .unwrap_or_else(|e| panic!("{what}: a reordering was refused: {e}"));
            for span in &instance.spans {
                assert_eq!(
                    engine_winner(&other, span),
                    engine_winner(&plan, span),
                    "{what}/{span}: load order changed the answer"
                );
            }
        }
    }

    // The classification compiler, which has no edges, over the same clauses.
    if edges(instance).is_empty() {
        match classify_winners(instance) {
            Ok(winners) => {
                assert!(
                    !shared_priority(instance),
                    "{what}: classification accepted a shared priority"
                );
                for (span, winner) in instance.spans.iter().zip(winners) {
                    assert_eq!(
                        winner,
                        model_winner(instance, span),
                        "{what}/{span}: classification"
                    );
                }
            }
            Err("DuplicatePriority") => assert!(shared_priority(instance), "{what}"),
            Err(_) => assert!(
                !shadow_pairs(instance).is_empty(),
                "{what}: classification refused a shadow the model does not see"
            ),
        }
    }
}

/// Every instance of the bounded space with at most one edge (the model also checks two).
fn enumerated(bounds: &Bounds) -> Vec<Instance> {
    let ids = &bounds.clauses;
    let spans = &bounds.spans;
    let mut priorities: Vec<Vec<i32>> = vec![Vec::new()];
    for _ in ids {
        priorities = priorities
            .into_iter()
            .flat_map(|prefix| {
                bounds.priorities.iter().map(move |p| {
                    let mut next = prefix.clone();
                    next.push(*p);
                    next
                })
            })
            .collect();
    }
    let subsets: Vec<Vec<String>> = (0..(1_u32 << spans.len()))
        .map(|mask| {
            spans
                .iter()
                .enumerate()
                .filter(|(bit, _)| mask & (1 << bit) != 0)
                .map(|(_, span)| span.clone())
                .collect()
        })
        .collect();
    let mut tables: Vec<Vec<Vec<String>>> = vec![Vec::new()];
    for _ in ids {
        tables = tables
            .into_iter()
            .flat_map(|prefix| {
                subsets.iter().map(move |subset| {
                    let mut next = prefix.clone();
                    next.push(subset.clone());
                    next
                })
            })
            .collect();
    }
    let mut edge_lists: Vec<Option<(usize, usize)>> = vec![None];
    for source in 0..ids.len() {
        for target in 0..ids.len() {
            edge_lists.push(Some((source, target)));
        }
    }
    let mut out = Vec::new();
    for priority in &priorities {
        for table in &tables {
            for edge in &edge_lists {
                out.push(Instance {
                    id: format!("enumerated {priority:?} {table:?} {edge:?}"),
                    spans: spans.clone(),
                    clauses: ids
                        .iter()
                        .enumerate()
                        .map(|(index, id)| Clause {
                            id: id.clone(),
                            priority: priority[index],
                            supersedes: match edge {
                                Some((source, target)) if *source == index => {
                                    vec![ids[*target].clone()]
                                }
                                _ => Vec::new(),
                            },
                            matches: table[index].clone(),
                        })
                        .collect(),
                    diagnostic: String::new(),
                    winners: BTreeMap::new(),
                    retired_winners: BTreeMap::new(),
                });
            }
        }
    }
    out
}

// ----------------------------------------------------------------------------
// The specification's generated section.
// ----------------------------------------------------------------------------

fn tla_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn tla_set<'a>(items: impl IntoIterator<Item = &'a String>) -> String {
    let items: Vec<String> = items.into_iter().map(|item| tla_string(item)).collect();
    format!("{{{}}}", items.join(", "))
}

fn tla_function(pairs: Vec<(String, String)>) -> String {
    if pairs.is_empty() {
        return "<<>>".to_string();
    }
    let parts: Vec<String> = pairs
        .into_iter()
        .map(|(key, value)| format!("{key} :> {value}"))
        .collect();
    format!("({})", parts.join(" @@ "))
}

fn tla_winners(winners: &BTreeMap<String, Option<String>>) -> String {
    tla_function(
        winners
            .iter()
            .map(|(span, winner)| {
                (
                    tla_string(span),
                    tla_string(winner.as_deref().unwrap_or("none")),
                )
            })
            .collect(),
    )
}

fn generated_section(manifest: &Manifest) -> String {
    let bounds = &manifest.bounds;
    let priorities: Vec<String> = bounds.priorities.iter().map(i32::to_string).collect();
    let mut out = String::new();
    out.push_str("\\* BEGIN GENERATED FROM instances/CheckedPrecedence.json\n");
    out.push_str(&format!("ManifestIds == {}\n", tla_set(&bounds.clauses)));
    out.push_str(&format!("ManifestSpans == {}\n", tla_set(&bounds.spans)));
    out.push_str(&format!(
        "ManifestPriorities == {{{}}}\n",
        priorities.join(", ")
    ));
    out.push_str(&format!("ManifestMaxEdges == {}\n", bounds.max_edges));
    out.push_str("ManifestInstances == {\n");
    let records: Vec<String> = manifest
        .instances
        .iter()
        .map(|instance| {
            let ids: Vec<String> = instance.clauses.iter().map(|c| c.id.clone()).collect();
            let prio = tla_function(
                instance
                    .clauses
                    .iter()
                    .map(|c| (tla_string(&c.id), c.priority.to_string()))
                    .collect(),
            );
            let matches = tla_function(
                instance
                    .clauses
                    .iter()
                    .map(|c| (tla_string(&c.id), tla_set(&c.matches)))
                    .collect(),
            );
            let edges: Vec<String> = edges(instance)
                .into_iter()
                .map(|(a, b)| format!("<<{}, {}>>", tla_string(a), tla_string(b)))
                .collect();
            // Spaced, because `<<<<` does not lex as two openings.
            let edges = if edges.is_empty() {
                "<<>>".to_string()
            } else {
                format!("<< {} >>", edges.join(", "))
            };
            format!(
                "    [id |-> {},\n     ids |-> {},\n     spans |-> {},\n     prio |-> {},\n     edges |-> {},\n     \
                 matches |-> {},\n     diagnostic |-> {},\n     winners |-> {},\n     retired |-> {}]",
                tla_string(&instance.id),
                tla_set(&ids),
                tla_set(&instance.spans),
                prio,
                edges,
                matches,
                tla_string(&instance.diagnostic),
                tla_winners(&instance.winners),
                tla_winners(&instance.retired_winners),
            )
        })
        .collect();
    out.push_str(&records.join(",\n"));
    out.push_str("\n}\n");
    out.push_str("\\* END GENERATED FROM instances/CheckedPrecedence.json\n");
    out
}

#[test]
fn checked_precedence_instances() {
    let manifest = manifest();

    // The specification checks exactly these instances.
    let path = repository().join("server/specs/CheckedPrecedence.tla");
    let spec = std::fs::read_to_string(&path).expect("the specification is readable");
    let begin = spec
        .find("\\* BEGIN GENERATED FROM")
        .expect("the specification has its generated section");
    let end_marker = "\\* END GENERATED FROM instances/CheckedPrecedence.json\n";
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

    for instance in &manifest.instances {
        check(instance, Some(instance));
    }
    let space = enumerated(&manifest.bounds);
    // Every refusal and every acceptance is reached, or the space says nothing about them.
    let reached: BTreeSet<&str> = space.iter().map(model_diagnostic).collect();
    assert_eq!(
        reached,
        BTreeSet::from([
            "DuplicatePriority",
            "ShadowedRule",
            "UselessSupersedes",
            "none"
        ]),
        "the bounded space must reach every diagnostic"
    );
    for instance in &space {
        check(instance, None);
    }
}

#[test]
fn the_retired_resolution_is_not_a_priority_order() {
    // The reason `supersedes` could not stay executable: under it, three clauses formed a cycle of pairwise
    // preferences, so no priority function reproduces every answer. Checked over the manifest's own case, so
    // the model and this test talk about the same instance.
    let manifest = manifest();
    let cycle = manifest
        .instances
        .iter()
        .find(|instance| instance.id == "the_retired_cycle_of_preferences")
        .expect("the manifest keeps the cycle");
    let ids: Vec<&str> = cycle.clauses.iter().map(|c| c.id.as_str()).collect();
    let mut orders: Vec<Vec<&str>> = vec![Vec::new()];
    for _ in &ids {
        let mut grown = Vec::new();
        for prefix in &orders {
            for id in ids.iter().filter(|id| !prefix.contains(id)) {
                let mut next = prefix.clone();
                next.push(*id);
                grown.push(next);
            }
        }
        orders = grown;
    }
    assert_eq!(orders.len(), 6);
    let reproduces = |order: &Vec<&str>| {
        cycle.spans.iter().all(|span| {
            let winner = order
                .iter()
                .find(|id| {
                    cycle
                        .clauses
                        .iter()
                        .any(|c| c.id == **id && c.matches.contains(span))
                })
                .map(|id| id.to_string());
            winner == model_retired(cycle, span)
        })
    };
    assert!(
        !orders.iter().any(reproduces),
        "some total order reproduces the retired answers, so the cycle case proves nothing"
    );
}
