"""Agents as Logfire spans around a conversation turn."""

from __future__ import annotations

from collections.abc import Awaitable, Callable, Sequence
from typing import Any

import logfire
from conversation import Conversation


def agent(
    name: str, conversation: Conversation, attachments: Sequence[dict[str, Any]] = ()
) -> Callable[[str], Awaitable[str]]:
    async def ask(prompt: str) -> str:
        # Span attributes stay off the names Logfire uses for conversations (`prompt`, `events`,
        # `request_data`, ...): the instrumented model calls below already carry the messages.
        with logfire.span("agent {agent_name}", agent_name=name):
            if not attachments:
                return await conversation.ask(prompt)
            return await conversation.ask(
                [{"type": "input_text", "text": prompt}, *attachments]
            )

    return ask
