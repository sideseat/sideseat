
/// The harness is only meaningful if the invariant checks can actually fail. Every assertion
/// here also documents a case that a previous version of these checks got wrong.
#[test]
fn invariant_checks_are_not_vacuous() {
    fn row(trace: &str, index: usize, role: &str, kind: &str, content: &str) -> InvariantRow {
        InvariantRow {
            span_path: vec![format!("span-{index}")],
            carrier: "attr:test".to_string(),
            carrier_orders_positions: true,
            carrier_proves_occurrence: false,
            order_time: chrono::DateTime::UNIX_EPOCH,
            position: index.to_string(),
            trace_id: trace.to_string(),
            span_id: "span-1".to_string(),
            index,
            role: role.to_string(),
            entry_type: kind.to_string(),
            content: content.to_string(),
            content_digest: format!("d:{content}"),
            occurrence_ordinal: 0,
            tool_use_id: None,
        }
    }

    fn tool_row(trace: &str, index: usize, kind: &str, id: &str) -> InvariantRow {
        InvariantRow {
            span_path: vec![format!("span-{index}")],
            carrier: "attr:test".to_string(),
            carrier_orders_positions: true,
            carrier_proves_occurrence: false,
            order_time: chrono::DateTime::UNIX_EPOCH,
            position: index.to_string(),
            trace_id: trace.to_string(),
            span_id: "span-1".to_string(),
            index,
            role: if kind == "tool_use" {
                "assistant"
            } else {
                "tool"
            }
            .to_string(),
            entry_type: kind.to_string(),
            content: format!("{{\"id\":\"{id}\"}}"),
            content_digest: format!("d:{kind}:{id}"),
            occurrence_ordinal: (index / 2) as u32,
            tool_use_id: Some(id.to_string()),
        }
    }

    let fires = |f: &dyn Fn()| std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err();

    // Same content twice in the SAME trace is a duplicate.
    let same_trace = vec![
        row("trace-a", 0, "user", "text", "Hello"),
        row("trace-a", 1, "user", "text", "Hello"),
    ];
    assert!(
        fires(&|| assert_no_duplicates("test", "synthetic", &same_trace)),
        "duplicate detection failed to fire for a within-trace duplicate"
    );

    // The same content in DIFFERENT traces is legitimate: a sample that runs the same
    // conversation twice produces exactly this, and flagging it was the original defect.
    let cross_trace = vec![
        row("trace-a", 0, "user", "text", "Hello"),
        row("trace-b", 1, "user", "text", "Hello"),
    ];
    assert!(
        !fires(&|| assert_no_duplicates("test", "synthetic", &cross_trace)),
        "the same content in two different traces must not be reported"
    );

    // A result whose id matches a call is fine, even when the call comes after it in the
    // list: matching is by id, not by position, because the session view interleaves.
    let matched = vec![
        tool_row("trace-a", 0, "tool_result", "id-1"),
        tool_row("trace-a", 1, "tool_use", "id-1"),
    ];
    assert!(
        !fires(&|| assert_tool_pairing("test", "synthetic", &matched)),
        "id-matched pairs must not be reported regardless of order"
    );

    // Parallel tool calls: two calls, two results, interleaved. A stack-based check reported
    // this as an error; matching by id must accept it.
    let parallel = vec![
        tool_row("trace-a", 0, "tool_use", "id-1"),
        tool_row("trace-a", 1, "tool_use", "id-2"),
        tool_row("trace-a", 2, "tool_result", "id-2"),
        tool_row("trace-a", 3, "tool_result", "id-1"),
    ];
    assert!(
        !fires(&|| assert_tool_pairing("test", "synthetic", &parallel)),
        "parallel tool calls must not be reported as mispaired"
    );

    // A result referencing an id no call emitted is a defect.
    let orphan = vec![
        tool_row("trace-a", 0, "tool_use", "id-1"),
        tool_row("trace-a", 1, "tool_result", "id-WRONG"),
    ];
    assert!(
        fires(&|| assert_tool_pairing("test", "synthetic", &orphan)),
        "a tool_result referencing an unknown call id must be reported"
    );

    // Pairing is per trace: a call in one trace must not answer a result in another.
    let cross_trace_pair = vec![
        tool_row("trace-a", 0, "tool_use", "id-1"),
        tool_row("trace-b", 1, "tool_result", "id-1"),
    ];
    assert!(
        fires(&|| assert_tool_pairing("test", "synthetic", &cross_trace_pair)),
        "pairing must be scoped per trace"
    );

    // Scope: a span view must not contain another span's or another trace's block.
    let leaked = vec![InvariantRow {
        carrier_orders_positions: true,
        carrier_proves_occurrence: false,
        order_time: chrono::DateTime::UNIX_EPOCH,
        carrier: "attr:test".to_string(),
        position: "0".to_string(),
        trace_id: "aaaaaaaa1111".to_string(),
        span_id: "bbbb2222".to_string(),
        span_path: vec!["bbbb2222".to_string()],
        index: 0,
        role: "user".to_string(),
        entry_type: "text".to_string(),
        content: "x".to_string(),
        content_digest: "d:x".to_string(),
        occurrence_ordinal: 0,
        tool_use_id: None,
    }];
    let in_scope = Scope::Span {
        trace_id: "aaaaaaaa1111".into(),
        span_id: "bbbb2222".into(),
    };
    let wrong_span = Scope::Span {
        trace_id: "aaaaaaaa1111".into(),
        span_id: "cccc3333".into(),
    };
    let wrong_trace = Scope::Trace {
        trace_id: "ffffffff9999".into(),
    };
    assert!(
        !fires(&|| assert_scope("test", "span x", &in_scope, &leaked)),
        "scope check must accept a block that is in scope"
    );
    assert!(
        fires(&|| assert_scope("test", "span x", &wrong_span, &leaked)),
        "scope check failed to catch a block from another span"
    );
    assert!(
        fires(&|| assert_scope("test", "trace x", &wrong_trace, &leaked)),
        "scope check failed to catch a block from another trace"
    );
    assert!(
        !fires(&|| assert_scope("test", "session x", &Scope::Session, &leaked)),
        "a session view spans traces, so nothing is constrained"
    );

    // Projection consistency: a count that disagrees with the list is a defect.
    let inconsistent = GoldenView {
        message_count: 5,
        role_sequence: vec!["user".into()],
        tool_names: vec![],
        tool_definition_count: 0,
        messages: vec![GoldenMessage {
            index: 0,
            role: "user".into(),
            entry_type: "text".into(),
            content: "a".into(),
            content_digest: "deadbeef".into(),
            tool_name: None,
            finish_reason: None,
            observation_type: None,
        }],
    };
    assert!(
        fires(&|| assert_projection_consistent("test", "synthetic", &inconsistent)),
        "projection consistency check failed to fire"
    );

    // A missing answer. The whole point of this check is a view that looks orderly, so the cases
    // it must fire on are all well-formed.
    let unanswered = vec![row("trace-a", 0, "user", "text", "the question")];
    assert!(
        fires(&|| assert_has_an_answer("test", "synthetic", &unanswered, true, &[])),
        "a question with no reply at all must be reported"
    );

    // The case that matters, and that the first version of this check missed: an earlier turn was
    // answered, the last one was not. "Some assistant message exists" is true here.
    let last_turn_dropped = vec![
        row("trace-a", 0, "user", "text", "first question"),
        row("trace-a", 1, "assistant", "text", "first answer"),
        row("trace-a", 2, "user", "text", "second question"),
    ];
    assert!(
        fires(&|| assert_has_an_answer("test", "synthetic", &last_turn_dropped, true, &[])),
        "an unanswered final turn must be reported even when an earlier turn was answered"
    );

    let answered = vec![
        row("trace-a", 0, "user", "text", "first question"),
        row("trace-a", 1, "assistant", "text", "first answer"),
        row("trace-a", 2, "user", "text", "second question"),
        row("trace-a", 3, "assistant", "text", "second answer"),
    ];
    assert!(
        !fires(&|| assert_has_an_answer("test", "synthetic", &answered, true, &[])),
        "a complete conversation must pass"
    );

    // A tool result counts as an answer: the turn was acted on, and the reply may be in a span
    // this view does not contain.
    let answered_by_tool = vec![
        row("trace-a", 0, "user", "text", "the question"),
        row("trace-a", 1, "tool", "tool_result", "the result"),
    ];
    assert!(
        !fires(&|| assert_has_an_answer("test", "synthetic", &answered_by_tool, true, &[])),
        "a tool result is an answer"
    );

    // Nothing was asked, so nothing is owed - a tool span's view holds no user message.
    let no_question = vec![row("trace-a", 0, "assistant", "tool_use", "call")];
    assert!(
        !fires(&|| assert_has_an_answer("test", "synthetic", &no_question, true, &[])),
        "a view with no question must not be required to hold an answer"
    );

    // The exemption mechanism still has to work, and it is tested with its *own* list rather than a
    // production entry: `NO_ANSWER_EXPECTED` is empty now that the defect behind its only entry is
    // fixed, and putting a placeholder there to keep a test alive would be a false statement in
    // production code about a fixture.
    assert!(
        !fires(&|| assert_has_an_answer(
            "some/fixture",
            "synthetic",
            &unanswered,
            true,
            &[("some/fixture", "declared unanswerable in the source")]
        )),
        "an exempt fixture must skip the check"
    );

    // The feed's weaker branch: it cannot ask about the last turn, but it must still fire when a
    // question has no answer anywhere - the shape of a whole framework's replies going missing.
    assert!(
        fires(&|| assert_has_an_answer("test", "synthetic", &unanswered, false, &[])),
        "the unordered form must still report a question with no answer at all"
    );
    // And it must accept what it cannot judge: an answer before its question is ordinary in a feed,
    // which descends across responses.
    let newest_first = vec![
        row("trace-a", 0, "assistant", "text", "second answer"),
        row("trace-a", 1, "user", "text", "second question"),
    ];
    assert!(
        !fires(&|| assert_has_an_answer("test", "synthetic", &newest_first, false, &[])),
        "the unordered form must not require the answer to follow the question"
    );
    assert!(
        fires(&|| assert_has_an_answer("test", "synthetic", &newest_first, true, &[])),
        "and the ordered form must still reject that same list, or the two forms are the same check"
    );
}

#[test]
fn canonical_labels_ignore_regenerated_trace_and_span_ids() {
    let (label, paths) = discover_fixtures()
        .into_iter()
        .find(|(label, _)| label == "javascript/sdk/canonical")
        .expect("JavaScript SDK conformance fixture");
    let rows = rows_for(&paths);
    let expected = build_golden(&label, &paths, &rows).golden;

    let trace_ids: Vec<String> = rows
        .iter()
        .map(|(_, row)| row.trace_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let span_ids: Vec<String> = rows
        .iter()
        .map(|(_, row)| row.span_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let trace_map: BTreeMap<String, String> = trace_ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            (
                id.clone(),
                format!("{:032x}", trace_ids.len().saturating_sub(index)),
            )
        })
        .collect();
    let span_map: BTreeMap<String, String> = span_ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            (
                id.clone(),
                format!("{:016x}", span_ids.len().saturating_sub(index)),
            )
        })
        .collect();

    let remapped: Vec<(String, MessageSpanRow)> = rows
        .into_iter()
        .map(|(name, mut row)| {
            row.trace_id = trace_map[&row.trace_id].clone();
            row.parent_span_id = row
                .parent_span_id
                .as_ref()
                .and_then(|parent| span_map.get(parent))
                .cloned();
            row.span_id = span_map[&row.span_id].clone();
            (name, row)
        })
        .collect();
    let actual = build_golden(&label, &paths, &remapped).golden;

    assert_eq!(
        actual, expected,
        "golden labels must not depend on regenerated trace or span ids"
    );
}

/// `passes_content_filter` reimplements the SQL predicate in Rust, so the two can drift: adding
/// a condition to the query would silently leave the harness feeding rows the API never
/// returns. This pins the coupling by checking the constant still mentions exactly the columns
/// the Rust version tests.
#[test]
fn content_filter_matches_the_sql_predicate() {
    use sideseat_query_sql::display::MESSAGE_CONTENT_FILTER;

    // The exact predicate, not a substring or clause count: checking only that the column names
    // appear left an inverted operator (`=` for `!=`) or a changed literal ('ERROR' -> 'error')
    // passing while `passes_content_filter` kept the old meaning.
    const EXPECTED: &str = "(messages != '[]' OR tool_definitions != '[]' OR tool_names != '[]' OR status_code = 'ERROR')";
    assert_eq!(
        MESSAGE_CONTENT_FILTER, EXPECTED,
        "the SQL predicate changed; re-derive passes_content_filter from it, then update this \
         expectation in the same commit"
    );
}

/// Processing the same fixture twice must give the same answer.
///
/// Not hypothetical: the output sort's tie-break omitted content, so two blocks sharing span,
/// message index, entry index, entry type and role but differing in content were left in
/// HashMap order. Repeated runs over one real fixture disagreed on 3 then 4 views — two
/// identical API requests could return the same messages in a different order.
#[test]
fn processing_is_deterministic() {
    let fixtures = discover_fixtures();
    if fixtures.is_empty() {
        eprintln!("processing_is_deterministic: no fixtures - skipping");
        return;
    }
    // One fixture per suite rather than the first eight alphabetically, which covered only
    // _synthetic and early ADK samples and left every other framework's ordering untested.
    let mut per_suite: BTreeMap<&str, &(String, Vec<PathBuf>)> = BTreeMap::new();
    for f in &fixtures {
        let suite = f.0.split('/').next().unwrap_or("");
        per_suite.entry(suite).or_insert(f);
    }
    for (label, paths) in per_suite.into_values() {
        let first = build_golden(label, paths, &rows_for(paths)).golden;
        let second = build_golden(label, paths, &rows_for(paths)).golden;
        assert!(
            first == second,
            "{label}: two identical runs produced different output:\n{}",
            describe_diff(label, &first, &second)
        );
    }
}

/// Invariant 1: redundant evidence changes nothing.
///
/// A retried OTLP delivery is the same span twice. Reconstruction must reach the same answer from the
/// duplicate as from the original - not merely the same *count*, which the existing property test
/// checks, but the same messages in the same order.
///
/// This is one of the two properties my carrier-claiming and output-timing experiments violated
/// without any test noticing: both changed order globally, and the only detector was a 107-file diff.
#[test]
fn redundant_evidence_does_not_change_the_answer() {
    let fixtures = discover_fixtures();
    if fixtures.is_empty() {
        eprintln!("redundant_evidence: no fixtures - skipping");
        return;
    }
    let mut per_suite: BTreeMap<&str, &(String, Vec<PathBuf>)> = BTreeMap::new();
    for f in &fixtures {
        let suite = f.0.split('/').next().unwrap_or("");
        per_suite.entry(suite).or_insert(f);
    }

    for (label, paths) in per_suite.into_values() {
        let rows = rows_for(paths);
        let baseline = build_golden(label, paths, &rows).golden;

        // Re-deliver every span: the same rows again, as a retried export would arrive.
        let doubled: Vec<(String, MessageSpanRow)> =
            rows.iter().chain(rows.iter()).cloned().collect();
        let with_duplicates = build_golden(label, paths, &doubled).golden;

        assert!(
            baseline == with_duplicates,
            "{label}: re-delivering every span changed the answer:\n{}",
            describe_diff(label, &baseline, &with_duplicates)
        );
    }
}

/// Invariant 7: the order spans arrive in does not decide the answer.
///
/// Spans reach the pipeline in whatever order a query returned them, and a page of the feed can hold
/// them in a different order again. Anything the answer depends on has to come from the payloads, not
/// from that arrival order - otherwise two identical requests can disagree, and an extraction change
/// that merely shifts arrival order looks like a content change.
#[test]
fn the_order_spans_arrive_in_does_not_change_the_answer() {
    let fixtures = discover_fixtures();
    if fixtures.is_empty() {
        eprintln!("arrival_order: no fixtures - skipping");
        return;
    }
    let mut per_suite: BTreeMap<&str, &(String, Vec<PathBuf>)> = BTreeMap::new();
    for f in &fixtures {
        let suite = f.0.split('/').next().unwrap_or("");
        per_suite.entry(suite).or_insert(f);
    }

    for (label, paths) in per_suite.into_values() {
        let rows = rows_for(paths);
        let baseline = build_golden(label, paths, &rows).golden;

        // Reversed rather than randomly shuffled: deterministic, and the permutation most likely to
        // expose a rule that depends on first-seen order.
        let reversed: Vec<(String, MessageSpanRow)> = rows.iter().rev().cloned().collect();
        let from_reversed = build_golden(label, paths, &reversed).golden;

        assert!(
            baseline == from_reversed,
            "{label}: reversing the order spans arrived in changed the answer:\n{}",
            describe_diff(label, &baseline, &from_reversed)
        );
    }
}

/// Invariant 2, as a metamorphic test: reading a carrier nobody read only *adds*.
///
/// `ExtractionMode::PerCarrier` shares a span's attributes out per carrier instead of giving the whole
/// span to the first extractor that recognises anything. That is a strict increase in what is read, so
/// the answer must be a strict extension of today's: every message that was there before is still
/// there, in the same relative order, with the newly readable carriers' messages interleaved.
///
/// This is the property my first attempt at carrier claiming violated - it repaired the langgraph span
/// views and reordered the trace views - and the only detector at the time was a 107-file snapshot
/// diff. Here the failure names the fixture, the view, and the message that moved.
///
/// The fixtures listed in `REORDERS_UNDER_PER_CARRIER` are the known-bad set: they gain their missing
/// answer *and* reorder. They are named so the defect is in the suite rather than in my head, and each
/// one leaves the list when the ordering work lands.
#[test]
fn reading_more_carriers_only_adds_messages() {
    let fixtures = discover_fixtures();
    if fixtures.is_empty() {
        eprintln!("metamorphic: no fixtures - skipping");
        return;
    }

    let pricing = PricingService::init_for_test().expect("offline pricing service");
    let mut reordered: Vec<String> = Vec::new();
    let mut extended: Vec<String> = Vec::new();

    for (label, paths) in &fixtures {
        let baseline = rows_for_mode(&pricing, paths, ExtractionMode::FirstMatch);
        let per_carrier = rows_for_mode(&pricing, paths, ExtractionMode::PerCarrier);

        let before = build_golden(label, paths, &baseline).golden;
        let after = build_golden(label, paths, &per_carrier).golden;

        // Both views matter, and they answer different questions. A span view is where a carrier
        // nobody read shows up as a *gain* - the langgraph RunnableSequence spans that showed a
        // question and no answer. A trace view is where the same content already existed on another
        // span, so it collapses and only the *order* can change.
        let views = before
            .trace_views
            .iter()
            .map(|(k, v)| (format!("trace {k}"), v, after.trace_views.get(k)))
            .chain(
                before
                    .span_views
                    .iter()
                    .map(|(k, v)| (format!("span {k}"), v, after.span_views.get(k))),
            );

        for (name, before_view, after_view) in views {
            let Some(after_view) = after_view else {
                panic!("{label} / {name}: the view disappeared when more carriers were read");
            };
            if before_view.messages.len() < after_view.messages.len() {
                extended.push(format!("{label} / {name}"));
            }
            // Compared by message *identity*, not by role. A role sequence cannot tell two assistant
            // messages apart, so swapping them read as order-preserving - which is exactly the failure
            // this invariant exists to catch.
            let identities = |view: &GoldenView| -> Vec<String> {
                view.messages
                    .iter()
                    .map(|m| format!("{}/{}/{}", m.role, m.entry_type, m.content_digest))
                    .collect()
            };
            let before_ids = identities(before_view);
            let after_ids = identities(after_view);
            if !is_subsequence(&before_ids, &after_ids) {
                reordered.push(format!(
                    "{label} / {name}: {:?} is not preserved in {:?}",
                    before_view.role_sequence, after_view.role_sequence
                ));
            }
        }
    }

    let unexpected: Vec<&String> = reordered
        .iter()
        .filter(|r| {
            !REORDERS_UNDER_PER_CARRIER
                .iter()
                .any(|(fixture, _)| r.starts_with(fixture))
        })
        .collect();
    assert!(
        unexpected.is_empty(),
        "reading more carriers reordered messages that were already visible:\n  {}",
        unexpected
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );

    // The known-bad list must stay honest in both directions: an entry that no longer reorders has
    // been fixed and should be removed, or the test stops meaning anything.
    for (fixture, reason) in REORDERS_UNDER_PER_CARRIER {
        assert!(
            reordered.iter().any(|r| r.starts_with(fixture)),
            "{fixture} no longer reorders under PerCarrier ({reason}) - remove it from \
             REORDERS_UNDER_PER_CARRIER"
        );
    }

    assert!(
        !extended.is_empty(),
        "reading every carrier added nothing anywhere, so this test compares two identical runs"
    );
    eprintln!(
        "metamorphic: {} view(s) gained messages under PerCarrier, {} reorder (all known)",
        extended.len(),
        reordered.len()
    );
}

/// Fixtures whose already-visible messages *move* when more carriers are read.
///
/// Not an exemption for convenience: each is a defect with a diagnosis, kept in the suite so the
/// ordering work has an acceptance test.
/// Empty, and it must stay empty: reading more carriers may only *add* messages.
///
/// It held `langgraph/` while a `RunnableSequence` span's question and answer could not both be read
/// without shifting the response order. The generation-dataflow constraint removed every one of those
/// ten reorders, so the exemption is gone - and an entry appearing here again means an extraction
/// change is silently moving messages that were already visible, which is the failure mode this whole
/// redesign exists to prevent.
const REORDERS_UNDER_PER_CARRIER: &[(&str, &str)] = &[];

/// A system instruction is the frame a request was made in, so it precedes the request's first user
/// turn.
///
/// Nine of the eleven suites carrying a `tool_use` sample satisfy this, four of them with a
/// byte-identical role/kind shape. The two that do not are the two where the instruction and the
/// question arrive on *different* spans: a framework reports the instruction on the span that sent it -
/// the generation span - while the question came in on the orchestration span that started earlier, so
/// ordering by when the evidence arrived puts the frame after the question.
///
/// This is a **known gap**, not an exemption: the invariant's antecedent is true and the output is
/// wrong. Listing it bidirectionally is what keeps that honest - a new fixture cannot join the list
/// silently, and a fixture that starts passing must be removed. Fixing it needs an ordering *edge*
/// (`system` → the request's other inputs) rather than a key term; two scalar attempts are recorded in
/// `docs/engineering/ingestion-architecture.md`, one of which repaired no trace view at all.
///
/// Scoped to the *first* system and the *first* user message of a trace, which is deliberately weaker
/// than the real relation. The real one is per **request**, one generation invocation's input envelope,
/// and a trace legitimately holds several: `adk/image_gen` carries a second instruction at index 9 after
/// the first turn completed, and `adk/reasoning` repeats one instruction at indices 0, 4 and 8. Comparing
/// firsts is immune to all of that.
///
/// What it cannot see: a trace whose *first* request carries no instruction while a later one does
/// would be accused wrongly. No fixture has that shape - every one of the 27 violations is the
/// one-position displacement above - so the weaker form is sound against this corpus and would need
/// replacing by the per-request relation before it could be trusted against an arbitrary producer.
///
/// Also deliberately not keyed on `role == System`: `developer` normalises to `System`, and an in-band
/// system message inside an ordered array is a turn in that array rather than a frame. Comparing firsts
/// sidesteps that too, where a general rule would have to distinguish them.
/// Fixtures whose trace view cannot order their system instruction first, with the reason. Empty:
/// every captured producer currently satisfies the invariant.
const SYSTEM_FRAME_GAP: &[(&str, &str)] = &[];

/// Every trace view whose system instruction sorts after the first user turn.
fn system_frame_violations(built: &Built) -> Vec<String> {
    let mut out = Vec::new();
    for (name, scope, rows) in &built.invariants {
        if !matches!(scope, Scope::Trace { .. }) {
            continue;
        }
        // The first frame precedes the first turn it frames: one in its own span's subtree. In a
        // multi-agent trace the request to the orchestrator legitimately comes before a sub-agent's
        // instructions, and a later instruction may re-frame a turn that has already completed.
        let Some((system, frame)) = rows.iter().enumerate().find(|(_, r)| r.role == "system") else {
            continue;
        };
        let framed_user = rows
            .iter()
            .position(|r| r.role == "user" && r.span_path.contains(&frame.span_id));
        if let Some(user) = framed_user
            && system > user
        {
            out.push(format!("{name}: system at {system}, first user it frames at {user}"));
        }
    }
    out
}

/// The frame precedes the first turn, and the exceptions are named in both directions.
#[test]
fn a_system_instruction_precedes_the_first_user_turn() {
    let fixtures = discover_fixtures();
    if fixtures.is_empty() {
        eprintln!("system frame: no fixtures - skipping");
        return;
    }

    let pricing = PricingService::init_for_test().expect("offline pricing service");
    let mut violations: Vec<String> = Vec::new();
    for (label, paths) in &fixtures {
        let rows = rows_for_mode(&pricing, paths, ExtractionMode::PerCarrier);
        let built = build_golden(label, paths, &rows);
        for v in system_frame_violations(&built) {
            violations.push(format!("{label} / {v}"));
        }
    }

    let unexpected: Vec<&String> = violations
        .iter()
        .filter(|v| !SYSTEM_FRAME_GAP.iter().any(|(f, _)| v.starts_with(f)))
        .collect();
    assert!(
        unexpected.is_empty(),
        "a system instruction sorted after the first user turn in a fixture that is not a known gap \
         - a frame is not a turn:\n  {}",
        unexpected
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );

    // Both directions, or the list rots into a formality: an entry that now passes has been fixed by
    // the ordering work and must be removed.
    for (fixture, reason) in SYSTEM_FRAME_GAP {
        assert!(
            violations.iter().any(|v| v.starts_with(fixture)),
            "{fixture} now orders its system instruction first ({reason}) - remove it from \
             SYSTEM_FRAME_GAP"
        );
    }
}

/// Replay a fixture through the real ingestion path in a chosen extraction mode.
fn rows_for_mode(
    pricing: &PricingService,
    paths: &[PathBuf],
    mode: ExtractionMode,
) -> Vec<(String, MessageSpanRow)> {
    let mut rows = Vec::new();
    for path in paths {
        let request = decode_request(path);
        rows.extend(normalize_for_test_with_mode(&request, pricing, mode));
    }
    rows
}

/// Whether `needle` appears in `haystack` in order, with gaps allowed.
fn is_subsequence(needle: &[String], haystack: &[String]) -> bool {
    let mut haystack = haystack.iter();
    needle
        .iter()
        .all(|wanted| haystack.any(|candidate| candidate == wanted))
}

#[test]
fn stable_span_names_normalize_runtime_ids_and_durations_only() {
    assert_eq!(
        stable_span_name("invoke_agent 26a6a15d-e291-496f-a550-60c96caa8406"),
        "invoke_agent <uuid>"
    );
    assert_eq!(
        stable_span_name("chat completion took 0.12345s"),
        "chat completion took <duration>s"
    );
    assert_eq!(
        stable_span_name("invoke_agent assistant"),
        "invoke_agent assistant"
    );
    assert_eq!(
        stable_span_name("not-a-uuid-26a6a15d-e291-496f-a550-60c96caa8406"),
        "not-a-uuid-26a6a15d-e291-496f-a550-60c96caa8406"
    );
}

/// The content digest must actually distinguish content, including changes past the preview
/// cutoff and ones that survive whitespace collapse. Otherwise truncating the preview would
/// quietly become the real comparison.
#[test]
fn content_digest_detects_changes_beyond_the_preview() {
    // Same LENGTH, differing only past the preview cutoff. A length change is already
    // caught by the "[N chars]" suffix the preview carries; this is the case only a digest
    // can see.
    let long_a = "x".repeat(MAX_CONTENT + 50);
    let mut long_b = long_a.clone();
    long_b.replace_range(MAX_CONTENT + 10..MAX_CONTENT + 11, "y");

    let a = json!({"type": "text", "text": long_a});
    let b = json!({"type": "text", "text": long_b});
    assert_ne!(
        content_digest(&a),
        content_digest(&b),
        "digest must differ when content changes past the preview cutoff"
    );

    let preview_a = normalise_content(&a);
    let preview_b = normalise_content(&b);
    assert_eq!(
        preview_a, preview_b,
        "the preview is expected to be identical here - that is why the digest is needed"
    );

    // Key order must not matter, or every golden would churn and duplicate detection would miss
    // a repeat whose keys arrived in another order. These two objects are equal but written in
    // different orders, which serde_json/preserve_order keeps distinct in to_string().
    let o1 = json!({"type": "tool_use", "name": "calc", "id": "1"});
    let o2 = json!({"id": "1", "name": "calc", "type": "tool_use"});
    assert_ne!(
        serde_json::to_string(&o1).unwrap(),
        serde_json::to_string(&o2).unwrap(),
        "precondition: preserve_order keeps these textually different"
    );
    assert_eq!(
        content_digest(&o1),
        content_digest(&o2),
        "digest must be canonical, not insertion-ordered"
    );

    // Nested objects too.
    let n1 = json!({"a": {"x": 1, "y": 2}, "b": [{"p": 1, "q": 2}]});
    let n2 = json!({"b": [{"q": 2, "p": 1}], "a": {"y": 2, "x": 1}});
    assert_eq!(content_digest(&n1), content_digest(&n2));
}

// ============================================================================
// Shadow order resolver (increment 1 of the reconstruction redesign)
// ============================================================================

/// Roles/kinds of one trace's blocks, in shadow-resolver order.
fn shadow_order_of(label: &str) -> Vec<(String, String)> {
    let (_, paths) = discover_fixtures()
        .into_iter()
        .find(|(l, _)| l == label)
        .unwrap_or_else(|| panic!("fixture {label} not found"));
    let all: Vec<MessageSpanRow> = rows_for(&paths).into_iter().map(|(_, r)| r).collect();
    let first = all[0].trace_id.clone();
    let rows = sorted_by_timestamp(
        all.into_iter()
            .filter(|r| r.trace_id == first && passes_content_filter(r))
            .collect(),
    );
    shadow_resolved_order(rows)
        .into_iter()
        .map(|b| (b.role.as_str().to_string(), b.entry_type))
        .collect()
}

/// The case that motivated the redesign. On `strands-js/swarm` the intro text and the tool call it
/// introduces are one `gen_ai.choice` emission on the chat span, but dedup keeps the Planner span's
/// re-listed copy of the text, so the scalar sort key detaches the text and the trace view returns
/// `assistant/tool_use, tool/tool_result, assistant/text` — the intro trailing the result it
/// introduces.
///
/// The shadow resolver contracts the emission (so the text stays with its call) and adds the
/// call → result edge, yielding intro, then call, then result. This is the property increment 1
/// exists to prove before any view consumes the new order.
#[test]
fn shadow_resolver_keeps_intro_with_its_call_before_the_result() {
    let order = shadow_order_of("strands-js/legacy/swarm");

    let text = order
        .iter()
        .position(|(r, k)| r == "assistant" && k == "text")
        .expect("an assistant text block");
    let call = order
        .iter()
        .position(|(_, k)| k == "tool_use")
        .expect("a tool_use block");
    let result = order
        .iter()
        .position(|(_, k)| k == "tool_result")
        .expect("a tool_result block");

    assert!(
        text < call,
        "intro text (at {text}) must precede the call it introduces (at {call}); order was {order:?}"
    );
    assert!(
        call < result,
        "the call (at {call}) must precede its result (at {result}); order was {order:?}"
    );
}

/// The resolver is a permutation: it reorders survivors, it does not add, drop or alter them.
#[test]
fn shadow_resolver_is_a_permutation_of_the_survivors() {
    for label in ["strands-js/legacy/swarm", "strands/sdk/tool_use", "strands/sdk/mcp_tools"] {
        let (_, paths) = discover_fixtures()
            .into_iter()
            .find(|(l, _)| l == label)
            .unwrap_or_else(|| panic!("fixture {label} not found"));
        let all: Vec<MessageSpanRow> = rows_for(&paths).into_iter().map(|(_, r)| r).collect();
        let first = all[0].trace_id.clone();
        let rows = sorted_by_timestamp(
            all.into_iter()
                .filter(|r| r.trace_id == first && passes_content_filter(r))
                .collect(),
        );

        let baseline = process_spans(rows.clone(), &FeedOptions::new());
        let shadow = shadow_resolved_order(rows);

        assert_eq!(
            shadow.len(),
            baseline.messages.len(),
            "{label}: shadow order dropped or added blocks"
        );
        let mut a: Vec<String> = shadow.iter().map(|b| b.content_hash.clone()).collect();
        let mut b: Vec<String> = baseline
            .messages
            .iter()
            .map(|b| b.content_hash.clone())
            .collect();
        a.sort();
        b.sort();
        assert_eq!(
            a, b,
            "{label}: shadow order is not a permutation of the survivors"
        );
    }
}

/// The resolver returns every survivor exactly once, under the constraints production enforces.
///
/// Permutation was only ever checked for `NEUTRAL`, which is the configuration that provably cannot
/// move anything - so it could not have caught a resolver that drops or duplicates a block once a
/// class is promoted. Checked here against the real pipeline: the message *set* of a view must not
/// depend on the ordering constraints at all, because ordering is downstream of deduplication.
///
/// Companion to `ordering_constraints_do_not_change_a_session_s_messages`, which covers the session
/// path this one cannot.
///
/// Scope, deliberately stated: this covers the single-trace path only. Ordering and deduplication are
/// *not* independent across traces - `process_multi_trace_spans` strips a session's repeated prefix by
/// matching the previous trace's reconstructed sequence, so changing the order changes what is
/// stripped. That coupling is why promoting the dataflow class moved `adk/tool_use`'s session count
/// from 24 to 29, and it is an architectural problem rather than a test gap: a session's
/// deduplication should not be a function of a sibling trace's presentation order.
#[test]
fn promoted_constraints_do_not_change_which_messages_appear() {
    let mut checked = 0usize;
    for (label, paths) in discover_fixtures() {
        let all = rows_for(&paths);
        let mut by_trace: BTreeMap<String, Vec<MessageSpanRow>> = BTreeMap::new();
        for (_, row) in all {
            by_trace.entry(row.trace_id.clone()).or_default().push(row);
        }
        for (trace_id, trace_rows) in by_trace {
            let rows = sorted_by_timestamp(
                trace_rows
                    .into_iter()
                    .filter(passes_content_filter)
                    .collect(),
            );
            if rows.is_empty() {
                continue;
            }
            let (legacy, _) = legacy_and_neutral_order(rows.clone());
            let produced = process_spans(rows, &FeedOptions::new()).messages;
            let digest = |blocks: &[sideseat_domain::sideml::feed::BlockEntry]| -> Vec<String> {
                let mut out: Vec<String> = blocks
                    .iter()
                    .map(|b| format!("{}/{}/{}", b.role.as_str(), b.entry_type, b.content_hash))
                    .collect();
                out.sort();
                out
            };
            assert_eq!(
                digest(&produced),
                digest(&legacy),
                "{label} / trace {trace_id}: promoting an ordering constraint changed which \
                 messages appear, not just their order"
            );
            checked += 1;
        }
    }
    assert!(checked > 50, "only checked {checked} traces");
}

/// The resolver cannot move a block with every constraint class off.
///
/// With `Constraints::NEUTRAL` the resolver enforces only what the previous sort already satisfies -
/// every edge already forward, every contracted emission already contiguous, the legacy index as the
/// pop seed - so its output must be the previous order exactly, on every trace of every fixture. That
/// is the proof the machinery has no opinion of its own: whatever production's promoted classes then
/// change is attributable to those classes, not to the graph, the Kahn resolve or the cycle fallback.
///
/// Checked as a property here rather than left to the goldens, which are regenerable: a golden diff
/// would show a scaffold reorder as "expected output changed" and could be blessed by accident.
#[test]
fn the_neutral_resolver_reproduces_the_legacy_order() {
    let mut checked = 0usize;
    for (label, paths) in discover_fixtures() {
        let all = rows_for(&paths);
        let mut by_trace: BTreeMap<String, Vec<MessageSpanRow>> = BTreeMap::new();
        for (_, row) in all {
            by_trace.entry(row.trace_id.clone()).or_default().push(row);
        }
        for (trace_id, trace_rows) in by_trace {
            let rows = sorted_by_timestamp(
                trace_rows
                    .into_iter()
                    .filter(passes_content_filter)
                    .collect(),
            );
            if rows.is_empty() {
                continue;
            }
            let (legacy, neutral) = legacy_and_neutral_order(rows);
            // The whole block, serialised. A role/type/span/hash fingerprint is too weak: two
            // identical tool calls with distinct ids share it, so swapping them would have passed -
            // and those two calls are exactly what the resolver's contraction reasons about.
            let seq = |blocks: &[sideseat_domain::sideml::feed::BlockEntry]| -> Vec<String> {
                blocks
                    .iter()
                    .map(|b| serde_json::to_string(b).expect("a block serialises"))
                    .collect()
            };
            assert_eq!(
                seq(&neutral),
                seq(&legacy),
                "{label} / trace {trace_id}: the resolver moved a block with every class off, so the \
                 machinery is not neutral"
            );
            checked += 1;
        }
    }
    assert!(
        checked > 50,
        "expected the corpus to contribute many traces, only checked {checked}"
    );
}
