// The built service worker, read from `dist/`. `pnpm build` runs before
// `pnpm test` in the gate and in CI, so a missing worker is a failure: a
// skipped check would pass without proving anything.
import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const sw = fileURLToPath(new URL("../dist/sw.js", import.meta.url));

describe("the built service worker", () => {
  it("exists (run `pnpm build` first)", () => {
    expect(existsSync(sw), sw).toBe(true);
  });

  it("answers only the console's routes with the console's app shell", () => {
    const source = readFileSync(sw, "utf8");
    expect(source).toContain("NavigationRoute");
    expect(source).toContain('"/console/index.html"');
    expect(source).toContain("allowlist");
    expect(source).toContain("^\\/console");
  });

  it("never answers the server's paths with the app shell", () => {
    const source = readFileSync(sw, "utf8");
    expect(source).toContain("denylist");
    for (const prefix of ["auth", "api", "mcp", "\\.well-known", "platform"]) {
      expect(source, prefix).toContain(`^\\/${prefix}`);
    }
  });
});
