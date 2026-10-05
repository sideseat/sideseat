##@ Release

.PHONY: version version-check bump sync-version publish publish-cli release sign-release sign-verify sign-notarize build-release publish-release publish-brew

version: ## Show package versions
	@echo "CLI:                $$(node -p "require('./cli/package.json').version")"
	@echo "Server:             $$(./scripts/release/workspace-version.sh)"
	@echo "SDK (JavaScript):   $$(node -p "require('./sdk/js/package.json').version")"
	@echo "SDK (Python):       $$(grep '__version__' sdk/python/src/sideseat/_version.py | sed 's/.*\"\(.*\)\".*/\1/')"
	@echo "SDK (Rust):         $$(sed -n 's/^version = \"\(.*\)\"/\1/p' sdk/rust/Cargo.toml | head -1)"
	@echo "SDK (.NET):         $$(sed -n 's:.*<Version>\(.*\)</Version>.*:\1:p' sdk/dotnet/SideSeat.csproj)"

version-check: ## Verify coordinated package versions
	@CLI_VERSION=$$(node -p "require('./cli/package.json').version") && \
	SERVER_VERSION=$$(./scripts/release/workspace-version.sh) && \
	MISMATCHED="" && \
	if [ "$$CLI_VERSION" != "$$SERVER_VERSION" ]; then \
		MISMATCHED="$$MISMATCHED\n  Rust workspace: $$SERVER_VERSION"; \
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

bump: ## Bump versions with TYPE=patch|minor|major
	@if [ "$(TYPE)" != "patch" ] && [ "$(TYPE)" != "minor" ] && [ "$(TYPE)" != "major" ]; then \
		echo "Error: TYPE must be patch, minor, or major (got: $(TYPE))"; \
		exit 1; \
	fi
	@echo "[bump] Bumping $(TYPE) version..."
	@cd $(CLI_DIR) && npm version $(TYPE) --no-git-tag-version
	@$(MAKE) --no-print-directory sync-version

sync-version: ## Synchronize server and CLI versions
	@NEW_VERSION=$$(node -p "require('./cli/package.json').version") && \
	TEMP_FILE=$$(mktemp) && \
	sed "s/^version = \".*\"/version = \"$$NEW_VERSION\"/" Cargo.toml > "$$TEMP_FILE" && \
	mv "$$TEMP_FILE" Cargo.toml && \
	cargo update --workspace --quiet && \
	CARGO_VERSION=$$(./scripts/release/workspace-version.sh) && \
	if [ "$$NEW_VERSION" != "$$CARGO_VERSION" ]; then \
		echo "Error: Version sync failed. Expected $$NEW_VERSION, got $$CARGO_VERSION"; \
		exit 1; \
	fi && \
	for pkg in $(PLATFORMS); do \
		node -e "const p=require('./cli/platforms/platform-'+'$$pkg'+'/package.json'); p.version='$$NEW_VERSION'; require('fs').writeFileSync('./cli/platforms/platform-'+'$$pkg'+'/package.json', JSON.stringify(p, null, 2)+'\n')"; \
	done && \
	node -e "const p=require('./cli/package.json'); Object.keys(p.optionalDependencies||{}).forEach(k=>p.optionalDependencies[k]='$$NEW_VERSION'); require('fs').writeFileSync('./cli/package.json', JSON.stringify(p, null, 2)+'\n')" && \
	echo "[sync-version] Version synced to $$NEW_VERSION (server + CLI only; SDKs maintained separately)"

# Publish
# =============================================================================

publish: publish-cli publish-sdk-js publish-sdk-python publish-sdk-dotnet publish-docker ## Publish CLI, SDKs, and Docker image

publish-cli: ## Publish CLI platform packages
	@echo "[publish-cli] Verifying npm authentication..."
	@npm whoami >/dev/null 2>&1 || { echo "Error: Not logged in to npm. Run 'npm login' first."; exit 1; }
	@echo "[publish-cli] Verifying binaries exist..."
	@$(foreach p,$(PLATFORMS),[ -f "$(call cli-bin,$(p))" ] || { echo "Error: Missing binary for $(p): $(call cli-bin,$(p)). Run 'make build-cli' first."; exit 1; };)
	@echo "[publish-cli] Verifying macOS code signatures..."
	@$(foreach p,$(DARWIN_PLATFORMS),codesign --verify --strict "$(call cli-bin,$(p))" 2>/dev/null || \
		{ echo "Error: $(call cli-bin,$(p)) is not signed. Run 'make sign-release' first."; exit 1; }; \
		codesign -dvv "$(call cli-bin,$(p))" 2>&1 | grep -q "flags=.*runtime" || \
		{ echo "Error: $(call cli-bin,$(p)) missing Hardened Runtime. Re-sign with --options runtime."; exit 1; }; \
		echo "  $(call cli-bin,$(p)): signed (Hardened Runtime)";)
	@$(MAKE) --no-print-directory version-check
	@echo "[publish-cli] Publishing platform packages..."
	@$(foreach p,$(PLATFORMS),(cd $(CLI_DIR)/platforms/platform-$(p) && npm publish --access public) &&) true
	@VERSION=$$(node -p "require('./$(CLI_DIR)/package.json').version"); \
	echo "[publish-cli] Waiting for platform packages to propagate (v$$VERSION)..."; \
	for p in $(PLATFORMS); do \
		attempt=1; \
		while [ $$attempt -le 60 ]; do \
			if npm view "@sideseat/platform-$$p@$$VERSION" version >/dev/null 2>&1; then \
				echo "  @sideseat/platform-$$p@$$VERSION available"; \
				break; \
			fi; \
			echo "  Waiting for @sideseat/platform-$$p@$$VERSION (attempt $$attempt/60)..."; \
			sleep 5; \
			attempt=$$((attempt + 1)); \
		done; \
		if [ $$attempt -gt 60 ]; then \
			echo "Error: @sideseat/platform-$$p@$$VERSION not available"; \
			exit 1; \
		fi; \
	done
	@echo "[publish-cli] Waiting 5 min for CDN propagation..."
	@sleep 300
	@echo "[publish-cli] Publishing main sideseat package..."
	@cd $(CLI_DIR) && npm publish --access public
	@VERSION=$$(node -p "require('./$(CLI_DIR)/package.json').version"); \
	echo "[publish-cli] Verifying sideseat@$$VERSION on registry..."; \
	attempt=1; \
	while [ $$attempt -le 30 ]; do \
		if npm view "sideseat@$$VERSION" version >/dev/null 2>&1; then \
			echo "[publish-cli] Published and verified sideseat@$$VERSION"; \
			exit 0; \
		fi; \
		echo "  Waiting for sideseat@$$VERSION (attempt $$attempt/30)..."; \
		sleep 5; \
		attempt=$$((attempt + 1)); \
	done; \
	echo "Warning: sideseat@$$VERSION published but not yet verified on registry"

release: ## Check, bump, commit, tag, and atomically push
	@./scripts/release/release.sh "$(TYPE)"

# Production signing identity is supplied through the environment or a make argument.
sign-release: ## Sign macOS platform binaries with Developer ID
	@[ "$(UNAME_S)" = "Darwin" ] || { echo "Error: code signing requires macOS"; exit 1; }
	@[ -n "$(SIGN_IDENTITY)" ] || { echo "Error: SIGN_IDENTITY required. Usage: make sign-release SIGN_IDENTITY=\"Developer ID Application: Name (TEAMID)\""; exit 1; }
	@for bin in $(foreach p,$(DARWIN_PLATFORMS),$(call cli-bin,$(p))); do \
		[ -f "$$bin" ] || { echo "Error: missing $$bin. Run 'make build-cli' first."; exit 1; }; \
		codesign --force --options runtime --sign "$(SIGN_IDENTITY)" --entitlements scripts/release/packaging/macos/entitlements.plist "$$bin" || \
			{ echo "Error: failed to sign $$bin"; exit 1; }; \
		echo "[sign-release] Signed $$bin"; \
	done

sign-verify: ## Verify code signature and entitlements on macOS platform binaries
	@[ "$(UNAME_S)" = "Darwin" ] || { echo "Error: signature verification requires macOS"; exit 1; }
	@for bin in $(foreach p,$(DARWIN_PLATFORMS),$(call cli-bin,$(p))); do \
		[ -f "$$bin" ] || { echo "Error: missing $$bin. Run 'make build-cli' first."; exit 1; }; \
		codesign --verify --strict "$$bin" || { echo "Error: invalid signature on $$bin"; exit 1; }; \
		echo "=== $$bin ==="; \
		echo "--- Signature ---"; \
		codesign -dvv "$$bin" || exit 1; \
		echo ""; \
		echo "--- Entitlements ---"; \
		codesign -d --entitlements :- "$$bin" || exit 1; \
		echo ""; \
	done

sign-notarize: ## Notarize macOS archives; ZIP files cannot be stapled
	@[ "$$(uname -s)" = "Darwin" ] || { echo "Error: notarization requires macOS"; exit 1; } && \
	VERSION=$$(node -p "require('./cli/package.json').version") && \
	OUTDIR="$(RELEASE_DIR)/v$$VERSION" && \
	[ -d "$$OUTDIR" ] || { echo "Error: $$OUTDIR not found. Run 'make build-release' first."; exit 1; } && \
	echo "[sign-notarize] Notarizing darwin archives for v$$VERSION..." && \
	for plat in $(DARWIN_PLATFORMS); do \
		ARCHIVE="sideseat-$$VERSION-$$plat.zip" && \
		[ -f "$$OUTDIR/$$ARCHIVE" ] || { echo "Error: $$OUTDIR/$$ARCHIVE not found"; exit 1; } && \
		echo "  Submitting $$ARCHIVE..." && \
		xcrun notarytool submit "$$OUTDIR/$$ARCHIVE" \
			--keychain-profile "$(NOTARY_PROFILE)" --wait --timeout 48h || \
			{ echo "Error: Notarization failed for $$plat"; exit 1; } && \
		echo "  $$plat: notarized"; \
	done && \
	echo "[sign-notarize] Done (stapling skipped -- not supported for ZIP/CLI; Gatekeeper checks online)"

build-release: ## Create release archives and checksums
	@VERSION=$$(node -p "require('./cli/package.json').version") && \
	OUTDIR="$(RELEASE_DIR)/v$$VERSION" && \
	echo "[build-release] Building release archives for v$$VERSION..." && \
	echo "[build-release] Verifying binaries exist..." && \
	$(foreach p,$(PLATFORMS),[ -f "$(call cli-bin,$(p))" ] || \
		{ echo "Error: Missing binary for $(p): $(call cli-bin,$(p)). Run 'make build-cli' first."; exit 1; } &&) \
	echo "[build-release] Verifying darwin code signatures..." && \
	$(foreach p,$(DARWIN_PLATFORMS),codesign --verify --strict "$(call cli-bin,$(p))" 2>/dev/null || \
		{ echo "Error: $(call cli-bin,$(p)) is not signed. Run 'make sign-release' first."; exit 1; } &&) \
	rm -rf "$$OUTDIR" && mkdir -p "$$OUTDIR" && \
	for plat in $(PLATFORMS); do \
		case $$plat in \
			darwin-*|win32-*) EXT=zip ;; \
			*)                EXT=tar.gz ;; \
		esac && \
		ARCHIVE="sideseat-$$VERSION-$$plat.$$EXT" && \
		case $$plat in \
			win32-*) BINFILE=sideseat.exe ;; \
			*)       BINFILE=sideseat ;; \
		esac && \
		TMPDIR=$$(mktemp -d) && \
		cp "$(CLI_DIR)/platforms/platform-$$plat/$$BINFILE" "$$TMPDIR/$$BINFILE" && \
		cp LICENSE "$$TMPDIR/LICENSE" && \
		if [ "$$EXT" = "zip" ]; then \
			(cd "$$TMPDIR" && zip -q "$$ARCHIVE" "$$BINFILE" LICENSE) && \
			mv "$$TMPDIR/$$ARCHIVE" "$$OUTDIR/$$ARCHIVE"; \
		else \
			tar czf "$$OUTDIR/$$ARCHIVE" -C "$$TMPDIR" "$$BINFILE" LICENSE; \
		fi && \
		rm -rf "$$TMPDIR" && \
		echo "  $$ARCHIVE"; \
	done && \
	if [ "$(NOTARIZE)" = "1" ]; then \
		if [ "$$(uname -s)" = "Darwin" ]; then \
			$(MAKE) sign-notarize; \
		else \
			echo "[build-release] WARNING: Not on macOS -- cannot notarize"; \
		fi; \
	else \
		echo "[build-release] Skipping notarization (use NOTARIZE=1 to enable)"; \
	fi && \
	echo "[build-release] Generating checksums..." && \
	(cd "$$OUTDIR" && $(SHA256CMD) sideseat-* > checksums-sha256.txt) && \
	echo "[build-release] Done: $$OUTDIR/"

publish-release: ## Upload archives to the GitHub release
	@VERSION=$$(node -p "require('./cli/package.json').version") && \
	OUTDIR="$(RELEASE_DIR)/v$$VERSION" && \
	echo "[publish-release] Publishing v$$VERSION to GitHub Releases..." && \
	[ -d "$$OUTDIR" ] || { echo "Error: $$OUTDIR not found. Run 'make build-release' first."; exit 1; } && \
	echo "[publish-release] Verifying checksums..." && \
	(cd "$$OUTDIR" && $(SHA256CMD) -c checksums-sha256.txt) || \
		{ echo "Error: Checksum verification failed"; exit 1; } && \
	TAG="v$$VERSION" && \
	TAG_COMMIT=$$(git rev-parse --verify "$$TAG^{commit}" 2>/dev/null) || \
		{ echo "Error: tag v$$VERSION is missing. Run 'make release TYPE=...' first."; exit 1; } && \
	HEAD_COMMIT=$$(git rev-parse HEAD) && \
	[ "$$TAG_COMMIT" = "$$HEAD_COMMIT" ] || \
		{ echo "Error: $$TAG points to $$TAG_COMMIT, but HEAD is $$HEAD_COMMIT. Build and publish from the release tag."; exit 1; } && \
	git ls-remote --exit-code --tags origin "refs/tags/$$TAG" >/dev/null 2>&1 || \
		{ echo "Error: $$TAG is not present on origin. Push it through 'make release TYPE=...'."; exit 1; } && \
	echo "[publish-release] Creating GitHub release..." && \
	gh release create "$$TAG" "$$OUTDIR"/* --generate-notes --title "$$TAG" && \
	echo "[publish-release] Done: https://github.com/$$(gh repo view --json nameWithOwner -q .nameWithOwner)/releases/tag/v$$VERSION" && \
	echo "[publish-release] Next: make publish-brew"

publish-brew: ## Update the Homebrew tap
	@VERSION=$$(node -p "require('./cli/package.json').version") && \
	CHECKSUMS="$(RELEASE_DIR)/v$$VERSION/checksums-sha256.txt" && \
	echo "[publish-brew] Publishing Homebrew formula for v$$VERSION..." && \
	[ -f "$$CHECKSUMS" ] || \
		{ echo "Error: $$CHECKSUMS not found. Run 'make build-release' first."; exit 1; } && \
	gh release view "v$$VERSION" >/dev/null 2>&1 || \
		{ echo "Error: GitHub Release v$$VERSION not found. Run 'make publish-release' first."; exit 1; } && \
	SHA_DARWIN_ARM64=$$(grep -F 'darwin-arm64' "$$CHECKSUMS" | awk '{print $$1}') && \
	SHA_DARWIN_X64=$$(grep -F 'darwin-x64' "$$CHECKSUMS" | awk '{print $$1}') && \
	SHA_LINUX_X64=$$(grep -F 'linux-x64' "$$CHECKSUMS" | awk '{print $$1}') && \
	SHA_LINUX_ARM64=$$(grep -F 'linux-arm64' "$$CHECKSUMS" | awk '{print $$1}') && \
	for hash in $$SHA_DARWIN_ARM64 $$SHA_DARWIN_X64 $$SHA_LINUX_X64 $$SHA_LINUX_ARM64; do \
		echo "$$hash" | grep -qE '^[0-9a-f]{64}$$' || \
			{ echo "Error: Invalid SHA256 hash: $$hash"; exit 1; }; \
	done && \
	FORMULA=$$(mktemp) && \
	sed -e "s/__VERSION__/$$VERSION/g" \
		-e "s/__SHA256_DARWIN_ARM64__/$$SHA_DARWIN_ARM64/g" \
		-e "s/__SHA256_DARWIN_X64__/$$SHA_DARWIN_X64/g" \
		-e "s/__SHA256_LINUX_X64__/$$SHA_LINUX_X64/g" \
		-e "s/__SHA256_LINUX_ARM64__/$$SHA_LINUX_ARM64/g" \
		scripts/release/packaging/homebrew/sideseat.rb.tmpl > "$$FORMULA" && \
	grep -q '__' "$$FORMULA" && \
		{ echo "Error: Unreplaced placeholders in generated formula"; rm -f "$$FORMULA"; exit 1; } || true && \
	ENCODED=$$(base64 < "$$FORMULA" | tr -d '\n') && \
	EXISTING_SHA=$$(gh api "repos/$(BREW_TAP_REPO)/contents/Formula/sideseat.rb" --jq '.sha' 2>/dev/null || echo "") && \
	if [ -n "$$EXISTING_SHA" ]; then \
		gh api --method PUT "repos/$(BREW_TAP_REPO)/contents/Formula/sideseat.rb" \
			-f message="Update sideseat to v$$VERSION" \
			-f content="$$ENCODED" \
			-f sha="$$EXISTING_SHA" \
			--silent; \
	else \
		gh api --method PUT "repos/$(BREW_TAP_REPO)/contents/Formula/sideseat.rb" \
			-f message="Add sideseat v$$VERSION" \
			-f content="$$ENCODED" \
			--silent; \
	fi && \
	rm -f "$$FORMULA" && \
	echo "[publish-brew] Formula pushed to $(BREW_TAP_REPO)" && \
	echo "[publish-brew] Install: brew tap sideseat/tap && brew install sideseat"
