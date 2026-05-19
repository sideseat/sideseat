"""SideSeat SDK AG-UI integration.

The renderer files (`renderer`, `format_table`, `block_renderers`, `state`,
`stream_buffer`, `snapshot_dedup`, `config`) are adapted verbatim from
`observability-talk/demos/agent/agui/`. The Strands converters
(`strands_run_to_agui`, `strands_multiagent_to_agui`) live under
`sideseat.runtime.agui.strands`.

This package is imported lazily on the first `agent.invoke` so that users
without the `[agui]` extra never trip an `ImportError` at SDK startup.
"""

from sideseat.runtime.agui.config import RenderConfig
from sideseat.runtime.agui.renderer import AgUiRenderer
from sideseat.runtime.agui.strands import (
    strands_multiagent_to_agui,
    strands_run_to_agui,
)

__all__ = [
    "AgUiRenderer",
    "RenderConfig",
    "strands_multiagent_to_agui",
    "strands_run_to_agui",
]
