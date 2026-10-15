# Console refresh hardening before sign-in is enabled

**Status:** DRAFT

## Brief

Issue #31 ("Enable web console sign-in in production"). Its acceptance
criteria are human-only steps (register the platform client, stage
`CXP_ENCRYPTION_KEY`) and a `fly.toml` PR that depends on them; those stay
open for Rob. This change is the hardening Rob's comment on the issue asks to
land before or with enabling, from the #30 database review:

> enforce the refresh store budget with SET LOCAL lock_timeout/statement_timeout
> in store_refreshed and release_lease (today the 20 s allowance is assumed,
> not enforced; worst case a user is signed out, nothing leaks); give
> delete_generation the same refresh_lease_until condition; add a test that
> the console_sessions_lookup policy survives DROP OWNED BY a previous owner.

Plus, from the caller: make sure `docs/deploy/fly.md` carries the enabling
runbook with the issue's exact steps, including the live verification.

## Premise corrections

- `docs/deploy/fly.md` already has a "Web console sign-in" section with the
  client registration, the staged secret, and the `fly.toml` change, in that
  order. It lacks the issue's live-verification checklist (a cross-site write
  refused, sign-out revoking the session). This change adds that as step 4
  rather than a second, duplicate section.
- The store transaction's first wait is not the `UPDATE`: `begin_live` takes
  the org's shared lifecycle lock (`pg_advisory_xact_lock_shared`) and runs the
  revocation check before the caller's first statement. A `SET LOCAL` issued
  after `begin_live` returns would leave both unbounded. The timeouts must be
  set inside `begin_live`, right after the pinned transaction opens and before
  the lifecycle lock.

## Assumptions

- Timeouts: `lock_timeout = 2 s`, `statement_timeout = 3 s`. After they are set
  the store runs at most four statements that can wait (the lifecycle lock,
  the revocation check, the `UPDATE`, the `COMMIT`), so 4 x 3 s + the pool's
  5 s acquire timeout = 17 s, inside the 20 s `STORE_ALLOWANCE`, with 3 s for
  the opening statements (`BEGIN`, `SET LOCAL ROLE`, the org setting, the
  timeouts themselves), which wait on nothing. Single-row statements finish in
  milliseconds; 2 s of lock wait is far beyond any competing statement on the
  row (a claim, a sign-out, a sweep batch).
- `release_lease` gets the same timeouts: it is best effort already (a lapsed
  lease frees itself), so failing fast costs nothing.
- `delete_generation` takes the whole `Lease` (generation and expiry), as
  `store_refreshed` and `release_lease` do; it gets no timeouts (not asked,
  and nothing waits on its timing).
- The pool's acquire timeout moves to one constant,
  `cxp_core::POOL_ACQUIRE_TIMEOUT`, used by `cxp-server`'s pool and by the
  compile-time budget check in `cxp-http`, so the arithmetic cannot drift.
- Run Profile: every value comes from the CLAUDE.md "Development Workflow"
  section; none assumed.

## Goal & success criteria

Make the store allowance a guarantee, not an assumption, and close the one
lease-blind delete.

- `store_refreshed` and `release_lease` fail with an error, without writing,
  when the row lock or the org's lifecycle lock is held past 2 s; they never
  wait unbounded.
- A compile-time assertion in `cxp-http` proves
  `POOL_ACQUIRE_TIMEOUT + 4 x statement_timeout <= STORE_ALLOWANCE`.
- `delete_generation` deletes nothing when the session's lease has been
  claimed anew since (same generation, different `refresh_lease_until`).
- A test proves `console_sessions_lookup` survives `REASSIGN OWNED BY old` +
  `DROP OWNED BY old` + `DROP ROLE old`, and that the same steps drop a policy
  that names the old role (so the test can fail).
- `docs/deploy/fly.md` lists the four enabling steps, verification included.

## Scope

In: `crates/cxp-core/src/{lib.rs,platform_events.rs,console_sessions.rs}`,
`crates/cxp-http/src/console/mod.rs`, `crates/cxp-server/src/boot.rs`, tests in
`crates/cxp-core/tests/{console_sessions.rs,db.rs}`, `docs/deploy/fly.md`.

Out: setting `CXP_CONSOLE_CLIENT_ID` in `fly.toml`; the human-only steps;
timeouts on `claim_refresh` (its duration is already measured and checked by
`fits_in_lease`); any migration.

## Architecture

1. `cxp_core::platform_events` gains `Timeouts { lock: Duration, statement:
   Duration }` and `begin_live_bounded(db, org, user, timeouts)`. `begin_live`
   and it share one body; the bounded one runs
   `SELECT set_config('lock_timeout', $1, true), set_config('statement_timeout', $2, true)`
   (values in milliseconds, bound, never formatted into SQL) right after
   `db.begin(org)` and before the lifecycle lock (`platform_events.rs:131`).
   `set_config(..., true)` is `SET LOCAL`: it ends with the transaction, so a
   pooled connection never carries it to the next borrower.
2. `cxp_core::console_sessions` gains `pub const STORE_TIMEOUTS: Timeouts` and
   `pub const STORE_STATEMENTS: u32 = 4`; `store_refreshed` and
   `release_lease` open with `begin_live_bounded(.., STORE_TIMEOUTS)`.
   `delete_generation(db, org, id_hash, lease: Lease)` adds
   `AND refresh_lease_until = $4`. Module docs updated.
3. `cxp_core::POOL_ACQUIRE_TIMEOUT` (5 s) in `lib.rs`; `boot.rs:187` uses it.
4. `cxp-http/src/console/mod.rs`: the compile-time block also asserts the
   store budget; the module docs replace "the statements are single-row" with
   the enforced bound; the `InvalidGrant` arm passes `lease`.

Public interface: `delete_generation`'s signature changes (crate-internal
callers only: `cxp-http`); the other additions are additive. No wire, route,
tool, config key, or schema change.

## Invariants impact

- Request paths open with `begin_live`: still true; `begin_live_bounded` is
  `begin_live` plus two `SET LOCAL`s, including the revoked-org check.
- No SQL outside `cxp-core`; runtime `sqlx::query` only: held.
- Database error text never reaches a response: a timeout is a
  `cxp_core::Error` from sqlx, mapped by `db_failure` to `503`, logged with
  `log_detail()` only.
- Small shared database: the change shortens the longest a refresh can hold a
  pooled connection.

## Migration

None.

## Out-of-band

None for this change. Enabling sign-in remains Rob's (#31): client register,
`fly secrets set --stage`, then the `fly.toml` PR, then live verification.

## Error handling & edge cases

- Store times out (lock or statement): the platform has rotated the pair; the
  existing `Err(e)` arm revokes the new pair and answers `503`; the stored
  refresh token is spent, so the next claim signs the person out. This is the
  issue's stated worst case: nothing leaks, nothing is presented twice by this
  holder.
- An `org.deleted` purge holding the lifecycle lock exclusively: the store now
  times out after 2 s instead of waiting for the purge; the session is being
  deleted anyway.
- `release_lease` times out: logged at `warn` (existing), lease lapses on its
  own.
- `delete_generation` with a stale lease: returns 0, the newer holder's session
  is untouched; the caller still answers `401` (`Inactive`), which is correct
  for this request because its refresh token was refused.

## Testing approach

`DATABASE_URL=... cargo test --workspace --locked` against a local Postgres.

- `console_sessions.rs`: hold the session row (`SELECT ... FOR UPDATE` on a
  second connection) and assert `store_refreshed` and `release_lease` return
  `Err` within a few seconds and the row is unchanged afterwards; hold the
  org's lifecycle lock exclusively and assert the same for `store_refreshed`;
  assert the settings do not leak to the next transaction on the connection.
- `console_sessions.rs`: `delete_generation` with a lease superseded at the
  same generation deletes nothing; with the current lease deletes the row.
- `db.rs`: the `DROP OWNED BY` survival test with the control policy.
- `cxp-http` budget: compile-time assertion.

## Risks & open questions

- `statement_timeout` applying to `COMMIT`: PostgreSQL arms it per statement
  message, `COMMIT` included, while `SET LOCAL` values are still in force; if a
  future version changes that, the bound loosens by one commit, which on a
  single-row update is negligible.
