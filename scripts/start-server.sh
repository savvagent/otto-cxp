#!/usr/bin/env bash
# Run cxp-server locally. cxp-server loads .env itself (dotenvy; it never
# overrides a variable already set). See .env.example.
set -euo pipefail

cd "$(dirname "$0")/.."

exec cargo run -p cxp-server "$@"
