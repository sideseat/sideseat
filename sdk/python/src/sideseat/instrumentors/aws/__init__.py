"""AWS provider instrumentation — patches botocore to capture Bedrock telemetry."""

from __future__ import annotations

import logging
from typing import TYPE_CHECKING, Any

from sideseat._utils import _module_available

if TYPE_CHECKING:
    from opentelemetry.sdk.trace import TracerProvider

logger = logging.getLogger("sideseat.instrumentors.aws")


class AWSInstrumentor:
    """Patches botocore ClientCreator to instrument AWS AI service clients.

    Singleton — calling instrument() multiple times is a no-op.
    """

    _instance: AWSInstrumentor | None = None

    def __init__(self, tracer_provider: TracerProvider | None = None) -> None:
        self._provider = tracer_provider

    def instrument(self) -> None:
        if AWSInstrumentor._instance is not None:
            return

        if not _module_available("wrapt"):
            logger.debug("wrapt not installed — AWS instrumentation unavailable")
            return

        import wrapt

        wrapt.wrap_function_wrapper(
            "botocore.client",
            "ClientCreator.create_client",
            self._on_create_client,
        )
        AWSInstrumentor._instance = self
        logger.debug("Patched botocore ClientCreator.create_client")

    def _on_create_client(
        self,
        wrapped: Any,
        instance: Any,
        args: tuple[Any, ...],
        kwargs: dict[str, Any],
    ) -> Any:
        client = wrapped(*args, **kwargs)

        service = getattr(client, "_service_model", None)
        if not service:
            return client

        name = service.service_name
        try:
            if name == "bedrock-runtime":
                from .bedrock import patch_bedrock_client

                patch_bedrock_client(client, self._provider)
            elif name == "bedrock-agent-runtime":
                from .bedrock_agent import patch_bedrock_agent_client

                patch_bedrock_agent_client(client, self._provider)
        except Exception as exc:
            logger.debug("Failed to patch %s client: %s", name, exc)

        return client
