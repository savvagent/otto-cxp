import { apiFetch } from "../api-client";
import type { Customer, Customer360, PaginatedResponse } from "../types";

export const customersApi = {
  list: (params?: { limit?: number; offset?: number }) =>
    apiFetch<PaginatedResponse<Customer>>(
      `/customers?limit=${params?.limit ?? 50}&offset=${params?.offset ?? 0}`,
    ),

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
