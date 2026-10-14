import React, { useState } from "react";
import type { Feedback } from "@cxp/sdk";
import { useCxpOptional } from "../context/CxpProvider.js";

export interface FeedbackFormLabels {
  title?: string;
  feedbackType?: string;
  subject?: string;
  description?: string;
  rating?: string;
  submit?: string;
  submitting?: string;
  success?: string;
  errorPrefix?: string;
  typeBugReport?: string;
  typeFeatureRequest?: string;
  typeComplaint?: string;
  typePraise?: string;
  typeSuggestion?: string;
  rating1?: string;
  rating2?: string;
  rating3?: string;
  rating4?: string;
  rating5?: string;
}

const DEFAULT_FEEDBACK_LABELS: Required<FeedbackFormLabels> = {
  title: "Submit Feedback",
  feedbackType: "Feedback Type",
  subject: "Subject",
  description: "Description",
  rating: "Rating (1-5)",
  submit: "Submit Feedback",
  submitting: "Submitting...",
  success: "Feedback submitted successfully!",
  errorPrefix: "Error:",
  typeBugReport: "Bug Report",
  typeFeatureRequest: "Feature Request",
  typeComplaint: "Complaint",
  typePraise: "Praise",
  typeSuggestion: "Suggestion",
  rating1: "1 - Poor",
  rating2: "2 - Fair",
  rating3: "3 - Good",
  rating4: "4 - Very Good",
  rating5: "5 - Excellent",
};

export interface FeedbackFormProps {
  customerId?: string;
  experience?: string;
  application?: string;
  page?: string;
  component?: string;
  labels?: FeedbackFormLabels;
  onSubmit?: (feedback: Feedback) => void;
}

/**
 * Component for submitting customer feedback.
 * Works standalone with demo behavior when no CxpProvider is present.
 *
 * @example
 * // Standalone demo mode
 * <FeedbackForm onSubmit={(fb) => console.log(fb)} />
 *
 * @example
 * // Connected to CXP backend
 * <CxpProvider client={cxpClient}>
 *   <FeedbackForm customerId="cust_123" />
 * </CxpProvider>
 */
export function FeedbackForm({
  customerId = "demo-customer",
  experience,
  application,
  page,
  component,
  labels: labelOverrides,
  onSubmit,
}: FeedbackFormProps) {
  const l = { ...DEFAULT_FEEDBACK_LABELS, ...labelOverrides };
  const cxp = useCxpOptional();
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
      let feedback: Feedback;

      if (cxp) {
        feedback = await cxp.feedback.submitFeedback(
          customerId,
          formData.feedbackType,
          {
            subject: formData.subject,
            body: formData.body,
            rating: formData.rating,
            experienceId: experience,
            applicationId: application,
            pageId: page,
            componentId: component,
          },
        );
      } else {
        // Demo mode: simulate a brief delay and return mock data
        await new Promise((r) => setTimeout(r, 600));
        feedback = {
          id: `demo-fb-${Date.now()}`,
          customerId,
          feedbackType: formData.feedbackType,
          subject: formData.subject,
          body: formData.body,
          rating: formData.rating,
          experienceId: experience,
          applicationId: application,
          pageId: page,
          componentId: component,
          metadata: {},
          createdAt: new Date().toISOString(),
        };
      }

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
        {l.title}
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
            {l.feedbackType}
          </label>
          <select
            id="feedbackType"
            name="feedbackType"
            value={formData.feedbackType}
            onChange={handleChange}
            required
            style={{
              width: "100%",
              boxSizing: "border-box",
              padding: "10px",
              border: "1px solid #e0e0e0",
              borderRadius: "4px",
              fontSize: "14px",
            }}
          >
            <option value="bug_report">{l.typeBugReport}</option>
            <option value="feature_request">{l.typeFeatureRequest}</option>
            <option value="complaint">{l.typeComplaint}</option>
            <option value="praise">{l.typePraise}</option>
            <option value="suggestion">{l.typeSuggestion}</option>
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
            {l.subject}
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
              boxSizing: "border-box",
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
            {l.description}
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
              boxSizing: "border-box",
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
            {l.rating}
          </label>
          <select
            id="rating"
            name="rating"
            value={formData.rating}
            onChange={handleChange}
            required
            style={{
              width: "100%",
              boxSizing: "border-box",
              padding: "10px",
              border: "1px solid #e0e0e0",
              borderRadius: "4px",
              fontSize: "14px",
            }}
          >
            <option value={1}>{l.rating1}</option>
            <option value={2}>{l.rating2}</option>
            <option value={3}>{l.rating3}</option>
            <option value={4}>{l.rating4}</option>
            <option value={5}>{l.rating5}</option>
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
          {loading ? l.submitting : l.submit}
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
            {l.success}
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
            {l.errorPrefix} {error.message}
          </div>
        )}
      </form>
    </div>
  );
}
