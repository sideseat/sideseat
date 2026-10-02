# Contributing to SideSeat

## Set up

[mise](https://mise.jdx.dev) installs the pinned toolchains (Node, Python, uv, .NET, cargo-nextest);
Rust comes from `rust-toolchain.toml` through rustup.

```bash
git clone https://github.com/sideseat/sideseat.git
cd sideseat
mise install          # or install the versions in mise.toml yourself
make setup            # dependencies and git hooks
make dev              # server on http://localhost:5388, UI with hot reload on http://localhost:5389
```

Supported Node versions: ^22.22.0 || ^24.0.0 || >=26.0.0. The Rust floor is `rust-version` in
`Cargo.toml`.

## The development loop

The loop is a requirement: keep it short.

| Command | What it runs | Budget |
| --- | --- | --- |
| `make quick` | format, lint, and unit tests for the areas you changed | under a minute |
| `make check` | every container-free gate | a few minutes |
| `make test-postgres`, `make test-clickhouse`, ... | backend parity in Docker | opt-in |

While iterating, run the narrowest command for what you touched:

```bash
cargo nextest run --locked -p sideseat-domain                     # one crate
cargo nextest run --locked -p sideseat-server --test message_goldens
uv run --locked --directory sdk/python pytest tests/test_client.py
npm --prefix web test -- --run src/components/thread
```

`make help` lists every command, grouped by area. The pre-commit hook checks only formatting, secrets,
and the file-length limit; the pre-push hook runs `make check`.

## Project structure

```
server/
  crates/core/          configuration, constants, shared utilities
  crates/ports/         storage and service traits, shared DTOs
  crates/domain/        SideML reconstruction, rules, retention, search
  crates/ingestion/     OTLP decoding, normalization, durability
  crates/query-sql/     typed analytical queries
  crates/api/           HTTP, gRPC, MCP, SSE, WebSocket
  crates/adapter-*/     databases, blob storage, cache, queues, secrets
  src/                  the composition root
  assets/rules/         what each framework's telemetry means - no framework knowledge lives in Rust
  tests/                message goldens, parity suites, repository invariants
web/                    React UI; web/src/components/ui is the design system
sdk/                    python/ js/ dotnet/ rust/
examples/               scenario suites per framework and the shared harness
docs/                   documentation site; docs/engineering/ holds internals
cli/                    the npm distribution wrapper
config/                 configuration schema and examples
deploy/                 container image and a local compose stack
make/                   Makefile fragments, one per area
packaging/              release metadata: Homebrew formula, macOS entitlements
scripts/                automation and benchmarks
specs/                  TLA+ specifications, checked by make harden-spec
tools/                  developer utilities: otel-replay, mcp-calculator, audit
```

[AGENTS.md](AGENTS.md) states the architecture rules and domain invariants every change must keep.

## Supporting a framework

1. Add a suite under `examples/python/<framework>` following [examples/README.md](examples/README.md).
2. Capture it: `make capture P=<framework>`.
3. Read the regenerated views. Where the conversation is wrong, fix the framework's rule asset in
   `server/assets/rules/`, the SDK integration, or - if the defect is general - the reconstruction, with a
   regression test.
4. Add or update the integration's documentation page under `docs/src/content/docs/docs/integrations/`.

## Code style

- Rust: `cargo fmt`, Clippy with `-D warnings`, `thiserror` in libraries and `anyhow` at boundaries.
- TypeScript: Prettier and ESLint; erasable syntax only (no enums, namespaces, or parameter
  properties). The web UI also runs `@shadcn/lint`: use the design tokens and component variants in
  `web/src/styles/index.css` and `web/src/components/ui/` instead of raw colors or arbitrary values.
- Python: Ruff and mypy (strict in the SDK).
- Comments explain constraints and tradeoffs, not what the code does or how it got that way.

## Pull requests

Keep each change focused, run `make check`, and explain why as well as what. Open an issue first for
large changes.

## Reporting issues

**Bugs** — Include steps to reproduce, expected vs actual behavior, version (`sideseat --version`), and OS. Use `SIDESEAT_LOG=debug` for verbose output.

**Features** — Describe the problem, proposed solution, and alternatives considered.

**Security** — Do not report publicly. Email support@sideseat.ai.

## Contributor License Agreement

By submitting a contribution, you agree to the following:

- **Copyright assignment** — You assign all copyright in your contribution to Sergey Pugachev for unified project ownership.
- **Patent license** — You grant a perpetual, worldwide, royalty-free patent license for your contribution.
- **Representations** — Your contribution is original work, doesn't violate third-party rights, and you have authority to submit it.
- **Waiver** — You waive claims against the project relating to your contribution.

This agreement is irrevocable once your contribution is merged.

## License

[Apache-2.0](LICENSE)
