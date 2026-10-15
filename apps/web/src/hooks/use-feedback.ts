import { useQuery } from "@tanstack/react-query";
import { feedbackApi } from "@/lib/api";

type FeedbackListParams = {
  limit?: number;
  offset?: number;
  customerId?: string;
  feedbackType?: string;
  experienceId?: string;
  applicationId?: string;
  pageId?: string;
  componentId?: string;
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
