//! The footprint gates that run in process, plus the invariants that keep the four ceilings honest.
//!
//! Two of the plan's four ceilings need a running server and live in `scripts/footprint-gates.sh`; the two
//! here are the ones a test binary can measure directly, and they are the two stated on **live allocated
//! bytes** rather than on RSS. `runtime::allocation` carries the argument for that distinction; the short
//! version is that both glibc and jemalloc retain freed pages, so an RSS-based "returns to baseline" gate
//! fails correct code.
//!
//! ```bash
//! cargo test --locked --release -p sideseat-server --test footprint -- --ignored --nocapture
//! ```
//!
//! or `make footprint`, which also runs the two resident-memory gates against a real server.
//!
//! `#[ignore]` on the two measurements, like every other benchmark in this repository: the 10 000-turn
//! fixture takes tens of seconds to build and a debug build's numbers describe the debug build. The
//! invariants beside them are ordinary tests and run in `make check`.
//!
//! **The measurements serialise themselves**, and do not rely on `--test-threads=1`. Both read a process-global
//! allocation counter, so run concurrently the queue test's live bytes fall inside the session test's
//! baseline-to-residue window - and freed before the session's final snapshot, they mask a leak of their own
//! size: a 52 MB regression reported as 48 MB. Relying on a Makefile flag left that true for anyone running
//! `cargo test --test footprint -- --ignored` by hand, which is the ordinary way to run one of them. They take
//! `MEASUREMENT_LOCK` instead, so the correctness is in the file that needs it. The Makefile still passes the
//! flag, because a serialised gate that also does not interleave its *output* is easier to read.

use sideseat_core::constants::{
    FOOTPRINT_IDLE_RSS_MAX_BYTES, FOOTPRINT_INGEST_RSS_MAX_BYTES, FOOTPRINT_QUEUED_SPAN_MAX_RATIO,
    FOOTPRINT_SESSION_READ_GROWTH_MAX_BYTES, FOOTPRINT_SESSION_READ_TURNS,
};
use sideseat_server::runtime::allocation::{
    ALLOCATOR_NAME, AllocationSnapshot, RESIDENT_CEILINGS_APPLY, describe_footprint, resident_bytes,
};

const MIB: f64 = 1_048_576.0;

/// Held for the duration of either live-allocation measurement.
///
/// Not a nicety: the counter is process-global, so two measurements overlapping means each is reading the
/// other's allocations as its own. See the module docs for what that hides.
static MEASUREMENT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Take the measurement lock, tolerating a previous panic.
///
/// A panicking measurement poisons the mutex, and the next one would then fail on the lock rather than on its
/// own assertion - reporting the wrong test as broken. The lock protects a counter, not an invariant, so a
/// poisoned guard is still a usable guard.
fn measurement_guard() -> std::sync::MutexGuard<'static, ()> {
    MEASUREMENT_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A long session read gives its memory back.
///
/// The gate is on **live allocated bytes** after the answer *and its memo* are dropped. Including the memo
/// in the measurement would be measuring the cache doing its job — a process that has served a 10 000-turn
/// read and kept the reconstruction is *supposed* to be holding it — so what is asserted is the residue once
/// both are gone, which is the only part that would be a leak.
///
/// The settle loop is not slack in the ceiling. Dropping a `moka` cache releases its entries through its own
/// housekeeping, so "returns to baseline" is an eventual property and checking it once is checking a race.
/// The loop has a stated deadline and reports how long it actually took, so a regression that merely gets
/// slower is visible rather than absorbed.
#[test]
#[ignore]
fn a_long_session_read_returns_to_its_baseline() {
    let _serialised = measurement_guard();
    let turns = std::env::var("FOOTPRINT_TURNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(FOOTPRINT_SESSION_READ_TURNS);

    // Warm every lazy static the read path touches - the rules engine parses its 43 embedded assets on first
    // use, and a one-off parse charged to this measurement would read as a leak of exactly its size.
    {
        let cache = sideseat_domain::sideml::feed::cache::ReconstructionCache::new();
        let _ = sideseat_domain::sideml::feed::process_spans_cached(
            &cache,
            session_rows(2),
            &sideseat_domain::sideml::feed::FeedOptions::new(),
        );
    }

    let baseline = AllocationSnapshot::now();
    let (blocks, peak_growth, input_bytes) = {
        let rows = session_rows(turns);
        let input_bytes: usize = rows.iter().map(|r| r.messages_json.len()).sum();
        let cache = sideseat_domain::sideml::feed::cache::ReconstructionCache::new();
        let result = sideseat_domain::sideml::feed::process_spans_cached(
            &cache,
            rows,
            &sideseat_domain::sideml::feed::FeedOptions::new(),
        );
        let peak = AllocationSnapshot::now().growth_since(&baseline);
        (result.messages.len(), peak, input_bytes)
    };

    // Poll rather than sleep a fixed time, and report the wait: an eventual property checked once is a race,
    // and a fixed sleep hides how long it really needed.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let settle = std::time::Instant::now();
    let mut growth = AllocationSnapshot::now().growth_since(&baseline);
    while growth > FOOTPRINT_SESSION_READ_GROWTH_MAX_BYTES && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
        growth = AllocationSnapshot::now().growth_since(&baseline);
    }
    let settled_in = settle.elapsed();

    eprintln!(
        "FOOTPRINT session read: {turns} turns, {:.1} MB input -> {blocks} blocks; peak live growth \
         {:.1} MB, residue {:.1} MB after {:?} (ceiling {:.0} MB); {}",
        input_bytes as f64 / MIB,
        peak_growth as f64 / MIB,
        growth as f64 / MIB,
        settled_in,
        FOOTPRINT_SESSION_READ_GROWTH_MAX_BYTES as f64 / MIB,
        describe_footprint()
    );

    assert!(
        blocks > 0,
        "nothing was reconstructed, so nothing was measured"
    );
    assert!(
        peak_growth > 0,
        "a {turns}-turn read that allocates nothing means the counters are not wired to the allocator"
    );
    assert!(
        growth <= FOOTPRINT_SESSION_READ_GROWTH_MAX_BYTES,
        "a {turns}-turn session read left {:.1} MB live after its answer and memo were dropped, over the \
         {:.0} MB ceiling",
        growth as f64 / MIB,
        FOOTPRINT_SESSION_READ_GROWTH_MAX_BYTES as f64 / MIB
    );
}

/// A queued span costs less than three times its decoded protobuf.
///
/// Measured on live allocated bytes with the batches still queued, over the sum of the *decoded protobuf*
/// sizes of what was published. The denominator is stated that way in the plan and it matters: HTTP accepts
/// gzip while the queue carries an uncompressed encoding, so a ratio against wire bytes is unachievable for a
/// valid repetitive request and would make the gate meaningless rather than strict.
///
/// Published through `StreamTopic` rather than straight into the backend, so what is measured is the
/// production path's retention including its encode, not a hand-assembled approximation of it.
#[test]
#[ignore]
fn a_queued_span_costs_less_than_three_times_its_protobuf() {
    let _serialised = measurement_guard();
    use prost::Message;

    // A gate that measured nothing must not pass. The fixtures are committed, so an empty set means
    // `FOOTPRINT_FIXTURE` names a directory that does not exist or holds no `.pb` files - a misconfiguration,
    // not a legitimate skip, and returning `Ok` for it is the "passes while seeing nothing" shape these gates
    // exist to remove. `make test-clickhouse` may skip on a missing URL because the *service* is optional;
    // nothing about this measurement is.
    let requests = fixture_requests();
    assert!(
        !requests.is_empty(),
        "no captured requests for FOOTPRINT_FIXTURE; a gate with no input cannot pass. Fixtures live in \
         server/tests/fixtures/messages/<suite>/<sample> and are committed"
    );

    let spans: usize = requests
        .iter()
        .map(|r| {
            r.resource_spans
                .iter()
                .flat_map(|rs| rs.scope_spans.iter())
                .map(|ss| ss.spans.len())
                .sum::<usize>()
        })
        .sum();
    let decoded_bytes: usize = requests.iter().map(|r| r.encoded_len()).sum();
    assert!(
        spans > 0 && decoded_bytes > 0,
        "the fixture carries no spans"
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");

    // The queue is never drained, which is the point: the ratio is about what a *backlog* costs.
    let topics = sideseat_messaging::TopicService::new(sideseat_adapter_topics::memory_backend());
    let topic = topics.stream_topic::<opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest>(
        "footprint-traces",
        |_| String::new(),
    );

    // One publish first, outside the measurement: the backend creates the stream's deque and its notifier
    // lazily, and charging that one-off to the ratio would inflate it by a fixed amount that shrinks as the
    // backlog grows - a gate whose verdict depends on how much it published.
    runtime
        .block_on(topic.publish(&requests[0]))
        .expect("first publish");

    let before = AllocationSnapshot::now();
    let mut published_spans = 0usize;
    let mut published_bytes = 0usize;
    for request in &requests {
        runtime.block_on(topic.publish(request)).expect("publish");
        published_spans += request
            .resource_spans
            .iter()
            .flat_map(|rs| rs.scope_spans.iter())
            .map(|ss| ss.spans.len())
            .sum::<usize>();
        published_bytes += request.encoded_len();
    }
    let held = AllocationSnapshot::now().growth_since(&before);
    let ratio = held as f64 / published_bytes as f64;

    eprintln!(
        "FOOTPRINT queue: {published_spans} spans / {:.1} MB decoded protobuf held as {:.1} MB live = \
         {ratio:.2}x (ceiling {FOOTPRINT_QUEUED_SPAN_MAX_RATIO:.1}x), {:.0} bytes per span; {}",
        published_bytes as f64 / MIB,
        held as f64 / MIB,
        held as f64 / published_spans as f64,
        describe_footprint()
    );

    assert!(
        held > 0,
        "queuing {published_spans} spans held nothing, so the measurement is not measuring the queue"
    );
    assert!(
        ratio <= FOOTPRINT_QUEUED_SPAN_MAX_RATIO,
        "a queued span costs {ratio:.2}x its decoded protobuf, over the \
         {FOOTPRINT_QUEUED_SPAN_MAX_RATIO:.1}x ceiling"
    );
}

/// The local term relation's physical cost is measured against a corpus whose
/// expected matches deliberately cross the per-field cap.
#[test]
#[ignore]
fn search_term_write_amplification_preserves_the_recall_floor() {
    use sideseat_ports::traits::SpanStore;
    use sideseat_ports::types::{
        NormalizedSpan, SEARCH_RECALL_FLOOR, SEARCH_TERMS_PER_FIELD, SearchField,
    };

    let _serialised = measurement_guard();
    let timestamp = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let mut spans = (0..512)
        .map(|span| NormalizedSpan {
            project_id: Some("search-footprint".to_string()),
            trace_id: format!("trace{span:04}"),
            span_id: format!("span{span:04}"),
            span_name: format!("regular{span:04}"),
            timestamp_start: timestamp,
            input_preview: Some(
                (0..96)
                    .map(|term| format!("r{span:04}x{term:03}"))
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    spans.extend((0..20).map(|span| {
        NormalizedSpan {
            project_id: Some("search-footprint".to_string()),
            trace_id: format!("overflow-trace{span:02}"),
            span_id: format!("overflow-span{span:02}"),
            span_name: format!("overflow{span:02}"),
            timestamp_start: timestamp,
            input_preview: Some(
                (0..=SEARCH_TERMS_PER_FIELD)
                    .map(|term| format!("o{span:02}x{term:03}"))
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            ..Default::default()
        }
    }));

    let mut indexed = spans.clone();
    sideseat_domain::search::index_spans(&mut indexed);
    let found = indexed
        .iter()
        .enumerate()
        .filter(|(position, span)| {
            let expected = if *position < 512 {
                format!("r{position:04}x095")
            } else {
                format!("o{:02}x{:03}", position - 512, SEARCH_TERMS_PER_FIELD)
            };
            span.search
                .field(SearchField::Prompt)
                .is_some_and(|field| field.terms.contains(&expected))
        })
        .count();
    let recall = found as f64 / indexed.len() as f64;
    assert!(
        recall >= SEARCH_RECALL_FLOOR,
        "the {}-term cap retained {:.3} recall, below the {:.3} floor",
        SEARCH_TERMS_PER_FIELD,
        recall,
        SEARCH_RECALL_FLOOR
    );

    #[derive(Debug)]
    struct Measurement {
        bytes: u64,
        elapsed: std::time::Duration,
        term_rows: u64,
        logical_term_bytes: u64,
    }

    async fn write(spans: Vec<NormalizedSpan>) -> Measurement {
        let directory = tempfile::TempDir::new().unwrap();
        let storage =
            sideseat_core::storage::AppStorage::init_for_test(directory.path().to_path_buf());
        let service = std::sync::Arc::new(
            sideseat_adapter_duckdb::DuckdbService::init(
                &storage,
                std::sync::Arc::new(sideseat_server::runtime::clock::SystemClock),
            )
            .await
            .unwrap(),
        );
        let repository = sideseat_adapter_duckdb::DuckdbRepository(std::sync::Arc::clone(&service));
        let started = std::time::Instant::now();
        repository.insert_spans(spans).await.unwrap();
        let elapsed = started.elapsed();
        let (term_rows, logical_term_bytes): (i64, i64) = {
            let conn = service.conn();
            conn.query_row(
                "SELECT COUNT(*), COALESCE(SUM(\
                 LENGTH(project_id) + LENGTH(trace_id) + LENGTH(span_id) + \
                 LENGTH(field) + LENGTH(term) + 1), 0) FROM span_terms",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
        };
        service.checkpoint().await.unwrap();
        let bytes = std::fs::metadata(
            storage
                .subdir(sideseat_core::storage::DataSubdir::Duckdb)
                .join(sideseat_core::constants::DUCKDB_DB_FILENAME),
        )
        .unwrap()
        .len();
        Measurement {
            bytes,
            elapsed,
            term_rows: term_rows as u64,
            logical_term_bytes: logical_term_bytes as u64,
        }
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let baseline = runtime.block_on(write(spans));
    let with_index = runtime.block_on(write(indexed));
    let physical_delta = with_index.bytes.saturating_sub(baseline.bytes);
    assert!(
        physical_delta > 0 && with_index.term_rows > 0,
        "the measurement wrote no physical term index"
    );
    eprintln!(
        "SEARCH INDEX footprint: {} spans, {} term rows ({:.1}/span), {:.0} logical term \
         bytes/span, {:.0} physical bytes/span; ingest {:?} baseline -> {:?} indexed; recall \
         {:.3} (floor {:.3})",
        532,
        with_index.term_rows,
        with_index.term_rows as f64 / 532.0,
        with_index.logical_term_bytes as f64 / 532.0,
        physical_delta as f64 / 532.0,
        baseline.elapsed,
        with_index.elapsed,
        recall,
        SEARCH_RECALL_FLOOR,
    );
}

/// The shell script enforces the ceilings this crate declares.
///
/// The two resident ceilings are read by a bash script and declared in Rust, so without this they are two
/// numbers that agree today. Same reason `every_script_that_locates_the_repository_root_finds_it` exists: a
/// gate whose threshold has a second spelling is a gate that silently loosens.
#[test]
fn the_footprint_script_enforces_the_declared_ceilings() {
    let script = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/footprint-gates.sh"),
    )
    .expect("scripts/footprint-gates.sh is the other half of the footprint gate");

    for (name, declared) in [
        ("IDLE_RSS_CEILING_BYTES", FOOTPRINT_IDLE_RSS_MAX_BYTES),
        ("INGEST_RSS_CEILING_BYTES", FOOTPRINT_INGEST_RSS_MAX_BYTES),
    ] {
        // The assignment is matched with its `=` and end of line, so a substring of a longer name or a
        // mention in a comment cannot satisfy it.
        let wanted = format!("{name}={declared}");
        assert!(
            script.lines().any(
                |line| line.trim_start().starts_with(&wanted) && line.trim() == wanted.as_str()
            ),
            "scripts/footprint-gates.sh must set `{wanted}`, matching \
             sideseat_core::constants; found:\n{}",
            script
                .lines()
                .filter(|l| l.contains(name))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

/// The pinned allocator is the one actually in force for this binary.
///
/// A `#[global_allocator]` in a library reaches every binary that links it, and this test is what says so
/// rather than assuming it: if the registration were removed, or shadowed by another crate's, both
/// measurements above would silently become statements about the system allocator, and the two resident
/// ceilings would be uninterpretable while every gate still passed.
#[test]
fn the_gates_run_under_the_allocator_they_claim() {
    let before = AllocationSnapshot::now();
    let block: Vec<u8> = vec![7u8; 4 * 1024 * 1024];
    let growth = AllocationSnapshot::now().growth_since(&before);
    assert_eq!(block[0], 7);
    assert!(
        growth >= 4 * 1024 * 1024,
        "the counting allocator is not in force in this test binary: a 4 MiB allocation showed {growth} \
         bytes of live growth"
    );

    if RESIDENT_CEILINGS_APPLY {
        assert_eq!(ALLOCATOR_NAME, "jemalloc");
        assert!(
            resident_bytes().is_some(),
            "the resident ceilings are claimed to apply here, so RSS has to be readable"
        );
    } else {
        eprintln!(
            "FOOTPRINT: resident ceilings do not apply under allocator {ALLOCATOR_NAME}; the \
             live-allocation gates still do"
        );
    }
}

/// A synthetic *incremental* session: one turn per span, the shape `bench_session_scaling` measures.
///
/// Incremental rather than replaying, deliberately. A replaying framework's input is quadratic in the turn
/// count, so a 10 000-turn replaying fixture is 10^8 message entries and would measure the fixture generator
/// against the footprint ceiling. The residue this gate is about does not depend on which shape produced the
/// blocks.
fn session_rows(turns: usize) -> Vec<sideseat_ports::types::MessageSpanRow> {
    use chrono::TimeZone;
    let t0 = chrono::Utc
        .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
        .single()
        .expect("valid time");

    (0..turns)
        .map(|turn| {
            let t = t0 + chrono::Duration::seconds(turn as i64);
            let messages = format!(
                r#"[{{"source":{{"attribute":{{"key":"llm.input_messages","time":"{ts}"}}}},"content":{{"role":"user","content":"question {turn}"}}}},{{"source":{{"event":{{"name":"gen_ai.choice","time":"{ts}"}}}},"content":{{"role":"assistant","content":"answer {turn}"}}}}]"#,
                ts = t.to_rfc3339()
            );
            sideseat_ports::types::MessageSpanRow {
                trace_id: format!("trace-{turn}"),
                span_id: format!("span-{turn}-0"),
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
        })
        .collect()
}

/// The captured OTLP requests of one fixture, decoded.
///
/// Real payloads rather than a synthetic protobuf, because the ratio is sensitive to what a real export's
/// attribute and event bulk looks like. `FOOTPRINT_FIXTURE` selects one, matching `BENCH`'s default so the
/// two measurements describe the same payload.
fn fixture_requests()
-> Vec<opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest> {
    use prost::Message;

    let want = std::env::var("FOOTPRINT_FIXTURE").unwrap_or_else(|_| "langgraph/swarm".to_string());
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/messages")
        .join(&want);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut paths: Vec<std::path::PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("pb"))
        .collect();
    paths.sort();

    paths
        .iter()
        .map(|p| {
            let bytes = std::fs::read(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
            opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest::decode(
                bytes.as_slice(),
            )
            .unwrap_or_else(|e| panic!("decode {}: {e}", p.display()))
        })
        .collect()
}
