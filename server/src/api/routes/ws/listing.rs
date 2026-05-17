//! HTTP read-only listing endpoint at
//! `GET /api/v1/project/{project_id}/registrations`.

use axum::Json;
use axum::extract::{Path, State};
use serde::{Deserialize, Serialize};

use crate::api::extractors::is_valid_project_id;
use crate::api::types::ApiError;
use crate::data::registrations::{RegistrationEntry, RegistrationKind};

use super::state::WsState;

#[derive(Debug, Deserialize)]
pub struct ProjectPath {
    pub project_id: String,
}

#[derive(Debug, Serialize)]
pub struct ListingResponse {
    pub agents: Vec<RegistrationEntry>,
    pub mcps: Vec<RegistrationEntry>,
    pub swarms: Vec<RegistrationEntry>,
    pub graphs: Vec<RegistrationEntry>,
}

pub async fn list_registrations(
    State(state): State<WsState>,
    Path(ProjectPath { project_id }): Path<ProjectPath>,
) -> Result<Json<ListingResponse>, ApiError> {
    if !is_valid_project_id(&project_id) {
        return Err(ApiError::bad_request(
            "invalid_project_id",
            "project_id has invalid characters or length",
        ));
    }
    let entries = state
        .registrations
        .list(&project_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let mut agents = Vec::new();
    let mut mcps = Vec::new();
    let mut swarms = Vec::new();
    let mut graphs = Vec::new();
    for e in entries {
        match e.kind {
            RegistrationKind::Agent => agents.push(e),
            RegistrationKind::Mcp => mcps.push(e),
            RegistrationKind::Swarm => swarms.push(e),
            RegistrationKind::Graph => graphs.push(e),
        }
    }
    Ok(Json(ListingResponse {
        agents,
        mcps,
        swarms,
        graphs,
    }))
}
