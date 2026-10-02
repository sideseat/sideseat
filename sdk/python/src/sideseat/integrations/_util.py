"""Helpers shared by integrations."""

from __future__ import annotations

import os
from collections.abc import Iterator, Mapping
from contextlib import AbstractContextManager, contextmanager

GENAI_CAPTURE_CONTENT = "OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT"

OTLP_EXPORTER_ENV = (
    "OTEL_EXPORTER_OTLP_ENDPOINT",
    "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
    "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
    "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
)


@contextmanager
def temporary_env(values: Mapping[str, str | None]) -> Iterator[None]:
    """Set (or, for ``None``, remove) variables for the duration of the block, then restore them."""
    saved = {key: os.environ.get(key) for key in values}
    try:
        for key, value in values.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        yield
    finally:
        for key, previous in saved.items():
            if previous is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = previous


def without_otlp_exporter_env() -> AbstractContextManager[None]:
    """Hide OTLP exporter variables while a framework configures itself.

    Frameworks that build their own provider create exporters from these variables, which would send
    every span a second time, outside SideSeat's processors.
    """
    return temporary_env(dict.fromkeys(OTLP_EXPORTER_ENV))


def default_env(key: str, value: str) -> None:
    """Set ``key`` unless the application already chose a value.

    Used only for switches instrumentations read lazily, after ``init`` returns, which is why it
    cannot be scoped like :func:`temporary_env`.
    """
    if not os.environ.get(key):
        os.environ[key] = value
