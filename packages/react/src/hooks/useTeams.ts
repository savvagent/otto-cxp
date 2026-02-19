import { useState, useEffect, useCallback } from "react";
import type { Team } from "@cxp/sdk";
import { useCxpOptional } from "../context/CxpProvider.js";

const DEMO_TEAMS: Team[] = [
  {
    id: "demo-team-1",
    name: "Platform Team",
    description: "Core platform and infrastructure",
    owner: "alice@example.com",
    metadata: {},
    createdAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
  },
  {
    id: "demo-team-2",
    name: "Growth Team",
    description: "User acquisition and retention",
    owner: "bob@example.com",
    metadata: {},
    createdAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
  },
];

export interface UseTeamsOptions {
  limit?: number;
  offset?: number;
}

/**
 * Hook to manage teams with full CRUD operations
 * @param options - Pagination options
 * @returns Teams array, loading state, error, and CRUD functions
 */
export function useTeams(options: UseTeamsOptions = {}) {
  const cxp = useCxpOptional();
  const [teams, setTeams] = useState<Team[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<Error | null>(null);

  const fetchTeams = useCallback(async () => {
    setLoading(true);
    setError(null);
    if (!cxp) {
      setTeams(DEMO_TEAMS);
      setLoading(false);
      return;
    }
    try {
      const data = await cxp.teams.list({
        limit: options.limit,
        offset: options.offset,
      });
      setTeams(data);
    } catch (err) {
      setError(err as Error);
    } finally {
      setLoading(false);
    }
  }, [cxp, options.limit, options.offset]);

  useEffect(() => {
    let cancelled = false;
    fetchTeams().then(() => {
      if (cancelled) setTeams([]);
    });
    return () => {
      cancelled = true;
    };
  }, [fetchTeams]);

  const createTeam = useCallback(
    async (name: string, opts?: { description?: string; owner?: string }) => {
      if (!cxp) return;
      setError(null);
      try {
        const team = await cxp.teams.create(name, opts);
        setTeams((prev) => [team, ...prev]);
        return team;
      } catch (err) {
        setError(err as Error);
        throw err;
      }
    },
    [cxp]
  );

  const updateTeam = useCallback(
    async (
      id: string,
      updates: { name?: string; description?: string; owner?: string }
    ) => {
      if (!cxp) return;
      setError(null);
      try {
        const team = await cxp.teams.update(id, updates);
        setTeams((prev) => prev.map((t) => (t.id === id ? team : t)));
        return team;
      } catch (err) {
        setError(err as Error);
        throw err;
      }
    },
    [cxp]
  );

  const deleteTeam = useCallback(
    async (id: string) => {
      if (!cxp) return;
      setError(null);
      try {
        await cxp.teams.delete(id);
        setTeams((prev) => prev.filter((t) => t.id !== id));
      } catch (err) {
        setError(err as Error);
        throw err;
      }
    },
    [cxp]
  );

  return { teams, loading, error, refetch: fetchTeams, createTeam, updateTeam, deleteTeam };
}
