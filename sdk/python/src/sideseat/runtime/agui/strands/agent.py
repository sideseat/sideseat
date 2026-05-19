"""Strands single-Agent → AG-UI bridge: thin wrapper around `ag_ui_strands.StrandsAgent`.

We deliberately do NOT reimplement the Strands→AG-UI conversion for
single-agent runs. The upstream `ag-ui-protocol/ag-ui` repo ships an
`ag_ui_strands` adapter that already handles the full set of event
mappings, session persistence, tool proxying, etc. This module just
calls into it so SideSeat stays in sync with upstream improvements.

Lifecycle: `StrandsAgent.run` emits its own `RUN_STARTED` /
`RUN_FINISHED` envelopes. The SideSeat invoke handler trusts those and
does NOT pre-emit synthetic ones, keeping a single source of lifecycle
events on the agent path.
"""

from __future__ import annotations

from collections.abc import AsyncIterator
from typing import Any

from ag_ui.core import BaseEvent, RunAgentInput


async def strands_run_to_agui(
    agent: Any,
    run_input: RunAgentInput,
    *,
    name: str = "agent",
    description: str = "",
) -> AsyncIterator[BaseEvent]:
    """Drive `ag_ui_strands.StrandsAgent.run()` against the user's Strands
    agent and yield each AG-UI event verbatim.

    `StrandsAgent` is imported lazily so test suites that monkeypatch
    `sys.modules["ag_ui_strands"]` between tests pick up the freshest
    binding instead of the cached module-level import."""
    from ag_ui_strands import StrandsAgent  # noqa: PLC0415 — see docstring

    sa = StrandsAgent(agent=agent, name=name, description=description)
    async for event in sa.run(run_input):
        yield event
