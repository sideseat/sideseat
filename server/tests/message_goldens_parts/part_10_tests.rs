// ============================================================================
// Equivalence oracle: detection by priority against the retired supersedes-ordered resolution
// ============================================================================

include!("retired_detect_order.rs");

/// What detection reads of one span: its name, its scope's name, its attributes and its resource's.
type DetectInput = (
    String,
    Option<String>,
    HashMap<String, String>,
    HashMap<String, String>,
);

/// The detection context of one captured span, as ingestion builds it.
fn detect_inputs(request: &ExportTraceServiceRequest) -> Vec<DetectInput> {
    use sideseat_ingestion::otlp::extract_attributes;
    let mut out = Vec::new();
    for resource in &request.resource_spans {
        let resource_attrs = resource
            .resource
            .as_ref()
            .map(|r| extract_attributes(&r.attributes))
            .unwrap_or_default();
        for scope in &resource.scope_spans {
            let scope_name = scope.scope.as_ref().map(|s| s.name.clone());
            for span in &scope.spans {
                out.push((
                    span.name.clone(),
                    scope_name.clone(),
                    extract_attributes(&span.attributes),
                    resource_attrs.clone(),
                ));
            }
        }
    }
    out
}

/// Every span of the corpus is labelled alike by the priority order and by the retired resolution.
///
/// The retired plan is the frozen ranks and inherited edges of `retired_detect_order.rs` over the current
/// predicates, resolved the way `supersedes` used to order: the lowest-ranked matching clause no matching clause
/// transitively beats. The migration moved two producers' evidence (`detection_by_priority_changes_only_the_stated_overlaps`
/// says exactly where the answers differ); no captured span is in one of those overlaps.
#[test]
fn detection_by_priority_matches_the_retired_resolution_over_the_corpus() {
    let plan = &sideseat_domain::rules::ruleset().detect;
    let mut spans = 0_usize;
    let mut disagreements: BTreeSet<String> = BTreeSet::new();
    for (label, paths) in discover_fixtures() {
        for path in &paths {
            for (span_name, scope_name, span_attrs, resource_attrs) in
                detect_inputs(&decode_request(path))
            {
                spans += 1;
                let ctx = sideseat_domain::rules::DetectContext {
                    span_name: &span_name,
                    scope_name: scope_name.as_deref(),
                    span_attrs: &span_attrs,
                    resource_attrs: &resource_attrs,
                };
                let now = plan.resolve(&ctx).map(|rule| rule.label.as_str());
                let retired = plan.retired_resolve(&ctx, RETIRED_ORDER, CLAUSE_ORIGIN);
                if now != retired {
                    disagreements.insert(format!(
                        "{label} / {span_name}: now {now:?}, retired {retired:?}"
                    ));
                }
            }
        }
    }
    assert!(spans > 1000, "the oracle compared only {spans} spans");
    assert!(
        disagreements.is_empty(),
        "{} corpus span(s) are labelled differently by priority and by the retired resolution:\n{}",
        disagreements.len(),
        disagreements
            .into_iter()
            .take(40)
            .collect::<Vec<_>>()
            .join("\n")
    );
    eprintln!("detection oracle: {spans} corpus spans, no disagreement");
}

/// Where the two resolutions differ, exhaustively over the overlaps the migration moved, and nowhere else.
///
/// Two shipped edges pointed at rules ranked ahead of their sources, and with the ranks they made detection a
/// cycle of preferences (OpenInference over Semantic Kernel, Semantic Kernel over Azure OpenAI, Azure OpenAI over
/// OpenInference) that no priority order can state. The migration chose, per overlap, and these are the choices:
///
/// - Azure OpenAI's own signals beside an OpenInference attribute, without OpenInference's `llm.provider` pair:
///   OpenInference now, Azure OpenAI before. The edge only existed for OpenInference's Azure spans, which carry
///   that pair and are still Azure OpenAI's through the alternative at priority 139 - which was always ahead.
///   Keeping the edge would have needed Azure OpenAI ahead of Semantic Kernel, relabelling Semantic Kernel's own
///   spans on Azure.
/// - OpenAI Agents' own scope or attribute namespace beside a later instrumentation library (MLflow, Langfuse,
///   Genkit, Traceloop, LiveKit): OpenAI Agents now, the library before. Those signals are the framework's own,
///   which is what the edge over Logfire already said.
/// - OpenAI Agents' default service name beside Logfire's signals *and* a later library's: OpenAI Agents now, the
///   library before. The service name alone stays behind every library, as it was; beside Logfire's signals it
///   is the evidence for the SDK's run-level spans, which Logfire writes without the SDK's scope.
#[test]
fn detection_by_priority_changes_only_the_stated_overlaps() {
    let plan = &sideseat_domain::rules::ruleset().detect;
    let attrs = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    };
    let none = HashMap::new();
    // (what, scope, span attributes, resource attributes, now, retired)
    type Case<'a> = (
        &'a str,
        Option<&'a str>,
        HashMap<String, String>,
        HashMap<String, String>,
        &'a str,
        &'a str,
    );
    let cases: Vec<Case<'_>> = vec![
        (
            "azure signal beside an openinference attribute",
            None,
            attrs(&[
                ("gen_ai.system", "azure_openai"),
                ("openinference.span.kind", "LLM"),
            ]),
            none.clone(),
            "OpenInference",
            "AzureOpenAI",
        ),
        (
            "openinference's azure pair is unchanged",
            None,
            attrs(&[
                ("llm.provider", "azure"),
                ("llm.system", "openai"),
                ("openinference.span.kind", "LLM"),
            ]),
            none.clone(),
            "AzureOpenAI",
            "AzureOpenAI",
        ),
        (
            "semantic kernel on azure is unchanged",
            Some("semantic_kernel.connectors.ai.chat_completion_client_base"),
            attrs(&[("gen_ai.system", "azure_openai")]),
            none.clone(),
            "SemanticKernel",
            "SemanticKernel",
        ),
        (
            "openai agents scope beside traceloop",
            Some("logfire.openai_agents"),
            attrs(&[("traceloop.span.kind", "workflow")]),
            none.clone(),
            "OpenAIAgents",
            "TraceLoop",
        ),
        (
            "openai agents attribute beside mlflow",
            None,
            attrs(&[("openai.agents.x", "1"), ("mlflow.spanType", "LLM")]),
            none.clone(),
            "OpenAIAgents",
            "MLflow",
        ),
        (
            "openai agents scope beside logfire is unchanged",
            Some("logfire.openai_agents"),
            attrs(&[("logfire.msg", "x")]),
            none.clone(),
            "OpenAIAgents",
            "OpenAIAgents",
        ),
        (
            "openai agents service name beside logfire is unchanged",
            None,
            attrs(&[("logfire.msg", "x")]),
            attrs(&[("service.name", "openai-agents")]),
            "OpenAIAgents",
            "OpenAIAgents",
        ),
        (
            "openai agents service name beside logfire and traceloop",
            None,
            attrs(&[("logfire.msg", "x"), ("traceloop.span.kind", "workflow")]),
            attrs(&[("service.name", "openai-agents")]),
            "OpenAIAgents",
            "TraceLoop",
        ),
        (
            "openai agents service name beside traceloop alone is unchanged",
            None,
            attrs(&[("traceloop.span.kind", "workflow")]),
            attrs(&[("service.name", "openai-agents")]),
            "TraceLoop",
            "TraceLoop",
        ),
        (
            "openai agents service name alone is unchanged",
            None,
            none.clone(),
            attrs(&[("service.name", "openai-agents")]),
            "OpenAIAgents",
            "OpenAIAgents",
        ),
    ];
    for (what, scope, span_attrs, resource_attrs, now, retired) in &cases {
        let ctx = sideseat_domain::rules::DetectContext {
            span_name: "probe",
            scope_name: *scope,
            span_attrs,
            resource_attrs,
        };
        assert_eq!(
            plan.resolve(&ctx).map(|rule| rule.label.as_str()),
            Some(*now),
            "{what}: the current label"
        );
        assert_eq!(
            plan.retired_resolve(&ctx, RETIRED_ORDER, CLAUSE_ORIGIN),
            Some(*retired),
            "{what}: the retired label"
        );
    }

    // And nowhere else: every pair of single-signal spans drawn from each clause's own literals resolves alike,
    // except the overlaps above. A clause's literal is the cheapest span it matches, so a pair of them is a span
    // in the overlap of two clauses - which is where a precedence change can show.
    // (scope, span attributes, resource attributes)
    type Probe = (
        Option<String>,
        HashMap<String, String>,
        HashMap<String, String>,
    );
    let mut probes: Vec<Probe> = Vec::new();
    for rule in plan.rules() {
        use sideseat_domain::rules::span_conditions::{SpanAtom, positive_atoms};
        for atom in positive_atoms(&rule.condition) {
            match atom {
                SpanAtom::SpanAttrKeyStartsWith { prefix } => {
                    probes.push((None, attrs(&[(&format!("{prefix}x"), "1")]), none.clone()));
                }
                SpanAtom::SpanAttrEquals { key, value }
                | SpanAtom::SpanAttrEqualsIgnoreAsciiCase { key, value } => {
                    probes.push((None, attrs(&[(key, value)]), none.clone()));
                }
                SpanAtom::SpanAttrExists { key } => {
                    probes.push((None, attrs(&[(key, "1")]), none.clone()));
                }
                SpanAtom::ScopeNameEquals { name } => {
                    probes.push((Some(name.clone()), none.clone(), none.clone()));
                }
                SpanAtom::ResourceAttrContains { key, value } => {
                    probes.push((None, none.clone(), attrs(&[(key, value)])));
                }
                _ => {}
            }
        }
    }
    let mut moved: BTreeSet<(String, String)> = BTreeSet::new();
    for (index, (scope_a, span_a, resource_a)) in probes.iter().enumerate() {
        for (scope_b, span_b, resource_b) in &probes[index..] {
            if scope_a.is_some() && scope_b.is_some() && scope_a != scope_b {
                continue;
            }
            let scope = scope_a.clone().or_else(|| scope_b.clone());
            let span_attrs: HashMap<String, String> = span_a
                .iter()
                .chain(span_b)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let resource_attrs: HashMap<String, String> = resource_a
                .iter()
                .chain(resource_b)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let ctx = sideseat_domain::rules::DetectContext {
                span_name: "probe",
                scope_name: scope.as_deref(),
                span_attrs: &span_attrs,
                resource_attrs: &resource_attrs,
            };
            let now = plan.resolve(&ctx).map(|rule| rule.label.clone());
            let retired = plan
                .retired_resolve(&ctx, RETIRED_ORDER, CLAUSE_ORIGIN)
                .map(str::to_string);
            if now != retired {
                moved.insert((retired.unwrap_or_default(), now.unwrap_or_default()));
            }
        }
    }
    let stated: BTreeSet<(String, String)> = [
        ("AzureOpenAI", "OpenInference"),
        ("MLflow", "OpenAIAgents"),
        ("Langfuse", "OpenAIAgents"),
        ("Genkit", "OpenAIAgents"),
        ("TraceLoop", "OpenAIAgents"),
        ("LiveKit", "OpenAIAgents"),
    ]
    .into_iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    assert!(
        moved.is_subset(&stated),
        "detection moved where the migration states nothing (retired -> now): {:?}",
        moved.difference(&stated).collect::<Vec<_>>()
    );
    assert!(
        moved.contains(&("AzureOpenAI".to_string(), "OpenInference".to_string()))
            && moved.contains(&("TraceLoop".to_string(), "OpenAIAgents".to_string())),
        "the probe pairs must reach the stated overlaps, or they prove nothing: {moved:?}"
    );
}

// ============================================================================
// Equivalence oracle: every `where` against the span predicate it replaced
// ============================================================================

/// Every clause's `where` answers as the retired declaration it replaced, for every span of the corpus.
///
/// The retired declarations - detection's `match` and `all_of`, a classification's `all_of`, a gate's `when`,
/// `unless` and instrumentation scope, a span fact's signal - are frozen in `retired_span_predicates.json` and
/// asked by the retired evaluator; each clause's current `where` is asked by the grammar, with the sources its
/// section is given. The generated-span half is `every_where_answers_as_the_predicate_it_replaced` in the
/// domain crate.
#[test]
fn every_where_answers_as_the_predicate_it_replaced_over_the_corpus() {
    use sideseat_domain::rules::retired_span_predicates::{
        RetiredSubject, current_conditions, retired_holds,
    };
    use sideseat_domain::rules::span_conditions::{SpanSubject, holds, lower};

    let frozen: serde_json::Value =
        serde_json::from_slice(include_bytes!("retired_span_predicates.json"))
            .expect("the frozen declarations decode");
    let records = frozen["clauses"].as_array().expect("a list of clauses");
    let current = current_conditions(
        &sideseat_domain::rules::assets::ParsedAssets::parse(
            &sideseat_domain::rules::schema::embedded_sources(),
        )
        .expect("the embedded assets parse"),
    );
    let lowered: Vec<_> = records
        .iter()
        .map(|record| {
            let clause = record["clause"].as_str().expect("a clause key");
            let (readable, condition) = current
                .get(clause)
                .unwrap_or_else(|| panic!("{clause} has no current `where`"));
            (
                clause,
                record,
                *readable,
                lower(condition, *readable).expect("the current condition lowers"),
            )
        })
        .collect();

    let mut seen: HashSet<u64> = HashSet::new();
    let mut spans = 0_usize;
    let mut disagreements: BTreeSet<String> = BTreeSet::new();
    let mut held: BTreeMap<&str, usize> = BTreeMap::new();
    for (label, paths) in discover_fixtures() {
        for path in &paths {
            for (span_name, scope_name, span_attrs, resource_attrs) in
                detect_inputs(&decode_request(path))
            {
                let digest = {
                    use std::hash::{Hash, Hasher};
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    span_name.hash(&mut hasher);
                    scope_name.hash(&mut hasher);
                    let mut pairs: Vec<_> = span_attrs.iter().chain(&resource_attrs).collect();
                    pairs.sort();
                    pairs.hash(&mut hasher);
                    hasher.finish()
                };
                if !seen.insert(digest) {
                    continue;
                }
                spans += 1;
                for (clause, record, readable, condition) in &lowered {
                    let old = retired_holds(
                        record,
                        &RetiredSubject {
                            span_name: &span_name,
                            scope_name: scope_name.as_deref(),
                            attrs: &span_attrs,
                            resource: &resource_attrs,
                        },
                    );
                    let new = holds(
                        condition,
                        &SpanSubject {
                            span_name: if readable.span_name { &span_name } else { "" },
                            attrs: &span_attrs,
                            scope_name: if readable.scope {
                                scope_name.as_deref()
                            } else {
                                None
                            },
                            resource: readable.resource.then_some(&resource_attrs),
                        },
                    );
                    if new {
                        *held.entry(clause).or_default() += 1;
                    }
                    if old != new {
                        disagreements.insert(format!(
                            "{label} / {span_name}: `{clause}` retired {old}, `where` {new}"
                        ));
                    }
                }
            }
        }
    }
    assert!(
        spans > 1000,
        "the oracle compared only {spans} distinct spans"
    );
    assert!(
        held.len() > records.len() / 2,
        "only {} of {} clauses held on any corpus span, so the comparison barely exercised them",
        held.len(),
        records.len()
    );
    assert!(
        disagreements.is_empty(),
        "{} corpus answer(s) differ between a `where` and the predicate it replaced:\n{}",
        disagreements.len(),
        disagreements
            .into_iter()
            .take(40)
            .collect::<Vec<_>>()
            .join("\n")
    );
    eprintln!(
        "where oracle: {} clauses over {spans} distinct corpus spans, {} of them held somewhere, no disagreement",
        records.len(),
        held.len()
    );
}
