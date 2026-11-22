-- C2.37: a guest (the workspace's guest-tier role, K1.27) reads only the
-- channels they were added to. Everyone else in a space still reads its
-- open channels without joining. Lists, browsing, search, mentions and
-- notifications all ask conversation_visible_to, so they follow.
--
-- The body is 20261117001500's, with one change: an open channel (and a
-- thread under one) needs channel membership when the viewer is a guest of
-- the channel's workspace.

CREATE OR REPLACE FUNCTION conversation_member_visible(target UUID, viewer UUID)
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
                  AND ((COALESCE(parent.visibility, c.visibility) = 'project'
                        AND NOT EXISTS (
                            SELECT 1 FROM organization_memberships g
                            WHERE g.organization_id = c.workspace_id
                              AND g.actor_id = viewer
                              AND role_is_guest(g.role_id)))
                       OR EXISTS (
                           SELECT 1 FROM conversation_members m
                           WHERE m.conversation_id = COALESCE(c.parent_channel_id, c.id)
                             AND m.actor_id = viewer))
          END
    )
$$;
