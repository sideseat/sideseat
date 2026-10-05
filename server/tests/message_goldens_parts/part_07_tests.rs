fn occurrence_row(index: usize, position: &str, proves_occurrence: bool) -> InvariantRow {
    InvariantRow {
        trace_id: "trace-a".to_string(),
        span_id: "span-1".to_string(),
        span_path: vec!["span-1".to_string()],
        index,
        role: "user".to_string(),
        entry_type: "text".to_string(),
        content: "same turn".to_string(),
        full_content: String::new(),
        content_digest: "same-digest".to_string(),
        occurrence_ordinal: index as u32,
        tool_use_id: None,
        carrier: "attr:ordered-conversation".to_string(),
        carrier_orders_positions: true,
        carrier_proves_occurrence: proves_occurrence,
        order_time: chrono::DateTime::UNIX_EPOCH,
        is_output: false,
        finish: None,
        position: position.to_string(),
    }
}

fn reused_id_row(index: usize, kind: &str) -> InvariantRow {
    InvariantRow {
        trace_id: "trace-a".to_string(),
        span_id: "span-1".to_string(),
        span_path: vec!["span-1".to_string()],
        index,
        role: if kind == "tool_use" {
            "assistant"
        } else {
            "tool"
        }
        .to_string(),
        entry_type: kind.to_string(),
        content: "{\"id\":\"reused\"}".to_string(),
        full_content: String::new(),
        content_digest: format!("{kind}-{index}"),
        occurrence_ordinal: (index / 2) as u32,
        tool_use_id: Some("reused".to_string()),
        carrier: "attr:test".to_string(),
        carrier_orders_positions: true,
        carrier_proves_occurrence: true,
        order_time: chrono::DateTime::UNIX_EPOCH,
        is_output: false,
        finish: None,
        position: index.to_string(),
    }
}

#[test]
fn duplicate_invariant_accepts_only_proven_distinct_occurrences() {
    let fires = |rows: &[InvariantRow]| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert_no_duplicates("test", "synthetic", rows);
        }))
        .is_err()
    };

    let distinct = vec![
        occurrence_row(0, "2.0", true),
        occurrence_row(1, "3.0", true),
    ];
    assert!(
        !fires(&distinct),
        "an ordered carrier's distinct positions are distinct occurrences"
    );

    let repeated_delivery = vec![
        occurrence_row(0, "2.0", true),
        occurrence_row(1, "2.0", true),
    ];
    assert!(
        fires(&repeated_delivery),
        "the same proven position delivered twice is still a duplicate"
    );

    let unproven = vec![
        occurrence_row(0, "2.0", true),
        occurrence_row(1, "3.0", false),
    ];
    assert!(
        fires(&unproven),
        "every retained repeat must carry occurrence evidence"
    );

    let mut separate_executions = vec![
        occurrence_row(0, "0.0", false),
        occurrence_row(1, "0.0", false),
    ];
    separate_executions[1].span_id = "span-2".to_string();
    separate_executions[1].span_path = vec!["span-2".to_string()];
    assert!(
        !fires(&separate_executions),
        "distinct execution spans with distinct ranks are distinct occurrences"
    );

    separate_executions[1].occurrence_ordinal = 0;
    assert!(
        fires(&separate_executions),
        "different spans without different occurrence ranks may only be re-listing one request"
    );
}

#[test]
fn reused_tool_ids_are_paired_by_occurrence() {
    let ordered = vec![
        reused_id_row(0, "tool_use"),
        reused_id_row(1, "tool_result"),
        reused_id_row(2, "tool_use"),
        reused_id_row(3, "tool_result"),
    ];
    assert_tool_pairing("test", "synthetic", &ordered);
    assert_tool_causality("test", "synthetic", &ordered);

    let misordered = vec![
        reused_id_row(0, "tool_use"),
        reused_id_row(1, "tool_result"),
        reused_id_row(2, "tool_result"),
        reused_id_row(3, "tool_use"),
    ];
    assert_tool_pairing("test", "synthetic", &misordered);
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert_tool_causality("test", "synthetic", &misordered);
        }))
        .is_err(),
        "a second result cannot consume the first occurrence's already-used call"
    );
}

/// SDK fixtures whose framework leaves media out of its native telemetry entirely - no placeholder
/// marks where it was - and whose SideSeat integration exports it, with the reason.
///
/// Parity then compares the SDK side without its media messages, and requires that the native side
/// has none and the SDK side has some, so the declaration cannot outlive the behaviour it describes.
const MEDIA_DROPPED_NATIVELY: &[(&str, &str)] = &[(
    "adk/sdk/files",
    "ADK's trace copy of a model request leaves out every inline image and document part; the \
     google-adk integration restores them",
)];

fn is_media(entry_type: &str) -> bool {
    matches!(entry_type, "image" | "document" | "audio" | "video" | "file")
}

fn without_media(mut golden: Golden) -> Golden {
    let strip = |view: &mut GoldenView| {
        view.messages.retain(|message| !is_media(&message.entry_type));
        for (index, message) in view.messages.iter_mut().enumerate() {
            message.index = index;
        }
        view.message_count = view.messages.len();
        view.role_sequence = view.messages.iter().map(|message| message.role.clone()).collect();
    };
    golden
        .span_views
        .values_mut()
        .chain(golden.trace_views.values_mut())
        .chain(golden.session_views.values_mut())
        .chain(std::iter::once(&mut golden.feed_view))
        .for_each(strip);
    golden
}

/// The form `capture.py`'s `anonymise` pins a Claude Code subagent's measured duration to.
const PINNED_DURATION: &str = "duration_ms: 0";

/// Rows with every pinned duration collapsed to one zero, for comparing two runs.
///
/// The Claude Code CLI tells the model how long each subagent took. Capture zeroes the digits but
/// keeps their count, because a protobuf payload's strings are length-prefixed, so two runs of one
/// conversation still differ wherever one subagent took 99 ms and the other 100 ms. Comparing the
/// pair with the count collapsed leaves every other character of those messages compared.
fn without_run_measurements(rows: Vec<(String, MessageSpanRow)>) -> Vec<(String, MessageSpanRow)> {
    rows.into_iter()
        .map(|(source, mut row)| {
            if row.messages_json.contains(PINNED_DURATION) {
                row.messages_json = collapse_pinned_durations(&row.messages_json);
            }
            (source, row)
        })
        .collect()
}

fn collapse_pinned_durations(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(PINNED_DURATION) {
        let end = at + PINNED_DURATION.len();
        out.push_str(&rest[..end]);
        rest = rest[end..].trim_start_matches('0');
    }
    out.push_str(rest);
    out
}

#[test]
fn a_pinned_duration_compares_by_presence_alone() {
    let collapsed = collapse_pinned_durations;

    assert_eq!(
        collapsed("tool_uses: 2 duration_ms: 00</usage> duration_ms: 0000 x"),
        collapsed("tool_uses: 2 duration_ms: 000</usage> duration_ms: 0 x"),
    );
    assert_eq!(collapsed("duration_ms: 1200"), "duration_ms: 1200", "only pinned values collapse");
    assert_ne!(
        collapsed("tool_uses: 2 duration_ms: 00"),
        collapsed("tool_uses: 3 duration_ms: 00"),
        "only the duration may differ"
    );
}

/// Producers whose native instrumentation records no tool definitions while their SideSeat integration
/// records them, with the reason.
///
/// Parity then compares the SDK side without its tool definitions. It requires that the native side has none
/// and that some SDK fixture of the producer has some, so the declaration cannot outlive the behaviour it
/// describes.
const TOOL_DEFINITIONS_DROPPED_NATIVELY: &[(&str, &str)] = &[(
    "bedrock",
    "OpenTelemetry's botocore instrumentation records the messages of a Converse call and not its tool \
     configuration; the bedrock integration records the tools offered",
)];

fn tool_definitions(golden: &Golden) -> usize {
    golden
        .span_views
        .values()
        .chain(golden.trace_views.values())
        .map(|view| view.tool_definition_count)
        .sum()
}

/// Aligns a pair whose native instrumentation is declared to drop tool definitions the SDK records.
///
/// Returns whether the SDK side had any, for the producer-wide check that the declaration still holds.
fn with_native_tool_definitions_aligned(
    producer: &str,
    native: Golden,
    mut sdk: Golden,
) -> (Golden, Golden, bool) {
    if !TOOL_DEFINITIONS_DROPPED_NATIVELY
        .iter()
        .any(|(declared, _)| *declared == producer)
    {
        return (native, sdk, false);
    }
    assert_eq!(
        tool_definitions(&native),
        0,
        "{producer}: declared as recording no tool definitions natively, which no longer holds - remove \
         the declaration"
    );
    let had = tool_definitions(&sdk) > 0;
    sdk.span_views
        .values_mut()
        .chain(sdk.trace_views.values_mut())
        .chain(sdk.session_views.values_mut())
        .chain(std::iter::once(&mut sdk.feed_view))
        .for_each(|view| view.tool_definition_count = 0);
    (native, sdk, had)
}

/// Producers whose native model calls are transport spans - classified as plain spans, so a framework's own
/// generation span is not counted twice - while their SideSeat integration records the call as a
/// generation, with the reason.
///
/// Parity then compares the SDK side with its generations read as plain spans. It requires that the native
/// side has no generation and that some SDK fixture of the producer has one, so the declaration cannot
/// outlive the behaviour it describes.
const MODEL_CALLS_ARE_TRANSPORT_NATIVELY: &[(&str, &str)] = &[(
    "bedrock",
    "OpenTelemetry's botocore instrumentation names the chat operation on the RPC span itself, and an RPC \
     span is never a generation; the bedrock integration emits a generation span",
)];

fn generations(golden: &Golden) -> usize {
    golden
        .span_views
        .values()
        .chain(golden.trace_views.values())
        .flat_map(|view| &view.messages)
        .filter(|message| message.observation_type.as_deref() == Some("generation"))
        .count()
}

/// Aligns a pair whose native model calls are declared transport spans.
///
/// Returns whether the SDK side had a generation, for the producer-wide check that the declaration holds.
fn with_native_model_calls_as_transport(
    producer: &str,
    native: Golden,
    mut sdk: Golden,
) -> (Golden, Golden, bool) {
    if !MODEL_CALLS_ARE_TRANSPORT_NATIVELY
        .iter()
        .any(|(declared, _)| *declared == producer)
    {
        return (native, sdk, false);
    }
    assert_eq!(
        generations(&native),
        0,
        "{producer}: declared as having no generation spans natively, which no longer holds - remove the \
         declaration"
    );
    let had = generations(&sdk) > 0;
    sdk.span_views
        .values_mut()
        .chain(sdk.trace_views.values_mut())
        .chain(sdk.session_views.values_mut())
        .chain(std::iter::once(&mut sdk.feed_view))
        .flat_map(|view| view.messages.iter_mut())
        .filter(|message| message.observation_type.as_deref() == Some("generation"))
        .for_each(|message| message.observation_type = Some("span".to_string()));
    (native, sdk, had)
}

/// Each declared native gap was seen: some SDK fixture of the producer had what its native side lacks.
fn native_gap_declarations_hold(
    definitions: &std::collections::BTreeSet<String>,
    generations: &std::collections::BTreeSet<String>,
) {
    for (declarations, seen, what) in [
        (TOOL_DEFINITIONS_DROPPED_NATIVELY, definitions, "tool definitions"),
        (MODEL_CALLS_ARE_TRANSPORT_NATIVELY, generations, "generations"),
    ] {
        for (producer, _) in declarations {
            assert!(
                seen.contains(*producer),
                "{producer}: declared as having {what} only the SDK records, and no SDK fixture has any - \
                 remove the declaration"
            );
        }
    }
}
