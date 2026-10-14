import type { CxpClient } from "@cxp/sdk";

export interface SeededIds {
  customerId: string;
  surveyId: string;
  sessionId: string;
}

/**
 * Seeds the database via the real CXP server with sample data
 * and returns the created IDs for use in the demo.
 */
export async function seedMockData(cxp: CxpClient): Promise<SeededIds> {
  const suffix = Date.now().toString(36);

  // 1. Create a customer
  const customer = await cxp.cdp.createCustomer(`jane.doe+${suffix}@example.com`, {
    name: "Jane Doe",
    phone: "+1-555-0123",
    company: "Acme Corp",
  });

  // 2. Create tickets
  const ticket1 = await cxp.support.createTicket(
    customer.id,
    "Dashboard charts not loading on Firefox",
    {
      description:
        "When opening the analytics dashboard in Firefox 120+, the charts render as blank white boxes. Works fine in Chrome and Safari. Console shows a WebGL context error.",
      priority: "high",
      channel: "web",
    }
  );

  await cxp.support.createTicket(customer.id, "Add CSV export to reports", {
    description:
      "It would be great to have a CSV export button on the reports page so we can import data into our BI tools.",
    priority: "medium",
    channel: "email",
  });

  const ticket3 = await cxp.support.createTicket(
    customer.id,
    "Billing page shows wrong currency",
    {
      description:
        "Our account is set to EUR but the billing page keeps showing USD amounts. This started after the last update.",
      priority: "high",
      channel: "web",
    }
  );

  // Move one ticket to in_progress
  await cxp.support.updateTicket(ticket3.id, {
    status: "in_progress",
    assignedTo: "agent-sarah",
  });

  // 4. Record interactions
  await cxp.cdp.recordInteraction(customer.id, "page_view", {
    page: "/dashboard",
    duration: 120,
  });
  await cxp.cdp.recordInteraction(customer.id, "feature_use", {
    feature: "report-builder",
    duration: 300,
  });
  await cxp.cdp.recordInteraction(customer.id, "support_contact", {
    channel: "chat",
    topic: "billing",
  });

  // 5. Submit feedback
  await cxp.feedback.submitFeedback(customer.id, "praise", {
    subject: "Love the new onboarding flow",
    body: "The step-by-step setup wizard made it really easy to get started. Great job!",
    rating: 5,
    applicationId: "Web App",
    pageId: "/onboarding",
  });

  await cxp.feedback.submitFeedback(customer.id, "feature_request", {
    subject: "Dark mode support",
    body: "Would love a dark mode option for the dashboard. I work late at night and the bright UI is harsh.",
    rating: 4,
    applicationId: "Web App",
    pageId: "/settings",
  });

  await cxp.feedback.submitFeedback(customer.id, "bug_report", {
    subject: "Notification bell count is off",
    body: "The notification badge shows 3 but there are actually 5 unread notifications.",
    rating: 2,
    applicationId: "Web App",
    pageId: "/notifications",
    component: "NotificationBell",
  });

  // 6. Create a survey with questions
  const survey = await cxp.feedback.createSurvey(
    "Product Satisfaction Survey - Q1 2026",
    [
      {
        id: "q1",
        text: "How likely are you to recommend us to a colleague? (1-10)",
        questionType: "rating",
        required: true,
      },
      {
        id: "q2",
        text: "Which features do you use most?",
        questionType: "multiple_choice",
        options: [
          "Dashboard",
          "Reports",
          "Integrations",
          "Team Management",
          "API",
        ],
        required: true,
      },
      {
        id: "q3",
        text: "What could we improve?",
        questionType: "text",
        required: false,
      },
    ],
    "Help us understand how we're doing and where we can improve."
  );

  // Submit a few survey responses to get NPS data
  await cxp.feedback.submitSurveyResponse({
    surveyId: survey.id,
    customerId: customer.id,
    answers: [
      { questionId: "q1", value: "9" },
      { questionId: "q2", value: "Dashboard" },
      { questionId: "q3", value: "More keyboard shortcuts would be awesome" },
    ],
  });

  // Create a second customer for more NPS data points
  const customer2 = await cxp.cdp.createCustomer(`bob.smith+${suffix}@example.com`, {
    name: "Bob Smith",
    company: "TechStart Inc",
  });

  await cxp.feedback.submitSurveyResponse({
    surveyId: survey.id,
    customerId: customer2.id,
    answers: [
      { questionId: "q1", value: "8" },
      { questionId: "q2", value: "Reports" },
    ],
  });

  // 7. Start a chat session
  const session = await cxp.support.startChatSession(customer.id);
  await cxp.support.sendChatMessage(
    session.id,
    "customer",
    "Hi, I'm having trouble with my dashboard charts not loading."
  );
  await cxp.support.sendChatMessage(
    session.id,
    "agent",
    "Hello Jane! I'm sorry to hear that. Can you tell me which browser you're using?"
  );
  await cxp.support.sendChatMessage(
    session.id,
    "customer",
    "Firefox 120. It works fine in Chrome."
  );

  return {
    customerId: customer.id,
    surveyId: survey.id,
    sessionId: session.id,
  };
}
