import { apiFetch } from "../api-client";
import type { Page, PaginatedResponse } from "../types";

export const pagesApi = {
  list: (params?: {
    limit?: number;
    offset?: number;
    application_id?: string;
  }) => {
    const searchParams = new URLSearchParams();
    searchParams.set("limit", String(params?.limit ?? 50));
    searchParams.set("offset", String(params?.offset ?? 0));
    if (params?.application_id)
      searchParams.set("application_id", params.application_id);
    return apiFetch<PaginatedResponse<Page>>(
      `/pages?${searchParams.toString()}`,
    );
  },

  get: (id: string) => apiFetch<Page>(`/pages/${id}`),

  create: (data: {
    application_id?: string;
    team_id?: string;
    name: string;
    path?: string;
    description?: string;
  }) =>
    apiFetch<Page>("/pages", {
      method: "POST",
      body: JSON.stringify(data),
    }),

  update: (
    id: string,
    data: { name?: string; path?: string; description?: string; team_id?: string },
  ) =>
    apiFetch<Page>(`/pages/${id}`, {
      method: "PUT",
      body: JSON.stringify(data),
    }),

  delete: (id: string) =>
    apiFetch<{ deleted: boolean }>(`/pages/${id}`, { method: "DELETE" }),
};
