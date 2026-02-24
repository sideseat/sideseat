"""Bedrock Runtime instrumentation — Converse, ConverseStream, InvokeModel."""

from __future__ import annotations

import functools
import io
import json
import logging
from typing import TYPE_CHECKING, Any

from opentelemetry import context, trace
from opentelemetry.trace import SpanKind, StatusCode

from sideseat.telemetry.encoding import encode_value

if TYPE_CHECKING:
    from opentelemetry.sdk.trace import TracerProvider
    from opentelemetry.trace import Span, Tracer

logger = logging.getLogger("sideseat.instrumentors.aws.bedrock")

_TRACER_NAME = "sideseat.aws.bedrock"

# Gen AI semantic convention attribute keys
_SYSTEM = "gen_ai.system"
_PROVIDER_NAME = "gen_ai.provider.name"
_OPERATION = "gen_ai.operation.name"
_REQUEST_MODEL = "gen_ai.request.model"
_RESPONSE_MODEL = "gen_ai.response.model"
_INPUT_TOKENS = "gen_ai.usage.input_tokens"
_OUTPUT_TOKENS = "gen_ai.usage.output_tokens"
_CACHE_READ_TOKENS = "gen_ai.usage.cache_read_input_tokens"
_CACHE_WRITE_TOKENS = "gen_ai.usage.cache_write_input_tokens"
_FINISH_REASONS = "gen_ai.response.finish_reasons"
_TOOL_DEFINITIONS = "gen_ai.tool.definitions"
_TEMPERATURE = "gen_ai.request.temperature"
_TOP_P = "gen_ai.request.top_p"
_MAX_TOKENS = "gen_ai.request.max_tokens"

_SYSTEM_VALUE = "aws_bedrock"


def patch_bedrock_client(client: Any, provider: TracerProvider | None) -> None:
    """Replace converse/invoke methods on a bedrock-runtime client."""
    tracer = _get_tracer(provider)

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


def _get_tracer(provider: TracerProvider | None) -> Tracer:
    if provider is not None:
        return provider.get_tracer(_TRACER_NAME)
    return trace.get_tracer(_TRACER_NAME)


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
            _emit_converse_events(span, kwargs, response)
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

        # Accumulation state
        self._blocks: list[dict[str, Any]] = []
        self._current_block: dict[str, Any] | None = None
        self._current_text = ""
        self._current_signature = ""
        self._stop_reason: str | None = None
        self._usage: dict[str, int] = {}

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

        self._process_chunk(chunk)
        return chunk

    def close(self) -> None:
        if hasattr(self._inner, "close"):
            self._inner.close()
        self._finalize()

    def __enter__(self) -> _ConverseStreamWrapper:
        return self

    def __exit__(self, *args: Any) -> None:
        self.close()

    # -- Proxy attributes to inner stream --
    def __getattr__(self, name: str) -> Any:
        return getattr(self._inner, name)

    def _process_chunk(self, chunk: dict[str, Any]) -> None:
        if "contentBlockStart" in chunk:
            start = chunk["contentBlockStart"].get("start", {})
            self._current_block = dict(start)
            self._current_text = ""
            self._current_signature = ""

        elif "contentBlockDelta" in chunk:
            delta = chunk["contentBlockDelta"].get("delta", {})
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

        elif "contentBlockStop" in chunk:
            if self._current_block is not None:
                block = self._current_block
                if "toolUse" in block:
                    # Parse accumulated JSON for tool input
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
                # else: unknown type (image, document, citations, guard, etc.)
                # Preserve start data verbatim for server-side normalization

                self._blocks.append(block)
                self._current_block = None
                self._current_text = ""
                self._current_signature = ""

        elif "messageStop" in chunk:
            self._stop_reason = chunk["messageStop"].get("stopReason")

        elif "metadata" in chunk:
            usage = chunk["metadata"].get("usage", {})
            if usage:
                self._usage = usage

    def _finalize(self) -> None:
        if self._ended:
            return
        self._ended = True

        span = self._span
        try:
            # Token usage
            if self._usage:
                _set_usage_attrs(span, self._usage)

            # Finish reason
            if self._stop_reason:
                span.set_attribute(_FINISH_REASONS, [self._stop_reason])

            # Output message
            output_content = self._blocks
            output_msg = {"role": "assistant", "content": output_content}

            # Output details (input already emitted at stream start)
            span.add_event(
                "gen_ai.client.inference.operation.details",
                {"gen_ai.output.messages": json.dumps(encode_value([output_msg]))},
            )

            # Choice event
            choice_attrs: dict[str, Any] = {
                "message": json.dumps(encode_value(output_content)),
            }
            if self._stop_reason:
                choice_attrs["finish_reason"] = self._stop_reason

            if self._tool_results:
                choice_attrs["tool.result"] = json.dumps(encode_value(self._tool_results))

            span.add_event("gen_ai.choice", choice_attrs)

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


# ---------------------------------------------------------------------------
# InvokeModel (sync)
# ---------------------------------------------------------------------------


_CLAUDE_FAMILIES = ("claude", "anthropic")


def _detect_model_family(model_id: str) -> str | None:
    """Return family name if we can extract messages, else None."""
    lower = model_id.lower()
    for family in _CLAUDE_FAMILIES:
        if family in lower:
            return "claude"
    return None


def _wrap_invoke_model(original: Any, tracer: Tracer) -> Any:
    @functools.wraps(original)
    def instrumented_invoke_model(**kwargs: Any) -> Any:
        model_id = kwargs.get("modelId", "unknown")
        with tracer.start_as_current_span(f"chat {model_id}", kind=SpanKind.CLIENT) as span:
            span.set_attribute(_SYSTEM, _SYSTEM_VALUE)
            span.set_attribute(_PROVIDER_NAME, _SYSTEM_VALUE)
            span.set_attribute(_OPERATION, "chat")
            span.set_attribute(_REQUEST_MODEL, model_id)

            try:
                response = original(**kwargs)
            except Exception as exc:
                span.set_status(StatusCode.ERROR, str(exc))
                span.record_exception(exc)
                raise

            # Read and rebuffer the streaming body
            body_bytes = response["body"].read()
            from botocore.response import StreamingBody

            response["body"] = StreamingBody(io.BytesIO(body_bytes), len(body_bytes))

            try:
                body = json.loads(body_bytes)
            except (json.JSONDecodeError, ValueError):
                return response

            # Response model (may differ from request)
            resp_model = body.get("model", model_id)
            span.set_attribute(_RESPONSE_MODEL, resp_model)

            # Usage (Claude Messages API uses snake_case keys)
            usage = body.get("usage", {})
            if usage:
                _set_invoke_model_usage_attrs(span, usage)

            # Stop reason
            stop = body.get("stop_reason")
            if stop:
                span.set_attribute(_FINISH_REASONS, [stop])

            # Content extraction (Claude only for now)
            family = _detect_model_family(model_id)
            if family == "claude":
                _emit_invoke_model_claude_events(span, kwargs, body)

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

    span.add_event(
        "gen_ai.client.inference.operation.details",
        {
            "gen_ai.input.messages": json.dumps(encode_value(input_msgs)),
            "gen_ai.output.messages": json.dumps(encode_value([output_msg])),
        },
    )
    _emit_input_events(span, input_msgs)

    choice_attrs: dict[str, Any] = {
        "message": json.dumps(encode_value(output_content)),
    }
    stop = body.get("stop_reason")
    if stop:
        choice_attrs["finish_reason"] = stop

    tool_results = _extract_tool_results(req_body)
    if tool_results:
        choice_attrs["tool.result"] = json.dumps(encode_value(tool_results))

    span.add_event("gen_ai.choice", choice_attrs)


# ---------------------------------------------------------------------------
# InvokeModel with response stream
# ---------------------------------------------------------------------------


def _wrap_invoke_model_stream(original: Any, tracer: Tracer) -> Any:
    @functools.wraps(original)
    def instrumented_invoke_model_stream(**kwargs: Any) -> Any:
        model_id = kwargs.get("modelId", "unknown")
        span = tracer.start_span(f"chat {model_id}", kind=SpanKind.CLIENT)
        token = context.attach(trace.set_span_in_context(span))

        span.set_attribute(_SYSTEM, _SYSTEM_VALUE)
        span.set_attribute(_PROVIDER_NAME, _SYSTEM_VALUE)
        span.set_attribute(_OPERATION, "chat")
        span.set_attribute(_REQUEST_MODEL, model_id)

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
    """Wraps InvokeModelWithResponseStream body, accumulating Claude streaming events."""

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
        self._kwargs = kwargs
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
            if self._family == "claude" and self._chunks:
                self._finalize_claude()
            span.set_status(StatusCode.OK)
        finally:
            span.end()
            context.detach(self._ctx_token)

    def _finalize_claude(self) -> None:
        """Parse accumulated Claude streaming chunks and emit events."""
        # Claude streaming sends JSON lines — parse each and merge
        content_blocks: list[dict[str, Any]] = []
        usage: dict[str, int] = {}
        stop_reason: str | None = None
        model: str | None = None
        block_texts: dict[int, str] = {}
        block_types: dict[int, dict[str, Any]] = {}

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
                    sig = base.get("signature")
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
            span.set_attribute(_RESPONSE_MODEL, model)

        if usage:
            _set_invoke_model_usage_attrs(span, usage)

        if stop_reason:
            span.set_attribute(_FINISH_REASONS, [stop_reason])

        # Emit events
        req_body = _parse_invoke_model_request(self._kwargs)
        input_msgs = _build_invoke_model_input_messages(req_body)
        output_msg = {"role": "assistant", "content": content_blocks}

        span.add_event(
            "gen_ai.client.inference.operation.details",
            {
                "gen_ai.input.messages": json.dumps(encode_value(input_msgs)),
                "gen_ai.output.messages": json.dumps(encode_value([output_msg])),
            },
        )
        _emit_input_events(span, input_msgs)

        choice_attrs: dict[str, Any] = {
            "message": json.dumps(encode_value(content_blocks)),
        }
        if stop_reason:
            choice_attrs["finish_reason"] = stop_reason

        tool_results = _extract_tool_results(req_body)
        if tool_results:
            choice_attrs["tool.result"] = json.dumps(encode_value(tool_results))

        span.add_event("gen_ai.choice", choice_attrs)

    def _on_error(self, exc: Exception) -> None:
        if self._ended:
            return
        self._ended = True
        self._span.set_status(StatusCode.ERROR, str(exc))
        self._span.record_exception(exc)
        self._span.end()
        context.detach(self._ctx_token)


# ---------------------------------------------------------------------------
# Shared helpers
# ---------------------------------------------------------------------------


def _set_request_attrs(span: Span, model_id: str, kwargs: dict[str, Any]) -> None:
    """Set common request attributes on a Converse span."""
    span.set_attribute(_SYSTEM, _SYSTEM_VALUE)
    span.set_attribute(_PROVIDER_NAME, _SYSTEM_VALUE)
    span.set_attribute(_OPERATION, "chat")
    span.set_attribute(_REQUEST_MODEL, model_id)

    # Tool definitions
    tool_config = kwargs.get("toolConfig")
    if tool_config:
        tools = tool_config.get("tools", [])
        defs: list[Any] = list(tools)
        tool_choice = tool_config.get("toolChoice")
        if tool_choice:
            defs.append({"toolChoice": tool_choice})
        span.set_attribute(_TOOL_DEFINITIONS, json.dumps(encode_value(defs)))

    # Inference config
    inf = kwargs.get("inferenceConfig")
    if inf:
        if "temperature" in inf:
            span.set_attribute(_TEMPERATURE, inf["temperature"])
        if "topP" in inf:
            span.set_attribute(_TOP_P, inf["topP"])
        if "maxTokens" in inf:
            span.set_attribute(_MAX_TOKENS, inf["maxTokens"])


def _set_response_attrs(span: Span, model_id: str, response: dict[str, Any]) -> None:
    """Set common response attributes from a Converse response."""
    span.set_attribute(_RESPONSE_MODEL, model_id)

    usage = response.get("usage", {})
    if usage:
        _set_usage_attrs(span, usage)

    stop = response.get("stopReason")
    if stop:
        span.set_attribute(_FINISH_REASONS, [stop])


def _set_usage_attrs(span: Span, usage: dict[str, Any]) -> None:
    """Set token usage attributes."""
    if "inputTokens" in usage:
        span.set_attribute(_INPUT_TOKENS, usage["inputTokens"])
    if "outputTokens" in usage:
        span.set_attribute(_OUTPUT_TOKENS, usage["outputTokens"])
    if "cacheReadInputTokenCount" in usage:
        span.set_attribute(_CACHE_READ_TOKENS, usage["cacheReadInputTokenCount"])
    if "cacheWriteInputTokenCount" in usage:
        span.set_attribute(_CACHE_WRITE_TOKENS, usage["cacheWriteInputTokenCount"])


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
        or not (set(b.keys()) & _BINARY_BLOCK_KEYS or b.get("type") in _BINARY_BLOCK_KEYS)
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
    """Extract toolResult blocks from input messages."""
    results: list[dict[str, Any]] = []
    for msg in kwargs.get("messages", []):
        for block in msg.get("content", []):
            if isinstance(block, dict) and "toolResult" in block:
                results.append(block)
    return results


def _parse_invoke_model_request(kwargs: dict[str, Any]) -> dict[str, Any]:
    """Parse JSON request body from InvokeModel kwargs."""
    try:
        body = kwargs.get("body")
        if isinstance(body, (str, bytes)):
            return json.loads(body)
        return body if isinstance(body, dict) else {}
    except (json.JSONDecodeError, ValueError):
        return {}


def _build_invoke_model_input_messages(
    req_body: dict[str, Any],
) -> list[dict[str, Any]]:
    """Build input messages from a Claude Messages API request body."""
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
    """Set token usage from InvokeModel response (snake_case keys)."""
    if "input_tokens" in usage:
        span.set_attribute(_INPUT_TOKENS, usage["input_tokens"])
    if "output_tokens" in usage:
        span.set_attribute(_OUTPUT_TOKENS, usage["output_tokens"])
    if "cache_read_input_tokens" in usage:
        span.set_attribute(_CACHE_READ_TOKENS, usage["cache_read_input_tokens"])
    if "cache_creation_input_tokens" in usage:
        span.set_attribute(_CACHE_WRITE_TOKENS, usage["cache_creation_input_tokens"])


def _emit_converse_events(span: Span, kwargs: dict[str, Any], response: dict[str, Any]) -> None:
    """Emit gen_ai events for a Converse response."""
    input_msgs = _build_input_messages(kwargs)
    output_msg = response.get("output", {}).get("message", {})

    span.add_event(
        "gen_ai.client.inference.operation.details",
        {
            "gen_ai.input.messages": json.dumps(encode_value(input_msgs)),
            "gen_ai.output.messages": json.dumps(encode_value([output_msg])),
        },
    )
    _emit_input_events(span, input_msgs)

    output_content = output_msg.get("content", [])
    choice_attrs: dict[str, Any] = {
        "message": json.dumps(encode_value(output_content)),
    }
    stop = response.get("stopReason")
    if stop:
        choice_attrs["finish_reason"] = stop

    tool_results = _extract_tool_results(kwargs)
    if tool_results:
        choice_attrs["tool.result"] = json.dumps(encode_value(tool_results))

    span.add_event("gen_ai.choice", choice_attrs)
