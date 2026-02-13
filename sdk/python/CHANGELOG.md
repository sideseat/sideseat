# Changelog

All notable changes to the SideSeat Python SDK will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
