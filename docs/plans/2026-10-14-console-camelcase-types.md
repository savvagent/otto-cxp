# Console camelCase types — plan

Spec: `docs/specs/2026-10-14-console-camelcase-types-design.md` (#15)

- [ ] **1. Fixture from Rust (test first).** In `crates/cxp-api/src/routes/mod.rs` add the
  `wire_fixture` test module; run `UPDATE_WIRE_FIXTURES=1 cargo test -p cxp-api --locked
  wire_fixture` to create `crates/cxp-api/tests/fixtures/wire.json`; rerun without the variable.
- [ ] **2. Failing web test.** Add `vitest` and a `test` script to `apps/web/package.json`,
  `resolveJsonModule` to its tsconfig, and `src/lib/types.test.ts`; `pnpm build` fails against the
  snake_case types.
- [ ] **3. Types.** Rewrite `apps/web/src/lib/types.ts` to camelCase, add `TicketSummary` and
  `FeedbackSummary`, keep `DashboardMetrics`.
- [ ] **4. Readers.** Fix every page flagged by `tsc -b` (list above in the spec); request bodies
  and query params stay snake_case.
- [ ] **5. Comment.** Correct the stale `#15` note in `routes/catalog.rs`.
- [ ] **6. Gates.** `cargo test --workspace --locked` (own DB port), clippy, fmt, the migrations
  check, and `pnpm install --frozen-lockfile && pnpm lint && pnpm build && pnpm test`.
- [ ] **7. Commit** with `Fixes #15`.
