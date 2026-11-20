-- A2.15: an app acting as itself hears a DM, a group or a private channel
-- only with a participant's consent.
--
-- * actors.reads_with_consent: whether an account hears private
--   conversations only where a participant said it may. It is how an
--   account is told about things, not what it may do: it changes nothing
--   the account may read or write through a call, only which events reach
--   it. Apps start with it on.
-- * conversation_app_consents: a participant's consent, per conversation
--   (the DM, group or channel a thread belongs to) and account. Withdrawing
--   it keeps the row, with who withdrew it and when.
-- * app_may_read(recipient, conversation): the rule, which
--   delivery_visible now applies to every event about a conversation.

ALTER TABLE actors ADD COLUMN reads_with_consent BOOLEAN NOT NULL DEFAULT false;

UPDATE actors SET reads_with_consent = true WHERE account_kind = 'app';

-- Runs after actors_account_kind (triggers fire by name), so a new app
-- account starts with it on.
CREATE FUNCTION actors_reads_with_consent()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.account_kind = 'app' THEN
        NEW.reads_with_consent := true;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER actors_reads_with_consent
BEFORE INSERT ON actors
FOR EACH ROW EXECUTE FUNCTION actors_reads_with_consent();

CREATE TABLE conversation_app_consents (
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    space_id UUID REFERENCES projects(id) ON DELETE CASCADE,
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    app_account_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    granted_by_account_id UUID NOT NULL REFERENCES actors(id),
    granted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_by_account_id UUID REFERENCES actors(id),
    revoked_at TIMESTAMPTZ,
    PRIMARY KEY (conversation_id, app_account_id),
    CHECK ((revoked_at IS NULL) = (revoked_by_account_id IS NULL))
);

CREATE INDEX conversation_app_consents_app_idx
    ON conversation_app_consents (app_account_id)
    WHERE revoked_at IS NULL;

SELECT tenancy_protect('conversation_app_consents', 'workspace_id', 'conversation_id');

-- The conversation whose participants decide (a thread's DM, group or
-- channel), and whether it needs consent at all: a DM or group, or a
-- channel or thread only its members see.
CREATE FUNCTION app_consent_root(target UUID)
RETURNS TABLE (root_id UUID, needs_consent BOOLEAN)
LANGUAGE sql
STABLE
AS $$
    SELECT COALESCE(parent.id, conversation.id),
           COALESCE(parent.kind, conversation.kind) IN ('dm', 'group')
           OR COALESCE(parent.visibility, conversation.visibility) = 'members'
           OR conversation.visibility = 'members'
    FROM conversations conversation
    LEFT JOIN conversations parent
        ON parent.id = conversation.parent_channel_id AND conversation.kind = 'thread'
    WHERE conversation.id = target
$$;

CREATE FUNCTION app_may_read(recipient UUID, target UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT target IS NULL
        OR NOT EXISTS (
               SELECT 1 FROM actors account
               WHERE account.id = recipient AND account.reads_with_consent)
        OR NOT COALESCE((SELECT root.needs_consent FROM app_consent_root(target) root), false)
        OR EXISTS (
               SELECT 1 FROM app_consent_root(target) root
               JOIN conversation_app_consents consent
                   ON consent.conversation_id = root.root_id
               WHERE consent.app_account_id = recipient AND consent.revoked_at IS NULL)
$$;

-- delivery_visible as 20261118093317 left it, and app_may_read for every
-- event about a conversation: one routed to the conversation, or one
-- addressed to an account (a mention, a reply request) that names it.
CREATE OR REPLACE FUNCTION delivery_visible(recipient UUID, ev events)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT EXISTS (SELECT 1 FROM actors a WHERE a.id = recipient AND a.status = 'active')
       AND (ev.audience_account_id IS NULL OR ev.audience_account_id = recipient)
       AND app_may_read(recipient, COALESCE(
               ev.conversation_id,
               CASE WHEN ev.payload->>'conversationId' ~ '^[0-9a-fA-F-]{36}$'
                    THEN (ev.payload->>'conversationId')::uuid END))
       AND CASE ev.audience
           -- The conversation rule alone: a DM stays its participants'
           -- (R20), a channel needs a reader of its space.
           WHEN 'conversation' THEN
               conversation_visible_to(COALESCE(ev.access_thing_id, ev.conversation_id), recipient)
           WHEN 'thing' THEN delivery_thing_visible(COALESCE(ev.access_thing_id, ev.thing_id), recipient)
           ELSE (
               CASE WHEN ev.workspace_id = direct_scope_id() THEN
                   (ev.conversation_id IS NOT NULL OR ev.audience_account_id IS NOT NULL)
                   AND (ev.conversation_id IS NULL
                        OR conversation_visible_to(ev.conversation_id, recipient))
               ELSE EXISTS (
                   SELECT 1 FROM organization_memberships m
                   WHERE m.organization_id = ev.workspace_id AND m.actor_id = recipient
                     AND m.enabled)
                   AND (ev.space_id IS NULL OR space_readable_by(ev.space_id, recipient))
               END
           )
               -- The thing it is about too (a task needs tasks.view), when
               -- no other thing is named to decide on.
               AND (COALESCE(ev.access_thing_id, ev.thing_id) IS NULL
                    OR delivery_thing_visible(COALESCE(ev.access_thing_id, ev.thing_id), recipient))
       END
$$;
