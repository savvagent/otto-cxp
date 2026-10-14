/**
 * Connect to a CXP resource server over streamable HTTP.
 */

import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";
import type { OAuthClientProvider } from "@modelcontextprotocol/sdk/client/auth.js";
import { UnauthorizedError } from "@modelcontextprotocol/sdk/client/auth.js";
import type { Transport } from "@modelcontextprotocol/sdk/shared/transport.js";
import { CxpClient } from "./client.js";
import { CxpError, fromTransportError } from "./call.js";

/**
 * Transport options: exactly one of `token` or `authProvider`. The union makes
 * passing both (or neither) a compile error; the runtime check remains for
 * JavaScript callers.
 */
export type CxpTransportOptions = {
  /** MCP endpoint, e.g. `https://cxp.example.com/mcp`. */
  url: string | URL;
} & (
  | {
      /** Static bearer token (an otto platform access token for this resource). */
      token: string;
      authProvider?: never;
    }
  | {
      /**
       * MCP OAuth client provider. When the server answers 401 the transport
       * runs the OAuth flow through this provider.
       */
      authProvider: OAuthClientProvider;
      token?: never;
    }
);

export type ConnectCxpOptions = CxpTransportOptions & {
  /** Client name reported to the server. Defaults to `cxp-sdk`. */
  name?: string | undefined;
  /** Client version reported to the server. Defaults to `0.1.0`. */
  version?: string | undefined;
};

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
    let used = false;
    this.finishAuth = async (authorizationCode) => {
      if (used) {
        throw new CxpError(
          "finishAuth was already called on this CxpAuthorizationRequired; " +
            "call connectCxp again to start a new authorization",
          { code: "invalid_state" }
        );
      }
      used = true;
      try {
        await transport.finishAuth(authorizationCode);
        // The provider now holds the tokens; connect on a fresh transport.
        return await connectCxp(options);
      } finally {
        // This transport only served the auth exchange. Release it whether the
        // exchange succeeded (the new connection has its own) or failed.
        await transport.close().catch(() => undefined);
      }
    };
  }
}

const LOOPBACK_HOSTS = new Set(["localhost", "127.0.0.1", "[::1]", "::1"]);

function invalid(message: string, cause?: unknown): CxpError {
  return new CxpError(message, {
    code: "invalid_argument",
    ...(cause === undefined ? {} : { cause }),
  });
}

/**
 * A static bearer token is a long-lived secret: refuse to send it in clear
 * text. Plain `http:` is only allowed to a loopback host (local development).
 */
function assertTokenSafeUrl(url: URL, caller: string): void {
  if (url.protocol === "https:") return;
  if (url.protocol === "http:" && LOOPBACK_HOSTS.has(url.hostname)) return;
  throw invalid(
    `${caller}: refusing to send a static \`token\` to ${url.protocol}//${url.host}; ` +
      "use an https URL (plain http is only allowed for localhost, 127.0.0.1 and ::1)"
  );
}

/**
 * Build the streamable HTTP transport for `url`, authenticating with either a
 * bearer `token` or an MCP `authProvider`. Use this directly when you manage
 * the MCP `Client` yourself (then wrap it with `new CxpClient({ client })`).
 */
export function createCxpTransport(
  options: CxpTransportOptions
): StreamableHTTPClientTransport {
  return buildTransport(options, "createCxpTransport");
}

function buildTransport(
  options: CxpTransportOptions,
  caller: string
): StreamableHTTPClientTransport {
  // Widened: JS callers can pass anything the union forbids.
  const { url, token, authProvider } = options as {
    url: string | URL;
    token?: string | undefined;
    authProvider?: OAuthClientProvider | undefined;
  };
  if (token && authProvider) {
    throw invalid(`${caller}: pass either \`token\` or \`authProvider\`, not both`);
  }
  if (!token && !authProvider) {
    throw invalid(`${caller}: a \`token\` or an \`authProvider\` is required`);
  }
  let target: URL;
  try {
    target = typeof url === "string" ? new URL(url) : url;
  } catch (cause) {
    throw invalid(`${caller}: \`url\` is not a valid URL`, cause);
  }
  if (token) {
    if (/[\r\n]/.test(token)) {
      throw invalid(`${caller}: \`token\` must not contain CR or LF characters`);
    }
    assertTokenSafeUrl(target, caller);
  }
  return new StreamableHTTPClientTransport(
    target,
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
  const transport = buildTransport(options, "connectCxp");
  const client = new Client(
    { name: options.name ?? "cxp-sdk", version: options.version ?? "0.1.0" },
    { capabilities: {} }
  );
  try {
    // The transport's `sessionId?: string` clashes with exactOptionalPropertyTypes.
    await client.connect(transport as Transport);
  } catch (e) {
    // Release the unused client (and its transport) before surfacing the error.
    await client.close().catch(() => undefined);
    if (e instanceof UnauthorizedError && options.authProvider) {
      throw new CxpAuthorizationRequired(options, transport);
    }
    // Static-token path: surface a server 401/503 as a typed CxpError.
    throw (options.authProvider ? undefined : fromTransportError(e)) ?? e;
  }

  return {
    cxp: new CxpClient({ client }),
    client,
    transport,
    close: () => client.close(),
  };
}
