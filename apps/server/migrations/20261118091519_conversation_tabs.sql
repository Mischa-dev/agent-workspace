-- C1.26: a page as a tab on a channel. One row per tab, in the order shown;
-- K3's link store mirrors each as conversation -embeds-> page, so the page's
-- backlinks say it's a tab in the channel. Only channels have tabs: a DM or
-- group lives in the direct scope, where nothing but conversations lives.

CREATE TABLE conversation_tabs (
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    space_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    thing_kind TEXT NOT NULL CHECK (thing_kind IN ('page')),
    thing_id UUID NOT NULL,
    position INTEGER NOT NULL DEFAULT 0 CHECK (position BETWEEN 0 AND 99),
    added_by_account_id UUID REFERENCES actors(id) ON DELETE SET NULL,
    added_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (conversation_id, thing_kind, thing_id)
);

CREATE INDEX conversation_tabs_by_thing ON conversation_tabs (thing_id);
CREATE INDEX conversation_tabs_by_workspace ON conversation_tabs (workspace_id);
SELECT tenancy_protect('conversation_tabs', 'workspace_id', 'conversation_id');

CREATE FUNCTION link_of(r conversation_tabs) RETURNS link_of_row
LANGUAGE sql IMMUTABLE AS $$
    SELECT ROW(r.conversation_id, r.thing_id, 'embeds', 'derived',
               NULL, NULL, r.added_by_account_id)::link_of_row
$$;

CREATE TRIGGER links_mirror AFTER INSERT OR DELETE ON conversation_tabs
    FOR EACH ROW EXECUTE FUNCTION links_mirror();
