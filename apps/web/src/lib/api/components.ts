import { apiFetch } from "../api-client";
import type { Component, PaginatedResponse } from "../types";

export const componentsApi = {
  list: (params?: { limit?: number; offset?: number; page_id?: string }) => {
    const searchParams = new URLSearchParams();
    searchParams.set("limit", String(params?.limit ?? 50));
    searchParams.set("offset", String(params?.offset ?? 0));
    if (params?.page_id) searchParams.set("page_id", params.page_id);
    return apiFetch<PaginatedResponse<Component>>(
      `/components?${searchParams.toString()}`,
    );
  },

  get: (id: string) => apiFetch<Component>(`/components/${id}`),

  create: (data: {
    page_id?: string;
    name: string;
    component_type?: string;
    description?: string;
  }) =>
    apiFetch<Component>("/components", {
      method: "POST",
      body: JSON.stringify(data),
    }),

  update: (
    id: string,
    data: { name?: string; component_type?: string; description?: string },
  ) =>
    apiFetch<Component>(`/components/${id}`, {
      method: "PUT",
      body: JSON.stringify(data),
    }),

  delete: (id: string) =>
    apiFetch<{ deleted: boolean }>(`/components/${id}`, { method: "DELETE" }),
};
