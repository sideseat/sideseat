//! Credentials API types

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

use crate::data::types::{CredentialPermissionRow, CredentialRow};
use crate::domain::providers::{CredentialSource, ResolvedCredential};

/// DTO for credential list/get responses (no secret value ever returned)
#[derive(Debug, Serialize, ToSchema)]
pub struct CredentialDto {
    pub id: String,
    pub provider_key: String,
    pub display_name: String,
    pub endpoint_url: Option<String>,
    pub extra_config: Option<serde_json::Value>,
    pub key_preview: Option<String>,
    /// "stored", "env", or "ambient"
    pub source: String,
    pub env_var_name: Option<String>,
    pub read_only: bool,
    pub created_by: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
}

impl From<ResolvedCredential> for CredentialDto {
    fn from(r: ResolvedCredential) -> Self {
        let (source_str, env_var_name) = match &r.source {
            CredentialSource::Stored => ("stored".to_string(), None),
            CredentialSource::Environment { var_name } => {
                ("env".to_string(), Some(var_name.clone()))
            }
            // Ambient: reuse env_var_name to carry the human-readable description
            // (e.g. "IRSA / EKS workload identity") for display in the UI.
            CredentialSource::Ambient { description } => {
                ("ambient".to_string(), Some(description.clone()))
            }
        };
        Self {
            id: r.id,
            provider_key: r.provider_key,
            display_name: r.display_name,
            endpoint_url: r.endpoint_url,
            extra_config: r.extra_config,
            key_preview: r.key_preview,
            source: source_str,
            env_var_name,
            read_only: r.read_only,
            created_by: r.created_by,
            created_at: r.created_at.and_then(|ts| DateTime::from_timestamp(ts, 0)),
        }
    }
}

/// DTO for credential rows (stored credentials) — used in create response
impl From<CredentialRow> for CredentialDto {
    fn from(row: CredentialRow) -> Self {
        let extra_config = row
            .extra_config
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());
        Self {
            id: row.id,
            provider_key: row.provider_key,
            display_name: row.display_name,
            endpoint_url: row.endpoint_url,
            extra_config,
            key_preview: row.key_preview,
            source: "stored".to_string(),
            env_var_name: None,
            read_only: false,
            created_by: row.created_by,
            created_at: DateTime::from_timestamp(row.created_at, 0),
        }
    }
}

/// DTO for permission list responses
#[derive(Debug, Serialize, ToSchema)]
pub struct CredentialPermissionDto {
    pub id: String,
    pub credential_id: String,
    pub organization_id: String,
    pub project_id: Option<String>,
    /// "allow" or "deny"
    pub access: String,
    pub created_by: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<CredentialPermissionRow> for CredentialPermissionDto {
    fn from(row: CredentialPermissionRow) -> Self {
        Self {
            id: row.id,
            credential_id: row.credential_id,
            organization_id: row.organization_id,
            project_id: row.project_id,
            access: row.access,
            created_by: row.created_by,
            created_at: DateTime::from_timestamp(row.created_at, 0).unwrap_or_else(Utc::now),
        }
    }
}

/// Request body for creating a credential
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateCredentialRequest {
    /// Display name (1-100 characters)
    #[validate(length(min = 1, max = 100, message = "Display name must be 1-100 characters"))]
    pub display_name: String,
    /// Provider key (e.g. "anthropic", "openai", "bedrock")
    #[validate(length(min = 1, max = 64))]
    pub provider_key: String,
    /// Secret value (API key, JSON credentials, etc.) — never returned
    pub secret_value: Option<String>,
    /// Custom endpoint URL (for ollama, azure, custom providers)
    pub endpoint_url: Option<String>,
    /// Provider-specific config (region, auth_mode, api_variant, etc.)
    pub extra_config: Option<serde_json::Value>,
}

/// Request body for updating credential metadata
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateCredentialRequest {
    /// New display name (1-100 characters)
    #[validate(length(min = 1, max = 100, message = "Display name must be 1-100 characters"))]
    pub display_name: Option<String>,
    /// New endpoint URL. Absent = no change; null = clear
    #[serde(
        default,
        deserialize_with = "crate::utils::serde::double_option_string"
    )]
    pub endpoint_url: Option<Option<String>>,
    /// New extra config. Absent = no change; null = clear
    #[serde(default, deserialize_with = "crate::utils::serde::double_option_value")]
    pub extra_config: Option<Option<serde_json::Value>>,
}

/// Request body for creating a permission rule
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreatePermissionRequest {
    /// Project ID to apply rule to. Absent = org-level default (all projects)
    pub project_id: Option<String>,
    /// "allow" or "deny"
    #[validate(custom(function = "validate_access"))]
    pub access: String,
}

fn validate_access(access: &str) -> Result<(), validator::ValidationError> {
    if access == "allow" || access == "deny" {
        Ok(())
    } else {
        Err(validator::ValidationError::new("invalid_access"))
    }
}

/// Result of a test-connection attempt
#[derive(Debug, Serialize, ToSchema)]
pub struct TestResultDto {
    pub success: bool,
    pub latency_ms: u64,
    pub error: Option<String>,
    pub model_hint: Option<String>,
}
