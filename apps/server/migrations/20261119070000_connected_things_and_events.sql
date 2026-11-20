-- M1.13: a connected module's kinds, things and the events it sends in.
--
-- * thing_kinds takes a qualified kind (`acme.build`, ruling R54) when its
--   module is `connected`. The row is the server's (a kind name is global,
--   as every kind's is); which install may write it is per workspace.
-- * module_kinds: the install that owns a qualified kind in a workspace.
--   Only it registers, writes or sends events about things of that kind
--   there, so another install can't pass itself off as it.
-- * module_things: what a connected thing has beyond its `things`
--   envelope (title, status, space): its address in the app and its
--   fields. The thing itself is a `things` row the app writes through
--   `thing_upsert`, so links, events and access work on it as on any.
-- * module_event_receipts: the `webhook-id` of each event an install sent
--   in the last few minutes, so a replayed one is refused. Older ones go:
--   an event signed more than five minutes ago is refused anyway.
-- * A live connected app reaches the spaces it is a member of, and a rule
--   may fire on its events.

ALTER TABLE thing_kinds DROP CONSTRAINT thing_kinds_kind_check;
ALTER TABLE thing_kinds ADD CONSTRAINT thing_kinds_kind_check
    CHECK ((position('.' IN kind) = 0) <> (module = 'connected'));

CREATE TABLE module_kinds (
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    kind thing_kind_name NOT NULL REFERENCES thing_kinds(kind),
    module_id UUID NOT NULL REFERENCES modules(id) ON DELETE CASCADE,
    one TEXT NOT NULL CHECK (char_length(one) BETWEEN 1 AND 40),
    many TEXT NOT NULL CHECK (char_length(many) BETWEEN 1 AND 40),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, kind),
    CHECK (position('.' IN kind) > 0)
);
CREATE INDEX module_kinds_module ON module_kinds (module_id);
SELECT tenancy_protect('module_kinds', 'workspace_id');

CREATE TABLE module_things (
    thing_id UUID PRIMARY KEY REFERENCES things(id) ON DELETE CASCADE,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    module_id UUID NOT NULL REFERENCES modules(id) ON DELETE CASCADE,
    external_url TEXT CHECK (external_url IS NULL OR char_length(external_url) <= 2000),
    fields JSONB NOT NULL DEFAULT '{}'::jsonb
        CHECK (jsonb_typeof(fields) = 'object' AND octet_length(fields::text) <= 16384),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX module_things_module ON module_things (module_id);
CREATE INDEX module_things_workspace ON module_things (workspace_id);
SELECT tenancy_protect('module_things', 'workspace_id');

CREATE TABLE module_event_receipts (
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    module_id UUID NOT NULL REFERENCES modules(id) ON DELETE CASCADE,
    webhook_id TEXT NOT NULL CHECK (char_length(webhook_id) BETWEEN 1 AND 200),
    received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (module_id, webhook_id)
);
CREATE INDEX module_event_receipts_received ON module_event_receipts (module_id, received_at);
SELECT tenancy_protect('module_event_receipts', 'workspace_id');

-- A connected module's app account signs in with its own secret, not as a
-- user: it reaches the spaces it is a member of while its install is live,
-- as a person does while their sign-in is active (rule 2: accounts differ
-- only in how they sign in). Otherwise unchanged from 0042.
CREATE OR REPLACE FUNCTION actor_can_access_project(viewer UUID, connection UUID, target UUID)
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
            WHERE project.id = target AND project.archived_at IS NULL
              AND (EXISTS (SELECT 1 FROM users user_account
                           WHERE user_account.actor_id = viewer
                             AND user_account.status = 'active')
                   OR EXISTS (SELECT 1 FROM module_apps app
                              WHERE app.account_id = viewer
                                AND app.workspace_id = project.organization_id
                                AND app.state = 'active'))
        )
    END
$$;

-- A rule may fire on a connected module's event.
ALTER TABLE automation_rules DROP CONSTRAINT automation_rules_trigger_kind_check;
ALTER TABLE automation_rules ADD CONSTRAINT automation_rules_trigger_kind_check CHECK (trigger_kind IN (
    'task_created', 'task_matches', 'run_finished', 'pull_request_checks_failed',
    'message_posted', 'record_created', 'record_changed', 'page_changed', 'sheet_changed',
    'whiteboard_changed', 'module_event', 'incoming_hook', 'schedule', 'capacity_window'
));
