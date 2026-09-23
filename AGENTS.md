# SideSeat agent guide

This file applies to the entire repository. A nested `AGENTS.md` may add narrower
instructions for its subtree.

## Product

SideSeat is an OpenTelemetry observability workbench for AI applications. It
stores traces, metrics, and logs, reconstructs framework-specific conversations
as SideML, and serves them through HTTP, gRPC, MCP, SSE, WebSocket, and the web
application.

## Architecture

The Rust workspace follows ports and adapters:

- `server/crates/core`: configuration, constants, CLI types, and shared utilities. It
  must not depend on SideSeat crates or infrastructure drivers.
- `server/crates/ports`: storage and service traits plus shared DTOs. It contains no
  adapter implementations or SQL.
- `server/crates/domain`: ingestion, SideML, rules, retention, search, files, storage
  governance, staging, and restore workflows.
- `server/crates/query-sql`: typed analytical queries and backend-specific lowering.
- `server/crates/api`: HTTP, gRPC, MCP, SSE, and WebSocket transport code.
- `server/crates/adapter-*`: implementations for databases, blobs, cache, secrets,
  registrations, and queues.
- `server`: the composition root. It selects adapters, wires services, starts
  background work, and owns process lifecycle.

Dependencies point inward. Domain code talks to ports, never directly to an
adapter. Adapters do not import sibling adapters. The server may depend on all
layers because it assembles the application.

Detailed architecture belongs in `docs/engineering/`. User-facing behavior
belongs in `docs/src/`. Do not turn agent instructions into a second
architecture manual.

## Domain invariants

- Framework-specific telemetry interpretation and SideML reconstruction
  knowledge belongs in `server/assets/rules/`, not Rust branches or constants.
  Product-facing integration catalogs may name supported frameworks, but must
  not control extraction behavior.
- Preserve raw telemetry during ingestion. SideML role derivation,
  normalization, history detection, and deduplication happen at read time.
- Tenant-scoped APIs use `ProjectId`; client-provided trace and span IDs are not
  globally unique.
- Analytics writes and transactional writes are not one transaction. Preserve
  the existing fences, tombstones, journal, confirmation, and compensation
  protocols when changing either side.
- A successful ingest response must not acknowledge data before its configured
  durability boundary.
- File and body ownership is reference-based. Do not weaken
  `pending_writers`, `durable`, legal-hold, or restore-reconciliation semantics.
- Embedded and distributed backends must return equivalent public answers where
  the capability is shared. Add parity coverage for backend-specific changes.

## Code conventions

- Prefer small cohesive modules with names from the problem domain.
- Keep public APIs narrow. Do not add compatibility re-exports.
- Use `thiserror` in libraries and `anyhow` at application boundaries.
- Explain constraints and non-obvious tradeoffs in comments. Do not narrate the
  editing process, repeat the code, preserve review history, or record temporary
  measurements in source comments.
- Public APIs receive useful rustdoc. Private code receives comments only when
  the reason cannot be expressed through naming and structure.
- Rust must remain warning-free under the workspace lint configuration.
- TypeScript uses erasable syntax: no enums, namespaces, or constructor
  parameter properties.
- Do not hand-edit generated files, lockfiles, captured fixtures, or
  `web/src/components/ui/` unless the task specifically targets their generator
  or source.
- Keep changes cross-platform where the surrounding component supports macOS,
  Linux, and Windows.

## Dependencies

- Use `--locked` for commands that resolve dependencies.
- Update Rust dependencies deliberately, then inspect `Cargo.lock`.
- `make update-python-deps` is the only workflow that intentionally rewrites
  Python lockfiles.
- Use package-local Node tooling; the repository has no root Node package.

## Verification

Run the smallest relevant checks while iterating, then the broader gate for the
affected surface:

```bash
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
make fmt-check
make lint
make test
make check
```

Backend and operational checks are opt-in because they start containers or
release binaries:

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
```

Choose checks proportionally:

- Storage or SQL changes: relevant unit tests plus both affected parity suites.
- Message reconstruction or rules: golden fixtures and invariants.
- API changes: server tests and the matching SDK/web tests.
- Scripts or Makefile: syntax/static checks and at least one representative
  target.
- Documentation or repository layout: repository structural tests.

The container-free aggregate does not substitute for live backend parity.

## Working practices

- Inspect `git status` before editing and preserve unrelated user changes.
- Use `rg` and `rg --files` for repository searches.
- Keep commits focused and independently reviewable.
- Remove dead code instead of suppressing warnings.
- Add a regression test for every defect whose failure can be reproduced.
- Before finishing, inspect the final diff, run applicable checks, and report
  remaining risks or intentionally unverified behavior.
