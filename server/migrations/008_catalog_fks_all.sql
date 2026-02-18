-- Add optional catalog FK columns to customers, tickets, and surveys tables

ALTER TABLE customers ADD COLUMN experience_id UUID REFERENCES experiences(id) ON DELETE SET NULL;
ALTER TABLE customers ADD COLUMN application_id UUID REFERENCES applications(id) ON DELETE SET NULL;
ALTER TABLE customers ADD COLUMN page_id UUID REFERENCES pages(id) ON DELETE SET NULL;
ALTER TABLE customers ADD COLUMN component_id UUID REFERENCES components(id) ON DELETE SET NULL;

CREATE INDEX idx_customers_experience_id ON customers(experience_id);
CREATE INDEX idx_customers_application_id ON customers(application_id);
CREATE INDEX idx_customers_page_id ON customers(page_id);
CREATE INDEX idx_customers_component_id ON customers(component_id);

ALTER TABLE tickets ADD COLUMN experience_id UUID REFERENCES experiences(id) ON DELETE SET NULL;
ALTER TABLE tickets ADD COLUMN application_id UUID REFERENCES applications(id) ON DELETE SET NULL;
ALTER TABLE tickets ADD COLUMN page_id UUID REFERENCES pages(id) ON DELETE SET NULL;
ALTER TABLE tickets ADD COLUMN component_id UUID REFERENCES components(id) ON DELETE SET NULL;

CREATE INDEX idx_tickets_experience_id ON tickets(experience_id);
CREATE INDEX idx_tickets_application_id ON tickets(application_id);
CREATE INDEX idx_tickets_page_id ON tickets(page_id);
CREATE INDEX idx_tickets_component_id ON tickets(component_id);

ALTER TABLE surveys ADD COLUMN experience_id UUID REFERENCES experiences(id) ON DELETE SET NULL;
ALTER TABLE surveys ADD COLUMN application_id UUID REFERENCES applications(id) ON DELETE SET NULL;
ALTER TABLE surveys ADD COLUMN page_id UUID REFERENCES pages(id) ON DELETE SET NULL;
ALTER TABLE surveys ADD COLUMN component_id UUID REFERENCES components(id) ON DELETE SET NULL;

CREATE INDEX idx_surveys_experience_id ON surveys(experience_id);
CREATE INDEX idx_surveys_application_id ON surveys(application_id);
CREATE INDEX idx_surveys_page_id ON surveys(page_id);
CREATE INDEX idx_surveys_component_id ON surveys(component_id);
