//! A fatal error stops the service: it is recorded, reported as in doubt, and survives nothing but a reopen - and
//! the reopened database holds every commit DuckDB reported durable.
//!
//! The failure is the real one, raised where DuckDB raises it: a checkpoint aborted by DuckDB's own
//! `debug_checkpoint_abort` setting fails exactly as a checkpoint out of memory does, inside the checkpoint, so
//! the instance invalidates itself and reports it as it would.

use std::sync::Arc;

use sideseat_core::storage::{AppStorage, DataSubdir};
use sideseat_ports::error::DataError;
use sideseat_ports::traits::{AnalyticsMaintenance, LogStore, RawStore, SpanStore};
use sideseat_ports::types::{
    NormalizedLog, NormalizedSpan, ProjectId, RawOrigin, RawRecordRow, StagedSignal,
};

use super::{DuckdbRepository, DuckdbService, TestClock};

const PROJECT: &str = "failing";

async fn open(storage: &AppStorage) -> Arc<DuckdbService> {
    Arc::new(
        DuckdbService::init(storage, Arc::new(TestClock))
            .await
            .expect("duckdb"),
    )
}

async fn service() -> (tempfile::TempDir, AppStorage, Arc<DuckdbService>) {
    let directory = tempfile::TempDir::new().expect("temp dir");
    let storage = AppStorage::init_for_test(directory.path().to_path_buf());
    std::fs::create_dir_all(storage.subdir(DataSubdir::Duckdb)).expect("duckdb dir");
    let service = open(&storage).await;
    (directory, storage, service)
}

fn span(id: u8) -> NormalizedSpan {
    NormalizedSpan {
        project_id: Some(PROJECT.to_string()),
        trace_id: format!("{:032x}", 1),
        span_id: format!("{id:016x}"),
        span_name: "call".to_string(),
        timestamp_start: chrono::DateTime::from_timestamp(1_790_000_000, 0).expect("instant"),
        ..Default::default()
    }
}

fn log(digest: &str) -> NormalizedLog {
    NormalizedLog {
        project_id: Some(PROJECT.to_string()),
        log_digest: digest.to_string(),
        timestamp: chrono::DateTime::from_timestamp(1_790_000_000, 0).expect("instant"),
        ..Default::default()
    }
}

/// Make every checkpoint from now on fail fatally, inside the checkpoint.
fn fail_checkpoints(service: &DuckdbService) {
    service
        .conn()
        .execute_batch("SET debug_checkpoint_abort = 'before_header'")
        .expect("the abort");
}

/// The error a store that has failed for good answers with: in doubt, so no write's bookkeeping is undone on its
/// strength, and transient, so an exporter sends again - to the process that replaces this one.
fn assert_stopped(error: &DataError) {
    assert!(error.write_in_doubt(), "not in doubt: {error}");
    assert!(error.is_transient(), "not transient: {error}");
}

/// A commit whose auto-checkpoint fails stops the service: the failure is recorded with DuckDB's reason, every
/// later call is refused as in doubt, and only a reopen serves again.
///
/// Whether the commit survives depends on how DuckDB committed it, which is why its error is in doubt. A commit
/// DuckDB wrote to the write-ahead log before checkpointing is durable, and its error says so. A commit large
/// enough (`auto_checkpoint_skip_wal_threshold`) is not written to the log at all - the checkpoint is to make it
/// durable - so a failed checkpoint fails the commit, and the reopened database rightly does not hold it.
#[tokio::test]
async fn a_commit_whose_checkpoint_fails_stops_the_service_until_a_reopen() {
    for logged in [true, false] {
        let (_directory, storage, service) = service().await;
        let repository = DuckdbRepository(Arc::clone(&service));
        let mut failures = service.subscribe_fatal_failure();
        repository
            .insert_spans(vec![span(1)])
            .await
            .expect("a span before");
        fail_checkpoints(&service);
        checkpoint_every_commit(&service, logged);

        let error = repository
            .insert_logs(&[log("after")])
            .await
            .expect_err("the commit's checkpoint fails");
        assert_stopped(&error);
        let failure = service.fatal_failure().expect("the failure is recorded");
        assert_eq!(
            failure.contains("COMMIT succeeded and is durable"),
            logged,
            "DuckDB's reason: {failure}"
        );
        failures
            .wait_for(Option::is_some)
            .await
            .expect("subscribers hear of it");
        assert_eq!(
            repository.fatal_failure().as_deref(),
            Some(&*failure),
            "the port reports it"
        );
        let refused = repository
            .count_project_rows(&ProjectId::from(PROJECT))
            .await
            .expect_err("a read after it");
        assert_stopped(&refused);

        drop(repository);
        drop(Arc::into_inner(service).expect("the last handle"));
        let reopened = DuckdbRepository(open(&storage).await);
        assert_eq!(reopened.fatal_failure(), None, "a reopened database serves");
        assert_eq!(
            reopened
                .count_project_rows(&ProjectId::from(PROJECT))
                .await
                .expect("the reopened database reads"),
            if logged { 2 } else { 1 },
            "the span, and the log record only if its commit reached the log"
        );
    }
}

/// Make every commit checkpoint first, logged or not; see the test below.
fn checkpoint_every_commit(service: &DuckdbService, logged: bool) {
    let skip_wal = if logged { u64::MAX } else { 0 };
    service
        .conn()
        .execute_batch(&format!(
            "SET checkpoint_threshold = '1B'; SET auto_checkpoint_skip_wal_threshold = {skip_wal}"
        ))
        .expect("every commit checkpoints");
}

/// The same through an appender, whose errors carry DuckDB's message without its type: the raw records are
/// written by one, committing as it flushes, and their commit's failed checkpoint is told apart by the probe.
#[tokio::test]
async fn an_appended_commit_whose_checkpoint_fails_stops_the_service() {
    let (_directory, _storage, service) = service().await;
    let repository = DuckdbRepository(Arc::clone(&service));
    repository
        .insert_spans(vec![span(1)])
        .await
        .expect("a span before");
    fail_checkpoints(&service);
    checkpoint_every_commit(&service, true);
    let at = chrono::DateTime::from_timestamp(1_790_000_000, 0).expect("instant");

    let error = repository
        .insert_raw_records(&[RawRecordRow {
            project_id: ProjectId::from(PROJECT),
            raw_id: "raw".to_string(),
            signal: StagedSignal::Traces,
            received_at: at,
            origin: RawOrigin::Received,
            version: 1,
            signal_until: at,
            hold_until: None,
            trace_ids: vec![format!("{:032x}", 1)],
            record: b"record".to_vec(),
        }])
        .await
        .expect_err("the appender's commit's checkpoint fails");
    assert_stopped(&error);
    let failure = service.fatal_failure().expect("the failure is recorded");
    assert!(
        failure.contains("COMMIT succeeded and is durable"),
        "the appender's own message: {failure}"
    );
}

/// A failure met by work whose caller stopped waiting - a read past the query timeout - is still recorded.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failure_met_after_its_caller_stopped_waiting_is_recorded() {
    let (_directory, _storage, service) = service().await;
    DuckdbRepository(Arc::clone(&service))
        .insert_spans(vec![span(1)])
        .await
        .expect("a span");
    fail_checkpoints(&service);
    let (started, start) = std::sync::mpsc::channel::<()>();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let waiting = tokio::spawn({
        let service = Arc::clone(&service);
        async move {
            service
                .run_query({
                    let service = Arc::clone(&service);
                    move || {
                        started.send(()).expect("the test waits");
                        released.recv().expect("the test releases");
                        service
                            .conn()
                            .execute_batch("CHECKPOINT")
                            .map_err(Into::into)
                    }
                })
                .await
        }
    });
    tokio::task::spawn_blocking(move || start.recv())
        .await
        .expect("the wait")
        .expect("the work started");
    waiting.abort();
    let _ = waiting.await;
    release.send(()).expect("the work waits");
    for _ in 0..200 {
        if service.fatal_failure().is_some() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the abandoned work's failure was not recorded");
}

/// A checkpoint of its own that fails - the periodic one, or the one at shutdown - stops the service too.
#[tokio::test]
async fn a_checkpoint_that_fails_stops_the_service() {
    let (_directory, _storage, service) = service().await;
    let repository = DuckdbRepository(Arc::clone(&service));
    repository
        .insert_spans(vec![span(1)])
        .await
        .expect("a span");
    fail_checkpoints(&service);

    service
        .checkpoint()
        .await
        .expect_err("the checkpoint fails");
    let failure = service.fatal_failure().expect("the failure is recorded");
    assert!(
        failure.contains("Failed to create checkpoint"),
        "DuckDB's reason: {failure}"
    );
}

/// A database invalidated where the service did not see it - by a statement made on the connection directly -
/// is recorded by the first call after it, which the database refuses.
#[tokio::test]
async fn the_first_call_after_an_unseen_failure_records_it() {
    let (_directory, _storage, service) = service().await;
    let repository = DuckdbRepository(Arc::clone(&service));
    repository
        .insert_spans(vec![span(1)])
        .await
        .expect("a span");
    fail_checkpoints(&service);
    service
        .conn()
        .execute_batch("CHECKPOINT")
        .expect_err("a checkpoint the service does not see");
    assert_eq!(service.fatal_failure(), None, "not seen yet");

    let refused = repository
        .count_project_rows(&ProjectId::from(PROJECT))
        .await
        .expect_err("the next call");
    assert_stopped(&refused);
    let failure = service.fatal_failure().expect("recorded by the next call");
    assert!(
        failure.contains("invalidated"),
        "DuckDB's refusal: {failure}"
    );
}

/// Work that fails on its own after meeting a fatal error it only logged - a rollback that failed behind the
/// error it returns - is recorded too: the error it returns is probed whatever it is.
#[tokio::test]
async fn an_error_of_the_work_met_with_the_database_invalidated_is_recorded() {
    let (_directory, _storage, service) = service().await;
    DuckdbRepository(Arc::clone(&service))
        .insert_spans(vec![span(1)])
        .await
        .expect("a span");
    fail_checkpoints(&service);
    let error = service
        .write(|conn| {
            if let Err(error) = conn.execute_batch("CHECKPOINT") {
                tracing::warn!(%error, "logged and dropped");
            }
            Err::<(), _>(crate::DuckdbError::Io(std::io::Error::other(
                "the work's own",
            )))
        })
        .expect_err("the work fails");
    assert_stopped(&DataError::from(error));
    let failure = service.fatal_failure().expect("recorded");
    assert!(
        failure.contains("invalidated") && failure.contains("the work's own"),
        "the refusal and the error that met it: {failure}"
    );
}

/// The same through `run_query`, for work that holds no guard when it returns: the connection is taken again to
/// probe it.
#[tokio::test]
async fn an_error_of_work_on_its_own_guard_met_with_the_database_invalidated_is_recorded() {
    let (_directory, _storage, service) = service().await;
    DuckdbRepository(Arc::clone(&service))
        .insert_spans(vec![span(1)])
        .await
        .expect("a span");
    fail_checkpoints(&service);
    let error = service
        .run_query({
            let service = Arc::clone(&service);
            move || {
                let checkpointed = service.conn().execute_batch("CHECKPOINT");
                if let Err(error) = checkpointed {
                    tracing::warn!(%error, "logged and dropped");
                }
                Err::<(), _>(crate::DuckdbError::Io(std::io::Error::other(
                    "the work's own",
                )))
            }
        })
        .await
        .expect("the work ran")
        .expect_err("the work fails");
    assert_stopped(&DataError::from(error));
    assert!(service.fatal_failure().is_some(), "recorded");
}

/// An error that leaves the database serving - a statement that is wrong - records nothing and stays settled.
#[tokio::test]
async fn an_error_the_database_serves_past_records_nothing() {
    let (_directory, _storage, service) = service().await;
    let error = service
        .run_query({
            let service = Arc::clone(&service);
            move || {
                service
                    .conn()
                    .execute_batch("SELECT * FROM no_such_table")
                    .map_err(Into::into)
            }
        })
        .await
        .expect("the work ran")
        .expect_err("the statement is wrong");
    assert_eq!(error.fatal_message(), None, "{error}");
    assert_eq!(service.fatal_failure(), None);
    assert!(!DataError::from(error).write_in_doubt());
    DuckdbRepository(Arc::clone(&service))
        .count_project_rows(&ProjectId::from(PROJECT))
        .await
        .expect("the database still serves");
}
