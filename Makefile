.PHONY: help setup setup-ci setup-cross setup-hooks dev dev-server dev-web dev-docs build build-server build-web build-docs test fmt fmt-check lint clean build-cli build-cli-local publish-cli publish-cli-dry-run version version-patch version-minor version-major run start

# Variables
DIRS := data server/migrations web/dist docs/dist
WEB_DIR := web
DOCS_DIR := docs
CARGO_FLAGS := --release
MACOSX_DEPLOYMENT_TARGET := 14.0

# Default target
.DEFAULT_GOAL := help

# Help
help:
	@echo "SideSeat Development Commands"
	@echo ""
	@echo "Setup & Development:"
	@echo "  make setup           - First-time setup (install deps, create dirs)"
	@echo "  make setup-hooks     - Install git hooks (pre-commit formatting)"
	@echo "  make dev             - Start both servers with hot reload"
	@echo "  make dev-server      - Start only Rust server"
	@echo "  make dev-web         - Start only Vite dev server"
	@echo "  make dev-docs        - Start Astro docs dev server"
	@echo ""
	@echo "Build & Test:"
	@echo "  make build           - Production build (web + server)"
	@echo "  make build-docs      - Build Astro documentation"
	@echo "  make test            - Run all tests"
	@echo "  make fmt             - Format all code"
	@echo "  make fmt-check       - Check code formatting (CI)"
	@echo "  make lint            - Lint all code"
	@echo "  make clean           - Clean build artifacts"
	@echo ""
	@echo "Cross-Platform CLI Building:"
	@echo "  make setup-cross         - Install cross-compilation tools (zig, cargo-zigbuild, mingw-w64)"
	@echo "  make build-cli-local     - Build CLI for current platform only (fast)"
	@echo "  make build-cli           - Build CLI for ALL platforms (requires setup-cross)"
	@echo ""
	@echo "NPM Publishing:"
	@echo "  make version             - Show current version"
	@echo "  make version-patch       - Bump patch version (1.0.0 -> 1.0.1)"
	@echo "  make version-minor       - Bump minor version (1.0.0 -> 1.1.0)"
	@echo "  make version-major       - Bump major version (1.0.0 -> 2.0.0)"
	@echo "  make publish-cli-dry-run - Test package contents without publishing"
	@echo "  make publish-cli         - Build and publish CLI package to npm (interactive)"
	@echo ""
	@echo "Prerequisites for cross-compilation:"
	@echo "  • Rust toolchain (rustup)"
	@echo "  • Zig compiler"
	@echo "  • cargo-zigbuild"
	@echo "  • MinGW-w64 (for Windows builds)"
	@echo ""
	@echo "Supported platforms:"
	@echo "  • macOS (Intel & Apple Silicon)"
	@echo "  • Linux (x64 & ARM64)"
	@echo "  • Windows (x64)"

# Prerequisites check
check-prereqs:
	@command -v rustc >/dev/null 2>&1 || (echo "Error: Rust not found. Install from https://rustup.rs" && exit 1)
	@command -v node >/dev/null 2>&1 || (echo "Error: Node.js not found" && exit 1)

# Setup targets
setup: check-prereqs
	@echo "✓ Prerequisites OK"
	@if [ "$$(uname)" = "Darwin" ]; then \
		command -v watchexec >/dev/null 2>&1 || (echo "Installing watchexec..." && brew install watchexec); \
	else \
		cargo install cargo-watch 2>/dev/null || echo "✓ cargo-watch already installed"; \
	fi
	@cd $(WEB_DIR) && npm install
	@cd $(DOCS_DIR) && npm install
	@mkdir -p $(DIRS) && touch web/dist/.gitkeep docs/dist/.gitkeep
	@[ -f .env ] || (cp .env.example .env && echo "✓ Created .env file")
	@echo "✓ Setup complete! Run 'make dev' to start."

setup-ci:
	@cd $(WEB_DIR) && npm ci
	@mkdir -p $(DIRS) && touch web/dist/.gitkeep
	@echo "✓ CI setup complete"

setup-hooks:
	@echo "Installing git hooks..."
	@./scripts/setup-hooks.sh

# Cross-compilation setup
RUST_TARGETS := x86_64-apple-darwin aarch64-apple-darwin x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu x86_64-pc-windows-gnu

setup-cross: check-prereqs
	@echo "=== Cross-Compilation Setup ==="
	@echo ""
	@echo "Step 1/4: Installing Rust targets..."
	@$(foreach target,$(RUST_TARGETS),rustup target add $(target) 2>/dev/null || echo "  ✓ $(target)";)
	@echo ""
	@echo "Step 2/4: Installing Zig compiler..."
	@if [ "$$(uname)" = "Darwin" ]; then \
		if command -v brew >/dev/null 2>&1; then \
			brew install zig 2>/dev/null || echo "  ✓ zig already installed"; \
		else \
			echo "  ⚠️  Homebrew not found"; \
			echo "  Install zig manually: https://ziglang.org/download/"; \
			exit 1; \
		fi \
	elif [ "$$(uname)" = "Linux" ]; then \
		if command -v apt-get >/dev/null 2>&1; then \
			echo "  Run: sudo apt-get install -y zig"; \
		elif command -v yum >/dev/null 2>&1; then \
			echo "  Run: sudo yum install -y zig"; \
		else \
			echo "  Install zig: https://ziglang.org/download/"; \
		fi; \
		if ! command -v zig >/dev/null 2>&1; then \
			echo "  Waiting for zig installation..."; \
			read -p "  Press Enter after installing zig..."; \
		fi \
	fi
	@command -v zig >/dev/null 2>&1 || (echo "  ✗ zig not found" && exit 1)
	@echo "  ✓ zig installed ($$(zig version))"
	@echo ""
	@echo "Step 3/4: Installing cargo-zigbuild..."
	@cargo install cargo-zigbuild 2>/dev/null || echo "  ✓ cargo-zigbuild already installed"
	@echo "  ✓ cargo-zigbuild installed ($$(cargo zigbuild --version | head -1))"
	@echo ""
	@echo "Step 4/4: Installing MinGW-w64 (for Windows builds)..."
	@if [ "$$(uname)" = "Darwin" ]; then \
		if command -v brew >/dev/null 2>&1; then \
			brew install mingw-w64 2>/dev/null || echo "  ✓ mingw-w64 already installed"; \
		else \
			echo "  ⚠️  Homebrew not found, skipping MinGW-w64"; \
		fi; \
		if command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then \
			echo "  ✓ mingw-w64 installed ($$(x86_64-w64-mingw32-gcc --version | head -1))"; \
		else \
			echo "  ⚠️  mingw-w64 not found (Windows builds may fail)"; \
		fi \
	elif [ "$$(uname)" = "Linux" ]; then \
		if command -v apt-get >/dev/null 2>&1; then \
			echo "  Run: sudo apt-get install -y mingw-w64"; \
		elif command -v yum >/dev/null 2>&1; then \
			echo "  Run: sudo yum install -y mingw64-gcc"; \
		fi; \
		if command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then \
			echo "  ✓ mingw-w64 installed"; \
		else \
			echo "  ⚠️  mingw-w64 not found (Windows builds will fail)"; \
		fi \
	fi
	@echo ""
	@echo "=== ✓ Cross-compilation setup complete! ==="
	@echo ""
	@echo "Available targets:"
	@echo "  • x86_64-apple-darwin (macOS Intel)"
	@echo "  • aarch64-apple-darwin (macOS Apple Silicon)"
	@echo "  • x86_64-unknown-linux-gnu (Linux x64)"
	@echo "  • aarch64-unknown-linux-gnu (Linux ARM64)"
	@echo "  • x86_64-pc-windows-gnu (Windows x64)"
	@echo ""
	@echo "Run: make build-cli"

# Development targets
dev:
	@echo "Starting SideSeat development servers..."
	@echo "Frontend: http://localhost:5002"
	@echo "Backend: http://localhost:5001"
	@trap 'kill 0' EXIT; $(MAKE) dev-server & sleep 2 && $(MAKE) dev-web & wait

# Use file-based secrets in dev mode to avoid keychain prompts
dev-server:
	@if command -v watchexec >/dev/null 2>&1; then \
		cd server && watchexec -r -e rs,toml -- "SIDESEAT_SECRET_BACKEND=file cargo run"; \
	elif command -v cargo-watch >/dev/null 2>&1; then \
		cd server && cargo watch -s "SIDESEAT_SECRET_BACKEND=file cargo run"; \
	else \
		echo "⚠️  No watch tool found. Install with: brew install watchexec"; \
		echo "Running without hot reload..."; \
		cd server && SIDESEAT_SECRET_BACKEND=file cargo run; \
	fi

dev-web:
	@cd $(WEB_DIR) && npm run dev

dev-docs:
	@echo "Starting Astro docs dev server..."
	@cd $(DOCS_DIR) && npm run dev

# Build targets
build: build-web build-server

build-web:
	@echo "Building frontend..."
	@rm -rf $(WEB_DIR)/dist
	@cd $(WEB_DIR) && npm run build
	@echo "✓ Frontend built to $(WEB_DIR)/dist"

build-server: build-web
	@echo "Building backend..."
	@MACOSX_DEPLOYMENT_TARGET=$(MACOSX_DEPLOYMENT_TARGET) cargo build $(CARGO_FLAGS)
	@echo "✓ Backend built to target/release/sideseat"

build-docs:
	@echo "Building Astro documentation..."
	@rm -rf $(DOCS_DIR)/dist
	@cd $(DOCS_DIR) && npx astro build
	@echo "✓ Docs built to $(DOCS_DIR)/dist"

# Testing & quality
test:
	@cargo test
	@cd $(WEB_DIR) && npm test || echo "No tests found"

fmt:
	@cargo fmt --all
	@cd $(WEB_DIR) && npm run fmt

fmt-check:
	@cargo fmt --all --check
	@cd $(WEB_DIR) && npx prettier --check "src/**/*.{ts,tsx,css}"

lint:
	@cargo clippy --all-targets --all-features -- -D warnings
	@cd $(WEB_DIR) && npm run lint

# Cleanup
clean:
	@cargo clean
	@rm -rf $(WEB_DIR)/dist/* $(DOCS_DIR)/dist/* target cli/bin/sideseat-* data/*.db*
	@mkdir -p $(WEB_DIR)/dist $(DOCS_DIR)/dist && touch $(WEB_DIR)/dist/.gitkeep $(DOCS_DIR)/dist/.gitkeep
	@echo "✓ Clean complete"

# CLI build targets
CLI_BIN_DIR := cli/bin
CLI_TARGETS := \
	darwin-x64:x86_64-apple-darwin \
	darwin-arm64:aarch64-apple-darwin \
	linux-x64:x86_64-unknown-linux-gnu \
	linux-arm64:aarch64-unknown-linux-gnu \
	win32-x64:x86_64-pc-windows-gnu

check-cross-tools:
	@command -v zig >/dev/null 2>&1 || (echo "Error: zig not found. Run: make setup-cross" && exit 1)
	@command -v cargo-zigbuild >/dev/null 2>&1 || (echo "Error: cargo-zigbuild not found. Run: make setup-cross" && exit 1)

build-cli: build-web
	@echo "Building CLI for all platforms..."
	@mkdir -p $(CLI_BIN_DIR)
	@command -v zig >/dev/null 2>&1 || (echo "Error: zig not found. Run: make setup-cross" && exit 1)
	@command -v cargo-zigbuild >/dev/null 2>&1 || (echo "Error: cargo-zigbuild not found. Run: make setup-cross" && exit 1)
	@if [ "$$(uname)" = "Darwin" ]; then \
		echo "Building macOS Intel..."; \
		MACOSX_DEPLOYMENT_TARGET=$(MACOSX_DEPLOYMENT_TARGET) cargo build $(CARGO_FLAGS) --target x86_64-apple-darwin; \
		cp target/x86_64-apple-darwin/release/sideseat $(CLI_BIN_DIR)/sideseat-darwin-x64; \
		echo "Building macOS ARM64..."; \
		MACOSX_DEPLOYMENT_TARGET=$(MACOSX_DEPLOYMENT_TARGET) cargo build $(CARGO_FLAGS) --target aarch64-apple-darwin; \
		cp target/aarch64-apple-darwin/release/sideseat $(CLI_BIN_DIR)/sideseat-darwin-arm64; \
		echo "Building Linux x64..."; \
		cargo zigbuild $(CARGO_FLAGS) --target x86_64-unknown-linux-gnu; \
		cp target/x86_64-unknown-linux-gnu/release/sideseat $(CLI_BIN_DIR)/sideseat-linux-x64; \
		echo "Building Linux ARM64..."; \
		cargo zigbuild $(CARGO_FLAGS) --target aarch64-unknown-linux-gnu; \
		cp target/aarch64-unknown-linux-gnu/release/sideseat $(CLI_BIN_DIR)/sideseat-linux-arm64; \
		echo "Building Windows x64..."; \
		cargo zigbuild $(CARGO_FLAGS) --target x86_64-pc-windows-gnu; \
		cp target/x86_64-pc-windows-gnu/release/sideseat.exe $(CLI_BIN_DIR)/sideseat-win32-x64.exe; \
	else \
		echo "Building all targets with zigbuild..."; \
		echo "Building macOS Intel..."; \
		cargo zigbuild $(CARGO_FLAGS) --target x86_64-apple-darwin; \
		cp target/x86_64-apple-darwin/release/sideseat $(CLI_BIN_DIR)/sideseat-darwin-x64; \
		echo "Building macOS ARM64..."; \
		cargo zigbuild $(CARGO_FLAGS) --target aarch64-apple-darwin; \
		cp target/aarch64-apple-darwin/release/sideseat $(CLI_BIN_DIR)/sideseat-darwin-arm64; \
		echo "Building Linux x64..."; \
		cargo zigbuild $(CARGO_FLAGS) --target x86_64-unknown-linux-gnu; \
		cp target/x86_64-unknown-linux-gnu/release/sideseat $(CLI_BIN_DIR)/sideseat-linux-x64; \
		echo "Building Linux ARM64..."; \
		cargo zigbuild $(CARGO_FLAGS) --target aarch64-unknown-linux-gnu; \
		cp target/aarch64-unknown-linux-gnu/release/sideseat $(CLI_BIN_DIR)/sideseat-linux-arm64; \
		echo "Building Windows x64..."; \
		cargo zigbuild $(CARGO_FLAGS) --target x86_64-pc-windows-gnu; \
		cp target/x86_64-pc-windows-gnu/release/sideseat.exe $(CLI_BIN_DIR)/sideseat-win32-x64.exe; \
	fi
	@echo "✓ All binaries built successfully!"
	@ls -lh $(CLI_BIN_DIR)/

build-cli-local: build-web
	@echo "Building CLI for current platform..."
	@mkdir -p $(CLI_BIN_DIR)
	@MACOSX_DEPLOYMENT_TARGET=$(MACOSX_DEPLOYMENT_TARGET) cargo build $(CARGO_FLAGS)
	@if [ "$$(uname)" = "Darwin" ]; then \
		if [ "$$(uname -m)" = "arm64" ]; then \
			cp target/release/sideseat $(CLI_BIN_DIR)/sideseat-darwin-arm64 && echo "✓ Built for macOS ARM64"; \
		else \
			cp target/release/sideseat $(CLI_BIN_DIR)/sideseat-darwin-x64 && echo "✓ Built for macOS Intel"; \
		fi; \
	elif [ "$$(uname)" = "Linux" ]; then \
		[ "$$(uname -m)" = "aarch64" ] && \
			cp target/release/sideseat $(CLI_BIN_DIR)/sideseat-linux-arm64 && echo "✓ Built for Linux ARM64" || \
			cp target/release/sideseat $(CLI_BIN_DIR)/sideseat-linux-x64 && echo "✓ Built for Linux x64"; \
	elif echo "$$(uname)" | grep -qE "MINGW|MSYS"; then \
		cp target/release/sideseat.exe $(CLI_BIN_DIR)/sideseat-win32-x64.exe && echo "✓ Built for Windows x64"; \
	fi

# NPM Publishing
CLI_DIR := cli

publish-cli: build-cli
	@echo "Publishing CLI package to npm..."
	@echo ""
	@echo "Current version: $$(cd $(CLI_DIR) && node -p "require('./package.json').version")"
	@echo ""
	@echo "Select version bump:"
	@echo "  1) patch (1.0.0 -> 1.0.1)"
	@echo "  2) minor (1.0.0 -> 1.1.0)"
	@echo "  3) major (1.0.0 -> 2.0.0)"
	@echo "  4) skip (keep current version)"
	@read -p "Choice [1-4]: " -n 1 -r choice; \
	echo; \
	case $$choice in \
		1) cd $(CLI_DIR) && npm version patch --no-git-tag-version;; \
		2) cd $(CLI_DIR) && npm version minor --no-git-tag-version;; \
		3) cd $(CLI_DIR) && npm version major --no-git-tag-version;; \
		4) echo "Keeping current version";; \
		*) echo "Invalid choice, keeping current version";; \
	esac
	@echo "New version: $$(cd $(CLI_DIR) && node -p "require('./package.json').version")"
	@echo ""
	@echo "Step 1/3: Verifying binaries..."
	@if [ ! -f $(CLI_BIN_DIR)/sideseat-darwin-x64 ] || \
	    [ ! -f $(CLI_BIN_DIR)/sideseat-darwin-arm64 ] || \
	    [ ! -f $(CLI_BIN_DIR)/sideseat-linux-x64 ] || \
	    [ ! -f $(CLI_BIN_DIR)/sideseat-linux-arm64 ] || \
	    [ ! -f $(CLI_BIN_DIR)/sideseat-win32-x64.exe ]; then \
		echo "✗ Error: Not all binaries built. Run 'make build-cli' first."; \
		exit 1; \
	fi
	@ls -lh $(CLI_BIN_DIR)/
	@echo "✓ All binaries present"
	@echo ""
	@echo "Step 2/3: Verifying package files..."
	@if [ ! -f $(CLI_DIR)/README.md ]; then \
		echo "✗ Error: README.md not found in $(CLI_DIR)"; \
		exit 1; \
	fi
	@if [ ! -f $(CLI_DIR)/package.json ]; then \
		echo "✗ Error: package.json not found in $(CLI_DIR)"; \
		exit 1; \
	fi
	@echo "✓ README.md exists ($$(wc -l < $(CLI_DIR)/README.md) lines)"
	@echo "✓ package.json exists"
	@echo ""
	@echo "Step 3/3: Testing package..."
	@cd $(CLI_DIR) && npm pack --dry-run
	@echo ""
	@read -p "Publish to npm? [y/N] " -n 1 -r; \
	echo; \
	if [[ $$REPLY =~ ^[Yy]$$ ]]; then \
		cd $(CLI_DIR) && npm publish; \
		echo ""; \
		echo "✓ Published to npm!"; \
	else \
		echo "Publish cancelled."; \
	fi

publish-cli-dry-run: build-cli
	@echo "Dry-run: Testing CLI package..."
	@cd $(CLI_DIR) && npm pack --dry-run
	@echo ""
	@echo "Package contents:"
	@cd $(CLI_DIR) && npm pack && tar -tzf *.tgz && rm -f *.tgz

# Version management
# Helper to sync version to Cargo.toml
sync-cargo-version = NEW_VERSION=$$(cd $(CLI_DIR) && node -p "require('./package.json').version") && \
	sed -i '' "s/^version = \".*\"/version = \"$$NEW_VERSION\"/" server/Cargo.toml && \
	echo "✓ Updated server/Cargo.toml to $$NEW_VERSION"

version-patch:
	@echo "Bumping patch version..."
	@cd $(CLI_DIR) && npm version patch --no-git-tag-version
	@$(sync-cargo-version)

version-minor:
	@echo "Bumping minor version..."
	@cd $(CLI_DIR) && npm version minor --no-git-tag-version
	@$(sync-cargo-version)

version-major:
	@echo "Bumping major version..."
	@cd $(CLI_DIR) && npm version major --no-git-tag-version
	@$(sync-cargo-version)

version:
	@echo "CLI version:    $$(cd $(CLI_DIR) && node -p "require('./package.json').version")"
	@echo "Server version: $$(grep '^version' server/Cargo.toml | head -1 | sed 's/.*\"\(.*\)\"/\1/')"

# Aliases
run: dev
start: dev
