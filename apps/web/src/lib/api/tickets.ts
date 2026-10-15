import { apiFetch } from "../api-client";
import type { Ticket, PaginatedResponse } from "../types";

export const ticketsApi = {
  list: (params?: {
    limit?: number;
    offset?: number;
    status?: string;
    priority?: string;
    customerId?: string;
    experienceId?: string;
    applicationId?: string;
    pageId?: string;
    componentId?: string;
  }) => {
    const searchParams = new URLSearchParams();
    searchParams.set("limit", String(params?.limit ?? 50));
    searchParams.set("offset", String(params?.offset ?? 0));
    if (params?.status) searchParams.set("status", params.status);
    if (params?.priority) searchParams.set("priority", params.priority);
    if (params?.customerId)
      searchParams.set("customerId", params.customerId);
    if (params?.experienceId)
      searchParams.set("experienceId", params.experienceId);
    if (params?.applicationId)
      searchParams.set("applicationId", params.applicationId);
    if (params?.pageId) searchParams.set("pageId", params.pageId);
    if (params?.componentId)
      searchParams.set("componentId", params.componentId);
    return apiFetch<PaginatedResponse<Ticket>>(
      `/tickets?${searchParams.toString()}`,
    );
  },

  get: (id: string) => apiFetch<Ticket>(`/tickets/${id}`),

  create: (data: {
    customerId: string;
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
      assignedTo?: string;
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
