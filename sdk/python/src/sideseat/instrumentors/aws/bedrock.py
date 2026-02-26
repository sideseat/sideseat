"""Bedrock Runtime instrumentation — Converse, ConverseStream, InvokeModel."""

from __future__ import annotations

import functools
import io
import json
import logging
from typing import TYPE_CHECKING, Any

from opentelemetry import context, trace
from opentelemetry.trace import SpanKind, StatusCode

from sideseat.instrumentors.aws._constants import (
    CACHE_READ_TOKENS,
    CACHE_WRITE_TOKENS,
    FINISH_REASONS,
    INPUT_TOKENS,
    MAX_TOKENS,
    OPERATION,
    OUTPUT_TOKENS,
    PROVIDER_NAME,
    REQUEST_MODEL,
    RESPONSE_MODEL,
    SYSTEM,
    SYSTEM_VALUE,
    TEMPERATURE,
    TOOL_DEFINITIONS,
    TOP_P,
    get_tracer,
)
from sideseat.telemetry.encoding import encode_value

if TYPE_CHECKING:
    from opentelemetry.sdk.trace import TracerProvider
    from opentelemetry.trace import Span, Tracer

logger = logging.getLogger("sideseat.instrumentors.aws.bedrock")

_TRACER_NAME = "sideseat.aws.bedrock"


def patch_bedrock_client(client: Any, provider: TracerProvider | None) -> None:
    """Replace converse/invoke methods on a bedrock-runtime client."""
    tracer = get_tracer(provider, _TRACER_NAME)

    for method_name, wrapper_fn in (
        ("converse", _wrap_converse),
        ("converse_stream", _wrap_converse_stream),
        ("invoke_model", _wrap_invoke_model),
        ("invoke_model_with_response_stream", _wrap_invoke_model_stream),
    ):
        original = getattr(client, method_name, None)
        if original is None:
            continue
        setattr(client, method_name, wrapper_fn(original, tracer))

    logger.debug("Patched bedrock-runtime client")


# ---------------------------------------------------------------------------
# Shared: Converse-style event accumulator
# ---------------------------------------------------------------------------


class _ConverseAccumulator:
    """Accumulates Converse-style streaming events into content blocks.

    Handles text, tool use, reasoning, and unknown block types.
    Used by ConverseStream (per-chunk) and Nova InvokeModel streaming
    (post-hoc parsing of accumulated bytes).
    """

    __slots__ = (
        "blocks",
        "stop_reason",
        "usage",
        "_current_block",
        "_current_text",
        "_current_signature",
    )

    def __init__(self) -> None:
        self.blocks: list[dict[str, Any]] = []
        self.stop_reason: str | None = None
        self.usage: dict[str, int] = {}
        self._current_block: dict[str, Any] | None = None
        self._current_text = ""
        self._current_signature = ""

    def process_event(self, event: dict[str, Any]) -> None:
        """Process a single Converse-style streaming event."""
        if "contentBlockStart" in event:
            start = event["contentBlockStart"].get("start", {})
            self._current_block = dict(start)
            self._current_text = ""
            self._current_signature = ""

        elif "contentBlockDelta" in event:
            delta = event["contentBlockDelta"].get("delta", {})
            if self._current_block is None:
                if "reasoningContent" in delta:
                    self._current_block = {"reasoningContent": {}}
                else:
                    self._current_block = {}
                self._current_text = ""
                self._current_signature = ""
            if "text" in delta:
                self._current_text += delta["text"]
            elif "toolUse" in delta:
                self._current_text += delta["toolUse"].get("input", "")
            elif "reasoningContent" in delta:
                rc = delta["reasoningContent"]
                if "text" in rc:
                    self._current_text += rc["text"]
                if "signature" in rc:
                    self._current_signature += rc["signature"]

        elif "contentBlockStop" in event:
            if self._current_block is not None:
                block = self._current_block
                if "toolUse" in block:
                    try:
                        block["toolUse"]["input"] = json.loads(self._current_text)
                    except (json.JSONDecodeError, ValueError):
                        block["toolUse"]["input"] = self._current_text
                elif "text" in block or not block:
                    block = {"text": self._current_text}
                elif "reasoningContent" in block:
                    reasoning_text: dict[str, Any] = {"text": self._current_text}
                    if self._current_signature:
                        reasoning_text["signature"] = self._current_signature
                    block["reasoningContent"] = {"reasoningText": reasoning_text}
                # else: unknown type — preserve start data verbatim

                self.blocks.append(block)
                self._current_block = None
                self._current_text = ""
                self._current_signature = ""

        elif "messageStop" in event:
            self.stop_reason = event["messageStop"].get("stopReason")

        elif "metadata" in event:
            usage = event["metadata"].get("usage", {})
            if usage:
                self.usage = usage


# ---------------------------------------------------------------------------
# Converse (sync)
# ---------------------------------------------------------------------------


def _wrap_converse(original: Any, tracer: Tracer) -> Any:
    @functools.wraps(original)
    def instrumented_converse(**kwargs: Any) -> Any:
        model_id = kwargs.get("modelId", "unknown")
        with tracer.start_as_current_span(f"chat {model_id}", kind=SpanKind.CLIENT) as span:
            _set_request_attrs(span, model_id, kwargs)

            try:
                response = original(**kwargs)
            except Exception as exc:
                span.set_status(StatusCode.ERROR, str(exc))
                span.record_exception(exc)
                raise

            _set_response_attrs(span, model_id, response)
            try:
                _emit_converse_events(span, kwargs, response)
            except Exception:
                logger.debug("Failed to emit converse events", exc_info=True)
            span.set_status(StatusCode.OK)
            return response

    return instrumented_converse


# ---------------------------------------------------------------------------
# ConverseStream
# ---------------------------------------------------------------------------


def _wrap_converse_stream(original: Any, tracer: Tracer) -> Any:
    @functools.wraps(original)
    def instrumented_converse_stream(**kwargs: Any) -> Any:
        model_id = kwargs.get("modelId", "unknown")
        span = tracer.start_span(f"chat {model_id}", kind=SpanKind.CLIENT)
        token = context.attach(trace.set_span_in_context(span))

        _set_request_attrs(span, model_id, kwargs)

        input_msgs = _build_input_messages(kwargs)
        tool_results = _extract_tool_results(kwargs)

        # Emit input early so error paths retain context
        span.add_event(
            "gen_ai.client.inference.operation.details",
            {"gen_ai.input.messages": json.dumps(encode_value(input_msgs))},
        )
        _emit_input_events(span, input_msgs)

        try:
            response = original(**kwargs)
        except Exception as exc:
            span.set_status(StatusCode.ERROR, str(exc))
            span.record_exception(exc)
            span.end()
            context.detach(token)
            raise

        stream = response.get("stream")
        if stream is None:
            span.end()
            context.detach(token)
            return response

        wrapper = _ConverseStreamWrapper(stream, span, token, tool_results)
        response["stream"] = wrapper
        return response

    return instrumented_converse_stream


class _ConverseStreamWrapper:
    """Proxies the EventStream, accumulating content blocks for span events."""

    __slots__ = ("_inner", "_span", "_ctx_token", "_tool_results", "_ended", "_acc")

    def __init__(
        self,
        inner: Any,
        span: Span,
        ctx_token: object,
        tool_results: list[dict[str, Any]],
    ) -> None:
        self._inner = iter(inner)
        self._span = span
        self._ctx_token = ctx_token
        self._tool_results = tool_results
        self._ended = False
        self._acc = _ConverseAccumulator()

    def __iter__(self) -> _ConverseStreamWrapper:
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

        self._acc.process_event(chunk)
        return chunk

    def close(self) -> None:
        if hasattr(self._inner, "close"):
            self._inner.close()
        self._finalize()

    def __enter__(self) -> _ConverseStreamWrapper:
        return self

    def __exit__(self, *args: Any) -> None:
        self.close()

    def __getattr__(self, name: str) -> Any:
        return getattr(self._inner, name)

    def _finalize(self) -> None:
        if self._ended:
            return
        self._ended = True

        span = self._span
        try:
            try:
                if self._acc.usage:
                    _set_usage_attrs(span, self._acc.usage)
                if self._acc.stop_reason:
                    span.set_attribute(FINISH_REASONS, [self._acc.stop_reason])

                output_msg = {"role": "assistant", "content": self._acc.blocks}
                _emit_span_events(
                    span,
                    output_msg,
                    stop_reason=self._acc.stop_reason,
                    tool_results=self._tool_results or None,
                )
            except Exception:
                logger.debug("Failed to emit stream events", exc_info=True)
            span.set_status(StatusCode.OK)
        finally:
            span.end()
            context.detach(self._ctx_token)  # type: ignore[arg-type]

    def _on_error(self, exc: Exception) -> None:
        if self._ended:
            return
        self._ended = True
        self._span.set_status(StatusCode.ERROR, str(exc))
        self._span.record_exception(exc)
        self._span.end()
        context.detach(self._ctx_token)  # type: ignore[arg-type]


# ---------------------------------------------------------------------------
# InvokeModel (sync)
# ---------------------------------------------------------------------------


_CLAUDE_FAMILIES = ("claude", "anthropic")
_NOVA_FAMILIES = ("nova",)


def _detect_model_family(model_id: str) -> str | None:
    """Return family name if we can extract messages, else None."""
    lower = model_id.lower()
    for family in _CLAUDE_FAMILIES:
        if family in lower:
            return "claude"
    for family in _NOVA_FAMILIES:
        if family in lower:
            return "nova"
    return None


def _wrap_invoke_model(original: Any, tracer: Tracer) -> Any:
    @functools.wraps(original)
    def instrumented_invoke_model(**kwargs: Any) -> Any:
        model_id = kwargs.get("modelId", "unknown")
        with tracer.start_as_current_span(f"chat {model_id}", kind=SpanKind.CLIENT) as span:
            span.set_attribute(SYSTEM, SYSTEM_VALUE)
            span.set_attribute(PROVIDER_NAME, SYSTEM_VALUE)
            span.set_attribute(OPERATION, "chat")
            span.set_attribute(REQUEST_MODEL, model_id)

            try:
                response = original(**kwargs)
            except Exception as exc:
                span.set_status(StatusCode.ERROR, str(exc))
                span.record_exception(exc)
                raise

            # Read and rebuffer the streaming body
            body_bytes = response["body"].read()
            from botocore.response import StreamingBody  # type: ignore[import-not-found]

            response["body"] = StreamingBody(io.BytesIO(body_bytes), len(body_bytes))

            try:
                body = json.loads(body_bytes)
            except (json.JSONDecodeError, ValueError):
                span.set_status(StatusCode.OK)
                return response

            try:
                family = _detect_model_family(model_id)

                if family == "nova":
                    span.set_attribute(RESPONSE_MODEL, model_id)
                    usage = body.get("usage", {})
                    if usage:
                        _set_usage_attrs(span, usage)
                    stop = body.get("stopReason")
                    if stop:
                        span.set_attribute(FINISH_REASONS, [stop])
                    _emit_invoke_model_nova_events(span, kwargs, body)
                elif family == "claude":
                    resp_model = body.get("model", model_id)
                    span.set_attribute(RESPONSE_MODEL, resp_model)
                    usage = body.get("usage", {})
                    if usage:
                        _set_invoke_model_usage_attrs(span, usage)
                    stop = body.get("stop_reason")
                    if stop:
                        span.set_attribute(FINISH_REASONS, [stop])
                    _emit_invoke_model_claude_events(span, kwargs, body)
            except Exception:
                logger.debug("Failed to emit invoke_model events", exc_info=True)

            span.set_status(StatusCode.OK)
            return response

    return instrumented_invoke_model


def _emit_invoke_model_claude_events(
    span: Span,
    kwargs: dict[str, Any],
    body: dict[str, Any],
) -> None:
    """Emit message events for Claude InvokeModel responses."""
    req_body = _parse_invoke_model_request(kwargs)
    input_msgs = _build_invoke_model_input_messages(req_body)
    output_content = body.get("content", [])
    output_msg = {"role": body.get("role", "assistant"), "content": output_content}
    tool_results = _extract_tool_results(req_body)
    _emit_span_events(
        span,
        output_msg,
        stop_reason=body.get("stop_reason"),
        tool_results=tool_results or None,
        input_msgs=input_msgs,
    )


def _emit_invoke_model_nova_events(
    span: Span,
    kwargs: dict[str, Any],
    body: dict[str, Any],
) -> None:
    """Emit message events for Nova InvokeModel responses."""
    req_body = _parse_invoke_model_request(kwargs)
    input_msgs = _build_invoke_model_input_messages(req_body)
    output_msg = body.get("output", {}).get("message", {})
    tool_results = _extract_tool_results(req_body)
    _emit_span_events(
        span,
        output_msg,
        stop_reason=body.get("stopReason"),
        tool_results=tool_results or None,
        input_msgs=input_msgs,
    )


# ---------------------------------------------------------------------------
# InvokeModel with response stream
# ---------------------------------------------------------------------------


def _wrap_invoke_model_stream(original: Any, tracer: Tracer) -> Any:
    @functools.wraps(original)
    def instrumented_invoke_model_stream(**kwargs: Any) -> Any:
        model_id = kwargs.get("modelId", "unknown")
        span = tracer.start_span(f"chat {model_id}", kind=SpanKind.CLIENT)
        token = context.attach(trace.set_span_in_context(span))

        span.set_attribute(SYSTEM, SYSTEM_VALUE)
        span.set_attribute(PROVIDER_NAME, SYSTEM_VALUE)
        span.set_attribute(OPERATION, "chat")
        span.set_attribute(REQUEST_MODEL, model_id)

        try:
            response = original(**kwargs)
        except Exception as exc:
            span.set_status(StatusCode.ERROR, str(exc))
            span.record_exception(exc)
            span.end()
            context.detach(token)
            raise

        body = response.get("body")
        if body is None:
            span.end()
            context.detach(token)
            return response

        family = _detect_model_family(model_id)
        wrapper = _InvokeModelStreamWrapper(body, span, token, kwargs, family)
        response["body"] = wrapper
        return response

    return instrumented_invoke_model_stream


class _InvokeModelStreamWrapper:
    """Wraps InvokeModelWithResponseStream body, accumulating streaming events."""

    __slots__ = ("_inner", "_span", "_ctx_token", "_req_body", "_family", "_ended", "_chunks")

    def __init__(
        self,
        inner: Any,
        span: Span,
        ctx_token: object,
        kwargs: dict[str, Any],
        family: str | None,
    ) -> None:
        self._inner = iter(inner)
        self._span = span
        self._ctx_token = ctx_token
        self._req_body = _parse_invoke_model_request(kwargs)
        self._family = family
        self._ended = False
        self._chunks: list[bytes] = []

    def __iter__(self) -> _InvokeModelStreamWrapper:
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

        # Accumulate raw bytes for later parsing
        if isinstance(chunk, dict) and "chunk" in chunk:
            data = chunk["chunk"].get("bytes", b"")
            if data:
                self._chunks.append(data)
        return chunk

    def close(self) -> None:
        if hasattr(self._inner, "close"):
            self._inner.close()
        self._finalize()

    def __enter__(self) -> _InvokeModelStreamWrapper:
        return self

    def __exit__(self, *args: Any) -> None:
        self.close()

    def __getattr__(self, name: str) -> Any:
        return getattr(self._inner, name)

    def _finalize(self) -> None:
        if self._ended:
            return
        self._ended = True

        span = self._span
        try:
            try:
                if self._chunks:
                    if self._family == "claude":
                        self._finalize_claude()
                    elif self._family == "nova":
                        self._finalize_nova()
            except Exception:
                logger.debug("Failed to emit stream events", exc_info=True)
            span.set_status(StatusCode.OK)
        finally:
            span.end()
            context.detach(self._ctx_token)  # type: ignore[arg-type]

    def _finalize_claude(self) -> None:
        """Parse accumulated Claude streaming chunks and emit events."""
        # Claude streaming sends JSON lines — parse each and merge
        content_blocks: list[dict[str, Any]] = []
        usage: dict[str, int] = {}
        stop_reason: str | None = None
        model: str | None = None
        block_texts: dict[int, str] = {}
        block_types: dict[int, dict[str, Any]] = {}
        block_signatures: dict[int, str] = {}

        for raw in self._chunks:
            try:
                event = json.loads(raw)
            except (json.JSONDecodeError, ValueError):
                continue

            event_type = event.get("type", "")

            if event_type == "message_start":
                msg = event.get("message", {})
                model = msg.get("model")
                u = msg.get("usage", {})
                if u:
                    usage.update(u)

            elif event_type == "content_block_start":
                idx = event.get("index", 0)
                block = event.get("content_block", {})
                block_types[idx] = block
                block_texts[idx] = ""

            elif event_type == "content_block_delta":
                idx = event.get("index", 0)
                delta = event.get("delta", {})
                if "text" in delta:
                    block_texts.setdefault(idx, "")
                    block_texts[idx] += delta["text"]
                elif "partial_json" in delta:
                    block_texts.setdefault(idx, "")
                    block_texts[idx] += delta["partial_json"]
                elif "thinking" in delta:
                    block_texts.setdefault(idx, "")
                    block_texts[idx] += delta["thinking"]
                elif "signature" in delta:
                    block_signatures.setdefault(idx, "")
                    block_signatures[idx] += delta["signature"]

            elif event_type == "content_block_stop":
                idx = event.get("index", 0)
                base = block_types.get(idx, {})
                text = block_texts.get(idx, "")
                bt = base.get("type", "text")

                if bt == "tool_use":
                    try:
                        parsed_input = json.loads(text)
                    except (json.JSONDecodeError, ValueError):
                        parsed_input = text
                    content_blocks.append(
                        {
                            "toolUse": {
                                "toolUseId": base.get("id", ""),
                                "name": base.get("name", ""),
                                "input": parsed_input,
                            }
                        }
                    )
                elif bt == "thinking":
                    reasoning_text: dict[str, Any] = {"text": text}
                    sig = block_signatures.get(idx) or base.get("signature")
                    if sig:
                        reasoning_text["signature"] = sig
                    content_blocks.append({"reasoningContent": {"reasoningText": reasoning_text}})
                else:
                    content_blocks.append({"text": text})

            elif event_type == "message_delta":
                delta = event.get("delta", {})
                stop_reason = delta.get("stop_reason", stop_reason)
                u = event.get("usage", {})
                if u:
                    usage.update(u)

            elif event_type == "message_stop":
                # Bedrock wraps Claude streaming with invocation metrics
                metrics = event.get("amazon-bedrock-invocationMetrics", {})
                if metrics:
                    if "inputTokenCount" in metrics:
                        usage["input_tokens"] = metrics["inputTokenCount"]
                    if "outputTokenCount" in metrics:
                        usage["output_tokens"] = metrics["outputTokenCount"]

        span = self._span

        if model:
            span.set_attribute(RESPONSE_MODEL, model)

        if usage:
            _set_invoke_model_usage_attrs(span, usage)

        if stop_reason:
            span.set_attribute(FINISH_REASONS, [stop_reason])

        req_body = self._req_body
        input_msgs = _build_invoke_model_input_messages(req_body)
        output_msg = {"role": "assistant", "content": content_blocks}
        tool_results = _extract_tool_results(req_body)

        _emit_span_events(
            span,
            output_msg,
            stop_reason=stop_reason,
            tool_results=tool_results or None,
            input_msgs=input_msgs,
        )

    def _finalize_nova(self) -> None:
        """Parse accumulated Nova streaming chunks and emit events.

        Nova InvokeModel streaming uses Converse-style events (contentBlockStart,
        contentBlockDelta, contentBlockStop). Falls back to full-response format
        for single-chunk responses. Delegates to _ConverseAccumulator for full
        block type support (text, tool use, reasoning, unknown).
        """
        acc = _ConverseAccumulator()

        for raw in self._chunks:
            try:
                event = json.loads(raw)
            except (json.JSONDecodeError, ValueError):
                continue

            # Full-response format (single chunk)
            if "output" in event:
                msg = event.get("output", {}).get("message", {})
                acc.blocks = msg.get("content", [])
                acc.usage = event.get("usage", {})
                acc.stop_reason = event.get("stopReason")
            else:
                acc.process_event(event)

        span = self._span

        if acc.usage:
            _set_usage_attrs(span, acc.usage)
        if acc.stop_reason:
            span.set_attribute(FINISH_REASONS, [acc.stop_reason])

        req_body = self._req_body
        input_msgs = _build_invoke_model_input_messages(req_body)
        output_msg = {"role": "assistant", "content": acc.blocks}
        tool_results = _extract_tool_results(req_body)

        _emit_span_events(
            span,
            output_msg,
            stop_reason=acc.stop_reason,
            tool_results=tool_results or None,
            input_msgs=input_msgs,
        )

    def _on_error(self, exc: Exception) -> None:
        if self._ended:
            return
        self._ended = True
        self._span.set_status(StatusCode.ERROR, str(exc))
        self._span.record_exception(exc)
        self._span.end()
        context.detach(self._ctx_token)  # type: ignore[arg-type]


# ---------------------------------------------------------------------------
# Shared helpers
# ---------------------------------------------------------------------------


def _set_request_attrs(span: Span, model_id: str, kwargs: dict[str, Any]) -> None:
    """Set common request attributes on a Converse span."""
    span.set_attribute(SYSTEM, SYSTEM_VALUE)
    span.set_attribute(PROVIDER_NAME, SYSTEM_VALUE)
    span.set_attribute(OPERATION, "chat")
    span.set_attribute(REQUEST_MODEL, model_id)

    # Tool definitions
    tool_config = kwargs.get("toolConfig")
    if tool_config:
        tools = tool_config.get("tools", [])
        defs: list[Any] = list(tools)
        tool_choice = tool_config.get("toolChoice")
        if tool_choice:
            defs.append({"toolChoice": tool_choice})
        span.set_attribute(TOOL_DEFINITIONS, json.dumps(encode_value(defs)))

    # Inference config
    inf = kwargs.get("inferenceConfig")
    if inf:
        if "temperature" in inf:
            span.set_attribute(TEMPERATURE, inf["temperature"])
        if "topP" in inf:
            span.set_attribute(TOP_P, inf["topP"])
        if "maxTokens" in inf:
            span.set_attribute(MAX_TOKENS, inf["maxTokens"])


def _set_response_attrs(span: Span, model_id: str, response: dict[str, Any]) -> None:
    """Set common response attributes from a Converse response."""
    span.set_attribute(RESPONSE_MODEL, model_id)

    usage = response.get("usage", {})
    if usage:
        _set_usage_attrs(span, usage)

    stop = response.get("stopReason")
    if stop:
        span.set_attribute(FINISH_REASONS, [stop])


def _set_usage_attrs(span: Span, usage: dict[str, Any]) -> None:
    """Set token usage attributes."""
    if "inputTokens" in usage:
        span.set_attribute(INPUT_TOKENS, usage["inputTokens"])
    if "outputTokens" in usage:
        span.set_attribute(OUTPUT_TOKENS, usage["outputTokens"])
    if "cacheReadInputTokenCount" in usage:
        span.set_attribute(CACHE_READ_TOKENS, usage["cacheReadInputTokenCount"])
    if "cacheWriteInputTokenCount" in usage:
        span.set_attribute(CACHE_WRITE_TOKENS, usage["cacheWriteInputTokenCount"])


_BINARY_BLOCK_KEYS = frozenset({"image", "document", "video", "audio"})


def _strip_binary_blocks(content: Any) -> Any:
    """Remove binary content blocks (images, documents) for lightweight events.

    Handles both Bedrock Converse format (key-based: {"image": {...}}) and
    Claude Messages API format (type-based: {"type": "image", ...}).
    """
    if not isinstance(content, list):
        return content
    return [
        b
        for b in content
        if not isinstance(b, dict)
        or not (any(k in _BINARY_BLOCK_KEYS for k in b) or b.get("type") in _BINARY_BLOCK_KEYS)
    ]


def _emit_input_events(span: Span, input_msgs: list[dict[str, Any]]) -> None:
    """Emit per-role gen_ai events for server input preview extraction.

    Only emits the system message and the last user message to avoid O(n^2)
    event growth in multi-turn conversations.  Binary content (images,
    documents) is stripped since the full data is already preserved in the
    gen_ai.client.inference.operation.details event.
    """
    # System message (always first if present)
    if input_msgs and input_msgs[0].get("role") == "system":
        content = _strip_binary_blocks(input_msgs[0].get("content", []))
        span.add_event("gen_ai.system.message", {"content": json.dumps(encode_value(content))})

    # Last user message for input preview
    for msg in reversed(input_msgs):
        if msg.get("role") == "user":
            content = _strip_binary_blocks(msg.get("content", []))
            span.add_event("gen_ai.user.message", {"content": json.dumps(encode_value(content))})
            break


def _build_input_messages(kwargs: dict[str, Any]) -> list[dict[str, Any]]:
    """Build input message list from Converse kwargs."""
    input_msgs: list[dict[str, Any]] = []
    system = kwargs.get("system")
    if system:
        input_msgs.append({"role": "system", "content": system})
    input_msgs.extend(kwargs.get("messages", []))
    return input_msgs


def _extract_tool_results(kwargs: dict[str, Any]) -> list[dict[str, Any]]:
    """Extract tool result blocks from input messages.

    Handles both Converse format ({"toolResult": {...}}) and
    Claude Messages API format ({"type": "tool_result", ...}).
    """
    results: list[dict[str, Any]] = []
    for msg in kwargs.get("messages", []):
        content = msg.get("content", [])
        if not isinstance(content, list):
            continue
        for block in content:
            if not isinstance(block, dict):
                continue
            if "toolResult" in block or block.get("type") == "tool_result":
                results.append(block)
    return results


def _parse_invoke_model_request(kwargs: dict[str, Any]) -> dict[str, Any]:
    """Parse JSON request body from InvokeModel kwargs."""
    try:
        body = kwargs.get("body")
        if isinstance(body, (str, bytes)):
            result: dict[str, Any] = json.loads(body)
            return result
        return body if isinstance(body, dict) else {}
    except (json.JSONDecodeError, ValueError):
        return {}


def _build_invoke_model_input_messages(
    req_body: dict[str, Any],
) -> list[dict[str, Any]]:
    """Build input messages from an InvokeModel request body.

    Handles both Claude Messages API (system can be string) and
    Nova format (system is always array of text blocks).
    """
    input_msgs: list[dict[str, Any]] = []
    if "system" in req_body:
        system_content = req_body["system"]
        if isinstance(system_content, str):
            input_msgs.append({"role": "system", "content": [{"text": system_content}]})
        else:
            input_msgs.append({"role": "system", "content": system_content})
    input_msgs.extend(req_body.get("messages", []))
    return input_msgs


def _set_invoke_model_usage_attrs(span: Span, usage: dict[str, Any]) -> None:
    """Set token usage from Claude InvokeModel response (snake_case keys)."""
    if "input_tokens" in usage:
        span.set_attribute(INPUT_TOKENS, usage["input_tokens"])
    if "output_tokens" in usage:
        span.set_attribute(OUTPUT_TOKENS, usage["output_tokens"])
    if "cache_read_input_tokens" in usage:
        span.set_attribute(CACHE_READ_TOKENS, usage["cache_read_input_tokens"])
    if "cache_creation_input_tokens" in usage:
        span.set_attribute(CACHE_WRITE_TOKENS, usage["cache_creation_input_tokens"])


def _emit_span_events(
    span: Span,
    output_msg: dict[str, Any],
    stop_reason: str | None = None,
    tool_results: list[dict[str, Any]] | None = None,
    input_msgs: list[dict[str, Any]] | None = None,
) -> None:
    """Emit standard gen_ai telemetry events on a span.

    When input_msgs is provided, emits a combined details event with both
    input and output plus per-role input preview events. When omitted,
    emits an output-only details event (for stream finalize where input
    was already emitted at stream start).
    """
    if input_msgs is not None:
        span.add_event(
            "gen_ai.client.inference.operation.details",
            {
                "gen_ai.input.messages": json.dumps(encode_value(input_msgs)),
                "gen_ai.output.messages": json.dumps(encode_value([output_msg])),
            },
        )
        _emit_input_events(span, input_msgs)
    else:
        span.add_event(
            "gen_ai.client.inference.operation.details",
            {"gen_ai.output.messages": json.dumps(encode_value([output_msg]))},
        )

    output_content = output_msg.get("content", [])
    choice_attrs: dict[str, Any] = {
        "message": json.dumps(encode_value(output_content)),
    }
    if stop_reason:
        choice_attrs["finish_reason"] = stop_reason
    if tool_results:
        choice_attrs["tool.result"] = json.dumps(encode_value(tool_results))
    span.add_event("gen_ai.choice", choice_attrs)


def _emit_converse_events(span: Span, kwargs: dict[str, Any], response: dict[str, Any]) -> None:
    """Emit gen_ai events for a Converse response."""
    input_msgs = _build_input_messages(kwargs)
    output_msg = response.get("output", {}).get("message", {})
    tool_results = _extract_tool_results(kwargs)
    _emit_span_events(
        span,
        output_msg,
        stop_reason=response.get("stopReason"),
        tool_results=tool_results or None,
        input_msgs=input_msgs,
    )
