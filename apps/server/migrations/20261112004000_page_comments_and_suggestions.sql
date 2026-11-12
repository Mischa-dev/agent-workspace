-- AWD-53 (criteria 6 and 7): suggestions and comments on pages.
--
-- project_page_suggestions: an agent's edits to a page someone else owns
-- wait here for the owner (or anyone who may edit the page) to accept or
-- reject them; the agent may withdraw them. `edits` holds block edits by
-- lasting block id, each with the hash of the block it read, so accepting
-- one never undoes a change the agent hadn't seen: a block that changed
-- since makes the suggestion refuse to apply. `before` keeps each changed
-- block's Markdown as the agent saw it, to show what it changes.
CREATE TABLE project_page_suggestions (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL,
    page_id UUID NOT NULL,
    author_actor_id UUID NOT NULL REFERENCES actors(id),
    note TEXT CHECK (note IS NULL OR char_length(note) <= 500),
    base_version BIGINT NOT NULL CHECK (base_version > 0),
    edits JSONB NOT NULL CHECK (
        jsonb_typeof(edits) = 'array'
        AND jsonb_array_length(edits) BETWEEN 1 AND 100
        AND octet_length(edits::text) <= 1048576
    ),
    status TEXT NOT NULL DEFAULT 'pending' CHECK (
        status IN ('pending', 'accepted', 'rejected', 'withdrawn')
    ),
    decided_by_actor_id UUID REFERENCES actors(id),
    decided_at TIMESTAMPTZ,
    -- The page's version the accepted edits made.
    applied_version BIGINT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (project_id, page_id) REFERENCES project_pages(project_id, id),
    CHECK ((status = 'pending') = (decided_at IS NULL)),
    CHECK (applied_version IS NULL OR status = 'accepted')
);

CREATE INDEX project_page_suggestions_page
    ON project_page_suggestions (page_id, created_at DESC);
CREATE INDEX project_page_suggestions_pending
    ON project_page_suggestions (page_id) WHERE status = 'pending';

-- project_page_comments: threads on a page. A thread's first comment has
-- thread_id = id and may be anchored to a top-level block by its lasting id
-- and the words it's about (the quote); replies carry the thread's id and no
-- anchor. Resolving is the thread's: it's kept on the first comment. A
-- deleted comment keeps its place (a thread's start, if it has replies) but
-- loses its words.
CREATE TABLE project_page_comments (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL,
    page_id UUID NOT NULL,
    thread_id UUID NOT NULL,
    author_actor_id UUID NOT NULL REFERENCES actors(id),
    via_oauth_grant_id UUID REFERENCES mcp_oauth_grants(id),
    body TEXT NOT NULL,
    block_id TEXT CHECK (
        block_id IS NULL OR (char_length(block_id) BETWEEN 1 AND 64 AND block_id ~ '^[A-Za-z0-9_-]+$')
    ),
    quote TEXT CHECK (quote IS NULL OR char_length(quote) BETWEEN 1 AND 1000),
    resolved_at TIMESTAMPTZ,
    resolved_by_actor_id UUID REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    edited_at TIMESTAMPTZ,
    deleted_at TIMESTAMPTZ,
    FOREIGN KEY (project_id, page_id) REFERENCES project_pages(project_id, id),
    FOREIGN KEY (thread_id) REFERENCES project_page_comments(id),
    CHECK (
        (deleted_at IS NULL AND char_length(body) BETWEEN 1 AND 10000)
        OR (deleted_at IS NOT NULL AND body = '')
    ),
    CHECK (quote IS NULL OR block_id IS NOT NULL),
    CHECK (thread_id = id OR (block_id IS NULL AND resolved_at IS NULL)),
    CHECK ((resolved_at IS NULL) = (resolved_by_actor_id IS NULL))
);

CREATE INDEX project_page_comments_page
    ON project_page_comments (page_id, created_at);
CREATE INDEX project_page_comments_thread
    ON project_page_comments (thread_id, created_at);
