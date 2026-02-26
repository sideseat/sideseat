"""SideSeat configuration with env var support and validation."""

import os
from dataclasses import dataclass
from importlib.metadata import PackageNotFoundError, version

from sideseat._version import __version__


class Frameworks:
    """Framework and provider identifiers for instrumentation.

    Usage:
        SideSeat(framework=Frameworks.Strands)
        SideSeat(framework=[Frameworks.Strands, Frameworks.Bedrock])
        SideSeat(framework=Frameworks.OpenAI)
    """

    # Frameworks
    Strands = "strands"
    LangChain = "langchain"
    LangGraph = "langgraph"
    CrewAI = "crewai"
    AutoGen = "autogen"
    OpenAIAgents = "openai-agents"
    GoogleADK = "google-adk"
    PydanticAI = "pydantic-ai"
    OpenAI = "openai"
    Anthropic = "anthropic"
    GoogleGenAI = "google_genai"
    # Providers
    Bedrock = "bedrock"


FRAMEWORK_PACKAGES = [
    (Frameworks.Strands, "strands-agents"),
    (Frameworks.LangGraph, "langgraph"),
    (Frameworks.LangChain, "langchain-core"),
    (Frameworks.CrewAI, "crewai"),
    (Frameworks.AutoGen, "autogen-agentchat"),
    (Frameworks.OpenAIAgents, "agents"),
    (Frameworks.GoogleADK, "google-adk"),
    (Frameworks.PydanticAI, "pydantic-ai"),
    (Frameworks.OpenAI, "openai"),
    (Frameworks.Anthropic, "anthropic"),
    (Frameworks.GoogleGenAI, "google-genai"),
]

_FRAMEWORK_KEYS = {key for key, _ in FRAMEWORK_PACKAGES}

# Packages too common as transitive deps for reliable auto-detection
_NO_AUTO_DETECT = {Frameworks.OpenAI, Frameworks.Anthropic, Frameworks.GoogleGenAI}


def _resolve_framework_input(
    framework: str | list[str] | None,
) -> tuple[str | None, tuple[str, ...]]:
    """Partition framework input into (framework, providers).

    A single framework string or list is split: items matching FRAMEWORK_PACKAGES
    keys become the framework; everything else becomes providers.
    At most one framework is allowed.
    """
    if framework is None:
        return None, ()
    if isinstance(framework, str):
        return (framework, ()) if framework in _FRAMEWORK_KEYS else (None, (framework,))
    fw_items = [x for x in framework if x in _FRAMEWORK_KEYS]
    prov_items = [x for x in framework if x not in _FRAMEWORK_KEYS]
    if len(fw_items) > 1:
        raise ValueError(f"At most one framework allowed, got {fw_items}")
    return (fw_items[0] if fw_items else None), tuple(prov_items)


@dataclass(frozen=True, slots=True)
class Config:
    """Immutable configuration for SideSeat SDK."""

    disabled: bool
    endpoint: str
    api_key: str | None
    project_id: str
    framework: str
    service_name: str
    service_version: str
    auto_instrument: bool
    enable_traces: bool
    enable_metrics: bool
    enable_logs: bool
    encode_binary: bool
    capture_content: bool
    debug: bool
    providers: tuple[str, ...]

    def __repr__(self) -> str:
        return (
            f"Config(disabled={self.disabled}, endpoint={self.endpoint!r}, "
            f"project_id={self.project_id!r}, framework={self.framework!r})"
        )

    @classmethod
    def create(
        cls,
        *,
        disabled: bool | None = None,
        endpoint: str | None = None,
        api_key: str | None = None,
        project_id: str | None = None,
        framework: str | list[str] | None = None,
        service_name: str | None = None,
        service_version: str | None = None,
        auto_instrument: bool = True,
        enable_traces: bool = True,
        enable_metrics: bool = True,
        enable_logs: bool | None = None,
        encode_binary: bool = True,
        capture_content: bool = True,
        debug: bool | None = None,
    ) -> "Config":
        """Create config with env var fallback chain."""
        # Check disabled first
        resolved_disabled = (
            disabled if disabled is not None else _parse_bool_env("SIDESEAT_DISABLED", False)
        )

        # Logs disabled by default (can be noisy), enable via env var or constructor
        resolved_enable_logs = (
            enable_logs
            if enable_logs is not None
            else _parse_bool_env("OTEL_PYTHON_LOGGING_AUTO_INSTRUMENTATION_ENABLED", False)
        )

        # Only set OTEL env vars if not disabled
        if not resolved_disabled:
            genai_content_env = "OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT"
            if capture_content and not os.getenv(genai_content_env):
                os.environ[genai_content_env] = "true"

        # Resolve endpoint - preserve path if present
        resolved_endpoint = _normalize_endpoint(
            endpoint
            or os.getenv("SIDESEAT_ENDPOINT")
            or os.getenv("OTEL_EXPORTER_OTLP_ENDPOINT")
            or "http://127.0.0.1:5388"
        )

        resolved_api_key = api_key or os.getenv("SIDESEAT_API_KEY")
        resolved_project_id = project_id or os.getenv("SIDESEAT_PROJECT") or "default"
        resolved_debug = debug if debug is not None else _parse_bool_env("SIDESEAT_DEBUG", False)

        # Partition framework input into framework + providers
        fw, providers = _resolve_framework_input(framework)

        # Framework detection
        if fw:
            # Explicit framework: use its package name and version
            fw_pkg_map = {key: pkg for key, pkg in FRAMEWORK_PACKAGES}
            resolved_framework = fw
            explicit_pkg = fw_pkg_map.get(fw, fw)
            try:
                explicit_ver = version(explicit_pkg)
            except PackageNotFoundError:
                explicit_ver = __version__
            resolved_service_name = service_name or explicit_pkg
            resolved_service_version = service_version or explicit_ver
        else:
            detected_key, detected_pkg, detected_version = _detect_framework()
            resolved_framework = detected_key
            resolved_service_name = service_name or detected_pkg
            resolved_service_version = service_version or detected_version

        return cls(
            disabled=resolved_disabled,
            endpoint=resolved_endpoint,
            api_key=resolved_api_key,
            project_id=resolved_project_id,
            framework=resolved_framework,
            service_name=resolved_service_name,
            service_version=resolved_service_version,
            auto_instrument=auto_instrument,
            enable_traces=enable_traces,
            enable_metrics=enable_metrics,
            enable_logs=resolved_enable_logs,
            encode_binary=encode_binary,
            capture_content=capture_content,
            debug=resolved_debug,
            providers=providers,
        )


def _normalize_endpoint(endpoint: str) -> str:
    """Normalize endpoint URL - validate and strip trailing slashes."""
    endpoint = endpoint.strip()
    if not endpoint.startswith(("http://", "https://")):
        raise ValueError(f"Invalid endpoint: {endpoint}. Must start with http:// or https://")
    return endpoint.rstrip("/")


def _parse_bool_env(key: str, default: bool) -> bool:
    """Parse boolean env var."""
    val = os.getenv(key, "").lower()
    if val in ("1", "true", "yes"):
        return True
    if val in ("0", "false", "no"):
        return False
    return default


def _detect_framework() -> tuple[str, str, str]:
    """Detect installed AI framework. Returns (key, package, version)."""
    for key, package in FRAMEWORK_PACKAGES:
        if key in _NO_AUTO_DETECT:
            continue
        try:
            return key, package, version(package)
        except PackageNotFoundError:
            continue
    return "sideseat", "sideseat", __version__
