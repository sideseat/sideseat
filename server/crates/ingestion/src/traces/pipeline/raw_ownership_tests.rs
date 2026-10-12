//! An ingest's claim on its raw records through the arbiter (`server/specs/RawRecordOwnership.tla`, `Confirm`,
//! `Rekey`, `Check`), and the ownership's release by deletion and retention.

use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use prost::Message;
use sideseat_ports::types::{
    ProjectId, StagedSignal, raw_record_key, raw_trace_key, raw_version, raw_version_epoch,
};

use super::pipeline_tests::temp_pipeline;
use super::*;

const PROJECT: &str = "default";

fn export(trace: u8) -> ExportTraceServiceRequest {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(Resource::default()),
            scope_spans: vec![ScopeSpans {
                spans: (1..=2u8)
                    .map(|span| Span {
                        trace_id: vec![trace; 16],
                        span_id: vec![span; 8],
                        name: "step".into(),
                        start_time_unix_nano: 1_700_000_000_000_000_000 + u64::from(span),
                        end_time_unix_nano: 1_700_000_000_000_000_100 + u64::from(span),
                        attributes: vec![KeyValue {
                            key: "input.value".into(),
                            value: Some(AnyValue {
                                value: Some(any_value::Value::StringValue(format!("step {span}"))),
                            }),
                        }],
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

fn received(request: &ExportTraceServiceRequest) -> ReceivedPayload {
    ReceivedPayload::new(
        request.encode_to_vec(),
        sideseat_domain::raw_payload::RawContent::Protobuf,
        chrono::Utc::now(),
    )
}

fn trace_hex(trace: u8) -> String {
    hex::encode([trace; 16])
}

/// The key the arbiter keeps a trace's ownership under.
fn trace_key(trace: u8) -> Vec<u8> {
    raw_trace_key(&trace_hex(trace))
}

/// A written export's traces own its record in the transactional store, and its version is keyed under the
/// epoch the ingest read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ingest_s_traces_own_its_record() {
    let store = temp_pipeline(false).await;
    let request = export(31);
    let received = received(&request);
    store.pipeline.ingest_now(&request, &received).await;
    let project = ProjectId::from(PROJECT);
    let raw_id = crate::traces::raw_id(PROJECT, &received);
    assert_eq!(
        store
            .database
            .raw_owner_traces(&project, &raw_id)
            .await
            .unwrap(),
        vec![trace_key(31)]
    );
    let stored = store
        .analytics
        .get_raw_records(&project, std::slice::from_ref(&raw_id))
        .await
        .unwrap();
    assert_eq!(stored[0].version, raw_version(0, 1).unwrap());
}

/// A record whose only stored version lies under a collection - a stale copy keyed before the decision - is not
/// a version the ingest may claim: it is invisible, the ingest keys a fresh one above the decision, claims
/// that, and the stale copy is counted as a late landing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ingest_over_a_collected_copy_keys_a_fresh_version_and_owns_it() {
    let store = temp_pipeline(false).await;
    let project = ProjectId::from(PROJECT);
    let request = export(32);
    let received = received(&request);
    let raw_id = crate::traces::raw_id(PROJECT, &received);

    // The first delivery stores the record; its rows go with a deletion of nothing - they are simply removed -
    // and the record, owned by nothing, is collected; but its version is left behind, as a landing after the
    // purge leaves one.
    store.pipeline.ingest_now(&request, &received).await;
    let stale = store
        .analytics
        .get_raw_records(&project, std::slice::from_ref(&raw_id))
        .await
        .unwrap()
        .remove(0);
    store
        .database
        .release_raw_owners(
            &project,
            &[(raw_record_key(&raw_id), trace_key(32))],
            i64::MAX,
        )
        .await
        .unwrap();
    let decided = store
        .database
        .collect_raw_records(&project, std::slice::from_ref(&raw_id), chrono::Utc::now())
        .await
        .unwrap();
    assert_eq!(decided.len(), 1);
    store
        .database
        .drop_raw_collection(&project, &raw_id, decided[0].epoch)
        .await
        .unwrap();
    let late_before = crate::traces::late_raw_landings();

    store.pipeline.ingest_now(&request, &received).await;
    let latest = store
        .analytics
        .get_raw_records(&project, std::slice::from_ref(&raw_id))
        .await
        .unwrap()
        .remove(0);
    assert!(
        raw_version_epoch(latest.version) > decided[0].epoch,
        "the claimed version is keyed above the decision: {} vs {}",
        latest.version,
        stale.version
    );
    assert_eq!(
        store
            .database
            .raw_owner_traces(&project, &raw_id)
            .await
            .unwrap(),
        vec![trace_key(32)],
        "the fresh version's traces own the record"
    );
    assert!(
        crate::traces::late_raw_landings() > late_before,
        "the stale copy is counted as a late landing"
    );
}

/// The payload settles only after its claim is granted: with the arbiter unable to confirm, the batch fails, the
/// registration - and its pending-writer entry - stays, and the payload is delivered again; once the arbiter
/// answers, the redelivery claims the record and settles.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_payload_whose_claim_fails_never_settles() {
    let store = temp_pipeline(false).await;
    let project = ProjectId::from(PROJECT);
    let request = export(33);
    let received = received(&request);
    let raw_id = crate::traces::raw_id(PROJECT, &received);
    let reference = store
        .pipeline
        .staging
        .stage(
            PROJECT,
            StagedSignal::Traces,
            &received.staged(),
            received.received_at,
            crate::traces::confirmation_records(&request),
            crate::staging::StagedWrite {
                partition_key: "p".to_string(),
                raw_id: Some(raw_id.clone()),
            },
        )
        .await
        .expect("stage");

    // The arbiter cannot confirm: every claim fails.
    sideseat_adapter_sqlite::test_support::inject_raw_confirm_fault(&store.sqlite)
        .await
        .unwrap();
    assert!(
        !store.pipeline.process_staged_reference(&reference).await,
        "a failed claim is not a final answer"
    );
    assert!(
        store
            .pipeline
            .staging
            .load(&reference.id)
            .await
            .unwrap()
            .is_some(),
        "the registration stays: the payload did not settle"
    );
    assert!(
        store
            .database
            .raw_writer_pending(&project, &raw_id)
            .await
            .unwrap(),
        "and so does its pending-writer entry, which keeps the record out of collection"
    );

    sideseat_adapter_sqlite::test_support::clear_raw_confirm_fault(&store.sqlite)
        .await
        .unwrap();
    assert!(store.pipeline.process_staged_reference(&reference).await);
    assert!(
        store
            .pipeline
            .staging
            .load(&reference.id)
            .await
            .unwrap()
            .is_none(),
        "settled once the claim is granted"
    );
    assert!(
        !store
            .database
            .raw_writer_pending(&project, &raw_id)
            .await
            .unwrap()
    );
    assert_eq!(
        store
            .database
            .raw_owner_traces(&project, &raw_id)
            .await
            .unwrap(),
        vec![trace_key(33)]
    );
}

/// The span rows stored for a trace, counted on the table itself rather than through any index.
fn stored_spans(store: &super::pipeline_tests::TempPipeline, trace: u8) -> i64 {
    let trace = trace_hex(trace);
    store
        .duckdb
        .write(move |conn| {
            conn.query_row(
                "SELECT count(*) FROM otel_spans WHERE trace_id = ?",
                [trace],
                |row| row.get(0),
            )
            .map_err(Into::into)
        })
        .unwrap()
}

/// The span rows of a trace that are their identity's current winner.
fn winning_spans(store: &super::pipeline_tests::TempPipeline, trace: u8) -> i64 {
    let trace = trace_hex(trace);
    store
        .duckdb
        .write(move |conn| {
            conn.query_row(
                "SELECT count(*) FROM otel_spans WHERE trace_id = ? AND superseded_at IS NULL",
                [trace],
                |row| row.get(0),
            )
            .map_err(Into::into)
        })
        .unwrap()
}

fn stage(
    store: &super::pipeline_tests::TempPipeline,
    request: &ExportTraceServiceRequest,
    received: &ReceivedPayload,
    raw_id: &str,
) -> impl std::future::Future<Output = crate::staging::StagedPayloadRef> {
    let staging = store.pipeline.staging.clone();
    let staged = received.staged();
    let received_at = received.received_at;
    let records = crate::traces::confirmation_records(request);
    let raw_id = raw_id.to_string();
    async move {
        staging
            .stage(
                PROJECT,
                StagedSignal::Traces,
                &staged,
                received_at,
                records,
                crate::staging::StagedWrite {
                    partition_key: "p".to_string(),
                    raw_id: Some(raw_id),
                },
            )
            .await
            .expect("stage")
    }
}

/// A delivery overtaken by a collection - its claim answered with the decision pending - does not settle: the
/// decision's purge takes the record, so a settled payload would leave an acknowledged export with no record and
/// no owner. It fails, the purge runs, and the redelivery keys the record again above the decision, owns it and
/// settles.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delivery_overtaken_by_a_collection_is_claimed_again_by_its_redelivery() {
    let store = temp_pipeline(false).await;
    let project = ProjectId::from(PROJECT);
    let request = export(36);
    let received = received(&request);
    let raw_id = crate::traces::raw_id(PROJECT, &received);
    let first = stage(&store, &request, &received, &raw_id).await;
    assert!(store.pipeline.process_staged_reference(&first).await);

    // The rows' ownership goes, and a collection is decided; its purge has not run yet.
    store
        .database
        .release_raw_owners(
            &project,
            &[(raw_record_key(&raw_id), trace_key(36))],
            i64::MAX,
        )
        .await
        .unwrap();
    let decided = store
        .database
        .collect_raw_records(&project, std::slice::from_ref(&raw_id), chrono::Utc::now())
        .await
        .unwrap();
    assert_eq!(decided.len(), 1);

    let again = stage(&store, &request, &received, &raw_id).await;
    assert!(
        !store.pipeline.process_staged_reference(&again).await,
        "a claim answered with a pending decision is not granted"
    );
    assert!(
        store
            .pipeline
            .staging
            .load(&again.id)
            .await
            .unwrap()
            .is_some(),
        "the overtaken delivery does not settle"
    );

    // The decision's purge takes the record's versions and is dropped; the rows stay, as an unacknowledged
    // write's do.
    store
        .pipeline
        .purge_raw_collection(&project, &raw_id, decided[0].epoch)
        .await
        .unwrap();

    assert!(store.pipeline.process_staged_reference(&again).await);
    assert_eq!(
        winning_spans(&store, 36),
        2,
        "the redelivery's rows are in place"
    );
    assert_eq!(
        store
            .database
            .raw_owner_traces(&project, &raw_id)
            .await
            .unwrap(),
        vec![trace_key(36)]
    );
    for _ in 0..3 {
        store.pipeline.reconcile_raw_records(64).await.unwrap();
    }
    assert_eq!(
        winning_spans(&store, 36),
        2,
        "and the record, owned again, keeps them"
    );
}

/// Retention removes the rows, the release takes their traces' ownership, and the next pass collects the record
/// nothing owns any more.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retention_releases_ownership_and_the_record_is_collected() {
    let store = temp_pipeline(false).await;
    let project = ProjectId::from(PROJECT);
    let request = export(34);
    let received = received(&request);
    let raw_id = crate::traces::raw_id(PROJECT, &received);
    store.pipeline.ingest_now(&request, &received).await;
    // The retention pass's delete.
    store
        .duckdb
        .write(|conn| {
            conn.execute_batch("DELETE FROM otel_spans")
                .map_err(Into::into)
        })
        .unwrap();
    sideseat_ports::traits::release_retained_raw_owners(
        store.database.as_ref(),
        store.analytics.as_ref(),
        &project,
        &[trace_hex(34)],
    )
    .await
    .unwrap();
    assert!(
        store
            .database
            .raw_owner_traces(&project, &raw_id)
            .await
            .unwrap()
            .is_empty()
    );
    for _ in 0..3 {
        store.pipeline.reconcile_raw_records(64).await.unwrap();
    }
    assert!(
        store
            .analytics
            .raw_records_page(&project, None, 10)
            .await
            .unwrap()
            .is_empty(),
        "the record nothing owns is collected"
    );
}

/// A retention release that failed after releasing and before queueing is retried, and the retry - finding the
/// owners already gone - still queues the traces' records, which are then collected.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_retried_retention_release_queues_records_already_released() {
    let store = temp_pipeline(false).await;
    let project = ProjectId::from(PROJECT);
    let request = export(38);
    let received = received(&request);
    let raw_id = crate::traces::raw_id(PROJECT, &received);
    store.pipeline.ingest_now(&request, &received).await;
    store
        .duckdb
        .write(|conn| {
            conn.execute_batch("DELETE FROM otel_spans")
                .map_err(Into::into)
        })
        .unwrap();
    // The first attempt released and failed before its enqueue; an earlier entry was cleared meanwhile.
    store
        .database
        .release_raw_owners(
            &project,
            &[(raw_record_key(&raw_id), trace_key(38))],
            i64::MAX,
        )
        .await
        .unwrap();
    let drained = store
        .analytics
        .pending_raw_records(usize::MAX, None)
        .await
        .unwrap();
    store.analytics.clear_raw_pending(&drained).await.unwrap();

    sideseat_ports::traits::release_retained_raw_owners(
        store.database.as_ref(),
        store.analytics.as_ref(),
        &project,
        &[trace_hex(38)],
    )
    .await
    .unwrap();
    assert!(
        store
            .analytics
            .pending_raw_records(usize::MAX, None)
            .await
            .unwrap()
            .iter()
            .any(|entry| entry.raw_id == raw_id),
        "the retry queues the record its first attempt released"
    );
}

/// A collection takes the record's versions and leaves its rows: rows naming a collected record are those of a
/// write the client was never told succeeded - here an ingest whose ownership is gone - and they go with their
/// trace's retention or deletion, on both backends alike. No reader takes them for stored: nothing owns them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_collection_leaves_the_rows_of_an_unacknowledged_write() {
    let store = temp_pipeline(false).await;
    let project = ProjectId::from(PROJECT);
    let request = export(39);
    let received = received(&request);
    let raw_id = crate::traces::raw_id(PROJECT, &received);
    store.pipeline.ingest_now(&request, &received).await;
    store
        .database
        .release_raw_owners(
            &project,
            &[(raw_record_key(&raw_id), trace_key(39))],
            i64::MAX,
        )
        .await
        .unwrap();
    store
        .analytics
        .enqueue_raw_records(&project, std::slice::from_ref(&raw_id))
        .await
        .unwrap();
    for _ in 0..3 {
        store.pipeline.reconcile_raw_records(64).await.unwrap();
    }
    assert!(
        store
            .analytics
            .get_raw_records(&project, std::slice::from_ref(&raw_id))
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        stored_spans(&store, 39),
        2,
        "the rows stay until their trace's retention"
    );
}

/// A collector that stalls after its decision can run its purge at any time later - after a takeover purged and
/// dropped the decision and a redelivery claimed the record again. The redelivery keyed its version under a
/// later epoch than the decision, so the late purge, bounded by the decision's epoch, cannot reach it, and no
/// collection removes rows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stalled_collector_s_late_purge_cannot_reach_a_granted_redelivery() {
    let store = temp_pipeline(false).await;
    let project = ProjectId::from(PROJECT);
    let request = export(41);
    let received = received(&request);
    let raw_id = crate::traces::raw_id(PROJECT, &received);
    store.pipeline.ingest_now(&request, &received).await;
    store
        .database
        .release_raw_owners(
            &project,
            &[(raw_record_key(&raw_id), trace_key(41))],
            i64::MAX,
        )
        .await
        .unwrap();
    // Collector A decides, then stalls.
    let decided = store
        .database
        .collect_raw_records(&project, std::slice::from_ref(&raw_id), chrono::Utc::now())
        .await
        .unwrap();
    assert_eq!(decided.len(), 1);
    // B takes the decision over: removes, purges, drops.
    store
        .pipeline
        .purge_raw_collection(&project, &raw_id, decided[0].epoch)
        .await
        .unwrap();
    // A redelivery claims the record again and settles.
    let again = stage(&store, &request, &received, &raw_id).await;
    assert!(store.pipeline.process_staged_reference(&again).await);
    assert_eq!(
        store
            .database
            .raw_owner_traces(&project, &raw_id)
            .await
            .unwrap(),
        vec![trace_key(41)]
    );
    // A resumes with its decision's purge.
    store
        .pipeline
        .purge_raw_collection(&project, &raw_id, decided[0].epoch)
        .await
        .unwrap();
    assert_eq!(
        winning_spans(&store, 41),
        2,
        "the granted redelivery's rows are in place"
    );
    assert_eq!(
        store
            .analytics
            .get_raw_records(&project, std::slice::from_ref(&raw_id))
            .await
            .unwrap()
            .len(),
        1,
        "and so is its record"
    );
}

/// A record its traces still own is never collected, whatever the analytics rows say - here they are gone
/// without a retention release, as a row a merge folded away is: the arbiter, not the rows, decides.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_owned_record_is_never_collected() {
    let store = temp_pipeline(false).await;
    let project = ProjectId::from(PROJECT);
    let request = export(35);
    let received = received(&request);
    let raw_id = crate::traces::raw_id(PROJECT, &received);
    store.pipeline.ingest_now(&request, &received).await;
    store
        .duckdb
        .write(|conn| {
            conn.execute_batch("DELETE FROM otel_spans")
                .map_err(Into::into)
        })
        .unwrap();
    store
        .analytics
        .enqueue_raw_records(&project, std::slice::from_ref(&raw_id))
        .await
        .unwrap();
    for _ in 0..3 {
        store.pipeline.reconcile_raw_records(64).await.unwrap();
    }
    assert_eq!(
        store
            .analytics
            .get_raw_records(&project, std::slice::from_ref(&raw_id))
            .await
            .unwrap()
            .len(),
        1
    );
}

/// The arbiter as a retention release sees it when an ingest confirms between the release's read of the grant
/// sequence and its release: every grant made so far is one the read did not cover.
struct GrantedAfterTheRead<'a>(&'a (dyn sideseat_ports::traits::RawArbiter + Sync));

#[async_trait::async_trait]
impl sideseat_ports::traits::RawArbiter for GrantedAfterTheRead<'_> {
    async fn raw_arbiter_epoch(
        &self,
        project_id: &ProjectId,
    ) -> Result<i64, sideseat_ports::error::DataError> {
        self.0.raw_arbiter_epoch(project_id).await
    }
    async fn raw_record_cutoffs(
        &self,
        project_id: &ProjectId,
        raw_ids: &[String],
    ) -> Result<HashMap<String, sideseat_ports::types::RawCutoff>, sideseat_ports::error::DataError>
    {
        self.0.raw_record_cutoffs(project_id, raw_ids).await
    }
    async fn confirm_raw_owners(
        &self,
        project_id: &ProjectId,
        claims: &[sideseat_ports::types::RawOwnerClaim],
    ) -> Result<Vec<sideseat_ports::types::RawConfirmOutcome>, sideseat_ports::error::DataError>
    {
        self.0.confirm_raw_owners(project_id, claims).await
    }
    async fn raw_grant_seq(
        &self,
        project_id: &ProjectId,
    ) -> Result<i64, sideseat_ports::error::DataError> {
        Ok(self.0.raw_grant_seq(project_id).await? - 1)
    }
    async fn raw_owners_of_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<(i64, Vec<u8>)>, sideseat_ports::error::DataError> {
        self.0.raw_owners_of_traces(project_id, trace_ids).await
    }
    async fn raw_owned_pairs(
        &self,
        project_id: &ProjectId,
        pairs: &[(i64, Vec<u8>)],
    ) -> Result<Vec<(i64, Vec<u8>)>, sideseat_ports::error::DataError> {
        self.0.raw_owned_pairs(project_id, pairs).await
    }
    async fn raw_owner_traces(
        &self,
        project_id: &ProjectId,
        raw_id: &str,
    ) -> Result<Vec<Vec<u8>>, sideseat_ports::error::DataError> {
        self.0.raw_owner_traces(project_id, raw_id).await
    }
    async fn release_raw_owners(
        &self,
        project_id: &ProjectId,
        owners: &[(i64, Vec<u8>)],
        granted_up_to: i64,
    ) -> Result<(), sideseat_ports::error::DataError> {
        self.0
            .release_raw_owners(project_id, owners, granted_up_to)
            .await
    }
    async fn collect_raw_records(
        &self,
        project_id: &ProjectId,
        raw_ids: &[String],
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<sideseat_ports::types::RawCollection>, sideseat_ports::error::DataError> {
        self.0.collect_raw_records(project_id, raw_ids, now).await
    }
    async fn drop_raw_collection(
        &self,
        project_id: &ProjectId,
        raw_id: &str,
        epoch: i64,
    ) -> Result<(), sideseat_ports::error::DataError> {
        self.0.drop_raw_collection(project_id, raw_id, epoch).await
    }
    async fn stale_raw_collections(
        &self,
        decided_before: chrono::DateTime<chrono::Utc>,
        limit: usize,
    ) -> Result<Vec<sideseat_ports::types::RawCollection>, sideseat_ports::error::DataError> {
        self.0.stale_raw_collections(decided_before, limit).await
    }
    async fn raw_writer_pending(
        &self,
        project_id: &ProjectId,
        raw_id: &str,
    ) -> Result<bool, sideseat_ports::error::DataError> {
        self.0.raw_writer_pending(project_id, raw_id).await
    }
}

/// An ingest that wrote expired rows the retention delete took, and confirmed after the release read the grant
/// sequence, keeps its owner - and the release says it is not done, so its cleanup stays owed: completed, nothing
/// would find the trace again, its rows being gone. The next attempt, reading a later sequence, releases it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_release_that_keeps_a_later_grant_is_not_complete() {
    let store = temp_pipeline(false).await;
    let project = ProjectId::from(PROJECT);
    let request = export(43);
    let received = received(&request);
    let raw_id = crate::traces::raw_id(PROJECT, &received);
    store.pipeline.ingest_now(&request, &received).await;
    store
        .duckdb
        .write(|conn| {
            conn.execute_batch("DELETE FROM otel_spans")
                .map_err(Into::into)
        })
        .unwrap();
    let complete = sideseat_ports::traits::release_retained_raw_owners(
        &GrantedAfterTheRead(store.database.as_ref()),
        store.analytics.as_ref(),
        &project,
        &[trace_hex(43)],
    )
    .await
    .unwrap();
    assert!(
        !complete,
        "a grant the read did not cover leaves the cleanup owed"
    );
    assert_eq!(
        store
            .database
            .raw_owner_traces(&project, &raw_id)
            .await
            .unwrap(),
        vec![trace_key(43)]
    );
    let complete = sideseat_ports::traits::release_retained_raw_owners(
        store.database.as_ref(),
        store.analytics.as_ref(),
        &project,
        &[trace_hex(43)],
    )
    .await
    .unwrap();
    assert!(complete);
    assert!(
        store
            .database
            .raw_owner_traces(&project, &raw_id)
            .await
            .unwrap()
            .is_empty(),
        "the retry releases it"
    );
}
