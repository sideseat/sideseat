##@ Format, lint, and hardening

.PHONY: fmt fmt-check file-length-check lint lint-advisory secret-scan-tree secret-scan-staged secret-scan-range fmt-check-python lint-python harden harden-supply harden-spec audit msrv

fmt: ## Format all source code
	@echo "[fmt] Formatting code..."
	@cargo fmt
	@[ -x "$(PRETTIER)" ] || { echo "Error: prettier not installed. Run 'make setup'."; exit 1; }
	@$(PRETTIER) --write "web/src/**/*.{ts,tsx,css,json}" "sdk/js/src/**/*.ts" "examples/javascript/{harness,strands,vercel-ai,claude-agent-sdk}/**/*.ts"
	@uv run --locked ruff format $(PYTHON_CHECKED)
	@echo "[fmt] Done"

fmt-check: ## Check source formatting
	@echo "[fmt-check] Checking formatting..."
	@cargo fmt --check
	@[ -x "$(PRETTIER)" ] || { echo "Error: prettier not installed. Run 'make setup'."; exit 1; }
	@$(PRETTIER) --check "web/src/**/*.{ts,tsx,css,json}" "sdk/js/src/**/*.ts" "examples/javascript/{harness,strands,vercel-ai,claude-agent-sdk}/**/*.ts"
	@$(MAKE) --no-print-directory fmt-check-python

file-length-check: ## Enforce the hard 1000-line source-file limit
	@./scripts/check/file-lengths.sh

lint: ## Run all linters
	@echo "[lint] Running linters..."
	@$(MAKE) --no-print-directory file-length-check
	$(call run-with-disk-guard,cargo clippy --locked --all-targets -- -D warnings)
	@command -v cargo-machete >/dev/null 2>&1 || { echo "[lint] cargo-machete is required: mise install"; exit 1; }
	@cargo machete
	@cd $(WEB_DIR) && npm run lint
	@cd sdk/js && npm run lint
	@cd examples/javascript && npm run lint
	@cd examples/javascript && npm run typecheck
	@$(MAKE) --no-print-directory lint-python
	@# The dev group owns mypy.
	@cd sdk/python && uv run --locked mypy src

# Advisory clippy lints, kept out of `lint` because that gate runs -D warnings and these
# are suggestions rather than defects. Non-blocking by design: review the output, do not
# gate on it. Keep this list identical to the advisory block in Cargo.toml.
lint-advisory: ## Run informational Clippy lints
	@echo "[lint-advisory] Advisory clippy lints (informational, does not fail)..."
	$(call run-with-disk-guard,cargo clippy --locked --all-targets -- \
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
		2>&1 | grep -E '^(warning|  -->)' | head -60 || true)

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

# =============================================================================
# Hardening gates
#
# Classes the compiler and test suite cannot see. Every gate here is blocking; the tools come from
# mise.toml (`mise install`).
# =============================================================================

# Python source roots covered by the shared format and lint gates.
PYTHON_CHECKED := sdk/python examples/python examples/cli scripts

fmt-check-python:
	@uv run --locked ruff format --check $(PYTHON_CHECKED)

lint-python:
	@uv run --locked ruff check $(PYTHON_CHECKED)

harden: harden-supply harden-spec ## Run supply-chain and specification gates
	@echo "[harden] All hardening gates passed"

harden-supply: audit secret-scan-tree ## Audit dependencies and secrets

# Needs the network, and its answer changes without the code changing, so it is not part of `check`: run it
# before a release and periodically.
audit: ## Audit every dependency graph for advisories, licences and bans
	@./scripts/check/audit.sh

# The floor `rust-version` in Cargo.toml promises. Declaring it refuses an older toolchain and clippy's
# incompatible_msrv catches an item that is too new, but neither compiles the workspace on that version.
MSRV := 1.94.1
msrv: ## Compile the whole workspace on the minimum supported Rust version
	@rustup toolchain install $(MSRV) --profile minimal >/dev/null
	@RUSTUP_TOOLCHAIN=$(MSRV) cargo check --locked --workspace --all-targets

# Every specification must have a matching configuration and pass TLC. The
# versioned tool archive is digest-checked on every run. Model checking remains
# separate from `check` because it takes minutes.
TLA_VERSION := 1.8.0
TLA_SHA256  := db131ddb48e7004d823bef4493df7b35694babe37505b9d9fa5685e7a331f1f1
TLA_DIR     := scripts/tools/tla
TLA_JAR     := $(TLA_DIR)/tla2tools-$(TLA_VERSION).jar

harden-spec: ## Model-check every TLA+ specification
	@if [ ! -f $(TLA_JAR) ]; then \
		echo "[harden-spec] fetching tla2tools $(TLA_VERSION)..."; \
		mkdir -p $(TLA_DIR); \
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
		for tla in server/specs/*.tla; do \
			[ "$${tla#*_TTrace_}" = "$$tla" ] || continue; \
			[ -f "$${tla%.tla}.cfg" ] || echo "$$tla has no .cfg, so nothing model-checks it"; \
		done; \
		for cfg in server/specs/*.cfg; do \
			[ -f "$${cfg%.cfg}.tla" ] || echo "$$cfg configures no specification"; \
		done); \
	if [ -n "$$orphans" ]; then \
		echo "[harden-spec] specification and configuration do not pair up:"; \
		echo "$$orphans" | sed 's/^/  /'; \
		echo "[harden-spec] Add the missing file, or delete the one left over."; \
		exit 1; \
	fi
	@# Each model runs capped (disk, time, heap): see scripts/check/model-check.sh.
	@failed=0; \
	for tla in server/specs/*.tla; do \
		[ "$${tla#*_TTrace_}" = "$$tla" ] || continue; \
		spec=$$(basename $$tla .tla); \
		printf "[harden-spec] %-22s " "$$spec"; \
		./scripts/check/model-check.sh $(CURDIR)/$(TLA_JAR) server/specs $$spec || failed=1; \
	done; \
	rm -rf server/specs/states server/specs/*_TTrace_*.bin server/specs/*_TTrace_*.tla; \
	exit $$failed
