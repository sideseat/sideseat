"""Settings resolution: explicit arguments, then environment variables, then defaults."""

from __future__ import annotations

import os
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from types import MappingProxyType
from typing import Any
from urllib.parse import urlsplit

from sideseat.errors import ConfigurationError

DEFAULT_ENDPOINT = "http://127.0.0.1:5388"
DEFAULT_PROJECT = "default"

_TRUE = frozenset({"1", "true", "yes"})
_FALSE = frozenset({"0", "false", "no"})


@dataclass(frozen=True, slots=True)
class Settings:
    """Resolved, immutable SDK settings. Built once by :func:`sideseat.init`."""

    endpoint: str
    project: str
    api_key: str | None
    service_name: str | None
    service_version: str | None
    integrations: tuple[Any, ...] | None
    capture_content: bool
    disabled: bool
    debug: bool
    export: bool
    metrics: bool
    logs: bool
    capture_python_logs: bool
    resource_attributes: Mapping[str, Any] = field(default_factory=lambda: MappingProxyType({}))
    span_processors: tuple[Any, ...] = ()

    @property
    def otlp_base(self) -> str:
        """The OTLP base URL; each signal is exported to ``{otlp_base}/v1/{signal}``."""
        path = urlsplit(self.endpoint).path
        if path and path != "/":
            return self.endpoint
        return f"{self.endpoint}/otel/{self.project}"

    def signal_endpoint(self, signal: str) -> str:
        return f"{self.otlp_base}/v1/{signal}"

    def headers(self) -> dict[str, str]:
        """OTLP request headers: ``OTEL_EXPORTER_OTLP_HEADERS`` plus the API key, which wins."""
        headers = _parse_headers(os.getenv("OTEL_EXPORTER_OTLP_HEADERS", ""))
        if self.api_key:
            headers["Authorization"] = f"Bearer {self.api_key}"
        return headers

    def identity(self) -> tuple[Any, ...]:
        """What a second ``init`` call must match to be treated as the same configuration."""
        return (
            self.endpoint,
            self.project,
            self.api_key,
            self.service_name,
            self.service_version,
            tuple(_integration_key(i) for i in self.integrations or ()),
            self.integrations is None,
            self.capture_content,
            self.disabled,
            self.export,
            self.metrics,
            self.logs,
            self.capture_python_logs,
            tuple(sorted(self.resource_attributes.items())),
        )


def resolve(
    *,
    endpoint: str | None,
    project: str | None,
    api_key: str | None,
    service_name: str | None,
    service_version: str | None,
    integrations: Sequence[Any] | str | None,
    capture_content: bool | None,
    disabled: bool | None,
    debug: bool | None,
    export: bool,
    metrics: bool,
    logs: bool,
    capture_python_logs: bool,
    resource_attributes: Mapping[str, Any] | None,
    span_processors: Sequence[Any] | None,
) -> Settings:
    resolved_integrations: tuple[Any, ...] | None
    if integrations is None:
        env = os.getenv("SIDESEAT_INTEGRATIONS", "").strip()
        resolved_integrations = _split_names(env) if env else None
    elif isinstance(integrations, str):
        resolved_integrations = _split_names(integrations)
    else:
        resolved_integrations = tuple(integrations)

    return Settings(
        endpoint=_endpoint(
            endpoint or os.getenv("SIDESEAT_ENDPOINT") or os.getenv("OTEL_EXPORTER_OTLP_ENDPOINT")
        ),
        project=(project or os.getenv("SIDESEAT_PROJECT_ID") or DEFAULT_PROJECT).strip(),
        api_key=api_key or os.getenv("SIDESEAT_API_KEY") or None,
        service_name=service_name or os.getenv("OTEL_SERVICE_NAME") or None,
        service_version=service_version or os.getenv("OTEL_SERVICE_VERSION") or None,
        integrations=resolved_integrations,
        capture_content=_flag(capture_content, "SIDESEAT_CAPTURE_CONTENT", True),
        disabled=_flag(disabled, "SIDESEAT_DISABLED", False),
        debug=_flag(debug, "SIDESEAT_DEBUG", False),
        export=export,
        metrics=metrics,
        logs=logs,
        capture_python_logs=capture_python_logs,
        resource_attributes=MappingProxyType(dict(resource_attributes or {})),
        span_processors=tuple(span_processors or ()),
    )


def _endpoint(raw: str | None) -> str:
    value = (raw or DEFAULT_ENDPOINT).strip().rstrip("/")
    scheme = urlsplit(value).scheme
    if scheme not in ("http", "https") or not urlsplit(value).netloc:
        raise ConfigurationError(f"endpoint must be an http(s) URL, got {raw!r}")
    return value


def _flag(explicit: bool | None, env: str, default: bool) -> bool:
    if explicit is not None:
        return explicit
    raw = os.getenv(env)
    if raw is None or raw.strip() == "":
        return default
    value = raw.strip().lower()
    if value in _TRUE:
        return True
    if value in _FALSE:
        return False
    raise ConfigurationError(f"{env} must be one of 1/0, true/false, yes/no; got {raw!r}")


def _split_names(raw: str) -> tuple[str, ...]:
    return tuple(name.strip() for name in raw.split(",") if name.strip())


def _parse_headers(raw: str) -> dict[str, str]:
    from urllib.parse import unquote

    headers: dict[str, str] = {}
    for pair in raw.split(","):
        key, sep, value = pair.partition("=")
        if sep and key.strip():
            headers[unquote(key.strip())] = unquote(value.strip())
    return headers


def _integration_key(integration: Any) -> str:
    return integration if isinstance(integration, str) else type(integration).__qualname__
