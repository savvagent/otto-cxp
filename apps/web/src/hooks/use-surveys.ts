import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { surveysApi } from "@/lib/api";
import type { Survey } from "@/lib/types";

export const surveyKeys = {
  all: ["surveys"] as const,
  lists: () => [...surveyKeys.all, "list"] as const,
  list: (params: Record<string, unknown>) =>
    [...surveyKeys.lists(), params] as const,
  details: () => [...surveyKeys.all, "detail"] as const,
  detail: (id: string) => [...surveyKeys.details(), id] as const,
  responses: (id: string) => [...surveyKeys.detail(id), "responses"] as const,
};

export function useSurveys(params?: {
  limit?: number;
  offset?: number;
  experience_id?: string;
  application_id?: string;
  page_id?: string;
  component_id?: string;
}) {
  return useQuery({
    queryKey: surveyKeys.list(params ?? {}),
    queryFn: () => surveysApi.list(params),
  });
}

export function useSurvey(id: string | undefined) {
  return useQuery({
    queryKey: surveyKeys.detail(id!),
    queryFn: () => surveysApi.get(id!),
    enabled: !!id,
  });
}

export function useSurveyResponses(id: string | undefined) {
  return useQuery({
    queryKey: surveyKeys.responses(id!),
    queryFn: () => surveysApi.getResponses(id!),
    enabled: !!id,
  });
}

export function useCreateSurvey() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (data: {
      title: string;
      description?: string;
      questions: unknown;
    }) => surveysApi.create(data),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: surveyKeys.lists() });
    },
  });
}

export function useUpdateSurvey() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      id,
      data,
    }: {
      id: string;
      data: {
        title?: string;
        description?: string;
        questions?: unknown;
        active?: boolean;
      };
    }) => surveysApi.update(id, data),
    onSuccess: (updatedSurvey: Survey) => {
      queryClient.invalidateQueries({ queryKey: surveyKeys.lists() });
      queryClient.invalidateQueries({
        queryKey: surveyKeys.detail(updatedSurvey.id),
      });
    },
  });
}

export function useDeleteSurvey() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => surveysApi.delete(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: surveyKeys.lists() });
    },
  });
}
