-- Migration 5: Add context fields to feedback for segmentation
-- Feedback can be scoped by experience, application, page, and component.

ALTER TABLE feedback
    ADD COLUMN experience VARCHAR,
    ADD COLUMN application VARCHAR,
    ADD COLUMN page VARCHAR,
    ADD COLUMN component VARCHAR;

CREATE INDEX idx_feedback_experience ON feedback(experience);
CREATE INDEX idx_feedback_application ON feedback(application);
CREATE INDEX idx_feedback_page ON feedback(page);
CREATE INDEX idx_feedback_component ON feedback(component);

-- Composite index for common filtering patterns
CREATE INDEX idx_feedback_app_page ON feedback(application, page);
CREATE INDEX idx_feedback_app_page_component ON feedback(application, page, component);

-- Also add context to embeddings so vector search can be scoped
ALTER TABLE feedback_embeddings
    ADD COLUMN experience VARCHAR,
    ADD COLUMN application VARCHAR,
    ADD COLUMN page VARCHAR,
    ADD COLUMN component VARCHAR;

CREATE INDEX idx_feedback_embeddings_app ON feedback_embeddings(application);
CREATE INDEX idx_feedback_embeddings_app_page ON feedback_embeddings(application, page);
