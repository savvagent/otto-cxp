import { useState } from "react";
import { useCxpOptional } from "../context/CxpProvider.js";

export interface QuickRatingLabels {
  title?: string;
  thankYou?: string;
  errorPrefix?: string;
  rating1?: string;
  rating2?: string;
  rating3?: string;
  rating4?: string;
  rating5?: string;
}

const DEFAULT_LABELS: Required<QuickRatingLabels> = {
  title: "How was your experience?",
  thankYou: "Thanks for your feedback!",
  errorPrefix: "Error:",
  rating1: "Terrible",
  rating2: "Bad",
  rating3: "Okay",
  rating4: "Good",
  rating5: "Great",
};

const EMOTICONS = ["😠", "😟", "😐", "🙂", "😄"];

export interface QuickRatingProps {
  customerId?: string;
  feedbackType?: string;
  /** Catalog experience UUID, sent as `experience_id`. */
  experience?: string;
  /** Catalog application UUID, sent as `application_id`. */
  application?: string;
  /** Catalog page UUID, sent as `page_id`. */
  page?: string;
  /** Catalog component UUID, sent as `component_id`. */
  component?: string;
  labels?: QuickRatingLabels;
  onSubmit?: (rating: number) => void;
}

export function QuickRating({
  customerId = "demo-customer",
  feedbackType = "praise",
  experience,
  application,
  page,
  component,
  labels: labelOverrides,
  onSubmit,
}: QuickRatingProps) {
  const l = { ...DEFAULT_LABELS, ...labelOverrides };
  const tooltips = [l.rating1, l.rating2, l.rating3, l.rating4, l.rating5];
  const cxp = useCxpOptional();
  const [hovered, setHovered] = useState<number | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const [submitted, setSubmitted] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleSelect = async (rating: number) => {
    setSelected(rating);
    setError(null);

    try {
      if (cxp) {
        await cxp.feedback.submitFeedback(customerId, feedbackType, {
          rating,
          subject: `Quick rating: ${rating}/5`,
          experienceId: experience,
          applicationId: application,
          pageId: page,
          componentId: component,
        });
      } else {
        await new Promise((r) => setTimeout(r, 400));
      }
      setSubmitted(true);
      onSubmit?.(rating);
    } catch (err) {
      setError((err as Error).message);
      setSelected(null);
    }
  };

  if (submitted) {
    return (
      <div style={{ textAlign: "center", padding: "16px" }}>
        <div style={{ fontSize: "32px", marginBottom: "8px" }}>
          {EMOTICONS[(selected ?? 3) - 1]}
        </div>
        <div style={{ fontSize: "14px", color: "#2e7d32" }}>{l.thankYou}</div>
      </div>
    );
  }

  return (
    <div style={{ textAlign: "center", padding: "16px" }}>
      <div
        style={{
          fontSize: "14px",
          fontWeight: "500",
          marginBottom: "12px",
        }}
      >
        {l.title}
      </div>
      <div
        style={{
          display: "inline-flex",
          gap: "4px",
        }}
      >
        {EMOTICONS.map((emoji, i) => {
          const rating = i + 1;
          const isActive = selected === rating || hovered === rating;
          return (
            <button
              key={rating}
              type="button"
              title={tooltips[i]}
              onClick={() => handleSelect(rating)}
              onMouseEnter={() => setHovered(rating)}
              onMouseLeave={() => setHovered(null)}
              disabled={selected !== null}
              style={{
                fontSize: "28px",
                padding: "8px",
                border: "2px solid",
                borderColor: isActive ? "#1976d2" : "transparent",
                borderRadius: "8px",
                backgroundColor: isActive ? "#e3f2fd" : "transparent",
                cursor: selected !== null ? "default" : "pointer",
                opacity: selected !== null && selected !== rating ? 0.4 : 1,
                transition: "opacity 0.2s, background-color 0.2s, border-color 0.2s",
                lineHeight: 1,
              }}
            >
              {emoji}
            </button>
          );
        })}
      </div>
      {error && (
        <div
          style={{
            marginTop: "8px",
            fontSize: "13px",
            color: "#c62828",
          }}
        >
          {l.errorPrefix} {error}
        </div>
      )}
    </div>
  );
}
