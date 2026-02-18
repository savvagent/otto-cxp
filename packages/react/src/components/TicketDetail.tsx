import { useState, useEffect } from "react";
import type { Ticket } from "@cxp/sdk";
import { useCxp } from "../context/CxpProvider.js";

export interface TicketDetailProps {
  ticketId: string;
}

/**
 * Component to display detailed ticket information
 * @example
 * <TicketDetail ticketId="ticket_123" />
 */
export function TicketDetail({ ticketId }: TicketDetailProps) {
  const cxp = useCxp();
  const [ticket, setTicket] = useState<Ticket | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<Error | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    cxp.support
      .getTicket(ticketId)
      .then((data) => {
        if (!cancelled) {
          setTicket(data);
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
  }, [cxp, ticketId]);

  if (loading) {
    return <div style={{ padding: "20px" }}>Loading ticket...</div>;
  }

  if (error) {
    return (
      <div style={{ padding: "20px", color: "#d32f2f" }}>
        Error: {error.message}
      </div>
    );
  }

  if (!ticket) {
    return <div style={{ padding: "20px" }}>Ticket not found.</div>;
  }

  return (
    <div style={{ padding: "20px", maxWidth: "800px" }}>
      <div
        style={{
          border: "1px solid #e0e0e0",
          borderRadius: "8px",
          padding: "24px",
          backgroundColor: "#fff",
        }}
      >
        <div style={{ marginBottom: "24px" }}>
          <h2 style={{ fontSize: "24px", fontWeight: "600", margin: "0 0 16px 0" }}>
            {ticket.subject}
          </h2>
          <div style={{ display: "flex", gap: "12px", flexWrap: "wrap" }}>
            <span
              style={{
                padding: "6px 16px",
                borderRadius: "16px",
                fontSize: "14px",
                fontWeight: "500",
                backgroundColor:
                  ticket.status === "open"
                    ? "#e3f2fd"
                    : ticket.status === "in_progress"
                    ? "#fff3e0"
                    : ticket.status === "resolved"
                    ? "#e8f5e9"
                    : "#f5f5f5",
                color:
                  ticket.status === "open"
                    ? "#1976d2"
                    : ticket.status === "in_progress"
                    ? "#f57c00"
                    : ticket.status === "resolved"
                    ? "#388e3c"
                    : "#616161",
              }}
            >
              {ticket.status}
            </span>
            <span
              style={{
                padding: "6px 16px",
                borderRadius: "16px",
                fontSize: "14px",
                fontWeight: "500",
                backgroundColor:
                  ticket.priority === "high"
                    ? "#ffebee"
                    : ticket.priority === "medium"
                    ? "#fff3e0"
                    : "#f5f5f5",
                color:
                  ticket.priority === "high"
                    ? "#c62828"
                    : ticket.priority === "medium"
                    ? "#f57c00"
                    : "#616161",
              }}
            >
              {ticket.priority} priority
            </span>
          </div>
        </div>

        <div style={{ marginBottom: "24px" }}>
          <h3 style={{ fontSize: "16px", fontWeight: "600", marginBottom: "8px" }}>
            Description
          </h3>
          <p style={{ margin: 0, lineHeight: "1.6", color: "#424242" }}>
            {ticket.description}
          </p>
        </div>

        <div style={{ display: "grid", gap: "12px", fontSize: "14px" }}>
          <div style={{ display: "flex", gap: "8px" }}>
            <span style={{ fontWeight: "600", color: "#616161" }}>Customer ID:</span>
            <span style={{ color: "#424242" }}>{ticket.customerId}</span>
          </div>
          {ticket.assignedTo && (
            <div style={{ display: "flex", gap: "8px" }}>
              <span style={{ fontWeight: "600", color: "#616161" }}>Assigned To:</span>
              <span style={{ color: "#424242" }}>{ticket.assignedTo}</span>
            </div>
          )}
          <div style={{ display: "flex", gap: "8px" }}>
            <span style={{ fontWeight: "600", color: "#616161" }}>Created:</span>
            <span style={{ color: "#424242" }}>
              {new Date(ticket.createdAt).toLocaleString()}
            </span>
          </div>
          <div style={{ display: "flex", gap: "8px" }}>
            <span style={{ fontWeight: "600", color: "#616161" }}>Updated:</span>
            <span style={{ color: "#424242" }}>
              {new Date(ticket.updatedAt).toLocaleString()}
            </span>
          </div>
          {ticket.resolvedAt && (
            <div style={{ display: "flex", gap: "8px" }}>
              <span style={{ fontWeight: "600", color: "#616161" }}>Resolved:</span>
              <span style={{ color: "#424242" }}>
                {new Date(ticket.resolvedAt).toLocaleString()}
              </span>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
