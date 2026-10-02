##@ Build

.PHONY: $(CLI_BUILD_TARGETS) build build-web build-server build-cli-preflight build-cli-summary build-cli build-docker publish-docker

build: build-web build-server ## Build the production web and server

build-web: ## Build the web application
	@[ -d "$(WEB_DIR)/node_modules" ] || { echo "Error: $(WEB_DIR)/node_modules not found. Run 'make setup' first."; exit 1; }
	@echo "[build-web] Building frontend..."
	@cd $(WEB_DIR) && npm run build

build-server: build-web ## Build the server
	@echo "[build-server] Building backend..."
	$(call run-with-disk-guard,cd $(SERVER_DIR) && cargo build --locked --release)
	@echo "[build-server] Binary: $(CARGO_TARGET_DIR)/release/sideseat"

# Per-platform targets (generated)
define MAKE_CLI_TARGET
build-cli-$(1): build-web
	@echo "[build-cli] $(1) ($(BUILD_CMD_$(1)))..."
	$$(call run-with-disk-guard,cd $$(SERVER_DIR) && $(BUILD_CMD_$(1)) --locked --release --target $(RUST_TARGET_$(1)))
	@cp "$$(CARGO_TARGET_DIR)/$(RUST_TARGET_$(1))/release/$(BIN_NAME_$(1))" "$$(call cli-bin,$(1))"
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
