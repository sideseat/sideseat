---
title: Secret Manager
description: Multi-backend secure storage for credentials, API keys, and secrets with scoping support.
---

The Secret Manager provides secure storage for sensitive data with support for multiple backends and secret scoping (global, organization, project, user).

## Backends

| Backend | CLI Value | Persistence | Notes |
|---------|-----------|-------------|-------|
| **macOS Data Protection Keychain** | `data-protection-keychain` | Persistent | Hardware-backed (Apple Silicon). Requires Developer ID signing. macOS default — auto-falls back to login keychain if unsigned. |
| **macOS Keychain** | `keychain` | Persistent | System keychain integration |
| **Windows Credential Manager** | `credential-manager` | Persistent | Protected by user account |
| **Linux Secret Service** | `secret-service` | Persistent | GNOME Keyring, KWallet, etc. |
| **Linux keyutils** | `keyutils` | Session-only | Kernel keyring, lost on reboot |
| **File** | `file` | Persistent | INSECURE - development only |
| **Environment Variables** | `env` | Read-only | Reads from env vars, cannot write |
| **AWS Secrets Manager** | `aws` | Persistent | Cloud-native, supports rotation |
| **HashiCorp Vault** | `vault` | Persistent | KV v2 engine |

### Platform Auto-Detection

When no backend is specified, the system auto-detects:
- **macOS** → Data Protection Keychain (falls back to login Keychain if binary is unsigned)
- **Windows** → Credential Manager
- **Linux** → Secret Service (falls back to File if D-Bus unavailable)

## Configuration

### CLI / Environment Variable

```bash
sideseat --secrets-backend vault
# or
SIDESEAT_SECRETS_BACKEND=env sideseat
```

### Config File

```json
{
  "secrets": {
    "backend": "vault",
    "vault": {
      "address": "https://vault.internal:8200",
      "mount": "secret",
      "prefix": "sideseat"
    }
  }
}
```

**Priority**: CLI args > env vars > config file > platform auto-detect.

### Environment Variables Backend

Reads secrets from environment variables. Read-only — the server cannot create or update secrets.

```json
{
  "secrets": {
    "backend": "env",
    "env": {
      "prefix": "SIDESEAT_SECRET_"
    }
  }
}
```

Environment variable names are derived from the secret key path:

| Secret Key | Environment Variable |
|-----------|---------------------|
| `global/jwt_signing_key` | `SIDESEAT_SECRET_GLOBAL_JWT_SIGNING_KEY` |
| `org/acme/api_key` | `SIDESEAT_SECRET_ORG_ACME_API_KEY` |
| `project/p1/token` | `SIDESEAT_SECRET_PROJECT_P1_TOKEN` |
| `user/u1/personal_token` | `SIDESEAT_SECRET_USER_U1_PERSONAL_TOKEN` |

Required secrets must be pre-configured as environment variables before starting the server with the `env` backend:

| Environment Variable | Secret Key |
|---------------------|-----------|
| `SIDESEAT_SECRET_GLOBAL_JWT_SIGNING_KEY` | `global/jwt_signing_key` |
| `SIDESEAT_SECRET_GLOBAL_API_KEY_SECRET` | `global/api_key_secret` |

| Env Var | Description |
|---------|-------------|
| `SIDESEAT_SECRETS_ENV_PREFIX` | Override the env var prefix (default: `SIDESEAT_SECRET_`) |

### AWS Secrets Manager Backend

Each secret is stored as an individual AWS Secrets Manager secret.

```json
{
  "secrets": {
    "backend": "aws",
    "aws": {
      "region": "us-east-1",
      "prefix": "sideseat"
    }
  }
}
```

Authentication uses the standard AWS SDK credential chain (env vars, instance profile, SSO).

| Env Var | Description |
|---------|-------------|
| `SIDESEAT_SECRETS_AWS_REGION` | AWS region |
| `SIDESEAT_SECRETS_AWS_PREFIX` | Prefix for secret names (default: `sideseat`) |

### HashiCorp Vault Backend

Uses the KV v2 secrets engine via HTTP API.

```json
{
  "secrets": {
    "backend": "vault",
    "vault": {
      "address": "https://vault.internal:8200",
      "mount": "secret",
      "prefix": "sideseat"
    }
  }
}
```

Authentication uses a Vault token. Priority: `SIDESEAT_SECRETS_VAULT_TOKEN` env var > `VAULT_TOKEN` env var (standard Vault convention) > `secrets.vault.token` in config file.

| Env Var | Description |
|---------|-------------|
| `SIDESEAT_SECRETS_VAULT_ADDR` | Vault server address (required) |
| `VAULT_TOKEN` / `SIDESEAT_SECRETS_VAULT_TOKEN` | Authentication token (required) |
| `SIDESEAT_SECRETS_VAULT_MOUNT` | KV v2 mount path (default: `secret`) |
| `SIDESEAT_SECRETS_VAULT_PREFIX` | Prefix within mount (default: `sideseat`) |

## Secret Scoping

Secrets are organized into four scope levels:

| Scope | Key Format | Example |
|-------|-----------|---------|
| **Global** | `global/{name}` | `global/jwt_signing_key` |
| **Organization** | `org/{org_id}/{name}` | `org/acme/openai_api_key` |
| **Project** | `project/{project_id}/{name}` | `project/p1/webhook_secret` |
| **User** | `user/{user_id}/{name}` | `user/u1/personal_token` |

### Scope Assignments

| Secret | Scope | Rationale |
|--------|-------|-----------|
| `jwt_signing_key` | Global | Infrastructure: one signing key for all JWTs |
| `api_key_secret` | Global | Infrastructure: HMAC key for API key hashing |
| Third-party API keys | Organization | Org isolation: each org has own keys |
| Webhook secrets | Project | Per-project configuration |
| Personal access tokens | User | User-specific credentials |

### Fallback Chain

Secrets can be looked up with a fallback chain across scopes:

```rust
// Try org-scoped first, fall back to global
secrets.get_with_fallback("openai_api_key", &[
    SecretScope::org("acme"),
    SecretScope::global(),
])
```

## Vault Migration

Existing local vault files (v1) use flat keys (`jwt_signing_key`). The new system (v2) uses scoped keys (`global/jwt_signing_key`). Migration happens automatically on startup:

1. Files without a `version` field are treated as v1
2. All flat keys are prefixed with `global/`
3. Version is set to 2 and saved immediately

No manual intervention needed.

## Health Checks

Cloud backends (AWS, Vault) run periodic health checks every 60 seconds. Local and env backends use a no-op health check. Failed health checks are logged as warnings but do not stop the server.

## Security

- Secret values are never logged — only key names appear in logs
- The `Secret` type redacts values in debug output (`[REDACTED]`)
- OS-native backends use platform encryption (Keychain, Credential Manager)
- File backend stores secrets in plain text — use only for development
- Corrupted vault files are backed up and a fresh vault is created
