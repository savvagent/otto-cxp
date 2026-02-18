#!/usr/bin/env bash
set -euo pipefail

CONTAINER_NAME="cxp-postgres"
IMAGE="docker.io/pgvector/pgvector:pg17"

# Check if container already exists
if podman container exists "$CONTAINER_NAME" 2>/dev/null; then
  STATE=$(podman inspect --format '{{.State.Status}}' "$CONTAINER_NAME")
  if [ "$STATE" = "running" ]; then
    echo "PostgreSQL is already running."
    exit 0
  fi
  echo "Starting existing container..."
  podman start "$CONTAINER_NAME"
else
  echo "Creating PostgreSQL (pgvector) container..."
  podman run -d \
    --name "$CONTAINER_NAME" \
    -e POSTGRES_USER=cxp \
    -e POSTGRES_PASSWORD=cxp_dev \
    -e POSTGRES_DB=cxp \
    -p 5432:5432 \
    -v cxp-pgdata:/var/lib/postgresql/data \
    "$IMAGE"
fi

# Wait for PostgreSQL to be ready
echo "Waiting for PostgreSQL to accept connections..."
for i in $(seq 1 30); do
  if podman exec "$CONTAINER_NAME" pg_isready -U cxp &>/dev/null; then
    echo "PostgreSQL is ready."
    exit 0
  fi
  sleep 1
done

echo "Error: PostgreSQL did not become ready in time" >&2
exit 1
