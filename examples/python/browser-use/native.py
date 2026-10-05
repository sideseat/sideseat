"""Browser Use's documented observability: Laminar, exporting over OTLP to another backend.

Browser Use instruments itself with Laminar's ``observe`` decorators and spans. With no Laminar
project key, Laminar builds an OTLP exporter from the standard ``OTEL_EXPORTER_OTLP_*`` variables. It
owns the tracer provider and an isolated span context, so a scenario's root spans are Laminar spans
carrying the session and user.
"""

import os
import sys
from collections.abc import Iterator
from contextlib import contextmanager
from typing import Any

from harness.telemetry import NativeTelemetry, auth_headers, traces_endpoint


def configure(native: NativeTelemetry) -> None:
    for key in ("LMNR_PROJECT_API_KEY", "LMNR_BASE_URL"):
        if os.getenv(key):
            raise SystemExit(
                f"unset {key}: a native capture exports to SideSeat, not Laminar"
            )
    # Laminar takes the service name from sys.argv[0] when it is imported, and ignores
    # OTEL_SERVICE_NAME; the program here is an entry-point script whose path names the checkout.
    sys.argv[0] = native.service_name
    from lmnr import Instruments, Laminar

    os.environ.update(
        {
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT": traces_endpoint(),
            "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL": "http/protobuf",
            "LMNR_TRACE_CONTENT": "true",
        }
    )
    if headers := auth_headers():
        os.environ["OTEL_EXPORTER_OTLP_TRACES_HEADERS"] = ",".join(
            f"{key}={value}" for key, value in headers.items()
        )
    # Bubus carries Browser Use's event structure; the suite's model client is the Anthropic SDK.
    Laminar.initialize(
        instruments={Instruments.BUBUS, Instruments.ANTHROPIC},
        force_http=True,
    )
    native.hand_over(trace=_trace, shutdown=_shutdown)


@contextmanager
def _trace(name: str, *, session_id: str, user_id: str) -> Iterator[Any]:
    from lmnr import Laminar

    with Laminar.start_as_current_span(
        name, session_id=session_id, user_id=user_id
    ) as span:
        yield span


def _shutdown() -> None:
    from lmnr import Laminar

    Laminar.flush()
    Laminar.shutdown()
