#!/bin/sh
# Auto-initializing and auto-unsealing Vault for local dev.
# Creates a static token (default: "devroot") for convenience.

INIT_FILE="/vault/data/init.json"
STATIC_TOKEN="${VAULT_DEV_TOKEN:-devroot}"
export VAULT_ADDR="http://127.0.0.1:8200"

# Start Vault server in background
vault server -config=/vault/config/vault.hcl &
VAULT_PID=$!

# Wait for Vault listener (exit code 2 = sealed but reachable)
echo "Waiting for Vault to start..."
READY=0
for i in $(seq 1 30); do
  vault status >/dev/null 2>&1
  rc=$?
  if [ $rc -eq 0 ] || [ $rc -eq 2 ]; then
    READY=1
    break
  fi
  sleep 0.5
done

if [ $READY -eq 0 ]; then
  echo "ERROR: Vault failed to start within 15 seconds" >&2
  kill $VAULT_PID 2>/dev/null
  exit 1
fi

# Initialize on first run
if [ ! -f "$INIT_FILE" ]; then
  echo "Initializing Vault (first run)..."
  if ! vault operator init -key-shares=1 -key-threshold=1 -format=json > "${INIT_FILE}.tmp"; then
    echo "ERROR: Vault initialization failed" >&2
    rm -f "${INIT_FILE}.tmp"
    kill $VAULT_PID 2>/dev/null
    exit 1
  fi
  mv "${INIT_FILE}.tmp" "$INIT_FILE"
fi

# Parse unseal key and original root token
FLAT=$(tr -d '\n ' < "$INIT_FILE")
UNSEAL_KEY=$(echo "$FLAT" | sed 's/.*"unseal_keys_b64":\["\([^"]*\)".*/\1/')
ORIG_TOKEN=$(echo "$FLAT" | sed 's/.*"root_token":"\([^"]*\)".*/\1/')

# Unseal if sealed
vault status >/dev/null 2>&1
if [ $? -eq 2 ]; then
  echo "Unsealing Vault..."
  vault operator unseal "$UNSEAL_KEY" >/dev/null
fi

# Create static token if it doesn't exist yet (first run after init)
export VAULT_TOKEN="$ORIG_TOKEN"
if ! vault token lookup "$STATIC_TOKEN" >/dev/null 2>&1; then
  echo "Creating static token..."
  vault token create -id="$STATIC_TOKEN" -policy=root -no-default-policy -orphan -period=0 >/dev/null
fi

export VAULT_TOKEN="$STATIC_TOKEN"

# Enable KV v2 at secret/ if not already present
if ! vault secrets list -format=json 2>/dev/null | grep -q '"secret/"'; then
  echo "Enabling KV v2 secrets engine..."
  vault secrets enable -path=secret kv-v2 2>/dev/null || true
fi

echo "Vault ready (token: $STATIC_TOKEN)"

# Foreground: wait for Vault process
wait $VAULT_PID
