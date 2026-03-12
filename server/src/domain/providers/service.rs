//! CredentialService — centralized multi-source credential management.
//!
//! Coordinates DB storage, SecretManager, env scanning, and caching.
//! This is the exception to the "no service layer" rule: credentials uniquely
//! require coordinating DB + SecretManager + env scanning with atomicity guarantees.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::core::constants::{CACHE_TTL_CRED_LIST, CACHE_TTL_CRED_SECRET, CRED_SECRET_PREFIX};
use crate::data::TransactionalService;
use crate::data::cache::{CacheKey, CacheService};
use crate::data::error::DataError;
use crate::data::secrets::{Secret, SecretKey, SecretManager, SecretScope};
use crate::data::types::{CredentialPermissionRow, CredentialRow};

use super::catalog::{ENV_MAPPINGS, is_known_provider};

/// Credential service errors
#[derive(Error, Debug)]
pub enum CredentialError {
    #[error("Credential not found")]
    NotFound,
    #[error("Project not found or does not belong to organization")]
    ProjectNotFound,
    #[error("Invalid provider key: {0}")]
    InvalidProvider(String),
    #[error("Secret store error: {0}")]
    Secret(String),
    #[error("Data error: {0}")]
    Data(#[from] DataError),
}

/// Source of a resolved credential
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CredentialSource {
    Stored,
    /// Static API key read from a named environment variable
    Environment { var_name: String },
    /// Cloud-platform ambient identity (IAM role, workload identity, managed identity).
    /// No static secret — the SDK provider fetches and refreshes tokens internally.
    Ambient { description: String },
}

/// Fully resolved credential (metadata + source, no secret value)
#[derive(Debug, Clone)]
pub struct ResolvedCredential {
    /// UUID for stored; `"env:{VAR_NAME}"` for env-scanned; `"ambient:{cloud}"` for ambient IAM
    pub id: String,
    pub provider_key: String,
    pub display_name: String,
    pub endpoint_url: Option<String>,
    pub extra_config: Option<serde_json::Value>,
    pub key_preview: Option<String>,
    pub source: CredentialSource,
    /// true for read-only credentials (env-scanned or ambient) — cannot be modified or deleted
    pub read_only: bool,
    pub created_at: Option<i64>,
    pub created_by: Option<String>,
}

/// Result of a test-connection attempt
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    pub success: bool,
    pub latency_ms: u64,
    pub error: Option<String>,
    pub model_hint: Option<String>,
}

/// Centralized credential management service
pub struct CredentialService {
    database: Arc<TransactionalService>,
    secrets: SecretManager,
    cache: Arc<CacheService>,
    scan_env: bool,
}

impl CredentialService {
    pub fn new(
        database: Arc<TransactionalService>,
        secrets: SecretManager,
        cache: Arc<CacheService>,
        scan_env: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            database,
            secrets,
            cache,
            scan_env,
        })
    }

    // =========================================================================
    // Resolution
    // =========================================================================

    /// List all credentials accessible by the organization.
    ///
    /// If `project_id` is `Some`, only returns credentials accessible to that project
    /// (based on permission rules). Read-only credentials (env-scanned, ambient) always appear regardless.
    ///
    /// If `project_id` is `None`, returns all stored credentials (admin view).
    pub async fn list_for_org(
        &self,
        org_id: &str,
        project_id: Option<&str>,
    ) -> Result<Vec<ResolvedCredential>, CredentialError> {
        let repo = self.database.repository();

        // Get accessible credential IDs if project filter is specified
        let accessible: Option<HashSet<String>> = if let Some(pid) = project_id {
            let ids = repo
                .get_credentials_accessible_by_project(org_id, pid)
                .await?;
            Some(ids.into_iter().collect())
        } else {
            None
        };

        // Load stored credentials (with caching)
        let stored_rows = self.list_stored_cached(org_id).await?;

        let mut result: Vec<ResolvedCredential> = stored_rows
            .into_iter()
            .filter(|row| {
                accessible
                    .as_ref()
                    .is_none_or(|set| set.contains(&row.id))
            })
            .map(row_to_resolved)
            .collect();

        // Append env-sourced and ambient credentials (always accessible, not filtered by project)
        if self.scan_env {
            result.extend(scan_env_credentials());
            result.extend(detect_ambient_credentials());
        }

        Ok(result)
    }

    /// Get a single resolved credential by ID. Works for stored, env-sourced, and ambient.
    pub async fn get_resolved(
        &self,
        org_id: &str,
        cred_id: &str,
    ) -> Result<Option<ResolvedCredential>, CredentialError> {
        if let Some(var_name) = cred_id.strip_prefix("env:") {
            if !self.scan_env {
                return Ok(None);
            }
            return Ok(env_credential_by_var(var_name));
        }

        if let Some(cloud) = cred_id.strip_prefix("ambient:") {
            if !self.scan_env {
                return Ok(None);
            }
            return Ok(detect_single_ambient(cloud));
        }

        let repo = self.database.repository();
        let row = repo.get_credential(cred_id, org_id).await?;
        Ok(row.map(row_to_resolved))
    }

    /// Get the secret value for a stored credential (with caching).
    ///
    /// - Env credentials: reads directly from the environment variable (always fresh).
    /// - Ambient credentials: returns `None` — the SDK provider fetches tokens internally.
    /// - Stored credentials: cached in the process-local store with a 5-minute TTL.
    pub async fn get_secret(
        &self,
        org_id: &str,
        cred_id: &str,
    ) -> Result<Option<String>, CredentialError> {
        if let Some(var_name) = cred_id.strip_prefix("env:") {
            return Ok(std::env::var(var_name).ok());
        }

        if cred_id.starts_with("ambient:") {
            // No static secret — the SDK provider (AWS SDK / gcp_auth / IMDS) manages
            // token acquisition and refresh entirely. Return None so test_connection.rs
            // routes via auth_mode in extra_config.
            return Ok(None);
        }

        let cache_key = CacheKey::credential_secret(org_id, cred_id);

        // Try process-local cache — secrets never go to Redis
        if let Ok(Some(cached)) = self.cache.get_local::<String>(&cache_key).await {
            return Ok(Some(cached));
        }

        // Load from secrets manager
        let key = SecretKey::new(
            format!("{}{}", CRED_SECRET_PREFIX, cred_id),
            SecretScope::org(org_id),
        );
        let secret = self
            .secrets
            .get_scoped(&key)
            .await
            .map_err(|e| CredentialError::Secret(e.to_string()))?;

        if let Some(ref s) = secret {
            let _ = self
                .cache
                .set_local(
                    &cache_key,
                    &s.value,
                    Some(Duration::from_secs(CACHE_TTL_CRED_SECRET)),
                )
                .await;
        }

        Ok(secret.map(|s| s.value))
    }

    // =========================================================================
    // CRUD
    // =========================================================================

    /// Create a new credential. Atomically: DB row first, then secret.
    /// If secret store fails, DB row is deleted (compensating transaction).
    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        org_id: &str,
        provider_key: &str,
        display_name: &str,
        secret_value: Option<&str>,
        endpoint_url: Option<&str>,
        extra_config: Option<&str>,
        key_preview: Option<&str>,
        created_by: Option<&str>,
    ) -> Result<CredentialRow, CredentialError> {
        if !is_known_provider(provider_key) {
            return Err(CredentialError::InvalidProvider(provider_key.to_string()));
        }

        let id = cuid2::create_id();
        let repo = self.database.repository();

        // 1. Create DB row
        let row = repo
            .create_credential(
                &id,
                org_id,
                provider_key,
                display_name,
                endpoint_url,
                extra_config,
                key_preview,
                created_by,
            )
            .await?;

        // 2. Store secret (if provided)
        if let Some(value) = secret_value {
            let key = SecretKey::new(
                format!("{}{}", CRED_SECRET_PREFIX, id),
                SecretScope::org(org_id),
            );
            if let Err(e) = self.secrets.set_scoped(&key, Secret::new(value)).await {
                // Compensating transaction: delete DB row
                let _ = repo.delete_credential(&id, org_id).await;
                return Err(CredentialError::Secret(e.to_string()));
            }
        }

        // Invalidate list cache
        self.invalidate_list_cache(org_id).await;

        Ok(row)
    }

    /// Update credential metadata (display_name, endpoint_url, extra_config).
    /// Does NOT change the secret.
    pub async fn update(
        &self,
        id: &str,
        org_id: &str,
        display_name: Option<&str>,
        endpoint_url: Option<Option<&str>>,
        extra_config: Option<Option<&str>>,
    ) -> Result<Option<CredentialRow>, CredentialError> {
        let repo = self.database.repository();

        // No-op guard: avoid unnecessary DB write and cache invalidation on empty PATCH
        if display_name.is_none() && endpoint_url.is_none() && extra_config.is_none() {
            return Ok(repo.get_credential(id, org_id).await?);
        }

        let updated = repo
            .update_credential(id, org_id, display_name, endpoint_url, extra_config)
            .await?;

        if updated.is_some() {
            self.invalidate_list_cache(org_id).await;
        }

        Ok(updated)
    }

    /// Delete a credential and its secret (best-effort secret cleanup).
    pub async fn delete(&self, id: &str, org_id: &str) -> Result<bool, CredentialError> {
        let repo = self.database.repository();
        let deleted = repo.delete_credential(id, org_id).await?;

        if deleted {
            // Best-effort secret cleanup (ignore error)
            let key = SecretKey::new(
                format!("{}{}", CRED_SECRET_PREFIX, id),
                SecretScope::org(org_id),
            );
            let _ = self.secrets.delete_scoped(&key).await;

            // Invalidate process-local caches (secrets never went to primary backend)
            self.invalidate_list_cache(org_id).await;
            let secret_cache_key = CacheKey::credential_secret(org_id, id);
            let _ = self.cache.delete_local(&secret_cache_key).await;
        }

        Ok(deleted)
    }

    // =========================================================================
    // Permissions
    // =========================================================================

    /// List permissions for a credential (validates ownership first).
    pub async fn list_permissions(
        &self,
        credential_id: &str,
        org_id: &str,
    ) -> Result<Vec<CredentialPermissionRow>, CredentialError> {
        let repo = self.database.repository();

        // Validate credential belongs to org
        if repo.get_credential(credential_id, org_id).await?.is_none() {
            return Err(CredentialError::NotFound);
        }

        Ok(repo.list_credential_permissions(credential_id).await?)
    }

    /// Create a permission. Validates credential and project ownership.
    pub async fn create_permission(
        &self,
        id: &str,
        credential_id: &str,
        org_id: &str,
        project_id: Option<&str>,
        access: &str,
        created_by: Option<&str>,
    ) -> Result<CredentialPermissionRow, CredentialError> {
        let repo = self.database.repository();

        // Validate credential belongs to org
        if repo.get_credential(credential_id, org_id).await?.is_none() {
            return Err(CredentialError::NotFound);
        }

        // Validate project belongs to org (if specified), using cache to avoid redundant DB hits
        if let Some(pid) = project_id {
            let project = repo.get_project(Some(self.cache.as_ref()), pid).await?;
            match project {
                Some(p) if p.organization_id == org_id => {}
                _ => return Err(CredentialError::ProjectNotFound),
            }
        }

        Ok(repo
            .create_credential_permission(id, credential_id, org_id, project_id, access, created_by)
            .await?)
    }

    /// Delete a permission. Validates credential ownership.
    pub async fn delete_permission(
        &self,
        id: &str,
        credential_id: &str,
        org_id: &str,
    ) -> Result<bool, CredentialError> {
        let repo = self.database.repository();

        // Validate credential belongs to org
        if repo.get_credential(credential_id, org_id).await?.is_none() {
            return Err(CredentialError::NotFound);
        }

        Ok(repo.delete_credential_permission(id, credential_id).await?)
    }

    // =========================================================================
    // Test connection
    // =========================================================================

    /// Test a credential by attempting to reach the provider API.
    pub async fn test(
        &self,
        org_id: &str,
        cred_id: &str,
    ) -> Result<TestResult, CredentialError> {
        let resolved = self
            .get_resolved(org_id, cred_id)
            .await?
            .ok_or(CredentialError::NotFound)?;

        let secret = self.get_secret(org_id, cred_id).await?;

        Ok(super::test_connection::test_credential(&resolved, secret.as_deref()).await)
    }

    // =========================================================================
    // Helpers
    // =========================================================================

    async fn list_stored_cached(
        &self,
        org_id: &str,
    ) -> Result<Vec<CredentialRow>, CredentialError> {
        let cache_key = CacheKey::credentials_for_org(org_id);

        // Credential metadata stays in process-local cache — it contains org structure
        // info that should not be replicated to an external store like Redis.
        if let Ok(Some(cached)) = self.cache.get_local::<Vec<CredentialRow>>(&cache_key).await {
            return Ok(cached);
        }

        let repo = self.database.repository();
        let rows = repo.list_credentials(org_id).await?;

        let _ = self
            .cache
            .set_local(
                &cache_key,
                &rows,
                Some(Duration::from_secs(CACHE_TTL_CRED_LIST)),
            )
            .await;

        Ok(rows)
    }

    async fn invalidate_list_cache(&self, org_id: &str) {
        let cache_key = CacheKey::credentials_for_org(org_id);
        let _ = self.cache.delete_local(&cache_key).await;
    }
}

// =========================================================================
// Helper functions
// =========================================================================

fn row_to_resolved(row: CredentialRow) -> ResolvedCredential {
    let extra_config = row
        .extra_config
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());
    ResolvedCredential {
        id: row.id,
        provider_key: row.provider_key,
        display_name: row.display_name,
        endpoint_url: row.endpoint_url,
        extra_config,
        key_preview: row.key_preview,
        source: CredentialSource::Stored,
        read_only: false,
        created_at: Some(row.created_at),
        created_by: row.created_by,
    }
}

fn scan_env_credentials() -> Vec<ResolvedCredential> {
    let mut result: Vec<ResolvedCredential> = Vec::new();
    let mut seen_providers: HashSet<&str> = HashSet::new();

    for mapping in ENV_MAPPINGS {
        if std::env::var(mapping.var_name).is_err() {
            continue;
        }

        // Dedup: one credential per provider key regardless of how many env vars map to it
        if seen_providers.contains(mapping.provider_key) {
            continue;
        }

        seen_providers.insert(mapping.provider_key);

        result.push(ResolvedCredential {
            id: format!("env:{}", mapping.var_name),
            provider_key: mapping.provider_key.to_string(),
            display_name: mapping.display_name.to_string(),
            endpoint_url: None,
            extra_config: None,
            key_preview: None,
            source: CredentialSource::Environment {
                var_name: mapping.var_name.to_string(),
            },
            read_only: true,
            created_at: None,
            created_by: None,
        });
    }

    result
}

// =========================================================================
// Ambient cloud identity detection
// =========================================================================

/// Detect all ambient cloud credentials available in the current environment.
///
/// Reads only environment variables — no HTTP probes, no I/O. Each cloud's
/// SDK handles actual token acquisition and refresh at use time.
fn detect_ambient_credentials() -> Vec<ResolvedCredential> {
    let mut result = Vec::new();
    if let Some(c) = detect_aws_ambient() {
        result.push(c);
    }
    if let Some(c) = detect_gcp_ambient() {
        result.push(c);
    }
    if let Some(c) = detect_azure_ambient() {
        result.push(c);
    }
    result
}

/// Re-detect a single ambient credential by its cloud suffix (e.g. "aws", "gcp", "azure").
fn detect_single_ambient(cloud: &str) -> Option<ResolvedCredential> {
    match cloud {
        "aws" => detect_aws_ambient(),
        "gcp" => detect_gcp_ambient(),
        "azure" => detect_azure_ambient(),
        _ => None,
    }
}

/// AWS ambient IAM identity.
///
/// Detected from any of: static access key env vars, IRSA/EKS workload identity
/// token file, ECS task role metadata URI, or Lambda/ECS execution environment.
/// The AWS SDK's default credential chain picks up all of these automatically via
/// `BedrockProvider::from_env()` → no special secret needed at test time.
fn detect_aws_ambient() -> Option<ResolvedCredential> {
    const INDICATORS: &[&str] = &[
        "AWS_ACCESS_KEY_ID",
        "AWS_WEB_IDENTITY_TOKEN_FILE",  // IRSA / EKS workload identity
        "AWS_ROLE_ARN",                 // explicit assume-role
        "ECS_CONTAINER_METADATA_URI_V4",
        "ECS_CONTAINER_METADATA_URI",
        "AWS_EXECUTION_ENV",            // Lambda / ECS
        "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
    ];

    if !env_any(INDICATORS) {
        return None;
    }

    let region = std::env::var("AWS_REGION")
        .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
        .unwrap_or_else(|_| "us-east-1".to_string());

    let description = if std::env::var("AWS_WEB_IDENTITY_TOKEN_FILE").is_ok() {
        "IRSA / EKS workload identity"
    } else if std::env::var("ECS_CONTAINER_METADATA_URI_V4").is_ok()
        || std::env::var("ECS_CONTAINER_METADATA_URI").is_ok()
    {
        "ECS task role"
    } else if std::env::var("AWS_ACCESS_KEY_ID").is_ok() {
        "environment credentials (access key)"
    } else {
        "EC2 / Lambda instance profile"
    };

    Some(ResolvedCredential {
        id: "ambient:aws".to_string(),
        provider_key: "bedrock".to_string(),
        display_name: "Amazon Bedrock (ambient IAM)".to_string(),
        endpoint_url: None,
        extra_config: Some(serde_json::json!({
            "auth_mode": "iam_ambient",
            "region": region,
        })),
        key_preview: None,
        source: CredentialSource::Ambient {
            description: description.to_string(),
        },
        read_only: true,
        created_at: None,
        created_by: None,
    })
}

/// GCP Application Default Credentials ambient identity.
///
/// Detected from: GOOGLE_APPLICATION_CREDENTIALS (service account file),
/// Cloud Run / Cloud Functions runtime env vars, or GCP project hints.
/// Token acquisition and refresh are handled by `gcp_auth::provider()` internally.
fn detect_gcp_ambient() -> Option<ResolvedCredential> {
    const INDICATORS: &[&str] = &[
        "GOOGLE_APPLICATION_CREDENTIALS",
        "GOOGLE_CLOUD_PROJECT",
        "GCLOUD_PROJECT",
        "GCP_PROJECT",
        "K_SERVICE",        // Cloud Run service
        "K_REVISION",       // Cloud Run revision
        "CLOUD_RUN_JOB",    // Cloud Run Jobs
        "FUNCTION_TARGET",  // Cloud Functions (2nd gen)
    ];

    if !env_any(INDICATORS) {
        return None;
    }

    // project_id is required by Vertex AI — skip if not determinable from env
    let project_id = std::env::var("GOOGLE_CLOUD_PROJECT")
        .or_else(|_| std::env::var("GCLOUD_PROJECT"))
        .or_else(|_| std::env::var("GCP_PROJECT"))
        .ok()?;

    let description = if std::env::var("GOOGLE_APPLICATION_CREDENTIALS").is_ok() {
        "service account file (GOOGLE_APPLICATION_CREDENTIALS)"
    } else if std::env::var("K_SERVICE").is_ok() || std::env::var("K_REVISION").is_ok() {
        "Cloud Run workload identity"
    } else if std::env::var("CLOUD_RUN_JOB").is_ok() {
        "Cloud Run Jobs workload identity"
    } else if std::env::var("FUNCTION_TARGET").is_ok() {
        "Cloud Functions workload identity"
    } else {
        "GCE instance identity"
    };

    let extra = serde_json::json!({
        "auth_mode": "adc",
        "location": "us-central1",
        "project_id": project_id,
    });

    Some(ResolvedCredential {
        id: "ambient:gcp".to_string(),
        provider_key: "vertex-ai".to_string(),
        display_name: "Google Vertex AI (ambient ADC)".to_string(),
        endpoint_url: None,
        extra_config: Some(extra),
        key_preview: None,
        source: CredentialSource::Ambient {
            description: description.to_string(),
        },
        read_only: true,
        created_at: None,
        created_by: None,
    })
}

/// Azure Managed Identity / Workload Identity ambient credential.
///
/// Detected from: AKS workload identity env vars, legacy MSI endpoint,
/// newer IDENTITY_ENDPOINT, or AZURE_CLIENT_ID (user-assigned MI).
/// Token is fetched live via IMDS or the workload identity OIDC endpoint.
fn detect_azure_ambient() -> Option<ResolvedCredential> {
    const INDICATORS: &[&str] = &[
        "AZURE_CLIENT_ID",          // user-assigned MI or workload identity client
        "AZURE_FEDERATED_TOKEN_FILE", // AKS workload identity federation
        "MSI_ENDPOINT",             // App Service / Azure Functions (legacy)
        "IDENTITY_ENDPOINT",        // App Service / Container Apps (newer)
        "IMDS_ENDPOINT",            // IMDS endpoint override (Azure Arc etc.)
    ];

    if !env_any(INDICATORS) {
        return None;
    }

    // Without an endpoint the credential is unusable for Azure AI Foundry
    let endpoint_url = std::env::var("AZURE_OPENAI_ENDPOINT")
        .or_else(|_| std::env::var("AZURE_OPENAI_ENDPOINT_URL"))
        .ok()?;

    let description = if std::env::var("AZURE_FEDERATED_TOKEN_FILE").is_ok() {
        "AKS workload identity federation"
    } else if std::env::var("MSI_ENDPOINT").is_ok() || std::env::var("IDENTITY_ENDPOINT").is_ok()
    {
        "Azure Managed Identity endpoint"
    } else if std::env::var("AZURE_CLIENT_ID").is_ok() {
        "Azure Managed Identity (user-assigned)"
    } else {
        "Azure Managed Identity (system-assigned)"
    };

    Some(ResolvedCredential {
        id: "ambient:azure".to_string(),
        provider_key: "azure-ai-foundry".to_string(),
        display_name: "Azure AI Foundry (ambient MI)".to_string(),
        endpoint_url: Some(endpoint_url),
        extra_config: Some(serde_json::json!({
            "auth_mode": "managed_identity",
            // Modern Foundry resources expose the /openai/v1/ path which does not
            // require a deployment name baked into the URL. The deployment is passed
            // as the model field in the request body instead.
            "api_variant": "v1",
        })),
        key_preview: None,
        source: CredentialSource::Ambient {
            description: description.to_string(),
        },
        read_only: true,
        created_at: None,
        created_by: None,
    })
}

/// Returns true if any of the given environment variable names are set.
fn env_any(vars: &[&str]) -> bool {
    vars.iter().any(|v| std::env::var(v).is_ok())
}

fn env_credential_by_var(var_name: &str) -> Option<ResolvedCredential> {
    if std::env::var(var_name).is_err() {
        return None;
    }
    let mapping = ENV_MAPPINGS.iter().find(|m| m.var_name == var_name)?;
    Some(ResolvedCredential {
        id: format!("env:{}", var_name),
        provider_key: mapping.provider_key.to_string(),
        display_name: mapping.display_name.to_string(),
        endpoint_url: None,
        extra_config: None,
        key_preview: None,
        source: CredentialSource::Environment {
            var_name: var_name.to_string(),
        },
        read_only: true,
        created_at: None,
        created_by: None,
    })
}
