#!/bin/sh
# `hot-path` runs the per-stage benchmark; anything else is passed to the server.
set -eu
if [ "${1:-}" = "hot-path" ]; then
  shift
  exec /usr/local/bin/ingest-hot-path --ignored --nocapture "$@"
fi
exec /usr/local/bin/sideseat "$@"
