import React, { useState, useEffect } from "react";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";
import { CxpClient } from "@cxp/sdk";
import { CxpProvider } from "@cxp/react";
import {
  TicketList,
  TicketDetail,
  CustomerCard,
  Customer360,
  ChatWidget,
  FeedbackForm,
  SurveyRenderer,
  NpsGauge,
  MetricsDashboard,
} from "@cxp/react";
import { seedMockData } from "./mockData";

const demos = [
  "MetricsDashboard",
  "TicketList",
  "TicketDetail",
  "CustomerCard",
  "Customer360",
  "ChatWidget",
  "FeedbackForm",
  "SurveyRenderer",
  "NpsGauge",
] as const;

type Demo = (typeof demos)[number];

const idRequirements: Record<Demo, string[]> = {
  MetricsDashboard: [],
  TicketList: [],
  TicketDetail: ["Ticket ID"],
  CustomerCard: ["Customer ID"],
  Customer360: ["Customer ID"],
  ChatWidget: ["Session ID"],
  FeedbackForm: ["Customer ID"],
  SurveyRenderer: ["Survey ID", "Customer ID"],
  NpsGauge: ["Survey ID"],
};

function MissingId({ fields }: { fields: string[] }) {
  return (
    <div style={{ padding: 40, color: "#757575", fontFamily: "system-ui, sans-serif" }}>
      <p style={{ margin: "0 0 8px", fontWeight: 500 }}>
        {fields.join(" and ")} required.
      </p>
      <p style={{ margin: 0, fontSize: 14 }}>
        Click <strong>Seed Mock Data</strong> or paste {fields.length > 1 ? "IDs" : "an ID"} in the sidebar.
      </p>
    </div>
  );
}

function DemoContent({
  active,
  customerId,
  ticketId,
  surveyId,
  sessionId,
}: {
  active: Demo;
  customerId: string;
  ticketId: string;
  surveyId: string;
  sessionId: string;
}) {
  const ids: Record<string, string> = {
    "Customer ID": customerId,
    "Ticket ID": ticketId,
    "Survey ID": surveyId,
    "Session ID": sessionId,
  };
  const missing = idRequirements[active].filter((f) => !ids[f]);
  if (missing.length > 0) {
    return <MissingId fields={missing} />;
  }

  switch (active) {
    case "MetricsDashboard":
      return <MetricsDashboard />;
    case "TicketList":
      return <TicketList customerId={customerId} />;
    case "TicketDetail":
      return <TicketDetail ticketId={ticketId} />;
    case "CustomerCard":
      return <CustomerCard customerId={customerId} />;
    case "Customer360":
      return <Customer360 customerId={customerId} />;
    case "ChatWidget":
      return <ChatWidget sessionId={sessionId} />;
    case "FeedbackForm":
      return (
        <FeedbackForm
          customerId={customerId}
          onSubmit={(fb) => console.log("Feedback submitted:", fb)}
        />
      );
    case "SurveyRenderer":
      return (
        <SurveyRenderer
          surveyId={surveyId}
          customerId={customerId}
          onComplete={() => console.log("Survey completed")}
        />
      );
    case "NpsGauge":
      return <NpsGauge surveyId={surveyId} />;
  }
}

export default function App() {
  const [active, setActive] = useState<Demo>("MetricsDashboard");
  const [cxpClient, setCxpClient] = useState<CxpClient | null>(null);
  const [error, setError] = useState<string | null>(null);

  // IDs for demo - user can change customerId
  const [customerId, setCustomerId] = useState("");
  const [ticketId, setTicketId] = useState("");
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
      } catch (err) {
        if (!cancelled) {
          setError((err as Error).message);
        }
      }
    }

    connect();
    return () => {
      cancelled = true;
    };
  }, []);

  if (error) {
    return (
      <div
        style={{
          padding: 40,
          fontFamily: "system-ui, sans-serif",
          color: "#d32f2f",
        }}
      >
        <h2>Connection Error</h2>
        <p>{error}</p>
        <p style={{ color: "#666" }}>
          Make sure the CXP server is running:{" "}
          <code>TRANSPORT=http cargo run -p cxp-server</code>
        </p>
      </div>
    );
  }

  if (!cxpClient) {
    return (
      <div
        style={{
          padding: 40,
          fontFamily: "system-ui, sans-serif",
          color: "#666",
        }}
      >
        Connecting to CXP server...
      </div>
    );
  }

  return (
    <CxpProvider client={cxpClient}>
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

          {/* Seed mock data */}
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
                  setTicketId(ids.ticketId);
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

          {/* ID inputs */}
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
              Ticket ID
              <input
                type="text"
                value={ticketId}
                onChange={(e) => setTicketId(e.target.value)}
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
            ticketId={ticketId}
            surveyId={surveyId}
            sessionId={sessionId}
          />
        </main>
      </div>
    </CxpProvider>
  );
}
