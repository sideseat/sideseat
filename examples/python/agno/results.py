"""Agno reports a failed run in its result rather than raising, so a scenario checks before it prints."""

from typing import Any

from agno.run.base import RunStatus


def answer(result: Any) -> Any:
    if result.status == RunStatus.error:
        raise RuntimeError(f"the Agno run failed: {result.content}")
    return result.content
