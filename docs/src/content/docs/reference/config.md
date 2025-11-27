---
title: Config Manager
description: Multi-source configuration management with JSON files, environment variables, and CLI arguments.
---

The Config Manager provides a unified configuration system that loads settings from multiple sources with intelligent merging and priority handling.

## Configuration Priority

Settings are loaded in the following order (highest to lowest priority):

1. **Command line arguments** (`--host`, `--port`)
2. **Environment variables** (`SIDESEAT_HOST`, `SIDESEAT_PORT`)
3. **Workdir config** (`./sideseat.json`)
4. **User config** (`~/.sideseat/config.json`)
5. **Default values**

Higher priority sources override lower priority sources. Objects are deep-merged, allowing partial overrides.

## Configuration Files

### User Config (`~/.sideseat/config.json`)

Located in the user config directory. This file is optional and provides user-level defaults.

```json
{
  "server": {
    "host": "127.0.0.1",
    "port": 5001
  },
  "logging": {
    "level": "info",
    "format": "compact"
  }
}
```

### Workdir Config (`./sideseat.json`)

Located in the current working directory. Project-specific settings that override user config.

```json
{
  "server": {
    "port": 3000
  },
  "logging": {
    "level": "debug"
  }
}
```

## Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `SIDESEAT_HOST` | Server host address | `127.0.0.1` |
| `SIDESEAT_PORT` | Server port | `5001` |
| `SIDESEAT_LOG` | Log level/filter | `info` |
| `SIDESEAT_AUTH_ENABLED` | Enable/disable authentication | `true` |
| `SIDESEAT_CONFIG_DIR` | Override config directory | Platform default |
| `SIDESEAT_DATA_DIR` | Override data directory | Platform default |
| `SIDESEAT_CACHE_DIR` | Override cache directory | Platform default |

```bash
# Example: Run on a different port
export SIDESEAT_PORT=8080
sideseat start

# Or pass directly
SIDESEAT_PORT=8080 sideseat start
```

## CLI Arguments

```bash
sideseat start --host 0.0.0.0 --port 3000
sideseat start -H 0.0.0.0 -p 3000  # Short form
sideseat start --no-auth           # Disable authentication
```

CLI arguments have the highest priority and always override other sources.

## Configuration Structure

### Full Config Schema

```json
{
  "server": {
    "host": "127.0.0.1",
    "port": 5001
  },
  "logging": {
    "level": "info",
    "format": "compact"
  },
  "storage": {
    "config_dir": "/custom/config/path",
    "data_dir": "/custom/data/path",
    "cache_dir": "/custom/cache/path"
  },
  "auth": {
    "enabled": true
  },
  "otel": {
    "enabled": true,
    "grpc_enabled": true,
    "grpc_port": 4317,
    "channel_capacity": 1000,
    "buffer_max_spans": 1000,
    "flush_interval_ms": 1000,
    "retention_days": null,
    "retention_max_gb": 20
  }
}
```

### Server Config

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `host` | string | `"127.0.0.1"` | Server bind address |
| `port` | number | `5001` | Server port |

### Logging Config

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `level` | string | `"info"` | Log level (trace, debug, info, warn, error) |
| `format` | string | `"compact"` | Log output format |

### Storage Config

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `config_dir` | string | null | Override config directory path |
| `data_dir` | string | null | Override data directory path |
| `cache_dir` | string | null | Override cache directory path |

### Auth Config

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | boolean | `true` | Enable/disable authentication |

Authentication can be disabled via:
- Config file: `"auth": { "enabled": false }`
- Environment variable: `SIDESEAT_AUTH_ENABLED=false`
- CLI flag: `--no-auth`

### OTel Config

OpenTelemetry collector configuration for trace ingestion and storage.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | boolean | `true` | Enable/disable OTel collector |
| `grpc_enabled` | boolean | `true` | Enable gRPC endpoint (port 4317) |
| `grpc_port` | number | `4317` | gRPC listener port |
| `channel_capacity` | number | `1000` | Bounded channel capacity for ingestion |
| `buffer_max_spans` | number | `1000` | Maximum spans in buffer before flush |
| `buffer_max_bytes` | number | `10485760` | Maximum bytes in buffer (10MB) |
| `flush_interval_ms` | number | `1000` | Flush interval in milliseconds |
| `flush_batch_size` | number | `100` | Batch size for flush operations |
| `max_file_size_mb` | number | `64` | Maximum Parquet file size in MB |
| `row_group_size` | number | `10000` | Rows per row group in Parquet files |
| `retention_days` | number | `null` | Retention days (null = size-based only) |
| `retention_max_gb` | number | `20` | Maximum storage size in GB (FIFO) |
| `retention_check_interval_secs` | number | `300` | Retention check interval (5 min) |
| `disk_warning_percent` | number | `80` | Disk usage warning threshold |
| `disk_critical_percent` | number | `95` | Disk usage critical threshold |
| `max_span_name_len` | number | `1000` | Maximum span name length |
| `max_attribute_count` | number | `100` | Maximum attributes per span |
| `max_attribute_value_len` | number | `10240` | Maximum attribute value length (10KB) |
| `max_events_per_span` | number | `100` | Maximum events per span |
| `sse_max_connections` | number | `100` | Maximum concurrent SSE connections |
| `sse_timeout_secs` | number | `3600` | SSE connection timeout (1 hour) |
| `sse_keepalive_secs` | number | `30` | SSE keepalive interval |

## Deep Merge Behavior

Configuration objects are deep-merged, not replaced. This allows partial overrides:

```json
// User config (~/.sideseat/config.json)
{
  "server": {
    "host": "127.0.0.1",
    "port": 5001
  },
  "logging": {
    "level": "info",
    "format": "compact"
  }
}

// Workdir config (./sideseat.json)
{
  "server": {
    "port": 3000
  },
  "logging": {
    "level": "debug"
  }
}

// Resulting merged config:
{
  "server": {
    "host": "127.0.0.1",  // From user config
    "port": 3000          // From workdir config (higher priority)
  },
  "logging": {
    "level": "debug",     // From workdir config
    "format": "compact"   // From user config
  }
}
```

## API Reference

### CliConfig Struct

```rust
pub struct CliConfig {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub no_auth: bool,
}
```

### Config Struct

```rust
pub struct Config {
    pub server: ServerConfig,
    pub logging: LoggingConfig,
    pub storage: StorageConfig,
    pub auth: AuthConfig,
}

pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

pub struct LoggingConfig {
    pub level: String,
    pub format: String,
}

pub struct StorageConfig {
    pub config_dir: Option<String>,
    pub data_dir: Option<String>,
    pub cache_dir: Option<String>,
}

pub struct AuthConfig {
    pub enabled: bool,
}
```

### ConfigManager Methods

| Method | Description |
|--------|-------------|
| `init(storage, cli_args)` | Initialize config from all sources |
| `config()` | Get reference to merged configuration |
| `sources()` | Get all configuration sources |
| `loaded_sources()` | Get only successfully loaded sources |

## Usage Example

```rust
use sideseat::core::{ConfigManager, CliConfig, StorageManager};

// Initialize storage first
let storage = StorageManager::init().await?;

// Create CLI config from parsed arguments
let cli_config = CliConfig {
    host: Some("0.0.0.0".to_string()),
    port: None,
};

// Initialize config manager
let config_manager = ConfigManager::init(&storage, &cli_config)?;
let config = config_manager.config();

println!("Server: {}:{}", config.server.host, config.server.port);
println!("Log level: {}", config.logging.level);

// List loaded configuration sources
for source in config_manager.loaded_sources() {
    if let Some(ref path) = source.path {
        println!("Loaded from: {}", path.display());
    }
}
```

## Error Handling

The Config Manager returns detailed errors for invalid configuration files:

```
Invalid JSON in '/home/user/.sideseat/config.json' at line 5, column 12: expected `,` or `}`
```

Common error scenarios:
- Invalid JSON syntax (returns error with line/column)
- File read permission denied
- Invalid port number in environment variable (warning logged, value ignored)

Missing configuration files are silently skipped and do not cause errors.

## .env File Support

SideSeat loads environment variables from a `.env` file in the current directory using `dotenvy`. This is processed before configuration initialization.

```bash
# .env
SIDESEAT_HOST=0.0.0.0
SIDESEAT_PORT=8080
SIDESEAT_LOG=debug
```

## Best Practices

1. **Use workdir config for project settings** - Keep project-specific settings in `sideseat.json`
2. **Use user config for personal defaults** - Store your preferred defaults in `~/.sideseat/config.json`
3. **Use environment variables for deployment** - Override settings without modifying files
4. **Use CLI arguments for one-off changes** - Quick overrides without changing any files
5. **Partial configs are fine** - Only specify the settings you want to override
6. **Don't store secrets in config files** - Use the Secret Manager for credentials
