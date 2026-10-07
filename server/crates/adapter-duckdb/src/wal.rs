//! Durability of DuckDB's write-ahead log as a file, not only as bytes - see [`WalDirectory`].

use duckdb::Connection;

/// Makes a newly created write-ahead log's directory entry durable.
///
/// DuckDB removes its WAL at every checkpoint and creates it again on the next commit, and it never syncs the
/// directory - so a commit synced into a fresh WAL can still be lost to a power failure, because the file
/// holding it may not exist afterwards. POSIX makes a new file's existence durable only by syncing its
/// directory, and no DuckDB setting keeps the WAL in place. So every write ends by comparing the WAL with the
/// one last made durable, and a different file - by inode - has its directory synced before the write reports
/// success ([`DuckdbService::write`]).
///
/// The comparison runs under the connection's lock, and this service has one connection: the only checkpoint
/// that can remove or recreate the WAL is one this same connection runs, inside a call the lock already
/// serialises. A checkpoint that ran inside the write itself has moved the write's data into the main file,
/// which DuckDB syncs as part of the checkpoint, so whatever WAL remains is the right one to judge.
pub(crate) struct WalDirectory {
    wal_path: std::path::PathBuf,
    directory: std::path::PathBuf,
    /// Inode of the WAL whose directory entry was last synced; 0 before any.
    synced_inode: std::sync::atomic::AtomicU64,
}

impl WalDirectory {
    pub(crate) fn new(db_path: &std::path::Path) -> Self {
        let mut wal_path = db_path.as_os_str().to_owned();
        wal_path.push(".wal");
        Self {
            wal_path: wal_path.into(),
            directory: db_path.parent().map_or_else(
                || std::path::PathBuf::from("."),
                std::path::Path::to_path_buf,
            ),
            synced_inode: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Sync the directory if the WAL is a file it has not yet made durable. A missing WAL needs nothing: its
    /// data is in the main file, synced by the checkpoint that removed it.
    pub(crate) fn sync_if_new(&self) -> std::io::Result<()> {
        use std::sync::atomic::Ordering;
        let Some(inode) = wal_inode(&self.wal_path) else {
            return Ok(());
        };
        if self.synced_inode.load(Ordering::Acquire) == inode {
            return Ok(());
        }
        // `sync_all` is F_FULLFSYNC on Apple platforms and fsync elsewhere, as for every other durable write.
        std::fs::File::open(&self.directory)?.sync_all()?;
        self.synced_inode.store(inode, Ordering::Release);
        Ok(())
    }

    fn is_synced(&self) -> bool {
        wal_inode(&self.wal_path).is_none_or(|inode| {
            self.synced_inode.load(std::sync::atomic::Ordering::Acquire) == inode
        })
    }
}

#[cfg(unix)]
fn wal_inode(path: &std::path::Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|metadata| metadata.ino())
}

/// Windows journals a file's creation with the file itself; there is no directory to sync.
#[cfg(not(unix))]
fn wal_inode(_path: &std::path::Path) -> Option<u64> {
    None
}

/// Exclusive access to the connection.
///
/// A write goes through [`DuckdbService::write`], which makes a new WAL's directory entry durable before it
/// reports success. Releasing the guard checks it again as a backstop, so a write that bypassed `write` is
/// caught in tests and still synced in production.
pub struct ConnectionGuard<'a> {
    pub(crate) connection: parking_lot::MappedMutexGuard<'a, Connection>,
    pub(crate) wal: &'a WalDirectory,
    /// Set by [`DuckdbService::write`], which has already synced and reported the outcome.
    pub(crate) checked: bool,
}

impl std::ops::Deref for ConnectionGuard<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        &self.connection
    }
}

impl std::ops::DerefMut for ConnectionGuard<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.connection
    }
}

impl Drop for ConnectionGuard<'_> {
    fn drop(&mut self) {
        if self.checked {
            return;
        }
        debug_assert!(
            std::thread::panicking() || self.wal.is_synced(),
            "a DuckDB write left a new WAL unsynced: write through DuckdbService::write"
        );
        if let Err(error) = self.wal.sync_if_new() {
            tracing::error!(
                directory = %self.wal.directory.display(),
                %error,
                "Could not sync the directory of DuckDB's new write-ahead log"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DuckdbService;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    /// A WAL recreated after a checkpoint is a new file, and its directory is synced again.
    #[cfg(unix)]
    #[test]
    fn a_recreated_wal_is_synced_again() {
        let root = tempfile::TempDir::new().expect("temp dir");
        let db = root.path().join("sideseat.duckdb");
        let wal = WalDirectory::new(&db);
        wal.sync_if_new().expect("sync");
        assert_eq!(
            wal.synced_inode.load(Ordering::Acquire),
            0,
            "no WAL, nothing to sync"
        );

        std::fs::write(root.path().join("sideseat.duckdb.wal"), b"first").expect("wal");
        wal.sync_if_new().expect("sync");
        let first = wal.synced_inode.load(Ordering::Acquire);
        assert_eq!(
            Some(first),
            wal_inode(&root.path().join("sideseat.duckdb.wal"))
        );

        // A checkpoint removes the WAL and the next commit creates another. Holding the old file open keeps
        // the filesystem from handing its inode straight back.
        let _old = std::fs::File::open(root.path().join("sideseat.duckdb.wal")).expect("old wal");
        std::fs::remove_file(root.path().join("sideseat.duckdb.wal")).expect("checkpoint");
        std::fs::write(root.path().join("sideseat.duckdb.wal"), b"second").expect("new wal");
        wal.sync_if_new().expect("sync");
        let second = wal.synced_inode.load(Ordering::Acquire);
        assert_ne!(
            second, first,
            "the new WAL's directory entry must be synced"
        );
        assert_eq!(
            Some(second),
            wal_inode(&root.path().join("sideseat.duckdb.wal"))
        );
    }

    /// A write through `write` leaves the new WAL's directory entry synced before it returns.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_write_syncs_a_new_wal_before_it_returns() {
        let root = tempfile::TempDir::new().expect("temp dir");
        tokio::fs::create_dir_all(root.path().join("duckdb"))
            .await
            .expect("dir");
        let storage = sideseat_core::storage::AppStorage::init_for_test(root.path().to_path_buf());
        let service = DuckdbService::init(&storage, Arc::new(crate::TestClock))
            .await
            .expect("duckdb");
        service
            .write(|conn| {
                conn.execute_batch(
                    "CHECKPOINT; CREATE TABLE probe(x INTEGER); INSERT INTO probe VALUES (1);",
                )
                .map_err(Into::into)
            })
            .unwrap_or_else(|error| panic!("write: {error}"));
        let wal = wal_inode(&service.wal.wal_path);
        assert!(wal.is_some(), "the write left a WAL");
        assert_eq!(Some(service.wal.synced_inode.load(Ordering::Acquire)), wal);
    }
}
