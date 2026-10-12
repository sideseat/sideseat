#!/bin/sh
# `hot-path` runs the per-stage benchmark, `write-path` the write-path one; anything else is passed to the server.
set -eu
if [ "${1:-}" = "hot-path" ]; then
  shift
  exec /usr/local/bin/ingest-hot-path --ignored --nocapture "$@"
fi
if [ "${1:-}" = "write-path" ]; then
  shift
  exec /usr/local/bin/ingest-write-path --ignored --nocapture "$@"
fi
exec /usr/local/bin/sideseat "$@"
