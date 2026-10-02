#!/usr/bin/env python3
"""Emit one canonical conversation through SideSeat SDK or raw OpenTelemetry."""

from __future__ import annotations

import argparse
import os
from collections.abc import Callable
from typing import Any

from opentelemetry import trace
from opentelemetry.context import Context
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
from opentelemetry.sdk.resources import Resource
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor
from opentelemetry.trace import SpanKind

SESSION_ID = "sdk-conformance-session"
USER_ID = "sdk-conformance-user"
INPUT_MESSAGES = (
    '[{"role":"user","parts":[{"type":"text",'
    '"content":"What is the weather in London?"}]}]'
)
FIRST_OUTPUT = (
    '[{"role":"assistant","parts":[{"type":"text",'
    '"content":"I will check the weather."}],"finish_reason":"tool_calls"}]'
)
FINAL_OUTPUT = (
    '[{"role":"assistant","parts":[{"type":"text",'
    '"content":"It is 18°C and sunny in London."}],"finish_reason":"stop"}]'
)


def add_first_model_call(set_attribute: Callable[[str, Any], None]) -> None:
    set_attribute("gen_ai.operation.name", "chat")
    set_attribute("gen_ai.provider.name", "conformance")
    set_attribute("gen_ai.request.model", "canonical-model")
    set_attribute("gen_ai.input.messages", INPUT_MESSAGES)
    set_attribute("gen_ai.output.messages", FIRST_OUTPUT)
    set_attribute("gen_ai.usage.input_tokens", 12)
    set_attribute("gen_ai.usage.output_tokens", 6)


def add_tool_call(set_attribute: Callable[[str, Any], None]) -> None:
    set_attribute("gen_ai.operation.name", "execute_tool")
    set_attribute("gen_ai.tool.name", "get_weather")
    set_attribute("gen_ai.tool.call.id", "call-weather-1")
    set_attribute("gen_ai.tool.type", "function")
    set_attribute("gen_ai.tool.call.arguments", '{"city":"London"}')
    set_attribute(
        "gen_ai.tool.call.result",
        '{"temperature_c":18,"condition":"sunny"}',
    )


def add_final_model_call(set_attribute: Callable[[str, Any], None]) -> None:
    set_attribute("gen_ai.operation.name", "chat")
    set_attribute("gen_ai.provider.name", "conformance")
    set_attribute("gen_ai.request.model", "canonical-model")
    set_attribute("gen_ai.output.messages", FINAL_OUTPUT)
    set_attribute("gen_ai.response.finish_reasons", '["stop"]')
    set_attribute("gen_ai.usage.input_tokens", 24)
    set_attribute("gen_ai.usage.output_tokens", 10)


def emit_children(span_factory: Callable[..., Any]) -> None:
    with span_factory("chat canonical-model", kind=SpanKind.CLIENT) as span:
        add_first_model_call(span.set_attribute)
    with span_factory("execute_tool get_weather") as span:
        add_tool_call(span.set_attribute)
    with span_factory("chat canonical-model", kind=SpanKind.CLIENT) as span:
        add_final_model_call(span.set_attribute)


def run_with_sideseat() -> None:
    import sideseat

    client = sideseat.init(
        service_name="python-conformance", integrations=[], metrics=False, logs=False
    )
    with client.trace("canonical-agent-run", session_id=SESSION_ID, user_id=USER_ID):
        emit_children(client.span)
    if not client.shutdown():
        raise RuntimeError("SideSeat Python SDK did not export its spans")


def run_with_opentelemetry() -> None:
    endpoint = os.getenv("SIDESEAT_ENDPOINT", "http://127.0.0.1:5388").rstrip("/")
    project = os.getenv("SIDESEAT_PROJECT_ID", "default")
    provider = TracerProvider(
        resource=Resource.create(
            {
                "service.name": "python-conformance",
                "sideseat.framework": "python-conformance",
            }
        )
    )
    provider.add_span_processor(
        BatchSpanProcessor(
            OTLPSpanExporter(endpoint=f"{endpoint}/otel/{project}/v1/traces")
        )
    )
    tracer = provider.get_tracer("SideSeat.Conformance.Raw")

    with tracer.start_as_current_span(
        "canonical-agent-run",
        context=Context(),
        attributes={"session.id": SESSION_ID, "user.id": USER_ID},
    ):

        def raw_span(name: str, **kwargs: Any) -> Any:
            attributes = {
                "session.id": SESSION_ID,
                "user.id": USER_ID,
            }
            return tracer.start_as_current_span(name, attributes=attributes, **kwargs)

        emit_children(raw_span)

    if not provider.force_flush():
        raise RuntimeError("raw OpenTelemetry provider did not flush its spans")
    provider.shutdown()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=("sdk", "otel"))
    args = parser.parse_args()
    if args.mode == "sdk":
        run_with_sideseat()
    else:
        run_with_opentelemetry()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
