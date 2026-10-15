-- Search that stays fast under row-level security (#17).
--
-- Under FORCE ROW LEVEL SECURITY the planner only uses LEAKPROOF operators as
-- index conditions ahead of the tenant policy. ILIKE, pg_trgm's `%`, and
-- full-text `@@` are not leakproof, so a trigram or full-text GIN index can
-- never serve a tenant query, and search_customers / search_articles read
-- every row of the caller's org (about 0.7 s for customers and several
-- seconds for the knowledge base at 300k rows; see
-- docs/specs/2026-10-14-search-under-rls-design.md). Marking those operators
-- leakproof needs a superuser; this migration needs none.
--
-- Instead each searchable row gets an inverted index of its own: one row per
-- (entity, gram) in a tenant table keyed (org_id, gram, entity id). Search
-- looks grams up with text equality (and, for queries under four characters,
-- a prefix range), all leakproof btree operators, so they are index
-- conditions under RLS. The grams only narrow the candidates; search still
-- applies its original ILIKE / `%` / `@@` predicate to them, so results are
-- exactly what the old full scan returned.
--
-- Grams, prefixed by kind (column collation "C", so a prefix is a byte range):
-- - `s` + a 4-character window of a lowercased string padded with three
--   U+0001, so every position starts one: `x ILIKE '%q%'` holds exactly
--   when lower(x) contains lower(q) (that is how Postgres evaluates ILIKE on
--   a multibyte database), so every 4-character window of lower(q) is a
--   stored window of a match, and a shorter lower(q) is a prefix of one.
--   Four characters rather than pg_trgm's three: still one row per position,
--   but far more selective, where three-character windows of common
--   syllables match too much of an org to narrow anything.
-- - `t` + a pg_trgm trigram of the customer's name, as show_trgm() prints it
--   (the same set similarity() compares): fuzzy candidates are counted from
--   these. `name_grams` carries the name's trigram count for that bound.
-- - `l` + a lexeme of the article's stored `search_tsv`.
--
-- Triggers keep the tables in step with every write, so the previous release
-- (which knows nothing of them) keeps them correct too; deleting a customer
-- or article cascades. The cost is storage: roughly 50 rows per customer and
-- (lexemes + title length) rows per article; the design doc has the numbers.
-- No GIN index is created: tests/db.rs still forbids them.

-- 4-character windows of `s`, in order (a string shorter than four
-- characters has none).
CREATE FUNCTION search_windows(s text) RETURNS SETOF text
  LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE AS $$
  SELECT substr(s, i, 4) FROM generate_series(1, length(s) - 3) AS i
$$;

-- Every gram a customer is found by.
CREATE FUNCTION customer_search_grams_of(email text, name text) RETURNS SETOF text
  LANGUAGE sql IMMUTABLE PARALLEL SAFE SET search_path = pg_catalog, public AS $$
  SELECT 's' || w FROM search_windows(lower(email) || E'\x01\x01\x01') AS w
  UNION
  SELECT 's' || w FROM search_windows(lower(name) || E'\x01\x01\x01') AS w
  UNION
  SELECT 't' || t FROM unnest(show_trgm(coalesce(name, ''))) AS t
$$;

-- Every term an article is found by.
CREATE FUNCTION knowledge_search_terms_of(title text, search_tsv tsvector) RETURNS SETOF text
  LANGUAGE sql IMMUTABLE PARALLEL SAFE SET search_path = pg_catalog, public AS $$
  SELECT 'l' || l FROM unnest(tsvector_to_array(search_tsv)) AS l
  UNION
  SELECT 's' || w FROM search_windows(lower(title) || E'\x01\x01\x01') AS w
$$;

CREATE TABLE customer_search_grams (
  org_id      uuid     NOT NULL,
  customer_id uuid     NOT NULL,
  gram        text     COLLATE "C" NOT NULL,
  -- cardinality(show_trgm(name)): the fuzzy bound needs it per candidate.
  name_grams  smallint NOT NULL,
  -- Lookup by gram; name_grams rides along so the lookup is index-only.
  PRIMARY KEY (org_id, gram, customer_id) INCLUDE (name_grams),
  FOREIGN KEY (org_id, customer_id) REFERENCES customers (org_id, id) ON DELETE CASCADE
);
-- The trigger's delete, the cascade, and the org.deleted purge.
CREATE INDEX customer_search_grams_org_customer_idx ON customer_search_grams (org_id, customer_id);

CREATE TABLE knowledge_search_terms (
  org_id     uuid NOT NULL,
  article_id uuid NOT NULL,
  term       text COLLATE "C" NOT NULL,
  PRIMARY KEY (org_id, term, article_id),
  FOREIGN KEY (org_id, article_id) REFERENCES knowledge_articles (org_id, id) ON DELETE CASCADE
);
CREATE INDEX knowledge_search_terms_org_article_idx ON knowledge_search_terms (org_id, article_id);

-- Tenant tables like every other (`Db::verify_tenant_isolation` finds them by
-- the policy name). The triggers below run as the writing role, so a pinned
-- otto_app transaction writes its own org's grams and no other.
DO $$
DECLARE
  t text;
BEGIN
  FOREACH t IN ARRAY ARRAY['customer_search_grams', 'knowledge_search_terms'] LOOP
    EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
    EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
    EXECUTE format(
      'CREATE POLICY %I ON %I USING (org_id = current_org()) WITH CHECK (org_id = current_org())',
      t || '_tenant_isolation', t
    );
  END LOOP;
END $$;

-- Rebuild a row's grams when it is created or its searchable text changes.
-- SECURITY INVOKER (the default): it reads and writes under the caller's RLS.
CREATE FUNCTION customer_search_grams_sync() RETURNS trigger
  LANGUAGE plpgsql SET search_path = pg_catalog, public AS $$
BEGIN
  IF TG_OP = 'UPDATE' THEN
    DELETE FROM public.customer_search_grams
    WHERE org_id = OLD.org_id AND customer_id = OLD.id;
  END IF;
  INSERT INTO public.customer_search_grams (org_id, customer_id, gram, name_grams)
  SELECT NEW.org_id, NEW.id, g, cardinality(show_trgm(coalesce(NEW.name, '')))
  FROM public.customer_search_grams_of(NEW.email, NEW.name) AS g;
  RETURN NULL;
END $$;

CREATE FUNCTION knowledge_search_terms_sync() RETURNS trigger
  LANGUAGE plpgsql SET search_path = pg_catalog, public AS $$
BEGIN
  IF TG_OP = 'UPDATE' THEN
    DELETE FROM public.knowledge_search_terms
    WHERE org_id = OLD.org_id AND article_id = OLD.id;
  END IF;
  INSERT INTO public.knowledge_search_terms (org_id, article_id, term)
  SELECT NEW.org_id, NEW.id, t
  FROM public.knowledge_search_terms_of(NEW.title, NEW.search_tsv) AS t;
  RETURN NULL;
END $$;

-- update_customer and update_article name every column in their SET lists,
-- so the WHEN clauses, not the column lists, skip rewrites that change nothing.
CREATE TRIGGER customers_search_grams_insert
  AFTER INSERT ON customers
  FOR EACH ROW EXECUTE FUNCTION customer_search_grams_sync();
CREATE TRIGGER customers_search_grams_update
  AFTER UPDATE OF email, name ON customers
  FOR EACH ROW
  WHEN (OLD.email IS DISTINCT FROM NEW.email OR OLD.name IS DISTINCT FROM NEW.name)
  EXECUTE FUNCTION customer_search_grams_sync();
CREATE TRIGGER knowledge_articles_search_terms_insert
  AFTER INSERT ON knowledge_articles
  FOR EACH ROW EXECUTE FUNCTION knowledge_search_terms_sync();
CREATE TRIGGER knowledge_articles_search_terms_update
  AFTER UPDATE OF title, content ON knowledge_articles
  FOR EACH ROW
  WHEN (OLD.title IS DISTINCT FROM NEW.title OR OLD.content IS DISTINCT FROM NEW.content)
  EXECUTE FUNCTION knowledge_search_terms_sync();

-- Index the rows that already exist. This reads every org's customers and
-- articles, so it needs a role that row-level security does not bind (a
-- superuser, as production's connecting role is today, or BYPASSRLS). Under
-- any other role it would see no rows and silently leave search missing every
-- existing customer, so it refuses instead when there is anything to index.
DO $$
BEGIN
  IF NOT (SELECT rolsuper OR rolbypassrls FROM pg_roles WHERE rolname = current_user)
     AND (pg_relation_size('customers') > 0 OR pg_relation_size('knowledge_articles') > 0)
  THEN
    RAISE EXCEPTION
      'migration 0004 must run as a superuser or BYPASSRLS role to index existing '
      'customers and articles; % is subject to row-level security', current_user;
  END IF;
END $$;

INSERT INTO customer_search_grams (org_id, customer_id, gram, name_grams)
SELECT c.org_id, c.id, g, cardinality(show_trgm(coalesce(c.name, '')))
FROM customers AS c, customer_search_grams_of(c.email, c.name) AS g;

INSERT INTO knowledge_search_terms (org_id, article_id, term)
SELECT a.org_id, a.id, t
FROM knowledge_articles AS a, knowledge_search_terms_of(a.title, a.search_tsv) AS t;

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'otto_app') THEN
    EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON customer_search_grams, '
            'knowledge_search_terms TO otto_app';
  END IF;
END $$;
