// Responses, request bodies and query parameters are all camelCase: the one
// `/api/v1` convention (`crates/cxp-api/src/routes/mod.rs`). Enum values stay as
// the server stores them. `src/lib/types.test.ts` checks these shapes against
// the payloads in `crates/cxp-api/tests/fixtures/wire.json`, which a Rust test
// generates.

export interface Customer {
  id: string;
  email: string;
  name: string | null;
  phone: string | null;
  company: string | null;
  metadata: Record<string, unknown>;
  tags: string[];
  experienceId: string | null;
  applicationId: string | null;
  pageId: string | null;
  componentId: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface Ticket {
  id: string;
  customerId: string;
  subject: string;
  description: string | null;
  status: string;
  priority: string;
  channel: string | null;
  assignedTo: string | null;
  tags: string[];
  metadata: Record<string, unknown>;
  experienceId: string | null;
  applicationId: string | null;
  pageId: string | null;
  componentId: string | null;
  resolvedAt: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface Feedback {
  id: string;
  customerId: string;
  feedbackType: string;
  subject: string | null;
  body: string | null;
  rating: number | null;
  metadata: Record<string, unknown>;
  experienceId: string | null;
  applicationId: string | null;
  pageId: string | null;
  componentId: string | null;
  createdAt: string;
}

export interface Survey {
  id: string;
  title: string;
  description: string | null;
  questions: unknown;
  active: boolean;
  experienceId: string | null;
  applicationId: string | null;
  pageId: string | null;
  componentId: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface SurveyResponse {
  id: string;
  surveyId: string;
  customerId: string;
  answers: Record<string, unknown>;
  completedAt: string | null;
  createdAt: string;
}

export interface Experience {
  id: string;
  name: string;
  description: string | null;
  teamId: string | null;
  metadata: Record<string, unknown>;
  createdAt: string;
  updatedAt: string;
}

export interface Application {
  id: string;
  experienceId: string | null;
  teamId: string | null;
  name: string;
  description: string | null;
  platform: string | null;
  metadata: Record<string, unknown>;
  createdAt: string;
  updatedAt: string;
}

export interface Page {
  id: string;
  applicationId: string | null;
  teamId: string | null;
  name: string;
  path: string | null;
  description: string | null;
  metadata: Record<string, unknown>;
  createdAt: string;
  updatedAt: string;
}

export interface Component {
  id: string;
  pageId: string | null;
  teamId: string | null;
  name: string;
  componentType: string | null;
  description: string | null;
  metadata: Record<string, unknown>;
  createdAt: string;
  updatedAt: string;
}

/** The ticket rows inside a customer 360 view: a subset of `Ticket`. */
export interface TicketSummary {
  id: string;
  subject: string;
  status: string;
  priority: string;
  channel: string | null;
  assignedTo: string | null;
  createdAt: string;
  resolvedAt: string | null;
}

/** The feedback rows inside a customer 360 view: a subset of `Feedback`. */
export interface FeedbackSummary {
  id: string;
  feedbackType: string;
  subject: string | null;
  body: string | null;
  rating: number | null;
  createdAt: string;
}

export interface Customer360 {
  customer: Customer;
  recentTickets: TicketSummary[];
  recentInteractions: Interaction[];
  recentFeedback: FeedbackSummary[];
}

export interface Interaction {
  id: string;
  customerId: string;
  interactionType: string;
  metadata: Record<string, unknown>;
  createdAt: string;
}

/** A team the caller belongs to, owned by the otto platform. */
export interface MyTeam {
  id: string;
  slug: string;
  name: string;
}

/**
 * `GET /dashboard/metrics`. The server also sends deprecated snake_case twins of
 * these keys, which the console does not read (their removal is #35).
 */
export interface DashboardMetrics {
  totalCustomers: number;
  totalTickets: number;
  openTickets: number;
  totalFeedback: number;
  totalSurveys: number;
  totalExperiences: number;
}

export interface PaginatedResponse<T> {
  data: T[];
  total: number;
  limit: number;
  offset: number;
}

/**
 * A search response: the list envelope plus whether `total` is capped. With
 * `totalCapped: false`, `total` and `data` are exact; with `true`, `total` is a
 * lower bound (matches are counted up to 1000) and `data` is the requested page
 * of the best-ranked matches found.
 */
export interface CappedPaginatedResponse<T> extends PaginatedResponse<T> {
  totalCapped: boolean;
}
