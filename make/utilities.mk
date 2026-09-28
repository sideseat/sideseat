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

# Report local storage and fail when the Cargo cache or free-space reserve is unhealthy.
disk: ## Report and enforce the local disk budget
	@bash scripts/cargo-target-dir.sh >/dev/null
	@echo "[disk] Free space:"
	@df -h . | tail -1
	@echo "[disk] Largest local directories:"
	@du -sh "$(CARGO_TARGET_DIR)" $(WEB_DIR)/node_modules docs/node_modules .sideseat 2>/dev/null | sort -rh || true
	@command -v docker >/dev/null 2>&1 && { echo "[disk] Docker:"; docker system df; } || true
	@# VM images are sparse: du reports host blocks while ls reports virtual capacity.
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
	@command -v colima >/dev/null 2>&1 && [ "$$(docker context show 2>/dev/null)" = "colima" ] && \
		echo "[disk] After pruning Colima, return sparse blocks to macOS: colima ssh -- sudo fstrim -av" || true
	@used=$$(du -sm "$(CARGO_TARGET_DIR)" 2>/dev/null | awk '{print $$1}'); \
	used=$${used:-0}; \
	available=$$(df -Pm . | awk 'NR == 2 {print $$4}'); \
	failed=0; \
	if [ "$$used" -gt "$(DISK_BUDGET_MB)" ]; then \
		echo "[disk] OVER BUDGET: Cargo target is $$used MB against a ceiling of $(DISK_BUDGET_MB) MB"; \
		echo "[disk] Reclaim: make clean-stale (keeps the current build) or make clean (cold rebuild)"; \
		failed=1; \
	else \
		echo "[disk] Cargo target is $$used MB, within the $(DISK_BUDGET_MB) MB budget"; \
	fi; \
	if [ "$$available" -lt "$(DISK_FREE_MIN_MB)" ]; then \
		echo "[disk] LOW SPACE: $$available MB free; reserve is $(DISK_FREE_MIN_MB) MB"; \
		failed=1; \
	else \
		echo "[disk] $$available MB free, above the $(DISK_FREE_MIN_MB) MB reserve"; \
	fi; \
	exit "$$failed"

# Finite Rust build and test recipes run this before and after their main command.
disk-guard:
	@bash scripts/cargo-target-dir.sh >/dev/null
	@used=$$(du -sm "$(CARGO_TARGET_DIR)" 2>/dev/null | awk '{print $$1}'); \
	used=$${used:-0}; \
	available=$$(df -Pm . | awk 'NR == 2 {print $$4}'); \
	if [ "$$used" -gt "$(DISK_BUDGET_MB)" ] || [ "$$available" -lt "$(DISK_FREE_MIN_MB)" ]; then \
		echo "[disk-guard] target=$$used MB, free=$$available MB; reclaiming stale artifacts"; \
		$(MAKE) --no-print-directory clean-stale; \
	fi; \
	used=$$(du -sm "$(CARGO_TARGET_DIR)" 2>/dev/null | awk '{print $$1}'); \
	used=$${used:-0}; \
	available=$$(df -Pm . | awk 'NR == 2 {print $$4}'); \
	if [ "$$used" -gt "$(DISK_BUDGET_MB)" ]; then \
		echo "[disk-guard] Cargo target remains $$used MB; limit is $(DISK_BUDGET_MB) MB"; \
		echo "[disk-guard] Run 'make clean' or raise DISK_BUDGET_MB."; \
		exit 1; \
	fi; \
	if [ "$$available" -lt "$(DISK_FREE_MIN_MB)" ]; then \
		echo "[disk-guard] only $$available MB free; reserve is $(DISK_FREE_MIN_MB) MB"; \
		echo "[disk-guard] Free space before continuing or lower DISK_FREE_MIN_MB."; \
		exit 1; \
	fi

clean: ## Remove all generated build artifacts
	@echo "[clean] Removing build artifacts..."
	@bash scripts/cargo-target-dir.sh >/dev/null
	@cargo clean
	@# The API build script recreates a placeholder web/dist when the real UI is absent.
	@rm -rf $(WEB_DIR)/dist
	@rm -rf $(WEB_DIR)/node_modules/.vite
	@rm -f $(CLI_DIR)/bin/sideseat-*
	@rm -f $(CLI_DIR)/platforms/*/sideseat $(CLI_DIR)/platforms/*/sideseat.exe
	@rm -rf sdk/js/dist
	@rm -rf sdk/python/dist
	@rm -rf $(RELEASE_DIR)
	@echo "[clean] Done. Cargo artifacts and web/dist are gone; the next Rust build is cold."
	@echo "[clean] The API crate recreates web/dist as a placeholder on the next build - run make build-web for the real UI."

# Aliases
run: dev ## Alias for dev
start: dev ## Alias for dev
