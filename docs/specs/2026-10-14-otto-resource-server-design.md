# otto-cxp as an otto platform resource server — design

**Status:** IMPLEMENTED (#19). Production deploy is a human hand-off (`docs/deploy/fly.md`); no separate plan document — the decomposition is the Stories section.
**Epic:** #1
**Reference implementation:** otto-flags (`savvagent/otto-flags`, local checkout
`/home/robhicks/dev/otto-flags`). Where this document is silent, do what otto-flags does.

## Brief

> An org on the otto platform can connect any MCP client to `https://otto-cxp.savvagent.com/mcp`,
> sign in with their otto account, and manage customers, tickets, surveys, and feedback scoped to
> their org, billed on their otto plan. otto-cxp joins otto-factory and otto-flags as a registered
> resource server, modeled on otto-flags.

Success criteria (verbatim from #1):

1. An unauthenticated request to `/mcp` or `/api` gets 401 with a `WWW-Authenticate` header
   pointing at `/.well-known/oauth-protected-resource`; a token issued for another service is
   rejected.
2. Two orgs' data are isolated: forced RLS under `otto_app` on every domain table,
   `verify_tenant_isolation` passes at boot, and an e2e test against a mock platform proves org A
   cannot read or write org B's rows through any tool or route.
3. `whoami` and `usage` work; `submit_feedback`, `create_survey`, `send_survey`, and
   `submit_survey_response` each ship exactly one usage event (deduplicated by `event_id`), and
   every other tool ships none.
4. `org.deleted`, `member.removed`, and `team.deleted` webhooks are signature-verified and applied
   idempotently, and catalog entities owned by a deleted team are handled.
5. Production: the `otto-cxp` Fly app serves `/healthz` and `/readyz`, the boot log shows
   `tenant isolation enforced as role "otto_app"` and
   `the otto platform accepted this service's credential`, CI is green on `main`, and otto-cxp
   appears on the otto-platform marketing site.

Non-goals (verbatim): stdio transport (dropped; HTTP only); Swift and Kotlin SDKs beyond their
stubs; scopes finer than `cxp:read` / `cxp:write`; new CXP domain features; otto-cli changes.

## Premise corrections

- **pgvector is not needed.** Nothing reads or writes `feedback_embeddings` at runtime:
  `server/src/db/embeddings.rs` and `models/embedding.rs` are not even compiled. The baseline
  drops the `vector` extension and that table, which removes the epic's first risk (pgvector on
  the shared `otto-db`). Plain `postgres:16` is enough; `pg_trgm` is a stock contrib extension
  (and `gen_random_uuid()` is built in, so `pgcrypto` is not needed).
- **35 tools, not 26.** 10 customer-data, 10 support, 10 feedback, 5 team tools. After the
  team tools go, 30 domain tools remain, plus `whoami` and `usage`: 32.
- **Prompts are advertised but never served** today (`enable_prompts()` with no handler). The
  port wires the existing four prompts rather than advertising something absent.
- **There is no "list an org's teams" endpoint a resource server can call.** otto-resource offers
  `member_teams(org, user)` (the caller's own teams) and `team(org, id)`. The console's team
  selector lists the caller's teams; writes verify the chosen team with `team()`.
- **`pnpm test` is red on trunk** (`@cxp/react` has no test files). The SDK story fixes it.

## Assumptions

- The Development Workflow section in `CLAUDE.md` was written by this epic, modeled on otto-flags
  (the repo had none). Every value in it is an assumption for a human to confirm.
- otto-platform pin: the same rev otto-flags pins, `fa1ae08c8d5760c1df9752281a078192032e7d24`
  (v0.5.0). Both crates are unchanged through v0.6.0.
- Local Postgres: `postgres:16` on port **15435**, user/password `cxp`/`cxp`, database
  `otto_cxp` (15433 is otto-factory's, 15434 otto-flags').
- Write scope implies read: a read checks for `cxp:read` **or** `cxp:write`; a write checks for
  `cxp:write`. `whoami` and `usage` check no scope (as in otto-flags).
- Only the four billable tools write `usage_outbox` rows; `whoami`, `usage`, and every other
  tool record nothing (criterion 3; otto-flags records free calls as `billable: false`, this
  service does not).
- REST `POST /api/v1/surveys` is the same operation as `create_survey` and charges the same
  usage event (tool name `create_survey`); otherwise the console would be a billing bypass. No
  other route charges.
- A deleted team's catalog entities keep existing with `team_id = NULL` (unowned, visible to the
  org), rather than being deleted. Deleting customer-facing catalog rows on a team change would
  also null every feedback/ticket/customer reference to them.
- `/api` keeps its existing `/api/v1/...` paths. The REST teams routes go; a read-only
  `GET /api/v1/me/teams` (the caller's platform teams) replaces them for the team selector.
- Tool outputs become structured (`Json<T>`, rmcp `structuredContent`, also emitted as JSON
  text content); errors become `ErrorData` with `data.code`. rmcp 2.0.0 requires an object root
  for output schemas, so **every list tool returns the envelope `{"items": [...], "total": n}`**
  (`total` omitted where the query has no count) and single-object tools return the object.
  This changes list tools' wire shape; `@cxp/sdk` (S2) unwraps `items`. Tool names and input
  field names stay exactly as today (snake_case, e.g. `customer_id`), minus the team tools.
- The permissive CORS layer goes; `/api` is called with bearer tokens or same-origin.
- The web console's sign-in (platform OAuth, sessions, static serving) is **not** in this epic:
  no success criterion needs it. It is filed as its own ticket. This epic only removes the
  console's Teams page and points its team selector at `/api/v1/me/teams` so the build stays
  green; until that ticket lands, the console gets 401s from `/api`.
- Seed data is rewritten per org (a psql variable), not discarded.

## Architecture

One binary, `cxp-server`, on `:8080` (`CXP_BIND`), serving:

| Path | Crate | Auth |
|---|---|---|
| `/healthz`, `/readyz` | cxp-server | none |
| `/.well-known/oauth-protected-resource` | cxp-mcp | none |
| `/mcp` (streamable HTTP, stateless, JSON responses) | cxp-mcp | bearer token |
| `/api/v1/...` | cxp-api | bearer token |
| `/platform/webhooks` | cxp-api | `Otto-Signature` HMAC |

### Workspace

```
Cargo.toml              workspace; members crates/cxp-{core,mcp,api,server}
rust-toolchain.toml     stable, rustfmt + clippy
compose.yaml            postgres:16 on 15435 (cxp/cxp/otto_cxp)
crates/cxp-core         all SQL, domain logic, tenancy, auth core, metering, lifecycle
crates/cxp-mcp          rmcp =2.0.0 server, tools, resources, prompts, /mcp + metadata routes
crates/cxp-api          REST /api/v1, /platform/webhooks
crates/cxp-server       binary: config, boot, health, background tasks, router, e2e tests
```

Root `[workspace.package]`: version `0.1.0`, edition 2021, license `AGPL-3.0-or-later`,
`rust-version = "1.88"`. Dependencies go in root `[workspace.dependencies]` matching otto-flags'
versions: `rmcp = { version = "=2.0.0", features = ["server", "macros",
"transport-streamable-http-server", "schemars"] }`, `schemars = "1.1"` (chrono04, uuid1),
`sqlx 0.8` (no default features; runtime-tokio, tls-rustls, postgres, chrono, uuid, macros,
migrate, json), axum 0.8, tower 0.5, tower-http 0.6, otto-tenant and otto-resource at the pinned
rev. The old `server/` crate (package `cxp-server`) stays in the workspace, untouched, until S11
deletes it. S1 creates **skeleton crates** `crates/cxp-mcp` and `crates/cxp-api` (final
`Cargo.toml` dependencies, empty `lib.rs`), adds them and `crates/cxp-core` to `members`, puts
every server dependency in `[workspace.dependencies]`, and commits `Cargo.lock`. S1 does not
create `crates/cxp-server` (its package name collides with the old `server/`); S11 swaps
`server` for `crates/cxp-server` in one change. Only S1 and S11 edit the root `Cargo.toml`;
a later story needing a new dependency adds it to its own crate's `Cargo.toml` only.

### cxp-core module map (S1 creates every file; S4–S8 fill their own)

```
lib.rs              pub mod declarations for everything below; re-exports Error, Result, MIGRATOR, migrate;
                    ping(&Db) (the /readyz probe, so no SQL lives outside cxp-core)
error.rs            Error enum (NotFound { kind: &'static str }, Invalid(String), Conflict(String),
                    UnknownTeam, AccessRevoked, QuotaExceeded{..}, Unavailable, Platform(..),
                    Internal(String), Tenant(..), Database(sqlx::Error)) with `code() -> &'static str` and
                    `retriable() -> bool` (the codes MCP and REST put on the wire: not_found,
                    invalid_argument, conflict, unknown_team, access_revoked, quota_exceeded,
                    unavailable, internal); Display never includes database text; a
                    database error reads the same through Tenant(Db(..)) as through Database;
                    pub(crate) map_fk(e, &[(constraint, kind)]) maps a composite-FK violation
                    to NotFound { kind }
migrate.rs          MIGRATOR { ignore_missing: true, ..sqlx::migrate!("./migrations") }; migrate(&Db)
scopes.rs           RESOURCE_NAME = "otto-cxp"; CXP_READ = "cxp:read"; CXP_WRITE = "cxp:write";
                    KNOWN = [CXP_READ, CXP_WRITE]; DEFAULT = [CXP_READ]
auth.rs             S4   Principal; AuthFailure; authenticate(); www_authenticate(); metadata()  (no axum)
platform_events.rs  S4   begin_live(); revoked(); apply(WebhookEvent) -> Outcome; sweep()
maintenance.rs      S4   pub(crate) begin(&Db) (unpinned, SET LOCAL app.maintenance = 'on')
usage.rs            S4   BILLABLE; Meter { charge(), report() }; ship_once(); run()
analysis/           pure NPS, CSAT, sentiment (moved from server/src/analysis, unchanged logic)
paging.rs           Page { limit, offset } with clamp (limit 1..=200, default 50; offset 0..=10_000)
customers.rs        S5   CustomersExt
interactions.rs     S5   InteractionsExt
segments.rs         S5   SegmentsExt
tickets.rs          S6   TicketsExt
knowledge.rs        S6   KnowledgeExt
chat.rs             S6   ChatExt
feedback.rs         S7   FeedbackExt (feedback, surveys, survey responses)
catalog.rs          S8   CatalogExt (experiences, applications, pages, components); VerifiedTeam,
                         made only by verify_team(&PlatformClient, OrgId, Uuid), carries the org
                         it was verified for; New*.team and TeamChange::Set take a VerifiedTeam,
                         and a write in any other org refuses it (unknown_team)
dashboard.rs        S8   DashboardExt (metrics, incl. ticket counts by status as one SQL aggregate)
```

S1 leaves each S4–S8 file as a module doc comment only (`lib.rs` already declares it). Each
story owns its files and its test file `crates/cxp-core/tests/<domain>.rs`, plus model structs
inside its own module. A story never imports another same-batch story's types: e.g. the
customer-360 view (S5) defines its own `TicketSummary`/`FeedbackSummary` with its own SQL, and
tests insert other stories' rows with raw SQL.

**Every `*Ext` method that either an MCP tool or a REST route needs** is written by the domain
story, not by S9/S10 — including REST-only operations (customer search and delete, ticket
delete, survey list/update/delete, the 360 view, full catalog CRUD with `team_id`). Each domain
story's acceptance criteria enumerate them from `server/src/tools/mod.rs` and `server/src/api/`.

**`0001_baseline.sql` is owned by S1, and after S1 merges only the integrator (the epic run)
edits it** until the epic merges to `main`. A later story that needs a schema change reports it
as a concern instead of editing `0001` or adding a migration. Shared
test helpers live in `crates/cxp-core/tests/common/mod.rs` (S1): `db(pool) -> Db`,
`org() -> OrgId` (fresh random), `user() -> UserId`, `otto_app_db(&pool)` (a pool whose
connections `SET ROLE otto_app`).

### Query pattern

Every domain operation is a method on an extension trait implemented for
`otto_tenant::Tx<'_>`, exactly as `flags_core::apps::AppsExt`:

```rust
pub trait CustomersExt {
    fn create_customer(&mut self, new: &NewCustomer) -> impl Future<Output = Result<Customer>> + Send;
    ...
}
impl CustomersExt for Tx<'_> { ... sqlx::query_as(..).bind(self.org())...fetch_one(self.conn()) }
```

- Runtime `sqlx::query`/`query_as` only. Every statement binds `org_id` explicitly as well as
  relying on RLS (defence in depth).
- Enum columns: `status::text AS status` on read, `$n::ticket_status` on write; model fields are
  `String`. Keep column-list constants (`TICKET_COLS`) as today.
- Models derive `Debug, Clone, Serialize, Deserialize, sqlx::FromRow, schemars::JsonSchema` with
  `#[serde(rename_all = "camelCase")]` (as today). Output types never carry `org_id`.
- Lists are bounded: every list takes `Page` and returns `(Vec<T>, i64 total)` where today's
  REST returns a total; nothing is unbounded (CSAT's `limit 10000` becomes an SQL aggregate).
- A missing row is `Error::NotFound { kind }`, never a panic or an `Option` bubbled as success.
- Customer resolution (UUID, email, or name) moves into `CustomersExt::resolve_customer`.
- `submit_survey_response` becomes one method in one transaction (today it is two writes).

### Schema: `crates/cxp-core/migrations/0001_baseline.sql` (S1)

Modeled on `otto-flags/crates/flags-core/migrations/0001_baseline.sql`: the tolerant
`CREATE ROLE otto_app` block, `pg_trgm`, `current_org()`, the six enums, then the
tables below, then a loop enabling and forcing RLS with `<table>_tenant_isolation` policies
(`USING (org_id = current_org()) WITH CHECK (org_id = current_org())`), then the grant block.
Ids default to `gen_random_uuid()`.

Every tenant table: `org_id uuid NOT NULL`, `id uuid PRIMARY KEY DEFAULT gen_random_uuid()`,
`UNIQUE (org_id, id)`; every reference to another tenant table is a composite
`FOREIGN KEY (org_id, x_id) REFERENCES x (org_id, id)`. Nullable references use
`ON DELETE SET NULL (x_id)` (Postgres 15+ column list, so `org_id` is not nulled); owned children
use `ON DELETE CASCADE`. Exception kept from the legacy schema: the catalog parent links
(`applications.experience_id`, `pages.application_id`, `components.page_id`) are nullable but
`ON DELETE CASCADE`, so deleting a catalog parent still deletes its children.

Indexes: btree only; every non-unique index leads with `org_id` except the two cross-org
background queues (`usage_outbox_due_idx`, `platform_events_received_idx`); every foreign key's
columns lead some index. `tests/db.rs` checks all three. Under FORCE ROW LEVEL SECURITY the
planner only uses LEAKPROOF operators as index conditions beneath the policy's
`org_id = current_org()`; the jsonb, array, trigram, and full-text operators are not leakproof,
so GIN indexes on them (metadata, tags, criteria, trigram, full-text) and the
`(org_id, priority|type|category)` btrees no query used were dropped: they could never serve a
tenant query and only cost writes. `tickets (org_id, status)` is kept: not as a filter (enum `=`
is not leakproof either) but so the dashboard's count-by-status runs as an index-only scan. Customer substring/fuzzy
search and knowledge full-text search therefore scan the org's rows (#17).

| Table | From | Changes beyond org_id/composite FKs |
|---|---|---|
| customers | 001, 008 | `UNIQUE (org_id, email)`; catalog refs (experience/application/page/component); stored `name_lower` + `(org_id, name_lower)` for resolve_customer's exact-name step |
| tickets | 001, 008 | catalog refs; `(org_id, customer_id, created_at DESC)` for the 360 view |
| interactions | 001 | — |
| feedback | 001, 007 | catalog refs; legacy free-text experience/application/page/component columns dropped; `(org_id, customer_id, created_at DESC)` |
| surveys | 001, 008 | catalog refs |
| survey_responses | 001 | — |
| chat_sessions | 001 | — |
| chat_messages | 001 | — |
| knowledge_articles | 002 | stored `search_tsv` (generated `to_tsvector('english', title \|\| ' ' \|\| content)`) that search matches and ranks; no GIN index (#17); `(org_id, published, created_at DESC)` |
| segments | 003 | — |
| segment_members | 003 | PK `(org_id, segment_id, customer_id)` |
| experiences | 006 | `team_id uuid NULL` (platform team id, no FK) |
| applications | 006, 009 | nullable `experience_id`; `team_id` |
| pages | 006, 009 | nullable `application_id`; `team_id` |
| components | 006, 009 | nullable `page_id`; `team_id` |

Dropped: `teams`, `feedback_embeddings`, the `vector`, `uuid-ossp`, and `pgcrypto` extensions,
and the legacy free-text columns from 005.

Service bookkeeping (copy otto-flags' definitions and policies):

| Table | RLS | Purpose |
|---|---|---|
| usage_outbox, usage_outbox_rejected | `org_id = current_org() OR (current_org() IS NULL AND current_setting('app.maintenance', true) = 'on')`: an unpinned transaction without the maintenance marker sees, changes, and deletes nothing, as on `platform_events` | metering |
| platform_events | tenant, keyed by `event_id`, **plus** otto-flags' `0003_unpinned_sweep_reads.sql` policies folded in: `platform_events_retention_read` (`current_org() IS NULL AND current_setting('app.maintenance', true) = 'on'`), the restrictive `platform_events_maintenance_delete`, and the delete-retention policy, so the sweep can delete under any role RLS applies to (the tests run it as `otto_app`; in production it runs as the connecting role, see #16) | webhook dedupe |
| deleted_orgs, removed_members | none (pre-org lookups, as in otto-flags) | tombstones |

Grants: `otto_app` gets SELECT/INSERT/UPDATE/DELETE on every table, except `deleted_orgs`
(SELECT only: tombstones are permanent, and the `org.deleted` handler writes them as the
connecting role), `removed_members` (no DELETE: the sweep expires
tombstones through the `SECURITY DEFINER` function `expire_removed_members()`, which can only
remove rows past twice the 300 s window), and `_sqlx_migrations` (none).

No `audit_events` table: this service does not call `Tx::audit`.

### Auth (`cxp_core::auth`, S4)

```rust
pub struct Principal { token_id, user_id: UserId, org_id: OrgId, role: Role, client_id: Option<String>,
                       scopes: Vec<String>, kind: TokenKind, expires_at }
impl Principal { fn can_read(&self) -> bool; fn require_read(&self) -> Result<(), MissingScope>;
                 fn require_write(&self) -> Result<(), MissingScope>; }
pub enum AuthFailure { Missing, Inactive, Revoked, Unavailable, Internal }
pub async fn authenticate(db: &Db, platform: &PlatformClient, authorization: Option<&str>)
    -> Result<Principal, AuthFailure>;     // parse "Bearer x" case-insensitively; introspect;
                                           // Ok(None) => Inactive (wrong audience lands here);
                                           // retriable Err => Unavailable; any other Err (our
                                           // credential refused, not the platform) => Internal;
                                           // tombstone => Revoked
pub fn www_authenticate(public_url: &str, failure: &AuthFailure) -> String;
    // Bearer realm="otto-cxp", error="invalid_token", error_description="…",
    // resource_metadata="{public_url}/.well-known/oauth-protected-resource"
pub fn metadata(resource_uri: &str, platform_url: &str) -> ProtectedResourceMetadata;
```

HTTP mapping (in cxp-mcp and cxp-api middleware, ~40 lines each): `Missing`/`Inactive`/`Revoked`
→ 401 + `WWW-Authenticate` + JSON body `{error, error_description}`; `Unavailable` → 503 +
`Retry-After: 5`; `Internal` → 500 (`error` `internal`), no `Retry-After`, logged at error. The audience is `CXP_RESOURCE_URI` and is never derived from `Host`.

### Tenancy and lifecycle (`cxp_core::platform_events`, S4)

`begin_live(db, org, Some(user))` as otto-flags: `db.begin(org)`, shared advisory lifecycle lock,
then refuse with `Error::AccessRevoked` when the org is in `deleted_orgs` or the user in
`removed_members` (300 s window). Every request path opens its transaction here.

`apply(db, WebhookEvent) -> Outcome { Applied | Duplicate | Ignored }`:
- `org.deleted`: tombstone (committed on its own), then short transactions that each take the
  exclusive lock and delete at most `PURGE_BATCH` (5000) rows per table, children before parents,
  stopping at the first table that still has rows (so nothing goes by cascade), until every
  table is empty. The dedupe marker is written in that last transaction, so a crash mid-purge
  leaves no marker and the redelivery finishes the purge. A duplicate re-runs the purge too.
- `member.removed`: tombstone the member, dedupe.
- `team.deleted`: dedupe, then `UPDATE experiences|applications|pages|components SET team_id =
  NULL WHERE org_id = $1 AND team_id = $2`; returns `Applied`.
- `Unknown` (a kind the pinned otto-resource does not know): `Ignored`, as otto-resource
  documents.
- Any other `LifecycleEvent` variant (one the pinned client parses but `apply` has no arm for,
  after a client bump): logged at error and refused as `Error::Unavailable`, so the webhook
  answers 503 and the platform redelivers until a release handles it.

### Metering (`cxp_core::usage`, S4)

`BILLABLE = ["submit_feedback", "create_survey", "send_survey", "submit_survey_response"]`.
`Meter::charge(&self, tx, user, tool)` returns `Error::Internal` (a programming error, code
`internal`) if `tool` is not in `BILLABLE`, optionally enforces the quota (`CXP_ENFORCE_QUOTAS`,
default off, fail open) and inserts one `usage_outbox` row in the caller's transaction.
`Meter::report(org)` returns the usage summary `whoami`/`usage` show (otto-flags' `Status`).
`ship_once`/`run` ship batches to `POST /internal/usage` (event_id dedupe at the platform), as
otto-flags, claiming due rows in `(next_attempt_at, id)` order, in maintenance transactions.
Rows the platform refuses per event in its receipt move to `usage_outbox_rejected` with the
reason, logged at error. A batch it refuses whole with a 4xx about the request, 413 included, is
split in halves and resent recursively until the refused events stand alone; an event refused
alone is kept and the refusal counts toward `MAX_ROW_FAILURES` (20, column `row_failures`). If
every event sent alone was refused and none was accepted, the refusal is systemic
(`ShipError::RefusedAll`, logged once at error): the whole batch is rescheduled without counting.
Splitting is bounded per pass by half the claim lease and `ShipperConfig::split_budget` requests
(64); unsent pieces are rescheduled uncounted (`ShipError::Unfinished`, retriable). A receipt
is trusted only if its counts add up and its rejected ids are distinct and all in the batch;
otherwise the batch is retried, and that counts toward `MAX_ROW_FAILURES` too, after which the
row is given up the same way. If settling the answered rows fails, the failed pieces are still
rescheduled with their own errors. Nothing else counts: transport
failures, 5xx, 408, 429, 401/403, 404, and an unknown org reschedule with backoff forever, so
those rows stay in the outbox (and in the period's quota) until they ship. `run` logs failures
that pass on their own (`ShipError::is_retriable`) at warn and the rest at error.
An enforcing meter counts only unshipped rows from the platform's current period.
The retention `sweep` (also maintenance, one transaction per step) forgets `platform_events`
markers past their retention, expired `removed_members` tombstones, and `usage_outbox_rejected`
rows older than 90 days.

### MCP (`cxp-mcp`, S9)

`Cxp` server struct (db, platform, meter, tool_router). Tools in `tools/{org,cdp,support,feedback}.rs`
with `#[tool_router(router = …)]` per file, combined in `tools/mod.rs`; `tools::NAMES` lists all
32 and a test checks every `BILLABLE` name is a tool and that names match the router. Every tool:
`caller(&parts)` → scope check → `tx(&caller)` (begin_live) → (`charge` if billable) → domain call
→ commit → `Json(out)`. Errors: `ErrorData` with `data: {code, retriable}`; codes include
`unauthenticated`, `insufficient_scope`, `not_found`, `invalid_argument`, `access_revoked`,
`quota_exceeded`, `unavailable`, `internal` (no database text). Resources (3) and prompts (4)
ported; resources need read scope; `metrics://dashboard` calls `DashboardExt` (S8). `lib.rs` exposes `Config { public_url, resource_uri,
platform_url, allowed_hosts, allowed_origins }` and `router(db, platform, meter, config) -> Result<Router, NoAllowedHosts>`
that mounts `/mcp` behind `require_bearer` and the open metadata route, refusing an empty
`allowed_hosts` (rmcp reads one as "accept any Host").

### REST (`cxp-api`, S10)

Same routes and JSON shapes as `server/src/api` today minus `/teams`, behind `require_bearer`: GET needs read, other methods need write. Plus
`GET /api/v1/me/teams` → `platform.member_teams(org, user)` as `[{id, slug, name}]`. Catalog
create/update with a `teamId` verifies it with `cxp_core::catalog::verify_team(platform, org, id)`
(unknown, or another org's → 400 `unknown_team`; platform down → 503), which returns the
`VerifiedTeam` the catalog write requires, so no route can store an unverified team id. On update, an absent team
field leaves the team unchanged and an explicit `null` clears it. Request bodies are snake_case
(`team_id`), and catalog bodies also accept `teamId` as a permanent alias; responses are
camelCase. The casing mismatch is resolved by #15 (the console reads camelCase); the broader
question of one convention for `/api/v1` is tracked in #24. `POST /api/v1/surveys` charges `create_survey`.
`POST /platform/webhooks`: verify `Otto-Signature` with `CXP_PLATFORM_WEBHOOK_SECRET` (bad
signature 401, bad body 400), `platform_events::apply` (an event at fault, code `invalid_argument`, is 400
`invalid_event`; any other failure, a bug here included, is 503 so the platform redelivers until
a fix applies it), 200 `{"result": …}`. Errors are JSON
`{error, code}` with no database text. Request bodies are capped at 256 KiB
(`cxp_api::MAX_BODY_BYTES`): `DefaultBodyLimit` on cxp-api's own routes, and cxp-server's
router-wide `RequestBodyLimitLayer` on every surface, `/mcp` included. `lib.rs`: `AppState::new(db, platform, meter, config)`,
`router(state) -> Router`.

### Binary (`cxp-server`, S11)

Config (all `CXP_*`): `CXP_PUBLIC_URL`, `CXP_RESOURCE_URI` (default `{public}/mcp`),
`CXP_PLATFORM_URL`, `CXP_INTROSPECTION_SECRET`, `CXP_PLATFORM_WEBHOOK_SECRET`,
`CXP_DATABASE_URL` (falls back to `DATABASE_URL`), `CXP_DB_MAX_CONNECTIONS` (5, from 2 to 20),
`CXP_RUN_MIGRATIONS` (true), `CXP_BIND` (`0.0.0.0:8080`), `CXP_ALLOWED_HOSTS`,
`CXP_ALLOWED_ORIGINS`, `CXP_ENFORCE_QUOTAS` (false), `CXP_LOG_FORMAT` (`text|json`); `Debug`
redacts secrets; `for_test()`. Boot as otto-flags: config → pool → `migrate` (logs `applying
migrations`) → `verify_tenant_isolation` (refuse to serve on problems; log the summary) →
platform credential check (logs `the otto platform accepted this service's credential`) → spawn
shipper and sweeper → serve with graceful shutdown and a final usage flush. `/healthz` static,
`/readyz` 503 while the boot platform check is `Rejected` or `Misconfigured`, else `SELECT 1`
with a 2 s timeout. A drain that fails, or a final flush that fails for a reason that will not
pass on its own, exits non-zero; a flush that cannot reach the platform logs at warn and leaves
its rows for the next start. `tests/e2e.rs` with a mock platform proves criteria 1–4
over HTTP.

## Stories

| Id | Story | Owns | Depends on | Batch |
|---|---|---|---|---|
| S1 | cxp-core foundation: workspace, skeleton crates, baseline schema, error, migrate, scopes, paging, analysis, test helpers, module stubs | root `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `compose.yaml`, `crates/cxp-core/**` (stubs only for S4–S8 files), `crates/cxp-mcp/{Cargo.toml,src/lib.rs}`, `crates/cxp-api/{Cargo.toml,src/lib.rs}` | — | 0 |
| S2 | TypeScript SDK and React package on HTTP + OAuth; console Teams page removed | `sdk/typescript/**`, `packages/react/**`, `apps/web/**` | — | 0 |
| S3 | otto-platform marketing entry (PR in savvagent/otto-platform; done when the PR is open) | otto-platform `web/` | — | 0 |
| S4 | cxp-core platform integration: auth core, lifecycle webhooks, maintenance, metering | `auth.rs`, `platform_events.rs`, `maintenance.rs`, `usage.rs`, `tests/lifecycle.rs` | S1 | 1 |
| S5 | Customer-data domain | `customers.rs`, `interactions.rs`, `segments.rs`, `tests/customers.rs` | S1 | 1 |
| S6 | Support domain | `tickets.rs`, `knowledge.rs`, `chat.rs`, `tests/support.rs` | S1 | 1 |
| S7 | Feedback domain | `feedback.rs`, `tests/feedback.rs` | S1 | 1 |
| S8 | Catalog and dashboard domain | `catalog.rs`, `dashboard.rs`, `tests/catalog.rs` | S1 | 1 |
| S9 | cxp-mcp on rmcp 2.0.0 | `crates/cxp-mcp/**` | S1, S4–S8 | 2 |
| S10 | cxp-api REST and webhooks | `crates/cxp-api/**` | S1, S4–S8 | 2 |
| S11 | cxp-server binary, Dockerfile, e2e tests, old server removed | `crates/cxp-server/**`, root `Cargo.toml` members, `Cargo.lock`, `Dockerfile`, `.dockerignore`, `server/**` (delete) | S9, S10 | 3 |
| S12 | Deploy and repo ops: fly.toml, CI, deploy doc, local seed, docs | `fly.toml`, `.github/**`, `docs/deploy/**`, `.env.example`, `scripts/**`, `docker-compose.yml` (delete), `README.md`, `CLAUDE.md` | S9, S10 (names); S11 at integration | 3 |

Gates: the `podman build` gate applies only once a `Dockerfile` exists (S11). S12's CI
`docker-build` job is validated at integration, after S11 merges.

S12's `scripts/seed-demo.sql` is local-only demo data (a psql `-v org=<uuid>` variable); it is
not application SQL, so "no SQL outside cxp-core" does not cover it, and running it against
`otto-db` is a production write: human-only.

Out of scope (own tickets): the web console's platform OAuth sign-in. The web app's types now
read the server's camelCase JSON (#15); one casing convention for `/api/v1` is #24.

## Out-of-band and hand-offs

All human (Rob), none performed by the agent: create the `otto-cxp` Fly app; create database
`otto_cxp` on `otto-db` and attach it; `resource register https://otto-cxp.savvagent.com/mcp
--name otto-cxp --scopes cxp:read,cxp:write --default-scopes cxp:read`; `rotate-secret`;
`set-webhook … https://otto-cxp.savvagent.com/platform/webhooks`; `fly secrets set` for `CXP_INTROSPECTION_SECRET`, `CXP_PLATFORM_WEBHOOK_SECRET`; `fly certs add` and DNS; repo secret `FLY_API_TOKEN` and variable
`FLY_DEPLOY=enabled`; merging the otto-platform marketing PR once otto-cxp is live. Exact
commands live in `docs/deploy/fly.md` (S12).

## Testing approach

- `crates/cxp-core/tests/*.rs`: `#[sqlx::test(migrator = "cxp_core::MIGRATOR")]` against real
  Postgres; every domain test file includes a two-org isolation test for its tables (org B sees
  nothing of org A and cannot update/delete it, and a composite FK refuses a cross-org reference).
- `tests/db.rs` (S1): migrations contiguous, rollback-tolerant boot, tenant isolation enforced on
  every tenant table (explicit list of all 15 domain tables plus the bookkeeping tables).
- `tests/lifecycle.rs` (S4): lifecycle `apply` idempotency including `team.deleted` nulling
  catalog `team_id`, the sweep deleting as `otto_app`, usage outbox and shipper against a stub.
- `crates/cxp-server/tests/e2e.rs` (S11): mock platform; 401 + header on `/mcp` and `/api`;
  wrong-audience token rejected; every tool and route under org B against org A's ids; usage
  events per tool; webhooks.
- pnpm: `@cxp/sdk` and `@cxp/react` vitest suites (S2); `sdk/` and `packages/` stay MIT.

## Risks

- rmcp 2.0.0 API differences from otto-flags' usage are copied, not discovered.
- Parallel DB tests: each parallel story runs its own Postgres container (see `CLAUDE.md`).
- Until the console-OAuth ticket lands, the local web console cannot call `/api`.
- `team.deleted` race: a catalog write that verified its team at the platform just before the
  team was deleted, but commits after the webhook nulled the org's team ids, keeps a stale
  `team_id`. Accepted: the window is one request (the team lookup is not cached; the
  route verifies the team before opening its transaction), and a team id the platform no longer knows simply matches no team in the console's
  selector (`/api/v1/me/teams`), so the entity shows as unassigned. A `deleted_teams` tombstone would close it if it ever matters.
- The usage shipper is the second caller of `cxp_core::maintenance` (after the sweep).
- Search under RLS (#17): forced row-level security keeps non-leakproof operators (`ILIKE`,
  trigram `%`, full-text `@@`) from being index conditions, so `search_customers` and
  `search_articles` scan the org's customers / published articles. Fine at today's org sizes;
  #17 tracks a leakproof-safe search path (e.g. a security-barrier function or per-org search
  index) before an org grows large.
- Database role (#16): cxp-server connects as the migrating role and assumes `otto_app` per
  tenant transaction; background maintenance and the tombstone write run as the connecting
  role. The grants narrow what `otto_app` can do, but the connecting role itself is still
  privileged; #16 tracks a least-privilege login role.
