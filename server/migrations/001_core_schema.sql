-- Migration 001: Core Schema
-- CXP (Customer Experience Platform)
-- Creates extensions, enums, and core tables

BEGIN;

-- Extensions
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
CREATE EXTENSION IF NOT EXISTS "pg_trgm";

-- Enums
CREATE TYPE ticket_status AS ENUM ('open', 'in_progress', 'waiting', 'resolved', 'closed');
CREATE TYPE ticket_priority AS ENUM ('low', 'medium', 'high', 'urgent');
CREATE TYPE ticket_channel AS ENUM ('email', 'chat', 'phone', 'web', 'social');
CREATE TYPE interaction_type AS ENUM ('page_view', 'purchase', 'support_contact', 'email_open', 'email_click', 'feature_use', 'login');
CREATE TYPE feedback_type AS ENUM ('bug_report', 'feature_request', 'complaint', 'praise', 'suggestion');
CREATE TYPE message_role AS ENUM ('customer', 'agent', 'system');

-- Customers
CREATE TABLE customers (
    id          UUID        PRIMARY KEY DEFAULT uuid_generate_v4(),
    email       VARCHAR     UNIQUE NOT NULL,
    name        VARCHAR,
    phone       VARCHAR,
    company     VARCHAR,
    metadata    JSONB       NOT NULL DEFAULT '{}',
    tags        TEXT[]      NOT NULL DEFAULT '{}',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Tickets
CREATE TABLE tickets (
    id          UUID            PRIMARY KEY DEFAULT uuid_generate_v4(),
    customer_id UUID            NOT NULL REFERENCES customers(id) ON DELETE CASCADE,
    subject     VARCHAR         NOT NULL,
    description TEXT,
    status      ticket_status   NOT NULL DEFAULT 'open',
    priority    ticket_priority NOT NULL DEFAULT 'medium',
    channel     ticket_channel,
    assigned_to VARCHAR,
    tags        TEXT[]          NOT NULL DEFAULT '{}',
    metadata    JSONB           NOT NULL DEFAULT '{}',
    resolved_at TIMESTAMPTZ,
    created_at  TIMESTAMPTZ     NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ     NOT NULL DEFAULT now()
);

-- Interactions
CREATE TABLE interactions (
    id               UUID             PRIMARY KEY DEFAULT uuid_generate_v4(),
    customer_id      UUID             NOT NULL REFERENCES customers(id) ON DELETE CASCADE,
    interaction_type interaction_type NOT NULL,
    metadata         JSONB            NOT NULL DEFAULT '{}',
    created_at       TIMESTAMPTZ      NOT NULL DEFAULT now()
);

-- Feedback
CREATE TABLE feedback (
    id            UUID          PRIMARY KEY DEFAULT uuid_generate_v4(),
    customer_id   UUID          NOT NULL REFERENCES customers(id) ON DELETE CASCADE,
    feedback_type feedback_type NOT NULL,
    subject       VARCHAR,
    body          TEXT,
    rating        INT           CHECK (rating >= 1 AND rating <= 5),
    metadata      JSONB         NOT NULL DEFAULT '{}',
    created_at    TIMESTAMPTZ   NOT NULL DEFAULT now()
);

-- Surveys
CREATE TABLE surveys (
    id          UUID        PRIMARY KEY DEFAULT uuid_generate_v4(),
    title       VARCHAR     NOT NULL,
    description TEXT,
    questions   JSONB       NOT NULL,
    active      BOOLEAN     NOT NULL DEFAULT true,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Survey Responses
CREATE TABLE survey_responses (
    id           UUID        PRIMARY KEY DEFAULT uuid_generate_v4(),
    survey_id    UUID        NOT NULL REFERENCES surveys(id) ON DELETE CASCADE,
    customer_id  UUID        NOT NULL REFERENCES customers(id) ON DELETE CASCADE,
    answers      JSONB       NOT NULL,
    completed_at TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Chat Sessions
CREATE TABLE chat_sessions (
    id          UUID        PRIMARY KEY DEFAULT uuid_generate_v4(),
    customer_id UUID        NOT NULL REFERENCES customers(id) ON DELETE CASCADE,
    status      VARCHAR     NOT NULL DEFAULT 'active',
    metadata    JSONB       NOT NULL DEFAULT '{}',
    started_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    ended_at    TIMESTAMPTZ
);

-- Chat Messages
CREATE TABLE chat_messages (
    id         UUID         PRIMARY KEY DEFAULT uuid_generate_v4(),
    session_id UUID         NOT NULL REFERENCES chat_sessions(id) ON DELETE CASCADE,
    role       message_role NOT NULL,
    content    TEXT         NOT NULL,
    metadata   JSONB        NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ  NOT NULL DEFAULT now()
);

-- Indexes: GIN on JSONB metadata columns
CREATE INDEX idx_customers_metadata    ON customers    USING GIN (metadata);
CREATE INDEX idx_tickets_metadata      ON tickets      USING GIN (metadata);
CREATE INDEX idx_interactions_metadata ON interactions  USING GIN (metadata);
CREATE INDEX idx_feedback_metadata     ON feedback      USING GIN (metadata);
CREATE INDEX idx_chat_sessions_metadata ON chat_sessions USING GIN (metadata);
CREATE INDEX idx_chat_messages_metadata ON chat_messages USING GIN (metadata);

-- Indexes: GIN on tags arrays
CREATE INDEX idx_customers_tags ON customers USING GIN (tags);
CREATE INDEX idx_tickets_tags   ON tickets   USING GIN (tags);

-- Indexes: trigram on customers.name
CREATE INDEX idx_customers_name_trgm ON customers USING GIN (name gin_trgm_ops);

-- Indexes: btree on customer_id foreign keys
CREATE INDEX idx_tickets_customer_id          ON tickets(customer_id);
CREATE INDEX idx_interactions_customer_id     ON interactions(customer_id);
CREATE INDEX idx_feedback_customer_id         ON feedback(customer_id);
CREATE INDEX idx_survey_responses_customer_id ON survey_responses(customer_id);
CREATE INDEX idx_chat_sessions_customer_id    ON chat_sessions(customer_id);

-- Indexes: ticket status and priority
CREATE INDEX idx_tickets_status   ON tickets(status);
CREATE INDEX idx_tickets_priority ON tickets(priority);

-- Indexes: additional useful lookups
CREATE INDEX idx_survey_responses_survey_id ON survey_responses(survey_id);
CREATE INDEX idx_chat_messages_session_id   ON chat_messages(session_id);
CREATE INDEX idx_interactions_type          ON interactions(interaction_type);
CREATE INDEX idx_interactions_created_at    ON interactions(created_at);

COMMIT;
