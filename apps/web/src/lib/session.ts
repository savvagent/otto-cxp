// The console's sign-in, which the server runs: `/auth/login` sends the browser
// to the otto platform and back with an HttpOnly session cookie the console
// never sees; `/auth/session` says whether it is signed in, and as whom;
// `/auth/logout` ends it.

/** What `GET /auth/session` answers. */
export interface Session {
  signedIn: boolean;
  /** False while the server has no console client configured. */
  loginEnabled: boolean;
  userId?: string;
  orgId?: string;
  role?: "owner" | "admin" | "member";
  scopes?: string[];
}

/** Why the last sign-in failed, from the `login_error` the server redirects with. */
export const LOGIN_ERRORS: Record<string, string> = {
  access_denied: "Sign-in was cancelled at the otto platform.",
  invalid_state: "That sign-in link expired or was started in another tab. Try again.",
  platform_unavailable: "The otto platform could not be reached. Try again shortly.",
  unavailable: "otto-cxp could not finish signing you in. Try again shortly.",
  failed: "Sign-in failed. Try again, or ask an admin of your organization for access.",
};

export function loginErrorMessage(search: string): string | null {
  const code = new URLSearchParams(search).get("login_error");
  if (code === null) return null;
  return LOGIN_ERRORS[code] ?? LOGIN_ERRORS.failed;
}

export async function fetchSession(): Promise<Session> {
  const response = await fetch("/auth/session", { credentials: "same-origin" });
  if (!response.ok) {
    throw new Error(`Could not check the sign-in (${response.status})`);
  }
  return response.json();
}

/** Where to send the browser to sign in, returning to `next` afterwards. */
export function loginUrl(next: string): string {
  return `/auth/login?next=${encodeURIComponent(next)}`;
}

/** Send the browser through the platform's sign-in, back to where it is now. */
export function signIn(): void {
  const { pathname, search } = window.location;
  window.location.assign(loginUrl(pathname + search));
}

export async function signOut(): Promise<void> {
  await fetch("/auth/logout", { method: "POST", credentials: "same-origin" });
  window.location.assign("/");
}
