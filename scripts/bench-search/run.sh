#!/usr/bin/env bash
# Time customer and knowledge-base search before and after #17 against the
# database scripts/bench-search/seed.sql filled, exactly as a request runs
# them: one client, prepared statements (as sqlx sends them, so the plan
# cache moves to a generic plan after five runs), as otto_app pinned to the
# big org. Prints p50/p95/max of whole-transaction latency in ms, after six
# warm-up runs. after-*.sql come from gen.py (the backlog batch, the search,
# and the page's rows, as cxp_core runs them).
#
#   scripts/bench-search/run.sh postgres://cxp:cxp@localhost:<port>/bench [runs]
#
# With CONTAINER=<podman container>, pgbench runs inside that container over
# its unix socket instead (the first argument is then just the database
# name), with the server's own pgbench version. Use it when the host's
# pgbench is older than the server or a rootless port forward sits between
# them: host pgbench 16 against Postgres 18 behind podman's pasta added a
# steady ~200 ms to every multi-kilobyte result (not server time).
#
# LOCAL POSTGRES ONLY.
set -euo pipefail

url=$1
runs=${2:-40}
here=$(cd "$(dirname "$0")" && pwd)
org=00000000-0000-0000-0000-00000000000a
logs=$(mktemp -d)
trap 'rm -rf "$logs"' EXIT

customers=(navy grace hopper "grace hopper" "Grace Hoper" "catherine.straski1@initech.example" zzzz lee ab)
articles=("password reset" account aulicism arbuscula arrhythmic antiprimer "Angiasthenia Aleurones" acc zebrafishy)

# `q` as `ILIKE '%q%'` with q's own \ % _ escaped (cxp_core::contains_pattern).
pattern() {
  local q=$1 out='%' c i
  for ((i = 0; i < ${#q}; i++)); do
    c=${q:i:1}
    case $c in '\' | '%' | '_') out+="\\$c" ;; *) out+=$c ;; esac
  done
  printf '%s%%' "$out"
}

if [ -n "${CONTAINER:-}" ]; then
  for f in "$here"/*.sql; do podman cp "$f" "$CONTAINER:/tmp/$(basename "$f")"; done
fi

bench() {
  local script=$1 q=$2 n=$3 dir
  dir=$(mktemp -d "$logs/run.XXXX")
  if [ -n "${CONTAINER:-}" ]; then
    podman exec "$CONTAINER" bash -c 'rm -rf /tmp/bench-lat && mkdir /tmp/bench-lat'
    podman exec -w /tmp/bench-lat -e PGOPTIONS="-c max_parallel_workers_per_gather=0" "$CONTAINER" \
      pgbench -U cxp -n -M prepared -c 1 -t $((n + 6)) -f "/tmp/$script" -D org="$org" -D q="$q" \
      -D pat="$(pattern "$q")" --log --log-prefix=lat "$url" >/dev/null
    podman exec "$CONTAINER" bash -c 'cat /tmp/bench-lat/lat*' >"$dir/lat"
  else
    (cd "$dir" && PGOPTIONS="-c max_parallel_workers_per_gather=0" pgbench -n -M prepared -c 1 \
      -t $((n + 6)) -f "$here/$script" -D org="$org" -D q="$q" -D pat="$(pattern "$q")" \
      --log --log-prefix=lat "$url" >/dev/null)
  fi
  cat "$dir"/lat* | tail -n +7 | awk '{print $3 / 1000}' | sort -n |
    awk -v s="${script%.sql}" -v q="$q" '{a[NR] = $1} END {
      printf "%-18s %-38s p50=%7.1f p95=%7.1f max=%7.1f ms\n", s, "\"" q "\"",
        a[int(NR * 0.5 + 0.5)], a[int(NR * 0.95 + 0.5)], a[NR] }'
}

for q in "${customers[@]}"; do bench after-customers.sql "$q" "$runs"; done
for q in "${articles[@]}"; do bench after-articles.sql "$q" "$runs"; done
# The scans take about a second each: fewer runs.
for q in "${customers[@]}"; do bench before-customers.sql "$q" 10; done
for q in "${articles[@]}"; do bench before-articles.sql "$q" 10; done
