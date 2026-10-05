# Shared variables, platform matrix, and the disk-guard helper.

# OS/arch detection
UNAME_S := $(shell uname -s 2>/dev/null || echo Windows)

ARGS ?=
TYPE ?= patch
NOTARIZE ?= 0
SERVER_DIR := server
WEB_DIR := web
CLI_DIR := cli
# The .NET SDK pinned in mise.toml, even when the calling shell (a git hook, an IDE) has not activated
# mise: an older system SDK on PATH cannot build the net10.0 targets.
DOTNET ?= $(or $(shell command -v mise >/dev/null 2>&1 && mise which dotnet 2>/dev/null),dotnet)

# Use the repository-pinned formatter; there is no root Node package.
PRETTIER := $(WEB_DIR)/node_modules/.bin/prettier

# Pricing data
PRICES_URL := https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json
PRICES_FILE := $(SERVER_DIR)/assets/pricing/model_prices_and_context_window.json

# Docker
DOCKER_IMAGE := sideseat/core
DOCKER_FILE  := scripts/deploy/Dockerfile

# Homebrew tap
BREW_TAP_REPO ?= sideseat/homebrew-tap

# Release archives
RELEASE_DIR     := release
SIGN_IDENTITY   ?=
NOTARY_PROFILE  ?= sideseat-notarize
SHA256CMD       := $(if $(filter Darwin,$(UNAME_S)),shasum -a 256,sha256sum)

# Local build storage
CARGO_TARGET_DIR ?= target
override CARGO_TARGET_DIR := $(abspath $(CARGO_TARGET_DIR))
export CARGO_TARGET_DIR

# The target size `make disk` reports against. Reclaiming runs, and a build is refused, only when free space
# falls below the reserve: a target over budget on a roomy disk harms nothing, and reclaiming while another
# build runs would delete the incremental state it is using.
DISK_BUDGET_MB   ?= 12000
DISK_FREE_MIN_MB ?= 10000

# Run finite commands that can grow the Cargo target directory with checks before and after them.
define run-with-disk-guard
@$(MAKE) --no-print-directory disk-guard
@command_status=0; guard_status=0; \
	$(1) || command_status=$$?; \
	$(MAKE) --no-print-directory disk-guard || guard_status=$$?; \
	[ "$$command_status" -eq 0 ] || exit "$$command_status"; \
	exit "$$guard_status"
endef

PLATFORMS        := darwin-arm64 darwin-x64 linux-x64 linux-arm64 win32-x64
DARWIN_PLATFORMS := darwin-arm64 darwin-x64

RUST_TARGET_darwin-arm64 := aarch64-apple-darwin
BUILD_CMD_darwin-arm64   := cargo build
BIN_NAME_darwin-arm64    := sideseat

RUST_TARGET_darwin-x64   := x86_64-apple-darwin
BUILD_CMD_darwin-x64     := cargo build
BIN_NAME_darwin-x64      := sideseat

RUST_TARGET_linux-x64    := x86_64-unknown-linux-gnu
BUILD_CMD_linux-x64      := cargo zigbuild
BIN_NAME_linux-x64       := sideseat

RUST_TARGET_linux-arm64  := aarch64-unknown-linux-gnu
BUILD_CMD_linux-arm64    := cargo zigbuild
BIN_NAME_linux-arm64     := sideseat

RUST_TARGET_win32-x64    := x86_64-pc-windows-gnu
BUILD_CMD_win32-x64      := CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=$(CURDIR)/scripts/release/mingw-static-link.sh cargo build
BIN_NAME_win32-x64       := sideseat.exe

# Derived lists
ALL_RUST_TARGETS  := $(foreach p,$(PLATFORMS),$(RUST_TARGET_$(p)))
CLI_BUILD_TARGETS := $(foreach p,$(PLATFORMS),build-cli-$(p))

# Helper: binary path for a platform
cli-bin = $(CLI_DIR)/platforms/platform-$(1)/$(BIN_NAME_$(1))
