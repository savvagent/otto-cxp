import { useCustomer } from "../hooks/useCustomer.js";

export interface CustomerCardProps {
  customerId: string;
}

/**
 * Component to display customer summary card
 * @example
 * <CustomerCard customerId="cust_123" />
 */
export function CustomerCard({ customerId }: CustomerCardProps) {
  const { customer, loading, error } = useCustomer(customerId);

  if (loading) {
    return (
      <div
        style={{
          border: "1px solid #e0e0e0",
          borderRadius: "8px",
          padding: "24px",
          backgroundColor: "#fff",
        }}
      >
        Loading customer...
      </div>
    );
  }

  if (error) {
    return (
      <div
        style={{
          border: "1px solid #e0e0e0",
          borderRadius: "8px",
          padding: "24px",
          backgroundColor: "#fff",
          color: "#d32f2f",
        }}
      >
        Error: {error.message}
      </div>
    );
  }

  if (!customer) {
    return (
      <div
        style={{
          border: "1px solid #e0e0e0",
          borderRadius: "8px",
          padding: "24px",
          backgroundColor: "#fff",
        }}
      >
        Customer not found.
      </div>
    );
  }

  return (
    <div
      style={{
        border: "1px solid #e0e0e0",
        borderRadius: "8px",
        padding: "24px",
        backgroundColor: "#fff",
      }}
    >
      <h3
        style={{
          fontSize: "20px",
          fontWeight: "600",
          margin: "0 0 16px 0",
        }}
      >
        {customer.name}
      </h3>

      <div style={{ display: "grid", gap: "12px", fontSize: "14px" }}>
        <div style={{ display: "flex", gap: "8px" }}>
          <span style={{ fontWeight: "600", color: "#616161" }}>Email:</span>
          <a
            href={`mailto:${customer.email}`}
            style={{ color: "#1976d2", textDecoration: "none" }}
          >
            {customer.email}
          </a>
        </div>

        {customer.company && (
          <div style={{ display: "flex", gap: "8px" }}>
            <span style={{ fontWeight: "600", color: "#616161" }}>Company:</span>
            <span style={{ color: "#424242" }}>{customer.company}</span>
          </div>
        )}

        {customer.tags && customer.tags.length > 0 && (
          <div style={{ marginTop: "8px" }}>
            <span
              style={{
                fontWeight: "600",
                color: "#616161",
                fontSize: "14px",
                marginBottom: "8px",
                display: "block",
              }}
            >
              Tags:
            </span>
            <div style={{ display: "flex", gap: "8px", flexWrap: "wrap" }}>
              {customer.tags.map((tag, index) => (
                <span
                  key={index}
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
          </div>
        )}
      </div>
    </div>
  );
}
