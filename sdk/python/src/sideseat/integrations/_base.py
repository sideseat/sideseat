"""The integration interface and the context integrations are installed with."""

from __future__ import annotations

from collections.abc import Iterator, Sequence
from contextlib import contextmanager
from dataclasses import dataclass, field
from importlib.metadata import PackageNotFoundError, version
from typing import TYPE_CHECKING, Any, ClassVar

if TYPE_CHECKING:
    from opentelemetry.sdk.resources import Resource
    from opentelemetry.sdk.trace import SpanProcessor, TracerProvider
    from opentelemetry.trace import Span

    from sideseat._config import Settings
    from sideseat._context import Correlation


@dataclass
class SetupContext:
    """What an integration can see and use while it is installed.

    The providers are ``None`` during :meth:`Integration.prepare` and set before
    :meth:`Integration.instrument` runs.
    """

    settings: Settings
    resource: Resource
    service_name: str
    service_version: str
    tracer_provider: TracerProvider | None = None
    logger_provider: Any | None = None
    meter_provider: Any | None = None
    notes: dict[str, Any] = field(default_factory=dict)

    @property
    def capture_content(self) -> bool:
        return self.settings.capture_content


class Integration:
    """Turns on the telemetry of one framework or provider.

    Subclasses set :attr:`name` and :attr:`packages` and override the hooks they need. Hooks run in
    this order: :meth:`prepare` for every integration, provider creation, :meth:`span_processors`,
    then :meth:`instrument`. :meth:`shutdown` runs at SDK shutdown in reverse order.
    """

    #: Stable identifier used in ``integrations=[...]``, ``SIDESEAT_INTEGRATIONS``, and the
    #: ``sideseat.framework`` resource attribute.
    name: ClassVar[str]
    #: Distribution names that identify the integration, most specific first.
    packages: ClassVar[tuple[str, ...]]
    #: Whether auto-detection may activate this integration. Provider client libraries are installed
    #: transitively by most frameworks, so their presence proves nothing about the application.
    detectable: ClassVar[bool] = True
    #: Extra that installs what this integration needs: ``pip install "sideseat[<extra>]"``.
    extra: ClassVar[str | None] = None
    #: Whether this integration builds the tracer provider instead of SideSeat.
    owns_tracer_provider: ClassVar[bool] = False

    def prepare(self, ctx: SetupContext) -> None:
        """Runs before any provider exists: environment switches and import-time patches."""

    def create_tracer_provider(self, ctx: SetupContext) -> TracerProvider:
        """Builds the tracer provider. Called only when :attr:`owns_tracer_provider` is true."""
        raise NotImplementedError(f"{self.name} does not own a tracer provider")

    def span_processors(self, ctx: SetupContext) -> Sequence[SpanProcessor]:
        """Processors that must run after correlation and before export."""
        return ()

    def instrument(self, ctx: SetupContext) -> None:
        """Runs once the providers exist: install instrumentors and hooks."""

    @contextmanager
    def activate(self, span: Span, correlation: Correlation) -> Iterator[None]:
        """Makes a span SideSeat started current in any context the framework keeps of its own."""
        yield

    def flush(self, timeout_millis: int) -> bool:
        """Exports anything the integration buffers outside the SideSeat pipeline."""
        return True

    def shutdown(self) -> None:
        """Releases what :meth:`instrument` installed."""

    @classmethod
    def installed_package(cls) -> tuple[str, str] | None:
        """The first installed distribution among :attr:`packages`, with its version."""
        for package in cls.packages:
            try:
                return package, version(package)
            except PackageNotFoundError:
                continue
        return None

    def __repr__(self) -> str:
        return f"{type(self).__name__}()"
