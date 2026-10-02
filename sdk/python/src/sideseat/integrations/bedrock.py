"""Amazon Bedrock through boto3: Converse, ConverseStream, InvokeModel, and agent runtimes.

Clients are instrumented when they are created, so create them after :func:`sideseat.init`.
"""

from __future__ import annotations

import logging
from typing import Any

from sideseat.integrations._base import Integration, SetupContext

logger = logging.getLogger("sideseat")


class Bedrock(Integration):
    name = "bedrock"
    packages = ("boto3", "botocore")
    detectable = False
    extra = "bedrock"

    def __init__(self) -> None:
        self._installed = False

    def instrument(self, ctx: SetupContext) -> None:
        import wrapt

        provider = ctx.tracer_provider

        def on_create_client(wrapped: Any, instance: Any, args: Any, kwargs: Any) -> Any:
            client = wrapped(*args, **kwargs)
            service = getattr(getattr(client, "_service_model", None), "service_name", None)
            try:
                if service == "bedrock-runtime":
                    from sideseat.integrations._bedrock.runtime import patch_bedrock_client

                    patch_bedrock_client(client, provider)
                elif service == "bedrock-agent-runtime":
                    from sideseat.integrations._bedrock.agent_runtime import (
                        patch_bedrock_agent_client,
                    )

                    patch_bedrock_agent_client(client, provider)
            except Exception:
                # A failure here must not break the application's client; it only loses telemetry.
                logger.warning("Could not instrument the %s client", service, exc_info=True)
            return client

        wrapt.wrap_function_wrapper(
            "botocore.client", "ClientCreator.create_client", on_create_client
        )
        self._installed = True

    def shutdown(self) -> None:
        if not self._installed:
            return
        import botocore.client

        wrapped = botocore.client.ClientCreator.create_client
        original = getattr(wrapped, "__wrapped__", None)
        if original is not None:
            botocore.client.ClientCreator.create_client = original
        self._installed = False
