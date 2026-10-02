from __future__ import annotations

import asyncio
import concurrent.futures

import pytest
from opentelemetry import context as otel_context
from opentelemetry import trace as otel_trace
from opentelemetry.trace import StatusCode

import sideseat
from sideseat.errors import ConfigurationError, SideSeatError
from sideseat.testing import capture


def test_trace_is_a_root_even_inside_another_span() -> None:
    with capture(integrations=[]) as spans:
        with sideseat.trace("outer"), sideseat.trace("inner"):
            pass
    outer, inner = spans.named("outer")[0], spans.named("inner")[0]
    assert inner.parent is None
    assert inner.context.trace_id != outer.context.trace_id


def test_span_is_a_child_of_the_active_span() -> None:
    with capture(integrations=[]) as spans:
        with sideseat.trace("root"), sideseat.span("child"):
            pass
    root, child = spans.named("root")[0], spans.named("child")[0]
    assert child.parent is not None and child.parent.span_id == root.context.span_id


def test_a_session_reaches_spans_a_framework_creates() -> None:
    with capture(integrations=[]) as spans:
        framework = otel_trace.get_tracer("some.framework")
        with sideseat.session("s-1", user_id="u-1"), framework.start_as_current_span("llm"):
            pass
    (span,) = spans.named("llm")
    assert span.attributes == {"session.id": "s-1", "user.id": "u-1"}


def test_trace_correlation_applies_to_descendants_only() -> None:
    with capture(integrations=[]) as spans:
        with sideseat.trace("conversation", session_id="s-2"), sideseat.span("step"):
            pass
        with sideseat.span("unrelated"):
            pass
    assert spans.named("conversation")[0].attributes["session.id"] == "s-2"
    assert spans.named("step")[0].attributes["session.id"] == "s-2"
    assert "session.id" not in (spans.named("unrelated")[0].attributes or {})


def test_a_nested_scope_overrides_and_then_restores() -> None:
    with capture(integrations=[]) as spans:
        with sideseat.session("outer", user_id="u"):
            with sideseat.session("inner"), sideseat.span("a"):
                pass
            with sideseat.span("b"):
                pass
    assert spans.named("a")[0].attributes == {"session.id": "inner", "user.id": "u"}
    assert spans.named("b")[0].attributes == {"session.id": "outer", "user.id": "u"}


def test_correlation_follows_async_tasks_and_context_propagating_threads() -> None:
    async def in_task() -> None:
        with sideseat.span("task"):
            await asyncio.sleep(0)

    def in_thread() -> None:
        with sideseat.span("thread"):
            pass

    with capture(integrations=[]) as spans:
        with sideseat.session("s-3"):
            asyncio.run(in_task())
            ctx = otel_context.get_current()
            with concurrent.futures.ThreadPoolExecutor(1) as pool:
                pool.submit(lambda: otel_context.attach(ctx) and in_thread()).result()
    assert spans.named("task")[0].attributes["session.id"] == "s-3"
    assert spans.named("thread")[0].attributes["session.id"] == "s-3"


def test_correlation_is_not_sent_over_the_network_as_baggage() -> None:
    from opentelemetry import baggage

    with capture(integrations=[]), sideseat.session("private", user_id="person"):
        assert baggage.get_all() == {}


def test_an_exception_marks_the_span_and_propagates() -> None:
    with capture(integrations=[]) as spans:
        with pytest.raises(RuntimeError), sideseat.span("fails"):
            raise RuntimeError("boom")
    (span,) = spans.named("fails")
    assert span.status.status_code is StatusCode.ERROR
    assert span.events[0].name == "exception"


def test_observe_wraps_sync_and_async_functions() -> None:
    @sideseat.observe()
    def plan() -> str:
        return "ok"

    @sideseat.observe("fetch-weather")
    async def fetch() -> int:
        return 3

    with capture(integrations=[]) as spans:
        assert plan() == "ok"
        assert asyncio.run(fetch()) == 3
    names = {s.name for s in spans.finished()}
    assert {"test_observe_wraps_sync_and_async_functions.<locals>.plan", "fetch-weather"} <= names


def test_the_resource_names_the_sdk_and_the_integrations() -> None:
    from sideseat.integrations.passthrough import Langflow

    with capture(integrations=[Langflow()], service_name="travel-agent") as spans:
        with sideseat.span("work"):
            pass
    resource = spans.finished()[0].resource.attributes
    assert resource["service.name"] == "travel-agent"
    assert resource["telemetry.sdk.name"] == "sideseat"
    assert resource["sideseat.framework"] == "langflow"
    assert tuple(resource["sideseat.integrations"]) == ("langflow",)


def test_init_twice_with_the_same_settings_returns_the_same_client() -> None:
    first = sideseat.init(integrations=[], export=False)
    assert sideseat.init(integrations=[], export=False) is first


def test_init_twice_with_different_settings_is_an_error() -> None:
    sideseat.init(integrations=[], export=False)
    with pytest.raises(ConfigurationError, match="different settings"):
        sideseat.init(integrations=[], export=False, project="other")


def test_using_the_module_api_before_init_is_an_error() -> None:
    with pytest.raises(SideSeatError, match="init"), sideseat.span("x"):
        pass


def test_disabled_records_nothing_and_flushes_successfully() -> None:
    client = sideseat.init(disabled=True)
    with sideseat.trace("ignored", session_id="s") as span:
        assert not span.is_recording()
    assert client.integrations == ()
    assert sideseat.flush() is True
    assert sideseat.shutdown() is True


def test_shutdown_is_idempotent_and_reports_success() -> None:
    client = sideseat.init(integrations=[], export=False)
    assert client.shutdown() is True
    assert client.shutdown() is True


def test_content_capture_sets_the_standard_genai_switch_unless_already_chosen(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import os

    with capture(integrations=[]):
        assert os.environ["OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT"] == "true"
    monkeypatch.setenv("OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT", "false")
    with capture(integrations=[]):
        assert os.environ["OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT"] == "false"


def test_an_application_provider_is_reused_not_replaced() -> None:
    from opentelemetry.sdk.trace import TracerProvider

    provider = TracerProvider()
    otel_trace.set_tracer_provider(provider)
    client = sideseat.init(integrations=[], export=False)
    assert client.tracer_provider is provider
