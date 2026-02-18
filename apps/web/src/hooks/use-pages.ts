import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { pagesApi } from "@/lib/api";
import type { Page } from "@/lib/types";

type PageListParams = {
  limit?: number;
  offset?: number;
  application_id?: string;
};

export const pageKeys = {
  all: ["pages"] as const,
  lists: () => [...pageKeys.all, "list"] as const,
  list: (params: Record<string, unknown>) =>
    [...pageKeys.lists(), params] as const,
  details: () => [...pageKeys.all, "detail"] as const,
  detail: (id: string) => [...pageKeys.details(), id] as const,
};

export function usePages(params?: PageListParams) {
  return useQuery({
    queryKey: pageKeys.list(params ?? {}),
    queryFn: () => pagesApi.list(params),
  });
}

export function usePage(id: string | undefined) {
  return useQuery({
    queryKey: pageKeys.detail(id!),
    queryFn: () => pagesApi.get(id!),
    enabled: !!id,
  });
}

export function useCreatePage() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (data: {
      application_id?: string;
      name: string;
      path?: string;
      description?: string;
    }) => pagesApi.create(data),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: pageKeys.lists() });
    },
  });
}

export function useUpdatePage() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      id,
      data,
    }: {
      id: string;
      data: { name?: string; path?: string; description?: string };
    }) => pagesApi.update(id, data),
    onSuccess: (updatedPage: Page) => {
      queryClient.invalidateQueries({ queryKey: pageKeys.lists() });
      queryClient.invalidateQueries({
        queryKey: pageKeys.detail(updatedPage.id),
      });
    },
  });
}

export function useDeletePage() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => pagesApi.delete(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: pageKeys.lists() });
    },
  });
}
