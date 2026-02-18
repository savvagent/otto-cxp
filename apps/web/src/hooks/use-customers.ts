import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { customersApi } from "@/lib/api";
import type { Customer } from "@/lib/types";

export const customerKeys = {
  all: ["customers"] as const,
  lists: () => [...customerKeys.all, "list"] as const,
  list: (params: Record<string, unknown>) =>
    [...customerKeys.lists(), params] as const,
  details: () => [...customerKeys.all, "detail"] as const,
  detail: (id: string) => [...customerKeys.details(), id] as const,
};

export function useCustomers(params?: {
  limit?: number;
  offset?: number;
  experience_id?: string;
  application_id?: string;
  page_id?: string;
  component_id?: string;
}) {
  return useQuery({
    queryKey: customerKeys.list(params ?? {}),
    queryFn: () => customersApi.list(params),
  });
}

export function useCustomer(id: string | undefined) {
  return useQuery({
    queryKey: customerKeys.detail(id!),
    queryFn: () => customersApi.get(id!),
    enabled: !!id,
  });
}

export function useCreateCustomer() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (data: {
      email: string;
      name?: string;
      phone?: string;
      company?: string;
      tags?: string[];
    }) => customersApi.create(data),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: customerKeys.lists() });
    },
  });
}

export function useUpdateCustomer() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      id,
      data,
    }: {
      id: string;
      data: {
        email?: string;
        name?: string;
        phone?: string;
        company?: string;
        tags?: string[];
      };
    }) => customersApi.update(id, data),
    onSuccess: (updatedCustomer: Customer) => {
      queryClient.invalidateQueries({ queryKey: customerKeys.lists() });
      queryClient.invalidateQueries({
        queryKey: customerKeys.detail(updatedCustomer.id),
      });
    },
  });
}

export function useDeleteCustomer() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => customersApi.delete(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: customerKeys.lists() });
    },
  });
}
