"""Framework instrumentation with guards and graceful fallbacks."""

import functools
import importlib
import logging
import threading
from collections.abc import Iterator
from contextlib import contextmanager
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from opentelemetry.sdk.trace import TracerProvider

from sideseat.config import Frameworks

logger = logging.getLogger("sideseat.instrumentation")

_instrumented: set[str] = set()
_lock = threading.Lock()
_OTEL_EXPORTER_ENV_KEYS = (
    "OTEL_EXPORTER_OTLP_ENDPOINT",
    "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
    "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
    "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
)

LOGFIRE_FRAMEWORKS = frozenset(
    {
        Frameworks.OpenAIAgents,
        Frameworks.PydanticAI,
        Frameworks.OpenAI,
        Frameworks.Anthropic,
        Frameworks.GoogleGenAI,
    }
)


def is_logfire_framework(framework: str) -> bool:
    """Check if framework uses Logfire for instrumentation."""
    return framework in LOGFIRE_FRAMEWORKS


def instrument(
    framework: str,
    provider: "TracerProvider | None",
    service_name: str | None = None,
    service_version: str | None = None,
) -> bool:
    """Instrument framework. Thread-safe, idempotent.

    Returns True if instrumented, False if skipped/failed.
    """
    with _lock:
        if framework in _instrumented:
            logger.debug("Framework %s already instrumented", framework)
            return False
        _instrumented.add(framework)

    try:
        if framework == Frameworks.Strands:
            pass  # Uses global provider
        elif framework in (Frameworks.LangChain, Frameworks.LangGraph):
            _instrument_openinference("langchain", "LangChainInstrumentor", provider)
        elif framework == Frameworks.CrewAI:
            _instrument_openinference("crewai", "CrewAIInstrumentor", provider)
        # These ship OpenInference instrumentors, so they reuse the same helper. Each
        # needs its own extra: the import is what fails without it, and instrument()
        # only warns, leaving the app running with no spans.
        elif framework == Frameworks.Agno:
            _instrument_openinference("agno", "AgnoInstrumentor", provider)
        elif framework == Frameworks.Smolagents:
            _instrument_openinference("smolagents", "SmolagentsInstrumentor", provider)
        elif framework == Frameworks.AG2:
            _instrument_openinference("autogen", "AutogenInstrumentor", provider)
        elif framework == Frameworks.Haystack:
            _instrument_openinference("haystack", "HaystackInstrumentor", provider)
        elif framework in (
            Frameworks.AgentScope,
            Frameworks.Langflow,
            Frameworks.BrowserUse,
        ):
            # These emit OpenTelemetry themselves and only need the global provider.
            pass
        elif framework == Frameworks.AutoGen:
            _instrument_openinference("autogen_agentchat", "AutogenAgentChatInstrumentor", provider)
        elif framework == Frameworks.OpenAIAgents:
            _instrument_logfire("openai_agents", service_name, service_version)
        elif framework == Frameworks.PydanticAI:
            _instrument_logfire("pydantic_ai", service_name, service_version)
        elif framework == Frameworks.OpenAI:
            _instrument_logfire("openai", service_name, service_version)
        elif framework == Frameworks.Anthropic:
            _instrument_logfire("anthropic", service_name, service_version)
        elif framework == Frameworks.GoogleGenAI:
            _instrument_logfire("google_genai", service_name, service_version)
        elif framework == Frameworks.VertexAI:
            _instrument_openllmetry_vertexai(provider)
        elif framework == Frameworks.GoogleADK:
            pass  # Uses global provider
        elif framework == Frameworks.AgentFramework:
            _enable_agent_framework_otel()
        elif framework == Frameworks.ClaudeAgentSDK:
            # Nothing to patch: the SDK spawns the Claude Code CLI, which carries its
            # own OTel instrumentation and is configured via subprocess env vars.
            pass
        else:
            logger.debug("Unknown framework: %s", framework)
            with _lock:
                _instrumented.discard(framework)
            return False

        logger.info("Instrumented: %s", framework)
        return True

    except ImportError as e:
        logger.warning("Instrumentation deps missing for %s: %s", framework, e)
        with _lock:
            _instrumented.discard(framework)
        return False
    except Exception as e:
        logger.warning("Instrumentation failed for %s: %s", framework, e)
        with _lock:
            _instrumented.discard(framework)
        return False


def _enable_agent_framework_otel() -> None:
    """Enable Agent Framework's built-in OTel gate and sensitive data capture."""
    from agent_framework.observability import OBSERVABILITY_SETTINGS

    OBSERVABILITY_SETTINGS.enable_instrumentation = True
    OBSERVABILITY_SETTINGS.enable_sensitive_data = True


def _instrument_openllmetry_vertexai(provider: "TracerProvider | None") -> None:
    """Instrument Vertex AI SDK via opentelemetry-instrumentation-vertexai (openllmetry)."""
    from opentelemetry.instrumentation.vertexai import VertexAIInstrumentor

    VertexAIInstrumentor().instrument(tracer_provider=provider)


def _instrument_openinference(
    module: str, class_name: str, provider: "TracerProvider | None"
) -> None:
    """OpenInference instrumentation with API variance handling."""
    import importlib

    mod = importlib.import_module(f"openinference.instrumentation.{module}")
    instrumentor_cls = getattr(mod, class_name)
    instrumentor = instrumentor_cls()

    # skip_dep_check: OpenInference dependency checks are overly strict — they
    # abort instrumentation on minor version skew even when all patch targets
    # exist.  We manage dependency versions ourselves via the lockfile.
    try:
        instrumentor.instrument(tracer_provider=provider, skip_dep_check=True)
    except TypeError as e:
        if "tracer_provider" in str(e):
            logger.debug("%s doesn't accept tracer_provider, using global", class_name)
            instrumentor.instrument(skip_dep_check=True)
        else:
            raise


def instrument_providers(
    provider: "TracerProvider | None",
    providers: tuple[str, ...] = (),
) -> None:
    """Instrument cloud providers explicitly listed in the providers config.

    Only activates provider instrumentation when the user opts in via
    ``framework=Frameworks.Bedrock``.
    """
    if "bedrock" in providers:
        _try_instrument_aws(provider)


def _try_instrument_aws(provider: "TracerProvider | None") -> None:
    """Instrument botocore for Bedrock telemetry if available."""
    from sideseat._utils import _module_available

    if not _module_available("botocore"):
        return

    with _lock:
        if "aws" in _instrumented:
            return
        _instrumented.add("aws")

    try:
        from sideseat.instrumentors.aws import AWSInstrumentor

        if AWSInstrumentor(tracer_provider=provider).instrument():
            logger.info("Instrumented: aws (botocore)")
        else:
            # Nothing was patched - `wrapt` is missing, and the instrumentor said so.
            # Recording it as done anyway logged success for telemetry that will never
            # be produced, and permanently blocked a retry after the extra is installed.
            with _lock:
                _instrumented.discard("aws")
    except Exception as e:
        logger.debug("AWS instrumentation skipped: %s", e)
        with _lock:
            _instrumented.discard("aws")


def _instrument_logfire(
    method_suffix: str,
    service_name: str | None,
    service_version: str | None,
) -> None:
    """Logfire instrumentation (creates its own provider)."""
    import logfire  # type: ignore[import-not-found]

    # Hide OTLP env vars while Logfire configures — SideSeat is the sole export
    # pipeline owner.
    # Prevents logfire.configure() from creating independent OTLP exporters
    # that bypass SideSeat's processors (including the streaming reparenter).
    # The base endpoint triggers exporters for ALL signals (traces, metrics,
    # logs); signal-specific endpoints trigger their respective exporters. The
    # values are restored immediately so initializing SideSeat never mutates the
    # application's lasting environment.
    with _suspend_otel_exporter_env():
        logfire.configure(
            service_name=service_name or f"{method_suffix.replace('_', '-')}-app",
            service_version=service_version or "0.0.0",
            send_to_logfire=False,
            console=False,
        )

    _apply_logfire_compatibility_patches(method_suffix)

    # Call the appropriate instrument method
    method = getattr(logfire, f"instrument_{method_suffix}")
    method()

    # Resolve abstract method gaps caused by framework SDK / logfire version skew.
    _patch_logfire_wrappers(method_suffix)


@contextmanager
def _suspend_otel_exporter_env() -> Iterator[None]:
    """Temporarily hide exporter env vars and restore their exact prior state."""
    import os

    saved = {key: os.environ[key] for key in _OTEL_EXPORTER_ENV_KEYS if key in os.environ}
    for key in _OTEL_EXPORTER_ENV_KEYS:
        os.environ.pop(key, None)
    try:
        yield
    finally:
        for key in _OTEL_EXPORTER_ENV_KEYS:
            os.environ.pop(key, None)
        os.environ.update(saved)


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
        import inspect

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
        import inspect

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


def _apply_logfire_compatibility_patches(integration: str) -> None:
    """Apply version-skew patches before Logfire captures integration callables."""
    if integration == "anthropic":
        _patch_logfire_anthropic_omit()
        _patch_logfire_anthropic_streaming()


def _wrap_logfire_instruments() -> None:
    """Wrap logfire.instrument_* to auto-apply abstract-method fixes.

    After any ``logfire.instrument_{x}()`` call, ``_patch_logfire_wrappers("{x}")``
    runs automatically — resolving version-skew gaps (e.g. framework SDK adds a
    new abstract method before logfire catches up) without requiring a manual
    patch call at every callsite.

    Called at module import time so the protection is active for any subsequent
    logfire instrumentation, whether through SideSeat or called directly.
    Idempotent: each logfire function is wrapped at most once.
    No-op if logfire is not installed.
    """
    try:
        import logfire  # type: ignore[import-not-found]
    except ImportError:
        return

    for attr_name in [a for a in dir(logfire) if a.startswith("instrument_")]:
        original = getattr(logfire, attr_name, None)
        if not callable(original) or getattr(original, "_sideseat_wrapped", False):
            continue

        integration = attr_name[len("instrument_") :]

        def _make_wrapper(orig: Any, integ: str) -> Any:
            @functools.wraps(orig)
            def wrapper(*args: Any, **kwargs: Any) -> Any:
                _apply_logfire_compatibility_patches(integ)
                result = orig(*args, **kwargs)
                _patch_logfire_wrappers(integ)
                return result

            wrapper._sideseat_wrapped = True  # type: ignore[attr-defined]
            return wrapper

        setattr(logfire, attr_name, _make_wrapper(original, integration))


# Wrap logfire instruments at module import time so the fix is in effect for
# any logfire.instrument_*() call made after this module is loaded — regardless
# of whether the caller goes through SideSeat or calls logfire directly.
_wrap_logfire_instruments()


def apply_framework_patches(framework: str, encode_binary: bool) -> None:
    """Apply framework-specific monkey patches before provider setup."""
    if encode_binary and framework == Frameworks.Strands:
        patch_strands_encoder()
    if framework == Frameworks.GoogleADK:
        patch_adk_tracing()


def patch_adk_tracing() -> bool:
    """Patch ADK tracing to preserve inline_data as base64 instead of stripping it.

    ADK's _build_llm_request_for_trace strips all parts with inline_data,
    losing multimodal content (images, PDFs) from telemetry. This patch
    base64-encodes the binary data so the actual content is preserved.
    """
    try:
        import base64

        from google.adk.telemetry import tracing as adk_tracing  # type: ignore  # noqa: I001

        def _patched(llm_request: Any) -> dict[str, Any]:
            result = {
                "model": llm_request.model,
                "config": llm_request.config.model_dump(
                    exclude_none=True, exclude="response_schema"
                ),
                "contents": [],
            }
            for content in llm_request.contents:
                dumped_parts = []
                for part in content.parts:
                    if part.inline_data:
                        data = part.inline_data.data
                        dumped_parts.append(
                            {
                                "inline_data": {
                                    "mime_type": part.inline_data.mime_type,
                                    "data": base64.b64encode(data).decode("ascii") if data else "",
                                }
                            }
                        )
                    else:
                        dumped = part.model_dump(exclude_none=True)
                        if dumped:
                            dumped_parts.append(dumped)
                result["contents"].append(
                    {
                        "role": content.role,
                        "parts": dumped_parts,
                    }
                )
            return result

        adk_tracing._build_llm_request_for_trace = _patched
        logger.debug("Patched ADK tracing")
        return True
    except ImportError:
        logger.debug("Google ADK not installed")
        return False


def patch_strands_encoder() -> bool:
    """Patch Strands JSONEncoder for base64 binary encoding."""
    try:
        from strands.telemetry import tracer  # type: ignore[import-not-found]

        from sideseat.telemetry.encoding import encode_value

        def _process_value(self: Any, value: Any) -> Any:
            return encode_value(value)

        tracer.JSONEncoder._process_value = _process_value
        logger.debug("Patched Strands encoder")
        return True
    except ImportError:
        logger.debug("Strands not installed")
        return False
