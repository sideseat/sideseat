"""Maps a harness model alias to a bedrock-runtime client and model id."""

from dataclasses import dataclass
from typing import Any

import boto3

from harness import Model
from harness.models import region


@dataclass(frozen=True)
class Bedrock:
    client: Any
    model_id: str


def build(model: Model) -> Bedrock:
    if model.surface != "bedrock":
        raise SystemExit(
            f"the Bedrock suite calls the Converse API; {model.alias} is {model.surface}"
        )
    # Built on first use, after telemetry is configured: SideSeat instruments clients as they are created.
    return Bedrock(boto3.client("bedrock-runtime", region_name=region()), model.id)
