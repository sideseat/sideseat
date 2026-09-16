# Contributing to SideSeat

## Prerequisites

- Rust 1.94.1+ (declared in `Cargo.toml`; cargo, clippy and a CI job all hold it)
- Node.js 22.22+ or 24+ (measured across the lockfiles; CI uses 24)
- [uv](https://docs.astral.sh/uv/)
- Make

## Setup

```bash
git clone https://github.com/sideseat/sideseat.git
cd sideseat
make setup
make dev
```

Dev server runs at http://localhost:5389 (UI) and http://localhost:5388 (API).

## Project Structure

```
crates/       Layer crates whose manifests enforce the boundary: core/ ports/
              The first holds configuration, constants, storage layout, the CLI and pure utilities; the
              second the traits, DTOs, filter vocabulary and error type. Neither names a driver, so a
              forbidden dependency does not compile rather than being caught in review
server/       Rust backend (Axum): Cargo.toml, build.rs, src/, tests/, assets/, proptest-regressions/
web/          React frontend (Vite)
cli/          npm distribution wrapper
sdk/          Client SDKs: python/ js/ rust/ (dotnet/ reserves the NuGet name; no implementation)
config/       Product configuration: JSON schema and example files
examples/     Runnable samples per framework, with their inputs
tools/        Standalone developer utilities: otel-replay/ mcp-calculator/ audit/
scripts/      Repository automation and performance measurement
packaging/    Release metadata: homebrew formula, macOS entitlements
deploy/       Container image and a local compose stack
specs/        TLA+ specifications, checked by `make harden-spec`
docs/         Public documentation site (Astro), plus docs/engineering/ (internal architecture and the ws-v1 wire protocol)
```

`server/assets/` holds everything compiled into the binary — the framework rule assets and the
pricing catalogue. `docs/engineering/` holds the internal architecture documents; the Astro site
builds from `docs/src/` only, so they are not published.

## Development Commands

```bash
make dev                                  # Start server + web in parallel
make dev-server                           # Start Rust server with hot reload
make dev-server ARGS="--debug --no-auth"  # Server without authentication
make dev-web                              # Start Vite dev server
make test                                 # Run all tests
make fmt                                  # Format code
make lint                                 # Lint code
make check                                # fmt-check + lint + test
make build                                # Production build
```

To generate test traces, run `uv run --locked --directory examples/python/strands strands tool_use
--sideseat`. `--locked` everywhere: a bare `uv run` rewrites that suite's lockfile to match a drifted
manifest, and `make update-python-deps` is the one command meant to do that.

### Release Workflow

```bash
make release TYPE=patch  # check, bump, commit, tag, push
make build-cli           # cross-compile all platforms
make build-release       # create archives + notarize + checksums
make build-docker        # build Docker image for current platform
make publish-cli         # publish platform packages + main package to npm
make publish-sdk-js      # publish JS SDK to npm
make publish-sdk-python  # publish Python SDK to PyPI
make publish-release     # upload to GitHub Releases
make publish-docker      # multi-arch build + push to registry
make publish-brew        # update Homebrew tap formula
```

Homebrew formula template: `packaging/homebrew/sideseat.rb.tmpl`. Users install via `brew tap sideseat/tap && brew install sideseat`.

## Code Style

### Rust

- Format with `cargo fmt`
- Lint with `cargo clippy` — no warnings allowed
- Use `thiserror` for library errors, `anyhow` for application errors
- Use `Arc<T>` + `parking_lot` for shared state
- Comments: minimal, explain why not what

### TypeScript

- Format with `npm run fmt`
- Lint with `npm run lint`
- No enums — use `as const`
- Never modify `web/src/components/ui/` (shadcn/ui)
- No constructor parameter properties:

```typescript
// Do this
private client: ApiClient;
constructor(client: ApiClient) { this.client = client; }

// Not this
constructor(private client: ApiClient) {}
```

## Pull Requests

1. Open an issue first for significant changes
2. Create a feature branch: `git checkout -b fix/issue-123`
3. Keep changes focused — one issue per PR
4. Run `make check` before committing (runs fmt-check + lint + test)
5. Write a clear description explaining why and what
6. Reference related issues

## Reporting Issues

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
