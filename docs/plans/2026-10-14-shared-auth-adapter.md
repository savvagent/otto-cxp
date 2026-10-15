# Shared auth adapter plan

Spec: `docs/specs/2026-10-14-shared-auth-adapter-design.md`. Issue #18.

- [x] 1. Create `crates/cxp-http` (Cargo.toml, src/lib.rs) with the
  `refuse` unit tests and a metadata-router test written first; add it to the
  root `Cargo.toml` members and `[workspace.dependencies]`.
- [x] 2. Point `crates/cxp-api/src/lib.rs` at `cxp_http::require_bearer` with
  `challenge_handler_401()`; delete `crates/cxp-api/src/auth.rs`.
- [x] 3. Point `crates/cxp-mcp/src/lib.rs` at it; keep only `principal_from`
  in `crates/cxp-mcp/src/auth.rs`; drop the metadata route, `Shared`, and
  `Config::{resource_uri, platform_url}`; adjust `tests/mcp.rs` as the spec
  says.
- [x] 4. Mount `cxp_http::protected_resource_router` in
  `crates/cxp-server/src/lib.rs`; add the challenge-path assertion.
- [x] 5. Gates: `DATABASE_URL=... cargo test --workspace --locked`,
  `cargo clippy --workspace --all-targets --locked -- -D warnings`,
  `cargo fmt --all -- --check`, migrations-append-only diff.
- [x] 6. Commit (`refactor(http): share one auth adapter ...`, `Fixes #18`).
