import { useState, useEffect } from "react";
import { useCxp } from "../context/CxpProvider.js";

export interface MetricsDashboard {
  totalTickets: number;
  openTickets: number;
  inProgressTickets: number;
  resolvedTickets: number;
  closedTickets: number;
  averageResolutionTime?: number;
  csatScore?: number;
  averageRating?: number;
}

/**
 * Hook to fetch dashboard metrics
 * @returns Metrics object, loading state, and error
 *
 * @example
 * const { metrics, loading, error } = useMetrics();
 * if (metrics) {
 *   console.log(`CSAT Score: ${metrics.csatScore}`);
 * }
 */
export function useMetrics() {
  const cxp = useCxp();
  const [metrics, setMetrics] = useState<MetricsDashboard | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<Error | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    cxp
      .getDashboardMetrics()
      .then((data) => {
        if (!cancelled) {
          const parsed = JSON.parse(data) as MetricsDashboard;
          setMetrics(parsed);
          setLoading(false);
        }
      })
      .catch((err) => {
        if (!cancelled) {
          setError(err);
          setLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [cxp]);

  return { metrics, loading, error };
}
