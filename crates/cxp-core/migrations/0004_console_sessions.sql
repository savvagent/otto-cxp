-- The web console's sign-in sessions (#14): the platform token pair otto-cxp
-- holds for a browser that signed in through the platform (OAuth authorization
-- code + PKCE), so the browser itself never sees a token.
--
-- The browser holds a random 32-byte cookie that is never stored; `id_hash` is
-- its SHA-256, so a database dump cannot be replayed as a cookie. The tokens
-- are sealed with CXP_ENCRYPTION_KEY (AES-256-GCM, stored as nonce ||
-- ciphertext), so a dump alone yields no credential either.
--
-- `org_id` and `user_id` are the platform's opaque ids, taken from the
-- introspected token, never from a request. A token opens exactly one org, so
-- a session does too. `expires_at` is when the refresh token it holds stops
-- working; past it the row is dead weight, and the hourly sweep deletes it.
-- Nothing references this table and it references nothing, so it has no
-- foreign keys.
--
-- Additive only: a previous release ignores the table and the function.

CREATE TABLE console_sessions (
  id_hash           bytea       PRIMARY KEY CHECK (octet_length(id_hash) = 32),
  org_id            uuid        NOT NULL,
  user_id           uuid        NOT NULL,
  access_token_enc  bytea       NOT NULL,
  refresh_token_enc bytea       NOT NULL,
  access_expires_at timestamptz NOT NULL,
  created_at        timestamptz NOT NULL DEFAULT now(),
  expires_at        timestamptz NOT NULL
);

-- "This user's sessions in this org" (member.removed) and "this org's
-- sessions" (the org.deleted purge). The sweep has no index of its own: an
-- index that does not lead with org_id is reserved for the two cross-org
-- queues (tests/db.rs), and this table holds one row per signed-in browser,
-- small enough to scan hourly in bounded batches.
CREATE INDEX console_sessions_org_user_idx ON console_sessions (org_id, user_id);

-- Tenant isolation, as on usage_outbox: a pinned transaction reaches its own
-- org's sessions only, and background maintenance (the expiry sweep, with the
-- app.maintenance marker) may read and delete across orgs. WITH CHECK has no
-- maintenance branch: nothing unpinned ever writes a session.
ALTER TABLE console_sessions ENABLE ROW LEVEL SECURITY;
ALTER TABLE console_sessions FORCE ROW LEVEL SECURITY;
CREATE POLICY console_sessions_tenant_isolation ON console_sessions
  USING (org_id = current_org()
         OR (current_org() IS NULL AND current_setting('app.maintenance', true) = 'on'))
  WITH CHECK (org_id = current_org());

-- A request arrives with a cookie and no org: the cookie is how the org is
-- learned. That one unpinned read goes through console_session_org() below,
-- which answers with the org of the one unexpired session whose hash it is
-- given, never a token; the request then pins that org and reads the row under
-- the tenant policy above.
--
-- The function is SECURITY DEFINER, so it runs as its owner (the migrating
-- role). While that role is a superuser it bypasses RLS; once it is not (#16),
-- FORCE ROW LEVEL SECURITY binds it too, and this SELECT-only policy is what
-- admits the lookup: unpinned, and only while app.console_session_lookup is
-- 'on', which the function's own SET clause sets for the duration of the call
-- and nothing else sets. A request path that forgot to pin still sees nothing.
CREATE POLICY console_sessions_lookup ON console_sessions
  FOR SELECT
  USING (current_org() IS NULL
         AND current_setting('app.console_session_lookup', true) = 'on');

CREATE FUNCTION console_session_org(session_hash bytea) RETURNS uuid
  LANGUAGE sql STABLE SECURITY DEFINER
  SET search_path = pg_catalog, public
  SET app.console_session_lookup = 'on'
AS $$
  SELECT org_id FROM public.console_sessions
  WHERE id_hash = session_hash AND expires_at > now();
$$;
REVOKE ALL ON FUNCTION console_session_org(bytea) FROM PUBLIC;

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'otto_app') THEN
    EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON console_sessions TO otto_app';
    EXECUTE 'GRANT EXECUTE ON FUNCTION console_session_org(bytea) TO otto_app';
  END IF;
END $$;
