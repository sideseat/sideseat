
/// A conversation that asked something must show an answer.
///
/// The other invariants are all about *not* returning the wrong thing - scope, duplicates, pairing.
/// None of them notices content that never arrives, which is the failure mode of a broken extractor:
/// the feed looks orderly and is missing the reply.
///
/// `ordered` asks the stronger question - that the *last* turn was answered - and is false only for
/// the project feed, whose order is descending across responses and ascending within one, so no
/// position in it is the last turn.
fn assert_has_an_answer(
    label: &str,
    view_name: &str,
    rows: &[InvariantRow],
    ordered: bool,
    exempt: &[(&str, &str)],
) {
    if let Some((_, reason)) = exempt.iter().find(|(l, _)| *l == label) {
        eprintln!("message_goldens: {label}: answer check skipped - {reason}");
        return;
    }

    if !ordered {
        if !rows.iter().any(|r| r.role == "user") {
            return;
        }
        assert!(
            rows.iter()
                .any(|r| r.role == "assistant" || r.role == "tool"),
            "{label} / {view_name}: {} messages, a question among them, and nothing from the \
             assistant or a tool anywhere",
            rows.len()
        );
        return;
    }

    // Keyed on the *last* question, not on whether the view holds an answer anywhere. "Some
    // assistant message exists" is satisfied by an earlier turn's reply, so a view that answered
    // turn 1 and lost the answer to turn 2 passed - which is most of the CrewAI defect, since
    // every one of its runs kept the history and only ever dropped the current reply.
    let Some(last_question) = rows.iter().rposition(|r| r.role == "user") else {
        return;
    };
    let answered = rows[last_question + 1..]
        .iter()
        .any(|r| r.role == "assistant" || r.role == "tool");
    assert!(
        answered,
        "{label} / {view_name}: {} messages, the last of them a user message at index \
         {last_question} with nothing from the assistant or a tool after it - the reply to the \
         final turn is missing rather than merely out of order",
        rows.len()
    );
}

/// Invariant 5: a result follows the call it answers.
///
/// Causality, not adjacency - Vercel emits `call, call, result, result`. Calls are queued per
/// `(trace, id)`, so a producer may reuse one id for sequential executions without letting the second
/// result match the first call. The project feed is exempt because it is newest-first across responses.
///
/// A cross-span tie used to break this: an index restarts at zero in every span, so the tool span's
/// result could sort before the generation span's call. That is what `adopt_call_positions` settles,
/// and this is the property that says so.
fn assert_tool_causality(label: &str, view_name: &str, rows: &[InvariantRow]) {
    let calls: HashSet<(&str, &str)> = rows
        .iter()
        .filter(|row| row.entry_type == "tool_use")
        .filter_map(|row| {
            row.tool_use_id
                .as_deref()
                .filter(|id| !id.is_empty())
                .map(|id| (row.trace_id.as_str(), id))
        })
        .collect();
    let mut available: HashMap<(&str, &str), VecDeque<usize>> = HashMap::new();
    for (position, row) in rows.iter().enumerate() {
        let Some(id) = row.tool_use_id.as_deref().filter(|s| !s.is_empty()) else {
            continue;
        };
        let key = (row.trace_id.as_str(), id);
        if row.entry_type == "tool_use" {
            available.entry(key).or_default().push_back(position);
        } else if row.entry_type == "tool_result" && calls.contains(&key) {
            let call_position = available.entry(key).or_default().pop_front();
            assert!(
                call_position.is_some(),
                "{label} / {view_name}: result occurrence for {id} is at index {position} before \
                 any unanswered call occurrence with that id"
            );
        }
    }
}

/// Invariant 3: survivors from one carrier keep the order that carrier stated.
///
/// A carrier states order only where it *has* one: inside an array. Two entries of `messages` are
/// ordered by their indices, and a block from `system` versus one from `messages` is not - object
/// members have no order, and Anthropic's payload puts its system prompt in a sibling member of the
/// array, which the pipeline deliberately renders first. So the comparison applies to paths that
/// diverge at an index, and to nothing else.
///
/// Scoped per (trace, span, carrier): across carriers or spans, order comes from other evidence, and
/// demanding a global position order would falsely accuse every re-sent history. In the project feed
/// it is scoped per response as well, because the feed descends across responses by contract and one
/// carrier can hold several - a client that runs its own tool loop reports every round on one span.
fn assert_carrier_subsequence(
    label: &str,
    view_name: &str,
    rows: &[InvariantRow],
    per_response: bool,
) {
    /// One carrier of one span, and in the feed one response of it.
    type CarrierKey<'a> = (
        &'a str,
        &'a str,
        &'a str,
        Option<chrono::DateTime<chrono::Utc>>,
    );
    /// A block of that carrier: (position path, index in the returned feed).
    type Placed<'a> = (&'a str, usize);

    let mut seen: HashMap<CarrierKey<'_>, Vec<Placed<'_>>> = HashMap::new();
    for (position, row) in rows.iter().enumerate() {
        // A synthesised block has no place in any payload, so there is no order to keep.
        if row.carrier == "synthesised" || row.position.is_empty() {
            continue;
        }
        // Nor does a carrier that states no order. This is the other half of "a carrier states order
        // only where it has one": the array-divergence test below covers object members, and this
        // covers carriers whose positions are not sequence evidence at all.
        if !row.carrier_orders_positions {
            continue;
        }
        seen.entry((
            row.trace_id.as_str(),
            row.span_id.as_str(),
            row.carrier.as_str(),
            per_response.then_some(row.order_time),
        ))
        .or_default()
        .push((row.position.as_str(), position));
    }

    for ((_, span_id, carrier, _), blocks) in seen {
        for (i, (earlier_path, earlier_index)) in blocks.iter().enumerate() {
            for (later_path, later_index) in blocks.iter().skip(i + 1) {
                let Some((earlier_sibling, later_sibling)) =
                    diverging_array_indices(earlier_path, later_path)
                else {
                    continue;
                };
                assert!(
                    earlier_sibling < later_sibling,
                    "{label} / {view_name}: carrier {carrier} of span {span_id} returned {later_path} \
                     at index {later_index} before {earlier_path} at index {earlier_index}, but the \
                     payload lists them the other way round"
                );
            }
        }
    }
}

/// The pair of array indices at which two position paths diverge, if they diverge at one.
///
/// `None` when they diverge at an object member - where the payload states no order - or when one path
/// is a prefix of the other, which is a parent and its child rather than two siblings.
fn diverging_array_indices(left: &str, right: &str) -> Option<(usize, usize)> {
    for (left_segment, right_segment) in left.split('.').zip(right.split('.')) {
        if left_segment == right_segment {
            continue;
        }
        let left_index = left_segment.parse::<usize>().ok()?;
        let right_index = right_segment.parse::<usize>().ok()?;
        return Some((left_index, right_index));
    }
    None
}

fn assert_tool_pairing(label: &str, view_name: &str, rows: &[InvariantRow]) {
    // Exempt fixtures skip only the "result must match a call" assertion, which their source
    // cannot satisfy. The duplicate-answer check below still applies: nothing about the Claude
    // CLI's subagent reporting makes it legitimate to render one invocation twice.
    let exempt = PAIRING_EXEMPT.iter().find(|(l, _)| *l == label);
    if let Some((_, reason)) = exempt {
        eprintln!("message_goldens: {label}: unmatched-result check skipped - {reason}");
    }
    let mut calls: BTreeMap<(&str, String), usize> = BTreeMap::new();
    for r in rows {
        if r.entry_type == "tool_use"
            && let Some(id) = extract_tool_use_id(r)
        {
            *calls
                .entry((r.trace_id.as_str(), id.to_string()))
                .or_insert(0) += 1;
        }
    }
    let mut answered: BTreeMap<(&str, String), usize> = BTreeMap::new();
    for r in rows {
        if r.entry_type != "tool_result" {
            continue;
        }
        let Some(id) = r.tool_use_id.as_deref() else {
            continue; // id-less results are matched by content by the pipeline
        };
        let key = (r.trace_id.as_str(), id.to_string());
        assert!(
            exempt.is_some() || calls.contains_key(&key),
            "{label} / {view_name}: tool_result at index {} has id {id:?} with no matching tool_use in trace {}",
            r.index,
            &r.trace_id[..r.trace_id.len().min(8)]
        );
        *answered.entry(key).or_insert(0) += 1;
    }
    for ((trace, id), n) in &answered {
        let Some(call_count) = calls.get(&(*trace, id.clone())) else {
            continue; // unmatched result: already handled (or exempted) above
        };
        assert!(
            n <= call_count,
            "{label} / {view_name}: tool_use id {id:?} in trace {} has {n} results but only {} call(s) - the same invocation is rendered more than once",
            &trace[..trace.len().min(8)],
            call_count
        );
    }
}

/// The projection itself must be self-consistent.
///
/// Deliberately narrow. Two earlier checks here were theatre: dense ascending indices cannot
/// fail because the projection assigns them with `enumerate()`, and "roles are one of four
/// known values" cannot fail because `ChatRole` has exactly four variants — an unmapped source
/// role has already become `User` by this point, silently, which is the defect those checks
/// looked like they were guarding. What remains are the fields that could genuinely disagree.
fn assert_projection_consistent(label: &str, view_name: &str, view: &GoldenView) {
    assert_eq!(
        view.role_sequence.len(),
        view.message_count,
        "{label} / {view_name}: role_sequence and message_count disagree"
    );
    assert_eq!(
        view.messages.len(),
        view.message_count,
        "{label} / {view_name}: message list and message_count disagree"
    );
    for m in &view.messages {
        assert!(
            !m.content_digest.is_empty(),
            "{label} / {view_name}: message {} has no content digest",
            m.index
        );
    }
}

/// Every returned block must belong to the scope that was requested.
///
/// This is the property the endpoints promise and the one a scoping bug breaks: a span view
/// leaking a sibling span's messages, or a trace view leaking another trace's, is invisible to
/// count and ordering checks because the totals still look plausible. Compared against exact
/// ids rather than a key prefix.
fn assert_scope(label: &str, view_name: &str, scope: &Scope, rows: &[InvariantRow]) {
    match scope {
        Scope::Span { trace_id, span_id } => {
            for r in rows {
                assert_eq!(
                    &r.trace_id, trace_id,
                    "{label} / {view_name}: block from another trace leaked into a span view"
                );
                assert_eq!(
                    &r.span_id, span_id,
                    "{label} / {view_name}: block from another span leaked into a span view"
                );
            }
        }
        Scope::Trace { trace_id } => {
            for r in rows {
                assert_eq!(
                    &r.trace_id, trace_id,
                    "{label} / {view_name}: block from another trace survived scope_feed_to_trace"
                );
            }
        }
        // A session legitimately spans traces, and the project feed spans everything, so there is
        // nothing to constrain in either.
        Scope::Session | Scope::Feed => {}
    }
}

/// A session's trace views must partition its session view exactly.
///
/// The trace endpoint loads the whole session, runs the same pipeline, then retains only the
/// requested trace's blocks; the session endpoint returns all of them. So summing the trace
/// views of one session must equal that session's view. This catches `scope_feed_to_trace`
/// dropping or duplicating blocks, and it is strictly stronger than what it replaced.
///
/// The previous check here - "a trace whose spans carry messages is not itself empty" - is
/// false. A trace can legitimately scope to nothing: `langgraph/rag_local` has 18 traces in one
/// session, 15 of which are pure cross-trace replays whose content is stripped as history and
/// only shown on the trace that first sent it.
fn assert_session_partitions_into_traces(
    label: &str,
    golden: &Golden,
    traces_of_session: &BTreeMap<String, BTreeSet<String>>,
    trace_labels: &BTreeMap<String, String>,
) {
    for (session_id, session_view) in &golden.session_views {
        let Some(traces) = traces_of_session.get(session_id) else {
            continue;
        };
        // A trace can carry two session ids (Google ADK emits its own plus the sample's).
        // Production builds that trace's view from whichever session it resolves to, so the
        // partition only holds for that session; skip the others rather than assert something
        // the API would not produce.
        // The partition is asserted for **every** session now. It used to `continue` for a session sharing
        // a trace with another - ADK emits its own session id alongside the sample's, so a trace appeared
        // under two - which skipped the check for exactly the fixtures where it mattered. A trace belongs to
        // one session, so that no longer happens.
        //
        // There is deliberately no assertion here that a session claims no foreign trace: this harness
        // *derives* `traces_of_session` from `session_of_trace`, so such a check compares a map with itself
        // and cannot fail. The production rule is proved where it is implemented - the DuckDB query tests and
        // the ClickHouse parity suite - not by a tautology here. What this loop does prove is that the
        // recorded trace views and the recorded session view agree, which are two independent numbers.
        let expected: usize = traces
            .iter()
            .filter_map(|trace_id| trace_labels.get(trace_id))
            .filter_map(|lbl| golden.trace_views.get(lbl))
            .map(|v| v.message_count)
            .sum();
        assert_eq!(
            expected, session_view.message_count,
            "{label} / session {session_id}: trace views sum to {expected} but the session view \
             has {} - scoping lost or duplicated blocks",
            session_view.message_count
        );
    }
}

/// Content must not be empty for text-bearing entries: an empty bubble in the UI is
/// indistinguishable from a parsing failure.
fn assert_no_empty_text(label: &str, view_name: &str, view: &GoldenView) {
    for m in &view.messages {
        if matches!(m.entry_type.as_str(), "text" | "thinking") {
            assert!(
                !m.content.trim().is_empty(),
                "{label} / {view_name}: empty {} block at index {}",
                m.entry_type,
                m.index
            );
        }
    }
}

fn check_invariants(label: &str, built: &Built) {
    let golden = &built.golden;
    let views: Vec<(String, &GoldenView)> = golden
        .span_views
        .iter()
        .map(|(k, v)| (format!("span {k}"), v))
        .chain(
            golden
                .trace_views
                .iter()
                .map(|(k, v)| (format!("trace {k}"), v)),
        )
        .chain(
            golden
                .session_views
                .iter()
                .map(|(k, v)| (format!("session {k}"), v)),
        )
        .chain(std::iter::once(("feed".to_string(), &golden.feed_view)))
        .collect();

    for (name, view) in &views {
        assert_projection_consistent(label, name, view);
        assert_no_empty_text(label, name, view);
    }

    for (name, scope, rows) in &built.invariants {
        assert_scope(label, name, scope, rows);
        assert_no_duplicates(label, name, rows);
        assert_carrier_subsequence(label, name, rows, matches!(scope, Scope::Feed));
        // Causality applies to every chronological view, span views included. It reads only *matched*
        // pairs, so it is vacuous for the usual span holding one half - while a span that holds both,
        // as an ADK `call_llm` does, is exactly where an inversion hides: one such view listed both
        // tool results ahead of both calls and no invariant objected, because the blanket exemption
        // covered it. The project feed keeps its exemption: it descends across responses by contract,
        // so a call and an earlier response's result do appear reversed there.
        if !matches!(scope, Scope::Feed) {
            assert_tool_causality(label, name, rows);
        }
        // Pairing stays span-exempt: a single span usually holds only one half, so the *other* half's
        // id is legitimately absent and demanding it would accuse every ordinary span view.
        if !name.starts_with("span ") {
            assert_tool_pairing(label, name, rows);
            // Span views are excluded here too: one span legitimately holds only the request or
            // only the reply.
            //
            // The feed gets the weaker form. Its order is descending across responses and ascending
            // within one, so no single position is "the last turn" - reversing it does not give
            // chronological order either. What still holds, and would still have caught a whole
            // framework's answers going missing, is that a feed showing a question shows something
            // that answered one.
            let ordered = !matches!(scope, Scope::Feed);
            assert_has_an_answer(label, name, rows, ordered, NO_ANSWER_EXPECTED);
        }
    }

    assert_session_partitions_into_traces(
        label,
        golden,
        &built.traces_of_session,
        &built.trace_labels,
    );

    // Not every sample uses sessions, so assert on traces: those always exist.
    let total: usize = golden.trace_views.values().map(|v| v.message_count).sum();
    assert!(
        total > 0,
        "{label}: every trace view is empty - the sample produced no messages at all"
    );
}

fn golden_path(label: &str) -> PathBuf {
    fixture_root().join(label).join("expected.json")
}

#[test]
fn message_goldens() {
    let fixtures = discover_fixtures();
    if fixtures.is_empty() {
        // Not a silent pass: capturing fixtures needs credentials and a live model, so a
        // clean checkout legitimately has none. Say so loudly instead of reporting success.
        eprintln!(
            "message_goldens: no fixtures under {} - run make capture P=<producer>",
            fixture_root().display()
        );
        return;
    }

    let update = std::env::var("UPDATE_GOLDENS").is_ok();
    let mut failures: Vec<String> = Vec::new();
    let mut violations: Vec<String> = Vec::new();
    let mut checked = 0usize;

    for (label, paths) in &fixtures {
        let rows = rows_for(paths);
        let built = build_golden(label, paths, &rows);
        let golden = &built.golden;

        if update {
            // Report violations rather than aborting: the point of a record run is to see
            // the whole picture, including which fixtures are currently wrong. Aborting on
            // the first one hides the rest.
            if let Err(e) = std::panic::catch_unwind(|| check_invariants(label, &built)) {
                let msg = e
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_string()))
                    .unwrap_or_else(|| "unknown".to_string());
                eprintln!("message_goldens: INVARIANT VIOLATION while recording {label}:\n  {msg}");
                violations.push(format!("{label}: {msg}"));
            }
        } else {
            check_invariants(label, &built);
        }

        let path = golden_path(label);
        if update {
            std::fs::create_dir_all(path.parent().unwrap()).ok();
            let json = serde_json::to_string_pretty(golden).unwrap();
            std::fs::write(&path, json + "\n").unwrap();
            eprintln!("message_goldens: wrote {}", path.display());
            continue;
        }

        let Ok(existing) = std::fs::read_to_string(&path) else {
            failures.push(format!(
                "{label}: no expectation file at {} (UPDATE_GOLDENS=1 to record)",
                path.display()
            ));
            continue;
        };
        let expected: Golden = match serde_json::from_str(&existing) {
            Ok(v) => v,
            Err(e) => {
                failures.push(format!("{label}: expectation file is not valid: {e}"));
                continue;
            }
        };
        checked += 1;

        if expected != *golden {
            failures.push(describe_diff(label, &expected, golden));
        }
    }

    if update {
        eprintln!("message_goldens: recorded {} fixture(s)", fixtures.len());
        // Recording still fails when an invariant was violated. Writing the files first is
        // deliberate - you want to see the whole picture - but exiting 0 would let a
        // known-bad expectation be committed as if it had been reviewed and accepted.
        assert!(
            violations.is_empty(),
            "recorded {} fixture(s) but {} violated an invariant; the written expectations \
             capture current (wrong) behaviour and must not be committed as-is:\n\n{}",
            fixtures.len(),
            violations.len(),
            violations.join("\n\n")
        );
        return;
    }

    assert!(
        failures.is_empty(),
        "message parsing changed for {} of {} fixture(s):\n\n{}",
        failures.len(),
        fixtures.len(),
        failures.join("\n\n")
    );
    eprintln!("message_goldens: {checked} fixture(s) matched");
}

/// Producers whose fixtures are SDK conformance programs rather than frameworks.
const CONFORMANCE_LANGUAGES: &[&str] = &["dotnet", "javascript", "python", "rust"];

/// SideSeat SDK setup must not change the conversation a user sees compared with a
/// standards-only OpenTelemetry setup emitting the same spans.
///
/// Conformance fixtures are paired as `<language>/native/<sample>` (plain OpenTelemetry) and
/// `<language>/sdk/<sample>`. Every view is compared, including span topology and session
/// grouping, so parity cannot pass by checking only a flattened message feed.
///
/// Span ids are regenerated on every run. The golden builder replaces them with `span-N` labels
/// ordered by timestamp, name, and user-visible projection; random ids only distinguish spans whose
/// golden views are identical. Parity still strips that final synthetic number as defence in depth
/// and compares the resulting `(trace/name, view)` multiset; span names, counts, per-span messages,
/// trace views and session views remain exact.
#[test]
fn sdk_and_plain_otel_conformance_are_identical() {
    let fixtures: BTreeMap<String, Vec<PathBuf>> = discover_fixtures().into_iter().collect();
    let mut compared = 0usize;

    // The languages are listed rather than discovered so a missing pair fails instead of shrinking
    // the comparison.
    const LANGUAGES: &[&str] = CONFORMANCE_LANGUAGES;
    const SAMPLE: &str = "canonical";
    for language in LANGUAGES {
        let sdk_label = format!("{language}/sdk/{SAMPLE}");
        let otel_label = format!("{language}/native/{SAMPLE}");
        let sdk_paths = fixtures
            .get(&sdk_label)
            .unwrap_or_else(|| panic!("missing SideSeat SDK conformance fixture {sdk_label}"));
        let otel_paths = fixtures.get(&otel_label).unwrap_or_else(|| {
            panic!("missing raw OpenTelemetry conformance fixture {otel_label}")
        });

        let sdk_rows = rows_for(sdk_paths);
        let otel_rows = rows_for(otel_paths);
        let sdk = build_golden(&sdk_label, sdk_paths, &sdk_rows).golden;
        let otel = build_golden(&otel_label, otel_paths, &otel_rows).golden;

        assert_eq!(
            sdk.span_count, otel.span_count,
            "{sdk_label}: SDK and raw OTel produced different span topology"
        );
        assert_eq!(
            sdk.trace_count, otel.trace_count,
            "{sdk_label}: trace count"
        );
        assert_eq!(
            sdk.session_count, otel.session_count,
            "{sdk_label}: session count"
        );
        let comparable_span_views =
            |views: &BTreeMap<String, GoldenView>| -> Vec<(String, String)> {
                let mut comparable: Vec<(String, String)> = views
                    .iter()
                    .map(|(key, view)| {
                        let stable_key = key
                            .rsplit_once("/span-")
                            .map_or(key.as_str(), |(prefix, _)| prefix);
                        (
                            stable_key.to_string(),
                            serde_json::to_string(view).expect("golden view is serializable"),
                        )
                    })
                    .collect();
                comparable.sort();
                comparable
            };
        assert_eq!(
            comparable_span_views(&sdk.span_views),
            comparable_span_views(&otel.span_views),
            "{sdk_label}: span message views differ from raw OTel"
        );
        assert_eq!(
            sdk.trace_views, otel.trace_views,
            "{sdk_label}: trace message views differ from raw OTel"
        );
        assert_eq!(
            sdk.session_views, otel.session_views,
            "{sdk_label}: session message views differ from raw OTel"
        );
        assert_eq!(
            sdk.feed_view, otel.feed_view,
            "{sdk_label}: project feed differs from raw OTel"
        );
        compared += 1;
    }

    assert_eq!(
        compared,
        LANGUAGES.len(),
        "not every declared SDK language had a complete conformance pair"
    );
}

/// A framework instrumented through SideSeat must expose the same conversation as its native
/// instrumentation.
///
/// Framework pairs are `<framework>/native/<sample>` and `<framework>/sdk/<sample>`. Their resource
/// attributes, span IDs, and measured duration embedded in Logfire's streaming span name can differ,
/// but the transport and user-visible contract are exact:
///
/// - both paths export the same spans (batch boundaries are exporter timing and may differ);
/// - every source span has the same message projection;
/// - the same conversations exist as traces, including empty transport-only traces, with the
///   results of concurrently executed tools compared as a set where they sit next to each other;
/// - the same session conversations exist, independent of producer-side ID scrubbing;
/// - the project feed holds the same messages; its order follows completion time, which concurrent
///   tool execution makes vary between runs, so the per-fixture golden pins the order instead.
///
/// The native Logfire control path installs the same streaming reparenter before its raw OTLP
/// exporter. Without it, Logfire 6 closes the request span before consuming the stream and exports
/// the completed response as an unrelated root trace — telemetry the server cannot safely reconnect.
/// Pairs whose total span count differs between runs for a reason outside the SDK, with the reason.
/// Every other comparison still holds for them, including the per-span message projections.
const VARIABLE_STEP_SPANS: &[(&str, &str)] = &[(
    "llama-index/sdk/tool_use",
    "LlamaIndex runs its aggregate_tool_results step once per tool result as the concurrent tools \
     finish, so the number of those message-less step spans follows completion timing",
)];

#[test]
fn framework_sdk_and_native_conversations_are_identical() {
    let fixtures: BTreeMap<String, Vec<PathBuf>> = discover_fixtures().into_iter().collect();

    // Tools a framework runs concurrently finish in whichever order they finish, so a run of
    // adjacent tool results is compared as a set: the same results, each once, in the same place.
    let with_concurrent_results_unordered = |view: &GoldenView| {
        let mut view = view.clone();
        let mut start = 0;
        while start < view.messages.len() {
            let end = start
                + view.messages[start..]
                    .iter()
                    .take_while(|m| m.entry_type == "tool_result")
                    .count();
            view.messages[start..end].sort_by(|a, b| a.content_digest.cmp(&b.content_digest));
            start = end.max(start + 1);
        }
        for (index, message) in view.messages.iter_mut().enumerate() {
            message.index = index;
        }
        view
    };
    let view_multiset = |views: &BTreeMap<String, GoldenView>| {
        let mut comparable: Vec<String> = views
            .values()
            .map(|view| {
                serde_json::to_string(&with_concurrent_results_unordered(view))
                    .expect("golden view is serializable")
            })
            .collect();
        comparable.sort();
        comparable
    };
    let span_multiset = |views: &BTreeMap<String, GoldenView>| {
        let mut comparable: Vec<(String, String)> = views
            .iter()
            .map(|(key, view)| {
                let after_trace = key.split_once('/').map_or(key.as_str(), |(_, rest)| rest);
                let span_name = after_trace
                    .rsplit_once("/span-")
                    .map_or(after_trace, |(name, _)| name);
                (
                    stable_span_name(span_name),
                    serde_json::to_string(view).expect("golden view is serializable"),
                )
            })
            .collect();
        comparable.sort();
        comparable
    };

    let native_labels: Vec<String> = fixtures
        .keys()
        .filter(|label| {
            let mut parts = label.split('/');
            let producer = parts.next().unwrap_or_default();
            parts.next() == Some("native") && !CONFORMANCE_LANGUAGES.contains(&producer)
        })
        .cloned()
        .collect();
    assert!(
        !native_labels.is_empty(),
        "no framework native/SDK fixture pairs were discovered"
    );

    let mut compared = 0usize;
    for native_label in native_labels {
        let sdk_label = native_label.replacen("/native/", "/sdk/", 1);
        let native_paths = &fixtures[&native_label];
        let sdk_paths = fixtures
            .get(&sdk_label)
            .unwrap_or_else(|| panic!("missing SideSeat framework fixture {sdk_label}"));

        let native_rows = without_run_measurements(rows_for(native_paths));
        let sdk_rows = without_run_measurements(rows_for(sdk_paths));
        let native = build_golden(&native_label, native_paths, &native_rows).golden;
        let sdk = build_golden(&sdk_label, sdk_paths, &sdk_rows).golden;
        let (native, sdk) = with_restored_media_aligned(&sdk_label, native, sdk);

        if !VARIABLE_STEP_SPANS.iter().any(|(label, _)| *label == sdk_label) {
            assert_eq!(
                sdk.span_count, native.span_count,
                "{sdk_label}: SDK changed the number of framework spans"
            );
        }
        assert_eq!(
            sdk.trace_count, native.trace_count,
            "{sdk_label}: SDK changed framework trace topology"
        );
        assert_eq!(
            sdk.session_count, native.session_count,
            "{sdk_label}: SDK changed framework session grouping"
        );
        // Where step spans repeat a variable number of times, each message-less step counts once.
        let projections = |views: &BTreeMap<String, GoldenView>| {
            let mut comparable = span_multiset(views);
            if VARIABLE_STEP_SPANS.iter().any(|(label, _)| *label == sdk_label) {
                comparable.dedup_by(|a, b| a == b && a.1.contains("\"message_count\":0"));
            }
            comparable
        };
        assert_eq!(
            projections(&sdk.span_views),
            projections(&native.span_views),
            "{sdk_label}: per-span message projections differ from native instrumentation"
        );
        assert_eq!(
            view_multiset(&sdk.trace_views),
            view_multiset(&native.trace_views),
            "{sdk_label}: trace conversations differ from native instrumentation"
        );
        assert_eq!(
            view_multiset(&sdk.session_views),
            view_multiset(&native.session_views),
            "{sdk_label}: session conversations differ from native instrumentation"
        );
        // The feed orders by completion time, and concurrently executed tools complete in a
        // different order on every run; the same messages must appear, each once.
        let feed_multiset = |view: &GoldenView| {
            let mut messages: Vec<String> = view
                .messages
                .iter()
                .map(|m| format!("{}|{}|{}", m.role, m.entry_type, m.content_digest))
                .collect();
            messages.sort();
            messages
        };
        assert_eq!(
            feed_multiset(&sdk.feed_view),
            feed_multiset(&native.feed_view),
            "{sdk_label}: project feed differs from native instrumentation"
        );
        compared += 1;
    }

    assert!(
        compared > 0,
        "no framework native/SDK fixture pairs were compared"
    );
}

/// The payload a framework writes in place of binary content it does not export.
const MEDIA_OMITTED: &str = "<replaced>";

/// Aligns a pair where native telemetry omitted media payloads that the SDK restores.
///
/// Some frameworks replace image and document bytes with a placeholder in their own telemetry, and
/// their SideSeat integration exports the bytes instead. That is the one difference parity allows:
/// when the native side carries the placeholder, media blocks on both sides are compared by type
/// alone. The SDK side must never carry the placeholder itself - restoring media is the point.
fn with_restored_media_aligned(label: &str, native: Golden, sdk: Golden) -> (Golden, Golden) {
    let sdk_json = serde_json::to_string(&sdk).expect("golden is serializable");
    assert!(
        !sdk_json.contains(MEDIA_OMITTED),
        "{label}: SDK telemetry lost a media payload the integration should restore"
    );
    if MEDIA_DROPPED_NATIVELY.iter().any(|(declared, _)| *declared == label) {
        let count = |golden: &Golden| {
            golden
                .trace_views
                .values()
                .flat_map(|view| &view.messages)
                .filter(|message| is_media(&message.entry_type))
                .count()
        };
        assert!(
            count(&native) == 0 && count(&sdk) > 0,
            "{label}: declared as media the native telemetry drops and the SDK restores, which no \
             longer holds - remove the declaration"
        );
        return (native, without_media(sdk));
    }
    if !serde_json::to_string(&native).expect("golden is serializable").contains(MEDIA_OMITTED) {
        return (native, sdk);
    }
    fn mask(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                let media = map.get("entry_type").and_then(serde_json::Value::as_str).is_some_and(|kind| {
                    matches!(kind, "image" | "document" | "audio" | "video" | "file")
                });
                if media {
                    map.insert("content".into(), serde_json::Value::String("<media>".into()));
                    map.insert("content_digest".into(), serde_json::Value::String(String::new()));
                }
                map.values_mut().for_each(mask);
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(mask),
            _ => {}
        }
    }
    let masked = |golden: Golden| -> Golden {
        let mut value = serde_json::to_value(golden).expect("golden is serializable");
        mask(&mut value);
        serde_json::from_value(value).expect("masked golden deserializes")
    };
    (masked(native), masked(sdk))
}

/// Human-readable differences between one expected and one actual view.
fn compare_view(name: &str, e: &GoldenView, a: &GoldenView) -> Vec<String> {
    let mut out = Vec::new();
    if e.message_count != a.message_count {
        out.push(format!(
            "  {name}: message_count expected {}, got {}",
            e.message_count, a.message_count
        ));
    }
    if e.role_sequence != a.role_sequence {
        out.push(format!("  {name}: role sequence changed"));
        out.push(format!("    expected: {}", e.role_sequence.join(" -> ")));
        out.push(format!("    actual:   {}", a.role_sequence.join(" -> ")));
    }
    if e.tool_names != a.tool_names {
        out.push(format!(
            "  {name}: tool_names expected {:?}, got {:?}",
            e.tool_names, a.tool_names
        ));
    }
    for (i, (em, am)) in e.messages.iter().zip(a.messages.iter()).enumerate() {
        if em != am {
            out.push(format!("  {name}: message {i} changed"));
            if em.role != am.role || em.entry_type != am.entry_type {
                out.push(format!(
                    "    kind: expected {}/{}, got {}/{}",
                    em.role, em.entry_type, am.role, am.entry_type
                ));
            }
            if em.content != am.content {
                out.push(format!(
                    "    content expected: {}",
                    em.content.chars().take(100).collect::<String>()
                ));
                out.push(format!(
                    "    content actual:   {}",
                    am.content.chars().take(100).collect::<String>()
                ));
            }
            break; // one example per view is enough to diagnose
        }
    }
    out
}

/// Views that appeared or vanished between expectation and actual.
fn key_set_diff(
    kind: &str,
    expected: &BTreeMap<String, GoldenView>,
    actual: &BTreeMap<String, GoldenView>,
) -> Vec<String> {
    let ek: HashSet<&String> = expected.keys().collect();
    let ak: HashSet<&String> = actual.keys().collect();
    let mut gone: Vec<&String> = ek.difference(&ak).copied().collect();
    let mut added: Vec<&String> = ak.difference(&ek).copied().collect();
    gone.sort();
    added.sort();
    let mut out = Vec::new();
    if !gone.is_empty() {
        out.push(format!("  {kind} views missing: {gone:?}"));
    }
    if !added.is_empty() {
        out.push(format!("  {kind} views added: {added:?}"));
    }
    out
}

/// A readable summary rather than two pretty-printed blobs: the useful signal is almost
/// always a count or a role sequence, so lead with those.
fn describe_diff(label: &str, expected: &Golden, actual: &Golden) -> String {
    let mut out = vec![format!("{label}:")];

    if expected.span_count != actual.span_count {
        out.push(format!(
            "  span_count: expected {}, got {}",
            expected.span_count, actual.span_count
        ));
    }
    if expected.trace_count != actual.trace_count {
        out.push(format!(
            "  trace_count: expected {}, got {}",
            expected.trace_count, actual.trace_count
        ));
    }
    if expected.session_count != actual.session_count {
        out.push(format!(
            "  session_count: expected {}, got {}",
            expected.session_count, actual.session_count
        ));
    }

    if expected.request_count != actual.request_count {
        out.push(format!(
            "  request_count: expected {}, got {} (fixture re-captured?)",
            expected.request_count, actual.request_count
        ));
    }

    // Key-set changes were previously invisible, leaving only "differs in a field not
    // summarised above" - useless when a view appeared or vanished.
    out.extend(key_set_diff(
        "trace",
        &expected.trace_views,
        &actual.trace_views,
    ));
    out.extend(key_set_diff(
        "span",
        &expected.span_views,
        &actual.span_views,
    ));
    out.extend(key_set_diff(
        "session",
        &expected.session_views,
        &actual.session_views,
    ));

    for (key, e) in &expected.session_views {
        if let Some(a) = actual.session_views.get(key) {
            out.extend(compare_view(&format!("session {key}"), e, a));
        }
    }

    for (key, e) in &expected.trace_views {
        if let Some(a) = actual.trace_views.get(key) {
            out.extend(compare_view(&format!("trace {key}"), e, a));
        }
    }
    for (key, e) in &expected.span_views {
        if let Some(a) = actual.span_views.get(key) {
            out.extend(compare_view(&format!("span {key}"), e, a));
        }
    }

    // The feed view, which is not keyed - a change in it would otherwise print only "differs in a
    // field not summarised above", which is what this whole function exists to avoid.
    out.extend(compare_view("feed", &expected.feed_view, &actual.feed_view));

    if out.len() == 1 {
        out.push("  differs in a field not summarised above".to_string());
    }
    out.join("\n")
}
