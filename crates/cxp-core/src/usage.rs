//! Usage metering (story S4): the `BILLABLE` tools, the `Meter` that charges
//! one `usage_outbox` row in the tool's own transaction (with optional quota
//! enforcement) and reports the org's usage, and the shipper (`ship_once`,
//! `run`) that drains the outbox to the platform.
