##@ Documentation

.PHONY: sync-protocol-schema docs-deps docs-system-deps build-docs dev-docs preview-docs

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
