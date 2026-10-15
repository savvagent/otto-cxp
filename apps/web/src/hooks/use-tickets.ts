import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { ticketsApi } from "@/lib/api";
import type { Ticket } from "@/lib/types";

type TicketListParams = {
  limit?: number;
  offset?: number;
  status?: string;
  priority?: string;
  customerId?: string;
  experienceId?: string;
  applicationId?: string;
  pageId?: string;
  componentId?: string;
};

export const ticketKeys = {
  all: ["tickets"] as const,
  lists: () => [...ticketKeys.all, "list"] as const,
  list: (params: Record<string, unknown>) =>
    [...ticketKeys.lists(), params] as const,
  details: () => [...ticketKeys.all, "detail"] as const,
  detail: (id: string) => [...ticketKeys.details(), id] as const,
};

export function useTickets(params?: TicketListParams) {
  return useQuery({
    queryKey: ticketKeys.list(params ?? {}),
    queryFn: () => ticketsApi.list(params),
  });
}

export function useTicket(id: string | undefined) {
  return useQuery({
    queryKey: ticketKeys.detail(id!),
    queryFn: () => ticketsApi.get(id!),
    enabled: !!id,
  });
}

export function useCreateTicket() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (data: {
      customerId: string;
      subject: string;
      description?: string;
      priority?: string;
      channel?: string;
    }) => ticketsApi.create(data),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ticketKeys.lists() });
    },
  });
}

export function useUpdateTicket() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      id,
      data,
    }: {
      id: string;
      data: {
        status?: string;
        priority?: string;
        assignedTo?: string;
        subject?: string;
        description?: string;
        tags?: string[];
      };
    }) => ticketsApi.update(id, data),
    onSuccess: (updatedTicket: Ticket) => {
      queryClient.invalidateQueries({ queryKey: ticketKeys.lists() });
      queryClient.invalidateQueries({
        queryKey: ticketKeys.detail(updatedTicket.id),
      });
    },
  });
}

export function useDeleteTicket() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => ticketsApi.delete(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ticketKeys.lists() });
    },
  });
}
