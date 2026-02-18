import { useState, useCallback, useEffect } from "react";
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
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<Error | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    cxp.support
      .getChatMessages(sessionId)
      .then((history) => {
        if (!cancelled) {
          setMessages(history);
          setLoading(false);
        }
      })
      .catch((err) => {
        if (!cancelled) {
          setError(err as Error);
          setLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [cxp, sessionId]);

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
