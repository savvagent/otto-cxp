export interface Customer {
  id: string;
  email: string;
  name: string | null;
  phone: string | null;
  company: string | null;
  metadata: Record<string, unknown>;
  tags: string[];
  experience_id: string | null;
  application_id: string | null;
  page_id: string | null;
  component_id: string | null;
  created_at: string;
  updated_at: string;
}

export interface Ticket {
  id: string;
  customer_id: string;
  subject: string;
  description: string | null;
  status: string;
  priority: string;
  channel: string | null;
  assigned_to: string | null;
  tags: string[];
  metadata: Record<string, unknown>;
  experience_id: string | null;
  application_id: string | null;
  page_id: string | null;
  component_id: string | null;
  resolved_at: string | null;
  created_at: string;
  updated_at: string;
}

export interface Feedback {
  id: string;
  customer_id: string;
  feedback_type: string;
  subject: string | null;
  body: string | null;
  rating: number | null;
  metadata: Record<string, unknown>;
  experience_id: string | null;
  application_id: string | null;
  page_id: string | null;
  component_id: string | null;
  created_at: string;
}

export interface Survey {
  id: string;
  title: string;
  description: string | null;
  questions: unknown;
  active: boolean;
  experience_id: string | null;
  application_id: string | null;
  page_id: string | null;
  component_id: string | null;
  created_at: string;
  updated_at: string;
}

export interface SurveyResponse {
  id: string;
  survey_id: string;
  customer_id: string;
  answers: Record<string, unknown>;
  completed_at: string | null;
  created_at: string;
}

export interface Experience {
  id: string;
  name: string;
  description: string | null;
  metadata: Record<string, unknown>;
  created_at: string;
  updated_at: string;
}

export interface Application {
  id: string;
  experience_id: string;
  name: string;
  description: string | null;
  platform: string | null;
  metadata: Record<string, unknown>;
  created_at: string;
  updated_at: string;
}

export interface Page {
  id: string;
  application_id: string;
  name: string;
  path: string | null;
  description: string | null;
  metadata: Record<string, unknown>;
  created_at: string;
  updated_at: string;
}

export interface Component {
  id: string;
  page_id: string;
  name: string;
  component_type: string | null;
  description: string | null;
  metadata: Record<string, unknown>;
  created_at: string;
  updated_at: string;
}

export interface Customer360 {
  customer: Customer;
  recent_tickets: Ticket[];
  recent_interactions: Interaction[];
  recent_feedback: Feedback[];
}

export interface Interaction {
  id: string;
  customer_id: string;
  interaction_type: string;
  metadata: Record<string, unknown>;
  created_at: string;
}

export interface DashboardMetrics {
  total_customers: number;
  total_tickets: number;
  open_tickets: number;
  total_feedback: number;
  total_surveys: number;
  total_experiences: number;
}

export interface PaginatedResponse<T> {
  data: T[];
  total: number;
  limit: number;
  offset: number;
}
