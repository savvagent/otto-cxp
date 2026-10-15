# Separate the migration owner from otto-cxp's runtime database role — design

**Status:** DRAFT
**Issue:** #27
**Plan:** `docs/plans/2026-10-15-split-migration-owner.md`

## Brief

> After #16, otto-cxp connects as `cxp_server`, which owns `otto_cxp` and every table in it so
> that it can run migrations at boot. A table owner can `ALTER TABLE … NO FORCE ROW LEVEL
> SECURITY`, `DISABLE ROW LEVEL SECURITY` or `DROP POLICY`. Any SQL execution as the connecting
> role could therefore turn off tenant isolation for every org. That includes the sweep, the
> usage shipper, tombstones, `/readyz`, or a future injection bug. The boot isolation proof runs
> only once. #16 confines the blast radius to `otto_cxp`; this issue closes the gap inside it.

Acceptance criteria (verbatim from #27):

1. The runtime login role owns no tenant table. It holds `otto_app` plus only the grants its
   background work needs, so it cannot alter RLS on any table.
2. Migrations run with a separate owner credential, either a NOLOGIN owner role the deploy
   assumes, or a release command with its own secret. The runtime role runs with
   `CXP_RUN_MIGRATIONS=false`, and boot refuses to run as a table owner in a deployment.
3. `docs/deploy/fly.md` documents the split and the exact production steps.

Also in scope (Rob's comment on #27, from the PR #26 reviews): re-run the database-role privilege
check after boot, in the maintenance sweep, logging at error or failing `/readyz`, since a later
`ALTER ROLE … BYPASSRLS` or `GRANT pg_read_all_data` goes unnoticed until restart; and `TRUNCATE`,
which, like `NO FORCE` / `DISABLE ROW LEVEL SECURITY` / `DROP POLICY`, ignores RLS for a table
owner.

Risks named on the ticket: production DDL and a new secret are human-only (Rob); the
`SECURITY DEFINER` functions and the maintenance policies must keep working when the runtime role
is not the owner.

## Premise corrections

- **"A NOLOGIN owner role the deploy assumes" does not close the gap.** A session can only assume
  (`SET ROLE`) a role it is a member of, and `SET ROLE` is ordinary SQL: if the runtime login
  could assume the owner at deploy time, an injection could too, at any time. (`db_role` already
  counts membership for exactly this reason.) The owner must be reachable only through a
  credential the serving process never logs in with. That makes the choice a **release command
  with its own secret**; a NOLOGIN owner can still exist, but only behind a second login, which
  adds a role and buys nothing.
- **A table owner is not the only role that can step around RLS.** `TRUNCATE` is a grantable
  privilege, and it ignores row-level security for whoever holds it. `TRIGGER` lets a non-owner
  attach a trigger to a tenant table. Owning the schema (`public` is owned by
  `pg_database_owner`, i.e. the database owner, on Postgres 15+) lets a role drop any table in
  it. The boot check therefore looks at what the runtime role owns in the database (the database
  itself, its schemas, relations, functions, types), through membership too, and at `TRUNCATE`
  and `TRIGGER` on any relation, not only at table ownership.
- **The runtime role needs exactly one grant beyond `otto_app`.** Every database path that runs
  as the connecting role rather than `SET LOCAL ROLE otto_app` was traced:
  - `maintenance::begin` transactions (the sweep, the usage shipper, expired console sessions):
    `DELETE`/`SELECT`/`UPDATE`/`INSERT` on `platform_events`, `usage_outbox`,
    `usage_outbox_rejected`, `console_sessions` — `otto_app` holds them, and the runtime role
    inherits them (a member with `INHERIT`, the default the #16 runbook used); the maintenance
    policies name no role, so they bind a non-owner exactly as they bound the owner under FORCE.
  - `SELECT expire_removed_members()` and `SELECT console_session_org($1)`: `EXECUTE` is granted
    to `otto_app` (`0001_baseline.sql:664`, `0003_console_sessions.sql:103`); both are
    `SECURITY DEFINER` and keep running as their owner.
  - `platform_events::revoked`, `cxp_core::ping`, the tenant-isolation proof: `SELECT`s
    `otto_app` holds, or none.
  - `platform_events::org_deleted`'s tombstone, `INSERT INTO deleted_orgs`
    (`crates/cxp-core/src/platform_events.rs:289`): the baseline **revokes** `INSERT` on
    `deleted_orgs` from `otto_app` (`0001_baseline.sql:682`), so a non-owner runtime role needs
    it granted. That is the one grant.
- **The `SECURITY DEFINER` functions need nothing.** `console_session_org` and
  `expire_removed_members` run as their owner, which becomes the migration owner. FORCE ROW
  LEVEL SECURITY binds that owner too, and `console_sessions_lookup` admits whichever role owns
  `console_session_org` at query time (it reads `pg_proc`), so the lookup follows the ownership
  transfer with no policy change. `removed_members` has no RLS; its owner may delete from it.
- **An ownership transfer takes `cxp_server`'s `CONNECT` with it.** The #16 runbook revoked
  `CONNECT` on `otto_cxp` from `PUBLIC` and granted it to `cxp_server` while `cxp_server` owned the
  database, so the entry is `cxp_server=c/cxp_server`. `REASSIGN OWNED` / `ALTER DATABASE … OWNER
  TO` rewrite every ACL entry naming the old owner, as grantee or grantor, to the new owner: after
  the transfer `cxp_server` could open no new connection. The runbook re-grants it in the same
  transaction, and its proof checks it.
- **Fly passes `release_command` to the image's `ENTRYPOINT`.** The Dockerfile's
  `ENTRYPOINT ["/usr/local/bin/cxp-server"]` has no `CMD`; Fly runs the release command as the
  machine's command, which Docker semantics append to the entrypoint. So the setting is
  `release_command = "migrate"` (the process runs `cxp-server migrate`); `"cxp-server migrate"`
  would run `cxp-server cxp-server migrate`. The gate run includes the built image run with that
  argument against the local database.
- **The running release cannot restart as a non-owner with migrations on.** Verified on Postgres
  16: sqlx's `CREATE TABLE IF NOT EXISTS _sqlx_migrations` fails with `permission denied for
  schema public` for a role without `CREATE` there, even when the table exists. So the runbook
  turns migrations off on the running release before it moves ownership.

## Assumptions

- Run Profile: every value comes from CLAUDE.md's "## Development Workflow"; none was assumed.
  Status labels: none exist, so no label steps. Deploy: `ci-auto`, but this PR stops at
  "open, reviewed, CI-green" and does not merge (Rob runs the runbook first).
- Fly runs `[deploy] release_command` in a temporary machine of the **new** image, with the new
  release's `[env]` and the app's secrets, before any machine is updated, and aborts the deploy
  if it exits non-zero. That is Fly's documented behavior and why the release command is chosen.
- The production roles today: `cxp_server` (LOGIN, owner of `otto_cxp` and everything in it,
  member of `otto_app`, `CONNECT` on `otto_cxp`, `SET` on `app.console_session_lookup`), as the
  #16 runbook left them. The runbook's first step verifies this read-only and its blocks refuse
  to run otherwise.
- The runtime role keeps its name, `cxp_server`, and its `DATABASE_URL`; the new owner is
  `cxp_owner`. Keeping the running secret and every existing log line and doc reference to
  `cxp_server` unchanged makes the production change one ownership transfer plus one new secret.
- `cxp_runtime` (NOLOGIN) is a new group role carrying the runtime's grants beyond `otto_app`.
  Roles are cluster-wide, so a migration cannot know a deployment's login name, but it can name a
  fixed group role (as it names `otto_app`) when it exists. A future migration that gives
  background work a new table privilege grants it to `cxp_runtime` the same way.
- `CXP_RUN_MIGRATIONS` keeps its `true` default (local runs, tests, CI); `fly.toml` sets it
  `false`. A deployment that serves with it `true` is refused (see Architecture).

## Goal & success criteria

The serving process connects as a role that owns nothing in `otto_cxp` and cannot truncate or
attach triggers to any table, so no SQL it runs can switch tenant isolation off or empty a table;
migrations run in Fly's release command as a separate owner login whose credential the serving
process never uses; and a privilege change made to the runtime role after boot is noticed within
the hour.

- In a deployment, boot refuses a runtime role that owns any object in the database (directly or
  through membership) or holds `TRUNCATE` or `TRIGGER` on any relation, naming what it found.
- In a deployment, boot refuses `CXP_RUN_MIGRATIONS=true`; `cxp-server migrate` migrates with
  `CXP_MIGRATION_DATABASE_URL` and refuses to fall back to the runtime URL there.
- An e2e test runs the whole service as a non-owner runtime role after an owner login migrated:
  requests, `org.deleted` (the tombstone), the sweep, the shipper, and the console lookup all
  work, and the runtime role cannot `ALTER TABLE`, `DROP POLICY`, or `TRUNCATE`.
- The hourly sweep re-inspects the role; a role that a deployment would refuse is logged at
  `error` and `/readyz` answers 503 until it is fixed.
- `docs/deploy/fly.md` has the runbook Rob runs before merge, with exact commands.

## Scope

**In:** the ownership/`TRUNCATE`/`TRIGGER` check in `cxp_core::db_role`; `cxp-server migrate`;
`CXP_MIGRATION_DATABASE_URL`; refusing an owner runtime role and `CXP_RUN_MIGRATIONS=true` in a
deployment; the runtime grant check (`INSERT` on `deleted_orgs`); migration 0005 (the grant to
`cxp_runtime`); the hourly re-check and its `/readyz` signal; `fly.toml` (`release_command`,
`CXP_RUN_MIGRATIONS = "false"`); `.env.example`; `docs/deploy/fly.md` (role section, runbook,
secrets table, Verify, Migrations); CLAUDE.md's config table and invariant text.

**Out:** per-service tenant roles instead of the shared `otto_app` (#28); checking that the
database holds every migration the binary embeds before serving (a skipped release command
aborts the deploy, so new machines never start; noted as a risk); revoking `CREATE` on schemas
from `PUBLIC` beyond what Postgres 15+ already does; anything on other services' databases.

## Architecture

### Roles after the split

| Role | Login | Owns | Holds | Used by |
|---|---|---|---|---|
| `cxp_owner` | yes | database `otto_cxp` and every object in it | `SET` on `app.console_session_lookup` | `cxp-server migrate` (Fly `release_command`), via `CXP_MIGRATION_DATABASE_URL` |
| `cxp_server` | yes | nothing | member of `otto_app` and `cxp_runtime`; `CONNECT` on `otto_cxp` | the serving process, via `DATABASE_URL` |
| `cxp_runtime` | no | nothing | `INSERT` on `deleted_orgs` (migration 0005) | group role for the runtime's grants beyond `otto_app` |
| `otto_app` | no | nothing | the tenant grants (unchanged) | `SET LOCAL ROLE` in tenant transactions |

Neither login is privileged (#16's check applies to both). `cxp_owner` is never a member of
`otto_app`: it never runs tenant work, and the baseline's `GRANT otto_app TO CURRENT_USER` already
tolerates `insufficient_privilege` on a fresh setup.

### Why a release command (and not a NOLOGIN owner the deploy assumes)

Recommendation: **`[deploy] release_command = "migrate"` (the entrypoint makes it
`cxp-server migrate`) with its own secret,
`CXP_MIGRATION_DATABASE_URL`**, connecting as the LOGIN role `cxp_owner`.

- *NOLOGIN owner assumed by the deploy:* the runtime login would have to be a member of the owner
  to `SET ROLE` to it, and then any SQL it runs, injected or not, can do the same. It fails AC 1
  in substance. Assuming it from a separate login is just the release-command design with an
  extra role.
- *Release command:* the owner credential is used only by a short-lived machine running the new
  image's `migrate` subcommand, before the new release's machines start; the serving process
  never reads `CXP_MIGRATION_DATABASE_URL`. Fly secrets are app-wide, so the variable is present
  in the serving machines' environment; that is acceptable for this issue's threat model (SQL run
  as the connecting role cannot read the process environment) and is called out in Risks. A
  failed migration aborts the deploy, leaving the old release serving, which is safer than
  today's crash-looping boot.
- *Advisory lock:* `migrate` still holds sqlx's advisory lock; there is one release machine at a
  time, so it never contends.

### `cxp_core::db_role` (`crates/cxp-core/src/db_role.rs`)

`DbRole` gains, read by `inspect_with` per user (session user, then current user when different),
skipped for a superuser user (already refused, and owns everything):

```rust
/// Objects in the current database owned by a role either user is a member of.
pub owned: Vec<OwnedObject>,   // at most MAX_REPORTED listed
pub owned_total: i64,          // how many in all
/// Relations either user may TRUNCATE or add a TRIGGER to without owning them.
pub table_privileges: Vec<TablePrivilege>, // at most MAX_REPORTED

pub struct OwnedObject { pub user: String, pub kind: &'static str /* "database" | "schema" |
    "table" | "function" | "type" ... */, pub name: String, pub owner: String }
pub struct TablePrivilege { pub user: String, pub table: String, pub privileges: Vec<&'static str> }
```

- Owned: `pg_database` (current database), `pg_namespace` (excluding `pg_catalog`,
  `information_schema`, `pg_toast*`, `pg_temp*`), `pg_class` (relations in those schemas, `relkind`
  mapped to table / view / sequence / index / ...), `pg_proc`, `pg_type` (excluding array and
  relation row types), `pg_extension` (an extension's owner may drop it, with every object in
  it), each `WHERE pg_has_role(user, owner, 'MEMBER')`. Membership counts as for
  privileges: a member can `SET ROLE` to the owner. Schema `public` owned by `pg_database_owner`
  is reported for the database owner (its implicit member).
- Table privileges: relations (`relkind IN ('r','p','v','m','f')`) in those schemas, not owned via
  membership, where `has_table_privilege(user, oid, 'TRUNCATE')` or `'TRIGGER'`.
- New methods: `owns_objects()`, `can_alter_tables()` (owned non-empty or table privileges
  non-empty). `is_refused_in_a_deployment()` keeps its meaning (privileged or reaches other
  databases) because `migrate` applies it to the owner; serving applies both.
- `summary()` keeps its current text and appends one clause before the shared-instance clause:
  - nothing owned and no table privileges: `; it owns nothing in this database and can TRUNCATE
    or add a TRIGGER to no table`;
  - otherwise `; it owns database "otto_cxp", schema "public", table "customers", … (N objects in
    all); can TRUNCATE table "x"`, each item prefixed with `session user "…" ` / `current user "…" `
    when the two users differ, as today.
- New: `pub async fn missing_runtime_grants(db: &Db) -> Result<Vec<&'static str>>` returning
  `["INSERT on deleted_orgs"]` when `deleted_orgs` exists and `has_table_privilege(current_user,
  'public.deleted_orgs', 'INSERT')` is false; empty otherwise (including before migrations).
  `RUNTIME_GRANTS` documents the list.

### Migration `0005_runtime_role_grants.sql`

```sql
-- The runtime login (cxp_server in production) owns nothing (#27). Beyond otto_app it needs only
-- what it does as itself rather than as otto_app: the org.deleted tombstone. Granted to the
-- NOLOGIN group role cxp_runtime, which the deployment creates and grants to its runtime login;
-- a migration cannot name a deployment's login, but it can name this role when it exists.
DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'cxp_runtime') THEN
    EXECUTE 'GRANT INSERT ON deleted_orgs TO cxp_runtime';
  ELSE
    RAISE NOTICE 'cxp_runtime does not exist; a runtime role that does not own deleted_orgs '
                 'cannot write the org.deleted tombstone until it is granted INSERT there';
  END IF;
END $$;
```

Additive only: a grant. The previous release reads and writes the same schema. Not a tenant
table, so no RLS or isolation-list change. No `SECURITY DEFINER` function is added.

### `cxp-server` (`crates/cxp-server`)

- **Config** (`config.rs`): `migration_database_url: Option<String>` from
  `CXP_MIGRATION_DATABASE_URL` (secret; redacted in `Debug` as `Some("<redacted>")`/`None`;
  `for_test` sets `None`). `CXP_RUN_MIGRATIONS` unchanged (default `true`).
- **`main.rs`**: `cxp-server` (no argument) serves as today; `cxp-server migrate` runs
  `boot::migrate(config)` and exits 0 on success; any other argument exits non-zero with a usage
  line. (An older image ignores arguments and would serve; see Error handling.)
- **`boot::migrate(config) -> Result<MigrateReport>`**: picks the URL —
  `migration_database_url`, else, only when not a deployment, `database_url`; in a deployment
  without it, refuses: `CXP_MIGRATION_DATABASE_URL is required in a deployment: migrations run as
  the database owner, never as the runtime role`. Connects with a 2-connection pool, inspects the
  role, applies `check_db_role` (privileges and other databases only: the migrator is meant to
  own), logs `applying migrations`, runs `cxp_core::migrate`, logs `migrations applied`, closes
  the pool. The shared-instance rule counts as a deployment here too.
- **`boot::check_db_role(role, config)`** (serving) refuses in a deployment when
  `role.is_refused_in_a_deployment() || role.can_alter_tables()`; the refusal and the `error`
  log name what was found and say the runtime role must own nothing and that migrations run as
  the owner through `cxp-server migrate` (docs/deploy/fly.md). Locally it warns, as today. A
  pure `role_verdict(role, config) -> Verdict { Clean, AllowedLocally, Refused }` backs both the
  boot check and the re-check. `check_migration_role(role, config)` is the migrate variant.
- **`run_with`**: after `check_db_role`, if `config.run_migrations` and the process is a
  deployment (config or shared instance), refuse: `refusing to serve: CXP_RUN_MIGRATIONS is on in
  a deployment; the runtime role cannot migrate: set CXP_RUN_MIGRATIONS=false and migrate with
  cxp-server migrate (the release command)`. With migrations off, log at `info`
  `not migrating: migrations run as the database owner through cxp-server migrate` in a
  deployment (the current `warn` wording stays for a local run). After migrations, call
  `missing_runtime_grants`; non-empty refuses in a deployment (`the runtime role cannot write
  the org.deleted tombstone (INSERT on deleted_orgs): GRANT cxp_runtime TO <role>`), warns
  locally.
- **The owner's credential never stays in a serving process** (`secret_env.rs`, `main.rs`). Fly
  secrets are app-wide, so `CXP_MIGRATION_DATABASE_URL` is in every serving machine's
  environment. `main` is no longer `#[tokio::main]`: when serving, its first act (after argument
  parsing, while single-threaded) is to re-execute itself without the variable if present
  (`std::os::unix::process::CommandExt::exec`, same executable and arguments, environment minus
  the variable plus the marker `CXP_MIGRATION_SECRET_REMOVED=1`), because `unsetenv` leaves the
  original block in memory and `/proc/<pid>/environ`. A failed exec refuses to serve. A value a
  local `.env` supplies afterwards is removed with `remove_var` before the runtime starts.
  `run_with` also drops `Config::migration_database_url`. The re-executed process logs that the
  variable was removed; nothing logs its value. `migrate` keeps and uses it. Tested on the built
  binary (`tests/secret_env.rs`, reading `/proc/<pid>/environ`).
- **Re-check** (`boot::recheck_db_role(db, config, alarm)`), called by `sweep_loop` after each
  sweep pass (hourly; the first pass runs at boot, right after the boot check, and is harmless):
  re-runs `inspect` and `role_verdict`. `Refused` → `tracing::error!` with the summary
  (`the database role changed after boot: <summary>; /readyz answers 503 until it is fixed`) and
  sets the alarm; otherwise clears it (logging `info` once when it clears). An `inspect` error is
  logged at `warn` and leaves the alarm as it was. Locally (`AllowedLocally`) nothing is logged.
- **`/readyz`** (`health.rs`): a `RoleAlarm` (`Arc<RwLock<Option<String>>>`, cheap clone) joins
  the platform check in the health state. While set, `/readyz` answers 503
  `{"status":"unready","reason":"database role refused"}` and logs `warn` per probe (the reason
  never carries the summary: no catalog text in a response). Consequence, chosen deliberately
  (fail closed, as boot does): `/readyz` is the app's only Fly health check, so with one machine a
  raised alarm takes the service out of rotation until the role is fixed. Recovery is the fix
  itself (revoke the grant or attribute; the next hourly pass clears the alarm) or, to clear it at
  once, a machine restart, which re-runs the boot check. fly.md says so. `router()`'s last parameter
  becomes `impl Into<health::Readiness>`, with `From<PlatformCheck>` so existing callers compile
  unchanged; boot builds a `Readiness` holding the alarm it shares with the sweeper.

### Public-interface changes

All additive: the `migrate` subcommand, `CXP_MIGRATION_DATABASE_URL`, `DbRole`'s new fields and
methods, `missing_runtime_grants`, `health::Readiness`/`RoleAlarm`, `/readyz`'s new reason string.
Behavior change (intended): a deployment refuses an owning runtime role and
`CXP_RUN_MIGRATIONS=true`.

## Invariants impact

- **Append-only migrations:** new `0005_runtime_role_grants.sql`, a grant only; previous release
  unaffected. Renumbered at rebase if another PR takes 0005.
- **Tenant isolation is RLS, proven:** strengthened. The boot proof is unchanged; RLS now binds a
  runtime role that cannot alter, drop, or truncate any table. Maintenance and the tombstone still
  run as the connecting role, now a non-owner, bound by the same policies. CLAUDE.md's invariant
  text about `cxp_server` owning the tables (and #27 being open) is updated.
- **`SECURITY DEFINER` functions:** unchanged; they run as `cxp_owner`; `console_sessions_lookup`
  follows the function's owner. No new one.
- **Composite FKs, begin_live, no SQL outside cxp-core, runtime queries only:** all new SQL lives
  in `cxp-core` (`db_role.rs`, the migration) as runtime `sqlx::query*`; `cxp-server` calls
  functions.
- **Auth, metering, teams, webhooks, untrusted text:** untouched. `/readyz`'s new reason is a
  fixed string.
- **Small shared database:** `migrate` uses a 2-connection pool for seconds; the re-check is a few
  catalog queries per hour on the existing pool; listed objects are capped at `MAX_REPORTED`.
- **Licensing, pin:** unchanged.

## Migration

`crates/cxp-core/migrations/0005_runtime_role_grants.sql` as above. Compatible with the code
running during rollout in both directions: it adds one grant to a role that may not exist.

## Out-of-band

All human-only (Rob), **before merge**, per CLAUDE.md "Database role change" and "New secret":

1. The runbook in `docs/deploy/fly.md` ("Runbook: split the migration owner from the runtime
   role", steps S0–S7): look (read-only); turn migrations off on the running release
   (`CXP_RUN_MIGRATIONS=false` as a secret, kept permanently: it also stops a rollback to a
   pre-#27 image, whose `fly.toml` lacks the setting, from migrating as a non-owner); check
   Postgres is 15+ (schema `public` follows the database owner); create `cxp_owner` and
   `cxp_runtime`; set `CXP_MIGRATION_DATABASE_URL`; in one transaction transfer ownership from
   `cxp_server` to `cxp_owner`, re-grant `CONNECT` on `otto_cxp` to `cxp_server`, grant `INSERT`
   on `deleted_orgs` to `cxp_runtime` by hand (idempotent with 0005, so the old release's
   tombstone keeps working before 0005 runs), add `ALTER DEFAULT PRIVILEGES FOR ROLE cxp_owner`
   entries and remove `cxp_server`'s, and revoke the parameter grant from `cxp_server`; prove
   both roles read-only and restart the old release to prove it boots as a non-owner; then merge
   (or hand deploy if `FLY_DEPLOY` is not enabled) and verify the release-command and boot log
   lines.
2. New secret `CXP_MIGRATION_DATABASE_URL`: in the secrets table and `.env.example`.
3. New non-secret config (agent, in this PR): `CXP_RUN_MIGRATIONS = "false"` in `fly.toml`
   `[env]` and `[deploy] release_command`; `config.rs` and `.env.example` document
   `CXP_MIGRATION_DATABASE_URL`.

## Error handling & edge cases

- **Deployment, runtime role still the owner** (runbook not run): boot refuses, naming the owned
  objects. Expected; the PR is not merged until the runbook has run.
- **Release command without `CXP_MIGRATION_DATABASE_URL`:** refuses with the variable's name; the
  deploy aborts; the old release keeps serving.
- **`CXP_MIGRATION_DATABASE_URL` pointing at a privileged role:** refused like a serving role.
- **A rollback to a pre-#27 image:** that binary ignores `migrate` and would start serving inside
  the release machine until Fly's release-command timeout, aborting the deploy. The runbook's
  rollback section says to roll back with `fly deploy --image <old> --config <fly.toml without
  release_command>` or by reverting this PR; either way the `CXP_RUN_MIGRATIONS=false` secret
  keeps the old binary from migrating as a non-owner.
- **Missing `cxp_runtime` grant:** boot refuses in a deployment (org.deleted would 503 forever);
  warns locally.
- **Re-check while the database is unreachable:** `warn`, alarm unchanged; `/readyz` reports the
  database itself anyway.
- **Re-check after the alarm was raised and the grant is revoked again:** clears, `info` once.
- **Machine suspended:** the sweep, and so the re-check, runs again on resume (within the hour).
- **A raised alarm is an outage** on a one-machine app (see `/readyz` above); a stray
  `GRANT TRUNCATE` to the runtime role is enough. Intended: such a role is what boot refuses.
- **Superuser locally:** owns everything; skipped like the other per-user reads; allowed with the
  existing warning.
- **Test cluster:** the per-test databases' `PUBLIC` defaults do not change the owner reads (they
  are scoped to the current database).

## Testing approach

Gates: `DATABASE_URL=postgres://cxp:cxp@localhost:15464/otto_cxp cargo test --workspace --locked`
(this worktree's own container), clippy, fmt, the migrations-append-only diff, `podman build -t
otto-cxp:check-issue27 .` (crates change). No pnpm paths change. Beyond the gates, two manual
proofs recorded in the PR: the built image run as Fly will run it (`podman run --rm --network
host -e … otto-cxp:check-issue27 migrate` against a scratch database, as a non-privileged owner,
in a deployment configuration), and the runbook's SQL (S1, S4, S5) run in order against a local
database put in production's #16 state (`cxp_server` owning everything, `PUBLIC` `CONNECT`
revoked), after which the new binary serves as `cxp_server`.

- `crates/cxp-core/tests/db.rs` (`RoleGuard` helpers already create and drop roles):
  `the_database_role_check_sees_what_a_role_owns_and_can_truncate` — a plain member of `otto_app`
  is clean; the database owner reports the database and schema `public`; a table owner reports
  the table; a member of a table owner reports it through membership; a role granted `TRUNCATE`
  or `TRIGGER` on one table reports that; the summary clause in each case.
  `missing_runtime_grants_names_the_tombstone_insert` — empty for the superuser pool and for a
  member of `cxp_runtime` after migrating; non-empty for a plain `otto_app` member.
- `crates/cxp-core/src/db_role.rs` unit tests: summary text for owned/privileged/clean,
  `can_alter_tables`.
- `crates/cxp-server/src/boot.rs` unit tests: `role_verdict` for each combination;
  `check_db_role` refuses an owner or `TRUNCATE` holder in a deployment, warns locally;
  `check_migration_role` allows an owner; `migration_url` picks and refuses as specified.
- `crates/cxp-server/src/config.rs`: `CXP_MIGRATION_DATABASE_URL` parsed and redacted.
- `crates/cxp-server/src/health.rs`: the alarm holds readiness back with the fixed reason.
- `crates/cxp-server/tests/e2e.rs`:
  - the least-privilege test becomes `the_service_runs_whole_as_a_runtime_role_that_owns_nothing`:
    an owner login migrates through `boot::migrate` (deployment config, migration URL set); the
    runtime login (member of `otto_app` and `cxp_runtime`, owns nothing) serves with
    `run_migrations = false`; requests, `org.deleted`, sweep, shipper, console lookup as before;
    then as the runtime role `ALTER TABLE customers NO FORCE ROW LEVEL SECURITY`,
    `DROP POLICY customers_tenant_isolation ON customers`, `TRUNCATE customers` each fail with
    `42501`/must-be-owner; log lines in order.
  - `a_deployment_refuses_an_owner_as_its_runtime_role` — the owner login as `database_url`,
    `run_migrations = false`: refused, error names the owned objects, nothing listens.
  - `a_deployment_refuses_to_migrate_as_the_runtime_role` — `run_migrations = true` in a
    deployment with a clean runtime role: refused naming `CXP_RUN_MIGRATIONS`.
  - `migrate_needs_its_own_credential_in_a_deployment` — no migration URL: refused, nothing
    migrated; a superuser migration URL in a deployment: refused.
  - `the_role_recheck_raises_and_clears_the_readiness_alarm` — clean runtime role → alarm clear;
    `GRANT pg_read_all_data TO <runtime>` → `recheck_db_role` sets the alarm and the health router
    answers 503 `database role refused`; revoke → cleared.
  - Mutation check (manual, recorded in the ledger): with `can_alter_tables()` dropped from the
    serving verdict, `a_deployment_refuses_an_owner_as_its_runtime_role` fails.

## Risks & open questions

- **The owner credential reaches every machine** (Fly secrets are app-wide). The serving process
  re-executes itself without it, so SQL as `cxp_server`, code execution as the unprivileged
  `cxp` user inside the serving process, and environment dumps of it do not find it. Root on a
  serving machine (Fly's init holds the secrets), and anyone holding the app's deploy token, still
  can. The stronger arrangement, a separate Fly app holding only the owner credential and running
  `cxp-server migrate` from the deploy job, is the follow-up #45 (another app, token, and CI
  ordering).
- **No schema-version check before serving.** If the release command were skipped, a newer binary
  would serve against an older schema and fail per query. Fly aborts the deploy when the release
  command fails, so new machines do not start; a follow-up could read `_sqlx_migrations` (it would
  need a `SELECT` grant to `cxp_runtime`).
- **The re-check is hourly and suspended with the machine.** A privilege granted and used within
  the hour is not prevented, only noticed; the boot proof and the role design are the
  prevention.
- **`cxp_runtime` is cluster-wide** on the shared `otto-db`; its name is otto-cxp's. Only this
  service grants to it.
- **Production steps are human-only** and must run before merge; this agent ran them only
  against a local container.
