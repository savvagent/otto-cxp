import { afterEach, describe, it, expect } from "vitest";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { CallToolRequestSchema, ListToolsRequestSchema } from "@modelcontextprotocol/sdk/types.js";
import { StreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/streamableHttp.js";
import { connectCxp, type CxpConnection } from "./connect.js";
import * as sdk from "./index.js";

const TOKEN = "good-token";
let server: Server | undefined;
let conn: CxpConnection | undefined;
const seenAuth: Array<string | undefined> = [];

async function startServer(): Promise<string> {
  server = createServer(async (req, res) => {
    seenAuth.push(req.headers.authorization);
    if (req.headers.authorization !== `Bearer ${TOKEN}`) {
      res.writeHead(401, { "content-type": "application/json" });
      res.end(JSON.stringify({ error: "invalid_token" }));
      return;
    }
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
  await new Promise<void>((r) => server!.listen(0, "127.0.0.1", r));
  return `http://127.0.0.1:${(server.address() as AddressInfo).port}/mcp`;
}

afterEach(async () => {
  await conn?.close();
  conn = undefined;
  await new Promise<void>((r) => (server ? server.close(() => r()) : r()));
  server = undefined;
  seenAuth.length = 0;
});

describe("connectCxp", () => {
  it("connects over streamable HTTP with a bearer token and calls a list tool", async () => {
    const url = await startServer();
    conn = await connectCxp({ url, token: TOKEN });
    const tickets = await conn.cxp.support.listTickets();
    expect(tickets).toEqual([{ id: "t1" }]);
    expect(seenAuth.length).toBeGreaterThan(0);
    expect(seenAuth.every((a) => a === `Bearer ${TOKEN}`)).toBe(true);
  });

  it("fails to connect with a wrong token", async () => {
    const url = await startServer();
    await expect(connectCxp({ url, token: "nope" })).rejects.toThrow();
  });

  it("requires exactly one of token or authProvider", async () => {
    await expect(connectCxp({ url: "http://127.0.0.1:1/mcp" })).rejects.toThrow(/required/);
    await expect(
      connectCxp({ url: "http://127.0.0.1:1/mcp", token: "t", authProvider: {} as never })
    ).rejects.toThrow(/not both/);
  });
});

describe("package surface", () => {
  it("no longer exports team support", () => {
    expect("TeamsDomain" in sdk).toBe(false);
    expect("connectCxp" in sdk).toBe(true);
    expect("CxpError" in sdk).toBe(true);
  });
});
