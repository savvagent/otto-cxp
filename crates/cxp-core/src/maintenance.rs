//! Unpinned, cross-org background access (story S4): `begin` opens a
//! transaction with no org pinned and `app.maintenance = 'on'` set, the marker
//! the retention policies on `platform_events` require. The only way
//! background code reaches more than one org's rows.
