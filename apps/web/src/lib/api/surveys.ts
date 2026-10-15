import { apiFetch } from "../api-client";
import type { Survey, SurveyResponse, PaginatedResponse } from "../types";

export const surveysApi = {
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
