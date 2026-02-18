-- Make parent FK columns on catalog entities nullable (remove cascading requirement)
ALTER TABLE applications ALTER COLUMN experience_id DROP NOT NULL;
ALTER TABLE pages ALTER COLUMN application_id DROP NOT NULL;
ALTER TABLE components ALTER COLUMN page_id DROP NOT NULL;
