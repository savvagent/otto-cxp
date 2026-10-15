/** Where the console is served. The server answers `/` and every other path itself. */
export const CONSOLE_BASE = "/console/";

/** The app shell the service worker answers the console's navigations with. */
export const APP_SHELL = `${CONSOLE_BASE}index.html`;

// The only navigations the service worker may answer from its cache: the
// console's own routes. Everything else is the server's.
export const CONSOLE_PATHS: RegExp[] = [/^\/console(?:\/|$|\?)/];

// What the service worker must never answer from its cache, even were the
// allowlist ever widened. Navigations to these are the server's own: the
// sign-in round trip through the platform (`/auth`), the REST API and MCP
// endpoint, discovery documents, and the platform's webhooks. Answering
// `/auth/login` with the cached `index.html` would leave a person unable to
// sign in.
export const SERVER_ONLY_PATHS: RegExp[] = [
  /^\/auth(?:\/|$|\?)/,
  /^\/api(?:\/|$|\?)/,
  /^\/mcp(?:\/|$|\?)/,
  /^\/\.well-known(?:\/|$|\?)/,
  /^\/platform(?:\/|$|\?)/,
];
