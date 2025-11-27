#!/bin/bash
# Run the OTEL E2E test suite

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

cd "$SCRIPT_DIR"

echo "Running OTEL E2E Test Suite..."
uv run test
