import React, { useState } from "react";
import type { Feedback } from "@cxp/sdk";
import { useCxp } from "../context/CxpProvider.js";

export interface FeedbackFormProps {
  customerId: string;
  experience?: string;
  application?: string;
  page?: string;
  component?: string;
  onSubmit?: (feedback: Feedback) => void;
}

/**
 * Component for submitting customer feedback
 * @example
 * <FeedbackForm
 *   customerId="cust_123"
 *   application="Dashboard"
 *   page="/settings"
 *   onSubmit={(feedback) => console.log("Submitted:", feedback)}
 * />
 */
export function FeedbackForm({ customerId, experience, application, page, component, onSubmit }: FeedbackFormProps) {
  const cxp = useCxp();
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const [success, setSuccess] = useState(false);

  const [formData, setFormData] = useState({
    feedbackType: "bug_report",
    subject: "",
    body: "",
    rating: 3,
  });

  const handleChange = (
    e: React.ChangeEvent<
      HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement
    >
  ) => {
    const { name, value } = e.target;
    setFormData((prev) => ({
      ...prev,
      [name]: name === "rating" ? parseInt(value, 10) : value,
    }));
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setLoading(true);
    setError(null);
    setSuccess(false);

    try {
      const feedback = await cxp.feedback.submitFeedback(
        customerId,
        formData.feedbackType,
        {
          subject: formData.subject,
          body: formData.body,
          rating: formData.rating,
          experience,
          application,
          page,
          component,
        },
      );

      setSuccess(true);
      setFormData({
        feedbackType: "bug_report",
        subject: "",
        body: "",
        rating: 3,
      });

      onSubmit?.(feedback);
    } catch (err) {
      setError(err as Error);
    } finally {
      setLoading(false);
    }
  };

  return (
    <div style={{ maxWidth: "600px", padding: "20px" }}>
      <h2 style={{ fontSize: "24px", fontWeight: "600", marginBottom: "24px" }}>
        Submit Feedback
      </h2>

      <form
        onSubmit={handleSubmit}
        style={{ display: "flex", flexDirection: "column", gap: "20px" }}
      >
        {/* Feedback Type */}
        <div>
          <label
            htmlFor="feedbackType"
            style={{
              display: "block",
              marginBottom: "8px",
              fontWeight: "500",
              fontSize: "14px",
            }}
          >
            Feedback Type
          </label>
          <select
            id="feedbackType"
            name="feedbackType"
            value={formData.feedbackType}
            onChange={handleChange}
            required
            style={{
              width: "100%",
              padding: "10px",
              border: "1px solid #e0e0e0",
              borderRadius: "4px",
              fontSize: "14px",
            }}
          >
            <option value="bug_report">Bug Report</option>
            <option value="feature_request">Feature Request</option>
            <option value="complaint">Complaint</option>
            <option value="praise">Praise</option>
            <option value="suggestion">Suggestion</option>
          </select>
        </div>

        {/* Subject */}
        <div>
          <label
            htmlFor="subject"
            style={{
              display: "block",
              marginBottom: "8px",
              fontWeight: "500",
              fontSize: "14px",
            }}
          >
            Subject
          </label>
          <input
            type="text"
            id="subject"
            name="subject"
            value={formData.subject}
            onChange={handleChange}
            required
            style={{
              width: "100%",
              padding: "10px",
              border: "1px solid #e0e0e0",
              borderRadius: "4px",
              fontSize: "14px",
            }}
          />
        </div>

        {/* Body */}
        <div>
          <label
            htmlFor="body"
            style={{
              display: "block",
              marginBottom: "8px",
              fontWeight: "500",
              fontSize: "14px",
            }}
          >
            Description
          </label>
          <textarea
            id="body"
            name="body"
            value={formData.body}
            onChange={handleChange}
            required
            rows={6}
            style={{
              width: "100%",
              padding: "10px",
              border: "1px solid #e0e0e0",
              borderRadius: "4px",
              fontSize: "14px",
              resize: "vertical",
            }}
          />
        </div>

        {/* Rating */}
        <div>
          <label
            htmlFor="rating"
            style={{
              display: "block",
              marginBottom: "8px",
              fontWeight: "500",
              fontSize: "14px",
            }}
          >
            Rating (1-5)
          </label>
          <select
            id="rating"
            name="rating"
            value={formData.rating}
            onChange={handleChange}
            required
            style={{
              width: "100%",
              padding: "10px",
              border: "1px solid #e0e0e0",
              borderRadius: "4px",
              fontSize: "14px",
            }}
          >
            <option value={1}>1 - Poor</option>
            <option value={2}>2 - Fair</option>
            <option value={3}>3 - Good</option>
            <option value={4}>4 - Very Good</option>
            <option value={5}>5 - Excellent</option>
          </select>
        </div>

        {/* Submit Button */}
        <button
          type="submit"
          disabled={loading}
          style={{
            padding: "12px",
            backgroundColor: loading ? "#bdbdbd" : "#1976d2",
            color: "#fff",
            border: "none",
            borderRadius: "4px",
            fontSize: "16px",
            fontWeight: "500",
            cursor: loading ? "not-allowed" : "pointer",
          }}
        >
          {loading ? "Submitting..." : "Submit Feedback"}
        </button>

        {/* Success Message */}
        {success && (
          <div
            style={{
              padding: "12px",
              backgroundColor: "#e8f5e9",
              color: "#2e7d32",
              borderRadius: "4px",
              fontSize: "14px",
            }}
          >
            Feedback submitted successfully!
          </div>
        )}

        {/* Error Message */}
        {error && (
          <div
            style={{
              padding: "12px",
              backgroundColor: "#ffebee",
              color: "#c62828",
              borderRadius: "4px",
              fontSize: "14px",
            }}
          >
            Error: {error.message}
          </div>
        )}
      </form>
    </div>
  );
}
