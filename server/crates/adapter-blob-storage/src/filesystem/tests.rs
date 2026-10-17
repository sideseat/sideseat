use super::*;
use sideseat_ports::blobs::{ExpectedContent, ObjectName};
use tempfile::TempDir;

/// What `bytes` are, as a writer that wrote them would expect them.
fn expected(bytes: &[u8]) -> ExpectedContent {
    ExpectedContent {
        len: bytes.len() as u64,
        digest: *blake3::hash(bytes).as_bytes(),
    }
}

fn test_hash() -> &'static str {
    "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2"
}

#[tokio::test]
async fn test_store_and_get() {
    let temp_dir = TempDir::new().unwrap();
    let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());

    let data = b"test content";
    storage
        .store(
            &ProjectId::from("project1"),
            ObjectName::new(test_hash(), 1),
            data,
        )
        .await
        .unwrap();

    let retrieved = storage
        .get(
            &ProjectId::from("project1"),
            ObjectName::new(test_hash(), 1),
        )
        .await
        .unwrap();
    assert_eq!(retrieved, data);
}

/// Publication goes through a temporary sibling; none may be left behind, and two writers racing on one
/// content-addressed hash must both succeed with one complete object.
#[tokio::test]
async fn store_publishes_atomically_and_leaves_no_temporary_files() {
    let temp_dir = TempDir::new().unwrap();
    let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());
    let project = ProjectId::from("project1");
    let data = vec![7u8; 256 * 1024];
    let (a, b) = tokio::join!(
        storage.store(&project, ObjectName::new(test_hash(), 1), &data),
        storage.store(&project, ObjectName::new(test_hash(), 1), &data)
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(
        storage
            .get(&project, ObjectName::new(test_hash(), 1))
            .await
            .unwrap(),
        data
    );
    let dir = storage
        .file_path(&project, ObjectName::new(test_hash(), 1))
        .parent()
        .unwrap()
        .to_path_buf();
    let names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec![format!("{}.1", test_hash())]);
}

/// A reader that sees the object must see all of it: the existence check is what content addressing
/// trusts, so a partially written object at the final path would be accepted as complete.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_object_is_never_visible_partially_written() {
    let temp_dir = TempDir::new().unwrap();
    let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());
    let project = ProjectId::from("project1");
    let data = vec![9u8; 64 * 1024 * 1024];
    let path = storage.file_path(&project, ObjectName::new(test_hash(), 1));
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // A plain thread polling the final path as fast as it can: every size it observes must be complete.
    let poller = {
        let stop = stop.clone();
        let expected = data.len() as u64;
        std::thread::spawn(move || {
            let mut partial = 0u32;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                if std::fs::metadata(&path).is_ok_and(|meta| meta.len() != expected) {
                    partial += 1;
                }
            }
            partial
        })
    };
    storage
        .store(&project, ObjectName::new(test_hash(), 1), &data)
        .await
        .unwrap();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(
        poller.join().unwrap(),
        0,
        "a reader saw a partially written object"
    );
}

#[tokio::test]
async fn test_content_addressed_dedup() {
    let temp_dir = TempDir::new().unwrap();
    let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());

    let data = b"test content";

    // Store twice with same hash
    storage
        .store(
            &ProjectId::from("project1"),
            ObjectName::new(test_hash(), 1),
            data,
        )
        .await
        .unwrap();
    storage
        .store(
            &ProjectId::from("project1"),
            ObjectName::new(test_hash(), 1),
            data,
        )
        .await
        .unwrap();

    // Should still work
    let retrieved = storage
        .get(
            &ProjectId::from("project1"),
            ObjectName::new(test_hash(), 1),
        )
        .await
        .unwrap();
    assert_eq!(retrieved, data);
}

#[tokio::test]
async fn test_exists() {
    let temp_dir = TempDir::new().unwrap();
    let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());

    assert!(
        !storage
            .exists(
                &ProjectId::from("project1"),
                ObjectName::new(test_hash(), 1)
            )
            .await
            .unwrap()
    );

    storage
        .store(
            &ProjectId::from("project1"),
            ObjectName::new(test_hash(), 1),
            b"data",
        )
        .await
        .unwrap();

    assert!(
        storage
            .exists(
                &ProjectId::from("project1"),
                ObjectName::new(test_hash(), 1)
            )
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn test_delete() {
    let temp_dir = TempDir::new().unwrap();
    let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());

    storage
        .store(
            &ProjectId::from("project1"),
            ObjectName::new(test_hash(), 1),
            b"data",
        )
        .await
        .unwrap();
    assert!(
        storage
            .exists(
                &ProjectId::from("project1"),
                ObjectName::new(test_hash(), 1)
            )
            .await
            .unwrap()
    );

    storage
        .delete(
            &ProjectId::from("project1"),
            ObjectName::new(test_hash(), 1),
        )
        .await
        .unwrap();
    assert!(
        !storage
            .exists(
                &ProjectId::from("project1"),
                ObjectName::new(test_hash(), 1)
            )
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn test_delete_nonexistent() {
    let temp_dir = TempDir::new().unwrap();
    let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());

    // Should not fail
    storage
        .delete(
            &ProjectId::from("project1"),
            ObjectName::new(test_hash(), 1),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn test_get_not_found() {
    let temp_dir = TempDir::new().unwrap();
    let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());

    let result = storage
        .get(
            &ProjectId::from("project1"),
            ObjectName::new(test_hash(), 1),
        )
        .await;
    assert!(matches!(result, Err(FileStorageError::NotFound { .. })));
}

#[tokio::test]
async fn test_invalid_hash() {
    let temp_dir = TempDir::new().unwrap();
    let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());

    let result = storage
        .store(
            &ProjectId::from("project1"),
            ObjectName::new("invalid", 1),
            b"data",
        )
        .await;
    assert!(matches!(result, Err(FileStorageError::Backend(_))));
}

#[tokio::test]
async fn test_file_path_sharding() {
    let storage = FilesystemStorage::new(PathBuf::from("/base"));
    let path = storage.file_path(
        &ProjectId::from("project1"),
        ObjectName::new(test_hash(), 1),
    );

    // Sharded by the hash, named by the hash and the incarnation: /base/project1/a1/b2/{hash}.1
    assert!(path.to_string_lossy().contains("/project1/a1/b2/"));
    assert!(
        path.to_string_lossy()
            .ends_with(&format!("{}.1", test_hash()))
    );
}

#[tokio::test]
async fn test_delete_project() {
    let temp_dir = TempDir::new().unwrap();
    let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());

    // Store multiple files
    let hash1 = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
    let hash2 = "b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3";

    storage
        .store(
            &ProjectId::from("project1"),
            ObjectName::new(hash1, 1),
            b"data1",
        )
        .await
        .unwrap();
    storage
        .store(
            &ProjectId::from("project1"),
            ObjectName::new(hash2, 1),
            b"data2",
        )
        .await
        .unwrap();

    let deleted = storage
        .delete_project(&ProjectId::from("project1"))
        .await
        .unwrap();
    assert_eq!(deleted, 2);

    // Verify files are gone
    assert!(
        !storage
            .exists(&ProjectId::from("project1"), ObjectName::new(hash1, 1))
            .await
            .unwrap()
    );
    assert!(
        !storage
            .exists(&ProjectId::from("project1"), ObjectName::new(hash2, 1))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn test_finalize_temp() {
    let temp_dir = TempDir::new().unwrap();
    let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());

    // Create a temp file
    let temp_file = temp_dir.path().join("temp_file");
    fs::write(&temp_file, b"temp content").await.unwrap();

    // Finalize it
    storage
        .finalize_temp(
            &ProjectId::from("project1"),
            ObjectName::new(test_hash(), 1),
            &temp_file,
            expected(b"temp content"),
        )
        .await
        .unwrap();

    // Temp file should be gone
    assert!(!temp_file.exists());

    // File should be in permanent storage
    let data = storage
        .get(
            &ProjectId::from("project1"),
            ObjectName::new(test_hash(), 1),
        )
        .await
        .unwrap();
    assert_eq!(data, b"temp content");
}

#[tokio::test]
async fn test_project_isolation() {
    let temp_dir = TempDir::new().unwrap();
    let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());

    // Store same hash in different projects
    storage
        .store(
            &ProjectId::from("project1"),
            ObjectName::new(test_hash(), 1),
            b"data1",
        )
        .await
        .unwrap();
    storage
        .store(
            &ProjectId::from("project2"),
            ObjectName::new(test_hash(), 1),
            b"data2",
        )
        .await
        .unwrap();

    // Each project should have its own copy
    let data1 = storage
        .get(
            &ProjectId::from("project1"),
            ObjectName::new(test_hash(), 1),
        )
        .await
        .unwrap();
    let data2 = storage
        .get(
            &ProjectId::from("project2"),
            ObjectName::new(test_hash(), 1),
        )
        .await
        .unwrap();

    assert_eq!(data1, b"data1");
    assert_eq!(data2, b"data2");
}

/// The directories synced under `root` since the test began, in order.
fn synced_under(root: &Path, from: usize) -> Vec<PathBuf> {
    SYNCED_DIRECTORIES.lock().unwrap()[from..]
        .iter()
        .filter(|dir| dir.starts_with(root))
        .cloned()
        .collect()
}

fn synced_so_far() -> usize {
    SYNCED_DIRECTORIES.lock().unwrap().len()
}

/// A project directory another writer has created but not yet made durable is synced into its holder by a
/// store beneath it, before the store returns: trusted because it existed, a power loss could take it and
/// the acknowledged object with it.
#[tokio::test]
async fn a_directory_another_writer_created_is_made_durable_before_the_store_returns() {
    let temp_dir = TempDir::new().unwrap();
    let base = temp_dir.path().canonicalize().unwrap();
    let storage = FilesystemStorage::new(base.clone());
    // Another writer's directory, created and not synced.
    std::fs::create_dir_all(base.join("project1")).unwrap();
    let from = synced_so_far();
    storage
        .store(
            &ProjectId::from("project1"),
            ObjectName::new(test_hash(), 1),
            b"bytes",
        )
        .await
        .unwrap();
    let synced = synced_under(&base, from);
    assert!(
        synced.contains(&base),
        "the project directory's entry was never synced into the base: {synced:?}"
    );
    let shard = base.join("project1").join("a1");
    assert!(synced.contains(&base.join("project1")), "{synced:?}");
    assert!(synced.contains(&shard), "{synced:?}");
    assert!(
        synced.contains(&shard.join("b2")),
        "the object's own entry: {synced:?}"
    );

    // Once durable, a directory is not synced again for the next object beneath it.
    let from = synced_so_far();
    let other = "a1b2ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
    storage
        .store(
            &ProjectId::from("project1"),
            ObjectName::new(other, 1),
            b"more",
        )
        .await
        .unwrap();
    assert_eq!(synced_under(&base, from), vec![shard.join("b2")]);
}

/// An object already in place - another writer's rename whose directory sync has not returned - is made
/// durable before a store of the same content answers.
#[tokio::test]
async fn an_object_another_writer_renamed_is_made_durable_before_the_store_returns() {
    let temp_dir = TempDir::new().unwrap();
    let base = temp_dir.path().canonicalize().unwrap();
    let storage = FilesystemStorage::new(base.clone());
    let directory = base.join("project1").join("a1").join("b2");
    std::fs::create_dir_all(&directory).unwrap();
    // Under the very name the store asks for, and with bytes of its own: the store takes the existing-object
    // path, which leaves them, rather than publishing its own, which would sync the directory anyway.
    let existing = directory.join(format!("{}.1", test_hash()));
    std::fs::write(&existing, b"theirs").unwrap();
    let from = synced_so_far();
    storage
        .store(
            &ProjectId::from("project1"),
            ObjectName::new(test_hash(), 1),
            b"bytes",
        )
        .await
        .unwrap();
    assert_eq!(std::fs::read(&existing).unwrap(), b"theirs");
    assert!(
        synced_under(&base, from).contains(&directory),
        "the existing object's entry was not synced"
    );
}

/// A directory remembered as durable and then removed outside a delete - the tree gone under the store - is
/// created again, not trusted into a failure on every later store.
#[tokio::test]
async fn a_directory_removed_under_the_store_is_created_again() {
    let temp_dir = TempDir::new().unwrap();
    let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());
    let project = ProjectId::from("project1");
    storage
        .store(&project, ObjectName::new(test_hash(), 1), b"first")
        .await
        .unwrap();
    std::fs::remove_dir_all(temp_dir.path().join("project1")).unwrap();
    let other = "a1b2ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
    storage
        .store(&project, ObjectName::new(other, 1), b"second")
        .await
        .unwrap();
    assert_eq!(
        storage
            .get(&project, ObjectName::new(other, 1))
            .await
            .unwrap(),
        b"second"
    );
}

/// Two incarnations of one hash are two objects: deleting one leaves the other, and both are listed until then.
#[tokio::test]
async fn incarnations_of_one_hash_are_separate_objects() {
    let dir = tempfile::TempDir::new().unwrap();
    let storage = FilesystemStorage::new(dir.path().to_path_buf());
    let project = ProjectId::from("project1");
    storage
        .store(&project, ObjectName::new(test_hash(), 3), b"old")
        .await
        .unwrap();
    storage
        .store(&project, ObjectName::new(test_hash(), 11), b"new")
        .await
        .unwrap();
    assert_eq!(
        storage.incarnations(&project, test_hash()).await.unwrap(),
        [3, 11]
    );
    storage
        .delete(&project, ObjectName::new(test_hash(), 3))
        .await
        .unwrap();
    assert_eq!(
        storage
            .get(&project, ObjectName::new(test_hash(), 11))
            .await
            .unwrap(),
        b"new"
    );
    assert_eq!(
        storage.incarnations(&project, test_hash()).await.unwrap(),
        [11]
    );
}

/// A shard another store emptied - its last object deleted, and the directory with it - is made again when a
/// temporary file is finalised into it, however durable this store remembered it to be.
#[tokio::test]
async fn a_temp_finalised_into_a_shard_another_store_emptied_is_published() {
    let dir = TempDir::new().unwrap();
    let storage = FilesystemStorage::new(dir.path().to_path_buf());
    let other_store = FilesystemStorage::new(dir.path().to_path_buf());
    let project = ProjectId::from("project1");
    storage
        .store(&project, ObjectName::new(test_hash(), 1), b"first")
        .await
        .unwrap();
    other_store
        .delete(&project, ObjectName::new(test_hash(), 1))
        .await
        .unwrap();
    let temp = dir.path().join("upload.tmp");
    std::fs::write(&temp, b"second").unwrap();
    storage
        .finalize_temp(
            &project,
            ObjectName::new(test_hash(), 2),
            &temp,
            expected(b"second"),
        )
        .await
        .unwrap();
    assert_eq!(
        storage
            .get(&project, ObjectName::new(test_hash(), 2))
            .await
            .unwrap(),
        b"second"
    );
}

/// The highest incarnation of every project, found through a project directory that is a symlink as reads find
/// objects through it: a restore's sequence floor that missed them would issue their names again.
#[cfg(unix)]
#[tokio::test]
async fn the_highest_incarnation_is_found_through_every_project_and_a_linked_one() {
    let dir = TempDir::new().unwrap();
    let elsewhere = TempDir::new().unwrap();
    let storage = FilesystemStorage::new(dir.path().to_path_buf());
    assert_eq!(
        storage.max_incarnation().await.unwrap(),
        0,
        "nothing stored"
    );
    storage
        .store(
            &ProjectId::from("one"),
            ObjectName::new(test_hash(), 4),
            b"a",
        )
        .await
        .unwrap();
    let linked = FilesystemStorage::new(elsewhere.path().to_path_buf());
    linked
        .store(
            &ProjectId::from("two"),
            ObjectName::new(test_hash(), 9),
            b"b",
        )
        .await
        .unwrap();
    std::os::unix::fs::symlink(elsewhere.path().join("two"), dir.path().join("two")).unwrap();
    std::fs::write(
        dir.path()
            .join("one")
            .join(&test_hash()[0..2])
            .join(&test_hash()[2..4])
            .join(format!("{}.77.123.tmp", test_hash())),
        b"partial",
    )
    .unwrap();
    assert_eq!(
        storage
            .get(&ProjectId::from("two"), ObjectName::new(test_hash(), 9))
            .await
            .unwrap(),
        b"b"
    );
    assert_eq!(
        storage.max_incarnation().await.unwrap(),
        9,
        "a temporary sibling is no object"
    );
}

/// A temporary file that is not what its writer wrote - truncated by a racing write of the same media - is refused
/// and removed, and nothing is stored under the name.
#[tokio::test]
async fn a_temporary_file_that_is_not_its_content_is_refused() {
    let dir = TempDir::new().unwrap();
    let storage = FilesystemStorage::new(dir.path().to_path_buf());
    let project = ProjectId::from("project1");
    let temp = dir.path().join("upload.tmp");
    std::fs::write(&temp, b"truncat").unwrap();
    let error = storage
        .finalize_temp(
            &project,
            ObjectName::new(test_hash(), 1),
            &temp,
            expected(b"truncated media"),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, FileStorageError::Corrupt { .. }), "{error}");
    assert!(!temp.exists(), "the refused temporary file is removed");
    assert!(
        !storage
            .exists(&project, ObjectName::new(test_hash(), 1))
            .await
            .unwrap()
    );

    // Same length, other bytes: refused by the digest.
    std::fs::write(&temp, b"truncated mediA").unwrap();
    let error = storage
        .finalize_temp(
            &project,
            ObjectName::new(test_hash(), 1),
            &temp,
            expected(b"truncated media"),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, FileStorageError::Corrupt { .. }), "{error}");
    assert!(
        !storage
            .exists(&project, ObjectName::new(test_hash(), 1))
            .await
            .unwrap()
    );
}

/// An object stored truncated - by an earlier finalisation of a racing write - is not trusted on its name: the next
/// finalisation of the content replaces it with the verified bytes.
#[tokio::test]
async fn a_stored_object_of_the_wrong_length_is_replaced() {
    let dir = TempDir::new().unwrap();
    let storage = FilesystemStorage::new(dir.path().to_path_buf());
    let project = ProjectId::from("project1");
    let name = ObjectName::new(test_hash(), 1);
    storage.store(&project, name, b"trunc").await.unwrap();
    let temp = dir.path().join("upload.tmp");
    std::fs::write(&temp, b"truncated media").unwrap();
    storage
        .finalize_temp(&project, name, &temp, expected(b"truncated media"))
        .await
        .unwrap();
    assert_eq!(
        storage.get(&project, name).await.unwrap(),
        b"truncated media"
    );
    assert!(!temp.exists());
}

/// An object of the right length that is not the content - a racing write left a gap at the full length - is not
/// trusted on its length either: it is hashed, and replaced.
#[tokio::test]
async fn a_stored_object_of_the_right_length_but_other_bytes_is_replaced() {
    let dir = TempDir::new().unwrap();
    let storage = FilesystemStorage::new(dir.path().to_path_buf());
    let project = ProjectId::from("project1");
    let name = ObjectName::new(test_hash(), 1);
    storage
        .store(&project, name, b"truncat\0\0\0\0\0\0\0\0")
        .await
        .unwrap();
    let temp = dir.path().join("upload.tmp");
    std::fs::write(&temp, b"truncated media").unwrap();
    storage
        .finalize_temp(&project, name, &temp, expected(b"truncated media"))
        .await
        .unwrap();
    assert_eq!(
        storage.get(&project, name).await.unwrap(),
        b"truncated media"
    );
    assert!(!temp.exists());
}
