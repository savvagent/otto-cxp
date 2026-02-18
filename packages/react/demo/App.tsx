import React, { useState, useEffect } from "react";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";
import { CxpClient } from "@cxp/sdk";
import { CxpProvider } from "@cxp/react";
import {
  ChatWidget,
  FeedbackForm,
  SurveyRenderer,
} from "@cxp/react";
import { seedMockData } from "./mockData";

const demos = [
  "ChatWidget",
  "FeedbackForm",
  "SurveyRenderer",
] as const;

type Demo = (typeof demos)[number];

function DemoContent({
  active,
  customerId,
  surveyId,
  sessionId,
}: {
  active: Demo;
  customerId: string;
  surveyId: string;
  sessionId: string;
}) {
  switch (active) {
    case "ChatWidget":
      return <ChatWidget sessionId={sessionId || undefined} />;
    case "FeedbackForm":
      return (
        <FeedbackForm
          customerId={customerId || undefined}
          onSubmit={(fb) => console.log("Feedback submitted:", fb)}
        />
      );
    case "SurveyRenderer":
      return (
        <SurveyRenderer
          surveyId={surveyId || undefined}
          customerId={customerId || undefined}
          onComplete={() => console.log("Survey completed")}
        />
      );
  }
}

export default function App() {
  const [active, setActive] = useState<Demo>("ChatWidget");
  const [cxpClient, setCxpClient] = useState<CxpClient | null>(null);
  const [connectionStatus, setConnectionStatus] = useState<
    "connecting" | "connected" | "failed"
  >("connecting");

  const [customerId, setCustomerId] = useState("");
  const [surveyId, setSurveyId] = useState("");
  const [sessionId, setSessionId] = useState("");
  const [seeding, setSeeding] = useState(false);
  const [seedError, setSeedError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    async function connect() {
      try {
        const transport = new StreamableHTTPClientTransport(
          new URL("/mcp", window.location.origin)
        );
        const mcpClient = new Client(
          { name: "cxp-demo", version: "0.1.0" },
          { capabilities: {} }
        );
        await mcpClient.connect(transport);

        if (cancelled) return;

        const client = new CxpClient({ client: mcpClient });
        setCxpClient(client);
        setConnectionStatus("connected");
      } catch {
        if (!cancelled) {
          setConnectionStatus("failed");
        }
      }
    }

    connect();
    return () => {
      cancelled = true;
    };
  }, []);

  const isDemo = connectionStatus !== "connected";

  const content = (
    <div
      style={{
        display: "flex",
        minHeight: "100vh",
        fontFamily: "system-ui, -apple-system, sans-serif",
      }}
    >
      {/* Sidebar */}
      <nav
        style={{
          width: 240,
          flexShrink: 0,
          borderRight: "1px solid #e0e0e0",
          backgroundColor: "#fafafa",
          padding: "20px 0",
          display: "flex",
          flexDirection: "column",
        }}
      >
        <h1
          style={{
            fontSize: 18,
            fontWeight: 700,
            padding: "0 16px 16px",
            margin: 0,
            borderBottom: "1px solid #e0e0e0",
          }}
        >
          CXP Components
        </h1>

        {/* Connection status */}
        <div
          style={{
            padding: "8px 16px",
            borderBottom: "1px solid #e0e0e0",
            fontSize: 12,
          }}
        >
          <span
            style={{
              display: "inline-block",
              width: 8,
              height: 8,
              borderRadius: "50%",
              marginRight: 6,
              backgroundColor:
                connectionStatus === "connected"
                  ? "#4caf50"
                  : connectionStatus === "connecting"
                    ? "#ff9800"
                    : "#9e9e9e",
            }}
          />
          {connectionStatus === "connected"
            ? "Connected to server"
            : connectionStatus === "connecting"
              ? "Connecting..."
              : "Demo mode (no server)"}
        </div>

        {/* Seed mock data — only when connected */}
        {connectionStatus === "connected" && cxpClient && (
          <div
            style={{
              padding: "12px 16px",
              borderBottom: "1px solid #e0e0e0",
            }}
          >
            <button
              onClick={async () => {
                setSeeding(true);
                setSeedError(null);
                try {
                  const ids = await seedMockData(cxpClient);
                  setCustomerId(ids.customerId);
                  setSurveyId(ids.surveyId);
                  setSessionId(ids.sessionId);
                } catch (err) {
                  setSeedError((err as Error).message);
                } finally {
                  setSeeding(false);
                }
              }}
              disabled={seeding}
              style={{
                width: "100%",
                padding: "8px 12px",
                backgroundColor: seeding ? "#bdbdbd" : "#388e3c",
                color: "#fff",
                border: "none",
                borderRadius: 4,
                fontSize: 13,
                fontWeight: 600,
                cursor: seeding ? "not-allowed" : "pointer",
              }}
            >
              {seeding ? "Seeding..." : "Seed Mock Data"}
            </button>
            {seedError && (
              <div style={{ color: "#d32f2f", fontSize: 11, marginTop: 4 }}>
                {seedError}
              </div>
            )}
          </div>
        )}

        {/* ID inputs — only when connected */}
        {connectionStatus === "connected" && (
          <div
            style={{
              padding: "12px 16px",
              borderBottom: "1px solid #e0e0e0",
              display: "flex",
              flexDirection: "column",
              gap: 8,
              fontSize: 12,
            }}
          >
            <label>
              Customer ID
              <input
                type="text"
                value={customerId}
                onChange={(e) => setCustomerId(e.target.value)}
                placeholder="paste UUID"
                style={{
                  display: "block",
                  width: "100%",
                  padding: "4px 6px",
                  fontSize: 11,
                  border: "1px solid #ccc",
                  borderRadius: 3,
                  marginTop: 2,
                  boxSizing: "border-box",
                }}
              />
            </label>
            <label>
              Survey ID
              <input
                type="text"
                value={surveyId}
                onChange={(e) => setSurveyId(e.target.value)}
                placeholder="paste UUID"
                style={{
                  display: "block",
                  width: "100%",
                  padding: "4px 6px",
                  fontSize: 11,
                  border: "1px solid #ccc",
                  borderRadius: 3,
                  marginTop: 2,
                  boxSizing: "border-box",
                }}
              />
            </label>
            <label>
              Session ID
              <input
                type="text"
                value={sessionId}
                onChange={(e) => setSessionId(e.target.value)}
                placeholder="paste UUID"
                style={{
                  display: "block",
                  width: "100%",
                  padding: "4px 6px",
                  fontSize: 11,
                  border: "1px solid #ccc",
                  borderRadius: 3,
                  marginTop: 2,
                  boxSizing: "border-box",
                }}
              />
            </label>
          </div>
        )}

        <ul style={{ listStyle: "none", margin: 0, padding: "8px 0" }}>
          {demos.map((name) => (
            <li key={name}>
              <button
                onClick={() => setActive(name)}
                style={{
                  display: "block",
                  width: "100%",
                  padding: "10px 16px",
                  border: "none",
                  background:
                    active === name ? "#e3f2fd" : "transparent",
                  color: active === name ? "#1565c0" : "#424242",
                  fontWeight: active === name ? 600 : 400,
                  fontSize: 14,
                  textAlign: "left",
                  cursor: "pointer",
                }}
              >
                {name}
              </button>
            </li>
          ))}
        </ul>
      </nav>

      {/* Main content */}
      <main
        style={{ flex: 1, backgroundColor: "#f5f5f5", overflow: "auto" }}
      >
        <DemoContent
          active={active}
          customerId={customerId}
          surveyId={surveyId}
          sessionId={sessionId}
        />
      </main>
    </div>
  );

  // Wrap in CxpProvider only when connected
  if (cxpClient) {
    return <CxpProvider client={cxpClient}>{content}</CxpProvider>;
  }

  return content;
}
