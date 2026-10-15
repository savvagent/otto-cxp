-- Benchmark data for search under RLS (#17): one org of 300k customers and
-- 300k knowledge-base articles (90% published), plus three smaller orgs so
-- row-level security has other orgs' rows to filter. Deterministic (setseed).
--
--   createdb -h localhost -p <port> -U cxp bench
--   # apply crates/cxp-core/migrations in order (or boot cxp-server once against it)
--   psql postgres://cxp:cxp@localhost:<port>/bench -f scripts/bench-search/seed.sql \
--     < /usr/share/dict/words
--
-- LOCAL POSTGRES ONLY, as a superuser (it disables triggers and bypasses
-- RLS to load fast). About 15 minutes and 15 GB on a laptop. Article text is
-- drawn Zipf-like from the lowercase words read on standard input (any word
-- list, one word per line); customer names from common first and last names plus
-- invented surnames. The search index tables are filled in bulk at the end,
-- from migration 0004's own gram functions (the triggers write the same rows
-- a row at a time). A bulk load packs the btrees about 25-30% denser than
-- inserts one at a time do, so production's index will be that much larger
-- per row than this database's.
\set ON_ERROR_STOP 1
SELECT setseed(0.17);
SET maintenance_work_mem = '1GB';
SET max_parallel_maintenance_workers = 0;
ALTER TABLE customers DISABLE TRIGGER USER;
ALTER TABLE knowledge_articles DISABLE TRIGGER USER;

CREATE TEMP TABLE first_names AS SELECT unnest(ARRAY[
 'James','Mary','Robert','Patricia','John','Jennifer','Michael','Linda','David','Elizabeth',
 'William','Barbara','Richard','Susan','Joseph','Jessica','Thomas','Sarah','Charles','Karen',
 'Christopher','Lisa','Daniel','Nancy','Matthew','Betty','Anthony','Margaret','Mark','Sandra',
 'Donald','Ashley','Steven','Kimberly','Paul','Emily','Andrew','Donna','Joshua','Michelle',
 'Kenneth','Carol','Kevin','Amanda','Brian','Dorothy','George','Melissa','Timothy','Deborah',
 'Ronald','Stephanie','Edward','Rebecca','Jason','Sharon','Jeffrey','Laura','Ryan','Cynthia',
 'Jacob','Kathleen','Gary','Amy','Nicholas','Angela','Eric','Shirley','Jonathan','Anna',
 'Stephen','Brenda','Larry','Pamela','Justin','Emma','Scott','Nicole','Brandon','Helen',
 'Benjamin','Samantha','Samuel','Katherine','Gregory','Christine','Alexander','Debra','Frank','Rachel',
 'Patrick','Carolyn','Raymond','Janet','Jack','Catherine','Dennis','Maria','Jerry','Heather',
 'Grace','Ada','Alan','Linus','Yuki','Wei','Priya','Aarav','Fatima','Omar','Sofia','Mateo',
 'Chloe','Lucas','Mia','Noah','Zoe','Liam','Ava','Ethan','Isla','Leo','Nora','Hugo','Ines'
]) AS n;

CREATE TEMP TABLE last_names AS SELECT unnest(ARRAY[
 'Smith','Johnson','Williams','Brown','Jones','Garcia','Miller','Davis','Rodriguez','Martinez',
 'Hernandez','Lopez','Gonzalez','Wilson','Anderson','Thomas','Taylor','Moore','Jackson','Martin',
 'Lee','Perez','Thompson','White','Harris','Sanchez','Clark','Ramirez','Lewis','Robinson',
 'Walker','Young','Allen','King','Wright','Scott','Torres','Nguyen','Hill','Flores',
 'Green','Adams','Nelson','Baker','Hall','Rivera','Campbell','Mitchell','Carter','Roberts',
 'Gomez','Phillips','Evans','Turner','Diaz','Parker','Cruz','Edwards','Collins','Reyes',
 'Stewart','Morris','Morales','Murphy','Cook','Rogers','Gutierrez','Ortiz','Morgan','Cooper',
 'Peterson','Bailey','Reed','Kelly','Howard','Ramos','Kim','Cox','Ward','Richardson',
 'Watson','Brooks','Chavez','Wood','James','Bennett','Gray','Mendoza','Ruiz','Hughes',
 'Price','Alvarez','Castillo','Sanders','Patel','Myers','Long','Ross','Foster','Jimenez',
 'Hopper','Lovelace','Turing','Torvalds','Tanaka','Zhang','Sharma','Kowalski','Novak','Muller'
]) AS n;

CREATE TEMP TABLE syl AS SELECT unnest(ARRAY[
 'ka','ro','mi','sen','dor','lia','tan','ber','gus','vin','pel','qua','zor','fen','lim','bra',
 'stra','nok','wel','hart','son','berg','ley','ford','ton','ski','ova','ez','ini','ard'
]) AS s;

CREATE TEMP TABLE domains AS SELECT unnest(ARRAY[
 'gmail.com','yahoo.com','outlook.com','hotmail.com','icloud.com','proton.me','aol.com',
 'acme.example','globex.example','initech.example','umbrella.example','navy.example',
 'bletchley.example','contoso.com','fabrikam.com','northwind.example','example.org'
]) AS d;

CREATE TEMP TABLE fa AS SELECT array_agg(n) a FROM first_names;
CREATE TEMP TABLE la AS SELECT array_agg(n) a FROM last_names;
CREATE TEMP TABLE sa AS SELECT array_agg(s) a FROM syl;
CREATE TEMP TABLE da AS SELECT array_agg(d) a FROM domains;

CREATE TEMP TABLE orgs (org uuid, n int, articles int);
INSERT INTO orgs VALUES
  ('00000000-0000-0000-0000-00000000000a', 300000, 300000),
  ('00000000-0000-0000-0000-00000000000b', 50000, 20000),
  ('00000000-0000-0000-0000-00000000000c', 50000, 20000),
  ('00000000-0000-0000-0000-00000000000d', 20000, 5000);

-- Customers: a last name from the list for 60%, an invented one (2-3
-- syllables) for the rest, so name trigrams have a long tail.
INSERT INTO customers (org_id, email, name, phone, company, created_at)
SELECT o.org,
       lower(fn) || '.' || lower(ln) || g || '@' || dom,
       fn || ' ' || ln,
       '+1555' || lpad(g::text, 7, '0'),
       CASE WHEN g % 3 = 0 THEN initcap(dom) END,
       now() - make_interval(secs => g * 37)
FROM orgs o
CROSS JOIN LATERAL generate_series(1, o.n) g
CROSS JOIN LATERAL (
  SELECT (SELECT a FROM fa)[1 + floor(random() * (SELECT cardinality(a) FROM fa))::int] AS fn,
         CASE WHEN random() < 0.6
              THEN (SELECT a FROM la)[1 + floor(random() * (SELECT cardinality(a) FROM la))::int]
              ELSE initcap((SELECT a FROM sa)[1 + floor(random() * 30)::int]
                           || (SELECT a FROM sa)[1 + floor(random() * 30)::int]
                           || CASE WHEN random() < 0.5 THEN (SELECT a FROM sa)[1 + floor(random() * 30)::int] ELSE '' END)
         END AS ln,
         (SELECT a FROM da)[1 + floor(random() * (SELECT cardinality(a) FROM da))::int] AS dom
  WHERE g IS NOT NULL
) x;


-- Knowledge base: a 40k-word vocabulary in a fixed random order, with a few
-- support words placed among the common ranks.
CREATE TEMP TABLE words (w text);
\copy words FROM pstdin
CREATE TEMP TABLE vocab AS
SELECT (row_number() OVER (ORDER BY k) - 1)::int AS i, w FROM (
  SELECT row_number() OVER (ORDER BY md5(w))::numeric AS k, w FROM (
    SELECT DISTINCT w FROM words WHERE w ~ '^[a-z]{3,12}$'
      AND w NOT IN ('password','refund','billing','shipping','account','invoice','login',
                    'security','delivery','subscription','cancel','order','payment','reset',
                    'upgrade','export','import','integration','webhook','permissions')
    ORDER BY w LIMIT 40000) AS d
  UNION ALL
  SELECT 40.5 + (o - 1) * 15, s FROM unnest(ARRAY['password','refund','billing','shipping',
    'account','invoice','login','security','delivery','subscription','cancel','order','payment',
    'reset','upgrade','export','import','integration','webhook','permissions'])
    WITH ORDINALITY AS u(s, o)) AS v;
CREATE UNIQUE INDEX ON vocab (i);
ANALYZE vocab;
SELECT max(i) + 1 AS n FROM vocab \gset

-- Word ranks are log-uniform over 1..n (Zipf, s = 1).
INSERT INTO knowledge_articles (org_id, title, content, category, published, created_at)
SELECT o.org,
       initcap((SELECT string_agg((SELECT v.w FROM vocab v WHERE v.i = r.k), ' ')
                FROM (SELECT floor(power(:n, random()))::int - 1 + 0 * g AS k
                      FROM generate_series(1, 5)) r)),
       (SELECT string_agg((SELECT v.w FROM vocab v WHERE v.i = r.k), ' ')
                FROM (SELECT floor(power(:n, random()))::int - 1 + 0 * g AS k
                      FROM generate_series(1, 150)) r),
       'general', g % 10 <> 0, now() - make_interval(secs => g * 61)
FROM orgs o CROSS JOIN LATERAL generate_series(1, o.articles) g;

-- The search index, in bulk: keys dropped, rows inserted, keys rebuilt, and
-- every row marked indexed (what cxp_core's per-org backlog indexing does a
-- batch at a time, through the triggers).
ALTER TABLE customers DISABLE TRIGGER USER;
ALTER TABLE knowledge_articles DISABLE TRIGGER USER;
ALTER TABLE customer_search_grams DISABLE TRIGGER ALL;
ALTER TABLE knowledge_search_terms DISABLE TRIGGER ALL;
ALTER TABLE customer_search_grams DROP CONSTRAINT customer_search_grams_pkey;
DROP INDEX customer_search_grams_org_customer_idx;
ALTER TABLE knowledge_search_terms DROP CONSTRAINT knowledge_search_terms_pkey;
DROP INDEX knowledge_search_terms_org_article_idx;
DROP INDEX knowledge_search_terms_frequent_idx;
TRUNCATE customer_search_grams, knowledge_search_terms;
INSERT INTO customer_search_grams (org_id, customer_id, gram, name_grams)
SELECT c.org_id, c.id, g, least(cardinality(show_trgm(coalesce(c.name, ''))), 32767)
FROM customers AS c, customer_search_grams_of(c.email, c.name) AS g;
INSERT INTO knowledge_search_terms (org_id, article_id, term, tf)
SELECT a.org_id, a.id, t.term, t.tf
FROM knowledge_articles AS a, knowledge_search_terms_of(a.title, a.search_tsv) AS t WHERE a.published;
ALTER TABLE customer_search_grams ADD PRIMARY KEY (org_id, gram, customer_id) INCLUDE (name_grams);
CREATE INDEX customer_search_grams_org_customer_idx ON customer_search_grams (org_id, customer_id);
ALTER TABLE knowledge_search_terms ADD PRIMARY KEY (org_id, term, article_id) INCLUDE (tf);
CREATE INDEX knowledge_search_terms_org_article_idx ON knowledge_search_terms (org_id, article_id);
CREATE INDEX knowledge_search_terms_frequent_idx
  ON knowledge_search_terms (org_id, term, tf DESC, article_id) WHERE tf >= 2;
UPDATE customers SET search_indexed = true;
UPDATE knowledge_articles SET search_indexed = true;
ALTER TABLE customers ENABLE TRIGGER USER;
ALTER TABLE knowledge_articles ENABLE TRIGGER USER;
ALTER TABLE customer_search_grams ENABLE TRIGGER ALL;
ALTER TABLE knowledge_search_terms ENABLE TRIGGER ALL;
VACUUM (FULL, ANALYZE) customers;
VACUUM (FULL, ANALYZE) knowledge_articles;
VACUUM ANALYZE customer_search_grams;
VACUUM ANALYZE knowledge_search_terms;
SELECT relname, n_live_tup, pg_size_pretty(pg_total_relation_size(relid)) AS size
FROM pg_stat_user_tables
WHERE relname IN ('customers', 'knowledge_articles', 'customer_search_grams', 'knowledge_search_terms')
ORDER BY 1;
