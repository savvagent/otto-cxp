# CXP — Customer Experience Platform

An MCP server that gives AI assistants structured access to CRM, support ticketing, feedback management, and analytics — all backed by PostgreSQL with pgvector for semantic search.

## Quick Start

**Prerequisites:** Rust (stable), Docker or Podman, Node.js + pnpm

```sh
# 1. Start the database
docker compose up -d          # or: podman-compose up -d

# 2. Configure environment (optional — defaults work out of the box)
cp .env.example .env

# 3. Run the server
cargo run -p cxp-server       # stdio mode (default)
```

Migrations run automatically on startup.

### HTTP Transport

```sh
TRANSPORT=http cargo run -p cxp-server
# Listening on http://127.0.0.1:3000/mcp
```

## Project Structure

```
cxp/
├── server/                 # Rust MCP server
│   ├── src/
│   │   ├── tools/          # 26 MCP tools (CDP, support, feedback)
│   │   ├── db/             # Database layer
│   │   ├── models/         # Rust structs
│   │   ├── analysis/       # NPS, CSAT, sentiment
│   │   ├── prompts/        # MCP prompts
│   │   └── resources/      # MCP resources
│   ├── migrations/         # 5 SQL migrations
│   └── tests/
├── sdk/
│   ├── typescript/         # @cxp/sdk — full MCP client SDK
│   ├── swift/              # CxpSdk — iOS/macOS (stub, v2 in progress)
│   └── kotlin/             # com.cxp — Kotlin/JVM (stub)
└── packages/
    └── react/              # @cxp/react — React components and hooks
```

## Tools

The server exposes 26 tools across three domains:

### CDP (Customer Data Platform)

| Tool | Description |
|------|-------------|
| `create_customer` | Create a customer profile |
| `update_customer` | Update an existing customer |
| `get_customer` | Fetch a customer by ID |
| `search_customers` | Text search by name or email |
| `record_interaction` | Log an event (page view, purchase, login, etc.) |
| `get_journey` | Retrieve interaction history |
| `create_segment` | Define a customer segment with JSON criteria |
| `get_segment_members` | List members of a segment |
| `get_customer_360` | Full 360 view: customer + tickets + interactions + feedback |

### Support

| Tool | Description |
|------|-------------|
| `create_ticket` | Open a support ticket |
| `update_ticket` | Change status, priority, or assignee |
| `get_ticket` | Fetch a ticket by ID |
| `list_tickets` | List with filters and pagination |
| `close_ticket` | Close and timestamp resolution |
| `search_knowledge_base` | Full-text search over knowledge articles |
| `start_chat_session` | Begin a live chat session |
| `send_chat_message` | Add a message (customer/agent/system) |
| `end_chat_session` | Close a chat session |

### Feedback

| Tool | Description |
|------|-------------|
| `create_survey` | Define a survey with JSON questions |
| `send_survey` | Send a survey to a customer |
| `get_survey_responses` | Retrieve responses for a survey |
| `submit_feedback` | Submit feedback (bug report, feature request, complaint, praise, suggestion) |
| `get_feedback` | List feedback with filters |
| `calculate_nps` | Net Promoter Score from survey responses |
| `calculate_csat` | Customer Satisfaction Score from feedback ratings |
| `analyze_sentiment` | Text sentiment analysis with score and label |

## Database

PostgreSQL 17 with pgvector, run via Docker/Podman:

```sh
docker compose up -d
```

The schema includes customers, tickets, interactions, feedback, surveys, chat sessions, knowledge articles, segments, and feedback embeddings (vector(1536) with IVFFlat cosine similarity index).

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `DATABASE_URL` | `postgres://cxp:cxp_dev@localhost:5432/cxp` | PostgreSQL connection string |
| `TRANSPORT` | `stdio` | `stdio` or `http` |
| `HTTP_HOST` | `127.0.0.1` | Bind address (HTTP mode) |
| `HTTP_PORT` | `3000` | Port (HTTP mode) |
| `RUST_LOG` | `info` | Log level |

All values have defaults, so `.env` is optional.

## SDKs

### TypeScript (`@cxp/sdk`)

Full client SDK wrapping `@modelcontextprotocol/sdk`. Three domain objects: `cxp.support`, `cxp.feedback`, `cxp.cdp`.

```sh
cd sdk/typescript && pnpm install && pnpm build
```

### Swift (`CxpSdk`)

iOS/macOS SDK targeting the HTTP transport. Stub implementation — full version coming in v2.

### Kotlin (`com.cxp`)

Kotlin/JVM SDK. Stub implementation.

## Guides

- **[Surveys](sdk/typescript/SURVEYS.md)** — survey data model, SDK usage, question types, NPS calculation, and the React `<SurveyRenderer>` component

## React Components (`@cxp/react`)

Pre-built components and hooks for building CX interfaces:

**Components:** `CustomerCard`, `Customer360`, `TicketList`, `TicketDetail`, `FeedbackForm`, `ChatWidget`, `MetricsDashboard`, `NpsGauge`, `SurveyRenderer`

**Hooks:** `useCustomer`, `useTickets`, `useFeedback`, `useChat`, `useMetrics`

```sh
cd packages/react && pnpm install && pnpm build
```

## Development

```sh
# Build everything
cargo build                    # Rust server
pnpm install && pnpm build     # TypeScript SDK + React package

# Run tests
cargo test                     # Rust
pnpm test                      # TypeScript/React
```
