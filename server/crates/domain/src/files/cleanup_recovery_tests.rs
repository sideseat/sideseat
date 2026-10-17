//! Startup recovery of temporary files keeps every file that may be the last copy of media an export still needs.

use super::*;

/// A store whose first finalisation fails after taking the file it was handed - a rename, then a failed sync - and
/// that delegates the rest.
struct FirstFinalizeFails {
    inner: Arc<dyn FileStorage>,
    failed: std::sync::atomic::AtomicBool,
    /// Instead of failing, truncate the file it was handed and let the store judge it - what a second recovery
    /// sharing the directory did to a shared copy.
    truncate: bool,
}

#[async_trait::async_trait]
impl FileStorage for FirstFinalizeFails {
    async fn store(
        &self,
        project_id: &ProjectId,
        object: ObjectName<'_>,
        data: &[u8],
    ) -> Result<(), FileStorageError> {
        self.inner.store(project_id, object, data).await
    }
    async fn get(
        &self,
        project_id: &ProjectId,
        object: ObjectName<'_>,
    ) -> Result<Vec<u8>, FileStorageError> {
        self.inner.get(project_id, object).await
    }
    async fn exists(
        &self,
        project_id: &ProjectId,
        object: ObjectName<'_>,
    ) -> Result<bool, FileStorageError> {
        self.inner.exists(project_id, object).await
    }
    async fn delete(
        &self,
        project_id: &ProjectId,
        object: ObjectName<'_>,
    ) -> Result<(), FileStorageError> {
        self.inner.delete(project_id, object).await
    }
    async fn incarnations(
        &self,
        project_id: &ProjectId,
        hash: &str,
    ) -> Result<Vec<i64>, FileStorageError> {
        self.inner.incarnations(project_id, hash).await
    }
    async fn scan_objects(
        &self,
        visitor: &mut dyn sideseat_ports::blobs::ObjectVisitor,
    ) -> Result<(), FileStorageError> {
        self.inner.scan_objects(visitor).await
    }
    async fn delete_project(&self, project_id: &ProjectId) -> Result<u64, FileStorageError> {
        self.inner.delete_project(project_id).await
    }
    async fn finalize_temp(
        &self,
        project_id: &ProjectId,
        object: ObjectName<'_>,
        temp_path: &Path,
        expected: ExpectedContent,
    ) -> Result<(), FileStorageError> {
        if !self.failed.swap(true, std::sync::atomic::Ordering::SeqCst) {
            if self.truncate {
                tokio::fs::write(temp_path, b"test").await.unwrap();
            } else {
                tokio::fs::remove_file(temp_path).await.ok();
                return Err(FileStorageError::Io(std::io::Error::other("sync failed")));
            }
        }
        self.inner
            .finalize_temp(project_id, object, temp_path, expected)
            .await
    }
}

/// A recovery whose finalisation fails - even after the store took the file it was given - keeps the temporary
/// file, the only copy of bytes it may still need, and a later recovery stores it.
#[tokio::test]
async fn a_recovery_the_store_refused_keeps_its_temp_file_for_the_next() {
    let (temp_dir, database, inner) = setup_test().await;
    let storage: Arc<dyn FileStorage> = Arc::new(FirstFinalizeFails {
        inner,
        failed: std::sync::atomic::AtomicBool::new(false),
        truncate: false,
    });
    let temp_path = temp_dir.path().join("temp");
    fs::create_dir_all(&temp_path).await.unwrap();
    let hash = test_hash();
    let file_path = temp_path.join(temp_file_name(
        "project1",
        &hash,
        expected(b"test content"),
        "n1",
    ));
    fs::write(&file_path, b"test content").await.unwrap();
    database
        .as_ref()
        .upsert_file(
            &ProjectId::from("project1"),
            &hash,
            Some("text/plain"),
            12,
            "sha256",
        )
        .await
        .unwrap();

    let first = cleanup_orphan_temp_files(&temp_path, &storage, &database)
        .await
        .unwrap();
    assert_eq!(first.finalize_failed, 1);
    assert!(
        file_path.exists(),
        "the temporary file survives the failure"
    );

    let second = cleanup_orphan_temp_files(&temp_path, &storage, &database)
        .await
        .unwrap();
    assert_eq!(second.finalized, 1);
    assert!(!file_path.exists());
    assert_eq!(
        storage
            .get(&ProjectId::from("project1"), at(&hash))
            .await
            .unwrap(),
        b"test content"
    );
}

/// A recovery that cannot read a temporary file's row keeps the file: a failed lookup is not an absent row, and the
/// file may be the last copy of media an export still needs.
#[tokio::test]
async fn a_recovery_that_cannot_read_the_row_keeps_the_temp_file() {
    let temp_dir = TempDir::new().unwrap();
    let app_storage = AppStorage::init_for_test(temp_dir.path().to_path_buf());
    let sqlite = Arc::new(
        SqliteService::init(&app_storage, Arc::new(TestClock))
            .await
            .unwrap(),
    );
    let database: Arc<dyn TransactionalRepository + Send + Sync> =
        Arc::new(SqliteRepository(Arc::clone(&sqlite)));
    let storage_path = temp_dir.path().join("files");
    fs::create_dir_all(&storage_path).await.unwrap();
    let storage: Arc<dyn FileStorage> = Arc::new(FilesystemStorage::new(storage_path));
    let temp_path = temp_dir.path().join("temp");
    fs::create_dir_all(&temp_path).await.unwrap();
    let hash = test_hash();
    let file_path = temp_path.join(temp_file_name(
        "project1",
        &hash,
        expected(b"test content"),
        "n1",
    ));
    fs::write(&file_path, b"test content").await.unwrap();
    database
        .upsert_file(
            &ProjectId::from("project1"),
            &hash,
            Some("text/plain"),
            12,
            "sha256",
        )
        .await
        .unwrap();

    sqlite.close().await;
    let unreadable = cleanup_orphan_temp_files(&temp_path, &storage, &database)
        .await
        .unwrap();
    assert_eq!(unreadable.orphaned_deleted, 0);
    assert!(file_path.exists(), "the file outlives a lookup that failed");

    let reopened: Arc<dyn TransactionalRepository + Send + Sync> =
        Arc::new(SqliteRepository(Arc::new(
            SqliteService::init(&app_storage, Arc::new(TestClock))
                .await
                .unwrap(),
        )));
    let recovered = cleanup_orphan_temp_files(&temp_path, &storage, &reopened)
        .await
        .unwrap();
    assert_eq!(recovered.finalized, 1);
    assert!(!file_path.exists());
    assert_eq!(
        storage
            .get(&ProjectId::from("project1"), at(&hash))
            .await
            .unwrap(),
        b"test content"
    );
}

/// A recovery whose copy is damaged under it - by a second recovery of the same directory - does not delete the
/// file it copied, which is still the content: it is kept, and stored by the next pass.
#[tokio::test]
async fn a_recovery_whose_copy_is_damaged_keeps_the_intact_file() {
    let (temp_dir, database, inner) = setup_test().await;
    let storage: Arc<dyn FileStorage> = Arc::new(FirstFinalizeFails {
        inner,
        failed: std::sync::atomic::AtomicBool::new(false),
        truncate: true,
    });
    let temp_path = temp_dir.path().join("temp");
    fs::create_dir_all(&temp_path).await.unwrap();
    let hash = test_hash();
    let file_path = temp_path.join(temp_file_name(
        "project1",
        &hash,
        expected(b"test content"),
        "n1",
    ));
    fs::write(&file_path, b"test content").await.unwrap();
    database
        .as_ref()
        .upsert_file(
            &ProjectId::from("project1"),
            &hash,
            Some("text/plain"),
            12,
            "sha256",
        )
        .await
        .unwrap();

    let first = cleanup_orphan_temp_files(&temp_path, &storage, &database)
        .await
        .unwrap();
    assert_eq!(first.invalid_deleted, 0);
    assert!(
        file_path.exists(),
        "the intact file survives its damaged copy"
    );
    assert_eq!(fs::read(&file_path).await.unwrap(), b"test content");

    let second = cleanup_orphan_temp_files(&temp_path, &storage, &database)
        .await
        .unwrap();
    assert_eq!(second.finalized, 1);
    assert_eq!(
        storage
            .get(&ProjectId::from("project1"), at(&hash))
            .await
            .unwrap(),
        b"test content"
    );
}
