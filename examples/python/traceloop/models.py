"""Maps a harness model alias to a Bedrock Runtime client and model id."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

import boto3
from botocore.config import Config

from harness import Model
from harness.models import region


@dataclass(frozen=True)
class BedrockModel:
    id: str
    client: Any


def build(model: Model) -> BedrockModel:
    if model.surface != "bedrock":
        raise SystemExit(
            f"the TraceLoop suite calls Bedrock Converse; {model.alias} is {model.surface}"
        )
    # Created after telemetry is configured: TraceLoop instruments clients as botocore makes them.
    # Long reasoning outlasts botocore's 60-second read timeout, and a retried call records a second,
    # different answer for the same request.
    config = Config(read_timeout=600)
    return BedrockModel(
        model.id, boto3.client("bedrock-runtime", region_name=region(), config=config)
    )
