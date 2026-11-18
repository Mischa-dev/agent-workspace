-- C2.19 follow-up: a custom emoji reaction is keyed by the emoji's id, not
-- only its :name:. A retired name can be reused for a new image; someone can
-- still join the old one's reaction, and both can sit on one message. A
-- standard emoji has no id, and NULLS NOT DISTINCT keeps it one per person.
ALTER TABLE message_reactions DROP CONSTRAINT message_reactions_pkey;
ALTER TABLE message_reactions ADD CONSTRAINT message_reactions_one
    UNIQUE NULLS NOT DISTINCT (message_id, actor_id, emoji, custom_emoji_id);
