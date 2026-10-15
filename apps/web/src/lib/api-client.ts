import { signIn } from "./session";

const BASE_URL = "/api/v1";

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

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
    // access revoked): sign in again and come back here.
    signIn();
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
