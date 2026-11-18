-- C2.7: joining and leaving channels, browsing them, and a topic.
--
-- A row in `conversation_members` now means "in this conversation" for a
-- public channel too: it's in your sidebar. Visibility doesn't change:
-- anyone in the space can still find, read and join a public channel.
-- `auto_join` channels take in everyone who arrives in the space.
ALTER TABLE conversations
    ADD COLUMN topic TEXT
        CHECK (topic IS NULL OR (char_length(topic) <= 250 AND topic !~ '[[:cntrl:]]')),
    ADD COLUMN description TEXT
        CHECK (description IS NULL OR char_length(description) <= 2000),
    ADD COLUMN auto_join BOOLEAN NOT NULL DEFAULT false;

-- Today everyone in a space is in every public channel: keep that. Each
-- existing public channel takes in newcomers, and everyone already in its
-- space joins it.
UPDATE conversations
SET auto_join = true
WHERE kind = 'channel' AND visibility = 'project';

INSERT INTO conversation_members (project_id, conversation_id, actor_id, added_by_actor_id)
SELECT channel.project_id, channel.id, membership.actor_id, membership.actor_id
FROM conversations channel
JOIN project_memberships membership
  ON membership.project_id = channel.project_id AND membership.enabled
WHERE channel.kind = 'channel' AND channel.visibility = 'project'
ON CONFLICT DO NOTHING;
