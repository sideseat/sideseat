//! Pricing API endpoints

use std::sync::Arc;

use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

use crate::api::extractors::ValidatedJson;
use crate::api::types::ApiError;
use crate::domain::pricing::{MatchType, ModelPricing, PricingService, SpanCostInput};

// ============================================================================
// State
// ============================================================================

#[derive(Clone)]
pub struct PricingApiState {
    pub pricing: Arc<PricingService>,
}

// ============================================================================
// Request/Response DTOs
// ============================================================================

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CalculateCostRequest {
    #[validate(length(min = 1, max = 256))]
    pub model: String,
    pub provider: Option<String>,
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(default)]
    pub cache_read_tokens: i64,
    #[serde(default)]
    pub cache_write_tokens: i64,
    #[serde(default)]
    pub reasoning_tokens: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CalculateCostResponse {
    pub input_cost: f64,
    pub output_cost: f64,
    pub cache_read_cost: f64,
    pub cache_write_cost: f64,
    pub reasoning_cost: f64,
    pub total_cost: f64,
    pub match_type: MatchType,
    pub confidence: f64,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct ModelPricingRequest {
    #[validate(length(min = 1, max = 256))]
    pub model: String,
    pub provider: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ModelPricingResponse {
    pub model: String,
    pub provider: Option<String>,
    pub input_cost_per_token: f64,
    pub output_cost_per_token: f64,
    pub cache_read_input_token_cost: f64,
    pub cache_creation_input_token_cost: f64,
    pub output_cost_per_reasoning_token: f64,
    pub mode: String,
    pub match_type: MatchType,
    pub confidence: f64,
}

impl ModelPricingResponse {
    fn from_pricing(
        model: String,
        provider: Option<String>,
        pricing: ModelPricing,
        match_type: MatchType,
    ) -> Self {
        Self {
            model,
            provider,
            input_cost_per_token: pricing.input_cost_per_token,
            output_cost_per_token: pricing.output_cost_per_token,
            cache_read_input_token_cost: pricing.cache_read_input_token_cost,
            cache_creation_input_token_cost: pricing.cache_creation_input_token_cost,
            output_cost_per_reasoning_token: pricing.output_cost_per_reasoning_token,
            mode: pricing.mode,
            match_type,
            confidence: match_type.confidence(),
        }
    }
}

// ============================================================================
// Routes
// ============================================================================

pub fn routes(pricing: Arc<PricingService>) -> Router<()> {
    let state = PricingApiState { pricing };
    Router::new()
        .route("/calculate", post(calculate_cost))
        .route("/models", post(get_model_pricing))
        .with_state(state)
}

// ============================================================================
// Handlers
// ============================================================================

/// Calculate cost for a given model and token usage
#[utoipa::path(
    post,
    path = "/api/v1/pricing/calculate",
    tag = "pricing",
    request_body = CalculateCostRequest,
    responses(
        (status = 200, description = "Calculated costs", body = CalculateCostResponse)
    )
)]
pub async fn calculate_cost(
    State(state): State<PricingApiState>,
    ValidatedJson(req): ValidatedJson<CalculateCostRequest>,
) -> Result<Json<CalculateCostResponse>, ApiError> {
    let input = SpanCostInput {
        system: req.provider,
        model: Some(req.model),
        input_tokens: req.input_tokens,
        output_tokens: req.output_tokens,
        total_tokens: req.input_tokens + req.output_tokens,
        cache_read_tokens: req.cache_read_tokens,
        cache_write_tokens: req.cache_write_tokens,
        reasoning_tokens: req.reasoning_tokens,
    };

    let output = state.pricing.calculate_cost(&input);

    Ok(Json(CalculateCostResponse {
        input_cost: output.input_cost,
        output_cost: output.output_cost,
        cache_read_cost: output.cache_read_cost,
        cache_write_cost: output.cache_write_cost,
        reasoning_cost: output.reasoning_cost,
        total_cost: output.total_cost,
        match_type: output.match_type.unwrap_or_default(),
        confidence: output.confidence(),
    }))
}

/// Get pricing information for a model
#[utoipa::path(
    post,
    path = "/api/v1/pricing/models",
    tag = "pricing",
    request_body = ModelPricingRequest,
    responses(
        (status = 200, description = "Model pricing information", body = ModelPricingResponse),
        (status = 404, description = "Model not found")
    )
)]
pub async fn get_model_pricing(
    State(state): State<PricingApiState>,
    ValidatedJson(req): ValidatedJson<ModelPricingRequest>,
) -> Result<Json<ModelPricingResponse>, ApiError> {
    let result = state
        .pricing
        .get_model_pricing(req.provider.as_deref(), &req.model);

    match result {
        Some((pricing, match_type)) => Ok(Json(ModelPricingResponse::from_pricing(
            req.model,
            req.provider,
            pricing,
            match_type,
        ))),
        None => Err(ApiError::not_found(
            "MODEL_NOT_FOUND",
            format!("Model not found: {}", req.model),
        )),
    }
}
