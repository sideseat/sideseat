//! Platform-aware storage directory management
//!
//! Manages platform-appropriate storage directories for configuration, data, cache,
//! logs, and temporary files. Automatically creates and verifies directory access
//! on initialization.
//!
//! ## Platform Paths
//!
//! | Type | Windows | macOS | Linux |
//! |------|---------|-------|-------|
//! | User Config | `%USERPROFILE%\SideSeat\` | `~/.sideseat/` | `~/.sideseat/` |
//! | Config | `%APPDATA%\SideSeat\config\` | `~/Library/Application Support/SideSeat/` | `$XDG_CONFIG_HOME/sideseat/` |
//! | Data | `%APPDATA%\SideSeat\data\` | `~/Library/Application Support/SideSeat/` | `$XDG_DATA_HOME/sideseat/` |
//! | Cache | `%LOCALAPPDATA%\SideSeat\cache\` | `~/Library/Caches/SideSeat/` | `$XDG_CACHE_HOME/sideseat/` |
//! | Logs | `%LOCALAPPDATA%\SideSeat\logs\` | `~/Library/Logs/SideSeat/` | `$XDG_STATE_HOME/sideseat/` |
//! | Temp | `%TEMP%\sideseat\` | `$TMPDIR/sideseat/` | `/tmp/sideseat/` |
//!
//! ## Usage
//!
//! ```rust,ignore
//! use sideseat::core::StorageManager;
//!
//! let storage = StorageManager::init().await?;
//!
//! // Access standard directories
//! println!("Config: {}", storage.config_dir().display());
//! println!("Data: {}", storage.data_dir().display());
//!
//! // Get path to a specific file
//! let db_path = storage.get_path(StorageType::Data, "app.db");
//! ```

use super::constants::{
    ACCESS_CHECK_FILE, APP_DOT_FOLDER, APP_NAME, APP_NAME_LOWER, ENV_CACHE_DIR, ENV_CONFIG_DIR,
    ENV_DATA_DIR,
};
use super::utils::expand_path;
use crate::error::{Error, Result};
use directories::ProjectDirs;
use std::path::{Path, PathBuf};
use tokio::fs;

/// Storage location types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageType {
    Config,
    Data,
    Cache,
    Logs,
    Temp,
}

/// Subdirectories within the data storage location
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSubdir {
    /// OpenTelemetry trace data (data/traces/)
    Traces,
}

impl DataSubdir {
    /// Returns the directory name for this subdirectory
    pub const fn as_str(&self) -> &'static str {
        match self {
            DataSubdir::Traces => "traces",
        }
    }

    /// Returns all data subdirectories
    pub const fn all() -> &'static [DataSubdir] {
        &[DataSubdir::Traces]
    }
}

/// Storage manager with resolved paths
///
/// Manages platform-appropriate storage directories for configuration,
/// data, cache, logs, and temporary files.
#[derive(Debug, Clone)]
pub struct StorageManager {
    /// Current working directory (where the app was started)
    work_dir: PathBuf,
    /// User-accessible config dir (~/.sideseat on Unix, %USERPROFILE%\SideSeat on Windows)
    user_config_dir: PathBuf,
    config_dir: PathBuf,
    data_dir: PathBuf,
    cache_dir: PathBuf,
    logs_dir: PathBuf,
    temp_dir: PathBuf,
    using_fallback: bool,
}

impl StorageManager {
    /// Initialize storage manager and create all directories
    ///
    /// Attempts to use platform-specific paths first, falls back to
    /// `.sideseat/` in the current working directory if that fails.
    pub async fn init() -> Result<Self> {
        let work_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let user_config_dir = Self::get_user_config_dir();

        // Try standard platform paths first
        if let Some(proj_dirs) = ProjectDirs::from("", "", APP_NAME) {
            let manager = Self {
                work_dir,
                user_config_dir,
                config_dir: Self::resolve_dir(ENV_CONFIG_DIR, || {
                    proj_dirs.config_dir().to_path_buf()
                }),
                data_dir: Self::resolve_dir(ENV_DATA_DIR, || proj_dirs.data_dir().to_path_buf()),
                cache_dir: Self::resolve_dir(ENV_CACHE_DIR, || proj_dirs.cache_dir().to_path_buf()),
                logs_dir: Self::get_logs_dir(&proj_dirs),
                temp_dir: std::env::temp_dir().join(APP_NAME_LOWER),
                using_fallback: false,
            };

            if manager.ensure_directories().await.is_ok() {
                return Ok(manager);
            }
        }

        // Fallback to current directory
        Self::init_fallback().await
    }

    /// Check environment variable override, otherwise use default
    ///
    /// Handles path expansion via [`expand_path`]:
    /// - Tilde expansion: `~/.sideseat` -> `/Users/name/.sideseat`
    /// - Relative paths: `./.sideseat`, `..`, `mydir` -> absolute path
    fn resolve_dir<F>(env_var: &str, default: F) -> PathBuf
    where
        F: FnOnce() -> PathBuf,
    {
        std::env::var(env_var).map(|s| expand_path(&s)).unwrap_or_else(|_| default())
    }

    async fn init_fallback() -> Result<Self> {
        let work_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let base = work_dir.join(APP_DOT_FOLDER);

        let manager = Self {
            work_dir,
            user_config_dir: Self::get_user_config_dir(),
            config_dir: base.join("config"),
            data_dir: base.join("data"),
            cache_dir: base.join("cache"),
            logs_dir: base.join("logs"),
            temp_dir: base.join("temp"),
            using_fallback: true,
        };

        manager.ensure_directories().await?;
        Ok(manager)
    }

    /// Get user config directory path
    /// Unix: ~/.sideseat (dotfile convention)
    /// Windows: %USERPROFILE%\SideSeat (no dot - Windows convention)
    fn get_user_config_dir() -> PathBuf {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));

        #[cfg(windows)]
        {
            home.join(APP_NAME)
        }

        #[cfg(not(windows))]
        {
            home.join(APP_DOT_FOLDER)
        }
    }

    /// Initialize all required directories with creation and access verification
    async fn ensure_directories(&self) -> Result<()> {
        // Base directories to create and verify
        let base_dirs = [
            ("config", &self.config_dir),
            ("data", &self.data_dir),
            ("cache", &self.cache_dir),
            ("logs", &self.logs_dir),
            ("temp", &self.temp_dir),
        ];

        // Create base directories
        for (name, dir) in &base_dirs {
            Self::create_and_verify_dir(name, dir).await?;
        }

        // Create data subdirectories
        for subdir in DataSubdir::all() {
            let name = subdir.as_str();
            let path = self.data_dir.join(name);
            Self::create_and_verify_dir(&format!("data/{}", name), &path).await?;
        }

        tracing::debug!("All storage directories initialized and verified");
        Ok(())
    }

    /// Create directory and verify read/write access
    ///
    /// Best practices applied:
    /// - No existence check before create_dir_all (avoids TOCTOU race condition)
    /// - create_dir_all is race-condition safe (handles concurrent creation)
    /// - Batched verification to minimize spawn_blocking calls
    async fn create_and_verify_dir(name: &str, path: &PathBuf) -> Result<()> {
        // Step 1: Create directory (idempotent, race-condition safe)
        // Do NOT check existence first - that's a TOCTOU anti-pattern
        fs::create_dir_all(path).await.map_err(|e| {
            Error::Storage(format!("Failed to create {} directory {:?}: {}", name, path, e))
        })?;

        // Step 2: Verify it's a directory (handles edge case where path exists as file)
        let metadata = fs::metadata(path).await.map_err(|e| {
            Error::Storage(format!("Cannot access {} directory {:?}: {}", name, path, e))
        })?;

        if !metadata.is_dir() {
            return Err(Error::Storage(format!(
                "{} path {:?} exists but is not a directory",
                name, path
            )));
        }

        // Step 3: Verify write access (create temp file, then remove)
        let test_file = path.join(ACCESS_CHECK_FILE);
        fs::write(&test_file, b"access_test").await.map_err(|e| {
            Error::Storage(format!("{} directory {:?} is not writable: {}", name, path, e))
        })?;

        // Step 4: Verify read access (read back the test file)
        fs::read(&test_file).await.map_err(|e| {
            Error::Storage(format!("{} directory {:?} is not readable: {}", name, path, e))
        })?;

        // Cleanup test file (ignore errors - not critical)
        let _ = fs::remove_file(&test_file).await;

        tracing::debug!("Verified {} directory: {:?}", name, path);
        Ok(())
    }

    /// Get logs directory following platform conventions
    /// Linux: Uses state_dir() -> $XDG_STATE_HOME/sideseat (proper XDG)
    /// macOS: ~/Library/Logs/SideSeat
    /// Windows: %LOCALAPPDATA%\SideSeat\logs
    #[cfg(target_os = "linux")]
    fn get_logs_dir(proj_dirs: &ProjectDirs) -> PathBuf {
        // state_dir() returns $XDG_STATE_HOME/sideseat or ~/.local/state/sideseat
        proj_dirs
            .state_dir()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| proj_dirs.data_dir().join("logs"))
    }

    #[cfg(target_os = "macos")]
    fn get_logs_dir(proj_dirs: &ProjectDirs) -> PathBuf {
        dirs::home_dir()
            .map(|h| h.join(format!("Library/Logs/{}", APP_NAME)))
            .unwrap_or_else(|| proj_dirs.data_dir().join("logs"))
    }

    #[cfg(target_os = "windows")]
    fn get_logs_dir(proj_dirs: &ProjectDirs) -> PathBuf {
        proj_dirs.data_local_dir().join("logs")
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    fn get_logs_dir(proj_dirs: &ProjectDirs) -> PathBuf {
        proj_dirs.data_dir().join("logs")
    }

    // === Path Accessors ===

    /// Get the current working directory (where the app was started)
    pub fn work_dir(&self) -> &Path {
        &self.work_dir
    }

    /// Get user config directory (~/.sideseat on Unix, %USERPROFILE%\SideSeat on Windows)
    ///
    /// This directory is NOT auto-created. Users can manually create it
    /// to place custom configuration files.
    pub fn user_config_dir(&self) -> &Path {
        &self.user_config_dir
    }

    /// Get the config directory path
    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    /// Get the data directory path
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Get the cache directory path
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Get the logs directory path
    pub fn logs_dir(&self) -> &Path {
        &self.logs_dir
    }

    /// Get the temp directory path
    pub fn temp_dir(&self) -> &Path {
        &self.temp_dir
    }

    /// Returns true if using fallback storage location
    pub fn using_fallback(&self) -> bool {
        self.using_fallback
    }

    // === User Config Directory Methods ===

    /// Check if user config directory exists (~/.sideseat)
    pub async fn user_config_exists(&self) -> bool {
        fs::try_exists(&self.user_config_dir).await.unwrap_or(false)
    }

    /// Get path to a file in the user config directory
    pub fn user_config_path(&self, filename: &str) -> PathBuf {
        self.user_config_dir.join(filename)
    }

    /// Check if a specific file exists in user config directory
    pub async fn user_config_file_exists(&self, filename: &str) -> bool {
        fs::try_exists(self.user_config_path(filename)).await.unwrap_or(false)
    }

    /// Read a file from user config directory (returns None if not found)
    pub async fn read_user_config(&self, filename: &str) -> Option<String> {
        fs::read_to_string(self.user_config_path(filename)).await.ok()
    }

    /// List files in user config directory (returns empty vec if dir doesn't exist)
    pub async fn list_user_config_files(&self) -> Vec<PathBuf> {
        let mut files = Vec::new();
        if let Ok(mut entries) = fs::read_dir(&self.user_config_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                files.push(entry.path());
            }
        }
        files
    }

    // === Subdirectory Accessors ===

    /// Get path to a data subdirectory
    pub fn data_subdir(&self, subdir: DataSubdir) -> PathBuf {
        self.data_dir.join(subdir.as_str())
    }

    // === General Path Utilities ===

    /// Get path for a specific file within a storage location
    pub fn get_path(&self, storage_type: StorageType, filename: &str) -> PathBuf {
        match storage_type {
            StorageType::Config => self.config_dir.join(filename),
            StorageType::Data => self.data_dir.join(filename),
            StorageType::Cache => self.cache_dir.join(filename),
            StorageType::Logs => self.logs_dir.join(filename),
            StorageType::Temp => self.temp_dir.join(filename),
        }
    }

    /// Check if a path exists
    pub async fn exists(&self, path: &Path) -> bool {
        fs::try_exists(path).await.unwrap_or(false)
    }

    /// Verify directory is writable
    pub async fn is_writable(&self, storage_type: StorageType) -> bool {
        let test_file = self.get_path(storage_type, ".write_test");
        if fs::write(&test_file, b"test").await.is_ok() {
            let _ = fs::remove_file(&test_file).await;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::Mutex;

    // Mutex to serialize tests that change the current directory
    // (std::env::set_current_dir is a global operation that affects all threads)
    static CWD_MUTEX: Mutex<()> = Mutex::const_new(());

    #[test]
    fn test_storage_type_variants() {
        // Ensure all storage types are distinct
        let types = [
            StorageType::Config,
            StorageType::Data,
            StorageType::Cache,
            StorageType::Logs,
            StorageType::Temp,
        ];

        for (i, t1) in types.iter().enumerate() {
            for (j, t2) in types.iter().enumerate() {
                if i == j {
                    assert_eq!(t1, t2);
                } else {
                    assert_ne!(t1, t2);
                }
            }
        }
    }

    #[test]
    fn test_user_config_dir_not_empty() {
        let user_config = StorageManager::get_user_config_dir();
        assert!(!user_config.as_os_str().is_empty());
    }

    #[test]
    fn test_resolve_dir_with_env_var() {
        // Test with non-existent env var (uses default)
        let result =
            StorageManager::resolve_dir("NONEXISTENT_VAR_12345", || PathBuf::from("/default"));
        assert_eq!(result, PathBuf::from("/default"));
    }

    // Note: expand_path tests are in utils.rs

    #[tokio::test]
    async fn test_storage_manager_init() {
        let _guard = CWD_MUTEX.lock().await;

        // Use fallback init with a temp directory to avoid CI environment issues
        let temp_dir = std::env::temp_dir().join(format!("sideseat_test_{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        // Change to temp dir for fallback behavior
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(&temp_dir).unwrap();

        let result = StorageManager::init().await;

        // Restore original directory
        std::env::set_current_dir(&original_dir).unwrap();

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);

        assert!(result.is_ok());
        let storage = result.unwrap();

        // All paths should be non-empty
        assert!(!storage.work_dir().as_os_str().is_empty());
        assert!(!storage.user_config_dir().as_os_str().is_empty());
        assert!(!storage.config_dir().as_os_str().is_empty());
        assert!(!storage.data_dir().as_os_str().is_empty());
        assert!(!storage.cache_dir().as_os_str().is_empty());
        assert!(!storage.logs_dir().as_os_str().is_empty());
        assert!(!storage.temp_dir().as_os_str().is_empty());
    }

    #[tokio::test]
    async fn test_get_path() {
        let _guard = CWD_MUTEX.lock().await;

        let temp_dir =
            std::env::temp_dir().join(format!("sideseat_test_path_{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(&temp_dir).unwrap();

        let storage = StorageManager::init().await.unwrap();

        // Test assertions before cleanup
        let config_path = storage.get_path(StorageType::Config, "test.json");
        assert!(config_path.ends_with("test.json"));

        let data_path = storage.get_path(StorageType::Data, "app.db");
        assert!(data_path.ends_with("app.db"));

        // Cleanup after assertions
        std::env::set_current_dir(&original_dir).unwrap();
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_data_subdir() {
        let _guard = CWD_MUTEX.lock().await;

        let temp_dir =
            std::env::temp_dir().join(format!("sideseat_test_subdir_{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(&temp_dir).unwrap();

        let storage = StorageManager::init().await.unwrap();

        let traces_path = storage.data_subdir(DataSubdir::Traces);
        assert!(traces_path.ends_with("traces"));

        // Cleanup after assertions
        std::env::set_current_dir(&original_dir).unwrap();
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_is_writable() {
        let _guard = CWD_MUTEX.lock().await;

        let temp_dir =
            std::env::temp_dir().join(format!("sideseat_test_write_{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(&temp_dir).unwrap();

        let storage = StorageManager::init().await.unwrap();

        // Data directory should be writable after init - test before cleanup
        assert!(storage.is_writable(StorageType::Data).await);
        assert!(storage.is_writable(StorageType::Cache).await);
        assert!(storage.is_writable(StorageType::Temp).await);

        // Cleanup after assertions
        std::env::set_current_dir(&original_dir).unwrap();
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
