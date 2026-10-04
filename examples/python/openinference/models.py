"""Maps a harness model alias to an instrumented Bedrock Runtime client and model id."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

import boto3
from botocore.config import Config
from openinference.instrumentation.bedrock import BedrockInstrumentor

from harness import Model
from harness.models import region


@dataclass(frozen=True)
class BedrockModel:
    id: str
    client: Any


def build(model: Model) -> BedrockModel:
    if model.surface != "bedrock":
        raise SystemExit(
            f"the OpenInference suite calls Bedrock Converse; {model.alias} is {model.surface}"
        )
    # The application installs its OpenInference instrumentors once telemetry is configured, in either
    # mode, on the global provider; the instrumentor patches clients as botocore creates them.
    BedrockInstrumentor().instrument()
    # Long reasoning outlasts botocore's 60-second read timeout, and a retried call records a second,
    # different answer for the same request.
    config = Config(read_timeout=600)
    return BedrockModel(
        model.id, boto3.client("bedrock-runtime", region_name=region(), config=config)
    )
