"""Built-in tool use over a scratch workspace.

Demonstrates:
- The Claude Code built-in tools (Read, Glob, Grep, Bash)
- allowed_tools auto-approval combined with a scoped cwd
- claude_code.tool spans nesting under claude_code.interaction
"""

import tempfile
from pathlib import Path

from agent_helpers import build_options, print_stream, trace_scope
from claude_agent_sdk import query


def _queries(workspace: Path) -> list[str]:
    """Prompts for the seeded workspace.

    The Read tool requires absolute paths, so the workspace path is interpolated. Left
    to guess, the model reaches for a bare "/config.py", Read fails with ENOENT and the
    span is flagged as an error — the model recovers, but every run shows up red in the
    UI. Telling it to use a relative path makes that worse, not better.
    """
    return [
        "How many Python files are in this directory? Use Glob.",
        f"Read {workspace / 'config.py'} and summarize what it configures in one sentence.",
        f"Which file under {workspace} mentions 'inventory'? Use Grep, then read that file.",
    ]


def _seed_workspace(root: Path) -> None:
    """Write a small, predictable tree so the run is reproducible."""
    (root / "config.py").write_text(
        'DATABASE_URL = "postgres://localhost/demo"\n'
        "POOL_SIZE = 10\n"
        "RETRY_ATTEMPTS = 3\n"
    )
    # The word "inventory" must appear in the body, not just the filename: Grep
    # matches file contents.
    (root / "inventory.py").write_text(
        '"""Warehouse inventory tracking."""\n\n'
        "ITEMS = {'widget': 12, 'gasket': 4}\n\n"
        "def restock(name, count):\n"
        "    ITEMS[name] = ITEMS.get(name, 0) + count\n"
    )
    (root / "README.md").write_text(
        "# Demo workspace\n\nA scratch tree for sample runs.\n"
    )


async def run(model, trace_attrs: dict, client, env: dict):
    with tempfile.TemporaryDirectory(prefix="sideseat-tool-use-") as tmp:
        workspace = Path(tmp)
        _seed_workspace(workspace)

        options = build_options(
            model,
            env,
            cwd=str(workspace),
            allowed_tools=["Read", "Glob", "Grep", "Bash"],
            system_prompt="You are a concise code explorer. Answer in one or two sentences.",
        )

        with trace_scope(client, "claude-agent-tool-use", trace_attrs):
            for prompt in _queries(workspace):
                print(f"--- {prompt} ---")
                await print_stream(query(prompt=prompt, options=options))
                print()
