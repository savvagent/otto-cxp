import { apiFetch } from "../api-client";
import type { DashboardMetrics } from "../types";

export const dashboardApi = {
  metrics: () => apiFetch<DashboardMetrics>("/dashboard/metrics"),
};
