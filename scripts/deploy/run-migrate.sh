#!/usr/bin/env bash
# Run `cxp-server migrate` as a one-off machine of the migration app, and fail
# unless it exits 0. The owner's credential (CXP_MIGRATION_DATABASE_URL) is a
# secret of that app alone; the otto-cxp app never holds it (#45).
#
# usage: run-migrate.sh <image> <label>
#   image  the image `fly deploy` is about to ship, e.g. registry.fly.io/otto-cxp:sha-<sha>
#   label  names the machine (migrate-<label>), so a rerun finds its own leftovers
# env:   FLY_API_TOKEN            deploy token of the migration app
#        MIGRATE_APP              default otto-cxp-migrate
#        MIGRATE_TIMEOUT_SECONDS  default 300
#        MIGRATE_POLL_SECONDS     default 5
set -euo pipefail

image="${1:?usage: run-migrate.sh <image> <label>}"
label="${2:?usage: run-migrate.sh <image> <label>}"
app="${MIGRATE_APP:-otto-cxp-migrate}"
timeout="${MIGRATE_TIMEOUT_SECONDS:-300}"
poll="${MIGRATE_POLL_SECONDS:-5}"
name="migrate-${label}"

machine_json() {
  flyctl machines list -a "$app" --json | jq -c --arg n "$name" '[.[] | select(.name == $n)] | first // empty'
}

destroy() {
  local id
  id="$(machine_json 2>/dev/null | jq -r '.id // empty' 2>/dev/null || true)"
  [ -n "$id" ] && flyctl machine destroy "$id" -a "$app" --force || true
}
trap destroy EXIT

# A failed earlier attempt for this label may have left its machine behind.
destroy

# The image's entrypoint is cxp-server, so the command is `migrate`.
# --restart no: a failed migration must stay failed, not retry on its own.
flyctl machine run "$image" migrate \
  -a "$app" --name "$name" --restart no --detach \
  -e CXP_LOG_FORMAT=json \
  --vm-size shared-cpu-1x --vm-memory 256

deadline=$((SECONDS + timeout))
state=""
m=""
while [ "$SECONDS" -lt "$deadline" ]; do
  m="$(machine_json)"
  if [ -z "$m" ]; then
    echo "::error::no machine named $name in $app" >&2
    exit 1
  fi
  state="$(jq -r '.state' <<<"$m")"
  case "$state" in stopped | destroyed) break ;; esac
  sleep "$poll"
done

id="$(jq -r '.id // empty' <<<"$m")"
[ -n "$id" ] && flyctl logs -a "$app" --machine "$id" --no-tail || true

if [ "$state" != stopped ] && [ "$state" != destroyed ]; then
  echo "::error::migration did not finish within ${timeout}s (state: ${state:-unknown}); the deploy is stopped" >&2
  exit 1
fi

# Fly leaves exit_code out of the event when it is 0 (the event then carries only
# "signal": -1), so an exit event with no code and no signal is exit 0. A process
# killed by a signal has no code and a signal other than -1: that stays unrecorded.
code="$(jq -r '[.events[]? | select(.type == "exit") | .request.exit_event
  | (.exit_code // (if (.signal // -1) == -1 then 0 else empty end))] | first // empty' <<<"$m")"
if [ "$code" != 0 ]; then
  echo "::error::cxp-server migrate did not exit 0 (exit code: ${code:-none recorded}); the deploy is stopped" >&2
  exit 1
fi
echo "migrations applied (machine $id exited 0)"
