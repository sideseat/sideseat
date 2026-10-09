"""The two ways a suite can configure telemetry, behind one interface.

``native`` is what a user of the framework would write following the framework's own documentation:
plain OpenTelemetry plus whatever the framework needs switched on, and nothing from SideSeat.
``sdk`` is the same program with ``sideseat.init``, and ``logs`` is a producer's own logging channel with
nothing else instrumented. Running a scenario in both of the first two modes and comparing
the captured telemetry is how the SDK is shown to add nothing wrong and lose nothing.
"""

from __future__ import annotations

import os
from collections.abc import Callable, Iterator, Sequence
from contextlib import AbstractContextManager, contextmanager
from typing import Any, Protocol
from urllib.parse import urlsplit

from opentelemetry import context as otel_context
from opentelemetry import trace as otel_trace
from opentelemetry.trace import Span


def traces_endpoint() -> str:
    """Where both modes export: the SideSeat project endpoint, or the capture recorder."""
    endpoint = (os.getenv("SIDESEAT_ENDPOINT") or "http://127.0.0.1:5388").rstrip("/")
    path = urlsplit(endpoint).path
    base = (
        endpoint
        if path and path != "/"
        else (f"{endpoint}/otel/{os.getenv('SIDESEAT_PROJECT_ID') or 'default'}")
    )
    return f"{base}/v1/traces"


def auth_headers() -> dict[str, str]:
    key = os.getenv("SIDESEAT_API_KEY")
    return {"Authorization": f"Bearer {key}"} if key else {}


class Telemetry(Protocol):
    mode: str

    def trace(self, name: str, *, session_id: str, user_id: str) -> Any: ...

    def shutdown(self) -> None: ...


class SdkTelemetry:
    mode = "sdk"

    def __init__(self, integrations: Sequence[str]) -> None:
        import sideseat

        self._sideseat = sideseat
        # Metrics stay off except in a `capture --metrics` run, which records them for the storage corpus.
        metrics = os.environ.get("SIDESEAT_CAPTURE_METRICS") == "1"
        self.client = sideseat.init(integrations=list(integrations), metrics=metrics)

    def trace(self, name: str, *, session_id: str, user_id: str) -> Any:
        return self._sideseat.trace(name, session_id=session_id, user_id=user_id)

    def shutdown(self) -> None:
        if not self._sideseat.shutdown():
            raise SystemExit("SideSeat could not export every span")


class NativeTelemetry:
    """Plain OpenTelemetry, configured the way each framework's documentation says to.

    A suite's ``native.py`` receives this object and calls :meth:`provider` (or builds the provider
    its framework owns and attaches :meth:`exporter` to it).
    """

    mode = "native"

    def __init__(self, service_name: str) -> None:
        self.service_name = service_name
        self._provider: Any = None
        self._trace: Callable[..., AbstractContextManager[Any]] | None = None
        self._shutdown: Callable[[], None] | None = None
        # Instrumentations record message content only when asked to; every native suite asks.
        os.environ.setdefault(
            "OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT", "true"
        )

    def exporter(self) -> Any:
        from opentelemetry.exporter.otlp.proto.http.trace_exporter import (
            OTLPSpanExporter,
        )

        return OTLPSpanExporter(endpoint=traces_endpoint(), headers=auth_headers())

    def provider(self) -> Any:
        """A tracer provider with an OTLP exporter, installed as the global provider."""
        from opentelemetry.sdk.resources import Resource
        from opentelemetry.sdk.trace import TracerProvider
        from opentelemetry.sdk.trace.export import BatchSpanProcessor

        if self._provider is None:
            provider = TracerProvider(
                resource=Resource.create({"service.name": self.service_name})
            )
            provider.add_span_processor(BatchSpanProcessor(self.exporter()))
            otel_trace.set_tracer_provider(provider)
            self._provider = provider
        return self._provider

    def adopt(self, provider: Any) -> None:
        """Use a provider the framework created, exporting from it with the standard exporter."""
        from opentelemetry.sdk.trace.export import BatchSpanProcessor

        provider.add_span_processor(BatchSpanProcessor(self.exporter()))
        self._provider = provider

    def hand_over(
        self,
        *,
        trace: Callable[..., AbstractContextManager[Any]],
        shutdown: Callable[[], None],
    ) -> None:
        """Let a framework that owns its tracer provider and span context open roots and flush.

        Laminar parents spans from an isolated context of its own, so a root span opened through
        OpenTelemetry would not be the parent of anything the framework records. ``trace`` is called
        as ``trace(name, session_id=..., user_id=...)``.
        """
        self._trace = trace
        self._shutdown = shutdown

    def trace(
        self, name: str, *, session_id: str, user_id: str
    ) -> AbstractContextManager[Any]:
        if self._trace is not None:
            return self._trace(name, session_id=session_id, user_id=user_id)
        return self._otel_trace(name, session_id=session_id, user_id=user_id)

    @contextmanager
    def _otel_trace(
        self, name: str, *, session_id: str, user_id: str
    ) -> Iterator[Span]:
        # Native OpenTelemetry has no session scope: the root span carries the identifiers, which is
        # what the framework documentation tells users to do.
        tracer = otel_trace.get_tracer("example")
        root = otel_trace.set_span_in_context(
            otel_trace.INVALID_SPAN, otel_context.get_current()
        )
        with tracer.start_as_current_span(
            name,
            context=root,
            attributes={"session.id": session_id, "user.id": user_id},
        ) as span:
            yield span

    def shutdown(self) -> None:
        if self._shutdown is not None:
            self._shutdown()
            return
        provider = self._provider or otel_trace.get_tracer_provider()
        flush: Callable[..., bool] | None = getattr(provider, "force_flush", None)
        if flush is not None and not flush(30_000):
            raise SystemExit("the native exporter could not export every span")
        shutdown = getattr(provider, "shutdown", None)
        if shutdown is not None:
            shutdown()


class LogsTelemetry:
    """A producer's **logging** channel: its own log records, exported as OTLP logs.

    For a producer that reports a conversation through structured log events rather than through span
    attributes. The suite's ``logs.py`` receives this object, calls :meth:`provider` so a record can name
    the span it belongs to, and attaches :meth:`handler` to the logger its framework writes to. Nothing
    else is instrumented, which is what makes the channel's own coverage measurable.
    """

    mode = "logs"

    def __init__(self, service_name: str) -> None:
        self._native = NativeTelemetry(service_name)
        self._logs: Any = None

    def provider(self) -> Any:
        """The tracer provider, so a record carries the trace and span ids of the turn it belongs to."""
        return self._native.provider()

    def handler(self) -> Any:
        """A logging handler whose records are exported as OTLP logs to the same endpoint."""
        from opentelemetry._logs import set_logger_provider
        from opentelemetry.exporter.otlp.proto.http._log_exporter import OTLPLogExporter
        from opentelemetry.sdk._logs import LoggerProvider, LoggingHandler
        from opentelemetry.sdk._logs.export import BatchLogRecordProcessor
        from opentelemetry.sdk.resources import Resource

        if self._logs is None:
            logs = LoggerProvider(
                resource=Resource.create({"service.name": self._native.service_name})
            )
            logs.add_log_record_processor(
                BatchLogRecordProcessor(
                    OTLPLogExporter(
                        endpoint=traces_endpoint().removesuffix("/v1/traces")
                        + "/v1/logs",
                        headers=auth_headers(),
                    )
                )
            )
            set_logger_provider(logs)
            self._logs = logs
        return LoggingHandler(logger_provider=self._logs)

    def trace(self, name: str, *, session_id: str, user_id: str) -> Any:
        return self._native.trace(name, session_id=session_id, user_id=user_id)

    def shutdown(self) -> None:
        self._native.shutdown()
        if self._logs is not None:
            self._logs.shutdown()
