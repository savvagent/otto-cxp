/**
 * Tests for SupportDomain
 */

import { describe, it, expect, beforeEach, vi } from "vitest";
import type { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { SupportDomain } from "./support.js";
import type { Ticket, ChatSession, ChatMessage, KnowledgeArticle } from "../types.js";

describe("SupportDomain", () => {
  let mockClient: Client;
  let support: SupportDomain;

  beforeEach(() => {
    mockClient = {
      callTool: vi.fn(),
    } as unknown as Client;

    support = new SupportDomain(mockClient);
  });

  describe("createTicket", () => {
    it("should create a ticket with basic info", async () => {
      const mockTicket: Ticket = {
        id: "ticket-123",
        customerId: "customer-456",
        subject: "Test issue",
        status: "open",
        priority: "medium",
        tags: [],
        metadata: {},
        createdAt: "2026-02-17T00:00:00Z",
        updatedAt: "2026-02-17T00:00:00Z",
      };

      vi.mocked(mockClient.callTool).mockResolvedValue({
        content: [{ type: "text", text: JSON.stringify(mockTicket) }],
      } as any);

      const result = await support.createTicket("customer-456", "Test issue");

      expect(mockClient.callTool).toHaveBeenCalledWith({
        name: "create_ticket",
        arguments: {
          customer_id: "customer-456",
          subject: "Test issue",
          description: undefined,
          priority: undefined,
          channel: undefined,
        },
      });
      expect(result).toEqual(mockTicket);
    });

    it("should create a ticket with options", async () => {
      const mockTicket: Ticket = {
        id: "ticket-123",
        customerId: "customer-456",
        subject: "Test issue",
        description: "Detailed description",
        status: "open",
        priority: "high",
        channel: "email",
        tags: [],
        metadata: {},
        createdAt: "2026-02-17T00:00:00Z",
        updatedAt: "2026-02-17T00:00:00Z",
      };

      vi.mocked(mockClient.callTool).mockResolvedValue({
        content: [{ type: "text", text: JSON.stringify(mockTicket) }],
      } as any);

      const result = await support.createTicket("customer-456", "Test issue", {
        description: "Detailed description",
        priority: "high",
        channel: "email",
      });

      expect(mockClient.callTool).toHaveBeenCalledWith({
        name: "create_ticket",
        arguments: {
          customer_id: "customer-456",
          subject: "Test issue",
          description: "Detailed description",
          priority: "high",
          channel: "email",
        },
      });
      expect(result).toEqual(mockTicket);
    });
  });

  describe("updateTicket", () => {
    it("should update ticket status", async () => {
      const mockTicket: Ticket = {
        id: "ticket-123",
        customerId: "customer-456",
        subject: "Test issue",
        status: "in_progress",
        priority: "medium",
        tags: [],
        metadata: {},
        createdAt: "2026-02-17T00:00:00Z",
        updatedAt: "2026-02-17T00:00:00Z",
      };

      vi.mocked(mockClient.callTool).mockResolvedValue({
        content: [{ type: "text", text: JSON.stringify(mockTicket) }],
      } as any);

      const result = await support.updateTicket("ticket-123", {
        status: "in_progress",
      });

      expect(mockClient.callTool).toHaveBeenCalledWith({
        name: "update_ticket",
        arguments: {
          id: "ticket-123",
          status: "in_progress",
          priority: undefined,
          assigned_to: undefined,
        },
      });
      expect(result.status).toBe("in_progress");
    });
  });

  describe("getTicket", () => {
    it("should get a ticket by ID", async () => {
      const mockTicket: Ticket = {
        id: "ticket-123",
        customerId: "customer-456",
        subject: "Test issue",
        status: "open",
        priority: "medium",
        tags: [],
        metadata: {},
        createdAt: "2026-02-17T00:00:00Z",
        updatedAt: "2026-02-17T00:00:00Z",
      };

      vi.mocked(mockClient.callTool).mockResolvedValue({
        content: [{ type: "text", text: JSON.stringify(mockTicket) }],
      } as any);

      const result = await support.getTicket("ticket-123");

      expect(mockClient.callTool).toHaveBeenCalledWith({
        name: "get_ticket",
        arguments: { id: "ticket-123" },
      });
      expect(result).toEqual(mockTicket);
    });
  });

  describe("listTickets", () => {
    it("should list all tickets", async () => {
      const mockTickets: Ticket[] = [
        {
          id: "ticket-1",
          customerId: "customer-1",
          subject: "Issue 1",
          status: "open",
          priority: "medium",
          tags: [],
          metadata: {},
          createdAt: "2026-02-17T00:00:00Z",
          updatedAt: "2026-02-17T00:00:00Z",
        },
      ];

      vi.mocked(mockClient.callTool).mockResolvedValue({
        structuredContent: { items: mockTickets, total: mockTickets.length },
        content: [],
      } as any);

      const result = await support.listTickets();

      expect(mockClient.callTool).toHaveBeenCalledWith({
        name: "list_tickets",
        arguments: {
          customer_id: undefined,
          status: undefined,
          priority: undefined,
          limit: undefined,
          offset: undefined,
        },
      });
      expect(result).toEqual(mockTickets);
    });

    it("should list tickets with filters", async () => {
      vi.mocked(mockClient.callTool).mockResolvedValue({
        structuredContent: { items: [] },
        content: [],
      } as any);

      await support.listTickets({
        customerId: "customer-123",
        status: "open",
        priority: "high",
        limit: 10,
        offset: 0,
      });

      expect(mockClient.callTool).toHaveBeenCalledWith({
        name: "list_tickets",
        arguments: {
          customer_id: "customer-123",
          status: "open",
          priority: "high",
          limit: 10,
          offset: 0,
        },
      });
    });
  });

  describe("closeTicket", () => {
    it("should close a ticket", async () => {
      const mockTicket: Ticket = {
        id: "ticket-123",
        customerId: "customer-456",
        subject: "Test issue",
        status: "closed",
        priority: "medium",
        tags: [],
        metadata: {},
        resolvedAt: "2026-02-17T00:00:00Z",
        createdAt: "2026-02-17T00:00:00Z",
        updatedAt: "2026-02-17T00:00:00Z",
      };

      vi.mocked(mockClient.callTool).mockResolvedValue({
        content: [{ type: "text", text: JSON.stringify(mockTicket) }],
      } as any);

      const result = await support.closeTicket("ticket-123");

      expect(mockClient.callTool).toHaveBeenCalledWith({
        name: "close_ticket",
        arguments: { id: "ticket-123" },
      });
      expect(result.status).toBe("closed");
    });
  });

  describe("searchKnowledgeBase", () => {
    it("should search knowledge base", async () => {
      const mockArticles: KnowledgeArticle[] = [
        {
          id: "article-1",
          title: "How to reset password",
          content: "Instructions...",
          tags: ["password", "security"],
          published: true,
          createdAt: "2026-02-17T00:00:00Z",
          updatedAt: "2026-02-17T00:00:00Z",
        },
      ];

      vi.mocked(mockClient.callTool).mockResolvedValue({
        structuredContent: { items: mockArticles, total: mockArticles.length },
        content: [],
      } as any);

      const result = await support.searchKnowledgeBase("password reset");

      expect(mockClient.callTool).toHaveBeenCalledWith({
        name: "search_knowledge_base",
        arguments: { query: "password reset" },
      });
      expect(result).toEqual(mockArticles);
    });
  });

  describe("chat operations", () => {
    it("should start a chat session", async () => {
      const mockSession: ChatSession = {
        id: "session-123",
        customerId: "customer-456",
        status: "active",
        metadata: {},
        startedAt: "2026-02-17T00:00:00Z",
      };

      vi.mocked(mockClient.callTool).mockResolvedValue({
        content: [{ type: "text", text: JSON.stringify(mockSession) }],
      } as any);

      const result = await support.startChatSession("customer-456");

      expect(mockClient.callTool).toHaveBeenCalledWith({
        name: "start_chat_session",
        arguments: { customer_id: "customer-456" },
      });
      expect(result).toEqual(mockSession);
    });

    it("should send a chat message", async () => {
      const mockMessage: ChatMessage = {
        id: "message-123",
        sessionId: "session-456",
        role: "agent",
        content: "Hello!",
        metadata: {},
        createdAt: "2026-02-17T00:00:00Z",
      };

      vi.mocked(mockClient.callTool).mockResolvedValue({
        content: [{ type: "text", text: JSON.stringify(mockMessage) }],
      } as any);

      const result = await support.sendChatMessage(
        "session-456",
        "agent",
        "Hello!"
      );

      expect(mockClient.callTool).toHaveBeenCalledWith({
        name: "send_chat_message",
        arguments: {
          session_id: "session-456",
          role: "agent",
          content: "Hello!",
        },
      });
      expect(result).toEqual(mockMessage);
    });

    it("should end a chat session", async () => {
      const mockSession: ChatSession = {
        id: "session-123",
        customerId: "customer-456",
        status: "ended",
        metadata: {},
        startedAt: "2026-02-17T00:00:00Z",
        endedAt: "2026-02-17T01:00:00Z",
      };

      vi.mocked(mockClient.callTool).mockResolvedValue({
        content: [{ type: "text", text: JSON.stringify(mockSession) }],
      } as any);

      const result = await support.endChatSession("session-123");

      expect(mockClient.callTool).toHaveBeenCalledWith({
        name: "end_chat_session",
        arguments: { session_id: "session-123" },
      });
      expect(result.status).toBe("ended");
    });
  });
});
