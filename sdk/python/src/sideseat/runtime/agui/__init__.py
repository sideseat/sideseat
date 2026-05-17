"""SideSeat SDK AG-UI integration.

The renderer files (`renderer`, `format_table`, `block_renderers`, `state`,
`stream_buffer`, `snapshot_dedup`, `config`) are adapted verbatim from
`observability-talk/demos/agent/agui/`. The converter (`strands_run_to_agui`)
is in-tree.

This package is imported lazily on the first `agent.invoke` so that users
without the `[agui]` extra never trip an `ImportError` at SDK startup.
"""

from sideseat.runtime.agui.config import RenderConfig
from sideseat.runtime.agui.converter import strands_run_to_agui
from sideseat.runtime.agui.renderer import AgUiRenderer

__all__ = ["AgUiRenderer", "RenderConfig", "strands_run_to_agui"]
