-- K4.4: conversations carry their workspace, and one full visibility rule.
--
-- * conversations.workspace_id (added with row security): an index
--   (workspace_id, kind). Writers set it explicitly.
-- * conversations.kept: a DM or group that belongs to its workspace and
--   ends with membership there (ruling R20; the `direct_messages` setting
--   that starts them comes later). Every existing one is personal.
-- * The split of ruling R3 (K1 section 3.10): conversation_visible_to
--   becomes K1's composer (C1.3 adds its 'subject' arm) over
--   conversation_member_visible, which K4 owns, and which is whole (K4
--   section 3.7): DMs, groups and their threads are for participants only
--   (no role passes), a kept one also ends with workspace membership;
--   channels and their threads need a reader of the space, and private ones
--   membership too. An inactive account reads nothing.
--
-- For channels this only narrows: every caller already paired the rule
-- with a membership check. For personal DMs it widens one thing on purpose
-- (R20): the rule alone keeps a DM readable after its participants stop
-- sharing a workspace (the routes still check membership until K4.6).
--
-- Timings (median EXPLAIN ANALYZE of seven, `conversation_visibility_timings`,
-- an ignored test in tests/integration/direct_conversations.rs: 50,000
-- messages over 200 channels and 200 DMs, one viewer, warm cache, a shared
-- four-core build machine under load, so read the ratios):
--   conversation list (every conversation of two spaces, by the rule):
--     before 19.4 ms, after 31.8 ms
--   message search (a word in every message, the rule on each match):
--     before 2,372 ms, after 4,171 ms
-- The new body checks the space per conversation, which the old one left to
-- the caller's membership join. Search pays it per matching message; asking
-- the rule once per conversation instead is left to the search owner. A
-- first version that called space_readable_by from the body cost 44 s.

-- conversations.workspace_id itself came with row security (migrations
-- 20261116183100 and 20261116183200): filled on insert, NOT NULL.

ALTER TABLE conversations
    ADD COLUMN kept BOOLEAN NOT NULL DEFAULT false,
    ADD CONSTRAINT conversations_kept_is_direct
        CHECK (NOT kept OR kind IN ('dm', 'group'));

CREATE INDEX conversations_workspace_kind_idx ON conversations (workspace_id, kind);

-- K4's helper (K4 section 3.7). It branches on the kind, not on whether
-- project_id is NULL, so it holds before and after K4.17. The space check is
-- space_readable_by written out in place: a SQL function holding EXISTS is
-- never inlined, and calling one per row from inside another cost ten times
-- the old rule in message search. Keep the two in step. It differs in one
-- place, on purpose: a run's own worker account, which has a space
-- membership and no workspace membership, still reads its space's channels.
CREATE FUNCTION conversation_member_visible(target UUID, viewer UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT EXISTS (
        SELECT 1
        FROM conversations c
        LEFT JOIN conversations parent ON parent.id = c.parent_channel_id
        JOIN actors a ON a.id = viewer AND a.status = 'active'
        WHERE c.id = target
          AND CASE
              -- DMs, groups and their threads: participants only (ruling
              -- R20). No role, admin, owner or server role passes. A kept
              -- one also ends with workspace membership.
              WHEN COALESCE(parent.kind, c.kind) IN ('dm', 'group') THEN EXISTS (
                  SELECT 1 FROM conversation_members m
                  WHERE m.conversation_id = COALESCE(c.parent_channel_id, c.id)
                    AND m.actor_id = viewer)
                  AND (NOT COALESCE(parent.kept, c.kept) OR EXISTS (
                      SELECT 1 FROM organization_memberships w
                      WHERE w.organization_id = c.workspace_id
                        AND w.actor_id = viewer AND w.enabled))
              -- Channels and their threads: readers of the space (its
              -- members, everyone in the workspace for the workspace's own
              -- space and open spaces, never an archived one); private
              -- channels, their members only. An account whose workspace
              -- membership is disabled reads none; one that never had a
              -- workspace membership (a run's own worker account, until K1.8
              -- makes those accounts) reads by its space membership alone.
              ELSE EXISTS (
                  SELECT 1 FROM projects s
                  WHERE s.id = c.project_id AND s.archived_at IS NULL
                    AND (
                        (EXISTS (
                             SELECT 1 FROM project_memberships p
                             WHERE p.project_id = s.id AND p.actor_id = viewer
                               AND p.enabled)
                         AND NOT EXISTS (
                             SELECT 1 FROM organization_memberships w
                             WHERE w.organization_id = s.organization_id
                               AND w.actor_id = viewer AND NOT w.enabled))
                        OR ((s.kind = 'workspace'
                             OR (s.visibility = 'workspace' AND s.kind IN ('team', 'standard')))
                            AND EXISTS (
                                SELECT 1 FROM organization_memberships w
                                WHERE w.organization_id = s.organization_id
                                  AND w.actor_id = viewer AND w.enabled))))
                  AND (COALESCE(parent.visibility, c.visibility) = 'project'
                       OR EXISTS (
                           SELECT 1 FROM conversation_members m
                           WHERE m.conversation_id = COALESCE(c.parent_channel_id, c.id)
                             AND m.actor_id = viewer))
          END
    )
$$;

-- K1's composer (K1 section 3.10): one arm per helper, OR-ed. C1.3 adds its
-- 'subject' arm. It holds no sub-select, so the planner inlines it.
CREATE OR REPLACE FUNCTION conversation_visible_to(target UUID, viewer UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT conversation_member_visible(target, viewer)
$$;
