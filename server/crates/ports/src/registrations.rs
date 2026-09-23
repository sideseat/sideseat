//! Port and transport-neutral values for SDK agent/MCP presence.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::types::ProjectId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrationKind {
    Agent,
    Mcp,
    Swarm,
    Graph,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistrationManifest {
    pub name: String,
    #[serde(default)]
    pub framework: Option<String>,
    #[serde(default)]
    pub runtime: Option<serde_json::Value>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub tools: Vec<serde_json::Value>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistrationEntry {
    pub project_id: ProjectId,
    pub kind: RegistrationKind,
    pub name: String,
    pub manifest: RegistrationManifest,
    pub owner_client_id: String,
    pub owner_connection_id: String,
    pub owning_instance_id: String,
    pub last_heartbeat_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplacedOwner {
    pub client_id: String,
    pub instance_id: String,
}

#[derive(Debug, Clone)]
pub enum UpsertOutcome {
    Inserted,
    UpdatedSameOwner,
    Replaced(DisplacedOwner),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum PresenceEvent {
    Registered(RegistrationEntry),
    Replaced {
        project_id: ProjectId,
        kind: RegistrationKind,
        name: String,
        prev_owner: DisplacedOwner,
        new_owner: DisplacedOwner,
    },
    Unregistered {
        project_id: ProjectId,
        kind: RegistrationKind,
        name: String,
        owner: DisplacedOwner,
    },
    Expired {
        project_id: ProjectId,
        kind: RegistrationKind,
        name: String,
        owner: DisplacedOwner,
    },
}

impl PresenceEvent {
    #[must_use]
    pub fn project_id(&self) -> &ProjectId {
        match self {
            Self::Registered(entry) => &entry.project_id,
            Self::Replaced { project_id, .. }
            | Self::Unregistered { project_id, .. }
            | Self::Expired { project_id, .. } => project_id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum ConnectionControl {
    Replaced {
        project_id: ProjectId,
        target_client_id: String,
        kind: RegistrationKind,
        name: String,
    },
    Invoke {
        project_id: ProjectId,
        target_client_id: String,
        request_id: String,
        agent_name: String,
        run_input: serde_json::Value,
    },
    Cancel {
        project_id: ProjectId,
        target_client_id: String,
        request_id: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum RegistrationStoreError {
    #[error("backend error: {0}")]
    Backend(String),
}

#[async_trait]
pub trait RegistrationStore: Send + Sync + 'static {
    async fn upsert(
        &self,
        entry: RegistrationEntry,
    ) -> Result<UpsertOutcome, RegistrationStoreError>;

    async fn remove(
        &self,
        project_id: &ProjectId,
        kind: RegistrationKind,
        name: &str,
        by_client_id: &str,
    ) -> Result<Option<RegistrationEntry>, RegistrationStoreError>;

    async fn list(
        &self,
        project_id: &ProjectId,
    ) -> Result<Vec<RegistrationEntry>, RegistrationStoreError>;

    async fn find(
        &self,
        project_id: &ProjectId,
        kind: RegistrationKind,
        name: &str,
    ) -> Result<Option<RegistrationEntry>, RegistrationStoreError>;

    async fn find_by_name(
        &self,
        project_id: &ProjectId,
        name: &str,
    ) -> Result<Option<RegistrationEntry>, RegistrationStoreError> {
        for kind in [
            RegistrationKind::Agent,
            RegistrationKind::Graph,
            RegistrationKind::Swarm,
        ] {
            if let Some(entry) = self.find(project_id, kind, name).await? {
                return Ok(Some(entry));
            }
        }
        Ok(None)
    }

    async fn remove_all_for_connection(
        &self,
        connection_id: &str,
    ) -> Result<Vec<RegistrationEntry>, RegistrationStoreError>;

    async fn touch(&self, client_id: &str, now_secs: u64) -> Result<(), RegistrationStoreError>;

    async fn expire_due(
        &self,
        now_secs: u64,
        ttl_secs: u64,
    ) -> Result<Vec<RegistrationEntry>, RegistrationStoreError>;
}
