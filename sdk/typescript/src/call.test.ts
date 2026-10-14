import { describe, it, expect, vi } from "vitest";
import type { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPError } from "@modelcontextprotocol/sdk/client/streamableHttp.js";
import { UnauthorizedError } from "@modelcontextprotocol/sdk/client/auth.js";
import { McpError } from "@modelcontextprotocol/sdk/types.js";
import { callList, callTool, CxpError } from "./call.js";
import { FeedbackDomain } from "./domains/feedback.js";
import { CdpDomain } from "./domains/cdp.js";
import { SupportDomain } from "./domains/support.js";

function clientReturning(value: unknown): Client {
  return { callTool: vi.fn().mockResolvedValue(value) } as unknown as Client;
}

describe("callTool", () => {
  it("prefers structuredContent over text content", async () => {
    const c = clientReturning({
      structuredContent: { id: "a" },
      content: [{ type: "text", text: JSON.stringify({ id: "text" }) }],
    });
    expect(await callTool(c, "get_ticket", { id: "a" })).toEqual({ id: "a" });
  });

  it("falls back to parsing the first text content", async () => {
    const c = clientReturning({ content: [{ type: "text", text: '{"id":"t"}' }] });
    expect(await callTool(c, "get_ticket", {})).toEqual({ id: "t" });
  });

  it("raises CxpError with the text when it is not JSON", async () => {
    const c = clientReturning({ content: [{ type: "text", text: "Error: boom" }] });
    await expect(callTool(c, "get_ticket", {})).rejects.toMatchObject({
      name: "CxpError",
      message: "Error: boom",
      code: "invalid_response",
    });
  });

  it("truncates long non-JSON text and keeps the parse error as cause", async () => {
    const c = clientReturning({ content: [{ type: "text", text: "x".repeat(2000) }] });
    const err = await callTool(c, "t", {}).catch((e) => e);
    expect(err).toBeInstanceOf(CxpError);
    expect(err.message.length).toBeLessThan(600);
    expect(err.code).toBe("invalid_response");
    expect(err.cause).toBeInstanceOf(SyntaxError);
  });

  it("keeps the McpError as cause", async () => {
    const mcp = new McpError(-32602, "bad params", { code: "invalid_argument" });
    const c = { callTool: vi.fn().mockRejectedValue(mcp) } as unknown as Client;
    const err = await callTool(c, "t", {}).catch((e) => e);
    expect(err.code).toBe("invalid_argument");
    expect(err.cause).toBe(mcp);
  });

  it.each([
    [401, "unauthenticated", false],
    [403, "insufficient_scope", false],
    [500, "internal", false],
    [503, "unavailable", true],
    [502, "transport_error", true],
    [404, "transport_error", false],
    [-1, "transport_error", false],
  ])("wraps a transport %i as %s", async (status, code, retriable) => {
    const http = new StreamableHTTPError(status, "http fail");
    const c = { callTool: vi.fn().mockRejectedValue(http) } as unknown as Client;
    const err = await callTool(c, "t", {}).catch((e) => e);
    expect(err).toBeInstanceOf(CxpError);
    expect(err.code).toBe(code);
    expect(err.retriable).toBe(retriable);
    expect(err.data).toEqual({ status });
    expect(err.cause).toBe(http);
  });

  it.each(["invalid_token", "access_revoked"])(
    "keeps the server's %s kind on a 401",
    async (kind) => {
      const http = new StreamableHTTPError(401, `Error POSTing to endpoint: {"error":"${kind}"}`);
      const c = { callTool: vi.fn().mockRejectedValue(http) } as unknown as Client;
      const err = await callTool(c, "t", {}).catch((e) => e);
      expect(err.code).toBe(kind);
      expect(err.retriable).toBe(false);
    }
  );

  it("truncates long transport messages", async () => {
    const http = new StreamableHTTPError(502, "y".repeat(2000));
    const c = { callTool: vi.fn().mockRejectedValue(http) } as unknown as Client;
    const err = await callTool(c, "t", {}).catch((e) => e);
    expect(err.message.length).toBeLessThanOrEqual(503);
    expect(err.cause).toBe(http);
  });

  it("wraps an OAuth UnauthorizedError as unauthenticated", async () => {
    const unauth = new UnauthorizedError("expired");
    const c = { callTool: vi.fn().mockRejectedValue(unauth) } as unknown as Client;
    const err = await callTool(c, "t", {}).catch((e) => e);
    expect(err).toBeInstanceOf(CxpError);
    expect(err.code).toBe("unauthenticated");
    expect(err.retriable).toBe(false);
    expect(err.cause).toBe(unauth);
  });

  it("rethrows non-HTTP errors as-is", async () => {
    const boom = new TypeError("fetch failed");
    const c = { callTool: vi.fn().mockRejectedValue(boom) } as unknown as Client;
    await expect(callTool(c, "t", {})).rejects.toBe(boom);
  });

  it("raises CxpError for isError results", async () => {
    const c = clientReturning({ isError: true, content: [{ type: "text", text: "bad input" }] });
    await expect(callTool(c, "get_ticket", {})).rejects.toBeInstanceOf(CxpError);
  });

  it("carries a code from structured content on isError results", async () => {
    const c = clientReturning({
      isError: true,
      structuredContent: { code: "not_found", retriable: false },
      content: [{ type: "text", text: "missing" }],
    });
    await expect(callTool(c, "t", {})).rejects.toMatchObject({ code: "not_found", retriable: false });
  });

  it("surfaces MCP errors as CxpError carrying data.code", async () => {
    const c = {
      callTool: vi
        .fn()
        .mockRejectedValue(new McpError(-32602, "not found", { code: "not_found", retriable: false })),
    } as unknown as Client;
    const err = await callTool(c, "get_ticket", {}).catch((e) => e);
    expect(err).toBeInstanceOf(CxpError);
    expect(err.code).toBe("not_found");
    expect(err.retriable).toBe(false);
    expect(err.message).toContain("not found");
  });

  it("leaves non-MCP errors untouched", async () => {
    const boom = new Error("network down");
    const c = { callTool: vi.fn().mockRejectedValue(boom) } as unknown as Client;
    await expect(callTool(c, "x", {})).rejects.toBe(boom);
  });
});

describe("callList", () => {
  it("unwraps items from structuredContent, with or without total", async () => {
    const withTotal = clientReturning({ structuredContent: { items: [1, 2], total: 9 } });
    expect(await callList(withTotal, "t", {})).toEqual([1, 2]);
    const noTotal = clientReturning({ structuredContent: { items: [3] } });
    expect(await callList(noTotal, "t", {})).toEqual([3]);
  });

  it("unwraps items from JSON text content", async () => {
    const c = clientReturning({ content: [{ type: "text", text: '{"items":[{"id":"x"}],"total":1}' }] });
    expect(await callList(c, "t", {})).toEqual([{ id: "x" }]);
  });

  it("rejects an unexpected shape with invalid_response", async () => {
    const c = clientReturning({ structuredContent: { id: "x" } });
    await expect(callList(c, "t", {})).rejects.toMatchObject({ code: "invalid_response" });
  });

  it("rejects a bare array (contract is the envelope only)", async () => {
    const c = clientReturning({ structuredContent: [{ id: "x" }] });
    await expect(callList(c, "t", {})).rejects.toMatchObject({ code: "invalid_response" });
    const t = clientReturning({ content: [{ type: "text", text: '[{"id":"x"}]' }] });
    await expect(callList(t, "t", {})).rejects.toMatchObject({ code: "invalid_response" });
  });
});

describe("domains", () => {
  it("getResponses calls get_survey_responses and unwraps items", async () => {
    const c = clientReturning({ structuredContent: { items: [{ id: "r1" }], total: 1 } });
    const out = await new FeedbackDomain(c).getResponses("s1");
    expect(c.callTool).toHaveBeenCalledWith({
      name: "get_survey_responses",
      arguments: { id: "s1" },
    });
    expect(out).toEqual([{ id: "r1" }]);
  });

  it("list methods across domains unwrap the envelope", async () => {
    const env = { structuredContent: { items: [{ id: "i" }] } };
    expect(await new SupportDomain(clientReturning(env)).listTickets()).toEqual([{ id: "i" }]);
    expect(await new CdpDomain(clientReturning(env)).searchCustomers("q")).toEqual([{ id: "i" }]);
    expect(await new FeedbackDomain(clientReturning(env)).getFeedback()).toEqual([{ id: "i" }]);
  });

  it("single-object tools return the structured object", async () => {
    const c = clientReturning({ structuredContent: { id: "t1" } });
    expect(await new SupportDomain(c).getTicket("t1")).toEqual({ id: "t1" });
  });
});
