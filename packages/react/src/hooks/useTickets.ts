import { useState, useEffect, useCallback } from "react";
import type { Ticket } from "@cxp/sdk";
import { useCxp } from "../context/CxpProvider.js";

export interface UseTicketsOptions {
  customerId?: string;
  status?: string;
  priority?: string;
  limit?: number;
  offset?: number;
}

/**
 * Hook to fetch tickets with optional filters
 * @param options - Filter options
 * @returns Tickets array, loading state, error, and refetch function
 *
 * @example
 * const { tickets, loading, error, refetch } = useTickets({
 *   customerId: "cust_123",
 *   status: "open"
 * });
 */
export function useTickets(options: UseTicketsOptions = {}) {
  const cxp = useCxp();
  const [tickets, setTickets] = useState<Ticket[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<Error | null>(null);

  const fetchTickets = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const data = await cxp.support.listTickets(options);
      setTickets(data);
    } catch (err) {
      setError(err as Error);
    } finally {
      setLoading(false);
    }
  }, [cxp, JSON.stringify(options)]);

  useEffect(() => {
    let cancelled = false;
    fetchTickets().then(() => {
      if (cancelled) {
        setTickets([]);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [fetchTickets]);

  return { tickets, loading, error, refetch: fetchTickets };
}
