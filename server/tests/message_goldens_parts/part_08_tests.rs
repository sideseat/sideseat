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

// ============================================================================
// Log-carried conversations
// ============================================================================

/// The fixtures whose conversation arrives, wholly or partly, as log records - and the turns each must show.
///
/// The last turn is the answer, and in every one of them the answer exists **only** in a log record, so a
/// replay that ignored logs loses it. That is what makes the check below falsifiable rather than a restatement
/// of the golden.
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
];

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
        let views = golden
            .span_views
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
