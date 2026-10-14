import { describe, it, expect, vi } from "vitest";
import type { Client } from "@modelcontextprotocol/sdk/client/index.js";
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
    });
  });

  it("raises CxpError for isError results", async () => {
    const c = clientReturning({ isError: true, content: [{ type: "text", text: "bad input" }] });
    await expect(callTool(c, "get_ticket", {})).rejects.toBeInstanceOf(CxpError);
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

  it("rejects an unexpected shape", async () => {
    const c = clientReturning({ structuredContent: { id: "x" } });
    await expect(callList(c, "t", {})).rejects.toBeInstanceOf(CxpError);
  });
});

describe("domains", () => {
  it("getResponses calls get_survey_responses and unwraps items", async () => {
    const c = clientReturning({ structuredContent: { items: [{ id: "r1" }], total: 1 } });
    const out = await new FeedbackDomain(c).getResponses("s1");
    expect(c.callTool).toHaveBeenCalledWith({
      name: "get_survey_responses",
      arguments: { survey_id: "s1" },
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
