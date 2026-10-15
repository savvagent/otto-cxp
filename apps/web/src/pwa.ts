// What the service worker must never answer from its cache. Navigations to
// these are the server's own: the sign-in round trip through the platform
// (`/auth`), the REST API and MCP endpoint, discovery documents, and the
// platform's webhooks. Answering `/auth/login` with the cached `index.html`
// would leave a person unable to sign in.
export const SERVER_ONLY_PATHS: RegExp[] = [
  /^\/auth(?:\/|$|\?)/,
  /^\/api(?:\/|$|\?)/,
  /^\/mcp(?:\/|$|\?)/,
  /^\/\.well-known(?:\/|$|\?)/,
  /^\/platform(?:\/|$|\?)/,
];
