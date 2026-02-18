import { useState, useEffect } from "react";
import type { Customer } from "@cxp/sdk";
import { useCxp } from "../context/CxpProvider.js";

/**
 * Hook to fetch customer data by ID
 * @param id - Customer ID
 * @returns Customer data, loading state, and error
 *
 * @example
 * const { customer, loading, error } = useCustomer("cust_123");
 */
export function useCustomer(id: string) {
  const cxp = useCxp();
  const [customer, setCustomer] = useState<Customer | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<Error | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    cxp.cdp
      .getCustomer(id)
      .then((data) => {
        if (!cancelled) {
          setCustomer(data);
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
  }, [cxp, id]);

  return { customer, loading, error };
}
