# Local Development Stack

Docker Compose server-mode stack with SideSeat, PostgreSQL, ClickHouse, Valkey, RedPanda, and Vault.

The distributed analytics profile requires ClickHouse 26.4 or newer; the Compose stack pins the
exact patch used by CI and the parity suite.

All data is stored in `./data/` via bind mounts. Delete it to reset everything.

## Quick Start

```bash
# From deploy/local:
docker compose up -d
docker compose ps        # Wait for all 6 services to be healthy
```

## Services

| Service    | Port | Credentials                |
|------------|------|----------------------------|
| PostgreSQL | 5432 | `postgres:postgres`        |
| ClickHouse | 8123 | `default` (no password)    |
| Valkey     | 6379 | password: `dev_password`   |
| RedPanda   | 19092 | no authentication          |
| Vault      | 8200 | token: `devroot`           |
| SideSeat   | 5388 / 4317 | auth disabled       |

## Config

`sideseat.json` configures SideSeat to use all compose services:

- **Transactional DB**: PostgreSQL (instead of default SQLite)
- **Analytics DB**: ClickHouse (instead of default DuckDB)
- **Cache**: Valkey/Redis (instead of default in-memory)
- **Queue**: RedPanda (independently from the Redis cache)
- **Secrets**: Vault with persistent file storage (instead of default platform keychain)

Auth is disabled for convenience. Enable it by setting `"auth": {"enabled": true}`.

Vault auto-initializes and auto-unseals on container start. Data persists in `data/vault/`.

## Environment Overrides

Ports and credentials can be overridden via environment variables or a `.env` file:

```bash
POSTGRES_PORT=5433
POSTGRES_PASSWORD=custom_pass
CLICKHOUSE_HTTP_PORT=8124
VALKEY_PORT=6380
VALKEY_PASSWORD=custom_pass
REDPANDA_PORT=29092
VAULT_PORT=8201
VAULT_DEV_TOKEN=custom_token
```

## Stop

```bash
docker compose down        # Stop, keep data
docker compose down -v     # Stop, remove Docker resources
rm -rf data                # Delete all persisted data
```
