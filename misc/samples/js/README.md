# JavaScript Agent Samples

TypeScript samples for **Strands Agents** and **Vercel AI SDK** with AWS Bedrock.

## Prerequisites

- **Node.js**: >= 20.0.0
- **AWS credentials**: Configure via environment variables or AWS profiles
- **AWS permissions**: `bedrock:InvokeModel` for Claude, Titan Embeddings, Titan Image
- **Python**: Required for MCP sample only

## Quick Start

```bash
cd misc/samples/js
npm install

# Run a sample
npm run strands -- tool-use
npm run vercel-ai -- tool-use
```

## Configuration

### Environment Variables

Create `misc/.env` (shared with Python samples):

```bash
AWS_REGION=us-east-1
# AWS_ACCESS_KEY_ID=...      # Or use AWS profiles
# AWS_SECRET_ACCESS_KEY=...

# Optional: SideSeat telemetry
SIDESEAT_ENDPOINT=http://127.0.0.1:5388
SIDESEAT_PROJECT_ID=default

# Optional: Custom models
EMBEDDING_MODEL=amazon.titan-embed-text-v2:0
IMAGE_GEN_MODEL=amazon.titan-image-generator-v2:0
```

### Model Aliases

| Alias                     | Model ID                                          |
| ------------------------- | ------------------------------------------------- |
| `bedrock-haiku` (default) | `global.anthropic.claude-haiku-4-5-20251001-v1:0` |
| `bedrock-sonnet`          | `global.anthropic.claude-sonnet-4-20250514-v1:0`  |

Uses cross-region inference profiles (`global.` prefix) for on-demand access.

## Usage

### Strands Agents

```bash
npm run strands -- <sample> [options]

# Options:
#   --model=<alias>   Model alias or full ID (default: bedrock-haiku)
#   --sideseat        Enable SideSeat SDK telemetry
#   --list            List available samples and models
#   --help            Show help

# Examples:
npm run strands -- tool-use
npm run strands -- reasoning --model=bedrock-sonnet
npm run strands -- all --sideseat
```

### Vercel AI SDK

```bash
npm run vercel-ai -- <sample> [options]

# Same options as Strands

# Examples:
npm run vercel-ai -- structured-output
npm run vercel-ai -- multi-step --model=bedrock-sonnet
```

## Available Samples

| Sample              | Strands | Vercel AI | Description                                      |
| ------------------- | :-----: | :-------: | ------------------------------------------------ |
| `tool-use`          |   Yes   |    Yes    | Weather forecast tools with prompt caching       |
| `mcp-tools`         |   Yes   |     -     | MCP server integration (Python calculator)       |
| `reasoning`         |   Yes   |    Yes    | Chain-of-thought with extended thinking          |
| `structured-output` |   Yes   |    Yes    | Zod schema validation for structured data        |
| `rag-local`         |   Yes   |    Yes    | Vector search with Titan Embeddings              |
| `files`             |   Yes   |    Yes    | Multimodal image analysis                        |
| `image-gen`         |   Yes   |    Yes    | Titan Image generation with artist/critic agents |
| `swarm`             |   Yes   |     -     | Multi-agent orchestration with handoffs          |
| `multi-step`        |    -    |    Yes    | Agentic loop with step tracking                  |

### Sample Details

**tool-use**: Demonstrates tool definition with Zod schemas, multiple tool calls, and prompt caching behavior.

**mcp-tools** (Strands only): Connects to an MCP server (`misc/mcp/calculator.py`) via stdio transport.

**reasoning**: Solves logic puzzles, math problems, and code analysis. Strands version supports extended thinking with `--model=bedrock-sonnet` (budget: 4096 tokens).

**structured-output**: Extracts structured person data using Zod schemas. Vercel AI uses native `generateObject()`, Strands uses tool-based approach.

**rag-local**: Indexes documents with Titan Embeddings, performs vector similarity search, and augments LLM responses with retrieved context.

**files**: Analyzes images using multimodal content blocks. PDF support pending SDK updates.

**image-gen**: Artist agent generates images via Titan Image, critic agent evaluates and selects the best.

**swarm** (Strands only): Planner agent routes tasks to specialist agents (researcher, coder, reviewer) via tool-based handoffs.

**multi-step** (Vercel AI only): Demonstrates `generateText` with `maxSteps` for agentic loops with automatic tool execution.

## Telemetry

Telemetry is always enabled via AWS SDK instrumentation. Use `--sideseat` to send traces to SideSeat:

```bash
# Start SideSeat server first
make dev-server

# Run with SideSeat telemetry
npm run strands -- tool-use --sideseat
```

## Development

### Code Quality

```bash
npm run typecheck      # TypeScript type checking
npm run lint           # ESLint
npm run format:check   # Prettier check
npm run format         # Prettier fix
```

### SDK Development

The SideSeat SDK is loaded directly from source via TypeScript path mapping. No build required - changes to `sdk/js/src/` are picked up immediately.

### Project Structure

```
misc/samples/js/
├── src/
│   ├── shared/           # Shared utilities
│   │   ├── config.ts     # Environment config, model aliases
│   │   ├── aws-client.ts # Singleton BedrockRuntimeClient
│   │   ├── telemetry.ts  # SideSeat/OTEL setup
│   │   ├── response.ts   # Response extraction utilities
│   │   └── tools/        # Shared tools (embeddings, image-gen)
│   ├── strands/          # Strands Agents samples
│   │   ├── runner.ts     # CLI runner
│   │   └── samples/      # Sample implementations
│   └── vercel-ai/        # Vercel AI SDK samples
│       ├── runner.ts     # CLI runner
│       └── samples/      # Sample implementations
├── output/               # Generated images (gitignored)
├── package.json
└── tsconfig.json
```

## Troubleshooting

**MCP server not found**: Run from `misc/samples/js` directory, ensure Python is in PATH.

**Image/PDF not found**: Run from `misc/samples/js` directory. Content files are in `misc/content/`.

**AWS credentials**: Ensure `AWS_REGION` is set. Use `aws configure` or set `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`.

**Extended thinking not working**: Only supported on `bedrock-sonnet` and `bedrock-haiku` aliases. The runner automatically enables it for the reasoning sample.
