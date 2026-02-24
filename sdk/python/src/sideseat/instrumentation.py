"""Framework instrumentation with guards and graceful fallbacks."""

import logging
import threading
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from opentelemetry.sdk.trace import TracerProvider

from sideseat.config import Frameworks

logger = logging.getLogger("sideseat.instrumentation")

_instrumented: set[str] = set()
_lock = threading.Lock()

LOGFIRE_FRAMEWORKS = frozenset({Frameworks.OpenAIAgents, Frameworks.PydanticAI})


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
        elif framework == Frameworks.AutoGen:
            _instrument_openinference("autogen_agentchat", "AutogenAgentChatInstrumentor", provider)
        elif framework == Frameworks.OpenAIAgents:
            _instrument_logfire("openai_agents", service_name, service_version)
        elif framework == Frameworks.PydanticAI:
            _instrument_logfire("pydantic_ai", service_name, service_version)
        elif framework == Frameworks.GoogleADK:
            pass  # Uses global provider
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


def _instrument_openinference(
    module: str, class_name: str, provider: "TracerProvider | None"
) -> None:
    """OpenInference instrumentation with API variance handling."""
    import importlib

    mod = importlib.import_module(f"openinference.instrumentation.{module}")
    instrumentor_cls = getattr(mod, class_name)
    instrumentor = instrumentor_cls()

    # Some instrumentors don't accept tracer_provider
    try:
        instrumentor.instrument(tracer_provider=provider)
    except TypeError as e:
        if "tracer_provider" in str(e):
            logger.debug("%s doesn't accept tracer_provider, using global", class_name)
            instrumentor.instrument()
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
    if "openai" in providers:
        _try_instrument_provider("openai", "openai", "openai", "OpenAIInstrumentor", provider)
    if "anthropic" in providers:
        _try_instrument_provider(
            "anthropic", "anthropic", "anthropic", "AnthropicInstrumentor", provider
        )
    if "vertex_ai" in providers:
        _try_instrument_provider(
            "vertex_ai", "vertexai", "vertexai", "VertexAIInstrumentor", provider
        )


def _try_instrument_provider(
    name: str,
    sdk_package: str,
    oi_module: str,
    oi_class: str,
    provider: "TracerProvider | None",
) -> None:
    """Instrument an OpenInference-based provider if its SDK is installed."""
    from sideseat._utils import _module_available

    if not _module_available(sdk_package):
        return

    with _lock:
        if name in _instrumented:
            return
        _instrumented.add(name)

    try:
        _instrument_openinference(oi_module, oi_class, provider)
        logger.info("Instrumented: %s", name)
    except Exception as e:
        logger.debug("%s instrumentation skipped: %s", name, e)
        with _lock:
            _instrumented.discard(name)


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

        AWSInstrumentor(tracer_provider=provider).instrument()
        logger.info("Instrumented: aws (botocore)")
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

    logfire.configure(
        service_name=service_name or f"{method_suffix.replace('_', '-')}-app",
        service_version=service_version or "0.0.0",
        send_to_logfire=False,
        console=False,
    )

    # Call the appropriate instrument method
    method = getattr(logfire, f"instrument_{method_suffix}")
    method()


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
