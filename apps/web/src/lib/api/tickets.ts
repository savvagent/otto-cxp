import { apiFetch } from "../api-client";
import type { Ticket, PaginatedResponse } from "../types";

export const ticketsApi = {
  list: (params?: {
    limit?: number;
    offset?: number;
    status?: string;
    priority?: string;
    customer_id?: string;
  }) => {
    const searchParams = new URLSearchParams();
    searchParams.set("limit", String(params?.limit ?? 50));
    searchParams.set("offset", String(params?.offset ?? 0));
    if (params?.status) searchParams.set("status", params.status);
    if (params?.priority) searchParams.set("priority", params.priority);
    if (params?.customer_id)
      searchParams.set("customer_id", params.customer_id);
    return apiFetch<PaginatedResponse<Ticket>>(
      `/tickets?${searchParams.toString()}`,
    );
  },

  get: (id: string) => apiFetch<Ticket>(`/tickets/${id}`),

  create: (data: {
    customer_id: string;
    subject: string;
    description?: string;
    priority?: string;
    channel?: string;
  }) =>
    apiFetch<Ticket>("/tickets", {
      method: "POST",
      body: JSON.stringify(data),
    }),

  update: (
    id: string,
    data: {
      status?: string;
      priority?: string;
      assigned_to?: string;
      subject?: string;
      description?: string;
      tags?: string[];
    },
  ) =>
    apiFetch<Ticket>(`/tickets/${id}`, {
      method: "PUT",
      body: JSON.stringify(data),
    }),

  delete: (id: string) =>
    apiFetch<{ deleted: boolean }>(`/tickets/${id}`, { method: "DELETE" }),
};
