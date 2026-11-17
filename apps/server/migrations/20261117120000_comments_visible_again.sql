-- C1.3 (comments on things), after K4.4: conversation_visible_to keeps its
-- 'subject' arm.
--
-- Migration 20261115060000 gave the composer a 'subject' arm (a comments
-- conversation, and a thread under one, is visible exactly when its thing
-- is: subject_visible_to). K4.4 (20261117001500) then made the composer
-- K4's whole member rule alone, as its comment says, for C1.3 to add its arm
-- to. This puts the arm back. Everything that isn't about a thing is decided
-- by conversation_member_visible unchanged; a comments conversation is
-- decided by its thing alone, never by a member list.

CREATE OR REPLACE FUNCTION conversation_visible_to(target UUID, viewer UUID)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
AS $$
    SELECT COALESCE(
        (SELECT subject_visible_to(
                    COALESCE(parent.subject_kind, conversation.subject_kind),
                    COALESCE(parent.subject_id, conversation.subject_id),
                    viewer)
         FROM conversations conversation
         LEFT JOIN conversations parent ON parent.id = conversation.parent_channel_id
         WHERE conversation.id = target
           AND COALESCE(parent.visibility, conversation.visibility) = 'subject'),
        conversation_member_visible(target, viewer))
$$;
