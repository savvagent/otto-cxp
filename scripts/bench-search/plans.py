# Check that search's generic plans stay per candidate (#41) against a
# database scripts/bench-search/seed.sql filled (use `-v size=mid` for the
# mid-size org, where generic plans once read the whole org). For each
# query it runs the search statement exactly as after-*.sql does (gen.py's
# output: as otto_app, the org pinned, JIT off) with plan_cache_mode =
# force_generic_plan under EXPLAIN (ANALYZE, FORMAT JSON), and prints the
# execution time and either "per-candidate" or what read a whole org:
#
# - an executed hashed subplan that reads customers, customer_search_grams,
#   knowledge_articles or knowledge_search_terms, or
# - inside the customer statement's `matches`, an executed hash or merge
#   join, or an executed read of customers by anything but an index scan.
#
# Only executed nodes count (EXPLAIN ANALYZE's `Actual Loops`); the plan
# shape test, crates/cxp-core/tests/search_plans.rs, flags planned ones too.
#
# Exits 1 if any query fails the check.
#
#   python3 scripts/bench-search/plans.py postgres://cxp:cxp@localhost:<port>/bench
#
# The queries are narrow ones, the case #41 is about; `ab` (run.sh's broad
# query) is left out because sub_new, its newest-first read, still reads the
# whole org in a generic plan (see SEARCH_CUSTOMERS_SQL's doc comment), so
# "per-candidate" here says nothing about it. Parallel workers are off, so
# times are not comparable with run.sh's. The statement is the `WITH ` line
# of each after-*.sql, as gen.py writes it.
#
# LOCAL POSTGRES ONLY.
import json, os, re, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))
URL = sys.argv[1]
ORG = '00000000-0000-0000-0000-00000000000a'
TABLES = {'customers', 'customer_search_grams', 'knowledge_articles', 'knowledge_search_terms'}
CUSTOMERS = ['grace', 'hoppr', 'grace hopper', 'catherine.straski1@initech.example',
             'user1234@', 'navy', 'lee', 'zzzz']
ARTICLES = ['password reset', 'account', 'brou', 'brougham', 'acc', 'aulicism',
            'Angiasthenia Aleurones', 'zebrafishy']


def pattern(q):
    """`q` as ILIKE '%q%' with q's own \\ % _ escaped (cxp_core::contains_pattern)."""
    return '%' + ''.join('\\' + c if c in '\\%_' else c for c in q) + '%'


def lit(s):
    return "'" + s.replace("'", "''") + "'"


def statement(script, subs):
    """The search statement of a generated after-*.sql, with its pgbench
    variables turned back into $n."""
    text = open(os.path.join(HERE, script)).read()
    sql = next(l for l in text.splitlines() if l.startswith('WITH '))
    sql = sql[: sql.rindex('\\gset')].strip()
    for var, n in subs:
        sql = sql.replace(var, n)
    return sql


def nodes(n):
    yield n
    for c in n.get('Plans', []):
        yield from nodes(c)


def problems(root, customer):
    found = []
    by_name = {n.get('Subplan Name'): n for n in nodes(root)}
    for n in nodes(root):
        for v in n.values():
            if isinstance(v, str):
                for name in re.findall(r'hashed (SubPlan \d+)', v):
                    if name not in by_name:
                        found.append(f"hashed {name} is not in the plan")
                        continue
                    for r in nodes(by_name[name]):
                        if r.get('Relation Name') in TABLES and r.get('Actual Loops', 0) > 0:
                            found.append(f"hashed {name} reads {r['Relation Name']}")
    if customer:
        for n in nodes(by_name['CTE matches']):
            if n['Node Type'] in ('Hash Join', 'Merge Join') and n.get('Actual Loops', 0) > 0:
                found.append(f"matches uses a {n['Node Type']}")
            if (n.get('Relation Name') == 'customers' and n.get('Actual Loops', 0) > 0
                    and n['Node Type'] not in ('Index Scan', 'Index Only Scan')):
                found.append(f"matches reads customers with a {n['Node Type']}")
    return sorted(set(found))


def check(what, sql, types, args):
    script = '\n'.join([
        'BEGIN;',
        'SET LOCAL ROLE otto_app;',
        f"SELECT set_config('app.org_id', {lit(ORG)}, true) \\g /dev/null",
        'SET LOCAL jit = off;',
        'SET LOCAL max_parallel_workers_per_gather = 0;',
        'SET LOCAL plan_cache_mode = force_generic_plan;',
        f'PREPARE s{types} AS {sql};',
        f'EXECUTE s{args} \\g /dev/null',
        f'EXPLAIN (ANALYZE, FORMAT JSON) EXECUTE s{args};',
        'ROLLBACK;',
    ])
    out = subprocess.run(['psql', '-X', '-q', '-At', '-v', 'ON_ERROR_STOP=1', URL],
                         input=script, capture_output=True, text=True, check=True).stdout
    plan = json.loads(out[out.index('['):])[0]
    return plan['Execution Time'], problems(plan['Plan'], what == 'customers')


failed = False
cust = statement('after-customers.sql', [(':org::uuid', '$1'), (':pat', '$2'), (':q', '$3'),
                                         ('50::bigint', '$4'), ('0::bigint', '$5')])
arts = statement('after-articles.sql', [(':org::uuid', '$1'), (':pat', '$3'), (':q', '$2'),
                                        ('20::bigint', '$4')])
for q in CUSTOMERS:
    ms, bad = check('customers', cust, '(uuid, text, text, bigint, bigint)',
                    f'({lit(ORG)}, {lit(pattern(q))}, {lit(q)}, 50, 0)')
    failed |= bool(bad)
    print(f"customers {lit(q):40} {ms:8.1f} ms  {'; '.join(bad) or 'per-candidate'}")
for q in ARTICLES:
    ms, bad = check('articles', arts, '(uuid, text, text, bigint)',
                    f'({lit(ORG)}, {lit(q)}, {lit(pattern(q))}, 20)')
    failed |= bool(bad)
    print(f"articles  {lit(q):40} {ms:8.1f} ms  {'; '.join(bad) or 'per-candidate'}")
sys.exit(1 if failed else 0)
