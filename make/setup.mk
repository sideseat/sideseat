##@ Setup

.PHONY: update-python-deps setup setup-hooks

# Normal uv commands use committed lockfiles. This target is the explicit upgrade path.
update-python-deps: ## Upgrade every Python lockfile
	@set -e; for manifest in $$(git ls-files '*pyproject.toml'); do \
		project=$$(dirname "$$manifest"); \
		echo "[update-python-deps] $$project"; \
		(cd "$$project" && uv lock --upgrade); \
	done

setup: ## Install development dependencies and hooks
	@echo "[setup] Checking prerequisites..."
	@command -v node >/dev/null 2>&1 || { echo "Error: node not found. Install Node.js ^22.22.0 || ^24.0.0 || >=26.0.0"; exit 1; }
	@# Bootstrap check before npm dependencies exist; `make node-floor` validates
	@# this declared range against every lockfile after setup.
	@node -e 'var v=process.versions.node.split(".").map(Number), ok=(v[0]===22 && v[1]>=22) || v[0]===24 || v[0]>=26; if (!ok) { console.error("Error: Node " + process.versions.node + " cannot build this repository. It needs ^22.22.0 || ^24.0.0 || >=26.0.0 (CI uses 24)."); process.exit(1); }'
	@command -v cargo >/dev/null 2>&1 || { echo "Error: cargo not found. Install Rust"; exit 1; }
	@command -v uv >/dev/null 2>&1 || { echo "Error: uv not found. Install with: curl -LsSf https://astral.sh/uv/install.sh | sh"; exit 1; }
	@command -v $(DOTNET) >/dev/null 2>&1 || { echo "Error: dotnet not found. Install .NET 10 SDK"; exit 1; }
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
