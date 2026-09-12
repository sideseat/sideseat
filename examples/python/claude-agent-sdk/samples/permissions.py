"""Tool permission control.

Demonstrates:
- can_use_tool, invoked only when the permission flow falls through to a prompt
- PermissionResultAllow with updated_input to rewrite a call
- PermissionResultDeny to block one
- disallowed_tools with a scoped rule, which denies in every mode

NOTE: do not also list a gated tool in allowed_tools. Allow rules approve the call
before can_use_tool runs, so the callback would never fire for it.
"""

import tempfile
from pathlib import Path

from agent_helpers import build_options, print_stream, trace_scope
from claude_agent_sdk import PermissionResultAllow, PermissionResultDeny, query
from claude_agent_sdk.types import ToolPermissionContext

# Relative paths matter: the permission callback redirects into a reviewed/ subdir
# resolved against cwd. An absolute path would land outside the sample workspace.
PROMPT = (
    "Using relative paths in the current directory, create notes.txt containing "
    "'hello', then create secrets.txt containing 'token=abc123'. "
    "Report what happened for each."
)


async def gate_tools(
    tool_name: str,
    input_data: dict,
    context: ToolPermissionContext,
) -> PermissionResultAllow | PermissionResultDeny:
    """Allow writes, except anything that looks like a secrets file."""
    path = str(input_data.get("file_path", ""))

    if tool_name == "Write" and "secret" in path.lower():
        print(f"  [permission] DENY {tool_name} -> {path}")
        return PermissionResultDeny(message="Writing secrets files is not allowed")

    if tool_name == "Write" and path:
        # Redirect every write into a reviewed/ subdirectory.
        redirected = str(Path(path).parent / "reviewed" / Path(path).name)
        print(f"  [permission] ALLOW {tool_name} -> {redirected} (redirected)")
        return PermissionResultAllow(
            updated_input={**input_data, "file_path": redirected}
        )

    print(f"  [permission] ALLOW {tool_name}")
    return PermissionResultAllow(updated_input=input_data)


async def run(model, trace_attrs: dict, client, env: dict):
    with tempfile.TemporaryDirectory(prefix="sideseat-permissions-") as tmp:
        workspace = Path(tmp)
        (workspace / "reviewed").mkdir()

        options = build_options(
            model,
            env,
            cwd=str(workspace),
            # Write is deliberately absent from allowed_tools so gate_tools runs.
            allowed_tools=["Read", "Glob"],
            # A scoped rule denies matching calls even under bypassPermissions.
            disallowed_tools=["Bash(rm *)"],
            can_use_tool=gate_tools,
            system_prompt="You are a careful file assistant.",
        )

        with trace_scope(client, "claude-agent-permissions", trace_attrs):
            print(f"--- {PROMPT} ---")
            await print_stream(query(prompt=PROMPT, options=options))

        written = sorted(p.name for p in (workspace / "reviewed").iterdir())
        print(f"\nFiles in reviewed/: {written or 'none'}")
