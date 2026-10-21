-- Meter surveys sent and responses collected (#52): outbox meter columns.
--
-- usage_outbox and usage_outbox_rejected gain meter (text) and quantity
-- (bigint), so an event can carry a metered amount (send_survey bills
-- otto-cxp/surveys_sent; submit_feedback and submit_survey_response bill
-- otto-cxp/responses_collected) instead of one billable tool call. A CHECK on
-- each keeps the pair together: both NULL for an unmetered row, or both set
-- with quantity > 0.
--
-- Additive and rollback-compatible. The previous release writes outbox rows
-- with meter and quantity NULL, which the CHECKs allow. Its shipper does not
-- read the new columns, so it ships every row unmetered during an overlap.
--
-- No policy or grant changes: the table-level grants and row-level security
-- policies from 0001 cover the new columns.
--
-- The CHECKs are NOT VALID: every existing row has both columns NULL (they
-- were just added), so there is nothing to validate, and skipping the scan
-- keeps tool calls from queueing behind it while the ACCESS EXCLUSIVE lock is
-- held. New and updated rows are checked as usual.

-- Fail fast rather than queue behind a long transaction. sqlx runs each
-- migration in its own transaction, so SET LOCAL ends with it.
SET LOCAL lock_timeout = '3s';

-- usage_outbox first: the shipper's move to rejected deletes from usage_outbox,
-- then inserts into usage_outbox_rejected (usage.rs MOVE_TO_REJECTED), so take
-- the locks in that order.
ALTER TABLE usage_outbox
  ADD COLUMN meter text,
  ADD COLUMN quantity bigint,
  ADD CONSTRAINT usage_outbox_meter_quantity
    CHECK ((meter IS NULL) = (quantity IS NULL) AND (quantity IS NULL OR quantity > 0)) NOT VALID;
ALTER TABLE usage_outbox_rejected
  ADD COLUMN meter text,
  ADD COLUMN quantity bigint,
  ADD CONSTRAINT usage_outbox_rejected_meter_quantity
    CHECK ((meter IS NULL) = (quantity IS NULL) AND (quantity IS NULL OR quantity > 0)) NOT VALID;
