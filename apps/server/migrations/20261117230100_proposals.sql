-- C1.7 (lane D2, documents): one proposals record for every kind of thing,
-- and page suggestions moved onto it.
--
-- * `proposals`: a change someone whose role only suggests on a thing (or
--   who asked to propose) made to it, waiting for someone who edits the
--   thing to accept or reject it; the author may withdraw it. `change` is
--   the kind's own shape and says what each part read, so accepting can
--   never undo a change the author hadn't seen. Pages keep today's block
--   edits: {"edits": [...]}. `applied_change_set_id` (C1.9's change sets),
--   `request_id` (A1's inbox card for it) and `thread_id` (its review
--   discussion) are filled by the slices that make them.
-- * Every `project_page_suggestions` row moves over with the same id
--   (`thing_kind = 'page'`, `operation = 'page_edit'`), and the old table
--   goes: there is one deploy, so nothing reads it after this.

CREATE TABLE proposals (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    space_id UUID NOT NULL REFERENCES projects(id),
    thing_kind TEXT NOT NULL CHECK (thing_kind ~ '^[a-z][a-z_]{1,31}$'),
    thing_id UUID NOT NULL,
    -- The operation the change would have run, by name.
    operation TEXT NOT NULL CHECK (operation ~ '^[a-z][a-z0-9_]{0,63}$'),
    author_actor_id UUID NOT NULL REFERENCES actors(id),
    via_oauth_grant_id UUID REFERENCES mcp_oauth_grants(id) DEFAULT current_via_grant(),
    requested_by_actor_id UUID REFERENCES actors(id),
    run_id UUID,
    base_version BIGINT NOT NULL CHECK (base_version > 0),
    change JSONB NOT NULL CHECK (
        jsonb_typeof(change) = 'object' AND octet_length(change::text) <= 1048576
    ),
    preview JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(preview) = 'object' AND octet_length(preview::text) <= 1048576
    ),
    note TEXT CHECK (note IS NULL OR char_length(note) <= 500),
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN (
        'pending', 'accepted', 'partly_accepted', 'rejected', 'withdrawn', 'stale'
    )),
    decided_by_actor_id UUID REFERENCES actors(id),
    decided_at TIMESTAMPTZ,
    decision_note TEXT CHECK (decision_note IS NULL OR char_length(decision_note) <= 1000),
    applied_version BIGINT,
    applied_change_set_id UUID,
    request_id UUID,
    thread_id UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((status = 'pending') = (decided_at IS NULL)),
    CHECK (applied_version IS NULL OR status IN ('accepted', 'partly_accepted'))
);
CREATE INDEX proposals_thing ON proposals (thing_kind, thing_id, created_at DESC);
CREATE INDEX proposals_thing_pending ON proposals (thing_kind, thing_id) WHERE status = 'pending';
CREATE INDEX proposals_author ON proposals (author_actor_id, created_at DESC);
CREATE INDEX proposals_workspace_pending
    ON proposals (workspace_id, created_at DESC) WHERE status = 'pending';
SELECT tenancy_protect('proposals', 'workspace_id');

INSERT INTO proposals (
    id, workspace_id, space_id, thing_kind, thing_id, operation, author_actor_id,
    via_oauth_grant_id, base_version, change, note, status, decided_by_actor_id,
    decided_at, applied_version, created_at, updated_at
)
SELECT suggestion.id, suggestion.workspace_id, suggestion.project_id, 'page', suggestion.page_id,
       'page_edit', suggestion.author_actor_id, NULL, suggestion.base_version,
       jsonb_build_object('edits', suggestion.edits), suggestion.note, suggestion.status,
       suggestion.decided_by_actor_id, suggestion.decided_at, suggestion.applied_version,
       suggestion.created_at, COALESCE(suggestion.decided_at, suggestion.created_at)
FROM project_page_suggestions suggestion;

DROP TABLE project_page_suggestions;
