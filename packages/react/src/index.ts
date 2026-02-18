// Context
export { CxpProvider, useCxp } from "./context/CxpProvider.js";

// Hooks
export {
  useCustomer,
  useTickets,
  useFeedback,
  useChat,
  useMetrics,
  type UseTicketsOptions,
  type UseFeedbackOptions,
} from "./hooks/index.js";

// Components
export {
  TicketList,
  TicketDetail,
  CustomerCard,
  Customer360,
  ChatWidget,
  FeedbackForm,
  SurveyRenderer,
  NpsGauge,
  MetricsDashboard,
  type TicketListProps,
  type TicketDetailProps,
  type CustomerCardProps,
  type Customer360Props,
  type ChatWidgetProps,
  type FeedbackFormProps,
  type SurveyRendererProps,
  type NpsGaugeProps,
} from "./components/index.js";
