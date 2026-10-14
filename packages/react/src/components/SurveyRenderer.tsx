import React, { useState, useEffect } from "react";
import type { Survey, SurveyQuestion } from "@cxp/sdk";
import { useCxpOptional } from "../context/CxpProvider.js";

export interface SurveyRendererLabels {
  loading?: string;
  errorPrefix?: string;
  noSurvey?: string;
  notFound?: string;
  thankYouTitle?: string;
  thankYouMessage?: string;
  submit?: string;
  submitting?: string;
}

const DEFAULT_SURVEY_LABELS: Required<SurveyRendererLabels> = {
  loading: "Loading survey...",
  errorPrefix: "Error:",
  noSurvey: "Provide a survey ID or seed mock data to view a survey.",
  notFound: "Survey not found.",
  thankYouTitle: "Thank you!",
  thankYouMessage: "Your survey response has been submitted successfully.",
  submit: "Submit Survey",
  submitting: "Submitting...",
};

export interface SurveyQuestionOverride {
  text?: string;
  options?: string[];
}

export interface SurveyContentOverrides {
  title?: string;
  description?: string;
  questions?: Record<string, SurveyQuestionOverride>;
}

export interface SurveyRendererProps {
  surveyId?: string;
  customerId?: string;
  onComplete?: () => void;
  /** Catalog experience UUID (`experience_id`). Currently unused by this component. */
  experience?: string;
  /** Catalog application UUID (`application_id`). Currently unused by this component. */
  application?: string;
  page?: string;
  component?: string;
  labels?: SurveyRendererLabels;
  content?: SurveyContentOverrides;
}

const DEMO_SURVEY: Survey = {
  id: "demo-survey",
  title: "Customer Satisfaction Survey",
  description:
    "We'd love to hear about your experience. Your feedback helps us improve!",
  active: true,
  questions: [
    {
      id: "q1",
      text: "How would you rate your overall experience?",
      questionType: "rating",
      required: true,
    },
    {
      id: "q2",
      text: "Which feature do you use most often?",
      questionType: "multiple_choice",
      options: ["Dashboard", "Reports", "Settings", "Integrations"],
      required: true,
    },
    {
      id: "q3",
      text: "What could we do to improve your experience?",
      questionType: "text",
      required: false,
    },
  ],
  createdAt: new Date().toISOString(),
  updatedAt: new Date().toISOString(),
};

/**
 * Component for rendering and completing surveys.
 * Works standalone with a demo survey when no CxpProvider is present.
 *
 * @example
 * // Standalone demo mode
 * <SurveyRenderer onComplete={() => alert("Done!")} />
 *
 * @example
 * // Connected to CXP backend
 * <CxpProvider client={cxpClient}>
 *   <SurveyRenderer surveyId="survey_123" customerId="cust_123" />
 * </CxpProvider>
 */
export function SurveyRenderer({
  surveyId = "demo-survey",
  customerId = "demo-customer",
  onComplete,
  labels: labelOverrides,
  content,
}: SurveyRendererProps) {
  const l = { ...DEFAULT_SURVEY_LABELS, ...labelOverrides };
  const cxp = useCxpOptional();
  const isDemo = !cxp;
  const hasValidId = surveyId !== "demo-survey" && surveyId !== "";

  const [survey, setSurvey] = useState<Survey | null>(isDemo ? DEMO_SURVEY : null);
  const [loading, setLoading] = useState(!isDemo && hasValidId);
  const [error, setError] = useState<Error | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [submitted, setSubmitted] = useState(false);
  const [answers, setAnswers] = useState<Record<string, string>>({});

  useEffect(() => {
    if (isDemo || !hasValidId) return;
    let cancelled = false;
    setLoading(true);
    cxp!.feedback
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
  }, [cxp, surveyId, isDemo, hasValidId]);

  const handleAnswerChange = (questionId: string, value: string) => {
    setAnswers((prev) => ({ ...prev, [questionId]: value }));
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!survey) return;

    setSubmitting(true);
    setError(null);

    try {
      if (cxp) {
        const surveyAnswers = Object.entries(answers).map(
          ([questionId, value]) => ({ questionId, value })
        );
        await cxp.feedback.submitSurveyResponse({
          surveyId,
          customerId,
          answers: surveyAnswers,
        });
      } else {
        // Demo mode: simulate submission delay
        await new Promise((r) => setTimeout(r, 600));
      }
      setSubmitted(true);
      onComplete?.();
    } catch (err) {
      setError(err as Error);
    } finally {
      setSubmitting(false);
    }
  };

  const resolveQuestion = (question: SurveyQuestion): SurveyQuestion => {
    const overrides = content?.questions?.[question.id];
    if (!overrides) return question;
    return {
      ...question,
      ...(overrides.text !== undefined && { text: overrides.text }),
      ...(overrides.options !== undefined && { options: overrides.options }),
    };
  };

  const renderQuestion = (question: SurveyQuestion) => {
    question = resolveQuestion(question);
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
                boxSizing: "border-box",
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
    return <div style={{ padding: "20px" }}>{l.loading}</div>;
  }

  if (error) {
    return (
      <div style={{ padding: "20px", color: "#d32f2f" }}>
        {l.errorPrefix} {error.message}
      </div>
    );
  }

  if (!isDemo && !hasValidId) {
    return (
      <div style={{ padding: "20px", color: "#757575" }}>
        {l.noSurvey}
      </div>
    );
  }

  if (!survey) {
    return <div style={{ padding: "20px" }}>{l.notFound}</div>;
  }

  if (submitted) {
    return (
      <div style={{ maxWidth: "700px", padding: "20px", textAlign: "center" }}>
        <div
          style={{
            padding: "24px",
            backgroundColor: "#e8f5e9",
            borderRadius: "8px",
            marginBottom: "16px",
          }}
        >
          <h3 style={{ color: "#2e7d32", fontSize: "20px", margin: "0 0 8px" }}>
            {l.thankYouTitle}
          </h3>
          <p style={{ color: "#388e3c", fontSize: "16px", margin: 0 }}>
            {l.thankYouMessage}
          </p>
        </div>
      </div>
    );
  }

  return (
    <div style={{ maxWidth: "700px", padding: "20px" }}>
      <h2 style={{ fontSize: "28px", fontWeight: "600", marginBottom: "8px" }}>
        {content?.title ?? survey.title}
      </h2>
      {(content?.description ?? survey.description) && (
        <p style={{ fontSize: "16px", color: "#666", marginBottom: "32px" }}>
          {content?.description ?? survey.description}
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
          {submitting ? l.submitting : l.submit}
        </button>

      </form>
    </div>
  );
}
