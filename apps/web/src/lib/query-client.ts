import { QueryClient } from "@tanstack/react-query";
import { ApiError } from "./api-error";

/** How many times a failed query is retried, unless {@link shouldRetry} says no. */
export const MAX_RETRIES = 1;

/**
 * Retry a failed query, except when retrying cannot help: a `401` (signed
 * out, or the session ended) or a `403` (refused) will answer the same again.
 */
export function shouldRetry(failureCount: number, error: unknown): boolean {
  if (error instanceof ApiError && (error.status === 401 || error.status === 403)) {
    return false;
  }
  return failureCount < MAX_RETRIES;
}

/** The console's one query client, shared with `apiFetch` so a `401` can sign the console out. */
export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 30_000,
      retry: shouldRetry,
    },
  },
});
