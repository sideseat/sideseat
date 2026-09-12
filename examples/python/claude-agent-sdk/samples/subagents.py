"""Subagent delegation.

Demonstrates:
- Programmatic subagents via AgentDefinition
- Delegation nesting: a subagent's llm_request and tool spans appear under the
  parent's claude_code.tool span, so the whole chain is one trace in SideSeat

NOTE: AgentDefinition is a dataclass with camelCase field names (disallowedTools,
maxTurns, permissionMode). Passing snake_case raises TypeError at construction.
"""

import tempfile
from pathlib import Path

from agent_helpers import build_options, print_stream, trace_scope
from claude_agent_sdk import AgentDefinition, query

PROMPT = (
    "Review the Python files in this directory. Use the style-reviewer subagent to "
    "check naming and the docs-reviewer subagent to check docstrings, then give me a "
    "combined two-bullet summary."
)


def _seed_workspace(root: Path) -> None:
    (root / "orders.py").write_text(
        "def ProcessOrder(x, y):\n"
        "    return {'total': x * y}\n\n"
        "def cancel(order_id):\n"
        '    """Cancel an order."""\n'
        "    return True\n"
    )
    (root / "shipping.py").write_text(
        "RATES = {'ground': 5.0, 'air': 18.5}\n\n"
        "def quote(weight_kg, method):\n"
        "    return weight_kg * RATES[method]\n"
    )


async def run(model, trace_attrs: dict, client, env: dict):
    with tempfile.TemporaryDirectory(prefix="sideseat-subagents-") as tmp:
        workspace = Path(tmp)
        _seed_workspace(workspace)

        options = build_options(
            model,
            env,
            cwd=str(workspace),
            allowed_tools=["Read", "Glob", "Grep", "Task"],
            agents={
                "style-reviewer": AgentDefinition(
                    description="Reviews naming and formatting conventions.",
                    prompt=(
                        "You review Python naming conventions. Report only concrete "
                        "issues, one line each."
                    ),
                    tools=["Read", "Glob", "Grep"],
                    maxTurns=4,
                ),
                "docs-reviewer": AgentDefinition(
                    description="Reviews docstring coverage and quality.",
                    prompt=(
                        "You review Python docstrings. Report only functions missing "
                        "or with inadequate docstrings, one line each."
                    ),
                    tools=["Read", "Glob", "Grep"],
                    maxTurns=4,
                ),
            },
            system_prompt="Delegate the review work to your subagents, then summarize.",
        )

        with trace_scope(client, "claude-agent-subagents", trace_attrs):
            print(f"--- {PROMPT} ---")
            await print_stream(query(prompt=PROMPT, options=options))
