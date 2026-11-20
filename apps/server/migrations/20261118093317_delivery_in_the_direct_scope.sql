-- DMs and groups live in the direct scope (K4.17), which has no
-- organization memberships. An event there reaches whoever the
-- conversation rule lets read it, or anyone it names, instead of members
-- of a workspace.
CREATE OR REPLACE FUNCTION delivery_visible(recipient UUID, ev events)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT EXISTS (SELECT 1 FROM actors a WHERE a.id = recipient AND a.status = 'active')
       AND (ev.audience_account_id IS NULL OR ev.audience_account_id = recipient)
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
