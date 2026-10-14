#!/usr/bin/env bash
# Run cxp-server locally, loading .env if present (see .env.example).
set -euo pipefail

cd "$(dirname "$0")/.."

if [ -f .env ]; then
  set -a
  # shellcheck disable=SC1091
  source .env
  set +a
fi

exec cargo run -p cxp-server
