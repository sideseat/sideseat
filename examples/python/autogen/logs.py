"""AutoGen's other channel: its structured events as OTLP log records, with no span instrumentation.

`autogen_core` logs one event per model call to `autogen_core.events` - `LLMCallEvent` for a complete
response, `LLMStreamStartEvent` and `LLMStreamEndEvent` for a streamed one - and each event's `str()` is
JSON whose `type` member names it. A logging handler sends those records to an OTLP log exporter, so the
conversation arrives as log records rather than as span attributes.

Deliberately without the OpenInference instrumentor, which `native.py` installs: this mode exists to show
what the logging channel carries on its own. It therefore carries no agent or tool spans - AutoGen logs
model calls and nothing else - which is what the mode's declared `not_exported` facts record.
"""

import logging

from harness.telemetry import LogsTelemetry


def configure(logs: LogsTelemetry) -> None:
    # The root span still comes from OpenTelemetry, so a record can name the span it belongs to: a log
    # record with no span id attaches to no conversation, which is a property of the channel rather than
    # of this suite.
    logs.provider()
    events = logging.getLogger("autogen_core.events")
    events.addHandler(logs.handler())
    events.setLevel(logging.INFO)
