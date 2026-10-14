import { afterEach, describe, it, expect } from "vitest";
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
import { connectCxp, CxpAuthorizationRequired, type CxpConnection } from "./connect.js";
import * as sdk from "./index.js";

const TOKEN = "good-token";
const OAUTH_TOKEN = "oauth-token";
let http: HttpServer | undefined;
let conn: CxpConnection | undefined;
const seenAuth: Array<string | undefined> = [];
let tokenRequests = 0;

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
      return json(200, { access_token: OAUTH_TOKEN, token_type: "Bearer" });
    }

    seenAuth.push(req.headers.authorization);
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
});

describe("connectCxp with a bearer token", () => {
  it("connects over streamable HTTP and calls a list tool", async () => {
    const url = await startServer();
    conn = await connectCxp({ url, token: TOKEN });
    expect(await conn.cxp.support.listTickets()).toEqual([{ id: "t1" }]);
    expect(seenAuth.length).toBeGreaterThan(0);
    expect(seenAuth.every((a) => a === `Bearer ${TOKEN}`)).toBe(true);
  });

  it("rejects a wrong token with a 401 error", async () => {
    const url = await startServer();
    const err = await connectCxp({ url, token: "nope" }).catch((e) => e);
    expect(err).toBeInstanceOf(StreamableHTTPError);
    expect(err.code).toBe(401);
    expect(err).not.toBeInstanceOf(CxpAuthorizationRequired);
  });

  it("requires exactly one of token or authProvider", async () => {
    await expect(connectCxp({ url: "http://127.0.0.1:1/mcp" })).rejects.toThrow(/required/);
    await expect(
      connectCxp({ url: "http://127.0.0.1:1/mcp", token: "t", authProvider: fakeProvider() })
    ).rejects.toThrow(/not both/);
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
