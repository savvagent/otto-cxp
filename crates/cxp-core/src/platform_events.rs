//! The otto platform's lifecycle events (story S4): `begin_live`, the
//! transaction every request path opens (refusing deleted orgs and removed
//! members), `apply` for the signed `org.deleted`, `member.removed`, and
//! `team.deleted` webhooks (idempotent on `event_id`), and the retention
//! `sweep` of the `platform_events` log.
