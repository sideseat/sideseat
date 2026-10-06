##@ Examples and fixtures

.PHONY: sample capture capture-offline matrix matrix-census matrix-check

# The recording proxy signs Bedrock requests, so capture needs the bedrock extra.
HARNESS := uv run --locked --extra bedrock --directory examples/python/harness

# make sample P=strands S=tool_use [SIDESEAT=1] [MODEL=haiku]
# A JavaScript producer is named for its suite directory plus `-js`: P=strands-js runs
# examples/javascript/strands.
SAMPLE_ARGS = $(S) $(if $(SIDESEAT),--sideseat) $(if $(MODEL),--model $(MODEL))
sample: ## Run one example scenario (P=producer S=scenario [SIDESEAT=1] [MODEL=alias])
	@[ -n "$(P)" ] || { echo "usage: make sample P=<producer> S=<scenario> [SIDESEAT=1] [MODEL=alias]"; exit 2; }
	@if [ -f "examples/javascript/$(patsubst %-js,%,$(P))/suite.json" ] && [ "$(P)" != "$(patsubst %-js,%,$(P))" ]; then \
		cd "examples/javascript/$(patsubst %-js,%,$(P))" && npm run --silent sample -- $(SAMPLE_ARGS); \
	else \
		uv run --locked --directory examples/python/$(P) sample $(SAMPLE_ARGS); \
	fi

# Live: records model traffic through the Bedrock proxy, then replays it for the SDK run.
capture: ## Capture golden fixtures live (P=producer [S=scenario] [MODEL=alias])
	@[ -n "$(P)" ] || { echo "usage: make capture P=<producer> [S=scenario]"; exit 2; }
	@$(HARNESS) capture $(P) $(S) $(if $(MODEL),--model $(MODEL))
	@UPDATE_GOLDENS=1 $(CARGO_TEST) -p sideseat-server --test message_goldens -E 'test(=message_goldens)'

capture-offline: ## Re-capture fixtures from committed model cassettes, without credentials
	@[ -n "$(P)" ] || { echo "usage: make capture-offline P=<producer> [S=scenario]"; exit 2; }
	@$(HARNESS) capture $(P) $(S) --offline
	@UPDATE_GOLDENS=1 $(CARGO_TEST) -p sideseat-server --test message_goldens -E 'test(=message_goldens)'

# The version matrix (docs/engineering/framework-versions.md). Opt-in: neither `make quick` nor `make test`
# runs it, because it installs one environment per historical release.
MATRIX := uv run --locked --directory examples/python/harness python -m harness matrix

matrix: ## Replay a suite against its historical releases, offline (P=producer [V=variant] [S=scenario])
	@[ -n "$(P)" ] || { echo "usage: make matrix P=<producer> [V=variant] [S=scenario]"; exit 2; }
	@$(MATRIX) $(P) $(V) $(if $(S),--scenario $(S))
	@UPDATE_GOLDENS=1 $(CARGO_TEST) -p sideseat-server --test message_goldens -E 'test(=message_goldens)'
	@uv run --locked --directory examples/python/harness python -m harness truth $(P)

# ADAPTIVE=1 samples and bisects between classes instead of running every release.
matrix-census: ## Classify a suite's support window by telemetry shape (P=producer [ADAPTIVE=1]; network)
	@[ -n "$(P)" ] || { echo "usage: make matrix-census P=<producer> [ADAPTIVE=1] [RETRY=1]"; exit 2; }
	@$(MATRIX) $(P) --census $(if $(RETRY),--retry) $(if $(ADAPTIVE),--adaptive)

matrix-check: ## Check every suite's census coverage and variant fixtures, offline
	@$(MATRIX) --check
