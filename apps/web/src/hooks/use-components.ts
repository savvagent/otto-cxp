import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { componentsApi } from "@/lib/api";
import type { Component } from "@/lib/types";

type ComponentListParams = {
  limit?: number;
  offset?: number;
  page_id?: string;
};

export const componentKeys = {
  all: ["components"] as const,
  lists: () => [...componentKeys.all, "list"] as const,
  list: (params: Record<string, unknown>) =>
    [...componentKeys.lists(), params] as const,
  details: () => [...componentKeys.all, "detail"] as const,
  detail: (id: string) => [...componentKeys.details(), id] as const,
};

export function useComponents(params?: ComponentListParams) {
  return useQuery({
    queryKey: componentKeys.list(params ?? {}),
    queryFn: () => componentsApi.list(params),
  });
}

export function useComponent(id: string | undefined) {
  return useQuery({
    queryKey: componentKeys.detail(id!),
    queryFn: () => componentsApi.get(id!),
    enabled: !!id,
  });
}

export function useCreateComponent() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (data: {
      page_id: string;
      name: string;
      component_type?: string;
      description?: string;
    }) => componentsApi.create(data),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: componentKeys.lists() });
    },
  });
}

export function useUpdateComponent() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      id,
      data,
    }: {
      id: string;
      data: { name?: string; component_type?: string; description?: string };
    }) => componentsApi.update(id, data),
    onSuccess: (updatedComponent: Component) => {
      queryClient.invalidateQueries({ queryKey: componentKeys.lists() });
      queryClient.invalidateQueries({
        queryKey: componentKeys.detail(updatedComponent.id),
      });
    },
  });
}

export function useDeleteComponent() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => componentsApi.delete(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: componentKeys.lists() });
    },
  });
}
