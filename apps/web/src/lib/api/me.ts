import { apiFetch } from "../api-client";
import type { MyTeam } from "../types";

export const meApi = {
  /** Teams the caller belongs to, from the otto platform. */
  teams: () => apiFetch<MyTeam[]>("/me/teams"),
};
