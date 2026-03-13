"""Telemetry setup for Bedrock samples."""

from sideseat import Frameworks, SideSeat


def setup_telemetry():
    """Initialize telemetry for Bedrock samples.

    SideSeat patches boto3 to capture Bedrock API calls (converse, invoke_model,
    converse_stream, invoke_model_with_response_stream) with full message events.
    """
    client = SideSeat(framework=Frameworks.Bedrock)
    client.telemetry.setup_console_exporter()
    return client
