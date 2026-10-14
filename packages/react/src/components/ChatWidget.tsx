import React, { useState, useRef, useEffect, useCallback } from "react";
import type { ChatMessage } from "@cxp/sdk";
import { useCxpOptional } from "../context/CxpProvider.js";

export interface ChatWidgetLabels {
  header?: string;
  placeholder?: string;
  send?: string;
  sending?: string;
  noSession?: string;
  empty?: string;
  errorPrefix?: string;
}

const DEFAULT_CHAT_LABELS: Required<ChatWidgetLabels> = {
  header: "Chat Support",
  placeholder: "Type your message...",
  send: "Send",
  sending: "Sending...",
  noSession: "Provide a session ID or seed mock data to start chatting.",
  empty: "No messages yet. Start the conversation!",
  errorPrefix: "Error:",
};

export interface ChatWidgetProps {
  sessionId?: string;
  /** Catalog experience UUID (`experience_id`). Currently unused by this component. */
  experience?: string;
  /** Catalog application UUID (`application_id`). Currently unused by this component. */
  application?: string;
  page?: string;
  component?: string;
  labels?: ChatWidgetLabels;
}

const DEMO_MESSAGES: ChatMessage[] = [
  {
    id: "demo-1",
    sessionId: "demo",
    role: "assistant",
    content: "Hi there! How can I help you today?",
    metadata: {},
    createdAt: new Date(Date.now() - 60000).toISOString(),
  },
  {
    id: "demo-2",
    sessionId: "demo",
    role: "user",
    content: "I have a question about my account settings.",
    metadata: {},
    createdAt: new Date(Date.now() - 30000).toISOString(),
  },
  {
    id: "demo-3",
    sessionId: "demo",
    role: "assistant",
    content:
      "Of course! I'd be happy to help with your account settings. What would you like to change?",
    metadata: {},
    createdAt: new Date().toISOString(),
  },
];

/**
 * Component for interactive chat widget.
 * Works standalone with demo data when no CxpProvider is present.
 *
 * @example
 * // Standalone demo mode
 * <ChatWidget />
 *
 * @example
 * // Connected to CXP backend
 * <CxpProvider client={cxpClient}>
 *   <ChatWidget sessionId="session_123" />
 * </CxpProvider>
 */
export function ChatWidget({
  sessionId = "demo",
  labels: labelOverrides,
}: ChatWidgetProps) {
  const l = { ...DEFAULT_CHAT_LABELS, ...labelOverrides };
  const cxp = useCxpOptional();
  const isDemo = !cxp;
  const hasValidId = sessionId !== "demo" && sessionId !== "";

  const [messages, setMessages] = useState<ChatMessage[]>(isDemo ? DEMO_MESSAGES : []);
  const [loading, setLoading] = useState(!isDemo && hasValidId);
  const [error, setError] = useState<Error | null>(null);
  const [inputValue, setInputValue] = useState("");

  useEffect(() => {
    if (isDemo || !hasValidId) return;
    let cancelled = false;
    setLoading(true);
    cxp!.support
      .getChatMessages(sessionId)
      .then((history: ChatMessage[]) => {
        if (!cancelled) {
          setMessages(history);
          setLoading(false);
        }
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setError(err as Error);
          setLoading(false);
        }
      });
    return () => { cancelled = true; };
  }, [cxp, sessionId, isDemo, hasValidId]);

  const sendMessage = useCallback(
    async (role: string, content: string) => {
      if (cxp) {
        setLoading(true);
        setError(null);
        try {
          const message = await cxp.support.sendChatMessage(sessionId, role, content);
          setMessages((prev) => [...prev, message]);
        } catch (err) {
          setError(err as Error);
          throw err;
        } finally {
          setLoading(false);
        }
      } else {
        // Demo mode
        const userMsg: ChatMessage = {
          id: `demo-${Date.now()}`,
          sessionId: "demo",
          role,
          content,
          metadata: {},
          createdAt: new Date().toISOString(),
        };
        setMessages((prev) => [...prev, userMsg]);
        setLoading(true);
        await new Promise((r) => setTimeout(r, 800));
        const reply: ChatMessage = {
          id: `demo-${Date.now() + 1}`,
          sessionId: "demo",
          role: "assistant",
          content: "Thanks for your message! This is a demo response.",
          metadata: {},
          createdAt: new Date().toISOString(),
        };
        setMessages((prev) => [...prev, reply]);
        setLoading(false);
      }
    },
    [cxp, sessionId],
  );
  const messagesEndRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages]);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!inputValue.trim() || loading) return;

    const message = inputValue;
    setInputValue("");

    try {
      await sendMessage("user", message);
    } catch (err) {
      console.error("Failed to send message:", err);
    }
  };

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        height: "600px",
        maxWidth: "600px",
        border: "1px solid #e0e0e0",
        borderRadius: "8px",
        backgroundColor: "#fff",
        overflow: "hidden",
      }}
    >
      {/* Header */}
      <div
        style={{
          padding: "16px",
          borderBottom: "1px solid #e0e0e0",
          backgroundColor: "#1976d2",
          color: "#fff",
        }}
      >
        <h3 style={{ margin: 0, fontSize: "18px", fontWeight: "600" }}>
          {l.header}
        </h3>
      </div>

      {/* Messages */}
      <div
        style={{
          flex: 1,
          overflowY: "auto",
          padding: "16px",
          display: "flex",
          flexDirection: "column",
          gap: "12px",
          backgroundColor: "#f5f5f5",
        }}
      >
        {!isDemo && !hasValidId && (
          <div style={{ textAlign: "center", color: "#757575", padding: "20px" }}>
            {l.noSession}
          </div>
        )}
        {messages.length === 0 && hasValidId && (
          <div style={{ textAlign: "center", color: "#757575", padding: "20px" }}>
            {l.empty}
          </div>
        )}
        {messages.map((message: ChatMessage, index: number) => (
          <div
            key={index}
            style={{
              alignSelf: message.role === "user" ? "flex-end" : "flex-start",
              maxWidth: "70%",
            }}
          >
            <div
              style={{
                padding: "12px 16px",
                borderRadius: "16px",
                backgroundColor:
                  message.role === "user" ? "#1976d2" : "#fff",
                color: message.role === "user" ? "#fff" : "#212121",
                boxShadow: "0 1px 2px rgba(0,0,0,0.1)",
              }}
            >
              <div style={{ fontSize: "14px", lineHeight: "1.5" }}>
                {message.content}
              </div>
              <div
                style={{
                  fontSize: "11px",
                  marginTop: "4px",
                  opacity: 0.7,
                }}
              >
                {new Date(message.createdAt).toLocaleTimeString()}
              </div>
            </div>
          </div>
        ))}
        {error && (
          <div
            style={{
              padding: "12px",
              backgroundColor: "#ffebee",
              color: "#c62828",
              borderRadius: "8px",
              fontSize: "14px",
            }}
          >
            {l.errorPrefix} {error.message}
          </div>
        )}
        <div ref={messagesEndRef} />
      </div>

      {/* Input */}
      <form
        onSubmit={handleSubmit}
        style={{
          padding: "16px",
          borderTop: "1px solid #e0e0e0",
          backgroundColor: "#fff",
        }}
      >
        <div style={{ display: "flex", gap: "8px" }}>
          <input
            type="text"
            value={inputValue}
            onChange={(e) => setInputValue(e.target.value)}
            placeholder={l.placeholder}
            disabled={loading}
            style={{
              flex: 1,
              padding: "12px",
              border: "1px solid #e0e0e0",
              borderRadius: "20px",
              fontSize: "14px",
              outline: "none",
            }}
          />
          <button
            type="submit"
            disabled={loading || !inputValue.trim()}
            style={{
              padding: "12px 24px",
              backgroundColor: loading ? "#bdbdbd" : "#1976d2",
              color: "#fff",
              border: "none",
              borderRadius: "20px",
              fontSize: "14px",
              fontWeight: "500",
              cursor: loading ? "not-allowed" : "pointer",
            }}
          >
            {loading ? l.sending : l.send}
          </button>
        </div>
      </form>
    </div>
  );
}
