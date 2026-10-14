/**
 * Shared tool-call helpers: structured output parsing, `{items, total}`
 * unwrapping, and typed MCP errors.
 */

import type { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { McpError } from "@modelcontextprotocol/sdk/types.js";

/**
 * Error raised for any failed CXP tool call. `code` is the server's stable
 * error code (`data.code` on the MCP error, e.g. `not_found`), when present.
 */
export class CxpError extends Error {
  readonly code: string | undefined;
  readonly retriable: boolean | undefined;
  readonly data: unknown;

  constructor(
    message: string,
    options: { code?: string | undefined; retriable?: boolean | undefined; data?: unknown } = {}
  ) {
    super(message);
    this.name = "CxpError";
    this.code = options.code;
    this.retriable = options.retriable;
    this.data = options.data;
  }
}

function fromMcpError(e: McpError): CxpError {
  const data = e.data as { code?: unknown; retriable?: unknown } | undefined;
  return new CxpError(e.message, {
    code: typeof data?.code === "string" ? data.code : undefined,
    retriable: typeof data?.retriable === "boolean" ? data.retriable : undefined,
    data: e.data,
  });
}

function firstText(content: unknown): string | undefined {
  if (!Array.isArray(content)) return undefined;
  const item = content.find(
    (c): c is { type: string; text: string } =>
      typeof c === "object" && c !== null && typeof (c as { text?: unknown }).text === "string"
  );
  return item?.text;
}

/**
 * Call a tool and return its payload: `structuredContent` when present,
 * otherwise the first text content parsed as JSON.
 */
export async function callTool<T>(
  client: Client,
  name: string,
  args: Record<string, unknown>
): Promise<T> {
  let result;
  try {
    result = await client.callTool({ name, arguments: args });
  } catch (e) {
    if (e instanceof McpError) throw fromMcpError(e);
    throw e;
  }

  const text = firstText(result.content);
  if (result.isError) {
    const sc = result.structuredContent as { code?: unknown; retriable?: unknown } | undefined;
    throw new CxpError(text ?? `Tool ${name} failed`, {
      code: typeof sc?.code === "string" ? sc.code : undefined,
      retriable: typeof sc?.retriable === "boolean" ? sc.retriable : undefined,
      data: result.structuredContent,
    });
  }
  if (result.structuredContent !== undefined && result.structuredContent !== null) {
    return result.structuredContent as T;
  }
  // The server emits every structured result as JSON text content as well, so
  // fall back to it for clients or proxies that drop `structuredContent`.
  if (text === undefined) {
    throw new CxpError(`Tool ${name} returned no content`, { code: "invalid_response" });
  }
  try {
    return JSON.parse(text) as T;
  } catch {
    throw new CxpError(text, { code: "invalid_response" });
  }
}

/** A list tool's output envelope. `total` is omitted when the query has no count. */
export interface ListEnvelope<T> {
  items: T[];
  total?: number;
}

/**
 * Call a list tool and unwrap `{items, total}` to the items array.
 */
export async function callList<T>(
  client: Client,
  name: string,
  args: Record<string, unknown>
): Promise<T[]> {
  const out = await callTool<ListEnvelope<T> | undefined>(client, name, args);
  if (out && !Array.isArray(out) && Array.isArray(out.items)) return out.items;
  throw new CxpError(`Tool ${name} did not return a {items, total?} envelope`, {
    code: "invalid_response",
  });
}
