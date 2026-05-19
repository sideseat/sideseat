"""Strands → AG-UI bridge.

Two converters drive a Strands runtime through AG-UI:

- :func:`strands_run_to_agui` wraps a single :class:`strands.Agent` via
  ``ag_ui_strands.StrandsAgent.run`` (the upstream library handles the
  full event-shape mapping).
- :func:`strands_multiagent_to_agui` drives a Strands ``Graph`` or ``Swarm``
  through ``stream_async`` and translates ``multiagent_node_*`` events into
  per-node ``STEP_STARTED``/``STEP_FINISHED`` blocks plus inner content via
  :class:`StrandsEventTranslator`.

Both converters yield AG-UI ``BaseEvent`` instances; the lifecycle invariant
is that exactly one ``RUN_STARTED`` and one terminal ``RUN_FINISHED``/
``RUN_ERROR`` reach the caller. The agent path delegates lifecycle to
``ag_ui_strands``; the multiagent path emits them itself.
"""

from sideseat.runtime.agui.strands.agent import strands_run_to_agui
from sideseat.runtime.agui.strands.multiagent import strands_multiagent_to_agui
from sideseat.runtime.agui.strands.translator import (
    StrandsEventTranslator,
    collect_text,
    translate_events,
)

__all__ = [
    "StrandsEventTranslator",
    "collect_text",
    "strands_multiagent_to_agui",
    "strands_run_to_agui",
    "translate_events",
]
