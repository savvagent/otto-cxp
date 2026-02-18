import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { experiencesApi } from "@/lib/api";
import type { Experience } from "@/lib/types";

export const experienceKeys = {
  all: ["experiences"] as const,
  lists: () => [...experienceKeys.all, "list"] as const,
  list: (params: Record<string, unknown>) =>
    [...experienceKeys.lists(), params] as const,
  details: () => [...experienceKeys.all, "detail"] as const,
  detail: (id: string) => [...experienceKeys.details(), id] as const,
};

export function useExperiences(params?: { limit?: number; offset?: number }) {
  return useQuery({
    queryKey: experienceKeys.list(params ?? {}),
    queryFn: () => experiencesApi.list(params),
  });
}

export function useExperience(id: string | undefined) {
  return useQuery({
    queryKey: experienceKeys.detail(id!),
    queryFn: () => experiencesApi.get(id!),
    enabled: !!id,
  });
}

export function useCreateExperience() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (data: { name: string; description?: string }) =>
      experiencesApi.create(data),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: experienceKeys.lists() });
    },
  });
}

export function useUpdateExperience() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      id,
      data,
    }: {
      id: string;
      data: { name?: string; description?: string };
    }) => experiencesApi.update(id, data),
    onSuccess: (updatedExperience: Experience) => {
      queryClient.invalidateQueries({ queryKey: experienceKeys.lists() });
      queryClient.invalidateQueries({
        queryKey: experienceKeys.detail(updatedExperience.id),
      });
    },
  });
}

export function useDeleteExperience() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => experiencesApi.delete(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: experienceKeys.lists() });
    },
  });
}
