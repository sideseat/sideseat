"""Maps a harness model alias to an OpenAI client and model id.

``langfuse.openai`` is Langfuse's drop-in OpenAI integration: importing it patches the OpenAI client
classes, so every client the application builds records generations. It is application code, not
telemetry setup, which is why it is imported here and not in ``native.py``.
"""

from dataclasses import dataclass
from typing import Any

import langfuse.openai  # noqa: F401

from harness import Model
from harness.clients import openai_client


@dataclass(frozen=True)
class OpenAIModel:
    client: Any
    id: str


def build(model: Model) -> OpenAIModel:
    return OpenAIModel(client=openai_client(model), id=model.id)
