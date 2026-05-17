"""Strands → AG-UI bridge: thin wrapper around `ag_ui_strands.StrandsAgent`.

We deliberately do NOT reimplement the Strands→AG-UI conversion. The
upstream `ag-ui-protocol/ag-ui` repo ships an `ag_ui_strands` adapter
(integrations/aws-strands/python) that already handles the full set of
event mappings, session persistence, tool proxying, etc. This module just
calls into it so SideSeat stays in sync with upstream improvements.

The orchestrator (`RuntimeClient._run_invoke_async`) owns the surrounding
`RUN_STARTED` / `RUN_FINISHED` envelope and prefers to emit those itself
to keep the wire shape consistent across runtime kinds. `ag_ui_strands`
already emits its own RUN_STARTED/FINISHED, so we let those flow through
verbatim — they replace ours from the consumer's perspective and the
client tolerates duplicates.
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
