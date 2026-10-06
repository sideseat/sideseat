from __future__ import annotations

import os
from collections.abc import Sequence
from typing import Any

import pytest
from opentelemetry.sdk.trace import ReadableSpan, Span, SpanProcessor, TracerProvider

import sideseat
from sideseat import integrations
from sideseat.errors import ConfigurationError, IntegrationError
from sideseat.integrations import Integration, SetupContext
from sideseat.integrations._util import temporary_env
from sideseat.testing import capture


def test_every_registered_integration_loads_with_its_own_name() -> None:
    for name in integrations.names():
        cls = integrations.load(name)
        assert issubclass(cls, Integration)
        assert cls.name == name
        assert cls.packages, name


def test_provider_libraries_are_never_auto_detected() -> None:
    for name in ("bedrock", "openai", "anthropic", "azure-openai", "google-genai", "vertex-ai"):
        assert integrations.load(name).detectable is False, name


def test_an_unknown_name_lists_the_known_ones() -> None:
    with pytest.raises(IntegrationError, match="strands"):
        integrations.load("strandz")


class _Missing(Integration):
    name = "missing"
    packages = ("definitely-not-installed",)
    extra = "missing"

    def prepare(self, ctx: SetupContext) -> None:
        import definitely_not_installed  # type: ignore[import-not-found]  # noqa: F401


def test_a_requested_integration_that_cannot_import_is_an_error_with_an_install_hint() -> None:
    with pytest.raises(IntegrationError, match=r'pip install "sideseat\[missing\]"'):
        sideseat.init(integrations=[_Missing()], export=False)


class _Recording(Integration):
    name = "recording"
    packages = ("opentelemetry-sdk",)

    def __init__(self, log: list[str]) -> None:
        self.log = log

    def prepare(self, ctx: SetupContext) -> None:
        assert ctx.tracer_provider is None
        self.log.append("prepare")

    def span_processors(self, ctx: SetupContext) -> Sequence[SpanProcessor]:
        return (_SeesCorrelation(self.log),)

    def instrument(self, ctx: SetupContext) -> None:
        assert ctx.tracer_provider is not None
        self.log.append("instrument")

    def shutdown(self) -> None:
        self.log.append("shutdown")


class _SeesCorrelation(SpanProcessor):
    def __init__(self, log: list[str]) -> None:
        self.log = log

    def on_start(self, span: Span, parent_context: Any = None) -> None:
        self.log.append(f"start:{(span.attributes or {}).get('session.id')}")

    def on_end(self, span: ReadableSpan) -> None:
        pass


def test_hooks_run_in_order_and_integration_processors_see_correlation() -> None:
    log: list[str] = []
    with capture(integrations=[_Recording(log)]):
        with sideseat.session("s-9"), sideseat.span("work"):
            pass
    assert log == ["prepare", "instrument", "start:s-9", "shutdown"]


def test_the_primary_integration_names_the_service_when_none_is_given() -> None:
    with capture(integrations=[_Recording([])]) as spans:
        with sideseat.span("x"):
            pass
    resource = spans.finished()[0].resource.attributes
    assert resource["service.name"] == "opentelemetry-sdk"
    assert resource["sideseat.framework"] == "recording"


class _Owner(Integration):
    name = "owner"
    packages = ("opentelemetry-sdk",)
    owns_tracer_provider = True

    def create_tracer_provider(self, ctx: SetupContext) -> TracerProvider:
        return TracerProvider(resource=ctx.resource)


class _SecondOwner(_Owner):
    name = "second-owner"


def test_two_provider_owners_are_rejected() -> None:
    with pytest.raises(ConfigurationError, match="only one integration"):
        sideseat.init(integrations=[_Owner(), _SecondOwner()], export=False)


def test_an_owned_provider_receives_sideseat_processors() -> None:
    with capture(integrations=[_Owner()]) as spans:
        with sideseat.session("s-owned"), sideseat.span("in-owned-provider"):
            pass
    assert spans.named("in-owned-provider")[0].attributes["session.id"] == "s-owned"


def test_temporary_env_restores_set_and_unset_variables(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("SIDESEAT_TEST_KEPT", "before")
    monkeypatch.delenv("SIDESEAT_TEST_ABSENT", raising=False)
    with temporary_env({"SIDESEAT_TEST_KEPT": None, "SIDESEAT_TEST_ABSENT": "during"}):
        assert "SIDESEAT_TEST_KEPT" not in os.environ
        assert os.environ["SIDESEAT_TEST_ABSENT"] == "during"
    assert os.environ["SIDESEAT_TEST_KEPT"] == "before"
    assert "SIDESEAT_TEST_ABSENT" not in os.environ


def test_the_claude_code_cli_environment_points_at_the_project() -> None:
    from sideseat._config import resolve
    from sideseat.integrations.claude_agent_sdk import cli_environment

    settings = resolve(
        endpoint="http://host:5388",
        project="p1",
        api_key="secret",
        service_name=None,
        service_version=None,
        integrations=None,
        capture_content=True,
        disabled=False,
        debug=False,
        export=True,
        metrics=False,
        logs=False,
        capture_python_logs=False,
        resource_attributes=None,
        span_processors=None,
    )
    env = cli_environment(settings)
    assert env["OTEL_EXPORTER_OTLP_TRACES_ENDPOINT"] == "http://host:5388/otel/p1/v1/traces"
    assert env["BETA_TRACING_ENDPOINT"] == "http://host:5388/otel/p1"
    assert env["OTEL_EXPORTER_OTLP_TRACES_HEADERS"] == "Authorization=Bearer secret"
    assert env["OTEL_LOG_USER_PROMPTS"] == "1"
    assert env["OTEL_TRACES_EXPORTER"] == "otlp"


def test_semantic_kernel_exports_the_log_records_its_diagnostics_write(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """Semantic Kernel writes prompts and completions as INFO records of its model-diagnostics
    logger and never as span attributes, so the integration must hand them to the log pipeline."""
    import logging
    import sys
    import types

    from opentelemetry import _logs
    from opentelemetry.sdk._logs.export import (
        InMemoryLogRecordExporter,
        SimpleLogRecordProcessor,
    )

    for module_name in (
        "semantic_kernel.utils.telemetry.agent_diagnostics.decorators",
        "semantic_kernel.utils.telemetry.model_diagnostics.decorators",
        "semantic_kernel.utils.telemetry.model_diagnostics.function_tracer",
    ):
        module = types.ModuleType(module_name)
        module.MODEL_DIAGNOSTICS_SETTINGS = types.SimpleNamespace(  # type: ignore[attr-defined]
            enable_otel_diagnostics=False, enable_otel_diagnostics_sensitive=False
        )
        monkeypatch.setitem(sys.modules, module_name, module)

    from sideseat.integrations.semantic_kernel import SemanticKernel

    monkeypatch.setattr(
        SemanticKernel, "installed_package", classmethod(lambda cls: ("semantic-kernel", "1.0"))
    )
    sideseat.init(integrations=["semantic-kernel"], export=False, metrics=False)
    exporter = InMemoryLogRecordExporter()
    _logs.get_logger_provider().add_log_record_processor(  # type: ignore[attr-defined]
        SimpleLogRecordProcessor(exporter)
    )
    logging.getLogger("semantic_kernel.utils.telemetry.model_diagnostics.decorators").info(
        '{"role": "user", "content": "Hello"}', extra={"event.name": "gen_ai.user.message"}
    )

    records = [data.log_record for data in exporter.get_finished_logs()]
    assert [record.body for record in records] == ['{"role": "user", "content": "Hello"}']

    sideseat.shutdown()
    for module_name in (
        "semantic_kernel.utils.telemetry.agent_diagnostics.decorators",
        "semantic_kernel.utils.telemetry.model_diagnostics.decorators",
    ):
        settings = sys.modules[module_name].MODEL_DIAGNOSTICS_SETTINGS
        assert not settings.enable_otel_diagnostics, module_name
        assert not settings.enable_otel_diagnostics_sensitive, module_name
    assert records[0].attributes is not None
    assert records[0].attributes["event.name"] == "gen_ai.user.message"


def test_langsmith_switches_hold_while_tracing_and_are_restored_at_shutdown(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from sideseat.integrations.langsmith import LangSmith

    monkeypatch.setenv("LANGSMITH_TRACING", "false")
    monkeypatch.delenv("LANGSMITH_TRACING_MODE", raising=False)
    integration = LangSmith()
    integration.prepare(None)  # type: ignore[arg-type]
    assert os.environ["LANGSMITH_TRACING"] == "true"
    assert os.environ["LANGSMITH_TRACING_MODE"] == "otel"
    integration.shutdown()
    assert os.environ["LANGSMITH_TRACING"] == "false"
    assert "LANGSMITH_TRACING_MODE" not in os.environ


def _fake_module(monkeypatch: pytest.MonkeyPatch, name: str, **attributes: Any) -> Any:
    import sys
    import types

    module = types.ModuleType(name)
    module.__dict__.update(attributes)
    monkeypatch.setitem(sys.modules, name, module)
    return module


def _pretend_installed(monkeypatch: pytest.MonkeyPatch, cls: type[Integration]) -> None:
    monkeypatch.setattr(cls, "installed_package", classmethod(lambda c: (c.packages[0], "1.0")))


def test_traceloop_keeps_content_off_after_init_where_its_instrumentations_read_it(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import types

    from opentelemetry.sdk.resources import Resource

    from sideseat._config import resolve
    from sideseat.integrations.traceloop import TraceLoop

    seen: list[str | None] = []
    traceloop = types.SimpleNamespace(
        init=lambda **_: seen.append(os.environ.get("TRACELOOP_TRACE_CONTENT"))
    )
    _fake_module(monkeypatch, "traceloop")
    _fake_module(monkeypatch, "traceloop.sdk", Traceloop=traceloop)
    _fake_module(
        monkeypatch,
        "traceloop.sdk.instruments",
        Instruments=types.SimpleNamespace(REQUESTS="requests", URLLIB3="urllib3"),
    )
    monkeypatch.setenv("TRACELOOP_TRACE_CONTENT", "true")
    settings = resolve(
        endpoint=None,
        project=None,
        api_key=None,
        service_name=None,
        service_version=None,
        integrations=None,
        capture_content=False,
        disabled=False,
        debug=False,
        export=False,
        metrics=False,
        logs=False,
        capture_python_logs=False,
        resource_attributes=None,
        span_processors=None,
    )
    ctx = SetupContext(
        settings=settings, resource=Resource.get_empty(), service_name="app", service_version="1"
    )
    TraceLoop().instrument(ctx)
    assert seen == ["false"]
    assert os.environ["TRACELOOP_TRACE_CONTENT"] == "false"


def test_agent_framework_settings_are_restored_at_shutdown(monkeypatch: pytest.MonkeyPatch) -> None:
    import types

    from sideseat.integrations.agent_framework import AgentFramework

    settings = types.SimpleNamespace(enable_instrumentation=False, enable_sensitive_data=False)
    _fake_module(monkeypatch, "agent_framework")
    _fake_module(monkeypatch, "agent_framework.observability", OBSERVABILITY_SETTINGS=settings)
    _pretend_installed(monkeypatch, AgentFramework)

    sideseat.init(integrations=["agent-framework"], export=False)
    assert (settings.enable_instrumentation, settings.enable_sensitive_data) == (True, True)
    sideseat.shutdown()
    assert (settings.enable_instrumentation, settings.enable_sensitive_data) == (False, False)


def test_the_strands_encoder_patch_is_removed_at_shutdown(monkeypatch: pytest.MonkeyPatch) -> None:
    import types

    from sideseat.integrations.strands import Strands

    def original(self: Any, value: Any) -> Any:
        return "<replaced>"

    encoder = type("JSONEncoder", (), {"_process_value": original})
    _fake_module(monkeypatch, "strands")
    tracer = types.SimpleNamespace(JSONEncoder=encoder)
    _fake_module(monkeypatch, "strands.telemetry", tracer=tracer)
    _pretend_installed(monkeypatch, Strands)

    sideseat.init(integrations=["strands"], export=False)
    assert encoder()._process_value(b"\x00") == "AA=="  # type: ignore[attr-defined]
    sideseat.shutdown()
    assert encoder._process_value is original  # type: ignore[attr-defined]
