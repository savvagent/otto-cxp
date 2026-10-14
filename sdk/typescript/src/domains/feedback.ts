/**
 * Feedback domain - handles surveys, feedback, NPS, CSAT, and sentiment
 */

import { callList, callTool } from "../call.js";
import type { Client } from "@modelcontextprotocol/sdk/client/index.js";
import type {
  Survey,
  SurveyQuestion,
  SurveyResponse,
  Feedback,
  SentimentResult,
  NpsResult,
  CsatResult,
} from "../types.js";

export interface SubmitFeedbackOptions {
  subject?: string;
  body?: string;
  rating?: number;
  experienceId?: string;
  applicationId?: string;
  pageId?: string;
  componentId?: string;
}

export interface GetFeedbackOptions {
  customerId?: string;
  feedbackType?: string;
  limit?: number;
}

export class FeedbackDomain {
  constructor(private client: Client) {}

  /**
   * Get a survey by ID
   */
  async getSurvey(surveyId: string): Promise<Survey> {
    return callTool(this.client, "get_survey", { id: surveyId });
  }

  /**
   * Submit a survey response with answers
   */
  async submitSurveyResponse(response: {
    surveyId: string;
    customerId: string;
    answers: unknown;
  }): Promise<SurveyResponse> {
    return callTool(this.client, "submit_survey_response", {
      survey_id: response.surveyId,
      customer_id: response.customerId,
      answers: JSON.stringify(response.answers),
    });
  }

  /**
   * Create a new survey
   */
  async createSurvey(
    title: string,
    questions: SurveyQuestion[],
    description?: string
  ): Promise<Survey> {
    return callTool(this.client, "create_survey", {
      title,
      description,
      questions: JSON.stringify(questions),
    });
  }

  /**
   * Send a survey to a customer
   */
  async sendSurvey(surveyId: string, customerId: string): Promise<SurveyResponse> {
    return callTool(this.client, "send_survey", {
      survey_id: surveyId,
      customer_id: customerId,
    });
  }

  /**
   * Get all responses for a survey
   */
  async getResponses(surveyId: string): Promise<SurveyResponse[]> {
    return callList(this.client, "get_survey_responses", {
      id: surveyId,
    });
  }

  /**
   * Submit customer feedback
   */
  async submitFeedback(
    customerId: string,
    feedbackType: string,
    options?: SubmitFeedbackOptions
  ): Promise<Feedback> {
    return callTool(this.client, "submit_feedback", {
      customer_id: customerId,
      feedback_type: feedbackType,
      subject: options?.subject,
      body: options?.body,
      rating: options?.rating,
      experience_id: options?.experienceId,
      application_id: options?.applicationId,
      page_id: options?.pageId,
      component_id: options?.componentId,
    });
  }

  /**
   * Get feedback with optional filtering
   */
  async getFeedback(options?: GetFeedbackOptions): Promise<Feedback[]> {
    return callList(this.client, "get_feedback", {
      customer_id: options?.customerId,
      feedback_type: options?.feedbackType,
      limit: options?.limit,
    });
  }

  /**
   * Calculate Net Promoter Score for a survey
   */
  async calculateNps(surveyId: string): Promise<NpsResult> {
    return callTool(this.client, "calculate_nps", {
      id: surveyId,
    });
  }

  /**
   * Calculate Customer Satisfaction score
   */
  async calculateCsat(feedbackType?: string): Promise<CsatResult> {
    return callTool(this.client, "calculate_csat", {
      feedback_type: feedbackType,
    });
  }

  /**
   * Analyze sentiment of text
   */
  async analyzeSentiment(text: string): Promise<SentimentResult> {
    return callTool(this.client, "analyze_sentiment", {
      text,
    });
  }
}
