import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { teamsApi } from "@/lib/api";
import type { Team } from "@/lib/types";

export const teamKeys = {
  all: ["teams"] as const,
  lists: () => [...teamKeys.all, "list"] as const,
  list: (params: Record<string, unknown>) =>
    [...teamKeys.lists(), params] as const,
  details: () => [...teamKeys.all, "detail"] as const,
  detail: (id: string) => [...teamKeys.details(), id] as const,
};

export function useTeams(params?: { limit?: number; offset?: number }) {
  return useQuery({
    queryKey: teamKeys.list(params ?? {}),
    queryFn: () => teamsApi.list(params),
  });
}

export function useTeam(id: string | undefined) {
  return useQuery({
    queryKey: teamKeys.detail(id!),
    queryFn: () => teamsApi.get(id!),
    enabled: !!id,
  });
}

export function useCreateTeam() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (data: { name: string; description?: string; owner?: string }) =>
      teamsApi.create(data),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: teamKeys.lists() });
    },
  });
}

export function useUpdateTeam() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      id,
      data,
    }: {
      id: string;
      data: { name?: string; description?: string; owner?: string };
    }) => teamsApi.update(id, data),
    onSuccess: (updatedTeam: Team) => {
      queryClient.invalidateQueries({ queryKey: teamKeys.lists() });
      queryClient.invalidateQueries({
        queryKey: teamKeys.detail(updatedTeam.id),
      });
    },
  });
}

export function useDeleteTeam() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => teamsApi.delete(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: teamKeys.lists() });
    },
  });
}
