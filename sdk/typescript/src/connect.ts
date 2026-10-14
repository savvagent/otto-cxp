/**
 * Connect to a CXP resource server over streamable HTTP.
 */

import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";
import type { OAuthClientProvider } from "@modelcontextprotocol/sdk/client/auth.js";
import { UnauthorizedError } from "@modelcontextprotocol/sdk/client/auth.js";
import type { Transport } from "@modelcontextprotocol/sdk/shared/transport.js";
import { CxpClient } from "./client.js";
import { CxpError } from "./call.js";

export interface CxpTransportOptions {
  /** MCP endpoint, e.g. `https://cxp.example.com/mcp`. */
  url: string | URL;
  /** Static bearer token (an otto platform access token for this resource). */
  token?: string | undefined;
  /**
   * MCP OAuth client provider. When the server answers 401 the transport runs
   * the OAuth flow through this provider. Mutually exclusive with `token`.
   */
  authProvider?: OAuthClientProvider | undefined;
}

export interface ConnectCxpOptions extends CxpTransportOptions {
  /** Client name reported to the server. Defaults to `cxp-sdk`. */
  name?: string | undefined;
  /** Client version reported to the server. Defaults to `0.1.0`. */
  version?: string | undefined;
}

export interface CxpConnection {
  /** The typed SDK client. */
  cxp: CxpClient;
  /** The underlying connected MCP client. */
  client: Client;
  /** The transport in use. */
  transport: StreamableHTTPClientTransport;
  /** Close the connection. */
  close: () => Promise<void>;
}

/**
 * Thrown by `connectCxp` when the OAuth flow needs the user to authorize in a
 * browser (the `authProvider` has already been asked to redirect). After the
 * redirect back, call `finishAuth(code)` with the authorization code: it
 * exchanges the code and returns the connected `CxpConnection`.
 */
export class CxpAuthorizationRequired extends CxpError {
  /** The transport that triggered the flow (its `finishAuth` is used internally). */
  readonly transport: StreamableHTTPClientTransport;
  readonly finishAuth: (authorizationCode: string) => Promise<CxpConnection>;

  constructor(
    options: ConnectCxpOptions,
    transport: StreamableHTTPClientTransport
  ) {
    super("Authorization required: complete the OAuth redirect, then call finishAuth(code)", {
      code: "authorization_required",
    });
    this.name = "CxpAuthorizationRequired";
    this.transport = transport;
    this.finishAuth = async (authorizationCode) => {
      await transport.finishAuth(authorizationCode);
      // The provider now holds the tokens; connect on a fresh transport.
      return connectCxp(options);
    };
  }
}

/**
 * Build the streamable HTTP transport for `url`, authenticating with either a
 * bearer `token` or an MCP `authProvider`. Use this directly when you manage
 * the MCP `Client` yourself (then wrap it with `new CxpClient({ client })`).
 */
export function createCxpTransport(
  options: CxpTransportOptions
): StreamableHTTPClientTransport {
  const { url, token, authProvider } = options;
  if (token && authProvider) {
    throw new Error("createCxpTransport: pass either `token` or `authProvider`, not both");
  }
  if (!token && !authProvider) {
    throw new Error("createCxpTransport: a `token` or an `authProvider` is required");
  }
  return new StreamableHTTPClientTransport(
    typeof url === "string" ? new URL(url) : url,
    authProvider
      ? { authProvider }
      : { requestInit: { headers: { Authorization: `Bearer ${token}` } } }
  );
}

/**
 * Connect to `url` over streamable HTTP and return a ready `CxpClient`.
 *
 * With a static `token` this just connects. With an `authProvider`, if the
 * provider needs a browser redirect, this throws `CxpAuthorizationRequired`;
 * after the redirect call its `finishAuth(code)` to get the connection.
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
  let transport: StreamableHTTPClientTransport;
  try {
    transport = createCxpTransport(options);
  } catch (e) {
    throw new Error((e as Error).message.replace("createCxpTransport", "connectCxp"));
  }
  const client = new Client(
    { name: options.name ?? "cxp-sdk", version: options.version ?? "0.1.0" },
    { capabilities: {} }
  );
  try {
    // The transport's `sessionId?: string` clashes with exactOptionalPropertyTypes.
    await client.connect(transport as Transport);
  } catch (e) {
    if (e instanceof UnauthorizedError && options.authProvider) {
      throw new CxpAuthorizationRequired(options, transport);
    }
    throw e;
  }

  return {
    cxp: new CxpClient({ client }),
    client,
    transport,
    close: () => client.close(),
  };
}
