"""Telemetry setup for Bedrock samples."""

from sideseat import Frameworks, SideSeat


def setup_telemetry(use_sideseat: bool = False):
    """Initialize telemetry for Bedrock samples.

    SideSeat patches boto3 to capture Bedrock API calls (converse, invoke_model,
    converse_stream, invoke_model_with_response_stream) with full message events.
    """
    if use_sideseat:
        client = SideSeat(framework=Frameworks.Bedrock)
        # client.telemetry.setup_file_exporter()
        client.telemetry.setup_console_exporter()
        return client

    # Without SideSeat, still patch Bedrock for basic tracing
    client = SideSeat(framework=Frameworks.Bedrock)
    client.telemetry.setup_console_exporter()
    return client
