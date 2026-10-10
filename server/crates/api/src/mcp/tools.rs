use std::sync::Arc;

use chrono::{DateTime, Utc};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, GetPromptResult, Implementation, PromptMessage, Role,
    ServerCapabilities, ServerConfig,
};
use rmcp::{ServerHandler, prompt, prompt_handler, prompt_router, tool, tool_handler, tool_router};

use crate::routes::otel::messages::{build_messages_response, scope_feed_to_trace};
use crate::routes::otel::sessions::session_row_to_summary;
use crate::routes::otel::stats::{
    INVALID_TIME_RANGE_MESSAGE, INVALID_TIMEZONE_MESSAGE, RANGE_TOO_LARGE_MESSAGE, StatsRangeError,
    normalize_timezone, stats_result_to_dto, validate_stats_time_range,
};
use crate::routes::otel::traces::{MAX_SPANS_PER_TRACE, trace_row_to_summary};
use crate::routes::otel::types::SpanEnvelopeDto;
use crate::routes::otel::types::{
    SessionSummaryDto, SpanDetailDto, SpanSummaryDto, TraceDetailDto, TraceSummaryDto,
};
use crate::types::{MAX_PAGE, MAX_PAGE_LIMIT, OrderBy, OrderDirection};
use sideseat_domain::sideml::{FeedOptions, extract_tools_from_rows, process_span, process_spans};
use sideseat_ports::traits::AnalyticsRepository;
use sideseat_ports::types::{
    ListSessionsParams, ListSpansParams, ListTracesParams, MessageQueryParams, ProjectId, SpanRow,
    StatsParams,
};

use super::types::*;

type McpError = rmcp::model::ErrorData;

#[derive(Clone)]
pub struct McpServer {
    analytics: Arc<crate::dependencies::AnalyticsStore>,
    files: Arc<sideseat_domain::files::FileService>,
    project_id: ProjectId,
}

impl McpServer {
    pub fn new(
        analytics: Arc<crate::dependencies::AnalyticsStore>,
        files: Arc<sideseat_domain::files::FileService>,
        project_id: String,
    ) -> Self {
        Self {
            analytics,
            files,
            project_id: project_id.into(),
        }
    }
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerConfig {
        // The protocol structs are non-exhaustive, so use their public builders and setters.
        let mut info = ServerConfig::default();
        info.instructions = Some(INSTRUCTIONS.to_string());
        info.capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_prompts()
            .build();
        info.server_info = Implementation::new("SideSeat", env!("CARGO_PKG_VERSION"));
        info
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
        let repo = self.analytics.as_ref();
        let time_window = parse_time_window(input.from_timestamp, input.to_timestamp)?;
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
            from_timestamp: time_window.from,
            to_timestamp: time_window.to,
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
        let repo = self.analytics.as_ref();
        let trace = repo
            .get_trace(&self.project_id, &input.trace_id)
            .await
            .map_err(mcp_err)?
            .ok_or_else(|| McpError::invalid_params("trace not found", None))?;

        let mut spans = repo
            .get_spans_for_trace(&self.project_id, &input.trace_id, MAX_SPANS_PER_TRACE + 1)
            .await
            .map_err(mcp_err)?;
        let spans_truncated = spans.len() > MAX_SPANS_PER_TRACE;
        spans.truncate(MAX_SPANS_PER_TRACE);

        let span_details: Vec<SpanDetailDto> = spans_to_dtos(repo, &self.project_id, &spans, false)
            .await?
            .into_iter()
            .map(|summary| SpanDetailDto { summary })
            .collect();

        let summary = trace_row_to_summary(trace);
        ok_json(&TraceDetailDto {
            summary,
            spans: span_details,
            spans_truncated,
        })
    }

    #[tool(
        description = "Get normalized LLM conversation. Returns messages with roles (system/user/assistant/tool), content blocks (text, tool_use, tool_result, thinking), tokens, costs. Provide trace_id, or session_id, or span_id together with its trace_id (a span id is unique only within a trace, and a session id will not do instead)."
    )]
    async fn get_messages(
        &self,
        Parameters(input): Parameters<GetMessagesInput>,
    ) -> Result<CallToolResult, McpError> {
        let repo = self.analytics.as_ref();
        let options = FeedOptions::new().with_role(input.role);

        // Direct span/session path; session rows still receive the pipeline's cross-trace replay handling.
        if input.span_id.is_some() || input.session_id.is_some() {
            // A span id is 8 bytes and unique only within a trace, so the pair is required to identify one
            // span. Returning a project-wide match for an unqualified span id would merge unrelated traces.
            if span_lacks_its_trace(input.span_id.as_deref(), input.trace_id.as_deref()) {
                return Err(McpError::invalid_params(
                    "span_id identifies a span only within a trace, because a span id is 8 bytes \
                     and traces reuse them. Pass trace_id as well - list_spans and get_trace both \
                     return it - or ask by trace_id or session_id instead.",
                    None,
                ));
            }

            // Not applied to a session query - a session spans several traces, and adding one
            // would return part of it while the response still claims to be the session.
            let trace_id = input.span_id.as_ref().and(input.trace_id.clone());
            let params = MessageQueryParams {
                project_id: self.project_id.clone(),
                span_id: input.span_id,
                session_id: input.session_id,
                trace_id,
                ..Default::default()
            };
            let result = repo.get_messages(&params).await.map_err(mcp_err)?;
            let envelopes: Vec<SpanEnvelopeDto> =
                result.rows.iter().map(SpanEnvelopeDto::from_row).collect();
            let processed = if params.span_id.is_some() {
                process_span(result.rows, &options)
            } else {
                process_spans(result.rows, &options)
            };
            // Session totals come from the session aggregate because message rows omit silent billed spans
            // and do not apply the parent/child billing deduplication. A span view contains one span, where
            // neither distinction applies.
            let session_totals = match &params.session_id {
                Some(session_id) if params.span_id.is_none() => repo
                    .get_session(&self.project_id, session_id)
                    .await
                    .map_err(mcp_err)?
                    .map(|s| (s.total_tokens, s.total_cost)),
                _ => None,
            };
            return ok_json(&build_messages_response(
                &processed,
                session_totals,
                envelopes,
            ));
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
        // Envelope scope follows the view: the query loads the whole session so cross-trace
        // stripping can run, and the caller asked about one trace.
        let envelopes: Vec<SpanEnvelopeDto> = result
            .rows
            .iter()
            .filter(|r| r.trace_id == trace_id)
            .map(SpanEnvelopeDto::from_row)
            .collect();

        let processed = process_spans(result.rows, &options);
        let processed = match scoped_tools {
            Some(scoped_tools) => scope_feed_to_trace(&processed, scoped_tools, &trace_id),
            None => processed,
        };

        let trace_totals = trace.map(|t| (t.total_tokens, t.total_cost));
        ok_json(&build_messages_response(
            &processed,
            trace_totals,
            envelopes,
        ))
    }

    #[tool(
        description = "Search operations across traces. Filter by observation_type (Generation=LLM, Tool=tool exec, Agent=agent step), model, framework, error status."
    )]
    async fn list_spans(
        &self,
        Parameters(input): Parameters<ListSpansInput>,
    ) -> Result<CallToolResult, McpError> {
        let repo = self.analytics.as_ref();
        let time_window = parse_time_window(input.from_timestamp, input.to_timestamp)?;
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
            from_timestamp: time_window.from,
            to_timestamp: time_window.to,
            ..Default::default()
        };
        let (rows, total) = repo.list_spans(&params).await.map_err(mcp_err)?;
        let spans = spans_to_dtos(repo, &self.project_id, &rows, false).await?;
        ok_json(&serde_json::json!({ "spans": spans, "total": total }))
    }

    #[tool(
        description = "Get raw OTLP span data: all attributes, events, resource metadata. For debugging framework-specific behavior."
    )]
    async fn get_raw_span(
        &self,
        Parameters(input): Parameters<GetRawSpanInput>,
    ) -> Result<CallToolResult, McpError> {
        let repo = self.analytics.as_ref();
        let mut span = repo
            .get_span(&self.project_id, &input.trace_id, &input.span_id)
            .await
            .map_err(mcp_err)?
            .ok_or_else(|| McpError::invalid_params("span not found", None))?;

        // Rendered from the raw record this span was derived from; nothing stores a second copy of it.
        span.raw_span = sideseat_ingestion::traces::raw_views::render_spans(
            &self.project_id,
            &[(span.trace_id.clone(), span.span_id.clone())],
            repo,
            &self.files,
        )
        .await
        .remove(&(span.trace_id.clone(), span.span_id.clone()));
        let dtos = spans_to_dtos(repo, &self.project_id, std::slice::from_ref(&span), true).await?;
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
        let repo = self.analytics.as_ref();
        let time_window = parse_time_window(input.from_timestamp, input.to_timestamp)?;
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
            from_timestamp: time_window.from,
            to_timestamp: time_window.to,
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
        let from_ts = parse_ts("from_timestamp", &input.from_timestamp)?;
        let to_ts = parse_ts("to_timestamp", &input.to_timestamp)?;

        validate_stats_time_range(from_ts, to_ts).map_err(|error| {
            let message = match error {
                StatsRangeError::InvalidOrder => INVALID_TIME_RANGE_MESSAGE,
                StatsRangeError::TooLarge => RANGE_TOO_LARGE_MESSAGE,
            };
            McpError::invalid_params(message, None)
        })?;

        let timezone = normalize_timezone(input.timezone)
            .map_err(|_| McpError::invalid_params(INVALID_TIMEZONE_MESSAGE, None))?;
        let params = StatsParams {
            project_id: self.project_id.clone(),
            from_timestamp: from_ts,
            to_timestamp: to_ts,
            timezone,
        };
        let repo = self.analytics.as_ref();
        let result = repo.get_project_stats(&params).await.map_err(mcp_err)?;
        ok_json(&stats_result_to_dto(result, from_ts, to_ts))
    }
}

#[prompt_router]
impl McpServer {
    #[prompt(
        description = "Get setup instructions for integrating SideSeat telemetry. Specify a framework for tailored code examples (SDK one-liner + direct OTLP fallback)."
    )]
    async fn setup_guide(&self, Parameters(args): Parameters<SetupGuideArgs>) -> GetPromptResult {
        let content = build_setup_guide(&self.project_id, args.framework.as_deref());
        let mut result = GetPromptResult::new(vec![PromptMessage::new_text(Role::User, content)]);
        result.description = Some("SideSeat integration guide".to_string());
        result
    }
}

/// Fetch event/link counts and build SpanSummaryDto for a slice of spans.
async fn spans_to_dtos(
    repo: &(dyn AnalyticsRepository + Send + Sync),
    project_id: &ProjectId,
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

/// Which ecosystem a framework belongs to. Determines whether the guide emits
/// pip/Python or npm/TypeScript instructions.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lang {
    Python,
    TypeScript,
}

#[derive(Copy, Clone)]
struct FrameworkSetup {
    display: &'static str,
    lang: Lang,
    /// Package to install alongside the SDK. pip name for Python, npm name for TypeScript.
    pip_pkg: &'static str,
    /// Optional-dependency extra the SDK needs for this framework's instrumentation.
    /// Without it the import fails, `instrument()` logs a warning and returns false, and
    /// the app runs with no spans at all - so the SDK install line must carry it.
    sdk_extra: &'static str,
    /// The SDK integration name, as `sideseat.init(integrations=[...])` takes it.
    integration: &'static str,
    sdk_snippet: &'static str,
    no_sdk_extra_pkgs: &'static str,
    no_sdk_extra_setup: &'static str,
}

/// Module-scoped so validation tests iterate the same framework table used by the guide.
const FRAMEWORKS: &[FrameworkSetup] = &[
    FrameworkSetup {
        display: "Strands Agents",
        lang: Lang::Python,
        pip_pkg: "strands-agents",
        sdk_extra: "",
        integration: "strands",
        sdk_snippet: "from strands import Agent\n\nagent = Agent()\nprint(agent(\"Hello\"))",
        no_sdk_extra_pkgs: "",
        no_sdk_extra_setup: "",
    },
    FrameworkSetup {
        display: "LangChain",
        lang: Lang::Python,
        pip_pkg: "langchain-openai",
        sdk_extra: "langchain",
        integration: "langchain",
        sdk_snippet: "from langchain_openai import ChatOpenAI\nllm = ChatOpenAI(model=\"gpt-6.1-sol\")\nprint(llm.invoke(\"Hello\").content)",
        no_sdk_extra_pkgs: "openinference-instrumentation-langchain",
        no_sdk_extra_setup: "from openinference.instrumentation.langchain import LangChainInstrumentor\nLangChainInstrumentor().instrument(tracer_provider=provider, skip_dep_check=True)",
    },
    FrameworkSetup {
        display: "LangGraph",
        lang: Lang::Python,
        pip_pkg: "langgraph langchain-openai",
        sdk_extra: "langgraph",
        integration: "langgraph",
        sdk_snippet: "from langgraph.prebuilt import create_react_agent\nfrom langchain_openai import ChatOpenAI\nagent = create_react_agent(ChatOpenAI(model=\"gpt-6.1-sol\"), [])\nprint(agent.invoke({\"messages\": [(\"user\", \"Hello\")]}))",
        no_sdk_extra_pkgs: "openinference-instrumentation-langchain",
        no_sdk_extra_setup: "from openinference.instrumentation.langchain import LangChainInstrumentor\nLangChainInstrumentor().instrument(tracer_provider=provider, skip_dep_check=True)",
    },
    FrameworkSetup {
        display: "CrewAI",
        lang: Lang::Python,
        pip_pkg: "crewai",
        sdk_extra: "crewai",
        integration: "crewai",
        sdk_snippet: "from crewai import Agent, Task, Crew\na = Agent(role=\"R\", goal=\"G\", backstory=\"B\")\nt = Task(description=\"D\", expected_output=\"O\", agent=a)\nprint(Crew(agents=[a], tasks=[t]).kickoff())",
        no_sdk_extra_pkgs: "openinference-instrumentation-crewai",
        no_sdk_extra_setup: "from openinference.instrumentation.crewai import CrewAIInstrumentor\nCrewAIInstrumentor().instrument(tracer_provider=provider, skip_dep_check=True)",
    },
    FrameworkSetup {
        display: "AutoGen",
        lang: Lang::Python,
        // autogen_ext.models.openai lives in autogen-ext, and the openai extra is what
        // pulls its OpenAI client dependencies.
        pip_pkg: "autogen-agentchat \"autogen-ext[openai]\"",
        sdk_extra: "autogen",
        integration: "autogen",
        sdk_snippet: "import asyncio\nfrom autogen_agentchat.agents import AssistantAgent\nfrom autogen_ext.models.openai import OpenAIChatCompletionClient\nagent = AssistantAgent(\"a\", model_client=OpenAIChatCompletionClient(model=\"gpt-6.1-sol\"))\nasyncio.run(agent.run(task=\"Hello\"))",
        no_sdk_extra_pkgs: "openinference-instrumentation-autogen-agentchat",
        no_sdk_extra_setup: "from openinference.instrumentation.autogen_agentchat import AutogenAgentChatInstrumentor\nAutogenAgentChatInstrumentor().instrument(tracer_provider=provider, skip_dep_check=True)",
    },
    FrameworkSetup {
        display: "OpenAI Agents SDK",
        lang: Lang::Python,
        pip_pkg: "openai-agents",
        sdk_extra: "openai-agents",
        integration: "openai-agents",
        sdk_snippet: "from agents import Agent, Runner\nprint(Runner.run_sync(Agent(name=\"A\", instructions=\"Helpful.\"), \"Hello\").final_output)",
        no_sdk_extra_pkgs: "logfire",
        no_sdk_extra_setup: "import logfire\nlogfire.configure(send_to_logfire=False, console=False)\nlogfire.instrument_openai_agents()",
    },
    FrameworkSetup {
        display: "PydanticAI",
        lang: Lang::Python,
        pip_pkg: "pydantic-ai",
        sdk_extra: "pydantic-ai",
        integration: "pydantic-ai",
        sdk_snippet: "from pydantic_ai import Agent\nprint(Agent(\"openai:gpt-6.1-sol\").run_sync(\"Hello\").output)",
        no_sdk_extra_pkgs: "logfire[pydantic-ai]",
        no_sdk_extra_setup: "import logfire\nlogfire.configure(send_to_logfire=False, console=False)\nlogfire.instrument_pydantic_ai()",
    },
    FrameworkSetup {
        display: "Google ADK",
        lang: Lang::Python,
        pip_pkg: "google-adk",
        sdk_extra: "",
        integration: "google-adk",
        sdk_snippet: "import asyncio\n\nfrom google.adk.agents import LlmAgent\nfrom google.adk.runners import Runner\nfrom google.adk.sessions import InMemorySessionService\nfrom google.genai import types\n\nagent = LlmAgent(model=\"gemini-2.5-flash\", name=\"assistant\", instruction=\"Be helpful.\")\n\nasync def main():\n    sessions = InMemorySessionService()\n    await sessions.create_session(app_name=\"demo\", user_id=\"u1\", session_id=\"s1\")\n    runner = Runner(agent=agent, app_name=\"demo\", session_service=sessions)\n    async for event in runner.run_async(\n        session_id=\"s1\",\n        user_id=\"u1\",\n        new_message=types.Content(role=\"user\", parts=[types.Part(text=\"Hello\")]),\n    ):\n        if event.content and event.content.parts:\n            for part in event.content.parts:\n                if getattr(part, \"text\", None):\n                    print(part.text)\n\nasyncio.run(main())",
        no_sdk_extra_pkgs: "",
        no_sdk_extra_setup: "",
    },
    FrameworkSetup {
        display: "Microsoft Agent Framework",
        lang: Lang::Python,
        pip_pkg: "agent-framework",
        sdk_extra: "",
        integration: "agent-framework",
        sdk_snippet: "import asyncio\nfrom agent_framework import Agent\nfrom agent_framework.openai import OpenAIChatClient\nprint(asyncio.run(Agent(client=OpenAIChatClient(model=\"gpt-6.1-sol\"), instructions=\"Helpful.\").run(\"Hello\")).text)",
        no_sdk_extra_pkgs: "",
        no_sdk_extra_setup: "from agent_framework.observability import OBSERVABILITY_SETTINGS\nOBSERVABILITY_SETTINGS.enable_instrumentation = True\nOBSERVABILITY_SETTINGS.enable_sensitive_data = True",
    },
    FrameworkSetup {
        display: "Amazon Bedrock",
        lang: Lang::Python,
        pip_pkg: "boto3",
        sdk_extra: "bedrock",
        integration: "bedrock",
        sdk_snippet: "import boto3\nr = boto3.client(\"bedrock-runtime\", region_name=\"us-east-1\").converse(modelId=\"global.anthropic.claude-sonnet-5-5\", messages=[{\"role\": \"user\", \"content\": [{\"text\": \"Hello\"}]}])\nprint(r[\"output\"][\"message\"][\"content\"][0][\"text\"])",
        no_sdk_extra_pkgs: "opentelemetry-instrumentation-botocore",
        no_sdk_extra_setup: "from opentelemetry.instrumentation.botocore import BotocoreInstrumentor\nBotocoreInstrumentor().instrument(tracer_provider=provider)",
    },
    FrameworkSetup {
        display: "Claude Agent SDK",
        lang: Lang::Python,
        pip_pkg: "claude-agent-sdk",
        sdk_extra: "",
        integration: "claude-agent-sdk",
        sdk_snippet: "import asyncio\nfrom claude_agent_sdk import query, ClaudeAgentOptions\n\n# The Agent SDK emits no telemetry itself: it spawns the Claude Code CLI, which\n# carries the OTel instrumentation and is configured via these env vars.\nOTEL_ENV = {\n    \"CLAUDE_CODE_ENABLE_TELEMETRY\": \"1\",\n    # Span tracing is beta and off without this flag.\n    \"CLAUDE_CODE_ENHANCED_TELEMETRY_BETA\": \"1\",\n    # Second beta tier. Without these two the message feed stays empty:\n    # assistant reply text exists nowhere else on the trace.\n    \"ENABLE_BETA_TRACING_DETAILED\": \"1\",\n    \"BETA_TRACING_ENDPOINT\": \"__OTLP_BASE__\",\n    # Never \"console\": the CLI writes telemetry to stdout, which is the SDK's\n    # message channel, and would corrupt the stream.\n    \"OTEL_TRACES_EXPORTER\": \"otlp\",\n    \"OTEL_EXPORTER_OTLP_TRACES_PROTOCOL\": \"http/protobuf\",\n    \"OTEL_EXPORTER_OTLP_TRACES_ENDPOINT\": \"__OTLP_ENDPOINT__\",\n    # Content is redacted by default, leaving the message feed empty.\n    \"OTEL_LOG_USER_PROMPTS\": \"1\",\n    \"OTEL_LOG_TOOL_DETAILS\": \"1\",\n}\n\nasync def main():\n    options = ClaudeAgentOptions(env=OTEL_ENV, allowed_tools=[\"Read\", \"Glob\"])\n    async for message in query(prompt=\"What is 2+2?\", options=options):\n        print(message)\n\nasyncio.run(main())",
        no_sdk_extra_pkgs: "",
        no_sdk_extra_setup: "",
    },
    FrameworkSetup {
        display: "Anthropic",
        lang: Lang::Python,
        pip_pkg: "anthropic",
        sdk_extra: "anthropic",
        integration: "anthropic",
        sdk_snippet: "import anthropic\nprint(anthropic.Anthropic().messages.create(model=\"claude-sonnet-5-5\", max_tokens=256, messages=[{\"role\": \"user\", \"content\": \"Hello\"}]).content[0].text)",
        no_sdk_extra_pkgs: "logfire[anthropic]",
        no_sdk_extra_setup: "import logfire\nlogfire.configure(send_to_logfire=False, console=False)\nlogfire.instrument_anthropic()",
    },
    FrameworkSetup {
        display: "OpenAI",
        lang: Lang::Python,
        pip_pkg: "openai",
        sdk_extra: "openai",
        integration: "openai",
        sdk_snippet: "from openai import OpenAI\nprint(OpenAI().chat.completions.create(model=\"gpt-6.1-sol\", messages=[{\"role\": \"user\", \"content\": \"Hello\"}]).choices[0].message.content)",
        no_sdk_extra_pkgs: "logfire[openai]",
        no_sdk_extra_setup: "import logfire\nlogfire.configure(send_to_logfire=False, console=False)\nlogfire.instrument_openai()",
    },
    FrameworkSetup {
        display: "Google Gemini",
        lang: Lang::Python,
        pip_pkg: "google-genai",
        sdk_extra: "google-genai",
        integration: "google-genai",
        sdk_snippet: "from google import genai\nprint(genai.Client(api_key=\"YOUR_KEY\").models.generate_content(model=\"gemini-2.5-flash\", contents=\"Hello\").text)",
        no_sdk_extra_pkgs: "logfire[google-genai]",
        no_sdk_extra_setup: "import logfire\nlogfire.configure(send_to_logfire=False, console=False)\nlogfire.instrument_google_genai()",
    },
    FrameworkSetup {
        display: "Google Vertex AI",
        lang: Lang::Python,
        pip_pkg: "google-genai",
        sdk_extra: "vertex-ai",
        integration: "vertex-ai",
        sdk_snippet: "from google import genai\nclient = genai.Client(enterprise=True, project=\"PROJECT_ID\", location=\"us-central1\")\nprint(client.models.generate_content(model=\"gemini-2.5-flash\", contents=\"Hello\").text)",
        no_sdk_extra_pkgs: "logfire[google-genai]",
        no_sdk_extra_setup: "import logfire\nlogfire.configure(send_to_logfire=False, console=False)\nlogfire.instrument_google_genai()",
    },
    FrameworkSetup {
        display: "Azure OpenAI",
        lang: Lang::Python,
        pip_pkg: "openai",
        sdk_extra: "azure-openai",
        integration: "azure-openai",
        sdk_snippet: "import os\nfrom openai import OpenAI\n\nazure = OpenAI(\n    api_key=os.environ[\"AZURE_OPENAI_API_KEY\"],\n    base_url=\"https://YOUR-RESOURCE.openai.azure.com/openai/v1/\",\n)\nresponse = azure.chat.completions.create(\n    model=\"YOUR-DEPLOYMENT\",\n    messages=[{\"role\": \"user\", \"content\": \"Hello\"}],\n)\nprint(response.choices[0].message.content)",
        no_sdk_extra_pkgs: "openinference-instrumentation-openai",
        no_sdk_extra_setup: "from openinference.instrumentation.openai import OpenAIInstrumentor\nOpenAIInstrumentor().instrument(tracer_provider=provider)",
    },
    FrameworkSetup {
        display: "Agno",
        lang: Lang::Python,
        pip_pkg: "agno openai",
        sdk_extra: "agno",
        integration: "agno",
        sdk_snippet: "from agno.agent import Agent\nfrom agno.models.openai import OpenAIChat\n\nagent = Agent(model=OpenAIChat(id=\"gpt-6.1-sol\"))\nagent.print_response(\"Hello\")",
        no_sdk_extra_pkgs: "openinference-instrumentation-agno",
        no_sdk_extra_setup: "from openinference.instrumentation.agno import AgnoInstrumentor\nAgnoInstrumentor().instrument(tracer_provider=provider)",
    },
    FrameworkSetup {
        display: "Smolagents",
        lang: Lang::Python,
        pip_pkg: "smolagents",
        sdk_extra: "smolagents",
        integration: "smolagents",
        sdk_snippet: "from smolagents import CodeAgent, InferenceClientModel\n\nagent = CodeAgent(tools=[], model=InferenceClientModel())\nprint(agent.run(\"What is 2+2?\"))",
        no_sdk_extra_pkgs: "openinference-instrumentation-smolagents",
        no_sdk_extra_setup: "from openinference.instrumentation.smolagents import SmolagentsInstrumentor\nSmolagentsInstrumentor().instrument(tracer_provider=provider)",
    },
    FrameworkSetup {
        display: "AG2",
        lang: Lang::Python,
        pip_pkg: "\"ag2[openai]\"",
        sdk_extra: "ag2",
        integration: "ag2",
        sdk_snippet: "from autogen import ConversableAgent\n\nassistant = ConversableAgent(\n    name=\"assistant\",\n    llm_config={\"model\": \"gpt-6.1-sol\"},\n)\nprint(assistant.generate_reply(messages=[{\"role\": \"user\", \"content\": \"Hello\"}]))",
        no_sdk_extra_pkgs: "openinference-instrumentation-autogen",
        no_sdk_extra_setup: "from openinference.instrumentation.autogen import AutogenInstrumentor\nAutogenInstrumentor().instrument(tracer_provider=provider)",
    },
    FrameworkSetup {
        display: "AgentScope",
        lang: Lang::Python,
        pip_pkg: "agentscope",
        sdk_extra: "agentscope",
        integration: "agentscope",
        // Runnable as a script: AgentScope's agent call is async, so it needs an
        // asyncio entry point rather than a bare top-level await.
        sdk_snippet: "import asyncio\nimport os\nfrom agentscope.agent import Agent\nfrom agentscope.credential import OpenAICredential\nfrom agentscope.message import UserMsg\nfrom agentscope.middleware import TracingMiddleware\nfrom agentscope.model import OpenAIChatModel\n\nasync def main():\n    model = OpenAIChatModel(\n        credential=OpenAICredential(api_key=os.environ[\"OPENAI_API_KEY\"]),\n        model=\"gpt-6.1-sol\",\n    )\n    # Explicit middleware keeps the same runnable body valid in the direct-OTLP\n    # guide. SideSeat recognises it and does not inject a duplicate.\n    agent = Agent(\n        name=\"assistant\",\n        system_prompt=\"Answer briefly.\",\n        model=model,\n        middlewares=[TracingMiddleware()],\n    )\n    reply = await agent.reply(UserMsg(\"user\", \"Hello!\"))\n    print(reply.get_text_content())\n\nasyncio.run(main())",
        no_sdk_extra_pkgs: "",
        no_sdk_extra_setup: "",
    },
    FrameworkSetup {
        display: "Langflow",
        lang: Lang::Python,
        pip_pkg: "langflow",
        sdk_extra: "",
        integration: "langflow",
        sdk_snippet: "# Langflow emits OpenTelemetry itself; run it with the provider configured\n# in the same process, or point its OTLP exporter at SideSeat.",
        no_sdk_extra_pkgs: "",
        no_sdk_extra_setup: "",
    },
    FrameworkSetup {
        display: "Haystack",
        lang: Lang::Python,
        pip_pkg: "haystack-ai",
        sdk_extra: "haystack",
        integration: "haystack",
        sdk_snippet: "from haystack import Pipeline\nfrom haystack.components.generators.chat import OpenAIChatGenerator\nfrom haystack.dataclasses import ChatMessage\n\npipeline = Pipeline()\npipeline.add_component(\"llm\", OpenAIChatGenerator(model=\"gpt-6.1-sol\"))\nresult = pipeline.run({\"llm\": {\"messages\": [ChatMessage.from_user(\"Hello\")]}})\nprint(result[\"llm\"][\"replies\"][0].text)",
        no_sdk_extra_pkgs: "openinference-instrumentation-haystack",
        no_sdk_extra_setup: "from openinference.instrumentation.haystack import HaystackInstrumentor\nHaystackInstrumentor().instrument(tracer_provider=provider)",
    },
    FrameworkSetup {
        display: "browser-use",
        lang: Lang::Python,
        pip_pkg: "browser-use",
        sdk_extra: "",
        integration: "browser-use",
        sdk_snippet: "import asyncio\nfrom browser_use import Agent, ChatOpenAI\n\n# browser-use emits OpenTelemetry itself and uses the global provider.\nasync def main():\n    agent = Agent(task=\"Find the docs\", llm=ChatOpenAI(model=\"gpt-6.1-sol\"))\n    print(await agent.run())\n\nasyncio.run(main())",
        no_sdk_extra_pkgs: "",
        no_sdk_extra_setup: "",
    },
    FrameworkSetup {
        display: "Vercel AI SDK",
        lang: Lang::TypeScript,
        pip_pkg: "ai @ai-sdk/otel @ai-sdk/amazon-bedrock",
        sdk_extra: "",
        integration: "vercel-ai",
        sdk_snippet: "import { generateText } from 'ai';\n\
                          import { bedrock } from '@ai-sdk/amazon-bedrock';\n\n\
                          const { text } = await generateText({\n\
                          \u{20}\u{20}model: bedrock('global.anthropic.claude-sonnet-5-5'),\n\
                          \u{20}\u{20}prompt: 'What is 2+2?',\n});\nconsole.log(text);",
        no_sdk_extra_pkgs: "",
        // Without the SDK, AI SDK 7 delivers telemetry only to a registered integration.
        no_sdk_extra_setup: "import { registerTelemetry } from 'ai';\nimport { LegacyOpenTelemetry } from '@ai-sdk/otel';\n\nregisterTelemetry(new LegacyOpenTelemetry());",
    },
    FrameworkSetup {
        display: "Strands TypeScript",
        lang: Lang::TypeScript,
        pip_pkg: "@strands-agents/sdk",
        sdk_extra: "",
        integration: "strands",
        sdk_snippet: "import { Agent } from '@strands-agents/sdk';\n\n\
                          const agent = new Agent({\n\
                          \u{20}\u{20}model: 'global.anthropic.claude-sonnet-5-5',\n\
                          });\n\
                          const result = await agent.invoke('Hello');\n\
                          console.log(result.toString());",
        no_sdk_extra_pkgs: "",
        no_sdk_extra_setup: "",
    },
    FrameworkSetup {
        // No parentheses: the alias is derived from the display name by lowercasing and
        // replacing spaces with hyphens, so "(TypeScript)" would make it unmatchable.
        display: "Claude Agent SDK TypeScript",
        lang: Lang::TypeScript,
        pip_pkg: "@anthropic-ai/claude-agent-sdk",
        sdk_extra: "",
        integration: "claude-agent-sdk",
        sdk_snippet: "import { query } from '@anthropic-ai/claude-agent-sdk';\n\n\
                          // The Agent SDK emits no telemetry itself: the Claude Code CLI it spawns\n\
                          // self-instruments and is configured through CLAUDE_CODE_* / OTEL_* env vars\n\
                          // on the subprocess. See the Claude Agent SDK integration page.\n\
                          // The CLI subprocess exports OTLP itself; these are its entire\n\
                          // configuration. Span tracing is beta, and message content needs a\n\
                          // second beta tier on top or the Messages tab stays empty.\n\
                          // options.env REPLACES the environment in TypeScript, so process.env is\n\
                          // spread or the subprocess loses PATH and credentials.\n\
                          const options = {\n\
                          \u{20}\u{20}env: {\n\
                          \u{20}\u{20}\u{20}\u{20}...process.env,\n\
                          \u{20}\u{20}\u{20}\u{20}CLAUDE_CODE_ENABLE_TELEMETRY: '1',\n\
                          \u{20}\u{20}\u{20}\u{20}CLAUDE_CODE_ENHANCED_TELEMETRY_BETA: '1',\n\
                          \u{20}\u{20}\u{20}\u{20}ENABLE_BETA_TRACING_DETAILED: '1',\n\
                          \u{20}\u{20}\u{20}\u{20}BETA_TRACING_ENDPOINT: '__OTLP_BASE__',\n\
                          \u{20}\u{20}\u{20}\u{20}OTEL_TRACES_EXPORTER: 'otlp',\n\
                          \u{20}\u{20}\u{20}\u{20}OTEL_METRICS_EXPORTER: 'none',\n\
                          \u{20}\u{20}\u{20}\u{20}OTEL_LOGS_EXPORTER: 'none',\n\
                          \u{20}\u{20}\u{20}\u{20}OTEL_EXPORTER_OTLP_TRACES_PROTOCOL: 'http/protobuf',\n\
                          \u{20}\u{20}\u{20}\u{20}OTEL_EXPORTER_OTLP_TRACES_ENDPOINT: '__OTLP_ENDPOINT__',\n\
                          \u{20}\u{20}\u{20}\u{20}OTEL_LOG_USER_PROMPTS: '1',\n\
                          \u{20}\u{20}\u{20}\u{20}OTEL_LOG_TOOL_DETAILS: '1',\n\
                          \u{20}\u{20}},\n\
                          };\n\n\
                          for await (const msg of query({ prompt: 'Hello', options })) console.log(msg);",
        no_sdk_extra_pkgs: "",
        no_sdk_extra_setup: "",
    },
];

/// The names `get_framework` accepts, as a caller would type them.
fn supported_framework_names() -> Vec<String> {
    let mut names: Vec<String> = FRAMEWORKS
        .iter()
        .map(|f| f.display.to_lowercase().replace(' ', "-"))
        .collect();
    names.sort();
    names.dedup();
    names
}

fn get_framework(name: &str) -> Option<FrameworkSetup> {
    FRAMEWORKS
        .iter()
        .find(|f| {
            f.display.to_lowercase().replace(' ', "-") == name
                || f.integration == name
                || f.integration.replace('-', "") == name.replace('-', "")
                || f.pip_pkg.split_whitespace().any(|p| p == name)
        })
        .copied()
}

fn build_setup_guide(project_id: &str, framework: Option<&str>) -> String {
    let otlp_url = format!("http://localhost:5388/otel/{project_id}/v1/traces");

    // Snippets are inserted as values, not re-formatted, so a snippet that needs the
    // endpoint carries this placeholder and is substituted after formatting.
    let guide = build_setup_guide_template(&otlp_url, framework);
    // BETA_TRACING_ENDPOINT takes the collector base URL, not the /v1/traces path.
    let otlp_base = otlp_url.trim_end_matches("/v1/traces");
    guide
        .replace("__OTLP_ENDPOINT__", &otlp_url)
        .replace("__OTLP_BASE__", otlp_base)
}

fn build_setup_guide_template(otlp_url: &str, framework: Option<&str>) -> String {
    match framework.and_then(|f| get_framework(&f.to_lowercase())) {
        Some(fw) if fw.lang == Lang::TypeScript => {
            let extra_setup = if fw.no_sdk_extra_setup.is_empty() {
                String::new()
            } else {
                format!("{}\n\n", fw.no_sdk_extra_setup)
            };
            format!(
                "## With SideSeat SDK (recommended)\n\n\
                 ```bash\nnpm install @sideseat/sdk {npm}\n```\n\n\
                 ```typescript\nimport * as sideseat from '@sideseat/sdk';\n\n\
                 await sideseat.init({{ integrations: ['{integration}'] }});\n\n{snippet}\n```\n\n\
                 ## Without SDK (direct OTLP)\n\n\
                 ```bash\nnpm install {npm} @opentelemetry/sdk-node @opentelemetry/exporter-trace-otlp-http\n```\n\n\
                 ```typescript\nimport {{ NodeSDK }} from '@opentelemetry/sdk-node';\n\
                 import {{ OTLPTraceExporter }} from '@opentelemetry/exporter-trace-otlp-http';\n\n\
                 const sdk = new NodeSDK({{\n\
                 \u{20}\u{20}traceExporter: new OTLPTraceExporter({{ url: '{otlp}' }}),\n}});\n\
                 sdk.start();\n\n{extra_setup}{snippet}\n```",
                npm = fw.pip_pkg,
                integration = fw.integration,
                snippet = fw.sdk_snippet,
                extra_setup = extra_setup,
                otlp = otlp_url,
            )
        }
        Some(fw) => {
            // `sideseat[extra]` is quoted: bare brackets are glob metacharacters in zsh.
            let sdk_pkg = if fw.sdk_extra.is_empty() {
                "sideseat".to_string()
            } else {
                format!("\"sideseat[{}]\"", fw.sdk_extra)
            };
            let extra_pkgs = if fw.no_sdk_extra_pkgs.is_empty() {
                String::new()
            } else {
                format!(" {}", fw.no_sdk_extra_pkgs)
            };
            format!(
                "## With SideSeat SDK (recommended)\n\n\
                 ```bash\npip install {sdk_pkg} {pip}\n```\n\n\
                 ```python\nimport sideseat\n\
                 sideseat.init(integrations=[\"{integration}\"])\n\n{snippet}\n```\n\n\
                 ## Without SDK (direct OTLP)\n\n\
                 ```bash\npip install {pip} opentelemetry-sdk opentelemetry-exporter-otlp-proto-http{extra_pkgs}\n```\n\n\
                 ```python\nfrom opentelemetry import trace\n\
                 from opentelemetry.sdk.trace import TracerProvider\n\
                 from opentelemetry.sdk.trace.export import BatchSpanProcessor\n\
                 from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter\n\n\
                 provider = TracerProvider()\n\
                 provider.add_span_processor(BatchSpanProcessor(OTLPSpanExporter(\n\
                     endpoint=\"{otlp}\"\n)))\n\
                 trace.set_tracer_provider(provider)\n\n\
                 {extra_setup}\n\n{snippet}\n```",
                pip = fw.pip_pkg,
                integration = fw.integration,
                snippet = fw.sdk_snippet,
                extra_pkgs = extra_pkgs,
                extra_setup = fw.no_sdk_extra_setup,
                otlp = otlp_url,
            )
        }
        None => format!(
            "## Generic OTLP Setup\n\n\
             ```bash\npip install opentelemetry-sdk opentelemetry-exporter-otlp-proto-http\n```\n\n\
             ```python\nfrom opentelemetry import trace\n\
             from opentelemetry.sdk.trace import TracerProvider\n\
             from opentelemetry.sdk.trace.export import BatchSpanProcessor\n\
             from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter\n\n\
             provider = TracerProvider()\n\
             provider.add_span_processor(BatchSpanProcessor(OTLPSpanExporter(\n\
                 endpoint=\"{otlp}\"\n)))\n\
             trace.set_tracer_provider(provider)\n```\n\n\
            Supported frameworks: {frameworks}",
            otlp = otlp_url,
            // Advertise directly from the table that resolves framework requests.
            frameworks = supported_framework_names().join(", "),
        ),
    }
}

fn ok_json(value: &impl serde::Serialize) -> Result<CallToolResult, McpError> {
    let json = serde_json::to_string(value).map_err(mcp_err)?;
    Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
}

fn mcp_err(e: impl std::fmt::Display) -> McpError {
    tracing::debug!(error = %e, "MCP tool error");
    McpError::internal_error(e.to_string(), None)
}

fn clamp_page(page: Option<u32>) -> u32 {
    page.unwrap_or(1).clamp(1, MAX_PAGE)
}

fn clamp_limit(limit: Option<u32>) -> u32 {
    limit.unwrap_or(20).clamp(1, MAX_PAGE_LIMIT)
}

fn parse_optional_ts(
    parameter: &str,
    value: Option<String>,
) -> Result<Option<DateTime<Utc>>, McpError> {
    value.map(|value| parse_ts(parameter, &value)).transpose()
}

struct TimeWindow {
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
}

fn parse_time_window(
    from_timestamp: Option<String>,
    to_timestamp: Option<String>,
) -> Result<TimeWindow, McpError> {
    let from_timestamp = parse_optional_ts("from_timestamp", from_timestamp)?;
    let to_timestamp = parse_optional_ts("to_timestamp", to_timestamp)?;
    if from_timestamp
        .zip(to_timestamp)
        .is_some_and(|(from, to)| from > to)
    {
        return Err(McpError::invalid_params(
            "from_timestamp must not be after to_timestamp",
            None,
        ));
    }
    Ok(TimeWindow {
        from: from_timestamp,
        to: to_timestamp,
    })
}

fn parse_ts(parameter: &str, value: &str) -> Result<DateTime<Utc>, McpError> {
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|_| {
            McpError::invalid_params(
                format!("invalid {parameter}: use an ISO 8601 timestamp"),
                None,
            )
        })
}

/// True when a request names a span but not the trace it belongs to.
///
/// A span id is 8 bytes and unique only within a trace, so on its own it can match spans in several
/// traces at once.
///
/// A session id is *not* a substitute, though it looks like one: the message query gives `span_id`
/// precedence and ignores `session_id` when both are set, so accepting the pair let exactly the
/// cross-trace merge this guard exists to stop back in - and the first version of this function
/// asserted that pair was fine.
fn span_lacks_its_trace(span_id: Option<&str>, trace_id: Option<&str>) -> bool {
    span_id.is_some() && trace_id.is_none()
}
#[cfg(test)]
#[path = "tools_tests.rs"]
mod tests;
