import { useQuery } from "@tanstack/react-query";
import { fetchSession, sessionKeys } from "@/lib/session";

export { sessionKeys };

/** Whether this browser is signed in to the console, and as whom. */
export function useSession() {
  return useQuery({
    queryKey: sessionKeys.all,
    queryFn: fetchSession,
    staleTime: 60_000,
  });
}
