# SideSeat

**AI Development Workbench** — Debug, trace, and understand your AI agents.

[![NuGet](https://img.shields.io/nuget/v/SideSeat)](https://www.nuget.org/packages/SideSeat)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)

SideSeat captures every LLM call, tool call, and agent decision, then displays them in a web UI as they happen. Built on [OpenTelemetry](https://opentelemetry.io/).

## Status

**This package holds the `SideSeat` name on NuGet and contains no implementation** — one version constant and
nothing else. There is no .NET client to install yet, and the repository does not build, test or publish it;
`sdk/python`, `sdk/js` and `sdk/rust` are the real SDKs.

Until there is one, .NET applications can be traced with any OpenTelemetry exporter pointed at a running
workbench, which needs no SDK at all:

```bash
npx sideseat   # the workbench, UI and API together on http://localhost:5388

# `http/protobuf` because .NET's OTLP exporter defaults to gRPC, and the *base* endpoint because the SDK
# appends `/v1/traces` itself. Use OTEL_EXPORTER_OTLP_TRACES_ENDPOINT if you would rather give the full path.
export OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:5388/otel/default
```

See [sideseat.ai/docs](https://sideseat.ai/docs) for the exporter configuration and full documentation.

## Resources

- [Documentation](https://sideseat.ai/docs)
- [GitHub](https://github.com/sideseat/sideseat)
- [Issues](https://github.com/sideseat/sideseat/issues)

## License

[MIT](LICENSE)
