//! Platform-aware data storage directory management.
//!
//! ## Platform Paths
//!
//! | Type | Windows | macOS | Linux |
//! |------|---------|-------|-------|
//! | Data | `%APPDATA%\SideSeat\` | `~/Library/Application Support/SideSeat/` | `$XDG_DATA_HOME/sideseat/` |

use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use strum::{IntoStaticStr, VariantArray};
use thiserror::Error;

use super::config::AppConfig;
use super::constants::{APP_DOT_FOLDER, APP_NAME, ENV_DATA_DIR};
use crate::utils::file::expand_path;

/// Why the data directory layout could not be created.
///
/// The failing path is a field rather than only part of the message, because the one thing an operator
/// does with this is look at that path: it is usually a permission or a mount, and the directory
/// SideSeat chose is not always the one they expected.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("Failed to create {what} directory: {}", path.display())]
    CreateDirectory {
        /// `data`, or the subdirectory's stored name.
        what: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Data subdirectories
#[derive(Debug, Clone, Copy, PartialEq, Eq, IntoStaticStr, VariantArray)]
#[strum(serialize_all = "snake_case")]
pub enum DataSubdir {
    Sqlite,
    Duckdb,
    Debug,
    Files,
    FilesTemp,
}

impl DataSubdir {
    /// The directory's name on disk.
    pub fn as_str(&self) -> &'static str {
        self.into()
    }

    /// Returns subdirectories that should always be created.
    /// Debug is excluded - it's only created when debug mode is enabled.
    /// Files and FilesTemp are excluded - created when file storage is enabled.
    pub const fn all() -> &'static [DataSubdir] {
        &[DataSubdir::Sqlite, DataSubdir::Duckdb]
    }

    /// Returns subdirectories for file storage (created when enabled).
    pub const fn files() -> &'static [DataSubdir] {
        &[DataSubdir::Files, DataSubdir::FilesTemp]
    }
}

/// Application storage manager
#[derive(Debug, Clone)]
pub struct AppStorage {
    data_dir: PathBuf,
}

impl AppStorage {
    /// Initialize storage with platform-appropriate data directory
    pub async fn init(config: &AppConfig) -> Result<Self, StorageError> {
        let data_dir = Self::resolve_data_dir();

        // Create directories first (canonicalize requires path to exist)
        Self::ensure_directories_static(&data_dir, config.debug, config.files.enabled).await?;

        // Now canonicalize to get clean path for logging
        let data_dir = data_dir.canonicalize().unwrap_or(data_dir);

        tracing::debug!(data_dir = %data_dir.display(), "Storage initialized");

        if config.debug {
            let debug_path = data_dir.join(DataSubdir::Debug.as_str());
            tracing::warn!(path = %debug_path.display(), "Debug mode enabled");
        } else {
            tracing::debug!("Debug mode not enabled");
        }

        if config.files.enabled {
            let files_path = data_dir.join(DataSubdir::Files.as_str());
            tracing::debug!(path = %files_path.display(), "File storage enabled");
        }

        Ok(Self { data_dir })
    }

    /// Resolve the data directory from the environment or platform default.
    pub fn resolve_data_dir() -> PathBuf {
        let configured = std::env::var(ENV_DATA_DIR).ok();
        Self::resolve_data_dir_with_override(configured.as_deref())
    }

    fn resolve_data_dir_with_override(configured: Option<&str>) -> PathBuf {
        if let Some(dir) = configured {
            return expand_path(dir);
        }

        if let Some(proj_dirs) = ProjectDirs::from("", "", APP_NAME) {
            return proj_dirs.data_dir().to_path_buf();
        }

        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        cwd.join(APP_DOT_FOLDER)
    }

    /// Create data directory and subdirectories (static version for init)
    async fn ensure_directories_static(
        data_dir: &Path,
        debug: bool,
        files_enabled: bool,
    ) -> Result<(), StorageError> {
        create_dir("data", data_dir.to_path_buf()).await?;

        for subdir in DataSubdir::all() {
            create_dir(subdir.as_str(), data_dir.join(subdir.as_str())).await?;
        }

        // Debug mode only; the directory is evidence the mode was on.
        if debug {
            create_dir("debug", data_dir.join(DataSubdir::Debug.as_str())).await?;
        }

        if files_enabled {
            for subdir in DataSubdir::files() {
                create_dir(subdir.as_str(), data_dir.join(subdir.as_str())).await?;
            }
        }

        Ok(())
    }

    /// Get the data directory path
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Get path to a subdirectory (canonicalized)
    pub fn subdir(&self, subdir: DataSubdir) -> PathBuf {
        let path = self.data_dir.join(subdir.as_str());
        path.canonicalize().unwrap_or(path)
    }

    /// Get path to a file within the data directory
    pub fn data_path(&self, filename: &str) -> PathBuf {
        self.data_dir.join(filename)
    }

    /// Get path to a file within a subdirectory
    pub fn subdir_path(&self, subdir: DataSubdir, filename: &str) -> PathBuf {
        self.data_dir.join(subdir.as_str()).join(filename)
    }

    /// Construct test storage and create its baseline database directories.
    ///
    /// Panics when the fixture directory cannot be created.
    pub fn init_for_test(data_dir: PathBuf) -> Self {
        std::fs::create_dir_all(&data_dir).unwrap_or_else(|error| {
            panic!(
                "failed to create test data directory {}: {error}",
                data_dir.display()
            )
        });
        for subdir in DataSubdir::all() {
            let path = data_dir.join(subdir.as_str());
            std::fs::create_dir_all(&path).unwrap_or_else(|error| {
                panic!(
                    "failed to create test {} directory {}: {error}",
                    subdir.as_str(),
                    path.display()
                )
            });
        }
        Self { data_dir }
    }
}

async fn create_dir(what: &'static str, path: PathBuf) -> Result<(), StorageError> {
    tokio::fs::create_dir_all(&path)
        .await
        .map_err(|source| StorageError::CreateDirectory { what, path, source })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The directory names on disk, written out.
    ///
    /// A changed spelling does not fail to compile; it makes an existing installation's databases
    /// invisible and silently creates empty ones beside them. `VariantArray` makes a new variant without
    /// a name here fail instead.
    #[test]
    fn test_data_subdir_as_str() {
        let expected = [
            (DataSubdir::Sqlite, "sqlite"),
            (DataSubdir::Duckdb, "duckdb"),
            (DataSubdir::Debug, "debug"),
            (DataSubdir::Files, "files"),
            (DataSubdir::FilesTemp, "files_temp"),
        ];
        for (subdir, name) in expected {
            assert_eq!(subdir.as_str(), name);
        }
        assert_eq!(DataSubdir::VARIANTS.len(), expected.len());
    }

    #[test]
    fn test_data_subdir_all() {
        let all = DataSubdir::all();
        // Debug and Files are excluded from all() - only created when enabled
        assert_eq!(all.len(), 2);
        assert!(all.contains(&DataSubdir::Sqlite));
        assert!(all.contains(&DataSubdir::Duckdb));
        assert!(!all.contains(&DataSubdir::Debug));
        assert!(!all.contains(&DataSubdir::Files));
        assert!(!all.contains(&DataSubdir::FilesTemp));
    }

    #[test]
    fn test_data_subdir_files() {
        let files = DataSubdir::files();
        assert_eq!(files.len(), 2);
        assert!(files.contains(&DataSubdir::Files));
        assert!(files.contains(&DataSubdir::FilesTemp));
    }

    #[test]
    fn data_dir_fallback_is_non_empty() {
        let path = AppStorage::resolve_data_dir_with_override(None);
        assert!(!path.as_os_str().is_empty());
    }

    #[test]
    fn data_dir_override_is_expanded() {
        let path = AppStorage::resolve_data_dir_with_override(Some("./sideseat-test-data"));
        assert!(path.is_absolute());
        assert!(path.ends_with("sideseat-test-data"));
    }

    /// The wording the `anyhow` context produced, and the I/O cause the context chain carried.
    #[tokio::test]
    async fn create_directory_error_keeps_the_message_and_the_cause() {
        let root = tempfile::tempdir().unwrap();
        let occupied = root.path().join("not-a-directory");
        std::fs::write(&occupied, b"occupied").unwrap();

        let error = create_dir("sqlite", occupied.join("sqlite"))
            .await
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            format!(
                "Failed to create sqlite directory: {}",
                occupied.join("sqlite").display()
            )
        );
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn test_storage_fails_fast_when_root_cannot_be_created() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("not-a-directory");
        std::fs::write(&file, b"occupied").unwrap();

        assert!(std::panic::catch_unwind(|| AppStorage::init_for_test(file)).is_err());
    }
}
