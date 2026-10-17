//! Checking a temporary file against the content its writer wrote, before it is stored under a content name.

use std::path::Path;

use sideseat_ports::blobs::{ExpectedContent, FileStorageError};

/// Refuse `bytes` unless they are `expected`: the length first, then the BLAKE3 digest.
pub(crate) fn check(
    hash: &str,
    bytes: &[u8],
    expected: ExpectedContent,
) -> Result<(), FileStorageError> {
    if bytes.len() as u64 != expected.len {
        return Err(FileStorageError::Corrupt {
            hash: hash.to_string(),
            detail: format!(
                "the temporary file holds {} bytes where {} were written",
                bytes.len(),
                expected.len
            ),
        });
    }
    if *blake3::hash(bytes).as_bytes() != expected.digest {
        return Err(FileStorageError::Corrupt {
            hash: hash.to_string(),
            detail: "the temporary file's bytes are not the ones written".to_string(),
        });
    }
    Ok(())
}

/// [`check`] over a file, read in pieces on a blocking thread, so a large file is never held in memory.
pub(crate) async fn check_file(
    hash: &str,
    path: &Path,
    expected: ExpectedContent,
) -> Result<(), FileStorageError> {
    let path = path.to_path_buf();
    let (len, digest) = tokio::task::spawn_blocking(move || -> std::io::Result<(u64, [u8; 32])> {
        let mut file = std::fs::File::open(&path)?;
        let mut hasher = blake3::Hasher::new();
        let len = std::io::copy(&mut file, &mut hasher)?;
        Ok((len, *hasher.finalize().as_bytes()))
    })
    .await
    .map_err(|error| FileStorageError::Backend(error.to_string()))??;
    if len != expected.len {
        return Err(FileStorageError::Corrupt {
            hash: hash.to_string(),
            detail: format!(
                "the temporary file holds {len} bytes where {} were written",
                expected.len
            ),
        });
    }
    if digest != expected.digest {
        return Err(FileStorageError::Corrupt {
            hash: hash.to_string(),
            detail: "the temporary file's bytes are not the ones written".to_string(),
        });
    }
    Ok(())
}
