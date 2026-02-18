import { apiFetch } from "../api-client";
import type { Survey, SurveyResponse, PaginatedResponse } from "../types";

export const surveysApi = {
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
    return apiFetch<PaginatedResponse<Survey>>(
      `/surveys?${searchParams.toString()}`,
    );
  },

  get: (id: string) => apiFetch<Survey>(`/surveys/${id}`),

  create: (data: {
    title: string;
    description?: string;
    questions: unknown;
  }) =>
    apiFetch<Survey>("/surveys", {
      method: "POST",
      body: JSON.stringify(data),
    }),

  update: (
    id: string,
    data: {
      title?: string;
      description?: string;
      questions?: unknown;
      active?: boolean;
    },
  ) =>
    apiFetch<Survey>(`/surveys/${id}`, {
      method: "PUT",
      body: JSON.stringify(data),
    }),

  delete: (id: string) =>
    apiFetch<{ deleted: boolean }>(`/surveys/${id}`, { method: "DELETE" }),

  getResponses: (id: string) =>
    apiFetch<SurveyResponse[]>(`/surveys/${id}/responses`),
};
