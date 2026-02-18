-- Migration 002: Knowledge Base
-- CXP (Customer Experience Platform)
-- Creates knowledge articles table with full-text search

BEGIN;

-- Knowledge Articles
CREATE TABLE knowledge_articles (
    id         UUID        PRIMARY KEY DEFAULT uuid_generate_v4(),
    title      VARCHAR     NOT NULL,
    content    TEXT        NOT NULL,
    category   VARCHAR,
    tags       TEXT[]      NOT NULL DEFAULT '{}',
    metadata   JSONB       NOT NULL DEFAULT '{}',
    published  BOOLEAN     NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Full-text search index on title and content
CREATE INDEX idx_knowledge_articles_fts
    ON knowledge_articles
    USING GIN (to_tsvector('english', title || ' ' || content));

-- GIN index on tags
CREATE INDEX idx_knowledge_articles_tags
    ON knowledge_articles
    USING GIN (tags);

-- Additional useful indexes
CREATE INDEX idx_knowledge_articles_category  ON knowledge_articles(category);
CREATE INDEX idx_knowledge_articles_published ON knowledge_articles(published);
CREATE INDEX idx_knowledge_articles_metadata  ON knowledge_articles USING GIN (metadata);

COMMIT;
