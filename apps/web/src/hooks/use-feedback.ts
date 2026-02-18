import { useQuery } from "@tanstack/react-query";
import { feedbackApi } from "@/lib/api";

type FeedbackListParams = {
  limit?: number;
  offset?: number;
  customer_id?: string;
  feedback_type?: string;
  experience_id?: string;
  application_id?: string;
  page_id?: string;
  component_id?: string;
};

export const feedbackKeys = {
  all: ["feedback"] as const,
  lists: () => [...feedbackKeys.all, "list"] as const,
  list: (params: Record<string, unknown>) =>
    [...feedbackKeys.lists(), params] as const,
};

export function useFeedback(params?: FeedbackListParams) {
  return useQuery({
    queryKey: feedbackKeys.list(params ?? {}),
    queryFn: () => feedbackApi.list(params),
  });
}
