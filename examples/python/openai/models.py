"""Maps a harness model alias to an OpenAI client and model id."""

from dataclasses import dataclass
from typing import Any

from harness import Model
from harness.clients import openai_client


@dataclass(frozen=True)
class OpenAIModel:
    client: Any
    id: str


def build(model: Model) -> OpenAIModel:
    return OpenAIModel(client=openai_client(model), id=model.id)
