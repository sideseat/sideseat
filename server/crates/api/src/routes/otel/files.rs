//! File retrieval endpoint
//!
//! Serves file content stored outside DuckDB.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::auth::ProjectRead;
use crate::types::ApiError;
use sideseat_core::utils::file_uri::is_valid_file_hash;
use sideseat_domain::files::{FileService, FileServiceError};

const FILE_CACHE_CONTROL: &str = "private, max-age=31536000, immutable";

/// State for files API
#[derive(Clone)]
pub struct FilesApiState {
    pub file_service: Arc<FileService>,
}

/// Path parameters for file retrieval
/// Note: project_id is also extracted by ProjectRead for auth, but Axum's Path
/// extractor requires ALL path params to be captured in the struct.
#[derive(Debug, Deserialize)]
pub struct FilePathParams {
    #[serde(rename = "project_id")]
    pub _project_id: String,
    pub hash: String,
}

/// Query parameters for file retrieval
#[derive(Debug, Deserialize)]
pub struct FileQueryParams {
    /// If true, serve with Content-Disposition: inline (display in browser)
    /// If false/absent, serve with Content-Disposition: attachment (download)
    #[serde(default)]
    pub inline: bool,
}

/// Get file by hash
///
/// Returns the file content with appropriate Content-Type header.
#[utoipa::path(
    get,
    path = "/api/v1/project/{project_id}/files/{hash}",
    tag = "files",
    params(
        ("project_id" = String, Path, description = "Project ID"),
        ("hash" = String, Path, description = "File BLAKE3 hash (64 hex chars)"),
        ("inline" = Option<bool>, Query, description = "If true, serve with Content-Disposition: inline (display in browser). Default: false (download)")
    ),
    responses(
        (status = 200, description = "File content"),
        (status = 404, description = "File not found"),
        (status = 503, description = "File storage disabled")
    )
)]
pub async fn get_file(
    State(state): State<FilesApiState>,
    auth: ProjectRead,
    Path(path): Path<FilePathParams>,
    Query(params): Query<FileQueryParams>,
) -> Result<Response, ApiError> {
    let project_id = &auth.project_id;
    let hash = &path.hash;

    // Validate hash format (64 hex chars)
    if !is_valid_file_hash(hash) {
        return Err(ApiError::bad_request(
            "INVALID_HASH",
            "Invalid file hash format",
        ));
    }

    // Get file from service
    let content = state
        .file_service
        .get_file(project_id, hash)
        .await
        .map_err(|e| match e {
            FileServiceError::ContentUnavailable { .. } => ApiError::not_found(
                "FILE_CONTENT_UNAVAILABLE",
                format!(
                    "File metadata was restored but content is unavailable: {}/{}",
                    project_id, hash
                ),
            ),
            FileServiceError::NotFound { .. } => ApiError::not_found(
                "FILE_NOT_FOUND",
                format!("File not found: {}/{}", project_id, hash),
            ),
            FileServiceError::Disabled => ApiError::service_unavailable("File storage is disabled"),
            _ => ApiError::internal(format!("Failed to retrieve file: {}", e)),
        })?;

    // Build response with Content-Type
    let mut headers = HeaderMap::new();

    // Set Content-Type from stored media_type or default to octet-stream
    let content_type = content
        .media_type
        .as_deref()
        .unwrap_or("application/octet-stream");
    headers.insert(
        header::CONTENT_TYPE,
        content_type
            .parse()
            .unwrap_or_else(|_| "application/octet-stream".parse().unwrap()),
    );

    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(FILE_CACHE_CONTROL),
    );

    // Add ETag (the hash is a perfect ETag)
    headers.insert(header::ETAG, format!("\"{}\"", hash).parse().unwrap());

    // Add Content-Length for download progress
    headers.insert(
        header::CONTENT_LENGTH,
        content.data.len().to_string().parse().unwrap(),
    );

    headers.insert(
        header::CONTENT_DISPOSITION,
        content_disposition(params.inline),
    );

    Ok((headers, Body::from(content.data)).into_response())
}

/// Check if file exists and get metadata
#[utoipa::path(
    head,
    path = "/api/v1/project/{project_id}/files/{hash}",
    tag = "files",
    params(
        ("project_id" = String, Path, description = "Project ID"),
        ("hash" = String, Path, description = "File BLAKE3 hash (64 hex chars)"),
        ("inline" = Option<bool>, Query, description = "Match the GET response disposition")
    ),
    responses(
        (status = 200, description = "File exists"),
        (status = 404, description = "File not found"),
        (status = 503, description = "File storage disabled")
    )
)]
pub async fn head_file(
    State(state): State<FilesApiState>,
    auth: ProjectRead,
    Path(path): Path<FilePathParams>,
    Query(params): Query<FileQueryParams>,
) -> Result<Response, ApiError> {
    let project_id = &auth.project_id;
    let hash = &path.hash;

    // Validate hash format
    if !is_valid_file_hash(hash) {
        return Err(ApiError::bad_request(
            "INVALID_HASH",
            "Invalid file hash format",
        ));
    }

    // Get file metadata (includes existence check)
    let metadata = state
        .file_service
        .get_file_metadata(project_id, hash)
        .await
        .map_err(|e| match e {
            FileServiceError::ContentUnavailable { .. } => ApiError::not_found(
                "FILE_CONTENT_UNAVAILABLE",
                format!(
                    "File metadata was restored but content is unavailable: {}/{}",
                    project_id, hash
                ),
            ),
            FileServiceError::NotFound { .. } => ApiError::not_found(
                "FILE_NOT_FOUND",
                format!("File not found: {}/{}", project_id, hash),
            ),
            FileServiceError::Disabled => ApiError::service_unavailable("File storage is disabled"),
            _ => ApiError::internal(format!("Failed to check file: {}", e)),
        })?;

    let mut headers = HeaderMap::new();

    // Set Content-Type from stored media_type
    let content_type = metadata
        .media_type
        .as_deref()
        .unwrap_or("application/octet-stream");
    headers.insert(
        header::CONTENT_TYPE,
        content_type
            .parse()
            .unwrap_or_else(|_| "application/octet-stream".parse().unwrap()),
    );

    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(FILE_CACHE_CONTROL),
    );

    // Add ETag
    headers.insert(header::ETAG, format!("\"{}\"", hash).parse().unwrap());

    // Add Content-Length
    headers.insert(
        header::CONTENT_LENGTH,
        metadata.size_bytes.to_string().parse().unwrap(),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        content_disposition(params.inline),
    );

    Ok((headers, Body::empty()).into_response())
}

fn content_disposition(inline: bool) -> HeaderValue {
    HeaderValue::from_static(if inline { "inline" } else { "attachment" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authenticated_files_are_cacheable_only_in_private_caches() {
        assert_eq!(
            HeaderValue::from_static(FILE_CACHE_CONTROL),
            "private, max-age=31536000, immutable"
        );
    }

    #[test]
    fn get_and_head_share_the_requested_content_disposition() {
        assert_eq!(content_disposition(false), "attachment");
        assert_eq!(content_disposition(true), "inline");
    }
}
