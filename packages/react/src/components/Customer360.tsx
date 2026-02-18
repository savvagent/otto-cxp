import { useState, useEffect } from "react";
import type { Customer360 as Customer360Type } from "@cxp/sdk";
import { useCxp } from "../context/CxpProvider.js";

export interface Customer360Props {
  customerId: string;
}

/**
 * Component to display comprehensive 360-degree customer view
 * @example
 * <Customer360 customerId="cust_123" />
 */
export function Customer360({ customerId }: Customer360Props) {
  const cxp = useCxp();
  const [view, setView] = useState<Customer360Type | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<Error | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    cxp.cdp
      .getCustomer360(customerId)
      .then((data) => {
        if (!cancelled) {
          setView(data);
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
  }, [cxp, customerId]);

  if (loading) {
    return <div style={{ padding: "20px" }}>Loading customer 360 view...</div>;
  }

  if (error) {
    return (
      <div style={{ padding: "20px", color: "#d32f2f" }}>
        Error: {error.message}
      </div>
    );
  }

  if (!view) {
    return <div style={{ padding: "20px" }}>Customer data not found.</div>;
  }

  return (
    <div style={{ padding: "20px", maxWidth: "1200px" }}>
      <h2 style={{ fontSize: "28px", fontWeight: "600", marginBottom: "24px" }}>
        Customer 360 View
      </h2>

      {/* Customer Info Section */}
      <section style={{ marginBottom: "32px" }}>
        <h3 style={{ fontSize: "20px", fontWeight: "600", marginBottom: "16px" }}>
          Customer Information
        </h3>
        <div
          style={{
            border: "1px solid #e0e0e0",
            borderRadius: "8px",
            padding: "20px",
            backgroundColor: "#fff",
          }}
        >
          <div style={{ display: "grid", gap: "12px", fontSize: "14px" }}>
            <div>
              <strong style={{ color: "#616161" }}>Name:</strong>{" "}
              {view.customer.name}
            </div>
            <div>
              <strong style={{ color: "#616161" }}>Email:</strong>{" "}
              {view.customer.email}
            </div>
            {view.customer.company && (
              <div>
                <strong style={{ color: "#616161" }}>Company:</strong>{" "}
                {view.customer.company}
              </div>
            )}
            {view.customer.tags && view.customer.tags.length > 0 && (
              <div style={{ display: "flex", gap: "8px", alignItems: "center" }}>
                <strong style={{ color: "#616161" }}>Tags:</strong>
                {view.customer.tags.map((tag, i) => (
                  <span
                    key={i}
                    style={{
                      padding: "4px 12px",
                      borderRadius: "12px",
                      fontSize: "12px",
                      backgroundColor: "#e3f2fd",
                      color: "#1976d2",
                    }}
                  >
                    {tag}
                  </span>
                ))}
              </div>
            )}
          </div>
        </div>
      </section>

      {/* Recent Tickets Section */}
      <section style={{ marginBottom: "32px" }}>
        <h3 style={{ fontSize: "20px", fontWeight: "600", marginBottom: "16px" }}>
          Recent Tickets
        </h3>
        {view.recentTickets.length === 0 ? (
          <div style={{ padding: "20px", color: "#757575" }}>No tickets found.</div>
        ) : (
          <div style={{ display: "flex", flexDirection: "column", gap: "12px" }}>
            {view.recentTickets.map((ticket) => (
              <div
                key={ticket.id}
                style={{
                  border: "1px solid #e0e0e0",
                  borderRadius: "8px",
                  padding: "16px",
                  backgroundColor: "#fff",
                }}
              >
                <div style={{ display: "flex", justifyContent: "space-between" }}>
                  <strong>{ticket.subject}</strong>
                  <span
                    style={{
                      padding: "4px 12px",
                      borderRadius: "12px",
                      fontSize: "12px",
                      backgroundColor:
                        ticket.status === "open" ? "#e3f2fd" : "#e8f5e9",
                      color: ticket.status === "open" ? "#1976d2" : "#388e3c",
                    }}
                  >
                    {ticket.status}
                  </span>
                </div>
                <div style={{ fontSize: "13px", color: "#666", marginTop: "8px" }}>
                  {new Date(ticket.createdAt).toLocaleDateString()}
                </div>
              </div>
            ))}
          </div>
        )}
      </section>

      {/* Recent Interactions Section */}
      <section style={{ marginBottom: "32px" }}>
        <h3 style={{ fontSize: "20px", fontWeight: "600", marginBottom: "16px" }}>
          Recent Interactions
        </h3>
        {view.recentInteractions.length === 0 ? (
          <div style={{ padding: "20px", color: "#757575" }}>
            No interactions found.
          </div>
        ) : (
          <div style={{ display: "flex", flexDirection: "column", gap: "12px" }}>
            {view.recentInteractions.map((interaction) => (
              <div
                key={interaction.id}
                style={{
                  border: "1px solid #e0e0e0",
                  borderRadius: "8px",
                  padding: "16px",
                  backgroundColor: "#fff",
                }}
              >
                <div style={{ display: "flex", justifyContent: "space-between" }}>
                  <strong>{interaction.interactionType}</strong>
                  <span style={{ fontSize: "13px", color: "#666" }}>
                    {new Date(interaction.createdAt).toLocaleDateString()}
                  </span>
                </div>
              </div>
            ))}
          </div>
        )}
      </section>

      {/* Recent Feedback Section */}
      <section>
        <h3 style={{ fontSize: "20px", fontWeight: "600", marginBottom: "16px" }}>
          Recent Feedback
        </h3>
        {view.recentFeedback.length === 0 ? (
          <div style={{ padding: "20px", color: "#757575" }}>No feedback found.</div>
        ) : (
          <div style={{ display: "flex", flexDirection: "column", gap: "12px" }}>
            {view.recentFeedback.map((feedback) => (
              <div
                key={feedback.id}
                style={{
                  border: "1px solid #e0e0e0",
                  borderRadius: "8px",
                  padding: "16px",
                  backgroundColor: "#fff",
                }}
              >
                <div style={{ display: "flex", justifyContent: "space-between" }}>
                  <strong>{feedback.subject}</strong>
                  {feedback.rating && (
                    <span style={{ color: "#f57c00" }}>
                      {feedback.rating}/5
                    </span>
                  )}
                </div>
                <div style={{ fontSize: "13px", color: "#666", marginTop: "8px" }}>
                  {feedback.feedbackType} - {new Date(feedback.createdAt).toLocaleDateString()}
                </div>
              </div>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}
