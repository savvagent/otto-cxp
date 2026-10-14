/**
 * Connect to a CXP resource server over streamable HTTP.
 */

import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";
import type { OAuthClientProvider } from "@modelcontextprotocol/sdk/client/auth.js";
import type { Transport } from "@modelcontextprotocol/sdk/shared/transport.js";
import { CxpClient } from "./client.js";

export interface ConnectCxpOptions {
  /** MCP endpoint, e.g. `https://cxp.example.com/mcp`. */
  url: string | URL;
  /** Static bearer token (an otto platform access token for this resource). */
  token?: string;
  /**
   * MCP OAuth client provider. When the server answers 401 the transport runs
   * the OAuth flow through this provider. Mutually exclusive with `token`.
   */
  authProvider?: OAuthClientProvider;
  /** Client name reported to the server. Defaults to `cxp-sdk`. */
  name?: string;
  /** Client version reported to the server. Defaults to `0.1.0`. */
  version?: string;
}

export interface CxpConnection {
  /** The typed SDK client. */
  cxp: CxpClient;
  /** The underlying connected MCP client. */
  client: Client;
  /** The transport; call `finishAuth(code)` on it after an OAuth redirect. */
  transport: StreamableHTTPClientTransport;
  /** Close the connection. */
  close: () => Promise<void>;
}

/**
 * Connect to `url` over streamable HTTP, authenticating with either a static
 * bearer `token` or an MCP `authProvider`, and return a ready `CxpClient`.
 *
 * @example
 * ```typescript
 * import { connectCxp } from "@cxp/sdk";
 *
 * const { cxp, close } = await connectCxp({
 *   url: "https://cxp.example.com/mcp",
 *   token: process.env.CXP_TOKEN,
 * });
 * const tickets = await cxp.support.listTickets({ status: "open" });
 * await close();
 * ```
 */
export async function connectCxp(options: ConnectCxpOptions): Promise<CxpConnection> {
  const { url, token, authProvider } = options;
  if (token && authProvider) {
    throw new Error("connectCxp: pass either `token` or `authProvider`, not both");
  }
  if (!token && !authProvider) {
    throw new Error("connectCxp: a `token` or an `authProvider` is required");
  }

  const transport = new StreamableHTTPClientTransport(
    typeof url === "string" ? new URL(url) : url,
    authProvider
      ? { authProvider }
      : { requestInit: { headers: { Authorization: `Bearer ${token}` } } }
  );
  const client = new Client(
    { name: options.name ?? "cxp-sdk", version: options.version ?? "0.1.0" },
    { capabilities: {} }
  );
  // The transport's `sessionId?: string` clashes with exactOptionalPropertyTypes.
  await client.connect(transport as Transport);

  return {
    cxp: new CxpClient({ client }),
    client,
    transport,
    close: () => client.close(),
  };
}
