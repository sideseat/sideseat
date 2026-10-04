"""Agents marked with OpenInference's ``@tracer.agent`` decorator.

The decorator records the arguments of the function it wraps as the agent's input, so the traced
function takes only the user's text; the conversation and any attachments are bound around it.
"""

from __future__ import annotations

from collections.abc import Awaitable, Callable, Sequence
from typing import Any

from converse import Conversation
from tracing import tracer


def agent(
    name: str, conversation: Conversation, attachments: Sequence[dict[str, Any]] = ()
) -> Callable[[str], Awaitable[str]]:
    @tracer.agent(name=name)
    async def ask(prompt: str) -> str:
        return await conversation.ask([{"text": prompt}, *attachments])

    return ask
