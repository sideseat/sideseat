"""Tests for framework instrumentation."""

import asyncio
import importlib
import os
import sys
import threading
from types import ModuleType, SimpleNamespace
from typing import Any

import pytest

from sideseat.config import Frameworks
from sideseat.instrumentation import (
    LAMINAR_FRAMEWORKS,
    LOGFIRE_FRAMEWORKS,
    _instrumented,
    _laminar_exporter_env,
    _lock,
    _patch_logfire_anthropic_omit,
    _patch_logfire_anthropic_streaming,
    _strip_anthropic_omit,
    _suspend_otel_exporter_env,
    _TraceLoopEnrichmentProcessor,
    _TraceLoopInlineImageUploader,
    instrument,
    is_laminar_framework,
    is_logfire_framework,
    patch_adk_tracing,
)


@pytest.fixture(autouse=True)
def reset_instrumentation_state() -> None:
    """Reset instrumentation state between tests."""
    with _lock:
        _instrumented.clear()


class TestIsLogfireFramework:
    """Tests for Logfire framework detection."""

    def test_openai_agents_is_logfire(self) -> None:
        """OpenAI Agents uses Logfire (logfire.instrument_openai_agents)."""
        assert is_logfire_framework(Frameworks.OpenAIAgents) is True

    def test_pydantic_ai_is_logfire(self) -> None:
        """PydanticAI should use Logfire."""
        assert is_logfire_framework(Frameworks.PydanticAI) is True

    def test_openai_is_logfire(self) -> None:
        """OpenAI should use Logfire."""
        assert is_logfire_framework(Frameworks.OpenAI) is True

    def test_anthropic_is_logfire(self) -> None:
        """Anthropic should use Logfire."""
        assert is_logfire_framework(Frameworks.Anthropic) is True

    def test_other_frameworks_not_logfire(self) -> None:
        """Other frameworks should not use Logfire."""
        assert is_logfire_framework(Frameworks.Strands) is False
        assert is_logfire_framework(Frameworks.LangChain) is False
        assert is_logfire_framework(Frameworks.CrewAI) is False
        assert is_logfire_framework(Frameworks.AutoGen) is False
        assert is_logfire_framework(Frameworks.GoogleADK) is False
        assert is_logfire_framework(Frameworks.ClaudeAgentSDK) is False
        assert is_logfire_framework(Frameworks.AzureOpenAI) is False

    def test_google_genai_is_logfire(self) -> None:
        """Google GenAI should use Logfire."""
        assert is_logfire_framework(Frameworks.GoogleGenAI) is True

    def test_vertex_ai_is_logfire(self) -> None:
        """The current Vertex AI path is the Google Gen AI SDK in cloud mode."""
        assert is_logfire_framework(Frameworks.VertexAI) is True

    def test_generic_logfire_uses_logfire_provider(self) -> None:
        """Generic Logfire spans need Logfire's provider even without another integration."""
        assert is_logfire_framework(Frameworks.Logfire) is True

    def test_logfire_frameworks_frozenset(self) -> None:
        """LOGFIRE_FRAMEWORKS should be a frozenset."""
        assert isinstance(LOGFIRE_FRAMEWORKS, frozenset)
        assert Frameworks.OpenAIAgents in LOGFIRE_FRAMEWORKS
        assert Frameworks.PydanticAI in LOGFIRE_FRAMEWORKS
        assert Frameworks.OpenAI in LOGFIRE_FRAMEWORKS
        assert Frameworks.Anthropic in LOGFIRE_FRAMEWORKS
        assert Frameworks.GoogleGenAI in LOGFIRE_FRAMEWORKS
        assert Frameworks.VertexAI in LOGFIRE_FRAMEWORKS
        assert Frameworks.Logfire in LOGFIRE_FRAMEWORKS


class TestIsLaminarFramework:
    """Tests for frameworks that need Laminar's provider."""

    def test_browser_use_is_laminar(self) -> None:
        assert is_laminar_framework(Frameworks.BrowserUse) is True
        assert LAMINAR_FRAMEWORKS == frozenset({Frameworks.BrowserUse})

    def test_other_frameworks_are_not_laminar(self) -> None:
        assert is_laminar_framework(Frameworks.LangChain) is False
        assert is_laminar_framework(Frameworks.Langflow) is False


class TestInstrument:
    """Tests for the instrument() function."""

    def test_strands_no_op(self) -> None:
        """Strands instrumentation should be a no-op (uses global provider)."""
        result = instrument(Frameworks.Strands, None)
        assert result is True
        assert Frameworks.Strands in _instrumented

    def test_google_adk_no_op(self) -> None:
        """Google ADK instrumentation should be a no-op (uses global provider)."""
        result = instrument(Frameworks.GoogleADK, None)
        assert result is True
        assert Frameworks.GoogleADK in _instrumented

    def test_claude_agent_sdk_no_op(self) -> None:
        """Claude Agent SDK is a no-op: the Claude Code CLI subprocess self-instruments."""
        result = instrument(Frameworks.ClaudeAgentSDK, None)
        assert result is True
        assert Frameworks.ClaudeAgentSDK in _instrumented

    def test_double_instrumentation_blocked(self) -> None:
        """Second instrumentation attempt should be skipped."""
        # First call
        result1 = instrument(Frameworks.Strands, None)
        assert result1 is True

        # Second call should be blocked
        result2 = instrument(Frameworks.Strands, None)
        assert result2 is False

    def test_unknown_framework(self) -> None:
        """Unknown framework should return False."""
        result = instrument("unknown-framework", None)
        assert result is False
        assert "unknown-framework" not in _instrumented

    def test_generic_logfire_configures_without_provider_instrumentation(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        """Generic Logfire must not guess and patch an unrelated model provider."""
        calls: list[dict[str, Any]] = []
        fake_logfire = SimpleNamespace(configure=lambda **kwargs: calls.append(kwargs))
        monkeypatch.setitem(sys.modules, "logfire", fake_logfire)
        monkeypatch.setenv("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector.example")

        assert instrument(
            Frameworks.Logfire,
            None,
            service_name="generic-logfire",
            service_version="1.2.3",
        )
        assert calls == [
            {
                "service_name": "generic-logfire",
                "service_version": "1.2.3",
                "send_to_logfire": False,
                "console": False,
            }
        ]
        assert os.environ["OTEL_EXPORTER_OTLP_ENDPOINT"] == "http://collector.example"

    def test_generic_logfire_has_a_service_name_without_provider_context(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        """The generic path has no provider suffix from which to derive its default."""
        calls: list[dict[str, Any]] = []
        fake_logfire = SimpleNamespace(configure=lambda **kwargs: calls.append(kwargs))
        monkeypatch.setitem(sys.modules, "logfire", fake_logfire)

        assert instrument(Frameworks.Logfire, None)
        assert calls[0]["service_name"] == "logfire-app"

    def test_missing_deps_graceful(self) -> None:
        """Missing instrumentation deps should not crash."""
        # LangChain instrumentation requires openinference-instrumentation-langchain
        # which is likely not installed in test env
        result = instrument(Frameworks.LangChain, None)
        # Result depends on whether deps are installed
        assert isinstance(result, bool)

    def test_browser_use_initializes_laminar_for_sideseat(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        """Current browser-use spans come from Laminar, OpenAI, and Bubus hooks."""
        calls: list[dict[str, Any]] = []

        class Instruments:
            OPENAI = "openai"
            BUBUS = "bubus"

        class Laminar:
            @staticmethod
            def is_initialized() -> bool:
                return False

            @staticmethod
            def initialize(**kwargs: Any) -> None:
                calls.append(
                    {
                        **kwargs,
                        "endpoint": os.environ.get("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT"),
                        "headers": os.environ.get("OTEL_EXPORTER_OTLP_TRACES_HEADERS"),
                        "protocol": os.environ.get("OTEL_EXPORTER_OTLP_TRACES_PROTOCOL"),
                    }
                )

        monkeypatch.setitem(
            sys.modules,
            "lmnr",
            SimpleNamespace(Instruments=Instruments, Laminar=Laminar),
        )
        monkeypatch.setitem(
            sys.modules,
            "lmnr.sdk.utils",
            SimpleNamespace(from_env=lambda key: None),
        )
        monkeypatch.setenv("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", "http://old")
        monkeypatch.setenv("OTEL_EXPORTER_OTLP_TRACES_HEADERS", "old=value")

        assert instrument(
            Frameworks.BrowserUse,
            None,
            otlp_endpoint="http://collector/otel/default/v1/traces",
            otlp_headers={"Authorization": "Bearer secret", "x-project": "default"},
            export_timeout_seconds=17,
        )
        assert calls == [
            {
                "instruments": {"openai", "bubus"},
                "export_timeout_seconds": 17,
                "force_http": True,
                "set_global_tracer_provider": True,
                "endpoint": "http://collector/otel/default/v1/traces",
                "headers": "Authorization=Bearer%20secret,x-project=default",
                "protocol": "http/protobuf",
            }
        ]
        assert os.environ["OTEL_EXPORTER_OTLP_TRACES_ENDPOINT"] == "http://old"
        assert os.environ["OTEL_EXPORTER_OTLP_TRACES_HEADERS"] == "old=value"

    def test_browser_use_rejects_laminar_destination_override(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        """A Laminar key would silently send the spans somewhere other than SideSeat."""

        class Laminar:
            @staticmethod
            def is_initialized() -> bool:
                return False

        monkeypatch.setitem(
            sys.modules,
            "lmnr",
            SimpleNamespace(Instruments=object(), Laminar=Laminar),
        )
        monkeypatch.setitem(
            sys.modules,
            "lmnr.sdk.utils",
            SimpleNamespace(
                from_env=lambda key: "lmnr-key" if key == "LMNR_PROJECT_API_KEY" else None
            ),
        )

        assert (
            instrument(
                Frameworks.BrowserUse,
                None,
                otlp_endpoint="http://collector/v1/traces",
            )
            is False
        )
        assert Frameworks.BrowserUse not in _instrumented

    def test_laminar_exporter_env_restores_absent_values(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        for key in (
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
            "OTEL_EXPORTER_OTLP_TRACES_HEADERS",
            "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL",
        ):
            monkeypatch.delenv(key, raising=False)

        with _laminar_exporter_env("http://collector/v1/traces", {}):
            assert os.environ["OTEL_EXPORTER_OTLP_TRACES_ENDPOINT"].endswith("/v1/traces")
            assert "OTEL_EXPORTER_OTLP_TRACES_HEADERS" not in os.environ

        assert "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT" not in os.environ
        assert "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL" not in os.environ

    def test_llamaindex_uses_its_openinference_instrumentor(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        """LlamaIndex must bind its instrumentor to SideSeat's tracer provider."""
        calls: list[tuple[str, str, Any]] = []

        def record(module: str, class_name: str, provider: Any) -> None:
            calls.append((module, class_name, provider))

        monkeypatch.setattr("sideseat.instrumentation._instrument_openinference", record)
        provider: Any = object()

        assert instrument(Frameworks.LlamaIndex, provider) is True
        assert calls == [("llama_index", "LlamaIndexInstrumentor", provider)]

    def test_vertex_ai_uses_google_genai_instrumentation(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        """Vertex mode and the Developer API share the current unified client."""
        calls: list[tuple[str, str | None, str | None]] = []

        def record(
            suffix: str,
            service_name: str | None,
            service_version: str | None,
        ) -> None:
            calls.append((suffix, service_name, service_version))

        monkeypatch.setattr("sideseat.instrumentation._instrument_logfire", record)

        assert instrument(
            Frameworks.VertexAI,
            None,
            service_name="vertex-service",
            service_version="2.26.0",
        )
        assert calls == [("google_genai", "vertex-service", "2.26.0")]

    def test_azure_openai_uses_its_openinference_instrumentor(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        """The OpenInference OpenAI integration preserves Azure provider identity."""
        calls: list[tuple[str, str, Any]] = []

        def record(module: str, class_name: str, provider: Any) -> None:
            calls.append((module, class_name, provider))

        monkeypatch.setattr("sideseat.instrumentation._instrument_openinference", record)
        provider: Any = object()

        assert instrument(Frameworks.AzureOpenAI, provider) is True
        assert calls == [("openai", "OpenAIInstrumentor", provider)]

    def test_ag2_injects_builtin_telemetry_once(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """AG2 1.x agents receive native telemetry without duplicate middleware."""

        class TelemetryMiddleware:
            def __init__(self, **kwargs: Any) -> None:
                self.kwargs = kwargs

        class Agent:
            def __init__(
                self,
                name: str,
                prompt: str = "",
                *,
                middleware: tuple[Any, ...] = (),
            ) -> None:
                self.name = name
                self.prompt = prompt
                self.middleware = middleware

        modules = {
            "ag2": SimpleNamespace(Agent=Agent),
            "ag2.middleware.builtin": SimpleNamespace(TelemetryMiddleware=TelemetryMiddleware),
        }
        real_import = importlib.import_module

        def fake_import(name: str, package: str | None = None) -> Any:
            return modules.get(name) or real_import(name, package)

        monkeypatch.setattr(importlib, "import_module", fake_import)
        provider: Any = object()

        assert instrument(Frameworks.AG2, provider) is True

        injected = Agent("weather")
        assert len(injected.middleware) == 1
        assert injected.middleware[0].kwargs == {
            "tracer_provider": provider,
            "capture_content": True,
            "agent_name": "weather",
        }

        explicit = TelemetryMiddleware(tracer_provider=provider)
        preserved = Agent("explicit", middleware=(explicit,))
        assert preserved.middleware == (explicit,)

    def test_agentscope_injects_builtin_tracing_once(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """AgentScope 2.x agents receive tracing without duplicate middleware."""

        class TracingMiddleware:
            pass

        class Agent:
            def __init__(
                self,
                name: str,
                system_prompt: str,
                model: Any,
                toolkit: Any = None,
                middlewares: list[Any] | None = None,
            ) -> None:
                self.name = name
                self.system_prompt = system_prompt
                self.model = model
                self.toolkit = toolkit
                self.middlewares = middlewares

        modules = {
            "agentscope.agent": SimpleNamespace(Agent=Agent),
            "agentscope.middleware": SimpleNamespace(TracingMiddleware=TracingMiddleware),
        }
        real_import = importlib.import_module

        def fake_import(name: str, package: str | None = None) -> Any:
            return modules.get(name) or real_import(name, package)

        monkeypatch.setattr(importlib, "import_module", fake_import)

        assert instrument(Frameworks.AgentScope, None) is True

        injected = Agent("weather", "Use tools.", object())
        assert injected.middlewares is not None
        assert len(injected.middlewares) == 1
        assert isinstance(injected.middlewares[0], TracingMiddleware)

        explicit = TracingMiddleware()
        preserved = Agent(
            "explicit",
            "Use tools.",
            object(),
            middlewares=[explicit],
        )
        assert preserved.middlewares == [explicit]

        positional = Agent(
            "positional",
            "Use tools.",
            object(),
            None,
            [],
        )
        assert positional.middlewares is not None
        assert len(positional.middlewares) == 1
        assert isinstance(positional.middlewares[0], TracingMiddleware)

    def test_haystack_uses_native_tracing_with_content(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        """Haystack 3 must emit its complete native content through SideSeat's provider."""
        enabled: list[Any] = []
        tracing = SimpleNamespace(
            tracer=SimpleNamespace(is_content_tracing_enabled=False),
            enable_tracing=enabled.append,
        )

        class OpenTelemetryTracer:
            def __init__(self, tracer: Any) -> None:
                self.tracer = tracer

        integration = SimpleNamespace(OpenTelemetryTracer=OpenTelemetryTracer)
        real_import = importlib.import_module

        def fake_import(name: str, package: str | None = None) -> Any:
            modules = {
                "haystack.tracing": tracing,
                "haystack_integrations.tracing.opentelemetry": integration,
            }
            return modules.get(name) or real_import(name, package)

        monkeypatch.setattr(importlib, "import_module", fake_import)
        provider = SimpleNamespace(get_tracer=lambda name: f"tracer:{name}")

        assert instrument(Frameworks.Haystack, provider) is True
        assert tracing.tracer.is_content_tracing_enabled is True
        assert len(enabled) == 1
        assert enabled[0].tracer == "tracer:haystack"

    def test_traceloop_reuses_sideseat_export_and_content_policy(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        """TraceLoop enriches the global provider without adding another exporter."""
        calls: list[dict[str, Any]] = []

        class Traceloop:
            @staticmethod
            def init(**kwargs: Any) -> None:
                calls.append(
                    {
                        **kwargs,
                        "content_setting": os.environ.get("TRACELOOP_TRACE_CONTENT"),
                    }
                )

        instruments = SimpleNamespace(REQUESTS="requests", URLLIB3="urllib3")
        monkeypatch.setitem(sys.modules, "traceloop.sdk", SimpleNamespace(Traceloop=Traceloop))
        monkeypatch.setitem(
            sys.modules,
            "traceloop.sdk.instruments",
            SimpleNamespace(Instruments=instruments),
        )
        monkeypatch.setenv("TRACELOOP_TRACE_CONTENT", "original")

        assert instrument(
            Frameworks.TraceLoop,
            object(),
            service_name="trace-app",
            service_version="1.2.3",
            capture_content=False,
        )

        assert len(calls) == 1
        call = calls[0]
        assert call["app_name"] == "trace-app"
        assert call["resource_attributes"] == {
            "service.version": "1.2.3",
            "sideseat.framework": "traceloop",
        }
        assert call["block_instruments"] == {"requests", "urllib3"}
        assert isinstance(call["processor"], _TraceLoopEnrichmentProcessor)
        assert isinstance(call["image_uploader"], _TraceLoopInlineImageUploader)
        assert call["use_attributes"] is True
        assert call["content_setting"] == "false"
        assert os.environ["TRACELOOP_TRACE_CONTENT"] == "original"

    def test_traceloop_image_uploader_preserves_inline_content(self) -> None:
        """SideSeat must not upload application images to TraceLoop's cloud."""
        uploader = _TraceLoopInlineImageUploader()
        result = asyncio.run(
            uploader.aupload_base64_image(
                "trace",
                "span",
                "message_0_content_1.jpeg",
                "aW1hZ2U=",
            )
        )
        assert result == "data:image/jpeg;base64,aW1hZ2U="

    def test_semantic_kernel_enables_every_diagnostics_snapshot(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        """Semantic Kernel must emit content even when imported before SideSeat."""
        snapshots = [
            SimpleNamespace(
                enable_otel_diagnostics=False,
                enable_otel_diagnostics_sensitive=False,
            )
            for _ in range(3)
        ]
        modules = {
            name: SimpleNamespace(MODEL_DIAGNOSTICS_SETTINGS=settings)
            for name, settings in zip(
                (
                    "semantic_kernel.utils.telemetry.agent_diagnostics.decorators",
                    "semantic_kernel.utils.telemetry.model_diagnostics.decorators",
                    "semantic_kernel.utils.telemetry.model_diagnostics.function_tracer",
                ),
                snapshots,
                strict=True,
            )
        }
        real_import = importlib.import_module

        def fake_import(name: str, package: str | None = None) -> Any:
            return modules.get(name) or real_import(name, package)

        monkeypatch.setattr(importlib, "import_module", fake_import)
        monkeypatch.setenv("OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT", "true")

        assert instrument(Frameworks.SemanticKernel, None) is True
        assert all(settings.enable_otel_diagnostics for settings in snapshots)
        assert all(settings.enable_otel_diagnostics_sensitive for settings in snapshots)
        assert os.environ["SEMANTICKERNEL_EXPERIMENTAL_GENAI_ENABLE_OTEL_DIAGNOSTICS"] == "true"
        assert (
            os.environ["SEMANTICKERNEL_EXPERIMENTAL_GENAI_ENABLE_OTEL_DIAGNOSTICS_SENSITIVE"]
            == "true"
        )

    def test_thread_safety(self) -> None:
        """Instrumentation should be thread-safe."""
        results: list[bool] = []
        errors: list[Exception] = []

        def try_instrument() -> None:
            try:
                result = instrument(Frameworks.Strands, None)
                results.append(result)
            except Exception as e:
                errors.append(e)

        threads = [threading.Thread(target=try_instrument) for _ in range(10)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()

        # No errors should occur
        assert len(errors) == 0
        # Only one thread should successfully instrument
        assert sum(results) == 1
        assert Frameworks.Strands in _instrumented


def test_adk_patch_preserves_native_sanitizer_and_weaves_inline_data(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """ADK's credential exclusions survive the multimodal compatibility patch."""
    tracing = ModuleType("google.adk.telemetry.tracing")
    sanitized = {
        "model": "test-model",
        "config": {"http_options": {}, "temperature": 0},
        "contents": [
            {
                "role": "user",
                "parts": [{"text": "before"}, {"text": "after"}],
            },
            {"role": "model", "parts": []},
        ],
    }
    calls: list[Any] = []

    def native_sanitizer(request: Any) -> dict[str, Any]:
        calls.append(request)
        return sanitized

    tracing._build_llm_request_for_trace = native_sanitizer  # type: ignore[attr-defined]
    telemetry = ModuleType("google.adk.telemetry")
    telemetry.tracing = tracing  # type: ignore[attr-defined]
    adk = ModuleType("google.adk")
    adk.telemetry = telemetry  # type: ignore[attr-defined]
    google = ModuleType("google")
    google.adk = adk  # type: ignore[attr-defined]
    monkeypatch.setitem(sys.modules, "google", google)
    monkeypatch.setitem(sys.modules, "google.adk", adk)
    monkeypatch.setitem(sys.modules, "google.adk.telemetry", telemetry)
    monkeypatch.setitem(sys.modules, "google.adk.telemetry.tracing", tracing)

    request = SimpleNamespace(
        contents=[
            SimpleNamespace(
                parts=[
                    SimpleNamespace(inline_data=None),
                    SimpleNamespace(
                        inline_data=SimpleNamespace(
                            data=b"image",
                            mime_type="image/png",
                        )
                    ),
                    SimpleNamespace(inline_data=None),
                ]
            ),
            SimpleNamespace(parts=None),
        ]
    )

    assert patch_adk_tracing() is True
    result = tracing._build_llm_request_for_trace(request)  # type: ignore[attr-defined]

    assert calls == [request]
    assert result["config"] == {"http_options": {}, "temperature": 0}
    assert result["contents"][0]["parts"] == [
        {"text": "before"},
        {
            "inline_data": {
                "mime_type": "image/png",
                "data": "aW1hZ2U=",
            }
        },
        {"text": "after"},
    ]
    assert result["contents"][1] == {"role": "model", "parts": []}
    assert patch_adk_tracing() is True


class TestAnthropicCompatibility:
    """Regression coverage for Anthropic request sentinels observed by Logfire."""

    def test_strip_anthropic_omit_recursively(self) -> None:
        class Omit:
            pass

        omit = Omit()
        value = {
            "model": "claude",
            "stop_sequences": omit,
            "messages": [
                {"role": "user", "content": "hello", "cache_control": omit},
                omit,
            ],
            "tools": ({"name": "weather", "extra": omit}, omit),
        }

        assert _strip_anthropic_omit(value, Omit) == {
            "model": "claude",
            "messages": [{"role": "user", "content": "hello"}],
            "tools": ({"name": "weather"},),
        }
        assert value["stop_sequences"] is omit

    def test_logfire_patch_sanitizes_copy_before_endpoint_reader(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        class Omit:
            pass

        omit = Omit()
        observed: list[dict[str, Any]] = []

        def original(options: Any, *, version: int = 2) -> tuple[dict[str, Any], int]:
            observed.append(options.json_data)
            return options.json_data, version

        integration = SimpleNamespace(get_endpoint_config=original)
        anthropic_types = SimpleNamespace(Omit=Omit)
        real_import = importlib.import_module

        def fake_import(name: str, package: str | None = None) -> Any:
            if name == "anthropic._types":
                return anthropic_types
            if name == "logfire._internal.integrations.llm_providers.anthropic":
                return integration
            return real_import(name, package)

        monkeypatch.setattr(importlib, "import_module", fake_import)

        class Options:
            def __init__(self, json_data: dict[str, Any]) -> None:
                self.json_data = json_data

            def model_copy(self, *, update: dict[str, Any]) -> "Options":
                return Options(update["json_data"])

        options = Options(
            {
                "model": "claude",
                "stop_sequences": omit,
                "messages": [{"role": "user", "content": "hello"}],
            }
        )

        assert _patch_logfire_anthropic_omit() is True
        assert integration.get_endpoint_config(options, version=2) == (
            {
                "model": "claude",
                "messages": [{"role": "user", "content": "hello"}],
            },
            2,
        )
        assert observed == [
            {
                "model": "claude",
                "messages": [{"role": "user", "content": "hello"}],
            }
        ]
        assert options.json_data["stop_sequences"] is omit
        assert _patch_logfire_anthropic_omit() is False

    def test_logfire_patch_carries_anthropic_stream_accumulator_state(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        calls: list[tuple[str, dict[str, Any]]] = []

        def accumulate_event(
            *,
            event: Any,
            current_snapshot: Any,
            json_bufs: dict[int, bytes],
        ) -> str:
            calls.append(
                (
                    "normal",
                    {
                        "event": event,
                        "current_snapshot": current_snapshot,
                        "json_bufs": json_bufs,
                    },
                )
            )
            json_bufs[0] = b"partial"
            return "normal-snapshot"

        def beta_accumulate_event(
            *,
            event: Any,
            current_snapshot: Any,
            json_bufs: dict[int, bytes],
            request_headers: dict[str, str],
        ) -> str:
            calls.append(
                (
                    "beta",
                    {
                        "event": event,
                        "current_snapshot": current_snapshot,
                        "json_bufs": json_bufs,
                        "request_headers": request_headers,
                    },
                )
            )
            return "beta-snapshot"

        class State:
            def __init__(self) -> None:
                self._message: Any = None
                self._chunk_count = 0

            def record_chunk(self, chunk: Any) -> None:
                raise TypeError("old Logfire omitted json_bufs")

        integration = SimpleNamespace(AnthropicMessageStreamState=State)
        messages = SimpleNamespace(accumulate_event=accumulate_event)
        beta_messages = SimpleNamespace(accumulate_event=beta_accumulate_event)
        real_import = importlib.import_module

        def fake_import(name: str, package: str | None = None) -> Any:
            modules = {
                "logfire._internal.integrations.llm_providers.anthropic": integration,
                "anthropic.lib.streaming._messages": messages,
                "anthropic.lib.streaming._beta_messages": beta_messages,
            }
            if name in modules:
                return modules[name]
            return real_import(name, package)

        monkeypatch.setattr(importlib, "import_module", fake_import)

        NormalChunk = type(
            "NormalChunk",
            (),
            {
                "__module__": "anthropic.types.raw_content_block_delta_event",
                "delta": SimpleNamespace(type="text_delta"),
            },
        )
        BetaChunk = type(
            "BetaChunk",
            (),
            {
                "__module__": "anthropic.types.beta.beta_raw_message_delta_event",
                "delta": SimpleNamespace(type="message_delta"),
            },
        )

        assert _patch_logfire_anthropic_streaming() is True
        state = State()
        state.record_chunk(NormalChunk())
        state.record_chunk(BetaChunk())

        assert calls[0][0] == "normal"
        assert calls[0][1]["current_snapshot"] is None
        assert calls[1][0] == "beta"
        assert calls[1][1]["current_snapshot"] == "normal-snapshot"
        assert calls[1][1]["json_bufs"] is calls[0][1]["json_bufs"]
        assert calls[1][1]["request_headers"] == {}
        assert state._message == "beta-snapshot"
        assert state._chunk_count == 1
        assert _patch_logfire_anthropic_streaming() is False


def test_suspend_otel_exporter_env_restores_exact_values(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector/base")
    monkeypatch.setenv("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", "http://collector/traces")
    monkeypatch.delenv("OTEL_EXPORTER_OTLP_METRICS_ENDPOINT", raising=False)
    monkeypatch.delenv("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT", raising=False)

    with _suspend_otel_exporter_env():
        assert "OTEL_EXPORTER_OTLP_ENDPOINT" not in os.environ
        assert "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT" not in os.environ
        # A value introduced inside the guarded configure call must not leak.
        monkeypatch.setenv("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT", "http://unexpected/logs")

    assert os.environ["OTEL_EXPORTER_OTLP_ENDPOINT"] == "http://collector/base"
    assert os.environ["OTEL_EXPORTER_OTLP_TRACES_ENDPOINT"] == "http://collector/traces"
    assert "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT" not in os.environ
    assert "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT" not in os.environ
