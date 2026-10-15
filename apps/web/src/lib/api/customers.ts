import { apiFetch } from "../api-client";
import type { CappedPaginatedResponse, Customer, Customer360, PaginatedResponse } from "../types";

export const customersApi = {
  list: (params?: {
    limit?: number;
    offset?: number;
    experienceId?: string;
    applicationId?: string;
    pageId?: string;
    componentId?: string;
  }) => {
    const searchParams = new URLSearchParams();
    searchParams.set("limit", String(params?.limit ?? 50));
    searchParams.set("offset", String(params?.offset ?? 0));
    if (params?.experienceId)
      searchParams.set("experienceId", params.experienceId);
    if (params?.applicationId)
      searchParams.set("applicationId", params.applicationId);
    if (params?.pageId) searchParams.set("pageId", params.pageId);
    if (params?.componentId)
      searchParams.set("componentId", params.componentId);
    return apiFetch<PaginatedResponse<Customer>>(
      `/customers?${searchParams.toString()}`,
    );
  },

  get: (id: string) => apiFetch<Customer>(`/customers/${id}`),

  create: (data: {
    email: string;
    name?: string;
    phone?: string;
    company?: string;
    tags?: string[];
  }) =>
    apiFetch<Customer>("/customers", {
      method: "POST",
      body: JSON.stringify(data),
    }),

  update: (
    id: string,
    data: {
      email?: string;
      name?: string;
      phone?: string;
      company?: string;
      tags?: string[];
    },
  ) =>
    apiFetch<Customer>(`/customers/${id}`, {
      method: "PUT",
      body: JSON.stringify(data),
    }),

  delete: (id: string) =>
    apiFetch<{ deleted: boolean }>(`/customers/${id}`, { method: "DELETE" }),

  search: (q: string) =>
    apiFetch<CappedPaginatedResponse<Customer>>(`/customers/search?q=${encodeURIComponent(q)}`),

  get360: (id: string) => apiFetch<Customer360>(`/customers/${id}/360`),
};
