/// Duplicate detection, the property most at risk from dedup changes.
///
/// Partitioned by trace: identity is (role, entry_type, content) **within one trace**. Two
/// identical prompts in two different traces are two legitimate messages, not a duplicate -
/// several samples run their whole conversation twice, once with a session id and once
/// without, so a session view contains each prompt twice by design.
///
/// A genuine repeat needs evidence that each copy is an occurrence of its own. Two copies from one place in one
/// payload - span, carrier and position - are never two, and otherwise the fixture's truth is the first evidence:
/// where its facts, allocated one to a block across the view, give every copy one of its own, and it holds as many
/// facts of this content as there are copies (`message_truth::Allowances`), each copy is one of them - four quoted
/// steps each behind the same preamble are four preambles, while a fifth copy needs the evidence below. `allowed`
/// is what the truth allows each row, `None` where no truth describes the fixture.
///
/// Where the truth cannot decide - no truth for the fixture, or fewer of its facts than copies - the payloads
/// must. Ordered payload positions prove repeated members within one span. Separate execution spans prove them
/// only when deduplication assigned a different occurrence rank to every copy. A re-delivery retains both the
/// span and rank, while peer spans merely re-listing one request retain the same rank. A copy the payloads prove
/// and the truth does not hold is the truth check's to report (`*.duplicated`, in its ledger): this check asks
/// whether the view shows a copy nothing accounts for.
///
/// That last rule is a stage, not the design. Seven such copies are ledgered today - `user_text.duplicated` fact-001
/// in traceloop 0.53.3 streaming and tool_use, `system.duplicated` fact-012 in claude-code multi_turn, and the
/// span-7 reminders of claude-agent-sdk(-js) multi_turn native and SDK - and a truth that decided alone would fail
/// them here a second time. Once they are fixed, where the truth holds what the copies show it decides: fewer facts
/// than copies is a duplicate however the payloads prove them.
fn assert_no_duplicates(
    label: &str,
    view_name: &str,
    rows: &[InvariantRow],
    allowed: Option<&[message_truth::Allowance]>,
) {
    if let Some(allowed) = allowed {
        assert_eq!(
            allowed.len(),
            rows.len(),
            "{label} / {view_name}: the truth's allowances describe another view"
        );
    }
    let mut seen: HashMap<DuplicateGroup<'_>, Vec<(usize, &InvariantRow)>> = HashMap::new();
    for (index, r) in rows.iter().enumerate() {
        seen.entry(duplicate_group(r)).or_default().push((index, r));
    }
    let mut dupes: Vec<String> = seen
        .iter()
        .filter(|(_, copies)| {
            if copies.len() < 2 {
                return false;
            }
            // One place in one payload shown twice is one observation twice, whatever the truth holds.
            let mut observed = HashSet::new();
            let repeats_an_observation = copies
                .iter()
                .filter(|(_, row)| !row.position.is_empty())
                .any(|(_, row)| !observed.insert((&row.span_id, &row.carrier, &row.position)));
            if repeats_an_observation {
                return true;
            }
            // Every copy must have a fact of its own, the allocation giving one to every block it competes with,
            // and the truth must hold as many facts of this content as there are copies.
            let truth_holds_all = allowed.is_some_and(|allowed| {
                copies.iter().all(|&(index, _)| {
                    allowed[index].accounted && copies.len() <= allowed[index].facts
                })
            });
            if truth_holds_all {
                return false;
            }
            let copies: Vec<&InvariantRow> = copies.iter().map(|&(_, row)| row).collect();
            let occurrences: HashSet<(&str, &str, &str)> = copies
                .iter()
                .filter(|row| row.carrier_proves_occurrence && !row.position.is_empty())
                .map(|row| {
                    (
                        row.span_id.as_str(),
                        row.carrier.as_str(),
                        row.position.as_str(),
                    )
                })
                .collect();
            let ordinals: HashSet<u32> = copies.iter().map(|row| row.occurrence_ordinal).collect();
            let spans: HashSet<&str> = copies.iter().map(|row| row.span_id.as_str()).collect();
            let carrier_proves_all = occurrences.len() == copies.len();
            let separate_executions_prove_all =
                ordinals.len() == copies.len() && spans.len() == copies.len();
            !carrier_proves_all && !separate_executions_prove_all
        })
        .map(|((trace, _scope, role, kind, content), copies)| {
            let head: String = content.chars().take(70).collect();
            let truth = allowed
                .and_then(|allowed| copies.iter().map(|&(index, _)| allowed[index].facts).max())
                .filter(|&facts| facts > 0)
                .map_or(String::new(), |facts| format!(" (the truth holds {facts})"));
            let at: Vec<String> = copies
                .iter()
                .map(|(_, row)| format!("{}:{}@{}", row.span_id, row.carrier, row.position))
                .collect();
            format!(
                "{}x in trace {} [{role}/{kind}] {head}{truth} at {}",
                copies.len(),
                &trace[..trace.len().min(8)],
                at.join(", ")
            )
        })
        .collect();
    dupes.sort();
    assert!(
        dupes.is_empty(),
        "{label} / {view_name}: duplicate messages within one trace:\n  {}",
        dupes.join("\n  ")
    );
}

/// A block's identity for the invariants. The view says only that reasoning was signed, yet two reasoning
/// blocks with one text and different signatures are two turns: the pipeline's identity keeps the signature
/// for that reason, and so must the duplicate check, or it would accuse two withheld turns of being one.
fn identity_digest(content: &sideseat_domain::sideml::ContentBlock, view_digest: &str) -> String {
    match content {
        sideseat_domain::sideml::ContentBlock::Thinking {
            signature: Some(signature),
            ..
        } => content_digest(&json!([view_digest, signature])),
        _ => view_digest.to_string(),
    }
}

/// The SHA-256 of a thinking block's signature, in the truth's hex: the reasoning's identity, which the view leaves
/// out (`message_truth::Allowances` reads it, so two copies of one signature are never two turns).
fn thinking_seal(content: &sideseat_domain::sideml::ContentBlock) -> Option<String> {
    use sha2::Digest as _;
    match content {
        sideseat_domain::sideml::ContentBlock::Thinking {
            signature: Some(signature),
            ..
        } => Some(
            sha2::Sha256::digest(signature.as_bytes())
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
        ),
        _ => None,
    }
}

/// The copies the duplicate invariant compares: one content within one trace - and, for an exception, within one
/// span, since it is composed from that span's own `exception_*` fields rather than read from a payload, so two
/// spans reporting the same failure are two failures, not a re-send (`openai-agents/image_gen`: three
/// `generate_image` executions each failed with the same message, and collapsing them showed three tool results
/// beside one explanation). The truth's allowance groups copies by this key too (`message_truth::Allowances`).
type DuplicateGroup<'a> = (&'a str, &'a str, &'a str, &'a str, &'a str);

fn duplicate_group(r: &InvariantRow) -> DuplicateGroup<'_> {
    let scope = if r.carrier == "attr:exception" {
        r.span_id.as_str()
    } else {
        ""
    };
    (
        r.trace_id.as_str(),
        scope,
        r.role.as_str(),
        r.entry_type.as_str(),
        r.content_digest.as_str(),
    )
}

/// A tool_use block's own call id: from the block field when set, otherwise from the id
/// embedded in the serialized content.
fn extract_tool_use_id(row: &InvariantRow) -> Option<&str> {
    row.tool_use_id.as_deref().or_else(|| {
        let start = row.content.find("\"id\":\"")? + 6;
        let rest = &row.content[start..];
        let end = rest.find('"')?;
        Some(&rest[..end])
    })
}

/// Tool calls and results must balance within a trace.
///
/// Counted per `(trace, tool_use_id)`: parallel calls may return out of order, and some producers
/// legitimately reuse an id for multiple sequential executions.
///
/// A result whose id matches no call is a defect, and results cannot outnumber calls for that id.
/// Occurrence order is checked separately by `assert_tool_causality`.
///
/// Unanswered calls are NOT asserted - a cancelled or failed turn legitimately leaves one open,
/// and a time-filtered view can cut between the two halves.
/// Fixtures whose SOURCE telemetry cannot satisfy tool pairing, with the reason.
///
/// A capability limit of the framework, not a parsing defect, so it is recorded per fixture
/// rather than weakening the check for everyone. Empty since the legacy Claude Agent SDK capture
/// it excused was retired: the current Claude Code CLI exports a subagent's calls beside their
/// results. Kept, like `NO_ANSWER_EXPECTED`, so the next genuine limit has a place to be declared.
const PAIRING_EXEMPT: &[(&str, &str)] = &[];

/// Fixtures whose source has no answer to show, with the reason.
///
/// Everything else is required to have one: a run that was asked something and completed must show
/// what it replied. CrewAI's answers were dropped for months because no invariant said so - its
/// reasoning fixture recorded `system -> user -> user -> user`, three questions and no answers, and
/// the goldens blessed it as correct.
/// Empty, and that is the point: the one entry it held is gone because the defect behind it was fixed.
///
/// `strands/legacy/error` was exempted on the grounds that "the sample exists to fail, so the run never
/// produced an answer". The run *did* produce an answer - a `ValidationException` - and three span views
/// displayed it while the trace view showed `system, user` and nothing else, because a parent error span
/// deferred to a child that had ERROR status and no exception fields to render. The exemption was
/// describing a defect rather than a property of the telemetry, which is exactly what an exemption must
/// never do: the rule is that a legitimate one proves the invariant's antecedent is false in the source.
///
/// Kept as an empty list rather than deleted, because the next fixture that genuinely cannot answer -
/// a cancelled run, a hard transport failure - needs somewhere to be declared with its reason.
const NO_ANSWER_EXPECTED: &[(&str, &str)] = &[];

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
        seal: None,
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
        seal: None,
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
            assert_no_duplicates("test", "synthetic", rows, None);
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

/// SDK fixtures whose framework leaves media out of one span's native telemetry entirely - no placeholder
/// marks where it was - and whose SideSeat integration exports it there, with the reason.
///
/// Parity then compares that span's SDK view without its media messages, for each span whose native view has
/// none and whose SDK view has some, and requires one such span, so the declaration cannot outlive the
/// behaviour it describes. The conversation views are compared whole: another carrier of the native capture
/// holds the media.
const MEDIA_DROPPED_NATIVELY: &[(&str, &str)] = &[(
    "adk/sdk/files",
    "ADK's `call_llm` copy of a model request leaves out every inline image and document part, which the \
     google-adk integration restores; its `generate_content` log events carry them natively",
)];

fn is_media(entry_type: &str) -> bool {
    matches!(
        entry_type,
        "image" | "document" | "audio" | "video" | "file"
    )
}

fn without_media(view: &mut GoldenView) {
    view.messages
        .retain(|message| !is_media(&message.entry_type));
    for (index, message) in view.messages.iter_mut().enumerate() {
        message.index = index;
    }
    view.message_count = view.messages.len();
    view.role_sequence = view
        .messages
        .iter()
        .map(|message| message.role.clone())
        .collect();
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
    assert_eq!(
        collapsed("duration_ms: 1200"),
        "duration_ms: 1200",
        "only pinned values collapse"
    );
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
        (
            TOOL_DEFINITIONS_DROPPED_NATIVELY,
            definitions,
            "tool definitions",
        ),
        (
            MODEL_CALLS_ARE_TRANSPORT_NATIVELY,
            generations,
            "generations",
        ),
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

/// `MESSAGE_FIXTURES=tracked` keeps only the samples git tracks, so a change can be verified against the
/// committed corpus while captures still being recorded sit untracked in the same tree.
/// `MESSAGE_FIXTURES=only:<producer>[,<producer>]` keeps only those producers' samples, so a recording
/// run for a new suite writes nothing outside its own directories while other work sits in the tree.
fn retain_requested_fixtures(root: &Path, fixtures: &mut BTreeMap<String, Vec<PathBuf>>) {
    if let Some(tracked) = requested_tracked_samples(root) {
        fixtures.retain(|label, _| tracked.contains(label));
    }
    if let Some(producers) = std::env::var("MESSAGE_FIXTURES")
        .ok()
        .and_then(|value| value.strip_prefix("only:").map(str::to_string))
    {
        let producers: Vec<&str> = producers.split(',').map(str::trim).collect();
        fixtures.retain(|label, _| {
            producers
                .iter()
                .any(|producer| label.split('/').next() == Some(producer))
        });
    }
}

/// Under `MESSAGE_FIXTURES=tracked`, the sample directories (relative to `root`) holding a file committed
/// at `HEAD`; `None` when every sample on disk is requested.
fn requested_tracked_samples(root: &Path) -> Option<std::collections::BTreeSet<String>> {
    if std::env::var("MESSAGE_FIXTURES").as_deref() != Ok("tracked") {
        return None;
    }
    let output = std::process::Command::new("git")
        // HEAD's tree, not the index: work staged but not committed is not the committed corpus.
        .args(["ls-tree", "-r", "-z", "--name-only", "HEAD", "--", "."])
        .current_dir(root)
        .output()
        .expect("MESSAGE_FIXTURES=tracked needs git");
    assert!(
        output.status.success(),
        "MESSAGE_FIXTURES=tracked needs a git checkout"
    );
    Some(
        output
            .stdout
            .split(|&b| b == 0)
            .filter_map(|path| std::str::from_utf8(path).ok())
            .filter_map(|path| path.rsplit_once('/').map(|(dir, _)| dir.to_string()))
            .collect(),
    )
}

/// Every copy must stand on a fact of its own: a copy the allocation left without one borrows no other copy's.
#[test]
fn a_copy_borrows_no_other_copy_s_allowance() {
    let row = |index: usize| {
        let mut row = occurrence_row(index, &index.to_string(), false);
        row.occurrence_ordinal = 0;
        row
    };
    let rows = vec![row(0), row(1)];
    let fires = |allowed: &[message_truth::Allowance]| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert_no_duplicates("test", "synthetic", &rows, Some(allowed));
        }))
        .is_err()
    };
    let own = message_truth::Allowance {
        facts: 2,
        accounted: true,
    };
    assert!(!fires(&[own, own]), "both copies stand on the two facts");
    let left_out = message_truth::Allowance {
        facts: 2,
        accounted: false,
    };
    assert!(
        fires(&[own, left_out]),
        "the allocation left the second copy without one"
    );
    let one = message_truth::Allowance {
        facts: 1,
        accounted: true,
    };
    assert!(
        fires(&[one, one]),
        "two copies of a content the truth holds once"
    );
}

/// The seal is the signature's SHA-256, extracted from the block itself: the truth-relative duplicate rule rests
/// on it, so a seal that silently became `None` would let two copies of one signature spend two sealed facts.
#[test]
fn a_thinking_block_s_seal_is_its_signature_s_digest() {
    use sideseat_domain::sideml::ContentBlock;
    assert_eq!(
        thinking_seal(&ContentBlock::Thinking {
            text: String::new(),
            signature: Some("sig".to_string()),
        })
        .as_deref(),
        // sha256("sig")
        Some("a543997d84f12798350c09bdef2cdb171bf41ed3e4a5f808af2feb0c56263009"),
    );
    assert_eq!(
        thinking_seal(&ContentBlock::Thinking {
            text: "visible".to_string(),
            signature: None,
        }),
        None
    );
    assert_eq!(thinking_seal(&ContentBlock::text("hi")), None);
}
