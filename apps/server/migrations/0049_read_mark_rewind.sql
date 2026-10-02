-- Marking a conversation unread (docs/plans/mcp-tools.md, Phase 3; mark_read
-- with unread: true, and the web's "Mark unread from here").
-- Phase 2 owns 0044, 0045 and 0046; Phase 3 uses 0047, 0048 and 0049 (this).
--
-- Marking unread moves a read mark backwards, which 0005 refused outright.
-- It still refuses, unless the one command that rewinds
-- (conversations::mark_unread_from) sets aw.read_rewind to exactly
-- '<conversation id>:<actor id>' of the row it changes, for its own
-- transaction (set_config(..., true)), and the rewind advances the version.
-- Every other path (mark-seen, mark_read, a stray UPDATE) still can't move a
-- read mark backwards. The trigger is replaced, not dropped.
CREATE OR REPLACE FUNCTION validate_conversation_read_state()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    next_sequence BIGINT;
    reader_enabled BOOLEAN;
BEGIN
    SELECT conversation.next_entry_sequence,
           membership.enabled AND actor.status = 'active'
      INTO next_sequence, reader_enabled
      FROM conversations conversation
      JOIN project_memberships membership
        ON membership.project_id = conversation.project_id
       AND membership.actor_id = NEW.actor_id
      JOIN actors actor ON actor.id = membership.actor_id
     WHERE conversation.project_id = NEW.project_id
       AND conversation.id = NEW.conversation_id
       FOR KEY SHARE OF conversation, membership, actor;

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
