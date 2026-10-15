import { apiFetch } from "../api-client";
import type { Component, PaginatedResponse } from "../types";

export const componentsApi = {
  list: (params?: { limit?: number; offset?: number; pageId?: string }) => {
    const searchParams = new URLSearchParams();
    searchParams.set("limit", String(params?.limit ?? 50));
    searchParams.set("offset", String(params?.offset ?? 0));
    if (params?.pageId) searchParams.set("pageId", params.pageId);
    return apiFetch<PaginatedResponse<Component>>(
      `/components?${searchParams.toString()}`,
    );
  },

  get: (id: string) => apiFetch<Component>(`/components/${id}`),

  create: (data: {
    pageId?: string;
    teamId?: string | null;
    name: string;
    componentType?: string;
    description?: string;
  }) =>
    apiFetch<Component>("/components", {
      method: "POST",
      body: JSON.stringify(data),
    }),

  update: (
    id: string,
    data: { name?: string; componentType?: string; description?: string; teamId?: string | null },
  ) =>
    apiFetch<Component>(`/components/${id}`, {
      method: "PUT",
      body: JSON.stringify(data),
    }),

  delete: (id: string) =>
    apiFetch<{ deleted: boolean }>(`/components/${id}`, { method: "DELETE" }),
};
