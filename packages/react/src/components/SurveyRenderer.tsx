import React, { useState, useEffect } from "react";
import type { Survey, SurveyQuestion } from "@cxp/sdk";
import { useCxp } from "../context/CxpProvider.js";

export interface SurveyRendererProps {
  surveyId: string;
  customerId: string;
  onComplete?: () => void;
}

/**
 * Component for rendering and completing surveys
 * @example
 * <SurveyRenderer
 *   surveyId="survey_123"
 *   customerId="cust_123"
 *   onComplete={() => console.log("Survey completed!")}
 * />
 */
export function SurveyRenderer({
  surveyId,
  customerId,
  onComplete,
}: SurveyRendererProps) {
  const cxp = useCxp();
  const [survey, setSurvey] = useState<Survey | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<Error | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [answers, setAnswers] = useState<Record<string, string>>({});

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    cxp.feedback
      .getSurvey(surveyId)
      .then((data) => {
        if (!cancelled) {
          setSurvey(data);
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

  const handleAnswerChange = (questionId: string, value: string) => {
    setAnswers((prev) => ({ ...prev, [questionId]: value }));
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!survey) return;

    setSubmitting(true);
    setError(null);

    try {
      const surveyAnswers = Object.entries(answers).map(
        ([questionId, value]) => ({ questionId, value })
      );

      await cxp.feedback.submitSurveyResponse({
        surveyId,
        customerId,
        answers: surveyAnswers,
      });
      onComplete?.();
    } catch (err) {
      setError(err as Error);
    } finally {
      setSubmitting(false);
    }
  };

  const renderQuestion = (question: SurveyQuestion) => {
    switch (question.questionType) {
      case "rating":
        return (
          <div key={question.id} style={{ marginBottom: "24px" }}>
            <label
              style={{
                display: "block",
                marginBottom: "12px",
                fontWeight: "500",
                fontSize: "16px",
              }}
            >
              {question.text}
              {question.required && (
                <span style={{ color: "#d32f2f" }}> *</span>
              )}
            </label>
            <div style={{ display: "flex", gap: "8px" }}>
              {[1, 2, 3, 4, 5, 6, 7, 8, 9, 10].map((rating) => (
                <button
                  key={rating}
                  type="button"
                  onClick={() =>
                    handleAnswerChange(question.id, rating.toString())
                  }
                  style={{
                    padding: "12px",
                    minWidth: "44px",
                    border: "2px solid",
                    borderColor:
                      answers[question.id] === rating.toString()
                        ? "#1976d2"
                        : "#e0e0e0",
                    borderRadius: "4px",
                    backgroundColor:
                      answers[question.id] === rating.toString()
                        ? "#e3f2fd"
                        : "#fff",
                    cursor: "pointer",
                    fontSize: "14px",
                    fontWeight: "500",
                  }}
                >
                  {rating}
                </button>
              ))}
            </div>
          </div>
        );

      case "text":
        return (
          <div key={question.id} style={{ marginBottom: "24px" }}>
            <label
              htmlFor={question.id}
              style={{
                display: "block",
                marginBottom: "8px",
                fontWeight: "500",
                fontSize: "16px",
              }}
            >
              {question.text}
              {question.required && (
                <span style={{ color: "#d32f2f" }}> *</span>
              )}
            </label>
            <textarea
              id={question.id}
              value={answers[question.id] || ""}
              onChange={(e) => handleAnswerChange(question.id, e.target.value)}
              required={question.required}
              rows={4}
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
        );

      case "multiple_choice":
        return (
          <div key={question.id} style={{ marginBottom: "24px" }}>
            <label
              style={{
                display: "block",
                marginBottom: "12px",
                fontWeight: "500",
                fontSize: "16px",
              }}
            >
              {question.text}
              {question.required && (
                <span style={{ color: "#d32f2f" }}> *</span>
              )}
            </label>
            <div style={{ display: "flex", flexDirection: "column", gap: "8px" }}>
              {question.options?.map((option) => (
                <label
                  key={option}
                  style={{
                    display: "flex",
                    alignItems: "center",
                    gap: "8px",
                    padding: "12px",
                    border: "1px solid #e0e0e0",
                    borderRadius: "4px",
                    cursor: "pointer",
                    backgroundColor:
                      answers[question.id] === option ? "#e3f2fd" : "#fff",
                  }}
                >
                  <input
                    type="radio"
                    name={question.id}
                    value={option}
                    checked={answers[question.id] === option}
                    onChange={(e) =>
                      handleAnswerChange(question.id, e.target.value)
                    }
                    required={question.required}
                    style={{ cursor: "pointer" }}
                  />
                  <span style={{ fontSize: "14px" }}>{option}</span>
                </label>
              ))}
            </div>
          </div>
        );

      default:
        return null;
    }
  };

  if (loading) {
    return <div style={{ padding: "20px" }}>Loading survey...</div>;
  }

  if (error) {
    return (
      <div style={{ padding: "20px", color: "#d32f2f" }}>
        Error: {error.message}
      </div>
    );
  }

  if (!survey) {
    return <div style={{ padding: "20px" }}>Survey not found.</div>;
  }

  return (
    <div style={{ maxWidth: "700px", padding: "20px" }}>
      <h2 style={{ fontSize: "28px", fontWeight: "600", marginBottom: "8px" }}>
        {survey.title}
      </h2>
      {survey.description && (
        <p style={{ fontSize: "16px", color: "#666", marginBottom: "32px" }}>
          {survey.description}
        </p>
      )}

      <form onSubmit={handleSubmit}>
        {survey.questions.map(renderQuestion)}

        <button
          type="submit"
          disabled={submitting}
          style={{
            padding: "14px 32px",
            backgroundColor: submitting ? "#bdbdbd" : "#1976d2",
            color: "#fff",
            border: "none",
            borderRadius: "4px",
            fontSize: "16px",
            fontWeight: "500",
            cursor: submitting ? "not-allowed" : "pointer",
            marginTop: "24px",
          }}
        >
          {submitting ? "Submitting..." : "Submit Survey"}
        </button>

      </form>
    </div>
  );
}
