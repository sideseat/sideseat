//! Where a file's bytes live: the blob store, as a port.
//!
//! **A port, not a service.** `FileStorage` was a trait in the data layer, so the domain reached into an adapter
//! module to name the abstraction it depends on - and `FileStorageError` with it. Neither has any dependency of
//! its own, which is what made this a move rather than a redesign: the trait was already the tightest seam in the
//! codebase, and the plan calls it the model for the rest.
//!
//! The filesystem and S3 implementations stay where they are. What moved is the statement of what they must do.

use async_trait::async_trait;
use thiserror::Error;

use crate::traits::SurvivorReferences;
use crate::types::ProjectId;

/// Errors from low-level file storage operations (filesystem/S3)
#[derive(Error, Debug)]
pub enum FileStorageError {
    #[error("File not found: {project_id}/{hash}")]
    NotFound { project_id: String, hash: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Storage backend error: {0}")]
    Backend(String),

    /// Not attempted: this process does not hold its store lease, so it deletes nothing.
    #[error("Not attempted: this process does not hold its store lease")]
    Fenced,

    /// Given up at the store lease's deadline: whether the store applied it is unknown, so whatever it guarded -
    /// a deletion claim - stays in place.
    #[error("Abandoned at the store lease's deadline; whether it was applied is unknown")]
    Abandoned,

    /// Refused: the bytes to be stored are not the content they were written as - a temporary file truncated or
    /// overwritten after its writer wrote it.
    #[error("Refused to store {hash}: {detail}")]
    Corrupt { hash: String, detail: String },
}

/// What a temporary file must hold to be stored: the length and BLAKE3 digest of the bytes its writer wrote.
///
/// An object's name is the hash of the content as it arrived (base64 text, which decodes the same from several
/// spellings), so stored bytes cannot be checked against their name. They are checked against this instead,
/// computed from the decoded bytes when the temporary file was written: a file another execution truncated or
/// overwrote in between is refused, never stored under the name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpectedContent {
    pub len: u64,
    pub digest: [u8; 32],
}

impl ExpectedContent {
    pub fn digest_hex(&self) -> String {
        hex::encode(self.digest)
    }
}

/// File content with metadata
#[derive(Debug)]
pub struct FileContent {
    /// Raw file bytes
    pub data: Vec<u8>,
    /// MIME type if known
    pub media_type: Option<String>,
}

/// The name one incarnation of a file's bytes is stored under: its content hash and its row's incarnation.
///
/// A file's row is created once per incarnation and takes a number its store never gives again (the row's id) -
/// a restore's repair moves the sequence past every incarnation still stored - and its bytes are stored under that
/// number as well as the hash. So a collector that claimed one incarnation and
/// paused - its claim taken over, the file collected, the same content ingested again under a new row - deletes
/// only the name it claimed when it wakes, never the bytes the new row names (`server/specs/MediaIncarnation.tla`).
/// An object no row reuses, a staged payload's, is stored under incarnation 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectName<'a> {
    pub hash: &'a str,
    pub incarnation: i64,
}

impl<'a> ObjectName<'a> {
    pub const fn new(hash: &'a str, incarnation: i64) -> Self {
        Self { hash, incarnation }
    }

    /// The object a stored name is, when it is exactly `{64 hex digits}.{decimal incarnation}` - not a temporary
    /// sibling, which has more after it.
    pub fn parse(name: &'a str) -> Option<Self> {
        let (hash, incarnation) = name.split_once('.')?;
        let is_hash = hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit());
        let is_number = !incarnation.is_empty() && incarnation.bytes().all(|b| b.is_ascii_digit());
        if !(is_hash && is_number) {
            return None;
        }
        Some(Self::new(hash, incarnation.parse().ok()?))
    }
}

impl std::fmt::Display for ObjectName<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.hash, self.incarnation)
    }
}

/// One stored object, as a scan of the whole store finds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredObject {
    pub project_id: ProjectId,
    pub hash: String,
    pub incarnation: i64,
}

impl StoredObject {
    pub fn name(&self) -> ObjectName<'_> {
        ObjectName::new(&self.hash, self.incarnation)
    }
}

/// What a scan of every stored object hands its batches to ([`FileStorage::scan_objects`]).
#[async_trait]
pub trait ObjectVisitor: Send {
    async fn visit(&mut self, objects: Vec<StoredObject>) -> Result<(), FileStorageError>;
}

/// Trait for file storage backends
///
/// All implementations must be thread-safe (Send + Sync) for use in async contexts. Files are organized per
/// project, each object under its [`ObjectName`].
#[async_trait]
pub trait FileStorage: Send + Sync {
    /// Store an object. Storing one already there is a no-op: an object's bytes are its hash's.
    async fn store(
        &self,
        project_id: &ProjectId,
        object: ObjectName<'_>,
        data: &[u8],
    ) -> Result<(), FileStorageError>;

    /// An object's bytes, or `NotFound`.
    async fn get(
        &self,
        project_id: &ProjectId,
        object: ObjectName<'_>,
    ) -> Result<Vec<u8>, FileStorageError>;

    /// Whether an object is stored.
    async fn exists(
        &self,
        project_id: &ProjectId,
        object: ObjectName<'_>,
    ) -> Result<bool, FileStorageError>;

    /// Delete an object; one that is not there is not an error. Other incarnations of the same hash are untouched.
    async fn delete(
        &self,
        project_id: &ProjectId,
        object: ObjectName<'_>,
    ) -> Result<(), FileStorageError>;

    /// The incarnations stored for `hash`, ascending: how a restore repair finds the bytes of a file whose row it
    /// lost, under whichever incarnation they were written.
    async fn incarnations(
        &self,
        project_id: &ProjectId,
        hash: &str,
    ) -> Result<Vec<i64>, FileStorageError>;

    /// Hand every stored object of every project to `visitor`, a batch at a time - a directory's or a listing
    /// page's - so the walk holds one batch, not the store. Temporary siblings are not objects.
    async fn scan_objects(&self, visitor: &mut dyn ObjectVisitor) -> Result<(), FileStorageError>;

    /// The highest incarnation any object of any project is named with, 0 when there is none.
    ///
    /// A restore takes the transactional store's incarnation sequence back to the backup's, while the blob store
    /// can hold objects named by incarnations issued after it; the repair moves the sequence past this, so no row
    /// created afterwards is named like an object of the lost timeline.
    async fn max_incarnation(&self) -> Result<i64, FileStorageError> {
        struct Highest(i64);
        #[async_trait]
        impl ObjectVisitor for Highest {
            async fn visit(&mut self, objects: Vec<StoredObject>) -> Result<(), FileStorageError> {
                self.0 = objects
                    .iter()
                    .map(|object| object.incarnation)
                    .fold(self.0, i64::max);
                Ok(())
            }
        }
        let mut highest = Highest(0);
        self.scan_objects(&mut highest).await?;
        Ok(highest.0)
    }

    /// Delete every object of a project, returning how many.
    async fn delete_project(&self, project_id: &ProjectId) -> Result<u64, FileStorageError>;

    /// Move a temporary file into place as `object`: a rename on a local filesystem, an upload and a delete of
    /// the temporary file on S3.
    ///
    /// The file is stored only if it holds `expected`, and is removed either way; otherwise
    /// [`FileStorageError::Corrupt`]. An object already stored under the name is kept only if it matches as far as
    /// the backend can tell without reading it back - its length, and on S3 the digest recorded when it was
    /// uploaded - and is replaced by the verified file otherwise.
    async fn finalize_temp(
        &self,
        project_id: &ProjectId,
        object: ObjectName<'_>,
        temp_path: &std::path::Path,
        expected: ExpectedContent,
    ) -> Result<(), FileStorageError>;
}

/// File-side retention work required by an analytics adapter after span expiry.
#[async_trait]
pub trait RetentionFileReconciler: Send + Sync {
    fn is_enabled(&self) -> bool;

    /// Release the associations the traces' surviving spans no longer reference, returning the traces it
    /// **deferred** - left holding an association it could neither keep nor release, as a writer was pending on
    /// it or a confirm since the release's horizon stamped it, or whose leftover could not be read - whose cleanup
    /// the caller must keep owed rather than complete.
    async fn reconcile_trace_survivors(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
        analytics: &dyn SurvivorReferences,
    ) -> Result<Vec<String>, String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stored_name_parses_only_as_exactly_a_hash_and_an_incarnation() {
        let hash = "a".repeat(64);
        let name = format!("{hash}.42");
        assert_eq!(ObjectName::parse(&name), Some(ObjectName::new(&hash, 42)));
        assert_eq!(
            ObjectName::parse(&format!("{hash}.0")),
            Some(ObjectName::new(&hash, 0))
        );
        for other in [
            format!("{hash}.42.1234.5f0c.tmp"),
            format!("{hash}."),
            hash.clone(),
            format!("{}.42", "a".repeat(63)),
            format!("{}.42", "g".repeat(64)),
            format!("{hash}.-1"),
            format!("{hash}.99999999999999999999"),
        ] {
            assert_eq!(ObjectName::parse(&other), None, "{other}");
        }
    }

    #[test]
    fn test_file_content_debug() {
        let content = FileContent {
            data: vec![1, 2, 3],
            media_type: Some("image/png".to_string()),
        };
        let debug = format!("{:?}", content);
        assert!(debug.contains("FileContent"));
        assert!(debug.contains("image/png"));
    }
}
