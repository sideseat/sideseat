use super::*;
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use sideseat_adapter_blob_storage::FilesystemStorage;
use sideseat_adapter_sqlite::{SqliteRepository, SqliteService};
use sideseat_core::storage::AppStorage;
use sideseat_ports::clock::Clock;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::TempDir;

#[derive(Debug)]
struct TestClock;

impl Clock for TestClock {
    fn now(&self) -> chrono::DateTime<Utc> {
        test_now()
    }
}

fn test_now() -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(1_704_067_200, 0).single().unwrap()
}

async fn setup() -> (
    TempDir,
    Arc<dyn TransactionalRepository + Send + Sync>,
    Arc<dyn FileStorage>,
    ContentBodyService,
) {
    let temp = TempDir::new().unwrap();
    let app_storage = AppStorage::init_for_test(temp.path().to_path_buf());
    let sqlite = Arc::new(
        SqliteService::init(&app_storage, Arc::new(TestClock))
            .await
            .unwrap(),
    );
    let database: Arc<dyn TransactionalRepository + Send + Sync> =
        Arc::new(SqliteRepository(sqlite));
    let storage: Arc<dyn FileStorage> =
        Arc::new(FilesystemStorage::new(temp.path().join("bodies")));
    let service = ContentBodyService {
        storage: Arc::clone(&storage),
        database: Arc::clone(&database),
    };
    (temp, database, storage, service)
}

fn message_row(messages: &str) -> MessageSpanRow {
    MessageSpanRow {
        trace_id: "trace".into(),
        span_id: "span".into(),
        parent_span_id: None,
        span_timestamp: test_now(),
        span_end_timestamp: Some(test_now()),
        messages_json: messages.into(),
        tool_definitions_json: "inline-tools".into(),
        tool_names_json: "inline-names".into(),
        log_messages_json: "[]".to_string(),
        body_cache_key: None,
        model: None,
        provider: None,
        status_code: None,
        exception_type: None,
        exception_message: None,
        exception_stacktrace: None,
        input_tokens: 0,
        output_tokens: 0,
        total_tokens: 0,
        cost_total: 0.0,
        observation_type: None,
        session_id: None,
        ingested_at: test_now(),
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
}

struct BackfillRows(Vec<SpanBodySource>);

#[async_trait]
impl SurvivorReferences for BackfillRows {
    async fn survivor_raw_records(
        &self,
        _project_id: &ProjectId,
        _trace_ids: &[String],
    ) -> Result<Vec<Vec<u8>>, DataError> {
        Ok(Vec::new())
    }

    async fn file_reference_fields_for_traces(
        &self,
        _project_id: &ProjectId,
        _trace_ids: &[String],
    ) -> Result<Vec<String>, DataError> {
        Ok(Vec::new())
    }

    async fn span_body_fields_for_traces(
        &self,
        _project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<SpanBodySource>, DataError> {
        Ok(self
            .0
            .iter()
            .filter(|row| trace_ids.contains(&row.trace_id))
            .cloned()
            .collect())
    }

    async fn span_body_backfill_page(
        &self,
        _project_id: &ProjectId,
        after: Option<(String, String)>,
        limit: usize,
    ) -> Result<Vec<SpanBodySource>, DataError> {
        Ok(self
            .0
            .iter()
            .filter(|row| {
                after
                    .as_ref()
                    .is_none_or(|cursor| (&row.trace_id, &row.span_id) > (&cursor.0, &cursor.1))
            })
            .take(limit)
            .cloned()
            .collect())
    }
}

struct CountingStorage {
    inner: FilesystemStorage,
    gets: AtomicUsize,
}

impl CountingStorage {
    fn new(path: std::path::PathBuf) -> Self {
        Self {
            inner: FilesystemStorage::new(path),
            gets: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl FileStorage for CountingStorage {
    async fn store(
        &self,
        project_id: &ProjectId,
        hash: &str,
        data: &[u8],
    ) -> Result<(), FileStorageError> {
        self.inner.store(project_id, hash, data).await
    }

    async fn get(&self, project_id: &ProjectId, hash: &str) -> Result<Vec<u8>, FileStorageError> {
        self.gets.fetch_add(1, Ordering::Relaxed);
        self.inner.get(project_id, hash).await
    }

    async fn exists(&self, project_id: &ProjectId, hash: &str) -> Result<bool, FileStorageError> {
        self.inner.exists(project_id, hash).await
    }

    async fn delete(&self, project_id: &ProjectId, hash: &str) -> Result<(), FileStorageError> {
        self.inner.delete(project_id, hash).await
    }

    async fn delete_project(&self, project_id: &ProjectId) -> Result<u64, FileStorageError> {
        self.inner.delete_project(project_id).await
    }

    async fn finalize_temp(
        &self,
        project_id: &ProjectId,
        hash: &str,
        temp_path: &Path,
    ) -> Result<(), FileStorageError> {
        self.inner.finalize_temp(project_id, hash, temp_path).await
    }
}

#[test]
fn body_hash_is_domain_separated_and_stable() {
    assert_eq!(
        ContentBodyService::hash(b"same"),
        ContentBodyService::hash(b"same")
    );
    assert_ne!(
        ContentBodyService::hash(b"same"),
        hex::encode(Sha256::digest(b"same"))
    );
}

#[test]
fn collection_deduplicates_objects_but_not_field_ownership() {
    let span = NormalizedSpan {
        project_id: Some("p".into()),
        trace_id: "t".into(),
        span_id: "s".into(),
        content_digest: "d".into(),
        messages: Some("[]".into()),
        tool_names: Some("[]".into()),
        ..Default::default()
    };
    let (objects, associations, digests) = collect(&[span]);
    assert_eq!(objects.len(), 1);
    assert_eq!(associations.len(), 2);
    assert_eq!(digests.len(), 1);
}

#[tokio::test]
async fn confirmed_body_is_preferred_and_missing_blob_falls_back_inline() {
    let (_temp, database, storage, service) = setup().await;
    let project_id = ProjectId::from("project");
    let span = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: "trace".into(),
        span_id: "span".into(),
        content_digest: "digest".into(),
        messages: Some("body-messages".into()),
        tool_definitions: Some("body-tools".into()),
        tool_names: Some("body-names".into()),
        ..Default::default()
    };
    let staged = service.stage(&[span], None).await.unwrap();
    assert_eq!(
        database
            .confirm_span_bodies(&staged.associations)
            .await
            .unwrap(),
        3
    );

    let mut rows = vec![message_row("inline-messages")];
    service.hydrate_message_rows(&project_id, &mut rows).await;
    assert_eq!(rows[0].messages_json, "body-messages");
    assert_eq!(rows[0].tool_definitions_json, "body-tools");
    assert_eq!(rows[0].tool_names_json, "body-names");
    let hydrated_key = rows[0].body_cache_key.clone().unwrap();

    storage
        .delete(&project_id, &ContentBodyService::hash(b"body-messages"))
        .await
        .unwrap();
    let mut fallback = vec![message_row("inline-messages")];
    service
        .hydrate_message_rows(&project_id, &mut fallback)
        .await;
    assert_eq!(fallback[0].messages_json, "inline-messages");
    assert_ne!(
        fallback[0].body_cache_key.as_deref(),
        Some(hydrated_key.as_str())
    );
}

#[tokio::test]
async fn shared_body_is_fetched_once_per_hydration() {
    let (temp, database, _storage, _service) = setup().await;
    let project_id = ProjectId::from("project");
    let storage = Arc::new(CountingStorage::new(temp.path().join("counted-bodies")));
    let service = ContentBodyService {
        storage: storage.clone(),
        database: Arc::clone(&database),
    };
    let spans = ["trace-a", "trace-b"].map(|trace_id| NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: trace_id.into(),
        span_id: "span".into(),
        content_digest: format!("digest-{trace_id}"),
        messages: Some("shared-body".into()),
        ..Default::default()
    });
    let staged = service.stage(&spans, None).await.unwrap();
    database
        .confirm_span_bodies(&staged.associations)
        .await
        .unwrap();

    let mut rows = vec![message_row("inline"), message_row("inline")];
    rows[0].trace_id = "trace-a".into();
    rows[1].trace_id = "trace-b".into();
    service.hydrate_message_rows(&project_id, &mut rows).await;

    assert_eq!(rows[0].messages_json, "shared-body");
    assert_eq!(rows[1].messages_json, "shared-body");
    assert_eq!(storage.gets.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn backfill_resumes_after_a_full_page_and_marks_cutover_complete() {
    let (_temp, database, _storage, service) = setup().await;
    let project_id = ProjectId::from("project");
    let rows = BackfillRows(
        (0..300)
            .map(|index| SpanBodySource {
                trace_id: format!("trace-{index:04}"),
                span_id: "span".into(),
                messages: Some("shared-body".into()),
                tool_definitions: None,
                tool_names: None,
                raw_span: None,
            })
            .collect(),
    );

    let first = service
        .backfill_project_page(&project_id, &rows, test_now())
        .await
        .unwrap();
    assert!(!first.complete);
    assert_eq!(first.cursor_trace_id.as_deref(), Some("trace-0255"));

    let second = service
        .backfill_project_page(
            &project_id,
            &rows,
            test_now() + chrono::Duration::seconds(1),
        )
        .await
        .unwrap();
    assert!(second.complete);
    assert_eq!(second.cursor_trace_id.as_deref(), Some("trace-0299"));
    assert_eq!(
        database
            .get_span_body_hash(&project_id, "trace-0299", "span", SpanBodyField::Messages,)
            .await
            .unwrap()
            .as_deref(),
        Some(ContentBodyService::hash(b"shared-body").as_str())
    );
}

#[tokio::test]
async fn stale_deletion_claim_is_finished_after_worker_crash() {
    let (_temp, database, storage, service) = setup().await;
    let project_id = ProjectId::from("project");
    let body_hash = ContentBodyService::hash(b"orphan");
    database
        .register_content_bodies(&[ContentBodyObject {
            project_id: project_id.clone(),
            body_hash: body_hash.clone(),
            logical_bytes: 6,
        }])
        .await
        .unwrap();
    storage
        .store(&project_id, &body_hash, b"orphan")
        .await
        .unwrap();
    assert!(
        database
            .claim_content_body_for_deletion(&project_id, &body_hash)
            .await
            .unwrap()
    );

    service
        .sweep_orphans(test_now() + chrono::Duration::seconds(301))
        .await;

    assert!(!storage.exists(&project_id, &body_hash).await.unwrap());
    assert!(
        database
            .get_stale_claimed_content_bodies(test_now() + chrono::Duration::hours(1), 10,)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn restore_cleanup_drains_more_than_one_orphan_page_without_grace() {
    let (_temp, database, storage, service) = setup().await;
    let project_id = ProjectId::from("project");
    let objects = (0..(ORPHAN_SWEEP_LIMIT + 44))
        .map(|index| {
            let bytes = format!("orphan-{index:03}");
            ContentBodyObject {
                project_id: project_id.clone(),
                body_hash: ContentBodyService::hash(bytes.as_bytes()),
                logical_bytes: bytes.len() as u64,
            }
        })
        .collect::<Vec<_>>();
    database
        .register_content_bodies(&objects)
        .await
        .expect("register body orphans");
    for (index, object) in objects.iter().enumerate() {
        storage
            .store(
                &project_id,
                &object.body_hash,
                format!("orphan-{index:03}").as_bytes(),
            )
            .await
            .expect("store body orphan");
    }

    let report = service
        .cleanup_orphans_after_restore()
        .await
        .expect("restore cleanup");

    assert_eq!(report.orphans_deleted, objects.len() as u64);
    assert_eq!(report.stale_claims_finalized, 0);
    assert!(
        database
            .get_orphan_content_bodies(chrono::DateTime::<Utc>::from_timestamp_nanos(i64::MAX), 1,)
            .await
            .expect("remaining orphans")
            .is_empty()
    );
    for object in [objects.first().unwrap(), objects.last().unwrap()] {
        assert!(
            !storage
                .exists(&project_id, &object.body_hash)
                .await
                .expect("orphan bytes lookup")
        );
    }
}
