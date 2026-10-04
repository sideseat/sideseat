"""Maps a harness model alias to an Anthropic client and model id."""

from dataclasses import dataclass
from typing import Any

from harness import Model
from harness.clients import anthropic_client

#: Current Claude models reject a fixed thinking budget; adaptive thinking with a summary is what
#: makes the reasoning visible in telemetry.
THINKING: dict[str, Any] = {"type": "adaptive", "display": "summarized"}
MAX_TOKENS = 16_000


@dataclass
class Claude:
    client: Any
    model: str


def build(model: Model) -> Claude:
    return Claude(client=anthropic_client(model), model=model.id)
