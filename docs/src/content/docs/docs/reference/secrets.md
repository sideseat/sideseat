---
title: Secret Manager
description: Cross-platform secure storage for credentials, API keys, and secrets using OS-native credential stores.
---

The Secret Manager provides secure storage for sensitive data using native credential stores on each platform. All secrets are stored in a single keychain entry (vault) to minimize permission prompts—one "Always Allow" grants access to all secrets.

## Platform Support

| Platform | Backend | Persistence | Notes |
|----------|---------|-------------|-------|
| **macOS** | Keychain | Persistent | Integrated with system security |
| **Windows** | Credential Manager | Persistent | Protected by user account |
| **Linux** | Secret Service | Persistent | GNOME Keyring, KWallet, etc. |
| **Linux** | keyutils (fallback) | Session-only | Used when Secret Service unavailable |

### Linux Notes

On Linux, the Secret Manager first attempts to use the Secret Service API (D-Bus based). This works with:
- GNOME Keyring
- KWallet
- Other Secret Service implementations

If Secret Service is unavailable (e.g., headless servers, minimal containers), it falls back to Linux `keyutils`. **Important**: keyutils stores secrets in the kernel keyring which is session-scoped—secrets do not persist across reboots.

## Environment Variables

| Variable | Description |
|----------|-------------|
| `SIDESEAT_SECRET_BACKEND` | Force a specific backend (see below) |

Valid backend values:
- `keychain` - Force macOS Keychain
- `credential-manager` - Force Windows Credential Manager
- `secret-service` - Force Linux Secret Service
- `keyutils` - Force Linux keyutils

## Secret Structure

Secrets are stored with metadata for tracking and management:

```rust
pub struct Secret {
    pub value: String,           // The actual secret
    pub metadata: SecretMetadata,
}

pub struct SecretMetadata {
    pub provider: Option<String>,     // e.g., "openai", "anthropic"
    pub scope: Option<String>,        // e.g., "api", "oauth"
    pub expires_at: Option<DateTime>, // Optional expiration
    pub created_at: DateTime,
    pub updated_at: DateTime,
}
```

## API Reference

### Initialization

```rust
use sideseat_server::core::SecretManager;

// Initialize (auto-detects platform backend)
let secrets = SecretManager::init().await?;

// Check which backend is active
println!("Using: {}", secrets.backend().name());

// Check if storage persists across reboots
if !secrets.is_persistent() {
    println!("Warning: Secrets will not persist after reboot");
}
```

### Storing Secrets

```rust
use sideseat_server::core::{SecretManager, Secret, SecretKey, SecretMetadata};
use chrono::Utc;

let secrets = SecretManager::init().await?;

// Simple API key storage
secrets.set_api_key("OPENAI_API_KEY", "sk-xxx...", Some("openai")).await?;

// Full secret with metadata
let secret = Secret {
    value: "github_pat_xxx...".to_string(),
    metadata: SecretMetadata {
        provider: Some("github".to_string()),
        scope: Some("repo,read:user".to_string()),
        expires_at: Some(Utc::now() + chrono::Duration::days(90)),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    },
};

let key = SecretKey::new("GITHUB_TOKEN");
secrets.set(&key, &secret).await?;
```

### Retrieving Secrets

```rust
// Get just the value
if let Some(api_key) = secrets.get_value("OPENAI_API_KEY").await? {
    println!("Got API key");
}

// Get full secret with metadata
let key = SecretKey::new("GITHUB_TOKEN");
if let Some(secret) = secrets.get(&key).await? {
    println!("Provider: {:?}", secret.metadata.provider);
    println!("Created: {}", secret.metadata.created_at);
}
```

### Updating Secrets

```rust
// Update value, preserving metadata
secrets.update_value("OPENAI_API_KEY", "sk-new-key...").await?;

// Or replace entirely
let new_secret = Secret::with_provider("new-token", "github");
secrets.set(&SecretKey::new("GITHUB_TOKEN"), &new_secret).await?;
```

### Deleting Secrets

```rust
secrets.delete(&SecretKey::new("OLD_API_KEY")).await?;
```

### Checking Existence

```rust
if secrets.exists(&SecretKey::new("OPENAI_API_KEY")).await? {
    println!("API key is configured");
}
```

## Secret Keys

Secrets are identified by a name and optional target:

```rust
// Simple key
let key = SecretKey::new("MY_API_KEY");

// Key with target (for disambiguation)
let key = SecretKey::with_target("API_KEY", "production");
```

The target is useful when you have multiple secrets with the same name for different environments or purposes.

## Expiration Handling

Secrets with an `expires_at` timestamp are automatically checked when retrieved:

```rust
let mut metadata = SecretMetadata::new();
metadata.expires_at = Some(Utc::now() + chrono::Duration::hours(1));

let secret = Secret {
    value: "temporary-token".to_string(),
    metadata,
};

secrets.set(&SecretKey::new("TEMP_TOKEN"), &secret).await?;

// Later, if expired, get() returns an error
match secrets.get(&SecretKey::new("TEMP_TOKEN")).await {
    Err(e) => println!("Token expired: {}", e),
    Ok(Some(s)) => println!("Token valid"),
    Ok(None) => println!("Token not found"),
}
```

## Vault Architecture

All secrets are stored in a single keychain entry called "vault". This design provides:

- **Single permission prompt** - One "Always Allow" grants access to all secrets
- **In-memory caching** - Vault loaded once at startup, reads are instant
- **Atomic updates** - All secrets saved together when any secret changes

### Keychain Access Pattern

| Operation | Keychain Access |
|-----------|-----------------|
| `SecretManager::init()` | 1 READ (loads vault) |
| `get()` / `get_value()` | 0 (in-memory cache) |
| `set()` / `set_api_key()` | 1 WRITE (saves vault) |
| `exists()` | 0 (in-memory cache) |

### macOS Keychain Prompts

On macOS, you'll see a keychain prompt on first access. Click **"Always Allow"** to grant permanent access. If prompted for both read and write, allow both for uninterrupted access.

## Security Considerations

1. **Secrets are never logged** - Only key names appear in logs, never values
2. **OS-level encryption** - All backends use platform-native encryption
3. **Memory safety** - Consider using `zeroize` for sensitive in-memory data
4. **No file fallback** - Secrets are never stored in plain text files
5. **Session warning** - A warning is logged when using non-persistent backends
6. **Vault consolidation** - All secrets in one entry reduces attack surface

## Error Handling

The Secret Manager returns `Error::Secret` for all secret-related errors:

```rust
use sideseat_server::Error;

match secrets.get_value("API_KEY").await {
    Ok(Some(value)) => { /* use value */ }
    Ok(None) => println!("Secret not found"),
    Err(Error::Secret(msg)) => println!("Secret error: {}", msg),
    Err(e) => println!("Other error: {}", e),
}
```

Common error scenarios:
- Secret not found (returns `Ok(None)` for get, error for delete/update)
- Secret expired (returns error)
- Backend unavailable (returns error during init or operations)
- Serialization failure (corrupted secret data)

## Best Practices

1. **Initialize once** - Create one `SecretManager` and share it
2. **Check persistence** - Warn users if using non-persistent backend
3. **Use providers** - Tag secrets with provider names for organization
4. **Set expiration** - Use `expires_at` for temporary tokens
5. **Handle missing secrets** - Always check for `None` returns
6. **Don't store in config** - Use Secret Manager instead of config files for credentials
