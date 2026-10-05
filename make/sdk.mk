##@ SDKs

.PHONY: build-sdk build-sdk-js build-sdk-python build-sdk-rust build-sdk-dotnet capture-sdk-conformance-dotnet capture-sdk-conformance-python capture-sdk-conformance-javascript capture-sdk-conformance-rust publish-sdk-js publish-sdk-python publish-sdk-dotnet

build-sdk: build-sdk-js build-sdk-python build-sdk-rust build-sdk-dotnet ## Build implemented SDKs

build-sdk-js: ## Build the JavaScript SDK
	@echo "[build-sdk-js] Building JS SDK..."
	@cd sdk/js && npm run build

build-sdk-python: ## Build the Python SDK
	@echo "[build-sdk-python] Building Python SDK..."
	@cd sdk/python && uv build

build-sdk-rust: ## Build the Rust SDK
	@echo "[build-sdk-rust] Building Rust SDK..."
	$(call run-with-disk-guard,cargo build --locked -p sideseat)

build-sdk-dotnet: ## Build the .NET SDK NuGet package
	@echo "[build-sdk-dotnet] Building .NET SDK package..."
	@$(DOTNET) restore sdk/dotnet/SideSeat.csproj --locked-mode
	@$(DOTNET) pack sdk/dotnet/SideSeat.csproj --configuration Release --no-restore

capture-sdk-conformance-dotnet: ## Capture .NET SDK-on and raw-OTel message fixtures
	@DOTNET_COMMAND="$(DOTNET)" ./scripts/fixtures/capture-dotnet-conformance.sh

capture-sdk-conformance-python: ## Capture Python SDK-on and raw-OTel message fixtures
	@UV_COMMAND=uv ./scripts/fixtures/capture-python-conformance.sh

capture-sdk-conformance-javascript: ## Capture JavaScript SDK-on and raw-OTel message fixtures
	@NPM_COMMAND=npm ./scripts/fixtures/capture-javascript-conformance.sh

capture-sdk-conformance-rust: ## Capture Rust SDK-on and raw-OTel message fixtures
	@CARGO_COMMAND=cargo ./scripts/fixtures/capture-rust-conformance.sh

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

publish-sdk-dotnet: ## Publish the .NET SDK to NuGet
	@test -n "$$NUGET_API_KEY" || { echo "Error: NUGET_API_KEY is required"; exit 1; }
	@$(MAKE) --no-print-directory build-sdk-dotnet
	@VERSION=$$(sed -n 's:.*<Version>\(.*\)</Version>.*:\1:p' sdk/dotnet/SideSeat.csproj); \
	PACKAGE="sdk/dotnet/bin/Release/SideSeat.$$VERSION.nupkg"; \
	test -f "$$PACKAGE" || { echo "Error: package not found: $$PACKAGE"; exit 1; }; \
	$(DOTNET) nuget push "$$PACKAGE" \
		--api-key "$$NUGET_API_KEY" \
		--source "https://api.nuget.org/v3/index.json"
	@echo "[publish-sdk-dotnet] Published $$(sed -n 's:.*<Version>\(.*\)</Version>.*:\1:p' sdk/dotnet/SideSeat.csproj)"
