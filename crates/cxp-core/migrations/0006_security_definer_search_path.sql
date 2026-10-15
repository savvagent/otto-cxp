-- The two SECURITY DEFINER functions, recreated with the same behavior but nothing left to name
-- resolution (#27). A SECURITY DEFINER function runs as its owner, the migration owner, so any
-- name it resolves through search_path is a place a lesser role could plant code that then runs
-- with the owner's rights (the CVE-2018-1058 pattern): an exact-match overload of now() or
-- make_interval() in a schema the caller can CREATE in, or a temporary table or function in
-- pg_temp, which Postgres searches first unless search_path names it explicitly.
--
-- So every call is schema-qualified (pg_catalog.now(), pg_catalog.make_interval(),
-- pg_catalog.count()), every table public-qualified, and search_path is pinned to
-- pg_catalog, public, pg_temp: pg_temp named last is never searched first, and is not searched
-- for functions at all. The operators (<, >, -, =) are left unqualified: each has an exact
-- match for its operand types in pg_catalog, which comes first, so no other schema's operator
-- can be preferred to it. 0001 and 0003 are applied and stay as they are (migrations are
-- append-only); CREATE OR REPLACE keeps each function's oid, owner, and grants, and the
-- console_sessions_lookup policy, which names console_session_org by signature, follows it.
--
-- CREATE OR REPLACE of console_session_org restates its SET app.console_session_lookup clause,
-- which needs the migrating role to hold SET on that custom parameter (runbook R1), as 0003 did.

CREATE OR REPLACE FUNCTION expire_removed_members() RETURNS bigint
  LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog, public, pg_temp AS $$
  WITH gone AS (
    DELETE FROM public.removed_members
    WHERE removed_at < pg_catalog.now() - pg_catalog.make_interval(secs => 600)
    RETURNING 1
  )
  SELECT pg_catalog.count(*) FROM gone;
$$;
-- CREATE OR REPLACE keeps the grants; restated so the invariant (no PUBLIC EXECUTE on a
-- SECURITY DEFINER function) reads true from this file alone.
REVOKE ALL ON FUNCTION expire_removed_members() FROM PUBLIC;

CREATE OR REPLACE FUNCTION console_session_org(session_hash bytea) RETURNS uuid
  LANGUAGE sql STABLE SECURITY DEFINER
  SET search_path = pg_catalog, public, pg_temp
  SET app.console_session_lookup = 'on'
AS $$
  SELECT org_id FROM public.console_sessions
  WHERE id_hash = session_hash AND expires_at > pg_catalog.now();
$$;
REVOKE ALL ON FUNCTION console_session_org(bytea) FROM PUBLIC;
