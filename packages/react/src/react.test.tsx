import { describe, it, expect, vi, afterEach } from "vitest";
import { render, renderHook, waitFor, cleanup } from "@testing-library/react";
import React from "react";
import type { CxpClient } from "@cxp/sdk";
import * as pkg from "./index.js";
import { CxpProvider, useCxp, useCxpOptional, useFeedback, useChat } from "./index.js";

afterEach(cleanup);

function mockClient(overrides: Record<string, unknown> = {}): CxpClient {
  return {
    feedback: { getFeedback: vi.fn().mockResolvedValue([]) },
    support: {
      getChatMessages: vi.fn().mockResolvedValue([]),
      sendChatMessage: vi.fn(),
    },
    ...overrides,
  } as unknown as CxpClient;
}

function wrapperFor(client: CxpClient) {
  return ({ children }: { children: React.ReactNode }) => (
    <CxpProvider client={client}>{children}</CxpProvider>
  );
}

describe("CxpProvider", () => {
  it("provides the client to useCxp", () => {
    const client = mockClient();
    const { result } = renderHook(() => useCxp(), { wrapper: wrapperFor(client) });
    expect(result.current).toBe(client);
  });

  it("useCxp throws outside a provider; useCxpOptional returns null", () => {
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});
    expect(() => renderHook(() => useCxp())).toThrow(/CxpProvider/);
    spy.mockRestore();
    const { result } = renderHook(() => useCxpOptional());
    expect(result.current).toBeNull();
  });

  it("renders children", () => {
    const { getByText } = render(
      <CxpProvider client={mockClient()}>
        <span>hello</span>
      </CxpProvider>
    );
    expect(getByText("hello")).toBeTruthy();
  });
});

describe("useFeedback", () => {
  it("loads feedback through the SDK client with the given filters", async () => {
    const items = [{ id: "f1" }];
    const getFeedback = vi.fn().mockResolvedValue(items);
    const client = mockClient({ feedback: { getFeedback } });
    const { result } = renderHook(() => useFeedback({ customerId: "c1", limit: 5 }), {
      wrapper: wrapperFor(client),
    });
    expect(result.current.loading).toBe(true);
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(getFeedback).toHaveBeenCalledWith({ customerId: "c1", limit: 5 });
    expect(result.current.feedback).toEqual(items);
    expect(result.current.error).toBeNull();
  });

  it("exposes SDK errors", async () => {
    const getFeedback = vi.fn().mockRejectedValue(new Error("not_found"));
    const client = mockClient({ feedback: { getFeedback } });
    const { result } = renderHook(() => useFeedback(), { wrapper: wrapperFor(client) });
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.error?.message).toBe("not_found");
  });
});

describe("useChat", () => {
  it("loads history for the session", async () => {
    const history = [{ id: "m1", content: "hi" }];
    const client = mockClient({
      support: { getChatMessages: vi.fn().mockResolvedValue(history) },
    });
    const { result } = renderHook(() => useChat("s1"), { wrapper: wrapperFor(client) });
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.messages).toEqual(history);
  });
});

describe("package surface", () => {
  it("no longer exports useTeams", () => {
    expect("useTeams" in pkg).toBe(false);
    expect("useFeedback" in pkg).toBe(true);
  });
});
