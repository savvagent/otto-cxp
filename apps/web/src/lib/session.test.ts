import { afterEach, describe, expect, it, vi } from "vitest";
import { apiFetch, ApiError } from "./api-client";
import { loginErrorMessage, loginUrl, LOGIN_ERRORS, signOut } from "./session";

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
      new Response(JSON.stringify(body), { status }),
  );
  vi.stubGlobal("fetch", fetch);
  return fetch;
}

describe("api calls carry the session cookie", () => {
  it("sends credentials to the same origin only", async () => {
    browserAt("/customers");
    const fetch = respond(200, { data: [] });
    await apiFetch("/customers");
    const [url, init] = fetch.mock.calls[0];
    expect(url).toBe("/api/v1/customers");
    expect(init?.credentials).toBe("same-origin");
  });

  it("a 401 sends the browser to sign in and back", async () => {
    const location = browserAt("/tickets?status=open");
    respond(401, { error: "unauthenticated", error_description: "sign in" });
    await expect(apiFetch("/tickets")).rejects.toBeInstanceOf(ApiError);
    expect(location.assign).toHaveBeenCalledWith(
      "/auth/login?next=%2Ftickets%3Fstatus%3Dopen",
    );
  });

  it("other failures do not", async () => {
    const location = browserAt("/tickets");
    respond(403, { error: "this request did not come from this site", code: "cross_site_request" });
    await expect(apiFetch("/tickets", { method: "POST" })).rejects.toMatchObject({
      status: 403,
    });
    expect(location.assign).not.toHaveBeenCalled();
  });
});

describe("sign-in and sign-out", () => {
  it("the login URL carries where to come back to", () => {
    expect(loginUrl("/customers/1?tab=x")).toBe(
      "/auth/login?next=%2Fcustomers%2F1%3Ftab%3Dx",
    );
  });

  it("a known login error is explained, an unknown one is generic, and none is nothing", () => {
    expect(loginErrorMessage("?login_error=access_denied")).toBe(LOGIN_ERRORS.access_denied);
    expect(loginErrorMessage("?login_error=%3Cscript%3E")).toBe(LOGIN_ERRORS.failed);
    expect(loginErrorMessage("?x=1")).toBeNull();
  });

  it("signing out posts to the server and leaves", async () => {
    const location = browserAt("/tickets");
    const fetch = respond(204, null);
    fetch.mockImplementation(async () => new Response(null, { status: 204 }));
    await signOut();
    const [url, init] = fetch.mock.calls[0];
    expect(url).toBe("/auth/logout");
    expect(init).toMatchObject({ method: "POST", credentials: "same-origin" });
    expect(location.assign).toHaveBeenCalledWith("/");
  });
});
