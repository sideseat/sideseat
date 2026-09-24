//! Composition of the high-level file service with concrete storage adapters.

use std::sync::Arc;

use sideseat_adapter_blob_storage::{FilesystemStorage, S3Storage};
use sideseat_adapter_cache::CacheService;
use sideseat_core::config::{FilesConfig, StorageBackend};
use sideseat_core::storage::{AppStorage, DataSubdir};
use sideseat_domain::files::{FileService, FileServiceError};
use sideseat_domain::storage_governance::StorageGovernanceService;
use sideseat_ports::blobs::{FileStorage, FileStorageError};

use super::storage::TransactionalService;

/// Build an ungoverned file service for standalone tools and benchmarks.
pub async fn create_file_service(
    config: FilesConfig,
    app_storage: &AppStorage,
    database: Arc<TransactionalService>,
    cache: Arc<CacheService>,
) -> Result<FileService, FileServiceError> {
    create_file_service_inner(config, app_storage, database, cache, None, true).await
}

pub(super) async fn create_governed_file_service(
    config: FilesConfig,
    app_storage: &AppStorage,
    database: Arc<TransactionalService>,
    cache: Arc<CacheService>,
    governance: Arc<StorageGovernanceService>,
) -> Result<FileService, FileServiceError> {
    create_file_service_inner(config, app_storage, database, cache, Some(governance), true).await
}

pub(super) async fn create_governed_file_service_deferred_cleanup(
    config: FilesConfig,
    app_storage: &AppStorage,
    database: Arc<TransactionalService>,
    cache: Arc<CacheService>,
    governance: Arc<StorageGovernanceService>,
) -> Result<FileService, FileServiceError> {
    create_file_service_inner(
        config,
        app_storage,
        database,
        cache,
        Some(governance),
        false,
    )
    .await
}

async fn create_file_service_inner(
    config: FilesConfig,
    app_storage: &AppStorage,
    database: Arc<TransactionalService>,
    cache: Arc<CacheService>,
    governance: Option<Arc<StorageGovernanceService>>,
    run_cleanup: bool,
) -> Result<FileService, FileServiceError> {
    let storage: Arc<dyn FileStorage> = match config.storage {
        StorageBackend::S3 => {
            let s3_config = config.s3.as_ref().ok_or_else(|| {
                FileServiceError::Storage(FileStorageError::Backend(
                    "S3 storage configured but no s3 config provided (missing bucket)".to_string(),
                ))
            })?;
            Arc::new(
                S3Storage::new(
                    s3_config.bucket.clone(),
                    s3_config.prefix.clone(),
                    s3_config.region.clone(),
                    s3_config.endpoint.clone(),
                )
                .await?,
            )
        }
        StorageBackend::Filesystem => {
            let files_path = config
                .filesystem_path
                .as_ref()
                .map(|path| sideseat_core::utils::file::expand_path(path))
                .unwrap_or_else(|| app_storage.subdir(DataSubdir::Files));
            Arc::new(FilesystemStorage::new(files_path))
        }
    };

    match (governance, run_cleanup) {
        (Some(governance), true) => {
            FileService::new_governed(
                config,
                app_storage.subdir(DataSubdir::FilesTemp),
                storage,
                Arc::from(database.repository()),
                cache,
                governance,
            )
            .await
        }
        (Some(governance), false) => {
            FileService::new_governed_deferred_cleanup(
                config,
                app_storage.subdir(DataSubdir::FilesTemp),
                storage,
                Arc::from(database.repository()),
                cache,
                governance,
            )
            .await
        }
        (None, _) => {
            FileService::new(
                config,
                app_storage.subdir(DataSubdir::FilesTemp),
                storage,
                Arc::from(database.repository()),
                cache,
            )
            .await
        }
    }
}
