use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;

// ============================================================================
// Replay: OTLP log exports -> log_messages_json
// ============================================================================

/// Whether a file in a sample directory is a captured **log** export, attached to the spans it names.
///
/// Its own prefix rather than another `req-`: a log export is not a trace request, and counting it as one
/// would change what the support matrix's request count means.
fn is_captured_logs(name: &str) -> bool {
    name.starts_with("logs-") && (name.ends_with(".pb") || name.ends_with(".json"))
}

fn decode_logs(path: &Path) -> ExportLogsServiceRequest {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    match path.extension().and_then(|e| e.to_str()) {
        Some("json") => serde_json::from_slice(&bytes)
            .unwrap_or_else(|e| panic!("decode JSON {}: {e}", path.display())),
        _ => ExportLogsServiceRequest::decode(bytes.as_slice())
            .unwrap_or_else(|e| panic!("decode protobuf {}: {e}", path.display())),
    }
}

/// Every log export captured beside these requests, in capture order.
fn log_exports_beside(paths: &[PathBuf]) -> Vec<PathBuf> {
    let directories: BTreeSet<&Path> = paths.iter().filter_map(|path| path.parent()).collect();
    let mut exports: Vec<PathBuf> = directories
        .into_iter()
        .filter_map(|directory| std::fs::read_dir(directory).ok())
        .flat_map(|entries| entries.flatten().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(is_captured_logs)
        })
        .collect();
    exports.sort();
    exports
}

// ============================================================================
// Replay: OTLP bytes -> MessageSpanRow
// ============================================================================

fn decode_request(path: &Path) -> ExportTraceServiceRequest {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    match path.extension().and_then(|e| e.to_str()) {
        Some("json") => serde_json::from_slice(&bytes)
            .unwrap_or_else(|e| panic!("decode JSON {}: {e}", path.display())),
        _ => ExportTraceServiceRequest::decode(bytes.as_slice())
            .unwrap_or_else(|e| panic!("decode protobuf {}: {e}", path.display())),
    }
}

/// Run the real ingestion path over every captured request, in capture order.
/// Returns `(span_name, row)`: `MessageSpanRow` carries no span name, but the golden keys
/// span views by name so a diff points at a recognisable span rather than a raw id.
/// Whether a file in a sample directory is a captured request the golden runner will replay.
///
/// One predicate, shared with `the_corpus_matches_the_support_matrix`. They had two: discovery required a
/// `req-` prefix while the matrix counted every `.pb`/`.json` that was not `expected.json`. So renaming
/// `req-001.pb` to `capture.pb` left the documented count unchanged while silently removing that request
/// from every golden check - the corpus would still claim to cover it.
fn is_captured_request(name: &str) -> bool {
    name.starts_with("req-") && (name.ends_with(".pb") || name.ends_with(".json"))
}

fn rows_for(paths: &[PathBuf]) -> Vec<(String, MessageSpanRow)> {
    let pricing = PricingService::init_for_test().expect("offline pricing service");
    let mut rows = Vec::new();
    for path in paths {
        let request = decode_request(path);
        rows.extend(normalize_for_test(&request, &pricing));
    }
    attach_log_messages(paths, &mut rows);
    rows
}

/// Join log-carried messages to their spans exactly as the message queries do.
///
/// The real log extraction reads each record; the stored winner is one per `(log_digest, ordinal)`, so a
/// re-sent record counts once; non-empty arrays are grouped by `(trace, span)` and concatenated in
/// `(timestamp, log_digest, ordinal)` order - the order and the byte shape both SQL dialects produce. A
/// record naming a span the fixture does not hold attaches to nothing, as a `LEFT JOIN` from spans does.
fn attach_log_messages(paths: &[PathBuf], rows: &mut [(String, MessageSpanRow)]) {
    /// One stored record's contribution: its sort key, then the inside of its messages array.
    type LogEntry = (chrono::DateTime<chrono::Utc>, String, u32, String);

    let received = chrono::DateTime::UNIX_EPOCH;
    let mut records: BTreeMap<(String, u32), sideseat_ports::types::NormalizedLog> =
        BTreeMap::new();
    for export in log_exports_beside(paths) {
        for log in sideseat_ingestion::logs::extract_logs_batch(&decode_logs(&export), received) {
            records.insert((log.log_digest.clone(), log.ordinal), log);
        }
    }
    let mut by_span: BTreeMap<(String, String), Vec<LogEntry>> = BTreeMap::new();
    for log in records.into_values() {
        let (Some(trace), Some(span), Some(messages)) = (log.trace_id, log.span_id, log.messages)
        else {
            continue;
        };
        if messages == "[]" {
            continue;
        }
        by_span.entry((trace, span)).or_default().push((
            log.timestamp,
            log.log_digest,
            log.ordinal,
            messages[1..messages.len() - 1].to_string(),
        ));
    }
    for (_, row) in rows.iter_mut() {
        if let Some(entries) = by_span.get_mut(&(row.trace_id.clone(), row.span_id.clone())) {
            entries.sort();
            let inner: Vec<&str> = entries.iter().map(|entry| entry.3.as_str()).collect();
            row.log_messages_json = format!("[{}]", inner.join(","));
        }
    }
}

/// The frame records each `(trace, frame key)` holds, as `get_request_frames` answers them: each stored record's
/// winner once, in `(timestamp, log_digest, ordinal)` order, read one past the record bound.
fn frame_records_beside(
    paths: &[PathBuf],
) -> BTreeMap<(String, u128), Vec<sideseat_ports::types::RequestFrameRecord>> {
    let mut records: BTreeMap<(String, u32), sideseat_ports::types::NormalizedLog> =
        BTreeMap::new();
    for export in log_exports_beside(paths) {
        let logs = sideseat_ingestion::logs::extract_logs_batch(
            &decode_logs(&export),
            chrono::DateTime::UNIX_EPOCH,
        );
        for log in logs {
            records.insert((log.log_digest.clone(), log.ordinal), log);
        }
    }
    let mut by_key: BTreeMap<(String, u128), Vec<sideseat_ports::types::RequestFrameRecord>> =
        BTreeMap::new();
    for log in records.into_values() {
        let (Some(trace_id), Some(span_id), Some(key), Some(messages)) =
            (log.trace_id, log.span_id, log.frame_key, log.messages)
        else {
            continue;
        };
        if messages == "[]" {
            continue;
        }
        by_key.entry((trace_id.clone(), key)).or_default().push(
            sideseat_ports::types::RequestFrameRecord {
                trace_id,
                span_id,
                timestamp: log.timestamp,
                log_digest: log.log_digest,
                ordinal: log.ordinal,
                messages_json: Some(messages),
            },
        );
    }
    for frames in by_key.values_mut() {
        frames.sort_by(|a, b| {
            (a.timestamp, &a.log_digest, a.ordinal).cmp(&(b.timestamp, &b.log_digest, b.ordinal))
        });
        frames.truncate(sideseat_core::constants::REQUEST_FRAMES_MAX_RECORDS + 1);
    }
    by_key
}

/// The occurrences a request's frame records hold, each as many times as they hold it: `(trace, span, carrier,
/// position, content)`, where the position names the record, its message's index in it, and the block's place in
/// that message - `frame:<digest>:<ordinal>.<index>...` - so a block shown from the right span and carrier but from
/// another record, message or place, or more often than the records hold it, is not an occurrence. Each record's
/// messages are read on their own.
pub(crate) type FrameOccurrences = BTreeMap<(String, String, String, String, String), usize>;

fn frame_occurrences(frames: &[sideseat_ports::types::RequestFrameRecord]) -> FrameOccurrences {
    let mut out = FrameOccurrences::new();
    for record in frames {
        let raw: Vec<sideseat_domain::observations::RawMessage> = record
            .messages_json
            .as_deref()
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or_default();
        for (index, message) in raw.iter().enumerate() {
            for message in sideseat_domain::sideml::to_sideml(std::slice::from_ref(message)) {
                // The message's path within its observation, after the observation's own place.
                let path = message.position.to_string();
                let within = path
                    .split_once('.')
                    .map_or(String::new(), |(_, rest)| format!(".{rest}"));
                let carrier = match &message.source {
                    sideseat_domain::observations::MessageSource::Event { name, .. } => {
                        format!("event:{name}")
                    }
                    sideseat_domain::observations::MessageSource::Attribute { key, .. } => {
                        format!("attr:{key}")
                    }
                };
                for (entry, block) in message.sideml.content.iter().enumerate() {
                    *out.entry((
                        record.trace_id.clone(),
                        record.span_id.clone(),
                        carrier.clone(),
                        format!(
                            "frame:{}:{}.{index}{within}.{entry}",
                            record.log_digest, record.ordinal
                        ),
                        serde_json::to_string(block).expect("a content block serialises"),
                    ))
                    .or_default() += 1;
                }
            }
        }
    }
    out
}

/// A request's view opened with its frame records, as the span route opens it; unchanged where none joined.
fn framed(
    view: sideseat_domain::sideml::FeedResult,
    frames: &[sideseat_ports::types::RequestFrameRecord],
) -> sideseat_domain::sideml::FeedResult {
    match frames.is_empty() {
        true => view,
        false => sideseat_domain::sideml::process_framed_request(
            view,
            frames.to_vec(),
            &FeedOptions::new(),
        ),
    }
}

// ============================================================================
// Log-carried conversations
// ============================================================================

/// The fixtures whose conversation arrives, wholly or partly, as log records - and the turns each must show.
///
/// The last turn is the answer, and in every one of them the answer exists **only** in a log record, so a
/// replay that ignored logs loses it. That is what makes the check below falsifiable rather than a restatement
/// of the golden. A fixture of one span owes the turns in its span view too; one whose conversation spans
/// several (`MULTI_SPAN_LOG_FIXTURES`) owes them in the views that show the whole conversation.
const LOG_FIXTURES: &[(&str, &[&str])] = &[
    (
        "_synthetic/log_events_before_span",
        &[
            "Answer in one word.",
            "What is the capital of France?",
            "Paris.",
        ],
    ),
    (
        "_synthetic/log_and_span_event_overlap",
        &["Name a primary colour.", "Red."],
    ),
    (
        "_synthetic/log_inference_details",
        &["Translate 'hello' to French.", "Bonjour."],
    ),
    (
        "_synthetic/quoted_steps_on_two_carriers",
        &[
            "Research the weather in Barcelona for the next two days, then write a one-paragraph packing list based on it.",
            "Pack light layers for the sun and an umbrella for the rain.",
        ],
    ),
];

/// Log fixtures whose conversation spans several spans, each span view showing its own part of it.
const MULTI_SPAN_LOG_FIXTURES: &[&str] = &["_synthetic/quoted_steps_on_two_carriers"];

fn contents(view: &GoldenView) -> Vec<String> {
    view.messages.iter().map(|m| m.content.clone()).collect()
}

/// Every view shows the log-carried turns exactly once, ending in the answer - and without the logs the
/// answer is gone, so the fixture really is exercising the join.
#[test]
fn log_carried_messages_reach_every_view_exactly_once() {
    let fixtures: BTreeMap<String, Vec<PathBuf>> = discover_fixtures().into_iter().collect();
    for (label, turns) in LOG_FIXTURES {
        let paths = fixtures
            .get(*label)
            .unwrap_or_else(|| panic!("missing log fixture {label}"));
        assert!(
            !log_exports_beside(paths).is_empty(),
            "{label} has no logs-* export beside its requests"
        );
        let rows = rows_for(paths);
        let golden = build_golden(label, paths, &rows).golden;
        let answer = turns.last().expect("a conversation ends in an answer");
        let span_views = if MULTI_SPAN_LOG_FIXTURES.contains(label) {
            BTreeMap::new()
        } else {
            golden.span_views.clone()
        };
        let views = span_views
            .iter()
            .chain(&golden.trace_views)
            .chain(&golden.session_views)
            .map(|(name, view)| (name.clone(), view))
            .chain(std::iter::once(("feed".to_string(), &golden.feed_view)));
        for (name, view) in views {
            let shown = contents(view);
            for turn in *turns {
                assert_eq!(
                    shown.iter().filter(|content| content == turn).count(),
                    1,
                    "{label} {name}: `{turn}` must appear exactly once, got {shown:?}"
                );
            }
            // The feed descends across responses, so only the views in conversation order end in the answer.
            if name == "feed" {
                continue;
            }
            assert_eq!(
                view.messages.last().map(|m| m.role.as_str()),
                Some("assistant"),
                "{label} {name}: the conversation must end in the model's answer, got {:?}",
                view.role_sequence
            );
        }

        // The same replay with the log records withheld. If the answer survived this, the fixture would not be
        // testing the join at all.
        let without: Vec<(String, MessageSpanRow)> = rows
            .into_iter()
            .map(|(name, mut row)| {
                row.log_messages_json = "[]".to_string();
                (name, row)
            })
            .collect();
        let blind = build_golden(label, paths, &without).golden;
        assert!(
            blind
                .trace_views
                .values()
                .all(|view| !contents(view).iter().any(|content| content == answer)),
            "{label}: the answer `{answer}` is visible without the log records, so this fixture does not \
             depend on them"
        );
    }

    // Every synthetic log fixture is listed, so one added later cannot go unchecked.
    for (label, paths) in &fixtures {
        if label.starts_with("_synthetic/") && !log_exports_beside(paths).is_empty() {
            assert!(
                LOG_FIXTURES.iter().any(|(listed, _)| listed == label),
                "{label} carries log records but is not in LOG_FIXTURES"
            );
        }
    }
}

/// A re-sent log export is one set of records, exactly as the stored winner is one row per identity.
#[test]
fn a_resent_log_export_attaches_once() {
    let fixtures: BTreeMap<String, Vec<PathBuf>> = discover_fixtures().into_iter().collect();
    let paths = &fixtures["_synthetic/log_events_before_span"];
    let once = rows_for(paths);
    let directory = tempfile::TempDir::new().expect("temp dir");
    let mut copied = Vec::new();
    for path in paths {
        let target = directory.path().join(path.file_name().unwrap());
        std::fs::copy(path, &target).expect("copy request");
        copied.push(target);
    }
    for export in log_exports_beside(paths) {
        std::fs::copy(&export, directory.path().join("logs-001.json")).expect("copy logs");
        std::fs::copy(&export, directory.path().join("logs-002.json")).expect("re-send logs");
    }
    let twice = rows_for(&copied);
    assert_eq!(
        once.iter()
            .map(|(_, row)| row.log_messages_json.clone())
            .collect::<Vec<_>>(),
        twice
            .iter()
            .map(|(_, row)| row.log_messages_json.clone())
            .collect::<Vec<_>>()
    );
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
