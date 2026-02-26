"""Bedrock Agent Runtime instrumentation — InvokeAgent, InvokeInlineAgent."""

from __future__ import annotations

import functools
import json
import logging
from typing import TYPE_CHECKING, Any

from opentelemetry import context, trace
from opentelemetry.trace import SpanKind, StatusCode

from sideseat.instrumentors.aws._constants import (
    AGENT_ID,
    INPUT_TOKENS,
    OPERATION,
    OUTPUT_TOKENS,
    PROVIDER_NAME,
    REQUEST_MODEL,
    RESPONSE_MODEL,
    SYSTEM,
    SYSTEM_VALUE,
    get_tracer,
)
from sideseat.telemetry.encoding import encode_value

if TYPE_CHECKING:
    from opentelemetry.sdk.trace import TracerProvider
    from opentelemetry.trace import Span, Tracer

logger = logging.getLogger("sideseat.instrumentors.aws.bedrock_agent")

_TRACER_NAME = "sideseat.aws.bedrock_agent"


def patch_bedrock_agent_client(client: Any, provider: TracerProvider | None) -> None:
    """Replace invoke_agent/invoke_inline_agent methods on a bedrock-agent-runtime client."""
    tracer = get_tracer(provider, _TRACER_NAME)

    for method_name, use_agent_id in (
        ("invoke_agent", True),
        ("invoke_inline_agent", False),
    ):
        original = getattr(client, method_name, None)
        if original is None:
            continue
        wrapped = _wrap_agent_method(original, tracer, use_agent_id=use_agent_id)
        setattr(client, method_name, wrapped)

    logger.debug("Patched bedrock-agent-runtime client")


# ---------------------------------------------------------------------------
# Shared agent method wrapper
# ---------------------------------------------------------------------------


def _wrap_agent_method(original: Any, tracer: Tracer, *, use_agent_id: bool) -> Any:
    @functools.wraps(original)
    def instrumented(**kwargs: Any) -> Any:
        agent_id = kwargs.get("agentId", "unknown") if use_agent_id else None
        span_name = f"invoke_agent {agent_id}" if agent_id else "invoke_inline_agent"

        span = tracer.start_span(span_name, kind=SpanKind.CLIENT)
        token = context.attach(trace.set_span_in_context(span))

        span.set_attribute(SYSTEM, SYSTEM_VALUE)
        span.set_attribute(PROVIDER_NAME, SYSTEM_VALUE)
        span.set_attribute(OPERATION, "invoke_agent")

        if agent_id and agent_id != "unknown":
            span.set_attribute(AGENT_ID, agent_id)

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
            span.set_status(StatusCode.OK)
            span.end()
            context.detach(token)
            return response

        wrapper = _InvokeAgentStreamWrapper(completion, span, token)
        response["completion"] = wrapper
        return response

    return instrumented


# ---------------------------------------------------------------------------
# Stream wrapper
# ---------------------------------------------------------------------------


class _InvokeAgentStreamWrapper:
    """Wraps agent completion stream, accumulating response chunks."""

    __slots__ = (
        "_inner",
        "_span",
        "_ctx_token",
        "_ended",
        "_response_text",
        "_input_tokens",
        "_output_tokens",
        "_model",
    )

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
            try:
                if self._model:
                    span.set_attribute(REQUEST_MODEL, self._model)
                    span.set_attribute(RESPONSE_MODEL, self._model)

                if self._input_tokens:
                    span.set_attribute(INPUT_TOKENS, self._input_tokens)
                if self._output_tokens:
                    span.set_attribute(OUTPUT_TOKENS, self._output_tokens)

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
            except Exception:
                logger.debug("Failed to emit agent stream events", exc_info=True)

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
