# =============================================================================
# SideSeat Makefile
# =============================================================================
#
# Build, test, version, and publish orchestration for all SideSeat packages.
#
# PREREQUISITES
#   bash, make, node 20+, cargo, uv
#   Windows: use Git Bash or MSYS2 (not PowerShell/cmd)
#
# QUICK START
#   make setup       Install all dependencies
#   make dev         Start server + web dev servers
#   make check       Format check + lint + test (all components)
#
# WORKFLOWS
#
#   Local development:
#     setup -> dev -> fmt -> check
#
#   Single-platform build:
#     build-cli-darwin-arm64   (or any platform from PLATFORMS list)
#
#   Full cross-compile (macOS only, requires zig + mingw):
#     build-cli                (preflight -> build all 5 platforms -> smoke test)
#
#   Version bump:
#     bump TYPE=patch          (or minor, major; syncs all packages)
#
#   Publish (manual, after build):
#     publish-cli              Verify binaries -> publish 5 platform pkgs -> main pkg
#     publish-sdk-js           Build + publish @sideseat/sdk
#     publish-sdk-python       Build + publish sideseat (PyPI)
#     publish                  All of the above
#
#   Tagged release (CI/CD):
#     release TYPE=patch       check -> bump -> commit -> tag -> push
#                              GitHub Actions handles build + publish on tag push
#
# TARGETS
#
#   Setup:
#     setup              Install all dependencies (node, cargo, uv, hooks)
#     setup-ci           Install CI-only dependencies (npm ci, cargo fetch)
#     setup-hooks        Install git hooks
#
#   Development:
#     dev                Start server (5388) + web (5389) in parallel
#     dev-server         Start Rust server with hot reload (watchexec/cargo-watch)
#     dev-web            Start Vite dev server
#
#   Format & Lint:
#     fmt                Format all code (cargo fmt, prettier, ruff)
#     fmt-check          Check formatting without modifying
#     lint               Run all linters (clippy, eslint, ruff, mypy)
#     check              fmt-check + lint + test
#
#   Test:
#     test               Run all tests
#     test-server        Rust tests (cargo test)
#     test-web           Web tests (vitest)
#     test-sdk-js        JS SDK tests
#     test-sdk-python    Python SDK tests (pytest)
#     coverage           Tests with coverage (tarpaulin + vitest)
#
#   Build (local):
#     build              Production build (web + server, current platform)
#     build-web          Build frontend (requires node_modules)
#     build-server       Build backend (depends on build-web)
#
#   Build -- SDKs:
#     build-sdk          Build all SDKs
#     build-sdk-js       Build JS SDK (npm run build)
#     build-sdk-python   Build Python SDK (uv build)
#
#   Build -- CLI (cross-compile):
#     build-cli              All 5 platforms (preflight -> build -> smoke test)
#     build-cli-<platform>   Single platform, e.g. build-cli-darwin-arm64
#     build-cli-preflight    Verify tools (zig, mingw, rust targets)
#     build-cli-summary      Smoke test native binary + print sizes
#
#     Platforms: darwin-arm64, darwin-x64, linux-x64, linux-arm64, win32-x64
#     Requires: macOS host, cargo-zigbuild, zig, mingw-w64
#
#   Version:
#     version            Show all package versions
#     version-check      Verify all packages have the same version
#     bump               Bump version (TYPE=patch|minor|major, default: patch)
#     sync-version       Sync cli/package.json version to all other packages
#
#   Publish:
#     publish            Publish everything (CLI + JS SDK + Python SDK)
#     publish-cli        Publish CLI: verify binaries -> platform pkgs -> main pkg
#     publish-sdk-js     Build + publish @sideseat/sdk to npm
#     publish-sdk-python Build + publish sideseat to PyPI
#
#   Release:
#     release            Full release: check -> bump -> commit -> tag -> push
#                        Usage: make release TYPE=patch (or minor, major)
#
#   Docs:
#     build-docs         Build documentation site (Astro/Starlight)
#     dev-docs           Start docs dev server
#     docs-preview       Preview built docs
#
#   Utilities:
#     clean              Remove build artifacts (target, dist, sdk artifacts)
#     download-prices    Update LLM pricing data from litellm
#     deps-check         Check for outdated dependencies (all components)
#
#   Aliases:
#     run, start         -> dev
#
# PLATFORM CONFIG
#
#   Adding a new platform requires 4 lines:
#     RUST_TARGET_<name> := <rustc triple>
#     BUILD_CMD_<name>   := cargo build | cargo zigbuild
#     BIN_NAME_<name>    := sideseat | sideseat.exe
#   Then append <name> to the PLATFORMS list.
#
#   All loops (version-check, sync-version, clean, publish, summary)
#   derive from PLATFORMS automatically.
#
# VARIABLES
#
#   ARGS     Extra args passed to dev-server (e.g. make dev-server ARGS="--no-auth")
#   TYPE     Version bump type for bump/release (patch, minor, major; default: patch)
#
# =============================================================================
# Preamble
# =============================================================================

SHELL := /bin/bash
.DELETE_ON_ERROR:

# OS/arch detection
UNAME_S := $(shell uname -s 2>/dev/null || echo Windows)
UNAME_M := $(shell uname -m)

# =============================================================================
# Variables
# =============================================================================

ARGS ?=
TYPE ?= patch
SERVER_DIR := server
WEB_DIR := web
CLI_DIR := cli

# Pricing data
PRICES_URL := https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json
PRICES_FILE := $(SERVER_DIR)/data/model_prices_and_context_window.json

# =============================================================================
# Platform Config (single source of truth)
# =============================================================================

PLATFORMS := darwin-arm64 darwin-x64 linux-x64 linux-arm64 win32-x64

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
BUILD_CMD_win32-x64      := CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=$(CURDIR)/misc/scripts/mingw-static-link.sh cargo build
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
.PHONY: setup setup-ci setup-hooks
.PHONY: dev dev-server dev-web
.PHONY: fmt fmt-check lint check
.PHONY: test test-server test-web test-sdk-js test-sdk-python coverage
.PHONY: build build-web build-server
.PHONY: build-sdk build-sdk-js build-sdk-python
.PHONY: build-cli build-cli-preflight build-cli-summary $(CLI_BUILD_TARGETS)
.PHONY: version version-check bump sync-version
.PHONY: publish publish-cli publish-sdk-js publish-sdk-python
.PHONY: release
.PHONY: build-docs dev-docs docs-preview
.PHONY: clean download-prices deps-check run start

.SILENT: help version

.DEFAULT_GOAL := help

# =============================================================================
# Help
# =============================================================================

help:
	@echo "SideSeat Development Commands"
	@echo ""
	@echo "Prerequisites: bash, make, node 20+, cargo, uv"
	@echo "Windows: Use Git Bash or MSYS2 (not PowerShell/cmd)"
	@echo ""
	@echo "Setup:"
	@echo "  make setup           Install dependencies and git hooks"
	@echo "  make setup-hooks     Install git hooks only"
	@echo ""
	@echo "Development:"
	@echo "  make dev             Start server + web (parallel)"
	@echo "  make dev-server      Start Rust server with hot reload"
	@echo "  make dev-web         Start Vite dev server"
	@echo ""
	@echo "Build:"
	@echo "  make build           Production build (web + server)"
	@echo "  make build-sdk       Build all SDKs"
	@echo "  make build-sdk-js    Build JS SDK"
	@echo "  make build-sdk-python Build Python SDK"
	@echo "  make build-cli       Cross-compile all platforms for npm"
	@echo "  make build-cli-<p>   Build single platform (e.g. build-cli-darwin-arm64)"
	@echo ""
	@echo "Testing:"
	@echo "  make test            Run all tests"
	@echo "  make coverage        Run tests with coverage"
	@echo ""
	@echo "Quality:"
	@echo "  make fmt             Format all code"
	@echo "  make fmt-check       Check formatting"
	@echo "  make lint            Run linters"
	@echo "  make check           Run all checks (fmt-check + lint + test)"
	@echo ""
	@echo "Versioning:"
	@echo "  make version         Show current version"
	@echo "  make version-check   Verify all packages have matching versions"
	@echo "  make bump            Bump version (TYPE=patch|minor|major, default: patch)"
	@echo ""
	@echo "Publish:"
	@echo "  make publish         Publish everything (CLI + SDKs)"
	@echo "  make publish-cli     Publish CLI platform packages + main package"
	@echo "  make publish-sdk-js  Build and publish JS SDK"
	@echo "  make publish-sdk-python Build and publish Python SDK"
	@echo ""
	@echo "Release:"
	@echo "  make release TYPE=patch  Check, bump, commit, tag, push (triggers CI/CD)"
	@echo "  (TYPE can be patch, minor, or major)"
	@echo ""
	@echo "Docs:"
	@echo "  make build-docs      Build documentation site"
	@echo "  make dev-docs        Start docs dev server"
	@echo "  make docs-preview    Preview built docs"
	@echo ""
	@echo "Other:"
	@echo "  make deps-check      Check for outdated dependencies"
	@echo "  make download-prices Update LLM pricing data"
	@echo "  make clean           Remove build artifacts"

# =============================================================================
# Setup
# =============================================================================

setup:
	@echo "[setup] Checking prerequisites..."
	@command -v node >/dev/null 2>&1 || { echo "Error: node not found. Install Node.js 20+"; exit 1; }
	@command -v cargo >/dev/null 2>&1 || { echo "Error: cargo not found. Install Rust"; exit 1; }
	@command -v uv >/dev/null 2>&1 || { echo "Error: uv not found. Install with: curl -LsSf https://astral.sh/uv/install.sh | sh"; exit 1; }
	@echo "[setup] Installing workspace dev tools..."
	@uv sync --group dev
	@echo "[setup] Fetching Rust dependencies..."
	@cargo fetch
	@echo "[setup] Installing JS dependencies..."
	@cd $(WEB_DIR) && npm install
	@cd sdk/js && npm install
	@cd misc/samples/js && npm install
	@echo "[setup] Installing Python dependencies..."
	@cd sdk/python && uv sync --extra dev
	@cd misc/samples/python && uv sync --group dev
	@echo "[setup] Installing cargo-tarpaulin..."
	@cargo install cargo-tarpaulin --quiet
	@mkdir -p .sideseat
	@$(MAKE) --no-print-directory setup-hooks
	@echo "[setup] Done. Run 'make dev' to start."

setup-ci:
	@echo "[setup-ci] Installing CI dependencies..."
	@cd $(WEB_DIR) && npm ci
	@cd $(SERVER_DIR) && cargo fetch

setup-hooks:
	@[ -d .git ] || { echo "Error: Not a git repository"; exit 1; }
	@git config core.hooksPath .githooks || { echo "Error: git config failed"; exit 1; }
	@chmod +x .githooks/* 2>/dev/null || true
	@echo "[setup-hooks] Git hooks installed"

# =============================================================================
# Development
# =============================================================================

dev:
	@echo "[dev] Starting server (port 5388) and web (port 5389)..."
	@trap 'kill 0' EXIT && \
	$(MAKE) dev-server & \
	sleep 2 && \
	$(MAKE) dev-web & \
	wait

dev-server:
	@if command -v watchexec >/dev/null 2>&1; then \
		cd $(SERVER_DIR) && watchexec -r -e rs,toml -- \
			"SIDESEAT_LOG=debug SIDESEAT_DATA_DIR=../.sideseat SIDESEAT_SECRET_BACKEND=file cargo run -- $(ARGS)"; \
	elif command -v cargo-watch >/dev/null 2>&1; then \
		cd $(SERVER_DIR) && SIDESEAT_LOG=debug SIDESEAT_DATA_DIR=../.sideseat SIDESEAT_SECRET_BACKEND=file \
			cargo watch -x "run -- $(ARGS)"; \
	else \
		echo "No watch tool found. Install: brew install watchexec"; \
		cd $(SERVER_DIR) && SIDESEAT_LOG=debug SIDESEAT_DATA_DIR=../.sideseat SIDESEAT_SECRET_BACKEND=file \
			cargo run -- $(ARGS); \
	fi

dev-web:
	@cd $(WEB_DIR) && npm run dev

# =============================================================================
# Format & Lint
# =============================================================================

fmt:
	@echo "[fmt] Formatting code..."
	@cargo fmt
	@npx prettier --write "web/src/**/*.{ts,tsx,css,json}" "sdk/js/src/**/*.ts" "misc/samples/js/src/**/*.ts"
	@uv run ruff format sdk/python misc/samples/python
	@echo "[fmt] Done"

fmt-check:
	@echo "[fmt-check] Checking formatting..."
	@cargo fmt --check
	@npx prettier --check "web/src/**/*.{ts,tsx,css,json}" "sdk/js/src/**/*.ts" "misc/samples/js/src/**/*.ts"
	@uv run ruff format --check sdk/python misc/samples/python

lint:
	@echo "[lint] Running linters..."
	@cargo clippy --all-targets -- -D warnings
	@cd $(WEB_DIR) && npm run lint
	@cd sdk/js && npm run lint
	@cd misc/samples/js && npm run lint
	@uv run ruff check sdk/python misc/samples/python
	@cd sdk/python && uv run mypy src

check: fmt-check lint test
	@echo "[check] All checks passed"

# =============================================================================
# Test
# =============================================================================

test: test-server test-web test-sdk-js test-sdk-python

test-server:
	@echo "[test-server] Running Rust tests..."
	@cd $(SERVER_DIR) && cargo test

test-web:
	@echo "[test-web] Running web tests..."
	@cd $(WEB_DIR) && npm test -- --run

test-sdk-js:
	@echo "[test-sdk-js] Running JS SDK tests..."
	@cd sdk/js && npm test

test-sdk-python:
	@echo "[test-sdk-python] Running Python SDK tests..."
	@cd sdk/python && uv run pytest

coverage:
	@echo "[coverage] Running tests with coverage..."
	@command -v cargo-tarpaulin >/dev/null 2>&1 || { echo "Error: cargo-tarpaulin not installed. Install with: cargo install cargo-tarpaulin"; exit 1; }
	@echo "[coverage] Rust coverage..."
	@cd $(SERVER_DIR) && cargo tarpaulin --out Html --output-dir ../coverage
	@echo "[coverage] Rust report: coverage/tarpaulin-report.html"
	@echo "[coverage] Web coverage..."
	@cd $(WEB_DIR) && npm run test:coverage -- --run

# =============================================================================
# Build (local dev)
# =============================================================================

build: build-web build-server

build-web:
	@[ -d "$(WEB_DIR)/node_modules" ] || { echo "Error: $(WEB_DIR)/node_modules not found. Run 'make setup' first."; exit 1; }
	@echo "[build-web] Building frontend..."
	@cd $(WEB_DIR) && npm run build

build-server: build-web
	@echo "[build-server] Building backend..."
	@cd $(SERVER_DIR) && cargo build --release
	@echo "[build-server] Binary: target/release/sideseat"

# =============================================================================
# Build -- SDKs
# =============================================================================

build-sdk: build-sdk-js build-sdk-python

build-sdk-js:
	@echo "[build-sdk-js] Building JS SDK..."
	@cd sdk/js && npm run build

build-sdk-python:
	@echo "[build-sdk-python] Building Python SDK..."
	@cd sdk/python && uv build

# =============================================================================
# Build -- CLI (cross-compile all platforms)
# =============================================================================

# Per-platform targets (generated)
define MAKE_CLI_TARGET
build-cli-$(1): build-web
	@echo "[build-cli] $(1) ($(BUILD_CMD_$(1)))..."
	@cd $$(SERVER_DIR) && $(BUILD_CMD_$(1)) --release --target $(RUST_TARGET_$(1))
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
	@echo "[build-cli] All platform packages ready for npm publish"

# Orchestrator: preflight -> build all -> summary
build-cli: build-cli-preflight
	@echo "[build-cli] Building all platform binaries..."
	@$(MAKE) $(CLI_BUILD_TARGETS)
	@$(MAKE) build-cli-summary

# =============================================================================
# Version
# =============================================================================

version:
	@echo "CLI:       $$(node -p "require('./cli/package.json').version")"
	@echo "Server:    $$(grep '^version = ' server/Cargo.toml | head -1 | sed 's/.*\"\(.*\)\".*/\1/')"
	@echo "SDK (JS):  $$(node -p "require('./sdk/js/package.json').version")"
	@echo "SDK (Py):  $$(grep '__version__' sdk/python/src/sideseat/_version.py | sed 's/.*\"\(.*\)\".*/\1/')"

version-check:
	@CLI_VERSION=$$(node -p "require('./cli/package.json').version") && \
	SERVER_VERSION=$$(grep '^version = ' server/Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/') && \
	MISMATCHED="" && \
	if [ "$$CLI_VERSION" != "$$SERVER_VERSION" ]; then \
		MISMATCHED="$$MISMATCHED\n  server/Cargo.toml: $$SERVER_VERSION"; \
	fi && \
	JS_SDK_VERSION=$$(node -p "require('./sdk/js/package.json').version") && \
	if [ "$$CLI_VERSION" != "$$JS_SDK_VERSION" ]; then \
		MISMATCHED="$$MISMATCHED\n  sdk/js/package.json: $$JS_SDK_VERSION"; \
	fi && \
	PY_SDK_VERSION=$$(grep '__version__' sdk/python/src/sideseat/_version.py | sed 's/.*"\(.*\)".*/\1/') && \
	if [ "$$CLI_VERSION" != "$$PY_SDK_VERSION" ]; then \
		MISMATCHED="$$MISMATCHED\n  sdk/python/_version.py: $$PY_SDK_VERSION"; \
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

bump:
	@if [ "$(TYPE)" != "patch" ] && [ "$(TYPE)" != "minor" ] && [ "$(TYPE)" != "major" ]; then \
		echo "Error: TYPE must be patch, minor, or major (got: $(TYPE))"; \
		exit 1; \
	fi
	@echo "[bump] Bumping $(TYPE) version..."
	@cd $(CLI_DIR) && npm version $(TYPE) --no-git-tag-version
	@$(MAKE) --no-print-directory sync-version

sync-version:
	@NEW_VERSION=$$(node -p "require('./cli/package.json').version") && \
	TEMP_FILE=$$(mktemp) && \
	sed "s/^version = \".*\"/version = \"$$NEW_VERSION\"/" server/Cargo.toml > "$$TEMP_FILE" && \
	mv "$$TEMP_FILE" server/Cargo.toml && \
	CARGO_VERSION=$$(grep '^version = ' server/Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/') && \
	if [ "$$NEW_VERSION" != "$$CARGO_VERSION" ]; then \
		echo "Error: Version sync failed. Expected $$NEW_VERSION, got $$CARGO_VERSION"; \
		exit 1; \
	fi && \
	cargo update -p sideseat-server --quiet && \
	for pkg in $(PLATFORMS); do \
		node -e "const p=require('./cli/platforms/platform-'+'$$pkg'+'/package.json'); p.version='$$NEW_VERSION'; require('fs').writeFileSync('./cli/platforms/platform-'+'$$pkg'+'/package.json', JSON.stringify(p, null, 2)+'\n')"; \
	done && \
	node -e "const p=require('./cli/package.json'); Object.keys(p.optionalDependencies||{}).forEach(k=>p.optionalDependencies[k]='$$NEW_VERSION'); require('fs').writeFileSync('./cli/package.json', JSON.stringify(p, null, 2)+'\n')" && \
	node -e "const p=require('./sdk/js/package.json'); p.version='$$NEW_VERSION'; require('fs').writeFileSync('./sdk/js/package.json', JSON.stringify(p, null, 2)+'\n')" && \
	echo "export const VERSION = '$$NEW_VERSION';" > sdk/js/src/version.ts && \
	echo "__version__ = \"$$NEW_VERSION\"" > sdk/python/src/sideseat/_version.py && \
	echo "[sync-version] Version synced to $$NEW_VERSION"

# =============================================================================
# Publish
# =============================================================================

publish: publish-cli publish-sdk-js publish-sdk-python

publish-cli:
	@echo "[publish-cli] Verifying npm authentication..."
	@npm whoami >/dev/null 2>&1 || { echo "Error: Not logged in to npm. Run 'npm login' first."; exit 1; }
	@echo "[publish-cli] Verifying binaries exist..."
	@$(foreach p,$(PLATFORMS),[ -f "$(call cli-bin,$(p))" ] || { echo "Error: Missing binary for $(p): $(call cli-bin,$(p)). Run 'make build-cli' first."; exit 1; };)
	@$(MAKE) --no-print-directory version-check
	@echo "[publish-cli] Publishing platform packages..."
	@$(foreach p,$(PLATFORMS),(cd $(CLI_DIR)/platforms/platform-$(p) && npm publish --access public) &&) true
	@echo "[publish-cli] Waiting for registry propagation..."
	@sleep 5
	@echo "[publish-cli] Publishing main sideseat package..."
	@cd $(CLI_DIR) && npm publish --access public
	@echo "[publish-cli] Published $$(node -p "require('./cli/package.json').version")"

publish-sdk-js:
	@echo "[publish-sdk-js] Verifying npm authentication..."
	@npm whoami >/dev/null 2>&1 || { echo "Error: Not logged in to npm. Run 'npm login' first."; exit 1; }
	@echo "[publish-sdk-js] Building and publishing..."
	@cd sdk/js && npm ci && npm run build && npm publish --access public
	@echo "[publish-sdk-js] Published $$(node -p "require('./sdk/js/package.json').version")"

publish-sdk-python:
	@echo "[publish-sdk-python] Building and publishing..."
	@cd sdk/python && uv build && uv publish
	@echo "[publish-sdk-python] Published $$(grep '__version__' sdk/python/src/sideseat/_version.py | sed 's/.*\"\(.*\)\".*/\1/')"

# =============================================================================
# Release
# =============================================================================

release:
	@if [ "$(TYPE)" != "patch" ] && [ "$(TYPE)" != "minor" ] && [ "$(TYPE)" != "major" ]; then \
		echo "Error: TYPE must be patch, minor, or major (got: $(TYPE))"; \
		exit 1; \
	fi
	@echo "[release] Running pre-release checks..."
	@$(MAKE) check
	@echo "[release] Bumping $(TYPE) version..."
	@$(MAKE) bump TYPE=$(TYPE)
	@NEW_VERSION=$$(node -p "require('./cli/package.json').version") && \
	echo "[release] Committing version $$NEW_VERSION..." && \
	git add -A && \
	git commit -m "Release v$$NEW_VERSION" && \
	echo "[release] Creating tag v$$NEW_VERSION..." && \
	git tag "v$$NEW_VERSION" && \
	echo "[release] Pushing to remote..." && \
	git push && git push --tags && \
	echo "[release] Done. GitHub Actions will build and publish v$$NEW_VERSION"

# =============================================================================
# Documentation
# =============================================================================

build-docs:
	@echo "[build-docs] Building documentation..."
	@[ -d "docs/node_modules" ] || { cd docs && npm install; }
	@cd docs && npm run build
	@echo "[build-docs] Output: docs/dist/"

dev-docs:
	@echo "[dev-docs] Starting docs dev server..."
	@[ -d "docs/node_modules" ] || { cd docs && npm install; }
	@cd docs && npm run dev

docs-preview:
	@echo "[docs-preview] Previewing built docs..."
	@[ -d "docs/dist" ] || { $(MAKE) build-docs; }
	@cd docs && npm run preview

# =============================================================================
# Utilities
# =============================================================================

deps-check:
	@echo "[deps-check] Checking for outdated dependencies..."
	@echo ""
	@echo "=== Server (Rust) ==="
	@command -v cargo-outdated >/dev/null 2>&1 && cd $(SERVER_DIR) && cargo outdated -R || echo "Install cargo-outdated: cargo install cargo-outdated"
	@echo ""
	@echo "=== Web ==="
	@cd $(WEB_DIR) && npm outdated || true
	@echo ""
	@echo "=== JS SDK ==="
	@cd sdk/js && npm outdated || true
	@echo ""
	@echo "=== Python SDK ==="
	@cd sdk/python && uv pip list --outdated || true
	@echo ""
	@echo "=== Docs ==="
	@cd docs && npm outdated || true

download-prices:
	@echo "[download-prices] Downloading LLM pricing data..."
	@mkdir -p $(SERVER_DIR)/data
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

clean:
	@echo "[clean] Removing build artifacts..."
	@rm -rf target
	@rm -rf $(WEB_DIR)/dist
	@rm -rf $(WEB_DIR)/node_modules/.vite
	@rm -f $(CLI_DIR)/bin/sideseat-*
	@rm -f $(CLI_DIR)/platforms/*/sideseat $(CLI_DIR)/platforms/*/sideseat.exe
	@rm -rf sdk/js/dist
	@rm -rf sdk/python/dist
	@echo "[clean] Done"

# Aliases
run: dev
start: dev
