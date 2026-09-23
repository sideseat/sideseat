#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

cd "$repo_root"
cargo metadata --locked --no-deps --format-version 1 | node -e '
const fs = require("node:fs");
const metadata = JSON.parse(fs.readFileSync(0, "utf8"));
const server = metadata.packages.find(({ name }) => name === "sideseat-server");

if (!server) {
  console.error("sideseat-server is not a workspace package");
  process.exit(1);
}

process.stdout.write(server.version + "\n");
'
