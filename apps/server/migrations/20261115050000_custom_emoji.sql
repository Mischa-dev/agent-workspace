-- C2.19: any emoji, and a workspace's own. A reaction is any standard
-- emoji from the shared list (src/reactions/emoji.json), checked in Rust,
-- so the fixed list in 0048's CHECK gives way to a shape check. A custom
-- emoji is a small image a workspace adds under a :name:; retiring one
-- takes it out of the picker and keeps it on the reactions that use it.

CREATE TABLE custom_emojis (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (name ~ '^[a-z0-9_]{2,32}$'),
    content_type TEXT NOT NULL CHECK (content_type IN ('image/png', 'image/gif', 'image/webp')),
    image BYTEA NOT NULL CHECK (octet_length(image) BETWEEN 1 AND 65536),
    created_by_account_id UUID NOT NULL REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    retired_at TIMESTAMPTZ,
    retired_by_account_id UUID REFERENCES actors(id)
);

-- One live emoji per name in a workspace; a retired name can be reused.
CREATE UNIQUE INDEX custom_emojis_live_name
    ON custom_emojis (workspace_id, name)
    WHERE retired_at IS NULL;

ALTER TABLE message_reactions DROP CONSTRAINT message_reactions_emoji_allowed;
ALTER TABLE message_reactions ADD CONSTRAINT message_reactions_emoji_shape CHECK (
    char_length(emoji) BETWEEN 1 AND 64 AND emoji !~ '[[:space:][:cntrl:]]'
);
ALTER TABLE message_reactions
    ADD COLUMN custom_emoji_id UUID REFERENCES custom_emojis(id);
