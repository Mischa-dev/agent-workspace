-- W22: things that make agents' work easier.

-- A page with a skill description is a skill: instructions an agent loads
-- only when the description says they apply, like a Claude Code skill. The
-- page keeps its title, body and versions; the description is the one line
-- an agent reads to decide.
ALTER TABLE project_pages
    ADD COLUMN skill_description TEXT CHECK (
        skill_description IS NULL
        OR (char_length(skill_description) BETWEEN 1 AND 500 AND skill_description !~ '[[:cntrl:]]')
    );

CREATE INDEX project_pages_skills
    ON project_pages (project_id, lower(title))
    WHERE skill_description IS NOT NULL AND archived_at IS NULL;

-- What an agent (or person) leaves for whoever picks a task up next: where
-- the work got to and what's left. Written when a claim is released, read
-- when the task is claimed or opened.
CREATE TABLE work_item_handoffs (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id),
    work_item_id UUID NOT NULL REFERENCES work_items(id),
    actor_id UUID NOT NULL REFERENCES actors(id),
    via_oauth_grant_id UUID REFERENCES mcp_oauth_grants(id),
    note TEXT NOT NULL CHECK (char_length(note) BETWEEN 1 AND 4000),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX work_item_handoffs_recent
    ON work_item_handoffs (project_id, work_item_id, created_at DESC);
