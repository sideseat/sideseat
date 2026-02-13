"""Framework instrumentation with guards and graceful fallbacks."""

import logging
import threading
from typing import TYPE_CHECKING

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
        elif framework == Frameworks.LangChain:
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
