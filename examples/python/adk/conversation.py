"""Runs an ADK agent in one ADK session, the way ADK's Runner documentation shows."""

from __future__ import annotations

from collections.abc import Sequence

from google.adk.agents import BaseAgent
from google.adk.agents.run_config import RunConfig
from google.adk.runners import InMemoryRunner
from google.genai import types

from harness import Run


class Conversation:
    """One ADK session; every :meth:`ask` continues it, so each request re-sends the history."""

    def __init__(
        self, run: Run, agent: BaseAgent, *, run_config: RunConfig | None = None
    ) -> None:
        self._run = run
        self._runner = InMemoryRunner(agent=agent, app_name=run.producer)
        self._run_config = run_config
        self._session_id: str | None = None

    async def ask(self, prompt: str | Sequence[types.Part]) -> str:
        if self._session_id is None:
            # The ADK session takes the scenario's session id, so the session ADK records and the
            # caller's agree.
            session = await self._runner.session_service.create_session(
                app_name=self._run.producer,
                user_id=self._run.user_id,
                session_id=self._run.session_id,
            )
            self._session_id = session.id
        parts = [types.Part(text=prompt)] if isinstance(prompt, str) else list(prompt)
        answer: list[str] = []
        async for event in self._runner.run_async(
            user_id=self._run.user_id,
            session_id=self._session_id,
            new_message=types.Content(role="user", parts=parts),
            run_config=self._run_config,
        ):
            if event.partial or not event.content:
                continue
            answer.extend(
                part.text
                for part in event.content.parts or ()
                if part.text and not part.thought
            )
        return "".join(answer)
