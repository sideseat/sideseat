"""A minimal agent loop on the Bedrock Converse API: ask, run the tools the model calls, answer.

OpenInference instruments the provider client, not an agent framework, so the scenarios bring their own
loop. Each request re-sends the whole conversation, which is what Converse requires.
"""

from __future__ import annotations

import inspect
import json
from collections.abc import Callable, Sequence
from dataclasses import dataclass, field
from typing import Any

from models import BedrockModel


@dataclass
class Tool:
    spec: dict[str, Any]
    call: Callable[..., Any]

    @property
    def name(self) -> str:
        return str(self.spec["name"])


@dataclass
class Conversation:
    model: BedrockModel
    system: str | None = None
    tools: Sequence[Tool] = ()
    stream: bool = False
    #: Extra Converse request fields, such as ``outputConfig`` or ``additionalModelRequestFields``.
    fields: dict[str, Any] = field(default_factory=dict)
    messages: list[dict[str, Any]] = field(default_factory=list)

    async def ask(self, prompt: str | list[dict[str, Any]]) -> str:
        """Send a user turn and keep calling tools until the model answers."""
        blocks = [{"text": prompt}] if isinstance(prompt, str) else prompt
        self.messages.append({"role": "user", "content": blocks})
        while True:
            message, stop = self._converse()
            self.messages.append(message)
            if stop != "tool_use":
                return "".join(b["text"] for b in message["content"] if "text" in b)
            results = [
                await self._run(b["toolUse"])
                for b in message["content"]
                if "toolUse" in b
            ]
            self.messages.append({"role": "user", "content": results})

    def _request(self) -> dict[str, Any]:
        request: dict[str, Any] = {
            "modelId": self.model.id,
            "messages": self.messages,
            "inferenceConfig": {"maxTokens": 16_000},
            **self.fields,
        }
        if self.system:
            request["system"] = [{"text": self.system}]
        if self.tools:
            request["toolConfig"] = {
                "tools": [{"toolSpec": t.spec} for t in self.tools],
                "toolChoice": {"auto": {}},
            }
        return request

    def _converse(self) -> tuple[dict[str, Any], str]:
        if not self.stream:
            response = self.model.client.converse(**self._request())
            return response["output"]["message"], response["stopReason"]
        response = self.model.client.converse_stream(**self._request())
        content: list[dict[str, Any]] = []
        tool_input = ""
        stop = "end_turn"
        for event in response["stream"]:
            if (
                start := event.get("contentBlockStart", {})
                .get("start", {})
                .get("toolUse")
            ):
                content.append({"toolUse": {**start, "input": {}}})
                tool_input = ""
            elif delta := event.get("contentBlockDelta", {}).get("delta"):
                if "text" in delta:
                    print(delta["text"], end="", flush=True)
                    if not content or "text" not in content[-1]:
                        content.append({"text": ""})
                    content[-1]["text"] += delta["text"]
                elif "toolUse" in delta:
                    tool_input += delta["toolUse"]["input"]
            elif "contentBlockStop" in event and content and "toolUse" in content[-1]:
                content[-1]["toolUse"]["input"] = json.loads(tool_input or "{}")
            elif "messageStop" in event:
                stop = event["messageStop"]["stopReason"]
        print()
        return {"role": "assistant", "content": content}, stop

    async def _run(self, use: dict[str, Any]) -> dict[str, Any]:
        tool = next(t for t in self.tools if t.name == use["name"])
        try:
            result = tool.call(**use["input"])
            if inspect.isawaitable(result):
                result = await result
        except Exception as error:  # the model reads the failure and answers anyway
            return {
                "toolResult": {
                    "toolUseId": use["toolUseId"],
                    "content": [{"text": f"{type(error).__name__}: {error}"}],
                    "status": "error",
                }
            }
        text = result if isinstance(result, str) else json.dumps(result)
        return {
            "toolResult": {"toolUseId": use["toolUseId"], "content": [{"text": text}]}
        }
