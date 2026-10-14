/**
 * Core data types for the CXP SDK
 */

/**
 * Customer entity
 */
export interface Customer {
  id: string;
  email: string;
  name?: string;
  phone?: string;
  company?: string;
  metadata: Record<string, unknown>;
  tags: string[];
  createdAt: string;
  updatedAt: string;
}

/**
 * Support ticket entity
 */
export interface Ticket {
  id: string;
  customerId: string;
  subject: string;
  description?: string;
  status: string;
  priority: string;
  channel?: string;
  assignedTo?: string;
  tags: string[];
  metadata: Record<string, unknown>;
  resolvedAt?: string;
  createdAt: string;
  updatedAt: string;
}

/**
 * Customer interaction entity
 */
export interface Interaction {
  id: string;
  customerId: string;
  interactionType: string;
  metadata: Record<string, unknown>;
  createdAt: string;
}

/**
 * Customer feedback entity
 */
export interface Feedback {
  id: string;
  customerId: string;
  feedbackType: string;
  subject?: string;
  body?: string;
  rating?: number;
  experienceId?: string;
  applicationId?: string;
  pageId?: string;
  componentId?: string;
  metadata: Record<string, unknown>;
  createdAt: string;
}

/**
 * Survey definition
 */
export interface Survey {
  id: string;
  title: string;
  description?: string;
  questions: SurveyQuestion[];
  active: boolean;
  createdAt: string;
  updatedAt: string;
}

/**
 * Survey question
 */
export interface SurveyQuestion {
  id: string;
  text: string;
  questionType: string;
  options?: string[];
  required: boolean;
}

/**
 * Survey response from a customer
 */
export interface SurveyResponse {
  id: string;
  surveyId: string;
  customerId: string;
  answers: SurveyAnswer[];
  completedAt?: string;
  createdAt: string;
}

/**
 * Individual answer within a survey response
 */
export interface SurveyAnswer {
  questionId: string;
  value: string;
}

/**
 * Knowledge base article
 */
export interface KnowledgeArticle {
  id: string;
  title: string;
  content: string;
  category?: string;
  tags: string[];
  published: boolean;
  createdAt: string;
  updatedAt: string;
}

/**
 * Customer segment definition
 */
export interface Segment {
  id: string;
  name: string;
  description?: string;
  criteria: Record<string, unknown>;
  segmentType: string;
  createdAt: string;
  updatedAt: string;
}

/**
 * Live chat session
 */
export interface ChatSession {
  id: string;
  customerId: string;
  status: string;
  metadata: Record<string, unknown>;
  startedAt: string;
  endedAt?: string;
}

/**
 * Message within a chat session
 */
export interface ChatMessage {
  id: string;
  sessionId: string;
  role: string;
  content: string;
  metadata: Record<string, unknown>;
  createdAt: string;
}

/**
 * 360-degree view of a customer
 */
export interface Customer360 {
  customer: Customer;
  recentTickets: TicketSummary[];
  recentInteractions: Interaction[];
  recentFeedback: FeedbackSummary[];
}

/**
 * A support ticket as the 360 view shows it
 */
export interface TicketSummary {
  id: string;
  subject: string;
  status: string;
  priority: string;
  channel?: string | null;
  assignedTo?: string | null;
  createdAt: string;
  resolvedAt?: string | null;
}

/**
 * A piece of feedback as the 360 view shows it
 */
export interface FeedbackSummary {
  id: string;
  feedbackType: string;
  subject?: string | null;
  body?: string | null;
  rating?: number | null;
  createdAt: string;
}

/**
 * Sentiment analysis result
 */
export interface SentimentResult {
  score: number;
  label: string;
  positiveWords: string[];
  negativeWords: string[];
}

/**
 * Metrics dashboard data
 */
export interface MetricsDashboard {
  tickets: {
    total: number;
    open: number;
    inProgress: number;
  };
  feedback: {
    total: number;
    csat: number;
    avgRating: number;
  };
}

/**
 * Feedback context information
 */
export interface FeedbackContext {
  experienceId?: string;
  applicationId?: string;
  pageId?: string;
  componentId?: string;
}

/**
 * NPS calculation result
 */
export interface NpsResult {
  /** Net Promoter Score (-100 to 100); null when nothing is scored */
  nps: number | null;
  totalResponses: number;
  scoredResponses: number;
  promoters: number;
  passives: number;
  detractors: number;
}

/**
 * CSAT calculation result
 */
export interface CsatResult {
  /** Satisfied share of rated feedback, 0 to 100; null when nothing is rated */
  csat: number | null;
  totalFeedback: number;
  ratedFeedback: number;
  satisfied: number;
  unsatisfied: number;
}
