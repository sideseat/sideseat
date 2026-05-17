"""SideSeat WS bridge: Strands presence + AG-UI invoke demo.

Registers a Strands agent with SideSeat over the persistent WebSocket and
then waits. While idle, the agent shows up in
`GET /api/v1/project/{project_id}/registrations` and on the
`presence:{project_id}` broadcast topic.

When a frontend (or `curl`) hits the AG-UI run-agent endpoint
(`POST /api/v1/project/{project_id}/agents/{name}/runs`), the SDK invokes
the local Strands agent through `ag_ui_strands.StrandsAgent`, streams
AG-UI events back over the same WebSocket, and paints the same stream
in this terminal with the rich `AgUiRenderer`. See
`strands_ws_invoke.md` for a copy-paste curl command.
"""

from __future__ import annotations

from strands import Agent, tool
from strands.handlers.callback_handler import null_callback_handler


@tool
def temperature_forecast(city: str, days: int = 3) -> dict:
    """Get the temperature forecast for a city."""
    return {
        "status": "success",
        "content": [{"type": "text", "text": f"{city}: clear, 22C, {days}d"}],
    }


@tool
def precipitation_forecast(city: str, days: int = 3) -> dict:
    """Get the precipitation forecast for a city."""
    return {
        "status": "success",
        "content": [{"type": "text", "text": f"{city}: low chance, {days}d"}],
    }


def run(model, trace_attrs: dict, *, client=None) -> None:
    if client is None:
        raise RuntimeError(
            "strands_ws sample requires --sideseat (the WS bridge lives on the SDK)"
        )

    # The trace attribute key MUST match the registration name so OTLP spans
    # and the WS-presence record cross-reference cleanly in the UI.
    name = "weather"
    trace_attrs = {**(trace_attrs or {}), "gen_ai.agent.name": name}

    agent = Agent(
        model=model,
        tools=[temperature_forecast, precipitation_forecast],
        system_prompt="You are a friendly weather agent.",
        trace_attributes=trace_attrs,
        name=name,  # used by SideSeat.register() to derive the identity
        # Mute Strands' default printing — `_run_invoke_async` mutes it
        # again at invoke time, but pre-empting here keeps the registry
        # banner clean too.
        callback_handler=null_callback_handler,
    )

    client.register([agent]).connect()  # connect() prints a banner then blocks
