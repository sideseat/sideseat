//! Composition of the high-level file service with concrete storage adapters.

use std::sync::Arc;

use sideseat_adapter_blob_storage::{FilesystemStorage, S3Storage};
use sideseat_domain::files::{FileService, FileServiceError};
use sideseat_domain::storage_governance::StorageGovernanceService;
use sideseat_ports::blobs::{FileStorage, FileStorageError};

use super::storage::TransactionalService;
use sideseat_adapter_cache::CacheService;
use sideseat_core::core::config::{FilesConfig, StorageBackend};
use sideseat_core::core::storage::{AppStorage, DataSubdir};

pub async fn create_file_service(
    config: FilesConfig,
    app_storage: &AppStorage,
    database: Arc<TransactionalService>,
    cache: Arc<CacheService>,
) -> Result<FileService, FileServiceError> {
    create_file_service_inner(config, app_storage, database, cache, None).await
}

pub async fn create_governed_file_service(
    config: FilesConfig,
    app_storage: &AppStorage,
    database: Arc<TransactionalService>,
    cache: Arc<CacheService>,
    governance: Arc<StorageGovernanceService>,
) -> Result<FileService, FileServiceError> {
    create_file_service_inner(config, app_storage, database, cache, Some(governance)).await
}

async fn create_file_service_inner(
    config: FilesConfig,
    app_storage: &AppStorage,
    database: Arc<TransactionalService>,
    cache: Arc<CacheService>,
    governance: Option<Arc<StorageGovernanceService>>,
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

    match governance {
        Some(governance) => {
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
        None => {
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
