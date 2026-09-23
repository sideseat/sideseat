//! HTTP read-only listing endpoint at
//! `GET /api/v1/project/{project_id}/registrations`.

use axum::Json;
use axum::extract::State;
use serde::Serialize;

use crate::auth::ProjectRead;
use crate::types::ApiError;
use sideseat_ports::registrations::{RegistrationEntry, RegistrationKind};

use super::state::WsState;

#[derive(Debug, Serialize)]
pub struct ListingResponse {
    pub agents: Vec<RegistrationEntry>,
    pub mcps: Vec<RegistrationEntry>,
    pub swarms: Vec<RegistrationEntry>,
    pub graphs: Vec<RegistrationEntry>,
}

impl ListingResponse {
    pub(super) fn from_entries(entries: impl IntoIterator<Item = RegistrationEntry>) -> Self {
        let mut response = Self {
            agents: Vec::new(),
            mcps: Vec::new(),
            swarms: Vec::new(),
            graphs: Vec::new(),
        };
        for entry in entries {
            match entry.kind {
                RegistrationKind::Agent => response.agents.push(entry),
                RegistrationKind::Mcp => response.mcps.push(entry),
                RegistrationKind::Swarm => response.swarms.push(entry),
                RegistrationKind::Graph => response.graphs.push(entry),
            }
        }
        response
            .agents
            .sort_by(|left, right| left.name.cmp(&right.name));
        response
            .mcps
            .sort_by(|left, right| left.name.cmp(&right.name));
        response
            .swarms
            .sort_by(|left, right| left.name.cmp(&right.name));
        response
            .graphs
            .sort_by(|left, right| left.name.cmp(&right.name));
        response
    }
}

pub async fn list_registrations(
    State(state): State<WsState>,
    access: ProjectRead,
) -> Result<Json<ListingResponse>, ApiError> {
    let entries = state
        .registrations
        .list(&access.project_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    Ok(Json(ListingResponse::from_entries(entries)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sideseat_ports::registrations::RegistrationManifest;

    fn entry(kind: RegistrationKind, name: &str) -> RegistrationEntry {
        RegistrationEntry {
            project_id: "project".into(),
            kind,
            name: name.into(),
            manifest: RegistrationManifest {
                name: name.into(),
                framework: None,
                runtime: None,
                model: None,
                system_prompt: None,
                tools: Vec::new(),
                metadata: serde_json::Value::Null,
            },
            owner_client_id: "client".into(),
            owner_connection_id: "connection".into(),
            owning_instance_id: "instance".into(),
            last_heartbeat_secs: 1,
        }
    }

    #[test]
    fn listing_groups_kinds_and_sorts_each_bucket() {
        let response = ListingResponse::from_entries([
            entry(RegistrationKind::Agent, "zeta"),
            entry(RegistrationKind::Graph, "graph"),
            entry(RegistrationKind::Agent, "alpha"),
            entry(RegistrationKind::Mcp, "mcp"),
            entry(RegistrationKind::Swarm, "swarm"),
        ]);

        assert_eq!(
            response
                .agents
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "zeta"]
        );
        assert_eq!(response.mcps[0].name, "mcp");
        assert_eq!(response.swarms[0].name, "swarm");
        assert_eq!(response.graphs[0].name, "graph");
    }
}
