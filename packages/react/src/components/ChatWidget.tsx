import React, { useState, useRef, useEffect } from "react";
import { useChat } from "../hooks/useChat.js";

export interface ChatWidgetProps {
  sessionId: string;
}

/**
 * Component for interactive chat widget
 * @example
 * <ChatWidget sessionId="session_123" />
 */
export function ChatWidget({ sessionId }: ChatWidgetProps) {
  const { messages, loading, error, sendMessage } = useChat(sessionId);
  const [inputValue, setInputValue] = useState("");
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
          Chat Support
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
        {messages.length === 0 && (
          <div style={{ textAlign: "center", color: "#757575", padding: "20px" }}>
            No messages yet. Start the conversation!
          </div>
        )}
        {messages.map((message, index) => (
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
            Error: {error.message}
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
            placeholder="Type your message..."
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
            {loading ? "Sending..." : "Send"}
          </button>
        </div>
      </form>
    </div>
  );
}
