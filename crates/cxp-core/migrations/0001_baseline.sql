-- otto-cxp domain baseline.
--
-- This database holds otto-cxp's customer-experience data -- customers and
-- their interactions and segments, support tickets, the knowledge base, chat,
-- feedback and surveys, and the experience catalog -- plus otto-cxp's own
-- bookkeeping (usage outbox, lifecycle-event log, tombstones). It holds no
-- identity: users, orgs, teams, memberships, tokens, plans, and usage totals
-- belong to the otto platform, in another database, so nothing here has a
-- foreign key to them. An `org_id`, user id, or `team_id` in this schema is an
-- opaque uuid the platform issued. The platform's signed lifecycle webhooks
-- (`POST /platform/webhooks`) clean up after deletions.
--
-- It replaces the pre-platform `server/migrations` (001..010) for a fresh
-- database: the same tables, now org-scoped, minus the local `teams` table
-- (teams belong to the platform), `feedback_embeddings` and the `vector`
-- extension (nothing read or wrote them), the `uuid-ossp` extension
-- (`gen_random_uuid()` is built in), and feedback's legacy free-text
-- experience/application/page/component columns (the catalog foreign keys
-- replaced them).
--
-- Tenant isolation: every table carrying an org's rows has a NOT NULL `org_id`,
-- FORCE ROW LEVEL SECURITY, and a policy named `<table>_tenant_isolation`
-- (`Db::verify_tenant_isolation` discovers tenant tables by that name).
-- Every reference between tenant tables is a composite
-- `(org_id, x_id) REFERENCES x (org_id, id)` foreign key: foreign-key checks
-- bypass row-level security, so a plain `REFERENCES x (id)` would accept
-- another org's id; keying on (org_id, id) makes the database refuse it.
-- Nullable references to the catalog from customers, tickets, feedback, and
-- surveys use `ON DELETE SET NULL (x_id)` (Postgres 15+), which nulls only the
-- reference and leaves the NOT NULL `org_id` alone; owned children, including
-- each catalog level under its parent, use `ON DELETE CASCADE`. Indexes lead with `org_id` (except the
-- GIN indexes, which cannot without btree_gin; RLS's `org_id` predicate is
-- combined with them through the `(org_id, ...)` btree indexes).
--
-- Append-only from here. sqlx checksums applied migrations; editing this file
-- after a deployment has run it stops that deployment from booting. CI enforces
-- this (`migrations-append-only`).

-- The role every tenant transaction runs as. NOLOGIN: it is never a connection
-- identity, only a `SET LOCAL ROLE` target, and its name is hard-coded in
-- `otto_tenant::Db::begin`.
--
-- Roles are cluster-scoped while migrations are database-scoped, so creation is
-- idempotent and tolerant of two concurrent migrations racing to create it
-- (`#[sqlx::test]` migrates many throwaway databases in one cluster in
-- parallel, and on the shared `otto-db` instance the role is shared with
-- otto-platform, otto-factory, and otto-flags). It also tolerates
-- `insufficient_privilege`: managed Postgres does not hand the application
-- CREATEROLE. There, FORCE ROW LEVEL SECURITY below carries the isolation
-- guarantee and `Db::verify_tenant_isolation` proves it at startup, refusing
-- to serve otherwise.
DO $$
BEGIN
  BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'otto_app') THEN
      CREATE ROLE otto_app NOLOGIN;
    END IF;
  EXCEPTION
    WHEN duplicate_object OR unique_violation THEN
      NULL; -- another database in this cluster created it first
    WHEN insufficient_privilege THEN
      RAISE NOTICE
        'could not CREATE ROLE otto_app (no CREATEROLE). Tenant transactions '
        'will run as the connecting role and rely on FORCE ROW LEVEL SECURITY, '
        'which holds only while that role is neither a superuser nor '
        'BYPASSRLS. cxp-server verifies this at startup and refuses to serve '
        'otherwise.';
  END;
END $$;

-- Both are stock contrib extensions (and trusted, so a database owner can
-- create them on managed Postgres): gen_random_uuid() and customer-name
-- similarity search.
CREATE EXTENSION IF NOT EXISTS "pgcrypto";
CREATE EXTENSION IF NOT EXISTS "pg_trgm";

CREATE OR REPLACE FUNCTION current_org() RETURNS uuid AS $$
  SELECT NULLIF(current_setting('app.org_id', true), '')::uuid;
$$ LANGUAGE sql STABLE;

-- ---------------------------------------------------------------------- enums

-- Read back with `col::text`, written with `$n::enum_name`; model fields are
-- `String`.
CREATE TYPE ticket_status AS ENUM ('open', 'in_progress', 'waiting', 'resolved', 'closed');
CREATE TYPE ticket_priority AS ENUM ('low', 'medium', 'high', 'urgent');
CREATE TYPE ticket_channel AS ENUM ('email', 'chat', 'phone', 'web', 'social');
CREATE TYPE interaction_type AS ENUM (
  'page_view', 'purchase', 'support_contact', 'email_open', 'email_click', 'feature_use', 'login'
);
CREATE TYPE feedback_type AS ENUM ('bug_report', 'feature_request', 'complaint', 'praise', 'suggestion');
CREATE TYPE message_role AS ENUM ('customer', 'agent', 'system');

-- -------------------------------------------------------------------- catalog

-- The experience catalog: Experience -> Application -> Page -> Component. Each
-- level's parent is optional (an entity may be created unparented), but a
-- parent owns its children: deleting it deletes them, as the pre-platform
-- schema did. Rows elsewhere that pointed at a deleted entity keep existing
-- with that reference nulled. `team_id` is the platform team that owns the entity: an
-- opaque platform id, verified at the platform on write, with no foreign key
-- (there is no local teams table). A `team.deleted` webhook nulls it.
CREATE TABLE experiences (
  id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  org_id      uuid         NOT NULL,
  name        varchar(255) NOT NULL,
  description text,
  metadata    jsonb        NOT NULL DEFAULT '{}',
  team_id     uuid,
  created_at  timestamptz  NOT NULL DEFAULT now(),
  updated_at  timestamptz  NOT NULL DEFAULT now(),
  UNIQUE (org_id, id)
);

CREATE INDEX experiences_org_created_idx ON experiences (org_id, created_at DESC);
CREATE INDEX experiences_org_team_idx ON experiences (org_id, team_id);

CREATE TABLE applications (
  id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  org_id        uuid         NOT NULL,
  experience_id uuid,
  name          varchar(255) NOT NULL,
  description   text,
  platform      varchar(100),
  metadata      jsonb        NOT NULL DEFAULT '{}',
  team_id       uuid,
  created_at    timestamptz  NOT NULL DEFAULT now(),
  updated_at    timestamptz  NOT NULL DEFAULT now(),
  UNIQUE (org_id, id),
  FOREIGN KEY (org_id, experience_id) REFERENCES experiences (org_id, id)
    ON DELETE CASCADE
);

CREATE INDEX applications_org_created_idx ON applications (org_id, created_at DESC);
CREATE INDEX applications_org_experience_idx ON applications (org_id, experience_id);
CREATE INDEX applications_org_team_idx ON applications (org_id, team_id);

CREATE TABLE pages (
  id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  org_id         uuid         NOT NULL,
  application_id uuid,
  name           varchar(255) NOT NULL,
  path           varchar(500),
  description    text,
  metadata       jsonb        NOT NULL DEFAULT '{}',
  team_id        uuid,
  created_at     timestamptz  NOT NULL DEFAULT now(),
  updated_at     timestamptz  NOT NULL DEFAULT now(),
  UNIQUE (org_id, id),
  FOREIGN KEY (org_id, application_id) REFERENCES applications (org_id, id)
    ON DELETE CASCADE
);

CREATE INDEX pages_org_created_idx ON pages (org_id, created_at DESC);
CREATE INDEX pages_org_application_idx ON pages (org_id, application_id);
CREATE INDEX pages_org_team_idx ON pages (org_id, team_id);

CREATE TABLE components (
  id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  org_id         uuid         NOT NULL,
  page_id        uuid,
  name           varchar(255) NOT NULL,
  component_type varchar(100),
  description    text,
  metadata       jsonb        NOT NULL DEFAULT '{}',
  team_id        uuid,
  created_at     timestamptz  NOT NULL DEFAULT now(),
  updated_at     timestamptz  NOT NULL DEFAULT now(),
  UNIQUE (org_id, id),
  FOREIGN KEY (org_id, page_id) REFERENCES pages (org_id, id)
    ON DELETE CASCADE
);

CREATE INDEX components_org_created_idx ON components (org_id, created_at DESC);
CREATE INDEX components_org_page_idx ON components (org_id, page_id);
CREATE INDEX components_org_team_idx ON components (org_id, team_id);

-- ------------------------------------------------------------- customer data

-- An org's end customers. Email is unique within an org, not across orgs: two
-- organizations may well share a customer. Optionally attached to the catalog
-- entity they came through.
CREATE TABLE customers (
  id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  org_id         uuid        NOT NULL,
  email          varchar     NOT NULL,
  name           varchar,
  phone          varchar,
  company        varchar,
  metadata       jsonb       NOT NULL DEFAULT '{}',
  tags           text[]      NOT NULL DEFAULT '{}',
  experience_id  uuid,
  application_id uuid,
  page_id        uuid,
  component_id   uuid,
  created_at     timestamptz NOT NULL DEFAULT now(),
  updated_at     timestamptz NOT NULL DEFAULT now(),
  UNIQUE (org_id, id),
  UNIQUE (org_id, email),
  FOREIGN KEY (org_id, experience_id) REFERENCES experiences (org_id, id)
    ON DELETE SET NULL (experience_id),
  FOREIGN KEY (org_id, application_id) REFERENCES applications (org_id, id)
    ON DELETE SET NULL (application_id),
  FOREIGN KEY (org_id, page_id) REFERENCES pages (org_id, id)
    ON DELETE SET NULL (page_id),
  FOREIGN KEY (org_id, component_id) REFERENCES components (org_id, id)
    ON DELETE SET NULL (component_id)
);

CREATE INDEX customers_org_created_idx ON customers (org_id, created_at DESC);
CREATE INDEX customers_org_experience_idx ON customers (org_id, experience_id);
CREATE INDEX customers_org_application_idx ON customers (org_id, application_id);
CREATE INDEX customers_org_page_idx ON customers (org_id, page_id);
CREATE INDEX customers_org_component_idx ON customers (org_id, component_id);
CREATE INDEX customers_metadata_idx ON customers USING GIN (metadata);
CREATE INDEX customers_tags_idx ON customers USING GIN (tags);
-- Fuzzy name lookup (`similarity(name, $q)`, `name ILIKE`).
CREATE INDEX customers_name_trgm_idx ON customers USING GIN (name gin_trgm_ops);

-- A customer's journey: page views, purchases, logins, and so on.
CREATE TABLE interactions (
  id               uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  org_id           uuid             NOT NULL,
  customer_id      uuid             NOT NULL,
  interaction_type interaction_type NOT NULL,
  metadata         jsonb            NOT NULL DEFAULT '{}',
  created_at       timestamptz      NOT NULL DEFAULT now(),
  UNIQUE (org_id, id),
  FOREIGN KEY (org_id, customer_id) REFERENCES customers (org_id, id) ON DELETE CASCADE
);

CREATE INDEX interactions_org_created_idx ON interactions (org_id, created_at DESC);
CREATE INDEX interactions_org_customer_idx ON interactions (org_id, customer_id, created_at DESC);
CREATE INDEX interactions_org_type_idx ON interactions (org_id, interaction_type);
CREATE INDEX interactions_metadata_idx ON interactions USING GIN (metadata);

-- Customer segments; `criteria` describes membership, `segment_members` holds
-- the evaluated members.
CREATE TABLE segments (
  id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  org_id       uuid        NOT NULL,
  name         varchar     NOT NULL,
  description  text,
  criteria     jsonb       NOT NULL,
  segment_type varchar     NOT NULL DEFAULT 'dynamic',
  created_at   timestamptz NOT NULL DEFAULT now(),
  updated_at   timestamptz NOT NULL DEFAULT now(),
  UNIQUE (org_id, id)
);

CREATE INDEX segments_org_created_idx ON segments (org_id, created_at DESC);
CREATE INDEX segments_org_type_idx ON segments (org_id, segment_type);
CREATE INDEX segments_criteria_idx ON segments USING GIN (criteria);

-- A junction table: keyed by its pair, so it has no `id` of its own.
CREATE TABLE segment_members (
  org_id      uuid        NOT NULL,
  segment_id  uuid        NOT NULL,
  customer_id uuid        NOT NULL,
  added_at    timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (org_id, segment_id, customer_id),
  FOREIGN KEY (org_id, segment_id) REFERENCES segments (org_id, id) ON DELETE CASCADE,
  FOREIGN KEY (org_id, customer_id) REFERENCES customers (org_id, id) ON DELETE CASCADE
);

CREATE INDEX segment_members_org_customer_idx ON segment_members (org_id, customer_id);

-- -------------------------------------------------------------------- support

CREATE TABLE tickets (
  id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  org_id         uuid            NOT NULL,
  customer_id    uuid            NOT NULL,
  subject        varchar         NOT NULL,
  description    text,
  status         ticket_status   NOT NULL DEFAULT 'open',
  priority       ticket_priority NOT NULL DEFAULT 'medium',
  channel        ticket_channel,
  assigned_to    varchar,
  tags           text[]          NOT NULL DEFAULT '{}',
  metadata       jsonb           NOT NULL DEFAULT '{}',
  experience_id  uuid,
  application_id uuid,
  page_id        uuid,
  component_id   uuid,
  resolved_at    timestamptz,
  created_at     timestamptz     NOT NULL DEFAULT now(),
  updated_at     timestamptz     NOT NULL DEFAULT now(),
  UNIQUE (org_id, id),
  FOREIGN KEY (org_id, customer_id) REFERENCES customers (org_id, id) ON DELETE CASCADE,
  FOREIGN KEY (org_id, experience_id) REFERENCES experiences (org_id, id)
    ON DELETE SET NULL (experience_id),
  FOREIGN KEY (org_id, application_id) REFERENCES applications (org_id, id)
    ON DELETE SET NULL (application_id),
  FOREIGN KEY (org_id, page_id) REFERENCES pages (org_id, id)
    ON DELETE SET NULL (page_id),
  FOREIGN KEY (org_id, component_id) REFERENCES components (org_id, id)
    ON DELETE SET NULL (component_id)
);

CREATE INDEX tickets_org_created_idx ON tickets (org_id, created_at DESC);
CREATE INDEX tickets_org_customer_idx ON tickets (org_id, customer_id);
-- The dashboard's count-by-status aggregate and status/priority filters.
CREATE INDEX tickets_org_status_idx ON tickets (org_id, status);
CREATE INDEX tickets_org_priority_idx ON tickets (org_id, priority);
CREATE INDEX tickets_org_experience_idx ON tickets (org_id, experience_id);
CREATE INDEX tickets_org_application_idx ON tickets (org_id, application_id);
CREATE INDEX tickets_org_page_idx ON tickets (org_id, page_id);
CREATE INDEX tickets_org_component_idx ON tickets (org_id, component_id);
CREATE INDEX tickets_metadata_idx ON tickets USING GIN (metadata);
CREATE INDEX tickets_tags_idx ON tickets USING GIN (tags);

CREATE TABLE knowledge_articles (
  id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  org_id     uuid        NOT NULL,
  title      varchar     NOT NULL,
  content    text        NOT NULL,
  category   varchar,
  tags       text[]      NOT NULL DEFAULT '{}',
  metadata   jsonb       NOT NULL DEFAULT '{}',
  published  boolean     NOT NULL DEFAULT false,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (org_id, id)
);

-- Listing (newest first) and filtering lead with org_id; full-text search uses
-- the GIN expression index, whose expression search queries must repeat
-- exactly: to_tsvector('english', title || ' ' || content).
CREATE INDEX knowledge_articles_org_created_idx ON knowledge_articles (org_id, created_at DESC);
CREATE INDEX knowledge_articles_org_category_idx ON knowledge_articles (org_id, category);
CREATE INDEX knowledge_articles_org_published_idx
  ON knowledge_articles (org_id, published, created_at DESC);
CREATE INDEX knowledge_articles_fts_idx
  ON knowledge_articles USING GIN (to_tsvector('english', title || ' ' || content));
CREATE INDEX knowledge_articles_tags_idx ON knowledge_articles USING GIN (tags);
CREATE INDEX knowledge_articles_metadata_idx ON knowledge_articles USING GIN (metadata);

CREATE TABLE chat_sessions (
  id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  org_id      uuid        NOT NULL,
  customer_id uuid        NOT NULL,
  status      varchar     NOT NULL DEFAULT 'active',
  metadata    jsonb       NOT NULL DEFAULT '{}',
  started_at  timestamptz NOT NULL DEFAULT now(),
  ended_at    timestamptz,
  UNIQUE (org_id, id),
  FOREIGN KEY (org_id, customer_id) REFERENCES customers (org_id, id) ON DELETE CASCADE
);

CREATE INDEX chat_sessions_org_customer_idx ON chat_sessions (org_id, customer_id);
CREATE INDEX chat_sessions_org_started_idx ON chat_sessions (org_id, started_at DESC);
CREATE INDEX chat_sessions_metadata_idx ON chat_sessions USING GIN (metadata);

CREATE TABLE chat_messages (
  id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  org_id     uuid         NOT NULL,
  session_id uuid         NOT NULL,
  role       message_role NOT NULL,
  content    text         NOT NULL,
  metadata   jsonb        NOT NULL DEFAULT '{}',
  created_at timestamptz  NOT NULL DEFAULT now(),
  UNIQUE (org_id, id),
  FOREIGN KEY (org_id, session_id) REFERENCES chat_sessions (org_id, id) ON DELETE CASCADE
);

-- A session's transcript, oldest first.
CREATE INDEX chat_messages_org_session_idx ON chat_messages (org_id, session_id, created_at);
CREATE INDEX chat_messages_metadata_idx ON chat_messages USING GIN (metadata);

-- ------------------------------------------------------------------- feedback

CREATE TABLE feedback (
  id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  org_id         uuid          NOT NULL,
  customer_id    uuid          NOT NULL,
  feedback_type  feedback_type NOT NULL,
  subject        varchar,
  body           text,
  rating         integer       CHECK (rating >= 1 AND rating <= 5),
  metadata       jsonb         NOT NULL DEFAULT '{}',
  experience_id  uuid,
  application_id uuid,
  page_id        uuid,
  component_id   uuid,
  created_at     timestamptz   NOT NULL DEFAULT now(),
  UNIQUE (org_id, id),
  FOREIGN KEY (org_id, customer_id) REFERENCES customers (org_id, id) ON DELETE CASCADE,
  FOREIGN KEY (org_id, experience_id) REFERENCES experiences (org_id, id)
    ON DELETE SET NULL (experience_id),
  FOREIGN KEY (org_id, application_id) REFERENCES applications (org_id, id)
    ON DELETE SET NULL (application_id),
  FOREIGN KEY (org_id, page_id) REFERENCES pages (org_id, id)
    ON DELETE SET NULL (page_id),
  FOREIGN KEY (org_id, component_id) REFERENCES components (org_id, id)
    ON DELETE SET NULL (component_id)
);

CREATE INDEX feedback_org_created_idx ON feedback (org_id, created_at DESC);
CREATE INDEX feedback_org_customer_idx ON feedback (org_id, customer_id);
CREATE INDEX feedback_org_type_idx ON feedback (org_id, feedback_type);
CREATE INDEX feedback_org_experience_idx ON feedback (org_id, experience_id);
CREATE INDEX feedback_org_application_idx ON feedback (org_id, application_id);
CREATE INDEX feedback_org_page_idx ON feedback (org_id, page_id);
CREATE INDEX feedback_org_component_idx ON feedback (org_id, component_id);
CREATE INDEX feedback_metadata_idx ON feedback USING GIN (metadata);

CREATE TABLE surveys (
  id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  org_id         uuid        NOT NULL,
  title          varchar     NOT NULL,
  description    text,
  questions      jsonb       NOT NULL,
  active         boolean     NOT NULL DEFAULT true,
  experience_id  uuid,
  application_id uuid,
  page_id        uuid,
  component_id   uuid,
  created_at     timestamptz NOT NULL DEFAULT now(),
  updated_at     timestamptz NOT NULL DEFAULT now(),
  UNIQUE (org_id, id),
  FOREIGN KEY (org_id, experience_id) REFERENCES experiences (org_id, id)
    ON DELETE SET NULL (experience_id),
  FOREIGN KEY (org_id, application_id) REFERENCES applications (org_id, id)
    ON DELETE SET NULL (application_id),
  FOREIGN KEY (org_id, page_id) REFERENCES pages (org_id, id)
    ON DELETE SET NULL (page_id),
  FOREIGN KEY (org_id, component_id) REFERENCES components (org_id, id)
    ON DELETE SET NULL (component_id)
);

CREATE INDEX surveys_org_created_idx ON surveys (org_id, created_at DESC);
CREATE INDEX surveys_org_experience_idx ON surveys (org_id, experience_id);
CREATE INDEX surveys_org_application_idx ON surveys (org_id, application_id);
CREATE INDEX surveys_org_page_idx ON surveys (org_id, page_id);
CREATE INDEX surveys_org_component_idx ON surveys (org_id, component_id);

CREATE TABLE survey_responses (
  id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  org_id       uuid        NOT NULL,
  survey_id    uuid        NOT NULL,
  customer_id  uuid        NOT NULL,
  answers      jsonb       NOT NULL,
  completed_at timestamptz,
  created_at   timestamptz NOT NULL DEFAULT now(),
  UNIQUE (org_id, id),
  FOREIGN KEY (org_id, survey_id) REFERENCES surveys (org_id, id) ON DELETE CASCADE,
  FOREIGN KEY (org_id, customer_id) REFERENCES customers (org_id, id) ON DELETE CASCADE
);

CREATE INDEX survey_responses_org_survey_idx ON survey_responses (org_id, survey_id, created_at DESC);
CREATE INDEX survey_responses_org_customer_idx ON survey_responses (org_id, customer_id);

-- --------------------------------------------------------------- usage outbox

-- One row per billable tool call (submit_feedback, create_survey, send_survey,
-- submit_survey_response), written in the tool's own transaction and shipped
-- to the platform's /internal/usage by a background task. The platform dedupes
-- on event_id, so shipping is retried freely.
CREATE TABLE usage_outbox (
  id              bigserial   PRIMARY KEY,
  event_id        uuid        NOT NULL DEFAULT gen_random_uuid() UNIQUE,
  org_id          uuid        NOT NULL,
  user_id         uuid,
  tool            text        NOT NULL,
  billable        boolean     NOT NULL,
  occurred_at     timestamptz NOT NULL DEFAULT now(),
  attempts        integer     NOT NULL DEFAULT 0,
  next_attempt_at timestamptz NOT NULL DEFAULT now(),
  last_error      text
);

-- The shipper drains across every org, so its queue index does not lead with
-- org_id; the per-org index serves the usage report and the org.deleted purge.
CREATE INDEX usage_outbox_due_idx ON usage_outbox (next_attempt_at, id);
CREATE INDEX usage_outbox_org_idx ON usage_outbox (org_id);

-- Usage the platform refused outright, kept with its reason rather than dropped.
CREATE TABLE usage_outbox_rejected (
  id          bigserial   PRIMARY KEY,
  event_id    uuid        NOT NULL,
  org_id      uuid        NOT NULL,
  user_id     uuid,
  tool        text        NOT NULL,
  billable    boolean     NOT NULL,
  occurred_at timestamptz NOT NULL,
  reason      text        NOT NULL,
  rejected_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX usage_outbox_rejected_org_idx ON usage_outbox_rejected (org_id, rejected_at DESC);

-- --------------------------------------------------------- platform event log

-- Lifecycle webhook deliveries already applied, so a redelivery is a no-op.
-- Written in the same pinned transaction as the cleanup it records, and kept
-- after an org.deleted purge so a replay still finds it. Swept for retention
-- by background maintenance (see the policies below).
CREATE TABLE platform_events (
  event_id    uuid PRIMARY KEY,
  org_id      uuid        NOT NULL,
  kind        text        NOT NULL,
  received_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX platform_events_received_idx ON platform_events (received_at);

-- ----------------------------------------------------------------- tombstones

-- Orgs the platform has deleted. Checked on every authenticated request so a
-- cached introspection cannot write after the purge. Permanent; not under RLS
-- because it is consulted before any org is pinned.
CREATE TABLE deleted_orgs (
  org_id     uuid PRIMARY KEY,
  deleted_at timestamptz NOT NULL DEFAULT now()
);

-- Members the platform has just removed, refused for a few minutes until the
-- platform's own introspection stops vouching for them. Not under RLS, for the
-- same reason as deleted_orgs.
CREATE TABLE removed_members (
  org_id     uuid        NOT NULL,
  user_id    uuid        NOT NULL,
  removed_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (org_id, user_id)
);

-- --------------------------------------------------------- row-level security

-- Every domain table, plus the platform event log. Adding a tenant table means
-- adding it here (in a new migration) and to the explicit list in
-- tests/db.rs's tenant-isolation test.
DO $$
DECLARE
  t text;
  tenant_tables text[] := ARRAY[
    'experiences',
    'applications',
    'pages',
    'components',
    'customers',
    'interactions',
    'segments',
    'segment_members',
    'tickets',
    'knowledge_articles',
    'chat_sessions',
    'chat_messages',
    'feedback',
    'surveys',
    'survey_responses',
    'platform_events'
  ];
BEGIN
  FOREACH t IN ARRAY tenant_tables LOOP
    EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
    -- FORCE covers the case where the application role IS the table owner.
    EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
    EXECUTE format(
      'CREATE POLICY %I ON %I USING (org_id = current_org()) WITH CHECK (org_id = current_org())',
      t || '_tenant_isolation', t
    );
  END LOOP;
END $$;

-- The platform_events retention sweep runs unpinned, across every org, as
-- otto_app. Policies are permissive and OR together, so each of these adds
-- exactly one thing to the tenant policy above; a pinned transaction still
-- reaches only its own org's rows.
--
-- Unpinned is not enough on its own: a request path that forgot to pin is
-- unpinned too. So each unpinned grant also needs the transaction-local marker
-- app.maintenance = 'on', which only cxp-core's background maintenance helper
-- (`maintenance::begin`) sets.
--
-- The DELETE policy grants the unpinned delete. Postgres also applies a
-- table's SELECT policies to a DELETE whose WHERE reads columns, and the sweep
-- has such a WHERE (received_at), so the read policy is needed too or the
-- sweep matches nothing under otto_app.
CREATE POLICY platform_events_retention ON platform_events
  FOR DELETE USING (current_org() IS NULL);
CREATE POLICY platform_events_retention_read ON platform_events
  FOR SELECT USING (current_org() IS NULL
                    AND current_setting('app.maintenance', true) = 'on');
-- A DELETE with no WHERE reads no columns and so needs no SELECT policy: the
-- permissive DELETE policy alone would let an unmarked, unpinned
-- `DELETE FROM platform_events` empty the table across orgs. This RESTRICTIVE
-- policy ANDs with the permissive ones, so an unpinned DELETE also needs the
-- maintenance marker, WHERE clause or not. Pinned deletes (the org.deleted
-- purge) are unaffected: current_org() is set.
CREATE POLICY platform_events_maintenance_delete ON platform_events
  AS RESTRICTIVE FOR DELETE
  USING (current_org() IS NOT NULL OR current_setting('app.maintenance', true) = 'on');

-- Written by pinned transactions, drained by a background shipper that serves
-- every org. A pinned transaction sees and writes only its own org's rows; any
-- unpinned transaction (current_org() IS NULL) can read and write every org's
-- rows. No marker guards this: request paths always pin (they open their
-- transactions with `platform_events::begin_live`), and only the usage
-- shipper runs unpinned against these tables.
ALTER TABLE usage_outbox ENABLE ROW LEVEL SECURITY;
ALTER TABLE usage_outbox FORCE ROW LEVEL SECURITY;
CREATE POLICY usage_outbox_tenant_isolation ON usage_outbox
  USING (org_id = current_org() OR current_org() IS NULL)
  WITH CHECK (org_id = current_org() OR current_org() IS NULL);

ALTER TABLE usage_outbox_rejected ENABLE ROW LEVEL SECURITY;
ALTER TABLE usage_outbox_rejected FORCE ROW LEVEL SECURITY;
CREATE POLICY usage_outbox_rejected_tenant_isolation ON usage_outbox_rejected
  USING (org_id = current_org() OR current_org() IS NULL)
  WITH CHECK (org_id = current_org() OR current_org() IS NULL);

-- ---------------------------------------------------------------------- grants

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'otto_app') THEN
    -- `SET LOCAL ROLE otto_app` requires the connecting role to be a member.
    BEGIN
      EXECUTE 'GRANT otto_app TO CURRENT_USER';
    EXCEPTION WHEN insufficient_privilege THEN
      RAISE NOTICE
        'otto_app exists but could not be granted to the migrating role. Tenant '
        'transactions will fall back to FORCE ROW LEVEL SECURITY; cxp-server '
        'verifies that at startup.';
    END;

    EXECUTE 'GRANT USAGE ON SCHEMA public TO otto_app';
    EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO otto_app';
    EXECUTE 'GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO otto_app';
    EXECUTE 'GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA public TO otto_app';
    EXECUTE 'ALTER DEFAULT PRIVILEGES IN SCHEMA public '
            'GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO otto_app';
    EXECUTE 'ALTER DEFAULT PRIVILEGES IN SCHEMA public '
            'GRANT USAGE, SELECT ON SEQUENCES TO otto_app';
  END IF;
END $$;
