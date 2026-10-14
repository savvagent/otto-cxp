import { connectCxp } from "../src/index.js";

// Placeholder catalog IDs. The server expects UUIDs of existing catalog
// entities for feedback context; replace these with real ones.
const WEB_APP_ID = "00000000-0000-4000-8000-000000000001";
const SETTINGS_PAGE_ID = "00000000-0000-4000-8000-000000000002";

async function main() {
  // Connect over streamable HTTP with a bearer token
  const { cxp, close } = await connectCxp({
    url: process.env.CXP_URL ?? "http://127.0.0.1:8080/mcp",
    token: process.env.CXP_TOKEN,
    name: "cxp-example",
  });

  console.log("Connected to CXP platform\n");

  // Example 1: Create a customer
  console.log("Creating customer...");
  const customer = await cxp.cdp.createCustomer("john.doe@example.com", {
    name: "John Doe",
    company: "Acme Inc",
  });
  console.log("Customer created:", customer.id);

  // Example 2: Create a support ticket
  console.log("\nCreating support ticket...");
  const ticket = await cxp.support.createTicket(
    customer.id,
    "Cannot access dashboard",
    {
      description: "Getting 404 error when trying to access /dashboard",
      priority: "high",
      channel: "email",
    }
  );
  console.log("Ticket created:", ticket.id);

  // Example 3: Record an interaction
  console.log("\nRecording customer interaction...");
  const interaction = await cxp.cdp.recordInteraction(
    customer.id,
    "page_view",
    {
      page: "/dashboard",
      referrer: "/login",
    }
  );
  console.log("Interaction recorded:", interaction.id);

  // Example 4: Submit feedback
  console.log("\nSubmitting customer feedback...");
  const feedback = await cxp.feedback.submitFeedback(
    customer.id,
    "feature_request",
    {
      subject: "Dark mode support",
      body: "Would love to see a dark mode option in the dashboard",
      rating: 4,
      applicationId: WEB_APP_ID,
      pageId: SETTINGS_PAGE_ID,
    }
  );
  console.log("Feedback submitted:", feedback.id);

  // Example 5: Get customer 360 view
  console.log("\nFetching customer 360 view...");
  const customer360 = await cxp.cdp.getCustomer360(customer.id);
  console.log("Customer 360:");
  console.log("- Customer:", customer360.customer.email);
  console.log("- Recent tickets:", customer360.recentTickets.length);
  console.log("- Recent interactions:", customer360.recentInteractions.length);
  console.log("- Recent feedback:", customer360.recentFeedback.length);

  // Example 6: Search knowledge base
  console.log("\nSearching knowledge base...");
  const articles = await cxp.support.searchKnowledgeBase("password reset");
  console.log(`Found ${articles.length} articles`);
  if (articles.length > 0) {
    console.log("First article:", articles[0]?.title);
  }

  // Example 7: Analyze sentiment
  console.log("\nAnalyzing sentiment...");
  const sentiment = await cxp.feedback.analyzeSentiment(
    "This product is absolutely amazing! I love it."
  );
  console.log("Sentiment:", sentiment.label);
  console.log("Score:", sentiment.score);

  // Example 8: Update ticket status
  console.log("\nUpdating ticket status...");
  const updatedTicket = await cxp.support.updateTicket(ticket.id, {
    status: "in_progress",
    assignedTo: "agent-123",
  });
  console.log("Ticket status:", updatedTicket.status);

  // Example 9: Create a customer segment
  console.log("\nCreating customer segment...");
  const segment = await cxp.cdp.createSegment(
    "Active Users",
    {
      last_interaction: { within_days: 30 },
    },
    {
      description: "Users who interacted in the last 30 days",
      segmentType: "behavioral",
    }
  );
  console.log("Segment created:", segment.id);

  // Example 10: Get customer journey
  console.log("\nFetching customer journey...");
  const journey = await cxp.cdp.getJourney(customer.id, 10);
  console.log(`Journey has ${journey.length} interactions`);

  console.log("\nExample completed successfully!");

  // Close the connection
  await close();
}

main().catch((error) => {
  console.error("Error:", error);
  process.exit(1);
});
