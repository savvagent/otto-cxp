import { useQuery } from "@tanstack/react-query";
import { meApi } from "@/lib/api";

export const meKeys = {
  all: ["me"] as const,
  teams: () => [...meKeys.all, "teams"] as const,
};

export function useMyTeams() {
  return useQuery({
    queryKey: meKeys.teams(),
    queryFn: () => meApi.teams(),
  });
}
