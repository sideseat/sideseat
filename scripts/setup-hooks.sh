#!/bin/bash
# Setup git hooks by copying them from hooks/ to .git/hooks/

set -e

HOOKS_DIR="hooks"
GIT_HOOKS_DIR=".git/hooks"

if [ ! -d "$GIT_HOOKS_DIR" ]; then
    echo "Error: .git/hooks directory not found. Are you in the repository root?"
    exit 1
fi

echo "Setting up git hooks..."
echo ""

# Copy all hooks from hooks/ to .git/hooks/
for hook in "$HOOKS_DIR"/*; do
    if [ -f "$hook" ]; then
        hook_name=$(basename "$hook")
        cp "$hook" "$GIT_HOOKS_DIR/$hook_name"
        chmod +x "$GIT_HOOKS_DIR/$hook_name"
        echo "✓ Installed: $hook_name"
    fi
done

echo ""
echo "✓ Git hooks installed successfully!"
echo ""
echo "Installed hooks:"
ls -1 "$GIT_HOOKS_DIR" | grep -v "\.sample$" || echo "  (none)"
