import { apiFetch } from "../api-client";
import type { Feedback, PaginatedResponse } from "../types";

export const feedbackApi = {
  list: (params?: {
    limit?: number;
    offset?: number;
    customerId?: string;
    feedbackType?: string;
    experienceId?: string;
    applicationId?: string;
    pageId?: string;
    componentId?: string;
  }) => {
    const searchParams = new URLSearchParams();
    searchParams.set("limit", String(params?.limit ?? 50));
    searchParams.set("offset", String(params?.offset ?? 0));
    if (params?.customerId)
      searchParams.set("customerId", params.customerId);
    if (params?.feedbackType)
      searchParams.set("feedbackType", params.feedbackType);
    if (params?.experienceId)
      searchParams.set("experienceId", params.experienceId);
    if (params?.applicationId)
      searchParams.set("applicationId", params.applicationId);
    if (params?.pageId) searchParams.set("pageId", params.pageId);
    if (params?.componentId)
      searchParams.set("componentId", params.componentId);
    return apiFetch<PaginatedResponse<Feedback>>(
      `/feedback?${searchParams.toString()}`,
    );
  },
};
