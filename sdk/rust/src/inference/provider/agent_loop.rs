use super::*;

/// Hook callbacks for the agent loop.
#[async_trait]
pub trait AgentHooks: Send + Sync {
    /// Called before each step to inspect or modify the config (e.g. adjust temperature,
    /// filter active tools, or inject step-specific context).
    async fn prepare_step(&self, _step: usize, _config: &mut ProviderConfig) {}
    /// Return `true` to block a tool call — its result will be replaced with an
    /// "approval required" message and the agent will be asked again.
    async fn needs_approval(&self, _tool: &ToolUseBlock) -> bool {
        false
    }
    /// Called after each step completes (including all tool results for that step).
    /// Use for logging, metrics, or updating external state.
    async fn on_step_finish(&self, _step: &AgentStep) {}
}

/// No-op hooks (default).
pub struct DefaultHooks;

#[async_trait]
impl AgentHooks for DefaultHooks {}

/// Run an agent loop with hooks, step recording, and a `max_steps` limit.
///
/// Like [`run_agent_loop`], but:
/// - Returns [`AgentResult`] containing all intermediate [`AgentStep`]s, not just the final response.
/// - Calls [`AgentHooks`] callbacks on every step (`prepare_step`, `needs_approval`, `on_step_finish`).
/// - Stops after `max_steps` iterations and returns an error if exceeded.
/// - `tool_handler` returns `Vec<(id, Vec<ContentBlock>)>` — supports rich (image, audio) results.
pub async fn run_agent_loop_with_hooks<P, F, Fut, H>(
    provider: &P,
    mut messages: Vec<Message>,
    config: ProviderConfig,
    tool_handler: F,
    hooks: &H,
    max_steps: Option<usize>,
) -> Result<AgentResult, ProviderError>
where
    P: ChatProvider,
    F: Fn(Vec<ToolUseBlock>) -> Fut,
    Fut: Future<Output = Vec<(String, Vec<ContentBlock>)>> + Send,
    H: AgentHooks,
{
    let mut steps: Vec<AgentStep> = Vec::new();
    let mut step_n: usize = 0;

    loop {
        if let Some(max) = max_steps
            && step_n >= max
        {
            return Err(ProviderError::InvalidRequest(format!(
                "max_steps ({max}) exceeded without reaching EndTurn"
            )));
        }

        let mut step_config = config.clone();
        hooks.prepare_step(step_n, &mut step_config).await;

        // Apply active_tools filter to step_config (not config)
        let effective_config = if let Some(ref names) = step_config.active_tools.clone() {
            let mut c = step_config.clone();
            c.tools.retain(|t| names.contains(&t.name));
            c
        } else {
            step_config
        };

        let response = provider
            .complete(messages.clone(), effective_config)
            .await?;

        if response.stop_reason != StopReason::ToolUse || !response.has_tool_use() {
            return Ok(AgentResult {
                response,
                steps,
                messages,
            });
        }

        let tool_uses: Vec<ToolUseBlock> = response.tool_uses().into_iter().cloned().collect();

        // Apply approval hook
        let approved_tool_uses = tool_uses.clone();
        let mut precomputed_results: Vec<Option<(String, Vec<ContentBlock>)>> =
            vec![None; approved_tool_uses.len()];
        for (i, tu) in approved_tool_uses.iter().enumerate() {
            if hooks.needs_approval(tu).await {
                precomputed_results[i] = Some((
                    tu.id.clone(),
                    vec![ContentBlock::text("Tool call requires human approval")],
                ));
            }
        }

        // Split tools needing approval from those that don't
        let tools_needing_call: Vec<ToolUseBlock> = approved_tool_uses
            .iter()
            .zip(precomputed_results.iter())
            .filter(|(_, r)| r.is_none())
            .map(|(t, _)| t.clone())
            .collect();

        let handler_results = if tools_needing_call.is_empty() {
            vec![]
        } else {
            tool_handler(tools_needing_call.clone()).await
        };

        // Merge results in original order
        let mut handler_iter = handler_results.into_iter();
        let tool_results: Vec<(String, Vec<ContentBlock>)> = approved_tool_uses
            .iter()
            .zip(precomputed_results.iter())
            .map(|(tu, precomputed)| {
                if let Some(r) = precomputed {
                    r.clone()
                } else {
                    handler_iter
                        .next()
                        .unwrap_or_else(|| (tu.id.clone(), vec![]))
                }
            })
            .collect();

        messages.push(Message::with_content(
            Role::Assistant,
            response.content.clone(),
        ));
        messages.push(Message::with_tool_result_blocks(tool_results.clone()));

        let step = AgentStep {
            step_number: step_n,
            response,
            tool_uses,
            tool_results,
        };
        hooks.on_step_finish(&step).await;
        steps.push(step);
        step_n += 1;
    }
}

/// Run an agentic tool-call loop until the model stops requesting tools.
///
/// Loop: `complete` → append assistant turn → call `tool_handler` with all
/// [`ToolUseBlock`]s → append tool results → repeat until `stop_reason != ToolUse`.
///
/// Returns the final [`Response`] (the one that didn't request more tools).
/// For step-by-step tracing, approval gates, or rich (non-text) tool results,
/// use [`run_agent_loop_with_hooks`] instead.
///
/// # Example
/// ```no_run
/// # use sideseat::{ChatProvider, ProviderConfig, Message, ToolUseBlock, run_agent_loop};
/// # async fn example() -> Result<(), sideseat::ProviderError> {
/// # let provider = sideseat::mock::MockProvider::new();
/// let response = run_agent_loop(
///     &provider,
///     vec![Message::user("Search for Rust async tutorials")],
///     ProviderConfig::new("claude-haiku-4-5-20251001"),
///     |tools: Vec<ToolUseBlock>| async move {
///         tools.into_iter()
///             .map(|tu| (tu.id, format!("Result for {}", tu.name)))
///             .collect()
///     },
/// ).await?;
/// println!("{}", response.first_text().unwrap_or(""));
/// # Ok(()) }
/// ```
pub async fn run_agent_loop<P, F, Fut>(
    provider: &P,
    messages: Vec<Message>,
    config: ProviderConfig,
    tool_handler: F,
) -> Result<Response, ProviderError>
where
    P: ChatProvider,
    F: Fn(Vec<ToolUseBlock>) -> Fut,
    Fut: Future<Output = Vec<(String, String)>> + Send,
{
    Ok(run_agent_loop_with_hooks(
        provider,
        messages,
        config,
        move |tools| {
            let fut = tool_handler(tools);
            async move {
                fut.await
                    .into_iter()
                    .map(|(id, text)| (id, vec![ContentBlock::text(text)]))
                    .collect()
            }
        },
        &DefaultHooks,
        None,
    )
    .await?
    .response)
}
