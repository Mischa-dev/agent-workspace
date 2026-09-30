-- How people and agents work here through MCP (docs/plans/agent-interface.md).
--
-- 1. Connected apps act as the person across their whole account. The grant
--    keeps a home Project (the person's Messages space) only as an anchor; each
--    call is checked against the person's current memberships.
-- 2. The app a person acted through is recorded on what it writes, and each
--    connected app has a toggle for showing "via <app>".
-- 3. Threads can live under DMs and groups, and every thread has a status and
--    an optional outcome.
-- 4. Full-text search over messages, tasks, pages and decisions.
-- 5. Project memory: pages with revision history, decisions, and pins.
-- 6. Task claims (with leases) and links from tasks to messages, pages,
--    decisions and conversations.

-- 1 and 2: connected apps --------------------------------------------------

ALTER TABLE mcp_oauth_authorization_codes
    ADD COLUMN show_via BOOLEAN NOT NULL DEFAULT true;

ALTER TABLE mcp_oauth_grants
    ADD COLUMN show_via BOOLEAN NOT NULL DEFAULT true,
    ADD COLUMN account_wide BOOLEAN NOT NULL DEFAULT false;

-- The connected app a person is acting through in this transaction, if any.
-- MCP write paths set it with set_config('aw.via_grant', ..., true), so rows
-- written deep inside shared command code are attributed without threading a
-- parameter through every call.
CREATE FUNCTION current_via_grant()
RETURNS UUID
LANGUAGE sql
STABLE
AS $$
    SELECT NULLIF(current_setting('aw.via_grant', true), '')::uuid
$$;

ALTER TABLE messages
    ADD COLUMN via_oauth_grant_id UUID REFERENCES mcp_oauth_grants(id);

ALTER TABLE work_item_activity
    ADD COLUMN via_oauth_grant_id UUID REFERENCES mcp_oauth_grants(id)
        DEFAULT current_via_grant();

ALTER TABLE work_item_comments
    ADD COLUMN via_oauth_grant_id UUID REFERENCES mcp_oauth_grants(id)
        DEFAULT current_via_grant();

-- True when a person or agent may act in a Project right now. People need an
-- enabled membership in the Project and its Organization; agents also go
-- through their connection's rules.
CREATE FUNCTION actor_can_access_project(viewer UUID, connection UUID, target UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT CASE
        WHEN connection IS NOT NULL THEN agent_connection_can_access(connection, target)
        ELSE EXISTS (
            SELECT 1
            FROM projects project
            JOIN project_memberships membership
              ON membership.project_id = project.id
             AND membership.actor_id = viewer
             AND membership.enabled
            JOIN organization_memberships organization_membership
              ON organization_membership.organization_id = project.organization_id
             AND organization_membership.actor_id = viewer
             AND organization_membership.enabled
            JOIN actors actor ON actor.id = viewer AND actor.status = 'active'
            JOIN users user_account
              ON user_account.actor_id = viewer AND user_account.status = 'active'
            WHERE project.id = target AND project.archived_at IS NULL
        )
    END
$$;

-- 3: threads -----------------------------------------------------------------

ALTER TABLE conversations
    ADD COLUMN thread_status TEXT NOT NULL DEFAULT 'Open' CHECK (
        char_length(thread_status) BETWEEN 1 AND 40
        AND thread_status !~ '[[:cntrl:]]'
    ),
    ADD COLUMN thread_outcome TEXT CHECK (
        thread_outcome IS NULL
        OR (char_length(thread_outcome) BETWEEN 1 AND 1000
            AND thread_outcome !~ '[[:cntrl:]]')
    ),
    ADD COLUMN thread_status_changed_at TIMESTAMPTZ,
    ADD COLUMN thread_status_changed_by_actor_id UUID REFERENCES actors(id);

CREATE INDEX conversations_thread_status_changed
    ON conversations (project_id, thread_status_changed_at DESC)
    WHERE kind = 'thread' AND thread_status_changed_at IS NOT NULL;

CREATE INDEX conversations_threads_by_parent
    ON conversations (parent_channel_id, created_at DESC)
    WHERE kind = 'thread';

-- Same rules as 0037, except a thread's parent may also be a DM or a group.
-- A thread under a DM or group is visible only to that conversation's members,
-- because conversation_visible_to already checks the parent.
CREATE OR REPLACE FUNCTION validate_conversation_parent()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    parent_kind TEXT;
    parent_project_id UUID;
    parent_archived_at TIMESTAMPTZ;
    origin_conversation_id UUID;
    origin_project_id UUID;
BEGIN
    IF TG_OP = 'UPDATE' THEN
        IF NEW.kind IS DISTINCT FROM OLD.kind THEN
            RAISE EXCEPTION 'Conversation kind is immutable'
                USING ERRCODE = '23514';
        END IF;
        IF NEW.project_id IS DISTINCT FROM OLD.project_id THEN
            RAISE EXCEPTION 'Conversation Project is immutable'
                USING ERRCODE = '23514';
        END IF;
        IF NEW.parent_channel_id IS DISTINCT FROM OLD.parent_channel_id THEN
            RAISE EXCEPTION 'Thread parent Channel is immutable'
                USING ERRCODE = '23514';
        END IF;
        IF NEW.origin_message_id IS DISTINCT FROM OLD.origin_message_id THEN
            RAISE EXCEPTION 'Thread origin Message is immutable'
                USING ERRCODE = '23514';
        END IF;
    END IF;

    IF NEW.kind <> 'thread' THEN
        RETURN NEW;
    END IF;

    SELECT parent.kind, parent.project_id, parent.archived_at
      INTO parent_kind, parent_project_id, parent_archived_at
      FROM conversations parent
     WHERE parent.id = NEW.parent_channel_id
       FOR SHARE;

    IF parent_kind IS NULL
       OR parent_kind NOT IN ('channel', 'dm', 'group')
       OR parent_project_id IS DISTINCT FROM NEW.project_id THEN
        RAISE EXCEPTION 'Thread parent must be a Channel, DM, or group in the same Project'
            USING ERRCODE = '23514';
    END IF;
    IF parent_archived_at IS NOT NULL THEN
        RAISE EXCEPTION 'Thread parent Conversation is archived'
            USING ERRCODE = '23514';
    END IF;

    IF NEW.origin_message_id IS NOT NULL THEN
        SELECT message.conversation_id, message.project_id
          INTO origin_conversation_id, origin_project_id
          FROM messages message
         WHERE message.id = NEW.origin_message_id
           FOR SHARE;

        IF origin_conversation_id IS DISTINCT FROM NEW.parent_channel_id
           OR origin_project_id IS DISTINCT FROM NEW.project_id THEN
            RAISE EXCEPTION 'Thread origin Message must belong to its parent Conversation'
                USING ERRCODE = '23514';
        END IF;
    END IF;

    RETURN NEW;
END;
$$;

-- 4: search ------------------------------------------------------------------

ALTER TABLE messages
    ADD COLUMN search_vector TSVECTOR
        GENERATED ALWAYS AS (to_tsvector('english', plain_text)) STORED;

CREATE INDEX messages_search ON messages USING GIN (search_vector);

ALTER TABLE work_items
    ADD COLUMN search_vector TSVECTOR
        GENERATED ALWAYS AS (
            setweight(to_tsvector('english', title), 'A')
            || setweight(to_tsvector('english', description), 'B')
        ) STORED;

CREATE INDEX work_items_search ON work_items USING GIN (search_vector);

-- 5: project memory ----------------------------------------------------------

CREATE TABLE project_pages (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id),
    title TEXT NOT NULL CHECK (
        char_length(title) BETWEEN 1 AND 200 AND title !~ '[[:cntrl:]]'
    ),
    body TEXT NOT NULL DEFAULT '' CHECK (char_length(body) <= 200000),
    is_guide BOOLEAN NOT NULL DEFAULT false,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    created_by_actor_id UUID REFERENCES actors(id),
    updated_by_actor_id UUID REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    archived_at TIMESTAMPTZ,
    search_vector TSVECTOR GENERATED ALWAYS AS (
        setweight(to_tsvector('english', title), 'A')
        || setweight(to_tsvector('english', body), 'B')
    ) STORED,
    UNIQUE (project_id, id)
);

CREATE UNIQUE INDEX project_pages_one_guide
    ON project_pages (project_id) WHERE is_guide AND archived_at IS NULL;
CREATE UNIQUE INDEX project_pages_active_title
    ON project_pages (project_id, lower(title)) WHERE archived_at IS NULL;
CREATE INDEX project_pages_recent ON project_pages (project_id, updated_at DESC);
CREATE INDEX project_pages_search ON project_pages USING GIN (search_vector);

-- Every saved version of a page, so any edit can be undone.
CREATE TABLE project_page_revisions (
    page_id UUID NOT NULL REFERENCES project_pages(id),
    version BIGINT NOT NULL CHECK (version > 0),
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    actor_id UUID REFERENCES actors(id),
    via_oauth_grant_id UUID REFERENCES mcp_oauth_grants(id),
    note TEXT CHECK (note IS NULL OR char_length(note) <= 500),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (page_id, version)
);

-- Existing project instructions become each project's guide page.
INSERT INTO project_pages (
    id, project_id, title, body, is_guide, created_by_actor_id, updated_by_actor_id
)
SELECT gen_random_uuid(), instructions.project_id, 'Project guide', instructions.content, true,
       instructions.updated_by_actor_id, instructions.updated_by_actor_id
FROM project_instructions instructions
JOIN projects project ON project.id = instructions.project_id
WHERE project.kind <> 'direct' AND instructions.content <> '';

INSERT INTO project_page_revisions (page_id, version, title, body, actor_id, note)
SELECT page.id, 1, page.title, page.body, page.created_by_actor_id,
       'Moved from project instructions'
FROM project_pages page;

CREATE TABLE project_decisions (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id),
    title TEXT NOT NULL CHECK (
        char_length(title) BETWEEN 1 AND 300 AND title !~ '[[:cntrl:]]'
    ),
    detail TEXT NOT NULL DEFAULT '' CHECK (char_length(detail) <= 20000),
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'superseded')),
    superseded_by_decision_id UUID REFERENCES project_decisions(id),
    -- A decision taken in a private conversation stays as private as it.
    source_conversation_id UUID REFERENCES conversations(id),
    source_message_id UUID REFERENCES messages(id),
    decided_by_actor_id UUID NOT NULL REFERENCES actors(id),
    via_oauth_grant_id UUID REFERENCES mcp_oauth_grants(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    search_vector TSVECTOR GENERATED ALWAYS AS (
        setweight(to_tsvector('english', title), 'A')
        || setweight(to_tsvector('english', detail), 'B')
    ) STORED,
    CHECK ((status = 'superseded') = (superseded_by_decision_id IS NOT NULL)),
    CHECK (source_message_id IS NULL OR source_conversation_id IS NOT NULL),
    UNIQUE (project_id, id)
);

CREATE INDEX project_decisions_recent ON project_decisions (project_id, created_at DESC);
CREATE INDEX project_decisions_source ON project_decisions (source_conversation_id);
CREATE INDEX project_decisions_search ON project_decisions USING GIN (search_vector);

-- Pages, decisions and messages pinned to a conversation form its brief.
CREATE TABLE conversation_pins (
    conversation_id UUID NOT NULL REFERENCES conversations(id),
    target_kind TEXT NOT NULL CHECK (target_kind IN ('message', 'page', 'decision')),
    target_id UUID NOT NULL,
    pinned_by_actor_id UUID NOT NULL REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (conversation_id, target_kind, target_id)
);

-- 6: task claims and links ---------------------------------------------------

-- One claim per task. A claim lasts until it expires or is released; the same
-- actor claiming again renews it. Claims are advisory: they tell other agents
-- someone is working on the task.
CREATE TABLE work_item_claims (
    work_item_id UUID PRIMARY KEY REFERENCES work_items(id),
    project_id UUID NOT NULL REFERENCES projects(id),
    actor_id UUID NOT NULL REFERENCES actors(id),
    note TEXT CHECK (note IS NULL OR char_length(note) <= 500),
    claimed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    CHECK (expires_at > claimed_at)
);

CREATE INDEX work_item_claims_actor ON work_item_claims (actor_id, expires_at);

CREATE TABLE work_item_links (
    work_item_id UUID NOT NULL REFERENCES work_items(id),
    project_id UUID NOT NULL REFERENCES projects(id),
    target_kind TEXT NOT NULL CHECK (
        target_kind IN ('message', 'page', 'decision', 'conversation')
    ),
    target_id UUID NOT NULL,
    created_by_actor_id UUID NOT NULL REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (work_item_id, target_kind, target_id)
);

CREATE INDEX work_item_links_target ON work_item_links (target_kind, target_id);
