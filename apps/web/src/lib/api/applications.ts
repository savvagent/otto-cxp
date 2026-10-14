import { apiFetch } from "../api-client";
import type { Application, PaginatedResponse } from "../types";

export const applicationsApi = {
  list: (params?: {
    limit?: number;
    offset?: number;
    experience_id?: string;
  }) => {
    const searchParams = new URLSearchParams();
    searchParams.set("limit", String(params?.limit ?? 50));
    searchParams.set("offset", String(params?.offset ?? 0));
    if (params?.experience_id)
      searchParams.set("experience_id", params.experience_id);
    return apiFetch<PaginatedResponse<Application>>(
      `/applications?${searchParams.toString()}`,
    );
  },

  get: (id: string) => apiFetch<Application>(`/applications/${id}`),

  create: (data: {
    experience_id?: string;
    team_id?: string;
    name: string;
    description?: string;
    platform?: string;
  }) =>
    apiFetch<Application>("/applications", {
      method: "POST",
      body: JSON.stringify(data),
    }),

  update: (
    id: string,
    data: { name?: string; description?: string; platform?: string; team_id?: string },
  ) =>
    apiFetch<Application>(`/applications/${id}`, {
      method: "PUT",
      body: JSON.stringify(data),
    }),

  delete: (id: string) =>
    apiFetch<{ deleted: boolean }>(`/applications/${id}`, {
      method: "DELETE",
    }),
};
