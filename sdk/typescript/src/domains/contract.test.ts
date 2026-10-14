/**
 * Tests that each fixed SDK call sends the argument names the cxp tools accept
 * (the param structs in server/src/tools/mod.rs), and that NPS/CSAT and
 * Customer 360 payloads type as the server returns them.
 */
import { describe, it, expect, beforeEach, vi } from "vitest";
import type { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { CdpDomain } from "./cdp.js";
import { FeedbackDomain } from "./feedback.js";
import type {
  Customer360,
  CsatResult,
  NpsResult,
} from "../types.js";

function reply(payload: unknown) {
  return { content: [{ type: "text", text: JSON.stringify(payload) }] } as any;
}

describe("tool argument contract", () => {
  let client: Client;
  let cdp: CdpDomain;
  let feedback: FeedbackDomain;

  beforeEach(() => {
    client = { callTool: vi.fn() } as unknown as Client;
    cdp = new CdpDomain(client);
    feedback = new FeedbackDomain(client);
  });

  it("get_segment_members sends id", async () => {
    vi.mocked(client.callTool).mockResolvedValue({
      structuredContent: { items: [] },
      content: [],
    } as any);
    await cdp.getSegmentMembers("seg-1");
    expect(client.callTool).toHaveBeenCalledWith({
      name: "get_segment_members",
      arguments: { id: "seg-1" },
    });
  });

  it("get_customer_360 sends id", async () => {
    vi.mocked(client.callTool).mockResolvedValue(reply({}));
    await cdp.getCustomer360("cust-1");
    expect(client.callTool).toHaveBeenCalledWith({
      name: "get_customer_360",
      arguments: { id: "cust-1" },
    });
  });

  it("update_customer sends only fields the tool accepts", async () => {
    vi.mocked(client.callTool).mockResolvedValue(reply({}));
    await cdp.updateCustomer("cust-1", { name: "Ada", company: "Acme" });
    expect(client.callTool).toHaveBeenCalledWith({
      name: "update_customer",
      arguments: {
        id: "cust-1",
        email: undefined,
        name: "Ada",
        phone: undefined,
        company: "Acme",
      },
    });
  });

  it("get_survey_responses sends id", async () => {
    vi.mocked(client.callTool).mockResolvedValue({
      structuredContent: { items: [] },
      content: [],
    } as any);
    await feedback.getResponses("survey-1");
    expect(client.callTool).toHaveBeenCalledWith({
      name: "get_survey_responses",
      arguments: { id: "survey-1" },
    });
  });

  it("submit_feedback sends the *_id catalog fields", async () => {
    vi.mocked(client.callTool).mockResolvedValue(reply({}));
    await feedback.submitFeedback("cust-1", "bug_report", {
      subject: "s",
      body: "b",
      rating: 2,
      experienceId: "exp-1",
      applicationId: "app-1",
      pageId: "page-1",
      componentId: "comp-1",
    });
    expect(client.callTool).toHaveBeenCalledWith({
      name: "submit_feedback",
      arguments: {
        customer_id: "cust-1",
        feedback_type: "bug_report",
        subject: "s",
        body: "b",
        rating: 2,
        experience_id: "exp-1",
        application_id: "app-1",
        page_id: "page-1",
        component_id: "comp-1",
      },
    });
  });

  it("calculate_nps sends id and returns counts alongside nps", async () => {
    const payload: NpsResult = {
      nps: 33.3,
      totalResponses: 7,
      scoredResponses: 6,
      promoters: 3,
      passives: 1,
      detractors: 2,
    };
    vi.mocked(client.callTool).mockResolvedValue(reply(payload));
    const result = await feedback.calculateNps("survey-1");
    expect(client.callTool).toHaveBeenCalledWith({
      name: "calculate_nps",
      arguments: { id: "survey-1" },
    });
    expect(result).toEqual(payload);
  });

  it("NpsResult and CsatResult allow a null score", async () => {
    const nps: NpsResult = {
      nps: null,
      totalResponses: 0,
      scoredResponses: 0,
      promoters: 0,
      passives: 0,
      detractors: 0,
    };
    const csat: CsatResult = {
      csat: null,
      totalFeedback: 0,
      ratedFeedback: 0,
      satisfied: 0,
      unsatisfied: 0,
    };
    vi.mocked(client.callTool).mockResolvedValueOnce(reply(nps));
    vi.mocked(client.callTool).mockResolvedValueOnce(reply(csat));
    expect((await feedback.calculateNps("s")).nps).toBeNull();
    expect((await feedback.calculateCsat()).csat).toBeNull();
  });

  it("calculate_csat sends feedback_type and returns counts", async () => {
    const payload: CsatResult = {
      csat: 75,
      totalFeedback: 5,
      ratedFeedback: 4,
      satisfied: 3,
      unsatisfied: 1,
    };
    vi.mocked(client.callTool).mockResolvedValue(reply(payload));
    const result = await feedback.calculateCsat("praise");
    expect(client.callTool).toHaveBeenCalledWith({
      name: "calculate_csat",
      arguments: { feedback_type: "praise" },
    });
    expect(result).toEqual(payload);
  });

  it("Customer360 carries ticket and feedback summaries", async () => {
    const payload: Customer360 = {
      customer: {
        id: "c",
        email: "a@b.c",
        metadata: {},
        tags: [],
        createdAt: "t",
        updatedAt: "t",
      },
      recentTickets: [
        {
          id: "t1",
          subject: "s",
          status: "open",
          priority: "low",
          channel: null,
          assignedTo: null,
          createdAt: "t",
          resolvedAt: null,
        },
      ],
      recentInteractions: [],
      recentFeedback: [
        {
          id: "f1",
          feedbackType: "praise",
          subject: null,
          body: null,
          rating: 5,
          createdAt: "t",
        },
      ],
    };
    vi.mocked(client.callTool).mockResolvedValue(reply(payload));
    const result = await cdp.getCustomer360("c");
    expect(result.recentTickets[0]?.status).toBe("open");
    expect(result.recentFeedback[0]?.rating).toBe(5);
  });
});
