
/// Every native half of a framework parity pair reaches the framework label its SDK slug declares.
///
/// Conversation goldens deliberately omit provenance, so a current instrumentor could keep producing the
/// same messages while its spans silently fell back to `OpenInference` or `Unknown`. Native captures are the
/// proof that server-side detection works without the SDK resource declaration filling the gap.
#[test]
fn native_framework_captures_exercise_their_declared_identity() {
    let plan = &sideseat_domain::rules::ruleset().detect;
    let mut suites: BTreeMap<String, (String, BTreeSet<String>)> = BTreeMap::new();

    for (label, paths) in discover_fixtures() {
        let Some((suite, _)) = label.split_once('/') else {
            continue;
        };
        let Some(slug) = suite.strip_suffix("-native") else {
            continue;
        };
        let Some(expected) = plan.label_from_declaration(slug) else {
            continue;
        };

        let found = rows_for(&paths)
            .into_iter()
            .filter_map(|(_, row)| row.framework)
            .collect::<BTreeSet<_>>();
        suites
            .entry(slug.to_string())
            .or_insert_with(|| (expected.to_string(), BTreeSet::new()))
            .1
            .extend(found);
    }

    for (suite, (expected, found)) in &suites {
        assert!(
            found.contains(expected),
            "{suite}-native: instrumentation never produced its declared `{expected}` framework label; \
             observed {found:?}"
        );
    }

    assert!(
        !suites.is_empty(),
        "no native framework capture had a server-recognised SDK slug"
    );
}

/// The declared classification plan answers as the sweep does for **every span of the corpus**.
///
/// The precedence cases live beside the sweep; this is the other half of the shadow comparison, and the half
/// that covers shapes nobody thought to write down. It also reports how many spans each answer covers, so a
/// rule that no captured span reaches is visible rather than assumed exercised.
#[test]
fn the_declared_classification_matches_the_sweep_across_the_corpus() {
    use sideseat_ingestion::otlp::extract_attributes;
    use sideseat_ingestion::traces::extract::attributes::{
        categorize_span_legacy, detect_observation_type_legacy,
    };
    use sideseat_ports::types::{ObservationType, SpanCategory};

    let plan = &sideseat_domain::rules::ruleset().observation_types;
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut spans = 0_usize;
    let mut disagreements: Vec<String> = Vec::new();

    for (label, paths) in discover_fixtures() {
        for path in &paths {
            let request = decode_request(path);
            for resource in &request.resource_spans {
                for scope in &resource.scope_spans {
                    for span in &scope.spans {
                        let attrs = extract_attributes(&span.attributes);
                        spans += 1;
                        let declared = plan
                            .observation_type(&span.name, &attrs)
                            .map(|verdict| verdict.value.to_string())
                            .unwrap_or_else(|| ObservationType::Span.as_str().to_string());
                        let swept = detect_observation_type_legacy(&span.name, &attrs)
                            .as_str()
                            .to_string();
                        *seen.entry(declared.clone()).or_default() += 1;
                        // **One recorded divergence, and it is a repair the retired sweep shared.** A tool
                        // execution whose producer also stamps the *owning agent's* name on the span: the
                        // retired sweep asked "does an agent name exist" (and the declared rules reproduced it
                        // at rank 100) before "does a tool name exist" (110), so a span named
                        // `execute_tool <tool>` carrying `gen_ai.tool.name`, `gen_ai.tool.call.id`,
                        // `gen_ai.tool.description` and `gen_ai.tool.type` was classified `agent` - while 86
                        // otherwise identical spans were classified `tool`, because their producer stamps no
                        // agent name. Same operation, two answers, decided by an attribute about something
                        // else. `observation.execute_tool` reads the operation the producer *names*, which is
                        // the conventions' own explicit statement and was read by nothing.
                        let repaired_tool_execution = declared == "tool"
                            && swept == "agent"
                            && attrs.get("gen_ai.operation.name").map(String::as_str)
                                == Some("execute_tool");
                        if declared != swept && !repaired_tool_execution {
                            disagreements.push(format!(
                                "{label} / {}: observation declared {declared}, swept {swept}",
                                span.name
                            ));
                        }
                        // The category is a separate question with its own precedence, so it gets its own
                        // comparison over the same spans.
                        let declared_category = plan
                            .span_category(&span.name, &attrs)
                            .map(|verdict| verdict.value.to_string())
                            .unwrap_or_else(|| SpanCategory::Other.as_str().to_string());
                        let swept_category = categorize_span_legacy(&span.name, &attrs)
                            .as_str()
                            .to_string();
                        *seen
                            .entry(format!("category:{declared_category}"))
                            .or_default() += 1;
                        if declared_category != swept_category {
                            disagreements.push(format!(
                                "{label} / {}: category declared {declared_category}, swept {swept_category}",
                                span.name
                            ));
                        }
                    }
                }
            }
        }
    }

    assert!(spans > 0, "the corpus produced no spans to classify");
    assert!(
        disagreements.is_empty(),
        "{} of {spans} spans classify differently:\n{}",
        disagreements.len(),
        disagreements.join("\n")
    );
    eprintln!("classification over {spans} corpus spans: {seen:?}");
}

/// The migrated member vocabulary answers as the retired lists did for **every raw message of the corpus**, and
/// every declared member is reached by one.
///
/// The goldens compare the *finished* views, so a divergence inside normalisation that two paths happen to
/// cancel out would not show. This compares the migrated units directly, over corpus data rather than
/// hand-written shapes.
///
/// "Every object" means **every object down to depth eight** of each parsed attribute: the walk is bounded, so a
/// producer nesting deeper than that is not covered here. Stated because the number below reads as exhaustive
/// and is not.
///
/// The unobserved members are an exact set, not a proportion. A floor would let the vocabulary quietly stop
/// describing what producers write while still passing; naming them means a member that *starts* being carried,
/// or one that stops, is a failure with a name.
#[test]
fn the_member_vocabulary_answers_as_it_did_across_the_corpus() {
    use sideseat_domain::sideml::{is_plain_data_value, is_plain_data_value_legacy};

    let plan = &sideseat_domain::rules::ruleset().message_members;
    let mut values = 0_usize;
    let mut disagreements = Vec::new();
    let mut members_seen: BTreeSet<String> = BTreeSet::new();

    // Every JSON value any captured span carries, walked - a message, a content block, a nested part. What
    // matters is that these are shapes producers actually wrote rather than shapes I thought of.
    fn walk(
        value: &serde_json::Value,
        depth: usize,
        seen: &mut BTreeSet<String>,
        count: &mut usize,
        disagreements: &mut Vec<String>,
    ) {
        if depth > 8 {
            return;
        }
        match value {
            serde_json::Value::Object(map) => {
                *count += 1;
                for key in map.keys() {
                    seen.insert(key.clone());
                }
                if is_plain_data_value(value) != is_plain_data_value_legacy(value) {
                    disagreements.push(format!(
                        "bare-data disagreement on {}",
                        &value.to_string()[..value.to_string().len().min(160)]
                    ));
                }
                for inner in map.values() {
                    walk(inner, depth + 1, seen, count, disagreements);
                }
            }
            serde_json::Value::Array(items) => {
                for inner in items {
                    walk(inner, depth + 1, seen, count, disagreements);
                }
            }
            _ => {}
        }
    }

    for (_, paths) in discover_fixtures() {
        for path in &paths {
            let request = decode_request(path);
            for resource in &request.resource_spans {
                for scope in &resource.scope_spans {
                    for span in &scope.spans {
                        for attr in &span.attributes {
                            let Some(text) = attr
                                .value
                                .as_ref()
                                .and_then(|v| v.value.as_ref())
                                .and_then(|v| match v {
                                    opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(s) => Some(s),
                                    _ => None,
                                })
                            else {
                                continue;
                            };
                            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(text) {
                                walk(
                                    &parsed,
                                    0,
                                    &mut members_seen,
                                    &mut values,
                                    &mut disagreements,
                                );
                            }
                        }
                        for event in &span.events {
                            for attr in &event.attributes {
                                let Some(text) = attr
                                    .value
                                    .as_ref()
                                    .and_then(|v| v.value.as_ref())
                                    .and_then(|v| match v {
                                        opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(s) => Some(s),
                                        _ => None,
                                    })
                                else {
                                    continue;
                                };
                                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(text)
                                {
                                    walk(
                                        &parsed,
                                        0,
                                        &mut members_seen,
                                        &mut values,
                                        &mut disagreements,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    assert!(
        values > 1_000,
        "the corpus produced only {values} objects to compare, which is too few to mean anything"
    );
    assert!(
        disagreements.is_empty(),
        "{} of {values} corpus objects answer differently:\n{}",
        disagreements.len(),
        disagreements.join("\n")
    );

    // Which declared members no captured message carries, as an exact set. Each is there for a dialect nobody
    // has captured, exactly as the unreached message rules are - and naming them means a member that starts
    // being carried, or one that stops, fails with a name instead of moving a proportion.
    const UNOBSERVED: &[&str] = &[
        "file_data",
        "finishReason",
        "functionCall",
        "functionResponse",
        "toolCalls",
        "video",
    ];

    let declared: BTreeSet<&str> = sideseat_domain::rules::ruleset()
        .message_members
        .content_in_order()
        .chain(plan.message_shaped_members())
        .chain(plan.content_block_members())
        .collect();
    let unobserved: BTreeSet<&str> = declared
        .iter()
        .copied()
        .filter(|m| !members_seen.contains(*m))
        .collect();
    let expected: BTreeSet<&str> = UNOBSERVED.iter().copied().collect();
    assert_eq!(
        unobserved, expected,
        "the set of declared members no corpus message carries has changed, over {values} objects - a member \
         that started being carried belongs out of UNOBSERVED, and one that stopped is either a capture that \
         went away or a vocabulary that has drifted from what producers write"
    );
    // And every listed member is declared, so the list cannot outlive what it exempts.
    for member in UNOBSERVED {
        assert!(
            declared.contains(member),
            "UNOBSERVED names `{member}`, which no rule declares"
        );
    }
}

/// Every declared **subdivision** either produces an answer over the corpus, or is exempted with a reason.
///
/// The rule-level gate beside this one could not see inside a rule: `SectionRoute.id`, `ElementPass.id` and
/// `DerivedCase.id` are required declarations, and `sectioned()` / `element_passes()` discarded them before an
/// emission was built - so two routes of one rule produced emissions with *identical* evidence, and a
/// diagnostic could name the rule and not the route. Now the emission carries the path, which is what lets this
/// ask the question at all.
///
/// It is also what makes the field load-bearing rather than a struct member nobody reads: before this, an
/// emission's `rule_id` was consumed by nothing outside the rules module, so adding a clause path would only
/// have moved the discard one level.
#[test]
fn no_declared_subdivision_is_dead_across_the_corpus() {
    use sideseat_domain::rules::schema::{MessageRule, RuleFile};

    /// Subdivisions no captured request exercises, and why.
    ///
    /// All four are Logfire's, for the reason the rule-level gate already records against every
    /// `logfire.*` rule: no fixture has been captured for that suite. So this list carries no reason the
    /// other does not, which is the state to want - a subdivision exempted for a reason of its own would
    /// mean a clause inside a *reached* rule that nothing reaches.
    const UNREACHED: &[(&str, &str)] = &[
        (
            "logfire.conversation/everything_else_that_carries",
            "no captured fixture for the suite",
        ),
        (
            "logfire.conversation/everything_else_that_carries/assistant",
            "no captured fixture for the suite",
        ),
        (
            "logfire.conversation/everything_else_that_carries/user",
            "no captured fixture for the suite",
        ),
        (
            "logfire.conversation/name",
            "no captured fixture for the suite",
        ),
    ];

    fn paths_of(rule: &MessageRule, prefix: &str, out: &mut Vec<String>) {
        if let Some(sections) = &rule.sections {
            for route in &sections.routes {
                out.push(format!("{prefix}/{}", route.id));
            }
        }
        if let Some(elements) = &rule.elements {
            for pass in &elements.passes {
                out.push(format!("{prefix}/{}", pass.id));
                if let Some(group) = &pass.group {
                    for case in &group.by {
                        out.push(format!("{prefix}/{}/{}", pass.id, case.id));
                    }
                }
            }
        }
        if let Some(set) = &rule.branch_set {
            // A branch leaf is a rule in its own right and keeps the *parent's* id in an emission, so its
            // subdivisions live in the parent's path space.
            for leaf in set
                .primary
                .iter()
                .chain(&set.fallback_if_primary_empty)
                .chain(&set.always)
            {
                paths_of(leaf, prefix, out);
            }
        }
    }

    let mut declared: Vec<String> = Vec::new();
    for (_, bytes) in sideseat_domain::rules::schema::embedded_sources() {
        let file: RuleFile = serde_json::from_slice(&bytes).expect("the asset parses");
        for rule in &file.messages {
            paths_of(rule, &rule.id, &mut declared);
        }
    }
    declared.sort();
    declared.dedup();
    // Six today: two section routes, two element passes, two derived cases. Asserted so the gate cannot pass
    // by finding nothing - which is how a walk that stopped covering a nesting would look.
    assert_eq!(
        declared.len(),
        6,
        "the declared subdivisions changed; the walk may have stopped covering a nesting: {declared:?}"
    );

    let fired = rules_that_emit();
    let exempt: BTreeSet<&str> = UNREACHED.iter().map(|(id, _)| *id).collect();

    let silent: Vec<&String> = declared
        .iter()
        .filter(|path| !fired.contains(*path) && !exempt.contains(path.as_str()))
        .collect();
    assert!(
        silent.is_empty(),
        "{} declared subdivision(s) never produced an answer over the corpus. Either the clause is dead - a \
         route no tag reaches, a pass whose predicate never holds - or the path is not being carried through \
         to the emission, which is the defect this gate exists to catch:\n  {}",
        silent.len(),
        silent
            .iter()
            .map(|p| p.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );

    // Both directions, as the rule-level gate is: an exemption that starts firing is a stale excuse.
    let revived: Vec<&str> = exempt
        .iter()
        .copied()
        .filter(|path| fired.contains(*path))
        .collect();
    assert!(
        revived.is_empty(),
        "exempted subdivision(s) now fire, so the exemption is stale: {revived:?}"
    );
}

/// What a span's persisted tool set loses: **which carrier said it**.
///
/// A `RawToolDefinition` carries its source; `flatten_tool_definitions` concatenates the contents and drops it,
/// unlike a message, which keeps `_source` through persistence. This measures what that costs across the corpus,
/// so the finding is a number rather than an argument - and pins it, so a later repair has a baseline.
#[test]
fn a_persisted_tool_set_reports_what_its_provenance_would_have_said() {
    use sideseat_domain::rules::MessageContext;
    use sideseat_ingestion::otlp::extract_attributes;
    use std::collections::{BTreeMap, BTreeSet};

    let plan = &sideseat_domain::rules::ruleset().messages;
    // Spans where two carriers name one tool with *different* content: a merge combines two producers'
    // statements into one neither made, and nothing can report that it happened.
    let mut conflicting = 0_usize;
    // Spans where two carriers state one tool identically: the array stores it twice, and read-time
    // deduplication hides it.
    let mut duplicated = 0_usize;
    let mut spans_declaring_tools = 0_usize;
    // The same question at the scope `deduplicate_tools` actually merges over - a whole sample, not one span.
    // A per-span count cannot see two spans of one trace declaring different forms of a tool, which is the shape
    // the read-time merge would silently combine.
    let mut samples_with_conflicts: Vec<String> = Vec::new();
    // Two forms of one name where **neither contradicts** the other: one states less than the other, which is
    // what "merge preserves complementary fields" is for. Counted, because "the forms differ" is the wrong
    // question - every sample that declares tools twice differs, and almost all of it is refinement.
    let mut refined = 0_usize;

    for (sample, paths) in discover_fixtures() {
        let mut sample_forms: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for path in &paths {
            let request = decode_request(path);
            for resource in &request.resource_spans {
                for scope in &resource.scope_spans {
                    for span in &scope.spans {
                        let attrs = extract_attributes(&span.attributes);
                        let is_tool = sideseat_domain::rules::ruleset().span_facts.holds(
                            sideseat_domain::rules::schema::SpanFact::ToolExecution,
                            &attrs,
                        );
                        let ctx = MessageContext::for_span(&span.name, &attrs, is_tool);

                        // name -> the distinct canonical forms declared for it, and by how many carriers
                        let mut by_name: BTreeMap<String, (BTreeSet<String>, usize)> =
                            BTreeMap::new();
                        for emission in plan.tool_definitions(&ctx) {
                            let items = match &emission.value {
                                serde_json::Value::Array(items) => items.clone(),
                                single => vec![single.clone()],
                            };
                            for item in items {
                                let canonical =
                                    sideseat_domain::sideml::tools::normalize_tools(&item);
                                for definition in canonical.as_array().cloned().unwrap_or_default()
                                {
                                    let Some(name) =
                                        sideseat_domain::sideml::extract_tool_name(&definition)
                                    else {
                                        continue;
                                    };
                                    let entry = by_name.entry(name).or_default();
                                    entry.0.insert(definition.to_string());
                                    entry.1 += 1;
                                }
                            }
                        }
                        if by_name.is_empty() {
                            continue;
                        }
                        spans_declaring_tools += 1;
                        if by_name.values().any(|(forms, _)| forms.len() > 1) {
                            conflicting += 1;
                        }
                        for (name, (forms, _)) in &by_name {
                            sample_forms
                                .entry(name.clone())
                                .or_default()
                                .extend(forms.iter().cloned());
                        }
                        if by_name
                            .values()
                            .any(|(forms, count)| forms.len() == 1 && *count > 1)
                        {
                            duplicated += 1;
                        }
                    }
                }
            }
        }
        for (name, forms) in &sample_forms {
            if forms.len() > 1 {
                refined += 1;
            }
            if let Some(detail) = contradiction_among(forms) {
                samples_with_conflicts.push(format!("{sample} :: {name} :: {detail}"));
            }
        }
    }

    println!(
        "tool sets: {spans_declaring_tools} spans declare tools; {duplicated} store a definition twice; \
         {conflicting} spans hold two forms of one tool name; {refined} tools are stated at two levels of \
         detail across a sample's spans; {} of those contradict",
        samples_with_conflicts.len()
    );
    assert!(
        spans_declaring_tools > 0,
        "the corpus must declare tools somewhere, or this measures nothing"
    );
    // Pinned, not aspirational: a repair that preserves provenance should be able to *report* these rather than
    // silently merge them, and a change that makes them worse should have to say so here.
    // Contradictions **do** occur - one, in `crewai/swarm` - so the invariant is not that they are absent but
    // that the merge no longer answers one by discarding the other. Each contradicting statement must survive as
    // its own definition, which is what a reader needs when two agents describe one tool differently.
    assert!(
        !samples_with_conflicts.is_empty(),
        "no contradiction was found, so the invariant below is vacuous - the corpus used to hold one in \
         `crewai/swarm`, whose agents each enumerate their own coworkers"
    );
    for conflict in &samples_with_conflicts {
        let (sample, rest) = conflict
            .split_once(" :: ")
            .expect("the report names its sample");
        let name = rest.split(" :: ").next().expect("and its tool");
        let survived = surviving_definitions(sample, name);
        assert!(
            survived >= 2,
            "`{name}` is stated in contradicting forms by `{sample}` and only {survived} survived - the merge \
             answered a disagreement by dropping a producer's statement"
        );
    }
    assert!(
        refined > 0,
        "the corpus must state some tool at two levels of detail, or the distinction between refinement and \
         contradiction is untested here"
    );
    // Reported, **not** gated: a definition stored twice is corrected by read-time deduplication, so it costs
    // storage rather than an answer, and pinning the exact number would make every new fixture edit this test.
    // The provenance loss is what makes it invisible; it is not what makes it wrong.
    assert!(
        duplicated <= spans_declaring_tools,
        "counted more duplicating spans than spans declaring tools"
    );
}

/// A member two forms of one tool definition both state and **disagree** about, if any.
///
/// The question that separates a sound merge from an invented statement. Two forms of one tool name are ordinary:
/// a carrier that names a tool and nothing else, beside one that carries its whole schema - and merging those is
/// exactly "preserve complementary fields". What a merge must not do is choose between two producers who both
/// said something and said different things, because the result is a definition neither of them made and, with
/// the provenance dropped at persistence, nothing can report that it happened.
///
/// Compared **leaf by leaf**, so a schema nested three deep is compared at its leaves rather than as one opaque
/// string - two objects differing only in a member one of them omits are not a disagreement.
#[cfg(test)]
fn contradiction_among(forms: &std::collections::BTreeSet<String>) -> Option<String> {
    fn leaves(prefix: &str, value: &serde_json::Value, out: &mut Vec<(String, String)>) {
        match value {
            serde_json::Value::Object(members) => {
                for (key, member) in members {
                    leaves(&format!("{prefix}/{key}"), member, out);
                }
            }
            // Not descended into, for the reason `contradiction_between` gives: a set has no element 0, and
            // comparing by index calls two complementary `required` lists a disagreement.
            serde_json::Value::Array(_) => {}
            scalar => out.push((prefix.to_string(), scalar.to_string())),
        }
    }

    let mut stated: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for form in forms {
        let parsed: serde_json::Value =
            serde_json::from_str(form).expect("a rendered definition parses");
        let mut found = Vec::new();
        leaves("", &parsed, &mut found);
        for (path, value) in found {
            match stated.get(&path) {
                Some(first) if *first != value => {
                    return Some(format!("{path}: {first} vs {value}"));
                }
                _ => {
                    stated.insert(path, value);
                }
            }
        }
    }
    None
}

/// How many definitions of one tool name a sample's whole set reduces to.
///
/// The read path's own `deduplicate_tools`, over every definition the sample's spans declare - so this measures
/// what a reader is shown rather than a re-implementation of it.
#[cfg(test)]
fn surviving_definitions(sample: &str, tool: &str) -> usize {
    use sideseat_domain::rules::MessageContext;
    use sideseat_ingestion::otlp::extract_attributes;

    let mut declared: Vec<serde_json::Value> = Vec::new();
    for (found, paths) in discover_fixtures() {
        if found != sample {
            continue;
        }
        for path in &paths {
            let request = decode_request(path);
            for resource in &request.resource_spans {
                for scope in &resource.scope_spans {
                    for span in &scope.spans {
                        let attrs = extract_attributes(&span.attributes);
                        let is_tool = sideseat_domain::rules::ruleset().span_facts.holds(
                            sideseat_domain::rules::schema::SpanFact::ToolExecution,
                            &attrs,
                        );
                        let ctx = MessageContext::for_span(&span.name, &attrs, is_tool);
                        for emission in sideseat_domain::rules::ruleset()
                            .messages
                            .tool_definitions(&ctx)
                        {
                            match emission.value {
                                serde_json::Value::Array(items) => declared.extend(items),
                                single => declared.push(single),
                            }
                        }
                    }
                }
            }
        }
    }
    sideseat_domain::sideml::feed::deduplicate_tools(declared)
        .iter()
        .filter(|definition| {
            sideseat_domain::sideml::extract_tool_name(definition).as_deref() == Some(tool)
        })
        .count()
}

/// No span's **observation type** and **span category** contradict each other.
///
/// The two are separate classifications with separate precedences, and that is deliberate - a transport call is a
/// plain observation with an HTTP category, and transport-level instrumentation records `gen_ai.*` on a plain
/// span. But they are only *partially* independent: six observation types name the same operation a category
/// names, and for those the pair has one right answer. Nothing said so, and nothing noticed when they disagreed.
///
/// It found 14 spans: a tool execution whose producer also stamps the owning agent's name, classified `agent` with
/// a category of `tool`. Fixed by reading the operation the producer *names* (`observation.execute_tool`), which
/// is the conventions' own statement and was read by nothing.
///
/// A **runtime** invariant over the corpus rather than a compile refusal, because the two rule sets are
/// predicates over spans: proving no span can satisfy an incompatible pair would mean deciding predicate
/// intersection, and the pairs that matter are the ones a real producer writes.
#[test]
fn no_span_is_classified_as_two_incompatible_things() {
    use sideseat_ingestion::otlp::extract_attributes;
    use std::collections::BTreeMap;
    // The six observation types that name the same operation a category names. `span`, `guardrail` and
    // `evaluator` leave the category free - a transport call is a plain observation with an HTTP category, and
    // transport-level instrumentation records `gen_ai.*` on a plain span.
    let implied: BTreeMap<&str, &str> = BTreeMap::from([
        ("generation", "llm"),
        ("embedding", "embedding"),
        ("agent", "agent"),
        ("tool", "tool"),
        ("chain", "chain"),
        ("retriever", "retriever"),
    ]);
    let plan = &sideseat_domain::rules::ruleset().observation_types;
    let mut bad: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut total = 0_usize;
    for (_, paths) in discover_fixtures() {
        for path in &paths {
            let request = decode_request(path);
            for resource in &request.resource_spans {
                for scope in &resource.scope_spans {
                    for span in &scope.spans {
                        let a = extract_attributes(&span.attributes);
                        let obs = plan.observation_type(&span.name, &a).map(|v| v.value);
                        let cat = plan.span_category(&span.name, &a).map(|v| v.value);
                        total += 1;
                        if let (Some(obs), Some(cat)) = (obs, cat)
                            && let Some(want) = implied.get(obs)
                            && *want != cat
                        {
                            *bad.entry((obs.to_string(), cat.to_string())).or_default() += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(total > 0, "the corpus produced no spans to classify");
    assert!(
        bad.is_empty(),
        "{} of {total} spans are classified as two incompatible things: {}",
        bad.values().sum::<usize>(),
        bad.iter()
            .map(|((obs, cat), count)| format!("({obs}, {cat}) x{count}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
}
