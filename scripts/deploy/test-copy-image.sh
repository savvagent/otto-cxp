#!/usr/bin/env bash
# Tests scripts/deploy/copy-image.sh against a fake container CLI (no network):
# it pulls with the source token, pushes with the migration token, and never
# pushes if the pull fails.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir "$tmp/bin"

cat > "$tmp/bin/docker" <<'FAKE'
#!/usr/bin/env bash
case "$1" in
  login) echo "login $(cat)" >> "$FAKE_LOG" ;;
  pull)  echo "pull $2" >> "$FAKE_LOG"
         if [ "${FAKE_PULL_FAILS:-}" = 1 ]; then exit 1; fi
         if [ "${FAKE_PULL_FAILS_ONCE:-}" = 1 ] && [ ! -e "$FAKE_LOG.once" ]; then touch "$FAKE_LOG.once"; exit 1; fi ;;
  *)     echo "$*" >> "$FAKE_LOG" ;;
esac
FAKE
chmod +x "$tmp/bin/docker"

export PATH="$tmp/bin:$PATH" FAKE_LOG="$tmp/log"
export FLY_API_TOKEN=src-token FLY_API_TOKEN_MIGRATE=dst-token
export COPY_PULL_TRIES=3 COPY_PULL_DELAY_SECONDS=0

: > "$FAKE_LOG"
"$here/copy-image.sh" registry.fly.io/otto-cxp:a registry.fly.io/otto-cxp-migrate:a > /dev/null
expected=$'login src-token\npull registry.fly.io/otto-cxp:a\nlogin dst-token\ntag registry.fly.io/otto-cxp:a registry.fly.io/otto-cxp-migrate:a\npush registry.fly.io/otto-cxp-migrate:a'
[ "$(cat "$FAKE_LOG")" = "$expected" ] || { echo "unexpected calls:"; cat "$FAKE_LOG"; exit 1; }

: > "$FAKE_LOG"
if FAKE_PULL_FAILS=1 "$here/copy-image.sh" registry.fly.io/otto-cxp:a registry.fly.io/otto-cxp-migrate:a > /dev/null 2>&1; then
  echo "a failed pull must fail the copy"; exit 1
fi
! grep -q '^push' "$FAKE_LOG" || { echo "pushed after a failed pull"; exit 1; }

# A pull that fails once and then works is retried, and the copy goes on.
: > "$FAKE_LOG"; rm -f "$FAKE_LOG.once"
FAKE_PULL_FAILS_ONCE=1 "$here/copy-image.sh" registry.fly.io/otto-cxp:a registry.fly.io/otto-cxp-migrate:a > /dev/null 2>&1
[ "$(grep -c '^pull' "$FAKE_LOG")" = 2 ] || { echo "expected one retry"; cat "$FAKE_LOG"; exit 1; }
grep -q '^push registry.fly.io/otto-cxp-migrate:a' "$FAKE_LOG" || { echo "no push after the retry"; exit 1; }

echo "copy-image: ok"
