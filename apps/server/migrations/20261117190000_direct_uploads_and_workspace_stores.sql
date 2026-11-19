-- K5.20: a workspace's own bucket, and uploads that go straight to a store.

-- A workspace's own bucket's store id is `ws-` and the bucket row's id, so
-- store ids grow past 32 characters.
ALTER TABLE files DROP CONSTRAINT files_store_id_check;
ALTER TABLE files ADD CONSTRAINT files_store_id_check
    CHECK (store_id ~ '^[a-z][a-z0-9_-]{0,47}$');
ALTER TABLE files DROP CONSTRAINT files_moved_from_store_id_check;
ALTER TABLE files ADD CONSTRAINT files_moved_from_store_id_check
    CHECK (moved_from_store_id ~ '^[a-z][a-z0-9_-]{0,47}$');
ALTER TABLE file_objects_pending DROP CONSTRAINT file_objects_pending_store_id_check;
ALTER TABLE file_objects_pending ADD CONSTRAINT file_objects_pending_store_id_check
    CHECK (store_id ~ '^[a-z][a-z0-9_-]{0,47}$');

-- A workspace's own S3-compatible bucket. Its access keys are a secret in
-- the keyring (`secret_id`), never in this row. Setting another bucket
-- retires this one rather than changing it: files already in it name it,
-- and are read from it until they are moved.
CREATE TABLE workspace_stores (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    endpoint TEXT NOT NULL CHECK (endpoint ~ '^https?://' AND length(endpoint) <= 300),
    bucket TEXT NOT NULL CHECK (bucket ~ '^[a-z0-9][a-z0-9.-]{1,62}$'),
    region TEXT NOT NULL CHECK (region ~ '^[a-z0-9-]{1,40}$'),
    prefix TEXT NOT NULL DEFAULT '' CHECK (length(prefix) <= 200),
    path_style BOOLEAN NOT NULL,
    secret_id UUID NOT NULL REFERENCES secrets(id),
    verified_at TIMESTAMPTZ NOT NULL,
    set_by_actor_id UUID REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    retired_at TIMESTAMPTZ
);
CREATE UNIQUE INDEX workspace_stores_active ON workspace_stores (workspace_id)
    WHERE retired_at IS NULL;
SELECT tenancy_protect('workspace_stores', 'workspace_id');

-- A direct upload between its start (a signed link handed out) and its
-- finish (the bytes checked and the file recorded). The bytes sit under
-- `storage_key`, which nothing serves until the finish makes a file of
-- them. One never finished is swept with its bytes after a day.
CREATE TABLE file_uploads (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    space_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    conversation_id UUID,
    work_item_id UUID,
    for_comment BOOLEAN NOT NULL DEFAULT false,
    uploaded_by_actor_id UUID NOT NULL REFERENCES actors(id),
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 255),
    content_type TEXT NOT NULL CHECK (length(content_type) <= 255),
    byte_size BIGINT NOT NULL CHECK (byte_size > 0),
    sha256 TEXT NOT NULL CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    storage_key TEXT NOT NULL UNIQUE CHECK (storage_key ~ '^[0-9a-f/.a-z-]{1,200}$'),
    store_id TEXT NOT NULL CHECK (store_id ~ '^[a-z][a-z0-9_-]{0,47}$'),
    file_id UUID REFERENCES files(id) ON DELETE SET NULL,
    finished_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (space_id, conversation_id) REFERENCES conversations(project_id, id) ON DELETE CASCADE,
    FOREIGN KEY (space_id, work_item_id) REFERENCES work_items(project_id, id) ON DELETE CASCADE,
    CHECK ((conversation_id IS NULL) <> (work_item_id IS NULL)),
    CHECK (NOT for_comment OR work_item_id IS NOT NULL)
);
CREATE INDEX file_uploads_unfinished ON file_uploads (created_at) WHERE finished_at IS NULL;
SELECT tenancy_protect('file_uploads', 'workspace_id');
