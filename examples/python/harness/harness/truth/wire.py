"""Decode a recorded model interaction into a provider-neutral model call.

A cassette (:mod:`harness.proxy`) records, for every request a scenario made, the response status,
the payload-describing headers and the response body - not the request body, of which only a digest
is kept. Each response is decoded here into what the model said: its text, reasoning and tool calls
in emitted order, the stop reason, the token usage and the model, so a reconstruction of the
conversation can be checked against the wire rather than against an earlier reconstruction.

Every format is decoded strictly. A content block, event or item this module does not know is an
error: silently skipping it would make the oracle blind to exactly the content a parser might also
drop.
"""

from __future__ import annotations

import base64
import hashlib
import json
import re
from collections.abc import Iterable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any
from urllib.parse import unquote, urlsplit

from harness.truth.framing import eventstream_frames, server_sent_events


class DecodeError(ValueError):
    pass


@dataclass
class Usage:
    """Token counts as the provider reports them.

    ``input`` keeps the provider's meaning: Anthropic and Bedrock count cached prompt tokens
    separately, OpenAI and Gemini include them in ``input``. ``input_includes_cache`` says which.
    """

    input: int | None = None
    output: int | None = None
    cache_read: int | None = None
    cache_write: int | None = None
    reasoning: int | None = None
    input_includes_cache: bool = False

    def to_json(self) -> dict[str, Any]:
        return {
            "input": self.input,
            "output": self.output,
            "cache_read": self.cache_read,
            "cache_write": self.cache_write,
            "reasoning": self.reasoning,
            "input_includes_cache": self.input_includes_cache,
        }


@dataclass
class ModelCall:
    """One model response, in provider-neutral terms."""

    #: ``bedrock.converse``, ``bedrock.converse_stream``, ``anthropic.messages``, ``openai.chat``,
    #: ``openai.responses`` or ``gemini.generate_content``.
    api: str
    status: int
    streamed: bool = False
    #: The model the request addressed (Bedrock names it in the path), else the response's.
    model: str | None = None
    #: The model the response names, when it names one; Bedrock's Anthropic route answers with the
    #: base model name rather than the inference profile the request addressed.
    response_model: str | None = None
    response_id: str | None = None
    stop_reason: str | None = None
    #: ``stop``, ``tool_use`` or ``max_tokens``; any other provider reason is kept as reported.
    finish: str | None = None
    usage: Usage | None = None
    #: ``text``, ``reasoning`` and ``tool_call`` parts in the order the model emitted them.
    parts: list[dict[str, Any]] = field(default_factory=list)
    #: The system prompt, when the response echoes the request's (OpenAI Responses ``instructions``).
    system_echo: str | None = None
    error: str | None = None

    @property
    def ok(self) -> bool:
        return self.error is None and 200 <= self.status < 300

    @property
    def tool_calls(self) -> list[dict[str, Any]]:
        return [part for part in self.parts if part["type"] == "tool_call"]

    def to_json(self) -> dict[str, Any]:
        return {
            "api": self.api,
            "status": self.status,
            "streamed": self.streamed,
            "model": self.model,
            "response_model": self.response_model,
            "response_id": self.response_id,
            "stop_reason": self.stop_reason,
            "finish": self.finish,
            "usage": self.usage.to_json() if self.usage else None,
            "parts": self.parts,
            "error": self.error,
        }


def text_part(text: str) -> dict[str, Any]:
    return {"type": "text", "text": text}


def reasoning_part(
    text: str | None,
    *,
    signed: bool = False,
    redacted: bool = False,
    signature: str | None = None,
) -> dict[str, Any]:
    """Reasoning the model emitted. ``text`` is ``None`` when only an opaque form was returned.

    A ``signature`` makes the part signed and gives it a ``seal``, the SHA-256 of the signature: the
    identity the rubric searches the telemetry for, so one withheld turn is told from another although
    both have no text. The signature itself never reaches a truth.
    """
    part: dict[str, Any] = {
        "type": "reasoning",
        "text": text,
        "signed": signed or bool(signature),
        "redacted": redacted,
    }
    if signature:
        part["seal"] = hashlib.sha256(signature.encode()).hexdigest()
    return part


def tool_call_part(call_id: str | None, name: str, arguments: Any) -> dict[str, Any]:
    return {
        "type": "tool_call",
        "id": call_id,
        "name": name,
        "arguments": _arguments(arguments),
    }


def server_call_part(
    call_id: str | None, name: str, arguments: Any, result: Any
) -> dict[str, Any]:
    """A tool the provider ran itself while answering, with what the run produced.

    Unlike a tool call, it ends nothing: the provider ran it inside this response, which goes on to
    answer, so the call and its result are both the response's own output.
    """
    return {
        "type": "server_tool_call",
        "id": call_id,
        "name": name,
        "arguments": arguments,
        "result": result,
    }


def _arguments(value: Any) -> Any:
    """Tool arguments as JSON: a streamed or string-encoded argument object is parsed."""
    if isinstance(value, str):
        if not value.strip():
            return {}
        try:
            return json.loads(value)
        except json.JSONDecodeError as error:
            raise DecodeError(
                f"tool arguments are not JSON: {value[:80]!r}: {error}"
            ) from None
    return value


_BEDROCK = re.compile(
    r"/model/(?P<model>[^/]+)/(?P<op>converse|converse-stream|invoke|invoke-with-response-stream)$"
)
_GEMINI = re.compile(
    r"/models/(?P<model>[^/:]+):(?P<op>generateContent|streamGenerateContent)$"
)


def decode(item: dict[str, Any]) -> ModelCall | None:
    """The model call a cassette interaction recorded, or ``None`` for a request to no model."""
    path = urlsplit(item["path"]).path
    body = (
        base64.b64decode(item["body"])
        if isinstance(item["body"], str)
        else item["body"]
    )
    content_type = (item.get("headers") or {}).get("content-type", "")
    status = int(item.get("status", 200))
    if bedrock := _BEDROCK.search(path):
        model, operation = unquote(bedrock["model"]), bedrock["op"]
        api = {
            "converse": "bedrock.converse",
            "converse-stream": "bedrock.converse_stream",
        }.get(operation, "anthropic.messages")
        if status >= 300:
            return _failed(api, status, body, model)
        if operation == "converse":
            call = converse(json.loads(body))
        elif operation == "converse-stream":
            call = converse_stream(eventstream_frames(body))
        elif operation == "invoke":
            call = anthropic_message(json.loads(body))
        else:
            call = anthropic_stream(_bedrock_chunks(eventstream_frames(body)))
        call.model = model
        return call
    streamed = "event-stream" in content_type
    if path.endswith("/chat/completions"):
        if status >= 300:
            return _failed("openai.chat", status, body, None)
        return chat_stream(body) if streamed else chat_completion(json.loads(body))
    if path.endswith("/responses"):
        if status >= 300:
            return _failed("openai.responses", status, body, None)
        return responses_stream(body) if streamed else responses(json.loads(body))
    if path.endswith("/v1/messages"):
        if status >= 300:
            return _failed("anthropic.messages", status, body, None)
        if streamed:
            return anthropic_stream(
                json.loads(event.data) for event in server_sent_events(body)
            )
        return anthropic_message(json.loads(body))
    if gemini := _GEMINI.search(path):
        if status >= 300:
            return _failed("gemini.generate_content", status, body, gemini["model"])
        chunks = (
            [json.loads(e.data) for e in server_sent_events(body)]
            if streamed or gemini["op"] == "streamGenerateContent"
            else [json.loads(body)]
        )
        call = gemini_chunks(chunks, streamed=len(chunks) > 1 or streamed)
        call.model = call.model or gemini["model"]
        return call
    return None


def _failed(api: str, status: int, body: bytes, model: str | None) -> ModelCall:
    try:
        value = json.loads(body)
        message = (
            (value.get("error") or {}).get("message")
            if isinstance(value.get("error"), dict)
            else value.get("message")
        ) or json.dumps(value)
    except (json.JSONDecodeError, AttributeError):
        message = body.decode("utf-8", "replace")
    return ModelCall(api=api, status=status, model=model, error=str(message))


# --- Bedrock Converse ---------------------------------------------------------------------------

_CONVERSE_FINISH = {
    "end_turn": "stop",
    "stop_sequence": "stop",
    "tool_use": "tool_use",
    "max_tokens": "max_tokens",
}


def _converse_usage(usage: dict[str, Any] | None) -> Usage | None:
    if not usage:
        return None
    return Usage(
        input=usage.get("inputTokens"),
        output=usage.get("outputTokens"),
        cache_read=usage.get(
            "cacheReadInputTokens", usage.get("cacheReadInputTokenCount")
        ),
        cache_write=usage.get(
            "cacheWriteInputTokens", usage.get("cacheWriteInputTokenCount")
        ),
    )


def converse(value: dict[str, Any]) -> ModelCall:
    parts = []
    for block in value["output"]["message"]["content"]:
        if "text" in block:
            parts.append(text_part(block["text"]))
        elif "toolUse" in block:
            use = block["toolUse"]
            parts.append(
                tool_call_part(use["toolUseId"], use["name"], use.get("input"))
            )
        elif "reasoningContent" in block:
            reasoning = block["reasoningContent"]
            if "reasoningText" in reasoning:
                text = reasoning["reasoningText"]
                parts.append(
                    reasoning_part(text.get("text"), signature=text.get("signature"))
                )
            elif "redactedContent" in reasoning:
                parts.append(reasoning_part(None, redacted=True))
            else:
                raise DecodeError(
                    f"unknown Converse reasoning block: {list(reasoning)}"
                )
        else:
            raise DecodeError(f"unknown Converse content block: {list(block)}")
    stop = value.get("stopReason")
    return ModelCall(
        api="bedrock.converse",
        status=200,
        stop_reason=stop,
        finish=_CONVERSE_FINISH.get(stop, stop),
        usage=_converse_usage(value.get("usage")),
        parts=parts,
    )


@dataclass
class _Block:
    """A content block being reassembled from stream deltas."""

    kind: str
    text: str = ""
    call_id: str | None = None
    name: str = ""
    #: The reasoning's signature, as the stream delivered it.
    signature: str = ""
    redacted: bool = False
    #: The block's initial ``input`` object, used when no argument delta follows it.
    initial: Any = None

    def part(self) -> dict[str, Any]:
        if self.kind == "text":
            return text_part(self.text)
        if self.kind == "reasoning":
            text = self.text if self.text or not self.redacted else None
            return reasoning_part(
                text, redacted=self.redacted, signature=self.signature or None
            )
        arguments = self.text if self.text else (self.initial or {})
        return tool_call_part(self.call_id, self.name, arguments)


def _ordered(blocks: dict[int, _Block]) -> list[dict[str, Any]]:
    return [blocks[index].part() for index in sorted(blocks)]


def converse_stream(frames: Iterable[Any]) -> ModelCall:
    call = ModelCall(api="bedrock.converse_stream", status=200, streamed=True)
    blocks: dict[int, _Block] = {}
    for frame in frames:
        if frame.message_type != "event":
            call.error = (
                f"{frame.event_type}: {frame.payload.decode('utf-8', 'replace')}"
            )
            continue
        event, value = frame.event_type, json.loads(frame.payload)
        if event == "messageStart":
            continue
        if event == "contentBlockStart":
            start = value.get("start") or {}
            if "toolUse" not in start:
                raise DecodeError(f"unknown ConverseStream block start: {list(start)}")
            blocks[value["contentBlockIndex"]] = _Block(
                "tool_call",
                call_id=start["toolUse"]["toolUseId"],
                name=start["toolUse"]["name"],
            )
        elif event == "contentBlockDelta":
            index, delta = value["contentBlockIndex"], value["delta"]
            if "text" in delta:
                blocks.setdefault(index, _Block("text")).text += delta["text"]
            elif "toolUse" in delta:
                if index not in blocks or blocks[index].kind != "tool_call":
                    raise DecodeError(f"tool input delta for unopened block {index}")
                blocks[index].text += delta["toolUse"].get("input", "")
            elif "reasoningContent" in delta:
                block = blocks.setdefault(index, _Block("reasoning"))
                reasoning = delta["reasoningContent"]
                if "text" in reasoning:
                    block.text += reasoning["text"]
                elif "signature" in reasoning:
                    block.signature += reasoning["signature"] or ""
                elif "redactedContent" in reasoning:
                    block.redacted = True
                else:
                    raise DecodeError(f"unknown reasoning delta: {list(reasoning)}")
            else:
                raise DecodeError(f"unknown ConverseStream delta: {list(delta)}")
        elif event == "contentBlockStop":
            continue
        elif event == "messageStop":
            call.stop_reason = value.get("stopReason")
            call.finish = _CONVERSE_FINISH.get(call.stop_reason, call.stop_reason)
        elif event == "metadata":
            call.usage = _converse_usage(value.get("usage"))
        else:
            raise DecodeError(f"unknown ConverseStream event: {event}")
    call.parts = _ordered(blocks)
    return call


# --- Anthropic Messages (InvokeModel on Bedrock, or the Messages API) ---------------------------


def _anthropic_usage(usage: dict[str, Any] | None, into: Usage | None = None) -> Usage:
    result = into or Usage()
    usage = usage or {}
    for key, attribute in (
        ("input_tokens", "input"),
        ("output_tokens", "output"),
        ("cache_read_input_tokens", "cache_read"),
        ("cache_creation_input_tokens", "cache_write"),
    ):
        if usage.get(key) is not None:
            setattr(result, attribute, usage[key])
    thinking = (usage.get("output_tokens_details") or {}).get("thinking_tokens")
    if thinking is not None:
        result.reasoning = thinking
    return result


def _anthropic_block(block: dict[str, Any]) -> dict[str, Any]:
    kind = block["type"]
    if kind == "text":
        return text_part(block["text"])
    if kind == "thinking":
        return reasoning_part(block["thinking"], signature=block.get("signature"))
    if kind == "redacted_thinking":
        return reasoning_part(None, redacted=True)
    if kind == "tool_use":
        return tool_call_part(block["id"], block["name"], block.get("input"))
    raise DecodeError(f"unknown Anthropic content block: {kind}")


def anthropic_message(value: dict[str, Any]) -> ModelCall:
    if value.get("type") != "message":
        raise DecodeError(f"not an Anthropic message: {str(value)[:120]}")
    stop = value.get("stop_reason")
    return ModelCall(
        api="anthropic.messages",
        status=200,
        model=value.get("model"),
        response_model=value.get("model"),
        response_id=value.get("id"),
        stop_reason=stop,
        finish=_CONVERSE_FINISH.get(stop, stop),
        usage=_anthropic_usage(value.get("usage")),
        parts=[_anthropic_block(block) for block in value["content"]],
    )


def _bedrock_chunks(frames: Iterable[Any]) -> Iterable[dict[str, Any]]:
    """The Anthropic events InvokeModelWithResponseStream carries base64-encoded in each chunk."""
    for frame in frames:
        if frame.message_type != "event":
            yield {
                "type": "error",
                "error": {
                    "type": frame.event_type,
                    "message": frame.payload.decode("utf-8", "replace"),
                },
            }
            continue
        if frame.event_type != "chunk":
            raise DecodeError(f"unknown InvokeModel stream event: {frame.event_type}")
        yield json.loads(base64.b64decode(json.loads(frame.payload)["bytes"]))


def anthropic_stream(events: Iterable[dict[str, Any]]) -> ModelCall:
    call = ModelCall(api="anthropic.messages", status=200, streamed=True)
    blocks: dict[int, _Block] = {}
    for event in events:
        kind = event["type"]
        if kind == "message_start":
            message = event["message"]
            call.model = call.response_model = message.get("model")
            call.response_id = message.get("id")
            call.usage = _anthropic_usage(message.get("usage"))
            for index, block in enumerate(message.get("content") or []):
                blocks[index] = _block_of(block)
        elif kind == "content_block_start":
            blocks[event["index"]] = _block_of(event["content_block"])
        elif kind == "content_block_delta":
            block, delta = blocks[event["index"]], event["delta"]
            dtype = delta["type"]
            if dtype == "text_delta":
                block.text += delta["text"]
            elif dtype == "input_json_delta":
                block.text += delta["partial_json"]
            elif dtype == "thinking_delta":
                block.text += delta["thinking"]
            elif dtype == "signature_delta":
                block.signature += delta["signature"] or ""
            elif dtype == "citations_delta":
                continue
            else:
                raise DecodeError(f"unknown Anthropic delta: {dtype}")
        elif kind == "message_delta":
            call.stop_reason = (event.get("delta") or {}).get("stop_reason")
            call.finish = _CONVERSE_FINISH.get(call.stop_reason, call.stop_reason)
            call.usage = _anthropic_usage(event.get("usage"), call.usage)
        elif kind in ("content_block_stop", "message_stop", "ping"):
            continue
        elif kind == "error":
            call.error = json.dumps(event.get("error"))
        else:
            raise DecodeError(f"unknown Anthropic stream event: {kind}")
    call.parts = _ordered(blocks)
    return call


def _block_of(block: dict[str, Any]) -> _Block:
    kind = block["type"]
    if kind == "text":
        return _Block("text", text=block.get("text", ""))
    if kind == "thinking":
        return _Block(
            "reasoning",
            text=block.get("thinking", ""),
            signature=block.get("signature") or "",
        )
    if kind == "redacted_thinking":
        return _Block("reasoning", redacted=True)
    if kind == "tool_use":
        return _Block(
            "tool_call",
            call_id=block["id"],
            name=block["name"],
            initial=block.get("input"),
        )
    raise DecodeError(f"unknown Anthropic content block: {kind}")


# --- OpenAI Chat Completions --------------------------------------------------------------------

_CHAT_FINISH = {
    "stop": "stop",
    "tool_calls": "tool_use",
    "function_call": "tool_use",
    "length": "max_tokens",
}


def _chat_usage(usage: dict[str, Any] | None) -> Usage | None:
    if not usage:
        return None
    prompt = usage.get("prompt_tokens_details") or {}
    completion = usage.get("completion_tokens_details") or {}
    return Usage(
        input=usage.get("prompt_tokens"),
        output=usage.get("completion_tokens"),
        cache_read=prompt.get("cached_tokens"),
        cache_write=prompt.get("cache_write_tokens"),
        reasoning=completion.get("reasoning_tokens"),
        input_includes_cache=True,
    )


def _single_choice(choices: list[dict[str, Any]]) -> dict[str, Any]:
    if len(choices) != 1:
        raise DecodeError(f"expected one choice, got {len(choices)}")
    return choices[0]


def chat_completion(value: dict[str, Any]) -> ModelCall:
    choice = _single_choice(value["choices"])
    message = choice["message"]
    parts = []
    if reasoning := message.get("reasoning_content") or message.get("reasoning"):
        parts.append(reasoning_part(reasoning))
    if message.get("content"):
        parts.append(text_part(message["content"]))
    for call in message.get("tool_calls") or []:
        function = call["function"]
        parts.append(
            tool_call_part(call.get("id"), function["name"], function.get("arguments"))
        )
    finish = choice.get("finish_reason")
    return ModelCall(
        api="openai.chat",
        status=200,
        model=value.get("model"),
        response_model=value.get("model"),
        response_id=value.get("id"),
        stop_reason=finish,
        finish=_CHAT_FINISH.get(finish, finish),
        usage=_chat_usage(value.get("usage")),
        parts=parts,
    )


def chat_stream(body: bytes) -> ModelCall:
    call = ModelCall(api="openai.chat", status=200, streamed=True)
    reasoning, text = "", ""
    calls: dict[int, _Block] = {}
    for event in server_sent_events(body):
        if event.data.strip() == "[DONE]":
            continue
        chunk = json.loads(event.data)
        call.model = call.response_model = chunk.get("model") or call.model
        call.response_id = chunk.get("id") or call.response_id
        if chunk.get("usage"):
            call.usage = _chat_usage(chunk["usage"])
        for choice in chunk.get("choices") or []:
            if choice.get("index", 0) != 0:
                raise DecodeError("streamed chat completion with more than one choice")
            delta = choice.get("delta") or {}
            reasoning += delta.get("reasoning_content") or delta.get("reasoning") or ""
            text += delta.get("content") or ""
            for piece in delta.get("tool_calls") or []:
                block = calls.setdefault(piece.get("index", 0), _Block("tool_call"))
                block.call_id = piece.get("id") or block.call_id
                function = piece.get("function") or {}
                block.name = function.get("name") or block.name
                block.text += function.get("arguments") or ""
            if choice.get("finish_reason"):
                call.stop_reason = choice["finish_reason"]
                call.finish = _CHAT_FINISH.get(call.stop_reason, call.stop_reason)
    if reasoning:
        call.parts.append(reasoning_part(reasoning))
    if text:
        call.parts.append(text_part(text))
    call.parts += _ordered(calls)
    return call


# --- OpenAI Responses ---------------------------------------------------------------------------


def _responses_usage(usage: dict[str, Any] | None) -> Usage | None:
    if not usage:
        return None
    details = usage.get("input_tokens_details") or {}
    return Usage(
        input=usage.get("input_tokens"),
        output=usage.get("output_tokens"),
        cache_read=details.get("cached_tokens"),
        cache_write=details.get("cache_write_tokens"),
        reasoning=(usage.get("output_tokens_details") or {}).get("reasoning_tokens"),
        input_includes_cache=True,
    )


def responses_reasoning_text(item: dict[str, Any]) -> str | None:
    """A reasoning item's visible text: its reasoning where the provider returns it, else its summary.

    Each list's parts are joined with a blank line; ``None`` when the item shows no text at all.
    """
    for member, kind in (("content", "reasoning_text"), ("summary", "summary_text")):
        texts = [
            part["text"]
            for part in item.get(member) or []
            if part.get("type") == kind and isinstance(part.get("text"), str)
        ]
        if texts:
            return "\n\n".join(texts)
    return None


def _responses_item(item: dict[str, Any]) -> list[dict[str, Any]]:
    kind = item["type"]
    if kind == "message":
        parts = []
        for content in item.get("content") or []:
            if content["type"] == "output_text":
                # Its citations name sources the search part already holds; the text is the answer.
                parts.append(text_part(content["text"]))
            elif content["type"] == "refusal":
                parts.append(text_part(content["refusal"]))
            else:
                raise DecodeError(
                    f"unknown Responses message content: {content['type']}"
                )
        return parts
    if kind == "function_call":
        return [tool_call_part(item["call_id"], item["name"], item.get("arguments"))]
    if kind == "web_search_call":
        # The provider's own search: what it searched for, and the sources it read.
        action = item.get("action") or {}
        query = action.get("query") or " ".join(action.get("queries") or [])
        sources = [s["url"] for s in action.get("sources") or [] if s.get("url")]
        return [
            server_call_part(
                item.get("id"),
                "web_search",
                {"query": query} if query else {},
                {"sources": sources},
            )
        ]
    if kind == "reasoning":
        # Without visible text the item carries only encrypted reasoning: withheld from view, not
        # redacted by a safety system, so it is reported as signed with no text.
        text = responses_reasoning_text(item)
        return [reasoning_part(text, signature=item.get("encrypted_content"))]
    raise DecodeError(f"unknown Responses output item: {kind}")


def _responses_finish(value: dict[str, Any], parts: list[dict[str, Any]]) -> str | None:
    status = value.get("status")
    if status == "incomplete":
        reason = (value.get("incomplete_details") or {}).get("reason")
        return "max_tokens" if reason == "max_output_tokens" else reason
    if status == "completed":
        return "tool_use" if any(p["type"] == "tool_call" for p in parts) else "stop"
    return status


def responses(value: dict[str, Any]) -> ModelCall:
    parts = [
        part for item in value.get("output") or [] for part in _responses_item(item)
    ]
    return ModelCall(
        api="openai.responses",
        status=200,
        model=value.get("model"),
        response_model=value.get("model"),
        response_id=value.get("id"),
        stop_reason=value.get("status"),
        finish=_responses_finish(value, parts),
        usage=_responses_usage(value.get("usage")),
        parts=parts,
        system_echo=value.get("instructions") or None,
    )


def responses_stream(body: bytes) -> ModelCall:
    """Reassembles a streamed response from its deltas, then checks it against the snapshots.

    The stream carries each item twice - as deltas, and whole in ``output_item.done`` and the final
    ``response.completed`` - so a decode that disagrees with either is a decoder bug, raised here.
    """
    items: dict[int, dict[str, Any]] = {}
    done: dict[int, dict[str, Any]] = {}
    final: dict[str, Any] | None = None
    for event in server_sent_events(body):
        if event.data.strip() == "[DONE]":
            continue
        value = json.loads(event.data)
        kind = value["type"]
        if kind == "response.output_item.added":
            item = dict(value["item"])
            if item["type"] == "message":
                item["content"] = []
            elif item["type"] == "function_call":
                item["arguments"] = ""
            elif item["type"] == "reasoning":
                item["summary"], item["content"] = [], []
            items[value["output_index"]] = item
        elif kind == "response.content_part.added":
            items[value["output_index"]]["content"].append(dict(value["part"]))
        elif kind == "response.output_text.delta":
            item = items[value["output_index"]]
            item["content"][value.get("content_index", 0)]["text"] += value["delta"]
        elif kind == "response.function_call_arguments.delta":
            items[value["output_index"]]["arguments"] += value["delta"]
        elif kind == "response.reasoning_summary_part.added":
            items[value["output_index"]]["summary"].append(dict(value["part"]))
        elif kind == "response.reasoning_summary_text.delta":
            summary = items[value["output_index"]]["summary"]
            summary[value.get("summary_index", 0)]["text"] += value["delta"]
        elif kind == "response.reasoning_text.delta":
            content = items[value["output_index"]]["content"]
            if not content:
                content.append({"type": "reasoning_text", "text": ""})
            content[value.get("content_index", 0)]["text"] += value["delta"]
        elif kind == "response.output_item.done":
            done[value["output_index"]] = value["item"]
        elif kind.startswith("response.web_search_call.") or kind == (
            "response.output_text.annotation.added"
        ):
            # A provider search's stages, and the citations its answer gains: the done item and the
            # completed response hold both whole.
            continue
        elif kind in ("response.completed", "response.incomplete", "response.failed"):
            final = value["response"]
        elif kind.endswith(".done") or kind in (
            "response.created",
            "response.in_progress",
            "response.queued",
        ):
            continue
        else:
            raise DecodeError(f"unknown Responses stream event: {kind}")
    if final is None:
        raise DecodeError("Responses stream ended without a terminal response event")
    parts = [part for index in sorted(items) for part in _responses_item(items[index])]
    for index, item in done.items():
        if _responses_item(item) != _responses_item(items[index]):
            raise DecodeError(
                f"Responses item {index} deltas disagree with its done event"
            )
    if final.get("output"):
        snapshot = [p for item in final["output"] for p in _responses_item(item)]
        if snapshot != parts:
            raise DecodeError("Responses deltas disagree with the completed response")
    call = responses({**final, "output": []})
    call.streamed, call.parts = True, parts
    call.finish = _responses_finish(final, parts)
    return call


# --- Gemini generateContent ---------------------------------------------------------------------

_GEMINI_FINISH = {"MAX_TOKENS": "max_tokens"}


def gemini_chunks(chunks: list[dict[str, Any]], *, streamed: bool) -> ModelCall:
    call = ModelCall(api="gemini.generate_content", status=200, streamed=streamed)
    parts: list[dict[str, Any]] = []
    for chunk in chunks:
        call.model = call.response_model = chunk.get("modelVersion") or call.model
        call.response_id = chunk.get("responseId") or call.response_id
        if usage := chunk.get("usageMetadata"):
            call.usage = Usage(
                input=usage.get("promptTokenCount"),
                output=usage.get("candidatesTokenCount"),
                cache_read=usage.get("cachedContentTokenCount"),
                reasoning=usage.get("thoughtsTokenCount"),
                input_includes_cache=True,
            )
        candidates = chunk.get("candidates") or []
        if len(candidates) > 1:
            raise DecodeError("Gemini response with more than one candidate")
        for candidate in candidates:
            for part in (candidate.get("content") or {}).get("parts") or []:
                _gemini_part(parts, part)
            if candidate.get("finishReason"):
                call.stop_reason = candidate["finishReason"]
    call.parts = parts
    has_calls = any(p["type"] == "tool_call" for p in parts)
    if call.stop_reason == "STOP":
        call.finish = "tool_use" if has_calls else "stop"
    else:
        call.finish = _GEMINI_FINISH.get(call.stop_reason or "", call.stop_reason)
    return call


def _gemini_part(parts: list[dict[str, Any]], part: dict[str, Any]) -> None:
    """Appends a part, joining a text piece onto the previous one of the same kind."""
    if "functionCall" in part or "function_call" in part:
        function = part.get("functionCall") or part["function_call"]
        parts.append(
            tool_call_part(function.get("id"), function["name"], function.get("args"))
        )
        return
    if not isinstance(part.get("text"), str):
        raise DecodeError(f"unknown Gemini part: {list(part)}")
    kind = "reasoning" if part.get("thought") else "text"
    signature = part.get("thoughtSignature") or part.get("thought_signature")
    if parts and parts[-1]["type"] == kind:
        parts[-1]["text"] += part["text"]
        if kind == "reasoning" and signature and "seal" not in parts[-1]:
            parts[-1].update(reasoning_part(parts[-1]["text"], signature=signature))
        return
    parts.append(
        reasoning_part(part["text"], signature=signature)
        if kind == "reasoning"
        else text_part(part["text"])
    )


def decode_cassette(path: Path) -> tuple[list[ModelCall], int]:
    """The model calls a cassette recorded, and how many of its requests reached no model."""
    interactions = json.loads(path.read_text())["interactions"]
    calls, ignored = [], 0
    for item in interactions:
        call = decode(item)
        if call is None:
            ignored += 1
        else:
            calls.append(call)
    return calls, ignored
