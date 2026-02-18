import { apiFetch } from "../api-client";
import type { Customer, Customer360, PaginatedResponse } from "../types";

export const customersApi = {
  list: (params?: {
    limit?: number;
    offset?: number;
    experience_id?: string;
    application_id?: string;
    page_id?: string;
    component_id?: string;
  }) => {
    const searchParams = new URLSearchParams();
    searchParams.set("limit", String(params?.limit ?? 50));
    searchParams.set("offset", String(params?.offset ?? 0));
    if (params?.experience_id)
      searchParams.set("experience_id", params.experience_id);
    if (params?.application_id)
      searchParams.set("application_id", params.application_id);
    if (params?.page_id) searchParams.set("page_id", params.page_id);
    if (params?.component_id)
      searchParams.set("component_id", params.component_id);
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
    apiFetch<Customer[]>(`/customers/search?q=${encodeURIComponent(q)}`),

  get360: (id: string) => apiFetch<Customer360>(`/customers/${id}/360`),
};
