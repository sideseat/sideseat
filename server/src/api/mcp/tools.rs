use std::sync::Arc;

use chrono::{DateTime, Utc};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo, ToolsCapability,
};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};

use crate::api::routes::otel::messages::{build_messages_response, scope_feed_to_trace};
use crate::api::routes::otel::sessions::session_row_to_summary;
use crate::api::routes::otel::stats::stats_result_to_dto;
use crate::api::routes::otel::traces::{MAX_SPANS_PER_TRACE, trace_row_to_summary};
use crate::api::routes::otel::types::{
    SessionSummaryDto, SpanDetailDto, SpanSummaryDto, TraceDetailDto, TraceSummaryDto,
};
use crate::api::types::{MAX_PAGE_LIMIT, OrderBy, OrderDirection};
use crate::data::AnalyticsService;
use crate::data::traits::AnalyticsRepository;
use crate::data::types::{
    ListSessionsParams, ListSpansParams, ListTracesParams, MessageQueryParams, SpanRow, StatsParams,
};
use crate::domain::sideml::{FeedOptions, extract_tools_from_rows, process_spans};

use super::types::*;

type McpError = rmcp::model::ErrorData;

#[derive(Clone)]
pub struct McpServer {
    analytics: Arc<AnalyticsService>,
    project_id: String,
    tool_router: ToolRouter<Self>,
}

impl McpServer {
    pub fn new(analytics: Arc<AnalyticsService>, project_id: String) -> Self {
        Self {
            analytics,
            project_id,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_handler]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(INSTRUCTIONS.to_string()),
            capabilities: ServerCapabilities {
                tools: Some(ToolsCapability::default()),
                ..Default::default()
            },
            server_info: Implementation {
                name: "SideSeat".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

const INSTRUCTIONS: &str = r#"SideSeat AI Observability - query LLM traces, conversations, and performance data.

WORKFLOW for prompt optimization:
1. list_traces to find relevant traces (filter by session, time, errors)
2. get_messages with trace_id to see the full conversation
3. get_stats for cost/token/latency analysis
4. list_spans with observation_type=Generation for specific LLM calls
5. get_raw_span for raw OTLP data when debugging

KEY CONCEPTS:
- Trace: one end-to-end AI operation (may contain multiple LLM calls)
- Span: single operation within a trace (Generation=LLM call, Tool=tool exec, Agent=agent step)
- Session: multi-turn conversation spanning multiple traces
- Messages: normalized conversation with roles: system, user, assistant, tool

TIPS:
- Start with list_traces(limit=5) for recent activity
- get_messages shows exactly what prompts were sent and responses received
- Filter list_spans by model/framework to compare across providers
- get_stats shows cost breakdown by model for optimization decisions"#;

#[tool_router]
impl McpServer {
    #[tool(
        description = "List recent AI traces. Returns trace name, duration, tokens, costs, I/O previews, error status."
    )]
    async fn list_traces(
        &self,
        Parameters(input): Parameters<ListTracesInput>,
    ) -> Result<CallToolResult, McpError> {
        let repo = self.analytics.repository();
        let params = ListTracesParams {
            project_id: self.project_id.clone(),
            page: clamp_page(input.page),
            limit: clamp_limit(input.limit),
            order_by: Some(OrderBy {
                column: "start_time".into(),
                direction: OrderDirection::Desc,
            }),
            session_id: input.session_id,
            environment: input.environment.map(|e| vec![e]),
            from_timestamp: parse_opt_ts(input.from_timestamp),
            to_timestamp: parse_opt_ts(input.to_timestamp),
            ..Default::default()
        };
        let (rows, total) = repo.list_traces(&params).await.map_err(mcp_err)?;
        let traces: Vec<TraceSummaryDto> = rows.into_iter().map(trace_row_to_summary).collect();
        ok_json(&serde_json::json!({ "traces": traces, "total": total }))
    }

    #[tool(
        description = "Get trace execution structure: span tree with agent steps, LLM calls, tool invocations, timing, models, tokens."
    )]
    async fn get_trace(
        &self,
        Parameters(input): Parameters<GetTraceInput>,
    ) -> Result<CallToolResult, McpError> {
        let repo = self.analytics.repository();
        let trace = repo
            .get_trace(&self.project_id, &input.trace_id)
            .await
            .map_err(mcp_err)?
            .ok_or_else(|| McpError::invalid_params("trace not found", None))?;

        let spans = repo
            .get_spans_for_trace(&self.project_id, &input.trace_id)
            .await
            .map_err(mcp_err)?;

        let span_details: Vec<SpanDetailDto> = spans_to_dtos(
            &*repo,
            &self.project_id,
            &spans[..spans.len().min(MAX_SPANS_PER_TRACE)],
            false,
        )
        .await?
        .into_iter()
        .map(|summary| SpanDetailDto { summary })
        .collect();

        let summary = trace_row_to_summary(trace);
        ok_json(&TraceDetailDto {
            summary,
            spans: span_details,
        })
    }

    #[tool(
        description = "Get normalized LLM conversation. Returns messages with roles (system/user/assistant/tool), content blocks (text, tool_use, tool_result, thinking), tokens, costs. Provide one of: trace_id, span_id, or session_id."
    )]
    async fn get_messages(
        &self,
        Parameters(input): Parameters<GetMessagesInput>,
    ) -> Result<CallToolResult, McpError> {
        let repo = self.analytics.repository();
        let options = FeedOptions::new().with_role(input.role);

        // Simple path: span or session scoped (no cross-trace dedup needed)
        if input.span_id.is_some() || input.session_id.is_some() {
            let params = MessageQueryParams {
                project_id: self.project_id.clone(),
                span_id: input.span_id,
                session_id: input.session_id,
                ..Default::default()
            };
            let result = repo.get_messages(&params).await.map_err(mcp_err)?;
            let processed = process_spans(result.rows, &options);
            return ok_json(&build_messages_response(processed, None));
        }

        // Trace path: session-aware loading for cross-trace dedup
        let trace_id = input.trace_id.ok_or_else(|| {
            McpError::invalid_params("provide trace_id, span_id, or session_id", None)
        })?;

        let trace = repo
            .get_trace(&self.project_id, &trace_id)
            .await
            .map_err(mcp_err)?;
        let session_id = trace
            .as_ref()
            .and_then(|t| t.session_id.as_ref())
            .filter(|s| !s.is_empty());

        let params = MessageQueryParams {
            project_id: self.project_id.clone(),
            session_id: session_id.map(|s| s.to_string()),
            trace_id: if session_id.is_none() {
                Some(trace_id.clone())
            } else {
                None
            },
            ..Default::default()
        };
        let result = repo.get_messages(&params).await.map_err(mcp_err)?;

        let scoped_tools = session_id.map(|_| {
            extract_tools_from_rows(result.rows.iter().filter(|r| r.trace_id == trace_id))
        });

        let mut processed = process_spans(result.rows, &options);

        if let Some(scoped_tools) = scoped_tools {
            scope_feed_to_trace(&mut processed, scoped_tools, &trace_id);
        }

        let trace_totals = trace.map(|t| (t.total_tokens, t.total_cost));
        ok_json(&build_messages_response(processed, trace_totals))
    }

    #[tool(
        description = "Search operations across traces. Filter by observation_type (Generation=LLM, Tool=tool exec, Agent=agent step), model, framework, error status."
    )]
    async fn list_spans(
        &self,
        Parameters(input): Parameters<ListSpansInput>,
    ) -> Result<CallToolResult, McpError> {
        let repo = self.analytics.repository();
        let params = ListSpansParams {
            project_id: self.project_id.clone(),
            page: clamp_page(input.page),
            limit: clamp_limit(input.limit),
            order_by: Some(OrderBy {
                column: "timestamp_start".into(),
                direction: OrderDirection::Desc,
            }),
            trace_id: input.trace_id,
            session_id: input.session_id,
            observation_type: input.observation_type,
            framework: input.framework,
            gen_ai_request_model: input.model,
            status_code: input.status_code,
            from_timestamp: parse_opt_ts(input.from_timestamp),
            to_timestamp: parse_opt_ts(input.to_timestamp),
            ..Default::default()
        };
        let (rows, total) = repo.list_spans(&params).await.map_err(mcp_err)?;
        let spans = spans_to_dtos(&*repo, &self.project_id, &rows, false).await?;
        ok_json(&serde_json::json!({ "spans": spans, "total": total }))
    }

    #[tool(
        description = "Get raw OTLP span data: all attributes, events, resource metadata. For debugging framework-specific behavior."
    )]
    async fn get_raw_span(
        &self,
        Parameters(input): Parameters<GetRawSpanInput>,
    ) -> Result<CallToolResult, McpError> {
        let repo = self.analytics.repository();
        let span = repo
            .get_span(&self.project_id, &input.trace_id, &input.span_id)
            .await
            .map_err(mcp_err)?
            .ok_or_else(|| McpError::invalid_params("span not found", None))?;

        let dtos =
            spans_to_dtos(&*repo, &self.project_id, std::slice::from_ref(&span), true).await?;
        ok_json(&SpanDetailDto {
            summary: dtos.into_iter().next().unwrap(),
        })
    }

    #[tool(
        description = "List multi-turn sessions. Each groups related traces across user interactions. Returns summaries with counts, tokens, costs."
    )]
    async fn list_sessions(
        &self,
        Parameters(input): Parameters<ListSessionsInput>,
    ) -> Result<CallToolResult, McpError> {
        let repo = self.analytics.repository();
        let params = ListSessionsParams {
            project_id: self.project_id.clone(),
            page: clamp_page(input.page),
            limit: clamp_limit(input.limit),
            order_by: Some(OrderBy {
                column: "start_time".into(),
                direction: OrderDirection::Desc,
            }),
            user_id: input.user_id,
            environment: input.environment.map(|e| vec![e]),
            from_timestamp: parse_opt_ts(input.from_timestamp),
            to_timestamp: parse_opt_ts(input.to_timestamp),
            ..Default::default()
        };
        let (rows, total) = repo.list_sessions(&params).await.map_err(mcp_err)?;
        let sessions: Vec<SessionSummaryDto> =
            rows.into_iter().map(session_row_to_summary).collect();
        ok_json(&serde_json::json!({ "sessions": sessions, "total": total }))
    }

    #[tool(
        description = "Project analytics for a time period: costs and tokens by model/framework, trace/session/span counts, trends, avg latency."
    )]
    async fn get_stats(
        &self,
        Parameters(input): Parameters<GetStatsInput>,
    ) -> Result<CallToolResult, McpError> {
        let from_ts = parse_ts(&input.from_timestamp)
            .ok_or_else(|| McpError::invalid_params("invalid from_timestamp", None))?;
        let to_ts = parse_ts(&input.to_timestamp)
            .ok_or_else(|| McpError::invalid_params("invalid to_timestamp", None))?;

        if from_ts >= to_ts {
            return Err(McpError::invalid_params(
                "from_timestamp must be before to_timestamp",
                None,
            ));
        }
        if (to_ts - from_ts).num_days() > 90 {
            return Err(McpError::invalid_params(
                "time range cannot exceed 90 days",
                None,
            ));
        }

        let params = StatsParams {
            project_id: self.project_id.clone(),
            from_timestamp: from_ts,
            to_timestamp: to_ts,
            timezone: input.timezone,
        };
        let repo = self.analytics.repository();
        let result = repo.get_project_stats(&params).await.map_err(mcp_err)?;
        ok_json(&stats_result_to_dto(result, from_ts, to_ts))
    }
}

/// Fetch event/link counts and build SpanSummaryDto for a slice of spans.
async fn spans_to_dtos(
    repo: &(dyn AnalyticsRepository + Send + Sync),
    project_id: &str,
    spans: &[SpanRow],
    include_raw: bool,
) -> Result<Vec<SpanSummaryDto>, McpError> {
    let span_keys: Vec<(String, String)> = spans
        .iter()
        .map(|r| (r.trace_id.clone(), r.span_id.clone()))
        .collect();
    let counts = repo
        .get_span_counts_bulk(project_id, &span_keys)
        .await
        .map_err(mcp_err)?;

    Ok(spans
        .iter()
        .map(|span| {
            let key = (span.trace_id.clone(), span.span_id.clone());
            let c = counts.get(&key);
            SpanSummaryDto::from_row(
                span,
                c.map(|c| c.event_count).unwrap_or(0),
                c.map(|c| c.link_count).unwrap_or(0),
                include_raw,
            )
        })
        .collect())
}

fn ok_json(value: &impl serde::Serialize) -> Result<CallToolResult, McpError> {
    let json = serde_json::to_string(value).map_err(mcp_err)?;
    Ok(CallToolResult::success(vec![Content::text(json)]))
}

fn mcp_err(e: impl std::fmt::Display) -> McpError {
    tracing::debug!(error = %e, "MCP tool error");
    McpError::internal_error(e.to_string(), None)
}

fn clamp_page(page: Option<u32>) -> u32 {
    page.unwrap_or(1).max(1)
}

fn clamp_limit(limit: Option<u32>) -> u32 {
    limit.unwrap_or(20).clamp(1, MAX_PAGE_LIMIT)
}

fn parse_opt_ts(s: Option<String>) -> Option<DateTime<Utc>> {
    crate::api::types::parse_timestamp_param(&s).ok().flatten()
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    parse_opt_ts(Some(s.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_opt_ts_valid_iso8601() {
        let result = parse_opt_ts(Some("2025-01-15T12:00:00Z".to_string()));
        assert!(result.is_some());
        assert_eq!(result.unwrap().timestamp(), 1736942400);
    }

    #[test]
    fn test_parse_opt_ts_none() {
        assert!(parse_opt_ts(None).is_none());
    }

    #[test]
    fn test_parse_opt_ts_invalid() {
        assert!(parse_opt_ts(Some("not-a-date".to_string())).is_none());
    }

    #[test]
    fn test_parse_ts_valid() {
        assert!(parse_ts("2025-01-15T12:00:00Z").is_some());
    }

    #[test]
    fn test_parse_ts_invalid() {
        assert!(parse_ts("garbage").is_none());
    }

    #[test]
    fn test_ok_json_serializes() {
        let val = serde_json::json!({"key": "value"});
        let result = ok_json(&val);
        assert!(result.is_ok());
        let call_result = result.unwrap();
        assert!(!call_result.content.is_empty());
    }

    #[test]
    fn test_clamp_page() {
        assert_eq!(clamp_page(None), 1);
        assert_eq!(clamp_page(Some(0)), 1);
        assert_eq!(clamp_page(Some(1)), 1);
        assert_eq!(clamp_page(Some(5)), 5);
    }

    #[test]
    fn test_clamp_limit() {
        assert_eq!(clamp_limit(None), 20);
        assert_eq!(clamp_limit(Some(0)), 1);
        assert_eq!(clamp_limit(Some(1)), 1);
        assert_eq!(clamp_limit(Some(50)), 50);
        assert_eq!(clamp_limit(Some(1000)), MAX_PAGE_LIMIT);
    }
}
