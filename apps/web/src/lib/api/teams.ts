import { apiFetch } from "../api-client";
import type { Team, PaginatedResponse } from "../types";

export const teamsApi = {
  list: (params?: { limit?: number; offset?: number }) =>
    apiFetch<PaginatedResponse<Team>>(
      `/teams?limit=${params?.limit ?? 50}&offset=${params?.offset ?? 0}`,
    ),

  get: (id: string) => apiFetch<Team>(`/teams/${id}`),

  create: (data: { name: string; description?: string; owner?: string }) =>
    apiFetch<Team>("/teams", {
      method: "POST",
      body: JSON.stringify(data),
    }),

  update: (id: string, data: { name?: string; description?: string; owner?: string }) =>
    apiFetch<Team>(`/teams/${id}`, {
      method: "PUT",
      body: JSON.stringify(data),
    }),

  delete: (id: string) =>
    apiFetch<{ deleted: boolean }>(`/teams/${id}`, { method: "DELETE" }),
};
