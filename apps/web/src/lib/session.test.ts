import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const toastError = vi.hoisted(() => vi.fn());
vi.mock("sonner", () => ({ toast: { error: toastError } }));

import { apiFetch, ApiError } from "./api-client";
import { queryClient, shouldRetry } from "./query-client";
import {
  fetchSession,
  loginErrorMessage,
  loginUrl,
  LOGIN_ERRORS,
  returnPath,
  sessionKeys,
  sessionSchema,
  signOut,
  type Session,
} from "./session";

const SIGNED_IN: Session = {
  signedIn: true,
  loginEnabled: true,
  userId: "6f1c1f3e-7d5b-4a43-9a51-2c1b0d5e8f10",
  orgId: "0b8f4d1a-3c2e-4f5a-8b6c-7d9e0f1a2b3c",
  role: "admin",
  scopes: ["cxp:read", "cxp:write"],
};

beforeEach(() => {
  queryClient.clear();
  toastError.mockReset();
});
afterEach(() => vi.unstubAllGlobals());

/** A browser at `path`, whose navigations are recorded. */
function browserAt(path: string) {
  const [pathname, search = ""] = path.split("?");
  const location = {
    pathname,
    search: search ? `?${search}` : "",
    assign: vi.fn(),
  };
  vi.stubGlobal("window", { location });
  return location;
}

function respond(status: number, body: unknown) {
  const fetch = vi.fn(
    async (_url: string, _init?: RequestInit) =>
      new Response(body === null ? null : JSON.stringify(body), { status }),
  );
  vi.stubGlobal("fetch", fetch);
  return fetch;
}

describe("api calls carry the session cookie", () => {
  it("sends credentials to the same origin only", async () => {
    browserAt("/console/customers");
    const fetch = respond(200, { data: [] });
    await apiFetch("/customers");
    const [url, init] = fetch.mock.calls[0];
    expect(url).toBe("/api/v1/customers");
    expect(init?.credentials).toBe("same-origin");
  });

  it("a 401 marks the session signed out and never navigates", async () => {
    const location = browserAt("/console/tickets?status=open");
    queryClient.setQueryData(sessionKeys.all, SIGNED_IN);
    respond(401, { error: "invalid_token", error_description: "access revoked" });
    await expect(apiFetch("/tickets")).rejects.toMatchObject({ status: 401 });
    expect(location.assign).not.toHaveBeenCalled();
    expect(queryClient.getQueryData(sessionKeys.all)).toEqual({
      signedIn: false,
      loginEnabled: true,
    });
  });

  it("a 401 with no session known asks the server again", async () => {
    browserAt("/console/tickets");
    const invalidate = vi.spyOn(queryClient, "invalidateQueries");
    respond(401, { error: "unauthenticated" });
    await expect(apiFetch("/tickets")).rejects.toBeInstanceOf(ApiError);
    expect(invalidate).toHaveBeenCalledWith({ queryKey: sessionKeys.all });
    invalidate.mockRestore();
  });

  it("other failures leave the session alone", async () => {
    const location = browserAt("/console/tickets");
    queryClient.setQueryData(sessionKeys.all, SIGNED_IN);
    respond(403, { error: "this request did not come from this site", code: "cross_site_request" });
    await expect(apiFetch("/tickets", { method: "POST" })).rejects.toMatchObject({
      status: 403,
    });
    expect(location.assign).not.toHaveBeenCalled();
    expect(queryClient.getQueryData(sessionKeys.all)).toEqual(SIGNED_IN);
  });
});

describe("query retries", () => {
  it("never retry a 401 or a 403", () => {
    expect(shouldRetry(0, new ApiError(401, "x"))).toBe(false);
    expect(shouldRetry(0, new ApiError(403, "x"))).toBe(false);
  });

  it("retry anything else once", () => {
    expect(shouldRetry(0, new ApiError(503, "x"))).toBe(true);
    expect(shouldRetry(0, new TypeError("network"))).toBe(true);
    expect(shouldRetry(1, new ApiError(503, "x"))).toBe(false);
  });
});

describe("the session answer", () => {
  it("parses both shapes", async () => {
    expect(sessionSchema.parse(SIGNED_IN)).toEqual(SIGNED_IN);
    respond(200, { signedIn: false, loginEnabled: false });
    await expect(fetchSession()).resolves.toEqual({ signedIn: false, loginEnabled: false });
  });

  it("refuses junk and failures", async () => {
    for (const junk of [
      { signedIn: true, loginEnabled: true },
      { ...SIGNED_IN, userId: "not-a-uuid" },
      { ...SIGNED_IN, role: "root" },
      { signedIn: "yes" },
      null,
    ]) {
      expect(sessionSchema.safeParse(junk).success, JSON.stringify(junk)).toBe(false);
    }
    respond(200, { signedIn: true, loginEnabled: true });
    await expect(fetchSession()).rejects.toThrow();
    respond(503, { error: "unavailable" });
    await expect(fetchSession()).rejects.toThrow(/503/);
  });
});

describe("sign-in and sign-out", () => {
  it("the login URL carries where to come back to, under /console/", () => {
    expect(loginUrl("/console/customers/1?tab=x")).toBe(
      "/auth/login?next=%2Fconsole%2Fcustomers%2F1%3Ftab%3Dx",
    );
    expect(returnPath({ pathname: "/console/tickets", search: "?s=open" })).toBe(
      "/console/tickets?s=open",
    );
    expect(returnPath({ pathname: "/console/", search: "?login_error=failed" })).toBe(
      "/console/",
    );
    expect(returnPath({ pathname: "/elsewhere", search: "" })).toBe("/console/");
  });

  it("a known login error is explained, an unknown one is generic, and none is nothing", () => {
    expect(loginErrorMessage("?login_error=access_denied")).toBe(LOGIN_ERRORS.access_denied);
    expect(loginErrorMessage("?login_error=%3Cscript%3E")).toBe(LOGIN_ERRORS.failed);
    expect(loginErrorMessage("?x=1")).toBeNull();
  });

  it("signing out posts to the server and leaves for the console's home", async () => {
    const location = browserAt("/console/tickets");
    const fetch = respond(204, null);
    await signOut();
    const [url, init] = fetch.mock.calls[0];
    expect(url).toBe("/auth/logout");
    expect(init).toMatchObject({ method: "POST", credentials: "same-origin" });
    expect(location.assign).toHaveBeenCalledWith("/console/");
    expect(toastError).not.toHaveBeenCalled();
  });

  it("a refused sign-out is shown and stays put", async () => {
    const location = browserAt("/console/tickets");
    respond(503, { error: "Savvagent CXP could not complete this right now", code: "unavailable" });
    await expect(signOut()).resolves.toBeUndefined();
    expect(location.assign).not.toHaveBeenCalled();
    expect(toastError).toHaveBeenCalledWith("Savvagent CXP could not complete this right now");
  });

  it("an unreachable server is shown, and nothing rejects", async () => {
    const location = browserAt("/console/tickets");
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new TypeError("Failed to fetch");
      }),
    );
    await expect(signOut()).resolves.toBeUndefined();
    expect(location.assign).not.toHaveBeenCalled();
    expect(toastError).toHaveBeenCalledTimes(1);
  });
});
