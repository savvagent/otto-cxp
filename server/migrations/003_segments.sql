-- Migration 003: Segments
-- CXP (Customer Experience Platform)
-- Creates customer segmentation tables

BEGIN;

-- Segments
CREATE TABLE segments (
    id           UUID        PRIMARY KEY DEFAULT uuid_generate_v4(),
    name         VARCHAR     NOT NULL,
    description  TEXT,
    criteria     JSONB       NOT NULL,
    segment_type VARCHAR     NOT NULL DEFAULT 'dynamic',
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Segment Members (junction table)
CREATE TABLE segment_members (
    segment_id  UUID        NOT NULL REFERENCES segments(id) ON DELETE CASCADE,
    customer_id UUID        NOT NULL REFERENCES customers(id) ON DELETE CASCADE,
    added_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (segment_id, customer_id)
);

-- GIN index on segment criteria
CREATE INDEX idx_segments_criteria ON segments USING GIN (criteria);

-- Additional useful indexes
CREATE INDEX idx_segment_members_customer_id ON segment_members(customer_id);
CREATE INDEX idx_segments_segment_type       ON segments(segment_type);

COMMIT;
