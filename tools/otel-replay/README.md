# otel-replay

Replays captured OTLP debug files into a running SideSeat server, so a trace can be re-ingested without
re-running the framework that produced it - which is what makes an ingestion or reconstruction fix testable
against real telemetry.

Its documentation used to live in `examples/README.md`, back when that file was a `misc/` grab-bag describing
everything in the directory. A tool documents itself.

Accepts `.jsonl`, `.jsonl.gz` and `.zip`.

```bash
uv run --locked --directory tools/otel-replay replay traces-strands.jsonl.gz
uv run --locked --directory tools/otel-replay replay traces-adk.jsonl.gz
uv run --locked --directory tools/otel-replay replay traces-vercel.jsonl.gz
uv run --locked --directory tools/otel-replay replay traces-langgraph.jsonl.gz
uv run --locked --directory tools/otel-replay replay traces-autogen.jsonl.gz
uv run --locked --directory tools/otel-replay replay traces-crewai.jsonl.gz

# Absolute path or custom server URL
uv run --locked --directory tools/otel-replay replay /path/to/file.jsonl
uv run --locked --directory tools/otel-replay replay traces-autogen.jsonl.gz --base-url http://localhost:5388
```

Load generation:

```bash
uv run --locked --directory tools/otel-replay generate_load --spans 100000 --workers 5
```
