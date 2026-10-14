/**
 * Shared tool-call helpers: structured output parsing, `{items, total}`
 * unwrapping, and typed MCP errors.
 */

import type { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPError } from "@modelcontextprotocol/sdk/client/streamableHttp.js";
import { UnauthorizedError } from "@modelcontextprotocol/sdk/client/auth.js";
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
    options: {
      code?: string | undefined;
      retriable?: boolean | undefined;
      data?: unknown;
      /** The underlying error, kept for debugging. */
      cause?: unknown;
    } = {}
  ) {
    super(message, "cause" in options ? { cause: options.cause } : undefined);
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
    cause: e,
  });
}

/** Longest server text echoed into an error message. */
const MAX_ERROR_TEXT = 500;

function truncate(text: string): string {
  return text.length > MAX_ERROR_TEXT ? `${text.slice(0, MAX_ERROR_TEXT)}...` : text;
}

/**
 * The server's error kind (`invalid_token`, `access_revoked`, ...) when the
 * transport error's message embeds the JSON response body. The MCP SDK's
 * `StreamableHTTPError` exposes only `code` (the HTTP status) and a message
 * that ends with the response text, so the body is the only place it survives.
 */
function serverKind(message: string): string | undefined {
  const m = /"(?:code|error)"\s*:\s*"([a-z][a-z0-9_]{0,63})"/.exec(message);
  return m?.[1];
}

/**
 * Map a transport-level HTTP failure to a `CxpError`:
 *
 * - 401 `unauthenticated` (not retriable); the server's own kind, such as
 *   `invalid_token` or `access_revoked`, is kept when the body carries one
 * - 403 `insufficient_scope` (not retriable)
 * - 500 `internal` (not retriable)
 * - 503 `unavailable` (retriable)
 * - any other HTTP status `transport_error`, retriable for 5xx, with the
 *   status in `data.status`
 *
 * An OAuth `UnauthorizedError` becomes `unauthenticated`. Messages are
 * truncated and the original error is kept as `cause`. Returns `undefined`
 * for anything else so callers rethrow it.
 */
export function fromTransportError(e: unknown): CxpError | undefined {
  if (e instanceof UnauthorizedError) {
    return new CxpError(truncate(e.message), {
      code: "unauthenticated",
      retriable: false,
      cause: e,
    });
  }
  if (!(e instanceof StreamableHTTPError)) return undefined;
  const status = e.code;
  const message = truncate(e.message);
  const data = { status };
  switch (status) {
    case 401:
      return new CxpError(message, {
        code: serverKind(e.message) ?? "unauthenticated",
        retriable: false,
        data,
        cause: e,
      });
    case 403:
      return new CxpError(message, { code: "insufficient_scope", retriable: false, data, cause: e });
    case 500:
      return new CxpError(message, { code: "internal", retriable: false, data, cause: e });
    case 503:
      return new CxpError(message, { code: "unavailable", retriable: true, data, cause: e });
    default:
      return new CxpError(message, {
        code: "transport_error",
        retriable: typeof status === "number" && status >= 500 && status <= 599,
        data,
        cause: e,
      });
  }
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
    throw fromTransportError(e) ?? e;
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
  } catch (cause) {
    throw new CxpError(truncate(text), { code: "invalid_response", cause });
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
