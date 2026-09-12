"""Telemetry setup for LangGraph samples."""

from openinference.instrumentation.langchain import LangChainInstrumentor
from opentelemetry.instrumentation.botocore import BotocoreInstrumentor
from sideseat import Frameworks

from common.telemetry import setup_base_telemetry


def setup_telemetry(use_sideseat: bool = False):
    """Initialize telemetry for LangGraph samples.

    Default: OpenTelemetry with console and OTLP exporters.
    Optional: SideSeat SDK with automatic OTLP setup + console exporter.

    Also instruments boto3/botocore for AWS call tracing.
    """

    def instrumentor(provider=None):
        LangChainInstrumentor().instrument(
            tracer_provider=provider, skip_dep_check=True
        )
        BotocoreInstrumentor().instrument()

    return setup_base_telemetry(
        instrumentor=instrumentor,
        use_sideseat=use_sideseat,
        framework=Frameworks.LangGraph,
    )
