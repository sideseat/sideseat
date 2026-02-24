"""Bedrock Agent Runtime instrumentation — InvokeAgent, InvokeInlineAgent."""

from __future__ import annotations

import functools
import json
import logging
from typing import TYPE_CHECKING, Any

from opentelemetry import context, trace
from opentelemetry.trace import SpanKind, StatusCode

from sideseat.telemetry.encoding import encode_value

if TYPE_CHECKING:
    from opentelemetry.sdk.trace import TracerProvider
    from opentelemetry.trace import Span, Tracer

logger = logging.getLogger("sideseat.instrumentors.aws.bedrock_agent")

_TRACER_NAME = "sideseat.aws.bedrock_agent"

_SYSTEM = "gen_ai.system"
_PROVIDER_NAME = "gen_ai.provider.name"
_OPERATION = "gen_ai.operation.name"
_REQUEST_MODEL = "gen_ai.request.model"
_RESPONSE_MODEL = "gen_ai.response.model"
_INPUT_TOKENS = "gen_ai.usage.input_tokens"
_OUTPUT_TOKENS = "gen_ai.usage.output_tokens"
_AGENT_ID = "gen_ai.agent.id"
_SYSTEM_VALUE = "aws_bedrock"


def patch_bedrock_agent_client(client: Any, provider: TracerProvider | None) -> None:
    """Replace invoke_agent/invoke_inline_agent methods on a bedrock-agent-runtime client."""
    tracer = _get_tracer(provider)

    for method_name, wrapper_fn in (
        ("invoke_agent", _wrap_invoke_agent),
        ("invoke_inline_agent", _wrap_invoke_inline_agent),
    ):
        original = getattr(client, method_name, None)
        if original is None:
            continue
        setattr(client, method_name, wrapper_fn(original, tracer))

    logger.debug("Patched bedrock-agent-runtime client")


def _get_tracer(provider: TracerProvider | None) -> Tracer:
    if provider is not None:
        return provider.get_tracer(_TRACER_NAME)
    return trace.get_tracer(_TRACER_NAME)


# ---------------------------------------------------------------------------
# InvokeAgent
# ---------------------------------------------------------------------------


def _wrap_invoke_agent(original: Any, tracer: Tracer) -> Any:
    @functools.wraps(original)
    def instrumented_invoke_agent(**kwargs: Any) -> Any:
        agent_id = kwargs.get("agentId", "unknown")
        span = tracer.start_span(f"invoke_agent {agent_id}", kind=SpanKind.CLIENT)
        token = context.attach(trace.set_span_in_context(span))

        span.set_attribute(_SYSTEM, _SYSTEM_VALUE)
        span.set_attribute(_PROVIDER_NAME, _SYSTEM_VALUE)
        span.set_attribute(_OPERATION, "invoke_agent")
        if agent_id != "unknown":
            span.set_attribute(_AGENT_ID, agent_id)

        input_text = kwargs.get("inputText", "")
        if input_text:
            span.add_event("gen_ai.user.message", {"content": input_text})

        try:
            response = original(**kwargs)
        except Exception as exc:
            span.set_status(StatusCode.ERROR, str(exc))
            span.record_exception(exc)
            span.end()
            context.detach(token)
            raise

        completion = response.get("completion")
        if completion is None:
            span.end()
            context.detach(token)
            return response

        wrapper = _InvokeAgentStreamWrapper(completion, span, token)
        response["completion"] = wrapper
        return response

    return instrumented_invoke_agent


# ---------------------------------------------------------------------------
# InvokeInlineAgent
# ---------------------------------------------------------------------------


def _wrap_invoke_inline_agent(original: Any, tracer: Tracer) -> Any:
    @functools.wraps(original)
    def instrumented_invoke_inline_agent(**kwargs: Any) -> Any:
        span = tracer.start_span("invoke_inline_agent", kind=SpanKind.CLIENT)
        token = context.attach(trace.set_span_in_context(span))

        span.set_attribute(_SYSTEM, _SYSTEM_VALUE)
        span.set_attribute(_PROVIDER_NAME, _SYSTEM_VALUE)
        span.set_attribute(_OPERATION, "invoke_agent")

        input_text = kwargs.get("inputText", "")
        if input_text:
            span.add_event("gen_ai.user.message", {"content": input_text})

        try:
            response = original(**kwargs)
        except Exception as exc:
            span.set_status(StatusCode.ERROR, str(exc))
            span.record_exception(exc)
            span.end()
            context.detach(token)
            raise

        completion = response.get("completion")
        if completion is None:
            span.end()
            context.detach(token)
            return response

        wrapper = _InvokeAgentStreamWrapper(completion, span, token)
        response["completion"] = wrapper
        return response

    return instrumented_invoke_inline_agent


# ---------------------------------------------------------------------------
# Shared stream wrapper
# ---------------------------------------------------------------------------


class _InvokeAgentStreamWrapper:
    """Wraps agent completion stream, accumulating response chunks."""

    def __init__(self, inner: Any, span: Span, ctx_token: object) -> None:
        self._inner = iter(inner)
        self._span = span
        self._ctx_token = ctx_token
        self._ended = False
        self._response_text = ""
        self._input_tokens = 0
        self._output_tokens = 0
        self._model: str | None = None

    def __iter__(self) -> _InvokeAgentStreamWrapper:
        return self

    def __next__(self) -> Any:
        try:
            chunk = next(self._inner)
        except StopIteration:
            self._finalize()
            raise
        except Exception as exc:
            self._on_error(exc)
            raise

        self._process_chunk(chunk)
        return chunk

    def close(self) -> None:
        if hasattr(self._inner, "close"):
            self._inner.close()
        self._finalize()

    def __enter__(self) -> _InvokeAgentStreamWrapper:
        return self

    def __exit__(self, *args: Any) -> None:
        self.close()

    def __getattr__(self, name: str) -> Any:
        return getattr(self._inner, name)

    def _process_chunk(self, chunk: dict[str, Any]) -> None:
        if "chunk" in chunk:
            data = chunk["chunk"]
            raw = data.get("bytes", b"")
            if raw:
                try:
                    self._response_text += raw.decode("utf-8")
                except (UnicodeDecodeError, AttributeError):
                    pass

        elif "trace" in chunk:
            trace_data = chunk["trace"].get("trace", {})
            # Extract model invocation data from all trace types
            for trace_key in (
                "orchestrationTrace",
                "preProcessingTrace",
                "postProcessingTrace",
                "routingClassifierTrace",
            ):
                sub_trace = trace_data.get(trace_key, {})
                model_invoke = sub_trace.get("modelInvocationOutput", {})
                if model_invoke:
                    self._accumulate_model_invoke(model_invoke)

    def _accumulate_model_invoke(self, model_invoke: dict[str, Any]) -> None:
        """Accumulate token usage and model info from a model invocation."""
        usage = model_invoke.get("metadata", {}).get("usage", {})
        if usage:
            self._input_tokens += usage.get("inputTokens", 0)
            self._output_tokens += usage.get("outputTokens", 0)
        if not self._model:
            fm = model_invoke.get("metadata", {}).get("foundationModel")
            if fm:
                self._model = fm

    def _finalize(self) -> None:
        if self._ended:
            return
        self._ended = True

        span = self._span
        try:
            if self._model:
                span.set_attribute(_REQUEST_MODEL, self._model)
                span.set_attribute(_RESPONSE_MODEL, self._model)

            if self._input_tokens:
                span.set_attribute(_INPUT_TOKENS, self._input_tokens)
            if self._output_tokens:
                span.set_attribute(_OUTPUT_TOKENS, self._output_tokens)

            if self._response_text:
                output_content = [{"text": self._response_text}]
                output_msg = {"role": "assistant", "content": output_content}
                span.add_event(
                    "gen_ai.client.inference.operation.details",
                    {
                        "gen_ai.output.messages": json.dumps(encode_value([output_msg])),
                    },
                )
                span.add_event(
                    "gen_ai.choice",
                    {
                        "message": json.dumps(encode_value(output_content)),
                        "finish_reason": "end_turn",
                    },
                )

            span.set_status(StatusCode.OK)
        finally:
            span.end()
            context.detach(self._ctx_token)

    def _on_error(self, exc: Exception) -> None:
        if self._ended:
            return
        self._ended = True
        self._span.set_status(StatusCode.ERROR, str(exc))
        self._span.record_exception(exc)
        self._span.end()
        context.detach(self._ctx_token)
