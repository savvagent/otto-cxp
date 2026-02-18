/**
 * Tests for CxpClient
 */

import { describe, it, expect, beforeEach, vi } from "vitest";
import type { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { CxpClient } from "./client.js";

describe("CxpClient", () => {
  let mockMcpClient: Client;
  let cxpClient: CxpClient;

  beforeEach(() => {
    // Create mock MCP client
    mockMcpClient = {
      callTool: vi.fn(),
      readResource: vi.fn(),
      connect: vi.fn(),
      close: vi.fn(),
    } as unknown as Client;

    cxpClient = new CxpClient({ client: mockMcpClient });
  });

  describe("initialization", () => {
    it("should create domain instances", () => {
      expect(cxpClient.support).toBeDefined();
      expect(cxpClient.feedback).toBeDefined();
      expect(cxpClient.cdp).toBeDefined();
    });
  });

  describe("readResource", () => {
    it("should read a resource by URI", async () => {
      const mockContent = "Resource content";
      vi.mocked(mockMcpClient.readResource).mockResolvedValue({
        contents: [{ type: "text", text: mockContent }],
      } as any);

      const result = await cxpClient.readResource("test://resource");

      expect(mockMcpClient.readResource).toHaveBeenCalledWith({
        uri: "test://resource",
      });
      expect(result).toBe(mockContent);
    });

    it("should return empty string if no content", async () => {
      vi.mocked(mockMcpClient.readResource).mockResolvedValue({
        contents: [],
      } as any);

      const result = await cxpClient.readResource("test://resource");

      expect(result).toBe("");
    });
  });

  describe("getCustomerProfile", () => {
    it("should read customer profile resource", async () => {
      const mockProfile = "Customer profile data";
      vi.mocked(mockMcpClient.readResource).mockResolvedValue({
        contents: [{ type: "text", text: mockProfile }],
      } as any);

      const result = await cxpClient.getCustomerProfile("customer-123");

      expect(mockMcpClient.readResource).toHaveBeenCalledWith({
        uri: "customer://profiles/customer-123",
      });
      expect(result).toBe(mockProfile);
    });
  });

  describe("getKnowledgeArticle", () => {
    it("should read knowledge article resource", async () => {
      const mockArticle = "Article content";
      vi.mocked(mockMcpClient.readResource).mockResolvedValue({
        contents: [{ type: "text", text: mockArticle }],
      } as any);

      const result = await cxpClient.getKnowledgeArticle("article-456");

      expect(mockMcpClient.readResource).toHaveBeenCalledWith({
        uri: "knowledge://articles/article-456",
      });
      expect(result).toBe(mockArticle);
    });
  });

  describe("getDashboardMetrics", () => {
    it("should read dashboard metrics resource", async () => {
      const mockMetrics = '{"tickets": {"total": 100}}';
      vi.mocked(mockMcpClient.readResource).mockResolvedValue({
        contents: [{ type: "text", text: mockMetrics }],
      } as any);

      const result = await cxpClient.getDashboardMetrics();

      expect(mockMcpClient.readResource).toHaveBeenCalledWith({
        uri: "metrics://dashboard",
      });
      expect(result).toBe(mockMetrics);
    });
  });
});
