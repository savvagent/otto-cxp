#!/usr/bin/env bash
# Copy the image the deploy is about to ship into the migration app's registry,
# so a machine of otto-cxp-migrate can run it: flyctl looks an image up inside
# the app named by `-a`, and a machine cannot run another app's image (#45).
# The copy is the same image (same digest), pulled with otto-cxp's token and
# pushed with the migration app's own token, so neither token reaches further.
#
# usage: copy-image.sh <image> <migration-image>
#   image            e.g. registry.fly.io/otto-cxp:sha-<sha>
#   migration-image  e.g. registry.fly.io/otto-cxp-migrate:sha-<sha>
# env:   FLY_API_TOKEN          token that may pull <image>
#        FLY_API_TOKEN_MIGRATE  token that may push <migration-image>
#        CONTAINER_CLI          default docker (podman works)
set -euo pipefail

src="${1:?usage: copy-image.sh <image> <migration-image>}"
dst="${2:?usage: copy-image.sh <image> <migration-image>}"
cli="${CONTAINER_CLI:-docker}"
: "${FLY_API_TOKEN:?FLY_API_TOKEN is required}"
: "${FLY_API_TOKEN_MIGRATE:?FLY_API_TOKEN_MIGRATE is required}"

# One registry entry at a time: the second login replaces the first.
printf '%s' "$FLY_API_TOKEN" | "$cli" login registry.fly.io -u x --password-stdin
"$cli" pull "$src"
printf '%s' "$FLY_API_TOKEN_MIGRATE" | "$cli" login registry.fly.io -u x --password-stdin
"$cli" tag "$src" "$dst"
"$cli" push "$dst"
echo "copied $src to $dst"
