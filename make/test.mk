##@ Test

# nextest runs the workspace in parallel processes; plain `cargo test` is the fallback.
CARGO_TEST := $(if $(shell command -v cargo-nextest 2>/dev/null),cargo nextest run --locked,cargo test --locked)

.PHONY: test test-rust test-server test-backup-restore test-clickhouse test-clickhouse-replicated test-clickhouse-two-shard test-postgres test-redis test-redpanda bench-http bench-http-distributed bench-ingest footprint footprint-storage footprint-storage-distributed test-web test-sdk-js test-sdk-python test-python-frameworks test-sdk-dotnet coverage

test: test-rust test-web test-sdk-js test-sdk-python test-sdk-dotnet ## Run all regular test suites

# Complete workspace, including the Rust SDK.
test-rust: ## Test the complete Rust workspace
	@echo "[test-rust] Running Rust tests (workspace)..."
	$(call run-with-disk-guard,$(CARGO_TEST) --workspace)

# Server package only, for the inner loop.
test-server: ## Test the server package
	@echo "[test-server] Running server tests..."
	$(call run-with-disk-guard,$(CARGO_TEST) -p sideseat-server)

# Destructive restore proof, isolated in a temporary directory. It is kept out of `make check` because it
# builds and launches the release-facing binary twice and deliberately destroys its fixture between phases.
test-backup-restore: ## Verify embedded backup and restore
	@echo "[test-backup-restore] checkpointing, destroying, restoring, and repairing embedded stores..."
	$(call run-with-disk-guard,SIDESEAT_RUN_BACKUP_RESTORE_TEST=1 \
		cargo test --locked -p sideseat-server --test backup_restore -- --nocapture)

# Docker names include a stable checkout-specific suffix. Two worktrees can therefore run or clean
# integration fixtures independently without deleting each other's containers.
DOCKER_SCOPE := $(shell printf '%s' '$(CURDIR)' | cksum | awk '{print $$1}')
CH_TEST_CONTAINER := sideseat-clickhouse-test-$(DOCKER_SCOPE)
CH_TEST_PORT ?= 8124
# Search text indexes require ClickHouse 26.4; pin the patch for reproducible
# runs. Override the variable to test another release.
CH_TEST_IMAGE ?= clickhouse/clickhouse-server:26.4.3.37

test-clickhouse: ## Test ClickHouse parity in Docker
	$(call run-with-disk-guard,CH_TEST_CONTAINER="$(CH_TEST_CONTAINER)" CH_TEST_PORT="$(CH_TEST_PORT)" \
		CH_TEST_IMAGE="$(CH_TEST_IMAGE)" ./scripts/test/container-test.sh clickhouse)

CH_REPL_CONTAINER := sideseat-clickhouse-replicated-test-$(DOCKER_SCOPE)
CH_REPL_PORT ?= 8299

# Covers clustered migrations, Keeper paths, replicated engines, and
# Distributed-table schema catch-up. One replica does not test convergence.
test-clickhouse-replicated: ## Test replicated ClickHouse migrations
	$(call run-with-disk-guard,CH_REPL_CONTAINER="$(CH_REPL_CONTAINER)" CH_REPL_PORT="$(CH_REPL_PORT)" \
		CH_TEST_IMAGE="$(CH_TEST_IMAGE)" ./scripts/test/container-test.sh clickhouse-replicated)

CH_NET := sideseat-ch-net-$(DOCKER_SCOPE)
CH_SHARD_1_CONTAINER := sideseat-ch-shard1-$(DOCKER_SCOPE)
CH_SHARD_2_CONTAINER := sideseat-ch-shard2-$(DOCKER_SCOPE)
CH_SHARD_PORT_1 ?= 8420
CH_SHARD_PORT_2 ?= 8430

# Distinguishes local tables from Distributed front ends and validates
# cross-shard behavior. Opt-in because distributed DDL makes this run slow.
test-clickhouse-two-shard: ## Test two-shard ClickHouse behavior
	$(call run-with-disk-guard,CH_NET="$(CH_NET)" CH_SHARD_1_CONTAINER="$(CH_SHARD_1_CONTAINER)" \
		CH_SHARD_2_CONTAINER="$(CH_SHARD_2_CONTAINER)" CH_SHARD_PORT_1="$(CH_SHARD_PORT_1)" \
		CH_SHARD_PORT_2="$(CH_SHARD_PORT_2)" CH_TEST_IMAGE="$(CH_TEST_IMAGE)" \
		./scripts/test/container-test.sh clickhouse-two-shard)

# PostgreSQL-specific transactional SQL and SQLite parity.
PG_TEST_CONTAINER := sideseat-postgres-test-$(DOCKER_SCOPE)
PG_TEST_PORT ?= 5433
PG_TEST_IMAGE ?= postgres:17-alpine

test-postgres: ## Test PostgreSQL parity in Docker
	$(call run-with-disk-guard,PG_TEST_CONTAINER="$(PG_TEST_CONTAINER)" PG_TEST_PORT="$(PG_TEST_PORT)" \
		PG_TEST_IMAGE="$(PG_TEST_IMAGE)" ./scripts/test/container-test.sh postgres)

# Durable consumer-group ingestion against append-only Redis.
REDIS_TEST_CONTAINER := sideseat-redis-test-$(DOCKER_SCOPE)
REDIS_TEST_PORT ?= 6399
REDIS_TEST_IMAGE ?= redis:7.4-alpine

test-redis: ## Test Redis-backed ingestion
	$(call run-with-disk-guard,REDIS_TEST_CONTAINER="$(REDIS_TEST_CONTAINER)" REDIS_TEST_PORT="$(REDIS_TEST_PORT)" \
		REDIS_TEST_IMAGE="$(REDIS_TEST_IMAGE)" ./scripts/test/container-test.sh redis)

REDPANDA_TEST_CONTAINER := sideseat-redpanda-test-$(DOCKER_SCOPE)
REDPANDA_TEST_PORT ?= 19092
# Pinned to the current stable RedPanda patch used by the server-mode Compose stack.
REDPANDA_TEST_IMAGE ?= docker.redpanda.com/redpandadata/redpanda:v26.2.3

test-redpanda: ## Test Redpanda-backed ingestion
	$(call run-with-disk-guard,REDPANDA_TEST_CONTAINER="$(REDPANDA_TEST_CONTAINER)" REDPANDA_TEST_PORT="$(REDPANDA_TEST_PORT)" \
		REDPANDA_TEST_IMAGE="$(REDPANDA_TEST_IMAGE)" ./scripts/test/container-test.sh redpanda)

# End-to-end HTTP latency for embedded and distributed deployments.
bench-http: ## Benchmark embedded HTTP latency
	$(call run-with-disk-guard,scripts/perf/bench-http-latency.sh embedded)

bench-http-distributed: ## Benchmark distributed HTTP latency
	$(call run-with-disk-guard,scripts/perf/bench-http-latency.sh distributed)

# Sustained trace-ingest throughput at rising offered rates. `BENCH_INGEST_ARGS` passes flags through, e.g.
# `container --cores 1,2,4,8 --memory 2g` for the hard-limited aarch64 container.
BENCH_INGEST_ARGS ?= local
bench-ingest: ## Measure sustained trace-ingest throughput
	$(call run-with-disk-guard,uv run --locked --script scripts/perf/ingest-throughput.py $(BENCH_INGEST_ARGS))

# What the backends store for the whole fixture corpus, per signal, against raw OTLP protobuf, and the floor
# that fails the run when compression regresses.
footprint-storage: ## Measure and gate stored bytes per signal (embedded)
	$(call run-with-disk-guard,uv run --locked --script scripts/perf/storage-footprint.py embedded --gate --verify-raw)

footprint-storage-distributed: ## Measure and gate stored bytes per signal (ClickHouse, in containers)
	$(call run-with-disk-guard,uv run --locked --script scripts/perf/storage-footprint.py distributed --gate)

# RSS gates run against the release server; in-process gates use allocation
# counters because system allocators may retain freed pages.
footprint: ## Enforce memory footprint ceilings
	$(call run-with-disk-guard,scripts/perf/footprint-gates.sh)
	@# Allocation counters are process-global, so serialize these tests.
	$(call run-with-disk-guard,cd $(SERVER_DIR) && cargo test --locked --release --test footprint -- --ignored --nocapture --test-threads=1)

test-web: ## Run web tests
	@echo "[test-web] Running web tests..."
	@cd $(WEB_DIR) && npm test -- --run

test-sdk-js: ## Run JavaScript SDK tests
	@echo "[test-sdk-js] Running JS SDK tests..."
	@cd sdk/js && npm test
	@cd examples/javascript/sdk-conformance && \
		npm run typecheck && npm run format:check && npm run conformance -- --help

test-sdk-python: ## Run Python SDK tests
	@echo "[test-sdk-python] Running Python SDK tests..."
	@cd sdk/python && uv run --locked pytest
	@cd examples/python/harness && uv run --locked --all-extras pytest -q
	@uv run --locked --project examples/python/sdk-conformance \
		python examples/python/sdk-conformance/conformance.py --help >/dev/null

test-python-frameworks: ## Import every Python framework suite in native and SideSeat modes
	@echo "[test-python-frameworks] Running native/SDK framework smoke matrix..."
	@./scripts/test/python-frameworks.sh

test-sdk-dotnet: ## Run non-vacuous .NET SDK tests
	@echo "[test-sdk-dotnet] Running .NET SDK tests..."
	@DOTNET_COMMAND="$(DOTNET)" ./scripts/test/dotnet-sdk.sh

coverage: ## Generate test coverage reports
	@echo "[coverage] Running tests with coverage..."
	@command -v cargo-tarpaulin >/dev/null 2>&1 || { echo "Error: cargo-tarpaulin not installed. Install with: cargo install cargo-tarpaulin"; exit 1; }
	@echo "[coverage] Rust coverage..."
	$(call run-with-disk-guard,cd $(SERVER_DIR) && cargo tarpaulin --locked --out Html --output-dir ../coverage)
	@echo "[coverage] Rust report: coverage/tarpaulin-report.html"
	@echo "[coverage] Web coverage..."
	@cd $(WEB_DIR) && npm run test:coverage -- --run
