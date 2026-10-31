// The console's sign-in, which the server runs: `/auth/login` sends the browser
// to the otto platform and back with an HttpOnly session cookie the console
// never sees; `/auth/session` says whether it is signed in, and as whom;
// `/auth/logout` ends it.
import type { QueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { z } from "zod";

/** Where the console lives. The server accepts a sign-in `next` only under it. */
export const CONSOLE_HOME = "/console/";

export const sessionKeys = {
  all: ["session"] as const,
};

const signedOutSchema = z.object({
  signedIn: z.literal(false),
  /** False while the server has no console client configured. */
  loginEnabled: z.boolean(),
});

const signedInSchema = z.object({
  signedIn: z.literal(true),
  loginEnabled: z.literal(true),
  userId: z.string().uuid(),
  orgId: z.string().uuid(),
  role: z.enum(["owner", "admin", "member"]),
  scopes: z.array(z.string()),
});

/** What `GET /auth/session` answers. */
export const sessionSchema = z.discriminatedUnion("signedIn", [signedOutSchema, signedInSchema]);

export type Session = z.infer<typeof sessionSchema>;
export type SignedOut = z.infer<typeof signedOutSchema>;

/** Why the last sign-in failed, from the `login_error` the server redirects with. */
export const LOGIN_ERRORS: Record<string, string> = {
  access_denied: "Sign-in was cancelled at the Savvagent platform.",
  invalid_state: "That sign-in link expired or was started in another tab. Try again.",
  platform_unavailable: "The Savvagent platform could not be reached. Try again shortly.",
  unavailable: "Savvagent CXP could not finish signing you in. Try again shortly.",
  failed: "Sign-in failed. Try again, or ask an admin of your organization for access.",
};

export function loginErrorMessage(search: string): string | null {
  const code = new URLSearchParams(search).get("login_error");
  if (code === null) return null;
  return LOGIN_ERRORS[code] ?? LOGIN_ERRORS.failed;
}

/** The session, or a thrown error for a non-2xx answer or an unexpected body. */
export async function fetchSession(): Promise<Session> {
  const response = await fetch("/auth/session", { credentials: "same-origin" });
  if (!response.ok) {
    throw new Error(`Could not check the sign-in (${response.status})`);
  }
  const parsed = sessionSchema.safeParse(await response.json());
  if (!parsed.success) {
    throw new Error("The server's sign-in answer was not understood");
  }
  return parsed.data;
}

/**
 * Show the sign-in screen: the API refused this browser's session. Keeps
 * whether sign-in is enabled from what the server last said; with nothing
 * known yet, asks the server again.
 */
export function markSignedOut(client: QueryClient): void {
  const previous = client.getQueryData<Session>(sessionKeys.all);
  if (previous === undefined) {
    void client.invalidateQueries({ queryKey: sessionKeys.all });
    return;
  }
  const signedOut: SignedOut = { signedIn: false, loginEnabled: previous.loginEnabled };
  client.setQueryData<Session>(sessionKeys.all, signedOut);
}

/** Where to send the browser to sign in, returning to `next` (a full path under `/console/`) afterwards. */
export function loginUrl(next: string): string {
  return `/auth/login?next=${encodeURIComponent(next)}`;
}

/**
 * Where to come back to after signing in: the browser's own path (which,
 * unlike the router's, includes `/console/`), or the console's home after a
 * failed attempt, so its error marker is not carried along.
 */
export function returnPath(location: Pick<Location, "pathname" | "search">): string {
  if (loginErrorMessage(location.search) !== null) return CONSOLE_HOME;
  if (!location.pathname.startsWith(CONSOLE_HOME)) return CONSOLE_HOME;
  return location.pathname + location.search;
}

/**
 * Sign out at the server, then leave. A refusal or an unreachable server is
 * shown, and the console stays as it is: the person is still signed in.
 */
export async function signOut(): Promise<void> {
  let response: Response;
  try {
    response = await fetch("/auth/logout", { method: "POST", credentials: "same-origin" });
  } catch {
    toast.error("Could not reach Savvagent CXP to sign out. Try again.");
    return;
  }
  if (!response.ok) {
    const body = await response.json().catch(() => ({}));
    toast.error(
      typeof body.error === "string" ? body.error : `Could not sign out (${response.status}). Try again.`,
    );
    return;
  }
  window.location.assign(CONSOLE_HOME);
}
