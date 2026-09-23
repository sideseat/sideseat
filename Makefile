# SideSeat repository automation. Run `make help` for supported commands.

SHELL := /bin/bash
.DELETE_ON_ERROR:

# OS/arch detection
UNAME_S := $(shell uname -s 2>/dev/null || echo Windows)

# =============================================================================
# Variables
# =============================================================================

ARGS ?=
TYPE ?= patch
NOTARIZE ?= 0
SERVER_DIR := server
WEB_DIR := web
CLI_DIR := cli

# Use the repository-pinned formatter; there is no root Node package.
PRETTIER := $(WEB_DIR)/node_modules/.bin/prettier

# Pricing data
PRICES_URL := https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json
PRICES_FILE := $(SERVER_DIR)/assets/pricing/model_prices_and_context_window.json

# Docker
DOCKER_IMAGE := sideseat/core
DOCKER_FILE  := deploy/Dockerfile

# Homebrew tap
BREW_TAP_REPO ?= sideseat/homebrew-tap

# Release archives
RELEASE_DIR     := release
SIGN_IDENTITY   ?=
NOTARY_PROFILE  ?= sideseat-notarize
SHA256CMD       := $(if $(filter Darwin,$(UNAME_S)),shasum -a 256,sha256sum)

# Local build storage
DISK_BUDGET_MB ?= 12000

# =============================================================================
# Platform Config (single source of truth)
# =============================================================================

PLATFORMS        := darwin-arm64 darwin-x64 linux-x64 linux-arm64 win32-x64
DARWIN_PLATFORMS := darwin-arm64 darwin-x64

RUST_TARGET_darwin-arm64 := aarch64-apple-darwin
BUILD_CMD_darwin-arm64   := cargo build
BIN_NAME_darwin-arm64    := sideseat

RUST_TARGET_darwin-x64   := x86_64-apple-darwin
BUILD_CMD_darwin-x64     := cargo build
BIN_NAME_darwin-x64      := sideseat

RUST_TARGET_linux-x64    := x86_64-unknown-linux-gnu
BUILD_CMD_linux-x64      := cargo zigbuild
BIN_NAME_linux-x64       := sideseat

RUST_TARGET_linux-arm64  := aarch64-unknown-linux-gnu
BUILD_CMD_linux-arm64    := cargo zigbuild
BIN_NAME_linux-arm64     := sideseat

RUST_TARGET_win32-x64    := x86_64-pc-windows-gnu
BUILD_CMD_win32-x64      := CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=$(CURDIR)/scripts/mingw-static-link.sh cargo build
BIN_NAME_win32-x64       := sideseat.exe

# Derived lists
ALL_RUST_TARGETS  := $(foreach p,$(PLATFORMS),$(RUST_TARGET_$(p)))
CLI_BUILD_TARGETS := $(foreach p,$(PLATFORMS),build-cli-$(p))

# Helper: binary path for a platform
cli-bin = $(CLI_DIR)/platforms/platform-$(1)/$(BIN_NAME_$(1))

# =============================================================================
# .PHONY
# =============================================================================

.PHONY: help
.PHONY: setup update-python-deps setup-hooks
.PHONY: dev dev-server dev-web
.PHONY: fmt fmt-check lint lint-advisory check
.PHONY: secret-scan-tree secret-scan-staged secret-scan-range
.PHONY: test test-rust test-server test-clickhouse test-clickhouse-replicated test-clickhouse-two-shard test-postgres test-redis test-redpanda test-backup-restore bench-http bench-http-distributed footprint test-web test-sdk-js test-sdk-python coverage
.PHONY: build build-web build-server
.PHONY: build-sdk build-sdk-js build-sdk-python build-sdk-rust
.PHONY: build-cli build-cli-preflight build-cli-summary $(CLI_BUILD_TARGETS)
.PHONY: version version-check bump sync-version
.PHONY: publish publish-cli publish-sdk-js publish-sdk-python
.PHONY: release
.PHONY: sync-protocol-schema docs-deps docs-system-deps build-docs dev-docs preview-docs
.PHONY: build-docker publish-docker
.PHONY: sign-release sign-verify sign-notarize
.PHONY: build-release publish-release publish-brew
.PHONY: node-floor clean clean-stale clean-docker disk disk-guard download-prices deps-check run start

.SILENT: help version

.DEFAULT_GOAL := help

# =============================================================================
# Help
# =============================================================================

help: ## Show available commands
	@awk -f scripts/make-help.awk $(MAKEFILE_LIST)
	@printf "\nDefaults: TYPE=%s  NOTARIZE=%s\n" "$(TYPE)" "$(NOTARIZE)"

# =============================================================================
# Setup
# =============================================================================

# Normal uv commands use committed lockfiles. This target is the explicit upgrade path.
update-python-deps: ## Upgrade every Python lockfile
	@set -e; for manifest in $$(git ls-files '*pyproject.toml'); do \
		project=$$(dirname "$$manifest"); \
		echo "[update-python-deps] $$project"; \
		(cd "$$project" && uv lock --upgrade); \
	done

setup: ## Install development dependencies and hooks
	@echo "[setup] Checking prerequisites..."
	@command -v node >/dev/null 2>&1 || { echo "Error: node not found. Install Node.js 22.22+ or 24+"; exit 1; }
	@# Bootstrap check before npm dependencies exist; `make node-floor` validates
	@# this declared range against every lockfile after setup.
	@node -e 'var v=process.versions.node.split(".").map(Number), ok=(v[0]===22 && v[1]>=22) || v[0]>=24; if (!ok) { console.error("Error: Node " + process.versions.node + " cannot build this repository. It needs 22.22+ or 24+ (CI uses 24)."); process.exit(1); }'
	@command -v cargo >/dev/null 2>&1 || { echo "Error: cargo not found. Install Rust"; exit 1; }
	@command -v uv >/dev/null 2>&1 || { echo "Error: uv not found. Install with: curl -LsSf https://astral.sh/uv/install.sh | sh"; exit 1; }
	@echo "[setup] Installing workspace dev tools..."
	@uv sync --locked --group dev
	@echo "[setup] Fetching Rust dependencies..."
	@cargo fetch --locked
	@echo "[setup] Installing JS dependencies..."
	@# Reproduce committed lockfiles; dependency updates are explicit package-local operations.
	@cd $(WEB_DIR) && npm ci
	@cd sdk/js && npm ci
	@cd examples/javascript && npm ci
	@echo "[setup] Installing Python dependencies..."
	@cd sdk/python && uv sync --locked --extra dev
	@# Install shared example helpers; framework suites sync lazily when invoked.
	@cd examples/python/common && uv sync --locked
	@echo "[setup] Installing cargo-tarpaulin..."
	@cargo install cargo-tarpaulin --quiet
	@mkdir -p .sideseat
	@$(MAKE) --no-print-directory setup-hooks
	@echo "[setup] Done. Run 'make dev' to start."

setup-hooks: ## Configure repository Git hooks
	@git rev-parse --git-dir >/dev/null 2>&1 || { echo "Error: Not a git repository"; exit 1; }
	@for hook in .githooks/pre-commit .githooks/pre-push; do \
		[ -f "$$hook" ] || { echo "Error: Missing $$hook"; exit 1; }; \
		chmod +x "$$hook"; \
	done
	@git config --local core.hooksPath .githooks
	@echo "[setup-hooks] Git hooks installed"

# =============================================================================
# Development
# =============================================================================

dev: ## Start server and web development processes
	@./scripts/dev.sh $(ARGS)

dev-server: ## Start the Rust server with reload
	@./scripts/dev-server.sh $(ARGS)

dev-web: ## Start the web development server
	@cd $(WEB_DIR) && npm run dev

# =============================================================================
# Format & Lint
# =============================================================================

fmt: ## Format all source code
	@echo "[fmt] Formatting code..."
	@cargo fmt
	@[ -x "$(PRETTIER)" ] || { echo "Error: prettier not installed. Run 'make setup'."; exit 1; }
	@$(PRETTIER) --write "web/src/**/*.{ts,tsx,css,json}" "sdk/js/src/**/*.ts" "examples/javascript/src/**/*.ts"
	@uv run --locked ruff format $(PYTHON_CHECKED)
	@echo "[fmt] Done"

fmt-check: ## Check source formatting
	@echo "[fmt-check] Checking formatting..."
	@cargo fmt --check
	@[ -x "$(PRETTIER)" ] || { echo "Error: prettier not installed. Run 'make setup'."; exit 1; }
	@$(PRETTIER) --check "web/src/**/*.{ts,tsx,css,json}" "sdk/js/src/**/*.ts" "examples/javascript/src/**/*.ts"
	@$(MAKE) --no-print-directory fmt-check-python

lint: ## Run all linters
	@echo "[lint] Running linters..."
	@cargo clippy --locked --all-targets -- -D warnings
	@cd $(WEB_DIR) && npm run lint
	@cd sdk/js && npm run lint
	@cd examples/javascript && npm run lint
	@cd examples/javascript && npm run typecheck
	@$(MAKE) --no-print-directory lint-python
	@# The dev extra owns mypy.
	@cd sdk/python && uv run --locked --extra dev mypy src

# Advisory clippy lints, kept out of `lint` because that gate runs -D warnings and these
# are suggestions rather than defects. Non-blocking by design: review the output, do not
# gate CI on it. Keep this list identical to the advisory block in Cargo.toml.
lint-advisory: ## Run informational Clippy lints
	@echo "[lint-advisory] Advisory clippy lints (informational, does not fail)..."
	@cargo clippy --locked --all-targets -- \
		-W clippy::redundant_clone \
		-W clippy::needless_collect \
		-W clippy::or_fun_call \
		-W clippy::useless_let_if_seq \
		-W clippy::significant_drop_tightening \
		-W clippy::branches_sharing_code \
		-W clippy::suboptimal_flops \
		-W clippy::trait_duplication_in_bounds \
		-W clippy::single_option_map \
		-W clippy::future_not_send \
		2>&1 | grep -E '^(warning|  -->)' | head -60 || true

# ---------------------------------------------------------------------------
# Secret scanning
#
# One definition each, so the hooks and `make harden` cannot drift apart. gitleaks is
# optional: a missing tool warns rather than fails, or a fresh clone could not commit.
# ---------------------------------------------------------------------------
secret-scan-tree:
	@echo "[secret-scan] Working tree..."
	@if command -v gitleaks >/dev/null 2>&1; then \
		gitleaks detect --no-git --config .gitleaks.toml --no-banner --redact; \
	else \
		echo "  SKIPPED: gitleaks not installed (brew install gitleaks)"; \
	fi

secret-scan-staged:
	@echo "[secret-scan] Staged changes..."
	@if command -v gitleaks >/dev/null 2>&1; then \
		gitleaks git --staged --config .gitleaks.toml --no-banner --redact \
			|| { echo "  Possible secret in staged changes. If it is a false positive, add it to .gitleaks.toml."; exit 1; }; \
	else \
		echo "  SKIPPED: gitleaks not installed (brew install gitleaks)"; \
	fi

# Range defaults to what a push would send; override with RANGE=...
RANGE ?= origin/main..HEAD
secret-scan-range:
	@echo "[secret-scan] Commits in $(RANGE)..."
	@if command -v gitleaks >/dev/null 2>&1; then \
		gitleaks git --config .gitleaks.toml --no-banner --redact --log-opts="$(RANGE)"; \
	else \
		echo "  SKIPPED: gitleaks not installed (brew install gitleaks)"; \
	fi

check: disk-guard fmt-check lint test ## Run formatting, lint, and test gates
	@echo "[check] All checks passed"

# =============================================================================
# Hardening gates
#
# Classes the compiler and test suite cannot see. Optional local tools report
# explicit skips; CI runs the blocking equivalents.
# =============================================================================

.PHONY: harden harden-supply harden-spec

# Python source roots covered by the shared format and lint gates.
PYTHON_CHECKED := sdk/python examples/python scripts tools

.PHONY: fmt-check-python lint-python

fmt-check-python:
	@uv run --locked ruff format --check $(PYTHON_CHECKED)

lint-python:
	@uv run --locked ruff check $(PYTHON_CHECKED)

harden: harden-supply harden-spec ## Run supply-chain and specification gates
	@echo "[harden] All hardening gates passed"

# Local skips are visible; CI installs and enforces cargo-deny and cargo-machete.
harden-supply: ## Audit dependencies and secrets
	@echo "[harden-supply] Vulnerable / banned / unlicensed dependencies..."
	@if command -v cargo-deny >/dev/null 2>&1; then \
		cargo deny check; \
	else \
		echo "  SKIPPED locally: cargo-deny not installed (cargo install cargo-deny). CI runs it blocking."; \
	fi
	@$(MAKE) --no-print-directory secret-scan-tree
	@echo "[harden-supply] Unused dependencies..."
	@if command -v cargo-machete >/dev/null 2>&1; then \
		cargo machete; \
	else \
		echo "  SKIPPED locally: cargo-machete not installed (cargo install cargo-machete). CI runs it blocking."; \
	fi

# Every specification must have a matching configuration and pass TLC. The
# versioned tool archive is digest-checked on every run. Model checking remains
# separate from `check` because it takes minutes.
TLA_VERSION := 1.8.0
TLA_SHA256  := db131ddb48e7004d823bef4493df7b35694babe37505b9d9fa5685e7a331f1f1
TLA_JAR     := .tools/tla2tools-$(TLA_VERSION).jar

harden-spec: ## Model-check every TLA+ specification
	@if [ ! -f $(TLA_JAR) ]; then \
		echo "[harden-spec] fetching tla2tools $(TLA_VERSION)..."; \
		mkdir -p .tools; \
		curl -sSL -o $(TLA_JAR).tmp \
			https://github.com/tlaplus/tlaplus/releases/download/v$(TLA_VERSION)/tla2tools.jar && \
			mv $(TLA_JAR).tmp $(TLA_JAR); \
	fi
	@#  Verified every run. `shasum` on macOS, `sha256sum` on Linux.
	@actual=$$( { shasum -a 256 $(TLA_JAR) 2>/dev/null || sha256sum $(TLA_JAR); } | cut -d' ' -f1 ); \
	if [ "$$actual" != "$(TLA_SHA256)" ]; then \
		echo "[harden-spec] $(TLA_JAR) has digest $$actual, expected $(TLA_SHA256)."; \
		echo "[harden-spec] Delete it and re-run to fetch a fresh copy."; \
		exit 1; \
	fi
	@# Validate spec/config pairs in both directions; TLC counterexample traces
	@# are generated artifacts rather than source specifications.
	@orphans=$$( \
		for tla in specs/*.tla; do \
			[ "$${tla#*_TTrace_}" = "$$tla" ] || continue; \
			[ -f "$${tla%.tla}.cfg" ] || echo "$$tla has no .cfg, so nothing model-checks it"; \
		done; \
		for cfg in specs/*.cfg; do \
			[ -f "$${cfg%.cfg}.tla" ] || echo "$$cfg configures no specification"; \
		done); \
	if [ -n "$$orphans" ]; then \
		echo "[harden-spec] specification and configuration do not pair up:"; \
		echo "$$orphans" | sed 's/^/  /'; \
		echo "[harden-spec] Add the missing file, or delete the one left over."; \
		exit 1; \
	fi
	@failed=0; \
	for tla in specs/*.tla; do \
		[ "$${tla#*_TTrace_}" = "$$tla" ] || continue; \
		spec=$$(basename $$tla .tla); \
		printf "[harden-spec] %-16s " "$$spec"; \
		out=$$(cd specs && java -XX:+UseParallelGC -cp ../$(TLA_JAR) tlc2.TLC \
			-workers auto -config $$spec.cfg $$spec.tla 2>&1); \
		if echo "$$out" | grep -q "Model checking completed. No error has been found"; then \
			echo "$$out" | grep -oE "[0-9]+ distinct states found" | head -1; \
		else \
			echo "FAILED"; \
			echo "$$out" | tail -25; \
			failed=1; \
		fi; \
	done; \
	rm -rf specs/states specs/*_TTrace_*.bin specs/*_TTrace_*.tla; \
	exit $$failed

# =============================================================================
# Test
# =============================================================================

test: test-rust test-web test-sdk-js test-sdk-python ## Run all regular test suites

# Complete workspace, including the Rust SDK.
test-rust: disk-guard ## Test the complete Rust workspace
	@echo "[test-rust] Running Rust tests (workspace)..."
	@cargo test --locked --workspace

# Server package only, for the inner loop.
test-server: ## Test the server package
	@echo "[test-server] Running server tests..."
	@cargo test --locked -p sideseat-server

# Destructive restore proof, isolated in a temporary directory. It is kept out of `make check` because it
# builds and launches the release-facing binary twice and deliberately destroys its fixture between phases.
test-backup-restore: disk-guard ## Verify embedded backup and restore
	@echo "[test-backup-restore] checkpointing, destroying, restoring, and repairing embedded stores..."
	@SIDESEAT_RUN_BACKUP_RESTORE_TEST=1 \
		cargo test --locked -p sideseat-server --test backup_restore -- --nocapture

# Docker names include a stable checkout-specific suffix. Two worktrees can therefore run or clean
# integration fixtures independently without deleting each other's containers.
DOCKER_SCOPE := $(shell printf '%s' '$(CURDIR)' | cksum | awk '{print $$1}')
CH_TEST_CONTAINER := sideseat-clickhouse-test-$(DOCKER_SCOPE)
CH_TEST_PORT ?= 8124
# Search text indexes require ClickHouse 26.4; pin the patch for reproducible
# local and CI runs. Override the variable to test another release.
CH_TEST_IMAGE ?= clickhouse/clickhouse-server:26.4.3.37

test-clickhouse: ## Test ClickHouse parity in Docker
	@CH_TEST_CONTAINER="$(CH_TEST_CONTAINER)" CH_TEST_PORT="$(CH_TEST_PORT)" \
		CH_TEST_IMAGE="$(CH_TEST_IMAGE)" ./scripts/container-test.sh clickhouse

CH_REPL_CONTAINER := sideseat-clickhouse-replicated-test-$(DOCKER_SCOPE)
CH_REPL_PORT ?= 8299

# Covers clustered migrations, Keeper paths, replicated engines, and
# Distributed-table schema catch-up. One replica does not test convergence.
test-clickhouse-replicated: ## Test replicated ClickHouse migrations
	@CH_REPL_CONTAINER="$(CH_REPL_CONTAINER)" CH_REPL_PORT="$(CH_REPL_PORT)" \
		CH_TEST_IMAGE="$(CH_TEST_IMAGE)" ./scripts/container-test.sh clickhouse-replicated

CH_NET := sideseat-ch-net-$(DOCKER_SCOPE)
CH_SHARD_1_CONTAINER := sideseat-ch-shard1-$(DOCKER_SCOPE)
CH_SHARD_2_CONTAINER := sideseat-ch-shard2-$(DOCKER_SCOPE)
CH_SHARD_PORT_1 ?= 8420
CH_SHARD_PORT_2 ?= 8430

# Distinguishes local tables from Distributed front ends and validates
# cross-shard behavior. Opt-in because distributed DDL makes this run slow.
test-clickhouse-two-shard: ## Test two-shard ClickHouse behavior
	@CH_NET="$(CH_NET)" CH_SHARD_1_CONTAINER="$(CH_SHARD_1_CONTAINER)" \
		CH_SHARD_2_CONTAINER="$(CH_SHARD_2_CONTAINER)" CH_SHARD_PORT_1="$(CH_SHARD_PORT_1)" \
		CH_SHARD_PORT_2="$(CH_SHARD_PORT_2)" CH_TEST_IMAGE="$(CH_TEST_IMAGE)" \
		./scripts/container-test.sh clickhouse-two-shard

# PostgreSQL-specific transactional SQL and SQLite parity.
PG_TEST_CONTAINER := sideseat-postgres-test-$(DOCKER_SCOPE)
PG_TEST_PORT ?= 5433
PG_TEST_IMAGE ?= postgres:17-alpine

test-postgres: ## Test PostgreSQL parity in Docker
	@PG_TEST_CONTAINER="$(PG_TEST_CONTAINER)" PG_TEST_PORT="$(PG_TEST_PORT)" \
		PG_TEST_IMAGE="$(PG_TEST_IMAGE)" ./scripts/container-test.sh postgres

# Durable consumer-group ingestion against append-only Redis.
REDIS_TEST_CONTAINER := sideseat-redis-test-$(DOCKER_SCOPE)
REDIS_TEST_PORT ?= 6399
REDIS_TEST_IMAGE ?= redis:7.4-alpine

test-redis: ## Test Redis-backed ingestion
	@REDIS_TEST_CONTAINER="$(REDIS_TEST_CONTAINER)" REDIS_TEST_PORT="$(REDIS_TEST_PORT)" \
		REDIS_TEST_IMAGE="$(REDIS_TEST_IMAGE)" ./scripts/container-test.sh redis

REDPANDA_TEST_CONTAINER := sideseat-redpanda-test-$(DOCKER_SCOPE)
REDPANDA_TEST_PORT ?= 19092
# Pinned to the current stable RedPanda patch used by the server-mode Compose stack.
REDPANDA_TEST_IMAGE ?= docker.redpanda.com/redpandadata/redpanda:v26.2.3

test-redpanda: ## Test Redpanda-backed ingestion
	@REDPANDA_TEST_CONTAINER="$(REDPANDA_TEST_CONTAINER)" REDPANDA_TEST_PORT="$(REDPANDA_TEST_PORT)" \
		REDPANDA_TEST_IMAGE="$(REDPANDA_TEST_IMAGE)" ./scripts/container-test.sh redpanda

# End-to-end HTTP latency for embedded and distributed deployments.
bench-http: disk-guard ## Benchmark embedded HTTP latency
	@scripts/bench-http-latency.sh embedded

bench-http-distributed: disk-guard ## Benchmark distributed HTTP latency
	@scripts/bench-http-latency.sh distributed

# RSS gates run against the release server; in-process gates use allocation
# counters because system allocators may retain freed pages.
footprint: disk-guard ## Enforce memory footprint ceilings
	@scripts/footprint-gates.sh
	@# Allocation counters are process-global, so serialize these tests.
	@cd $(SERVER_DIR) && cargo test --locked --release --test footprint -- --ignored --nocapture --test-threads=1

test-web: ## Run web tests
	@echo "[test-web] Running web tests..."
	@cd $(WEB_DIR) && npm test -- --run

test-sdk-js: ## Run JavaScript SDK tests
	@echo "[test-sdk-js] Running JS SDK tests..."
	@cd sdk/js && npm test

test-sdk-python: ## Run Python SDK tests
	@echo "[test-sdk-python] Running Python SDK tests..."
	@cd sdk/python && uv run --locked --extra dev pytest

coverage: ## Generate test coverage reports
	@echo "[coverage] Running tests with coverage..."
	@command -v cargo-tarpaulin >/dev/null 2>&1 || { echo "Error: cargo-tarpaulin not installed. Install with: cargo install cargo-tarpaulin"; exit 1; }
	@echo "[coverage] Rust coverage..."
	@cd $(SERVER_DIR) && cargo tarpaulin --locked --out Html --output-dir ../coverage
	@echo "[coverage] Rust report: coverage/tarpaulin-report.html"
	@echo "[coverage] Web coverage..."
	@cd $(WEB_DIR) && npm run test:coverage -- --run

# =============================================================================
# Build (local dev)
# =============================================================================

build: build-web build-server ## Build the production web and server

build-web: ## Build the web application
	@[ -d "$(WEB_DIR)/node_modules" ] || { echo "Error: $(WEB_DIR)/node_modules not found. Run 'make setup' first."; exit 1; }
	@echo "[build-web] Building frontend..."
	@cd $(WEB_DIR) && npm run build

build-server: build-web ## Build the server
	@echo "[build-server] Building backend..."
	@cd $(SERVER_DIR) && cargo build --locked --release
	@echo "[build-server] Binary: target/release/sideseat"

# =============================================================================
# Build -- SDKs
# =============================================================================

build-sdk: build-sdk-js build-sdk-python build-sdk-rust ## Build implemented SDKs

build-sdk-js: ## Build the JavaScript SDK
	@echo "[build-sdk-js] Building JS SDK..."
	@cd sdk/js && npm run build

build-sdk-python: ## Build the Python SDK
	@echo "[build-sdk-python] Building Python SDK..."
	@cd sdk/python && uv build

build-sdk-rust: ## Build the Rust SDK
	@echo "[build-sdk-rust] Building Rust SDK..."
	@cargo build --locked -p sideseat

# =============================================================================
# Build -- CLI (cross-compile all platforms)
# =============================================================================

# Per-platform targets (generated)
define MAKE_CLI_TARGET
build-cli-$(1): build-web
	@echo "[build-cli] $(1) ($(BUILD_CMD_$(1)))..."
	@cd $$(SERVER_DIR) && $(BUILD_CMD_$(1)) --locked --release --target $(RUST_TARGET_$(1))
	@cp target/$(RUST_TARGET_$(1))/release/$(BIN_NAME_$(1)) $$(call cli-bin,$(1))
	@chmod +x $$(call cli-bin,$(1)) 2>/dev/null || true
endef
$(foreach p,$(PLATFORMS),$(eval $(call MAKE_CLI_TARGET,$(p))))

# Preflight: verify tools and rust targets
build-cli-preflight:
	@[ "$(UNAME_S)" = "Darwin" ] || { echo "Error: build-cli requires macOS (cross-compilation host)"; exit 1; }
	@command -v cargo-zigbuild >/dev/null 2>&1 || { echo "Error: cargo-zigbuild not found. Install: cargo install cargo-zigbuild"; exit 1; }
	@command -v zig >/dev/null 2>&1 || { echo "Error: zig not found. Install: brew install zig"; exit 1; }
	@command -v x86_64-w64-mingw32-g++ >/dev/null 2>&1 || { echo "Error: mingw-w64 not found. Install: brew install mingw-w64"; exit 1; }
	@MISSING=""; for t in $(ALL_RUST_TARGETS); do \
		rustup target list --installed | grep -q "^$$t$$" || MISSING="$$MISSING $$t"; \
	done; \
	if [ -n "$$MISSING" ]; then \
		echo "Error: Missing Rust targets:$$MISSING"; \
		echo "Install: rustup target add$$MISSING"; \
		exit 1; \
	fi

# Summary: smoke test + binary sizes
build-cli-summary:
	@echo "[build-cli] Smoke test (native binary)..."
	@$(call cli-bin,darwin-arm64) --version || \
		$(call cli-bin,darwin-x64) --version || \
		{ echo "Error: Native binary smoke test failed"; exit 1; }
	@echo "[build-cli] Platform binaries:"
	@$(foreach p,$(PLATFORMS),SIZE=$$(ls -lh "$(call cli-bin,$(p))" | awk '{print $$5}') && \
		echo "  @sideseat/platform-$(p)  $$SIZE";)
	@echo "[build-cli] All platform binaries built"

# Orchestrator: preflight -> build all -> summary
build-cli: build-cli-preflight ## Build CLI packages for all platforms
	@echo "[build-cli] Building all platform binaries..."
	@$(MAKE) $(CLI_BUILD_TARGETS)
	@$(MAKE) build-cli-summary

# =============================================================================
# Version
# =============================================================================

version: ## Show package versions
	@echo "CLI:                $$(node -p "require('./cli/package.json').version")"
	@echo "Server:             $$(./scripts/workspace-version.sh)"
	@echo "SDK (JavaScript):   $$(node -p "require('./sdk/js/package.json').version")"
	@echo "SDK (Python):       $$(grep '__version__' sdk/python/src/sideseat/_version.py | sed 's/.*\"\(.*\)\".*/\1/')"
	@echo "SDK (Rust):         $$(sed -n 's/^version = \"\(.*\)\"/\1/p' sdk/rust/Cargo.toml | head -1)"
	@echo "SDK (.NET, stub):   $$(sed -n 's:.*<Version>\(.*\)</Version>.*:\1:p' sdk/dotnet/SideSeat.csproj)"

version-check: ## Verify coordinated package versions
	@CLI_VERSION=$$(node -p "require('./cli/package.json').version") && \
	SERVER_VERSION=$$(./scripts/workspace-version.sh) && \
	MISMATCHED="" && \
	if [ "$$CLI_VERSION" != "$$SERVER_VERSION" ]; then \
		MISMATCHED="$$MISMATCHED\n  Rust workspace: $$SERVER_VERSION"; \
	fi && \
	for pkg in $(PLATFORMS); do \
		PKG_VERSION=$$(node -p "require('./cli/platforms/platform-'+'$$pkg'+'/package.json').version") && \
		if [ "$$CLI_VERSION" != "$$PKG_VERSION" ]; then \
			MISMATCHED="$$MISMATCHED\n  cli/platforms/platform-$$pkg: $$PKG_VERSION"; \
		fi; \
	done && \
	for dep in $$(node -p "Object.entries(require('./cli/package.json').optionalDependencies||{}).map(([k,v])=>k+':'+v).join(' ')"); do \
		DEP_VERSION=$${dep#*:} && \
		DEP_NAME=$${dep%%:*} && \
		if [ "$$CLI_VERSION" != "$$DEP_VERSION" ]; then \
			MISMATCHED="$$MISMATCHED\n  optionalDependencies[$$DEP_NAME]: $$DEP_VERSION"; \
		fi; \
	done && \
	if [ -n "$$MISMATCHED" ]; then \
		echo "Version mismatch (expected $$CLI_VERSION):$$MISMATCHED"; \
		exit 1; \
	fi && \
	echo "All versions match: $$CLI_VERSION"

bump: ## Bump versions with TYPE=patch|minor|major
	@if [ "$(TYPE)" != "patch" ] && [ "$(TYPE)" != "minor" ] && [ "$(TYPE)" != "major" ]; then \
		echo "Error: TYPE must be patch, minor, or major (got: $(TYPE))"; \
		exit 1; \
	fi
	@echo "[bump] Bumping $(TYPE) version..."
	@cd $(CLI_DIR) && npm version $(TYPE) --no-git-tag-version
	@$(MAKE) --no-print-directory sync-version

sync-version: ## Synchronize server and CLI versions
	@NEW_VERSION=$$(node -p "require('./cli/package.json').version") && \
	TEMP_FILE=$$(mktemp) && \
	sed "s/^version = \".*\"/version = \"$$NEW_VERSION\"/" Cargo.toml > "$$TEMP_FILE" && \
	mv "$$TEMP_FILE" Cargo.toml && \
	cargo update --workspace --quiet && \
	CARGO_VERSION=$$(./scripts/workspace-version.sh) && \
	if [ "$$NEW_VERSION" != "$$CARGO_VERSION" ]; then \
		echo "Error: Version sync failed. Expected $$NEW_VERSION, got $$CARGO_VERSION"; \
		exit 1; \
	fi && \
	for pkg in $(PLATFORMS); do \
		node -e "const p=require('./cli/platforms/platform-'+'$$pkg'+'/package.json'); p.version='$$NEW_VERSION'; require('fs').writeFileSync('./cli/platforms/platform-'+'$$pkg'+'/package.json', JSON.stringify(p, null, 2)+'\n')"; \
	done && \
	node -e "const p=require('./cli/package.json'); Object.keys(p.optionalDependencies||{}).forEach(k=>p.optionalDependencies[k]='$$NEW_VERSION'); require('fs').writeFileSync('./cli/package.json', JSON.stringify(p, null, 2)+'\n')" && \
	echo "[sync-version] Version synced to $$NEW_VERSION (server + CLI only; SDKs maintained separately)"

# =============================================================================
# Publish
# =============================================================================

publish: publish-cli publish-sdk-js publish-sdk-python publish-docker ## Publish CLI, SDKs, and Docker image

publish-cli: ## Publish CLI platform packages
	@echo "[publish-cli] Verifying npm authentication..."
	@npm whoami >/dev/null 2>&1 || { echo "Error: Not logged in to npm. Run 'npm login' first."; exit 1; }
	@echo "[publish-cli] Verifying binaries exist..."
	@$(foreach p,$(PLATFORMS),[ -f "$(call cli-bin,$(p))" ] || { echo "Error: Missing binary for $(p): $(call cli-bin,$(p)). Run 'make build-cli' first."; exit 1; };)
	@echo "[publish-cli] Verifying macOS code signatures..."
	@$(foreach p,$(DARWIN_PLATFORMS),codesign --verify --strict "$(call cli-bin,$(p))" 2>/dev/null || \
		{ echo "Error: $(call cli-bin,$(p)) is not signed. Run 'make sign-release' first."; exit 1; }; \
		codesign -dvv "$(call cli-bin,$(p))" 2>&1 | grep -q "flags=.*runtime" || \
		{ echo "Error: $(call cli-bin,$(p)) missing Hardened Runtime. Re-sign with --options runtime."; exit 1; }; \
		echo "  $(call cli-bin,$(p)): signed (Hardened Runtime)";)
	@$(MAKE) --no-print-directory version-check
	@echo "[publish-cli] Publishing platform packages..."
	@$(foreach p,$(PLATFORMS),(cd $(CLI_DIR)/platforms/platform-$(p) && npm publish --access public) &&) true
	@VERSION=$$(node -p "require('./$(CLI_DIR)/package.json').version"); \
	echo "[publish-cli] Waiting for platform packages to propagate (v$$VERSION)..."; \
	for p in $(PLATFORMS); do \
		attempt=1; \
		while [ $$attempt -le 60 ]; do \
			if npm view "@sideseat/platform-$$p@$$VERSION" version >/dev/null 2>&1; then \
				echo "  @sideseat/platform-$$p@$$VERSION available"; \
				break; \
			fi; \
			echo "  Waiting for @sideseat/platform-$$p@$$VERSION (attempt $$attempt/60)..."; \
			sleep 5; \
			attempt=$$((attempt + 1)); \
		done; \
		if [ $$attempt -gt 60 ]; then \
			echo "Error: @sideseat/platform-$$p@$$VERSION not available"; \
			exit 1; \
		fi; \
	done
	@echo "[publish-cli] Waiting 5 min for CDN propagation..."
	@sleep 300
	@echo "[publish-cli] Publishing main sideseat package..."
	@cd $(CLI_DIR) && npm publish --access public
	@VERSION=$$(node -p "require('./$(CLI_DIR)/package.json').version"); \
	echo "[publish-cli] Verifying sideseat@$$VERSION on registry..."; \
	attempt=1; \
	while [ $$attempt -le 30 ]; do \
		if npm view "sideseat@$$VERSION" version >/dev/null 2>&1; then \
			echo "[publish-cli] Published and verified sideseat@$$VERSION"; \
			exit 0; \
		fi; \
		echo "  Waiting for sideseat@$$VERSION (attempt $$attempt/30)..."; \
		sleep 5; \
		attempt=$$((attempt + 1)); \
	done; \
	echo "Warning: sideseat@$$VERSION published but not yet verified on registry"

publish-sdk-js: ## Publish the JavaScript SDK
	@echo "[publish-sdk-js] Verifying npm authentication..."
	@npm whoami >/dev/null 2>&1 || { echo "Error: Not logged in to npm. Run 'npm login' first."; exit 1; }
	@echo "[publish-sdk-js] Building and publishing..."
	@cd sdk/js && npm ci && npm run build && npm publish --access public
	@echo "[publish-sdk-js] Published $$(node -p "require('./sdk/js/package.json').version")"

publish-sdk-python: ## Publish the Python SDK
	@echo "[publish-sdk-python] Building and publishing..."
	@cd sdk/python && uv build && uv publish
	@echo "[publish-sdk-python] Published $$(grep '__version__' sdk/python/src/sideseat/_version.py | sed 's/.*\"\(.*\)\".*/\1/')"

# =============================================================================
# Release
# =============================================================================

release: ## Check, bump, commit, tag, and atomically push
	@./scripts/release.sh "$(TYPE)"

# =============================================================================
# Docker
# =============================================================================

build-docker: ## Build the local Docker image
	@echo "[build-docker] Building $(DOCKER_IMAGE) for current platform..."
	@docker build -t $(DOCKER_IMAGE) -f $(DOCKER_FILE) .
	@echo "[build-docker] Done. Run: docker run -p 5388:5388 -v sideseat-data:/data $(DOCKER_IMAGE)"

publish-docker: ## Publish the multi-platform Docker image
	@echo "[publish-docker] Building and pushing multi-arch image..."
	@VERSION=$$(node -p "require('./cli/package.json').version") && \
	docker buildx build --platform linux/amd64,linux/arm64 \
		-t $(DOCKER_IMAGE):latest -t $(DOCKER_IMAGE):$$VERSION \
		-f $(DOCKER_FILE) --push .
	@VERSION=$$(node -p "require('./cli/package.json').version") && \
	echo "[publish-docker] Pushed $(DOCKER_IMAGE):latest and $(DOCKER_IMAGE):$$VERSION"

# =============================================================================
# Documentation
# =============================================================================

# The Python SDK bundles the canonical WebSocket schema for standalone installs.
sync-protocol-schema: ## Synchronize the WebSocket protocol schema
	@cp docs/engineering/protocol-ws-v1/schema.json sdk/python/src/sideseat/runtime/_schema.json
	@echo "[sync-protocol-schema] sdk/python now bundles docs/engineering/protocol-ws-v1/schema.json"

docs-deps:
	@# Reinstall when the lockfile changes so local builds use the pinned dependency tree.
	@if [ ! -d docs/node_modules ] || [ docs/package-lock.json -nt docs/node_modules ]; then \
		echo "[docs-deps] Installing documentation dependencies..."; \
		cd docs && npm ci; \
	fi
	@# Mermaid rendering uses the browser matched to the locked Playwright package.
	@cd docs && npx --no-install playwright install chromium
	@[ "$$(uname -s)" != "Linux" ] || echo "[docs-deps] On Linux, if the build cannot launch the browser: make docs-system-deps (needs sudo)"

docs-system-deps: docs-deps ## Install Linux documentation system packages
	@# Kept explicit because Playwright may request elevated privileges for system packages.
	@echo "[docs-system-deps] Installing the system libraries the diagram browser needs (sudo)..."
	@cd docs && npx --no-install playwright install-deps chromium

build-docs: docs-deps ## Build the documentation site
	@echo "[build-docs] Building documentation..."
	@cd docs && npm run build
	@echo "[build-docs] Output: docs/dist/"

dev-docs: docs-deps ## Start the documentation development server
	@echo "[dev-docs] Starting docs dev server..."
	@cd docs && npm run dev

preview-docs: docs-deps ## Preview the built documentation
	@echo "[preview-docs] Previewing built docs..."
	@[ -d "docs/dist" ] || { $(MAKE) build-docs; }
	@cd docs && npm run preview

# =============================================================================
# Code Signing (macOS)
# =============================================================================

# Production signing identity is supplied through the environment or a make argument.
sign-release: ## Sign macOS platform binaries with Developer ID
	@[ "$(UNAME_S)" = "Darwin" ] || { echo "Error: code signing requires macOS"; exit 1; }
	@[ -n "$(SIGN_IDENTITY)" ] || { echo "Error: SIGN_IDENTITY required. Usage: make sign-release SIGN_IDENTITY=\"Developer ID Application: Name (TEAMID)\""; exit 1; }
	@for bin in $(foreach p,$(DARWIN_PLATFORMS),$(call cli-bin,$(p))); do \
		[ -f "$$bin" ] || { echo "Error: missing $$bin. Run 'make build-cli' first."; exit 1; }; \
		codesign --force --options runtime --sign "$(SIGN_IDENTITY)" --entitlements packaging/macos/entitlements.plist "$$bin" || \
			{ echo "Error: failed to sign $$bin"; exit 1; }; \
		echo "[sign-release] Signed $$bin"; \
	done

sign-verify: ## Verify code signature and entitlements on macOS platform binaries
	@[ "$(UNAME_S)" = "Darwin" ] || { echo "Error: signature verification requires macOS"; exit 1; }
	@for bin in $(foreach p,$(DARWIN_PLATFORMS),$(call cli-bin,$(p))); do \
		[ -f "$$bin" ] || { echo "Error: missing $$bin. Run 'make build-cli' first."; exit 1; }; \
		codesign --verify --strict "$$bin" || { echo "Error: invalid signature on $$bin"; exit 1; }; \
		echo "=== $$bin ==="; \
		echo "--- Signature ---"; \
		codesign -dvv "$$bin" || exit 1; \
		echo ""; \
		echo "--- Entitlements ---"; \
		codesign -d --entitlements :- "$$bin" || exit 1; \
		echo ""; \
	done

sign-notarize: ## Notarize macOS archives; ZIP files cannot be stapled
	@[ "$$(uname -s)" = "Darwin" ] || { echo "Error: notarization requires macOS"; exit 1; } && \
	VERSION=$$(node -p "require('./cli/package.json').version") && \
	OUTDIR="$(RELEASE_DIR)/v$$VERSION" && \
	[ -d "$$OUTDIR" ] || { echo "Error: $$OUTDIR not found. Run 'make build-release' first."; exit 1; } && \
	echo "[sign-notarize] Notarizing darwin archives for v$$VERSION..." && \
	for plat in $(DARWIN_PLATFORMS); do \
		ARCHIVE="sideseat-$$VERSION-$$plat.zip" && \
		[ -f "$$OUTDIR/$$ARCHIVE" ] || { echo "Error: $$OUTDIR/$$ARCHIVE not found"; exit 1; } && \
		echo "  Submitting $$ARCHIVE..." && \
		xcrun notarytool submit "$$OUTDIR/$$ARCHIVE" \
			--keychain-profile "$(NOTARY_PROFILE)" --wait --timeout 48h || \
			{ echo "Error: Notarization failed for $$plat"; exit 1; } && \
		echo "  $$plat: notarized"; \
	done && \
	echo "[sign-notarize] Done (stapling skipped -- not supported for ZIP/CLI; Gatekeeper checks online)"

# =============================================================================
# Release Archives
# =============================================================================

build-release: ## Create release archives and checksums
	@VERSION=$$(node -p "require('./cli/package.json').version") && \
	OUTDIR="$(RELEASE_DIR)/v$$VERSION" && \
	echo "[build-release] Building release archives for v$$VERSION..." && \
	echo "[build-release] Verifying binaries exist..." && \
	$(foreach p,$(PLATFORMS),[ -f "$(call cli-bin,$(p))" ] || \
		{ echo "Error: Missing binary for $(p): $(call cli-bin,$(p)). Run 'make build-cli' first."; exit 1; } &&) \
	echo "[build-release] Verifying darwin code signatures..." && \
	$(foreach p,$(DARWIN_PLATFORMS),codesign --verify --strict "$(call cli-bin,$(p))" 2>/dev/null || \
		{ echo "Error: $(call cli-bin,$(p)) is not signed. Run 'make sign-release' first."; exit 1; } &&) \
	rm -rf "$$OUTDIR" && mkdir -p "$$OUTDIR" && \
	for plat in $(PLATFORMS); do \
		case $$plat in \
			darwin-*|win32-*) EXT=zip ;; \
			*)                EXT=tar.gz ;; \
		esac && \
		ARCHIVE="sideseat-$$VERSION-$$plat.$$EXT" && \
		case $$plat in \
			win32-*) BINFILE=sideseat.exe ;; \
			*)       BINFILE=sideseat ;; \
		esac && \
		TMPDIR=$$(mktemp -d) && \
		cp "$(CLI_DIR)/platforms/platform-$$plat/$$BINFILE" "$$TMPDIR/$$BINFILE" && \
		cp LICENSE "$$TMPDIR/LICENSE" && \
		if [ "$$EXT" = "zip" ]; then \
			(cd "$$TMPDIR" && zip -q "$$ARCHIVE" "$$BINFILE" LICENSE) && \
			mv "$$TMPDIR/$$ARCHIVE" "$$OUTDIR/$$ARCHIVE"; \
		else \
			tar czf "$$OUTDIR/$$ARCHIVE" -C "$$TMPDIR" "$$BINFILE" LICENSE; \
		fi && \
		rm -rf "$$TMPDIR" && \
		echo "  $$ARCHIVE"; \
	done && \
	if [ "$(NOTARIZE)" = "1" ]; then \
		if [ "$$(uname -s)" = "Darwin" ]; then \
			$(MAKE) sign-notarize; \
		else \
			echo "[build-release] WARNING: Not on macOS -- cannot notarize"; \
		fi; \
	else \
		echo "[build-release] Skipping notarization (use NOTARIZE=1 to enable)"; \
	fi && \
	echo "[build-release] Generating checksums..." && \
	(cd "$$OUTDIR" && $(SHA256CMD) sideseat-* > checksums-sha256.txt) && \
	echo "[build-release] Done: $$OUTDIR/"

publish-release: ## Upload archives to the GitHub release
	@VERSION=$$(node -p "require('./cli/package.json').version") && \
	OUTDIR="$(RELEASE_DIR)/v$$VERSION" && \
	echo "[publish-release] Publishing v$$VERSION to GitHub Releases..." && \
	[ -d "$$OUTDIR" ] || { echo "Error: $$OUTDIR not found. Run 'make build-release' first."; exit 1; } && \
	echo "[publish-release] Verifying checksums..." && \
	(cd "$$OUTDIR" && $(SHA256CMD) -c checksums-sha256.txt) || \
		{ echo "Error: Checksum verification failed"; exit 1; } && \
	if ! git rev-parse "v$$VERSION" >/dev/null 2>&1; then \
		echo "[publish-release] Creating tag v$$VERSION..." && \
		git tag "v$$VERSION" && \
		git push origin "v$$VERSION"; \
	fi && \
	echo "[publish-release] Creating GitHub release..." && \
	gh release create "v$$VERSION" "$$OUTDIR"/* --generate-notes --title "v$$VERSION" && \
	echo "[publish-release] Done: https://github.com/$$(gh repo view --json nameWithOwner -q .nameWithOwner)/releases/tag/v$$VERSION" && \
	echo "[publish-release] Next: make publish-brew"

# =============================================================================
# Homebrew Tap
# =============================================================================

publish-brew: ## Update the Homebrew tap
	@VERSION=$$(node -p "require('./cli/package.json').version") && \
	CHECKSUMS="$(RELEASE_DIR)/v$$VERSION/checksums-sha256.txt" && \
	echo "[publish-brew] Publishing Homebrew formula for v$$VERSION..." && \
	[ -f "$$CHECKSUMS" ] || \
		{ echo "Error: $$CHECKSUMS not found. Run 'make build-release' first."; exit 1; } && \
	gh release view "v$$VERSION" >/dev/null 2>&1 || \
		{ echo "Error: GitHub Release v$$VERSION not found. Run 'make publish-release' first."; exit 1; } && \
	SHA_DARWIN_ARM64=$$(grep -F 'darwin-arm64' "$$CHECKSUMS" | awk '{print $$1}') && \
	SHA_DARWIN_X64=$$(grep -F 'darwin-x64' "$$CHECKSUMS" | awk '{print $$1}') && \
	SHA_LINUX_X64=$$(grep -F 'linux-x64' "$$CHECKSUMS" | awk '{print $$1}') && \
	SHA_LINUX_ARM64=$$(grep -F 'linux-arm64' "$$CHECKSUMS" | awk '{print $$1}') && \
	for hash in $$SHA_DARWIN_ARM64 $$SHA_DARWIN_X64 $$SHA_LINUX_X64 $$SHA_LINUX_ARM64; do \
		echo "$$hash" | grep -qE '^[0-9a-f]{64}$$' || \
			{ echo "Error: Invalid SHA256 hash: $$hash"; exit 1; }; \
	done && \
	FORMULA=$$(mktemp) && \
	sed -e "s/__VERSION__/$$VERSION/g" \
		-e "s/__SHA256_DARWIN_ARM64__/$$SHA_DARWIN_ARM64/g" \
		-e "s/__SHA256_DARWIN_X64__/$$SHA_DARWIN_X64/g" \
		-e "s/__SHA256_LINUX_X64__/$$SHA_LINUX_X64/g" \
		-e "s/__SHA256_LINUX_ARM64__/$$SHA_LINUX_ARM64/g" \
		packaging/homebrew/sideseat.rb.tmpl > "$$FORMULA" && \
	grep -q '__' "$$FORMULA" && \
		{ echo "Error: Unreplaced placeholders in generated formula"; rm -f "$$FORMULA"; exit 1; } || true && \
	ENCODED=$$(base64 < "$$FORMULA" | tr -d '\n') && \
	EXISTING_SHA=$$(gh api "repos/$(BREW_TAP_REPO)/contents/Formula/sideseat.rb" --jq '.sha' 2>/dev/null || echo "") && \
	if [ -n "$$EXISTING_SHA" ]; then \
		gh api --method PUT "repos/$(BREW_TAP_REPO)/contents/Formula/sideseat.rb" \
			-f message="Update sideseat to v$$VERSION" \
			-f content="$$ENCODED" \
			-f sha="$$EXISTING_SHA" \
			--silent; \
	else \
		gh api --method PUT "repos/$(BREW_TAP_REPO)/contents/Formula/sideseat.rb" \
			-f message="Add sideseat v$$VERSION" \
			-f content="$$ENCODED" \
			--silent; \
	fi && \
	rm -f "$$FORMULA" && \
	echo "[publish-brew] Formula pushed to $(BREW_TAP_REPO)" && \
	echo "[publish-brew] Install: brew tap sideseat/tap && brew install sideseat"

# =============================================================================
# Utilities
# =============================================================================

deps-check: ## Report outdated dependencies
	@./scripts/deps-check.sh

node-floor: ## Derive the supported Node.js floor
	@#  Prints the Node versions every installed `engines.node` range accepts. Needs an installed tree for
	@#  `semver`, which is transitive rather than declared - the script says so and stops if none is there.
	@node scripts/node-floor.mjs --check

download-prices: ## Refresh model pricing data
	@echo "[download-prices] Downloading LLM pricing data..."
	@mkdir -p $(dir $(PRICES_FILE))
	@if command -v curl >/dev/null 2>&1; then \
		curl -fsSL "$(PRICES_URL)" -o "$(PRICES_FILE)" || \
			{ echo "Error: Download failed"; exit 1; }; \
	elif command -v wget >/dev/null 2>&1; then \
		wget -q "$(PRICES_URL)" -O "$(PRICES_FILE)" || \
			{ echo "Error: Download failed"; exit 1; }; \
	else \
		echo "Error: curl or wget required"; exit 1; \
	fi
	@echo "[download-prices] Saved to $(PRICES_FILE)"

# Reclaim stale and incremental Cargo artifacts without removing the current build.
clean-stale: ## Remove stale Cargo artifacts
	@./scripts/clean-stale.sh

# Docker resources created by test and benchmark targets. Keep this list explicit:
# machine-wide prune commands can remove caches or anonymous volumes owned by other projects.
SIDESEAT_TEST_CONTAINERS = \
	$(CH_TEST_CONTAINER) \
	$(CH_REPL_CONTAINER) \
	$(CH_SHARD_1_CONTAINER) \
	$(CH_SHARD_2_CONTAINER) \
	$(PG_TEST_CONTAINER) \
	$(REDIS_TEST_CONTAINER) \
	$(REDPANDA_TEST_CONTAINER) \
	sideseat-bench-pg-$(DOCKER_SCOPE) \
	sideseat-bench-ch-$(DOCKER_SCOPE) \
	sideseat-bench-minio-$(DOCKER_SCOPE)

clean-docker: ## Remove this checkout's test containers
	@command -v docker >/dev/null 2>&1 || { echo "[clean-docker] docker not installed; nothing to do"; exit 0; }
	@docker info >/dev/null 2>&1 || { echo "[clean-docker] Docker daemon is unavailable"; exit 1; }
	@echo "[clean-docker] Removing this repo's throwaway containers..."
	@containers=$$(docker container ls -a --format '{{.Names}}') || exit 1; \
	for container in $(SIDESEAT_TEST_CONTAINERS); do \
		if printf '%s\n' "$$containers" | grep -Fqx "$$container"; then \
			docker rm -fv "$$container" >/dev/null; \
		fi; \
	done
	@networks=$$(docker network ls --format '{{.Name}}') || exit 1; \
	if printf '%s\n' "$$networks" | grep -Fqx "$(CH_NET)"; then \
		docker network rm "$(CH_NET)" >/dev/null; \
	fi

# What is using space, and **whether it is within budget** - which is the difference between a report and a
# gate. Exits non-zero over the ceiling, the way a missed latency ceiling fails `make bench-http`.
disk: ## Report and enforce the local disk budget
	@echo "[disk] Free space:"
	@df -h . | tail -1
	@echo "[disk] Largest local directories:"
	@du -sh target $(WEB_DIR)/node_modules docs/node_modules .sideseat 2>/dev/null | sort -rh || true
	@command -v docker >/dev/null 2>&1 && { echo "[disk] Docker:"; docker system df; } || true
	@#  The container runtime's **VM disk images**, which is where this machine's space actually went and why
	@#  it went unnoticed: 100 GB in a Colima data disk, 20 GB in its boot disk and 24 GB in a Docker Desktop
	@#  image, against 4 GB of `target/`. They are *sparse*, so `du` reports the blocks in use and `ls`
	@#  the provisioned size - and neither shrinks when images inside the VM are pruned. `docker system df`
	@#  above reports what is reclaimable *inside* the VM, which is a different number from what the host
	@#  gets back, and reading the first as the second is the mistake that let 144 GB hide.
	@echo "[disk] Container VM disk images (sparse; pruning inside the VM does not shrink these):"
	@for image in "$$HOME/.colima/_lima/_disks"/*/datadisk "$$HOME/.colima/_lima"/*/diffdisk \
	              "$$HOME/.colima/_lima"/*/disk \
	              "$$HOME/Library/Containers/com.docker.docker/Data/vms"/*/data/Docker.raw; do \
		[ -f "$$image" ] || continue; \
		printf '  %-6s in use   %-6s provisioned   %s\n' \
			"$$(du -h "$$image" 2>/dev/null | cut -f1)" \
			"$$(ls -lh "$$image" 2>/dev/null | awk '{print $$5}')" \
			"$$image"; \
	done
	@command -v docker >/dev/null 2>&1 && { \
		echo "[disk] Active runtime: $$(docker context show 2>/dev/null)"; \
		echo "[disk] An inactive runtime's disk is dead weight - reclaiming it means deleting that VM."; \
	} || true
	@used=$$(du -sm target 2>/dev/null | cut -f1 || echo 0); \
	if [ "$$used" -gt "$(DISK_BUDGET_MB)" ]; then \
		echo "[disk] OVER BUDGET: target/ is $$used MB against a ceiling of $(DISK_BUDGET_MB) MB"; \
		echo "[disk] Reclaim: make clean-stale (keeps the current build) or make clean (cold rebuild)"; \
		exit 1; \
	else \
		echo "[disk] target/ is $$used MB, within the $(DISK_BUDGET_MB) MB budget"; \
	fi

# The cheap enforcement, wired into the targets that cause the growth.
#
# A `du` on target/ costs 0.3s, so this can run habitually - which is the point: the manual targets existed
# and the disk still filled twice, because nothing ran them. Over budget it reclaims exactly what a rebuild
# regenerates cheaply and says what it took; the current build is never touched.
#
# It does **not** fail the build. Disk usage is not a correctness property, and aborting someone's test run
# over it would be the wrong trade - `make disk` is the gate that fails, this is the thing that keeps the
# number from getting there. If a reclaim cannot bring it under, it says so and carries on, because the
# alternative is a cold rebuild nobody asked for mid-session.
disk-guard:
	@used=$$(du -sm target 2>/dev/null | cut -f1 || echo 0); \
	if [ "$$used" -gt "$(DISK_BUDGET_MB)" ]; then \
		echo "[disk-guard] target/ is $$used MB, over the $(DISK_BUDGET_MB) MB budget - reclaiming"; \
		$(MAKE) --no-print-directory clean-stale; \
		after=$$(du -sm target 2>/dev/null | cut -f1 || echo 0); \
		if [ "$$after" -gt "$(DISK_BUDGET_MB)" ]; then \
			echo "[disk-guard] still $$after MB: the current build itself exceeds the budget."; \
			echo "[disk-guard] Either raise DISK_BUDGET_MB or run make clean for a cold rebuild."; \
			command -v cargo-sweep >/dev/null 2>&1 || \
				echo "[disk-guard] cargo-sweep is not installed, so stale artifacts of older builds were kept: cargo install cargo-sweep"; \
		fi; \
	fi

clean: ## Remove all generated build artifacts
	@echo "[clean] Removing build artifacts..."
	@rm -rf target
	@#  `dist` is simply removed. The server *embeds* it, so it has to exist to compile - and that is
	@#  the API crate build script's job, on the next build. Writing a placeholder here as well gave the same
	@#  artifact two owners with different content, and since the build script preserves any existing
	@#  `index.html`, whichever ran last decided what a UI-less binary served.
	@rm -rf $(WEB_DIR)/dist
	@rm -rf $(WEB_DIR)/node_modules/.vite
	@rm -f $(CLI_DIR)/bin/sideseat-*
	@rm -f $(CLI_DIR)/platforms/*/sideseat $(CLI_DIR)/platforms/*/sideseat.exe
	@rm -rf sdk/js/dist
	@rm -rf sdk/python/dist
	@rm -rf $(RELEASE_DIR)
	@echo "[clean] Done. target/ and web/dist are gone; the next Rust build is cold."
	@echo "[clean] The API crate recreates web/dist as a placeholder on the next build - run make build-web for the real UI."

# Aliases
run: dev ## Alias for dev
start: dev ## Alias for dev
