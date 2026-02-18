-- Add optional catalog FK columns to feedback table
ALTER TABLE feedback ADD COLUMN experience_id UUID REFERENCES experiences(id) ON DELETE SET NULL;
ALTER TABLE feedback ADD COLUMN application_id UUID REFERENCES applications(id) ON DELETE SET NULL;
ALTER TABLE feedback ADD COLUMN page_id UUID REFERENCES pages(id) ON DELETE SET NULL;
ALTER TABLE feedback ADD COLUMN component_id UUID REFERENCES components(id) ON DELETE SET NULL;

CREATE INDEX idx_feedback_experience_id ON feedback(experience_id);
CREATE INDEX idx_feedback_application_id ON feedback(application_id);
CREATE INDEX idx_feedback_page_id ON feedback(page_id);
CREATE INDEX idx_feedback_component_id ON feedback(component_id);
