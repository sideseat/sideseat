"""Shared helpers for Claude Agent SDK samples.

The trace scope and message-stream printing are identical across samples, so they
live here rather than being repeated nine times.
"""

from contextlib import nullcontext

from claude_agent_sdk import (
    AssistantMessage,
    ClaudeAgentOptions,
    ResultMessage,
    TextBlock,
    ThinkingBlock,
    ToolResultBlock,
    ToolUseBlock,
    UserMessage,
)

# Keep runs bounded so a sample can't loop indefinitely against Bedrock.
DEFAULT_MAX_TURNS = 8


def trace_scope(client, name: str, trace_attrs: dict):
    """Open a SideSeat root span, or a no-op when running without --sideseat.

    The Agent SDK reads the active span and injects TRACEPARENT into the CLI
    subprocess, so the agent run nests under this span.
    """
    if client is None:
        return nullcontext()
    return client.trace(
        name,
        session_id=trace_attrs.get("session.id"),
        user_id=trace_attrs.get("user.id"),
    )


def build_options(model, env: dict, **overrides) -> ClaudeAgentOptions:
    """Build ClaudeAgentOptions with the sample defaults applied.

    `env` merges on top of the inherited process environment in Python (unlike the
    TypeScript SDK, where it replaces the environment wholesale).
    """
    options = {
        "model": model.model_id,
        "env": env,
        "max_turns": DEFAULT_MAX_TURNS,
        # Ignore the developer's own ~/.claude and any project settings so samples
        # behave identically on every machine.
        "setting_sources": [],
        "stderr": _print_stderr,
    }
    options.update(overrides)
    return ClaudeAgentOptions(**options)


def _print_stderr(line: str) -> None:
    """Surface CLI diagnostics, including OTLP exporter failures."""
    if line.strip():
        print(f"  [cli] {line.rstrip()}")


async def print_stream(stream, show_thinking: bool = False) -> ResultMessage | None:
    """Print an Agent SDK message stream and return the final ResultMessage."""
    result = None
    async for message in stream:
        if isinstance(message, AssistantMessage):
            for block in message.content:
                if isinstance(block, TextBlock):
                    print(block.text)
                elif isinstance(block, ThinkingBlock) and show_thinking:
                    print(f"  [thinking] {block.thinking[:1000]}")
                elif isinstance(block, ToolUseBlock):
                    print(f"  [tool] {block.name} {_preview(block.input)}")
        elif isinstance(message, UserMessage):
            # Tool results come back on a UserMessage, not an AssistantMessage.
            blocks = message.content if isinstance(message.content, list) else []
            for block in blocks:
                if isinstance(block, ToolResultBlock):
                    label = "error" if block.is_error else "result"
                    print(f"  [{label}] {_preview(block.content)}")
        elif isinstance(message, ResultMessage):
            result = message
            print_result(message)
    return result


def print_result(message: ResultMessage) -> None:
    """Print the cost and token summary from a ResultMessage.

    total_cost_usd is a client-side estimate, not billing data. Note that per-step
    output_tokens on assistant messages is a placeholder; only the result message
    carries the real output count.
    """
    usage = message.usage or {}
    print(
        f"  [usage] turns={message.num_turns} "
        f"in={usage.get('input_tokens', 0)} "
        f"out={usage.get('output_tokens', 0)} "
        f"cache_read={usage.get('cache_read_input_tokens', 0)} "
        f"cost=${message.total_cost_usd or 0:.6f}"
    )


def _preview(value, limit: int = 120) -> str:
    text = str(value).replace("\n", " ")
    return text if len(text) <= limit else f"{text[:limit]}..."
