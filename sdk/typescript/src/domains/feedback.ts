/**
 * Feedback domain - handles surveys, feedback, NPS, CSAT, and sentiment
 */

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
  experience?: string;
  application?: string;
  page?: string;
  component?: string;
}

export interface GetFeedbackOptions {
  customerId?: string;
  feedbackType?: string;
  limit?: number;
}

export class FeedbackDomain {
  constructor(private client: Client) {}

  /**
   * Call an MCP tool and parse the JSON response
   */
  private async call<T>(name: string, args: Record<string, unknown>): Promise<T> {
    const result = await this.client.callTool({ name, arguments: args });
    const text = (result.content as Array<{ type: string; text: string }>)[0]?.text ?? "{}";
    try {
      return JSON.parse(text) as T;
    } catch {
      throw new Error(text);
    }
  }

  /**
   * Get a survey by ID
   */
  async getSurvey(surveyId: string): Promise<Survey> {
    return this.call("get_survey", { id: surveyId });
  }

  /**
   * Submit a survey response with answers
   */
  async submitSurveyResponse(response: {
    surveyId: string;
    customerId: string;
    answers: unknown;
  }): Promise<SurveyResponse> {
    return this.call("submit_survey_response", {
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
    return this.call("create_survey", {
      title,
      description,
      questions: JSON.stringify(questions),
    });
  }

  /**
   * Send a survey to a customer
   */
  async sendSurvey(surveyId: string, customerId: string): Promise<SurveyResponse> {
    return this.call("send_survey", {
      survey_id: surveyId,
      customer_id: customerId,
    });
  }

  /**
   * Get all responses for a survey
   */
  async getResponses(surveyId: string): Promise<SurveyResponse[]> {
    return this.call("get_responses", {
      survey_id: surveyId,
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
    return this.call("submit_feedback", {
      customer_id: customerId,
      feedback_type: feedbackType,
      subject: options?.subject,
      body: options?.body,
      rating: options?.rating,
      experience: options?.experience,
      application: options?.application,
      page: options?.page,
      component: options?.component,
    });
  }

  /**
   * Get feedback with optional filtering
   */
  async getFeedback(options?: GetFeedbackOptions): Promise<Feedback[]> {
    return this.call("get_feedback", {
      customer_id: options?.customerId,
      feedback_type: options?.feedbackType,
      limit: options?.limit,
    });
  }

  /**
   * Calculate Net Promoter Score for a survey
   */
  async calculateNps(surveyId: string): Promise<NpsResult> {
    return this.call("calculate_nps", {
      id: surveyId,
    });
  }

  /**
   * Calculate Customer Satisfaction score
   */
  async calculateCsat(feedbackType?: string): Promise<CsatResult> {
    return this.call("calculate_csat", {
      feedback_type: feedbackType,
    });
  }

  /**
   * Analyze sentiment of text
   */
  async analyzeSentiment(text: string): Promise<SentimentResult> {
    return this.call("analyze_sentiment", {
      text,
    });
  }
}
