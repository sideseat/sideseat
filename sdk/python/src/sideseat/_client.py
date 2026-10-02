"""The SideSeat client: builds the OpenTelemetry pipeline and owns its lifecycle."""

from __future__ import annotations

import atexit
import functools
import inspect
import logging
import threading
from collections.abc import Callable, Iterator, Mapping
from contextlib import ExitStack, contextmanager
from typing import TYPE_CHECKING, Any, TypeVar, cast

from opentelemetry import context as otel_context
from opentelemetry import trace as otel_trace
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.trace import Span, SpanKind, Tracer

from sideseat import _context, _resource
from sideseat._config import Settings
from sideseat._version import __version__
from sideseat.errors import ConfigurationError, IntegrationError
from sideseat.integrations import Integration, SetupContext, resolve
from sideseat.integrations._util import GENAI_CAPTURE_CONTENT, default_env

if TYPE_CHECKING:
    from sideseat.runtime import RuntimeClient

logger = logging.getLogger("sideseat")

F = TypeVar("F", bound=Callable[..., Any])

_DEFAULT_TIMEOUT_MILLIS = 30_000


class SideSeat:
    """A configured telemetry pipeline. Create it with :func:`sideseat.init`."""

    def __init__(self, settings: Settings) -> None:
        self._settings = settings
        self._integrations: list[Integration] = []
        self._owned_providers: list[Any] = []
        self._logging_handler: logging.Handler | None = None
        self._shutdown_lock = threading.Lock()
        self._shut_down = False
        self._tracer_provider: Any = otel_trace.NoOpTracerProvider()
        self._logger_provider: Any = None
        self._meter_provider: Any = None
        self._runtime: RuntimeClient | None = None
        if settings.debug:
            logging.getLogger("sideseat").setLevel(logging.DEBUG)
        if not settings.disabled:
            self._start()
        self._tracer: Tracer = self._tracer_provider.get_tracer("sideseat", __version__)
        atexit.register(self.shutdown)

    # -- pipeline ------------------------------------------------------------------------------

    def _start(self) -> None:
        settings = self._settings
        candidates, explicit = resolve(settings.integrations)
        primary = candidates[0].installed_package() if candidates else None
        service_name = settings.service_name or (primary[0] if primary else "sideseat-app")
        service_version = settings.service_version or (primary[1] if primary else __version__)
        resource = _resource.build(
            service_name=service_name,
            service_version=service_version,
            integrations=[i.name for i in candidates],
            extra=settings.resource_attributes,
        )
        ctx = SetupContext(
            settings=settings,
            resource=resource,
            service_name=service_name,
            service_version=service_version,
        )
        if settings.capture_content:
            default_env(GENAI_CAPTURE_CONTENT, "true")

        for integration in candidates:
            if self._guard(
                integration, explicit, "prepare", functools.partial(integration.prepare, ctx)
            ):
                self._integrations.append(integration)

        owners = [i for i in self._integrations if i.owns_tracer_provider]
        if len(owners) > 1:
            names = ", ".join(i.name for i in owners)
            raise ConfigurationError(f"only one integration can own the tracer provider: {names}")
        provider = self._owner_tracer_provider(owners[0], explicit, ctx) if owners else None
        if provider is None:
            owners = []
            provider = self._default_tracer_provider(ctx)

        provider.add_span_processor(_context.CorrelationProcessor())
        for integration in self._integrations:
            for processor in integration.span_processors(ctx):
                provider.add_span_processor(processor)
        for processor in settings.span_processors:
            provider.add_span_processor(processor)
        if settings.export and not ctx.notes.get("exporter_installed"):
            from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
            from opentelemetry.sdk.trace.export import BatchSpanProcessor

            exporter = OTLPSpanExporter(
                endpoint=settings.signal_endpoint("traces"), headers=settings.headers()
            )
            provider.add_span_processor(BatchSpanProcessor(exporter))
        self._tracer_provider = provider
        ctx.tracer_provider = provider

        if settings.logs:
            self._logger_provider = self._start_logs(resource)
        if settings.metrics and not owners:
            self._meter_provider = self._start_metrics(resource)
        ctx.logger_provider = self._logger_provider
        ctx.meter_provider = self._meter_provider

        for integration in list(self._integrations):
            if not self._guard(
                integration, explicit, "instrument", functools.partial(integration.instrument, ctx)
            ):
                self._integrations.remove(integration)
        logger.debug(
            "SideSeat exporting to %s with integrations %s",
            settings.otlp_base,
            [i.name for i in self._integrations],
        )

    def _owner_tracer_provider(self, owner: Integration, explicit: bool, ctx: SetupContext) -> Any:
        created: list[Any] = []
        if not self._guard(
            owner,
            explicit,
            "create its tracer provider",
            lambda: created.append(owner.create_tracer_provider(ctx)),
        ):
            self._integrations.remove(owner)
            return None
        self._owned_providers.append(created[0])
        return created[0]

    def _default_tracer_provider(self, ctx: SetupContext) -> Any:
        existing = otel_trace.get_tracer_provider()
        if isinstance(existing, TracerProvider):
            # The application configured OpenTelemetry itself: add to its provider rather than
            # replace it, and leave its shutdown to the application.
            return existing
        provider = TracerProvider(resource=ctx.resource)
        otel_trace.set_tracer_provider(provider)
        self._owned_providers.append(provider)
        return provider

    def _start_logs(self, resource: Any) -> Any:
        from opentelemetry import _logs
        from opentelemetry.exporter.otlp.proto.http._log_exporter import OTLPLogExporter
        from opentelemetry.sdk._logs import LoggerProvider, LoggingHandler
        from opentelemetry.sdk._logs.export import BatchLogRecordProcessor

        settings = self._settings
        provider = _logs.get_logger_provider()
        if not isinstance(provider, LoggerProvider):
            provider = LoggerProvider(resource=resource)
            _logs.set_logger_provider(provider)
            self._owned_providers.append(provider)
        if settings.export:
            exporter = OTLPLogExporter(
                endpoint=settings.signal_endpoint("logs"), headers=settings.headers()
            )
            provider.add_log_record_processor(BatchLogRecordProcessor(exporter))
        if settings.capture_python_logs:
            self._logging_handler = LoggingHandler(logger_provider=provider)
            logging.getLogger().addHandler(self._logging_handler)
        return provider

    def _start_metrics(self, resource: Any) -> Any:
        from opentelemetry import metrics
        from opentelemetry.sdk.metrics import MeterProvider

        settings = self._settings
        if isinstance(metrics.get_meter_provider(), MeterProvider) or not settings.export:
            return metrics.get_meter_provider()
        from opentelemetry.exporter.otlp.proto.http.metric_exporter import OTLPMetricExporter
        from opentelemetry.sdk.metrics.export import PeriodicExportingMetricReader

        exporter = OTLPMetricExporter(
            endpoint=settings.signal_endpoint("metrics"), headers=settings.headers()
        )
        provider = MeterProvider(
            resource=resource, metric_readers=[PeriodicExportingMetricReader(exporter)]
        )
        metrics.set_meter_provider(provider)
        self._owned_providers.append(provider)
        return provider

    @staticmethod
    def _guard(
        integration: Integration, explicit: bool, stage: str, step: Callable[[], None]
    ) -> bool:
        """Runs one integration step. A requested integration must work; a detected one may not."""
        try:
            step()
            return True
        except ImportError as error:
            hint = (
                f'pip install "sideseat[{integration.extra}]"'
                if integration.extra
                else (f"pip install {integration.packages[0]}")
            )
            if explicit:
                raise IntegrationError(
                    f"integration {integration.name!r} cannot {stage}: {error}. "
                    f"Install it with: {hint}"
                ) from error
            logger.warning(
                "Skipping detected integration %s: %s (%s)", integration.name, error, hint
            )
            return False

    # -- public API ------------------------------------------------------------------------------

    @property
    def settings(self) -> Settings:
        return self._settings

    @property
    def integrations(self) -> tuple[str, ...]:
        """Names of the integrations that were installed, primary first."""
        return tuple(i.name for i in self._integrations)

    @property
    def tracer_provider(self) -> Any:
        return self._tracer_provider

    def get_tracer(self, name: str, version: str | None = None) -> Tracer:
        return cast(Tracer, self._tracer_provider.get_tracer(name, version))

    @contextmanager
    def session(self, session_id: str, *, user_id: str | None = None) -> Iterator[None]:
        """Attributes every span started inside the block to a session and, optionally, a user."""
        with _context.scope(session_id=session_id, user_id=user_id):
            yield

    @contextmanager
    def trace(
        self,
        name: str,
        *,
        session_id: str | None = None,
        user_id: str | None = None,
        attributes: Mapping[str, Any] | None = None,
        kind: SpanKind = SpanKind.INTERNAL,
    ) -> Iterator[Span]:
        """Starts a new trace: a root span, even when another span is active."""
        detached = otel_trace.set_span_in_context(
            otel_trace.INVALID_SPAN, otel_context.get_current()
        )
        ctx = _context.with_correlation(detached, session_id=session_id, user_id=user_id)
        token = otel_context.attach(ctx)
        try:
            with self._span(name, ctx, attributes, kind) as span:
                yield span
        finally:
            otel_context.detach(token)

    @contextmanager
    def span(
        self,
        name: str,
        *,
        attributes: Mapping[str, Any] | None = None,
        kind: SpanKind = SpanKind.INTERNAL,
    ) -> Iterator[Span]:
        """Starts a child of the active span, or a root span when none is active."""
        with self._span(name, otel_context.get_current(), attributes, kind) as span:
            yield span

    def observe(
        self,
        name: str | None = None,
        *,
        attributes: Mapping[str, Any] | None = None,
        kind: SpanKind = SpanKind.INTERNAL,
    ) -> Callable[[F], F]:
        """Decorates a function so each call runs in a span named after it."""

        def decorate(func: F) -> F:
            span_name = name or func.__qualname__
            if inspect.iscoroutinefunction(func):

                @functools.wraps(func)
                async def run_async(*args: Any, **kwargs: Any) -> Any:
                    with self.span(span_name, attributes=attributes, kind=kind):
                        return await func(*args, **kwargs)

                return cast(F, run_async)

            @functools.wraps(func)
            def run(*args: Any, **kwargs: Any) -> Any:
                with self.span(span_name, attributes=attributes, kind=kind):
                    return func(*args, **kwargs)

            return cast(F, run)

        return decorate

    @contextmanager
    def _span(
        self,
        name: str,
        ctx: otel_context.Context,
        attributes: Mapping[str, Any] | None,
        kind: SpanKind,
    ) -> Iterator[Span]:
        with (
            self._tracer.start_as_current_span(
                name, context=ctx, kind=kind, attributes=dict(attributes or {})
            ) as span,
            ExitStack() as stack,
        ):
            correlation = _context.current()
            for integration in self._integrations:
                stack.enter_context(integration.activate(span, correlation))
            yield span

    def flush(self, timeout_millis: int = _DEFAULT_TIMEOUT_MILLIS) -> bool:
        """Exports everything pending. Returns whether all of it was exported."""
        if self._settings.disabled:
            return True
        ok = True
        for integration in self._integrations:
            ok = integration.flush(timeout_millis) and ok
        for provider in (self._tracer_provider, self._logger_provider, self._meter_provider):
            force_flush = getattr(provider, "force_flush", None)
            if force_flush is not None:
                ok = bool(force_flush(timeout_millis)) and ok
        return ok

    def shutdown(self, timeout_millis: int = _DEFAULT_TIMEOUT_MILLIS) -> bool:
        """Flushes and stops the pipeline. Idempotent; runs automatically at exit."""
        with self._shutdown_lock:
            if self._shut_down:
                return True
            self._shut_down = True
        if self._runtime is not None:
            self._runtime.disconnect()
        ok = self.flush(timeout_millis)
        for integration in reversed(self._integrations):
            try:
                integration.shutdown()
            except Exception:
                ok = False
                logger.warning(
                    "Integration %s failed to shut down", integration.name, exc_info=True
                )
        if self._logging_handler is not None:
            logging.getLogger().removeHandler(self._logging_handler)
        for provider in reversed(self._owned_providers):
            try:
                provider.shutdown()
            except Exception:
                ok = False
                logger.warning("A telemetry provider failed to shut down", exc_info=True)
        atexit.unregister(self.shutdown)
        return ok

    def runtime(self) -> RuntimeClient:
        """The runtime channel client: agent presence, introspection, and invocation.

        Created on first use and disconnected at shutdown. Requires ``pip install "sideseat[runtime]"``.
        """
        if self._runtime is None:
            from sideseat.runtime import RuntimeClient

            self._runtime = RuntimeClient(
                endpoint=self._settings.endpoint,
                project_id=self._settings.project,
                api_key=self._settings.api_key,
            )
        return self._runtime

    def __repr__(self) -> str:
        state = "disabled" if self._settings.disabled else self._settings.otlp_base
        return f"SideSeat({state}, integrations={list(self.integrations)})"
