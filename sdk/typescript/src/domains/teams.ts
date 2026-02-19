/**
 * Teams domain - handles team management
 */

import type { Client } from "@modelcontextprotocol/sdk/client/index.js";
import type { Team } from "../types.js";

export interface CreateTeamOptions {
  description?: string;
  owner?: string;
  metadata?: Record<string, unknown>;
}

export interface UpdateTeamOptions {
  name?: string;
  description?: string;
  owner?: string;
  metadata?: Record<string, unknown>;
}

export interface ListTeamsOptions {
  limit?: number;
  offset?: number;
}

export class TeamsDomain {
  constructor(private client: Client) {}

  private async call<T>(name: string, args: Record<string, unknown>): Promise<T> {
    const result = await this.client.callTool({ name, arguments: args });
    const text = (result.content as Array<{ type: string; text: string }>)[0]?.text ?? "{}";
    try {
      return JSON.parse(text) as T;
    } catch {
      throw new Error(text);
    }
  }

  /**
   * Create a new team
   */
  async create(name: string, options?: CreateTeamOptions): Promise<Team> {
    return this.call("create_team", {
      name,
      description: options?.description,
      owner: options?.owner,
      metadata: options?.metadata ? JSON.stringify(options.metadata) : undefined,
    });
  }

  /**
   * Get a team by ID
   */
  async get(id: string): Promise<Team> {
    return this.call("get_team", { id });
  }

  /**
   * List teams with optional pagination
   */
  async list(options?: ListTeamsOptions): Promise<Team[]> {
    return this.call("list_teams", {
      limit: options?.limit,
      offset: options?.offset,
    });
  }

  /**
   * Update an existing team
   */
  async update(id: string, updates: UpdateTeamOptions): Promise<Team> {
    return this.call("update_team", {
      id,
      name: updates.name,
      description: updates.description,
      owner: updates.owner,
      metadata: updates.metadata ? JSON.stringify(updates.metadata) : undefined,
    });
  }

  /**
   * Delete a team by ID
   */
  async delete(id: string): Promise<string> {
    return this.call("delete_team", { id });
  }
}
