# SideSeat agent guide

This file applies to the entire repository. A nested `AGENTS.md` may add narrower instructions for its
subtree. `CLAUDE.md` only imports this file; edit this one.

## Product

SideSeat is an OpenTelemetry observability workbench for AI applications. It stores traces, metrics, and
logs, reconstructs each framework's conversations as SideML, and serves them through HTTP, gRPC, MCP, SSE,
WebSocket, and the web application. The SDKs (`sdk/python`, `sdk/js`, `sdk/dotnet`, `sdk/rust`) configure
OpenTelemetry export for an application; they carry no parsing logic of their own.

## Hard requirement: byte-level efficiency, total correctness, minimal resources

This is what distinguishes SideSeat. Every change is judged on all three, and none is traded for another.

- **Bytes.** Storage, memory, and wire formats are optimised at the byte level. Targets are absolute, so a
  corpus's share of media cannot flatter them: at most 480 bytes stored per span, 300 per log record, and 150
  per metric point, excluding media, which is stored once per project, losslessly, and reported separately (the
  baseline was 13,230 bytes per span); and sustained ingest of at least 10,000 spans per second on one Ampere A1
  core (aarch64, Neoverse N1) with the server and its embedded backend inside 2 GB of RAM, with the throughput
  curve over 1, 2, 4 and 8 cores measured and the stored bytes and every answer identical whatever the core
  count. Each figure has a regression gate, and each release should improve on it. Redundancy is a defect:
  repeated attributes, re-sent message history, JSON text, and inline media are encoded once, by reference,
  with dictionaries and compact binary encodings. Prefer the simplest change that reaches a target over a
  redesign.
- **Correctness.** 100 %, proven rather than assumed. Raw telemetry round-trips byte for byte, every
  reconstructed conversation matches its truth, and an optimisation that changes any golden, truth, or parity
  answer is wrong until shown otherwise.
- **Resources.** Memory is bounded by design, not by luck: streaming over buffering, fixed-size caches,
  back-pressure instead of unbounded queues, no per-request allocation that grows with history. CPU and
  disk work is proportional to new data, never to what is already stored.
- **Measured, never estimated.** A claim about size, speed, or memory comes with a reproducible measurement
  (`make footprint`, `make bench-http`, `scripts/perf/`) taken before and after the change. Regression gates fail
  the build when a measured figure gets worse.

## Repository map

```
server/                 Rust workspace root for the backend
  crates/<layer>/       ports-and-adapters crates, see Architecture
  src/                  composition root: adapter selection, wiring, process lifecycle
  assets/rules/         every framework's telemetry semantics, as declarative JSON
    conventions/        published conventions: OTel GenAI semconv, generic input/output
    producers/          one asset per framework or provider
    vocabulary/         cross-framework tables: span fields, content blocks, roles, categories
  tests/                message goldens, backend parity suites, repository invariants
    fixtures/messages/  captured OTLP per <framework>/<mode>/<scenario>, with the parsing rubric (README.md)
    fixtures/metrics/   captured metric exports for the storage corpus (`harness capture --metrics`)
  specs/                TLA+ models of ordering, carrier claiming and invocation flow
web/                    React UI; web/src/components/ui is the shadcn design system
sdk/                    python/ js/ dotnet/ rust/
examples/               one suite per framework per language, on a shared scenario harness
docs/                   src/ is the user documentation site; engineering/ holds internals
cli/                    the npm distribution wrapper
config/                 configuration JSON schema and examples
make/                   Makefile fragments, one per area; `make help` lists every target
scripts/                automation grouped by purpose; scripts/README.md maps it
  check/ hooks/ test/ perf/ fixtures/ dev/ release/ ops/ deploy/ tools/
```

Where a new thing goes:

- Framework telemetry knowledge: a rule asset under `server/assets/rules/`. Never Rust.
- A framework example: `examples/<language>/<framework>/`, built on the harness in
  `examples/python/harness` (or `examples/javascript/harness`), with `native/` and `sdk/` modes.
- A script: the `scripts/` subdirectory of its purpose, wired to a `make` target. Nothing loose in `scripts/`.
- A standalone developer utility with its own environment: `scripts/tools/<name>/`.
- A container image or compose stack: `scripts/deploy/`.
- A formal model: `server/specs/<Name>.tla` with a matching `<Name>.cfg`; `make harden-spec` checks every pair
  with the TLA+ tools it downloads into `scripts/tools/tla/` (gitignored, digest-checked).
- Internals documentation: `docs/engineering/`. User-facing behaviour: `docs/src/`.

Repository invariants in `server/tests/repository*.rs` hold this layout to account: the documented structure
must match the tree, every cited path must resolve, and `scripts/` must stay grouped.

## Architecture

The Rust workspace follows ports and adapters:

- `server/crates/core`: configuration, constants, CLI types, and shared utilities. It must not depend on
  SideSeat crates or infrastructure drivers.
- `server/crates/ports`: storage and service traits plus shared DTOs. It contains no adapter
  implementations or SQL.
- `server/crates/domain`: SideML, the rules engine, retention, search, files, storage governance, and
  restore workflows.
- `server/crates/ingestion`: OTLP decoding, normalization, signal identity, staging, durability, and
  persistence orchestration.
- `server/crates/messaging`: typed stream and broadcast messaging over the queue port.
- `server/crates/query-sql`: typed analytical queries and backend-specific lowering.
- `server/crates/rule-assets`: deterministic embedding of the rule JSON. It contains no interpretation
  logic or dependencies on other SideSeat crates.
- `server/crates/api`: HTTP, gRPC, MCP, SSE, and WebSocket transport code.
- `server/crates/adapter-*`: implementations for databases, blobs, cache, secrets, registrations, queues,
  and model-provider credential probes.
- `server`: the composition root. It selects adapters, wires services, starts background work, and owns
  process lifecycle.

Dependencies point inward. API and ingestion use messaging rather than a queue adapter; messaging depends
only on ports. Ingestion depends on domain and ports; domain talks to ports and consumes rule assets, never
an adapter. Adapters do not import sibling adapters. The server may depend on all layers because it
assembles the application.

Detailed architecture belongs in `docs/engineering/`: `architecture-diagrams.md`,
`ingestion-architecture.md`, `framework-rules-engine.md`, `sdk-contract.md`. Do not turn this file into a
second architecture manual.

## Hard requirement: Rust knows no framework

Rust code is a generic interpreter of declarative rules. It must not know anything specific to a
framework: no framework names, no framework attribute keys, span names, scope names, payload shapes, role
spellings, or "if this producer then" branches. All of that lives in the JSON assets under
`server/assets/rules/`, and the engine reads it.

- To support a framework or fix its parsing, change its asset. If the rule language cannot express what the
  telemetry means, extend the language in `domain::rules` generically - a new operator any asset can use -
  and then use it from the asset.
- Rust may name only published conventions (OpenTelemetry semantic conventions, generic `input.value` and
  `output.value`), SideSeat's own vocabulary (`sideseat.*`), and model providers where it prices or connects
  to them.
- Product-facing catalogues may list supported frameworks, such as the MCP setup guide, but must not
  control extraction.
- Two tests enforce this, and neither may be weakened to make a change pass:
  `no_production_module_names_a_framework` (framework names and aliases) and
  `no_production_module_spells_a_framework_attribute_key` (every key a non-convention asset declares). Test
  code and `#[cfg(test)]` equivalence oracles are exempt; production code is not.

## Domain invariants

- Preserve raw telemetry during ingestion. SideML role derivation, normalization, history detection, and
  deduplication happen at read time.
- Raw telemetry is the authority and is stored exactly once, completely and losslessly (byte-exact round
  trip, media by content hash). Everything derived from it - extracted columns, message views, search terms -
  is a cache that a re-derivation rebuilds identically from the raw store, so a parsing defect is fixed by
  re-parsing rather than lost. Storage is minimised by never keeping a second copy of raw content, not by
  keeping less of it.
- Every reconstructed conversation must satisfy the rubric in `server/tests/fixtures/messages/README.md`:
  complete messages, correct roles, no duplicates, tool calls before their results, and the same order for
  spans, traces, and sessions. The native and SDK modes of a framework must reconstruct identically. Since
  rubric v2 each fixture is also checked against its parser-independent truth (`fixtures/truth/`) on full
  content; a remaining violation lives in the shrink-only `known-violations.json`, never in a new exemption.
- No new databases or storage systems. Data lives in the ones SideSeat already has: SQLite and DuckDB
  embedded; PostgreSQL and ClickHouse distributed; the blob store (filesystem or S3-compatible) for files,
  media and raw content; Redis or Redpanda for the ingestion queue; and the existing secret backends. New
  tables, columns, encodings and blob layouts inside them are fine; a new engine, file format with its own
  server, or external service is not.
- No backward compatibility for stored data. Every backend is at schema version 2, created from scratch; there
  is no migration chain, dual read, or legacy format. A store at any other version is refused at startup with
  a message naming the found and supported versions and the explicit reset command; data is never migrated,
  silently ignored, or deleted without that explicit action.
- Tenant-scoped APIs use `ProjectId`; client-provided trace and span IDs are not globally unique.
- Analytics writes and transactional writes are not one transaction. Preserve the existing fences,
  tombstones, journal, confirmation, and compensation protocols when changing either side.
- A successful ingest response must not acknowledge data before its configured durability boundary.
- File and body ownership is reference-based. Do not weaken `pending_writers`, `durable`, legal-hold, or
  restore-reconciliation semantics.
- Embedded and distributed backends must return equivalent public answers where the capability is shared.
  Add parity coverage for backend-specific changes.
- Everything must work with horizontally scaled, ephemeral server instances: no correctness may depend on
  process-local state.

## Examples, fixtures and models

- The golden fixtures are captured from the example suites, never written by hand:
  `make capture P=<framework>` records live runs through a recording proxy; `make capture-offline P=...`
  replays them. Review the regenerated views with `scripts/fixtures/review-goldens.py`.
- Tests replay fixtures and use the in-process fake model servers. Live model calls happen only in
  `make capture`.
- Live captures reach Claude through Amazon Bedrock on the default AWS credential chain, and only call
  `bedrock-runtime`. A run that would record a credential is discarded; never commit one.
- Use the latest models. The catalogue is `examples/python/harness/harness/models.py`: Claude Sonnet 5.5 by
  default, Opus 5.5, Haiku 4.5, and the current GPT model on Bedrock's OpenAI endpoint. Update the catalogue,
  not individual suites.

## Code conventions

- Prefer small cohesive modules with names from the problem domain. Source files stay under 1000 lines;
  the pre-commit hook enforces it.
- Keep public APIs narrow. Do not add compatibility re-exports. SDKs carry no backward-compatibility shims.
- Use `thiserror` in libraries and `anyhow` at application boundaries.
- Explain constraints and non-obvious tradeoffs in comments. Do not narrate the editing process, repeat the
  code, preserve review history, or record temporary measurements in source comments.
- Public APIs receive useful rustdoc. Private code receives comments only when the reason cannot be
  expressed through naming and structure.
- Rust must remain warning-free under the workspace lint configuration.
- TypeScript uses erasable syntax: no enums, namespaces, or constructor parameter properties.
- In `web/`, use the tokens in `web/src/styles/index.css` and the variants in `web/src/components/ui/`
  instead of raw colours, arbitrary values, inline styles, or restyled components; `npm --prefix web run
  lint` runs the `@shadcn/lint` rules.
- Do not hand-edit generated files, lockfiles, captured fixtures, or `web/src/components/ui/` unless the
  task specifically targets their generator or source.
- Keep changes cross-platform where the surrounding component supports macOS, Linux, and Windows.
- Commit messages follow Conventional Commits with a scope: `fix(rules): ...`, `perf(quick): ...`,
  `test(goldens): ...`. The body says why.

## Dependencies

- Use `--locked` for commands that resolve dependencies.
- Update Rust dependencies deliberately, then inspect `Cargo.lock`.
- `make update-python-deps` is the only workflow that intentionally rewrites Python lockfiles.
- Use package-local Node tooling; the repository has no root Node package.
- `mise` pins the toolchains: Node, Python, uv, .NET, cargo-nextest.
- Rust libraries, by default: `thiserror` for every library crate's errors and `anyhow` only in the server
  binary and tests; `serde` for serialisation; `tracing` for events and spans, with the subscriber installed
  only by the server binary, never by a library or the SDK. Use `strum` when an enum has a string form that is
  not just its JSON one, or variants must be enumerated - when the strings exist only for JSON, `serde`'s
  `rename_all` is enough. Use `itertools` where it removes real code, not to turn loops into long adapter
  chains. Situational: `derive_more` for `From`/`Display`/arithmetic on newtypes (never `Deref`, which erases
  the type boundary), `bon` only for constructors with many optional parameters. Not used: `miette` and
  `color-eyre`, which suit compilers and CLIs rather than a service.

## Hard requirement: the fastest possible developer loop

The developer loop is a product requirement, not a convenience. Every change must keep it short.

- `make quick` is the inner loop. It must finish in under 60 seconds on a warm cache. It runs formatting,
  lint, and unit tests for the areas changed relative to `main`, and nothing else.
- While iterating, run only the narrowest command for the code you touched: one crate
  (`cargo test -p <crate> --lib`), one package (`uv run --locked pytest <path>`, `npm test -- <file>`), one
  golden (`cargo nextest run -p sideseat-server message_goldens -E 'test(<suite>)'`). Never start a
  workspace-wide build or test run to check a local change.
- Tests are deterministic and offline by default: replay captured OTLP fixtures and use the fake model
  servers. Live model calls happen only in `make capture`, never in `make quick`, `make test`, or a hook.
- A new test, gate, hook, or dependency that makes `make quick` slower than the budget belongs in
  `make test` or an opt-in target instead. Measure it before adding it.
- Git hooks stay cheap: pre-commit runs formatting, the file-length check, and the secret scan only; heavier
  checks belong to pre-push, which runs `make check`; push with `make push`, which keeps the SSH connection
  alive while that runs. There is no hosted CI: the hooks and the make targets
  are the gates. Never bypass them (`--no-verify`): a hook that fails on someone else's
  work in progress is a reason to wait for or fix that work, not to commit unchecked.
- If the loop has become slow, fixing that takes priority over the feature you are working on.

## Verification

Run `make quick` while iterating, then the broader gate for the affected surface:

```bash
make quick
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo nextest run --locked --workspace
make fmt-check
make lint
make test
make check
```

Backend and operational checks are opt-in because they start containers, release binaries, or model
checkers:

```bash
make test-postgres
make test-clickhouse
make test-clickhouse-replicated
make test-clickhouse-two-shard
make test-redis
make test-redpanda
make test-backup-restore
make footprint
make bench-http
make bench-http-distributed
make harden-spec
make audit        # supply-chain advisories; needs the network; before a release and periodically
make msrv         # the whole workspace on the minimum supported Rust version
```

Choose checks proportionally:

- Storage or SQL changes: relevant unit tests plus both affected parity suites.
- Message reconstruction or rules: golden fixtures and invariants, and both framework-knowledge sweeps.
- Ordering, claiming, or invocation protocol changes: update the TLA+ model and run `make harden-spec`.
- API changes: server tests and the matching SDK/web tests.
- Scripts or Makefile: syntax/static checks and at least one representative target.
- Documentation or repository layout: repository structural tests.

The container-free aggregate does not substitute for live backend parity.

## Working practices

- Inspect `git status` before editing and preserve unrelated user changes.
- Use `rg` and `rg --files` for repository searches.
- Keep commits focused and independently reviewable.
- When several agents share one working tree, keep it compiling: work in slices that build, run
  `cargo check --workspace --all-targets` before pausing, and park a slice that cannot compile yet outside the
  tree rather than leaving it in place - everyone else's checks run in the same tree.
- When several agents share one working tree, never stage in the shared index: commit with
  `scripts/dev/commit-paths.sh -m "<message>" -- <paths>`, which builds the commit in a private index from HEAD
  (so it cannot sweep in another agent's work) and then refreshes the shared index for those paths (so it
  never lags HEAD). Restore a file with `git checkout HEAD -- <path>`, never `git checkout -- <path>`.
  Before reporting a commit as green, run `make verify-head` (`ARGS=--test` for the goldens): a green run in
  the shared tree includes everyone else's uncommitted work and says nothing about HEAD.
- Remove dead code instead of suppressing warnings.
- Add a regression test for every defect whose failure can be reproduced, and check that it fails without
  the fix.
- Before finishing, inspect the final diff, run applicable checks, and report remaining risks or
  intentionally unverified behavior.
