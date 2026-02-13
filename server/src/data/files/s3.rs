//! S3-based file storage implementation
//!
//! Stores files in AWS S3 (or S3-compatible services like MinIO) with a sharded key structure:
//! `{prefix}/{project_id}/{hash[0:2]}/{hash[2:4]}/{hash}`
//!
//! This module is only available when the `s3` feature is enabled.

use std::path::Path;

use async_trait::async_trait;
use aws_sdk_s3::Client;
use aws_sdk_s3::primitives::ByteStream;

use super::error::FileStorageError;
use super::storage::FileStorage;

/// S3-based file storage
#[derive(Debug, Clone)]
pub struct S3Storage {
    /// S3 client
    client: Client,
    /// S3 bucket name
    bucket: String,
    /// Key prefix for all files
    prefix: String,
}

impl S3Storage {
    /// Create a new S3 storage with the given configuration
    pub async fn new(
        bucket: String,
        prefix: String,
        region: Option<String>,
        endpoint: Option<String>,
    ) -> Result<Self, FileStorageError> {
        let mut config_loader = aws_config::defaults(aws_config::BehaviorVersion::latest());

        // Set region if provided
        if let Some(region) = region {
            config_loader = config_loader.region(aws_sdk_s3::config::Region::new(region));
        }

        let config = config_loader.load().await;

        // Build S3 client with optional custom endpoint
        let mut s3_config = aws_sdk_s3::config::Builder::from(&config);

        if let Some(endpoint_url) = endpoint {
            s3_config = s3_config.endpoint_url(endpoint_url).force_path_style(true); // Required for most S3-compatible services
        }

        let client = Client::from_conf(s3_config.build());

        tracing::debug!(
            bucket = %bucket,
            prefix = %prefix,
            "S3 storage initialized"
        );

        Ok(Self {
            client,
            bucket,
            prefix,
        })
    }

    /// Get the full S3 key for a file
    ///
    /// Returns key like: `{prefix}/{project}/{hash[0:2]}/{hash[2:4]}/{hash}`
    fn object_key(&self, project_id: &str, hash: &str) -> String {
        let shard1 = &hash[0..2];
        let shard2 = &hash[2..4];
        format!(
            "{}/{}/{}/{}/{}",
            self.prefix, project_id, shard1, shard2, hash
        )
    }

    /// Get the prefix for a project
    fn project_prefix(&self, project_id: &str) -> String {
        format!("{}/{}/", self.prefix, project_id)
    }

    /// Validate hash format (64 hex characters)
    fn validate_hash(hash: &str) -> Result<(), FileStorageError> {
        if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(FileStorageError::Backend(format!(
                "Invalid hash format: expected 64 hex chars, got {}",
                hash.len()
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl FileStorage for S3Storage {
    async fn store(
        &self,
        project_id: &str,
        hash: &str,
        data: &[u8],
    ) -> Result<(), FileStorageError> {
        Self::validate_hash(hash)?;

        let key = self.object_key(project_id, hash);

        // Check if object already exists (content-addressed deduplication)
        match self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
        {
            Ok(_) => {
                tracing::trace!(
                    project_id,
                    hash,
                    "File already exists in S3, skipping upload (content-addressed)"
                );
                return Ok(());
            }
            Err(err) => {
                // Only continue if the error is "not found"
                let service_err = err.into_service_error();
                if !service_err.is_not_found() {
                    return Err(FileStorageError::Backend(format!(
                        "S3 head_object error: {}",
                        service_err
                    )));
                }
            }
        }

        // Upload the object
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .body(ByteStream::from(data.to_vec()))
            .send()
            .await
            .map_err(|e| FileStorageError::Backend(format!("S3 put_object error: {}", e)))?;

        tracing::debug!(
            project_id,
            hash,
            size = data.len(),
            key = %key,
            "File stored in S3"
        );

        Ok(())
    }

    async fn get(&self, project_id: &str, hash: &str) -> Result<Vec<u8>, FileStorageError> {
        Self::validate_hash(hash)?;

        let key = self.object_key(project_id, hash);

        let response = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
            .map_err(|e| {
                let service_err = e.into_service_error();
                if service_err.is_no_such_key() {
                    FileStorageError::NotFound {
                        project_id: project_id.to_string(),
                        hash: hash.to_string(),
                    }
                } else {
                    FileStorageError::Backend(format!("S3 get_object error: {}", service_err))
                }
            })?;

        let data = response
            .body
            .collect()
            .await
            .map_err(|e| FileStorageError::Backend(format!("S3 body read error: {}", e)))?
            .into_bytes()
            .to_vec();

        Ok(data)
    }

    async fn exists(&self, project_id: &str, hash: &str) -> Result<bool, FileStorageError> {
        Self::validate_hash(hash)?;

        let key = self.object_key(project_id, hash);

        match self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
        {
            Ok(_) => Ok(true),
            Err(err) => {
                let service_err = err.into_service_error();
                if service_err.is_not_found() {
                    Ok(false)
                } else {
                    Err(FileStorageError::Backend(format!(
                        "S3 head_object error: {}",
                        service_err
                    )))
                }
            }
        }
    }

    async fn delete(&self, project_id: &str, hash: &str) -> Result<(), FileStorageError> {
        Self::validate_hash(hash)?;

        let key = self.object_key(project_id, hash);

        // S3 delete_object doesn't fail if object doesn't exist
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
            .map_err(|e| FileStorageError::Backend(format!("S3 delete_object error: {}", e)))?;

        tracing::debug!(project_id, hash, "File deleted from S3");

        Ok(())
    }

    async fn delete_project(&self, project_id: &str) -> Result<u64, FileStorageError> {
        let prefix = self.project_prefix(project_id);
        let mut deleted_count = 0u64;
        let mut continuation_token: Option<String> = None;

        loop {
            // List objects with the project prefix
            let mut request = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(&prefix);

            if let Some(token) = continuation_token {
                request = request.continuation_token(token);
            }

            let response = request.send().await.map_err(|e| {
                FileStorageError::Backend(format!("S3 list_objects_v2 error: {}", e))
            })?;

            let objects: Vec<_> = response
                .contents()
                .iter()
                .filter_map(|obj| obj.key().map(|k| k.to_string()))
                .collect();

            if objects.is_empty() {
                break;
            }

            // Delete objects in batches (S3 allows up to 1000 per request)
            for chunk in objects.chunks(1000) {
                let delete_objects: Vec<_> = chunk
                    .iter()
                    .map(|key| {
                        aws_sdk_s3::types::ObjectIdentifier::builder()
                            .key(key)
                            .build()
                            .expect("key is provided")
                    })
                    .collect();

                let delete_request = aws_sdk_s3::types::Delete::builder()
                    .set_objects(Some(delete_objects))
                    .build()
                    .map_err(|e| {
                        FileStorageError::Backend(format!("S3 delete request build error: {}", e))
                    })?;

                self.client
                    .delete_objects()
                    .bucket(&self.bucket)
                    .delete(delete_request)
                    .send()
                    .await
                    .map_err(|e| {
                        FileStorageError::Backend(format!("S3 delete_objects error: {}", e))
                    })?;

                deleted_count += chunk.len() as u64;
            }

            // Check if there are more objects
            if response.is_truncated() == Some(true) {
                continuation_token = response.next_continuation_token().map(|s| s.to_string());
            } else {
                break;
            }
        }

        tracing::debug!(
            project_id,
            deleted = deleted_count,
            "Project files deleted from S3"
        );

        Ok(deleted_count)
    }

    async fn finalize_temp(
        &self,
        project_id: &str,
        hash: &str,
        temp_path: &Path,
    ) -> Result<(), FileStorageError> {
        Self::validate_hash(hash)?;

        let key = self.object_key(project_id, hash);

        // Check if object already exists (content-addressed deduplication)
        match self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
        {
            Ok(_) => {
                // File already exists, just remove temp
                tokio::fs::remove_file(temp_path).await.ok();
                tracing::trace!(
                    project_id,
                    hash,
                    "File already exists in S3, removed temp (content-addressed)"
                );
                return Ok(());
            }
            Err(err) => {
                let service_err = err.into_service_error();
                if !service_err.is_not_found() {
                    return Err(FileStorageError::Backend(format!(
                        "S3 head_object error: {}",
                        service_err
                    )));
                }
            }
        }

        // Read temp file and upload to S3
        let data = tokio::fs::read(temp_path)
            .await
            .map_err(FileStorageError::Io)?;

        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .body(ByteStream::from(data))
            .send()
            .await
            .map_err(|e| FileStorageError::Backend(format!("S3 put_object error: {}", e)))?;

        // Remove temp file
        tokio::fs::remove_file(temp_path).await.ok();

        tracing::debug!(
            project_id,
            hash,
            key = %key,
            "File finalized to S3"
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper function to compute object key (same logic as S3Storage::object_key)
    fn compute_object_key(prefix: &str, project_id: &str, hash: &str) -> String {
        let shard1 = &hash[0..2];
        let shard2 = &hash[2..4];
        format!("{}/{}/{}/{}/{}", prefix, project_id, shard1, shard2, hash)
    }

    // Helper function to compute project prefix (same logic as S3Storage::project_prefix)
    fn compute_project_prefix(prefix: &str, project_id: &str) -> String {
        format!("{}/{}/", prefix, project_id)
    }

    #[test]
    fn test_object_key() {
        let key = compute_object_key(
            "sideseat/files",
            "project1",
            "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
        );

        assert_eq!(
            key,
            "sideseat/files/project1/a1/b2/a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2"
        );
    }

    #[test]
    fn test_project_prefix() {
        let prefix = compute_project_prefix("sideseat/files", "my-project");
        assert_eq!(prefix, "sideseat/files/my-project/");
    }

    #[test]
    fn test_validate_hash_valid() {
        let result = S3Storage::validate_hash(
            "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_hash_invalid_length() {
        let result = S3Storage::validate_hash("abc123");
        assert!(matches!(result, Err(FileStorageError::Backend(_))));
    }

    #[test]
    fn test_validate_hash_invalid_chars() {
        let result = S3Storage::validate_hash(
            "g1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
        );
        assert!(matches!(result, Err(FileStorageError::Backend(_))));
    }
}
