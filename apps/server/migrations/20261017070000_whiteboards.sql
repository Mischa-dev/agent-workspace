-- Whiteboards (W16, task AWD-15): Excalidraw scenes the server owns, so
-- people and agents edit the same stored state and no browser has to be open.
--
-- A whiteboard belongs to a workspace (organization). With no project it is
-- for everyone in the workspace; with a project only that project's members
-- see it. Links to tasks, projects and pages are separate rows, so one board
-- can show up in many places, and unlinking or archiving any of them never
-- touches the board.
--
-- The scene is stored one row per Excalidraw element, with the element's own
-- version and versionNonce. Writes merge per element the way Excalidraw's
-- reconcile does, so two people editing different shapes never overwrite each
-- other, and `seq` (the board's change counter at the element's last change)
-- lets a client fetch only what changed since it last looked.

CREATE TABLE whiteboards (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id),
    project_id UUID,
    title TEXT NOT NULL CHECK (
        char_length(title) BETWEEN 1 AND 200 AND title !~ '[[:cntrl:]]'
    ),
    -- Bumped on every scene change.
    scene_seq BIGINT NOT NULL DEFAULT 0 CHECK (scene_seq >= 0),
    -- Bumped on every change to the board itself (title, who sees it, trash).
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    element_count INTEGER NOT NULL DEFAULT 0 CHECK (element_count >= 0),
    file_bytes BIGINT NOT NULL DEFAULT 0 CHECK (file_bytes >= 0),
    -- The board's words (text and labels), for search.
    text_content TEXT NOT NULL DEFAULT '' CHECK (char_length(text_content) <= 20000),
    duplicated_from_id UUID REFERENCES whiteboards(id),
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    updated_by_actor_id UUID REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    scene_updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    deleted_by_actor_id UUID REFERENCES actors(id),
    search_vector TSVECTOR GENERATED ALWAYS AS (
        setweight(to_tsvector('simple', title), 'A')
        || setweight(to_tsvector('simple', text_content), 'B')
    ) STORED,
    -- A board's project is always in the board's own workspace.
    FOREIGN KEY (organization_id, project_id) REFERENCES projects(organization_id, id),
    CHECK ((deleted_at IS NULL) = (deleted_by_actor_id IS NULL))
);

CREATE INDEX whiteboards_recent
    ON whiteboards (organization_id, updated_at DESC, id)
    WHERE deleted_at IS NULL;
CREATE INDEX whiteboards_project ON whiteboards (project_id) WHERE project_id IS NOT NULL;
CREATE INDEX whiteboards_search ON whiteboards USING GIN (search_vector);

CREATE TABLE whiteboard_elements (
    whiteboard_id UUID NOT NULL REFERENCES whiteboards(id) ON DELETE CASCADE,
    element_id TEXT NOT NULL CHECK (element_id ~ '^[A-Za-z0-9_-]{1,100}$'),
    element_type TEXT NOT NULL CHECK (element_type ~ '^[a-z][a-z0-9_]{0,31}$'),
    version INTEGER NOT NULL CHECK (version > 0),
    version_nonce BIGINT NOT NULL,
    is_deleted BOOLEAN NOT NULL DEFAULT false,
    -- Excalidraw's fractional index: the element's place in the stacking order.
    sort_index TEXT CHECK (sort_index IS NULL OR char_length(sort_index) <= 100),
    data JSONB NOT NULL CHECK (
        jsonb_typeof(data) = 'object' AND octet_length(data::text) <= 262144
    ),
    seq BIGINT NOT NULL CHECK (seq > 0),
    updated_by_actor_id UUID REFERENCES actors(id),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (whiteboard_id, element_id)
);

CREATE INDEX whiteboard_elements_changes ON whiteboard_elements (whiteboard_id, seq);

-- Images placed on a board. Excalidraw names each file by its content hash.
CREATE TABLE whiteboard_files (
    whiteboard_id UUID NOT NULL REFERENCES whiteboards(id) ON DELETE CASCADE,
    file_id TEXT NOT NULL CHECK (file_id ~ '^[A-Za-z0-9_-]{1,100}$'),
    mime_type TEXT NOT NULL CHECK (
        mime_type IN ('image/png', 'image/jpeg', 'image/gif', 'image/webp', 'image/svg+xml')
    ),
    data BYTEA NOT NULL CHECK (octet_length(data) BETWEEN 1 AND 4194304),
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (whiteboard_id, file_id)
);

CREATE TABLE whiteboard_links (
    whiteboard_id UUID NOT NULL REFERENCES whiteboards(id) ON DELETE CASCADE,
    target_kind TEXT NOT NULL CHECK (target_kind IN ('project', 'work_item', 'page')),
    target_id UUID NOT NULL,
    -- The project the target is in (the project itself for a project link),
    -- so reads can check the viewer's membership there.
    target_project_id UUID NOT NULL REFERENCES projects(id),
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (whiteboard_id, target_kind, target_id)
);

CREATE INDEX whiteboard_links_target ON whiteboard_links (target_kind, target_id);

-- Saved copies of the whole scene, so any change can be undone: taken every
-- few minutes while a board is edited, before every restore, and before an
-- agent removes many elements at once.
CREATE TABLE whiteboard_revisions (
    whiteboard_id UUID NOT NULL REFERENCES whiteboards(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL CHECK (revision > 0),
    scene_seq BIGINT NOT NULL CHECK (scene_seq >= 0),
    elements JSONB NOT NULL CHECK (jsonb_typeof(elements) = 'array'),
    element_count INTEGER NOT NULL CHECK (element_count >= 0),
    actor_id UUID REFERENCES actors(id),
    via_oauth_grant_id UUID REFERENCES mcp_oauth_grants(id),
    note TEXT CHECK (note IS NULL OR char_length(note) <= 500),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (whiteboard_id, revision)
);

-- Who can see a board: an active, enabled member of its workspace, and for a
-- project's board an enabled member of that (unarchived) project. Every
-- whiteboard query goes through this, like conversation_visible_to.
CREATE FUNCTION whiteboard_visible_to(target_whiteboard_id UUID, viewer_actor_id UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT EXISTS (
        SELECT 1
        FROM whiteboards board
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = board.organization_id
         AND organization_membership.actor_id = viewer_actor_id
         AND organization_membership.enabled
        JOIN actors actor ON actor.id = viewer_actor_id AND actor.status = 'active'
        WHERE board.id = target_whiteboard_id
          AND (
              board.project_id IS NULL
              OR EXISTS (
                  SELECT 1
                  FROM project_memberships membership
                  JOIN projects project ON project.id = membership.project_id
                  WHERE membership.project_id = board.project_id
                    AND membership.actor_id = viewer_actor_id
                    AND membership.enabled
                    AND project.archived_at IS NULL
              )
          )
    )
$$;

-- Board changes reach open browsers through the outbox like every other
-- change. Realtime checks whiteboard_visible_to for these events.
ALTER TABLE outbox_events
    DROP CONSTRAINT outbox_events_resource_kind_check,
    ADD CONSTRAINT outbox_events_resource_kind_check CHECK (
        resource_kind IN (
            'project', 'conversation', 'message', 'read_state', 'agent_run',
            'work_item', 'notification', 'whiteboard'
        )
    );
