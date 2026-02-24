"""Error handling sample — calls Bedrock with an invalid model ID.

Demonstrates:
- Error telemetry capture (exception type, message, stacktrace)
- Error span status in traces
"""


def run(bedrock, trace_attrs: dict):
    """Run converse with an invalid model ID to generate error telemetry."""
    import boto3

    region = "us-east-1"
    client = boto3.client("bedrock-runtime", region_name=region)

    client.converse(
        modelId="nonexistent-model-id-12345",
        messages=[{"role": "user", "content": [{"text": "Hello"}]}],
    )
