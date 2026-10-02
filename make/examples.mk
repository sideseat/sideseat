##@ Examples and fixtures

.PHONY: sample capture capture-offline

HARNESS := uv run --locked --directory examples/python/harness

# make sample P=strands S=tool_use [SIDESEAT=1] [MODEL=haiku]
sample: ## Run one example scenario (P=producer S=scenario [SIDESEAT=1] [MODEL=alias])
	@[ -n "$(P)" ] || { echo "usage: make sample P=<producer> S=<scenario> [SIDESEAT=1] [MODEL=alias]"; exit 2; }
	@uv run --locked --directory examples/python/$(P) sample $(S) $(if $(SIDESEAT),--sideseat) $(if $(MODEL),--model $(MODEL))

# Live: records model traffic through the Bedrock proxy, then replays it for the SDK run.
capture: ## Capture golden fixtures live (P=producer [S=scenario] [MODEL=alias])
	@[ -n "$(P)" ] || { echo "usage: make capture P=<producer> [S=scenario]"; exit 2; }
	@$(HARNESS) capture $(P) $(S) $(if $(MODEL),--model $(MODEL))
	@UPDATE_GOLDENS=1 $(CARGO_TEST) -p sideseat-server --test message_goldens -E 'test(=message_goldens)'

capture-offline: ## Re-capture fixtures from committed model cassettes, without credentials
	@[ -n "$(P)" ] || { echo "usage: make capture-offline P=<producer> [S=scenario]"; exit 2; }
	@$(HARNESS) capture $(P) $(S) --offline
	@UPDATE_GOLDENS=1 $(CARGO_TEST) -p sideseat-server --test message_goldens -E 'test(=message_goldens)'
