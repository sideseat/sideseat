"""Strict decoders for what a framework sent a model: the request side of each wire format.

The response decoders (:mod:`harness.truth.wire`) say what the model said; these say what it was told -
the system instruction, the conversation as sent with its message boundaries, attachments by digest,
tool calls and results with their ids, and the tools offered. A block a decoder does not know is an
error, as on the response side: an unrecognised part is a part the truth would silently lose.
"""

from __future__ import annotations

import base64
import binascii
import hashlib
import json
import re
from dataclasses import dataclass, field
from typing import Any
from urllib.parse import unquote, urlsplit

from harness.truth.wire import DecodeError

_BEDROCK = re.compile(
    r"/model/(?P<model>[^/]+)/(?P<op>converse|converse-stream|invoke|invoke-with-response-stream)$"
)
_GEMINI = re.compile(
    r"/models/(?P<model>[^/:]+):(?P<op>generateContent|streamGenerateContent)$"
)


@dataclass
class ModelRequest:
    """One request: ordered system parts, ordered messages of typed parts, the tools offered."""

    api: str
    model: str | None
    system: list[dict[str, Any]] = field(default_factory=list)
    messages: list[dict[str, Any]] = field(default_factory=list)
    tools: list[str] = field(default_factory=list)

    def to_json(self) -> dict[str, Any]:
        return {
            "api": self.api,
            "model": self.model,
            "system": self.system,
            "messages": self.messages,
            "tools": self.tools,
        }


def _text(text: Any) -> dict[str, Any]:
    if not isinstance(text, str):
        raise DecodeError(f"text is not a string: {text!r:.80}")
    return {"type": "text", "text": text}


_URLSAFE = re.compile(r"[A-Za-z0-9_-]*={0,2}")


def _bytes(modality: str, data: str) -> bytes:
    """Inline bytes in either base64 alphabet: Google's clients send the URL-safe one."""
    try:
        return base64.b64decode(data, validate=True)
    except binascii.Error:
        if _URLSAFE.fullmatch(data):
            return base64.urlsafe_b64decode(data + "=" * (-len(data) % 4))
        raise DecodeError(f"{modality} bytes are not base64") from None


def _media(modality: str, mime: str | None, data: str | bytes) -> dict[str, Any]:
    raw = _bytes(modality, data) if isinstance(data, str) else data
    return {
        "type": "media",
        "modality": modality,
        "media_type": mime,
        "size": len(raw),
        "sha256": hashlib.sha256(raw).hexdigest(),
    }


def _arguments(value: Any) -> Any:
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


def _call(call_id: Any, name: Any, arguments: Any) -> dict[str, Any]:
    if not isinstance(name, str) or not name:
        raise DecodeError(f"a tool call without a name: {name!r}")
    return {
        "type": "tool_call",
        "id": call_id if isinstance(call_id, str) and call_id else None,
        "name": name,
        "arguments": _arguments(arguments),
    }


def _result(call_id: Any, content: Any, *, error: bool = False) -> dict[str, Any]:
    return {
        "type": "tool_result",
        "id": call_id if isinstance(call_id, str) and call_id else None,
        "content": content,
        "is_error": error,
    }


def _reasoning(text: Any, *, signed: bool = False) -> dict[str, Any]:
    return {"type": "reasoning", "text": text, "signed": signed}


# --- Bedrock Converse -------------------------------------------------------------------------------

_CONVERSE_MEDIA = {"image": "image", "document": "document", "video": "video"}


#: Converse names a format where every other API states a MIME type.
_CONVERSE_DOCUMENT_TYPES = {
    "pdf": "application/pdf",
    "csv": "text/csv",
    "txt": "text/plain",
    "html": "text/html",
    "md": "text/markdown",
    "doc": "application/msword",
    "docx": "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    "xls": "application/vnd.ms-excel",
    "xlsx": "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
}


def _converse_mime(modality: str, format_: str | None) -> str | None:
    if format_ is None:
        return None
    if modality == "document":
        if format_ not in _CONVERSE_DOCUMENT_TYPES:
            raise DecodeError(f"unknown converse document format: {format_!r}")
        return _CONVERSE_DOCUMENT_TYPES[format_]
    return f"{modality}/{format_}"


def _converse_block(block: dict[str, Any]) -> dict[str, Any] | None:
    if "text" in block:
        return _text(block["text"])
    if "cachePoint" in block:
        return None
    for key, modality in _CONVERSE_MEDIA.items():
        if key in block:
            media = block[key]
            source = media.get("source") or {}
            if "bytes" not in source:
                raise DecodeError(
                    f"converse {key} without inline bytes: {sorted(source)}"
                )
            part = _media(
                modality, _converse_mime(modality, media.get("format")), source["bytes"]
            )
            if media.get("name") is not None:
                part["name"] = media["name"]
            return part
    if "toolUse" in block:
        use = block["toolUse"]
        return _call(use.get("toolUseId"), use.get("name"), use.get("input", {}))
    if "toolResult" in block:
        result = block["toolResult"]
        content = []
        for item in result.get("content", []):
            if "text" in item:
                content.append(_text(item["text"]))
            elif "json" in item:
                content.append({"type": "json", "value": item["json"]})
            else:
                part = _converse_block(item)
                if part is None:
                    continue
                content.append(part)
        return _result(
            result.get("toolUseId"), content, error=result.get("status") == "error"
        )
    if "reasoningContent" in block:
        reasoning = block["reasoningContent"]
        if "reasoningText" in reasoning:
            text = reasoning["reasoningText"]
            return _reasoning(text.get("text"), signed=bool(text.get("signature")))
        if "redactedContent" in reasoning:
            return _reasoning(None, signed=True)
    raise DecodeError(f"unknown converse request block: {sorted(block)}")


def converse_request(value: dict[str, Any], model: str | None) -> ModelRequest:
    request = ModelRequest("bedrock.converse", model)
    for block in value.get("system", []):
        part = _converse_block(block)
        if part is not None:
            request.system.append(part)
    for message in value.get("messages", []):
        parts = [p for b in message.get("content", []) if (p := _converse_block(b))]
        request.messages.append({"role": message["role"], "parts": parts})
    for tool in (value.get("toolConfig") or {}).get("tools", []):
        if "toolSpec" in tool:
            request.tools.append(tool["toolSpec"]["name"])
        elif "cachePoint" not in tool:
            raise DecodeError(f"unknown converse tool entry: {sorted(tool)}")
    return request


# --- Anthropic Messages -----------------------------------------------------------------------------


def _anthropic_content(content: Any) -> list[dict[str, Any]]:
    if isinstance(content, str):
        return [_text(content)]
    return [p for block in content if (p := _anthropic_block(block))]


def _anthropic_block(block: dict[str, Any]) -> dict[str, Any] | None:
    kind = block.get("type")
    if kind == "text":
        return _text(block["text"])
    if kind in ("image", "document"):
        source = block.get("source") or {}
        if source.get("type") != "base64":
            raise DecodeError(
                f"anthropic {kind} source {source.get('type')!r} is not inline"
            )
        part = _media(kind, source.get("media_type"), source["data"])
        if block.get("title") is not None:
            part["name"] = block["title"]
        return part
    if kind == "tool_use":
        return _call(block.get("id"), block.get("name"), block.get("input", {}))
    if kind == "tool_result":
        content = block.get("content", [])
        return _result(
            block.get("tool_use_id"),
            _anthropic_content(content),
            error=bool(block.get("is_error")),
        )
    if kind == "thinking":
        return _reasoning(block.get("thinking"), signed=bool(block.get("signature")))
    if kind == "redacted_thinking":
        return _reasoning(None, signed=True)
    raise DecodeError(f"unknown anthropic request block: {kind!r}")


def anthropic_request(value: dict[str, Any], model: str | None) -> ModelRequest:
    request = ModelRequest("anthropic.messages", model)
    system = value.get("system")
    if system is not None:
        request.system = _anthropic_content(system)
    for message in value.get("messages", []):
        request.messages.append(
            {"role": message["role"], "parts": _anthropic_content(message["content"])}
        )
    request.tools = [tool["name"] for tool in value.get("tools", [])]
    return request


# --- OpenAI Chat Completions ------------------------------------------------------------------------

_DATA_URL = re.compile(r"data:(?P<mime>[^;,]+)?(?:;[^,]*)?;base64,(?P<data>.*)", re.S)


def _data_url(url: str, modality: str) -> dict[str, Any]:
    match = _DATA_URL.fullmatch(url)
    if not match:
        raise DecodeError(f"{modality} is not an inline data URL: {url[:40]!r}")
    return _media(modality, match["mime"], match["data"])


def _chat_content(content: Any) -> list[dict[str, Any]]:
    if content is None:
        return []
    if isinstance(content, str):
        return [_text(content)]
    parts = []
    for block in content:
        kind = block.get("type")
        if kind == "text":
            parts.append(_text(block["text"]))
        elif kind == "image_url":
            parts.append(_data_url(block["image_url"]["url"], "image"))
        elif kind == "file":
            file = block["file"]
            part = _data_url(file["file_data"], "document")
            if file.get("filename") is not None:
                part["name"] = file["filename"]
            parts.append(part)
        elif kind == "input_audio":
            audio = block["input_audio"]
            parts.append(_media("audio", audio.get("format"), audio["data"]))
        else:
            raise DecodeError(f"unknown chat content part: {kind!r}")
    return parts


def chat_request(value: dict[str, Any], model: str | None) -> ModelRequest:
    request = ModelRequest("openai.chat", value.get("model", model))
    for message in value.get("messages", []):
        role = message["role"]
        if role in ("system", "developer"):
            request.system.extend(_chat_content(message["content"]))
            continue
        if role == "tool":
            parts = [
                _result(message.get("tool_call_id"), _chat_content(message["content"]))
            ]
        else:
            parts = _chat_content(message.get("content"))
            for call in message.get("tool_calls") or []:
                function = call["function"]
                parts.append(
                    _call(call.get("id"), function["name"], function.get("arguments"))
                )
        request.messages.append({"role": role, "parts": parts})
    request.tools = [tool["function"]["name"] for tool in value.get("tools", [])]
    return request


# --- OpenAI Responses -------------------------------------------------------------------------------


def _responses_content(content: Any) -> list[dict[str, Any]]:
    if isinstance(content, str):
        return [_text(content)]
    parts = []
    for block in content:
        kind = block.get("type")
        if kind in ("input_text", "output_text", "text"):
            parts.append(_text(block["text"]))
        elif kind == "input_image":
            parts.append(_data_url(block["image_url"], "image"))
        elif kind == "input_file":
            part = _data_url(block["file_data"], "document")
            if block.get("filename") is not None:
                part["name"] = block["filename"]
            parts.append(part)
        elif kind == "refusal":
            parts.append(_text(block["refusal"]))
        else:
            raise DecodeError(f"unknown responses content part: {kind!r}")
    return parts


def responses_request(value: dict[str, Any], model: str | None) -> ModelRequest:
    request = ModelRequest("openai.responses", value.get("model", model))
    if value.get("instructions"):
        request.system.append(_text(value["instructions"]))
    items = value.get("input", [])
    if isinstance(items, str):
        items = [{"role": "user", "content": items}]
    for item in items:
        kind = item.get("type", "message")
        if kind == "message":
            role = item["role"]
            parts = _responses_content(item["content"])
            if role in ("system", "developer"):
                request.system.extend(parts)
            else:
                request.messages.append({"role": role, "parts": parts})
        elif kind == "function_call":
            request.messages.append(
                {
                    "role": "assistant",
                    "parts": [
                        _call(item.get("call_id"), item["name"], item.get("arguments"))
                    ],
                }
            )
        elif kind == "function_call_output":
            output = item.get("output")
            content = _responses_content(output) if output is not None else []
            request.messages.append(
                {"role": "tool", "parts": [_result(item.get("call_id"), content)]}
            )
        elif kind == "reasoning":
            signed = bool(item.get("encrypted_content"))
            text = "\n".join(s.get("text", "") for s in item.get("summary", [])) or None
            # Encrypted reasoning with no summary is withheld, not redacted: signed, with no text,
            # as Anthropic's withheld thinking is.
            if text is None and signed:
                text = ""
            request.messages.append(
                {"role": "assistant", "parts": [_reasoning(text, signed=signed)]}
            )
        else:
            raise DecodeError(f"unknown responses input item: {kind!r}")
    request.tools = [tool["name"] for tool in value.get("tools", []) if "name" in tool]
    return request


# --- Gemini -----------------------------------------------------------------------------------------


#: The modalities a MIME type's top level names as itself; everything else - a PDF, a spreadsheet, plain
#: text - is a document, as every other API's decoder here calls it.
_GEMINI_MEDIA_MODALITIES = ("image", "audio", "video")


def _gemini_modality(mime: str) -> str:
    """The modality of inline data, from its MIME type: Gemini names no modality of its own."""
    top = mime.split("/")[0]
    return top if top in _GEMINI_MEDIA_MODALITIES else "document"


def _gemini_part(part: dict[str, Any]) -> dict[str, Any]:
    if "text" in part:
        if part.get("thought"):
            return _reasoning(part["text"], signed=bool(part.get("thoughtSignature")))
        return _text(part["text"])
    if "inlineData" in part:
        data = part["inlineData"]
        # The REST field is `mimeType`; Google's Python client sends its own `mime_type` spelling.
        mime = data.get("mimeType") or data.get("mime_type") or ""
        return _media(_gemini_modality(mime), mime, data["data"])
    if "functionCall" in part:
        call = part["functionCall"]
        return _call(call.get("id"), call.get("name"), call.get("args", {}))
    if "functionResponse" in part:
        response = part["functionResponse"]
        return _result(
            response.get("id"), [{"type": "json", "value": response.get("response")}]
        )
    raise DecodeError(f"unknown gemini request part: {sorted(part)}")


def gemini_request(value: dict[str, Any], model: str | None) -> ModelRequest:
    request = ModelRequest("gemini.generate_content", model)
    instruction = value.get("systemInstruction") or value.get("system_instruction")
    if instruction:
        request.system = [_gemini_part(p) for p in instruction.get("parts", [])]
    for content in value.get("contents", []):
        request.messages.append(
            {
                "role": content.get("role", "user"),
                "parts": [_gemini_part(p) for p in content.get("parts", [])],
            }
        )
    for tool in value.get("tools", []):
        for declaration in (
            tool.get("functionDeclarations") or tool.get("function_declarations") or []
        ):
            request.tools.append(declaration["name"])
    return request


def decode_request(method: str, path: str, body: bytes) -> ModelRequest | None:
    """The model request an interaction carried, or ``None`` for a request to no model."""
    del method
    route = urlsplit(path).path
    if bedrock := _BEDROCK.search(route):
        model, operation = unquote(bedrock["model"]), bedrock["op"]
        value = json.loads(body)
        if operation.startswith("converse"):
            return converse_request(value, model)
        return anthropic_request(value, model)
    if route.endswith("/chat/completions"):
        return chat_request(json.loads(body), None)
    if route.endswith("/responses"):
        return responses_request(json.loads(body), None)
    if route.endswith("/v1/messages"):
        return anthropic_request(json.loads(body), json.loads(body).get("model"))
    if gemini := _GEMINI.search(route):
        return gemini_request(json.loads(body), gemini["model"])
    return None
