-- Search that stays fast under row-level security (#17).
--
-- Under FORCE ROW LEVEL SECURITY the planner only uses LEAKPROOF operators as
-- index conditions ahead of the tenant policy. ILIKE, pg_trgm's `%`, and
-- full-text `@@` are not leakproof, so a trigram or full-text GIN index can
-- never serve a tenant query, and search_customers / search_articles read
-- every row of the caller's org (one to three seconds at 300k rows; see
-- docs/specs/2026-10-14-search-under-rls-design.md). Marking those operators
-- leakproof needs a superuser; this migration needs none, and neither does
-- anything it leaves to do.
--
-- Instead each searchable row gets an inverted index of its own: one row per
-- (entity, gram) in a tenant table keyed (org_id, gram, entity id). Search
-- looks grams up with text equality and prefix ranges, leakproof btree
-- operators, so they are index conditions under RLS. Search reads a bounded
-- number of these rows, keeps its original ILIKE / `%` / `@@` predicate as
-- the judge of every result, and counts matches only up to a cap
-- (`cxp_core::search`).
--
-- Grams, prefixed by kind (column collation "C", so a prefix is a byte range):
-- - `s` + a 4-character window of a lowercased string padded with three
--   U+0001, so every position starts one: `x ILIKE '%q%'` holds exactly
--   when lower(x) contains lower(q) (that is how Postgres evaluates ILIKE on
--   a multibyte database), so every 4-character window of lower(q) is a
--   stored window of a match, and a shorter lower(q) is a prefix of one.
-- - `t` + a pg_trgm trigram of the customer's name, as show_trgm() prints it
--   (the same set similarity() compares). `name_grams` carries the name's
--   trigram count, which bounds how many trigrams a fuzzy match shares.
-- - `l` + a lexeme of a published article's stored `search_tsv`, with `tf`,
--   how many positions it has there: for a one-word query, ts_rank is a
--   strictly increasing function of it.
--
-- Triggers keep the tables in step with every write, so the previous release
-- (which knows nothing of them) keeps them correct too; deleting a customer
-- or article cascades.
--
-- No backfill runs here. Under #16 the migrating role is bound by row-level
-- security and sees no tenant rows, so it could not read them anyway. Instead
-- `search_indexed` marks what the index covers: ADD COLUMN ... DEFAULT false
-- gives every row that exists now `false` without reading or rewriting one
-- (a catalog default), and every row written afterwards is indexed and
-- `true`. Search reads the not-yet-indexed rows of the caller's org directly,
-- through a partial index on them, and indexes them in small batches in the
-- caller's own pinned transaction (`cxp_core::search::index_backlog`).

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

-- Every term a published article is found by, with its lexemes' counts.
CREATE FUNCTION knowledge_search_terms_of(title text, search_tsv tsvector)
  RETURNS TABLE (term text, tf smallint)
  LANGUAGE sql IMMUTABLE PARALLEL SAFE SET search_path = pg_catalog, public AS $$
  SELECT 'l' || u.lexeme, coalesce(cardinality(u.positions), 1)::smallint
  FROM unnest(search_tsv) AS u
  UNION
  SELECT 's' || w, NULL FROM search_windows(lower(title) || E'\x01\x01\x01') AS w
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
  org_id     uuid     NOT NULL,
  article_id uuid     NOT NULL,
  term       text     COLLATE "C" NOT NULL,
  -- Positions of an `l` lexeme in search_tsv; NULL for an `s` window.
  tf         smallint,
  PRIMARY KEY (org_id, term, article_id) INCLUDE (tf),
  FOREIGN KEY (org_id, article_id) REFERENCES knowledge_articles (org_id, id) ON DELETE CASCADE
);
CREATE INDEX knowledge_search_terms_org_article_idx ON knowledge_search_terms (org_id, article_id);
-- A lexeme's articles that use it more than once, most first: where a
-- one-word query with more matches than search reads finds its best ones
-- (ts_rank orders them by tf). About one lexeme row in nine.
CREATE INDEX knowledge_search_terms_frequent_idx
  ON knowledge_search_terms (org_id, term, tf DESC, article_id) WHERE tf >= 2;

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

-- What the index covers. Rows that exist now read `false` (a catalog default:
-- nothing is read or rewritten); from here on every row is written `true`
-- and indexed by the triggers below. The partial indexes find an org's
-- remaining backlog without reading the rest of it, and are empty once it is
-- indexed.
ALTER TABLE customers ADD COLUMN search_indexed boolean NOT NULL DEFAULT false;
ALTER TABLE customers ALTER COLUMN search_indexed SET DEFAULT true;
CREATE INDEX customers_search_backlog_idx ON customers (org_id, id) WHERE NOT search_indexed;
ALTER TABLE knowledge_articles ADD COLUMN search_indexed boolean NOT NULL DEFAULT false;
ALTER TABLE knowledge_articles ALTER COLUMN search_indexed SET DEFAULT true;
CREATE INDEX knowledge_articles_search_backlog_idx
  ON knowledge_articles (org_id, id) WHERE NOT search_indexed;

-- Any write to a row leaves it indexed: setting the flag here is what lets an
-- ordinary update (or `index_backlog`'s `SET search_indexed = true`) index a
-- row that predates this migration.
CREATE FUNCTION search_mark_indexed() RETURNS trigger
  LANGUAGE plpgsql SET search_path = pg_catalog, public AS $$
BEGIN
  NEW.search_indexed := true;
  RETURN NEW;
END $$;

-- Rebuild a row's grams when it is created, its searchable text changes, or
-- it is first indexed. SECURITY INVOKER (the default): it reads and writes
-- under the caller's RLS.
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

-- An article is indexed only while it is published: search only ever
-- returns published articles, so its counts can come from the index alone.
CREATE FUNCTION knowledge_search_terms_sync() RETURNS trigger
  LANGUAGE plpgsql SET search_path = pg_catalog, public AS $$
BEGIN
  IF TG_OP = 'UPDATE' THEN
    DELETE FROM public.knowledge_search_terms
    WHERE org_id = OLD.org_id AND article_id = OLD.id;
  END IF;
  IF NEW.published THEN
    INSERT INTO public.knowledge_search_terms (org_id, article_id, term, tf)
    SELECT NEW.org_id, NEW.id, t.term, t.tf
    FROM public.knowledge_search_terms_of(NEW.title, NEW.search_tsv) AS t;
  END IF;
  RETURN NULL;
END $$;

CREATE TRIGGER customers_search_mark
  BEFORE INSERT OR UPDATE ON customers
  FOR EACH ROW EXECUTE FUNCTION search_mark_indexed();
CREATE TRIGGER knowledge_articles_search_mark
  BEFORE INSERT OR UPDATE ON knowledge_articles
  FOR EACH ROW EXECUTE FUNCTION search_mark_indexed();

-- update_customer and update_article name every column in their SET lists,
-- so the WHEN clauses, not column lists, skip rewrites that change nothing.
-- (No column list at all: a row first indexed by an unrelated update must be
-- caught too, and `UPDATE OF` sees only the SET list, not the BEFORE
-- trigger's change.)
CREATE TRIGGER customers_search_grams_insert
  AFTER INSERT ON customers
  FOR EACH ROW EXECUTE FUNCTION customer_search_grams_sync();
CREATE TRIGGER customers_search_grams_update
  AFTER UPDATE ON customers
  FOR EACH ROW
  WHEN (OLD.email IS DISTINCT FROM NEW.email OR OLD.name IS DISTINCT FROM NEW.name
        OR NOT OLD.search_indexed)
  EXECUTE FUNCTION customer_search_grams_sync();
CREATE TRIGGER knowledge_articles_search_terms_insert
  AFTER INSERT ON knowledge_articles
  FOR EACH ROW EXECUTE FUNCTION knowledge_search_terms_sync();
CREATE TRIGGER knowledge_articles_search_terms_update
  AFTER UPDATE ON knowledge_articles
  FOR EACH ROW
  WHEN (OLD.title IS DISTINCT FROM NEW.title OR OLD.content IS DISTINCT FROM NEW.content
        OR OLD.published IS DISTINCT FROM NEW.published OR NOT OLD.search_indexed)
  EXECUTE FUNCTION knowledge_search_terms_sync();

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'otto_app') THEN
    EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON customer_search_grams, '
            'knowledge_search_terms TO otto_app';
  END IF;
END $$;
