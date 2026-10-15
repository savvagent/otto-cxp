-- search_articles before #17 (the full-org scan), as a request runs it.
BEGIN;
SET LOCAL ROLE otto_app;
SELECT set_config('app.org_id', :org, true);
SELECT id, title, content, category, tags, metadata, published, created_at, updated_at FROM knowledge_articles WHERE org_id = :org::uuid AND published = true AND (search_tsv @@ plainto_tsquery('english', :q) OR title ILIKE :pat) ORDER BY ts_rank(search_tsv, plainto_tsquery('english', :q)) DESC, created_at DESC, id LIMIT 20;
COMMIT;
