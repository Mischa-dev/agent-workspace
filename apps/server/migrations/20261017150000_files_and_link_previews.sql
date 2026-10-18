-- Files and link previews (W20).
--
-- A file is uploaded into one place: a conversation (for a message) or a task
-- (for the task itself or one of its comments). Until the message or comment
-- it was uploaded for is sent, only its uploader sees it (attached_at is
-- NULL); unsent uploads are cleaned up after a day. Task files uploaded to
-- the task itself are attached straight away.
--
-- Reading a file always checks the place it belongs to: a message's file
-- needs `conversation_visible_to` and the message not removed, a task's file
-- needs the project. The bytes live in the file store (local disk under the
-- data root today, an object store later) under `storage_key`; the database
-- never holds them. Removed files keep their row, so their bytes can be
-- deleted afterwards (blob_deleted_at).

CREATE TABLE files (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id),
    project_id UUID NOT NULL,
    conversation_id UUID,
    work_item_id UUID,
    message_id UUID,
    comment_id UUID REFERENCES work_item_comments(id),
    uploaded_by_actor_id UUID NOT NULL REFERENCES actors(id),
    name TEXT NOT NULL CHECK (
        char_length(name) BETWEEN 1 AND 255 AND name !~ '[[:cntrl:]/\\]'
    ),
    content_type TEXT NOT NULL CHECK (content_type ~ '^[a-z0-9][a-z0-9!#$&^_.+-]{0,63}/[a-z0-9][a-z0-9!#$&^_.+-]{0,63}$'),
    byte_size BIGINT NOT NULL CHECK (byte_size >= 0),
    sha256 TEXT NOT NULL CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    storage_key TEXT NOT NULL UNIQUE CHECK (storage_key ~ '^[0-9a-f/-]{1,200}$'),
    -- Set for images shown inline (PNG, JPEG, GIF, WebP).
    image_width INTEGER CHECK (image_width > 0),
    image_height INTEGER CHECK (image_height > 0),
    thumbnail_key TEXT UNIQUE CHECK (thumbnail_key ~ '^[0-9a-f/.a-z-]{1,200}$'),
    thumbnail_bytes BIGINT CHECK (thumbnail_bytes >= 0),
    position SMALLINT NOT NULL DEFAULT 0 CHECK (position BETWEEN 0 AND 99),
    attached_at TIMESTAMPTZ,
    removed_at TIMESTAMPTZ,
    removed_by_actor_id UUID REFERENCES actors(id),
    blob_deleted_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    search_vector TSVECTOR GENERATED ALWAYS AS (
        to_tsvector('simple', regexp_replace(name, '[._-]+', ' ', 'g'))
    ) STORED,
    FOREIGN KEY (organization_id, project_id) REFERENCES projects(organization_id, id),
    FOREIGN KEY (project_id, conversation_id) REFERENCES conversations(project_id, id),
    FOREIGN KEY (project_id, work_item_id) REFERENCES work_items(project_id, id),
    FOREIGN KEY (project_id, message_id) REFERENCES messages(project_id, id),
    -- Uploaded for exactly one place.
    CHECK ((conversation_id IS NULL) <> (work_item_id IS NULL)),
    CHECK (message_id IS NULL OR conversation_id IS NOT NULL),
    CHECK (comment_id IS NULL OR work_item_id IS NOT NULL),
    -- A message's files are attached when it is sent, never before.
    CHECK (conversation_id IS NULL OR (message_id IS NULL) = (attached_at IS NULL)),
    CHECK (comment_id IS NULL OR attached_at IS NOT NULL),
    CHECK ((image_width IS NULL) = (image_height IS NULL)),
    CHECK ((thumbnail_key IS NULL) = (thumbnail_bytes IS NULL)),
    CHECK ((removed_at IS NULL) = (removed_by_actor_id IS NULL))
);

CREATE INDEX files_message ON files (message_id, position) WHERE message_id IS NOT NULL;
CREATE INDEX files_work_item ON files (project_id, work_item_id, created_at)
    WHERE work_item_id IS NOT NULL;
CREATE INDEX files_unattached ON files (created_at) WHERE attached_at IS NULL;
CREATE INDEX files_blobs_to_delete ON files (removed_at)
    WHERE removed_at IS NOT NULL AND blob_deleted_at IS NULL;
CREATE INDEX files_workspace_bytes ON files (organization_id) WHERE blob_deleted_at IS NULL;
CREATE INDEX files_search ON files USING gin (search_vector);

-- Link previews: title, description and image of a web page (Open Graph),
-- fetched once per URL per workspace by a background job and shared by every
-- message in the workspace that links to it.
CREATE TABLE link_previews (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id),
    url TEXT NOT NULL CHECK (char_length(url) BETWEEN 8 AND 2048),
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'ready', 'failed')),
    title TEXT CHECK (char_length(title) <= 300),
    description TEXT CHECK (char_length(description) <= 600),
    site_name TEXT CHECK (char_length(site_name) <= 100),
    -- The page's image, fetched and stored like a file's thumbnail.
    image_key TEXT UNIQUE CHECK (image_key ~ '^[0-9a-f/.a-z-]{1,200}$'),
    image_bytes BIGINT CHECK (image_bytes >= 0),
    image_width INTEGER CHECK (image_width > 0),
    image_height INTEGER CHECK (image_height > 0),
    image_type TEXT CHECK (image_type IN ('image/png', 'image/jpeg', 'image/gif', 'image/webp')),
    failure TEXT CHECK (char_length(failure) <= 200),
    fetched_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (organization_id, url),
    CHECK ((image_key IS NULL) = (image_bytes IS NULL)),
    CHECK ((image_key IS NULL) = (image_type IS NULL))
);

-- Which previews a message shows, in the order its links appear. The author
-- (or a moderator) can hide one; hidden previews stay hidden.
CREATE TABLE message_link_previews (
    project_id UUID NOT NULL,
    message_id UUID NOT NULL,
    link_preview_id UUID NOT NULL REFERENCES link_previews(id),
    position SMALLINT NOT NULL CHECK (position BETWEEN 0 AND 9),
    hidden_at TIMESTAMPTZ,
    hidden_by_actor_id UUID REFERENCES actors(id),
    PRIMARY KEY (message_id, link_preview_id),
    FOREIGN KEY (project_id, message_id) REFERENCES messages(project_id, id),
    CHECK ((hidden_at IS NULL) = (hidden_by_actor_id IS NULL))
);

CREATE INDEX message_link_previews_preview ON message_link_previews (link_preview_id);
