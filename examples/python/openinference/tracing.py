"""The OpenInference tracer whose decorators mark the scenarios' agents, chains, and tools."""

from openinference.instrumentation import OITracer, TraceConfig
from opentelemetry import trace

# A proxy until telemetry is configured, then the configured provider's tracer.
tracer = OITracer(trace.get_tracer("example.openinference"), TraceConfig())
