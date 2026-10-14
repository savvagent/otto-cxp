-- How many times shipping a usage row failed for a reason about the row itself
-- (a receipt that does not account for it, or a refusal of it alone), as
-- opposed to `attempts`, which counts every try. Only these count toward
-- `usage::MAX_ROW_FAILURES`: a row waiting out a platform outage, a refused
-- credential, or rate limiting is kept and retried with backoff however long
-- that takes, and still counts toward its period's quota.
ALTER TABLE usage_outbox ADD COLUMN row_failures integer NOT NULL DEFAULT 0;
