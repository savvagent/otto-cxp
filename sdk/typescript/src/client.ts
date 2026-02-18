/**
 * Main CXP SDK client
 */

import type { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { SupportDomain } from "./domains/support.js";
import { FeedbackDomain } from "./domains/feedback.js";
import { CdpDomain } from "./domains/cdp.js";

export interface CxpClientOptions {
  /**
   * Pre-connected MCP Client instance
   */
  client: Client;
}

/**
 * CXP SDK Client - provides typed access to all CXP platform capabilities
 *
 * @example
 * ```typescript
 * import { Client } from "@modelcontextprotocol/sdk/client/index.js";
 * import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";
 * import { CxpClient } from "@cxp/sdk";
 *
 * const transport = new StdioClientTransport({
 *   command: "cxp-mcp-server"
 * });
 *
 * const mcpClient = new Client({
 *   name: "my-app",
 *   version: "1.0.0"
 * }, {
 *   capabilities: {}
 * });
 *
 * await mcpClient.connect(transport);
 *
 * const cxp = new CxpClient({ client: mcpClient });
 *
 * // Use the SDK
 * const ticket = await cxp.support.createTicket(
 *   "customer-123",
 *   "Issue with login"
 * );
 * ```
 */
export class CxpClient {
  private client: Client;

  /**
   * Support domain - tickets, knowledge base, chat
   */
  public readonly support: SupportDomain;

  /**
   * Feedback domain - surveys, feedback, NPS, CSAT, sentiment
   */
  public readonly feedback: FeedbackDomain;

  /**
   * CDP domain - customers, interactions, segments, 360 views
   */
  public readonly cdp: CdpDomain;

  constructor(options: CxpClientOptions) {
    this.client = options.client;
    this.support = new SupportDomain(this.client);
    this.feedback = new FeedbackDomain(this.client);
    this.cdp = new CdpDomain(this.client);
  }

  /**
   * Read a resource by URI
   */
  async readResource(uri: string): Promise<string> {
    const result = await this.client.readResource({ uri });
    const content = result.contents[0];
    return content && "text" in content ? content.text : "";
  }

  /**
   * Get customer profile resource
   */
  async getCustomerProfile(id: string): Promise<string> {
    return this.readResource(`customer://profiles/${id}`);
  }

  /**
   * Get knowledge article resource
   */
  async getKnowledgeArticle(id: string): Promise<string> {
    return this.readResource(`knowledge://articles/${id}`);
  }

  /**
   * Get dashboard metrics resource
   */
  async getDashboardMetrics(): Promise<string> {
    return this.readResource("metrics://dashboard");
  }
}
