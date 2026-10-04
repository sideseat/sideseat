"""A minimal agent loop on the Responses API: ask, run the tools the model calls, answer.

Logfire instruments the provider client, not an agent framework, so the scenarios bring their own
loop. It is the Responses API rather than Chat Completions because current GPT models always reason,
and Bedrock serves function tools to a reasoning model only there. Each request re-sends the whole
conversation, as a stateless client does.
"""

from __future__ import annotations

import inspect
import json
from collections.abc import Callable, Sequence
from dataclasses import dataclass, field
from typing import Any

import logfire
from models import OpenAIModel


@dataclass
class Tool:
    spec: dict[str, Any]
    call: Callable[..., Any]

    @property
    def name(self) -> str:
        return str(self.spec["name"])


@dataclass
class Conversation:
    model: OpenAIModel
    system: str | None = None
    tools: Sequence[Tool] = ()
    stream: bool = False
    #: Extra request fields, such as ``text`` for a response format.
    fields: dict[str, Any] = field(default_factory=dict)
    items: list[dict[str, Any]] = field(default_factory=list)

    async def ask(self, prompt: str | list[dict[str, Any]]) -> str:
        """Send a user turn and keep calling tools until the model answers."""
        self.items.append({"role": "user", "content": prompt})
        while True:
            response = self._create()
            output = [item.model_dump(exclude_none=True) for item in response.output]
            self.items.extend(output)
            calls = [item for item in output if item["type"] == "function_call"]
            if not calls:
                return str(response.output_text)
            for call in calls:
                self.items.append(await self._run(call))

    def _request(self) -> dict[str, Any]:
        request: dict[str, Any] = {
            "model": self.model.id,
            "input": self.items,
            **self.fields,
        }
        if self.system:
            request["instructions"] = self.system
        if self.tools:
            request["tools"] = [t.spec for t in self.tools]
        return request

    def _create(self) -> Any:
        client = self.model.client
        if not self.stream:
            return client.responses.create(**self._request())
        completed = None
        for event in client.responses.create(**self._request(), stream=True):
            if event.type == "response.output_text.delta":
                print(event.delta, end="", flush=True)
            elif event.type == "response.completed":
                completed = event.response
        print()
        if completed is None:
            raise RuntimeError("the response stream ended without completing")
        return completed

    async def _run(self, call: dict[str, Any]) -> dict[str, Any]:
        name = call["name"]
        tool = next(t for t in self.tools if t.name == name)
        with logfire.span("running tool {tool_name}", tool_name=name):
            try:
                result = tool.call(**json.loads(call.get("arguments") or "{}"))
                if inspect.isawaitable(result):
                    result = await result
            except Exception as error:  # the model reads the failure and answers anyway
                result = f"{type(error).__name__}: {error}"
        text = result if isinstance(result, str) else json.dumps(result)
        return {
            "type": "function_call_output",
            "call_id": call["call_id"],
            "output": text,
        }
