/**
 * CDP (Customer Data Platform) domain - handles customers, interactions, segments, and 360 views
 */

import { callList, callTool } from "../call.js";
import type { Client } from "@modelcontextprotocol/sdk/client/index.js";
import type {
  Customer,
  Interaction,
  Segment,
  Customer360,
} from "../types.js";

export interface CreateCustomerOptions {
  name?: string;
  phone?: string;
  company?: string;
}

export interface UpdateCustomerOptions {
  email?: string;
  name?: string;
  phone?: string;
  company?: string;
}

export interface CreateSegmentOptions {
  description?: string;
  segmentType?: string;
}

export class CdpDomain {
  constructor(private client: Client) {}

  /**
   * Create a new customer
   */
  async createCustomer(
    email: string,
    options?: CreateCustomerOptions
  ): Promise<Customer> {
    return callTool(this.client, "create_customer", {
      email,
      name: options?.name,
      phone: options?.phone,
      company: options?.company,
    });
  }

  /**
   * Update an existing customer
   */
  async updateCustomer(
    id: string,
    updates: UpdateCustomerOptions
  ): Promise<Customer> {
    return callTool(this.client, "update_customer", {
      id,
      email: updates.email,
      name: updates.name,
      phone: updates.phone,
      company: updates.company,
    });
  }

  /**
   * Get a customer by ID
   */
  async getCustomer(id: string): Promise<Customer> {
    return callTool(this.client, "get_customer", { id });
  }

  /**
   * Search for customers
   */
  async searchCustomers(query: string): Promise<Customer[]> {
    return callList(this.client, "search_customers", { query });
  }

  /**
   * Record a customer interaction
   */
  async recordInteraction(
    customerId: string,
    interactionType: string,
    metadata?: Record<string, unknown>
  ): Promise<Interaction> {
    return callTool(this.client, "record_interaction", {
      customer_id: customerId,
      interaction_type: interactionType,
      metadata: metadata ? JSON.stringify(metadata) : undefined,
    });
  }

  /**
   * Get customer journey (interaction history)
   */
  async getJourney(customerId: string, limit?: number): Promise<Interaction[]> {
    return callList(this.client, "get_journey", {
      customer_id: customerId,
      limit,
    });
  }

  /**
   * Create a customer segment
   */
  async createSegment(
    name: string,
    criteria: Record<string, unknown>,
    options?: CreateSegmentOptions
  ): Promise<Segment> {
    return callTool(this.client, "create_segment", {
      name,
      criteria: JSON.stringify(criteria),
      description: options?.description,
      segment_type: options?.segmentType,
    });
  }

  /**
   * Get members of a segment
   */
  async getSegmentMembers(segmentId: string): Promise<Customer[]> {
    return callList(this.client, "get_segment_members", {
      id: segmentId,
    });
  }

  /**
   * Get 360-degree view of a customer
   */
  async getCustomer360(customerId: string): Promise<Customer360> {
    return callTool(this.client, "get_customer_360", {
      id: customerId,
    });
  }
}
