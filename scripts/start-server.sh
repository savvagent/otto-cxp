#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

# Load .env if present
if [ -f .env ]; then
  set -a
  source .env
  set +a
fi

# Override transport to HTTP
export TRANSPORT=http
export HTTP_HOST="${HTTP_HOST:-127.0.0.1}"
export HTTP_PORT="${HTTP_PORT:-3000}"
export RUST_LOG="${RUST_LOG:-info}"
export DATABASE_URL="${DATABASE_URL:-postgres://cxp:cxp_dev@localhost:5432/cxp}"

echo "Starting CXP MCP server (HTTP) on http://${HTTP_HOST}:${HTTP_PORT}/mcp"
cargo run --manifest-path server/Cargo.toml
