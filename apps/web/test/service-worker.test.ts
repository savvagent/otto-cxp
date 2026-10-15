// The built service worker, read from `dist/` (so it lives outside `src/`,
// which `tsc` checks without Node's types). `pnpm build` runs before
// `pnpm test` in the gate and in CI; run alone before any build, there is no
// worker to check and the test says so by not running.
import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const sw = fileURLToPath(new URL("../dist/sw.js", import.meta.url));

describe("the built service worker", () => {
  it.runIf(existsSync(sw))("never answers the server's paths with the app shell", () => {
    const source = readFileSync(sw, "utf8");
    expect(source).toContain("NavigationRoute");
    expect(source).toContain("denylist");
    for (const prefix of ["auth", "api", "mcp", "\\.well-known", "platform"]) {
      expect(source, prefix).toContain(`^\\/${prefix}`);
    }
  });
});
