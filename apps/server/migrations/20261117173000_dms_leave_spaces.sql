-- K4.12 to K4.17, K4.23 and K4.24 (rulings R20, R31): DMs and groups leave
-- the hidden Messages projects. They belong to the people in them, not to a
-- space: a DM, a group, a thread under one and everything that hangs off
-- them carry no space (`project_id` NULL). New ones carry the direct scope
-- as their workspace; existing ones keep the workspace they were made in and
-- are personal (`kept = false`), with no data moved between workspaces.
--
-- In order:
-- 1. The direct scope: `organizations.kind`, one reserved row with a fixed
--    id (`spaces::DIRECT_SCOPE_ID`, SQL `direct_scope_id()`) that holds no
--    member, space or role, ever.
-- 2. The DM tables take a NULL space; read states are keyed by
--    conversation and account; one personal DM per pair in the direct scope.
-- 3. The triggers on those tables read a DM's workspace from its
--    conversation and its participants instead of a space membership.
-- 4. What a role allows in a DM (K4.12): `direct_allows` — some workspace the
--    account shares with every other participant gives it, through the
--    account's role there (a kept DM: its own workspace only). Starting one
--    needs `direct.start` in a workspace shared with each other person.
-- 5. The one-time move (K4.16): DM rows lose their space; what the hidden
--    projects held for the whole workspace (agent connections, secrets,
--    notices, Later saves, webhooks) moves to the workspace's own space;
--    their scaffolding goes; then the hidden projects themselves (K4.17).
--    Any row still pointing at one stops the migration, naming its table.

-- 1. The direct scope -------------------------------------------------------

ALTER TABLE organizations ADD COLUMN kind TEXT NOT NULL DEFAULT 'team'
    CHECK (kind IN ('team', 'direct'));

CREATE FUNCTION direct_scope_id() RETURNS UUID
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT '00000000-0000-7000-8000-00000000d1ec'::UUID
$$;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM organizations
        WHERE slug = 'aw-direct-scope' AND id <> direct_scope_id()
    ) THEN
        RAISE EXCEPTION 'A workspace already holds the slug aw-direct-scope; rename it first';
    END IF;
END
$$;

-- The row is made with the workspace triggers off: it gets no roles, no
-- built-in host, no workspace space and no slug history.
ALTER TABLE organizations DISABLE TRIGGER USER;
INSERT INTO organizations (id, kind, name, slug)
VALUES (direct_scope_id(), 'direct', 'Direct messages', 'aw-direct-scope')
ON CONFLICT (id) DO NOTHING;
ALTER TABLE organizations ENABLE TRIGGER USER;

CREATE UNIQUE INDEX organizations_one_direct ON organizations (kind) WHERE kind = 'direct';

ALTER TABLE organization_memberships ADD CONSTRAINT organization_memberships_not_direct
    CHECK (organization_id <> '00000000-0000-7000-8000-00000000d1ec');
ALTER TABLE projects ADD CONSTRAINT projects_not_direct
    CHECK (organization_id <> '00000000-0000-7000-8000-00000000d1ec');
ALTER TABLE workspace_roles ADD CONSTRAINT workspace_roles_not_direct
    CHECK (organization_id <> '00000000-0000-7000-8000-00000000d1ec');
-- Only DMs and groups (and threads under them) live there; a thread's
-- parent is checked by the parent trigger.
ALTER TABLE conversations ADD CONSTRAINT conversations_direct_scope_kinds
    CHECK (workspace_id <> '00000000-0000-7000-8000-00000000d1ec'
           OR kind IN ('dm', 'group', 'thread'));

-- 2. DM tables take a NULL space ---------------------------------------------

-- Read states are one per conversation and account, wherever it lives.
ALTER TABLE conversation_read_states DROP CONSTRAINT conversation_read_states_pkey;
ALTER TABLE conversation_read_states ADD PRIMARY KEY (conversation_id, actor_id);

ALTER TABLE conversations ALTER COLUMN project_id DROP NOT NULL;
ALTER TABLE conversation_members ALTER COLUMN project_id DROP NOT NULL;
ALTER TABLE conversation_read_states ALTER COLUMN project_id DROP NOT NULL;
ALTER TABLE conversation_notification_levels ALTER COLUMN project_id DROP NOT NULL;
ALTER TABLE messages ALTER COLUMN project_id DROP NOT NULL;
ALTER TABLE message_link_previews ALTER COLUMN project_id DROP NOT NULL;
ALTER TABLE message_mentions ALTER COLUMN project_id DROP NOT NULL;
ALTER TABLE message_reactions ALTER COLUMN project_id DROP NOT NULL;
ALTER TABLE message_revisions ALTER COLUMN project_id DROP NOT NULL;
ALTER TABLE message_work_item_references ALTER COLUMN project_id DROP NOT NULL;
ALTER TABLE reply_requests ALTER COLUMN project_id DROP NOT NULL;
ALTER TABLE files ALTER COLUMN project_id DROP NOT NULL;
ALTER TABLE scheduled_messages ALTER COLUMN project_id DROP NOT NULL;
ALTER TABLE saved_items ALTER COLUMN project_id DROP NOT NULL;
ALTER TABLE notifications ALTER COLUMN project_id DROP NOT NULL;
ALTER TABLE thing_references ALTER COLUMN project_id DROP NOT NULL;
ALTER TABLE agent_sessions ALTER COLUMN project_id DROP NOT NULL;
ALTER TABLE launches ALTER COLUMN project_id DROP NOT NULL;
ALTER TABLE external_execution_requests ALTER COLUMN project_id DROP NOT NULL;
ALTER TABLE message_drafts ALTER COLUMN space_id DROP NOT NULL;

-- One personal DM per pair on the server (R20); at most one kept DM per
-- pair in a workspace. Pre-R20 DMs (kept = false, a team workspace's id)
-- may repeat a pair across workspaces: they stay, labelled.
DROP INDEX conversations_dm_pair_unique;
CREATE UNIQUE INDEX conversations_dm_pair_personal ON conversations (dm_key)
    WHERE kind = 'dm' AND workspace_id = '00000000-0000-7000-8000-00000000d1ec';
CREATE UNIQUE INDEX conversations_dm_pair_kept ON conversations (workspace_id, dm_key)
    WHERE kind = 'dm' AND kept;
CREATE INDEX conversations_dm_pair ON conversations (dm_key) WHERE kind = 'dm';

-- 3. Triggers read a DM's workspace from its conversation --------------------

-- A row with no space takes its workspace from its conversation (or its
-- message's), so the tables' NOT NULL workspace column holds.
CREATE OR REPLACE FUNCTION fill_workspace_from_project()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    row_json JSONB;
BEGIN
    IF NEW.workspace_id IS NOT NULL THEN
        RETURN NEW;
    END IF;
    IF NEW.project_id IS NOT NULL THEN
        SELECT organization_id INTO NEW.workspace_id FROM projects WHERE id = NEW.project_id;
        RETURN NEW;
    END IF;
    row_json := to_jsonb(NEW);
    IF row_json ? 'conversation_id' AND row_json->>'conversation_id' IS NOT NULL THEN
        SELECT workspace_id INTO NEW.workspace_id
        FROM conversations WHERE id = (row_json->>'conversation_id')::UUID;
    ELSIF row_json ? 'message_id' AND row_json->>'message_id' IS NOT NULL THEN
        SELECT workspace_id INTO NEW.workspace_id
        FROM messages WHERE id = (row_json->>'message_id')::UUID;
    END IF;
    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION derive_message_project()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    conversation_project_id UUID;
    found_conversation BOOLEAN;
BEGIN
    SELECT project_id, true
      INTO conversation_project_id, found_conversation
      FROM conversations
     WHERE id = NEW.conversation_id
       FOR KEY SHARE;

    IF found_conversation IS NULL THEN
        RAISE EXCEPTION 'Message Conversation does not exist'
            USING ERRCODE = '23503';
    END IF;

    IF NEW.project_id IS NULL THEN
        NEW.project_id := conversation_project_id;
    ELSIF NEW.project_id IS DISTINCT FROM conversation_project_id THEN
        RAISE EXCEPTION 'Message and Conversation must belong to the same Project'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION validate_conversation_read_state()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    next_sequence BIGINT;
    reader_enabled BOOLEAN;
    conversation_project UUID;
BEGIN
    SELECT conversation.next_entry_sequence, conversation.project_id
      INTO next_sequence, conversation_project
      FROM conversations conversation
     WHERE conversation.id = NEW.conversation_id
       AND conversation.project_id IS NOT DISTINCT FROM NEW.project_id
       FOR KEY SHARE OF conversation;

    IF conversation_project IS NULL THEN
        -- A DM, a group or a thread under one: its participants, active.
        SELECT actor.status = 'active'
               AND conversation_member_visible(NEW.conversation_id, NEW.actor_id)
          INTO reader_enabled
          FROM actors actor
         WHERE actor.id = NEW.actor_id
           FOR KEY SHARE OF actor;
    ELSE
        SELECT membership.enabled AND actor.status = 'active'
          INTO reader_enabled
          FROM project_memberships membership
          JOIN actors actor ON actor.id = membership.actor_id
         WHERE membership.project_id = conversation_project
           AND membership.actor_id = NEW.actor_id
           FOR KEY SHARE OF membership, actor;
    END IF;

    IF next_sequence IS NULL OR reader_enabled IS DISTINCT FROM true THEN
        RAISE EXCEPTION 'Read state requires an enabled same-Project membership'
            USING ERRCODE = '23514';
    END IF;

    IF NEW.highest_seen_sequence >= next_sequence THEN
        RAISE EXCEPTION 'Read state cannot advance beyond durable Conversation entries'
            USING ERRCODE = '23514';
    END IF;

    IF TG_OP = 'UPDATE' THEN
        IF NEW.highest_seen_sequence < OLD.highest_seen_sequence THEN
            IF current_setting('aw.read_rewind', true) IS DISTINCT FROM
               (NEW.conversation_id::text || ':' || NEW.actor_id::text)
               OR NEW.conversation_id IS DISTINCT FROM OLD.conversation_id
               OR NEW.actor_id IS DISTINCT FROM OLD.actor_id THEN
                RAISE EXCEPTION 'Conversation read state cannot move backwards'
                    USING ERRCODE = '23514';
            END IF;
            IF NEW.version <= OLD.version THEN
                RAISE EXCEPTION 'Rewinding read state must advance its version'
                    USING ERRCODE = '23514';
            END IF;
        END IF;
        IF NEW.highest_seen_sequence > OLD.highest_seen_sequence
           AND NEW.version <= OLD.version THEN
            RAISE EXCEPTION 'Advancing read state must advance its version'
                USING ERRCODE = '23514';
        END IF;
        IF NEW.highest_seen_sequence = OLD.highest_seen_sequence
           AND NEW.version IS DISTINCT FROM OLD.version THEN
            RAISE EXCEPTION 'Idempotent read state cannot churn its version'
                USING ERRCODE = '23514';
        END IF;
    END IF;

    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION validate_message_reaction()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    reactor_enabled BOOLEAN;
BEGIN
    IF TG_OP = 'UPDATE' THEN
        RAISE EXCEPTION 'Message reactions are added or removed, never changed'
            USING ERRCODE = '23514';
    END IF;

    IF NEW.project_id IS NULL THEN
        SELECT actor.status = 'active' INTO reactor_enabled
          FROM actors actor WHERE actor.id = NEW.actor_id FOR KEY SHARE;
    ELSE
        SELECT membership.enabled AND actor.status = 'active'
          INTO reactor_enabled
          FROM project_memberships membership
          JOIN actors actor ON actor.id = membership.actor_id
         WHERE membership.project_id = NEW.project_id
           AND membership.actor_id = NEW.actor_id
           FOR KEY SHARE OF membership, actor;
    END IF;

    IF reactor_enabled IS DISTINCT FROM true THEN
        RAISE EXCEPTION 'Reacting Actor must have an enabled Project membership'
            USING ERRCODE = '23514';
    END IF;

    IF NOT conversation_visible_to(NEW.conversation_id, NEW.actor_id) THEN
        RAISE EXCEPTION 'Reacting Actor must be able to see the Conversation'
            USING ERRCODE = '23514';
    END IF;

    IF EXISTS (
        SELECT 1 FROM messages
         WHERE id = NEW.message_id AND removed_at IS NOT NULL
    ) THEN
        RAISE EXCEPTION 'Removed Messages cannot get reactions'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION validate_message_mention()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    ast_contains_actor BOOLEAN;
    mentionable BOOLEAN;
BEGIN
    SELECT EXISTS (
               SELECT 1
                 FROM jsonb_array_elements(message.content->'nodes') AS node
                WHERE node->>'type' = 'mention'
                  AND COALESCE(node->>'actor_id', node->>'actorId') = NEW.actor_id::text
           )
      INTO ast_contains_actor
      FROM messages message
     WHERE message.id = NEW.message_id
       AND message.project_id IS NOT DISTINCT FROM NEW.project_id
       FOR KEY SHARE;

    IF ast_contains_actor IS DISTINCT FROM true THEN
        RAISE EXCEPTION 'Message mention projection must match its AST'
            USING ERRCODE = '23514';
    END IF;

    IF NEW.project_id IS NULL THEN
        SELECT actor.status = 'active' INTO mentionable
          FROM actors actor WHERE actor.id = NEW.actor_id FOR KEY SHARE;
    ELSE
        SELECT membership.enabled AND actor.status = 'active'
          INTO mentionable
          FROM project_memberships membership
          JOIN actors actor ON actor.id = membership.actor_id
         WHERE membership.project_id = NEW.project_id
           AND membership.actor_id = NEW.actor_id
         FOR KEY SHARE OF membership, actor;
    END IF;

    IF mentionable IS DISTINCT FROM true THEN
        RAISE EXCEPTION 'Mentioned Actor must have an enabled active Project membership'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION validate_work_item_reference_projection()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    node JSONB;
    expected_work_item_id UUID;
BEGIN
    IF TG_TABLE_NAME = 'message_work_item_references' THEN
        SELECT message.content->'nodes'->NEW.node_index
          INTO node
          FROM messages message
         WHERE message.id = NEW.message_id
           AND message.project_id IS NOT DISTINCT FROM NEW.project_id
         FOR KEY SHARE;
        expected_work_item_id := NEW.work_item_id;
    ELSE
        SELECT comment.content->'nodes'->NEW.node_index
          INTO node
          FROM work_item_comments comment
         WHERE comment.id = NEW.comment_id
           AND comment.project_id = NEW.project_id
           AND comment.work_item_id = NEW.source_work_item_id
         FOR KEY SHARE;
        expected_work_item_id := NEW.referenced_work_item_id;
    END IF;

    IF node IS NULL
       OR node->>'type' IS DISTINCT FROM 'work_item_ref'
       OR COALESCE(node->>'work_item_id', node->>'workItemId')
          IS DISTINCT FROM expected_work_item_id::text THEN
        RAISE EXCEPTION 'Work Item reference projection must match its AST node'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION validate_message_revision()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    current_revision INTEGER;
    current_content JSONB;
    current_plain_text TEXT;
    current_removed TIMESTAMPTZ;
BEGIN
    IF current_setting('aw.message_command', true) IS DISTINCT FROM NEW.message_id::text THEN
        RAISE EXCEPTION 'Message revisions are written only by the edit command'
            USING ERRCODE = '23514';
    END IF;
    SELECT revision, content, plain_text, removed_at
      INTO current_revision, current_content, current_plain_text, current_removed
      FROM messages
     WHERE id = NEW.message_id AND project_id IS NOT DISTINCT FROM NEW.project_id
       FOR KEY SHARE;
    IF current_revision IS NULL OR current_removed IS NOT NULL THEN
        RAISE EXCEPTION 'Removed or missing Messages take no revisions'
            USING ERRCODE = '23514';
    END IF;
    IF NEW.redacted_at IS NOT NULL THEN
        RAISE EXCEPTION 'New revisions cannot start redacted' USING ERRCODE = '23514';
    END IF;
    IF NOT ((NEW.revision = current_revision
             AND NEW.content = current_content
             AND NEW.plain_text = current_plain_text)
            OR NEW.revision = current_revision + 1) THEN
        RAISE EXCEPTION 'A revision is the current version or the next one'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION agent_sessions_match_workspace()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.project_id IS NULL THEN
        -- Bound to a DM or group: no space to match.
        RETURN NEW;
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM projects
        WHERE id = NEW.project_id AND organization_id = NEW.organization_id
    ) THEN
        RAISE EXCEPTION 'Agent session workspace must match its project' USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

-- An event with no space (a DM's) takes its workspace from its
-- conversation.
CREATE OR REPLACE FUNCTION stage_event(
    p_type TEXT, p_class TEXT, p_space UUID, p_thing_kind TEXT, p_thing UUID,
    p_audience TEXT, p_actor UUID, p_conversation UUID DEFAULT NULL,
    p_account UUID DEFAULT NULL, p_access_thing UUID DEFAULT NULL,
    p_version BIGINT DEFAULT NULL, p_changes TEXT[] DEFAULT '{}',
    p_payload JSONB DEFAULT '{}'
)
RETURNS VOID
LANGUAGE sql
AS $$
    INSERT INTO event_staging (
        id, workspace_id, space_id, thing_id, thing_kind, access_thing_id, thing_version,
        type, class, changes, audience, conversation_id, conversation_restricted,
        audience_account_id, actor_account_id, via_credential_id, requested_by_account_id,
        run_id, request_id, payload
    )
    SELECT gen_random_uuid(), COALESCE(project.organization_id, conversation.workspace_id),
           p_space, p_thing, p_thing_kind,
           p_access_thing, p_version, p_type, p_class, p_changes, p_audience,
           CASE WHEN p_audience = 'conversation'
                THEN COALESCE(conversation.parent_channel_id, conversation.id) END,
           CASE WHEN p_audience = 'conversation'
                THEN COALESCE(parent.visibility, conversation.visibility) <> 'project' END,
           CASE WHEN p_audience = 'account' THEN p_account END,
           COALESCE(NULLIF(current_setting('aw.actor', true), '')::uuid, p_actor),
           NULLIF(current_setting('aw.via_grant', true), '')::uuid,
           NULLIF(current_setting('aw.requested_by', true), '')::uuid,
           NULLIF(current_setting('aw.run', true), '')::uuid,
           NULLIF(current_setting('aw.request_id', true), ''),
           p_payload
    FROM (SELECT 1) one
    LEFT JOIN projects project ON project.id = p_space
    -- visibility: only the root and its restriction are read, to route the event
    LEFT JOIN conversations conversation ON conversation.id = p_conversation
    LEFT JOIN conversations parent ON parent.id = conversation.parent_channel_id
    WHERE COALESCE(project.organization_id, conversation.workspace_id) IS NOT NULL
      -- A conversation event needs its conversation (gone in a cascade).
      AND (p_audience <> 'conversation' OR conversation.id IS NOT NULL)
$$;

-- References from a DM message carry no space and the DM's workspace.
CREATE OR REPLACE FUNCTION thing_references_replace(
    p_project_id UUID, p_source_kind TEXT, p_source_id UUID, p_thing_kind TEXT,
    p_thing_id UUID, p_body TEXT
)
RETURNS VOID
LANGUAGE plpgsql
AS $$
DECLARE
    source_workspace UUID;
BEGIN
    DELETE FROM thing_references
    WHERE source_kind = p_source_kind AND source_id = p_source_id;
    IF p_body IS NULL OR p_body = '' THEN
        RETURN;
    END IF;
    IF p_project_id IS NULL AND p_thing_kind = 'conversation' THEN
        SELECT workspace_id INTO source_workspace FROM conversations WHERE id = p_thing_id;
    END IF;
    INSERT INTO thing_references
        (project_id, workspace_id, source_kind, source_id, thing_kind, thing_id,
         target_kind, target_id)
    SELECT p_project_id, source_workspace, p_source_kind, p_source_id, p_thing_kind, p_thing_id,
           found.target_kind, found.target_id
    FROM thing_reference_targets(p_body) found
    -- A thing never lists itself (a page linking to its own heading, a
    -- message in a thread linking to that thread).
    WHERE found.target_id <> p_thing_id
    ON CONFLICT DO NOTHING;
END;
$$;

CREATE OR REPLACE VIEW thing_source_conversations AS
 SELECT conversation.id,
        CASE conversation.kind
            WHEN 'thread'::text THEN 'thread'::text
            WHEN 'channel'::text THEN 'channel'::text
            ELSE 'conversation'::text
        END AS kind,
    conversation.workspace_id,
    conversation.project_id AS space_id,
    thing_if_registered(conversation.parent_channel_id) AS parent_id,
    NULL::uuid AS access_parent_id,
    "left"(conversation.name, 300) AS title,
        CASE
            WHEN (conversation.kind = 'thread'::text) THEN thing_status_key(conversation.thread_status)
            ELSE NULL::text
        END AS status,
    NULL::uuid AS owner_account_id,
    NULL::uuid AS created_by_account_id,
    NULL::uuid AS updated_by_account_id,
    conversation.version,
    conversation.created_at,
    conversation.updated_at,
    conversation.archived_at,
    NULL::timestamp with time zone AS deleted_at
   FROM conversations conversation;

CREATE OR REPLACE VIEW thing_source_messages AS
 SELECT message.id,
    'message'::text AS kind,
    message.workspace_id,
    message.project_id AS space_id,
    thing_if_registered(message.conversation_id) AS parent_id,
    thing_if_registered(message.conversation_id) AS access_parent_id,
    NULL::text AS title,
    NULL::text AS status,
    message.author_actor_id AS owner_account_id,
    message.author_actor_id AS created_by_account_id,
    message.removed_by_actor_id AS updated_by_account_id,
    (message.revision)::bigint AS version,
    message.created_at,
    COALESCE(message.removed_at, message.edited_at, message.created_at) AS updated_at,
    NULL::timestamp with time zone AS archived_at,
    message.removed_at AS deleted_at
   FROM messages message;

-- Row rules: a direct-scope row is its participants' (H1.21's stopgap,
-- K4's to fill). Pre-R20 personal DMs carry a team workspace's id and pass
-- by that branch as before.
CREATE OR REPLACE FUNCTION tenancy_direct_allows(workspace UUID, conversation UUID)
RETURNS BOOLEAN
LANGUAGE sql STABLE PARALLEL SAFE AS $$
    SELECT workspace = '00000000-0000-7000-8000-00000000d1ec'::UUID
       AND conversation IS NOT NULL
       AND EXISTS (
           SELECT 1
           FROM conversations c
           JOIN conversation_members m
             ON m.conversation_id = COALESCE(c.parent_channel_id, c.id)
           WHERE c.id = conversation
             AND m.actor_id = tenancy_account())
$$;

-- 4. What a role allows in a DM (K4.12) ---------------------------------------

-- Whether `account` may do what `permission` covers in the DM or group
-- `target` (or a thread under one): some workspace it shares with every
-- other active participant gives it, through the account's role there. A
-- kept DM is its workspace's: only that workspace's role counts. Agents and
-- people alike; an archived or left participant doesn't count, so a DM
-- whose other side left every shared workspace refuses sending but stays
-- readable (visibility is participation alone).
CREATE FUNCTION direct_allows(target UUID, account UUID, permission TEXT)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT EXISTS (
        SELECT 1
        FROM conversations c
        LEFT JOIN conversations parent ON parent.id = c.parent_channel_id
        JOIN actors me ON me.id = account AND me.status = 'active'
        JOIN organization_memberships mine
          ON mine.actor_id = account AND mine.enabled
        JOIN organizations workspace
          ON workspace.id = mine.organization_id AND workspace.lifecycle = 'active'
        WHERE c.id = target
          AND COALESCE(parent.kind, c.kind) IN ('dm', 'group')
          AND (NOT COALESCE(parent.kept, c.kept) OR mine.organization_id = c.workspace_id)
          AND role_allows(mine.role, mine.role_id, permission)
          AND NOT EXISTS (
              SELECT 1
              FROM conversation_members other
              JOIN actors other_actor ON other_actor.id = other.actor_id
                                     AND other_actor.status = 'active'
              WHERE other.conversation_id = COALESCE(c.parent_channel_id, c.id)
                AND other.actor_id <> account
                AND NOT EXISTS (
                    SELECT 1 FROM organization_memberships theirs
                    WHERE theirs.organization_id = mine.organization_id
                      AND theirs.actor_id = other.actor_id
                      AND theirs.enabled))
    )
$$;

-- The permission keys `account` holds in the DM or group `target`, for the
-- server's one row read (`ConversationAccessRow`).
CREATE FUNCTION direct_permissions(target UUID, account UUID)
RETURNS TEXT[]
LANGUAGE sql
STABLE
AS $$
    SELECT COALESCE(array_agg(catalogue.key ORDER BY catalogue.key), '{}')
    FROM permission_catalogue catalogue
    WHERE direct_allows(target, account, catalogue.key)
$$;

-- Whether `account` may start a DM or group with `other` (`direct.start`
-- in a workspace both are enabled members of).
CREATE FUNCTION direct_may_start(account UUID, other UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT EXISTS (
        SELECT 1
        FROM organization_memberships mine
        JOIN organization_memberships theirs
          ON theirs.organization_id = mine.organization_id
         AND theirs.actor_id = other AND theirs.enabled
        JOIN organizations workspace
          ON workspace.id = mine.organization_id AND workspace.lifecycle = 'active'
        JOIN actors me ON me.id = account AND me.status = 'active'
        JOIN actors them ON them.id = other AND them.status = 'active'
        WHERE mine.actor_id = account AND mine.enabled
          AND role_allows(mine.role, mine.role_id, 'direct.start')
    )
$$;

-- The name and handle `account` goes by where there's no space to read
-- them from (a DM or group): its profile in `preferred_workspace` when it
-- has one, else its earliest; a person's own name before any workspace.
CREATE FUNCTION account_profile(account UUID, preferred_workspace UUID)
RETURNS TABLE (display_name TEXT, handle TEXT)
LANGUAGE sql
STABLE
AS $$
    SELECT named.display_name, named.handle
    FROM (
        SELECT profile.display_name, profile.handle,
               0 AS rank, (profile.organization_id = preferred_workspace) AS preferred,
               profile.created_at
        FROM organization_actor_profiles profile
        WHERE profile.actor_id = account
        UNION ALL
        SELECT person.display_name, person.login_name, 1, false, person.created_at
        FROM users person
        WHERE person.actor_id = account
        UNION ALL
        SELECT 'Former member', 'former-member', 2, false, now()
    ) named
    ORDER BY named.rank, named.preferred DESC NULLS LAST, named.created_at
    LIMIT 1
$$;

-- Whether an agent's connection may act in DMs and groups at all (R19):
-- it and its agent are active, and the agent is an enabled member of some
-- active workspace. DMs belong to the account, not to the workspace the
-- connection was made in. What it may do in one comes from `direct_allows`.
CREATE FUNCTION agent_connection_can_reach_direct(connection_uuid UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT EXISTS (
        SELECT 1
        FROM external_agent_connections c
        JOIN actors a ON a.id = c.actor_id AND a.status = 'active'
        JOIN organization_memberships om
          ON om.actor_id = a.id AND om.enabled
        JOIN organizations workspace
          ON workspace.id = om.organization_id AND workspace.lifecycle = 'active'
        WHERE c.id = connection_uuid AND c.status = 'active'
    )
$$;

-- `agent_connection_can_access` for something that may have no space (a
-- DM or group, or a request in one).
CREATE FUNCTION agent_connection_can_reach(connection_uuid UUID, target_project UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT CASE WHEN target_project IS NULL
                THEN agent_connection_can_reach_direct(connection_uuid)
                ELSE agent_connection_can_access(connection_uuid, target_project) END
$$;

-- The name and handle `account` goes by in `space`: its membership there,
-- or (no space, or not a member) its workspace profile.
CREATE FUNCTION place_profile(space UUID, account UUID)
RETURNS TABLE (display_name TEXT, handle TEXT)
LANGUAGE sql
STABLE
AS $$
    SELECT named.display_name, named.handle
    FROM (
        SELECT membership.display_name, membership.handle, 0 AS rank
        FROM project_memberships membership
        WHERE membership.project_id = space AND membership.actor_id = account
        UNION ALL
        SELECT profile.display_name, profile.handle, 1
        FROM account_profile(account, NULL) profile
    ) named
    ORDER BY named.rank
    LIMIT 1
$$;

-- The Messages space no longer follows the workspace role: there is none.
DROP TRIGGER IF EXISTS organization_memberships_direct_space_role ON organization_memberships;
DROP FUNCTION IF EXISTS direct_space_follows_workspace_role();

-- 5. The one-time move (K4.16, K4.17) -----------------------------------------

DO $move$
DECLARE
    content_tables TEXT[] := ARRAY[
        'conversations', 'conversation_members', 'conversation_read_states',
        'conversation_notification_levels', 'messages', 'message_link_previews',
        'message_mentions', 'message_reactions', 'message_revisions',
        'message_work_item_references', 'reply_requests', 'files',
        'scheduled_messages', 'saved_items', 'notifications', 'thing_references',
        'agent_sessions', 'launches', 'external_execution_requests'];
    scaffolding_tables TEXT[] := ARRAY[
        'project_counters', 'project_instructions', 'project_navigation_states',
        'project_task_settings', 'settings', 'secrets', 'notification_project_mutes',
        'project_layout_rules', 'onboarding_progress'];
    target TEXT;
    reference RECORD;
    paired RECORD;
BEGIN
    IF NOT EXISTS (SELECT 1 FROM projects WHERE kind = 'direct') THEN
        RETURN;
    END IF;

    CREATE TEMP TABLE direct_moves ON COMMIT DROP AS
    SELECT direct.id AS direct_id, direct.organization_id,
           workspace_space_make(direct.organization_id) AS space_id
    FROM projects direct
    WHERE direct.kind = 'direct';

    -- Everyone who was in Messages is in the workspace's own space, so what
    -- moves there keeps its members.
    INSERT INTO project_memberships (
        id, project_id, actor_id, role, role_id, display_name, handle, enabled,
        disabled_at, disabled_by_actor_id
    )
    SELECT gen_random_uuid(), moves.space_id, member.actor_id,
           CASE WHEN member.role IN ('owner', 'admin', 'member', 'viewer', 'worker')
                THEN member.role ELSE 'member' END,
           member.role_id, member.display_name,
           CASE WHEN EXISTS (
                    SELECT 1 FROM project_memberships taken
                    WHERE taken.project_id = moves.space_id AND taken.enabled
                      AND lower(taken.handle) = lower(member.handle))
                THEN left(member.handle, 25) || '-' || right(replace(member.actor_id::text, '-', ''), 6)
                ELSE member.handle END,
           false, COALESCE(member.disabled_at, now()), member.disabled_by_actor_id
    FROM project_memberships member
    JOIN direct_moves moves ON moves.direct_id = member.project_id
    ON CONFLICT (project_id, actor_id) DO NOTHING;
    -- Added disabled; the enabled workspace members come back through the
    -- workspace's own rule, as on any join.
    PERFORM workspace_space_add_member(moves.space_id, member.actor_id)
    FROM project_memberships member
    JOIN direct_moves moves ON moves.direct_id = member.project_id
    WHERE member.enabled;

    -- Keys that pair a space with a conversation or message are set aside
    -- while both sides move, then checked again whole.
    CREATE TEMP TABLE direct_paired_keys ON COMMIT DROP AS
    SELECT constraint_row.conrelid::regclass::text AS table_name,
           constraint_row.conname AS name,
           pg_get_constraintdef(constraint_row.oid) AS definition
    FROM pg_constraint constraint_row
    WHERE constraint_row.contype = 'f'
      AND array_length(constraint_row.conkey, 1) > 1
      AND constraint_row.confrelid IN ('conversations'::regclass, 'messages'::regclass);
    FOR paired IN SELECT * FROM direct_paired_keys LOOP
        EXECUTE format('ALTER TABLE %I DROP CONSTRAINT %I', paired.table_name, paired.name);
    END LOOP;

    -- DM content loses its space, with the tables' triggers off: this is a
    -- move, not a change anyone made. The scaffolding goes first, since
    -- some of it names a DM.
    FOREACH target IN ARRAY scaffolding_tables LOOP
        EXECUTE format(
            'DELETE FROM %I WHERE project_id IN (SELECT direct_id FROM direct_moves)', target);
    END LOOP;
    FOREACH target IN ARRAY content_tables LOOP
        EXECUTE format('ALTER TABLE %I DISABLE TRIGGER USER', target);
        EXECUTE format(
            'UPDATE %I SET project_id = NULL WHERE project_id IN (SELECT direct_id FROM direct_moves)',
            target);
        EXECUTE format('ALTER TABLE %I ENABLE TRIGGER USER', target);
    END LOOP;
    UPDATE message_drafts SET space_id = NULL
    WHERE space_id IN (SELECT direct_id FROM direct_moves);
    UPDATE things SET space_id = NULL
    WHERE space_id IN (SELECT direct_id FROM direct_moves);
    UPDATE requests SET space_id = NULL
    WHERE space_id IN (SELECT direct_id FROM direct_moves);
    UPDATE runs SET space_id = NULL
    WHERE space_id IN (SELECT direct_id FROM direct_moves);
    UPDATE run_usage SET space_id = NULL
    WHERE space_id IN (SELECT direct_id FROM direct_moves);
    UPDATE audit_events SET project_id = NULL
    WHERE project_id IN (SELECT direct_id FROM direct_moves);
    UPDATE ai_usage SET project_id = NULL
    WHERE project_id IN (SELECT direct_id FROM direct_moves);
    UPDATE background_jobs SET project_id = NULL
    WHERE project_id IN (SELECT direct_id FROM direct_moves);
    UPDATE webhook_deliveries SET project_id = NULL
    WHERE project_id IN (SELECT direct_id FROM direct_moves);

    -- Whatever else the hidden projects held belonged to the whole
    -- workspace (an agent connected only for DMs, its secrets and sign-ins,
    -- webhooks, invitations): it moves to the workspace's own space.
    FOR reference IN
        SELECT constraint_row.conrelid::regclass::text AS table_name,
               attribute.attname AS column_name
        FROM pg_constraint constraint_row
        JOIN pg_attribute attribute
          ON attribute.attrelid = constraint_row.conrelid
         AND attribute.attnum = constraint_row.conkey[array_length(constraint_row.conkey, 1)]
        WHERE constraint_row.contype = 'f'
          AND constraint_row.confrelid = 'projects'::regclass
          AND constraint_row.conrelid <> 'projects'::regclass
          AND constraint_row.conrelid::regclass::text <> 'project_memberships'
          AND NOT (constraint_row.conrelid::regclass::text = ANY (content_tables))
          AND NOT (constraint_row.conrelid::regclass::text = ANY (scaffolding_tables))
          AND constraint_row.conrelid::regclass::text NOT IN (
              'message_drafts', 'things', 'requests', 'runs', 'run_usage', 'audit_events',
              'ai_usage', 'background_jobs', 'webhook_deliveries')
    LOOP
        BEGIN
            EXECUTE format(
                'UPDATE %I SET %I = moves.space_id FROM direct_moves moves WHERE %I.%I = moves.direct_id',
                reference.table_name, reference.column_name,
                reference.table_name, reference.column_name);
        EXCEPTION WHEN unique_violation THEN
            -- A one-per-space row (settings and the like): the workspace
            -- space already has its own, which stands.
            EXECUTE format(
                'DELETE FROM %I WHERE %I IN (SELECT direct_id FROM direct_moves)',
                reference.table_name, reference.column_name);
        END;
    END LOOP;

    FOR paired IN SELECT * FROM direct_paired_keys LOOP
        EXECUTE format('ALTER TABLE %I ADD CONSTRAINT %I %s',
                       paired.table_name, paired.name, paired.definition);
    END LOOP;

    DELETE FROM project_memberships
    WHERE project_id IN (SELECT direct_id FROM direct_moves);
    -- Stops here, naming the table, if anything still points at one.
    DELETE FROM projects WHERE id IN (SELECT direct_id FROM direct_moves);
END
$move$;

DROP INDEX IF EXISTS projects_one_direct_space_per_organization;
ALTER TABLE projects DROP CONSTRAINT projects_kind_check;
ALTER TABLE projects ADD CONSTRAINT projects_kind_check
    CHECK (kind IN ('standard', 'workspace', 'team', 'personal'));

-- A DM or group has no space; anything else has one. A thread follows its
-- parent (the parent trigger).
ALTER TABLE conversations ADD CONSTRAINT conversations_direct_have_no_space
    CHECK ((kind IN ('dm', 'group')) = (project_id IS NULL) OR kind = 'thread');
