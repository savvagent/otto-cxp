import React, { createContext, useContext } from "react";
import type { CxpClient } from "@cxp/sdk";

const CxpContext = createContext<CxpClient | null>(null);

export function CxpProvider({
  client,
  children,
}: {
  client: CxpClient;
  children: React.ReactNode;
}) {
  return <CxpContext.Provider value={client}>{children}</CxpContext.Provider>;
}

export function useCxp(): CxpClient {
  const ctx = useContext(CxpContext);
  if (!ctx) throw new Error("useCxp must be used within a CxpProvider");
  return ctx;
}

/**
 * Returns the CxpClient if inside a CxpProvider, or null otherwise.
 * Useful for components that support a standalone demo mode.
 */
export function useCxpOptional(): CxpClient | null {
  return useContext(CxpContext);
}
