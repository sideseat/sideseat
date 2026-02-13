//! OpenAPI specification and Swagger UI

use axum::http::header;
use axum::response::{Html, IntoResponse, Json};
use utoipa::OpenApi;

use crate::api::routes::{
    api_keys, auth, favorites, health, organizations, otel, pricing, projects, users,
};
use crate::api::types::{OrderDirection, PaginationMeta};
use crate::data::types::ApiKeyScope;
use crate::domain::sideml::{
    CacheControl, ChatMessage, ChatRole, ContentBlock, FinishReason, JsonSchemaDetails,
    ResponseFormat, ToolChoice,
};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "SideSeat API",
        version = env!("CARGO_PKG_VERSION"),
        description = "AI Development Workbench"
    ),
    tags(
        (name = "health", description = "Health check endpoint"),
        (name = "auth", description = "Authentication endpoints"),
        (name = "organizations", description = "Organization management"),
        (name = "users", description = "User management"),
        (name = "projects", description = "Project management"),
        (name = "pricing", description = "Pricing calculation"),
        (name = "traces", description = "Trace queries"),
        (name = "spans", description = "Span queries"),
        (name = "sessions", description = "Session queries"),
        (name = "stats", description = "Project statistics"),
        (name = "favorites", description = "User favorites"),
        (name = "files", description = "File storage"),
        (name = "feed", description = "Project-wide activity feed"),
        (name = "api-keys", description = "API key management")
    ),
    paths(
        // Health
        health::health,
        // Auth
        auth::exchange_token,
        auth::auth_status,
        auth::logout,
        // Organizations
        organizations::list_organizations,
        organizations::create_org,
        organizations::get_org,
        organizations::update_org,
        organizations::delete_org,
        organizations::list_org_members,
        organizations::add_org_member,
        organizations::update_member_role,
        organizations::remove_org_member,
        // Users
        users::get_current_user,
        users::update_current_user,
        // Projects
        projects::list_projects,
        projects::create_project,
        projects::get_project,
        projects::update_project,
        projects::delete_project,
        // Pricing
        pricing::calculate_cost,
        pricing::get_model_pricing,
        // Traces
        otel::traces::list_traces,
        otel::traces::get_trace,
        otel::traces::delete_traces,
        otel::traces::get_trace_filter_options,
        otel::messages::get_trace_messages,
        // Spans
        otel::spans::list_spans,
        otel::spans::list_trace_spans,
        otel::spans::get_span,
        otel::spans::delete_spans,
        otel::spans::get_span_filter_options,
        otel::messages::get_span_messages,
        // Sessions
        otel::sessions::list_sessions,
        otel::sessions::get_session,
        otel::sessions::delete_sessions,
        otel::messages::get_session_messages,
        otel::sessions::get_session_filter_options,
        // Stats
        otel::stats::get_project_stats,
        // Feed
        otel::feed::get_feed_messages,
        otel::feed::get_feed_spans,
        // Favorites
        favorites::add_favorite_simple,
        favorites::add_favorite_composite,
        favorites::remove_favorite_simple,
        favorites::remove_favorite_composite,
        favorites::check_favorites,
        favorites::list_favorites,
        // Files
        otel::files::get_file,
        otel::files::head_file,
        // API Keys
        api_keys::create_api_key,
        api_keys::list_api_keys,
        api_keys::delete_api_key,
    ),
    components(schemas(
        // API types
        PaginationMeta,
        OrderDirection,
        // Health
        health::HealthResponse,
        // Auth
        auth::ExchangeRequest,
        auth::ExchangeResponse,
        auth::AuthStatusResponse,
        // Organizations
        organizations::types::OrganizationDto,
        organizations::types::OrgWithRoleDto,
        organizations::types::MemberDto,
        organizations::types::CreateOrgRequest,
        organizations::types::UpdateOrgRequest,
        organizations::types::AddMemberRequest,
        organizations::types::UpdateMemberRoleRequest,
        organizations::types::ListOrgsQuery,
        organizations::types::ListMembersQuery,
        // Users
        users::types::UserDto,
        users::types::UserOrgDto,
        users::types::UserProfileResponse,
        users::types::UpdateUserRequest,
        // Projects
        projects::types::ProjectDto,
        projects::types::CreateProjectRequest,
        projects::types::UpdateProjectRequest,
        projects::types::ListProjectsQuery,
        // OTEL types
        otel::types::TraceSummaryDto,
        otel::types::TraceDetailDto,
        otel::types::SpanSummaryDto,
        otel::types::SpanDetailDto,
        otel::types::SessionSummaryDto,
        otel::types::SessionDetailDto,
        otel::types::TraceInSessionDto,
        otel::types::BlockDto,
        otel::types::MessagesResponseDto,
        otel::types::MessagesMetadataDto,
        // Trace types
        otel::traces::DeleteTracesBody,
        otel::traces::FilterOptionsResponse,
        otel::traces::FilterOptionDto,
        // Session types
        otel::sessions::DeleteSessionsBody,
        // Stats types
        otel::types::ProjectStatsDto,
        otel::types::PeriodDto,
        otel::types::CountsDto,
        otel::types::CostsDto,
        otel::types::TokensDto,
        otel::types::FrameworkBreakdownDto,
        otel::types::ModelBreakdownDto,
        otel::types::TrendBucketDto,
        otel::types::LatencyBucketDto,
        // Feed types
        otel::types::FeedPagination,
        otel::types::FeedMessagesMetadata,
        otel::types::FeedMessagesResponse,
        otel::types::FeedSpansResponse,
        // Span types
        otel::spans::DeleteSpansBody,
        otel::spans::SpanIdentifier,
        // Favorites types
        favorites::types::EntityType,
        favorites::types::SpanIdentifier,
        favorites::types::CheckFavoritesRequest,
        favorites::types::CheckFavoritesResponse,
        favorites::types::ListFavoritesResponse,
        // Pricing types
        pricing::CalculateCostRequest,
        pricing::CalculateCostResponse,
        pricing::ModelPricingRequest,
        pricing::ModelPricingResponse,
        crate::domain::pricing::MatchType,
        // API Keys types
        ApiKeyScope,
        api_keys::types::CreateApiKeyRequest,
        api_keys::types::CreateApiKeyResponse,
        api_keys::types::ApiKeyDto,
        // SideML types
        ChatRole,
        ContentBlock,
        FinishReason,
        ToolChoice,
        ResponseFormat,
        JsonSchemaDetails,
        CacheControl,
        ChatMessage,
    ))
)]
pub struct ApiDoc;

/// Serve OpenAPI JSON specification
pub async fn openapi_json() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/json")],
        Json(ApiDoc::openapi()),
    )
}

/// Serve Swagger UI from CDN
pub async fn swagger_ui_html() -> Html<&'static str> {
    Html(SWAGGER_UI_HTML)
}

const SWAGGER_UI_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>SideSeat API Documentation</title>
    <link rel="stylesheet" type="text/css" href="https://unpkg.com/swagger-ui-dist@5/swagger-ui.css">
    <style>
        html { box-sizing: border-box; overflow-y: scroll; }
        *, *:before, *:after { box-sizing: inherit; }
        body { margin: 0; background: #fafafa; }
    </style>
</head>
<body>
    <div id="swagger-ui"></div>
    <script src="https://unpkg.com/swagger-ui-dist@5/swagger-ui-bundle.js"></script>
    <script src="https://unpkg.com/swagger-ui-dist@5/swagger-ui-standalone-preset.js"></script>
    <script>
        window.onload = () => {
            window.ui = SwaggerUIBundle({
                url: "/api/openapi.json",
                dom_id: '#swagger-ui',
                presets: [
                    SwaggerUIBundle.presets.apis,
                    SwaggerUIStandalonePreset
                ],
                layout: "StandaloneLayout",
                deepLinking: true,
                showExtensions: true,
                showCommonExtensions: true
            });
        };
    </script>
</body>
</html>"#;
