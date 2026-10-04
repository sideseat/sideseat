"""Options every scenario shares, and printing of the Agent SDK's message stream."""

from __future__ import annotations

import os
import re
import tempfile
from collections.abc import AsyncIterable
from pathlib import Path
from typing import Any

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

from harness import Run, content

# The CLI inherits this process's environment. Run from inside a Claude Code session, that holds the
# session's own variables, which would pick the child's model and effort, truncate the content its
# telemetry records, and attach it to the parent session; none of them belong to the scenario.
_HOST_SESSION = re.compile(
    r"CLAUDECODE|CLAUDE_PID|CLAUDE_EFFORT|ANTHROPIC_DEFAULT_\w+_MODEL"
    r"|CLAUDE_CODE_(SESSION|CHILD|MESSAGING|ENTRYPOINT|EXECPATH|PATH|OTEL_CONTENT|ENABLE_AUTO)\w*"
)
for _name in [name for name in os.environ if _HOST_SESSION.fullmatch(name)]:
    del os.environ[_name]

# Without these the CLI adds the developer's memory files, CLAUDE.md instructions, and git guidance
# to the first user turn: private, machine-specific context that is not part of the scenario.
_ISOLATED = {
    "CLAUDE_CODE_DISABLE_AUTO_MEMORY": "1",
    "CLAUDE_CODE_DISABLE_CLAUDE_MDS": "1",
    "CLAUDE_CODE_DISABLE_GIT_INSTRUCTIONS": "1",
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
}

#: Outside any repository, so the CLI's environment summary names no project.
WORKSPACE = Path(tempfile.gettempdir()) / "sideseat-claude-agent-sdk"


def options(run: Run, **overrides: Any) -> ClaudeAgentOptions:
    """Options for a run on the harness model, with the shared system prompt and no built-in tools.

    ``setting_sources=[]`` ignores the developer's ``~/.claude`` and any project settings, so a
    capture does not depend on the machine it ran on.
    """
    WORKSPACE.mkdir(exist_ok=True)
    settings: dict[str, Any] = {
        "model": run.llm.id,
        "env": {**_ISOLATED, **run.llm.env},
        "cwd": WORKSPACE,
        "system_prompt": content.SYSTEM,
        "tools": [],
        "setting_sources": [],
        "max_turns": 8,
        "stderr": _print_stderr,
    }
    settings.update(overrides)
    return ClaudeAgentOptions(**settings)


async def show(stream: AsyncIterable[Any]) -> ResultMessage:
    """Print a message stream and return its result, failing if the run did not succeed."""
    result: ResultMessage | None = None
    async for message in stream:
        if isinstance(message, AssistantMessage):
            for block in message.content:
                if isinstance(block, TextBlock):
                    print(block.text)
                elif isinstance(block, ThinkingBlock):
                    print(f"  [thinking] {block.thinking[:200]}")
                elif isinstance(block, ToolUseBlock):
                    print(f"  [tool] {block.name} {block.input}")
        elif isinstance(message, UserMessage) and isinstance(message.content, list):
            for block in message.content:
                if isinstance(block, ToolResultBlock):
                    print(
                        f"  [{'error' if block.is_error else 'result'}] {block.content}"
                    )
        elif isinstance(message, ResultMessage):
            result = message
    if result is None or result.is_error:
        raise RuntimeError(f"the agent run did not succeed: {result}")
    return result


def _print_stderr(line: str) -> None:
    if line.strip():
        print(f"  [cli] {line.rstrip()}")
