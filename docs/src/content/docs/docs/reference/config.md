---
title: Config Manager
description: How SideSeat loads configuration from files, env vars, and CLI flags.
---

SideSeat merges configuration from multiple sources so you can keep local defaults and override when needed.

## Configuration Priority

Settings are loaded in this order (highest priority first):

1. **CLI flags** (e.g., `--host`, `--port`)
2. **Environment variables** (e.g., `SIDESEAT_HOST`, `SIDESEAT_PORT`)
3. **Project config** (`./sideseat.json` or `--config <path>`)
4. **Profile config** (`~/.sideseat/sideseat.json`)
5. **Built-in defaults**

Objects are deep-merged, so you can override only the fields you need.

## Config Files

SideSeat looks for JSON config files in two places:

- **Profile config**: `~/.sideseat/sideseat.json`
- **Project config**: `./sideseat.json` (or `--config <path>`)

Example project config:

```json
{
  "server": {
    "host": "127.0.0.1",
    "port": 5388
  },
  "auth": {
    "enabled": true
  },
  "otel": {
    "grpc": {
      "enabled": true,
      "port": 4317
    },
    "retention": {
      "max_age_minutes": 10080,
      "max_spans": 5000000
    },
    "auth": {
      "required": false
    }
  },
  "pricing": {
    "sync_hours": 24
  },
  "files": {
    "enabled": true,
    "storage": "filesystem",
    "quota_bytes": 1073741824,
    "filesystem": {
      "path": "/custom/files"
    }
  },
  "rate_limit": {
    "enabled": false,
    "per_ip": false,
    "api_rpm": 1000,
    "ingestion_rpm": 10000,
    "auth_rpm": 100,
    "files_rpm": 1000,
    "bypass_header": "X-SideSeat-Bypass"
  },
  "database": {
    "transactional": "sqlite",
    "analytics": "duckdb",
    "cache": "memory"
  },
  "update": {
    "enabled": true
  },
  "debug": false
}
```

## Environment Variables

Common environment variables:

| Variable | Description |
|----------|-------------|
| `SIDESEAT_HOST` | Server host address |
| `SIDESEAT_PORT` | Server port (default `5388`) |
| `SIDESEAT_LOG` | Log level/filter |
| `SIDESEAT_DEBUG` | Enable debug mode (`true`/`false`) |
| `SIDESEAT_CONFIG` | Path to a config file |
| `SIDESEAT_OTEL_GRPC_ENABLED` | Enable OTLP gRPC ingestion |
| `SIDESEAT_OTEL_GRPC_PORT` | OTLP gRPC port (default `4317`) |
| `SIDESEAT_OTEL_RETENTION_MAX_AGE_MINUTES` | Retention max age (minutes) |
| `SIDESEAT_OTEL_RETENTION_MAX_SPANS` | Retention max spans |
| `SIDESEAT_OTEL_AUTH_REQUIRED` | Require auth for OTLP ingestion |
| `SIDESEAT_PRICING_SYNC_HOURS` | Pricing sync interval |
| `SIDESEAT_NO_UPDATE_CHECK` | Disable update checks |
| `SIDESEAT_DATA_DIR` | Override data directory |

For the full list of CLI flags and env vars, see the [CLI Reference](/docs/reference/cli/).

## Config Sections

### Server

| Field | Type | Description |
|-------|------|-------------|
| `host` | string | Server bind address |
| `port` | number | Server port |

### Auth

| Field | Type | Description |
|-------|------|-------------|
| `enabled` | boolean | Enable/disable authentication |

### OpenTelemetry (`otel`)

| Field | Type | Description |
|-------|------|-------------|
| `grpc.enabled` | boolean | Enable OTLP gRPC ingestion |
| `grpc.port` | number | OTLP gRPC port |
| `retention.max_age_minutes` | number | Retention max age in minutes (null = no limit) |
| `retention.max_spans` | number | Retention max spans (null = no limit) |
| `auth.required` | boolean | Require auth for OTLP ingestion |

### Pricing

| Field | Type | Description |
|-------|------|-------------|
| `sync_hours` | number | Pricing data sync interval (hours) |

### Files

| Field | Type | Description |
|-------|------|-------------|
| `enabled` | boolean | Enable file storage |
| `storage` | string | `filesystem` or `s3` |
| `quota_bytes` | number | Storage quota per project |
| `filesystem.path` | string | Filesystem storage path |
| `s3.bucket` | string | S3 bucket name |
| `s3.prefix` | string | S3 prefix/key path |
| `s3.region` | string | S3 region |
| `s3.endpoint` | string | Custom S3 endpoint |

### Rate Limiting

| Field | Type | Description |
|-------|------|-------------|
| `enabled` | boolean | Enable rate limiting |
| `per_ip` | boolean | Enable per-IP limits |
| `api_rpm` | number | API requests per minute |
| `ingestion_rpm` | number | OTLP ingestion requests per minute |
| `auth_rpm` | number | Auth requests per minute |
| `files_rpm` | number | File upload requests per minute |
| `bypass_header` | string | Header name to bypass rate limits |

### Database

| Field | Type | Description |
|-------|------|-------------|
| `transactional` | string | `sqlite` (default) or `postgres` |
| `analytics` | string | `duckdb` (default) or `clickhouse` |
| `cache` | string | `memory` (default) or `redis` |
| `postgres.url` | string | PostgreSQL connection URL |
| `postgres.max_connections` | number | Max connections |
| `postgres.min_connections` | number | Min connections |
| `postgres.acquire_timeout_secs` | number | Acquire timeout (seconds) |
| `postgres.idle_timeout_secs` | number | Idle timeout (seconds) |
| `postgres.max_lifetime_secs` | number | Max lifetime (seconds) |
| `postgres.statement_timeout_secs` | number | Statement timeout (seconds) |
| `clickhouse.url` | string | ClickHouse connection URL |
| `clickhouse.database` | string | Database name |
| `clickhouse.user` | string | Username |
| `clickhouse.password` | string | Password |
| `clickhouse.timeout_secs` | number | Query timeout (seconds) |
| `clickhouse.compression` | boolean | Enable compression |
| `clickhouse.async_insert` | boolean | Enable async inserts |
| `clickhouse.wait_for_async_insert` | boolean | Wait for insert completion |
| `clickhouse.cluster` | string | Cluster name for sharding |
| `clickhouse.distributed` | boolean | Enable distributed tables |
| `redis.url` | string | Redis connection URL |
| `memory_cache.max_entries` | number | Max in-memory cache entries |
| `memory_cache.eviction_policy` | string | `tinylfu` or `lru` |

### Update

| Field | Type | Description |
|-------|------|-------------|
| `enabled` | boolean | Enable update checks |

### Debug

| Field | Type | Description |
|-------|------|-------------|
| `debug` | boolean | Enable debug mode |
