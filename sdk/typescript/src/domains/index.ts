/**
 * Domain modules for CXP SDK
 */

export { SupportDomain } from "./support.js";
export type {
  CreateTicketOptions,
  UpdateTicketOptions,
  ListTicketsOptions,
} from "./support.js";

export { FeedbackDomain } from "./feedback.js";
export type {
  SubmitFeedbackOptions,
  GetFeedbackOptions,
} from "./feedback.js";

export { CdpDomain } from "./cdp.js";
export type {
  CreateCustomerOptions,
  UpdateCustomerOptions,
  CreateSegmentOptions,
} from "./cdp.js";
