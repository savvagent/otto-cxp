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
