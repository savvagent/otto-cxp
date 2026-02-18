import { useMetrics } from "../hooks/useMetrics.js";

/**
 * Component to display key metrics dashboard
 * @example
 * <MetricsDashboard />
 */
export function MetricsDashboard() {
  const { metrics, loading, error } = useMetrics();

  if (loading) {
    return <div style={{ padding: "20px" }}>Loading metrics...</div>;
  }

  if (error) {
    return (
      <div style={{ padding: "20px", color: "#d32f2f" }}>
        Error: {error.message}
      </div>
    );
  }

  if (!metrics) {
    return <div style={{ padding: "20px" }}>No metrics available.</div>;
  }

  return (
    <div style={{ padding: "20px" }}>
      <h2 style={{ fontSize: "28px", fontWeight: "600", marginBottom: "24px" }}>
        Dashboard Metrics
      </h2>

      <div
        style={{
          display: "grid",
          gridTemplateColumns: "repeat(auto-fit, minmax(250px, 1fr))",
          gap: "20px",
        }}
      >
        {/* Total Tickets */}
        <div
          style={{
            padding: "24px",
            border: "1px solid #e0e0e0",
            borderRadius: "8px",
            backgroundColor: "#fff",
          }}
        >
          <h3
            style={{
              fontSize: "14px",
              fontWeight: "500",
              color: "#757575",
              marginBottom: "8px",
            }}
          >
            Total Tickets
          </h3>
          <div style={{ fontSize: "36px", fontWeight: "700", color: "#212121" }}>
            {metrics.totalTickets}
          </div>
        </div>

        {/* Open Tickets */}
        <div
          style={{
            padding: "24px",
            border: "1px solid #e0e0e0",
            borderRadius: "8px",
            backgroundColor: "#fff",
          }}
        >
          <h3
            style={{
              fontSize: "14px",
              fontWeight: "500",
              color: "#757575",
              marginBottom: "8px",
            }}
          >
            Open Tickets
          </h3>
          <div style={{ fontSize: "36px", fontWeight: "700", color: "#1976d2" }}>
            {metrics.openTickets}
          </div>
        </div>

        {/* In Progress */}
        <div
          style={{
            padding: "24px",
            border: "1px solid #e0e0e0",
            borderRadius: "8px",
            backgroundColor: "#fff",
          }}
        >
          <h3
            style={{
              fontSize: "14px",
              fontWeight: "500",
              color: "#757575",
              marginBottom: "8px",
            }}
          >
            In Progress
          </h3>
          <div style={{ fontSize: "36px", fontWeight: "700", color: "#f57c00" }}>
            {metrics.inProgressTickets}
          </div>
        </div>

        {/* Resolved */}
        <div
          style={{
            padding: "24px",
            border: "1px solid #e0e0e0",
            borderRadius: "8px",
            backgroundColor: "#fff",
          }}
        >
          <h3
            style={{
              fontSize: "14px",
              fontWeight: "500",
              color: "#757575",
              marginBottom: "8px",
            }}
          >
            Resolved Tickets
          </h3>
          <div style={{ fontSize: "36px", fontWeight: "700", color: "#388e3c" }}>
            {metrics.resolvedTickets}
          </div>
        </div>

        {/* Closed */}
        <div
          style={{
            padding: "24px",
            border: "1px solid #e0e0e0",
            borderRadius: "8px",
            backgroundColor: "#fff",
          }}
        >
          <h3
            style={{
              fontSize: "14px",
              fontWeight: "500",
              color: "#757575",
              marginBottom: "8px",
            }}
          >
            Closed Tickets
          </h3>
          <div style={{ fontSize: "36px", fontWeight: "700", color: "#616161" }}>
            {metrics.closedTickets}
          </div>
        </div>

        {/* CSAT Score */}
        {metrics.csatScore !== undefined && (
          <div
            style={{
              padding: "24px",
              border: "1px solid #e0e0e0",
              borderRadius: "8px",
              backgroundColor: "#fff",
            }}
          >
            <h3
              style={{
                fontSize: "14px",
                fontWeight: "500",
                color: "#757575",
                marginBottom: "8px",
              }}
            >
              CSAT Score
            </h3>
            <div
              style={{ fontSize: "36px", fontWeight: "700", color: "#1976d2" }}
            >
              {metrics.csatScore.toFixed(1)}%
            </div>
          </div>
        )}

        {/* Average Rating */}
        {metrics.averageRating !== undefined && (
          <div
            style={{
              padding: "24px",
              border: "1px solid #e0e0e0",
              borderRadius: "8px",
              backgroundColor: "#fff",
            }}
          >
            <h3
              style={{
                fontSize: "14px",
                fontWeight: "500",
                color: "#757575",
                marginBottom: "8px",
              }}
            >
              Average Rating
            </h3>
            <div
              style={{ fontSize: "36px", fontWeight: "700", color: "#f57c00" }}
            >
              {metrics.averageRating.toFixed(1)}/5
            </div>
          </div>
        )}

        {/* Average Resolution Time */}
        {metrics.averageResolutionTime !== undefined && (
          <div
            style={{
              padding: "24px",
              border: "1px solid #e0e0e0",
              borderRadius: "8px",
              backgroundColor: "#fff",
            }}
          >
            <h3
              style={{
                fontSize: "14px",
                fontWeight: "500",
                color: "#757575",
                marginBottom: "8px",
              }}
            >
              Avg Resolution Time
            </h3>
            <div
              style={{ fontSize: "36px", fontWeight: "700", color: "#388e3c" }}
            >
              {Math.round(metrics.averageResolutionTime / 3600)}h
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
