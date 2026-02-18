-- Migration 4: Feedback embeddings for semantic search via pgvector
CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE feedback_embeddings (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    feedback_id UUID NOT NULL REFERENCES feedback(id) ON DELETE CASCADE,
    chunk_index INT NOT NULL DEFAULT 0,
    chunk_text TEXT NOT NULL,
    embedding vector(1536),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(feedback_id, chunk_index)
);

CREATE INDEX idx_feedback_embeddings_feedback_id ON feedback_embeddings(feedback_id);
CREATE INDEX idx_feedback_embeddings_vector ON feedback_embeddings USING ivfflat (embedding vector_cosine_ops) WITH (lists = 100);
