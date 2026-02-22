# Local Development Stack

Docker Compose stack with PostgreSQL, ClickHouse, Valkey (Redis), and Vault for local development.

All data is stored in `./data/` via bind mounts. Delete it to reset everything.

## Quick Start

```bash
# From misc/docker/local:
docker compose up -d
docker compose ps        # Wait for all 4 services to be healthy

# From the repository root:
make dev-server ARGS="--config misc/docker/local/sideseat.json"
```

## Services

| Service    | Port | Credentials                |
|------------|------|----------------------------|
| PostgreSQL | 5432 | `postgres:postgres`        |
| ClickHouse | 8123 | `default` (no password)    |
| Valkey     | 6379 | password: `dev_password`   |
| Vault      | 8200 | token: `devroot`           |

## Config

`sideseat.json` configures SideSeat to use all compose services:

- **Transactional DB**: PostgreSQL (instead of default SQLite)
- **Analytics DB**: ClickHouse (instead of default DuckDB)
- **Cache**: Valkey/Redis (instead of default in-memory)
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
VAULT_PORT=8201
VAULT_DEV_TOKEN=custom_token
```

## Stop

```bash
docker compose down        # Stop, keep data
docker compose down -v     # Stop, remove Docker resources
rm -rf data                # Delete all persisted data
```
