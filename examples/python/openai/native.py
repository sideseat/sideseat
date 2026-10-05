"""Logfire's documented OpenAI setup, exporting to an OTLP endpoint instead of Logfire's cloud.

The SideSeat ``openai`` integration runs ``logfire.instrument_openai``, so this is the setup it
replaces. OpenTelemetry's own ``opentelemetry-instrumentation-openai-v2`` would not be a comparable
pair: it does not trace the Responses API, and by default records message content only as log events.
"""

from typing import Any

import logfire
from opentelemetry.sdk.trace.export import BatchSpanProcessor

from harness.telemetry import NativeTelemetry


def _keep_session_id(match: logfire.ScrubMatch) -> Any:
    # Logfire redacts every attribute whose name mentions "session". A session id is not a secret,
    # and redacting it puts every conversation in one session called "[Scrubbed due to 'session']".
    return match.value if match.path == ("attributes", "session.id") else None


def configure(native: NativeTelemetry) -> None:
    logfire.configure(
        service_name=native.service_name,
        send_to_logfire=False,
        console=False,
        scrubbing=logfire.ScrubbingOptions(callback=_keep_session_id),
        additional_span_processors=[BatchSpanProcessor(native.exporter())],
    )
    logfire.instrument_openai()
