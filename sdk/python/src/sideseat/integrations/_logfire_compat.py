"""Version-skew repairs between Logfire and the libraries it instruments.

Each repair detects the condition it fixes and does nothing otherwise, so a Logfire release that
fixes the skew upstream makes the repair inert rather than harmful. Remove one when the conformance
fixtures pass without it on the supported versions.
"""

from __future__ import annotations

import functools
import importlib
import inspect
import logging
from typing import Any

logger = logging.getLogger("sideseat")


def apply_before(integration: str) -> None:
    """Repairs that must be in place before Logfire captures the integration's callables."""
    if integration == "anthropic":
        _patch_logfire_anthropic_omit()
        _patch_logfire_anthropic_streaming()


def apply_after(integration: str) -> None:
    """Repairs to the wrapper classes ``logfire.instrument_<integration>`` installed."""
    _patch_logfire_wrappers(integration)


def _make_property(name: str) -> property:
    """Create a property that delegates to self.wrapped."""
    return property(lambda self: getattr(self.wrapped, name, None))


def _make_delegate(name: str) -> Any:
    """Create a method that delegates to self.wrapped."""

    def delegate(self: Any, *args: Any, **kwargs: Any) -> Any:
        return getattr(self.wrapped, name)(*args, **kwargs)

    return delegate


def _patch_logfire_wrappers(integration_module: str) -> None:
    """Resolve unimplemented abstract methods in logfire wrapper classes.

    When a framework SDK evolves faster than logfire, wrapper classes can
    become un-instantiable due to unresolved abstract methods.

    This scans the logfire integration module for concrete classes that still
    have unresolved abstract methods, then adds delegation to ``self.wrapped``
    — the same pattern logfire uses for every other method on these wrappers.

    Safe to call for any logfire integration — modules without abstract wrapper
    classes are scanned in microseconds with no effect.
    """
    try:
        mod = importlib.import_module(f"logfire._internal.integrations.{integration_module}")
    except ImportError:
        return

    for cls_name, cls in inspect.getmembers(mod, inspect.isclass):
        # Only patch classes defined in this module, not imported bases
        if cls.__module__ != mod.__name__:
            continue

        abstracts: frozenset[str] = getattr(cls, "__abstractmethods__", frozenset())
        if not abstracts:
            continue

        patched: set[str] = set()
        for method_name in abstracts:
            # Determine whether the abstract declaration is a property or method
            is_prop = any(
                isinstance(base.__dict__[method_name], property)
                for base in cls.__mro__
                if method_name in base.__dict__
            )

            if is_prop:
                setattr(cls, method_name, _make_property(method_name))
            else:
                setattr(cls, method_name, _make_delegate(method_name))

            patched.add(method_name)

        if patched:
            # Runtime metaprogramming mypy can't model: cls is inferred as type[object].
            cls.__abstractmethods__ = abstracts - patched  # type: ignore[attr-defined]
            logger.debug("Patched %s: %s", cls_name, ", ".join(sorted(patched)))


_OMITTED = object()


def _strip_anthropic_omit(value: Any, omit_type: type[Any]) -> Any:
    """Copy an Anthropic request value without ``Omit`` sentinels.

    Anthropic 1.8 keeps ``Omit`` values in ``FinalRequestOptions.json_data`` until
    the HTTP request is prepared. Logfire 6.0.0b7 observes the earlier structure,
    attempts to JSON-encode fields such as ``stop_sequences``, and abandons the
    whole span when it encounters the sentinel. The telemetry copy must mirror
    what Anthropic will actually send without mutating the live request options.
    """
    if isinstance(value, omit_type):
        return _OMITTED
    if isinstance(value, dict):
        dict_result: dict[Any, Any] = {}
        changed = False
        for key, item in value.items():
            stripped = _strip_anthropic_omit(item, omit_type)
            if stripped is _OMITTED:
                changed = True
                continue
            dict_result[key] = stripped
            changed = changed or stripped is not item
        return dict_result if changed else value
    if isinstance(value, list):
        list_result: list[Any] = []
        changed = False
        for item in value:
            stripped = _strip_anthropic_omit(item, omit_type)
            if stripped is _OMITTED:
                changed = True
                continue
            list_result.append(stripped)
            changed = changed or stripped is not item
        return list_result if changed else value
    if isinstance(value, tuple):
        tuple_result: list[Any] = []
        changed = False
        for item in value:
            stripped = _strip_anthropic_omit(item, omit_type)
            if stripped is _OMITTED:
                changed = True
                continue
            tuple_result.append(stripped)
            changed = changed or stripped is not item
        return tuple(tuple_result) if changed else value
    return value


def _patch_logfire_anthropic_omit() -> bool:
    """Make Logfire's Anthropic request reader compatible with Anthropic 1.8."""
    try:
        anthropic_types = importlib.import_module("anthropic._types")
        integration = importlib.import_module(
            "logfire._internal.integrations.llm_providers.anthropic"
        )
    except ImportError:
        return False

    omit_type = getattr(anthropic_types, "Omit", None)
    original = getattr(integration, "get_endpoint_config", None)
    if not isinstance(omit_type, type) or not callable(original):
        return False
    if getattr(original, "_sideseat_anthropic_omit_safe", False):
        return False

    @functools.wraps(original)
    def get_endpoint_config(options: Any, *args: Any, **kwargs: Any) -> Any:
        json_data = getattr(options, "json_data", None)
        if isinstance(json_data, dict):
            sanitized = _strip_anthropic_omit(json_data, omit_type)
            if sanitized is not json_data:
                model_copy = getattr(options, "model_copy", None)
                if callable(model_copy):
                    options = model_copy(update={"json_data": sanitized})
                else:  # pragma: no cover - retained for older Pydantic-based clients
                    import copy

                    options = copy.copy(options)
                    options.json_data = sanitized
        return original(options, *args, **kwargs)

    get_endpoint_config._sideseat_anthropic_omit_safe = True  # type: ignore[attr-defined]
    integration.get_endpoint_config = get_endpoint_config  # type: ignore[attr-defined]
    logger.debug("Patched Logfire Anthropic Omit handling")
    return True


def _patch_logfire_anthropic_streaming() -> bool:
    """Adapt Logfire's stream accumulator to Anthropic 1.8's required state."""
    try:
        integration = importlib.import_module(
            "logfire._internal.integrations.llm_providers.anthropic"
        )
        messages = importlib.import_module("anthropic.lib.streaming._messages")
        beta_messages = importlib.import_module("anthropic.lib.streaming._beta_messages")
    except ImportError:
        return False

    state_cls = getattr(integration, "AnthropicMessageStreamState", None)
    accumulate = getattr(messages, "accumulate_event", None)
    beta_accumulate = getattr(beta_messages, "accumulate_event", None)
    if not isinstance(state_cls, type) or not callable(accumulate) or not callable(beta_accumulate):
        return False

    original = getattr(state_cls, "record_chunk", None)
    if not callable(original) or getattr(original, "_sideseat_anthropic_stream_safe", False):
        return False

    # Anthropic 1.8 added a persistent JSON buffer to both accumulators. A future
    # Logfire that already carries it must keep its own implementation.
    parameters = inspect.signature(accumulate).parameters
    if "json_bufs" not in parameters:
        return False
    code = getattr(original, "__code__", None)
    code_names = (*getattr(code, "co_names", ()), *getattr(code, "co_varnames", ()))
    if any("json_buf" in name for name in code_names):
        return False

    accumulate_parameters = inspect.signature(accumulate).parameters
    beta_parameters = inspect.signature(beta_accumulate).parameters

    @functools.wraps(original)
    def record_chunk(self: Any, chunk: Any) -> None:
        json_bufs = self.__dict__.setdefault("_sideseat_anthropic_json_bufs", {})
        is_beta = type(chunk).__module__.startswith("anthropic.types.beta")
        accumulator = beta_accumulate if is_beta else accumulate
        supported = beta_parameters if is_beta else accumulate_parameters
        kwargs: dict[str, Any] = {
            "event": chunk,
            "current_snapshot": self._message,
        }
        if "json_bufs" in supported:
            kwargs["json_bufs"] = json_bufs
        if "request_headers" in supported:
            # Logfire has no access to the high-level stream's request here. Its
            # previous implementation also supplied an empty mapping.
            kwargs["request_headers"] = {}
        self._message = accumulator(**kwargs)

        if getattr(getattr(chunk, "delta", None), "type", None) == "text_delta":
            self._chunk_count += 1

    record_chunk._sideseat_anthropic_stream_safe = True  # type: ignore[attr-defined]
    state_cls.record_chunk = record_chunk  # type: ignore[attr-defined]
    logger.debug("Patched Logfire Anthropic 1.8 stream accumulation")
    return True
