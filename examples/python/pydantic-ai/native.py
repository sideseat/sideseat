"""Pydantic AI's documented OpenTelemetry setup without Logfire: a global provider and instrument_all."""

from pydantic_ai import Agent

from harness.telemetry import NativeTelemetry


def configure(native: NativeTelemetry) -> None:
    native.provider()
    Agent.instrument_all()
