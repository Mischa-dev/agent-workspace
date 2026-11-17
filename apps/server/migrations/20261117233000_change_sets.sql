-- C1.9 (lane D2, documents): change sets, so every write to a document says
-- what it touched and can be undone in one step.
--
-- * `document_change_sets`: one row per write (a person's typing in the
--   page editor is grouped per minute), naming the items it touched by
--   lasting id (`{"blocks": [...]}` for a page) and, in `inverse`, what it
--   replaced, so undo can put back the items nobody has changed since. An
--   undo is a change set of its own (`undoes_change_set_id`), so undo is
--   undoable; the original is marked `undone_by_change_set_id`.
-- * `proposals.applied_change_set_id` now points at the change set that
--   accepting a proposal made.

CREATE TABLE document_change_sets (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    space_id UUID NOT NULL REFERENCES projects(id),
    thing_kind TEXT NOT NULL CHECK (thing_kind ~ '^[a-z][a-z_]{1,31}$'),
    thing_id UUID NOT NULL,
    actor_id UUID NOT NULL REFERENCES actors(id),
    via_oauth_grant_id UUID REFERENCES mcp_oauth_grants(id) DEFAULT current_via_grant(),
    requested_by_actor_id UUID REFERENCES actors(id),
    run_id UUID,
    version_from BIGINT NOT NULL,
    version_to BIGINT NOT NULL CHECK (version_to > version_from),
    touched JSONB NOT NULL CHECK (
        jsonb_typeof(touched) = 'object' AND octet_length(touched::text) <= 262144
    ),
    -- NULL when what it replaced was too large to keep: undo then offers a
    -- restore of the version before it instead.
    inverse JSONB CHECK (inverse IS NULL OR octet_length(inverse::text) <= 1048576),
    -- Typing in the editor joins the writer's last change set until then.
    open_until TIMESTAMPTZ,
    proposal_id UUID REFERENCES proposals(id),
    undoes_change_set_id UUID REFERENCES document_change_sets(id),
    undone_by_change_set_id UUID REFERENCES document_change_sets(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX document_change_sets_thing
    ON document_change_sets (thing_kind, thing_id, created_at DESC);
CREATE INDEX document_change_sets_actor ON document_change_sets (actor_id, created_at DESC);
SELECT tenancy_protect('document_change_sets', 'workspace_id');

ALTER TABLE proposals
    ADD CONSTRAINT proposals_applied_change_set_fkey
    FOREIGN KEY (applied_change_set_id) REFERENCES document_change_sets(id);
