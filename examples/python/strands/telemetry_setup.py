"""Telemetry setup for Strands samples.

Strands has its own telemetry system (StrandsTelemetry) with built-in
instrumentation, so it doesn't use the common telemetry base.
"""

from opentelemetry.instrumentation.botocore import BotocoreInstrumentor
from sideseat import Frameworks, SideSeat
from strands.telemetry import StrandsTelemetry


def setup_telemetry(use_sideseat: bool = False):
    """Initialize telemetry with standard configuration.

    Default: StrandsTelemetry with console, OTLP trace, and OTLP metrics.
    Optional: SideSeat SDK with automatic OTLP setup + file exporter.

    Also instruments boto3/botocore for AWS call tracing.
    """
    if use_sideseat:
        # SideSeat owns both Strands and Bedrock instrumentation in SDK mode.
        client = SideSeat(framework=[Frameworks.Strands, Frameworks.Bedrock])
        # client.telemetry.setup_file_exporter()
        client.telemetry.setup_console_exporter()
        return client
    else:
        # The raw OpenTelemetry branch uses the upstream botocore instrumentor.
        BotocoreInstrumentor().instrument()
        telemetry = StrandsTelemetry()
        telemetry.setup_console_exporter()
        telemetry.setup_otlp_exporter()
        telemetry.setup_meter(enable_otlp_exporter=True)
        return telemetry
