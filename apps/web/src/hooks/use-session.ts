import { useQuery } from "@tanstack/react-query";
import { fetchSession } from "@/lib/session";

export const sessionKeys = {
  all: ["session"] as const,
};

/** Whether this browser is signed in to the console, and as whom. */
export function useSession() {
  return useQuery({
    queryKey: sessionKeys.all,
    queryFn: fetchSession,
    staleTime: 60_000,
  });
}
