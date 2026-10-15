-- search_customers before #17 (the full-org scan), as a request runs it.
BEGIN;
SET LOCAL ROLE otto_app;
SELECT set_config('app.org_id', :org, true);
SELECT id, email, name, phone, company, metadata, tags, experience_id, application_id, page_id, component_id, created_at, updated_at FROM customers WHERE org_id = :org::uuid AND (email ILIKE :pat OR name ILIKE :pat OR name % :q) ORDER BY CASE WHEN email ILIKE :pat THEN 0 ELSE 1 END, similarity(name, :q) DESC NULLS LAST, created_at DESC, id LIMIT 50 OFFSET 0;
COMMIT;
