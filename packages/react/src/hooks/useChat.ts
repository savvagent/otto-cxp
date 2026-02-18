import { useState, useCallback } from "react";
import type { ChatMessage } from "@cxp/sdk";
import { useCxp } from "../context/CxpProvider.js";

/**
 * Hook to manage chat session messages
 * @param sessionId - Chat session ID
 * @returns Messages array, loading state, error, and sendMessage function
 *
 * @example
 * const { messages, loading, error, sendMessage } = useChat("session_123");
 * await sendMessage("user", "Hello, I need help");
 */
export function useChat(sessionId: string) {
  const cxp = useCxp();
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);

  const sendMessage = useCallback(
    async (role: string, content: string) => {
      setLoading(true);
      setError(null);
      try {
        const message = await cxp.support.sendChatMessage(
          sessionId,
          role,
          content
        );
        setMessages((prev) => [...prev, message]);
      } catch (err) {
        setError(err as Error);
        throw err;
      } finally {
        setLoading(false);
      }
    },
    [cxp, sessionId]
  );

  return { messages, loading, error, sendMessage };
}
