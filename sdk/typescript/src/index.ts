/**
 * CXP SDK - TypeScript client for the Customer Experience Platform
 *
 * @packageDocumentation
 */

// Main client
export { CxpClient } from "./client.js";
export type { CxpClientOptions } from "./client.js";

// Connecting and errors
export { connectCxp } from "./connect.js";
export type { ConnectCxpOptions, CxpConnection } from "./connect.js";
export { CxpError } from "./call.js";

// Domain modules
export {
  SupportDomain,
  FeedbackDomain,
  CdpDomain,
} from "./domains/index.js";

export type {
  CreateTicketOptions,
  UpdateTicketOptions,
  ListTicketsOptions,
  SubmitFeedbackOptions,
  GetFeedbackOptions,
  CreateCustomerOptions,
  UpdateCustomerOptions,
  CreateSegmentOptions,
} from "./domains/index.js";

// Core types
export type {
  Customer,
  Ticket,
  Interaction,
  Feedback,
  Survey,
  SurveyQuestion,
  SurveyResponse,
  SurveyAnswer,
  KnowledgeArticle,
  Segment,
  ChatSession,
  ChatMessage,
  Customer360,
  SentimentResult,
  MetricsDashboard,
  FeedbackContext,
  NpsResult,
  CsatResult,
} from "./types.js";

// Type utilities
export type {
  AsyncReturnType,
  CamelCaseKeys,
  SnakeCaseKeys,
  DeepReadonly,
  DeepPartial,
  JsonValue,
  Predicate,
  Comparator,
  Mapper,
  Result,
} from "./type-utils.js";

export {
  assertNever,
  isDefined,
  isString,
  isNumber,
  isObject,
  isArray,
  success,
  failure,
  unwrap,
  unwrapOr,
  mapResult,
} from "./type-utils.js";
