import { describe, expect, it } from "vitest";
import { SERVER_ONLY_PATHS } from "./pwa";

const serverOnly = (path: string) => SERVER_ONLY_PATHS.some((re) => re.test(path));

// Workbox tests the denylist against `pathname + search`.
const SERVER_PATHS = [
  "/auth/login?next=%2Ftickets",
  "/auth/callback?code=x&state=y",
  "/auth/session",
  "/auth",
  "/api/v1/customers",
  "/mcp",
  "/.well-known/oauth-protected-resource",
  "/platform/webhooks",
];
const CONSOLE_PATHS = ["/", "/dashboard", "/tickets/1", "/authors", "/apis", "/catalog/pages"];

describe("the service worker leaves the server's paths alone", () => {
  it("denies the app-shell fallback to every server path", () => {
    for (const path of SERVER_PATHS) {
      expect(serverOnly(path), path).toBe(true);
    }
  });

  it("still serves the app shell for the console's own routes", () => {
    for (const path of CONSOLE_PATHS) {
      expect(serverOnly(path), path).toBe(false);
    }
  });
});
