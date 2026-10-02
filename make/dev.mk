##@ Develop

.PHONY: dev dev-server dev-web run start

dev: ## Start server and web development processes
	@./scripts/dev.sh $(ARGS)

dev-server: ## Start the Rust server with reload
	@./scripts/dev-server.sh $(ARGS)

dev-web: ## Start the web development server
	@cd $(WEB_DIR) && npm run dev

run: dev ## Alias for dev
start: dev ## Alias for dev
