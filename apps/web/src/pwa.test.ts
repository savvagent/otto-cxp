import { describe, expect, it } from "vitest";
import { APP_SHELL, CONSOLE_PATHS, SERVER_ONLY_PATHS } from "./pwa";

const matches = (res: RegExp[], path: string) => res.some((re) => re.test(path));
/** Whether Workbox would answer a navigation to `path` with the app shell. */
const shellFor = (path: string) =>
  matches(CONSOLE_PATHS, path) && !matches(SERVER_ONLY_PATHS, path);

// Workbox tests both lists against `pathname + search`.
const SERVER_PATHS = [
  "/auth/login?next=%2Fconsole%2Ftickets",
  "/auth/callback?code=x&state=y",
  "/auth/session",
  "/auth",
  "/api/v1/customers",
  "/mcp",
  "/.well-known/oauth-protected-resource",
  "/platform/webhooks",
];
// The server's, not the console's: `/` is the connector install page.
const NOT_CONSOLE = ["/", "/dashboard", "/tickets/1", "/consoles", "/nope"];
const CONSOLE_ROUTES = [
  "/console",
  "/console/",
  "/console/dashboard",
  "/console/tickets/1",
  "/console/catalog/pages?tab=x",
];

describe("the service worker answers only the console's routes from cache", () => {
  it("the app shell is the console's index", () => {
    expect(APP_SHELL).toBe("/console/index.html");
  });

  it("denies the app-shell fallback to every server path", () => {
    for (const path of SERVER_PATHS) {
      expect(matches(SERVER_ONLY_PATHS, path), path).toBe(true);
      expect(shellFor(path), path).toBe(false);
    }
  });

  it("leaves every path outside /console/ to the network", () => {
    for (const path of NOT_CONSOLE) {
      expect(shellFor(path), path).toBe(false);
    }
  });

  it("serves the app shell for the console's own routes", () => {
    for (const path of CONSOLE_ROUTES) {
      expect(shellFor(path), path).toBe(true);
    }
  });
});
