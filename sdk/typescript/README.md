# CXP TypeScript SDK

TypeScript SDK for the Customer Experience Platform (CXP). Provides typed access to all CXP capabilities through an MCP client.

## Installation

```bash
npm install @cxp/sdk
```

## Usage

The server is an OAuth-protected MCP resource served over streamable HTTP at
`https://<host>/mcp`. Connect with a static bearer token (an otto platform access
token for this resource):

```typescript
import { connectCxp } from "@cxp/sdk";

const { cxp, close } = await connectCxp({
  url: "https://cxp.example.com/mcp",
  token: process.env.CXP_TOKEN,
});
```

Or pass an MCP `OAuthClientProvider` and let the transport run the OAuth flow
(call `transport.finishAuth(code)` on the returned `transport` after the redirect):

```typescript
const { cxp } = await connectCxp({ url, authProvider });
```

Already have a connected MCP `Client`? Wrap it: `new CxpClient({ client })`.

List methods return plain arrays (the `{ items, total }` tool envelope is
unwrapped). Failed calls throw `CxpError`; `error.code` carries the server's
stable error code, such as `not_found`.

```typescript
// Use the SDK
const ticket = await cxp.support.createTicket(
  "customer-123",
  "Issue with login",
  {
    description: "User cannot log in with correct credentials",
    priority: "high"
  }
);

console.log("Created ticket:", ticket.id);
```

## API Overview

### Support Domain

Handles ticketing, knowledge base, and chat support.

```typescript
// Tickets
const ticket = await cxp.support.createTicket(customerId, subject, options);
await cxp.support.updateTicket(ticketId, { status: "in_progress" });
await cxp.support.closeTicket(ticketId);
const tickets = await cxp.support.listTickets({ status: "open" });

// Knowledge Base
const articles = await cxp.support.searchKnowledgeBase("password reset");

// Chat
const session = await cxp.support.startChatSession(customerId);
await cxp.support.sendChatMessage(session.id, "agent", "Hello!");
await cxp.support.endChatSession(session.id);
```

### Feedback Domain

Handles surveys, feedback collection, and analytics.

```typescript
// Surveys
const survey = await cxp.feedback.createSurvey(
  "Customer Satisfaction",
  questions
);
await cxp.feedback.sendSurvey(survey.id, customerId);
const responses = await cxp.feedback.getResponses(survey.id);

// Feedback
await cxp.feedback.submitFeedback(customerId, "bug_report", {
  subject: "Button not working",
  body: "The submit button is disabled",
  application: "web-app",
  page: "/checkout"
});

// Analytics
const nps = await cxp.feedback.calculateNps(surveyId);
const csat = await cxp.feedback.calculateCsat("post_purchase");
const sentiment = await cxp.feedback.analyzeSentiment("Great service!");
```

### CDP Domain

Handles customer data, interactions, and segmentation.

```typescript
// Customers
const customer = await cxp.cdp.createCustomer("user@example.com", {
  name: "John Doe",
  company: "Acme Inc"
});
await cxp.cdp.updateCustomer(customer.id, { phone: "+1234567890" });

// Interactions
await cxp.cdp.recordInteraction(customer.id, "page_view", {
  page: "/pricing"
});
const journey = await cxp.cdp.getJourney(customer.id);

// Segments
const segment = await cxp.cdp.createSegment(
  "High Value Customers",
  { lifetime_value: { gt: 1000 } }
);
const members = await cxp.cdp.getSegmentMembers(segment.id);

// 360 View
const view = await cxp.cdp.getCustomer360(customer.id);
```

### Resources

Access CXP resources directly.

```typescript
// Customer profiles
const profile = await cxp.getCustomerProfile(customerId);

// Knowledge articles
const article = await cxp.getKnowledgeArticle(articleId);

// Metrics dashboard
const metrics = await cxp.getDashboardMetrics();
```

## TypeScript Support

The SDK is written in TypeScript with full type definitions. All methods return properly typed results.

```typescript
import type { Ticket, Customer, Survey } from "@cxp/sdk";

const ticket: Ticket = await cxp.support.createTicket(/* ... */);
const customer: Customer = await cxp.cdp.getCustomer(id);
```

## Building

```bash
npm run build
```

## Testing

```bash
npm test
npm run test:watch
```

## License

MIT
