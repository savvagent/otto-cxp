# Least-privilege database role (#16) — plan

Spec: `docs/specs/2026-10-14-least-privilege-db-role-design.md`. Gates use the worktree's own
Postgres (`DATABASE_URL=postgres://cxp:cxp@localhost:<port>/otto_cxp`).

- [ ] **1. Read the role's privileges (`cxp-core`).**
  - Failing test first: `crates/cxp-core/tests/db.rs`
    `the_database_role_check_sees_every_superuser_equivalent` (plain, BYPASSRLS, member of
    BYPASSRLS, CREATEROLE, REPLICATION, `pg_execute_server_program`, `pg_read_all_data`,
    superuser pool) and `the_database_role_check_sees_a_superuser_session_behind_set_role`.
  - Add `crates/cxp-core/src/db_role.rs` (`DbRole`, `PrivilegedRole`, `inspect`, `summary`, unit
    test of the summary) and `pub mod db_role;` in `lib.rs`.
  - `cargo test -p cxp-core --test db the_database_role`

- [ ] **2. Refuse a privileged role in a deployment (`cxp-server`).**
  - Failing tests first: `config.rs` `only_loopback_urls_without_fly_are_not_a_deployment`;
    `boot.rs` `a_deployment_refuses_a_privileged_database_role`.
  - `Config::is_deployment`; `boot::check_db_role`, called in `run_with` after connecting and
    before migrations; `Boot::database_role`.
  - `cargo test -p cxp-server --lib`

- [ ] **3. Prove the whole service as a least-privilege owner (`tests/e2e.rs`).**
  - `the_service_runs_whole_as_a_least_privilege_database_owner` and
    `a_deployment_refuses_a_superuser_database_role` (`#[sqlx::test(migrations = false)]`; the
    test creates and drops its own role).
  - The existing boot test also expects the loopback warning line.
  - Mutation check: with `maintenance::begin` not setting the marker, the first test fails.
  - `cargo test -p cxp-server --test e2e`

- [ ] **4. Runbook and wording.**
  - `docs/deploy/fly.md`: "Database role" section (R0 to R7), Infrastructure paragraph, secrets
    table, first-deploy note, Verify log lines.
  - `fly.toml` header comments, `.env.example` database comment, `CLAUDE.md` isolation invariant
    and `CXP_PUBLIC_URL` row.
  - Dry-run the runbook SQL on a local container with a simulated superuser attach role; boot as
    the new role with a non-loopback public URL; restore the container.

- [ ] **5. Gates and commit.**
  - `DATABASE_URL=... cargo test --workspace --locked`
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`
  - `cargo fmt --all -- --check`
  - `git diff --name-status --diff-filter=MDR origin/main...HEAD -- 'crates/*/migrations/*'`
    prints nothing
  - Commit `feat(server): refuse a privileged database role in a deployment`, body ending
    `Fixes #16`.
