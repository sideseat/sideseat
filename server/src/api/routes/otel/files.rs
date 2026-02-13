//! File retrieval endpoint
//!
//! Serves file content stored outside DuckDB.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::api::auth::ProjectRead;
use crate::api::types::ApiError;
use crate::data::files::{FileService, FileServiceError};

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
    #[allow(dead_code)] // Auth handled by ProjectRead extractor
    pub project_id: String,
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
        ("hash" = String, Path, description = "File SHA-256 hash (64 hex chars)"),
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
    if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
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

    // Add cache headers (files are content-addressed, can be cached forever)
    headers.insert(
        header::CACHE_CONTROL,
        "public, max-age=31536000, immutable".parse().unwrap(),
    );

    // Add ETag (the hash is a perfect ETag)
    headers.insert(header::ETAG, format!("\"{}\"", hash).parse().unwrap());

    // Add Content-Length for download progress
    headers.insert(
        header::CONTENT_LENGTH,
        content.data.len().to_string().parse().unwrap(),
    );

    // Set Content-Disposition based on inline parameter
    let disposition = if params.inline {
        "inline"
    } else {
        "attachment"
    };
    headers.insert(header::CONTENT_DISPOSITION, disposition.parse().unwrap());

    Ok((headers, Body::from(content.data)).into_response())
}

/// Check if file exists and get metadata
#[utoipa::path(
    head,
    path = "/api/v1/project/{project_id}/files/{hash}",
    tag = "files",
    params(
        ("project_id" = String, Path, description = "Project ID"),
        ("hash" = String, Path, description = "File SHA-256 hash (64 hex chars)")
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
) -> Result<Response, ApiError> {
    let project_id = &auth.project_id;
    let hash = &path.hash;

    // Validate hash format
    if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
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

    // Add cache headers
    headers.insert(
        header::CACHE_CONTROL,
        "public, max-age=31536000, immutable".parse().unwrap(),
    );

    // Add ETag
    headers.insert(header::ETAG, format!("\"{}\"", hash).parse().unwrap());

    // Add Content-Length
    headers.insert(
        header::CONTENT_LENGTH,
        metadata.size_bytes.to_string().parse().unwrap(),
    );

    Ok((headers, Body::empty()).into_response())
}
