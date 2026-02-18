import { useState, useEffect } from "react";
import { useCxp } from "../context/CxpProvider.js";

export interface NpsGaugeProps {
  surveyId: string;
}

/**
 * Component to display NPS (Net Promoter Score) with colored indicator
 * @example
 * <NpsGauge surveyId="survey_123" />
 */
export function NpsGauge({ surveyId }: NpsGaugeProps) {
  const cxp = useCxp();
  const [nps, setNps] = useState<number | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<Error | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    cxp.feedback
      .calculateNps(surveyId)
      .then((score) => {
        if (!cancelled) {
          setNps(score.score);
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
  }, [cxp, surveyId]);

  const getColor = (score: number) => {
    if (score < 0) return "#d32f2f"; // Red
    if (score <= 50) return "#f57c00"; // Yellow/Orange
    return "#388e3c"; // Green
  };

  const getLabel = (score: number) => {
    if (score < 0) return "Poor";
    if (score <= 50) return "Good";
    return "Excellent";
  };

  if (loading) {
    return (
      <div
        style={{
          padding: "24px",
          border: "1px solid #e0e0e0",
          borderRadius: "8px",
          backgroundColor: "#fff",
          textAlign: "center",
        }}
      >
        Loading NPS...
      </div>
    );
  }

  if (error) {
    return (
      <div
        style={{
          padding: "24px",
          border: "1px solid #e0e0e0",
          borderRadius: "8px",
          backgroundColor: "#fff",
          textAlign: "center",
          color: "#d32f2f",
        }}
      >
        Error: {error.message}
      </div>
    );
  }

  if (nps === null) {
    return (
      <div
        style={{
          padding: "24px",
          border: "1px solid #e0e0e0",
          borderRadius: "8px",
          backgroundColor: "#fff",
          textAlign: "center",
        }}
      >
        No NPS data available
      </div>
    );
  }

  return (
    <div
      style={{
        padding: "32px",
        border: "1px solid #e0e0e0",
        borderRadius: "8px",
        backgroundColor: "#fff",
        textAlign: "center",
      }}
    >
      <h3
        style={{
          fontSize: "18px",
          fontWeight: "600",
          marginBottom: "24px",
          color: "#616161",
        }}
      >
        Net Promoter Score
      </h3>

      <div
        style={{
          display: "inline-flex",
          flexDirection: "column",
          alignItems: "center",
          gap: "16px",
        }}
      >
        <div
          style={{
            width: "160px",
            height: "160px",
            borderRadius: "50%",
            backgroundColor: getColor(nps),
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            boxShadow: "0 4px 8px rgba(0,0,0,0.1)",
          }}
        >
          <div style={{ textAlign: "center" }}>
            <div
              style={{
                fontSize: "48px",
                fontWeight: "700",
                color: "#fff",
                lineHeight: "1",
              }}
            >
              {nps}
            </div>
            <div
              style={{
                fontSize: "16px",
                fontWeight: "500",
                color: "#fff",
                marginTop: "8px",
              }}
            >
              {getLabel(nps)}
            </div>
          </div>
        </div>

        <div
          style={{
            display: "flex",
            gap: "24px",
            fontSize: "12px",
            color: "#757575",
          }}
        >
          <div style={{ textAlign: "center" }}>
            <div
              style={{
                width: "12px",
                height: "12px",
                borderRadius: "50%",
                backgroundColor: "#d32f2f",
                margin: "0 auto 4px",
              }}
            />
            <div>&lt; 0</div>
          </div>
          <div style={{ textAlign: "center" }}>
            <div
              style={{
                width: "12px",
                height: "12px",
                borderRadius: "50%",
                backgroundColor: "#f57c00",
                margin: "0 auto 4px",
              }}
            />
            <div>0-50</div>
          </div>
          <div style={{ textAlign: "center" }}>
            <div
              style={{
                width: "12px",
                height: "12px",
                borderRadius: "50%",
                backgroundColor: "#388e3c",
                margin: "0 auto 4px",
              }}
            />
            <div>&gt; 50</div>
          </div>
        </div>
      </div>
    </div>
  );
}
