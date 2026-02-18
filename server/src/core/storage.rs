//! Platform-aware data storage directory management
//!
//! ## Platform Paths
//!
//! | Type | Windows | macOS | Linux |
//! |------|---------|-------|-------|
//! | Data | `%APPDATA%\SideSeat\` | `~/Library/Application Support/SideSeat/` | `$XDG_DATA_HOME/sideseat/` |

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::ProjectDirs;

use super::config::AppConfig;
use super::constants::{APP_DOT_FOLDER, APP_NAME, ENV_DATA_DIR};
use crate::utils::file::expand_path;

/// Data subdirectories
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSubdir {
    Sqlite,
    Duckdb,
    Debug,
    Files,
    FilesTemp,
}

impl DataSubdir {
    pub const fn as_str(&self) -> &'static str {
        match self {
            DataSubdir::Sqlite => "sqlite",
            DataSubdir::Duckdb => "duckdb",
            DataSubdir::Debug => "debug",
            DataSubdir::Files => "files",
            DataSubdir::FilesTemp => "files_temp",
        }
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
    pub async fn init(config: &AppConfig) -> Result<Self> {
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

    /// Resolve data directory from env var or platform default
    pub fn resolve_data_dir() -> PathBuf {
        // Check env var override first
        if let Ok(dir) = std::env::var(ENV_DATA_DIR) {
            return expand_path(&dir);
        }

        // Use platform-specific directory
        if let Some(proj_dirs) = ProjectDirs::from("", "", APP_NAME) {
            return proj_dirs.data_dir().to_path_buf();
        }

        // Fallback to local .sideseat
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        cwd.join(APP_DOT_FOLDER)
    }

    /// Create data directory and subdirectories (static version for init)
    async fn ensure_directories_static(
        data_dir: &Path,
        debug: bool,
        files_enabled: bool,
    ) -> Result<()> {
        // Create base data directory
        tokio::fs::create_dir_all(data_dir)
            .await
            .with_context(|| format!("Failed to create data directory: {}", data_dir.display()))?;

        // Create subdirectories
        for subdir in DataSubdir::all() {
            let path = data_dir.join(subdir.as_str());
            tokio::fs::create_dir_all(&path).await.with_context(|| {
                format!(
                    "Failed to create {} directory: {}",
                    subdir.as_str(),
                    path.display()
                )
            })?;
        }

        // Create debug directory if debug mode is enabled
        if debug {
            let path = data_dir.join(DataSubdir::Debug.as_str());
            tokio::fs::create_dir_all(&path)
                .await
                .with_context(|| format!("Failed to create debug directory: {}", path.display()))?;
        }

        // Create file storage directories if enabled
        if files_enabled {
            for subdir in DataSubdir::files() {
                let path = data_dir.join(subdir.as_str());
                tokio::fs::create_dir_all(&path).await.with_context(|| {
                    format!(
                        "Failed to create {} directory: {}",
                        subdir.as_str(),
                        path.display()
                    )
                })?;
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

    /// Create AppStorage for testing with a specific data directory
    #[cfg(test)]
    pub fn init_for_test(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_data_subdir_as_str() {
        assert_eq!(DataSubdir::Sqlite.as_str(), "sqlite");
        assert_eq!(DataSubdir::Duckdb.as_str(), "duckdb");
        assert_eq!(DataSubdir::Debug.as_str(), "debug");
        assert_eq!(DataSubdir::Files.as_str(), "files");
        assert_eq!(DataSubdir::FilesTemp.as_str(), "files_temp");
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
    fn test_resolve_data_dir_fallback() {
        // Without env var set, should return a non-empty path
        // SAFETY: Test runs single-threaded, no concurrent access to env var
        unsafe { std::env::remove_var(ENV_DATA_DIR) };
        let path = AppStorage::resolve_data_dir();
        assert!(!path.as_os_str().is_empty());
    }
}
