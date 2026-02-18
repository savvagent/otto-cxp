import { apiFetch } from "../api-client";
import type { Feedback, PaginatedResponse } from "../types";

export const feedbackApi = {
  list: (params?: {
    limit?: number;
    offset?: number;
    customer_id?: string;
    feedback_type?: string;
    experience_id?: string;
    application_id?: string;
    page_id?: string;
    component_id?: string;
  }) => {
    const searchParams = new URLSearchParams();
    searchParams.set("limit", String(params?.limit ?? 50));
    searchParams.set("offset", String(params?.offset ?? 0));
    if (params?.customer_id)
      searchParams.set("customer_id", params.customer_id);
    if (params?.feedback_type)
      searchParams.set("feedback_type", params.feedback_type);
    if (params?.experience_id)
      searchParams.set("experience_id", params.experience_id);
    if (params?.application_id)
      searchParams.set("application_id", params.application_id);
    if (params?.page_id) searchParams.set("page_id", params.page_id);
    if (params?.component_id)
      searchParams.set("component_id", params.component_id);
    return apiFetch<PaginatedResponse<Feedback>>(
      `/feedback?${searchParams.toString()}`,
    );
  },
};
