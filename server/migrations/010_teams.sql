-- Teams table
CREATE TABLE IF NOT EXISTS teams (
    id UUID PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    description TEXT,
    owner VARCHAR(255),
    metadata JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Add team_id FK to catalog entities
ALTER TABLE experiences ADD COLUMN IF NOT EXISTS team_id UUID REFERENCES teams(id);
ALTER TABLE applications ADD COLUMN IF NOT EXISTS team_id UUID REFERENCES teams(id);
ALTER TABLE pages ADD COLUMN IF NOT EXISTS team_id UUID REFERENCES teams(id);
ALTER TABLE components ADD COLUMN IF NOT EXISTS team_id UUID REFERENCES teams(id);

-- Indexes for team_id lookups
CREATE INDEX IF NOT EXISTS idx_experiences_team_id ON experiences(team_id);
CREATE INDEX IF NOT EXISTS idx_applications_team_id ON applications(team_id);
CREATE INDEX IF NOT EXISTS idx_pages_team_id ON pages(team_id);
CREATE INDEX IF NOT EXISTS idx_components_team_id ON components(team_id);
