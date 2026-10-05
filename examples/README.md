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
├── javascript/
│   ├── harness/       one npm project: the same CLI, prompts, tools and models as Python
│   ├── <framework>/   one directory per framework, declared by its suite manifest
│   └── sdk-conformance/  the TypeScript SDK conformance program
├── cli/               coding-agent CLIs (Claude Code, Codex): one capture driver for both
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

make sample P=strands-js S=tool_use            # a TypeScript suite: its directory name plus -js
cd examples/javascript/strands && npm run sample -- tool_use --sideseat --model haiku
```

The TypeScript suites share one npm project: run `npm ci` in `examples/javascript` once (`make setup`
does), and `npm run sample` from a suite's directory. They need Node.js ^22.13.0 or 24 and later,
which is what their lockfile accepts (`make node-floor` measures it); the repository as a whole needs
^22.22.0 || ^24.0.0 || >=26.0.0.

Credentials and endpoints can also live in `examples/.env` (copy `.env.example`); the process
environment wins over it.

## Scenarios

Every suite implements the same scenarios with the same prompts and tools
(`harness/content.py`), so fixtures from different frameworks describe the same conversations. The
TypeScript harness reads them, with the scenario catalog and the model aliases, from
`javascript/harness/content.json`, which the capture tool renders from the Python harness
(`capture --export-content`) and a harness test keeps current. The tool bodies are re-implemented
in TypeScript and checked against the results the Python tools return before any scenario runs.

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
  SideSeat. The suite's `native.py` (or `native.ts`) configures it.
- **sdk** (`--sideseat`) replaces that with `sideseat.init(integrations=[...])`, or
  `await sideseat.init({ integrations: [...] })` in TypeScript.

Capturing both and comparing them is how the SDK is shown to lose nothing and add nothing wrong:
`framework_sdk_and_native_conversations_are_identical` requires the same spans, traces, sessions,
and messages from both.

## Capturing fixtures

```bash
make capture P=strands                 # every scenario, both modes, then regenerate expectations
make capture P=strands S=tool_use
make capture P=vercel-ai-js            # a TypeScript suite, captured the same way
make capture-offline P=strands         # replay committed model traffic; no credentials needed
```

The native run talks to Bedrock through a recording proxy and saves the model's responses to
`<suite>/cassettes/<scenario>.json`; the SDK run replays them byte for byte, so both runs hold the
same conversation. Providers that Bedrock does not serve (Gemini, Vertex AI, Azure OpenAI) run
their real client library against a `fake-*` model instead: a local server, started in-process,
that speaks the provider's wire format and answers the shared prompts with the shared tools
(`harness/fakes/`). Those captures need no credentials and no cassette. Fixtures land in `server/tests/fixtures/messages/<producer>/<native|sdk>/<scenario>/`.

A regenerated `expected.json` is a claim, not a result. Read every trace and session view against
the rubric in `server/tests/fixtures/messages/README.md` before committing it: every user question,
system prompt, tool call, tool result, and answer present, once, in the order they happened.

## Coding-agent CLIs

A CLI is a separate process configured by its own settings, so `examples/cli/capture.py` drives the
installed binary instead of a suite: one process per user turn, later turns resuming the session, with
the harness's recorder, anonymisation and recording proxy. Each run gets a fresh `HOME` (and
`CODEX_HOME`), a fixed workspace under `/tmp/sideseat-cli`, no inherited environment and no
credentials - the proxy signs for `bedrock-runtime` itself.

```bash
set -a; . ~/.aws/sideseat-capture.env; set +a   # credentials for the proxy only
uv run --locked --project examples/python/harness python examples/cli/capture.py claude-code
uv run --locked --project examples/python/harness python examples/cli/capture.py codex tool_use
uv run --locked --project examples/python/harness python examples/cli/capture.py codex --offline
```

`CLAUDE_BIN` and `CODEX_BIN` name binaries other than the ones on `PATH`. Scenarios: `tool_use` (read
`forecast.json`, run `wc -l`, answer), `multi_turn` (the shared three questions, each a resumed
process), `error` (a shell command that fails) and `multi_agent` (a sub-agent reads the file). Claude
Code is captured in two telemetry modes - `native`, the detailed tracing tier plus log events, and
`logs`, without the detailed tier - and Codex in one. Cassettes live in `cli/<tool>/cassettes/`.

## Adding a framework suite

The steps are the same in both languages. A TypeScript suite is a directory of
`examples/javascript` with a `suite.json` (`producer`, `integrations`, optionally `default-model`
and `service-name`), `native.ts`, `models.ts`, and `scenarios/<name>.ts`; its dependencies go in the
shared `package.json`. The Python steps:

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
