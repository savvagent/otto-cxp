import { afterEach, describe, it, expect, vi } from "vitest";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { createCxpTransport } from "./connect.js";
import { createServer, type Server as HttpServer } from "node:http";
import type { AddressInfo } from "node:net";
import { UnauthorizedError } from "@modelcontextprotocol/sdk/client/auth.js";
import type { OAuthClientProvider } from "@modelcontextprotocol/sdk/client/auth.js";
import { StreamableHTTPError } from "@modelcontextprotocol/sdk/client/streamableHttp.js";
import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/streamableHttp.js";
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
} from "@modelcontextprotocol/sdk/types.js";
import { CxpError } from "./call.js";
import { connectCxp, CxpAuthorizationRequired, type CxpConnection } from "./connect.js";
import * as sdk from "./index.js";

const TOKEN = "good-token";
const OAUTH_TOKEN = "oauth-token";
let http: HttpServer | undefined;
let conn: CxpConnection | undefined;
const seenAuth: Array<string | undefined> = [];
let tokenRequests = 0;
let tokenEndpointStatus = 200;
let mcpStatus: number | undefined;

async function startServer(accepted: string[] = [TOKEN]): Promise<string> {
  http = createServer(async (req, res) => {
    const origin = `http://127.0.0.1:${(http!.address() as AddressInfo).port}`;
    const path = new URL(req.url ?? "/", origin).pathname;
    const json = (status: number, body: unknown) => {
      res.writeHead(status, { "content-type": "application/json" });
      res.end(JSON.stringify(body));
    };
    if (path.startsWith("/.well-known/oauth-protected-resource")) return json(404, {});
    if (path.startsWith("/.well-known/oauth-authorization-server")) {
      return json(200, {
        issuer: origin,
        authorization_endpoint: `${origin}/authorize`,
        token_endpoint: `${origin}/token`,
        response_types_supported: ["code"],
        code_challenge_methods_supported: ["S256"],
      });
    }
    if (path === "/token") {
      tokenRequests++;
      req.resume();
      if (tokenEndpointStatus !== 200) return json(tokenEndpointStatus, { error: "invalid_grant" });
      return json(200, { access_token: OAUTH_TOKEN, token_type: "Bearer" });
    }

    seenAuth.push(req.headers.authorization);
    if (mcpStatus !== undefined) return json(mcpStatus, { error: "forced" });
    const bearer = req.headers.authorization?.replace(/^Bearer /, "");
    if (!bearer || !accepted.includes(bearer)) return json(401, { error: "invalid_token" });

    const mcp = new Server(
      { name: "fake-cxp", version: "0.0.0" },
      { capabilities: { tools: {} } }
    );
    mcp.setRequestHandler(ListToolsRequestSchema, async () => ({
      tools: [{ name: "list_tickets", inputSchema: { type: "object" as const } }],
    }));
    mcp.setRequestHandler(CallToolRequestSchema, async () => ({
      content: [{ type: "text" as const, text: '{"items":[{"id":"t1"}],"total":1}' }],
      structuredContent: { items: [{ id: "t1" }], total: 1 },
    }));
    const transport = new StreamableHTTPServerTransport({ sessionIdGenerator: undefined });
    res.on("close", () => void transport.close());
    await mcp.connect(transport);
    await transport.handleRequest(req, res);
  });
  await new Promise<void>((r) => http!.listen(0, "127.0.0.1", r));
  return `http://127.0.0.1:${(http.address() as AddressInfo).port}/mcp`;
}

function fakeProvider(): OAuthClientProvider & { redirects: URL[] } {
  let tokens: { access_token: string; token_type: string } | undefined;
  let verifier = "";
  const redirects: URL[] = [];
  return {
    redirects,
    get redirectUrl() {
      return "http://localhost/callback";
    },
    get clientMetadata() {
      return {
        redirect_uris: ["http://localhost/callback"],
        client_name: "test",
        token_endpoint_auth_method: "none",
      };
    },
    clientInformation: () => ({ client_id: "test-client" }),
    tokens: () => tokens,
    saveTokens: (t) => {
      tokens = t as typeof tokens;
    },
    redirectToAuthorization: (u) => {
      redirects.push(u);
    },
    saveCodeVerifier: (v) => {
      verifier = v;
    },
    codeVerifier: () => verifier,
  };
}

afterEach(async () => {
  await conn?.close();
  conn = undefined;
  await new Promise<void>((r) => (http ? http.close(() => r()) : r()));
  http = undefined;
  seenAuth.length = 0;
  tokenRequests = 0;
  tokenEndpointStatus = 200;
  mcpStatus = undefined;
  vi.restoreAllMocks();
});

describe("connectCxp with a bearer token", () => {
  it("connects over streamable HTTP and calls a list tool", async () => {
    const url = await startServer();
    conn = await connectCxp({ url, token: TOKEN });
    expect(await conn.cxp.support.listTickets()).toEqual([{ id: "t1" }]);
    expect(seenAuth.length).toBeGreaterThan(0);
    expect(seenAuth.every((a) => a === `Bearer ${TOKEN}`)).toBe(true);
  });

  it("wraps a 401 as a non-retriable CxpError keeping the server kind and the cause", async () => {
    const url = await startServer();
    const err = await connectCxp({ url, token: "nope" }).catch((e) => e);
    expect(err).toBeInstanceOf(CxpError);
    expect(err).not.toBeInstanceOf(CxpAuthorizationRequired);
    expect(err.code).toBe("invalid_token");
    expect(err.retriable).toBe(false);
    expect(err.cause).toBeInstanceOf(StreamableHTTPError);
    expect(err.cause.code).toBe(401);
  });

  it("wraps a 503 as a retriable unavailable CxpError with the cause", async () => {
    const url = await startServer();
    mcpStatus = 503;
    const err = await connectCxp({ url, token: TOKEN }).catch((e) => e);
    expect(err).toBeInstanceOf(CxpError);
    expect(err.code).toBe("unavailable");
    expect(err.retriable).toBe(true);
    expect(err.cause).toBeInstanceOf(StreamableHTTPError);
    expect(err.cause.code).toBe(503);
  });

  it("wraps a 500 as a non-retriable internal CxpError", async () => {
    const url = await startServer();
    mcpStatus = 500;
    const err = await connectCxp({ url, token: TOKEN }).catch((e) => e);
    expect(err).toBeInstanceOf(CxpError);
    expect(err.code).toBe("internal");
    expect(err.retriable).toBe(false);
    expect(err.cause).toBeInstanceOf(StreamableHTTPError);
  });

  it("wraps a 500 as internal on the authProvider path too", async () => {
    const url = await startServer();
    mcpStatus = 500;
    const err = await connectCxp({ url, authProvider: fakeProvider() }).catch((e) => e);
    expect(err).toBeInstanceOf(CxpError);
    expect(err).not.toBeInstanceOf(CxpAuthorizationRequired);
    expect(err.code).toBe("internal");
    expect(err.cause).toBeInstanceOf(StreamableHTTPError);
  });

  it("requires exactly one of token or authProvider", async () => {
    // JS callers can bypass the discriminated union, so the runtime check stays.
    const neither = { url: "http://127.0.0.1:1/mcp" } as unknown as Parameters<typeof connectCxp>[0];
    const both = {
      url: "http://127.0.0.1:1/mcp",
      token: "t",
      authProvider: fakeProvider(),
    } as unknown as Parameters<typeof connectCxp>[0];
    await expect(connectCxp(neither)).rejects.toThrow(/connectCxp: .*required/);
    await expect(connectCxp(both)).rejects.toThrow(/connectCxp: .*not both/);
  });

  it("reports validation failures as invalid_argument naming the caller", () => {
    const err = (() => {
      try {
        createCxpTransport({ url: "http://10.0.0.5/mcp", token: "t" });
      } catch (e) {
        return e as CxpError;
      }
    })();
    expect(err).toBeInstanceOf(CxpError);
    expect(err!.code).toBe("invalid_argument");
    expect(err!.message).toMatch(/^createCxpTransport:/);
  });

  it("keeps the parse error as cause for an invalid url", async () => {
    const err = await connectCxp({ url: "not a url", token: "t" }).catch((e) => e);
    expect(err).toBeInstanceOf(CxpError);
    expect(err.code).toBe("invalid_argument");
    expect(err.message).toMatch(/^connectCxp:/);
    expect(err.cause).toBeInstanceOf(TypeError);
  });

  it("types token and authProvider as mutually exclusive", () => {
    const provider = fakeProvider();
    // @ts-expect-error both token and authProvider
    const bad: Parameters<typeof connectCxp>[0] = { url: "u", token: "t", authProvider: provider };
    // @ts-expect-error neither
    const none: Parameters<typeof connectCxp>[0] = { url: "u" };
    expect([bad, none]).toHaveLength(2);
  });
});

describe("static token safety", () => {
  it("refuses a token over plain http to a non-loopback host", async () => {
    await expect(connectCxp({ url: "http://cxp.example.com/mcp", token: "t" })).rejects.toThrow(
      /refusing to send a static `token`.*https/
    );
    expect(() => createCxpTransport({ url: "http://10.0.0.5/mcp", token: "t" })).toThrow(/https/);
  });

  it("refuses a token for a non-http(s) scheme", () => {
    expect(() => createCxpTransport({ url: "ftp://cxp.example.com/mcp", token: "t" })).toThrow(
      /refusing/
    );
  });

  it("allows https and loopback http", () => {
    for (const u of [
      "https://cxp.example.com/mcp",
      "http://localhost:3000/mcp",
      "http://127.0.0.1:3000/mcp",
      "http://[::1]:3000/mcp",
    ]) {
      expect(() => createCxpTransport({ url: u, token: "t" }), u).not.toThrow();
    }
  });

  it("does not apply the https rule to an authProvider", () => {
    expect(() =>
      createCxpTransport({ url: "http://cxp.example.com/mcp", authProvider: fakeProvider() })
    ).not.toThrow();
  });

  it("rejects a token containing CR or LF", async () => {
    for (const bad of ["abc\r\nX-Injected: 1", "abc\ndef", "abc\rdef"]) {
      expect(() => createCxpTransport({ url: "https://cxp.example.com/mcp", token: bad })).toThrow(
        /CR or LF/
      );
    }
    await expect(
      connectCxp({ url: "https://cxp.example.com/mcp", token: "a\nb" })
    ).rejects.toThrow(/connectCxp: .*CR or LF/);
  });
});

describe("client and transport cleanup", () => {
  it("closes the unused client before throwing CxpAuthorizationRequired", async () => {
    const url = await startServer([OAUTH_TOKEN]);
    let closed = false;
    vi.spyOn(Client.prototype, "close").mockImplementation(async () => {
      await new Promise((r) => setTimeout(r, 25));
      closed = true;
    });
    const err = await connectCxp({ url, authProvider: fakeProvider() }).catch((e) => e);
    expect(err).toBeInstanceOf(CxpAuthorizationRequired);
    expect(closed).toBe(true);
  });

  it("closes the client when the connection fails for another reason", async () => {
    const url = await startServer();
    let closed = false;
    vi.spyOn(Client.prototype, "close").mockImplementation(async () => {
      await new Promise((r) => setTimeout(r, 25));
      closed = true;
    });
    mcpStatus = 500;
    await expect(connectCxp({ url, token: TOKEN })).rejects.toMatchObject({ code: "internal" });
    expect(closed).toBe(true);
  });

  it("closes the auth transport when the code exchange fails", async () => {
    const url = await startServer([OAUTH_TOKEN]);
    const err = (await connectCxp({ url, authProvider: fakeProvider() }).catch(
      (e) => e
    )) as CxpAuthorizationRequired;
    expect(err).toBeInstanceOf(CxpAuthorizationRequired);
    const close = vi.spyOn(err.transport, "close");
    tokenEndpointStatus = 400;
    await expect(err.finishAuth("bad-code")).rejects.toThrow();
    expect(tokenRequests).toBeGreaterThan(0);
    expect(close).toHaveBeenCalledTimes(1);
  });

  it("throws a clear error when finishAuth is called twice", async () => {
    const url = await startServer([OAUTH_TOKEN]);
    const err = (await connectCxp({ url, authProvider: fakeProvider() }).catch(
      (e) => e
    )) as CxpAuthorizationRequired;
    conn = await err.finishAuth("auth-code");
    const again = await err.finishAuth("auth-code").catch((e) => e);
    expect(again).toBeInstanceOf(CxpError);
    expect(again.code).toBe("invalid_state");
    expect(again.message).toMatch(/already called/);
    expect(tokenRequests).toBe(1);
  });

  it("closes the auth transport after a successful exchange too", async () => {
    const url = await startServer([OAUTH_TOKEN]);
    const err = (await connectCxp({ url, authProvider: fakeProvider() }).catch(
      (e) => e
    )) as CxpAuthorizationRequired;
    const close = vi.spyOn(err.transport, "close");
    conn = await err.finishAuth("auth-code");
    expect(close).toHaveBeenCalledTimes(1);
    expect(await conn.cxp.support.listTickets()).toEqual([{ id: "t1" }]);
  });
});

describe("connectCxp with an OAuth provider", () => {
  it("surfaces the redirect, then finishAuth(code) yields a working connection", async () => {
    const url = await startServer([OAUTH_TOKEN]);
    const authProvider = fakeProvider();

    const err = await connectCxp({ url, authProvider }).catch((e) => e);
    expect(err).toBeInstanceOf(CxpAuthorizationRequired);
    expect(err.code).toBe("authorization_required");
    expect(authProvider.redirects).toHaveLength(1);
    expect(authProvider.redirects[0]!.pathname).toBe("/authorize");

    conn = await (err as CxpAuthorizationRequired).finishAuth("auth-code");
    expect(tokenRequests).toBe(1);
    expect(await conn.cxp.support.listTickets()).toEqual([{ id: "t1" }]);
    expect(seenAuth.at(-1)).toBe(`Bearer ${OAUTH_TOKEN}`);
  });

  it("connects directly when the provider already holds a valid token", async () => {
    const url = await startServer([OAUTH_TOKEN]);
    const authProvider = fakeProvider();
    authProvider.saveTokens({ access_token: OAUTH_TOKEN, token_type: "Bearer" });
    conn = await connectCxp({ url, authProvider });
    expect(await conn.cxp.support.listTickets()).toEqual([{ id: "t1" }]);
    expect(authProvider.redirects).toHaveLength(0);
  });
});

describe("package surface", () => {
  it("no longer exports team support", () => {
    expect("TeamsDomain" in sdk).toBe(false);
    expect("connectCxp" in sdk).toBe(true);
    expect("createCxpTransport" in sdk).toBe(true);
    expect("CxpError" in sdk).toBe(true);
    expect(UnauthorizedError).toBeDefined();
  });
});
