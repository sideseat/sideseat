# Examples

Runnable scenarios for every framework and provider SideSeat supports. They serve two purposes:
showing how each integration is set up, and producing the OTLP fixtures the message goldens
(`server/tests/message_goldens.rs`) replay to prove that SideSeat reads every trace, span, and
session correctly.

```
examples/
├── assets/            the image and PDF the files scenario sends
├── data/              tabular input for samples that read a dataset
├── python/
│   ├── harness/       shared CLI, model catalog, prompts and tools, telemetry modes, capture tool
│   └── <framework>/   one uv project per framework or provider
├── javascript/        TypeScript suites
└── dotnet/            .NET conformance program
```

## Running a scenario

Every live model runs on Amazon Bedrock, so AWS credentials in the default chain are all you need.
Start SideSeat (`make dev-server`), then:

```bash
make sample P=strands S=tool_use               # native telemetry, as Strands documents it
make sample P=strands S=tool_use SIDESEAT=1    # the same program with sideseat.init
make sample P=strands S=tool_use MODEL=haiku   # another model from the catalog

uv run --locked --directory examples/python/strands sample --list
```

Credentials and endpoints can also live in `examples/.env` (copy `.env.example`); the process
environment wins over it.

## Scenarios

Every suite implements the same scenarios with the same prompts and tools
(`harness/content.py`), so fixtures from different frameworks describe the same conversations.

| Scenario | Proves | Required |
| --- | --- | --- |
| `chat` | system prompt, one question, one answer | yes |
| `multi_turn` | three questions in one conversation; history re-sent each request | yes |
| `tool_use` | two tools called several times; results feed the answer | yes |
| `session` | two traces that belong to one session and one user | yes |
| `error` | a tool raises; the model sees the error and answers | yes |
| `streaming` | a streamed answer that calls a tool | when supported |
| `structured_output` | an answer constrained to a schema | when supported |
| `reasoning` | visible thinking before the answer | when supported |
| `files` | an image and a PDF in the user turn | when supported |
| `multi_agent` | agents handing work to each other | when supported |
| `mcp_tools` | tools served by an MCP server | when supported |

## Telemetry modes

- **native** is what a user of the framework writes by following the framework's documentation:
  plain OpenTelemetry plus whatever the framework needs switched on. It imports nothing from
  SideSeat. The suite's `native.py` configures it.
- **sdk** (`--sideseat`) replaces that with `sideseat.init(integrations=[...])`.

Capturing both and comparing them is how the SDK is shown to lose nothing and add nothing wrong:
`framework_sdk_and_native_conversations_are_identical` requires the same spans, traces, sessions,
and messages from both.

## Capturing fixtures

```bash
make capture P=strands                 # every scenario, both modes, then regenerate expectations
make capture P=strands S=tool_use
make capture-offline P=strands         # replay committed model traffic; no credentials needed
```

The native run talks to Bedrock through a recording proxy and saves the model's responses to
`<suite>/cassettes/<scenario>.json`; the SDK run replays them byte for byte, so both runs hold the
same conversation. Fixtures land in `server/tests/fixtures/messages/<producer>/<native|sdk>/<scenario>/`.

A regenerated `expected.json` is a claim, not a result. Read every trace and session view against
the rubric in `server/tests/fixtures/messages/README.md` before committing it: every user question,
system prompt, tool call, tool result, and answer present, once, in the order they happened.

## Adding a framework suite

1. Copy `python/strands` and rename it. Set `[tool.sideseat-example]` in `pyproject.toml`:
   `producer` (the fixture directory name), `integrations` (what `--sideseat` passes to
   `sideseat.init`).
2. Depend on the latest stable release of the framework. Pin nothing below what the framework
   itself requires.
3. `native.py` configures the framework's documented telemetry; `models.py` maps a catalog model
   to the framework's model object; `tools.py` wraps the shared tools.
4. Implement every required scenario and each optional one the framework supports, using the shared
   prompts.
5. `make capture P=<producer>`, read the views, and fix what is wrong where it is wrong: the SDK
   integration in `sdk/python/src/sideseat/integrations/`, the producer's rule asset in
   `server/assets/rules/producers/`, or the scenario.
6. Update the producer's rows in the support matrix in `server/tests/fixtures/messages/README.md`.
