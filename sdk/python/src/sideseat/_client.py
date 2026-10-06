"""The SideSeat client: builds the OpenTelemetry pipeline and owns its lifecycle."""

from __future__ import annotations

import atexit
import functools
import inspect
import logging
import os
import re
import signal
import threading
import time
from collections.abc import Callable, Iterator, Mapping
from contextlib import ExitStack, contextmanager
from importlib import metadata
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
from sideseat.integrations._util import content_switch

if TYPE_CHECKING:
    from sideseat.runtime import RuntimeClient

logger = logging.getLogger("sideseat")

F = TypeVar("F", bound=Callable[..., Any])

_DEFAULT_TIMEOUT_MILLIS = 30_000
# A process being terminated is usually given a few seconds before it is killed.
_SIGNAL_TIMEOUT_MILLIS = 5_000


class SideSeat:
    """A configured telemetry pipeline. Create it with :func:`sideseat.init`."""

    def __init__(self, settings: Settings) -> None:
        self._settings = settings
        self._integrations: list[Integration] = []
        self._owned_providers: list[Any] = []
        self._logging_handler: logging.Handler | None = None
        # What stops the processors and readers SideSeat added to providers it does not own.
        self._attached: list[Callable[[], Any]] = []
        self._shutdown_lock = threading.Lock()
        self._shutdown_started = False
        self._shutdown_done = threading.Event()
        self._shutdown_result = False
        self._sigterm_installed = False
        self._tracer_provider: Any = otel_trace.NoOpTracerProvider()
        self._logger_provider: Any = None
        self._meter_provider: Any = None
        # A reader SideSeat added to the application's meter provider, which flushes only the
        # readers it was built with.
        self._added_reader: Any = None
        self._runtime: RuntimeClient | None = None
        if settings.debug:
            logging.getLogger("sideseat").setLevel(logging.DEBUG)
        if not settings.disabled:
            self._start()
            self._install_sigterm_handler()
        self._tracer: Tracer = self._tracer_provider.get_tracer("sideseat", __version__)
        atexit.register(self.shutdown)

    # -- pipeline ------------------------------------------------------------------------------

    def _start(self) -> None:
        settings = self._settings
        candidates, explicit = resolve(settings.integrations)
        # Anything that does not reach `prepare` is not active, so it must not name the service or
        # appear in the resource. Hooks that import nothing would not notice a missing package.
        for integration in candidates:
            if self._guard(
                integration, explicit, "start", functools.partial(_require_installed, integration)
            ):
                self._integrations.append(integration)
        ctx = self._setup_context(self._integrations)
        content_switch(settings)

        for integration in list(self._integrations):
            if not self._guard(
                integration, explicit, "prepare", functools.partial(integration.prepare, ctx)
            ):
                self._integrations.remove(integration)
        described = self._setup_context(self._integrations)
        ctx.resource = described.resource
        ctx.service_name = described.service_name
        ctx.service_version = described.service_version

        owners = [i for i in self._integrations if i.owns_tracer_provider]
        if len(owners) > 1:
            names = ", ".join(i.name for i in owners)
            raise ConfigurationError(f"only one integration can own the tracer provider: {names}")
        provider = self._owner_tracer_provider(owners[0], explicit, ctx) if owners else None
        if provider is None:
            if owners:
                # The owner was skipped, so it must not describe the service either.
                described = self._setup_context(self._integrations)
                ctx.resource = described.resource
                ctx.service_name = described.service_name
                ctx.service_version = described.service_version
            ctx.logger_provider = ctx.meter_provider = None
            provider = self._default_tracer_provider(ctx)

        processors: list[Any] = [_context.CorrelationProcessor()]
        for integration in self._integrations:
            processors.extend(integration.span_processors(ctx))
        processors.extend(settings.span_processors)
        if settings.export and not ctx.notes.get("exporter_installed"):
            from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
            from opentelemetry.sdk.trace.export import BatchSpanProcessor

            exporter = OTLPSpanExporter(
                endpoint=settings.signal_endpoint("traces"), headers=settings.headers()
            )
            processors.append(BatchSpanProcessor(exporter))
        for processor in processors:
            provider.add_span_processor(processor)
        if provider not in self._owned_providers:
            self._attached.extend(processor.shutdown for processor in processors)
        self._tracer_provider = provider
        ctx.tracer_provider = provider

        if settings.logs:
            self._logger_provider = ctx.logger_provider or self._start_logs(ctx)
            if settings.capture_python_logs:
                from opentelemetry.sdk._logs import LoggingHandler

                self._logging_handler = LoggingHandler(logger_provider=self._logger_provider)
                logging.getLogger().addHandler(self._logging_handler)
        if settings.metrics:
            self._meter_provider = ctx.meter_provider or self._start_metrics(ctx)
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

    def _setup_context(self, integrations: list[Integration]) -> SetupContext:
        settings = self._settings
        primary = integrations[0].installed_package() if integrations else None
        service_name = settings.service_name or (primary[0] if primary else "sideseat-app")
        service_version = settings.service_version or (primary[1] if primary else __version__)
        resource = _resource.build(
            service_name=service_name,
            service_version=service_version,
            integrations=[i.name for i in integrations],
            extra=settings.resource_attributes,
        )
        return SetupContext(
            settings=settings,
            resource=resource,
            service_name=service_name,
            service_version=service_version,
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
        # Providers the owner built for logs and metrics are stopped with its tracer provider.
        self._owned_providers += [p for p in (ctx.logger_provider, ctx.meter_provider) if p]
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

    def _start_logs(self, ctx: SetupContext) -> Any:
        from opentelemetry import _logs
        from opentelemetry.sdk._logs import LoggerProvider

        provider = _logs.get_logger_provider()
        if not isinstance(provider, LoggerProvider):
            provider = LoggerProvider(resource=ctx.resource)
            _logs.set_logger_provider(provider)
            self._owned_providers.append(provider)
        processor = ctx.log_record_processor()
        if processor is not None:
            provider.add_log_record_processor(processor)
            if provider not in self._owned_providers:
                self._attached.append(processor.shutdown)
        return provider

    def _start_metrics(self, ctx: SetupContext) -> Any:
        from opentelemetry import metrics
        from opentelemetry.sdk.metrics import MeterProvider

        existing = metrics.get_meter_provider()
        if isinstance(existing, MeterProvider):
            # OpenTelemetry 1.44 added readers to built meter providers; before it, a provider
            # takes readers only when it is constructed.
            if not hasattr(existing, "add_metric_reader"):
                logger.warning(
                    "The application already set a meter provider and this opentelemetry-sdk "
                    "cannot add a reader to it, so its metrics will not reach SideSeat; upgrade "
                    "opentelemetry-sdk to 1.44, or give the provider a reader exporting to %s",
                    ctx.settings.signal_endpoint("metrics"),
                )
            elif (reader := ctx.metric_reader()) is not None:
                existing.add_metric_reader(reader)
                self._added_reader = reader
                self._attached.append(functools.partial(_detach_reader, existing, reader))
            return existing
        reader = ctx.metric_reader()
        if reader is None:
            return None
        provider = MeterProvider(resource=ctx.resource, metric_readers=[reader])
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
        _context.require_text("session_id", session_id)
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
        """Exports everything pending. Returns whether all of it was exported within the timeout."""
        if self._settings.disabled:
            return True
        deadline = time.monotonic() + timeout_millis / 1000
        return _within(deadline, "flush", functools.partial(self._flush, deadline))

    def _flush(self, deadline: float) -> bool:
        ok = True
        for integration in self._integrations:
            ok = _attempt(f"flush {integration.name}", integration.flush, _left(deadline)) and ok
        signals = (
            self._tracer_provider,
            self._logger_provider,
            self._meter_provider,
            self._added_reader,
        )
        for provider in signals:
            force_flush = getattr(provider, "force_flush", None)
            if force_flush is not None:
                ok = _attempt("flush a telemetry provider", force_flush, _left(deadline)) and ok
        return ok and time.monotonic() <= deadline

    def shutdown(self, timeout_millis: int = _DEFAULT_TIMEOUT_MILLIS) -> bool:
        """Flushes and stops the pipeline within the timeout, and reports whether it all succeeded.

        Runs automatically when the process exits, including on SIGTERM. Later calls report the
        first call's result.
        """
        with self._shutdown_lock:
            first = not self._shutdown_started
            self._shutdown_started = True
        if not first:
            self._shutdown_done.wait(timeout_millis / 1000)
            return self._shutdown_done.is_set() and self._shutdown_result
        atexit.unregister(self.shutdown)
        self._remove_sigterm_handler()
        if self._logging_handler is not None:
            logging.getLogger().removeHandler(self._logging_handler)
        deadline = time.monotonic() + timeout_millis / 1000
        # OpenTelemetry's provider shutdowns take no timeout and wait for in-flight exports, so the
        # work runs on a daemon thread the process does not wait for.
        stop = functools.partial(self._stop, deadline)
        self._shutdown_result = _within(deadline, "shutdown", stop)
        self._shutdown_done.set()
        return self._shutdown_result

    def _stop(self, deadline: float) -> bool:
        ok = True
        if self._runtime is not None:
            ok = _attempt("disconnect the runtime channel", self._runtime.disconnect) and ok
        if not self._settings.disabled:
            ok = self._flush(deadline) and ok
        for integration in reversed(self._integrations):
            ok = _attempt(f"shut down {integration.name}", integration.shutdown) and ok
        for stop in self._attached:
            ok = _attempt("stop a processor", stop) and ok
        for provider in reversed(self._owned_providers):
            ok = _attempt("shut down a telemetry provider", provider.shutdown) and ok
        return ok

    def _install_sigterm_handler(self) -> None:
        """Shuts down on SIGTERM, which ends a Python process without running ``atexit``.

        Only when nothing else handles SIGTERM, and only from the main thread, where Python allows
        it. After shutting down, the signal is raised again with the default disposition, so the
        process ends the way it would have without SideSeat.
        """
        if threading.current_thread() is not threading.main_thread():
            return
        try:
            if signal.getsignal(signal.SIGTERM) is not signal.SIG_DFL:
                return
            signal.signal(signal.SIGTERM, self._on_sigterm)
        except (ValueError, OSError):
            return
        self._sigterm_installed = True

    def _remove_sigterm_handler(self) -> None:
        if not self._sigterm_installed or threading.current_thread() is not threading.main_thread():
            return
        self._sigterm_installed = False
        if signal.getsignal(signal.SIGTERM) == self._on_sigterm:
            signal.signal(signal.SIGTERM, signal.SIG_DFL)

    def _on_sigterm(self, signum: int, frame: Any) -> None:
        self.shutdown(_SIGNAL_TIMEOUT_MILLIS)
        signal.signal(signal.SIGTERM, signal.SIG_DFL)
        os.kill(os.getpid(), signal.SIGTERM)

    def runtime(self) -> RuntimeClient:
        """The runtime channel client: agent presence, introspection, and invocation.

        Created on first use and disconnected at shutdown. Requires the ``runtime`` extra:
        ``pip install "sideseat[runtime]"``.
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


def _require_installed(integration: Integration) -> None:
    if integration.packages and integration.installed_package() is None:
        raise ImportError(f"none of {', '.join(integration.packages)} is installed")
    missing = [name for name in _extra_requirements(integration.extra) if not _installed(name)]
    if missing:
        raise ImportError(f"{', '.join(missing)} is not installed")


@functools.cache
def _extra_requirements(extra: str | None) -> tuple[str, ...]:
    """Distributions the SideSeat extra ``extra`` installs, read from the package metadata."""
    if extra is None:
        return ()
    try:
        requirements = metadata.requires("sideseat") or []
    except metadata.PackageNotFoundError:
        return ()
    names = []
    for requirement in requirements:
        marker = re.search(r"""extra\s*==\s*["']([^"']+)["']""", requirement)
        name = re.match(r"\s*([A-Za-z0-9][A-Za-z0-9._-]*)", requirement)
        if marker and name and marker.group(1) == extra:
            names.append(name.group(1))
    return tuple(names)


def _installed(distribution: str) -> bool:
    try:
        metadata.version(distribution)
    except metadata.PackageNotFoundError:
        return False
    return True


def _detach_reader(provider: Any, reader: Any) -> None:
    """Remove a reader SideSeat added to the application's meter provider, and stop it.

    The provider unregisters the reader before stopping it, so the reader's last collection finds
    no provider and OpenTelemetry warns; the flush just before exports that collection instead.
    """
    try:
        reader.force_flush()
    finally:
        export_logger = logging.getLogger("opentelemetry.sdk.metrics._internal.export")
        export_logger.addFilter(_not_unregistered_collect)
        try:
            provider.remove_metric_reader(reader)
        finally:
            export_logger.removeFilter(_not_unregistered_collect)


def _not_unregistered_collect(record: logging.LogRecord) -> bool:
    return "until it is registered" not in record.getMessage()


def _left(deadline: float) -> int:
    """Milliseconds until ``deadline``."""
    return max(int((deadline - time.monotonic()) * 1000), 0)


def _attempt(what: str, step: Callable[..., Any], *args: Any) -> bool:
    """Runs one flush or shutdown step. Telemetry never raises into the application after init."""
    try:
        # Some providers (Logfire's proxies) return None from a flush that completed.
        return step(*args) is not False
    except Exception:
        logger.warning("SideSeat could not %s", what, exc_info=True)
        return False


def _within(deadline: float, operation: str, work: Callable[[], bool]) -> bool:
    """``work()``, or ``False`` if it has not finished by ``deadline``."""
    result: list[bool] = []

    def run() -> None:
        result.append(_attempt(operation, work))

    worker = threading.Thread(target=run, name=f"sideseat-{operation}", daemon=True)
    worker.start()
    worker.join(max(deadline - time.monotonic(), 0))
    if not result:
        logger.warning("SideSeat %s did not finish within its timeout", operation)
        return False
    return result[0]
