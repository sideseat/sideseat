# Changelog

All notable changes to the SideSeat Python SDK will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.0.8] - 2026-03-13

### Added

- Microsoft Agent Framework support with full OTel span parsing
- Universal logfire abstract method patch via `_wrap_logfire_instruments()` — auto-applies version-skew fixes for all `logfire.instrument_*` calls at import time

### Fixed

- CrewAI instrumentation: skip OpenInference dependency check that blocked instrumentation on minor version skew
- Logfire wrapper classes with unresolved abstract methods (`LogfireTraceWrapper`, `LogfireSpanWrapper`) now patched automatically on any `logfire.instrument_*` call

## [1.0.7] - 2025-02-15

### Added

- Bedrock instrumentation: Converse, ConverseStream, InvokeModel, and InvokeModelWithResponseStream APIs with full OTel span attributes
- Bedrock Agent Runtime instrumentation: InvokeAgent and InvokeInlineAgent
- OpenAI and Anthropic provider instrumentation via logfire with streaming trace reparenting fix (`_LogfireStreamingProcessor`)
- VertexAI provider migrated to Google GenAI SDK with logfire instrumentation
- `client.trace(session_id=)` for session grouping
- Tool definitions captured as span attribute fallback

### Changed

- `client.session()` removed — use `client.trace(session_id=)` instead

## [1.0.6] - 2025-01-30

### Added

- Bedrock provider support (`Frameworks.Bedrock`) via custom botocore instrumentation (`AWSInstrumentor`)
- `AWSInstrumentor` patches `ClientCreator.create_client` via wrapt for zero-config Bedrock tracing

## [1.0.5] - 2025-01-26

### Added

- `SideSeat` client with zero-config initialization
- Automatic framework detection: Strands, LangChain, CrewAI, AutoGen, OpenAI Agents, Google ADK, PydanticAI
- OTLP export for traces, metrics, and logs
- Dual initialization paths: standard mode (own TracerProvider) and Logfire mode
- `Frameworks` constants for explicit framework selection
- Global instance management via `init()`, `get_client()`, `shutdown()`, `is_initialized()`
- Context manager protocol for automatic shutdown
- `Config` immutable dataclass with env var fallback chain
- Environment variable support: `SIDESEAT_ENDPOINT`, `SIDESEAT_API_KEY`, `SIDESEAT_PROJECT`, `SIDESEAT_DISABLED`, `SIDESEAT_DEBUG`
- OTEL env var compatibility: `OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_PYTHON_LOGGING_AUTO_INSTRUMENTATION_ENABLED`
- `TelemetryClient` for advanced configuration
- `JsonFileSpanExporter` for JSONL trace output
- Console exporter for debugging
- `encode_value()` utility for base64 binary encoding
- `span_to_dict()` for span serialization
- `patch_strands_encoder()` for Strands binary encoding
- `get_otel_resource()` for OTEL resource creation
- Thread-safe instrumentation guards
- Graceful shutdown with force flush
- Connection validation via `validate_connection()`
- Custom span creation via `span()` context manager with automatic error status
