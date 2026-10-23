#!/usr/bin/env bash
# Tests scripts/deploy/run-migrate.sh against a fake flyctl on PATH (no network,
# no Fly): it must pass on exit 0 and fail on a nonzero exit, a missing exit,
# a timeout, or no machine; and it must always destroy the machine it made.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir "$tmp/bin"

# FAKE_SCENARIO: ok | fail | no-exit | hang | no-machine | run-fails
cat > "$tmp/bin/flyctl" <<'FAKE'
#!/usr/bin/env bash
echo "flyctl $*" >> "$FAKE_LOG"
case "$1 $2" in
  "machine run")
    [ "$FAKE_SCENARIO" = run-fails ] && { echo "run failed" >&2; exit 1; }
    exit 0 ;;
  "machines list")
    [ "$FAKE_SCENARIO" = no-machine ] && { echo '[]'; exit 0; }
    case "$FAKE_SCENARIO" in
      ok)      state=stopped; ev='[{"type":"exit","request":{"exit_event":{"exit_code":0}}},{"type":"start"}]' ;;
      fail)    state=stopped; ev='[{"type":"exit","request":{"exit_event":{"exit_code":1}}},{"type":"start"}]' ;;
      no-exit) state=stopped; ev='[{"type":"start"}]' ;;
      hang)    state=started; ev='[{"type":"start"}]' ;;
    esac
    printf '[{"id":"m123","name":"%s","state":"%s","events":%s}]\n' "$FAKE_NAME" "$state" "$ev"
    exit 0 ;;
  "machine destroy") exit 0 ;;
  "logs "*|"logs -a") echo "log line"; exit 0 ;;
esac
exit 0
FAKE
chmod +x "$tmp/bin/flyctl"

fails=0
run() { # scenario, expected exit (0 or nonzero), label
  local scenario="$1" want="$2" label="$3"
  : > "$tmp/log"
  local got=0
  PATH="$tmp/bin:$PATH" FAKE_LOG="$tmp/log" FAKE_SCENARIO="$scenario" \
    FAKE_NAME="migrate-abc123" MIGRATE_TIMEOUT_SECONDS=2 MIGRATE_POLL_SECONDS=1 \
    "$here/run-migrate.sh" registry.fly.io/otto-cxp:sha-abc123 abc123 \
    > "$tmp/out" 2>&1 || got=$?
  if { [ "$want" = 0 ] && [ "$got" != 0 ]; } || { [ "$want" != 0 ] && [ "$got" = 0 ]; }; then
    echo "FAIL $label: exit $got"; cat "$tmp/out"; fails=$((fails + 1)); return
  fi
  echo "ok   $label"
}

has_log() { grep -q -- "$1" "$tmp/log" || { echo "FAIL: log lacks: $1"; cat "$tmp/log"; fails=$((fails + 1)); }; }

run ok 0 "exit 0 passes"
has_log "machine run registry.fly.io/otto-cxp:sha-abc123 migrate"
has_log "-a otto-cxp-migrate"
has_log "--restart no"
has_log "machine destroy"
run fail 1 "nonzero exit fails"
has_log "machine destroy"
run no-exit 1 "stopped without an exit event fails"
run hang 1 "a machine that never stops times out"
has_log "machine destroy"
run no-machine 1 "no machine fails"
run run-fails 1 "a failed machine run fails"

[ "$fails" = 0 ] || exit 1
echo "all passed"
