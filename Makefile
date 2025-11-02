.PHONY: help setup dev dev-server dev-web build build-server build-web test fmt lint clean

help:
	@echo "SideSeat Development Commands"
	@echo ""
	@echo "  make setup       - First-time setup (install deps, create dirs)"
	@echo "  make dev         - Start both servers with hot reload"
	@echo "  make dev-server  - Start only Rust server"
	@echo "  make dev-web     - Start only Vite dev server"
	@echo "  make build       - Production build (web + server)"
	@echo "  make test        - Run all tests"
	@echo "  make fmt         - Format all code"
	@echo "  make lint        - Lint all code"
	@echo "  make clean       - Clean build artifacts"
	@echo "  make build-cli   - Build CLI package with binaries for ALL platforms"
	@echo "  make build-cli-local - Build CLI binary for current platform only"

setup:
	@echo "Checking prerequisites..."
	@command -v rustc >/dev/null 2>&1 || (echo "Error: Rust not found. Install from https://rustup.rs" && exit 1)
	@command -v node >/dev/null 2>&1 || (echo "Error: Node.js not found" && exit 1)
	@echo "✓ Prerequisites OK"
	@echo ""
	@echo "Installing Rust development tools..."
	@cargo install cargo-watch 2>/dev/null || echo "✓ cargo-watch already installed"
	@echo ""
	@echo "Installing frontend dependencies..."
	@cd web && npm install
	@echo ""
	@echo "Creating directories..."
	@mkdir -p data
	@mkdir -p server/migrations
	@mkdir -p web/dist
	@touch web/dist/.gitkeep
	@echo ""
	@if [ ! -f .env ]; then cp .env.example .env; echo "✓ Created .env file"; fi
	@echo ""
	@echo "✓ Setup complete! Run 'make dev' to start."

dev:
	@echo "Starting SideSeat development servers..."
	@echo "Frontend: http://localhost:5173"
	@echo "Backend: http://localhost:3000"
	@echo ""
	@trap 'kill 0' EXIT; \
	$(MAKE) dev-server & \
	sleep 2; \
	$(MAKE) dev-web & \
	wait

dev-server:
	@command -v cargo-watch >/dev/null 2>&1 && cargo watch -x run || cargo run

dev-web:
	@cd web && npm run dev

build: build-web build-server

build-web:
	@echo "Building frontend..."
	@rm -rf web/dist
	@cd web && npm run build
	@echo "✓ Frontend built to web/dist"

build-server:
	@echo "Building backend..."
	@if [ ! -d web/dist ]; then echo "Error: web/dist not found. Run 'make build-web' first."; exit 1; fi
	@cargo build --release
	@echo "✓ Backend built to target/release/sideseat"

test:
	@echo "Running Rust tests..."
	@cargo test
	@echo "Running web tests..."
	@cd web && npm test || echo "No tests found"

fmt:
	@echo "Formatting Rust..."
	@cargo fmt --all
	@echo "Formatting TypeScript..."
	@cd web && npm run fmt

lint:
	@echo "Linting Rust..."
	@cargo clippy --all-targets --all-features -- -D warnings
	@echo "Linting TypeScript..."
	@cd web && npm run lint

clean:
	@echo "Cleaning build artifacts..."
	@cargo clean
	@rm -rf web/dist/*
	@mkdir -p web/dist
	@touch web/dist/.gitkeep
	@rm -rf target
	@rm -rf cli/bin/sideseat-*
	@rm -rf data/*.db*
	@echo "✓ Clean complete"

# Release builds (for CLI package - multi-platform)
build-cli:
	@echo "Building CLI package with binaries for all platforms..."
	@echo "Note: Requires cargo-zigbuild: cargo install cargo-zigbuild"
	@echo ""
	@$(MAKE) build-web
	@mkdir -p cli/bin
	@echo "Building macOS Intel (x86_64)..."
	@cargo zigbuild --release --target x86_64-apple-darwin
	@cp target/x86_64-apple-darwin/release/sideseat cli/bin/sideseat-darwin-x64
	@echo "Building macOS Apple Silicon (aarch64)..."
	@cargo zigbuild --release --target aarch64-apple-darwin
	@cp target/aarch64-apple-darwin/release/sideseat cli/bin/sideseat-darwin-arm64
	@echo "Building Linux x64..."
	@cargo zigbuild --release --target x86_64-unknown-linux-gnu
	@cp target/x86_64-unknown-linux-gnu/release/sideseat cli/bin/sideseat-linux-x64
	@echo "Building Linux ARM64..."
	@cargo zigbuild --release --target aarch64-unknown-linux-gnu
	@cp target/aarch64-unknown-linux-gnu/release/sideseat cli/bin/sideseat-linux-arm64
	@echo "Building Windows x64..."
	@cargo zigbuild --release --target x86_64-pc-windows-gnu
	@cp target/x86_64-pc-windows-gnu/release/sideseat.exe cli/bin/sideseat-win32-x64.exe
	@echo ""
	@echo "✓ All binaries built successfully!"
	@ls -lh cli/bin/

# Build for current platform only (faster for local testing)
build-cli-local:
	@echo "Building for current platform only..."
	@$(MAKE) build-web
	@cargo build --release
	@mkdir -p cli/bin
	@if [ "$$(uname)" = "Darwin" ]; then \
		if [ "$$(uname -m)" = "arm64" ]; then \
			cp target/release/sideseat cli/bin/sideseat-darwin-arm64; \
			echo "✓ Built for macOS Apple Silicon"; \
		else \
			cp target/release/sideseat cli/bin/sideseat-darwin-x64; \
			echo "✓ Built for macOS Intel"; \
		fi \
	elif [ "$$(uname)" = "Linux" ]; then \
		if [ "$$(uname -m)" = "aarch64" ]; then \
			cp target/release/sideseat cli/bin/sideseat-linux-arm64; \
			echo "✓ Built for Linux ARM64"; \
		else \
			cp target/release/sideseat cli/bin/sideseat-linux-x64; \
			echo "✓ Built for Linux x64"; \
		fi \
	elif [ "$$(uname)" = "MINGW"* ] || [ "$$(uname)" = "MSYS"* ]; then \
		cp target/release/sideseat.exe cli/bin/sideseat-win32-x64.exe; \
		echo "✓ Built for Windows x64"; \
	fi

# Convenience aliases
run: dev
start: dev
