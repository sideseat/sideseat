#!/usr/bin/env bash
set -euo pipefail

scenario="${1:-}"
case "$scenario" in
  clickhouse | clickhouse-replicated | clickhouse-two-shard | postgres | redis | redpanda) ;;
  *)
    echo "Usage: $0 {clickhouse|clickhouse-replicated|clickhouse-two-shard|postgres|redis|redpanda}" >&2
    exit 2
    ;;
esac

command -v docker >/dev/null 2>&1 || {
  echo "[$scenario] docker is required" >&2
  exit 1
}
docker info >/dev/null 2>&1 || {
  echo "[$scenario] Docker daemon is unavailable" >&2
  exit 1
}

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
containers=()
networks=()

cleanup() {
  local exit_code=$?
  local cleanup_failed=0
  local index
  local name

  trap - EXIT INT TERM
  if ! docker info >/dev/null 2>&1; then
    echo "[$scenario] cleanup failed: Docker daemon is unavailable" >&2
    cleanup_failed=1
  else
    for ((index = ${#containers[@]} - 1; index >= 0; index--)); do
      name="${containers[index]}"
      if docker container inspect "$name" >/dev/null 2>&1; then
        if ! docker rm -fv "$name" >/dev/null; then
          echo "[$scenario] cleanup failed for container $name" >&2
          cleanup_failed=1
        fi
      fi
    done
    for ((index = ${#networks[@]} - 1; index >= 0; index--)); do
      name="${networks[index]}"
      if docker network inspect "$name" >/dev/null 2>&1; then
        if ! docker network rm "$name" >/dev/null; then
          echo "[$scenario] cleanup failed for network $name" >&2
          cleanup_failed=1
        fi
      fi
    done
  fi

  if ((exit_code == 0 && cleanup_failed != 0)); then
    exit_code=1
  fi
  exit "$exit_code"
}

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

require_env() {
  local name="$1"
  if [[ -z "${!name:-}" ]]; then
    echo "[$scenario] missing required environment variable: $name" >&2
    exit 2
  fi
}

prepare_container() {
  local name="$1"
  containers+=("$name")
  if docker container inspect "$name" >/dev/null 2>&1; then
    docker rm -fv "$name" >/dev/null
  fi
}

prepare_network() {
  local name="$1"
  networks+=("$name")
  if docker network inspect "$name" >/dev/null 2>&1; then
    docker network rm "$name" >/dev/null
  fi
}

wait_for_http() {
  local url="$1"
  local attempts="$2"
  local container="$3"
  local log_lines="$4"
  local attempt

  for ((attempt = 1; attempt <= attempts; attempt++)); do
    if curl --fail --silent --max-time 2 "$url" >/dev/null; then
      return
    fi
    sleep 1
  done

  echo "[$scenario] service did not become ready at $url" >&2
  docker logs --tail "$log_lines" "$container" >&2 || true
  return 1
}

cd "$repo_root"

case "$scenario" in
  clickhouse)
    require_env CH_TEST_CONTAINER
    require_env CH_TEST_PORT
    require_env CH_TEST_IMAGE
    prepare_container "$CH_TEST_CONTAINER"

    echo "[test-clickhouse] starting $CH_TEST_IMAGE on port $CH_TEST_PORT..."
    docker run -d --name "$CH_TEST_CONTAINER" -p "$CH_TEST_PORT:8123" \
      -e CLICKHOUSE_USER=sideseat -e CLICKHOUSE_PASSWORD=sideseat \
      -e CLICKHOUSE_DEFAULT_ACCESS_MANAGEMENT=1 "$CH_TEST_IMAGE" >/dev/null
    wait_for_http "http://127.0.0.1:$CH_TEST_PORT/ping" 60 "$CH_TEST_CONTAINER" 20

    SIDESEAT_TEST_CLICKHOUSE_URL="http://127.0.0.1:$CH_TEST_PORT" \
      SIDESEAT_TEST_CLICKHOUSE_USER=sideseat \
      SIDESEAT_TEST_CLICKHOUSE_PASSWORD=sideseat \
      cargo test --locked -p sideseat-server --test clickhouse_parity -- --test-threads=1
    ;;

  clickhouse-replicated)
    require_env CH_REPL_CONTAINER
    require_env CH_REPL_PORT
    require_env CH_TEST_IMAGE
    prepare_container "$CH_REPL_CONTAINER"

    echo "[test-clickhouse-replicated] starting $CH_TEST_IMAGE on port $CH_REPL_PORT..."
    docker run -d --name "$CH_REPL_CONTAINER" -p "$CH_REPL_PORT:8123" \
      -e CLICKHOUSE_USER=sideseat -e CLICKHOUSE_PASSWORD=sideseat \
      -e CLICKHOUSE_DEFAULT_ACCESS_MANAGEMENT=1 \
      -v "$repo_root/scripts/clickhouse-replicated/cluster.xml:/etc/clickhouse-server/config.d/cluster.xml:ro" \
      "$CH_TEST_IMAGE" >/dev/null
    wait_for_http "http://127.0.0.1:$CH_REPL_PORT/ping" 90 "$CH_REPL_CONTAINER" 30

    SIDESEAT_TEST_CLICKHOUSE_REPLICATED_URL="http://127.0.0.1:$CH_REPL_PORT" \
      SIDESEAT_TEST_CLICKHOUSE_USER=sideseat \
      SIDESEAT_TEST_CLICKHOUSE_PASSWORD=sideseat \
      cargo test --locked -p sideseat-server --test clickhouse_parity replicated -- --test-threads=1
    ;;

  clickhouse-two-shard)
    for name in CH_NET CH_SHARD_1_CONTAINER CH_SHARD_2_CONTAINER CH_SHARD_PORT_1 CH_SHARD_PORT_2 CH_TEST_IMAGE; do
      require_env "$name"
    done
    prepare_container "$CH_SHARD_1_CONTAINER"
    prepare_container "$CH_SHARD_2_CONTAINER"
    prepare_network "$CH_NET"

    echo "[two-shard] starting two $CH_TEST_IMAGE nodes; expect about 15 minutes"
    docker network create "$CH_NET" >/dev/null
    for node in 1 2; do
      if [[ "$node" == 1 ]]; then
        port="$CH_SHARD_PORT_1"
        container="$CH_SHARD_1_CONTAINER"
      else
        port="$CH_SHARD_PORT_2"
        container="$CH_SHARD_2_CONTAINER"
      fi

      docker_args=(run -d --name "$container" --hostname "ch-shard$node" \
        --network "$CH_NET" --network-alias "ch-shard$node" -p "$port:8123" \
        -e CLICKHOUSE_USER=sideseat -e CLICKHOUSE_PASSWORD=sideseat \
        -e CLICKHOUSE_DEFAULT_ACCESS_MANAGEMENT=1 \
        -v "$repo_root/scripts/clickhouse-replicated/two-shard-common.xml:/etc/clickhouse-server/config.d/cluster.xml:ro" \
        -v "$repo_root/scripts/clickhouse-replicated/two-shard-node-$node.xml:/etc/clickhouse-server/config.d/node.xml:ro")
      if [[ "$node" == 1 ]]; then
        docker_args+=(
          -v "$repo_root/scripts/clickhouse-replicated/two-shard-keeper.xml:/etc/clickhouse-server/config.d/keeper.xml:ro"
        )
      fi
      docker_args+=("$CH_TEST_IMAGE")
      docker "${docker_args[@]}" >/dev/null
    done
    wait_for_http "http://127.0.0.1:$CH_SHARD_PORT_1/ping" 90 "$CH_SHARD_1_CONTAINER" 30
    wait_for_http "http://127.0.0.1:$CH_SHARD_PORT_2/ping" 90 "$CH_SHARD_2_CONTAINER" 30

    SIDESEAT_TEST_CLICKHOUSE_TWO_SHARD_URL="http://127.0.0.1:$CH_SHARD_PORT_1" \
      SIDESEAT_TEST_CLICKHOUSE_USER=sideseat \
      SIDESEAT_TEST_CLICKHOUSE_PASSWORD=sideseat \
      cargo test --locked -p sideseat-server --test clickhouse_parity two_shard -- --test-threads=1 --nocapture
    ;;

  postgres)
    for name in PG_TEST_CONTAINER PG_TEST_PORT PG_TEST_IMAGE; do
      require_env "$name"
    done
    prepare_container "$PG_TEST_CONTAINER"

    echo "[test-postgres] starting $PG_TEST_IMAGE on port $PG_TEST_PORT..."
    docker run -d --name "$PG_TEST_CONTAINER" -p "$PG_TEST_PORT:5432" \
      -e POSTGRES_USER=sideseat -e POSTGRES_PASSWORD=sideseat -e POSTGRES_DB=sideseat \
      "$PG_TEST_IMAGE" >/dev/null
    ready=false
    for ((attempt = 1; attempt <= 60; attempt++)); do
      if docker exec "$PG_TEST_CONTAINER" psql -U sideseat -d sideseat -Atqc "SELECT 1" >/dev/null 2>&1; then
        ready=true
        break
      fi
      sleep 1
    done
    if [[ "$ready" == false ]]; then
      echo "[test-postgres] server did not become ready" >&2
      docker logs --tail 20 "$PG_TEST_CONTAINER" >&2 || true
      exit 1
    fi

    SIDESEAT_TEST_POSTGRES_URL="postgres://sideseat:sideseat@127.0.0.1:$PG_TEST_PORT/sideseat" \
      cargo test --locked -p sideseat-server --test postgres_parity -- --test-threads=1
    ;;

  redis)
    for name in REDIS_TEST_CONTAINER REDIS_TEST_PORT REDIS_TEST_IMAGE; do
      require_env "$name"
    done
    prepare_container "$REDIS_TEST_CONTAINER"

    echo "[test-redis] starting $REDIS_TEST_IMAGE on port $REDIS_TEST_PORT..."
    docker run -d --name "$REDIS_TEST_CONTAINER" -p "$REDIS_TEST_PORT:6379" \
      "$REDIS_TEST_IMAGE" redis-server \
      --appendonly yes --appendfsync always --maxmemory-policy noeviction >/dev/null
    ready=false
    for ((attempt = 1; attempt <= 60; attempt++)); do
      if docker exec "$REDIS_TEST_CONTAINER" redis-cli ping 2>/dev/null | grep -q PONG; then
        ready=true
        break
      fi
      sleep 1
    done
    if [[ "$ready" == false ]]; then
      echo "[test-redis] server did not become ready" >&2
      docker logs --tail 20 "$REDIS_TEST_CONTAINER" >&2 || true
      exit 1
    fi

    SIDESEAT_TEST_REDIS_URL="redis://127.0.0.1:$REDIS_TEST_PORT" \
      cargo test --locked -p sideseat-adapter-topics redis_stream_tests -- --test-threads=1
    ;;

  redpanda)
    for name in REDPANDA_TEST_CONTAINER REDPANDA_TEST_PORT REDPANDA_TEST_IMAGE; do
      require_env "$name"
    done
    prepare_container "$REDPANDA_TEST_CONTAINER"

    echo "[test-redpanda] starting $REDPANDA_TEST_IMAGE on port $REDPANDA_TEST_PORT..."
    docker run -d --name "$REDPANDA_TEST_CONTAINER" -p "$REDPANDA_TEST_PORT:9092" \
      "$REDPANDA_TEST_IMAGE" redpanda start \
      --overprovisioned --smp 1 --memory 1G --reserve-memory 0M \
      --node-id 0 --check=false --kafka-addr 0.0.0.0:9092 \
      --advertise-kafka-addr "127.0.0.1:$REDPANDA_TEST_PORT" >/dev/null
    ready=false
    for ((attempt = 1; attempt <= 60; attempt++)); do
      if docker exec "$REDPANDA_TEST_CONTAINER" rpk cluster health --exit-when-healthy >/dev/null 2>&1; then
        ready=true
        break
      fi
      sleep 1
    done
    if [[ "$ready" == false ]]; then
      echo "[test-redpanda] broker did not become ready" >&2
      docker logs --tail 40 "$REDPANDA_TEST_CONTAINER" >&2 || true
      exit 1
    fi

    SIDESEAT_TEST_REDPANDA_BROKERS="127.0.0.1:$REDPANDA_TEST_PORT" \
      cargo test --locked -p sideseat-adapter-topics redpanda_tests -- --test-threads=1 --nocapture
    ;;
esac
