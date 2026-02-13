//! Favorites API types

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::core::constants::MAX_CHECK_BATCH;

/// Entity type for favorites (trace, session, span)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    Trace,
    Session,
    Span,
}

impl EntityType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Session => "session",
            Self::Span => "span",
        }
    }
}

/// Span identifier (composite key)
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct SpanIdentifier {
    pub trace_id: String,
    pub span_id: String,
}

/// Request body for batch checking favorites
#[derive(Debug, Deserialize, ToSchema)]
pub struct CheckFavoritesRequest {
    /// Entity type to check
    pub entity_type: EntityType,

    /// IDs to check (for trace/session) - max 500
    pub ids: Option<Vec<String>>,

    /// Span identifiers to check (for spans) - max 500
    pub spans: Option<Vec<SpanIdentifier>>,
}

impl CheckFavoritesRequest {
    /// Validate fields based on entity_type and batch limits
    pub fn validate(&self) -> Result<(), &'static str> {
        match self.entity_type {
            EntityType::Trace | EntityType::Session => {
                if self.ids.is_none() || self.ids.as_ref().is_some_and(|v| v.is_empty()) {
                    return Err("ids field is required for trace/session entity types");
                }
                if self.spans.is_some() {
                    return Err("spans field is not allowed for trace/session entity types");
                }
                if self
                    .ids
                    .as_ref()
                    .is_some_and(|ids| ids.len() > MAX_CHECK_BATCH)
                {
                    return Err("Maximum 500 IDs allowed per request");
                }
            }
            EntityType::Span => {
                if self.spans.is_none() || self.spans.as_ref().is_some_and(|v| v.is_empty()) {
                    return Err("spans field is required for span entity type");
                }
                if self.ids.is_some() {
                    return Err("ids field is not allowed for span entity type");
                }
                if self
                    .spans
                    .as_ref()
                    .is_some_and(|spans| spans.len() > MAX_CHECK_BATCH)
                {
                    return Err("Maximum 500 spans allowed per request");
                }
            }
        }
        Ok(())
    }
}

/// Response for batch check favorites
#[derive(Debug, Serialize, ToSchema)]
pub struct CheckFavoritesResponse {
    /// IDs that are favorited
    pub favorites: Vec<String>,
}

/// Response for listing favorite IDs
#[derive(Debug, Serialize, ToSchema)]
pub struct ListFavoritesResponse {
    /// Favorite entity IDs (limited to MAX_FAVORITES_PER_PROJECT)
    pub favorites: Vec<String>,
}

/// Response for add favorite operation
#[derive(Debug, Serialize, ToSchema)]
pub struct AddFavoriteResponse {
    /// Whether the favorite was newly created (vs already existed)
    pub created: bool,
}

/// Response for remove favorite operation
#[derive(Debug, Serialize, ToSchema)]
pub struct RemoveFavoriteResponse {
    /// Whether a favorite was actually removed (vs didn't exist)
    pub removed: bool,
}
