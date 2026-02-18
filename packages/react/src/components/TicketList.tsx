import { useTickets } from "../hooks/useTickets.js";

export interface TicketListProps {
  customerId?: string;
  status?: string;
}

/**
 * Component to display a list of tickets
 * @example
 * <TicketList customerId="cust_123" status="open" />
 */
export function TicketList({ customerId, status }: TicketListProps) {
  const { tickets, loading, error } = useTickets({ customerId, status });

  if (loading) {
    return <div style={{ padding: "20px" }}>Loading tickets...</div>;
  }

  if (error) {
    return (
      <div style={{ padding: "20px", color: "#d32f2f" }}>
        Error: {error.message}
      </div>
    );
  }

  if (tickets.length === 0) {
    return <div style={{ padding: "20px" }}>No tickets found.</div>;
  }

  return (
    <div style={{ padding: "20px" }}>
      <h2 style={{ marginBottom: "20px", fontSize: "24px", fontWeight: "600" }}>
        Tickets
      </h2>
      <div style={{ display: "flex", flexDirection: "column", gap: "12px" }}>
        {tickets.map((ticket) => (
          <div
            key={ticket.id}
            style={{
              border: "1px solid #e0e0e0",
              borderRadius: "8px",
              padding: "16px",
              backgroundColor: "#fff",
            }}
          >
            <div
              style={{
                display: "flex",
                justifyContent: "space-between",
                alignItems: "start",
                marginBottom: "8px",
              }}
            >
              <h3 style={{ fontSize: "18px", fontWeight: "500", margin: 0 }}>
                {ticket.subject}
              </h3>
              <span
                style={{
                  padding: "4px 12px",
                  borderRadius: "12px",
                  fontSize: "12px",
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
            </div>
            <div
              style={{
                display: "flex",
                gap: "16px",
                fontSize: "14px",
                color: "#666",
              }}
            >
              <span>Priority: {ticket.priority}</span>
              <span>
                Created: {new Date(ticket.createdAt).toLocaleDateString()}
              </span>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
