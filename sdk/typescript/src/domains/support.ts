/**
 * Support domain - handles tickets, knowledge base, and chat
 */

import { callList, callTool } from "../call.js";
import type { Client } from "@modelcontextprotocol/sdk/client/index.js";
import type { Ticket, KnowledgeArticle, ChatSession, ChatMessage } from "../types.js";

export interface CreateTicketOptions {
  description?: string;
  priority?: string;
  channel?: string;
}

export interface UpdateTicketOptions {
  status?: string;
  priority?: string;
  assignedTo?: string;
}

export interface ListTicketsOptions {
  customerId?: string;
  status?: string;
  priority?: string;
  limit?: number;
  offset?: number;
}

export class SupportDomain {
  constructor(private client: Client) {}

  /**
   * Create a new support ticket
   */
  async createTicket(
    customerId: string,
    subject: string,
    options?: CreateTicketOptions
  ): Promise<Ticket> {
    return callTool(this.client, "create_ticket", {
      customer_id: customerId,
      subject,
      description: options?.description,
      priority: options?.priority,
      channel: options?.channel,
    });
  }

  /**
   * Update an existing ticket
   */
  async updateTicket(id: string, updates: UpdateTicketOptions): Promise<Ticket> {
    return callTool(this.client, "update_ticket", {
      id,
      status: updates.status,
      priority: updates.priority,
      assigned_to: updates.assignedTo,
    });
  }

  /**
   * Get a ticket by ID
   */
  async getTicket(id: string): Promise<Ticket> {
    return callTool(this.client, "get_ticket", { id });
  }

  /**
   * List tickets with optional filtering
   */
  async listTickets(options?: ListTicketsOptions): Promise<Ticket[]> {
    return callList(this.client, "list_tickets", {
      customer_id: options?.customerId || undefined,
      status: options?.status || undefined,
      priority: options?.priority || undefined,
      limit: options?.limit,
      offset: options?.offset,
    });
  }

  /**
   * Close a ticket
   */
  async closeTicket(id: string): Promise<Ticket> {
    return callTool(this.client, "close_ticket", { id });
  }

  /**
   * Search the knowledge base
   */
  async searchKnowledgeBase(query: string): Promise<KnowledgeArticle[]> {
    return callList(this.client, "search_knowledge_base", { query });
  }

  /**
   * Start a new chat session
   */
  async startChatSession(customerId: string): Promise<ChatSession> {
    return callTool(this.client, "start_chat_session", { customer_id: customerId });
  }

  /**
   * Send a message in a chat session
   */
  async sendChatMessage(
    sessionId: string,
    role: string,
    content: string
  ): Promise<ChatMessage> {
    return callTool(this.client, "send_chat_message", {
      session_id: sessionId,
      role,
      content,
    });
  }

  /**
   * Get all messages in a chat session
   */
  async getChatMessages(sessionId: string): Promise<ChatMessage[]> {
    return callList(this.client, "get_chat_messages", { session_id: sessionId });
  }

  /**
   * End a chat session
   */
  async endChatSession(sessionId: string): Promise<ChatSession> {
    return callTool(this.client, "end_chat_session", { session_id: sessionId });
  }
}
