import { useState, useEffect, useCallback } from "react";
import type { Feedback } from "@cxp/sdk";
import { useCxp } from "../context/CxpProvider.js";

export interface UseFeedbackOptions {
  customerId?: string;
  feedbackType?: string;
  limit?: number;
}

/**
 * Hook to fetch feedback with optional filters
 * @param options - Filter options
 * @returns Feedback array, loading state, error, and refetch function
 *
 * @example
 * const { feedback, loading, error, refetch } = useFeedback({
 *   customerId: "cust_123",
 *   feedbackType: "feature_request"
 * });
 */
export function useFeedback(options: UseFeedbackOptions = {}) {
  const cxp = useCxp();
  const [feedback, setFeedback] = useState<Feedback[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<Error | null>(null);

  const fetchFeedback = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const data = await cxp.feedback.getFeedback(options);
      setFeedback(data);
    } catch (err) {
      setError(err as Error);
    } finally {
      setLoading(false);
    }
  }, [cxp, JSON.stringify(options)]);

  useEffect(() => {
    let cancelled = false;
    fetchFeedback().then(() => {
      if (cancelled) {
        setFeedback([]);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [fetchFeedback]);

  return { feedback, loading, error, refetch: fetchFeedback };
}
