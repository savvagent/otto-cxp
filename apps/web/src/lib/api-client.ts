import { ApiError } from "./api-error";
import { queryClient } from "./query-client";
import { markSignedOut } from "./session";

export { ApiError };

const BASE_URL = "/api/v1";

export async function apiFetch<T>(
  path: string,
  options?: RequestInit,
): Promise<T> {
  // Merge through Headers so caller headers of any HeadersInit shape are kept
  // and spreading `options` cannot replace the merged set.
  const headers = new Headers(options?.headers);
  if (!headers.has("Content-Type")) {
    headers.set("Content-Type", "application/json");
  }
  const response = await fetch(`${BASE_URL}${path}`, {
    ...options,
    headers,
    // The session cookie the server set at sign-in. Same origin only: the
    // server refuses a cookie-authenticated write from anywhere else.
    credentials: "same-origin",
  });

  if (response.status === 401) {
    // Signed out, or the session ended (expired, signed out elsewhere, or
    // access revoked). Never navigate from here: `/auth/session` can still
    // say signed in while `/api` refuses (a local revocation), and a redirect
    // through a silent platform sign-in would loop. The console shows its
    // sign-in screen instead, and the person decides.
    markSignedOut(queryClient);
  }
  if (!response.ok) {
    const body = await response.json().catch(() => ({}));
    throw new ApiError(
      response.status,
      body.error_description || body.error || `Request failed: ${response.statusText}`,
    );
  }

  return response.json();
}
