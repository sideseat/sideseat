"""
OTEL Load Test - Generates synthetic Strands-like traces

Sends a configurable number of traces to test retention and performance.
Each trace simulates a Strands agent invocation with nested spans.

Usage:
    uv run loadtest                    # 1M spans (default)
    uv run loadtest --spans 100000     # 100K spans
    uv run loadtest --batch 5000       # 5K batch size
    uv run loadtest --workers 8        # 8 parallel workers
"""

from dotenv import load_dotenv

load_dotenv()

import argparse
import math
import random
import string
import time
import uuid
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field

from opentelemetry import trace
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
from opentelemetry.sdk.resources import Resource
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor
from tqdm import tqdm

OTLP_ENDPOINT = "http://127.0.0.1:5388/otel/default/v1/traces"

AGENT_NAMES = ["researcher", "analyst", "planner", "coder", "reviewer", "orchestrator"]
TOOL_NAMES = [
    "calculator",
    "web_search",
    "weather_forecast",
    "database_query",
    "api_call",
]
MODEL_IDS = ["claude-3-haiku", "claude-3-sonnet", "gpt-4-turbo", "gemini-pro"]
OPERATIONS = [
    "agent_invoke",
    "tool_call",
    "model_invoke",
    "parallel_fetch",
    "session_tracker",
]


@dataclass
class Stats:
    spans_sent: int = 0
    traces_sent: int = 0
    errors: int = 0
    start_time: float = 0.0
    last_report_time: float = 0.0
    last_report_traces: int = 0
    interval_rates: list = field(default_factory=list)


def random_string(length: int = 8) -> str:
    return "".join(random.choices(string.ascii_lowercase + string.digits, k=length))


def create_tracer_provider() -> TracerProvider:
    resource = Resource.create(
        {
            "service.name": "loadtest",
            "service.version": "1.0.0",
            "deployment.environment": "test",
        }
    )
    provider = TracerProvider(resource=resource)
    exporter = OTLPSpanExporter(endpoint=OTLP_ENDPOINT)
    processor = BatchSpanProcessor(
        exporter,
        max_queue_size=50000,
        max_export_batch_size=5000,
        schedule_delay_millis=1000,
    )
    provider.add_span_processor(processor)
    return provider


def generate_agent_trace(tracer: trace.Tracer, trace_num: int) -> int:
    """Generate a single agent trace with nested spans. Returns span count."""
    span_count = 0
    session_id = f"session-{uuid.uuid4().hex[:16]}"
    agent_name = random.choice(AGENT_NAMES)
    model_id = random.choice(MODEL_IDS)

    with tracer.start_as_current_span(
        f"agent.invoke.{agent_name}",
        attributes={
            "agent.name": agent_name,
            "session.id": session_id,
            "trace.number": trace_num,
            "gen_ai.system": "strands",
            "gen_ai.request.model": model_id,
        },
    ) as root_span:
        span_count += 1

        with tracer.start_as_current_span(
            "model.invoke",
            attributes={
                "gen_ai.request.model": model_id,
                "gen_ai.usage.input_tokens": random.randint(100, 2000),
                "gen_ai.usage.output_tokens": random.randint(50, 1000),
            },
        ):
            span_count += 1

        num_tools = random.randint(1, 4)
        for tool_idx in range(num_tools):
            tool_name = random.choice(TOOL_NAMES)

            with tracer.start_as_current_span(
                f"tool.call.{tool_name}",
                attributes={
                    "tool.name": tool_name,
                    "tool.index": tool_idx,
                    "tool.status": "success" if random.random() > 0.05 else "error",
                },
            ) as tool_span:
                span_count += 1

                if random.random() > 0.6:
                    with tracer.start_as_current_span(
                        f"tool.{tool_name}.nested",
                        attributes={
                            "nested.operation": random.choice(OPERATIONS),
                            "nested.data": random_string(32),
                        },
                    ):
                        span_count += 1

                if random.random() > 0.7:
                    tool_span.add_event(
                        "tool.result",
                        attributes={
                            "result.size": random.randint(10, 1000),
                            "result.type": random.choice(["json", "text", "binary"]),
                        },
                    )

        if random.random() > 0.3:
            with tracer.start_as_current_span(
                "model.invoke",
                attributes={
                    "gen_ai.request.model": model_id,
                    "gen_ai.usage.input_tokens": random.randint(500, 3000),
                    "gen_ai.usage.output_tokens": random.randint(100, 2000),
                },
            ):
                span_count += 1

        root_span.add_event(
            "agent.complete",
            attributes={
                "total_tools": num_tools,
                "response.length": random.randint(100, 5000),
            },
        )

    return span_count


def run_load_test(total_spans: int, batch_size: int, workers: int) -> None:
    print("OTEL Load Test")
    print(f"  Target spans: {total_spans:,}")
    print(f"  Batch size: {batch_size:,}")
    print(f"  Workers: {workers}")
    print(f"  Endpoint: {OTLP_ENDPOINT}")
    print()

    provider = create_tracer_provider()
    trace.set_tracer_provider(provider)

    stats = Stats(start_time=time.time(), last_report_time=time.time())

    avg_spans_per_trace = 5  # empirical average from generator shape
    estimated_traces = max(1, math.ceil(total_spans / avg_spans_per_trace))

    print(f"Estimated traces: {estimated_traces:,}")
    print()

    tracer = provider.get_tracer("loadtest")

    with tqdm(total=total_spans, unit="spans", desc="Generating") as pbar:
        with ThreadPoolExecutor(max_workers=workers) as executor:
            trace_num = 0

            while stats.spans_sent < total_spans:
                futures = []
                remaining_spans = total_spans - stats.spans_sent
                remaining_traces_estimate = max(1, math.ceil(remaining_spans / avg_spans_per_trace))
                batch_traces = min(batch_size, remaining_traces_estimate)

                for _ in range(batch_traces):
                    futures.append(executor.submit(generate_agent_trace, tracer, trace_num))
                    trace_num += 1

                for future in futures:
                    try:
                        spans = future.result()
                        stats.spans_sent += spans
                        stats.traces_sent += 1
                        pbar.update(min(spans, total_spans - pbar.n))
                    except Exception:
                        stats.errors += 1

                now = time.time()
                if now - stats.last_report_time >= 30:
                    interval_traces = stats.traces_sent - stats.last_report_traces
                    interval_time = now - stats.last_report_time
                    interval_rate = interval_traces / interval_time if interval_time > 0 else 0
                    stats.interval_rates.append(interval_rate)

                    overall_rate = stats.traces_sent / (now - stats.start_time)

                    tqdm.write(
                        f"\n[{time.strftime('%H:%M:%S')}] "
                        f"Interval: {interval_rate:,.0f} traces/sec | "
                        f"Overall: {overall_rate:,.0f} traces/sec | "
                        f"Total: {stats.traces_sent:,} traces ({stats.spans_sent:,} spans)"
                    )

                    stats.last_report_time = now
                    stats.last_report_traces = stats.traces_sent

    # Flush and shutdown
    print("\nFlushing telemetry...")
    provider.force_flush(timeout_millis=30000)
    provider.shutdown()

    elapsed = time.time() - stats.start_time
    overall_trace_rate = stats.traces_sent / elapsed if elapsed > 0 else 0
    overall_span_rate = stats.spans_sent / elapsed if elapsed > 0 else 0

    print()
    print("=" * 60)
    print("Load Test Complete")
    print("=" * 60)
    print(f"  Traces sent:    {stats.traces_sent:,}")
    print(f"  Spans sent:     {stats.spans_sent:,}")
    print(f"  Errors:         {stats.errors}")
    print(f"  Duration:       {elapsed:.1f}s")
    print(f"  Trace rate:     {overall_trace_rate:,.0f} traces/sec")
    print(f"  Span rate:      {overall_span_rate:,.0f} spans/sec")
    if stats.interval_rates:
        avg_rate = sum(stats.interval_rates) / len(stats.interval_rates)
        print(
            f"  Avg interval:   {avg_rate:,.0f} traces/sec (over {len(stats.interval_rates)} intervals)"
        )
    print("=" * 60)


def main() -> None:
    parser = argparse.ArgumentParser(description="OTEL Load Test")
    parser.add_argument(
        "--spans",
        type=int,
        default=1_000_000,
        help="Total number of spans to generate (default: 1,000,000)",
    )
    parser.add_argument(
        "--batch",
        type=int,
        default=1000,
        help="Traces per batch (default: 1,000)",
    )
    parser.add_argument(
        "--workers",
        type=int,
        default=4,
        help="Number of parallel workers (default: 4)",
    )
    args = parser.parse_args()

    run_load_test(args.spans, args.batch, args.workers)


if __name__ == "__main__":
    main()
