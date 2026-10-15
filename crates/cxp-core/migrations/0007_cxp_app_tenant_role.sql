-- otto-cxp's own tenant role (#28). Tenant transactions SET LOCAL ROLE cxp_app
-- (cxp_core::TENANT_ROLE) instead of otto_app, which the otto services on otto-db may share: a
-- privilege any of them grants otto_app would otherwise reach otto-cxp's tenant transactions.
--
-- Roles are cluster-wide. Create it where the migrating role may (tests, a local superuser),
-- tolerating a concurrent creator. Where it may not (production's cxp_owner has no CREATEROLE)
-- it must already exist (docs/deploy/fly.md, runbook T1); otherwise this migration fails, so the
-- release command aborts the deploy instead of recording 0007 with no grants made.
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'cxp_app') THEN
    BEGIN
      CREATE ROLE cxp_app NOLOGIN;
    EXCEPTION
      WHEN duplicate_object OR unique_violation THEN
        NULL; -- another database in this cluster created it first
      WHEN insufficient_privilege THEN
        -- The fix goes in the message itself: sqlx reports a migration's error message and
        -- drops its HINT.
        RAISE EXCEPTION 'role cxp_app does not exist and % may not create it: as a superuser, '
                        'CREATE ROLE cxp_app NOLOGIN; GRANT cxp_app TO <the runtime login>; then '
                        'migrate again (docs/deploy/fly.md, runbook T1)', current_user;
    END;
  END IF;
END $$;

-- The same grants 0001, 0003, and 0004 gave otto_app, one statement each over every table,
-- sequence, and function in public, then 0001's three narrowings. otto_app keeps its own: the
-- previous release runs tenant transactions as otto_app until this one replaces it, and again
-- after a rollback. A later migration revokes them.
GRANT USAGE ON SCHEMA public TO cxp_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO cxp_app;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO cxp_app;
-- Includes the two SECURITY DEFINER functions, expire_removed_members() and
-- console_session_org(bytea): both are REVOKE ALL ... FROM PUBLIC and granted to the tenant role
-- by name, as 0001 and 0003 did for otto_app.
GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA public TO cxp_app;
-- Tables and sequences a later migration creates as the migrating role (cxp_owner in
-- production) get the same.
ALTER DEFAULT PRIVILEGES IN SCHEMA public
  GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO cxp_app;
ALTER DEFAULT PRIVILEGES IN SCHEMA public
  GRANT USAGE, SELECT ON SEQUENCES TO cxp_app;
-- As 0001 for otto_app: a tombstone is permanent and written by the runtime role (cxp_runtime,
-- 0005), never by a tenant transaction; removed members expire only through
-- expire_removed_members(); the migrator's ledger is not the app's.
REVOKE INSERT, UPDATE, DELETE ON deleted_orgs FROM cxp_app;
REVOKE DELETE ON removed_members FROM cxp_app;
REVOKE ALL ON public._sqlx_migrations FROM cxp_app;
