"""SideSeat WS bridge: pure-Strands presence demo.

Demonstrates `client.agent(agent, name=...)` + `client.connect()` against a
running SideSeat server. The script blocks on `connect()`; the server exposes
the agent via `GET /api/v1/project/{project_id}/registrations` and the
`presence:{project_id}` topic.

This sample does NOT invoke the agent. Other samples (e.g. `tool_use.py`)
already cover invocation paths and span emission. The goal here is to show
how an SDK-side process makes itself discoverable.
"""

from __future__ import annotations

from strands import Agent, tool


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
    )

    client.register([agent]).connect()  # connect() prints a banner then blocks
