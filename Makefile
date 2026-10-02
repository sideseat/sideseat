# SideSeat repository automation. `make help` lists every command; the recipes live in make/*.mk.
#
# Three loops, from fastest to broadest:
#   make quick   changed areas only: format, lint, and unit tests (the inner loop, under a minute)
#   make check   every container-free gate: format, lint, and all regular tests
#   make test-*  opt-in suites that start containers, release binaries, or benchmarks

SHELL := /bin/bash
.DELETE_ON_ERROR:
.DEFAULT_GOAL := help

include make/config.mk

##@ Everyday

.PHONY: help quick check

help: ## Show available commands
	@awk -f scripts/make-help.awk $(MAKEFILE_LIST)
	@printf "\nDefaults: TYPE=%s  NOTARIZE=%s\n" "$(TYPE)" "$(NOTARIZE)"

quick: ## Format, lint, and test only what changed since main
	@./scripts/quick.sh $(ARGS)

check: fmt-check lint test ## Run every container-free gate
	@echo "[check] All checks passed"

include make/setup.mk
include make/dev.mk
include make/quality.mk
include make/test.mk
include make/sdk.mk
include make/build.mk
include make/docs.mk
include make/release.mk
include make/utilities.mk
