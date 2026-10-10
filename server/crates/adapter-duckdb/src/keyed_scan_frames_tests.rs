//! The frames read (`MessageStore::get_request_frames`) reads the frame records of one trace stating one key, and
//! no more of the store: through the key's index wherever the frames lie, and within the bound however many there
//! are. The parent's store and profiler, and the same measures.

use super::*;

/// **A framed request's frame read is bounded in its statement**: the frame records of one trace stating one key,
/// exactly, each identity's winner once, in record order, and one past the record bound - and it reads those
/// records only, through the key's index, never the store's three full row groups. A record that frames nothing
/// stores NULL, never an empty key, which is what keeps it out of the index.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_frame_read_reads_only_the_frames_of_its_trace_and_key() {
    use sideseat_ports::traits::MessageStore;
    use sideseat_ports::types::{NormalizedLog, RequestFramesParams};
    let s = store().await;
    let start = Utc.timestamp_opt(1_700_000_000, 0).single().expect("start");
    let frame =
        |digest: &str, trace: &str, key: Option<u128>, second: i64, text: &str| NormalizedLog {
            project_id: Some(PROJECT.into()),
            log_digest: digest.to_string(),
            ordinal: 0,
            timestamp: start + TimeDelta::seconds(second),
            time: Some(start + TimeDelta::seconds(second)),
            trace_id: Some(trace.to_string()),
            span_id: Some(format!("span-of-{digest}")),
            ingested_at: Some(start),
            messages: Some(format!(r#"[{{"text":"{text}"}}]"#)),
            frame_key: key,
            ..Default::default()
        };
    let bound = sideseat_core::constants::REQUEST_FRAMES_MAX_RECORDS;
    let mut logs = vec![
        frame("f-b", "frame-trace", Some(1), 20, "second"),
        frame("f-a", "frame-trace", Some(1), 10, "first"),
        frame("f-other-key", "frame-trace", Some(2), 5, "another key"),
        frame(
            "f-other-trace",
            "another-trace",
            Some(1),
            5,
            "another trace",
        ),
        frame("f-none", "frame-trace", None, 5, "frames nothing"),
        // Another project's record under the same trace id and key: a client's trace ids are not unique across
        // projects, so the project is read after the key.
        NormalizedLog {
            project_id: Some("q".into()),
            ..frame("f-q", "frame-trace", Some(1), 15, "another project")
        },
    ];
    // More records under one key than a view joins, after the two above.
    logs.extend((0..bound + 3).map(|n| {
        frame(
            &format!("f-many-{n:02}"),
            "frame-trace",
            Some(1),
            100 + n as i64,
            "many",
        )
    }));
    s.repo.insert_logs(&logs).await.expect("the frame records");
    // A re-delivery of one record replaces it: the read sees its winner once.
    s.repo
        .insert_logs(&[frame(
            "f-a",
            "frame-trace",
            Some(1),
            10,
            "first, re-delivered",
        )])
        .await
        .expect("the re-delivery");

    let params = |trace: &str, key: u128| RequestFramesParams {
        project_id: ProjectId::from(PROJECT),
        trace_id: trace.to_string(),
        key,
        ingested_before_us: None,
    };
    let frames = s
        .repo
        .get_request_frames(&params("frame-trace", 1))
        .await
        .expect("the frames");
    assert_eq!(
        frames.len(),
        bound + 1,
        "one past the record bound, so the view can tell it was cut"
    );
    assert_eq!(
        frames
            .iter()
            .take(2)
            .map(|f| f.log_digest.as_str())
            .collect::<Vec<_>>(),
        ["f-a", "f-b"],
        "in record order"
    );
    assert!(
        frames[0]
            .messages_json
            .as_deref()
            .is_some_and(|messages| messages.contains("re-delivered")),
        "the winning revision"
    );
    assert!(
        frames.iter().all(|f| f.trace_id == "frame-trace"
            && f.log_digest.starts_with("f-")
            && !f.log_digest.contains("other")
            && f.log_digest != "f-none"
            && f.log_digest != "f-q"),
        "only this project's and this trace's records stating this key"
    );
    let scanned = rows_scanned(&s.profile);
    // Every record stating key 1: the many, the two before them, another trace's and another project's - a stored
    // key is opaque here, so these share it, and the project and trace filters apply over the keyed rows.
    let keyed = (bound + 3 + 4) as u64;
    assert!(
        scanned <= 2 * keyed,
        "the frame read scanned {scanned} rows in its two passes for the {keyed} its key names, so it read the store"
    );
    let (nulls, empties): (i64, i64) = s
        .service
        .conn()
        .query_row(
            "SELECT count(*) FILTER (WHERE frame_key IS NULL), count(*) FILTER (WHERE frame_key = 0) \
             FROM otel_logs",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("the stored keys");
    assert_eq!(
        (nulls, empties),
        (LOGS_AND_POINTS as i64 + 1, 0),
        "every record that frames nothing - the store's and `f-none` - stores NULL, and none a zero key"
    );
    assert_eq!(
        s.repo
            .get_request_frames(&RequestFramesParams {
                project_id: ProjectId::from("q"),
                ..params("frame-trace", 1)
            })
            .await
            .expect("the other project's frames")
            .iter()
            .map(|f| f.log_digest.as_str())
            .collect::<Vec<_>>(),
        ["f-q"],
        "each project reads only its own frames under a shared trace id and key"
    );
    assert!(
        s.repo
            .get_request_frames(&params("frame-trace", 3))
            .await
            .expect("no frames")
            .is_empty()
    );
    assert!(
        s.repo
            .get_request_frames(&params("frame-trace", 0))
            .await
            .expect("no key")
            .is_empty(),
        "the zero key frames nothing"
    );

    // Bytes are bounded in the statement too: past the byte bound a record comes back as its identity alone.
    let big = "x".repeat(sideseat_core::constants::REQUEST_FRAMES_MAX_BYTES);
    s.repo
        .insert_logs(&[
            frame("f-big-1", "big-trace", Some(u128::MAX - 11), 1, "small"),
            frame("f-big-2", "big-trace", Some(u128::MAX - 11), 2, &big),
        ])
        .await
        .expect("a large frame");
    let big_frames = s
        .repo
        .get_request_frames(&params("big-trace", u128::MAX - 11))
        .await
        .expect("the large frames");
    assert_eq!(
        big_frames
            .iter()
            .map(|f| (f.log_digest.as_str(), f.messages_json.is_some()))
            .collect::<Vec<_>>(),
        [("f-big-1", true), ("f-big-2", false)],
        "the record past the byte bound is returned without its bytes"
    );
}

/// **The frame read stays keyed when frames are everywhere**: one record in a hundred is a frame, of a thousand
/// traces, spread over most of the store's row groups, so their statistics cannot rule a key out - and the read of
/// one trace's frames still scans only its own records, through the index.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_frame_read_stays_keyed_when_frames_are_in_most_row_groups() {
    use sideseat_ports::traits::MessageStore;
    use sideseat_ports::types::RequestFramesParams;
    let s = store().await;
    s.service
        .write(|conn| {
            conn.execute_batch(&format!(
                "INSERT INTO otel_logs (project_id, log_digest, ordinal, \"timestamp\", severity_number, \
                 dropped_attributes_count, flags, ingested_at, trace_id, span_id, messages, frame_key) \
                 SELECT '{PROJECT}', 'frame-' || lpad(i::VARCHAR, 8, '0'), 0, \
                 make_timestamp({START_US} + i * 1000000), 9, 0, 0, make_timestamp({START_US}), \
                 lpad(((i // 100) % 1000)::VARCHAR, 32, '0'), 'span', '[{{\"text\":\"frame\"}}]', \
                 CASE WHEN i % 100 = 0 THEN ((i // 100) % 1000 + 1)::UHUGEINT END \
                 FROM range({}) r(i) ORDER BY i; CHECKPOINT;",
                2 * LOGS_AND_POINTS
            ))
            .map_err(Into::into)
        })
        .expect("frames in most row groups");
    let (groups, framed_groups): (i64, i64) = s
        .service
        .conn()
        .query_row(
            "SELECT count(DISTINCT row_group_id), \
             count(DISTINCT row_group_id) FILTER (WHERE stats LIKE '%Has No Null: true%') \
             FROM pragma_storage_info('otel_logs') WHERE column_name = 'frame_key' AND segment_type = 'UHUGEINT'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("the row groups");
    assert!(
        framed_groups * 3 >= groups * 2,
        "the frames should be in most of the {groups} row groups, but are in {framed_groups}"
    );
    // Trace 7's frames, keyed 8: i = 700 + 100,000 n, spread over the row groups the frames were written to.
    let frames = s
        .repo
        .get_request_frames(&RequestFramesParams {
            project_id: ProjectId::from(PROJECT),
            trace_id: format!("{:0>32}", 7),
            key: 8,
            ingested_before_us: None,
        })
        .await
        .expect("the frames");
    assert_eq!(
        frames
            .iter()
            .map(|frame| frame.log_digest.as_str())
            .collect::<Vec<_>>(),
        (0..8)
            .map(|n| format!("frame-{:08}", 700 + 100_000 * n))
            .collect::<Vec<_>>(),
        "the trace's frames, from every row group they are in"
    );
    let scanned = rows_scanned(&s.profile);
    assert!(
        scanned <= 16,
        "the frame read scanned {scanned} rows in its two passes for the trace's 8, so it read the store"
    );
}

/// **A trace with more frames than memory holds is still answered.** Twenty thousand frame records of 16 KB
/// under one key, 320 MB of messages against the engine's 200 MB limit: the read chooses its records from their
/// identities and lengths and reads messages for the chosen alone, so it returns the bound's worth. Read in one pass
/// with the messages carried along, the same statement ran out of memory.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_trace_with_more_frames_than_memory_holds_is_still_answered() {
    use sideseat_ports::traits::MessageStore;
    use sideseat_ports::types::RequestFramesParams;
    let temp = tempfile::TempDir::new().expect("temp dir");
    let storage = AppStorage::init_for_test(temp.path().to_path_buf());
    std::fs::create_dir_all(storage.subdir(sideseat_core::storage::DataSubdir::Duckdb))
        .expect("duckdb dir");
    let service = Arc::new(
        DuckdbService::init(&storage, Arc::new(TestClock))
            .await
            .expect("duckdb"),
    );
    service
        .write(|conn| {
            conn.execute_batch(&format!(
                "INSERT INTO otel_logs (project_id, log_digest, ordinal, \"timestamp\", severity_number, \
                 dropped_attributes_count, flags, ingested_at, trace_id, span_id, messages, frame_key) \
                 SELECT '{PROJECT}', lpad(i::VARCHAR, 8, '0'), 0, make_timestamp({START_US} + i * 1000), 9, 0, 0, \
                 make_timestamp({START_US}), 'trace', 'span', \
                 '[{{\"text\":\"' || i::VARCHAR || repeat('x', 16384) || '\"}}]', 1::UHUGEINT \
                 FROM range(20000) r(i); CHECKPOINT;"
            ))
            .map_err(Into::into)
        })
        .expect("the frames");
    let frames = DuckdbRepository(Arc::clone(&service))
        .get_request_frames(&RequestFramesParams {
            project_id: ProjectId::from(PROJECT),
            trace_id: "trace".to_string(),
            key: 1,
            ingested_before_us: None,
        })
        .await
        .expect("answered within the engine's memory");
    assert_eq!(
        frames.len(),
        sideseat_core::constants::REQUEST_FRAMES_MAX_RECORDS + 1
    );
    assert!(frames.iter().all(|frame| frame.messages_json.is_some()));
    assert_eq!(
        frames[0].log_digest, "00000000",
        "the first records, in order"
    );
}

/// **However many records a key finds, the frame read holds the bound's worth.** A hundred thousand frame records
/// under one key, one in four of the trace's 400,000 log records, read under a 24 MB memory limit: the read returns
/// the first ones. Keeping the first records is an aggregate of the bound's size over the key's scan, so what it
/// holds within the limit is the blocks its row fetches pin, whatever the count; with the key's records
/// materialised and ranked by a window, the same read ran out of memory under it (this read passes under 20 MB, that
/// one first under 32 MB). What the index scan holds outside
/// the limit, and what a read of a million such records costs in time, is in docs/engineering/request-context.md.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_frame_read_holds_the_bound_however_many_records_a_key_finds() {
    use sideseat_ports::traits::MessageStore;
    use sideseat_ports::types::RequestFramesParams;
    let temp = tempfile::TempDir::new().expect("temp dir");
    let storage = AppStorage::init_for_test(temp.path().to_path_buf());
    std::fs::create_dir_all(storage.subdir(sideseat_core::storage::DataSubdir::Duckdb))
        .expect("duckdb dir");
    let service = Arc::new(
        DuckdbService::init(&storage, Arc::new(TestClock))
            .await
            .expect("duckdb"),
    );
    service
        .write(|conn| {
            conn.execute_batch(&format!(
                "INSERT INTO otel_logs (project_id, log_digest, ordinal, \"timestamp\", severity_number, \
                 dropped_attributes_count, flags, ingested_at, trace_id, span_id, messages, frame_key) \
                 SELECT '{PROJECT}', lpad(i::VARCHAR, 8, '0'), 0, make_timestamp({START_US} + i * 1000), 9, 0, 0, \
                 make_timestamp({START_US}), 'trace', 'span', '[{{\"text\":\"f\"}}]', \
                 CASE WHEN i % 4 = 0 THEN 1::UHUGEINT END \
                 FROM range(400000) r(i); CHECKPOINT;"
            ))
            .map_err(Into::into)
        })
        .expect("the log records");
    service
        .conn()
        .execute_batch("SET memory_limit = '24MB';")
        .expect("a tight limit");
    let frames = DuckdbRepository(Arc::clone(&service))
        .get_request_frames(&RequestFramesParams {
            project_id: ProjectId::from(PROJECT),
            trace_id: "trace".to_string(),
            key: 1,
            ingested_before_us: None,
        })
        .await
        .expect("answered under the limit");
    assert_eq!(
        frames.len(),
        sideseat_core::constants::REQUEST_FRAMES_MAX_RECORDS + 1
    );
    assert_eq!(
        frames[0].log_digest, "00000000",
        "the first records, in order"
    );
    assert_eq!(
        frames[1].log_digest, "00000004",
        "and only the frames among them"
    );
}
