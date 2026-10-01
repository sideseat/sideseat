
/// How reconstruction scales with the history a read has to reprocess.
///
/// Normalisation happens at query time, so a session read reparses and rehashes the whole session on
/// every request - there is no cache. Whether that matters is a question about the curve, not about the
/// design, and the curve was unmeasured. Two shapes, because frameworks differ in the one way that
/// decides the answer:
///
/// - **replaying**: every generation span re-sends the whole conversation so far, which is what ADK,
///   LangGraph and Vercel do. The *input* is then quadratic in the turn count however fast the pipeline
///   is, so this is the shape that can hurt.
/// - **incremental**: every span carries only its own turn, as Strands' per-message events do. Linear
///   input.
///
/// ```bash
/// cargo test --locked --release -p sideseat-server bench_session_scaling -- --ignored --nocapture
/// ```
#[test]
#[ignore]
fn bench_session_scaling() {
    fn row(
        trace: usize,
        span: usize,
        messages: String,
        t: chrono::DateTime<chrono::Utc>,
    ) -> MessageSpanRow {
        MessageSpanRow {
            trace_id: format!("trace-{trace}"),
            span_id: format!("span-{trace}-{span}"),
            parent_span_id: None,
            span_timestamp: t,
            span_end_timestamp: Some(t),
            messages_json: messages,
            tool_definitions_json: "[]".to_string(),
            tool_names_json: "[]".to_string(),
            body_cache_key: None,
            model: Some("claude".to_string()),
            provider: Some("bedrock".to_string()),
            status_code: None,
            exception_type: None,
            exception_message: None,
            exception_stacktrace: None,
            input_tokens: 10,
            output_tokens: 5,
            total_tokens: 15,
            cost_total: 0.001,
            observation_type: Some("generation".to_string()),
            session_id: Some("session-1".to_string()),
            ingested_at: t,
            scope_name: None,
            scope_version: None,
            span_name: None,
            framework: None,
            response_model: None,
            response_id: None,
            temperature: None,
            top_p: None,
            max_tokens: None,
            finish_reasons: None,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            cost_input: 0.0,
            cost_output: 0.0,
        }
    }

    use chrono::TimeZone;
    let t0 = chrono::Utc
        .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
        .single()
        .expect("valid time");

    for turns in [100usize, 1_000, 10_000] {
        for shape in ["incremental", "replaying"] {
            // The replaying shape is quadratic in its own input, so the largest size would be 10^8
            // message entries - which measures the fixture generator, not the pipeline.
            if shape == "replaying" && turns > 1_000 {
                continue;
            }

            let build = std::time::Instant::now();
            let mut rows = Vec::with_capacity(turns);
            for turn in 0..turns {
                let t = t0 + chrono::Duration::seconds(turn as i64);
                let mut entries: Vec<String> = Vec::new();
                let history = if shape == "replaying" { 0 } else { turn };
                for past in history..=turn {
                    entries.push(format!(
                        r#"{{"source":{{"attribute":{{"key":"llm.input_messages","time":"{}"}}}},"content":{{"role":"user","content":"question {past}"}}}}"#,
                        (t0 + chrono::Duration::seconds(past as i64)).to_rfc3339()
                    ));
                }
                entries.push(format!(
                    r#"{{"source":{{"event":{{"name":"gen_ai.choice","time":"{}"}}}},"content":{{"role":"assistant","content":"answer {turn}"}}}}"#,
                    t.to_rfc3339()
                ));
                rows.push(row(turn, 0, format!("[{}]", entries.join(",")), t));
            }
            let built = build.elapsed();
            let payload: usize = rows.iter().map(|r| r.messages_json.len()).sum();

            // Cold, then warm: the second read is what a user's second look at a session costs, and
            // whether the memo earns its place is exactly that difference.
            let cache = sideseat_domain::sideml::feed::cache::ReconstructionCache::new();
            let start = std::time::Instant::now();
            let result = sideseat_domain::sideml::feed::process_spans_cached(
                &cache,
                rows.clone(),
                &FeedOptions::new(),
            );
            let cold = start.elapsed();
            let start = std::time::Instant::now();
            let warm_result = sideseat_domain::sideml::feed::process_spans_cached(
                &cache,
                rows,
                &FeedOptions::new(),
            );
            let warm = start.elapsed();
            assert_eq!(
                warm_result.messages.len(),
                result.messages.len(),
                "a warm read must answer with the same blocks"
            );
            eprintln!(
                "SESSION {shape} {turns} turns: {} KB input -> {} blocks, cold {:?}, warm {:?} \
                 (fixture built in {:?})",
                payload / 1024,
                result.messages.len(),
                cold,
                warm,
                built
            );
        }
    }
}

/// A session known only to the store reconstructs exactly as one named on every row.
///
/// The feed groups traces into conversations so a replay crossing traces can be recognised, and it derived
/// that grouping from the `session_id` on the rows it was handed. Those rows have been through
/// `MESSAGE_CONTENT_FILTER`, and a framework records the session on the span that knows it - usually a root
/// carrying no messages, which the filter removes. When every row naming the session was filtered out, each
/// trace became its own conversation, the cross-trace stripping never ran, and the second trace's re-sent
/// history came back as duplicates while the response still reported `session_scoped`.
///
/// Checked against the fixtures where cross-trace stripping actually fires - the ones whose session view
/// holds fewer messages than its traces' views sum to - because a fixture with nothing to strip would pass
/// this test without exercising anything.
#[test]
fn a_session_known_only_to_the_store_reconstructs_identically() {
    let fixtures = discover_fixtures();
    if fixtures.is_empty() {
        eprintln!("session grouping: no fixtures - run scripts/message-fixtures/capture.sh");
        return;
    }

    let mut exercised = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for (label, paths) in &fixtures {
        let rows = rows_for(paths);

        // The trace -> session mapping the store would report.
        let mut session_of_trace: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for (_, row) in &rows {
            if let Some(session) = row.session_id.as_deref().filter(|s| !s.is_empty()) {
                session_of_trace
                    .entry(row.trace_id.clone())
                    .or_insert_with(|| session.to_string());
            }
        }
        // Only fixtures whose session spans more than one trace can strip anything across traces.
        let traces: std::collections::HashSet<&String> = session_of_trace.keys().collect();
        if session_of_trace.is_empty() || traces.len() < 2 {
            continue;
        }

        let with_session_on_rows: Vec<MessageSpanRow> =
            rows.iter().map(|(_, r)| r.clone()).collect();

        // The same rows with every session id removed - the state the content filter leaves when the only
        // spans naming the session carried no messages.
        let stripped: Vec<MessageSpanRow> = with_session_on_rows
            .iter()
            .cloned()
            .map(|mut r| {
                r.session_id = None;
                r
            })
            .collect();

        let named = process_feed(with_session_on_rows, &FeedOptions::new());
        let from_store = process_feed(
            stripped.clone(),
            &FeedOptions::new().with_session_of_trace(session_of_trace.clone()),
        );
        let ungrouped = process_feed(stripped, &FeedOptions::new());

        let describe = |result: &sideseat_domain::sideml::FeedResult| -> Vec<String> {
            result
                .messages
                .iter()
                .map(|b| format!("{}:{}", b.role.as_str(), b.content_hash))
                .collect()
        };

        let named_blocks = describe(&named);
        let store_blocks = describe(&from_store);

        if named_blocks != store_blocks {
            failures.push(format!(
                "{label}: a store-supplied session differs from one on the rows\n  on rows: {named_blocks:?}\n  from store: {store_blocks:?}"
            ));
            continue;
        }

        // And the grouping is what does the work: without it, this fixture returns more.
        if describe(&ungrouped).len() > named_blocks.len() {
            exercised += 1;
        }
    }

    assert!(
        failures.is_empty(),
        "session grouping must not depend on which rows survived the content filter:\n{}",
        failures.join("\n")
    );
    assert!(
        exercised > 0,
        "no fixture exercised cross-trace stripping, so this test proved nothing"
    );
    eprintln!("session grouping: {exercised} fixture(s) exercised cross-trace stripping");
}

/// Every sample the support matrix excuses is one `.gitignore` actually excludes.
///
/// The exclusion list exists so a clean checkout passes `the_corpus_matches_the_support_matrix`, and that is
/// only legitimate for samples the repository deliberately does not carry. Without this check the list would
/// equally excuse a sample somebody forgot to commit, which is the opposite of what the matrix is for.
#[test]
fn local_only_samples_are_actually_gitignored() {
    let gitignore = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo root")
            .join(".gitignore"),
    )
    .expect("read .gitignore");

    for (suite, sample) in [("strands-js", "image-gen"), ("vercel-ai-js", "image-gen")] {
        let path = format!("server/tests/fixtures/messages/{suite}/{sample}/");
        assert!(
            gitignore.lines().any(|line| line.trim() == path),
            "{path} is excused from the support matrix but is not gitignored, so a missing capture there \
             would be silently accepted"
        );
    }
}

/// Every span-reported exception reaches the trace, and independent reports keep their multiplicity.
///
/// This is the check that the five defects of reviews 14-16 would have failed, and none of the existing
/// invariants could: a false equivalence *removes* an occurrence, and `assert_no_duplicates` only rejects
/// duplicate survivors, so all five passed it vacuously.
///
/// Two parts, because they catch different failures:
///
/// - **at least one survives**: `strands/error` rendered *no* error at all, because a parent deferred to
///   an ERROR-status child with nothing to report. The antecedent is source-backed - a span view holding
///   an `attr:exception` block means a row had renderable exception fields - so this cannot accuse a
///   trace whose ERROR status carried no detail;
/// - **independent reports keep their count**: `openai-agents/image_gen` reported three separate
///   `generate_image` failures as one. Where several *distinct spans* each report the same text, the
///   surviving set must be the reporting set.
///
/// Deliberately not "every span exception has a trace representative": that flags the 18 legitimately
/// suppressed parent copies across ten fixtures, since an ancestor re-reports its descendant's failure.
/// Telling that suppression from a false equivalence needs `parent_span_id`, which `InvariantRow` does
/// not carry - so the check is stated over what the harness can actually prove.
fn exception_conservation_violations(built: &Built) -> Vec<String> {
    let mut out = Vec::new();
    for (name, scope, trace_rows) in &built.invariants {
        let Scope::Trace { trace_id } = scope else {
            continue;
        };
        // Which spans of this trace reported which exception text, and the ancestor chain of each -
        // so an exception propagating up a hierarchy is told from the same failure reported by
        // independent siblings.
        let mut reported: HashMap<&str, Vec<(&str, &[String])>> = HashMap::new();
        for (_, other_scope, rows) in &built.invariants {
            let Scope::Span {
                trace_id: span_trace,
                span_id,
            } = other_scope
            else {
                continue;
            };
            if span_trace != trace_id {
                continue;
            }
            for r in rows.iter().filter(|r| r.carrier == "attr:exception") {
                reported
                    .entry(r.content_digest.as_str())
                    .or_default()
                    .push((span_id.as_str(), r.span_path.as_slice()));
            }
        }
        if reported.is_empty() {
            continue;
        }

        // The distinct failures for one digest are the **leaf** reporting spans: a span whose id
        // appears in another reporter's ancestor path is re-reporting a descendant's failure (the
        // production leaf-error rule), so it is not a separate occurrence. Three independent
        // sibling failures (`openai-agents/image_gen`) each stay a leaf; one error propagating up
        // `invoke_agent -> step -> chat` (`vercel-ai-js/error`) has only `chat` as a leaf.
        let leaf_reporters = |reporters: &[(&str, &[String])]| -> BTreeSet<String> {
            reporters
                .iter()
                .filter(|(id, _)| {
                    !reporters.iter().any(|(other, other_path)| {
                        other != id && other_path.iter().any(|a| a == id)
                    })
                })
                .map(|(id, _)| id.to_string())
                .collect()
        };

        let survivors: HashMap<&str, BTreeSet<&str>> = trace_rows.iter().fold(
            HashMap::new(),
            |mut acc: HashMap<&str, BTreeSet<&str>>, r| {
                acc.entry(r.content_digest.as_str())
                    .or_default()
                    .insert(r.span_id.as_str());
                acc
            },
        );

        if !reported.keys().any(|d| survivors.contains_key(d)) {
            out.push(format!(
                "{name}: {} exception(s) reported by spans, none in the trace view",
                reported.len()
            ));
        }
        for (digest, reporters) in &reported {
            let leaves = leaf_reporters(reporters);
            if leaves.len() < 2 {
                continue;
            }
            let kept = survivors.get(digest).cloned().unwrap_or_default();
            if kept.len() < leaves.len() {
                out.push(format!(
                    "{name}: an exception reported by {} independent (non-ancestral) spans survives \
                     on {} - distinct failures must keep their multiplicity",
                    leaves.len(),
                    kept.len()
                ));
            }
        }
    }
    out
}

/// A single-trace fixture whose spans carried a user turn shows one.
///
/// Narrower than the framing ratchet above and aimed at a different failure: not *where* the turn sits
/// but whether it survives at all. `strands-js/swarm` lost its request to two history phases with no
/// copy left to carry it, and nothing objected.
///
/// Restricted to single-trace fixtures because a later trace in a session may legitimately have its turn
/// stripped as a replay of an earlier one - `langgraph/tool_use` trace-4 is exactly that, and without the
/// restriction this would accuse it.
fn lost_user_turn_violations(built: &Built) -> Vec<String> {
    let traces: BTreeSet<&str> = built
        .invariants
        .iter()
        .filter_map(|(_, scope, _)| match scope {
            Scope::Trace { trace_id } => Some(trace_id.as_str()),
            _ => None,
        })
        .collect();
    if traces.len() != 1 {
        return Vec::new();
    }

    let mut out = Vec::new();
    for (name, scope, trace_rows) in &built.invariants {
        let Scope::Trace { trace_id } = scope else {
            continue;
        };
        if trace_rows.is_empty() || trace_rows.iter().any(|r| r.role == "user") {
            continue;
        }
        let carried = built.invariants.iter().any(|(_, s, rows)| {
            matches!(s, Scope::Span { trace_id: t, .. } if t == trace_id)
                && rows.iter().any(|r| r.role == "user")
        });
        if carried {
            out.push(format!(
                "{name}: no user turn, although a span of this single-trace fixture carried one"
            ));
        }
    }
    out
}

/// Both conservation checks, over the whole corpus, with no exemptions.
#[test]
fn a_reported_occurrence_reaches_the_trace() {
    let fixtures = discover_fixtures();
    if fixtures.is_empty() {
        eprintln!("conservation: no fixtures - skipping");
        return;
    }

    let pricing = PricingService::init_for_test().expect("offline pricing service");
    let mut violations: Vec<String> = Vec::new();
    for (label, paths) in &fixtures {
        let rows = rows_for_mode(&pricing, paths, ExtractionMode::PerCarrier);
        let built = build_golden(label, paths, &rows);
        for v in exception_conservation_violations(&built) {
            violations.push(format!("{label} / {v}"));
        }
        for v in lost_user_turn_violations(&built) {
            violations.push(format!("{label} / {v}"));
        }
    }

    assert!(
        violations.is_empty(),
        "content a span reported did not reach its trace:\n  {}",
        violations.join("\n  ")
    );
}

/// The fixtures whose ordering evidence contradicts itself, pinned in both directions.
///
/// A cycle means two constraint classes disagree about a pair of blocks, and the resolver breaks it
/// deterministically toward the legacy order - which is a *guess*, warned about in production and,
/// until this test, invisible in the suite (the golden tests install no tracing subscriber, so the
/// `warn!` reached nobody). The code claimed "no corpus fixture cycles at this constraint density";
/// The list is currently empty: every contradiction previously found in the corpus has been removed.
///
/// Both currently produce byte-correct output, because the deterministic release lands on the legacy
/// order and the goldens bless it - so a cycle here is a contradiction in the *evidence*, not yet a
/// wrong answer. The list is bidirectional for the same reason every list in this file is: a new
/// cycling fixture must be examined rather than silently guessed into order, and a fixture that stops
/// cycling means a constraint class changed and this entry no longer measures anything.
const EVIDENCE_CONTRADICTS_ITSELF: &[(&str, &str)] = &[];

/// Cycles are contradictions, and a contradiction is examined, never silently guessed into order.
#[test]
fn ordering_contradictions_are_pinned() {
    let fixtures = discover_fixtures();
    if fixtures.is_empty() {
        eprintln!("cycles: no fixtures - skipping");
        return;
    }
    let pricing = PricingService::init_for_test().expect("offline pricing service");
    let mut cycling: Vec<String> = Vec::new();
    for (label, paths) in &fixtures {
        let rows = rows_for_mode(&pricing, paths, ExtractionMode::PerCarrier);
        sideseat_domain::sideml::feed::order_graph::CYCLES_BROKEN_IN_TESTS
            .with(|c| *c.borrow_mut() = 0);
        let _ = build_golden(label, paths, &rows);
        let n = sideseat_domain::sideml::feed::order_graph::CYCLES_BROKEN_IN_TESTS
            .with(|c| *c.borrow());
        if n > 0 {
            cycling.push(label.clone());
        }
    }

    let unexpected: Vec<&String> = cycling
        .iter()
        .filter(|l| !EVIDENCE_CONTRADICTS_ITSELF.iter().any(|(f, _)| l == f))
        .collect();
    assert!(
        unexpected.is_empty(),
        "ordering evidence contradicts itself in a fixture not on the pinned list - the resolver is \
         guessing there, and the guess must be examined:\n  {}",
        unexpected
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );
    for (fixture, reason) in EVIDENCE_CONTRADICTS_ITSELF {
        assert!(
            cycling.iter().any(|l| l == fixture),
            "{fixture} no longer cycles ({reason}) - a constraint class changed; remove the entry"
        );
    }
}

// ============================================================================
// Dead-rule measurement
// ============================================================================

/// Every declared rule id, branch leaves included - a leaf emits under its own id.
fn declared_rule_ids() -> BTreeSet<String> {
    fn walk(
        rule: &sideseat_domain::rules::message_rules::CompiledMessageRule,
        out: &mut BTreeSet<String>,
    ) {
        // A branch **parent** never emits under its own id: `emit_rule` delegates to the leaves immediately,
        // and the parent's own reading fields are refused at compile time. So it is not a candidate for
        // deadness - only its leaves are.
        match &rule.branch_set {
            Some(set) => {
                for sub in set.primary.iter().chain(&set.fallback).chain(&set.always) {
                    walk(sub, out);
                }
            }
            None => {
                out.insert(rule.rule_id.clone());
            }
        }
    }
    let mut out = BTreeSet::new();
    for rule in sideseat_domain::rules::ruleset().messages.rules() {
        walk(rule, &mut out);
    }
    out
}

/// Which rules actually emit something, over every captured request of the whole corpus.
/// One emission's provenance as a single string: the rule, then the clauses inside it that produced it.
///
/// The same shape `expr::ClausePath` renders, so a diagnostic and this gate name a clause the same way. For a
/// rule that answered directly this is just the rule id, which the caller records anyway - the value is in the
/// subdivisions, whose required ids used to be discarded before an emission was built.
fn clause_paths(emission: &sideseat_domain::rules::message_rules::Emission<'_>) -> Vec<String> {
    emission
        .evidence
        .paths()
        .iter()
        .map(|path| {
            std::iter::once(path.root.as_str())
                .chain(path.steps.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect()
}

fn rules_that_emit() -> BTreeSet<String> {
    use sideseat_domain::rules::MessageContext;
    use sideseat_domain::rules::message_rules::OwnedCarrier;
    use sideseat_ingestion::otlp::extract_attributes;

    let plan = &sideseat_domain::rules::ruleset().messages;
    let mut fired = BTreeSet::new();
    for (_, paths) in discover_fixtures() {
        for path in &paths {
            let request = decode_request(path);
            for resource in &request.resource_spans {
                for scope in &resource.scope_spans {
                    let (scope_name, scope_version) = scope
                        .scope
                        .as_ref()
                        .map(|scope| {
                            (
                                Some(scope.name.as_str()).filter(|name| !name.is_empty()),
                                Some(scope.version.as_str()).filter(|version| !version.is_empty()),
                            )
                        })
                        .unwrap_or((None, None));
                    for span in &scope.spans {
                        let attrs = extract_attributes(&span.attributes);
                        // The same declared fact the extractor asks, so this measures the plan as ingestion
                        // exercises it rather than a variant of it.
                        let is_tool = sideseat_domain::rules::ruleset().span_facts.holds(
                            sideseat_domain::rules::schema::SpanFact::ToolExecution,
                            &attrs,
                        );
                        let ctx = MessageContext::for_scoped_span(
                            &span.name,
                            scope_name,
                            scope_version,
                            &attrs,
                            is_tool,
                        );
                        let mut read: std::collections::HashSet<OwnedCarrier> =
                            std::collections::HashSet::new();
                        // The sources the dialects produced, in the form the answer-recovery test reads.
                        let mut dialect_output: Vec<
                            sideseat_ingestion::traces::extract::MessageSource,
                        > = Vec::new();
                        for emission in plan.run(&ctx) {
                            fired.insert(emission.rule_id.to_string());
                            fired.extend(clause_paths(&emission));
                            read.extend(emission.owns.iter().cloned());
                            // Only a *message* is output. A `Claim` enters ownership and produces nothing, so
                            // counting one as the span's answer made the measurement skip the recovery pass
                            // that ingestion still runs.
                            if emission.target
                                != sideseat_domain::rules::schema::EmitTarget::Message
                            {
                                continue;
                            }
                            let time = chrono::Utc::now();
                            let name = emission.carrier.name().to_string();
                            dialect_output.push(if emission.carrier.is_event() {
                                sideseat_ingestion::traces::extract::MessageSource::Event {
                                    name,
                                    time,
                                }
                            } else {
                                sideseat_ingestion::traces::extract::MessageSource::Attribute {
                                    key: name,
                                    time,
                                }
                            });
                        }
                        // The fallback stage, gated as ingestion gates it: never on a tool span; with an
                        // **empty** claimed set when no dialect read anything (there is nothing to inherit);
                        // and otherwise only on a *generation* span, where `output.value` means "the answer"
                        // rather than a chain node's state. Asked unconditionally it credited a fallback rule
                        // on spans ingestion never asks, which would hide a genuinely dead one.
                        //
                        // Including the last condition: ingestion skips the recovery pass when a dialect
                        // already produced the span's *output*, so crediting a fallback rule there would hide
                        // a genuinely dead one.
                        if !is_tool {
                            let observation =
                                sideseat_ingestion::traces::extract::attributes::detect_observation_type(
                                    &span.name, &attrs,
                                );
                            let generation = observation == ObservationType::Generation;
                            if read.is_empty() {
                                for emission in
                                    plan.fallback(&ctx, &std::collections::HashSet::new())
                                {
                                    fired.insert(emission.rule_id.to_string());
                                }
                            } else if generation && !dialect_output.iter().any(|source| {
                                sideseat_ingestion::traces::extract::messages::carrier_holds_span_output(
                                    source,
                                    &span.name,
                                    observation,
                                )
                            }) {
                                for emission in plan.fallback(&ctx, &read) {
                                    fired.insert(emission.rule_id.to_string());
                                    fired.extend(clause_paths(&emission));
                                }
                            }
                        }
                        for emission in plan.tool_definitions(&ctx) {
                            fired.insert(emission.rule_id.to_string());
                            fired.extend(clause_paths(&emission));
                        }
                        for event in &span.events {
                            let event_attrs = extract_attributes(&event.attributes);
                            let reading = plan.from_event(
                                &event.name,
                                &event_attrs,
                                &span.name,
                                &attrs,
                                is_tool,
                            );
                            for emission in reading.emissions {
                                fired.insert(emission.rule_id.to_string());
                                fired.extend(clause_paths(&emission));
                            }
                        }
                    }
                }
            }
        }
    }
    fired
}

/// No declared rule is dead, measured rather than reasoned about.
///
/// The compile-time conflict check is a *static approximation* of a runtime property, and repeated adversarial reviews
/// running found successively deeper approximation errors - each one "this rule can never emit and compilation
/// accepted it". Some of those cases need a satisfiability decision over payload predicates, which the checker
/// deliberately is not. This is the measurement that covers them: a rule that emits nothing anywhere in the
/// corpus is either dead or exercised by nothing, and both want to be visible.
///
/// The exemption list is the honest part. Every entry is a rule the corpus does not reach, with the reason,
/// and it is checked in **both** directions - an exempted rule that starts firing fails too, so the list
/// cannot rot into a permanent excuse.
#[test]
fn no_declared_rule_is_dead_across_the_corpus() {
    /// Rules no captured request exercises, and why. Unreached is not the same as dead, and the difference is
    /// what each reason has to say. Two thirds of the list is one cause - a supported dialect nobody has
    /// captured - and the rest are shapes the captured runs never produced.
    const UNREACHED: &[(&str, &str)] = &[
        (
            "autogen.autogen_event",
            "the suite is captured through OpenInference; no fixture emits the legacy AutoGen event shape",
        ),
        (
            "autogen.body",
            "the suite is captured through OpenInference; no fixture emits the legacy AutoGen body shape",
        ),
        (
            "autogen.log_body",
            "the suite is captured through OpenInference; no fixture emits the legacy AutoGen log-body shape",
        ),
        (
            "autogen.message",
            "the suite is captured through OpenInference; no fixture emits the legacy bare-message shape",
        ),
        (
            "google-adk.data",
            "the suite is captured; `gcp.vertex.agent.data` appears in no fixture",
        ),
        // The two ADK leaves that read a **tool execution** span, which the branch may not - and the reason is
        // measured rather than assumed. Granting the permission to `tool_response` alone (the per-leaf, per-axis
        // filter keeps its siblings out) does repair the span views: an ADK tool span goes from 0 messages to
        // its 1 result, which is what it carried. It also *duplicates the trace view* - 7 messages to 10 on
        // `adk/tool_use` - and neither route out works:
        //
        // - Attaching the span's `gen_ai.tool.call.id` makes the result an orphan: ADK's tool spans carry real
        //   `tooluse_*` ids while the model's own `tool_use` blocks arrive with **no** id and get a synthesised
        //   one, so the two are different id spaces. The tool-id correspondence invariant catches it.
        // - Leaving it id-less relies on content identity, and the raw response normalises to a different value
        //   from the copy the model re-sends, so two of the three still stand beside it.
        //
        // A blank span view is the lesser fault: a duplicated turn in the trace view is what a user reads as
        // the model having answered twice. Closing this needs evidence the telemetry does not carry - a link
        // between ADK's tool-span call id and the call the model actually issued.
        (
            "google-adk.tool_call_args",
            "reads a tool execution span, which this branch may not - see the note above",
        ),
        (
            "google-adk.tool_response",
            "reads a tool execution span, which this branch may not - see the note above",
        ),
        (
            "langgraph.message",
            "the suite is captured; no fixture writes a single message under a bare `message` key",
        ),
        ("langsmith.completion", "no captured fixture for the suite"),
        ("langsmith.prompt", "no captured fixture for the suite"),
        ("livekit.chat_ctx", "no captured fixture for the suite"),
        (
            "livekit.function_tools",
            "no captured fixture for the suite",
        ),
        ("livekit.instructions", "no captured fixture for the suite"),
        (
            "livekit.response_calls_only",
            "no captured fixture for the suite",
        ),
        ("livekit.response_text", "no captured fixture for the suite"),
        (
            "livekit.tool_arguments",
            "no captured fixture for the suite",
        ),
        ("livekit.tool_output", "no captured fixture for the suite"),
        ("livekit.user_input", "no captured fixture for the suite"),
        ("mlflow.chat_tools", "no captured fixture for the suite"),
        ("mlflow.span_inputs", "no captured fixture for the suite"),
        ("mlflow.span_outputs", "no captured fixture for the suite"),
        (
            "openinference.embedding_text",
            "the dialect is captured; no fixture has an embedding span",
        ),
        (
            "openinference.tools",
            "the dialect is captured; no fixture states its tools under this key",
        ),
        (
            "pydantic-ai.tool_arguments",
            "Pydantic AI 2.50 emits gen_ai.tool.call.arguments instead of this legacy key",
        ),
        (
            "pydantic-ai.tool_response",
            "Pydantic AI 2.50 emits gen_ai.tool.call.result instead of this legacy key",
        ),
        (
            "semconv.indexed_completion",
            "no captured producer uses the indexed convention spelling",
        ),
        (
            "semconv.indexed_prompt",
            "no captured producer uses the indexed convention spelling",
        ),
    ];

    let declared = declared_rule_ids();
    let fired = rules_that_emit();
    let exempt: BTreeSet<String> = UNREACHED.iter().map(|(id, _)| id.to_string()).collect();

    let silent: Vec<&String> = declared
        .iter()
        .filter(|id| !fired.contains(*id) && !exempt.contains(*id))
        .collect();
    assert!(
        silent.is_empty(),
        "{} of {} declared rules emit nothing anywhere in the corpus - each is dead, or exercised by no \
         captured request. Add it to UNREACHED with the reason, or fix the rule:\n{}",
        silent.len(),
        declared.len(),
        silent
            .iter()
            .map(|id| format!("  {id}"))
            .collect::<Vec<_>>()
            .join("\n")
    );

    let now_firing: Vec<&(&str, &str)> = UNREACHED
        .iter()
        .filter(|(id, _)| fired.contains(*id))
        .collect();
    assert!(
        now_firing.is_empty(),
        "listed as unreached and now firing - remove from UNREACHED: {now_firing:?}"
    );
    // A rule id in the list that no longer exists is a stale excuse.
    let unknown: Vec<&(&str, &str)> = UNREACHED
        .iter()
        .filter(|(id, _)| !declared.contains(*id))
        .collect();
    assert!(
        unknown.is_empty(),
        "UNREACHED names rules that do not exist: {unknown:?}"
    );
}
