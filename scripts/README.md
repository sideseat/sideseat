# scripts

Automation behind the `make` targets, grouped by what it is for. Run it through `make`; call a script
directly only when you need a flag the target does not pass. Every script finds the repository root from
its own location, so it works from any directory.

| Directory   | Purpose                                                     | Entry points                                  |
| ----------- | ----------------------------------------------------------- | --------------------------------------------- |
| `check/`    | The inner loop, static gates, capped model checking, the durability proof and the supply-chain audit | `make quick`, `make file-length-check`, `make deps-check`, `make node-floor`, `make harden-spec`, `make test-durability`, `make audit` |
| `hooks/`    | Git hooks: pre-commit (format, file length, secrets), pre-push (`make check`) | `make setup-hooks` |
| `test/`     | Suites that need containers, SDK toolchains or sample envs  | `make test-postgres` and the other `test-*` targets |
| `perf/`     | Latency, ingest and read benchmarks, footprint ceilings, storage | `make bench-http`, `make bench-ingest`, `make bench-reads`, `make footprint`, `make footprint-storage`, `storage-entropy.py` |
| `fixtures/` | Capturing and reviewing the message golden fixtures         | `make capture`, `make capture-sdk-*`          |
| `dev/`      | Running SideSeat locally, the build cache, shared-tree commits | `make dev`, `make dev-server`, `make clean-stale`, `commit-paths.sh` |
| `release/`  | Versioning, release cutting, packaging metadata             | `make release`, `make version`                |
| `ops/`      | Operator tools shipped in the docs: backup and restore      | see the backup-restore reference              |
| `deploy/`   | The container image and a local distributed compose stack   | `make build-docker`, `scripts/deploy/local/`  |
| `tools/`    | Standalone developer utilities with their own environments  | `otel-replay`, `mcp-calculator`, `audit`, `synctrace` (the write and sync tracer `make test-durability` loads), `tla/` (downloaded TLA+ tools) |

Rules:

- A new script goes in the directory of its purpose; nothing lives loose in `scripts/`.
- Scripts are POSIX-portable Bash or Python and stay cross-platform where the target supports Windows.
- A script that needs a dependency installs nothing globally: it uses the repository's pinned toolchain
  (`mise`, `uv --locked`, package-local `npm`).
- Downloaded third-party binaries, such as `tools/tla/`, are gitignored and digest-checked by the target that
  fetches them.
